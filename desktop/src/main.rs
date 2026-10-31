//! rettui's desktop app: the web UI in a window of its own, without a
//! browser. A prototype.
//!
//! It works as the Android app does: rettui runs in this process (its web
//! UI on 127.0.0.1, on a port kept between runs so the page keeps what it
//! saved), and the window opens its login link. Links that aren't rettui's
//! open in the system's browser. Closing the window stops rettui, which
//! saves everything first.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use tauri::webview::DownloadEvent;
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder};

/// The port tried first: beside `rettui --web`'s 8740, so both can run.
const PORT: u16 = 8741;
/// How long rettui may take to start listening.
const START: Duration = Duration::from_secs(30);

fn main() {
    // `--data-dir` as rettui takes it, for a second identity's folder.
    let mut args = std::env::args().skip(1);
    let data_dir = match (args.next().as_deref(), args.next()) {
        (Some("--data-dir"), Some(dir)) => Some(PathBuf::from(dir)),
        _ => None,
    };
    if let Err(e) = run(data_dir) {
        eprintln!("rettui: {e}");
        std::process::exit(1);
    }
}

fn run(data_dir: Option<PathBuf>) -> Result<(), String> {
    let token_path = rettui::web_token_path(data_dir.clone()).map_err(|e| e.to_string())?;
    let folder = token_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let port = pick_port(&folder.join("desktop_port"));

    let mut command = vec!["rettui".to_string(), "--web".into(), format!("127.0.0.1:{port}")];
    if let Some(dir) = &data_dir {
        command.extend(["--data-dir".into(), dir.display().to_string()]);
    }
    // Set once the window is up: rettui stopping by itself (a signal, say)
    // closes it.
    let app_handle: Arc<OnceLock<AppHandle>> = Arc::default();
    let server = {
        let app_handle = app_handle.clone();
        thread::spawn(move || {
            let result = rettui::run_with(command).map_err(|e| format!("{e:#}"));
            if let Some(handle) = app_handle.get() {
                handle.exit(0);
            }
            result
        })
    };

    // Wait for the web UI, or for rettui to stop without it (a broken
    // config, say).
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let until = Instant::now() + START;
    while TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err() {
        if server.is_finished() {
            return Err(server.join().map_err(|_| "rettui stopped unexpectedly".to_string())?.err().unwrap_or_default());
        }
        if Instant::now() > until {
            return Err("the web UI didn't start".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
    let token = std::fs::read_to_string(&token_path).map_err(|e| format!("reading {}: {e}", token_path.display()))?;
    let home = Url::parse(&format!("http://127.0.0.1:{port}/?token={}", token.trim())).map_err(|e| e.to_string())?;
    let downloads = directories::UserDirs::new().and_then(|d| d.download_dir().map(Path::to_path_buf)).unwrap_or(folder);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            let _ = app_handle.set(handle.clone());
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(home))
                .title("rettui")
                .inner_size(1200.0, 800.0)
                .min_inner_size(360.0, 480.0)
                // Tells the page it's in the desktop app, as the Android
                // app's user agent does.
                .initialization_script("window.RETTUI_DESKTOP = 1;")
                .on_navigation(move |url| {
                    if is_local(url, port) {
                        return true;
                    }
                    use tauri_plugin_opener::OpenerExt as _;
                    let _ = handle.opener().open_url(url.as_str(), None::<&str>);
                    false
                })
                // Exports, backups and files from messages go to Downloads.
                .on_download(move |_, event| match event {
                    DownloadEvent::Requested { url, destination } => {
                        let name = destination.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "download".into());
                        *destination = downloads.join(name);
                        is_local(&url, port)
                    }
                    _ => true,
                })
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .map_err(|e| e.to_string())?;

    app.run(|_, event| {
        if let RunEvent::Exit = event {
            rettui::request_stop();
        }
    });
    // rettui saves everything before it returns.
    server.join().map_err(|_| "rettui stopped unexpectedly".to_string())?
}

/// The port last used (so the page keeps its saved state, which is kept
/// per address), or [`PORT`], or any free one if another program has it.
fn pick_port(saved: &Path) -> u16 {
    let last = std::fs::read_to_string(saved).ok().and_then(|p| p.trim().parse().ok()).unwrap_or(PORT);
    let free = |port: u16| TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok().and_then(|l| l.local_addr().ok()).map(|a| a.port());
    let port = free(last).or_else(|| free(0)).unwrap_or(last);
    if port != last || !saved.exists() {
        let _ = std::fs::write(saved, port.to_string());
    }
    port
}

/// Whether `url` is this rettui's web UI.
fn is_local(url: &Url, port: u16) -> bool {
    url.scheme() == "http" && url.host_str() == Some("127.0.0.1") && url.port() == Some(port)
}
