use crate::events::EventSink;
use crate::settings::{atomic_json, Settings};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant, SystemTime};

const FFMPEG_URL: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
const MAX_DOWNLOAD: u64 = 768 * 1024 * 1024;
const MAX_BINARY: u64 = 512 * 1024 * 1024;
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);

#[derive(Clone)]
struct CachedBinary {
    length: u64,
    modified: Option<SystemTime>,
    version: String,
}

pub struct Tools {
    root: PathBuf,
    settings: Arc<Settings>,
    versions: Mutex<HashMap<PathBuf, CachedBinary>>,
    install: Mutex<()>,
    runtime_attempt: Mutex<Option<Result<(), String>>>,
    ytdlp_check: Mutex<Option<Instant>>,
    last_result: Mutex<Value>,
    discovery_override: Option<Vec<PathBuf>>,
}

impl Tools {
    pub fn new(root: PathBuf, settings: Arc<Settings>) -> Self {
        Self {
            root,
            settings,
            versions: Mutex::new(HashMap::new()),
            install: Mutex::new(()),
            runtime_attempt: Mutex::new(None),
            ytdlp_check: Mutex::new(None),
            last_result: Mutex::new(json!({})),
            discovery_override: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn settings(&self) -> &Arc<Settings> {
        &self.settings
    }

    pub fn status(&self) -> Value {
        let ffmpeg = self.ffmpeg();
        let ffprobe = self.ffprobe();
        json!({
            "installed": ffmpeg.is_some() && ffprobe.is_some(),
            "path": ffmpeg.as_ref().and_then(|p| p.parent()),
            "ffmpeg": ffmpeg, "ffprobe": ffprobe,
            "custom_path": self.settings.raw().get("ffmpeg_path"),
        })
    }

    pub fn set_custom_path(&self, path: &Path) -> Result<(), String> {
        let directory = if path.is_file() {
            path.parent().ok_or("Invalid FFmpeg path")?
        } else {
            path
        };
        for name in ["ffmpeg", "ffprobe"] {
            if self
                .validated(&directory.join(executable_name(name)), name)
                .is_none()
            {
                return Err(format!(
                    "The selected folder must contain a working {}",
                    executable_name(name)
                ));
            }
        }
        self.settings.merge_raw(&json!({"ffmpeg_path":directory}))
    }

    pub fn ffmpeg(&self) -> Option<PathBuf> {
        self.resolve_ffmpeg("ffmpeg")
    }

    pub fn ffprobe(&self) -> Option<PathBuf> {
        self.resolve_ffmpeg("ffprobe")
    }

    fn resolve_ffmpeg(&self, name: &str) -> Option<PathBuf> {
        let config = self.settings.raw();
        let custom = config
            .get("ffmpeg_path")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        let dirs = custom
            .into_iter()
            .chain([self.root.join("ffmpeg")])
            .chain(self.search_dirs());
        for directory in dedupe_dirs(dirs) {
            let ffmpeg = self.find_in_dirs("ffmpeg", [directory.clone()]);
            let ffprobe = self.find_in_dirs("ffprobe", [directory]);
            if ffmpeg.is_some() && ffprobe.is_some() {
                return if name == "ffmpeg" { ffmpeg } else { ffprobe };
            }
        }
        None
    }

    pub fn ytdlp(&self) -> Option<PathBuf> {
        self.ytdlp_cancellable(&AtomicBool::new(false))
            .ok()
            .flatten()
    }

    pub fn ytdlp_cancellable(&self, cancel: &AtomicBool) -> Result<Option<PathBuf>, String> {
        check_cancel(cancel)?;
        let managed = self.root.join("ytdlp-bin");
        let active = read_json(&managed.join("active.json"));
        let mut candidates = Vec::new();
        for key in ["directory", "previous"] {
            if let Some(name) = active
                .get(key)
                .and_then(Value::as_str)
                .filter(|name| safe_component(name))
            {
                candidates.push(managed.join(name).join(executable_name("yt-dlp")));
            }
        }
        candidates.extend([
            self.root.join("bin").join(executable_name("yt-dlp")),
            self.root.join(executable_name("yt-dlp")),
        ]);
        if self.discovery_override.is_none() {
            if let Ok(exe) = std::env::current_exe() {
                if let Some(directory) = exe.parent() {
                    candidates.push(directory.join("tools").join(executable_name("yt-dlp")));
                    candidates.push(directory.join(executable_name("yt-dlp")));
                }
            }
        }
        candidates.extend(
            self.search_dirs()
                .into_iter()
                .map(|dir| dir.join(executable_name("yt-dlp"))),
        );
        newest_ytdlp(candidates, |path| {
            self.validated_cancellable(path, "yt-dlp", cancel)
        })
    }

    pub fn runtime_args(&self) -> Vec<String> {
        self.runtimes()
            .into_iter()
            .flat_map(|(name, path)| {
                [
                    "--js-runtimes".to_owned(),
                    format!("{name}:{}", path.display()),
                ]
            })
            .collect()
    }

    fn runtimes(&self) -> Vec<(&'static str, PathBuf)> {
        self.runtimes_cancellable(&AtomicBool::new(false))
            .unwrap_or_default()
    }

    fn runtimes_cancellable(
        &self,
        cancel: &AtomicBool,
    ) -> Result<Vec<(&'static str, PathBuf)>, String> {
        let dirs = self.search_dirs();
        let mut runtimes = Vec::new();
        for name in ["deno", "node", "bun"] {
            let managed = if name == "deno" {
                vec![self.root.join("deno")]
            } else {
                Vec::new()
            };
            if let Some(path) = self.find_in_dirs_cancellable(
                name,
                managed.into_iter().chain(dirs.clone()),
                cancel,
            )? {
                runtimes.push((name, path));
            }
        }
        Ok(runtimes)
    }

    fn search_dirs(&self) -> Vec<PathBuf> {
        self.discovery_override.clone().unwrap_or_else(search_dirs)
    }

    fn find_in_dirs(
        &self,
        name: &str,
        directories: impl IntoIterator<Item = PathBuf>,
    ) -> Option<PathBuf> {
        self.find_in_dirs_cancellable(name, directories, &AtomicBool::new(false))
            .ok()
            .flatten()
    }

    fn find_in_dirs_cancellable(
        &self,
        name: &str,
        directories: impl IntoIterator<Item = PathBuf>,
        cancel: &AtomicBool,
    ) -> Result<Option<PathBuf>, String> {
        for dir in dedupe_dirs(directories) {
            let path = dir.join(executable_name(name));
            if self.validated_cancellable(&path, name, cancel)?.is_some() {
                return Ok(Some(path));
            }
            if let Ok(target) = fs::read_link(&path) {
                let target = if target.is_absolute() {
                    target
                } else {
                    dir.join(target)
                };
                if self.validated_cancellable(&target, name, cancel)?.is_some() {
                    return Ok(Some(target));
                }
            }
        }
        Ok(None)
    }

    fn validated(&self, path: &Path, name: &str) -> Option<String> {
        self.validated_cancellable(path, name, &AtomicBool::new(false))
            .ok()
            .flatten()
    }

    fn validated_cancellable(
        &self,
        path: &Path,
        name: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<String>, String> {
        check_cancel(cancel)?;
        let Ok(metadata) = fs::metadata(path) else {
            return Ok(None);
        };
        if !metadata.is_file() || metadata.len() == 0 {
            return Ok(None);
        }
        if let Some(cached) = cancellable_lock(&self.versions, cancel)?.get(path) {
            if cached.length == metadata.len() && cached.modified == metadata.modified().ok() {
                return Ok(Some(cached.version.clone()));
            }
        }
        let version = match probe_version_cancellable(path, name, cancel) {
            Ok(version) => version,
            Err(error) if error == "Cancelled" => return Err(error),
            Err(_) => return Ok(None),
        };
        cancellable_lock(&self.versions, cancel)?.insert(
            path.to_owned(),
            CachedBinary {
                length: metadata.len(),
                modified: metadata.modified().ok(),
                version: version.clone(),
            },
        );
        Ok(Some(version))
    }

    pub fn ensure_ytdlp(&self, sink: &EventSink) -> Result<PathBuf, String> {
        self.ensure_ytdlp_cancellable(sink, &AtomicBool::new(false))
    }

    pub fn ensure_ytdlp_cancellable(
        &self,
        sink: &EventSink,
        cancel: &AtomicBool,
    ) -> Result<PathBuf, String> {
        if let Some(path) = self.ytdlp_cancellable(cancel)? {
            return Ok(path);
        }
        self.update_ytdlp_cancellable(true, sink, cancel)?;
        self.ytdlp_cancellable(cancel)?
            .ok_or_else(|| "yt-dlp could not be started after installation".to_owned())
    }

    pub fn update_ytdlp(&self, force: bool, sink: &EventSink) -> Result<Value, String> {
        self.update_ytdlp_cancellable(force, sink, &AtomicBool::new(false))
    }

    pub fn update_ytdlp_cancellable(
        &self,
        force: bool,
        sink: &EventSink,
        cancel: &AtomicBool,
    ) -> Result<Value, String> {
        let _install = cancellable_lock(&self.install, cancel)?;
        if !force
            && !self.settings.get()["auto_update_ytdlp"]
                .as_bool()
                .unwrap_or(true)
        {
            return Ok(json!({"checked":false,"reason":"disabled"}));
        }
        {
            let mut last = cancellable_lock(&self.ytdlp_check, cancel)?;
            if !force && last.is_some_and(|last| last.elapsed() < CHECK_INTERVAL) {
                return Ok(json!({"checked":false,"reason":"interval"}));
            }
            *last = Some(Instant::now());
        }
        let result = self.install_ytdlp(sink, cancel);
        if result.as_ref().is_err_and(|error| error == "Cancelled") {
            if let Ok(mut last) = self.ytdlp_check.lock() {
                *last = None;
            }
            return result;
        }
        if let Ok(mut last) = self.last_result.lock() {
            *last = match &result {
                Ok(value) => value.clone(),
                Err(error) => json!({"checked":true,"updated":false,"error":error}),
            };
        }
        result
    }

    fn install_ytdlp(&self, sink: &EventSink, cancel: &AtomicBool) -> Result<Value, String> {
        check_cancel(cancel)?;
        progress(sink, "yt-dlp", 0, "Checking yt-dlp for updates...");
        let client = client()?;
        let release = github_release(&client, "yt-dlp/yt-dlp", cancel)?;
        let version = release["tag_name"]
            .as_str()
            .ok_or("yt-dlp did not return a version")?;
        if date_version(version).is_none() {
            return Err("yt-dlp returned an invalid version".to_owned());
        }
        let running = match self.ytdlp_cancellable(cancel)? {
            Some(path) => self.validated_cancellable(&path, "yt-dlp", cancel)?,
            None => None,
        };
        if running
            .as_ref()
            .is_some_and(|current| !is_newer(version, current))
        {
            return Ok(json!({"checked":true,"updated":false,"running":running,"latest":version}));
        }
        let filename = ytdlp_asset();
        let asset = github_asset(&release, filename, "yt-dlp/yt-dlp")?;
        let sums = github_asset(&release, "SHA2-256SUMS", "yt-dlp/yt-dlp")?;
        let checksum = checksum_for(&get_text(&client, sums.url()?, cancel)?, filename)
            .ok_or("The yt-dlp release has no checksum for this platform")?;
        let managed = self.root.join("ytdlp-bin");
        fs::create_dir_all(&managed).map_err(|e| e.to_string())?;
        let stage = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&managed)
            .map_err(|e| e.to_string())?;
        let executable = stage.path().join(executable_name("yt-dlp"));
        download(
            &client,
            asset.url()?,
            &executable,
            asset.size(),
            &checksum,
            sink,
            "yt-dlp",
            cancel,
        )?;
        executable_permissions(&executable)?;
        let installed = probe_version_cancellable(&executable, "yt-dlp", cancel)?;
        if date_version(&installed) != date_version(version) {
            return Err(format!(
                "The downloaded yt-dlp reported {installed}, expected {version}"
            ));
        }
        let directory = format!("{}-{}", version, uuid::Uuid::new_v4());
        let destination = managed.join(&directory);
        check_cancel(cancel)?;
        rename_when_free_cancellable(stage.path(), &destination, cancel)?;
        let previous = read_json(&managed.join("active.json"));
        atomic_json(
            &managed.join("active.json"),
            &json!({
                "version":version, "directory":directory,
                "previous":previous.get("directory").and_then(Value::as_str).filter(|s| safe_component(s)),
            }),
        )?;
        progress(sink, "yt-dlp", 100, "yt-dlp is ready.");
        Ok(json!({"checked":true,"updated":true,"running":running,"installed":version}))
    }

    pub fn ensure_runtime(&self, sink: &EventSink) -> Result<(), String> {
        self.ensure_runtime_cancellable(sink, &AtomicBool::new(false))
    }

    pub fn ensure_runtime_cancellable(
        &self,
        sink: &EventSink,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        self.ensure_runtime_with(cancel, || self.install_deno(sink, cancel))
    }

    fn ensure_runtime_with(
        &self,
        cancel: &AtomicBool,
        install: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if !self.runtimes_cancellable(cancel)?.is_empty() {
            return Ok(());
        }
        let mut attempt = cancellable_lock(&self.runtime_attempt, cancel)?;
        if let Some(result) = &*attempt {
            return result.clone();
        }
        let _install = cancellable_lock(&self.install, cancel)?;
        let result = install();
        if !result.as_ref().is_err_and(|error| error == "Cancelled") {
            *attempt = Some(result.clone());
        }
        result
    }

    fn install_deno(&self, sink: &EventSink, cancel: &AtomicBool) -> Result<(), String> {
        check_cancel(cancel)?;
        progress(sink, "deno", 0, "Installing Deno for YouTube downloads...");
        let client = client()?;
        let release = github_release(&client, "denoland/deno", cancel)?;
        let filename = deno_asset();
        let asset = github_asset(&release, filename, "denoland/deno")?;
        let checksum = match asset.digest() {
            Some(digest) => digest,
            None => {
                let sums =
                    github_asset(&release, &format!("{filename}.sha256sum"), "denoland/deno")?;
                checksum_for(&get_text(&client, sums.url()?, cancel)?, filename)
                    .ok_or("The Deno release has no checksum for this platform")?
            }
        };
        fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let stage = tempfile::Builder::new()
            .prefix(".deno-staging-")
            .tempdir_in(&self.root)
            .map_err(|e| e.to_string())?;
        let archive = stage.path().join("deno.zip");
        download(
            &client,
            asset.url()?,
            &archive,
            asset.size(),
            &checksum,
            sink,
            "deno",
            cancel,
        )?;
        let staged = extract_named_cancellable(&archive, stage.path(), &["deno"], cancel)?;
        for (name, path) in &staged {
            probe_version_cancellable(path, name, cancel)?;
        }
        let release_version = release["tag_name"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches('v');
        let actual = probe_version_cancellable(&staged[0].1, "deno", cancel)?;
        if actual.split_whitespace().nth(1) != Some(release_version) {
            return Err("The downloaded Deno version does not match its release".to_owned());
        }
        check_cancel(cancel)?;
        publish_files_cancellable(&self.root.join("deno"), &staged, cancel)?;
        progress(sink, "deno", 100, "Deno is ready.");
        Ok(())
    }

    pub fn install_ffmpeg(&self, sink: &EventSink) -> Result<(), String> {
        self.install_ffmpeg_cancellable(sink, &AtomicBool::new(false))
    }

    pub fn install_ffmpeg_cancellable(
        &self,
        sink: &EventSink,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let _install = cancellable_lock(&self.install, cancel)?;
        if !cfg!(windows) {
            return Err(
                "Install FFmpeg with your system package manager on this platform".to_owned(),
            );
        }
        progress(
            sink,
            "ffmpeg",
            0,
            "Connecting to the FFmpeg download server...",
        );
        let client = client()?;
        let checksum_text = get_text(&client, &format!("{FFMPEG_URL}.sha256"), cancel)?;
        let checksum = checksum_for(&checksum_text, "ffmpeg-release-essentials.zip")
            .ok_or("The FFmpeg server did not return a valid checksum")?;
        fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let stage = tempfile::Builder::new()
            .prefix(".ffmpeg-staging-")
            .tempdir_in(&self.root)
            .map_err(|e| e.to_string())?;
        let archive = stage.path().join("ffmpeg.zip");
        download(
            &client, FFMPEG_URL, &archive, None, &checksum, sink, "ffmpeg", cancel,
        )?;
        progress(sink, "ffmpeg", 85, "Extracting FFmpeg and FFprobe...");
        let staged =
            extract_named_cancellable(&archive, stage.path(), &["ffmpeg", "ffprobe"], cancel)?;
        for (name, path) in &staged {
            probe_version_cancellable(path, name, cancel)?;
        }
        check_cancel(cancel)?;
        publish_files_cancellable(&self.root.join("ffmpeg"), &staged, cancel)?;
        progress(sink, "ffmpeg", 100, "FFmpeg is installed.");
        Ok(())
    }

    pub fn diagnostics(&self) -> Value {
        let ffmpeg = self.ffmpeg();
        let ffprobe = self.ffprobe();
        let ytdlp = self.ytdlp();
        let describe = |path: Option<PathBuf>, name: &str| json!({"version":path.as_ref().and_then(|p| self.validated(p, name)),"path":path});
        json!({
            "ffmpeg":describe(ffmpeg,"ffmpeg"), "ffprobe":describe(ffprobe,"ffprobe"),
            "yt-dlp":describe(ytdlp,"yt-dlp"),
            "runtimes":self.runtimes().into_iter().map(|(name,path)| json!({
                "name":name,"version":self.validated(&path,name),"path":path
            })).collect::<Vec<_>>(),
            "yt-dlp update":self.last_result.lock().map(|v| v.clone()).unwrap_or(Value::Null),
            "certificate_store":"Mozilla root certificates (rustls)",
        })
    }
}

fn executable_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn ytdlp_asset() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "aarch64") => "yt-dlp_arm64.exe",
        ("windows", "x86") => "yt-dlp_x86.exe",
        ("windows", _) => "yt-dlp.exe",
        ("macos", _) => "yt-dlp_macos",
        (_, "aarch64") => "yt-dlp_linux_aarch64",
        _ => "yt-dlp_linux",
    }
}

fn deno_asset() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "aarch64") => "deno-aarch64-pc-windows-msvc.zip",
        ("windows", _) => "deno-x86_64-pc-windows-msvc.zip",
        ("macos", "aarch64") => "deno-aarch64-apple-darwin.zip",
        ("macos", _) => "deno-x86_64-apple-darwin.zip",
        (_, "aarch64") => "deno-aarch64-unknown-linux-gnu.zip",
        _ => "deno-x86_64-unknown-linux-gnu.zip",
    }
}

fn progress(sink: &EventSink, tool: &str, percent: u64, text: &str) {
    sink(
        json!({"tool":tool,"percent":percent,"progress":percent,"status":text,"log":format!("> [FinFetcher] {text}")}),
    );
}

fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent(format!("FinFetcher/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(1800))
        .build()
        .map_err(|e| format!("Could not initialize downloads: {e}"))
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Cancelled".to_owned())
    } else {
        Ok(())
    }
}

fn cancellable_lock<'a, T>(
    mutex: &'a Mutex<T>,
    cancel: &AtomicBool,
) -> Result<MutexGuard<'a, T>, String> {
    loop {
        check_cancel(cancel)?;
        match mutex.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(_)) => {
                return Err("Tool installation state is unavailable".to_owned())
            }
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

fn network<T>(
    cancel: &AtomicBool,
    work: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    check_cancel(cancel)?;
    tauri::async_runtime::block_on(async {
        tokio::select! {
            biased;
            _ = async {
                while !cancel.load(Ordering::Acquire) { tokio::time::sleep(Duration::from_millis(25)).await; }
            } => Err("Cancelled".to_owned()),
            result = work => result,
        }
    })
}

fn get_text(client: &Client, url: &str, cancel: &AtomicBool) -> Result<String, String> {
    network(cancel, async {
        let mut response = client
            .get(url)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| format!("Could not check {url}: {e}"))?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            check_cancel(cancel)?;
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err("Tool metadata exceeds the permitted size".to_owned());
            }
            bytes.extend_from_slice(&chunk);
        }
        String::from_utf8(bytes).map_err(|e| e.to_string())
    })
}

fn github_release(client: &Client, repository: &str, cancel: &AtomicBool) -> Result<Value, String> {
    serde_json::from_str(&get_text(
        client,
        &format!("https://api.github.com/repos/{repository}/releases/latest"),
        cancel,
    )?)
    .map_err(|e| format!("Invalid release response for {repository}: {e}"))
}

struct Asset<'a>(&'a Value);

impl Asset<'_> {
    fn url(&self) -> Result<&str, String> {
        self.0["browser_download_url"]
            .as_str()
            .ok_or_else(|| "Release download URL is missing".to_owned())
    }
    fn size(&self) -> Option<u64> {
        self.0["size"].as_u64()
    }
    fn digest(&self) -> Option<String> {
        self.0["digest"]
            .as_str()?
            .strip_prefix("sha256:")
            .filter(|s| valid_hash(s))
            .map(str::to_owned)
    }
}

fn github_asset<'a>(release: &'a Value, name: &str, repository: &str) -> Result<Asset<'a>, String> {
    let asset = release["assets"]
        .as_array()
        .and_then(|assets| assets.iter().find(|a| a["name"] == name))
        .ok_or_else(|| format!("The release does not contain {name}"))?;
    let asset = Asset(asset);
    if !asset.url()?.starts_with(&format!(
        "https://github.com/{repository}/releases/download/"
    )) {
        return Err("The tool release returned an unexpected download host".to_owned());
    }
    Ok(asset)
}

fn download(
    client: &Client,
    url: &str,
    dest: &Path,
    expected: Option<u64>,
    checksum: &str,
    sink: &EventSink,
    tool: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    network(cancel, async {
        let mut response = client
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| format!("Could not download {tool}: {e}"))?;
        let expected = expected.or(response.content_length());
        if expected.is_some_and(|size| size > MAX_DOWNLOAD) {
            return Err("The tool download is larger than expected".to_owned());
        }
        let parent = dest
            .parent()
            .ok_or("Tool download has no destination folder")?;
        let mut file = tempfile::Builder::new()
            .prefix(".download-")
            .tempfile_in(parent)
            .map_err(|e| format!("Could not write {tool}: {e}"))?;
        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        let mut last = Instant::now();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("The {tool} download stopped early: {e}"))?
        {
            check_cancel(cancel)?;
            total += chunk.len() as u64;
            if total > MAX_DOWNLOAD {
                return Err("The tool download is larger than expected".to_owned());
            }
            file.write_all(&chunk)
                .map_err(|e| format!("Could not write {tool}: {e}"))?;
            hasher.update(&chunk);
            if last.elapsed() >= Duration::from_millis(100) {
                let percent = expected
                    .filter(|size| *size > 0)
                    .map(|size| (total * 80 / size).min(80))
                    .unwrap_or(0);
                progress(
                    sink,
                    tool,
                    percent,
                    &format!("Downloading {tool}... {:.1} MB", total as f64 / 1048576.0),
                );
                last = Instant::now();
            }
        }
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        if expected.is_some_and(|size| size != total) {
            return Err(format!(
                "The {tool} download stopped early: received {total} of {} bytes",
                expected.unwrap()
            ));
        }
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(checksum) {
            return Err(format!(
                "The {tool} checksum did not match. The previous installation is unchanged."
            ));
        }
        check_cancel(cancel)?;
        file.persist(dest)
            .map_err(|e| format!("Could not finish the {tool} download: {e}"))?;
        Ok(())
    })
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

fn checksum_for(text: &str, filename: &str) -> Option<String> {
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(hash) = fields.next() else {
            continue;
        };
        if !valid_hash(hash) {
            continue;
        }
        match fields.next() {
            Some(name) if name.trim_start_matches('*') == filename => return Some(hash.to_owned()),
            None => return Some(hash.to_owned()),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
fn extract_named(
    archive: &Path,
    destination: &Path,
    names: &[&str],
) -> Result<Vec<(String, PathBuf)>, String> {
    extract_named_cancellable(archive, destination, names, &AtomicBool::new(false))
}

fn extract_named_cancellable(
    archive: &Path,
    destination: &Path,
    names: &[&str],
    cancel: &AtomicBool,
) -> Result<Vec<(String, PathBuf)>, String> {
    check_cancel(cancel)?;
    let mut zip = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?)
        .map_err(|e| format!("The downloaded archive could not be opened: {e}"))?;
    let mut extracted = HashMap::new();
    for index in 0..zip.len() {
        check_cancel(cancel)?;
        let mut entry = zip.by_index(index).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let Some(safe) = entry.enclosed_name() else {
            return Err("The archive contains an unsafe path".to_owned());
        };
        let filename = safe.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let Some(name) = names
            .iter()
            .find(|name| filename.eq_ignore_ascii_case(&executable_name(name)))
        else {
            continue;
        };
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("The archive contains a symbolic link instead of an executable".to_owned());
        }
        if entry.size() == 0 || entry.size() > MAX_BINARY {
            return Err("The archive contains an invalid executable size".to_owned());
        }
        if extracted.contains_key(*name) {
            return Err(format!("The archive contains more than one {name}"));
        }
        let dest = destination.join(executable_name(name));
        let mut file = File::create(&dest).map_err(|e| e.to_string())?;
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let count = entry.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            copied += count as u64;
            if copied > MAX_BINARY {
                return Err("The archive contains an invalid executable size".to_owned());
            }
            file.write_all(&buffer[..count])
                .map_err(|e| e.to_string())?;
        }
        if copied != entry.size() {
            return Err("The extracted executable is incomplete".to_owned());
        }
        file.sync_all().map_err(|e| e.to_string())?;
        executable_permissions(&dest)?;
        extracted.insert((*name).to_owned(), dest);
    }
    names
        .iter()
        .map(|name| {
            extracted
                .remove(*name)
                .map(|path| ((*name).to_owned(), path))
                .ok_or_else(|| {
                    format!(
                        "The downloaded archive does not contain {}",
                        executable_name(name)
                    )
                })
        })
        .collect()
}

#[cfg(test)]
fn publish_files(managed: &Path, staged: &[(String, PathBuf)]) -> Result<(), String> {
    publish_files_cancellable(managed, staged, &AtomicBool::new(false))
}

fn publish_files_cancellable(
    managed: &Path,
    staged: &[(String, PathBuf)],
    cancel: &AtomicBool,
) -> Result<(), String> {
    check_cancel(cancel)?;
    publish_with(managed, staged, &|from, to| {
        rename_when_free_cancellable(from, to, cancel)
    })
}

fn publish_with(
    managed: &Path,
    staged: &[(String, PathBuf)],
    rename: &impl Fn(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    fs::create_dir_all(managed).map_err(|e| e.to_string())?;
    let mut changes: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let result: Result<(), String> = (|| {
        for (name, source) in staged {
            let target = managed.join(executable_name(name));
            let previous = if target.exists() {
                let previous = managed.join(format!(
                    "{}.old-{}",
                    executable_name(name),
                    uuid::Uuid::new_v4()
                ));
                rename(&target, &previous)?;
                Some(previous)
            } else {
                None
            };
            changes.push((target.clone(), previous));
            rename(source, &target)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut failures = Vec::new();
        for (target, previous) in changes.iter().rev() {
            if target.exists() {
                if let Err(error) = fs::remove_file(target) {
                    failures.push(error.to_string());
                    continue;
                }
            }
            if let Some(previous) = previous {
                if let Err(error) = rename_when_free(previous, target) {
                    failures.push(error);
                }
            }
        }
        return if failures.is_empty() {
            if error == "Cancelled" {
                Err(error)
            } else {
                Err(format!(
                    "Tool installation failed. The previous installation was restored: {error}"
                ))
            }
        } else {
            Err(format!("Tool installation failed: {error}. Recovery also failed: {}. Previous executables remain in {}.", failures.join(", "), managed.display()))
        };
    }
    for (_, previous) in changes {
        if let Some(previous) = previous {
            let _ = fs::remove_file(previous);
        }
    }
    Ok(())
}

fn rename_when_free(from: &Path, to: &Path) -> Result<(), String> {
    rename_when_free_cancellable(from, to, &AtomicBool::new(false))
}

fn rename_when_free_cancellable(from: &Path, to: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let mut last = None;
    for attempt in 0..100 {
        check_cancel(cancel)?;
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let retry = matches!(error.raw_os_error(), Some(5 | 32 | 33));
                last = Some(error);
                if !retry {
                    break;
                }
            }
        }
        if attempt < 99 {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Err(format!(
        "Could not replace {}: {}",
        to.display(),
        last.unwrap()
    ))
}

fn executable_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
fn probe_version(path: &Path, name: &str) -> Result<String, String> {
    probe_version_cancellable(path, name, &AtomicBool::new(false))
}

fn probe_version_cancellable(
    path: &Path,
    name: &str,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let mut args = vec![if matches!(name, "ffmpeg" | "ffprobe") {
        "-version".to_owned()
    } else {
        "--version".to_owned()
    }];
    if name == "yt-dlp" {
        args.push("--ignore-config".to_owned());
    }
    let output = crate::process::run(
        path,
        &args,
        cancel,
        Some(Duration::from_secs(15)),
        |_, _| {},
    )?;
    if !output.success {
        return Err(format!("{name} failed its version check: {}", output.error));
    }
    let version = output.stdout.lines().next().unwrap_or("").trim();
    let valid = match name {
        "ffmpeg" | "ffprobe" => version.starts_with(&format!("{name} version ")),
        "yt-dlp" => date_version(version).is_some(),
        "deno" => version
            .strip_prefix("deno ")
            .is_some_and(|version| version_at_least(version, "2.3.0")),
        "node" => version_at_least(version.trim_start_matches('v'), "22.0.0"),
        "bun" => version_at_least(version, "1.2.11") && !version_at_least(version, "1.3.15"),
        _ => false,
    };
    if !valid {
        return Err(format!("{name} returned an unsupported version: {version}"));
    }
    Ok(version.to_owned())
}

fn version_at_least(value: &str, minimum: &str) -> bool {
    semver::Version::parse(value.split_whitespace().next().unwrap_or(""))
        .ok()
        .is_some_and(|version| version >= semver::Version::parse(minimum).unwrap())
}

fn date_version(value: &str) -> Option<Vec<u32>> {
    let parts: Option<Vec<u32>> = value
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()) {
                None
            } else {
                part.parse().ok()
            }
        })
        .collect();
    parts.filter(|parts| {
        parts.len() >= 3
            && parts[0] >= 2020
            && (1..=12).contains(&parts[1])
            && (1..=31).contains(&parts[2])
    })
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (date_version(candidate), date_version(current)) {
        (Some(mut candidate), Some(mut current)) => {
            let count = candidate.len().max(current.len());
            candidate.resize(count, 0);
            current.resize(count, 0);
            candidate > current
        }
        (Some(_), None) => true,
        _ => false,
    }
}

fn newest_ytdlp(
    paths: Vec<PathBuf>,
    mut validate: impl FnMut(&Path) -> Result<Option<String>, String>,
) -> Result<Option<PathBuf>, String> {
    let mut newest: Option<(PathBuf, String)> = None;
    for path in paths {
        let Some(version) = validate(&path)? else {
            continue;
        };
        if newest
            .as_ref()
            .is_none_or(|(_, current)| is_newer(&version, current))
        {
            newest = Some((path, version));
        }
    }
    Ok(newest.map(|(path, _)| path))
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-_".contains(&c))
}

fn read_json(path: &Path) -> Value {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| json!({}))
}

fn dedupe_dirs(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| {
            let key = path
                .to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_ascii_lowercase();
            seen.insert(key)
        })
        .collect()
}

fn expand_env(value: &str) -> String {
    let mut output = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let name = &after[..end];
        output.push_str(&std::env::var(name).unwrap_or_else(|_| format!("%{name}%")));
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    output
}

fn search_dirs() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        directories.extend(std::env::split_paths(&path));
    }
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        for (hive, key) in [
            (HKEY_CURRENT_USER, "Environment"),
            (
                HKEY_LOCAL_MACHINE,
                "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment",
            ),
        ] {
            if let Ok(key) = winreg::RegKey::predef(hive).open_subkey(key) {
                if let Ok(path) = key.get_value::<String, _>("Path") {
                    directories.extend(
                        path.split(';')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(|part| PathBuf::from(expand_env(part.trim_matches('"')))),
                    );
                }
            }
        }
        for (variable, suffix) in [
            ("LOCALAPPDATA", "Microsoft/WinGet/Links"),
            ("LOCALAPPDATA", "Programs/ffmpeg/bin"),
            ("ProgramFiles", "ffmpeg/bin"),
            ("ProgramFiles", "nodejs"),
            ("ProgramData", "chocolatey/bin"),
            ("USERPROFILE", ".deno/bin"),
            ("USERPROFILE", ".bun/bin"),
        ] {
            if let Some(root) = std::env::var_os(variable) {
                directories.push(PathBuf::from(root).join(suffix));
            }
        }
    }
    dedupe_dirs(directories)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn tools(root: &Path) -> Tools {
        let mut tools = Tools::new(root.into(), Arc::new(Settings::new(root.into()).unwrap()));
        tools.discovery_override = Some(Vec::new());
        tools
    }

    #[test]
    fn numeric_tool_versions_never_downgrade() {
        assert!(is_newer("2026.08.19", "2026.7.4"));
        assert!(!is_newer("2026.7.4", "2026.08.19"));
        assert!(!is_newer("2026.8.19.0", "2026.08.19"));
        assert!(date_version("2026.13.01").is_none());
        assert!(date_version("2026.8.1/../../x").is_none());
    }

    #[test]
    fn newer_bundled_tools_win_and_equal_managed_versions_keep_priority() {
        let paths = vec![
            PathBuf::from("active"),
            PathBuf::from("previous"),
            PathBuf::from("bundled"),
        ];
        let selected = newest_ytdlp(paths.clone(), |path| {
            Ok(Some(
                match path.to_str().unwrap() {
                    "active" => "2026.08.19",
                    "previous" => "2026.7.4",
                    _ => "2026.9.1",
                }
                .to_owned(),
            ))
        })
        .unwrap();
        assert_eq!(selected, Some(PathBuf::from("bundled")));
        assert_eq!(
            newest_ytdlp(paths.clone(), |_| Ok(Some("2026.9.1".to_owned()))).unwrap(),
            Some(PathBuf::from("active"))
        );
        assert_eq!(
            newest_ytdlp(paths, |path| Ok(
                (path == Path::new("previous")).then(|| "2026.7.4".to_owned())
            ))
            .unwrap(),
            Some(PathBuf::from("previous"))
        );
    }

    #[test]
    fn checksum_selection_requires_the_requested_asset() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        assert_eq!(
            checksum_for(
                &format!("\n{a}  yt-dlp_linux\n\n{b} *yt-dlp.exe\n"),
                "yt-dlp.exe"
            ),
            Some(b.clone())
        );
        assert_eq!(checksum_for(&a, "ffmpeg.zip"), Some(a));
        assert_eq!(checksum_for(&format!("{b} other.exe"), "yt-dlp.exe"), None);
        assert_eq!(checksum_for("not a hash", "yt-dlp.exe"), None);
    }

    #[test]
    fn a_failed_second_binary_publication_restores_the_complete_old_pair() {
        let dir = tempfile::tempdir().unwrap();
        let managed = dir.path().join("managed");
        let stage = dir.path().join("stage");
        fs::create_dir_all(&managed).unwrap();
        fs::create_dir_all(&stage).unwrap();
        let staged: Vec<_> = ["ffmpeg", "ffprobe"]
            .into_iter()
            .map(|name| {
                fs::write(managed.join(executable_name(name)), format!("old {name}")).unwrap();
                let source = stage.join(executable_name(name));
                fs::write(&source, format!("new {name}")).unwrap();
                (name.to_owned(), source)
            })
            .collect();
        let result = publish_with(&managed, &staged, &|from, to| {
            if from == staged[1].1 {
                return Err("simulated file lock".to_owned());
            }
            fs::rename(from, to).map_err(|e| e.to_string())
        });
        assert!(result
            .unwrap_err()
            .contains("previous installation was restored"));
        for name in ["ffmpeg", "ffprobe"] {
            assert_eq!(
                fs::read_to_string(managed.join(executable_name(name))).unwrap(),
                format!("old {name}")
            );
        }
        assert_eq!(fs::read_dir(managed).unwrap().count(), 2);
    }

    #[test]
    fn publication_replaces_both_tools_and_removes_old_copies() {
        let dir = tempfile::tempdir().unwrap();
        let managed = dir.path().join("managed");
        let staged: Vec<_> = ["ffmpeg", "ffprobe"]
            .into_iter()
            .map(|name| {
                let file = dir.path().join(executable_name(name));
                fs::write(&file, name).unwrap();
                (name.to_owned(), file)
            })
            .collect();
        publish_files(&managed, &staged).unwrap();
        for name in ["ffmpeg", "ffprobe"] {
            assert_eq!(
                fs::read_to_string(managed.join(executable_name(name))).unwrap(),
                name
            );
        }
    }

    fn archive(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body).unwrap();
        }
        let path = dir.join("download.zip");
        fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
        path
    }

    #[test]
    fn archive_layout_is_independent_of_release_directory_names() {
        let dir = tempfile::tempdir().unwrap();
        let first = format!("ffmpeg-9.0/bin/{}", executable_name("ffmpeg"));
        let second = format!("ffmpeg-9.0/bin/{}", executable_name("ffprobe"));
        let path = archive(
            dir.path(),
            &[
                (&first, b"ffmpeg"),
                (&second, b"ffprobe"),
                ("README", b"ignored"),
            ],
        );
        let extracted = extract_named(&path, dir.path(), &["ffmpeg", "ffprobe"]).unwrap();
        assert_eq!(extracted.len(), 2);
        assert_eq!(fs::read(&extracted[0].1).unwrap(), b"ffmpeg");
    }

    #[test]
    fn archive_traversal_and_incomplete_tool_pairs_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let first = executable_name("ffmpeg");
        let path = archive(dir.path(), &[(&first, b"ffmpeg")]);
        assert!(extract_named(&path, dir.path(), &["ffmpeg", "ffprobe"])
            .unwrap_err()
            .contains("ffprobe"));
        let path = archive(dir.path(), &[("../escape", b"bad")]);
        assert!(extract_named(&path, dir.path(), &["ffmpeg"])
            .unwrap_err()
            .contains("unsafe path"));
        assert!(!dir.path().parent().unwrap().join("escape").exists());
    }

    #[test]
    fn a_custom_directory_with_invalid_executables_does_not_replace_saved_path() {
        let dir = tempfile::tempdir().unwrap();
        let manager = tools(dir.path());
        manager
            .settings
            .merge_raw(&json!({"ffmpeg_path":"previous"}))
            .unwrap();
        fs::write(
            dir.path().join(executable_name("ffmpeg")),
            b"not executable",
        )
        .unwrap();
        assert!(manager.set_custom_path(dir.path()).is_err());
        assert_eq!(manager.settings.raw()["ffmpeg_path"], "previous");
    }

    #[test]
    fn disabled_automatic_updates_make_no_network_request() {
        let dir = tempfile::tempdir().unwrap();
        let manager = tools(dir.path());
        manager
            .settings
            .save(&json!({"auto_update_ytdlp":false}))
            .unwrap();
        assert_eq!(
            manager
                .update_ytdlp(false, &crate::events::silent())
                .unwrap(),
            json!({"checked":false,"reason":"disabled"})
        );
        assert!(!dir.path().join("ytdlp-bin").exists());
    }

    #[test]
    fn downloads_validate_byte_count_and_hash_against_a_local_server() {
        use std::net::TcpListener;
        let bytes = b"downloaded executable contents";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .unwrap();
                stream.write_all(bytes).unwrap();
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let hash = format!("{:x}", Sha256::digest(bytes));
        let url = format!("http://{address}/tool");
        let sink = crate::events::silent();
        let client = client().unwrap();
        download(
            &client,
            &url,
            &dir.path().join("good"),
            Some(bytes.len() as u64),
            &hash,
            &sink,
            "test",
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(fs::read(dir.path().join("good")).unwrap(), bytes);
        assert!(download(
            &client,
            &url,
            &dir.path().join("short"),
            Some(500),
            &hash,
            &sink,
            "test",
            &AtomicBool::new(false),
        )
        .unwrap_err()
        .contains("stopped early"));
        assert!(download(
            &client,
            &url,
            &dir.path().join("wrong"),
            None,
            &"0".repeat(64),
            &sink,
            "test",
            &AtomicBool::new(false),
        )
        .unwrap_err()
        .contains("checksum"));
        server.join().unwrap();
    }

    #[test]
    fn active_pointers_cannot_escape_the_managed_directory() {
        for value in ["", ".", "..", "../outside", "C:\\tools", "/tmp/exe", "a/b"] {
            assert!(!safe_component(value));
        }
        assert!(safe_component("2026.08.19-a3d24c"));
    }

    struct StalledServer {
        url: String,
        ready: std::sync::mpsc::Receiver<()>,
        release: std::sync::mpsc::Sender<()>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl StalledServer {
        fn new(body: bool) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let (ready_send, ready) = std::sync::mpsc::channel();
            let (release, release_recv) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                let started = Instant::now();
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if started.elapsed() > Duration::from_secs(8) {
                                return;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("Could not accept local test request: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 4096];
                stream.read(&mut request).unwrap();
                if body {
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\npartial").unwrap();
                    stream.flush().unwrap();
                }
                let _ = ready_send.send(());
                let _ = release_recv.recv_timeout(Duration::from_secs(6));
            });
            Self {
                url: format!("http://{address}/tool"),
                ready,
                release,
                worker: Some(worker),
            }
        }
    }

    impl Drop for StalledServer {
        fn drop(&mut self) {
            let _ = self.release.send(());
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }

    #[test]
    fn cancelling_a_stalled_bootstrap_removes_partial_files_and_allows_retry() {
        let server = StalledServer::new(true);
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(tools(dir.path()));
        let active = dir.path().join("active.json");
        atomic_json(&active, &json!({"version":"previous"})).unwrap();
        let previous = fs::read(&active).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let worker_manager = manager.clone();
        let url = server.url.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = worker_manager.ensure_runtime_with(&cancelled, || {
                let stage = tempfile::Builder::new()
                    .prefix(".runtime-test-")
                    .tempdir_in(worker_manager.root())
                    .unwrap();
                download(
                    &client().unwrap(),
                    &url,
                    &stage.path().join("runtime.zip"),
                    None,
                    &"0".repeat(64),
                    &crate::events::silent(),
                    "runtime",
                    &cancelled,
                )?;
                atomic_json(
                    &worker_manager.root().join("active.json"),
                    &json!({"version":"new"}),
                )
            });
            let _ = send.send(result);
        });
        server.ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let partial_exists = fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .any(|entry| {
                    fs::read_dir(entry.path()).unwrap().flatten().any(|file| {
                        file.file_name().to_string_lossy().starts_with(".download-")
                            && fs::read(file.path()).is_ok_and(|bytes| !bytes.is_empty())
                    })
                });
            if partial_exists {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "The body never reached the staged download file"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let started = Instant::now();
        cancel.store(true, Ordering::Release);
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap_err(),
            "Cancelled"
        );
        worker.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(fs::read(&active).unwrap(), previous);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(manager.runtime_attempt.lock().unwrap().is_none());
        let mut retried = false;
        assert_eq!(
            manager
                .ensure_runtime_with(&AtomicBool::new(false), || {
                    retried = true;
                    Err("second attempt reached".to_owned())
                })
                .unwrap_err(),
            "second attempt reached"
        );
        assert!(retried);
    }

    #[test]
    fn cancelling_before_http_headers_arrive_interrupts_metadata_requests() {
        let server = StalledServer::new(false);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let url = server.url.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = send.send(get_text(&client().unwrap(), &url, &cancelled));
        });
        server.ready.recv_timeout(Duration::from_secs(5)).unwrap();
        cancel.store(true, Ordering::Release);
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap_err(),
            "Cancelled"
        );
        worker.join().unwrap();
    }

    #[test]
    fn a_cancelled_install_can_leave_the_queue_without_waiting_for_another_install() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(tools(dir.path()));
        manager
            .settings
            .save(&json!({"auto_update_ytdlp":false}))
            .unwrap();
        let occupied = manager.install.lock().unwrap();
        let worker_manager = manager.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = send.send(worker_manager.update_ytdlp_cancellable(
                false,
                &crate::events::silent(),
                &cancelled,
            ));
        });
        std::thread::sleep(Duration::from_millis(50));
        cancel.store(true, Ordering::Release);
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap_err(),
            "Cancelled"
        );
        worker.join().unwrap();
        drop(occupied);
    }

    #[test]
    fn already_cancelled_bootstrap_never_starts_an_install() {
        let dir = tempfile::tempdir().unwrap();
        let manager = tools(dir.path());
        let cancel = AtomicBool::new(true);
        let sink = crate::events::silent();
        assert_eq!(
            manager
                .ensure_ytdlp_cancellable(&sink, &cancel)
                .unwrap_err(),
            "Cancelled"
        );
        assert_eq!(
            manager
                .update_ytdlp_cancellable(true, &sink, &cancel)
                .unwrap_err(),
            "Cancelled"
        );
        assert_eq!(
            manager
                .ensure_runtime_cancellable(&sink, &cancel)
                .unwrap_err(),
            "Cancelled"
        );
        assert_eq!(
            manager
                .install_ffmpeg_cancellable(&sink, &cancel)
                .unwrap_err(),
            "Cancelled"
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        assert!(manager.runtime_attempt.lock().unwrap().is_none());
    }

    #[test]
    fn cancellation_during_publication_restores_the_previous_pair() {
        let dir = tempfile::tempdir().unwrap();
        let managed = dir.path().join("managed");
        fs::create_dir(&managed).unwrap();
        let staged: Vec<_> = ["ffmpeg", "ffprobe"]
            .into_iter()
            .map(|name| {
                fs::write(managed.join(executable_name(name)), format!("old {name}")).unwrap();
                let source = dir.path().join(executable_name(name));
                fs::write(&source, format!("new {name}")).unwrap();
                (name.to_owned(), source)
            })
            .collect();
        let cancel = AtomicBool::new(false);
        let result = publish_with(&managed, &staged, &|from, to| {
            rename_when_free_cancellable(from, to, &cancel)?;
            if from == staged[0].1 {
                cancel.store(true, Ordering::Release);
            }
            Ok(())
        });
        assert_eq!(result.unwrap_err(), "Cancelled");
        for name in ["ffmpeg", "ffprobe"] {
            assert_eq!(
                fs::read_to_string(managed.join(executable_name(name))).unwrap(),
                format!("old {name}")
            );
        }
        assert_eq!(fs::read_dir(managed).unwrap().count(), 2);
    }

    #[test]
    #[ignore = "Downloads official executables into an isolated temporary installation"]
    fn official_ytdlp_installation_is_verified_and_usable() {
        let dir = tempfile::tempdir().unwrap();
        let manager = tools(dir.path());
        let result = manager
            .update_ytdlp(true, &crate::events::silent())
            .unwrap();
        let executable = manager.ytdlp().unwrap();
        assert!(executable.starts_with(dir.path().join("ytdlp-bin")));
        assert_eq!(result["updated"], true);
        assert!(probe_version(&executable, "yt-dlp").is_ok());
        assert_eq!(result["checked"], true);
    }

    #[test]
    #[ignore = "Downloads official executables into an isolated temporary installation"]
    fn official_deno_installation_is_verified_and_discovered() {
        let dir = tempfile::tempdir().unwrap();
        let manager = tools(dir.path());
        assert!(manager.runtime_args().is_empty());
        manager.ensure_runtime(&crate::events::silent()).unwrap();
        assert_eq!(
            manager.runtime_args(),
            vec![
                "--js-runtimes".to_owned(),
                format!(
                    "deno:{}",
                    dir.path()
                        .join("deno")
                        .join(executable_name("deno"))
                        .display()
                )
            ]
        );
        assert!(probe_version(
            &dir.path().join("deno").join(executable_name("deno")),
            "deno"
        )
        .is_ok());
    }

    #[test]
    #[ignore = "Downloads official executables into an isolated temporary installation"]
    fn official_ffmpeg_installation_is_verified_and_discovered() {
        let dir = tempfile::tempdir().unwrap();
        let manager = tools(dir.path());
        assert_eq!(manager.status()["installed"], false);
        manager.install_ffmpeg(&crate::events::silent()).unwrap();
        assert_eq!(manager.status()["installed"], true);
        for name in ["ffmpeg", "ffprobe"] {
            let path = dir.path().join("ffmpeg").join(executable_name(name));
            assert!(probe_version(&path, name).is_ok());
            assert_eq!(manager.resolve_ffmpeg(name), Some(path));
        }
    }
}
