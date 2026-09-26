use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::SystemTime,
};

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

pub struct DownloadOutput {
    directory: Option<tempfile::TempDir>,
    pub destination: PathBuf,
    approved: HashMap<PathBuf, Stamp>,
}

impl DownloadOutput {
    pub fn new(destination: &Path) -> Result<Self, String> {
        fs::create_dir_all(destination)
            .map_err(|e| format!("Could not create the destination: {e}"))?;
        let destination = destination.canonicalize().map_err(|e| e.to_string())?;
        let directory = tempfile::Builder::new()
            .prefix(".finfetcher-")
            .tempdir_in(&destination)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            directory: Some(directory),
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
        match self.directory.take() {
            Some(directory) => format!(
                "{message} The new download is in {}",
                directory.keep().display()
            ),
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
        let mut copies = Vec::new();
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
            let temporary = tempfile::Builder::new()
                .prefix(".finfetcher-save-")
                .tempfile_in(&parent)
                .map_err(|e| e.to_string())?;
            fs::copy(&source, temporary.path())
                .map_err(|e| format!("Could not save the download: {e}"))?;
            temporary.as_file().sync_all().map_err(|e| e.to_string())?;
            let backup = if current.is_some() {
                let backup = tempfile::Builder::new()
                    .prefix(".finfetcher-previous-")
                    .tempfile_in(&parent)
                    .map_err(|e| e.to_string())?;
                fs::copy(&target, backup.path()).map_err(|e| e.to_string())?;
                Some(backup)
            } else {
                None
            };
            copies.push((source, target, current, temporary, backup));
        }
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled".into());
        }
        for (_, target, previous, _, _) in &copies {
            if stamp(target)? != *previous {
                return Err("A destination changed while the download was being saved. Existing files were kept.".into());
            }
        }
        let mut published: Vec<(PathBuf, Option<tempfile::NamedTempFile>)> = Vec::new();
        let mut result = HashMap::new();
        for (source, target, previous, temporary, backup) in copies {
            let saved = if previous.is_some() {
                temporary.persist(&target)
            } else {
                temporary.persist_noclobber(&target)
            };
            if let Err(error) = saved {
                let mut failures = Vec::new();
                for (path, previous) in published.into_iter().rev() {
                    if let Some(previous) = previous {
                        if let Err(error) = previous.persist(&path) {
                            let reason = error.error.to_string();
                            let recovery = error
                                .file
                                .into_temp_path()
                                .keep()
                                .map_err(|e| e.to_string());
                            failures.push(format!(
                                "{}: {reason}. Previous file retained at {}",
                                path.display(),
                                recovery.map(|p| p.display().to_string()).unwrap_or_else(
                                    |e| format!("an unavailable temporary path ({e})")
                                )
                            ));
                        }
                    } else if let Err(error) = fs::remove_file(&path) {
                        failures.push(format!("{}: {error}", path.display()));
                    }
                }
                return Err(format!(
                    "Could not save {}: {}{}",
                    target.display(),
                    error.error,
                    if failures.is_empty() {
                        String::new()
                    } else {
                        format!(". Could not restore {}", failures.join(", "))
                    }
                ));
            }
            result.insert(source, target.clone());
            published.push((target, backup));
        }
        Ok(result)
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
        let locked = fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(directory.path().join("b.mp4"))
            .unwrap();
        assert!(output
            .publish(&AtomicBool::new(false))
            .unwrap_err()
            .contains("Could not save"));
        for name in ["a.mp4", "b.mp4"] {
            assert_eq!(fs::read(directory.path().join(name)).unwrap(), b"old clip");
        }
        drop(locked);
    }
}
