use crate::{
    events::EventSink,
    integrations,
    media::{self, add, enabled, string, DownloadRequest},
    output::DownloadOutput,
    process,
    settings::Settings,
    tools::Tools,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};

pub struct Job {
    pub id: String,
    request: DownloadRequest,
    pub cancelled: AtomicBool,
    destination: Mutex<(Option<String>, Option<Value>)>,
    wake: Condvar,
}

impl Job {
    fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Acquire) {
            Err("Cancelled".into())
        } else {
            Ok(())
        }
    }

    fn ask(&self, paths: Vec<PathBuf>, sink: &EventSink) -> Result<Value, String> {
        self.check()?;
        let id = uuid::Uuid::new_v4().to_string();
        *self.destination.lock().unwrap_or_else(|e| e.into_inner()) = (Some(id.clone()), None);
        sink(json!({"destination_request":{"id":id,"paths":paths}}));
        let mut answer = self
            .destination
            .lock()
            .map_err(|_| "The destination dialog is unavailable.")?;
        loop {
            self.check()?;
            if let Some(result) = answer.1.take() {
                return Ok(result);
            }
            answer = self
                .wake
                .wait_timeout(answer, Duration::from_millis(100))
                .map_err(|_| "The destination dialog is unavailable.")?
                .0;
        }
    }
}

#[derive(Default)]
struct State {
    job: Option<Arc<Job>>,
    installing: bool,
    closing: bool,
}

pub struct Downloads {
    tools: Arc<Tools>,
    settings: Arc<Settings>,
    state: Mutex<State>,
    finished: Condvar,
    cookies: Mutex<Option<Option<tempfile::TempDir>>>,
    browser_cookies: bool,
    encoders: Mutex<HashMap<PathBuf, String>>,
    shutdown: AtomicBool,
}

impl Downloads {
    pub fn new(tools: Arc<Tools>, settings: Arc<Settings>) -> Self {
        Self {
            tools,
            settings,
            state: Mutex::new(State::default()),
            finished: Condvar::new(),
            cookies: Mutex::new(None),
            browser_cookies: true,
            encoders: Mutex::new(HashMap::new()),
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn without_browser_cookies(mut self) -> Self {
        self.browser_cookies = false;
        self
    }

    pub fn begin(&self, payload: Value) -> Result<Arc<Job>, String> {
        let request: DownloadRequest = serde_json::from_value(payload)
            .map_err(|e| format!("Invalid download request: {e}"))?;
        request.validate()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Download state is unavailable.")?;
        if state.closing || state.installing {
            return Err("FinFetcher is closing or installing an update.".into());
        }
        if state.job.is_some() {
            return Err("A download is already running. Wait for it or cancel it first.".into());
        }
        let job = Arc::new(Job {
            id: uuid::Uuid::new_v4().to_string(),
            request,
            cancelled: AtomicBool::new(false),
            destination: Mutex::new((None, None)),
            wake: Condvar::new(),
        });
        state.job = Some(job.clone());
        Ok(job)
    }

    pub fn busy(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .job
            .is_some()
    }

    pub fn refresh_tools(&self) {
        if !self.busy() && !self.shutdown.load(Ordering::Acquire) {
            let _ = self.tools.update_ytdlp_cancellable(
                false,
                &crate::events::silent(),
                &self.shutdown,
            );
        }
    }

    pub fn install_tools(&self, sink: &EventSink) -> Result<(), String> {
        self.tools.install_ffmpeg_cancellable(sink, &self.shutdown)
    }

    pub fn reserve_install(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Download state is unavailable.")?;
        if state.job.is_some() {
            return Err("A download is still running. Wait for it to finish or cancel it before installing an update.".into());
        }
        if state.installing || state.closing {
            return Err("FinFetcher is already closing or installing an update.".into());
        }
        state.installing = true;
        Ok(())
    }

    pub fn release_install(&self) {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .installing = false;
    }

    pub fn cancel(&self) -> Value {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = &state.job {
            job.cancelled.store(true, Ordering::Release);
            job.wake.notify_all();
            json!({"success":true})
        } else {
            json!({"success":false,"error":"No download is running"})
        }
    }

    pub fn destination(&self, answer: Value) -> Result<Value, String> {
        let action = string(&answer, "action");
        if !matches!(action, "overwrite" | "folder" | "cancel") {
            return Err("Choose overwrite, another folder, or cancel.".into());
        }
        if action == "folder" && !Path::new(string(&answer, "path")).is_dir() {
            return Err("Choose an existing folder.".into());
        }
        let state = self
            .state
            .lock()
            .map_err(|_| "Download state is unavailable.")?;
        let job = state
            .job
            .as_ref()
            .ok_or("This download is no longer waiting for a destination.")?;
        let mut waiting = job
            .destination
            .lock()
            .map_err(|_| "The destination dialog is unavailable.")?;
        if waiting.0.as_deref() != answer["id"].as_str() || waiting.0.is_none() {
            return Err("This download is no longer waiting for a destination.".into());
        }
        waiting.0 = None;
        waiting.1 = Some(answer);
        job.wake.notify_all();
        Ok(json!({"success":true}))
    }

    pub fn close(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.cancel();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closing = true;
        let deadline = Instant::now() + Duration::from_secs(15);
        while state.job.is_some() && Instant::now() < deadline {
            state = self
                .finished
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    pub fn run(&self, job: Arc<Job>, sink: EventSink) {
        let sink = if job.request.log_to_file {
            let destination = job
                .request
                .save_path
                .clone()
                .unwrap_or_else(default_downloads);
            let _ = fs::create_dir_all(&destination);
            let file = Mutex::new(
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(destination.join("download_log.txt"))
                    .ok(),
            );
            Arc::new(move |event: Value| {
                if let Some(line) = event["log"].as_str().or_else(|| event["error"].as_str()) {
                    if let Some(file) = file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                        let _ = writeln!(file, "{line}");
                    }
                }
                sink(event);
            }) as EventSink
        } else {
            sink
        };
        sink(json!({"log":"> [FinFetcher] Preparing download..."}));
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_inner(&job, &sink)))
                .unwrap_or_else(|_| Err("The download stopped unexpectedly.".into()));
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state
                .job
                .as_ref()
                .is_some_and(|current| current.id == job.id)
            {
                state.job = None;
            }
            self.finished.notify_all();
        }
        match result {
            Ok(()) => sink(json!({"status":"completed"})),
            Err(error) if error == "Cancelled" || job.cancelled.load(Ordering::Acquire) => {
                sink(json!({"status":"cancelled"}))
            }
            Err(error) => {
                sink(json!({"error":error}));
                sink(json!({"status":"error"}));
            }
        }
    }

    fn runtime(&self, url: &str, sink: &EventSink, cancel: &AtomicBool) {
        if is_youtube(url) {
            if let Err(error) = self.tools.ensure_runtime_cancellable(sink, cancel) {
                sink(json!({"log":format!("> [FinFetcher] {error}")}));
            }
        }
    }

    fn tool_args(&self, args: &mut Vec<String>) {
        if let Some(ffmpeg) = self.tools.ffmpeg() {
            add(args, "--ffmpeg-location", &ffmpeg.to_string_lossy());
        }
        args.extend(self.tools.runtime_args());
        add(
            args,
            "--cache-dir",
            &self.tools.root().join("cache").to_string_lossy(),
        );
    }

    fn cookie_args(&self, executable: &Path, cancel: &AtomicBool) -> Vec<String> {
        if !self.browser_cookies {
            return Vec::new();
        }
        let mut cached = loop {
            if cancel.load(Ordering::Acquire) {
                return Vec::new();
            }
            match self.cookies.try_lock() {
                Ok(cached) => break cached,
                Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(25))
                }
            }
        };
        if cached.is_none() {
            let mut selected = None;
            for browser in installed_browsers() {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                let Ok(directory) = tempfile::Builder::new()
                    .prefix("cookies-")
                    .tempdir_in(self.tools.root())
                else {
                    break;
                };
                let path = directory.path().join("cookies.txt");
                let args = vec![
                    "--ignore-config".into(),
                    "--cookies-from-browser".into(),
                    browser.into(),
                    "--cookies".into(),
                    path.to_string_lossy().into_owned(),
                    "--list-impersonate-targets".into(),
                ];
                let result = process::run(
                    executable,
                    &args,
                    cancel,
                    Some(Duration::from_secs(10)),
                    |_, _| {},
                );
                if result.is_ok_and(|out| out.success)
                    && fs::read_to_string(&path).is_ok_and(|text| {
                        text.lines()
                            .any(|line| !line.starts_with('#') && line.split('\t').count() >= 7)
                    })
                {
                    selected = Some(directory);
                    break;
                }
            }
            if !cancel.load(Ordering::Acquire) {
                *cached = Some(selected);
            }
        }
        cached
            .as_ref()
            .and_then(Option::as_ref)
            .map(|dir| {
                vec![
                    "--cookies".into(),
                    dir.path()
                        .join("cookies.txt")
                        .to_string_lossy()
                        .into_owned(),
                ]
            })
            .unwrap_or_default()
    }

    pub fn inspect(&self, url: &str, stream: bool) -> Result<Value, String> {
        media::validate_url(url)?;
        let sink = crate::events::silent();
        let executable = self.tools.ensure_ytdlp_cancellable(&sink, &self.shutdown)?;
        self.runtime(url, &sink, &self.shutdown);
        let mut args = vec![
            "--ignore-config".into(),
            "--dump-single-json".into(),
            "--skip-download".into(),
            "--encoding".into(),
            "utf-8".into(),
            "--socket-timeout".into(),
            "20".into(),
        ];
        if stream {
            args.extend([
                "--no-playlist".into(),
                "--format".into(),
                "best[ext=mp4]/best".into(),
            ]);
        } else {
            args.push("--flat-playlist".into());
        }
        self.tool_args(&mut args);
        args.extend(self.cookie_args(&executable, &self.shutdown));
        args.extend(["--".into(), url.into()]);
        let info = extract(&executable, &args, &self.shutdown)?;
        if stream {
            let url = info["url"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("Could not find a stream with both video and audio.")?;
            media::validate_url(url)?;
            Ok(
                json!({"stream_url":url,"title":info["title"],"duration":info["duration"],"thumbnail":info["thumbnail"]}),
            )
        } else {
            Ok(media::metadata_response(&info))
        }
    }

    fn run_inner(&self, job: &Job, sink: &EventSink) -> Result<(), String> {
        job.check()?;
        let executable = self.tools.ensure_ytdlp_cancellable(sink, &job.cancelled)?;
        if enabled(&self.settings.get(), "auto_update_ytdlp") {
            if let Err(error) = self
                .tools
                .update_ytdlp_cancellable(false, sink, &job.cancelled)
            {
                sink(
                    json!({"log":format!("> [FinFetcher] Could not update yt-dlp: {error}. Using the available version.")}),
                );
            }
        }
        let executable = self
            .tools
            .ytdlp_cancellable(&job.cancelled)?
            .unwrap_or(executable);
        job.check()?;
        self.runtime(&job.request.url, sink, &job.cancelled);
        job.check()?;
        let settings = self.settings.get();
        let cookies = self.cookie_args(&executable, &job.cancelled);
        let mut urls = vec![(job.request.url.clone(), None)];
        if job.request.kind == "playlist" {
            let mut args = vec![
                "--ignore-config".into(),
                "--dump-single-json".into(),
                "--flat-playlist".into(),
                "--skip-download".into(),
                "--encoding".into(),
                "utf-8".into(),
            ];
            self.tool_args(&mut args);
            args.extend(cookies.clone());
            args.extend(["--".into(), job.request.url.clone()]);
            let info = extract(&executable, &args, &job.cancelled)?;
            if let Some(entries) = info["entries"].as_array() {
                urls = entries
                    .iter()
                    .filter(|entry| entry.is_object())
                    .filter_map(|entry| {
                        entry_url(entry)
                            .map(|url| (url, entry["formats"].is_array().then(|| entry.clone())))
                    })
                    .collect();
                if urls.is_empty() {
                    return Err("The playlist has no downloadable entries.".into());
                }
            }
        }
        let mut destination = job
            .request
            .save_path
            .clone()
            .unwrap_or_else(default_downloads);
        for (index, (url, info)) in urls.iter().enumerate() {
            job.check()?;
            if urls.len() > 1 {
                sink(
                    json!({"log":format!("[download] Downloading video {} of {}",index+1,urls.len())}),
                );
            }
            let mut request = job.request.clone();
            request.url = url.clone();
            let final_file = self.download_one(
                &executable,
                &request,
                info.as_ref(),
                &settings,
                &cookies,
                &mut destination,
                job,
                sink,
            )?;
            if request.pass_to_flipperclipper && request.mode == "video" {
                if let Some(path) = final_file {
                    let message = match integrations::open_clip(&path) {
                        Ok(()) => "Opened the video in FlipperClipper.".into(),
                        Err(error) => error,
                    };
                    sink(json!({"log":format!("> [FinFetcher] {message}")}));
                }
            }
        }
        Ok(())
    }

    fn download_one(
        &self,
        executable: &Path,
        request: &DownloadRequest,
        initial_info: Option<&Value>,
        settings: &Value,
        cookies: &[String],
        destination: &mut PathBuf,
        job: &Job,
        sink: &EventSink,
    ) -> Result<Option<PathBuf>, String> {
        let mut output = DownloadOutput::new(destination)?;
        let trimmed = request.trim()?.is_some();
        let mut fast = trimmed && enabled(settings, "fast_trim");
        let mut software = false;
        let mut forbidden_retries = 0;
        let use_archive = request.kind == "playlist" && enabled(settings, "use_download_archive");
        let archive_path = output.path().join(".finfetcher-archive.txt");
        let mut archive_destination = output.destination.clone();
        let mut archive_baseline = BTreeSet::new();
        if use_archive {
            archive_baseline = archive_lines(&destination.join(".finfetcher-archive.txt"))?;
            write_archive(&archive_path, &archive_baseline)?;
        }
        let final_file = loop {
            job.check()?;
            let mut metadata_args =
                media::options(request, settings, output.path(), None, false, "libx264")?;
            metadata_args.extend([
                "--dump-single-json".into(),
                "--simulate".into(),
                "--no-progress".into(),
            ]);
            self.tool_args(&mut metadata_args);
            metadata_args.extend_from_slice(cookies);
            if let Some(info) = initial_info.filter(|_| forbidden_retries == 0) {
                let path = output.path().join(".finfetcher-playlist.json");
                fs::write(&path, serde_json::to_vec(info).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                add(
                    &mut metadata_args,
                    "--load-info-json",
                    &path.to_string_lossy(),
                );
            } else {
                metadata_args.extend(["--".into(), request.url.clone()]);
            }
            let info = extract(executable, &metadata_args, &job.cancelled)?;
            if use_archive && archived(&info, &archive_path) {
                sink(json!({"log":"[download] This video is already in the download archive."}));
                return Ok(None);
            }
            let extension = if request.mode == "audio" {
                string(settings, "audio_format")
            } else {
                info["ext"]
                    .as_str()
                    .unwrap_or(string(settings, "container"))
            };
            let name = output_filename(&info)
                .and_then(|p| Path::new(p).file_name())
                .ok_or("yt-dlp did not provide an output filename.")?;
            let source_name = PathBuf::from(name);
            let final_name = if request.mode == "audio" {
                source_name.with_extension(extension)
            } else {
                source_name.clone()
            };
            output.prepare(&[source_name, final_name], &job.cancelled, |paths| {
                job.ask(paths, sink)
            })?;
            *destination = output.destination.clone();
            if use_archive && archive_destination != *destination {
                archive_baseline = archive_lines(&destination.join(".finfetcher-archive.txt"))?;
                write_archive(&archive_path, &archive_baseline)?;
                archive_destination = destination.clone();
                if archived(&info, &archive_path) {
                    sink(
                        json!({"log":"[download] This video is already in the download archive."}),
                    );
                    return Ok(None);
                }
            }
            let encoder = if !software
                && fast
                && enabled(settings, "precise_trim")
                && request.mode == "video"
                && extension != "webm"
            {
                self.encoder(&job.cancelled)
            } else {
                "libx264".into()
            };
            let info_path = output.path().join(".finfetcher-info.json");
            fs::write(
                &info_path,
                serde_json::to_vec(&info).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let mut args = media::options(
                request,
                settings,
                output.path(),
                Some(extension),
                fast,
                &encoder,
            )?;
            self.tool_args(&mut args);
            args.extend_from_slice(cookies);
            args.extend([
                "--no-simulate".into(),
                "--newline".into(),
                "--progress".into(),
                "--no-colors".into(),
                "--progress-delta".into(),
                "0.2".into(),
            ]);
            add(
                &mut args,
                "--progress-template",
                "download:FINFETCHER_PROGRESS:%(progress)j",
            );
            add(
                &mut args,
                "--progress-template",
                "postprocess:FINFETCHER_POSTPROCESS:%(progress)j",
            );
            add(
                &mut args,
                "--print",
                "after_move:FINFETCHER_FILE:%(filepath)j",
            );
            if use_archive {
                add(
                    &mut args,
                    "--download-archive",
                    &archive_path.to_string_lossy(),
                );
            }
            add(&mut args, "--load-info-json", &info_path.to_string_lossy());
            if fast {
                sink(
                    json!({"log":if enabled(settings,"precise_trim") {"> [FinFetcher] Fetching and precisely encoding the selected range."} else {"> [FinFetcher] Copying the selected range from the preceding keyframe."}}),
                );
            }
            let mut final_file = None;
            let result = process::run(executable, &args, &job.cancelled, None, |_, line| {
                if let Some(value) = line.strip_prefix("FINFETCHER_FILE:") {
                    if let Ok(path) = serde_json::from_str::<String>(value) {
                        final_file = Some(PathBuf::from(&path));
                        sink(json!({"log":format!("[postprocess] Final file: {path}")}));
                    }
                } else if let Some(value) = line.strip_prefix("FINFETCHER_PROGRESS:") {
                    if let Ok(progress) = serde_json::from_str::<Value>(value) {
                        sink(progress_event(&progress));
                    }
                } else if line.starts_with("FINFETCHER_POSTPROCESS:") {
                    sink(json!({"log":"[postprocess] Processing downloaded media..."}));
                } else if !line.is_empty() {
                    sink(json!({"log":line}));
                }
            })?;
            job.check()?;
            let usable = final_file
                .as_ref()
                .is_some_and(|path| self.usable(path, &job.cancelled));
            if result.success && usable {
                break final_file;
            }
            if result.success && final_file.is_none() && use_archive && output.files()?.is_empty() {
                sink(json!({"log":"[download] This video is already in the download archive."}));
                return Ok(None);
            }
            let error = if result.error.is_empty() {
                "The download did not produce usable media.".to_string()
            } else {
                result.error
            };
            if fast && encoder != "libx264" && !software {
                software = true;
                sink(
                    json!({"log":"> [FinFetcher] Hardware trim failed. Retrying with software encoding."}),
                );
            } else if fast && (media::partial_refusal(&error) || result.success && !usable) {
                fast = false;
                sink(
                    json!({"log":"> [FinFetcher] Fast trim did not work for this source. Downloading in full and trimming with FFmpeg instead."}),
                );
            } else if error.contains("403")
                && !self.tools.runtime_args().is_empty()
                && forbidden_retries < 2
            {
                forbidden_retries += 1;
                sink(
                    json!({"log":"> [FinFetcher] The source refused that session. Retrying with a fresh one..."}),
                );
            } else {
                return Err(error);
            }
            clear_attempt(output.path(), &archive_path)?;
        };
        if let (Some(file), Some(range)) = (final_file.as_ref(), request.trim()?) {
            if !fast {
                if let Err(error) =
                    self.trim(file, range, enabled(settings, "precise_trim"), job, sink)
                {
                    if error == "Cancelled" {
                        return Err(error);
                    }
                    return Err(output.preserve(&format!(
                        "Trimming failed: {error}. The full download was retained."
                    )));
                }
            }
        }
        job.check()?;
        let files = output.files()?;
        output.prepare(&files, &job.cancelled, |paths| job.ask(paths, sink))?;
        let published = output.publish(&job.cancelled)?;
        *destination = output.destination.clone();
        if use_archive && archive_path.is_file() {
            merge_archive(
                &destination.join(".finfetcher-archive.txt"),
                &archive_path,
                &archive_baseline,
            )?;
        }
        sink(json!({"log":format!("> [FinFetcher] Saved to: {}",destination.display())}));
        let final_file = final_file.and_then(|file| published.get(&file).cloned());
        if let Some(file) = &final_file {
            sink(json!({"file":file}));
        }
        Ok(final_file)
    }

    fn usable(&self, path: &Path, cancel: &AtomicBool) -> bool {
        if fs::metadata(path).map_or(true, |m| m.len() == 0) {
            return false;
        }
        let Some(probe) = self.tools.ffprobe() else {
            return true;
        };
        let args = vec![
            "-v".into(),
            "error".into(),
            "-count_packets".into(),
            "-read_intervals".into(),
            "%+1".into(),
            "-show_entries".into(),
            "stream=codec_type,nb_read_packets".into(),
            "-of".into(),
            "json".into(),
            path.to_string_lossy().into_owned(),
        ];
        process::run(
            &probe,
            &args,
            cancel,
            Some(Duration::from_secs(15)),
            |_, _| {},
        )
        .ok()
        .filter(|r| r.success)
        .and_then(|r| serde_json::from_str::<Value>(&r.stdout).ok())
        .is_some_and(|value| {
            value["streams"].as_array().is_some_and(|v| {
                v.iter().any(|s| {
                    matches!(string(s, "codec_type"), "video" | "audio")
                        && string(s, "nb_read_packets")
                            .parse::<u64>()
                            .is_ok_and(|n| n > 0)
                })
            })
        })
    }

    fn encoder(&self, cancel: &AtomicBool) -> String {
        let Some(ffmpeg) = self.tools.ffmpeg() else {
            return "libx264".into();
        };
        let mut cache = self.encoders.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(value) = cache.get(&ffmpeg) {
            return value.clone();
        }
        let encoder = ["h264_nvenc", "h264_qsv", "h264_amf"]
            .into_iter()
            .find(|encoder| {
                let args = [
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "nullsrc=s=256x144",
                    "-frames:v",
                    "1",
                    "-c:v",
                    encoder,
                    "-f",
                    "null",
                    "-",
                ]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>();
                process::run(
                    &ffmpeg,
                    &args,
                    cancel,
                    Some(Duration::from_secs(5)),
                    |_, _| {},
                )
                .is_ok_and(|r| r.success)
            })
            .unwrap_or("libx264")
            .to_string();
        cache.insert(ffmpeg, encoder.clone());
        encoder
    }

    fn trim(
        &self,
        file: &Path,
        range: (f64, f64),
        precise: bool,
        job: &Job,
        sink: &EventSink,
    ) -> Result<(), String> {
        let ffmpeg = self
            .tools
            .ffmpeg()
            .ok_or("FFmpeg is required for trimming.")?;
        let extension = file.extension().and_then(|s| s.to_str()).unwrap_or("mp4");
        let target = file.with_file_name(format!(
            ".finfetcher-trim-{}.{}",
            uuid::Uuid::new_v4(),
            extension
        ));
        let mut encoder = if precise
            && !matches!(
                extension,
                "webm" | "mp3" | "wav" | "flac" | "m4a" | "opus" | "ogg"
            ) {
            self.encoder(&job.cancelled)
        } else {
            "libx264".into()
        };
        loop {
            let result = process::run(
                &ffmpeg,
                &media::trim_args(file, &target, range, precise, &encoder),
                &job.cancelled,
                None,
                |_, line| sink(json!({"log":format!("[ffmpeg] {line}")})),
            )?;
            job.check()?;
            if result.success && self.usable(&target, &job.cancelled) {
                fs::rename(&target, file).map_err(|e| e.to_string())?;
                return Ok(());
            }
            if encoder == "libx264" {
                return Err(result.error);
            }
            encoder = "libx264".into();
            sink(
                json!({"log":"> [FinFetcher] Hardware trim failed. Retrying with software encoding."}),
            );
        }
    }
}

fn extract(executable: &Path, args: &[String], cancel: &AtomicBool) -> Result<Value, String> {
    let output = process::run(
        executable,
        args,
        cancel,
        Some(Duration::from_secs(45)),
        |_, _| {},
    )?;
    if !output.success {
        return Err(if output.error.is_empty() {
            "Could not inspect the video.".into()
        } else {
            output.error
        });
    }
    output
        .stdout
        .lines()
        .rev()
        .find_map(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .filter(Value::is_object)
        })
        .ok_or_else(|| "yt-dlp did not return video information.".into())
}

fn output_filename(info: &Value) -> Option<&str> {
    info["_filename"]
        .as_str()
        .or_else(|| info["filename"].as_str())
        .or_else(|| {
            info["requested_downloads"]
                .as_array()?
                .iter()
                .find_map(|download| {
                    download["_filename"]
                        .as_str()
                        .or_else(|| download["filename"].as_str())
                })
        })
}

fn archived(info: &Value, path: &Path) -> bool {
    let Some(id) = info["id"].as_str() else {
        return false;
    };
    let Some(extractor) = info["extractor_key"]
        .as_str()
        .or_else(|| info["ie_key"].as_str())
    else {
        return false;
    };
    let key = format!("{} {id}", extractor.to_lowercase());
    fs::read_to_string(path).is_ok_and(|archive| {
        archive.lines().any(|line| {
            line.trim() == key
                || info["_old_archive_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(line.trim())))
        })
    })
}

fn archive_lines(path: &Path) -> Result<BTreeSet<String>, String> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(error) => Err(format!("Could not read the download archive: {error}")),
    }
}

fn write_archive(path: &Path, lines: &BTreeSet<String>) -> Result<(), String> {
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().ok_or("Invalid archive path")?)
            .map_err(|e| e.to_string())?;
    for line in lines {
        writeln!(temporary, "{line}").map_err(|e| e.to_string())?;
    }
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn merge_archive(target: &Path, staged: &Path, baseline: &BTreeSet<String>) -> Result<(), String> {
    let mut existing = archive_lines(target)?;
    existing.extend(archive_lines(staged)?.difference(baseline).cloned());
    write_archive(target, &existing)
}

fn clear_attempt(directory: &Path, archive: &Path) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path() == archive {
            continue;
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            fs::remove_dir_all(entry.path()).map_err(|e| e.to_string())?;
        } else {
            fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn entry_url(entry: &Value) -> Option<String> {
    for key in ["url", "webpage_url", "original_url"] {
        if let Some(url) = entry[key]
            .as_str()
            .filter(|url| media::validate_url(url).is_ok())
        {
            return Some(url.into());
        }
    }
    if string(entry, "ie_key").eq_ignore_ascii_case("youtube") {
        return entry["id"]
            .as_str()
            .map(|id| format!("https://www.youtube.com/watch?v={id}"));
    }
    None
}

pub fn default_downloads() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Downloads")
}

fn is_youtube(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|host| {
            host == "youtu.be" || host == "youtube.com" || host.ends_with(".youtube.com")
        })
}

fn installed_browsers() -> Vec<&'static str> {
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from);
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    [
        ("firefox", roaming.as_ref(), "Mozilla/Firefox/Profiles"),
        ("chrome", local.as_ref(), "Google/Chrome/User Data"),
        ("edge", local.as_ref(), "Microsoft/Edge/User Data"),
        (
            "brave",
            local.as_ref(),
            "BraveSoftware/Brave-Browser/User Data",
        ),
        ("opera", roaming.as_ref(), "Opera Software"),
    ]
    .into_iter()
    .filter(|(_, root, path)| root.is_some_and(|root| root.join(path).is_dir()))
    .map(|(name, _, _)| name)
    .collect()
}

fn progress_event(progress: &Value) -> Value {
    let downloaded = progress["downloaded_bytes"].as_u64().unwrap_or(0);
    let total = progress["total_bytes"]
        .as_u64()
        .or_else(|| progress["total_bytes_estimate"].as_u64());
    let percent = total
        .filter(|n| *n > 0)
        .map(|total| downloaded as f64 * 100.0 / total as f64);
    let speed = progress["speed"]
        .as_f64()
        .map(|n| media::format_size(Some(n.max(0.0) as u64)))
        .unwrap_or_else(|| "Unknown".into());
    let eta = progress["eta"]
        .as_u64()
        .map(|n| format!("{}:{:02}", n / 60, n % 60))
        .unwrap_or_else(|| "?".into());
    json!({"log":format!("[download] {} of {} at {speed}/s ETA {eta}",percent.map(|n|format!("{n:.1}%")).unwrap_or_else(||"?".into()),media::format_size(total)),"progress":progress,"percent":percent})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn engine(root: &Path) -> Downloads {
        let settings = Arc::new(Settings::new(root.to_owned()).unwrap());
        let tools = Arc::new(Tools::new(root.to_owned(), settings.clone()));
        Downloads::new(tools, settings).without_browser_cookies()
    }

    #[test]
    fn reserves_before_spawn_and_blocks_updates_until_job_finishes() {
        let root = tempfile::tempdir().unwrap();
        let engine = engine(root.path());
        let job = engine
            .begin(json!({"url":"https://example.com/video"}))
            .unwrap();
        assert!(engine
            .begin(json!({"url":"https://example.com/another"}))
            .is_err());
        assert!(engine.reserve_install().is_err());
        assert!(engine.busy());
        engine.cancel();
        let events = Arc::new(Mutex::new(Vec::new()));
        let capture = events.clone();
        engine.run(
            job,
            Arc::new(move |event| capture.lock().unwrap().push(event)),
        );
        assert!(!engine.busy());
        assert!(engine.reserve_install().is_ok());
        assert!(engine
            .begin(json!({"url":"https://example.com/another"}))
            .is_err());
        assert_eq!(
            events.lock().unwrap().last().unwrap()["status"],
            "cancelled"
        );
    }

    #[test]
    fn stale_destination_answer_does_not_release_current_prompt() {
        let root = tempfile::tempdir().unwrap();
        let engine = engine(root.path());
        let job = engine
            .begin(json!({"url":"https://example.com/video"}))
            .unwrap();
        *job.destination.lock().unwrap() = (Some("new-prompt".into()), None);
        assert!(engine
            .destination(json!({"id":"old-prompt","action":"overwrite"}))
            .is_err());
        assert_eq!(
            job.destination.lock().unwrap().0.as_deref(),
            Some("new-prompt")
        );
        assert_eq!(
            engine
                .destination(json!({"id":"new-prompt","action":"cancel"}))
                .unwrap()["success"],
            true
        );
    }

    #[test]
    fn progress_is_parsed_from_machine_fields() {
        let event = progress_event(
            &json!({"downloaded_bytes":512,"total_bytes":1024,"speed":1024,"eta":3}),
        );
        assert_eq!(event["percent"], 50.0);
        assert!(event["log"].as_str().unwrap().contains("50.0%"));
    }

    #[test]
    fn filenames_support_current_and_older_ytdlp_metadata() {
        for info in [
            json!({"_filename":"clip.mp4"}),
            json!({"filename":"clip.mp4"}),
            json!({"requested_downloads":[{"_filename":"clip.mp4"}]}),
            json!({"requested_downloads":[{"filename":"clip.mp4"}]}),
        ] {
            assert_eq!(output_filename(&info), Some("clip.mp4"));
        }
        assert_eq!(output_filename(&json!({"requested_downloads":[]})), None);
        assert_eq!(output_filename(&json!({"title":"clip","ext":"mp4"})), None);
    }

    #[test]
    fn archive_matches_extractor_and_id_without_confusing_other_videos() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("archive.txt");
        fs::write(&path, "youtube abc\ngeneric xyz\n").unwrap();
        assert!(archived(
            &json!({"extractor_key":"Youtube","id":"abc"}),
            &path
        ));
        assert!(!archived(
            &json!({"extractor_key":"Generic","id":"abc"}),
            &path
        ));
        assert!(!archived(
            &json!({"extractor_key":"Youtube","id":"ab"}),
            &path
        ));
    }

    #[test]
    fn playlist_entries_prefer_their_media_over_the_parent_page() {
        assert_eq!(entry_url(&json!({"url":"https://example.com/first.mp4","webpage_url":"https://example.com/playlist"})).as_deref(),Some("https://example.com/first.mp4"));
        assert_eq!(
            entry_url(&json!({"url":"abc","ie_key":"Youtube","id":"abc"})).as_deref(),
            Some("https://www.youtube.com/watch?v=abc")
        );
    }

    #[test]
    fn relocated_archive_keeps_destination_history_without_copying_source_history() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.txt");
        let stage = directory.path().join("staged.txt");
        fs::write(&target, "youtube target-old\n").unwrap();
        fs::write(&stage, "youtube source-old\nyoutube current\n").unwrap();
        let baseline = BTreeSet::from(["youtube source-old".to_string()]);
        merge_archive(&target, &stage, &baseline).unwrap();
        assert_eq!(
            archive_lines(&target).unwrap(),
            BTreeSet::from([
                "youtube target-old".to_string(),
                "youtube current".to_string()
            ])
        );
    }
}
