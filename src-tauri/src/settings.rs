use serde_json::{json, Map, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Settings {
    root: PathBuf,
    values: Mutex<Value>,
}

pub fn defaults() -> Value {
    json!({
        "concurrent_fragments": 4, "rate_limit_kbps": 0,
        "use_download_archive": false, "auto_update_ytdlp": true,
        "fast_trim": true, "precise_trim": true, "log_to_file": false,
        "container": "mp4", "audio_format": "mp3", "audio_quality": "0",
        "subtitles_enabled": false, "subtitle_langs": "en", "subtitles_auto": false,
        "embed_subtitles": true, "sponsorblock_enabled": false,
        "sponsorblock_categories": ["sponsor", "selfpromo", "interaction"],
        "embed_thumbnail": true, "embed_metadata": true, "embed_chapters": true,
        "split_chapters": false
    })
}

impl Settings {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root)
            .map_err(|e| format!("Could not open FinFetcher's settings folder: {e}"))?;
        let values = read_config(&root.join("config.json")).unwrap_or_else(|_| json!({}));
        Ok(Self {
            root,
            values: Mutex::new(values),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn get(&self) -> Value {
        normalize(&self.raw(), &defaults())
    }

    pub fn raw(&self) -> Value {
        self.values
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn save(&self, incoming: &Value) -> Result<Value, String> {
        let mut stored = self
            .values
            .lock()
            .map_err(|_| "Settings are unavailable".to_owned())?;
        let mut disk = read_config(&self.root.join("config.json"))?;
        let current = normalize(&disk, &normalize(&stored, &defaults()));
        let normalized = normalize(incoming, &current);
        disk.as_object_mut()
            .unwrap()
            .extend(normalized.as_object().unwrap().clone());
        atomic_json(&self.root.join("config.json"), &disk)?;
        *stored = disk;
        Ok(normalized)
    }

    pub fn merge_raw(&self, incoming: &Value) -> Result<(), String> {
        let Some(incoming) = incoming.as_object() else {
            return Err("Settings must be an object".to_owned());
        };
        let mut stored = self
            .values
            .lock()
            .map_err(|_| "Settings are unavailable".to_owned())?;
        let mut disk = read_config(&self.root.join("config.json"))?;
        disk.as_object_mut().unwrap().extend(incoming.clone());
        atomic_json(&self.root.join("config.json"), &disk)?;
        *stored = disk;
        Ok(())
    }
}

fn read_config(path: &Path) -> Result<Value, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(format!("Could not read {}: {error}", path.display())),
    };
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("The settings file is invalid and was left unchanged: {e}"))?;
    if !value.is_object() {
        return Err("The settings file must contain an object and was left unchanged".to_owned());
    }
    Ok(value)
}

pub(crate) fn atomic_json(path: &Path, value: &Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("The settings path has no parent folder")?;
    fs::create_dir_all(parent).map_err(|e| format!("Could not create settings folder: {e}"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut temp, value).map_err(|e| e.to_string())?;
    temp.write_all(b"\n").map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path)
        .map_err(|e| format!("Could not save {}: {}", path.display(), e.error))?;
    Ok(())
}

pub fn normalize(raw: &Value, base: &Value) -> Value {
    let mut normalized = Map::new();
    for (key, default) in defaults().as_object().unwrap() {
        let fallback = base.get(key).unwrap_or(default);
        let value = raw.get(key).unwrap_or(fallback);
        let normalized_value = match key.as_str() {
            "concurrent_fragments" => bounded_integer(value, fallback, 1, 16),
            "rate_limit_kbps" => bounded_integer(value, fallback, 0, 1024 * 1024),
            "container" => enum_value(value, fallback, &["mp4", "mkv", "webm"]),
            "audio_format" => enum_value(value, fallback, &["mp3", "m4a", "opus", "flac", "wav"]),
            "audio_quality" => {
                let quality = value
                    .as_str()
                    .map(str::trim)
                    .and_then(|s| s.parse::<i64>().ok())
                    .or_else(|| value.as_i64());
                match quality.filter(|q| (0..=320).contains(q)) {
                    Some(quality) => json!(quality.to_string()),
                    None => fallback.clone(),
                }
            }
            "subtitle_langs" => {
                let mut languages = Vec::new();
                if let Some(text) = value.as_str() {
                    for language in text.split(',').map(str::trim) {
                        if !language.is_empty()
                            && !languages.contains(&language)
                            && language
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || "-_.*".contains(c))
                        {
                            languages.push(language);
                        }
                    }
                }
                if languages.is_empty() {
                    fallback.clone()
                } else {
                    json!(languages.join(","))
                }
            }
            "sponsorblock_categories" => {
                if let Some(categories) = value.as_array() {
                    let allowed = [
                        "sponsor",
                        "intro",
                        "outro",
                        "selfpromo",
                        "preview",
                        "filler",
                        "interaction",
                        "music_offtopic",
                        "poi_highlight",
                    ];
                    let mut cleaned = Vec::new();
                    for category in categories.iter().filter_map(Value::as_str) {
                        if allowed.contains(&category) && !cleaned.contains(&category) {
                            cleaned.push(category);
                        }
                    }
                    json!(cleaned)
                } else {
                    fallback.clone()
                }
            }
            _ => boolean(value)
                .map(Value::Bool)
                .unwrap_or_else(|| fallback.clone()),
        };
        normalized.insert(key.clone(), normalized_value);
    }
    if normalized["sponsorblock_categories"]
        .as_array()
        .is_some_and(Vec::is_empty)
    {
        normalized.insert("sponsorblock_enabled".to_owned(), Value::Bool(false));
    }
    Value::Object(normalized)
}

fn bounded_integer(value: &Value, fallback: &Value, min: i64, max: i64) -> Value {
    let integer = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
        .or_else(|| value.as_f64().map(|n| n as i64))
        .or_else(|| value.as_bool().map(i64::from));
    integer
        .map(|n| json!(n.clamp(min, max)))
        .unwrap_or_else(|| fallback.clone())
}

fn boolean(value: &Value) -> Option<bool> {
    if let Some(value) = value.as_bool() {
        return Some(value);
    }
    if let Some(value) = value.as_f64() {
        return Some(value != 0.0);
    }
    match value.as_str()?.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn enum_value(value: &Value, fallback: &Value, allowed: &[&str]) -> Value {
    if value.as_str().is_some_and(|value| allowed.contains(&value)) {
        value.clone()
    } else {
        fallback.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn preferences_survive_restart_without_overwriting_other_owners() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::new(dir.path().into()).unwrap();
        settings
            .merge_raw(
                &json!({"ffmpeg_path":"C:/Tools", "update_channel":"beta", "future":{"a":3}}),
            )
            .unwrap();
        settings
            .save(&json!({"container":"mkv", "log_to_file":true, "ffmpeg_path":"bad"}))
            .unwrap();
        let reopened = Settings::new(dir.path().into()).unwrap();
        assert_eq!(reopened.get()["container"], "mkv");
        assert_eq!(reopened.raw()["ffmpeg_path"], "C:/Tools");
        assert_eq!(reopened.raw()["update_channel"], "beta");
        assert_eq!(reopened.raw()["future"], json!({"a":3}));
    }

    #[test]
    fn invalid_inputs_keep_prior_values_and_normalize_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::new(dir.path().into()).unwrap();
        settings
            .save(&json!({"audio_format":"flac", "audio_quality":"192", "container":"mkv"}))
            .unwrap();
        let value = settings
            .save(&json!({
                "audio_format":"exe", "audio_quality":"128 -y", "container":null,
                "concurrent_fragments":100, "rate_limit_kbps":-1,
                "embed_thumbnail":"off", "subtitle_langs":"en, en, --bad value,ja.*,zh-Hans",
                "sponsorblock_enabled":true, "sponsorblock_categories":["unknown"]
            }))
            .unwrap();
        assert_eq!(value["audio_format"], "flac");
        assert_eq!(value["audio_quality"], "192");
        assert_eq!(value["container"], "mkv");
        assert_eq!(value["concurrent_fragments"], 16);
        assert_eq!(value["rate_limit_kbps"], 0);
        assert_eq!(value["embed_thumbnail"], false);
        assert_eq!(value["subtitle_langs"], "en,ja.*,zh-Hans");
        assert_eq!(value["sponsorblock_enabled"], false);
    }

    #[test]
    fn independent_owners_merge_the_latest_disk_state() {
        let dir = tempfile::tempdir().unwrap();
        let one = Settings::new(dir.path().into()).unwrap();
        let two = Settings::new(dir.path().into()).unwrap();
        one.merge_raw(&json!({"ffmpeg_path":"new"})).unwrap();
        two.save(&json!({"precise_trim":false})).unwrap();
        one.merge_raw(&json!({"update_channel":"alpha"})).unwrap();
        let reopened = Settings::new(dir.path().into()).unwrap();
        assert_eq!(reopened.raw()["ffmpeg_path"], "new");
        assert_eq!(reopened.raw()["precise_trim"], false);
    }

    #[test]
    fn concurrent_updates_are_serialized_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Arc::new(Settings::new(dir.path().into()).unwrap());
        let handles: Vec<_> = (0..12)
            .map(|index| {
                let settings = settings.clone();
                std::thread::spawn(move || {
                    settings
                        .merge_raw(&json!({format!("key{index}"): index}))
                        .unwrap()
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let persisted = Settings::new(dir.path().into()).unwrap().raw();
        for index in 0..12 {
            assert_eq!(persisted[format!("key{index}")], index);
        }
    }

    #[test]
    fn corrupt_existing_settings_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.json"), b"{broken").unwrap();
        let settings = Settings::new(dir.path().into()).unwrap();
        assert_eq!(settings.get(), defaults());
        assert!(settings.save(&json!({"container":"mkv"})).is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("config.json")).unwrap(),
            "{broken"
        );
    }
}
