use finfetcher::{downloads::Downloads, process, settings::Settings, tools::Tools};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct FixtureServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    ranges: Arc<AtomicUsize>,
    worker: Option<JoinHandle<()>>,
}

impl FixtureServer {
    fn new(sources: HashMap<String, Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let ranges = Arc::new(AtomicUsize::new(0));
        let origin = format!("http://{address}");
        let sources = Arc::new(
            sources
                .into_iter()
                .map(|(path, bytes)| {
                    let bytes = if path.ends_with(".html") {
                        String::from_utf8(bytes)
                            .unwrap()
                            .replace("{{fixture_origin}}", &origin)
                            .into_bytes()
                    } else {
                        bytes
                    };
                    (path, bytes)
                })
                .collect::<HashMap<_, _>>(),
        );
        let stopping = stop.clone();
        let request_count = requests.clone();
        let range_count = ranges.clone();
        let worker = thread::spawn(move || {
            let mut connections = Vec::new();
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let sources = sources.clone();
                        let requests = request_count.clone();
                        let ranges = range_count.clone();
                        connections.push(thread::spawn(move || {
                            if let Err(error) = serve(stream, &sources, &requests, &ranges) {
                                if !matches!(
                                    error.kind(),
                                    std::io::ErrorKind::BrokenPipe
                                        | std::io::ErrorKind::ConnectionReset
                                        | std::io::ErrorKind::ConnectionAborted
                                ) {
                                    eprintln!("Fixture connection failed: {error}");
                                }
                            }
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("Fixture listener failed: {error}"),
                }
            }
            for connection in connections {
                connection.join().unwrap();
            }
        });
        Self {
            address,
            stop,
            requests,
            ranges,
            worker: Some(worker),
        }
    }

    fn url(&self, extension: &str) -> String {
        format!("http://{}/source.{extension}", self.address)
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn serve(
    mut stream: TcpStream,
    sources: &HashMap<String, Vec<u8>>,
    requests: &AtomicUsize,
    ranges: &AtomicUsize,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let mut words = first.split_whitespace();
    let method = words.next().unwrap_or("");
    let path = words.next().unwrap_or("").split('?').next().unwrap_or("");
    let mut range = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header == "\r\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("range") {
                range = Some(value.trim().to_string());
            }
        }
    }
    requests.fetch_add(1, Ordering::Relaxed);
    let Some(bytes) = sources.get(path) else {
        stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )?;
        return Ok(());
    };
    let mut start = 0;
    let mut end = bytes.len() - 1;
    if let Some(range) = &range {
        ranges.fetch_add(1, Ordering::Relaxed);
        let value = range.strip_prefix("bytes=").unwrap_or("");
        let Some((left, right)) = value.split_once('-') else {
            stream.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
            return Ok(());
        };
        if left.is_empty() {
            start = bytes.len().saturating_sub(right.parse().unwrap_or(0));
        } else {
            start = left.parse().unwrap_or(bytes.len());
            if !right.is_empty() {
                end = right.parse::<usize>().unwrap_or(end).min(end);
            }
        }
    }
    if start > end {
        write!(stream, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", bytes.len())?;
        return Ok(());
    }
    let status = if range.is_some() {
        "206 Partial Content"
    } else {
        "200 OK"
    };
    let content_type = if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".vtt") {
        "text/vtt; charset=utf-8"
    } else if path.ends_with(".jpg") {
        "image/jpeg"
    } else if path.ends_with(".webm") {
        "video/webm"
    } else {
        "video/mp4"
    };
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n", end - start + 1)?;
    if range.is_some() {
        write!(
            stream,
            "Content-Range: bytes {start}-{end}/{}\r\n",
            bytes.len()
        )?;
    }
    stream.write_all(b"\r\n")?;
    if method != "HEAD" {
        stream.write_all(&bytes[start..=end])?;
    }
    Ok(())
}

fn command_bytes(program: &Path, arguments: &[&str]) -> Vec<u8> {
    let output = process::command(program)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("Could not run {}: {error}", program.display()));
    assert!(
        output.status.success(),
        "{} failed: {}",
        program.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn generate_source(ffmpeg: &Path, source: &Path, extension: &str) {
    let mut arguments = vec![
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x180:rate=30:duration=10",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=10",
    ];
    arguments.extend(if extension == "mp4" {
        vec![
            "-c:v",
            "libx264",
            "-g",
            "150",
            "-keyint_min",
            "150",
            "-sc_threshold",
            "0",
            "-c:a",
            "aac",
        ]
    } else {
        vec![
            "-c:v",
            "libvpx-vp9",
            "-g",
            "150",
            "-cpu-used",
            "5",
            "-c:a",
            "libopus",
        ]
    });
    let source = source.to_string_lossy();
    arguments.push(&source);
    command_bytes(ffmpeg, &arguments);
}

fn probe(ffprobe: &Path, path: &Path) -> Value {
    serde_json::from_slice(&command_bytes(
        ffprobe,
        &[
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
            &path.to_string_lossy(),
        ],
    ))
    .unwrap()
}

fn media_duration(info: &Value) -> f64 {
    info["format"]["duration"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn first_frame(ffmpeg: &Path, path: &Path, seek: &str) -> Vec<u8> {
    command_bytes(
        ffmpeg,
        &[
            "-v",
            "error",
            "-ss",
            seek,
            "-i",
            &path.to_string_lossy(),
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
}

fn assert_moving_video(ffmpeg: &Path, ffprobe: &Path, output: &Path, source: &Path, precise: bool) {
    let frames = command_bytes(
        ffmpeg,
        &[
            "-v",
            "error",
            "-i",
            &output.to_string_lossy(),
            "-t",
            "0.5",
            "-map",
            "0:v:0",
            "-f",
            "framemd5",
            "-",
        ],
    );
    let frames = String::from_utf8(frames).unwrap();
    let hashes: HashSet<_> = frames
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.rsplit(',').next())
        .map(str::trim)
        .collect();
    assert!(
        hashes.len() > 10,
        "The clip does not move during its first half second: {}\n{frames}",
        output.display()
    );
    let info = probe(ffprobe, output);
    let video = info["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stream| stream["codec_type"] == "video")
        .unwrap();
    let start: f64 = video["start_time"].as_str().unwrap_or("0").parse().unwrap();
    assert!(
        (0.0..0.15).contains(&start),
        "Video has hidden preroll or a delayed start: {video}"
    );
    assert!(
        info["streams"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stream| stream["codec_type"] == "audio"),
        "Audio was lost: {info}"
    );
    let decoded = first_frame(ffmpeg, output, "0");
    assert_eq!(decoded.len(), 32 * 18);
    assert!(
        decoded.iter().map(|&pixel| u64::from(pixel)).sum::<u64>() / decoded.len() as u64 > 10,
        "The clip starts with a black frame."
    );
    if precise {
        assert!(
            (media_duration(&info) - 4.0).abs() < 0.15,
            "Wrong precise duration: {info}"
        );
        let expected = first_frame(ffmpeg, source, "3");
        let difference: f64 = decoded
            .iter()
            .zip(&expected)
            .map(|(&actual, &expected)| (f64::from(actual) - f64::from(expected)).abs())
            .sum::<f64>()
            / decoded.len() as f64;
        assert!(difference < 12.0, "The precise cut starts at the wrong source time (mean grayscale difference {difference:.2}).");
    } else {
        let packets: Value = serde_json::from_slice(&command_bytes(
            ffprobe,
            &[
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_packets",
                "-show_entries",
                "packet=pts_time,flags",
                "-of",
                "json",
                &output.to_string_lossy(),
            ],
        ))
        .unwrap();
        let first = &packets["packets"][0];
        assert!(
            first["flags"].as_str().unwrap().contains('K'),
            "The copy cut does not begin with a keyframe: {first}"
        );
        assert!(
            first["pts_time"].as_str().unwrap().parse::<f64>().unwrap() >= 0.0,
            "The copy cut has hidden negative timestamps: {first}"
        );
    }
}

fn execute(
    engine: &Arc<Downloads>,
    request: Value,
    on_event: impl Fn(&Value) + Send + Sync + 'static,
) -> Vec<Value> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let job = engine.begin(request).unwrap();
    let finished = Arc::new(AtomicBool::new(false));
    let done = finished.clone();
    let cancelling = engine.clone();
    let watchdog = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(45);
        while !done.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        if !done.load(Ordering::Acquire) {
            cancelling.cancel();
            return true;
        }
        false
    });
    engine.run(
        job,
        Arc::new(move |event| {
            on_event(&event);
            captured.lock().unwrap().push(event);
        }),
    );
    finished.store(true, Ordering::Release);
    assert!(
        !watchdog.join().unwrap(),
        "A local download did not finish within 45 seconds."
    );
    assert!(!engine.busy(), "The engine retained a finished job.");
    Arc::try_unwrap(events).unwrap().into_inner().unwrap()
}

fn assert_status(events: &[Value], expected: &str, name: &str) {
    let tail = &events[events.len().saturating_sub(30)..];
    assert_eq!(
        events.last().and_then(|event| event["status"].as_str()),
        Some(expected),
        "{name}: {}",
        serde_json::to_string_pretty(tail).unwrap()
    );
}

fn assert_published_file(events: &[Value], output: &Path) {
    let published: Vec<_> = events
        .iter()
        .filter_map(|event| event["file"].as_str())
        .collect();
    assert_eq!(
        published.len(),
        1,
        "Missing or duplicate published-file events: {events:?}"
    );
    let published = Path::new(published[0]);
    assert_eq!(
        published.canonicalize().unwrap(),
        output.canonicalize().unwrap()
    );
    assert!(
        !published.components().any(|part| part
            .as_os_str()
            .to_string_lossy()
            .starts_with(".finfetcher-")),
        "Final file event exposes a staging path: {}",
        published.display()
    );
}

fn output_file(destination: &Path, extension: &str) -> PathBuf {
    let outputs: Vec<_> = fs::read_dir(destination)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(
        outputs.len(),
        1,
        "Unexpected download leftovers: {outputs:?}"
    );
    assert_eq!(
        outputs[0].extension().and_then(|value| value.to_str()),
        Some(extension),
        "Wrong output type: {outputs:?}"
    );
    outputs[0].clone()
}

fn assert_no_staged_media(destination: &Path) {
    for entry in fs::read_dir(destination).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert!(
                fs::read_dir(&path).unwrap().next().is_none(),
                "Media was downloaded before destination confirmation: {}",
                path.display()
            );
        }
    }
}

fn runtime(root: &Path) -> (Arc<Settings>, Arc<Downloads>, PathBuf, PathBuf) {
    let settings = Arc::new(Settings::new(root.join("settings")).unwrap());
    settings.save(&json!({"auto_update_ytdlp":false,"embed_thumbnail":false,"embed_metadata":false,"embed_chapters":false,"use_download_archive":false})).unwrap();
    let tools = Arc::new(Tools::new(settings.root().to_owned(), settings.clone()));
    let ffmpeg = tools
        .ffmpeg()
        .expect("Real download tests require a working ffmpeg on PATH.");
    let ffprobe = tools
        .ffprobe()
        .expect("Real download tests require a working ffprobe on PATH.");
    tools
        .ytdlp()
        .expect("Real download tests require a working yt-dlp on PATH. They never install it.");
    let engine = Arc::new(Downloads::new(tools, settings.clone()).without_browser_cookies());
    (settings, engine, ffmpeg, ffprobe)
}

#[test]
#[ignore = "Runs real yt-dlp and FFmpeg against generated local HTTP fixtures. Requires all three tools on PATH."]
fn real_downloads_preserve_media_and_destination_workflows() {
    let root = tempfile::tempdir().unwrap();
    let (settings, engine, ffmpeg, ffprobe) = runtime(root.path());
    let mut sources = HashMap::new();
    for extension in ["mp4", "webm"] {
        let source = root.path().join(format!("source.{extension}"));
        generate_source(&ffmpeg, &source, extension);
        sources.insert(format!("/source.{extension}"), fs::read(source).unwrap());
    }
    sources.insert("/first.mp4".into(), sources["/source.mp4"].clone());
    sources.insert("/second.webm".into(), sources["/source.webm"].clone());
    sources.insert("/playlist.html".into(), b"<!doctype html><title>Fixture Playlist</title><video controls src=\"/first.mp4\"></video><video controls src=\"/second.webm\"></video>".to_vec());
    let server = FixtureServer::new(sources);

    let metadata = engine.inspect(&server.url("mp4"), false).unwrap();
    assert_eq!(metadata["title"], "source");
    assert_eq!(metadata["is_playlist"], false);
    assert_eq!(metadata["entries_count"], 1);
    assert!(
        !metadata["formats"].as_array().unwrap().is_empty(),
        "Metadata has no selectable formats: {metadata}"
    );
    let stream = engine.inspect(&server.url("mp4"), true).unwrap();
    assert_eq!(stream["stream_url"], server.url("mp4"));
    assert_eq!(stream["title"], "source");

    for extension in ["mp4", "webm"] {
        let destination = root.path().join(format!("ordinary-{extension}"));
        let events = execute(
            &engine,
            json!({"url":server.url(extension),"save_path":destination}),
            |_| {},
        );
        assert_status(&events, "completed", &format!("ordinary {extension}"));
        let output = output_file(&destination, extension);
        assert_published_file(&events, &output);
        assert_eq!(
            fs::read(&output).unwrap(),
            fs::read(root.path().join(format!("source.{extension}"))).unwrap(),
            "Ordinary download changed the source media."
        );
        assert!((media_duration(&probe(&ffprobe, &output)) - 10.0).abs() < 0.15);
        for fast in [true, false] {
            for precise in [true, false] {
                let name = format!("{extension}-fast-{fast}-precise-{precise}");
                let started = Instant::now();
                settings
                    .save(&json!({"fast_trim":fast,"precise_trim":precise}))
                    .unwrap();
                let destination = root.path().join(&name);
                let events = execute(
                    &engine,
                    json!({"url":server.url(extension),"save_path":destination,"trim_start":"3","trim_end":"7"}),
                    |_| {},
                );
                assert_status(&events, "completed", &name);
                if fast {
                    assert!(
                        !events.iter().any(|event| event["log"]
                            .as_str()
                            .is_some_and(|line| line.contains("Downloading in full"))),
                        "{name} silently used the full-download fallback: {events:?}"
                    );
                }
                let output = output_file(&destination, extension);
                assert_published_file(&events, &output);
                assert_moving_video(
                    &ffmpeg,
                    &ffprobe,
                    &output,
                    &root.path().join(format!("source.{extension}")),
                    precise,
                );
                eprintln!("{name}: {:.2}s", started.elapsed().as_secs_f64());
            }
        }
    }

    for format in ["mp3", "m4a", "opus", "flac", "wav"] {
        settings.save(&json!({"audio_format":format})).unwrap();
        let destination = root.path().join(format!("audio-{format}"));
        let events = execute(
            &engine,
            json!({"url":server.url("mp4"),"mode":"audio","save_path":destination}),
            |_| {},
        );
        assert_status(&events, "completed", &format!("audio {format}"));
        let output = output_file(&destination, format);
        assert_published_file(&events, &output);
        let info = probe(&ffprobe, &output);
        let streams = info["streams"].as_array().unwrap();
        assert_eq!(
            streams.len(),
            1,
            "Audio download retained a video stream: {info}"
        );
        assert_eq!(streams[0]["codec_type"], "audio");
        assert!(
            (media_duration(&info) - 10.0).abs() < 0.15,
            "Wrong audio duration: {info}"
        );
    }

    settings
        .save(&json!({"fast_trim":true,"precise_trim":true}))
        .unwrap();
    for action in ["overwrite", "folder", "cancel"] {
        let destination = root.path().join(format!("conflict-{action}"));
        fs::create_dir_all(&destination).unwrap();
        let original = destination.join("source.mp4");
        fs::write(&original, b"original clip").unwrap();
        let alternate = root.path().join(format!("alternate-{action}"));
        fs::create_dir_all(&alternate).unwrap();
        let prompted = Arc::new(AtomicUsize::new(0));
        let prompts = prompted.clone();
        let responding = engine.clone();
        let existing = original.clone();
        let chosen = alternate.clone();
        let before_download = destination.clone();
        let progress_seen = Arc::new(AtomicBool::new(false));
        let progress = progress_seen.clone();
        let events = execute(
            &engine,
            json!({"url":server.url("mp4"),"save_path":destination,"trim_start":"3","trim_end":"7"}),
            move |event| {
                if event.get("progress").is_some() {
                    progress.store(true, Ordering::Release);
                }
                if let Some(request) = event.get("destination_request") {
                    prompts.fetch_add(1, Ordering::Relaxed);
                    assert!(
                        !progress.load(Ordering::Acquire),
                        "The conflict was discovered after transfer started."
                    );
                    assert_eq!(fs::read(&existing).unwrap(), b"original clip");
                    assert_no_staged_media(&before_download);
                    assert!(request["paths"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(
                            |path| Path::new(path.as_str().unwrap()).canonicalize().unwrap()
                                == existing.canonicalize().unwrap()
                        ));
                    responding
                        .destination(json!({"id":request["id"],"action":action,"path":chosen}))
                        .unwrap();
                }
            },
        );
        assert_eq!(
            prompted.load(Ordering::Relaxed),
            1,
            "Unexpected destination prompts: {events:?}"
        );
        assert_status(
            &events,
            if action == "cancel" {
                "cancelled"
            } else {
                "completed"
            },
            action,
        );
        if action != "overwrite" {
            assert_eq!(fs::read(&original).unwrap(), b"original clip");
        }
        if action == "folder" {
            assert_published_file(&events, &alternate.join("source.mp4"));
            assert_moving_video(
                &ffmpeg,
                &ffprobe,
                &output_file(&alternate, "mp4"),
                &root.path().join("source.mp4"),
                true,
            );
        }
        if action == "overwrite" {
            assert_published_file(&events, &original);
            assert_moving_video(
                &ffmpeg,
                &ffprobe,
                &output_file(&destination, "mp4"),
                &root.path().join("source.mp4"),
                true,
            );
        }
        if action == "cancel" {
            assert!(!events.iter().any(|event| event.get("file").is_some()));
            assert!(!progress_seen.load(Ordering::Acquire));
            assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
            assert_eq!(fs::read_dir(&alternate).unwrap().count(), 0);
        }
    }

    settings.save(&json!({"rate_limit_kbps":16})).unwrap();
    let cancelling = engine.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let observed = cancelled.clone();
    let destination = root.path().join("cancel-active-transfer");
    let events = execute(
        &engine,
        json!({"url":server.url("mp4"),"save_path":destination}),
        move |event| {
            if event.get("progress").is_some() && !observed.swap(true, Ordering::AcqRel) {
                assert_eq!(cancelling.cancel()["success"], true);
            }
        },
    );
    assert!(
        cancelled.load(Ordering::Acquire),
        "No transfer progress arrived before cancellation: {events:?}"
    );
    assert_status(&events, "cancelled", "active transfer cancellation");
    assert!(!events.iter().any(|event| event.get("file").is_some()));
    assert_eq!(
        fs::read_dir(destination).unwrap().count(),
        0,
        "Cancelled transfer left media or temporary files behind."
    );

    settings
        .save(&json!({"rate_limit_kbps":0,"use_download_archive":true}))
        .unwrap();
    let playlist_url = format!("http://{}/playlist.html", server.address);
    let playlist = engine.inspect(&playlist_url, false).unwrap();
    assert_eq!(playlist["title"], "Fixture Playlist");
    assert_eq!(playlist["is_playlist"], true);
    assert_eq!(playlist["entries_count"], 2);
    assert_eq!(playlist["entries"][0]["title"], "Fixture Playlist (1)");
    assert_eq!(playlist["entries"][1]["title"], "Fixture Playlist (2)");
    let destination = root.path().join("playlist");
    let request = json!({"url":playlist_url,"type":"playlist","save_path":destination,"trim_start":"3","trim_end":"7"});
    let responding = engine.clone();
    let events = execute(&engine, request.clone(), move |event| {
        if let Some(conflict) = event.get("destination_request") {
            responding
                .destination(json!({"id":conflict["id"],"action":"cancel"}))
                .unwrap();
        }
    });
    assert_status(&events, "completed", "first playlist download");
    assert!(
        !events
            .iter()
            .any(|event| event.get("destination_request").is_some()),
        "Distinct playlist entries collided: {events:?}"
    );
    let files: Vec<_> = fs::read_dir(&destination)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| !path.file_name().unwrap().to_string_lossy().starts_with('.'))
        .collect();
    assert_eq!(
        files.len(),
        2,
        "Playlist did not produce two separate files: {files:?}"
    );
    let published: Vec<_> = events
        .iter()
        .filter_map(|event| event["file"].as_str())
        .map(PathBuf::from)
        .collect();
    assert_eq!(
        published.len(),
        2,
        "Playlist has missing published-file events: {events:?}"
    );
    for (index, extension) in ["mp4", "webm"].into_iter().enumerate() {
        let file = destination.join(format!("Fixture Playlist ({}).{extension}", index + 1));
        assert_eq!(
            fs::read(&file).unwrap(),
            fs::read(root.path().join(format!("source.{extension}"))).unwrap(),
            "Playlist lost an entry, changed its media or incorrectly applied a single-video trim."
        );
        assert!(
            published
                .iter()
                .any(|path| path.canonicalize().unwrap() == file.canonicalize().unwrap()),
            "Published-file event did not identify {}",
            file.display()
        );
    }
    let archive_path = destination.join(".finfetcher-archive.txt");
    let archive =
        fs::read_to_string(&archive_path).expect("Playlist download did not create an archive.");
    assert_eq!(
        archive.lines().count(),
        2,
        "Archive must contain one entry for each video: {archive}"
    );
    let before: Vec<_> = files
        .iter()
        .map(|path| {
            (
                path.clone(),
                fs::read(path).unwrap(),
                fs::metadata(path).unwrap().modified().unwrap(),
            )
        })
        .collect();
    let responding = engine.clone();
    let events = execute(&engine, request, move |event| {
        if let Some(conflict) = event.get("destination_request") {
            responding
                .destination(json!({"id":conflict["id"],"action":"cancel"}))
                .unwrap();
        }
    });
    assert_status(&events, "completed", "archived playlist rerun");
    assert!(
        !events
            .iter()
            .any(|event| event.get("destination_request").is_some()),
        "Already archived entries prompted for overwrite: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| event.get("progress").is_some() || event.get("file").is_some()),
        "Already archived entries were downloaded or republished: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["log"]
                .as_str()
                .is_some_and(|line| line.contains("already in the download archive")))
            .count(),
        2,
        "Not every archived entry was skipped: {events:?}"
    );
    assert_eq!(fs::read_to_string(&archive_path).unwrap(), archive);
    for (path, bytes, modified) in before {
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            modified,
            "Archived output was replaced: {}",
            path.display()
        );
    }
    assert_eq!(
        fs::read_dir(destination).unwrap().count(),
        3,
        "Playlist left staging files behind."
    );
    assert!(server.requests.load(Ordering::Relaxed) > 20);
    assert!(
        server.ranges.load(Ordering::Relaxed) > 0,
        "The fast-trim paths did not request HTTP byte ranges."
    );
}

#[test]
#[ignore = "Runs real subtitle, metadata and cover postprocessing with yt-dlp, FFmpeg and FFprobe on PATH."]
fn real_media_options_preserve_subtitles_titles_and_cover_art() {
    let root = tempfile::tempdir().unwrap();
    let (settings, engine, ffmpeg, ffprobe) = runtime(root.path());
    let source = root.path().join("source.mp4");
    generate_source(&ffmpeg, &source, "mp4");
    let cover = root.path().join("cover.jpg");
    command_bytes(
        &ffmpeg,
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=96x64",
            "-frames:v",
            "1",
            "-update",
            "1",
            &cover.to_string_lossy(),
        ],
    );
    let captions = b"WEBVTT\n\n00:00:00.000 --> 00:00:02.000\nFirst caption\n\n00:00:03.000 --> 00:00:05.000\nSecond caption\n";
    let page = b"<!doctype html><title>Fixture Media Options</title><video controls src=\"/source.mp4\" poster=\"/cover.jpg\"><track kind=\"subtitles\" srclang=\"en\" src=\"/captions.vtt\"></video>";
    let server = FixtureServer::new(HashMap::from([
        ("/source.mp4".into(), fs::read(&source).unwrap()),
        ("/cover.jpg".into(), fs::read(&cover).unwrap()),
        ("/captions.vtt".into(), captions.to_vec()),
        ("/options.html".into(), page.to_vec()),
    ]));
    let url = format!("http://{}/options.html", server.address);
    let title = "Fixture Media Options (1)";

    settings
        .save(&json!({"subtitles_enabled":true,"subtitle_langs":"en","embed_subtitles":false}))
        .unwrap();
    let destination = root.path().join("subtitle-sidecar");
    let events = execute(&engine, json!({"url":url,"save_path":destination}), |_| {});
    assert_status(&events, "completed", "subtitle sidecar");
    let output = destination.join(format!("{title}.mp4"));
    assert_published_file(&events, &output);
    assert_eq!(
        fs::read(&output).unwrap(),
        fs::read(&source).unwrap(),
        "Writing sidecar captions changed the video."
    );
    assert_eq!(
        fs::read(destination.join(format!("{title}.en.vtt"))).unwrap(),
        captions
    );
    assert_eq!(
        fs::read_dir(&destination).unwrap().count(),
        2,
        "Sidecar download left unexpected files."
    );

    settings
        .save(&json!({"embed_subtitles":true,"embed_metadata":true,"embed_thumbnail":true}))
        .unwrap();
    let destination = root.path().join("embedded-video");
    let events = execute(&engine, json!({"url":url,"save_path":destination}), |_| {});
    assert_status(&events, "completed", "embedded video subtitles/title/cover");
    let output = destination.join(format!("{title}.mp4"));
    assert_published_file(&events, &output);
    let info = probe(&ffprobe, &output);
    assert_eq!(
        info["format"]["tags"]["title"], title,
        "Video title metadata was not embedded: {info}"
    );
    let streams = info["streams"].as_array().unwrap();
    assert!(
        streams
            .iter()
            .any(|stream| stream["codec_type"] == "subtitle" && stream["codec_name"] == "mov_text"),
        "MP4 did not contain embedded subtitles: {info}"
    );
    assert!(
        streams
            .iter()
            .any(|stream| stream["disposition"]["attached_pic"] == 1),
        "MP4 did not contain attached cover art: {info}"
    );
    let subtitles = command_bytes(
        &ffmpeg,
        &[
            "-v",
            "error",
            "-i",
            &output.to_string_lossy(),
            "-map",
            "0:s:0",
            "-f",
            "srt",
            "-",
        ],
    );
    let subtitles = String::from_utf8(subtitles).unwrap();
    assert!(
        subtitles.contains("First caption") && subtitles.contains("Second caption"),
        "Embedded subtitle text was lost: {subtitles}"
    );
    assert!(
        (media_duration(&info) - 10.0).abs() < 0.15,
        "Postprocessing changed video duration: {info}"
    );

    settings
        .save(&json!({"subtitles_enabled":false,"audio_format":"mp3"}))
        .unwrap();
    let destination = root.path().join("embedded-audio");
    let events = execute(
        &engine,
        json!({"url":url,"mode":"audio","save_path":destination}),
        |_| {},
    );
    assert_status(&events, "completed", "audio title/cover");
    let output = output_file(&destination, "mp3");
    assert_published_file(&events, &output);
    let info = probe(&ffprobe, &output);
    assert_eq!(
        info["format"]["tags"]["title"], title,
        "Audio title metadata was not embedded: {info}"
    );
    let streams = info["streams"].as_array().unwrap();
    assert_eq!(
        streams
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .count(),
        1
    );
    assert!(
        streams
            .iter()
            .any(|stream| stream["disposition"]["attached_pic"] == 1),
        "MP3 did not contain attached cover art: {info}"
    );
    assert!(
        streams
            .iter()
            .filter(|stream| stream["codec_type"] == "video")
            .all(|stream| stream["disposition"]["attached_pic"] == 1),
        "Audio download retained a moving video stream: {info}"
    );
    assert!(
        (media_duration(&info) - 10.0).abs() < 0.15,
        "Postprocessing changed audio duration: {info}"
    );
}

#[test]
#[ignore = "Runs real chapter embedding/splitting and download logging with yt-dlp, FFmpeg and FFprobe on PATH."]
fn real_chapters_and_download_logs_are_published() {
    let root = tempfile::tempdir().unwrap();
    let (settings, engine, ffmpeg, ffprobe) = runtime(root.path());
    let source = root.path().join("source.mp4");
    generate_source(&ffmpeg, &source, "mp4");
    let metadata = json!({
        "@context":"https://schema.org", "@type":"VideoObject",
        "name":"Fixture Chapters", "contentUrl":"{{fixture_origin}}/source.mp4", "duration":"PT10S",
        "hasPart":[
            {"@type":"Clip","name":"Opening","startOffset":0,"endOffset":5},
            {"@type":"Clip","name":"Closing","startOffset":5,"endOffset":10}
        ]
    });
    let page = format!("<!doctype html><title>Fixture Chapters</title><script type=\"application/ld+json\">{metadata}</script>");
    let server = FixtureServer::new(HashMap::from([
        ("/source.mp4".into(), fs::read(&source).unwrap()),
        ("/chapters.html".into(), page.into_bytes()),
    ]));
    settings
        .save(&json!({"embed_chapters":true,"split_chapters":true}))
        .unwrap();
    let destination = root.path().join("chapter-output");
    let events = execute(
        &engine,
        json!({"url":format!("http://{}/chapters.html",server.address),"save_path":destination,"log_to_file":true}),
        |_| {},
    );
    assert_status(&events, "completed", "embedded and split chapters");
    let full = destination.join("Fixture Chapters.mp4");
    assert_published_file(&events, &full);
    let info: Value = serde_json::from_slice(&command_bytes(
        &ffprobe,
        &[
            "-v",
            "error",
            "-show_chapters",
            "-show_format",
            "-of",
            "json",
            &full.to_string_lossy(),
        ],
    ))
    .unwrap();
    let chapters = info["chapters"].as_array().unwrap();
    assert_eq!(
        chapters.len(),
        2,
        "Chapter embedding did not preserve both sections: {info}"
    );
    for (index, title) in ["Opening", "Closing"].into_iter().enumerate() {
        let expected_start = index as f64 * 5.0;
        let chapter = &chapters[index];
        assert_eq!(
            chapter["tags"]["title"], title,
            "Chapter title changed: {chapter}"
        );
        let start: f64 = chapter["start_time"].as_str().unwrap().parse().unwrap();
        let end: f64 = chapter["end_time"].as_str().unwrap().parse().unwrap();
        assert!(
            (start - expected_start).abs() < 0.05 && (end - expected_start - 5.0).abs() < 0.05,
            "Chapter boundaries changed: {chapter}"
        );
        let split = destination.join(format!("Fixture Chapters - {:03} {title}.mp4", index + 1));
        assert!(
            split.is_file(),
            "Split chapter was not published beside the full download: {}",
            split.display()
        );
        let split_info = probe(&ffprobe, &split);
        assert!(
            (media_duration(&split_info) - 5.0).abs() < 0.2,
            "Split chapter has the wrong duration: {split_info}"
        );
        let streams = split_info["streams"].as_array().unwrap();
        assert!(
            streams.iter().any(|stream| stream["codec_type"] == "video")
                && streams.iter().any(|stream| stream["codec_type"] == "audio"),
            "Chapter lost video or audio: {split_info}"
        );
        let expected = first_frame(&ffmpeg, &source, &expected_start.to_string());
        let actual = first_frame(&ffmpeg, &split, "0");
        assert_eq!(actual.len(), expected.len());
        let difference = actual
            .iter()
            .zip(&expected)
            .map(|(&actual, &expected)| (f64::from(actual) - f64::from(expected)).abs())
            .sum::<f64>()
            / actual.len() as f64;
        assert!(difference < 12.0, "Chapter {title} starts at the wrong source time (mean grayscale difference {difference:.2}).");
    }
    assert!(
        (media_duration(&info) - 10.0).abs() < 0.15,
        "Splitting chapters changed the full download duration."
    );
    let log_path = destination.join("download_log.txt");
    let log = fs::read_to_string(&log_path)
        .expect("Requested download logging did not create a file beside the media.");
    assert!(
        log.contains("Preparing download"),
        "Log is missing job startup: {log}"
    );
    assert!(
        log.contains("[download]") && log.contains("% of ") && log.contains("ETA"),
        "Log is missing useful transfer progress: {log}"
    );
    assert!(
        log.contains("Saved to:"),
        "Log is missing the saved destination: {log}"
    );
    assert_eq!(
        fs::read_dir(&destination).unwrap().count(),
        4,
        "Chapter processing left unpublished or temporary files."
    );

    let events = execute(
        &engine,
        json!({"url":format!("http://{}/missing.mp4",server.address),"save_path":destination,"log_to_file":true}),
        |_| {},
    );
    assert_status(&events, "error", "failed download with file logging");
    let error = events
        .iter()
        .find_map(|event| event["error"].as_str())
        .unwrap();
    assert!(
        error.contains("404"),
        "The failure fixture did not produce its expected HTTP error: {error}"
    );
    let appended = fs::read_to_string(&log_path).unwrap();
    assert!(
        appended.starts_with(&log),
        "A second download replaced the existing log."
    );
    assert!(
        appended[log.len()..].contains(error),
        "The failed download's actionable error was not appended to the log: {appended}"
    );
    assert_eq!(
        fs::read_dir(destination).unwrap().count(),
        4,
        "Failed download left staged files beside the earlier successful chapter outputs."
    );
}
