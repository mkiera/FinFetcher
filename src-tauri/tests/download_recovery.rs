#![cfg(windows)]

use finfetcher::{downloads::Downloads, process, settings::Settings, tools::Tools};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FIXTURE_SOURCE: &str = r#"
use std::{fs::{self,OpenOptions},io::Write,path::{Path,PathBuf},process::{Command,Stdio},time::{Duration,Instant}};
use std::os::windows::process::CommandExt;
fn quoted(value:&str)->String {
    let mut result=String::from("\"");
    for character in value.chars() {
        match character {
            '\\'=>result.push_str("\\\\"), '"'=>result.push_str("\\\""),
            '\n'=>result.push_str("\\n"), '\r'=>result.push_str("\\r"),
            '\t'=>result.push_str("\\t"), other=>result.push(other),
        }
    }
    result.push('"'); result
}
fn value<'a>(args:&'a [String],name:&str)->Option<&'a str> {
    args.iter().position(|arg|arg==name).and_then(|index|args.get(index+1)).map(String::as_str)
}
fn append(path:&Path,line:&str) {
    let mut file=OpenOptions::new().create(true).append(true).open(path).unwrap();
    writeln!(file,"{line}").unwrap(); file.flush().unwrap();
}
fn main() {
    let executable=std::env::current_exe().unwrap();
    let name=executable.file_stem().unwrap().to_string_lossy();
    let args:Vec<String>=std::env::args().skip(1).collect();
    if args.iter().any(|arg|matches!(arg.as_str(),"--version"|"-version")) {
        println!("{}",match name.as_ref() {"ffmpeg"=>"ffmpeg version recovery-fixture","ffprobe"=>"ffprobe version recovery-fixture","deno"=>"deno 2.9.7",_=>"9999.12.31"});
        return;
    }
    let root=executable.ancestors().find(|parent|parent.join("fixture.conf").is_file()).unwrap();
    let configuration=fs::read_to_string(root.join("fixture.conf")).unwrap();
    let fields:Vec<_>=configuration.lines().collect();
    let scenario=fields[0];
    if name=="ffprobe" {
        let code=Command::new(fields[4]).creation_flags(0x08000000).args(&args).stdin(Stdio::null()).status().unwrap();
        std::process::exit(code.code().unwrap_or(1));
    }
    if name=="ffmpeg" {
        let encoder=value(&args,"-c:v").unwrap_or("none");
        if args.iter().any(|arg|arg=="nullsrc=s=256x144") {
            std::process::exit(if scenario.starts_with("hardware_") && encoder=="h264_nvenc" {0}else{1});
        }
        append(&root.join("local-trims.log"),&format!("{{\"encoder\":{},\"input\":{}}}",quoted(encoder),quoted(value(&args,"-i").unwrap_or(""))));
        if scenario=="processing_cancel" {
            eprintln!("FINFETCHER_FIXTURE_TRIM_STARTED");
            std::io::stderr().flush().unwrap();
            let started=Instant::now();
            while started.elapsed()<Duration::from_secs(8) {
                append(&root.join("processor.heartbeat"),"working");
                std::thread::sleep(Duration::from_millis(10));
            }
            std::process::exit(9);
        }
        if scenario=="processing_failure" || scenario=="hardware_local" && encoder!="libx264" {
            eprintln!("Encoder initialization failed for {encoder}"); std::process::exit(9);
        }
        let code=Command::new(fields[3]).creation_flags(0x08000000).args(&args).stdin(Stdio::null()).status().unwrap();
        std::process::exit(code.code().unwrap_or(1));
    }
    let stage=PathBuf::from(value(&args,"--paths").expect("Missing output folder"));
    let target=stage.join("clip.mp4");
    if args.iter().any(|arg|arg=="--dump-single-json") {
        println!("{{\"id\":\"fixture\",\"title\":\"clip\",\"ext\":\"mp4\",\"duration\":12,\"_filename\":{},\"webpage_url\":\"https://example.test/fixture\"}}",quoted(&target.to_string_lossy()));
        return;
    }
    let previous=fs::read_to_string(root.join("attempts.log")).unwrap_or_default();
    let attempt=previous.lines().count()+1;
    let fast=value(&args,"--download-sections").is_some();
    let downloader=value(&args,"--downloader-args").unwrap_or("");
    let encoder=if downloader.contains("h264_nvenc") {"h264_nvenc"}else{"libx264"};
    append(&root.join("attempts.log"),&format!("{{\"attempt\":{attempt},\"fast\":{fast},\"encoder\":{}}}",quoted(encoder)));
    if attempt>1 && stage.join("failed.part").exists() {
        eprintln!("The previous failed attempt was not cleared"); std::process::exit(12);
    }
    if attempt==1 && matches!(scenario,"http_range"|"range_retry"|"hardware_range") {
        fs::write(stage.join("failed.part"),b"partial previous download").unwrap();
        eprintln!("{}",match scenario {"http_range"=>"ERROR: ffmpeg exited with code 3436169992","range_retry"=>"HTTP Error 403",_=>"Hardware encoder initialization failed"});
        std::process::exit(1);
    }
    if attempt==1 && scenario=="empty_range" {
        fs::write(&target,b"").unwrap();
    } else if attempt==1 && scenario=="header_range" {
        fs::copy(fields[5],&target).unwrap();
    } else {
        fs::copy(if fast {fields[2]}else{fields[1]},&target).unwrap();
    }
    println!("FINFETCHER_PROGRESS:{{\"status\":\"finished\",\"downloaded_bytes\":{},\"total_bytes\":{}}}",fs::metadata(&target).unwrap().len(),fs::metadata(&target).unwrap().len());
    println!("FINFETCHER_POSTPROCESS:{{\"status\":\"finished\"}}");
    println!("FINFETCHER_FILE:{}",quoted(&target.to_string_lossy()));
}
"#;

struct Fixtures {
    directory: tempfile::TempDir,
    executable: PathBuf,
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    full: PathBuf,
    range: PathBuf,
    empty_header: PathBuf,
}

impl Fixtures {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("finfetcher-recovery-fixtures-")
            .tempdir()
            .unwrap();
        let settings = Arc::new(Settings::new(directory.path().join("discovery")).unwrap());
        let tools = Tools::new(settings.root().to_owned(), settings);
        let ffmpeg = tools
            .ffmpeg()
            .expect("Recovery tests require real FFmpeg on PATH");
        let ffprobe = tools
            .ffprobe()
            .expect("Recovery tests require real FFprobe on PATH");
        let executable = directory.path().join("recovery-fixture.exe");
        let mut compiler = process::command("rustc")
            .args([
                "--crate-name",
                "download_recovery_fixture",
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
            .write_all(FIXTURE_SOURCE.as_bytes())
            .unwrap();
        let compiled = compiler.wait_with_output().unwrap();
        assert!(
            compiled.status.success(),
            "Fixture compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let full = directory.path().join("full.mp4");
        let range = directory.path().join("range.mp4");
        let empty_header = directory.path().join("empty-header.mp4");
        checked(
            &ffmpeg,
            &[
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=30:duration=12",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=12",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-g",
                "30",
                "-c:a",
                "aac",
                "-shortest",
                &full.to_string_lossy(),
            ],
        );
        checked(
            &ffmpeg,
            &[
                "-v",
                "error",
                "-ss",
                "3",
                "-i",
                &full.to_string_lossy(),
                "-t",
                "4",
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-c:a",
                "aac",
                &range.to_string_lossy(),
            ],
        );
        checked(
            &ffmpeg,
            &[
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=160x90:r=30",
                "-t",
                "0",
                "-c:v",
                "libx264",
                "-movflags",
                "empty_moov+default_base_moof+frag_keyframe",
                &empty_header.to_string_lossy(),
            ],
        );
        Self {
            directory,
            executable,
            ffmpeg,
            ffprobe,
            full,
            range,
            empty_header,
        }
    }

    fn engine(&self, scenario: &str) -> (tempfile::TempDir, Arc<Downloads>, PathBuf) {
        let directory = tempfile::Builder::new()
            .prefix(&format!("{scenario}-"))
            .tempdir_in(self.directory.path())
            .unwrap();
        let root = directory.path().join("data");
        let settings = Arc::new(Settings::new(root.clone()).unwrap());
        settings.save(&json!({"auto_update_ytdlp":false,"embed_thumbnail":false,"embed_metadata":false,
            "embed_chapters":false,"fast_trim":!matches!(scenario,"hardware_local"|"processing_failure"|"processing_cancel"),
            "precise_trim":true})).unwrap();
        fs::write(
            root.join("fixture.conf"),
            format!(
                "{scenario}\n{}\n{}\n{}\n{}\n{}\n",
                self.full.display(),
                self.range.display(),
                self.ffmpeg.display(),
                self.ffprobe.display(),
                self.empty_header.display()
            ),
        )
        .unwrap();
        for (folder, name) in [
            ("ytdlp-bin/fixture", "yt-dlp.exe"),
            ("ffmpeg", "ffmpeg.exe"),
            ("ffmpeg", "ffprobe.exe"),
            ("deno", "deno.exe"),
        ] {
            let destination = root.join(folder);
            fs::create_dir_all(&destination).unwrap();
            fs::copy(&self.executable, destination.join(name)).unwrap();
        }
        fs::write(
            root.join("ytdlp-bin/active.json"),
            br#"{"directory":"fixture","version":"9999.12.31"}"#,
        )
        .unwrap();
        let tools = Arc::new(Tools::new(root.clone(), settings.clone()));
        assert_eq!(
            tools.ytdlp().unwrap(),
            root.join("ytdlp-bin/fixture/yt-dlp.exe")
        );
        assert_eq!(tools.ffmpeg().unwrap(), root.join("ffmpeg/ffmpeg.exe"));
        let engine = Arc::new(Downloads::new(tools, settings).without_browser_cookies());
        let destination = directory.path().join("output");
        fs::create_dir(&destination).unwrap();
        (directory, engine, destination)
    }

    fn assert_four_second_media(&self, path: &Path) {
        let metadata: Value = serde_json::from_slice(&checked(
            &self.ffprobe,
            &[
                "-v",
                "error",
                "-count_frames",
                "-show_streams",
                "-show_format",
                "-of",
                "json",
                &path.to_string_lossy(),
            ],
        ))
        .unwrap();
        let duration = metadata["format"]["duration"]
            .as_str()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or_default();
        assert!(
            (duration - 4.0).abs() < 0.15,
            "Wrong recovered duration: {metadata}"
        );
        let streams = metadata["streams"].as_array().unwrap();
        let video = streams
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .unwrap();
        assert_eq!(
            video["nb_read_frames"], "120",
            "The recovered video is incomplete: {metadata}"
        );
        assert!(streams.iter().any(|stream| stream["codec_type"] == "audio"));
        let frame = |source: &Path| {
            checked(
                &self.ffmpeg,
                &[
                    "-v",
                    "error",
                    "-i",
                    &source.to_string_lossy(),
                    "-frames:v",
                    "1",
                    "-vf",
                    "scale=32:18",
                    "-pix_fmt",
                    "gray",
                    "-f",
                    "rawvideo",
                    "-",
                ],
            )
        };
        let expected = frame(&self.range);
        let actual = frame(path);
        assert_eq!(actual.len(), expected.len());
        let difference: f64 = actual
            .iter()
            .zip(expected.iter())
            .map(|(a, b)| a.abs_diff(*b) as f64)
            .sum::<f64>()
            / actual.len() as f64;
        assert!(
            difference < 4.0,
            "The clip starts at the wrong source time, mean pixel difference {difference}"
        );
    }
}

fn checked(program: &Path, args: &[&str]) -> Vec<u8> {
    let output = process::command(program).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{} failed: {}",
        program.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn records(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn execute(engine: &Arc<Downloads>, destination: &Path, cancel_processing: bool) -> Vec<Value> {
    let job = engine
        .begin(
            json!({"url":"https://example.test/fixture","save_path":destination,
        "trim_start":"3","trim_end":"7"}),
        )
        .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let responding = engine.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let done = finished.clone();
    let cancelling = engine.clone();
    let watchdog = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        if !done.load(Ordering::Acquire) {
            cancelling.cancel();
            return true;
        }
        false
    });
    engine.run(
        job,
        Arc::new(move |event: Value| {
            if let Some(request) = event.get("destination_request") {
                responding
                    .destination(json!({"id":request["id"],"action":"overwrite"}))
                    .unwrap();
            }
            if cancel_processing
                && event["log"]
                    .as_str()
                    .is_some_and(|line| line.contains("FINFETCHER_FIXTURE_TRIM_STARTED"))
            {
                responding.cancel();
            }
            captured.lock().unwrap().push(event);
        }),
    );
    finished.store(true, Ordering::Release);
    assert!(
        !watchdog.join().unwrap(),
        "The recovery fixture did not finish within 30 seconds"
    );
    assert!(!engine.busy());
    Arc::try_unwrap(events).unwrap().into_inner().unwrap()
}

fn assert_status(events: &[Value], expected: &str, scenario: &str) {
    assert_eq!(
        events.last().unwrap()["status"],
        expected,
        "{scenario}: {}",
        serde_json::to_string_pretty(events).unwrap()
    );
}

#[test]
#[ignore = "Requires real FFmpeg and FFprobe on PATH. Uses generated media and isolated Rust executables without network."]
fn recovery_paths_preserve_media_and_existing_destinations() {
    let fixtures = Fixtures::new();
    for scenario in [
        "header_range",
        "http_range",
        "range_retry",
        "hardware_range",
        "hardware_local",
        "empty_range",
    ] {
        let (directory, engine, destination) = fixtures.engine(scenario);
        let events = execute(&engine, &destination, false);
        assert_status(&events, "completed", scenario);
        let output = destination.join("clip.mp4");
        fixtures.assert_four_second_media(&output);
        assert_eq!(
            fs::read_dir(&destination).unwrap().count(),
            1,
            "{scenario} left staged media behind"
        );
        let attempts = records(&directory.path().join("data/attempts.log"));
        let trims = records(&directory.path().join("data/local-trims.log"));
        match scenario {
            "range_retry" | "hardware_range" => {
                assert_eq!(attempts.len(), 2, "{scenario}: {attempts:?}");
                assert!(attempts.iter().all(|attempt| attempt["fast"] == true));
                assert!(
                    trims.is_empty(),
                    "A successful range was trimmed twice: {trims:?}"
                );
                assert_eq!(
                    fs::read(&output).unwrap(),
                    fs::read(&fixtures.range).unwrap()
                );
                if scenario == "hardware_range" {
                    assert_eq!(attempts[0]["encoder"], "h264_nvenc");
                    assert_eq!(attempts[1]["encoder"], "libx264");
                }
            }
            "hardware_local" => {
                assert_eq!(attempts.len(), 1);
                assert_eq!(attempts[0]["fast"], false);
                assert_eq!(trims.len(), 2);
                assert_eq!(trims[0]["encoder"], "h264_nvenc");
                assert_eq!(trims[1]["encoder"], "libx264");
            }
            _ => {
                assert_eq!(attempts.len(), 2, "{scenario}: {attempts:?}");
                assert_eq!(attempts[0]["fast"], true);
                assert_eq!(attempts[1]["fast"], false);
                assert_eq!(trims.len(), 1, "{scenario}: {trims:?}");
            }
        }
        eprintln!(
            "Verified {scenario}: {} attempts, {} local trims, valid 4-second media",
            attempts.len(),
            trims.len()
        );
    }
    for scenario in ["processing_failure", "processing_cancel"] {
        let (directory, engine, destination) = fixtures.engine(scenario);
        let target = destination.join("clip.mp4");
        fs::write(&target, b"original user file").unwrap();
        let cancelled = scenario == "processing_cancel";
        let events = execute(&engine, &destination, cancelled);
        assert_status(
            &events,
            if cancelled { "cancelled" } else { "error" },
            scenario,
        );
        assert!(events
            .iter()
            .any(|event| event.get("destination_request").is_some()));
        assert_eq!(fs::read(&target).unwrap(), b"original user file");
        assert_eq!(
            records(&directory.path().join("data/attempts.log")).len(),
            1
        );
        assert_eq!(
            records(&directory.path().join("data/local-trims.log")).len(),
            1
        );
        let remaining: Vec<_> = fs::read_dir(&destination)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        if cancelled {
            assert_eq!(
                remaining.len(),
                1,
                "Cancellation retained staged media: {remaining:?}"
            );
            let heartbeat = directory.path().join("data/processor.heartbeat");
            let stopped = fs::read(&heartbeat).unwrap_or_default();
            std::thread::sleep(Duration::from_millis(250));
            assert_eq!(
                fs::read(&heartbeat).unwrap_or_default(),
                stopped,
                "The cancelled processor kept writing"
            );
        } else {
            let retained: Vec<_> = remaining.iter().filter(|path| path.is_dir()).collect();
            assert_eq!(
                retained.len(),
                1,
                "The full download was not retained after trimming failed"
            );
            assert_eq!(
                fs::read(retained[0].join("clip.mp4")).unwrap(),
                fs::read(&fixtures.full).unwrap()
            );
            assert!(events.iter().any(|event| event["error"]
                .as_str()
                .is_some_and(|error| error.contains("full download was retained"))));
        }
        eprintln!("Verified {scenario}: original destination survived approved overwrite");
    }
}
