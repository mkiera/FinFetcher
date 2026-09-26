use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut result = Command::new(program);
    result.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        result.creation_flags(0x08000000);
    }
    result
}

#[derive(Debug)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub error: String,
}

fn reader(
    input: impl Read + Send + 'static,
    stderr: bool,
    tx: mpsc::Sender<(bool, String)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut input = BufReader::new(input);
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            match input.read_until(b'\n', &mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&bytes)
                        .trim_end_matches(['\r', '\n'])
                        .to_owned();
                    if tx.send((stderr, text)).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

pub fn run(
    program: &Path,
    args: &[String],
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    mut line: impl FnMut(bool, &str),
) -> Result<Output, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Cancelled".into());
    }
    let mut cmd = command(program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000 | 0x00000004);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Could not start {}: {e}", program.display()))?;
    let tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let (tx, rx) = mpsc::channel();
    let out = reader(
        child.stdout.take().ok_or("Missing process output")?,
        false,
        tx.clone(),
    );
    let err = reader(
        child.stderr.take().ok_or("Missing process errors")?,
        true,
        tx,
    );
    let started = Instant::now();
    let mut stdout = String::new();
    let mut errors = VecDeque::new();
    let mut abort = None;
    let status = loop {
        if cancel.load(Ordering::Acquire) {
            abort = Some("Cancelled".to_string());
        }
        if timeout.is_some_and(|limit| started.elapsed() >= limit) {
            abort = Some("The operation timed out. Check your connection and try again.".into());
        }
        if abort.is_some() {
            tree.terminate(&mut child);
            break child.wait();
        }
        if let Ok((stderr, text)) = rx.recv_timeout(Duration::from_millis(40)) {
            collect_line(stderr, &text, &mut stdout, &mut errors, &mut line);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => (),
            Err(e) => {
                tree.terminate(&mut child);
                break Err(e);
            }
        }
    };
    tree.terminate(&mut child);
    let _ = out.join();
    let _ = err.join();
    for (stderr, text) in rx.try_iter() {
        collect_line(stderr, &text, &mut stdout, &mut errors, &mut line);
    }
    if let Some(error) = abort {
        return Err(error);
    }
    let status = status.map_err(|e| e.to_string())?;
    let error = errors.into_iter().collect::<Vec<_>>().join("\n");
    Ok(Output {
        success: status.success(),
        stdout,
        error,
    })
}

fn collect_line(
    stderr: bool,
    text: &str,
    stdout: &mut String,
    errors: &mut VecDeque<String>,
    line: &mut impl FnMut(bool, &str),
) {
    line(stderr, text);
    if stderr {
        if errors.len() == 60 {
            errors.pop_front();
        }
        errors.push_back(text.to_owned());
    } else if stdout.len() + text.len() < 32 * 1024 * 1024 {
        stdout.push_str(text);
        stdout.push('\n');
    }
}

#[cfg(windows)]
struct ProcessTree(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProcessTree {
    fn attach(child: &Child) -> Result<Self, String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::{
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                    THREADENTRY32,
                },
                JobObjects::{
                    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                },
                Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
            },
        };
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let tree = Self(job);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(job, child.as_raw_handle()) == 0
            {
                return Err(format!(
                    "Could not supervise the media process: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut found = Thread32First(snapshot, &mut entry);
            let mut resumed = false;
            while found != 0 {
                if entry.th32OwnerProcessID == child.id() {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        resumed = ResumeThread(thread) != u32::MAX;
                        CloseHandle(thread);
                    }
                    break;
                }
                found = Thread32Next(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            if !resumed {
                return Err("Could not resume the supervised media process.".into());
            }
            Ok(tree)
        }
    }

    fn terminate(&self, _child: &mut Child) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(not(windows))]
struct ProcessTree;

#[cfg(not(windows))]
impl ProcessTree {
    fn attach(_: &Child) -> Result<Self, String> {
        Ok(Self)
    }
    fn terminate(&self, child: &mut Child) {
        let _ = child.kill();
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn captures_both_streams_without_shell_encoding_loss() {
        let result = run(
            Path::new("powershell.exe"),
            &[
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "[Console]::WriteLine('output'); [Console]::Error.WriteLine('detail'); exit 3"
                    .into(),
            ],
            &AtomicBool::new(false),
            Some(Duration::from_secs(10)),
            |_, _| {},
        )
        .unwrap();
        assert!(!result.success);
        assert_eq!(result.stdout.trim(), "output");
        assert_eq!(result.error, "detail");
    }

    #[test]
    fn cancels_a_silent_process_and_reaps_it() {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = cancel.clone();
        let started = Instant::now();
        let thread = std::thread::spawn(move || {
            run(
                Path::new("powershell.exe"),
                &[
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    "Start-Sleep -Seconds 30".into(),
                ],
                &worker,
                None,
                |_, _| {},
            )
        });
        std::thread::sleep(Duration::from_millis(300));
        cancel.store(true, Ordering::Release);
        assert_eq!(thread.join().unwrap().unwrap_err(), "Cancelled");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn timeout_stops_a_silent_child() {
        let result = run(
            Path::new("powershell.exe"),
            &[
                "-NoProfile".into(),
                "-Command".into(),
                "Start-Sleep -Seconds 30".into(),
            ],
            &AtomicBool::new(false),
            Some(Duration::from_millis(200)),
            |_, _| {},
        );
        assert!(result.unwrap_err().contains("timed out"));
    }
}
