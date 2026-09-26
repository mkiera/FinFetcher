use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DownloadRequest {
    pub url: String,
    #[serde(default = "video")]
    pub mode: String,
    #[serde(default = "single", rename = "type")]
    pub kind: String,
    pub save_path: Option<PathBuf>,
    #[serde(default)]
    pub log_to_file: bool,
    #[serde(default = "maximum")]
    pub quality: String,
    pub trim_start: Option<String>,
    pub trim_end: Option<String>,
    #[serde(default)]
    pub pass_to_flipperclipper: bool,
}

fn video() -> String {
    "video".into()
}
fn single() -> String {
    "single".into()
}
fn maximum() -> String {
    "max".into()
}

pub fn validate_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "Enter a valid video URL.")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("Use an HTTP or HTTPS video URL.".into());
    }
    Ok(())
}

impl DownloadRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_url(&self.url)?;
        if !matches!(self.mode.as_str(), "video" | "audio") {
            return Err("Unknown download mode.".into());
        }
        if !matches!(self.kind.as_str(), "single" | "playlist") {
            return Err("Unknown download type.".into());
        }
        if self.quality != "max"
            && self
                .quality
                .strip_suffix('p')
                .and_then(|s| s.parse::<u32>().ok())
                .filter(|n| (1..=16384).contains(n))
                .is_none()
        {
            return Err("Invalid video quality.".into());
        }
        self.trim()?;
        Ok(())
    }

    pub fn trim(&self) -> Result<Option<(f64, f64)>, String> {
        if self.kind == "playlist" {
            return Ok(None);
        }
        match (&self.trim_start, &self.trim_end) {
            (None, None) => Ok(None),
            (Some(start), Some(end)) if start.is_empty() && end.is_empty() => Ok(None),
            (Some(start), Some(end)) => {
                let start = timestamp(start).ok_or("Invalid trim start.")?;
                let end = timestamp(end).ok_or("Invalid trim end.")?;
                if end <= start {
                    return Err("The trim end must be after the start.".into());
                }
                Ok(Some((start, end)))
            }
            _ => Err("Provide both a trim start and end.".into()),
        }
    }
}

pub fn timestamp(value: &str) -> Option<f64> {
    let parts: Vec<_> = value.trim().split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut seconds = 0.0;
    for part in parts {
        let number: f64 = part.parse().ok()?;
        if !number.is_finite() || number < 0.0 {
            return None;
        }
        seconds = seconds * 60.0 + number;
    }
    seconds.is_finite().then_some(seconds)
}

pub fn enabled(settings: &Value, key: &str) -> bool {
    settings[key].as_bool().unwrap_or(false)
}
pub fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

pub fn format_selector(request: &DownloadRequest) -> String {
    if request.mode == "audio" {
        return "bestaudio/best".into();
    }
    match request
        .quality
        .strip_suffix('p')
        .and_then(|s| s.parse::<u32>().ok())
    {
        Some(height) => {
            format!("bestvideo[height<={height}]+bestaudio/best[height<={height}]/best")
        }
        None => "bestvideo+bestaudio/best".into(),
    }
}

pub fn options(
    request: &DownloadRequest,
    settings: &Value,
    output: &Path,
    actual_extension: Option<&str>,
    fast_trim: bool,
    encoder: &str,
) -> Result<Vec<String>, String> {
    request.validate()?;
    let mut args: Vec<String> = [
        "--ignore-config",
        "--no-playlist",
        "--no-mtime",
        "--no-embed-info-json",
        "--encoding",
        "utf-8",
        "--socket-timeout",
        "20",
        "--retries",
        "3",
        "--fragment-retries",
        "3",
        "--no-write-playlist-metafiles",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    add(&mut args, "--format", &format_selector(request));
    add(&mut args, "--paths", &output.to_string_lossy());
    add(
        &mut args,
        "--output",
        if request.mode == "audio" {
            "%(artist&{} - |)s%(title)s.%(ext)s"
        } else {
            "%(title)s.%(ext)s"
        },
    );
    let rate = settings["rate_limit_kbps"].as_u64().unwrap_or(0);
    add(
        &mut args,
        "--concurrent-fragments",
        &if rate > 0 {
            1
        } else {
            settings["concurrent_fragments"].as_u64().unwrap_or(4)
        }
        .to_string(),
    );
    if rate > 0 {
        add(
            &mut args,
            "--limit-rate",
            &rate.saturating_mul(1024).to_string(),
        );
    }
    let target = if request.mode == "audio" {
        string(settings, "audio_format")
    } else {
        string(settings, "container")
    };
    if request.mode == "audio" {
        args.push("--extract-audio".into());
        add(&mut args, "--audio-format", target);
        add(
            &mut args,
            "--audio-quality",
            string(settings, "audio_quality"),
        );
    } else {
        add(
            &mut args,
            "--merge-output-format",
            if target == "webm" { "webm/mkv" } else { target },
        );
    }
    if enabled(settings, "subtitles_enabled") {
        args.push("--write-subs".into());
        if enabled(settings, "subtitles_auto") {
            args.push("--write-auto-subs".into());
        }
        add(&mut args, "--sub-langs", string(settings, "subtitle_langs"));
        if enabled(settings, "embed_subtitles") && matches!(target, "mp4" | "mkv" | "webm") {
            args.push("--embed-subs".into());
        }
    }
    let trimmed = request.trim()?.is_some();
    if enabled(settings, "sponsorblock_enabled") && !trimmed {
        let categories: Vec<_> = settings["sponsorblock_categories"]
            .as_array()
            .map(|v| v.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !categories.is_empty() {
            add(&mut args, "--sponsorblock-remove", &categories.join(","));
        }
    }
    if enabled(settings, "embed_metadata") {
        args.push("--embed-metadata".into());
    }
    args.push(
        if enabled(settings, "embed_chapters") && !trimmed {
            "--embed-chapters"
        } else {
            "--no-embed-chapters"
        }
        .into(),
    );
    if enabled(settings, "split_chapters") && !trimmed {
        args.push("--split-chapters".into());
        add(
            &mut args,
            "--output",
            "chapter:%(title)s - %(section_number)03d %(section_title)s.%(ext)s",
        );
    }
    let extension = if request.mode == "audio" {
        target
    } else {
        actual_extension.unwrap_or(target)
    };
    if enabled(settings, "embed_thumbnail")
        && thumbnail_supported(target)
        && thumbnail_supported(extension)
    {
        args.push("--embed-thumbnail".into());
    }
    if fast_trim {
        if let Some((start, end)) = request.trim()? {
            add(&mut args, "--download-sections", &format!("*{start}-{end}"));
            let precise = enabled(settings, "precise_trim");
            args.push(
                if precise {
                    "--force-keyframes-at-cuts"
                } else {
                    "--no-force-keyframes-at-cuts"
                }
                .into(),
            );
            if precise && request.mode == "video" {
                add(
                    &mut args,
                    "--downloader-args",
                    &format!(
                        "ffmpeg_o:{}",
                        video_encode_args(extension, encoder).join(" ")
                    ),
                );
            } else if !precise {
                add(
                    &mut args,
                    "--downloader-args",
                    "ffmpeg_o:-avoid_negative_ts make_zero",
                );
            }
        }
    }
    Ok(args)
}

pub fn add(args: &mut Vec<String>, name: &str, value: &str) {
    args.extend([name.into(), value.into()]);
}

pub fn thumbnail_supported(extension: &str) -> bool {
    matches!(
        extension,
        "mp3" | "mkv" | "mka" | "m4a" | "mp4" | "m4v" | "mov" | "opus" | "flac" | "ogg"
    )
}

pub fn video_encode_args(extension: &str, encoder: &str) -> Vec<String> {
    let values: Vec<&str> = if extension.trim_start_matches('.') == "webm" {
        vec![
            "-c:v",
            "libvpx-vp9",
            "-deadline",
            "good",
            "-cpu-used",
            "5",
            "-row-mt",
            "1",
            "-crf",
            "31",
            "-b:v",
            "0",
            "-c:a",
            "libopus",
            "-b:a",
            "128k",
        ]
    } else {
        let mut result = vec!["-c:v", encoder];
        result.extend(match encoder {
            "h264_nvenc" => vec!["-rc", "vbr", "-cq", "22", "-b:v", "0"],
            "h264_qsv" => vec!["-global_quality", "22"],
            "h264_amf" => vec!["-rc", "cqp", "-qp_i", "22", "-qp_p", "22"],
            _ => vec!["-preset", "veryfast", "-crf", "22"],
        });
        result.extend(["-c:a", "aac", "-b:a", "192k"]);
        result
    };
    values.into_iter().map(String::from).collect()
}

pub fn trim_args(
    source: &Path,
    output: &Path,
    range: (f64, f64),
    precise: bool,
    encoder: &str,
) -> Vec<String> {
    let mut args = vec![
        "-nostdin".into(),
        "-y".into(),
        "-ss".into(),
        range.0.to_string(),
        "-i".into(),
        source.to_string_lossy().into_owned(),
        "-t".into(),
        (range.1 - range.0).to_string(),
    ];
    let extension = source.extension().and_then(|s| s.to_str()).unwrap_or("");
    let audio: Option<Vec<&str>> = match extension {
        "mp3" => Some(vec!["-vn", "-c:a", "libmp3lame", "-b:a", "192k"]),
        "m4a" | "aac" => Some(vec!["-vn", "-c:a", "aac", "-b:a", "192k"]),
        "flac" => Some(vec!["-vn", "-c:a", "flac"]),
        "opus" => Some(vec!["-vn", "-c:a", "libopus", "-b:a", "128k"]),
        "ogg" => Some(vec!["-vn", "-c:a", "libvorbis", "-q:a", "5"]),
        "wav" => Some(vec!["-vn", "-c:a", "pcm_s16le"]),
        _ => None,
    };
    if let Some(audio) = audio {
        args.extend(audio.into_iter().map(String::from));
    } else if precise {
        args.extend(video_encode_args(extension, encoder));
    } else {
        args.extend(
            ["-c", "copy", "-avoid_negative_ts", "make_zero"]
                .into_iter()
                .map(String::from),
        );
    }
    args.extend([
        "-map_metadata".into(),
        "0".into(),
        output.to_string_lossy().into_owned(),
    ]);
    args
}

pub fn metadata_response(info: &Value) -> Value {
    let playlist = info.get("entries").is_some() || string(info, "_type") == "playlist";
    let entries = info["entries"].as_array().cloned().unwrap_or_default();
    let size = estimate_size(&info["formats"]);
    json!({"title":info["title"].as_str().unwrap_or("Unknown Title"),"duration":info["duration"].as_f64().unwrap_or(0.0),
        "thumbnail":info["thumbnail"].as_str().unwrap_or(""),"is_playlist":playlist,"formats":info["formats"].as_array().cloned().unwrap_or_default(),
        "entries_count":if playlist { entries.len() } else { 1 },"entries":entries.into_iter().filter(Value::is_object).take(50).map(|entry| json!({"id":entry["id"],"title":entry["title"],"duration":entry["duration"],"thumbnail":entry["thumbnail"]})).collect::<Vec<_>>(),
        "size":if playlist { None } else { size },"size_formatted":if playlist { "Varies per video".into() } else { format_size(size) }})
}

fn estimate_size(formats: &Value) -> Option<u64> {
    let formats = formats.as_array()?;
    let video = formats
        .iter()
        .filter(|f| !matches!(string(f, "vcodec"), "" | "none"))
        .max_by_key(|f| f["height"].as_u64().unwrap_or(0));
    let audio = formats
        .iter()
        .filter(|f| {
            matches!(string(f, "vcodec"), "" | "none")
                && !matches!(string(f, "acodec"), "" | "none")
        })
        .max_by_key(|f| f["abr"].as_f64().unwrap_or(0.0) as u64);
    let bytes = |f: &Value| {
        f["filesize"]
            .as_u64()
            .or_else(|| f["filesize_approx"].as_u64())
            .unwrap_or(0)
    };
    let size = video.map(bytes).unwrap_or(0)
        + if video.is_some_and(|f| !matches!(string(f, "acodec"), "" | "none")) {
            0
        } else {
            audio.map(bytes).unwrap_or(0)
        };
    (size > 0).then_some(size)
}

pub fn format_size(bytes: Option<u64>) -> String {
    match bytes {
        None | Some(0) => "Unknown".into(),
        Some(n) if n < 1024 * 1024 => format!("{:.1} KB", n as f64 / 1024.0),
        Some(n) if n < 1024 * 1024 * 1024 => format!("{:.1} MB", n as f64 / (1024.0 * 1024.0)),
        Some(n) => format!("{:.2} GB", n as f64 / (1024.0 * 1024.0 * 1024.0)),
    }
}

pub fn partial_refusal(message: &str) -> bool {
    message.contains("cannot be partially downloaded")
        || message.contains("partially, but ffmpeg is not installed")
        || [
            [248, b'4', b'0', b'0'],
            [248, b'4', b'0', b'1'],
            [248, b'4', b'0', b'3'],
            [248, b'4', b'0', b'4'],
            [248, b'4', b'2', b'9'],
            [248, b'4', b'X', b'X'],
            [248, b'5', b'X', b'X'],
            *b"EOF ",
        ]
        .iter()
        .any(|tag| {
            message.contains(&format!(
                "ffmpeg exited with code {}",
                u32::from_le_bytes(*tag).wrapping_neg()
            ))
        })
        || message.contains("Output file is empty")
        || message.contains("Stream ends prematurely")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> DownloadRequest {
        serde_json::from_value(
            json!({"url":"https://example.com/clip","trim_start":"3","trim_end":"7"}),
        )
        .unwrap()
    }
    fn settings() -> Value {
        json!({"container":"mp4","audio_format":"mp3","audio_quality":"0","precise_trim":true,"embed_thumbnail":true,"embed_metadata":true,"embed_chapters":true,"split_chapters":true,"sponsorblock_enabled":true,"sponsorblock_categories":["sponsor"],"concurrent_fragments":8,"rate_limit_kbps":12})
    }

    #[test]
    fn range_uses_actual_webm_codec_and_omits_full_timeline_operations() {
        let args = options(
            &request(),
            &settings(),
            Path::new("dest"),
            Some("webm"),
            true,
            "h264_nvenc",
        )
        .unwrap();
        let value = args[args.iter().position(|s| s == "--downloader-args").unwrap() + 1].clone();
        assert!(value.contains("libvpx-vp9"));
        assert!(!value.contains("h264"));
        for option in [
            "--embed-thumbnail",
            "--embed-chapters",
            "--split-chapters",
            "--sponsorblock-remove",
        ] {
            assert!(!args.iter().any(|s| s == option), "{option}");
        }
        assert!(args
            .windows(2)
            .any(|w| w == ["--download-sections", "*3-7"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--concurrent-fragments", "1"]));
        assert!(args.windows(2).any(|w| w == ["--limit-rate", "12288"]));
    }

    #[test]
    fn full_download_respects_chapter_and_sponsor_settings() {
        let mut request = request();
        request.trim_start = None;
        request.trim_end = None;
        let args = options(
            &request,
            &settings(),
            Path::new("dest"),
            Some("mp4"),
            false,
            "libx264",
        )
        .unwrap();
        for option in [
            "--embed-thumbnail",
            "--embed-chapters",
            "--split-chapters",
            "--sponsorblock-remove",
        ] {
            assert!(args.iter().any(|s| s == option));
        }
        assert!(!args.iter().any(|s| s == "--force-keyframes-at-cuts"));
    }

    #[test]
    fn invalid_or_incomplete_ranges_fail_before_download() {
        for value in ["-1", "NaN", "inf", "00::3", "1:2:3:4"] {
            assert!(timestamp(value).is_none(), "{value}");
        }
        assert_eq!(timestamp("1:02:03.5"), Some(3723.5));
        let mut request = request();
        request.trim_end = Some("2".into());
        assert!(request.validate().is_err());
        request.kind = "playlist".into();
        assert_eq!(request.trim().unwrap(), None);
    }

    #[test]
    fn local_trim_seeks_before_decode_and_copy_starts_at_zero() {
        let args = trim_args(
            Path::new("clip.mp4"),
            Path::new("trim.mp4"),
            (3.0, 7.0),
            false,
            "libx264",
        );
        assert!(args.iter().position(|s| s == "-ss") < args.iter().position(|s| s == "-i"));
        assert!(args
            .windows(2)
            .any(|w| w == ["-avoid_negative_ts", "make_zero"]));
    }

    #[test]
    fn null_playlist_entries_are_safe_and_combined_size_is_not_doubled() {
        let response = metadata_response(&json!({"entries":[null,{"title":"one"}]}));
        assert_eq!(response["entries_count"], 2);
        assert_eq!(response["entries"].as_array().unwrap().len(), 1);
        let response = metadata_response(
            &json!({"formats":[{"vcodec":"h264","acodec":"aac","filesize":1000},{"vcodec":"none","acodec":"aac","filesize":400}]}),
        );
        assert_eq!(response["size"], 1000);
    }

    #[test]
    fn only_fetch_failures_trigger_full_download_retry() {
        for code in [
            3486501640u32,
            3469724424,
            3436169992,
            3419392776,
            3335375624,
            2812791560,
            2812791304,
            3753488571,
        ] {
            assert!(partial_refusal(&format!(
                "ERROR: ffmpeg exited with code {code}"
            )));
        }
        for message in [
            "ERROR: ffmpeg exited with code 4294967274",
            "HTTP Error 403",
            "This video is not available in your country",
        ] {
            assert!(!partial_refusal(message));
        }
    }
}
