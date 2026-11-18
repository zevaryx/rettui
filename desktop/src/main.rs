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

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::webview::DownloadEvent;
use tauri::{App, AppHandle, Manager, RunEvent, Runtime, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// The port tried first: beside `rettui --web`'s 8740, so both can run.
const PORT: u16 = 8741;
/// How long rettui may take to start listening.
const START: Duration = Duration::from_secs(30);
/// How long one question to what holds the port may take: a program that
/// never answers must not hold the window up for long.
const ASK: Duration = Duration::from_secs(2);
/// The most read of an answer: rettui's is a short header and 64 hex digits.
const ANSWER_MAX: usize = 2048;
/// The port last used, kept between runs (see [`pick_port`]).
const SAVED_PORT_FILE: &str = "desktop_port";
/// The length under which rettui does not take the login secret in the data folder
/// as one, and makes a new one (see `load_token` in src/web/mod.rs).
const MIN_TOKEN: usize = 32;
/// The file rettui writes the port it listens on to, once it is listening.
const BOUND_PORT_FILE: &str = "desktop_bound_port";

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
            let asked = pick_port(&folder.join(SAVED_PORT_FILE));
            let home = start(&handle, data_dir.as_deref(), &token_path, asked, &started)?;
            // A click on a notification brings the window forward; the page
            // opens what it's about.
            {
                use tauri_plugin_notification::NotificationExt as _;
                let handle = handle.clone();
                app.notification().on_action(move |_| show(&handle))?;
            }
            app.manage(notify::start(&handle));
            app.manage(manage::Downloads(downloads.clone()));
            app.manage(manage::Runner { data_dir, token_path, port: asked, server: started });
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
                    if is_local(url, web_port()) {
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
                        is_local(&url, web_port())
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

/// Starts rettui's web UI, asking for the port `asked`, and waits for it, giving the
/// link the window opens. rettui binds the port itself and says which it got
/// (another one, if `asked` is held), so the port is never free between
/// choosing it and using it; and the login secret goes into a link only once
/// what answers there has proved it is rettui (see [`verify`]). Its stopping
/// by itself (a signal, say) closes the app.
fn start<R: Runtime>(
    app: &AppHandle<R>,
    data_dir: Option<&Path>,
    token_path: &Path,
    asked: u16,
    server: &Mutex<Option<Server>>,
) -> Result<Url, String> {
    let mut command = vec!["rettui".to_string(), "--web".into(), format!("127.0.0.1:{asked}")];
    if let Some(dir) = data_dir {
        command.extend(["--data-dir".into(), dir.display().to_string()]);
    }
    // What an earlier run said is not this run's port.
    let port_file = token_path.with_file_name(BOUND_PORT_FILE);
    let _ = std::fs::remove_file(&port_file);
    rettui::set_port_file(port_file.clone());
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
    let waited = wait_for_web_ui(&thread, &port_file, token_path);
    *server.lock().unwrap_or_else(|e| e.into_inner()) = Some(thread);
    let (port, token) = match waited {
        Ok(up) => up,
        Err(e) => {
            // rettui's own error, if it gave one.
            stop(server)?;
            return Err(e);
        }
    };
    WEB_PORT.store(port, Ordering::SeqCst);
    // rettui takes another port when the one asked for is held; the page's
    // saved state goes with the port, so the next run asks for that one.
    if port != asked {
        let _ = std::fs::write(token_path.with_file_name(SAVED_PORT_FILE), port.to_string());
    }
    Url::parse(&format!("http://127.0.0.1:{port}/?token={token}")).map_err(|e| e.to_string())
}

/// The port the web UI is on, as the last [`start`] found it. One app runs
/// one rettui per process (the single-instance plugin sees to that), so there
/// is one.
static WEB_PORT: AtomicU16 = AtomicU16::new(0);

fn web_port() -> u16 {
    WEB_PORT.load(Ordering::SeqCst)
}

/// Waits until rettui says which port it listens on and that answers as
/// rettui. The port and the login secret.
fn wait_for_web_ui(thread: &Server, port_file: &Path, token_path: &Path) -> Result<(u16, String), String> {
    let until = Instant::now() + START;
    let port = loop {
        if let Some(port) = std::fs::read_to_string(port_file).ok().and_then(|p| p.trim().parse::<u16>().ok()).filter(|p| *p != 0) {
            break port;
        }
        if thread.is_finished() {
            return Err("rettui stopped before its web UI started".into());
        }
        if Instant::now() > until {
            return Err("the web UI didn't start".into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    // The port is bound before rettui makes the secret on a first start.
    let token = loop {
        match std::fs::read_to_string(token_path) {
            // At least what rettui accepts (it makes a new one of a shorter one).
            Ok(token) if token.trim().len() >= MIN_TOKEN => break token.trim().to_string(),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("reading {}: {e}", token_path.display())),
            _ => {}
        }
        if thread.is_finished() {
            return Err("rettui stopped before its web UI started".into());
        }
        if Instant::now() > until {
            return Err("the web UI didn't start".into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    verify(port, &token, until, || thread.is_finished())?;
    Ok((port, token))
}

/// Checks that what answers on `port` is rettui, before the secret goes
/// anywhere near it: it is asked the HMAC, keyed by `token`, of a fresh
/// nonce and the port (`GET /api/hello`), which only a program that knows
/// the token can give, and the answer is compared in constant time. Only the
/// nonce is sent. rettui may take a while to start answering (or answer that
/// it is busy), so it is asked again until `until`; a 200 with the wrong
/// proof, or an answer that is not HTTP, ends it at once.
fn verify(port: u16, token: &str, until: Instant, stopped: impl Fn() -> bool) -> Result<(), String> {
    loop {
        let nonce = rettui::hello_nonce();
        match ask_hello(port, &nonce, until) {
            Ok(said) if rettui::hello_matches(token, port, &nonce, &said) => return Ok(()),
            Ok(_) => return Err(format!("what answers on port {port} is not rettui, so the web UI was not opened")),
            Err(_) => {}
        }
        if stopped() {
            return Err("rettui stopped before its web UI started".into());
        }
        if Instant::now() > until {
            return Err(format!("nothing on port {port} answered as rettui does, so the web UI was not opened"));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// What `GET /api/hello?nonce=…` on 127.0.0.1:`port` says: the body of a 200
/// answer; nothing for another answer that is final (not HTTP, or a status
/// rettui does not give). An error for what may pass: no answer yet (within
/// [`ASK`] and before `until`), or a 429 or 5xx, which a busy or starting
/// rettui may give.
fn ask_hello(port: u16, nonce: &str, until: Instant) -> std::io::Result<String> {
    let give_up = (Instant::now() + ASK).min(until);
    let left = || give_up.saturating_duration_since(Instant::now());
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, left().max(Duration::from_millis(1)))?;
    stream.set_write_timeout(Some(left().max(Duration::from_millis(1))))?;
    write!(stream, "GET /api/hello?nonce={nonce} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")?;
    // A listener that sends a little at a time can't stretch this past `give_up`.
    let mut reply = Vec::new();
    let mut chunk = [0u8; 512];
    while reply.len() < ANSWER_MAX {
        if left().is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(left()))?;
        match stream.read(&mut chunk)? {
            0 => break,
            n => reply.extend_from_slice(&chunk[..n]),
        }
    }
    if reply.is_empty() {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    let reply = String::from_utf8_lossy(&reply);
    let (head, body) = reply.split_once("\r\n\r\n").unwrap_or((&reply, ""));
    let status =
        head.lines().next().and_then(|line| line.strip_prefix("HTTP/1.")).and_then(|rest| rest.split(' ').nth(1)?.parse::<u16>().ok());
    match status {
        Some(200) => Ok(body.trim().to_string()),
        Some(429 | 500..=599) => Err(std::io::ErrorKind::WouldBlock.into()),
        _ => Ok(String::new()),
    }
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
fn restore_and_restart<R: Runtime>(app: &AppHandle<R>, file: &str, passphrase: Option<String>) -> Result<String, String> {
    let runner = app.state::<manage::Runner>();
    RESTARTING.store(true, Ordering::SeqCst);
    let stopped = stop(&runner.server);
    let restored = match stopped {
        Ok(()) => tauri::async_runtime::block_on(rettui::manage::restore_files(runner.data_dir.clone(), file, passphrase)),
        Err(e) => Err(format!("rettui didn't stop cleanly, so nothing was restored: {e}")),
    };
    // The port rettui holds (which may not be the first one asked for): it
    // is kept bound while rettui is stopped, and asked for again.
    let port = if web_port() != 0 { web_port() } else { runner.port };
    let started = start(app, runner.data_dir.as_deref(), &runner.token_path, port, &runner.server);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use tauri::test::mock_app;

    /// Held by a test that runs rettui for real.
    static LIVE: Mutex<()> = Mutex::new(());

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

    /// A plain listener that answers every connection with `reply` (nothing,
    /// for `None`, keeping the connection open) and gives back what it was sent.
    fn listener(reply: Option<&'static str>) -> (TcpListener, mpsc::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let (tx, rx) = mpsc::channel();
        let own = listener.try_clone().unwrap();
        thread::spawn(move || {
            for conn in own.incoming() {
                let Ok(mut conn) = conn else { break };
                let _ = conn.set_read_timeout(Some(Duration::from_millis(500)));
                let mut seen = Vec::new();
                let mut chunk = [0u8; 512];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match conn.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => seen.extend_from_slice(&chunk[..n]),
                    }
                }
                let _ = tx.send(seen);
                match reply {
                    Some(reply) => drop(conn.write_all(reply.as_bytes())),
                    None => thread::sleep(Duration::from_secs(3)),
                }
            }
        });
        (listener, rx)
    }

    fn port(listener: &TcpListener) -> u16 {
        listener.local_addr().unwrap().port()
    }

    /// Everything the listener was sent: no token, only the question.
    fn assert_no_token(seen: &mpsc::Receiver<Vec<u8>>) {
        while let Ok(bytes) = seen.try_recv() {
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains(TOKEN) && !text.contains("token="), "the listener was sent the token: {text}");
            assert!(text.starts_with("GET /api/hello?nonce="), "{text}");
        }
    }

    #[test]
    fn what_answers_wrongly_is_not_rettui_and_gets_no_token() {
        let wrong = format!("HTTP/1.1 200 OK\r\nContent-Length: 64\r\n\r\n{}", "ab".repeat(32)).leak();
        for reply in [&*wrong, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n", "HTTP/1.1 200 OK\r\n\r\n", "not http at all"] {
            let (held, seen) = listener(Some(reply));
            let result = verify(port(&held), TOKEN, Instant::now() + START, || false);
            let error = result.expect_err(reply);
            assert!(error.contains("is not rettui"), "{error}");
            assert!(!error.contains(TOKEN));
            assert_no_token(&seen);
        }
    }

    #[test]
    fn a_busy_answer_is_asked_again() {
        // Always busy: asked again until the time is up, then it fails; the token is never sent.
        let (held, seen) = listener(Some("HTTP/1.1 503 Service Unavailable\r\n\r\n"));
        let result = verify(port(&held), TOKEN, Instant::now() + Duration::from_millis(350), || false);
        assert!(result.expect_err("busy").contains("answered as rettui does"));
        assert!(seen.try_iter().count() >= 2, "it should have been asked again");
    }

    #[test]
    fn what_never_answers_is_not_given_the_token_either() {
        let (held, seen) = listener(None);
        let result = verify(port(&held), TOKEN, Instant::now() + Duration::from_millis(100), || false);
        assert!(result.expect_err("silent").contains("answered as rettui does"));
        assert_no_token(&seen);
    }

    #[test]
    fn a_stopped_rettui_ends_the_wait() {
        // Nothing listens on the port the listener had.
        let gone = port(&TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap());
        let result = verify(gone, TOKEN, Instant::now() + START, || true);
        assert_eq!(result.unwrap_err(), "rettui stopped before its web UI started");
    }

    #[test]
    fn answers_are_read_as_far_as_their_status() {
        let ask = |reply| {
            let (held, _) = listener(Some(reply));
            ask_hello(port(&held), &rettui::hello_nonce(), Instant::now() + ASK)
        };
        assert_eq!(ask("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n abc\n").unwrap(), "abc");
        // Final: another answer, or none that is HTTP.
        assert_eq!(ask("HTTP/1.1 404 Not Found\r\n\r\nabc").unwrap(), "");
        assert_eq!(ask("HTTP/1.1 401 Unauthorized\r\n\r\nabc").unwrap(), "");
        assert_eq!(ask("SSH-2.0-OpenSSH_9.0\r\n\r\n").unwrap(), "");
        // Passing: nothing yet, or busy.
        assert!(ask("").is_err());
        assert!(ask("HTTP/1.1 503 Service Unavailable\r\n\r\n").is_err());
        assert!(ask("HTTP/1.1 500 Oops\r\n\r\n").is_err());
        assert!(ask("HTTP/1.1 429 Too Many Requests\r\n\r\n").is_err());
    }

    /// DSK-01. The evidence that this fails on the base is the tester's
    /// `dsk01_b` and `dsk01_c` (pocs/DSK/t_dsk.rs), which run the same steps
    /// there. One test starts rettui for real, twice (a stop leaves a request
    /// behind that would stop a second test's run in this process).
    ///
    /// 1. Another program holds the port rettui is asked for, and an old port
    ///    file names it: the file is removed, rettui takes another port, the
    ///    link is for that one, and the other program is never connected to.
    /// 2. A restore (the same stop and start): the port stays bound while
    ///    rettui is stopped, so a program trying to take it all the while
    ///    never can, and rettui comes back on the same port.
    #[test]
    fn a_program_holding_the_port_gets_nothing_and_a_restore_does_not_free_it() {
        // One rettui at a time in this process: its stop request, restart flag and port are the process's.
        let _alone = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("rettui-desktop-dsk01-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let data = dir.join("data");
        std::fs::create_dir_all(dir.join("reticulum")).unwrap();
        // No interfaces, no first-start guide, no update check: nothing leaves this machine.
        std::fs::write(dir.join("reticulum/config"), "[reticulum]\n  share_instance = No\n\n[interfaces]\n").unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let settings =
            format!(r#"{{"welcomed":true,"update_check":false,"rns_config":{:?}}}"#, dir.join("reticulum").display().to_string());
        std::fs::write(data.join("settings.json"), settings).unwrap();
        let token_path = rettui::web_token_path(Some(data.clone())).unwrap();

        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        held.set_nonblocking(true).unwrap();
        let asked = port(&held);
        // A port file from an earlier run, naming the port the other program holds.
        let port_file = token_path.with_file_name(BOUND_PORT_FILE);
        std::fs::write(&port_file, asked.to_string()).unwrap();

        let app = mock_app();
        let server: Arc<Mutex<Option<Server>>> = Arc::default();
        let url = start(app.handle(), Some(&data), &token_path, asked, &server).expect("rettui should have taken another port");
        let bound = web_port();
        assert_ne!(url.port(), Some(asked));
        assert_eq!(url.port(), Some(bound));
        assert!(is_local(&url, bound) && !is_local(&url, asked));
        let token = std::fs::read_to_string(&token_path).unwrap();
        assert!(url.as_str().ends_with(&format!("/?token={}", token.trim())));
        // The next run asks for the port rettui has.
        assert_eq!(std::fs::read_to_string(token_path.with_file_name(SAVED_PORT_FILE)).unwrap(), bound.to_string());
        // Nothing connected to the other program, let alone sent it anything.
        assert_eq!(held.accept().map(|_| ()).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);

        // A restore: the file isn't there, so nothing is restored, but rettui
        // is stopped and started again all the same. Meanwhile another
        // program tries to take the port every millisecond.
        app.manage(manage::Runner { data_dir: Some(data.clone()), token_path: token_path.clone(), port: asked, server: server.clone() });
        let (done, taken) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let thief = {
            let (done, taken) = (done.clone(), taken.clone());
            thread::spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    if TcpListener::bind((Ipv4Addr::LOCALHOST, bound)).is_ok() {
                        taken.store(true, Ordering::SeqCst);
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            })
        };
        let restored = restore_and_restart(app.handle(), dir.join("no-such-backup").to_str().unwrap(), None);
        done.store(true, Ordering::SeqCst);
        thief.join().unwrap();
        // (stopping also ends the mock runtime's exit, which it can't do: an error here is that)
        let _ = stop(&server);
        assert!(restored.is_err(), "there was no backup to restore");
        assert!(!taken.load(Ordering::SeqCst), "another program took the port while rettui was stopped");
        assert_eq!(web_port(), bound, "rettui should have come back on the same port");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
