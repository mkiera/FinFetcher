#![cfg(windows)]

use finfetcher::{downloads::Downloads, process, settings::Settings, tools::Tools};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
};

const FIXTURE_SOURCE: &str = r#"
use std::{fs::{self,OpenOptions},io::Write,path::PathBuf,time::Duration};
fn quoted(value:&str)->String {
    let mut result=String::from("\"");
    for character in value.chars() {
        match character { '\\'=>result.push_str("\\\\"), '"'=>result.push_str("\\\""), other=>result.push(other) }
    }
    result.push('"'); result
}
fn value<'a>(args:&'a [String],name:&str)->Option<&'a str> {
    args.iter().position(|arg|arg==name).and_then(|index|args.get(index+1)).map(String::as_str)
}
fn main() {
    let executable=std::env::current_exe().unwrap();
    let name=executable.file_stem().unwrap().to_string_lossy().into_owned();
    let args:Vec<String>=std::env::args().skip(1).collect();
    if args.iter().any(|arg|matches!(arg.as_str(),"--version"|"-version")) {
        println!("{}",match name.as_str() {"ffmpeg"=>"ffmpeg version playlist-fixture","ffprobe"=>"ffprobe version playlist-fixture",_=>"9999.12.31"});
        return;
    }
    if name=="ffprobe" {
        println!("{{\"streams\":[{{\"codec_type\":\"video\",\"nb_read_packets\":\"1\"}}]}}");
        return;
    }
    if name=="ffmpeg" { std::process::exit(1); }
    let root=executable.ancestors().find(|parent|parent.join("fixture.conf").is_file()).unwrap().to_owned();
    let delay:u64=fs::read_to_string(root.join("fixture.conf")).unwrap().trim().parse().unwrap();
    if args.iter().any(|arg|arg=="--flat-playlist") {
        std::thread::sleep(Duration::from_secs(delay));
        let entries:Vec<String>=["one","private","channel","three"].iter().map(|id|format!("{{\"_type\":\"url\",\"id\":\"{id}\",\"url\":\"https://example.test/{id}\"}}")).collect();
        println!("{{\"_type\":\"playlist\",\"id\":\"list\",\"title\":\"Fixture list\",\"entries\":[{}]}}",entries.join(","));
        return;
    }
    let stage=PathBuf::from(value(&args,"--paths").expect("Missing output folder"));
    if args.iter().any(|arg|arg=="--simulate") {
        let url=args.last().unwrap();
        let id=url.rsplit('/').next().unwrap();
        match id {
            "private"=>{ eprintln!("ERROR: [generic] private: Private video"); std::process::exit(1); }
            "channel"=>{ println!("{{\"_type\":\"playlist\",\"id\":\"channel\",\"title\":\"Fixture - Videos\",\"entries\":[]}}"); }
            _=>println!("{{\"id\":\"{id}\",\"title\":\"{id}\",\"ext\":\"mp4\",\"extractor_key\":\"Generic\",\"_filename\":{},\"webpage_url\":\"{url}\"}}",quoted(&stage.join(format!("{id}.mp4")).to_string_lossy())),
        }
        return;
    }
    let info=fs::read_to_string(value(&args,"--load-info-json").unwrap()).unwrap();
    let start=info.find("\"id\":\"").unwrap()+6;
    let id=&info[start..start+info[start..].find('"').unwrap()];
    let target=stage.join(format!("{id}.mp4"));
    fs::write(&target,b"fixture media").unwrap();
    let mut log=OpenOptions::new().create(true).append(true).open(root.join("downloads.log")).unwrap();
    writeln!(log,"{id}").unwrap();
    println!("FINFETCHER_FILE:{}",quoted(&target.to_string_lossy()));
}
"#;

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    destination: PathBuf,
    engine: Arc<Downloads>,
}

fn new_fixture(listing_delay_seconds: u64) -> Fixture {
    let directory = tempfile::Builder::new()
        .prefix("finfetcher-playlist-")
        .tempdir()
        .unwrap();
    let executable = directory.path().join("playlist-fixture.exe");
    let mut compiler = process::command("rustc")
        .args([
            "--crate-name",
            "playlist_fixture",
            "--edition",
            "2021",
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
    let root = directory.path().join("data");
    let settings = Arc::new(Settings::new(root.clone()).unwrap());
    settings
        .save(
            &json!({"auto_update_ytdlp":false,"embed_thumbnail":false,"embed_metadata":false,
            "embed_chapters":false,"embed_subtitles":false}),
        )
        .unwrap();
    fs::write(root.join("fixture.conf"), listing_delay_seconds.to_string()).unwrap();
    for (folder, name) in [
        ("ytdlp-bin/fixture", "yt-dlp.exe"),
        ("ffmpeg", "ffmpeg.exe"),
        ("ffmpeg", "ffprobe.exe"),
    ] {
        fs::create_dir_all(root.join(folder)).unwrap();
        fs::copy(&executable, root.join(folder).join(name)).unwrap();
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
    assert_eq!(tools.ffprobe().unwrap(), root.join("ffmpeg/ffprobe.exe"));
    let engine = Arc::new(Downloads::new(tools, settings).without_browser_cookies());
    let destination = directory.path().join("output");
    fs::create_dir(&destination).unwrap();
    Fixture {
        _directory: directory,
        root,
        destination,
        engine,
    }
}

fn download(fixture: &Fixture, cancel_at: Option<&'static str>) -> Vec<Value> {
    let job = fixture
        .engine
        .begin(json!({"url":"https://example.test/list","type":"playlist",
            "save_path":fixture.destination}))
        .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let engine = fixture.engine.clone();
    fixture.engine.run(
        job,
        Arc::new(move |event: Value| {
            if cancel_at.is_some_and(|text| event["log"].as_str().is_some_and(|l| l.contains(text)))
            {
                engine.cancel();
            }
            captured.lock().unwrap().push(event);
        }),
    );
    assert!(!fixture.engine.busy());
    Arc::try_unwrap(events).unwrap().into_inner().unwrap()
}

fn downloaded(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("downloads.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn logged(events: &[Value], text: &str) -> bool {
    events.iter().any(|event| {
        event["log"]
            .as_str()
            .or_else(|| event["error"].as_str())
            .is_some_and(|line| line.contains(text))
    })
}

#[test]
fn failing_playlist_entries_do_not_stop_later_entries() {
    let fixture = new_fixture(0);
    let events = download(&fixture, None);
    let report = serde_json::to_string_pretty(&events).unwrap();
    assert_eq!(downloaded(&fixture.root), ["one", "three"], "{report}");
    assert!(fixture.destination.join("one.mp4").is_file());
    assert!(fixture.destination.join("three.mp4").is_file());
    assert!(logged(&events, "Private video"), "{report}");
    assert!(
        logged(&events, "\"Fixture - Videos\" is a playlist"),
        "{report}"
    );
    assert_eq!(events.last().unwrap()["status"], "error", "{report}");
    let error = events
        .iter()
        .rev()
        .find_map(|event| event["error"].as_str())
        .unwrap();
    assert!(error.starts_with("2 of 4 videos"), "{error}");

    let fresh = new_fixture(0);
    let events = download(&fresh, Some("Downloading video 1 of 4"));
    assert_eq!(events.last().unwrap()["status"], "cancelled");
    assert!(downloaded(&fresh.root).is_empty());
    assert!(!logged(&events, "Downloading video 2 of 4"));
}

#[test]
#[ignore = "Waits 50 seconds for a slow isolated playlist listing."]
fn long_playlist_listings_are_not_cut_off_at_45_seconds() {
    let fixture = new_fixture(50);
    let engine = fixture.engine.clone();
    let preview = std::thread::spawn(move || engine.inspect("https://example.test/list", false));
    let events = download(&fixture, None);
    let info = preview.join().unwrap().unwrap();
    assert_eq!(info["entries_count"], 4);
    assert_eq!(downloaded(&fixture.root), ["one", "three"]);
    assert!(!logged(&events, "timed out"));
}
