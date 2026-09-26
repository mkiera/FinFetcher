use crate::{events::EventSink, settings::Settings};
use chrono::{DateTime, Local, NaiveDateTime, Utc};
use reqwest::blocking::{Client, Response};
use semver::{BuildMetadata, Version};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const API: &str = "https://api.github.com/repos/mkiera/FinFetcher";
const MAX_DOWNLOAD: u64 = 350 * 1024 * 1024;
const MAX_INSTALLER: u64 = 300 * 1024 * 1024;
const INSTALLER_SWITCHES: &[&str] = &[
    "/SILENT",
    "/SP-",
    "/NOCANCEL",
    "/NORESTART",
    "/CLOSEAPPLICATIONS",
    "/NORESTARTAPPLICATIONS",
];

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

pub fn parse_version(value: &str) -> Option<Version> {
    let value = value.trim().strip_prefix('v').unwrap_or(value.trim());
    let mut parsed = Version::parse(value).ok().or_else(|| {
        let core_end = value.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
        let (core, tail) = value.split_at(core_end);
        let (suffix, name) = tail.split_once('-').unwrap_or((tail, "0"));
        if !matches!(suffix, "b" | "f")
            || name.is_empty()
            || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return None;
        }
        Version::parse(&format!("{core}-legacy.{suffix}.{name}")).ok()
    })?;
    parsed.build = BuildMetadata::EMPTY;
    Some(parsed)
}

pub fn is_installer(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.ends_with(".exe") && name.contains("setup")
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 160
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        && (name.to_ascii_lowercase().ends_with(".exe")
            || name.to_ascii_lowercase().ends_with(".zip"))
}

pub fn safe_url(value: &str) -> bool {
    if value.bytes().any(|c| c <= 32) {
        return false;
    }
    reqwest::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && matches!(
                url.host_str(),
                Some(
                    "github.com"
                        | "api.github.com"
                        | "objects.githubusercontent.com"
                        | "release-assets.githubusercontent.com"
                        | "nightly.link"
                )
            )
    })
}

fn storage_redirect(current: &reqwest::Url, next: &reqwest::Url) -> bool {
    matches!(current.host_str(), Some("nightly.link" | "api.github.com"))
        && next.scheme() == "https"
        && next.port_or_known_default() == Some(443)
        && next.username().is_empty()
        && next.password().is_none()
        && next.fragment().is_none()
        && next.path().starts_with("/actions-results/")
        && next
            .host_str()
            .and_then(|v| v.strip_prefix("productionresultssa"))
            .and_then(|v| v.strip_suffix(".blob.core.windows.net"))
            .is_some_and(|v| !v.is_empty() && v.bytes().all(|c| c.is_ascii_digit()))
        && next.query_pairs().any(|(key, _)| key == "sig")
}

fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent("FinFetcher-Updater/2.0")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

fn request(client: &Client, value: &str) -> Result<Response, String> {
    if !safe_url(value) {
        return Err("Untrusted update host".into());
    }
    let mut url = reqwest::Url::parse(value).map_err(|e| e.to_string())?;
    for _ in 0..10 {
        let response = client
            .get(url.clone())
            .header("Accept", "application/vnd.github+json")
            .send()
            .map_err(|e| e.to_string())?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or("The update server returned a redirect without a location.")?;
            let next = url.join(location).map_err(|e| e.to_string())?;
            if !safe_url(next.as_str()) && !storage_redirect(&url, &next) {
                return Err("The update server redirected to an untrusted address.".into());
            }
            url = next;
            continue;
        }
        return response.error_for_status().map_err(|e| e.to_string());
    }
    Err("Too many update redirects.".into())
}

pub fn release_rows(records: &Value, channel: &str, running: &str) -> Vec<Value> {
    let current = parse_version(running);
    let mut rows: Vec<Value> = records.as_array().into_iter().flatten().filter_map(|record| {
        let tag = text(&record["tag_name"]);
        let parsed = parse_version(tag)?;
        let prerelease = record["prerelease"] == true || !parsed.pre.is_empty();
        if record["draft"] == true || (prerelease && channel != "prerelease")
            || (parsed.major, parsed.minor, parsed.patch) < (1, 2, 0)
        {
            return None;
        }
        let assets: Vec<&Value> = record["assets"].as_array().into_iter().flatten()
            .filter(|asset| text(&asset["name"]).to_ascii_lowercase().ends_with(".exe"))
            .collect();
        let picked = assets.iter().find(|asset| is_installer(text(&asset["name"])))
            .or_else(|| assets.first());
        let asset = picked.map(|asset| json!({"name":asset["name"],
            "url":asset["browser_download_url"],"size":asset["size"],
            "is_installer":is_installer(text(&asset["name"]))}));
        Some(json!({"version":tag.strip_prefix('v').unwrap_or(tag),"tag":tag,
            "prerelease":prerelease,"html_url":record["html_url"],
            "published_at":record["published_at"],"exe_asset":asset,"is_current":current.as_ref()==Some(&parsed)}))
    }).collect();
    rows.sort_by(|a, b| {
        parse_version(text(&b["version"])).cmp(&parse_version(text(&a["version"])))
    });
    rows
}

pub fn alpha_rows(runs: &Value, branches: &Value, identity: &Value) -> Vec<Value> {
    let live: HashSet<String> = branches
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| text(&v["name"]).to_ascii_lowercase())
        .collect();
    let mut newest: HashMap<String, Value> = HashMap::new();
    for run in runs.as_array().into_iter().flatten() {
        let branch = text(&run["head_branch"]);
        let key = branch.to_ascii_lowercase();
        if run["conclusion"] != "success" || branch.is_empty() || !live.contains(&key) {
            continue;
        }
        if newest
            .get(&key)
            .is_none_or(|prior| run["id"].as_u64() > prior["id"].as_u64())
        {
            newest.insert(key, run.clone());
        }
    }
    let mut rows: Vec<Value> = newest.into_values().map(|run| {
        let branch = text(&run["head_branch"]);
        let run_id = run["id"].as_u64().unwrap_or_default();
        let sha = text(&run["head_sha"]);
        let stamped_run = identity["run_id"].as_str().map(str::to_owned)
            .or_else(|| identity["run_id"].as_u64().map(|v| v.to_string())).unwrap_or_default();
        let stamped_sha = text(identity.get("sha").unwrap_or(&identity["commit"]));
        let current = if stamped_run.is_empty() {
            !stamped_sha.is_empty() && !sha.is_empty() && stamped_sha == sha
        } else { stamped_run == run_id.to_string() };
        json!({"branch":branch,"sha":sha.chars().take(7).collect::<String>(),
            "version":"","run_id":run_id,"is_current":current,
            "artifact_name":"FinFetcher-Setup","published_at":run["created_at"],
            "html_url":run["html_url"],"exe_asset":{"name":"FinFetcher-Setup.zip",
                "url":format!("https://nightly.link/mkiera/FinFetcher/actions/runs/{run_id}/FinFetcher-Setup.zip"),
                "size":0,"is_installer":true}})
    }).collect();
    rows.sort_by(|a, b| b["run_id"].as_u64().cmp(&a["run_id"].as_u64()));
    rows
}

fn cooldown(value: &Value) -> bool {
    let timestamp = text(value);
    let parsed = DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|v| v.timestamp())
        .or_else(|| {
            NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .and_then(|v| v.and_local_timezone(Local).single())
                .map(|v| v.timestamp())
        });
    parsed.is_some_and(|v| {
        let age = Utc::now().timestamp() - v;
        (0..3600).contains(&age)
    })
}

pub struct Updater {
    root: PathBuf,
    settings_store: Arc<Settings>,
    version: String,
    identity: Mutex<Value>,
    cache: Mutex<HashMap<String, (Instant, String, Value)>>,
    artifacts_refresh: AtomicBool,
    download_lock: Mutex<()>,
}

impl Updater {
    pub fn new(root: PathBuf, settings: Arc<Settings>, version: String) -> Self {
        Self {
            root,
            settings_store: settings,
            version,
            identity: Mutex::new(
                serde_json::from_str(option_env!("FINFETCHER_BUILD_IDENTITY_JSON").unwrap_or("{}"))
                    .unwrap_or(json!({})),
            ),
            cache: Mutex::new(HashMap::new()),
            artifacts_refresh: AtomicBool::new(true),
            download_lock: Mutex::new(()),
        }
    }

    pub fn set_identity(&self, identity: Value) {
        *self.identity.lock().unwrap() = identity;
    }

    pub fn settings(&self) -> Value {
        let settings = self.settings_store.raw();
        json!({"update_channel":settings.get("update_channel").filter(|v|
            matches!(v.as_str(),Some("stable"|"prerelease"|"alpha"))).unwrap_or(&json!("stable")),
            "auto_check_updates":settings["auto_check_updates"].as_bool().unwrap_or(true),
            "skipped_version":settings["skipped_version"],"current_version":self.version,
            "can_self_update":cfg!(windows) && !cfg!(debug_assertions)})
    }

    pub fn save_settings(&self, value: &Value) -> Result<Value, String> {
        let values = value
            .as_object()
            .ok_or("Update settings must be an object.")?;
        let mut patch = serde_json::Map::new();
        for (key, value) in values {
            match key.as_str() {
                "update_channel"
                    if matches!(value.as_str(), Some("stable" | "prerelease" | "alpha")) => {}
                "auto_check_updates" if value.is_boolean() => {}
                "skipped_version"
                    if value.is_null()
                        || value.as_str().is_some_and(|v| parse_version(v).is_some()) => {}
                "update_channel" | "auto_check_updates" | "skipped_version" => {
                    return Err(format!("Invalid {key}"))
                }
                _ => continue,
            }
            patch.insert(key.clone(), value.clone());
        }
        self.settings_store.merge_raw(&Value::Object(patch))?;
        Ok(json!({"success":true}))
    }

    fn json(&self, endpoint: &str) -> Result<Value, String> {
        let mut response = request(&client()?, &format!("{API}/{endpoint}"))?;
        let mut bytes = Vec::new();
        response
            .by_ref()
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("The update response is too large.".into());
        }
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    fn pages(&self, endpoint: &str, array: Option<&str>) -> Result<Value, String> {
        let mut records = Vec::new();
        for page in 1..=10 {
            let separator = if endpoint.contains('?') { '&' } else { '?' };
            let result = self.json(&format!("{endpoint}{separator}per_page=100&page={page}"))?;
            let values = array
                .map(|key| &result[key])
                .unwrap_or(&result)
                .as_array()
                .ok_or("The update server returned an invalid list.")?;
            records.extend(values.iter().cloned());
            if values.len() < 100 {
                break;
            }
        }
        Ok(json!(records))
    }

    fn cached(
        &self,
        key: &str,
        refresh: bool,
        fetch: impl FnOnce() -> Result<Value, String>,
    ) -> Result<(String, Value), String> {
        let mut cache = self.cache.lock().unwrap();
        if let Some((when, fetched, data)) = cache.get(key) {
            if !refresh || when.elapsed() < Duration::from_secs(60) {
                return Ok((fetched.clone(), data.clone()));
            }
        }
        let records = match fetch() {
            Ok(records) => records,
            Err(error) => {
                return cache
                    .get(key)
                    .map(|(_, fetched, data)| (fetched.clone(), data.clone()))
                    .ok_or(error);
            }
        };
        let fetched = Utc::now().to_rfc3339();
        cache.insert(
            key.into(),
            (Instant::now(), fetched.clone(), records.clone()),
        );
        Ok((fetched, records))
    }

    fn records(&self, refresh: bool) -> Result<(String, Value), String> {
        self.cached("releases", refresh, || self.pages("releases", None))
    }

    pub fn check(&self, force: bool) -> Result<Value, String> {
        let settings = self.settings();
        if !force && settings["auto_check_updates"] == false {
            return Ok(json!({"skipped":true,"reason":"disabled"}));
        }
        if !force && cooldown(&self.settings_store.raw()["last_update_check"]) {
            return Ok(json!({"skipped":true,"reason":"cooldown"}));
        }
        if settings["update_channel"] == "alpha" {
            if force {
                self.artifacts_refresh.store(true, Ordering::Release);
            }
            return Ok(json!({"skipped":true,"reason":"manual_alpha"}));
        }
        let (_, records) = self.records(true)?;
        self.artifacts_refresh.store(true, Ordering::Release);
        self.settings_store
            .merge_raw(&json!({"last_update_check":Utc::now().to_rfc3339()}))?;
        let current = parse_version(&self.version);
        let update = release_rows(&records, text(&settings["update_channel"]), &self.version)
            .into_iter()
            .find(|row| {
                !row["exe_asset"].is_null()
                    && parse_version(text(&row["version"]))
                        .zip(current.clone())
                        .is_some_and(|(a, b)| a > b)
            });
        Ok(match update {
            Some(update) => json!({"available":true,"current_version":self.version,
                "was_skipped":update["version"]==settings["skipped_version"],"update":update}),
            None => json!({"available":false,"current_version":self.version}),
        })
    }

    pub fn releases(&self, channel: &str) -> Result<Value, String> {
        let (fetched, records) = self.records(false)?;
        Ok(
            json!({"releases":release_rows(&records,channel,&self.version),
            "current_version":self.version,"fetched_at":fetched}),
        )
    }

    pub fn artifacts(&self) -> Result<Value, String> {
        let refresh = self.artifacts_refresh.swap(false, Ordering::AcqRel);
        let (fetched, mut result) = self.cached("artifacts", refresh, || self.fetch_artifacts())?;
        result["fetched_at"] = json!(fetched);
        Ok(result)
    }

    fn fetch_artifacts(&self) -> Result<Value, String> {
        let runs = self.pages(
            "actions/workflows/build-test.yml/runs?status=success",
            Some("workflow_runs"),
        )?;
        let branches = self.pages("branches", None)?;
        let identity = self.identity.lock().unwrap().clone();
        let mut rows = alpha_rows(&runs, &branches, &identity);
        rows.retain_mut(|row| {
            let Ok(artifacts) = self.pages(&format!("actions/runs/{}/artifacts",row["run_id"]),Some("artifacts")) else { return true; };
            let eligible: Vec<&Value> = artifacts.as_array().unwrap().iter().filter(|v| v["expired"] != true
                && (text(&v["name"]) == "FinFetcher-Setup" || text(&v["name"]).starts_with("FinFetcher-Setup_")
                    || text(&v["name"]).starts_with("FinFetcher_"))).collect();
            let Some(artifact) = eligible.iter().find(|v| v["name"]=="FinFetcher-Setup").or_else(|| eligible.first()) else { return false; };
            let name = text(&artifact["name"]);
            row["artifact_name"] = json!(name);
            row["exe_asset"] = json!({"name":format!("{name}.zip"),"size":artifact["size_in_bytes"],
                "url":format!("https://nightly.link/mkiera/FinFetcher/actions/runs/{}/{name}.zip",row["run_id"]),
                "is_installer":name.to_ascii_lowercase().contains("setup")});
            true
        });
        Ok(json!({"artifacts":rows,"current_version":self.version,
            "current_build":identity}))
    }

    pub fn download(&self, url: &str, name: &str, sink: &EventSink) -> Result<PathBuf, String> {
        let _guard = self
            .download_lock
            .try_lock()
            .map_err(|_| "An update is already downloading.")?;
        if !safe_name(name) {
            return Err("Invalid update asset name.".into());
        }
        if !safe_url(url) {
            return Err("Untrusted update host".into());
        }
        let folder = self.root.join("updates");
        fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let attempt = tempfile::Builder::new()
            .prefix("update-")
            .tempdir_in(&folder)
            .map_err(|e| e.to_string())?;
        let downloaded = attempt.path().join("download.part");
        sink(json!({"percent":0,"status":"Starting download..."}));
        let mut response = request(&client()?, url)?;
        let total = response.content_length().unwrap_or(0);
        if total > MAX_DOWNLOAD {
            return Err("The update file is too large.".into());
        }
        let mut output = File::create(&downloaded).map_err(|e| e.to_string())?;
        let mut received = 0u64;
        let mut buffer = [0u8; 65536];
        let mut last = Instant::now() - Duration::from_secs(1);
        loop {
            let count = response.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            received += count as u64;
            if received > MAX_DOWNLOAD {
                return Err("The update file is too large.".into());
            }
            output
                .write_all(&buffer[..count])
                .map_err(|e| e.to_string())?;
            if last.elapsed() >= Duration::from_millis(100) {
                sink(json!({"percent":if total>0 {received*95/total} else {0},
                    "status":format!("Downloading... {:.1} MB",received as f64/1048576.0)}));
                last = Instant::now();
            }
        }
        output.sync_all().map_err(|e| e.to_string())?;
        drop(output);
        if total > 0 && total != received {
            return Err("The download stopped before the complete file arrived. Try again.".into());
        }
        let final_name = if name.to_ascii_lowercase().ends_with(".zip") {
            sink(json!({"percent":96,"status":"Extracting installer..."}));
            extract_installer(&downloaded, attempt.path())?
        } else {
            name.to_owned()
        };
        let path = attempt.path().join(&final_name);
        if !name.to_ascii_lowercase().ends_with(".zip") {
            fs::rename(&downloaded, &path).map_err(|e| e.to_string())?;
        }
        validate_binary(&path)?;
        let _ = fs::remove_file(&downloaded);
        let destination = attempt.keep().join(final_name);
        sink(
            json!({"percent":100,"status":"Download complete!","success":true,"path":destination}),
        );
        Ok(destination)
    }

    pub fn apply(&self, path: &Path) -> Result<Value, String> {
        if !cfg!(windows) || cfg!(debug_assertions) {
            return Err(
                "Self-update is only available in the packaged Windows application.".into(),
            );
        }
        let path = installer_path(path, &self.root.join("updates"))?;
        let message = launch_installer(&path, &self.root)?;
        Ok(json!({"success":true,"message":message,"log_path":self.root.join("update.log")}))
    }
}

fn validate_binary(path: &Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if !(64..=MAX_INSTALLER).contains(&size) {
        return Err("The downloaded executable has an invalid size.".into());
    }
    let mut header = [0u8; 64];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    if &header[..2] != b"MZ" {
        return Err("The downloaded file is not a Windows executable.".into());
    }
    let offset = u32::from_le_bytes(header[60..64].try_into().unwrap()) as u64;
    if offset < 64 || offset > size.saturating_sub(4) {
        return Err("The downloaded executable has an invalid header.".into());
    }
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut signature = [0u8; 4];
    file.read_exact(&mut signature).map_err(|e| e.to_string())?;
    if signature != *b"PE\0\0" {
        return Err("The downloaded executable is incomplete or invalid.".into());
    }
    Ok(())
}

fn extract_installer(archive: &Path, destination: &Path) -> Result<String, String> {
    let mut zip = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| {
            let file = zip.by_index(i).ok()?;
            let name = file.name();
            if !file.is_file() || !safe_name(name) || !name.to_ascii_lowercase().ends_with(".exe") {
                return None;
            }
            Some(name.to_owned())
        })
        .collect();
    let name = names
        .iter()
        .find(|v| v.eq_ignore_ascii_case("FinFetcher-Setup.exe"))
        .or_else(|| names.iter().find(|v| is_installer(v)))
        .or_else(|| names.first())
        .ok_or("The artifact does not contain a root application or installer executable.")?
        .clone();
    let mut file = zip.by_name(&name).map_err(|e| e.to_string())?;
    if file.size() > MAX_INSTALLER {
        return Err("The extracted update is too large.".into());
    }
    let expected = file.size();
    let mut output = File::create(destination.join(&name)).map_err(|e| e.to_string())?;
    let copied = std::io::copy(&mut file.by_ref().take(MAX_INSTALLER + 1), &mut output)
        .map_err(|e| e.to_string())?;
    if copied != expected || copied > MAX_INSTALLER {
        return Err("The update archive is incomplete.".into());
    }
    output.sync_all().map_err(|e| e.to_string())?;
    Ok(name)
}

fn installer_path(path: &Path, folder: &Path) -> Result<PathBuf, String> {
    let folder = folder.canonicalize().map_err(|e| e.to_string())?;
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(&folder) || !path.is_file() {
        return Err("Update file is outside the updates folder.".into());
    }
    if !path
        .file_name()
        .and_then(|v| v.to_str())
        .is_some_and(is_installer)
    {
        return Err(format!("{} is an older portable application, not the FinFetcher installer. To use it, uninstall the installed copy first, then run this downloaded file.",path.display()));
    }
    validate_binary(&path)?;
    Ok(path)
}

fn pending_dialog(log: &str) -> Option<String> {
    let mut messages: Vec<String> = Vec::new();
    for line in log.trim_start_matches('\u{feff}').lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !line.starts_with(|c: char| c.is_ascii_digit())
            && !trimmed.starts_with("Message box (")
            && !trimmed.starts_with("User chose ")
            && !messages.is_empty()
        {
            messages.last_mut().unwrap().push(' ');
            messages.last_mut().unwrap().push_str(trimmed);
        } else {
            messages.push(
                trimmed
                    .split_once("   ")
                    .map(|(_, v)| v.trim())
                    .unwrap_or(trimmed)
                    .to_owned(),
            );
        }
    }
    let mut pending = None;
    for message in messages {
        if message.starts_with("Message box (") {
            pending = Some(
                message
                    .split_once(':')
                    .map(|(_, v)| v.trim())
                    .unwrap_or(&message)
                    .to_owned(),
            );
        } else if message.starts_with("User chose ") {
            pending = None;
        }
    }
    pending
}

fn launch_installer(path: &Path, root: &Path) -> Result<String, String> {
    let log_path = root.join("update.log");
    match fs::remove_file(&log_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Could not prepare the update log: {error}")),
    }
    let mut command = Command::new(path);
    command
        .args(INSTALLER_SWITCHES)
        .arg(format!("/LOG={}", log_path.display()))
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000008 | 0x00000200);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not start the installer: {e}"))?;
    let deadline = Instant::now() + Duration::from_millis(1500);
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if status.success() {
                return Ok("The installer completed. FinFetcher will close now.".into());
            }
            return Err(format!(
                "The installer stopped with exit code {}. Its log is at {}.",
                status.code().unwrap_or(-1),
                log_path.display()
            ));
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    if let Ok(log) = fs::read_to_string(&log_path) {
        if let Some(message) = pending_dialog(&log) {
            return Err(format!("The installer is waiting for an answer: {message} FinFetcher has stayed open. Its log is at {}.",log_path.display()));
        }
    }
    Ok(format!("The installer is running. FinFetcher will close so it can replace these files. If the updated application does not reopen, check {}.",log_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str, prerelease: bool) -> Value {
        json!({"tag_name":format!("v{version}"),"prerelease":prerelease,"assets":[
            {"name":"FinFetcher-Legacy.exe","size":100,"browser_download_url":"https://github.com/mkiera/FinFetcher/legacy"},
            {"name":"FinFetcher-Setup.exe","size":100,"browser_download_url":"https://github.com/mkiera/FinFetcher/setup"}]})
    }

    fn executable() -> Vec<u8> {
        let mut bytes = vec![0; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes
    }

    #[test]
    fn historical_versions_and_numeric_beta_precedence() {
        for tag in [
            "v1.0.1f-streaming",
            "v1.2.3b-bundle-certifi",
            "v1.2.4b",
            "v1.2.9f-flipperclipper",
        ] {
            assert!(parse_version(tag).is_some(), "{tag}");
            assert!(parse_version("1.2.10-beta.1") > parse_version(tag));
        }
        assert!(parse_version("1.2.10-beta.11") > parse_version("1.2.10-beta.2"));
        assert!(parse_version("1.2.10") > parse_version("1.2.10-beta.11"));
        assert_eq!(parse_version("v1.2.9+abc.01"), parse_version("1.2.9+def"));
        for invalid in [
            "1",
            "1.2.3.4",
            "01.2.3",
            "1.2.3-beta.01",
            "vv1.2.3",
            "1.2.3f-",
        ] {
            assert!(parse_version(invalid).is_none());
        }
    }

    #[test]
    fn release_filter_prefers_installer_and_rejects_misflagged_betas() {
        let records = json!([
            release("1.2.10-beta.11", false),
            release("1.2.10-beta.2", true),
            release("1.2.10", false),
            release("invalid", false)
        ]);
        let stable = release_rows(&records, "stable", "1.2.10+stamp");
        assert_eq!(stable.len(), 1);
        assert_eq!(stable[0]["is_current"], true);
        assert_eq!(stable[0]["exe_asset"]["name"], "FinFetcher-Setup.exe");
        let beta = release_rows(&records, "prerelease", "1.2.9");
        assert_eq!(
            beta.iter().map(|v| text(&v["version"])).collect::<Vec<_>>(),
            vec!["1.2.10", "1.2.10-beta.11", "1.2.10-beta.2"]
        );
    }

    #[test]
    fn historical_bare_exe_stays_visible_but_is_not_installable() {
        let mut old = release("1.2.4b", true);
        old["assets"] = json!([{"name":"FinFetcher.exe","size":100,"browser_download_url":"https://github.com/mkiera/FinFetcher/old"}]);
        let rows = release_rows(&json!([old]), "prerelease", "1.2.9");
        assert_eq!(rows[0]["exe_asset"]["is_installer"], false);
    }

    #[test]
    fn alpha_selects_live_branches_and_exact_run_before_matching_commit() {
        let rows = alpha_rows(
            &json!([
            {"id":10,"head_branch":"beta","head_sha":"abcd","conclusion":"success"},
            {"id":11,"head_branch":"Beta","head_sha":"abcd","conclusion":"success"},
            {"id":12,"head_branch":"gone","conclusion":"success"}]),
            &json!([{"name":"beta"}]),
            &json!({"run_id":"10","sha":"abcd"}),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["run_id"], 11);
        assert_eq!(rows[0]["is_current"], false);
    }

    #[test]
    fn redirect_and_filename_checks_reject_untrusted_execution_paths() {
        for url in [
            "http://github.com/a",
            "https://github.com.evil.test/a",
            "https://user@github.com/a",
            "https://github.com:8443/a",
            "https://github.com/a#x",
        ] {
            assert!(!safe_url(url));
        }
        for name in [
            "../FinFetcher-Setup.exe",
            "C:\\setup.exe",
            "setup.exe:stream",
            "setup.bat",
        ] {
            assert!(!safe_name(name));
        }
        assert!(safe_name("FinFetcher-Setup_beta.zip"));
        let source = reqwest::Url::parse("https://nightly.link/mkiera/FinFetcher/a.zip").unwrap();
        assert!(storage_redirect(
            &source,
            &reqwest::Url::parse(
                "https://productionresultssa4.blob.core.windows.net/actions-results/1/a.zip?sig=x"
            )
            .unwrap()
        ));
        assert!(!storage_redirect(
            &source,
            &reqwest::Url::parse(
                "https://evil.blob.core.windows.net/actions-results/1/a.zip?sig=x"
            )
            .unwrap()
        ));
    }

    #[test]
    fn archive_extracts_only_root_executable_and_prefers_setup() {
        let folder = tempfile::tempdir().unwrap();
        let archive = folder.path().join("artifact.zip");
        let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
        for name in [
            "../FinFetcher-Setup.exe",
            "nested/FinFetcher-Setup.exe",
            "FinFetcher-Legacy.exe",
            "FinFetcher-Setup.exe",
        ] {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&executable()).unwrap();
        }
        writer.finish().unwrap();
        assert_eq!(
            extract_installer(&archive, folder.path()).unwrap(),
            "FinFetcher-Setup.exe"
        );
        validate_binary(&folder.path().join("FinFetcher-Setup.exe")).unwrap();
        assert!(!folder.path().join("nested").exists());
        assert!(!folder.path().join("FinFetcher-Legacy.exe").exists());
    }

    #[test]
    fn installer_requires_owned_folder_name_and_complete_pe_header() {
        let folder = tempfile::tempdir().unwrap();
        let updates = folder.path().join("updates");
        fs::create_dir(&updates).unwrap();
        let outside = folder.path().join("FinFetcher-Setup.exe");
        fs::write(&outside, executable()).unwrap();
        assert!(installer_path(&outside, &updates).is_err());
        let inside = updates.join("FinFetcher-Setup.exe");
        fs::write(&inside, b"MZinvalid").unwrap();
        assert!(installer_path(&inside, &updates).is_err());
        fs::write(&inside, executable()).unwrap();
        assert!(installer_path(&inside, &updates).is_ok());
        let bare = updates.join("FinFetcher-Legacy.exe");
        fs::write(&bare, executable()).unwrap();
        assert!(installer_path(&bare, &updates).is_err());
    }

    #[test]
    fn pending_installer_dialog_preserves_multiline_reason() {
        let log="\u{feff}2026-09-25 00:00:00   Log opened.\n2026-09-25 00:00:01   Message box (OK): Cannot install.\n   Close another installer.\n";
        assert_eq!(
            pending_dialog(log).as_deref(),
            Some("Cannot install. Close another installer.")
        );
        assert!(pending_dialog(&format!("{log}2026-09-25 00:00:02   User chose OK.\n")).is_none());
    }

    #[test]
    fn update_preferences_preserve_other_settings_and_alpha_is_manual() {
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join("config.json"),br#"{"future":{"keep":true},"ffmpeg_path":"C:\\Media Tools","update_channel":"stable"}"#).unwrap();
        let settings = Arc::new(Settings::new(folder.path().to_owned()).unwrap());
        let updater = Updater::new(folder.path().to_owned(), settings, "1.2.10".into());
        updater.save_settings(&json!({"update_channel":"alpha","auto_check_updates":false,"ffmpeg_path":"overwrite"})).unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(folder.path().join("config.json")).unwrap()).unwrap();
        assert_eq!(saved["future"]["keep"], true);
        assert_eq!(saved["ffmpeg_path"], "C:\\Media Tools");
        assert_eq!(updater.check(false).unwrap()["reason"], "disabled");
        assert_eq!(updater.check(true).unwrap()["reason"], "manual_alpha");
        assert!(updater
            .save_settings(&json!({"update_channel":"bad"}))
            .is_err());
        assert!(updater
            .save_settings(&json!({"auto_check_updates":"false"}))
            .is_err());
    }

    #[test]
    fn cooldown_accepts_existing_local_timestamps_and_new_utc_stamps() {
        assert!(cooldown(&json!(Utc::now().to_rfc3339())));
        assert!(cooldown(&json!(Local::now()
            .naive_local()
            .format("%Y-%m-%dT%H:%M:%S%.f")
            .to_string())));
        assert!(!cooldown(&json!("2000-01-01T00:00:00")));
        assert!(!cooldown(&json!("invalid")));
    }

    #[test]
    fn cached_lists_refresh_explicitly_and_preserve_last_good_response() {
        let root = tempfile::tempdir().unwrap();
        let settings = Arc::new(Settings::new(root.path().to_owned()).unwrap());
        let updater = Updater::new(root.path().to_owned(), settings, "1.2.10".into());
        assert!(updater
            .cached("releases", false, || Err("offline".into()))
            .is_err());
        let original = updater
            .cached("releases", false, || Ok(json!([1])))
            .unwrap();
        assert_eq!(
            updater
                .cached("releases", true, || panic!("refresh floor"))
                .unwrap(),
            original
        );
        updater.cache.lock().unwrap().get_mut("releases").unwrap().0 =
            Instant::now() - Duration::from_secs(3600);
        assert_eq!(
            updater
                .cached("releases", false, || panic!("ordinary tab read"))
                .unwrap(),
            original
        );
        assert_eq!(
            updater
                .cached("releases", true, || Err("offline".into()))
                .unwrap(),
            original
        );
        let refreshed = updater.cached("releases", true, || Ok(json!([2]))).unwrap();
        assert_eq!(refreshed.1, json!([2]));
        assert_ne!(refreshed.0, original.0);
    }

    #[cfg(windows)]
    #[test]
    fn installer_handoff_does_not_wait_for_application_exit_and_reports_early_failures() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("FinFetcher-Setup.exe");
        let mut compiler = Command::new("rustc");
        use std::os::windows::process::CommandExt;
        compiler.creation_flags(0x08000000);
        let compiled = compiler
            .args(["--edition", "2021"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/update_app.rs"))
            .arg("-o")
            .arg(&path)
            .status()
            .unwrap();
        assert!(compiled.success());
        for mode in ["waiting", "fail", "dialog"] {
            fs::write(root.path().join("installer-mode.txt"), mode).unwrap();
            let _ = fs::remove_file(root.path().join("release-installer.txt"));
            let _ = fs::remove_file(root.path().join("installer-exited.txt"));
            let start = Instant::now();
            let result = launch_installer(&path, root.path());
            fs::write(root.path().join("release-installer.txt"), "exit").unwrap();
            if mode != "fail" {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !root.path().join("installer-exited.txt").exists()
                    && Instant::now() < deadline
                {
                    thread::sleep(Duration::from_millis(20));
                }
                assert!(root.path().join("installer-exited.txt").exists());
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            match mode {
                "waiting" => assert!(result.unwrap().contains("installer is running")),
                "fail" => assert!(result.unwrap_err().contains("exit code 7")),
                "dialog" => assert!(result.unwrap_err().contains("Fixture installer blocked")),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    #[ignore = "Downloads the current public release installer from GitHub"]
    fn live_release_download_uses_real_github_redirects() {
        let root = tempfile::tempdir().unwrap();
        let settings = Arc::new(Settings::new(root.path().to_owned()).unwrap());
        let updater = Updater::new(root.path().to_owned(), settings, "1.2.10".into());
        let releases = updater.releases("stable").unwrap();
        let release = releases["releases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["exe_asset"]["is_installer"] == true)
            .expect("A public FinFetcher installer release");
        let asset = &release["exe_asset"];
        let sink = crate::events::silent();
        let downloaded = updater
            .download(text(&asset["url"]), text(&asset["name"]), &sink)
            .unwrap();
        installer_path(&downloaded, &root.path().join("updates")).unwrap();
        assert_eq!(
            fs::metadata(&downloaded).unwrap().len(),
            asset["size"].as_u64().unwrap()
        );
    }
}
