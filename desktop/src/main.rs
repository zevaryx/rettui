//! rettui's desktop app: the web UI in a window of its own, without a
//! browser, and a window of its own for what the web UI leaves out
//! (identity keys, backups with the identity, pipe interfaces).
//!
//! It works as the Android app does: rettui runs in this process (its web
//! UI on 127.0.0.1, on a port kept between runs so the page keeps what it
//! saved), and the window opens its login link. Links that aren't rettui's
//! open in the system's browser. Closing the window leaves rettui running
//! in the background, with an icon in the system tray to open it again or
//! quit (the Keep running when closed setting); with that off, or with no
//! tray to put the icon in, closing it stops rettui, which saves everything
//! first.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod manage;
mod notify;

use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::webview::DownloadEvent;
use tauri::{App, AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// The port tried first: beside `rettui --web`'s 8740, so both can run.
const PORT: u16 = 8741;
/// How long rettui may take to start listening.
const START: Duration = Duration::from_secs(30);

fn main() {
    // What the updater runs to check a new copy works here (see rettui's
    // src/update/install.rs).
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("rettui-desktop {}", rettui::VERSION);
        return;
    }
    // `--data-dir` as rettui takes it, for a second identity's folder; and
    // `--manage`, to open the management window (the launcher's action, for
    // where there's no tray icon to open it from).
    let mut args = std::env::args().skip(1);
    let (mut data_dir, mut manage) = (None, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => data_dir = args.next().map(PathBuf::from),
            "--manage" => manage = true,
            _ => {}
        }
    }
    let data_dir = data_dir.or_else(flatpak_data_dir);
    if let Err(e) = run(data_dir, manage) {
        eprintln!("rettui: {e}");
        std::process::exit(1);
    }
}

fn run(data_dir: Option<PathBuf>, manage: bool) -> Result<(), String> {
    let token_path = rettui::web_token_path(data_dir.clone()).map_err(|e| e.to_string())?;
    let folder = token_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let downloads = directories::UserDirs::new().and_then(|d| d.download_dir().map(Path::to_path_buf)).unwrap_or(folder.clone());
    // rettui, started once the single-instance check has passed: a second
    // copy only brings up the first one's window, and must not start a
    // second rettui on the same folder (or move the port the page's state
    // is kept under).
    let server: Arc<Mutex<Option<Server>>> = Arc::default();
    let started = server.clone();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, args, _cwd| {
                if args.iter().any(|a| a == "--manage") { manage::open(app) } else { show(app) }
            },
        ))
        .plugin(tauri_plugin_opener::init())
        // Notifications, which the webview doesn't show: the page sends
        // them here (capabilities/web-ui.json allows it that and nothing
        // else).
        .plugin(tauri_plugin_notification::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            let port = pick_port(&folder.join("desktop_port"));
            let home = start(&handle, data_dir.as_deref(), &token_path, port, &started)?;
            // A click on a notification brings the window forward; the page
            // opens what it's about.
            {
                use tauri_plugin_notification::NotificationExt as _;
                let handle = handle.clone();
                app.notification().on_action(move |_| show(&handle))?;
            }
            app.manage(notify::start(&handle));
            app.manage(manage::Downloads(downloads.clone()));
            app.manage(manage::Runner { data_dir, token_path, port, server: started });
            add_tray(app);
            add_menu(app)?;
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
            if manage {
                manage::open(app.handle());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            notify::notify_show,
            notify::notify_close,
            manage::manage_defaults,
            manage::manage_identities,
            manage::manage_keep_identity,
            manage::manage_export_identity,
            manage::manage_switch_identity,
            manage::manage_rename_identity,
            manage::manage_delete_identity,
            manage::manage_back_up,
            manage::manage_passphrase_weakness,
            manage::manage_restore_check,
            manage::manage_restore,
            manage::manage_pipes,
            manage::manage_add_pipe,
            manage::manage_set_pipe,
            manage::manage_remove_pipe,
            manage::manage_restart_reticulum,
        ])
        .on_window_event(|window, event| match event {
            // Closing leaves rettui running, the window hidden until the
            // tray icon brings it back.
            WindowEvent::CloseRequested { api, .. } if rettui::keep_running() && TRAY.load(Ordering::Relaxed) => {
                api.prevent_close();
                let _ = window.hide();
            }
            // Minimizing hides it the same way, if asked to.
            WindowEvent::Resized(_)
                if rettui::minimize_to_tray() && TRAY.load(Ordering::Relaxed) && window.is_minimized().unwrap_or(false) =>
            {
                let _ = window.hide();
            }
            // The webview's own idea of focus stays true on Linux while the
            // window is behind others, so the page is told (it won't
            // notify of what's on screen in a window with the focus).
            WindowEvent::Focused(focused) if window.label() == "main" => {
                if let Some(page) = window.get_webview_window("main") {
                    // Coming back catches the page up (and reads what's on screen).
                    let _ = page.eval(format!(
                        "window.rettuiWindowFocused={focused};if({focused})document.dispatchEvent(new Event('visibilitychange'))"
                    ));
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(e) => {
            stop(&server)?;
            return Err(match e {
                tauri::Error::Setup(e) => e.to_string(),
                e => e.to_string(),
            });
        }
    };

    app.run(|app, event| match event {
        RunEvent::Exit => rettui::request_stop(),
        // A click on the dock icon on a Mac, with the window hidden.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show(app),
        _ => {
            let _ = app;
        }
    });
    stop(&server)
}

type Server = thread::JoinHandle<Result<(), String>>;

/// Starts rettui's web UI on `port` and waits for it, giving the link the
/// window opens. Its stopping by itself (a signal, say) closes the app.
fn start(app: &AppHandle, data_dir: Option<&Path>, token_path: &Path, port: u16, server: &Mutex<Option<Server>>) -> Result<Url, String> {
    let mut command = vec!["rettui".to_string(), "--web".into(), format!("127.0.0.1:{port}")];
    if let Some(dir) = data_dir {
        command.extend(["--data-dir".into(), dir.display().to_string()]);
    }
    rettui::desktop_app();
    let handle = app.clone();
    let thread = thread::spawn(move || {
        let result = rettui::run_with(command).map_err(|e| format!("{e:#}"));
        // Stopped on purpose, to be started again (a restore): the app stays.
        if !RESTARTING.load(Ordering::SeqCst) {
            handle.exit(0);
        }
        result
    });
    // Wait for the web UI, or for rettui to stop without it (a broken
    // config, say).
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let until = Instant::now() + START;
    let mut failed = None;
    while TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err() {
        if thread.is_finished() {
            failed = Some("rettui stopped before its web UI started".into());
            break;
        }
        if Instant::now() > until {
            failed = Some("the web UI didn't start".into());
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    *server.lock().unwrap_or_else(|e| e.into_inner()) = Some(thread);
    if let Some(e) = failed {
        // rettui's own error, if it gave one.
        stop(server)?;
        return Err(e);
    }
    let token = std::fs::read_to_string(token_path).map_err(|e| format!("reading {}: {e}", token_path.display()))?;
    Url::parse(&format!("http://127.0.0.1:{port}/?token={}", token.trim())).map_err(|e| e.to_string())
}

/// Set while rettui is stopped to be started again, so its stopping doesn't
/// close the app.
static RESTARTING: AtomicBool = AtomicBool::new(false);

/// Restores the backup `file` (already looked into): stops rettui, which
/// saves everything first, restores it with what was there moved aside, and
/// starts rettui again, then has the main window load it. The files are
/// restored whether or not rettui stopped cleanly, and rettui is started
/// again whether or not the restore worked, so a failed restore leaves
/// the app running as it was.
fn restore_and_restart(app: &AppHandle, file: &str, passphrase: Option<String>) -> Result<String, String> {
    let runner = app.state::<manage::Runner>();
    RESTARTING.store(true, Ordering::SeqCst);
    let stopped = stop(&runner.server);
    let restored = match stopped {
        Ok(()) => tauri::async_runtime::block_on(rettui::manage::restore_files(runner.data_dir.clone(), file, passphrase)),
        Err(e) => Err(format!("rettui didn't stop cleanly, so nothing was restored: {e}")),
    };
    let started = start(app, runner.data_dir.as_deref(), &runner.token_path, runner.port, &runner.server);
    RESTARTING.store(false, Ordering::SeqCst);
    let home = match started {
        Ok(home) => home,
        Err(e) => {
            return Err(format!(
                "{}rettui couldn't start again: {e}. Quit the app and open it again.",
                restored.map(|d| format!("{d}. ")).unwrap_or_default()
            ));
        }
    };
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.navigate(home);
    }
    restored
}

/// Stops rettui, if it started, and waits: it saves everything first.
fn stop(server: &Mutex<Option<Server>>) -> Result<(), String> {
    let Some(thread) = server.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return Ok(());
    };
    rettui::request_stop();
    thread.join().map_err(|_| "rettui stopped unexpectedly".to_string())?
}

/// The menu entry that opens the management window.
const MANAGE: &str = "Identities, backups and pipes…";

/// On a Mac, the usual menu bar with the management window added to it (the
/// other systems have the tray icon and the launcher's action). Without the
/// usual menu a Mac's text fields lose copy and paste.
fn add_menu(app: &App) -> tauri::Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let menu = Menu::default(app.handle())?;
    let item = MenuItem::with_id(app, "manage", MANAGE, true, Some("CmdOrCtrl+Shift+M"))?;
    let submenu = tauri::menu::Submenu::with_items(app, "rettui", true, &[&item])?;
    menu.append(&submenu)?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id().as_ref() == "manage" {
            manage::open(app);
        }
    });
    Ok(())
}

/// Whether the tray icon is up. Without one a hidden window couldn't be
/// brought back, so closing and minimizing then work as usual.
static TRAY: AtomicBool = AtomicBool::new(false);

/// The tray icon: a click opens the window, its menu opens it or quits.
/// Linux needs the appindicator library for it, which panics when it's
/// missing; the app then runs without one.
fn add_tray(app: &App) {
    let build = || -> tauri::Result<()> {
        let open = MenuItem::with_id(app, "open", "Open rettui", true, None::<&str>)?;
        let manage = MenuItem::with_id(app, "manage", MANAGE, true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&open, &manage, &quit])?;
        let mut tray = TrayIconBuilder::with_id("main")
            .tooltip("rettui")
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, event| match event.id.as_ref() {
                "open" => show(app),
                "manage" => manage::open(app),
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                    show(tray.app_handle());
                }
            });
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone());
        }
        // The icon goes to the tray as a file. A Flatpak's own temporary
        // folders are hidden from the desktop, so there it goes in the app's
        // cache folder, which has the same path outside the sandbox.
        if std::env::var_os("FLATPAK_ID").is_some()
            && let Some(cache) = std::env::var_os("XDG_CACHE_HOME")
        {
            tray = tray.temp_dir_path(PathBuf::from(cache).join("tray-icon"));
        }
        tray.build(app)?;
        Ok(())
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)) {
        Ok(Ok(())) => TRAY.store(true, Ordering::Relaxed),
        Ok(Err(e)) => eprintln!("rettui: no tray icon: {e}"),
        Err(_) => eprintln!("rettui: no tray icon (is libayatana-appindicator installed?)"),
    }
}

/// Brings the window back: from the tray, the taskbar or behind others.
pub(crate) fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// In a Flatpak, the folder the terminal rettui uses
/// (`~/.local/share/rettui`, which the Flatpak may write to), rather than
/// one inside the sandbox: both then have the same identity and messages.
fn flatpak_data_dir() -> Option<PathBuf> {
    std::env::var_os("FLATPAK_ID")?;
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share/rettui"))
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
