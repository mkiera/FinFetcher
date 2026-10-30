#![windows_subsystem = "windows"]

use std::{env, fs, process, thread, time::{Duration, Instant}};

fn main() {
    if env::args().any(|arg| arg == "/install") { process::exit(7); }
    let exe = env::current_exe().unwrap();
    let install = exe.parent().unwrap();
    if let Some(log) = env::args().find_map(|arg| arg.strip_prefix("/LOG=").map(str::to_owned)) {
        let mode = fs::read_to_string(install.join("installer-mode.txt")).unwrap_or_default();
        if mode.trim() == "fail" { process::exit(7); }
        let message = if mode.trim() == "dialog" { "2026-09-25 00:00:00   Message box (OK): Fixture installer blocked.\n" } else { "2026-09-25 00:00:00   Log opened.\n" };
        fs::write(log, message).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !install.join("release-installer.txt").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        fs::write(install.join("installer-exited.txt"), "done").unwrap();
        return;
    }
    fs::write(install.join("executable-build.txt"), if cfg!(old_payload) { "old" } else { "new" }).unwrap();
    let version = fs::read_to_string(install.join("version.txt")).unwrap();
    if matches!(version.trim(), "old" | "blocked") {
        let runtime = install.join("_internal");
        env::set_current_dir(&runtime).unwrap();
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)] {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let _lock = options.open(runtime.join("runtime.pyd")).unwrap();
        fs::write(install.join("started.txt"), process::id().to_string()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(50);
        while Instant::now() < deadline {
            if version.trim() == "old" && install.join("update.log").exists() {
                thread::sleep(Duration::from_millis(1500));
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
    } else {
        fs::write(install.join("relaunched.txt"), version).unwrap();
    }
}
