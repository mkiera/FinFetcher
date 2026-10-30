use serde_json::Value;
use std::{
    collections::HashMap,
    fs, io,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};

const STAGING_PREFIX: &str = ".finfetcher-stage-";
const KEPT_MARKER: &str = ".finfetcher-kept";

#[derive(Clone, Debug, PartialEq)]
struct Stamp {
    size: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

fn stamp(path: &Path) -> Result<Option<Stamp>, String> {
    match fs::symlink_metadata(path) {
        Ok(info) if info.is_file() && !info.file_type().is_symlink() => Ok(Some(Stamp {
            size: info.len(),
            modified: info.modified().ok(),
            created: info.created().ok(),
        })),
        Ok(_) => Err(format!(
            "The destination is not a regular file: {}",
            path.display()
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

struct Staged {
    source: PathBuf,
    target: PathBuf,
    previous: Option<Stamp>,
    saved: PathBuf,
    moved: bool,
}

pub struct DownloadOutput {
    directory: Option<tempfile::TempDir>,
    lock: Option<fs::File>,
    pub destination: PathBuf,
    approved: HashMap<PathBuf, Stamp>,
}

impl DownloadOutput {
    pub fn new(destination: &Path) -> Result<Self, String> {
        fs::create_dir_all(destination)
            .map_err(|e| format!("Could not create the destination: {e}"))?;
        let destination = destination.canonicalize().map_err(|e| e.to_string())?;
        remove_leftovers(&destination);
        let directory = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(&destination)
            .map_err(|e| e.to_string())?;
        let lock = hold(directory.path()).map_err(|e| e.to_string())?;
        Ok(Self {
            directory: Some(directory),
            lock: Some(lock),
            destination,
            approved: HashMap::new(),
        })
    }

    pub fn path(&self) -> &Path {
        self.directory
            .as_ref()
            .expect("Active output directory")
            .path()
    }

    pub fn prepare(
        &mut self,
        names: &[PathBuf],
        cancel: &AtomicBool,
        mut ask: impl FnMut(Vec<PathBuf>) -> Result<Value, String>,
    ) -> Result<(), String> {
        for name in names {
            if name.is_absolute()
                || name
                    .components()
                    .any(|p| !matches!(p, Component::Normal(_)))
            {
                return Err("The downloader returned a filename outside the destination.".into());
            }
        }
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err("Cancelled".into());
            }
            let mut conflicts = Vec::new();
            let mut stamps = Vec::new();
            for name in names {
                let target = self.destination.join(name);
                if let Some(current) = stamp(&target)? {
                    if self.approved.get(&target) != Some(&current) && !conflicts.contains(&target)
                    {
                        conflicts.push(target.clone());
                        stamps.push((target, current));
                    }
                }
            }
            if conflicts.is_empty() {
                return Ok(());
            }
            let answer = ask(conflicts)?;
            match answer["action"].as_str() {
                Some("cancel") => return Err("Cancelled".into()),
                Some("overwrite") => {
                    self.approved.extend(stamps);
                }
                Some("folder") => {
                    let folder = answer["path"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or("Choose a destination folder.")?;
                    fs::create_dir_all(folder).map_err(|e| e.to_string())?;
                    self.destination = Path::new(folder)
                        .canonicalize()
                        .map_err(|e| e.to_string())?;
                }
                _ => return Err("Invalid destination choice.".into()),
            }
        }
    }

    pub fn files(&self) -> Result<Vec<PathBuf>, String> {
        let mut result = Vec::new();
        collect(self.path(), self.path(), &mut result)?;
        result.sort();
        Ok(result)
    }

    pub fn preserve(&mut self, message: &str) -> String {
        self.lock.take();
        match self.directory.take() {
            Some(directory) => {
                let directory = directory.keep();
                let _ = fs::write(directory.join(KEPT_MARKER), b"");
                format!("{message} The new download is in {}", directory.display())
            }
            None => message.into(),
        }
    }

    pub fn publish(&mut self, cancel: &AtomicBool) -> Result<HashMap<PathBuf, PathBuf>, String> {
        let result = self.publish_inner(cancel);
        match result {
            Err(error) if error != "Cancelled" => Err(self.preserve(&error)),
            result => result,
        }
    }

    fn publish_inner(&self, cancel: &AtomicBool) -> Result<HashMap<PathBuf, PathBuf>, String> {
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled".into());
        }
        let files = self.files()?;
        if files.is_empty() {
            return Err("The download did not produce a file.".into());
        }
        let mut staged = Vec::new();
        if let Err(error) = self.stage(files, cancel, &mut staged) {
            return Err(with_failures(error, unstage(&staged)));
        }
        let mut published: Vec<(&Staged, Option<PathBuf>)> = Vec::new();
        for (index, item) in staged.iter().enumerate() {
            let mut failures = Vec::new();
            let backup = if item.previous.is_some() {
                let backup = item
                    .saved
                    .with_file_name(format!(".finfetcher-previous-{}", uuid::Uuid::new_v4()));
                move_file(&item.target, &backup).map(|_| Some(backup))
            } else {
                Ok(None)
            };
            let saved = backup.and_then(|backup| {
                if let Err(error) = move_file(&item.saved, &item.target) {
                    if let Some(backup) = &backup {
                        if let Err(error) = move_file(backup, &item.target) {
                            failures.push(format!(
                                "{}: {error}. Previous file retained at {}",
                                item.target.display(),
                                backup.display()
                            ));
                        }
                    }
                    return Err(error);
                }
                Ok(backup)
            });
            let error = match saved {
                Ok(backup) => {
                    published.push((item, backup));
                    continue;
                }
                Err(error) => error,
            };
            for (item, backup) in published.into_iter().rev() {
                let returned = if item.moved {
                    move_file(&item.target, &item.source)
                } else {
                    fs::remove_file(&item.target)
                };
                match (returned, backup) {
                    (Err(error), backup) => failures.push(format!(
                        "{}: {error}{}",
                        item.target.display(),
                        backup
                            .map(|b| format!(". Previous file retained at {}", b.display()))
                            .unwrap_or_default()
                    )),
                    (Ok(()), Some(backup)) => {
                        if let Err(error) = move_file(&backup, &item.target) {
                            failures.push(format!(
                                "{}: {error}. Previous file retained at {}",
                                item.target.display(),
                                backup.display()
                            ));
                        }
                    }
                    (Ok(()), None) => {}
                }
            }
            failures.extend(unstage(&staged[index..]));
            return Err(with_failures(
                format!("Could not save {}: {error}", item.target.display()),
                failures,
            ));
        }
        let mut result = HashMap::new();
        for (item, backup) in published {
            if let Some(backup) = backup {
                let _ = fs::remove_file(backup);
            }
            result.insert(item.source.clone(), item.target.clone());
        }
        Ok(result)
    }

    fn stage(
        &self,
        files: Vec<PathBuf>,
        cancel: &AtomicBool,
        staged: &mut Vec<Staged>,
    ) -> Result<(), String> {
        for name in files {
            let target = self.destination.join(&name);
            let source = self.path().join(&name);
            let current = stamp(&target)?;
            if current
                .as_ref()
                .is_some_and(|s| self.approved.get(&target) != Some(s))
            {
                return Err("A new filename conflict appeared. Existing files were kept.".into());
            }
            let parent = target.parent().ok_or("Invalid destination.")?;
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            let parent = parent.canonicalize().map_err(|e| e.to_string())?;
            if !parent.starts_with(&self.destination) {
                return Err("A destination folder points outside the selected folder.".into());
            }
            let saved = parent.join(format!(".finfetcher-save-{}", uuid::Uuid::new_v4()));
            let moved = place(&source, &saved, move_file)?;
            staged.push(Staged {
                source,
                target,
                previous: current,
                saved,
                moved,
            });
            if cancel.load(Ordering::Acquire) {
                return Err("Cancelled".into());
            }
        }
        for item in staged.iter() {
            if stamp(&item.target)? != item.previous {
                return Err("A destination changed while the download was being saved. Existing files were kept.".into());
            }
        }
        Ok(())
    }
}

impl Drop for DownloadOutput {
    fn drop(&mut self) {
        self.lock.take();
        let Some(directory) = self.directory.take() else {
            return;
        };
        let directory = directory.keep();
        // Media processes can hold files for a moment after cancel returns.
        for _ in 0..20 {
            match fs::remove_dir_all(&directory) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                _ => return,
            }
        }
    }
}

fn with_failures(error: String, failures: Vec<String>) -> String {
    if failures.is_empty() {
        error
    } else {
        format!("{error}. Could not restore {}", failures.join(", "))
    }
}

fn unstage(staged: &[Staged]) -> Vec<String> {
    let mut failures = Vec::new();
    for item in staged {
        if !item.moved {
            let _ = fs::remove_file(&item.saved);
        } else if let Err(error) = move_file(&item.saved, &item.source) {
            failures.push(format!(
                "{}: {error}. The new file is at {}",
                item.source.display(),
                item.saved.display()
            ));
        }
    }
    failures
}

fn place(
    source: &Path,
    saved: &Path,
    mover: fn(&Path, &Path) -> io::Result<()>,
) -> Result<bool, String> {
    match mover(source, saved) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            let copied = fs::copy(source, saved)
                .and_then(|_| fs::OpenOptions::new().write(true).open(saved)?.sync_all());
            if let Err(error) = copied {
                let _ = fs::remove_file(saved);
                return Err(format!("Could not save the download: {error}"));
            }
            Ok(false)
        }
        Err(error) => Err(format!("Could not save the download: {error}")),
    }
}

#[cfg(windows)]
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let wide = |path: &Path| -> Vec<u16> { path.as_os_str().encode_wide().chain([0]).collect() };
    let (from, to) = (wide(from), wide(to));
    // No flags: fail instead of replacing an existing file or copying across volumes.
    let moved = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
    };
    if moved == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    fs::rename(from, to)
}

#[cfg(windows)]
fn hold(folder: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    // Leaving out FILE_SHARE_DELETE is what marks the folder as in use for remove_leftovers.
    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(folder)
}

#[cfg(not(windows))]
fn hold(folder: &Path) -> io::Result<fs::File> {
    fs::File::open(folder)
}

#[cfg(windows)]
fn unused(folder: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    fs::OpenOptions::new()
        .access_mode(DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(folder)
        .is_ok()
}

#[cfg(not(windows))]
fn unused(_: &Path) -> bool {
    false
}

fn remove_leftovers(destination: &Path) {
    let Ok(entries) = fs::read_dir(destination) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && !path.join(KEPT_MARKER).exists()
            && unused(&path)
        {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn collect(root: &Path, folder: &Path, result: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(folder).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err("The downloader created an unexpected link.".into());
        }
        if kind.is_dir() {
            collect(root, &entry.path(), result)?;
        } else if kind.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".finfetcher-")
                || name.ends_with(".part")
                || name.ends_with(".ytdl")
                || name.contains(".temp.")
            {
                continue;
            }
            result.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_owned(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cancelling_confirmation_preserves_the_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("clip.mp4");
        fs::write(&target, b"old clip").unwrap();
        let mut output = DownloadOutput::new(directory.path()).unwrap();
        fs::write(output.path().join("clip.mp4"), b"new clip").unwrap();
        assert_eq!(
            output
                .prepare(&["clip.mp4".into()], &AtomicBool::new(false), |paths| {
                    assert_eq!(paths.len(), 1);
                    Ok(json!({"action":"cancel"}))
                })
                .unwrap_err(),
            "Cancelled"
        );
        drop(output);
        assert_eq!(fs::read(target).unwrap(), b"old clip");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn confirmation_lists_each_existing_path_once() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("clip.mp4"), b"old").unwrap();
        let mut output = DownloadOutput::new(directory.path()).unwrap();
        output
            .prepare(
                &["clip.mp4".into(), "clip.mp4".into()],
                &AtomicBool::new(false),
                |paths| {
                    assert_eq!(paths.len(), 1);
                    Ok(json!({"action":"overwrite"}))
                },
            )
            .unwrap();
    }

    #[test]
    fn approved_overwrite_and_alternate_folder_publish_atomically() {
        for alternate in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let other = tempfile::tempdir().unwrap();
            let target = directory.path().join("clip.mp4");
            fs::write(&target, b"old clip").unwrap();
            let mut output = DownloadOutput::new(directory.path()).unwrap();
            fs::write(output.path().join("clip.mp4"), b"new clip").unwrap();
            output
                .prepare(&["clip.mp4".into()], &AtomicBool::new(false), |_| {
                    Ok(if alternate {
                        json!({"action":"folder","path":other.path()})
                    } else {
                        json!({"action":"overwrite"})
                    })
                })
                .unwrap();
            output.publish(&AtomicBool::new(false)).unwrap();
            if alternate {
                assert_eq!(fs::read(target).unwrap(), b"old clip");
                assert_eq!(
                    fs::read(other.path().join("clip.mp4")).unwrap(),
                    b"new clip"
                );
            } else {
                assert_eq!(fs::read(target).unwrap(), b"new clip");
            }
        }
    }

    #[test]
    fn later_conflict_keeps_both_existing_and_downloaded_files() {
        let directory = tempfile::tempdir().unwrap();
        let mut output = DownloadOutput::new(directory.path()).unwrap();
        let stage = output.path().to_owned();
        fs::write(stage.join("clip.mp4"), b"download").unwrap();
        output
            .prepare(&["clip.mp4".into()], &AtomicBool::new(false), |_| {
                panic!("No conflict yet")
            })
            .unwrap();
        fs::write(directory.path().join("clip.mp4"), b"appeared later").unwrap();
        assert!(output
            .publish(&AtomicBool::new(false))
            .unwrap_err()
            .contains("new filename conflict"));
        drop(output);
        assert_eq!(
            fs::read(directory.path().join("clip.mp4")).unwrap(),
            b"appeared later"
        );
        assert_eq!(fs::read(stage.join("clip.mp4")).unwrap(), b"download");
    }

    #[test]
    fn cancelled_publish_leaves_old_files_and_rejects_traversal() {
        let directory = tempfile::tempdir().unwrap();
        let mut output = DownloadOutput::new(directory.path()).unwrap();
        assert!(output
            .prepare(
                &["../outside.mp4".into()],
                &AtomicBool::new(false),
                |_| panic!()
            )
            .is_err());
        fs::write(output.path().join("clip.mp4"), b"download").unwrap();
        assert_eq!(
            output.publish(&AtomicBool::new(true)).unwrap_err(),
            "Cancelled"
        );
        assert!(!directory.path().join("clip.mp4").exists());
    }

    #[cfg(windows)]
    #[test]
    fn failed_second_replacement_restores_the_first_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let mut output = DownloadOutput::new(directory.path()).unwrap();
        for name in ["a.mp4", "b.mp4"] {
            fs::write(directory.path().join(name), b"old clip").unwrap();
            fs::write(output.path().join(name), b"new clip").unwrap();
        }
        output
            .prepare(
                &["a.mp4".into(), "b.mp4".into()],
                &AtomicBool::new(false),
                |_| Ok(json!({"action":"overwrite"})),
            )
            .unwrap();
        let stage = output.path().to_owned();
        let original = identity(&directory.path().join("a.mp4"));
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(directory.path().join("b.mp4"))
            .unwrap();
        let error = output.publish(&AtomicBool::new(false)).unwrap_err();
        assert!(error.contains("Could not save"), "{error}");
        assert!(!error.contains("Could not restore"), "{error}");
        for name in ["a.mp4", "b.mp4"] {
            assert_eq!(fs::read(directory.path().join(name)).unwrap(), b"old clip");
            assert_eq!(fs::read(stage.join(name)).unwrap(), b"new clip");
        }
        assert_eq!(identity(&directory.path().join("a.mp4")), original);
        drop(locked);
        assert_eq!(names(directory.path()).len(), 3);
    }

    fn names(folder: &Path) -> Vec<String> {
        fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[cfg(windows)]
    fn identity(path: &Path) -> (u32, u32, u32) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let file = fs::File::open(path).unwrap();
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        assert_ne!(
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) },
            0
        );
        (
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
        )
    }

    #[cfg(windows)]
    #[test]
    fn saving_on_the_same_volume_moves_files_instead_of_copying() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("clip.mp4");
        fs::write(&target, b"old clip").unwrap();
        let mut output = DownloadOutput::new(directory.path()).unwrap();
        let source = output.path().join("clip.mp4");
        fs::write(&source, b"new clip").unwrap();
        let downloaded = identity(&source);
        output
            .prepare(&["clip.mp4".into()], &AtomicBool::new(false), |_| {
                Ok(json!({"action":"overwrite"}))
            })
            .unwrap();
        output.publish(&AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new clip");
        assert_eq!(identity(&target), downloaded);
        assert!(!source.exists());
        drop(output);
        assert_eq!(names(directory.path()), ["clip.mp4"]);
    }

    #[test]
    fn saving_across_volumes_falls_back_to_a_copy() {
        #[cfg(windows)]
        assert_eq!(
            io::Error::from_raw_os_error(17).kind(),
            io::ErrorKind::CrossesDevices
        );
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("clip.mp4");
        let saved = directory.path().join("saved.mp4");
        fs::write(&source, b"new clip").unwrap();
        assert!(!place(&source, &saved, |_, _| Err(
            io::ErrorKind::CrossesDevices.into()
        ))
        .unwrap());
        assert_eq!(fs::read(&saved).unwrap(), b"new clip");
        assert_eq!(fs::read(&source).unwrap(), b"new clip");
        fs::remove_file(&saved).unwrap();
        assert!(place(&source, &saved, |_, _| Err(
            io::ErrorKind::PermissionDenied.into()
        ))
        .is_err());
        assert!(!saved.exists());
    }

    #[test]
    fn new_download_removes_only_unused_staging_leftovers() {
        let directory = tempfile::tempdir().unwrap();
        let mut kept = DownloadOutput::new(directory.path()).unwrap();
        fs::write(kept.path().join("clip.mp4"), b"kept").unwrap();
        let kept = PathBuf::from(
            kept.preserve("Saving failed.")
                .rsplit_once("The new download is in ")
                .unwrap()
                .1,
        );
        let running = DownloadOutput::new(directory.path()).unwrap();
        fs::write(running.path().join("clip.mp4.part"), b"running").unwrap();
        let leftover = directory.path().join(".finfetcher-stage-AbC123");
        fs::create_dir_all(leftover.join("nested")).unwrap();
        fs::write(leftover.join("nested").join("clip.mp4.part"), b"partial").unwrap();
        let archive = directory.path().join(".finfetcher-archive.txt");
        fs::write(&archive, b"youtube abc").unwrap();
        let older = directory.path().join(".finfetcher-AbC123");
        let unrelated = directory.path().join("finfetcher-stage-notes");
        for folder in [&older, &unrelated] {
            fs::create_dir(folder).unwrap();
        }
        let next = DownloadOutput::new(directory.path()).unwrap();
        assert!(!leftover.exists());
        assert_eq!(fs::read(kept.join("clip.mp4")).unwrap(), b"kept");
        assert_eq!(
            fs::read(running.path().join("clip.mp4.part")).unwrap(),
            b"running"
        );
        assert!(archive.is_file() && older.is_dir() && unrelated.is_dir());
        drop(next);
        drop(running);
    }

    #[cfg(windows)]
    #[test]
    fn cancelled_download_removes_staging_once_media_processes_release_files() {
        use std::os::windows::fs::OpenOptionsExt;
        let directory = tempfile::tempdir().unwrap();
        let output = DownloadOutput::new(directory.path()).unwrap();
        let stage = output.path().to_owned();
        let partial = stage.join("clip.mp4.part");
        fs::write(&partial, b"partial").unwrap();
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&partial)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            drop(held);
        });
        drop(output);
        release.join().unwrap();
        assert!(!stage.exists());
    }
}
