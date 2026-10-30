use finfetcher::{downloads::Downloads, settings::Settings, tools::Tools};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[test]
#[ignore = "Requires YouTube access and downloads a three-second public test clip"]
fn youtube_metadata_and_precise_clip() {
    let directory = tempfile::tempdir().unwrap();
    let settings = Arc::new(Settings::new(directory.path().join("state")).unwrap());
    settings.save(&json!({"auto_update_ytdlp":true,"embed_thumbnail":false,"embed_metadata":false,"embed_chapters":false,"fast_trim":true,"precise_trim":true})).unwrap();
    let tools = Arc::new(Tools::new(settings.root().to_owned(), settings.clone()));
    let engine = Downloads::new(tools, settings).without_browser_cookies();
    let url = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
    let info = engine
        .inspect(url, false)
        .expect("Public YouTube metadata extraction failed");
    assert!(info["title"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(info["duration"].as_f64().is_some_and(|n| n > 3.0));
    assert!(!info["is_playlist"].as_bool().unwrap());
    let destination = directory.path().join("media");
    let job=engine.begin(json!({"url":url,"save_path":destination,"quality":"360p","trim_start":"0","trim_end":"3"})).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    engine.run(
        job,
        Arc::new(move |event| {
            if let Some(line) = event["log"].as_str() {
                eprintln!("{line}");
            }
            capture.lock().unwrap().push(event);
        }),
    );
    let events = events.lock().unwrap();
    assert_eq!(events.last().unwrap()["status"], "completed", "{events:#?}");
    assert!(events.iter().any(|event| event["file"]
        .as_str()
        .is_some_and(|path| std::path::Path::new(path).is_file())));
}
