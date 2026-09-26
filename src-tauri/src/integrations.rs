use std::path::{Path, PathBuf};

pub fn flipperclipper() -> Option<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(key) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{A67FDBB5-CF45-489D-8A41-0E7576A446F1}_is1") {
            if let Ok(folder) = key.get_value::<String,_>("InstallLocation") { paths.push(PathBuf::from(folder).join("FlipperClipper.exe")); }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path).map(|folder| {
            folder.join(if cfg!(windows) {
                "FlipperClipper.exe"
            } else {
                "flipperclipper"
            })
        }));
    }
    for name in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(name) {
            paths.push(PathBuf::from(&root).join("Programs/FlipperClipper/FlipperClipper.exe"));
            paths.push(PathBuf::from(root).join("FlipperClipper/FlipperClipper.exe"));
        }
    }
    first_installed(paths)
}

pub fn open_clip(path: &Path) -> Result<(), String> {
    let executable = flipperclipper().ok_or("FlipperClipper is no longer installed.")?;
    clip_command(&executable, path)?
        .spawn()
        .map_err(|e| format!("FlipperClipper did not open: {e}"))?;
    Ok(())
}

fn first_installed(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths
        .into_iter()
        .find(|path| path.is_file())
        .and_then(|path| path.canonicalize().ok())
}

fn clip_command(executable: &Path, path: &Path) -> Result<std::process::Command, String> {
    if !path.is_file() {
        return Err(format!(
            "The downloaded video was not found: {}",
            path.display()
        ));
    }
    let mut command = crate::process::command(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000008 | 0x00000200);
    }
    command.arg(path.canonicalize().map_err(|e| e.to_string())?);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_candidates_preserve_registry_then_path_then_standard_precedence() {
        let directory = tempfile::tempdir().unwrap();
        let registered = directory.path().join("registered.exe");
        let path = directory.path().join("path.exe");
        std::fs::write(&registered, b"registered").unwrap();
        std::fs::write(&path, b"path").unwrap();
        assert_eq!(
            first_installed([registered.clone(), path.clone()]),
            Some(registered.canonicalize().unwrap())
        );
        assert_eq!(
            first_installed([directory.path().join("missing.exe"), path.clone()]),
            Some(path.canonicalize().unwrap())
        );
    }

    #[test]
    fn command_passes_one_exact_absolute_media_path_and_rejects_missing_files() {
        let directory = tempfile::tempdir().unwrap();
        let clip = directory.path().join("a clip & quote ' 名.mp4");
        std::fs::write(&clip, b"media").unwrap();
        let command = clip_command(Path::new("FlipperClipper.exe"), &clip).unwrap();
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![clip.canonicalize().unwrap().as_os_str()]
        );
        assert!(clip_command(
            Path::new("FlipperClipper.exe"),
            &directory.path().join("missing.mp4")
        )
        .is_err());
    }
}
