#![cfg(windows)]

use finfetcher::process;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

const FIXTURE: &str = r#"
use std::{fs::{self,OpenOptions},io::Write,path::PathBuf,process::{Command,Stdio},time::{Duration,Instant}};
fn main() {
    let arguments: Vec<_> = std::env::args_os().collect();
    let role = arguments[1].to_str().unwrap();
    let root = PathBuf::from(&arguments[2]);
    assert!(matches!(role,"parent"|"child"|"grandchild"));
    fs::write(root.join(format!("{role}.pid")), std::process::id().to_string()).unwrap();
    let mut heartbeat = OpenOptions::new().create_new(true).write(true)
        .open(root.join(format!("{role}.heartbeat"))).unwrap();
    let started = Instant::now();
    let _descendant = match role {
        "parent" | "child" => {
            let mut command = Command::new(std::env::current_exe().unwrap());
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
            Some(command.args([if role=="parent" {"child"} else {"grandchild"}])
                .arg(&root).stdin(Stdio::null()).spawn().unwrap())
        }
        _ => None,
    };
    while started.elapsed() < Duration::from_secs(12) {
        writeln!(heartbeat,"{role}").unwrap();
        heartbeat.flush().unwrap();
        if role == "parent" && root.join("parent-exit").is_file() {
            println!("parent completed");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
"#;

struct Fixture {
    _directory: tempfile::TempDir,
    executable: PathBuf,
}

fn fixture() -> &'static Fixture {
    static FIXTURE_BUILD: OnceLock<Fixture> = OnceLock::new();
    FIXTURE_BUILD.get_or_init(|| {
        let directory = tempfile::Builder::new()
            .prefix("finfetcher-process-fixture-")
            .tempdir()
            .unwrap();
        let executable = directory.path().join("process-tree-fixture.exe");
        let mut compiler = process::command("rustc")
            .args([
                "--crate-name",
                "process_tree_fixture",
                "--edition",
                "2021",
                "-O",
                "-o",
            ])
            .arg(&executable)
            .arg("-")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        compiler
            .stdin
            .take()
            .unwrap()
            .write_all(FIXTURE.as_bytes())
            .unwrap();
        let compiled = compiler.wait_with_output().unwrap();
        assert!(
            compiled.status.success(),
            "Fixture failed to compile: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        Fixture {
            _directory: directory,
            executable,
        }
    })
}

struct VerifiedProcess {
    handle: HANDLE,
    role: &'static str,
}

impl VerifiedProcess {
    fn open(pid: u32, role: &'static str, executable: &Path) -> Self {
        unsafe {
            let handle = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
                0,
                pid,
            );
            assert!(
                !handle.is_null(),
                "Could not open {role} fixture PID {pid}: {}",
                std::io::Error::last_os_error()
            );
            let mut path = vec![0_u16; 32768];
            let mut length = path.len() as u32;
            if QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length) == 0 {
                let error = std::io::Error::last_os_error();
                CloseHandle(handle);
                panic!("Could not validate {role} fixture PID {pid}: {error}");
            }
            let actual = PathBuf::from(String::from_utf16_lossy(&path[..length as usize]));
            let expected = fs::canonicalize(executable).unwrap();
            if fs::canonicalize(&actual).unwrap() != expected {
                CloseHandle(handle);
                panic!(
                    "PID {pid} belongs to {}, expected {}",
                    actual.display(),
                    expected.display()
                );
            }
            assert_eq!(
                WaitForSingleObject(handle, 0),
                WAIT_TIMEOUT,
                "{role} was not alive before the action"
            );
            Self { handle, role }
        }
    }

    fn assert_exited(&self) {
        assert_eq!(
            unsafe { WaitForSingleObject(self.handle, 1000) },
            WAIT_OBJECT_0,
            "{} survived the supervisor returning",
            self.role
        );
    }

    fn cleanup(&self) {
        unsafe {
            if WaitForSingleObject(self.handle, 0) == WAIT_TIMEOUT {
                TerminateProcess(self.handle, 99);
                WaitForSingleObject(self.handle, 2000);
            }
        }
    }
}

impl Drop for VerifiedProcess {
    fn drop(&mut self) {
        self.cleanup();
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

struct RunningTree {
    directory: tempfile::TempDir,
    cancel: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<process::Output, String>>,
    worker: Option<JoinHandle<()>>,
    processes: Vec<VerifiedProcess>,
}

impl RunningTree {
    fn start(timeout: Option<Duration>) -> Self {
        let executable = fixture().executable.clone();
        let directory = tempfile::Builder::new()
            .prefix("finfetcher-process-tree-")
            .tempdir()
            .unwrap();
        let root = directory.path().to_owned();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let (send, result) = mpsc::channel();
        let program = executable.clone();
        let worker = std::thread::spawn(move || {
            let output = process::run(
                &program,
                &["parent".to_owned(), root.to_string_lossy().into_owned()],
                &cancelled,
                timeout,
                |_, _| {},
            );
            let _ = send.send(output);
        });
        let mut running = Self {
            directory,
            cancel,
            result,
            worker: Some(worker),
            processes: Vec::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(4);
        for role in ["parent", "child", "grandchild"] {
            let pid = loop {
                if let Some(pid) =
                    fs::read_to_string(running.directory.path().join(format!("{role}.pid")))
                        .ok()
                        .and_then(|text| text.parse::<u32>().ok())
                {
                    break pid;
                }
                assert!(Instant::now() < deadline, "{role} never started");
                std::thread::sleep(Duration::from_millis(10));
            };
            running
                .processes
                .push(VerifiedProcess::open(pid, role, &executable));
        }
        loop {
            if running.heartbeat_lengths().iter().all(|length| *length > 0) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "The fixture tree never started writing"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        running
    }

    fn finish(&mut self, limit: Duration) -> Result<process::Output, String> {
        let result = self
            .result
            .recv_timeout(limit)
            .expect("The supervisor did not return after stopping the process tree");
        self.worker.take().unwrap().join().unwrap();
        result
    }

    fn heartbeat_lengths(&self) -> Vec<u64> {
        ["parent", "child", "grandchild"]
            .into_iter()
            .map(|role| {
                fs::metadata(self.directory.path().join(format!("{role}.heartbeat")))
                    .map(|m| m.len())
                    .unwrap_or(0)
            })
            .collect()
    }

    fn assert_no_survivors_or_later_writes(&self) {
        for process in &self.processes {
            process.assert_exited();
        }
        let finished = self.heartbeat_lengths();
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(
            self.heartbeat_lengths(),
            finished,
            "A descendant wrote files after the supervisor returned"
        );
    }
}

impl Drop for RunningTree {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        for process in self.processes.iter().rev() {
            process.cleanup();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
fn cancellation_terminates_parent_child_and_grandchild_before_returning() {
    let mut tree = RunningTree::start(None);
    let started = Instant::now();
    tree.cancel.store(true, Ordering::Release);
    assert_eq!(
        tree.finish(Duration::from_secs(3)).unwrap_err(),
        "Cancelled"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    tree.assert_no_survivors_or_later_writes();
}

#[test]
fn timeout_terminates_parent_child_and_grandchild_before_returning() {
    let mut tree = RunningTree::start(Some(Duration::from_secs(5)));
    assert!(tree
        .finish(Duration::from_secs(6))
        .unwrap_err()
        .contains("timed out"));
    tree.assert_no_survivors_or_later_writes();
}

#[test]
fn normal_parent_exit_terminates_remaining_descendants() {
    let mut tree = RunningTree::start(None);
    fs::write(tree.directory.path().join("parent-exit"), b"exit").unwrap();
    let output = tree.finish(Duration::from_secs(3)).unwrap();
    assert!(output.success);
    assert_eq!(output.stdout.trim(), "parent completed");
    tree.assert_no_survivors_or_later_writes();
}
