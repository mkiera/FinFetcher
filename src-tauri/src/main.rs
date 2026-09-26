#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use finfetcher::{
    downloads::Downloads, events::EventSink, integrations, media, settings::Settings, tools::Tools,
    updates::Updater,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

struct Desktop {
    settings: Arc<Settings>,
    tools: Arc<Tools>,
    downloads: Arc<Downloads>,
    updater: Arc<Updater>,
    version: String,
}

fn request(
    app: &tauri::AppHandle,
    route: &str,
    method: &str,
    payload: Value,
) -> Result<Value, String> {
    let state = app.state::<Desktop>();
    match (method, route) {
        ("GET", "/version.txt") => Ok(json!({"version":state.version})),
        ("GET", "/api/setup/check") => Ok(state.tools.status()),
        ("POST", "/api/setup/browse") => {
            let path = payload["path"]
                .as_str()
                .filter(|p| !p.is_empty())
                .ok_or("Choose an FFmpeg folder.")?;
            state.tools.set_custom_path(Path::new(path))?;
            Ok(json!({"success":true,"path":path}))
        }
        ("POST", "/api/setup/exit") => {
            exit_later(app);
            Ok(json!({"success":true}))
        }
        ("GET", "/api/integrations/flipperclipper") => {
            Ok(json!({"installed":integrations::flipperclipper().is_some()}))
        }
        ("GET", "/api/settings") => Ok(state.settings.get()),
        ("POST", "/api/settings") => {
            Ok(json!({"success":true,"settings":state.settings.save(&payload)?}))
        }
        ("POST", "/api/info") => state
            .downloads
            .inspect(media::string(&payload, "url"), false),
        ("POST", "/api/stream") => state
            .downloads
            .inspect(media::string(&payload, "url"), true),
        ("POST", "/api/download/cancel") => Ok(state.downloads.cancel()),
        ("POST", "/api/download/destination") => state.downloads.destination(payload),
        ("GET", "/api/update/settings") => Ok(state.updater.settings()),
        ("POST", "/api/update/settings") => state.updater.save_settings(&payload),
        ("GET", "/api/update/check") => {
            let downloads = state.downloads.clone();
            std::thread::spawn(move || downloads.refresh_tools());
            state
                .updater
                .check(payload["force"] == true || payload["force"] == "true")
        }
        ("GET", "/api/update/releases") => state
            .updater
            .releases(payload["channel"].as_str().unwrap_or("stable")),
        ("GET", "/api/update/artifacts") => state.updater.artifacts(),
        ("POST", "/api/update/apply") => {
            let path = payload["path"]
                .as_str()
                .ok_or("Downloaded installer not found.")?;
            state.downloads.reserve_install()?;
            match state.updater.apply(Path::new(path)) {
                Ok(result) => {
                    exit_later(app);
                    Ok(result)
                }
                Err(error) => {
                    state.downloads.release_install();
                    Err(error)
                }
            }
        }
        ("GET", "/api/debug") => Ok(diagnostics(&state)),
        ("POST", "/api/debug/test") => {
            let url = payload["url"]
                .as_str()
                .unwrap_or("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
            Ok(match state.downloads.inspect(url, false) {
                Ok(info) => {
                    json!({"success":true,"message":"yt-dlp can fetch video information.","title":info["title"]})
                }
                Err(error) => {
                    json!({"success":false,"message":"The diagnostic failed.","error":error})
                }
            })
        }
        _ => Err("Unknown desktop request.".into()),
    }
}

fn diagnostics(state: &Desktop) -> Value {
    let tools = state.tools.diagnostics();
    let describe = |key: &str| {
        if let Some(path) = tools[key]["path"].as_str() {
            format!(
                "{} ({path})",
                tools[key]["version"].as_str().unwrap_or("Unknown version")
            )
        } else {
            "Not installed".into()
        }
    };
    let runtimes = tools["runtimes"]
        .as_array()
        .map(|v| {
            v.iter()
                .map(|r| {
                    format!(
                        "{} {}",
                        r["name"].as_str().unwrap_or(""),
                        r["version"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    json!({"system":{"os":std::env::consts::OS,"os_version":"","platform":format!("{}-{}",std::env::consts::ARCH,std::env::consts::OS),"runtime":"Rust / Tauri","runtime_version":"Tauri 2","executable":std::env::current_exe().ok()},
        "dependencies":{"yt-dlp":describe("yt-dlp"),"ffmpeg":describe("ffmpeg"),"ffprobe":describe("ffprobe"),"CA store":tools["certificate_store"],"JS runtime":if runtimes.is_empty(){"Not installed. YouTube downloads will install Deno automatically.".into()}else{runtimes},"yt-dlp update":tools["yt-dlp update"]["error"]},
        "paths":{"data":state.settings.root(),"downloads":finfetcher::downloads::default_downloads()},"version":state.version,"tools":tools})
}

fn exit_later(app: &tauri::AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        handle.state::<Desktop>().downloads.close();
        handle.exit(0);
    });
}

#[tauri::command]
async fn desktop_request(
    app: tauri::AppHandle,
    route: String,
    method: String,
    payload: Value,
) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || request(&app, &route, &method, payload))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn desktop_stream(
    app: tauri::AppHandle,
    route: String,
    payload: Value,
    stream_id: String,
) -> Result<(), String> {
    if stream_id.is_empty() || stream_id.len() > 128 {
        return Err("Invalid progress stream.".into());
    }
    if !matches!(
        route.as_str(),
        "/api/download" | "/api/setup/install-sync" | "/api/update/download"
    ) {
        return Err("Unknown progress stream.".into());
    }
    let state = app.state::<Desktop>();
    let job = if route == "/api/download" {
        Some(state.downloads.begin(payload.clone())?)
    } else {
        None
    };
    let downloads = state.downloads.clone();
    let updater = state.updater.clone();
    let handle = app.clone();
    let id = stream_id.clone();
    let sink: EventSink = Arc::new(move |event| {
        let _ = handle.emit("finfetcher-stream", json!({"id":id,"data":event}));
    });
    tauri::async_runtime::spawn_blocking(move || {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                match route.as_str() {
                    "/api/download" => {
                        downloads.run(job.expect("Reserved download"), sink.clone());
                        Ok(())
                    }
                    "/api/setup/install-sync" => {
                        downloads.install_tools(&sink)?;
                        sink(
                            json!({"percent":100,"status":"Installation complete!","success":true}),
                        );
                        Ok(())
                    }
                    "/api/update/download" => {
                        updater.download(
                            media::string(&payload, "url"),
                            media::string(&payload, "name"),
                            &sink,
                        )?;
                        Ok(())
                    }
                    _ => unreachable!(),
                }
            }))
            .unwrap_or_else(|_| Err("The operation stopped unexpectedly.".into()));
        if let Err(error) = result {
            sink(json!({"success":false,"error":error,"status":format!("Error: {error}")}));
        }
        let _ = app.emit("finfetcher-stream", json!({"id":stream_id,"done":true}));
    });
    Ok(())
}

#[tauri::command]
async fn desktop_select_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .blocking_pick_folder()
            .map(|path| {
                path.into_path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .map_err(|e| e.to_string())
            })
            .transpose()
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn desktop_open_external(app: tauri::AppHandle, url: String) -> Result<(), String> {
    media::validate_url(&url)?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

fn data_directory() -> Result<PathBuf, String> {
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--state-dir" {
            return arguments
                .next()
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .ok_or("--state-dir requires an absolute folder path.".into());
        }
    }
    if let Some(path) = std::env::var_os("FINFETCHER_DATA_DIR") {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return Ok(path);
        }
        return Err("FINFETCHER_DATA_DIR must be an absolute path.".into());
    }
    std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .map(|root| root.join("FinFetcher"))
        .ok_or("Could not find the application data folder.".into())
}

fn startup_error(error: impl std::fmt::Display) -> ! {
    eprintln!("FinFetcher could not start: {error}");
    #[cfg(windows)]
    if !std::env::args().any(|argument| argument == "--hidden") {
        let message: Vec<u16> = format!("FinFetcher could not start.\n\n{error}\0")
            .encode_utf16()
            .collect();
        let title: Vec<u16> = "FinFetcher\0".encode_utf16().collect();
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                0x10,
            );
        }
    }
    std::process::exit(1)
}

fn main() {
    #[cfg(windows)]
    unsafe {
        let app_id: Vec<u16> = "FinFetcher.App.1\0".encode_utf16().collect();
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr());
    }
    let directory = match data_directory() {
        Ok(path) => path,
        Err(error) => startup_error(error),
    };
    let settings = match Settings::new(directory.clone()) {
        Ok(value) => Arc::new(value),
        Err(error) => startup_error(error),
    };
    let tools = Arc::new(Tools::new(directory.clone(), settings.clone()));
    let downloads = Arc::new(Downloads::new(tools.clone(), settings.clone()));
    let version = env!("FINFETCHER_BUILD_VERSION").to_owned();
    let updater = Arc::new(Updater::new(
        directory.clone(),
        settings.clone(),
        version.clone(),
    ));
    if let Ok(identity) = serde_json::from_str(env!("FINFETCHER_BUILD_IDENTITY_JSON")) {
        updater.set_identity(identity);
    }
    let state = Desktop {
        settings,
        tools,
        downloads,
        updater,
        version,
    };
    let mut context = tauri::generate_context!();
    let hidden = std::env::args().any(|argument| argument == "--hidden");
    for window in &mut context.config_mut().app.windows {
        window.create = false;
        if hidden {
            window.visible = false;
        }
    }
    let navigation = tauri::plugin::Builder::<tauri::Wry>::new("navigation")
        .on_navigation(|_, url| {
            (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"))
        })
        .build();
    let result=tauri::Builder::default()
        .plugin(navigation)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .setup(move|app| {
            for config in app.config().app.windows.clone() {
                tauri::WebviewWindowBuilder::from_config(app,&config)?
                    .data_directory(directory.join("webview2"))
                    .build()?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![desktop_request,desktop_stream,desktop_select_folder,desktop_open_external])
        .on_window_event(|window,event|{
            if let tauri::WindowEvent::CloseRequested{api,..}=event {
                api.prevent_close();
                let handle=window.app_handle().clone();
                tauri::async_runtime::spawn_blocking(move||{
                    let state=handle.state::<Desktop>();
                    if state.downloads.busy() {
                        let confirmed=handle.dialog()
                            .message("Closing FinFetcher stops the current download. Existing files will be kept.")
                            .title("Stop download?")
                            .kind(MessageDialogKind::Warning)
                            .buttons(MessageDialogButtons::OkCancel)
                            .blocking_show();
                        if !confirmed {return;}
                    }
                    state.downloads.close();
                    handle.exit(0);
                });
            }
        })
        .run(context);
    if let Err(error) = result {
        startup_error(error);
    }
}
