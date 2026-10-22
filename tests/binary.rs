//! The `rettui` program itself, run as it would be from a shell: its
//! arguments, the commands that need no network, and the web UI from start
//! to a clean stop.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A fresh folder for one test, with a Reticulum config that has no
/// interfaces of its own beyond `interfaces`.
fn folder(name: &str, interfaces: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rettui-bin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("rns")).unwrap();
    std::fs::write(
        dir.join("rns/config"),
        format!(
            "[reticulum]\n  enable_transport = No\n  share_instance = No\n  panic_on_interface_error = No\n\n\
             [logging]\n  loglevel = 2\n\n[interfaces]\n{interfaces}"
        ),
    )
    .unwrap();
    // No update checks, nor map tiles from the web.
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/settings.json"), r#"{"update_check": false, "welcomed": true, "display_name": "bin"}"#).unwrap();
    dir
}

fn rettui(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rettui"));
    command.arg("--data-dir").arg(dir.join("data")).arg("--rns-config").arg(dir.join("rns"));
    command
}

fn run(dir: &Path, args: &[&str]) -> Output {
    rettui(dir).args(args).output().unwrap()
}

fn said(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn arguments_and_the_address() {
    let dir = folder("address", "");
    let help = run(&dir, &["--help"]);
    assert!(help.status.success() && said(&help).contains("--web"), "{}", said(&help));

    // The same address each time (the identity is made once, and kept).
    let address = run(&dir, &["address"]);
    assert!(address.status.success(), "{}", said(&address));
    let hash = String::from_utf8_lossy(&address.stdout).trim().to_string();
    assert_eq!(hash.len(), 32, "{hash}");
    assert_eq!(String::from_utf8_lossy(&run(&dir, &["address"]).stdout).trim(), hash);
    let link = run(&dir, &["address", "--link"]);
    assert!(String::from_utf8_lossy(&link.stdout).starts_with(&format!("lxma://{hash}:")), "{}", said(&link));

    // What can't be.
    let both = run(&dir, &["--web", "127.0.0.1:1", "address"]);
    assert!(!both.status.success() && said(&both).contains("cannot be combined"), "{}", said(&both));
    let mode = run(&dir, &["send", &hash, "hi", "--mode", "pigeon"]);
    assert!(!mode.status.success() && said(&mode).contains("unknown delivery mode"), "{}", said(&mode));
    let https = run(&dir, &["--https"]);
    assert!(!https.status.success(), "--https without --web");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn backing_up_and_restoring() {
    let dir = folder("backup", "");
    let address = String::from_utf8_lossy(&run(&dir, &["address"]).stdout).trim().to_string();
    let file = dir.join("backup.tar.gz");
    let backup = run(&dir, &["backup", file.to_str().unwrap()]);
    assert!(backup.status.success() && file.is_file(), "{}", said(&backup));

    // Into an empty folder: the same identity there.
    let other = dir.join("other");
    let restore = Command::new(env!("CARGO_BIN_EXE_rettui")).arg("--data-dir").arg(&other).args(["restore"]).arg(&file).output().unwrap();
    assert!(restore.status.success(), "{}", said(&restore));
    let restored = Command::new(env!("CARGO_BIN_EXE_rettui")).arg("--data-dir").arg(&other).arg("address").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&restored.stdout).trim(), address);
    // Over what's there: only with --force.
    let again = Command::new(env!("CARGO_BIN_EXE_rettui")).arg("--data-dir").arg(&other).args(["restore"]).arg(&file).output().unwrap();
    assert!(!again.status.success(), "{}", said(&again));
    let forced =
        Command::new(env!("CARGO_BIN_EXE_rettui")).arg("--data-dir").arg(&other).args(["restore", "--force"]).arg(&file).output().unwrap();
    assert!(forced.status.success(), "{}", said(&forced));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_identity_copied_and_kept_elsewhere() {
    let dir = folder("identity", "");
    // None yet: nothing to copy.
    let file = dir.join("identity-copy");
    let none = run(&dir, &["identity", "export", file.to_str().unwrap()]);
    assert!(!none.status.success() && said(&none).contains("start rettui once"), "{}", said(&none));

    let address = String::from_utf8_lossy(&run(&dir, &["address"]).stdout).trim().to_string();
    let export = run(&dir, &["identity", "export", file.to_str().unwrap()]);
    assert!(export.status.success() && said(&export).contains(&address), "{}", said(&export));
    let again = run(&dir, &["identity", "export", file.to_str().unwrap()]);
    assert!(!again.status.success() && said(&again).contains("exists already"), "{}", said(&again));

    // Kept as another identity in another data directory, whose own stays
    // in use.
    let other = folder("identity-other", "");
    let theirs = String::from_utf8_lossy(&run(&other, &["address"]).stdout).trim().to_string();
    let import = run(&other, &["identity", "import", file.to_str().unwrap(), "--name", "Amy"]);
    assert!(import.status.success() && said(&import).contains(&address), "{}", said(&import));
    assert!(other.join("data/identities").join(&address[..16]).join("identity").is_file());
    assert_eq!(String::from_utf8_lossy(&run(&other, &["address"]).stdout).trim(), theirs);
    let twice = run(&other, &["identity", "import", file.to_str().unwrap(), "--name", "Amy"]);
    assert!(!twice.status.success() && said(&twice).contains("here already"), "{}", said(&twice));
    let not = run(&other, &["identity", "import", dir.join("rns/config").to_str().unwrap(), "--name", "X"]);
    assert!(!not.status.success() && said(&not).contains("isn't an identity file"), "{}", said(&not));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn a_message_on_paper() {
    let dir = folder("paper", "");
    let to = String::from_utf8_lossy(&run(&dir, &["address", "--link"]).stdout).trim().to_string();
    let paper = run(&dir, &["send", &to, "On paper", "--mode", "paper"]);
    assert!(paper.status.success() && said(&paper).contains("lxm://"), "{}", said(&paper));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A running `rettui --web`, stopped (as `docker stop` and systemd do) when
/// dropped.
struct Web {
    child: Child,
    said: Arc<Mutex<Vec<String>>>,
}

impl Web {
    /// Started, once it says where it is.
    fn start(dir: &Path, args: &[&str]) -> Self {
        let mut child = rettui(dir).args(args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let said = Arc::new(Mutex::new(Vec::new()));
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let kept = said.clone();
        std::thread::spawn(move || {
            for line in lines.map_while(Result::ok) {
                kept.lock().unwrap().push(line);
            }
        });
        let web = Web { child, said };
        web.wait_for("Scripts can send the token");
        web
    }

    /// What it printed, once it has printed `text`.
    fn wait_for(&self, text: &str) -> String {
        let started = Instant::now();
        loop {
            let said = self.said.lock().unwrap().join("\n");
            if said.contains(text) {
                return said;
            }
            assert!(started.elapsed() < Duration::from_secs(30), "rettui didn't say {text:?}: {said}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Stopped by SIGTERM: it stops Reticulum and exits by itself.
    #[cfg(unix)]
    fn stop(mut self) -> std::process::ExitStatus {
        Command::new("kill").arg("-TERM").arg(self.child.id().to_string()).status().unwrap();
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(started.elapsed() < Duration::from_secs(30), "rettui didn't stop");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Web {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A GET over TLS (the web UI's own certificate, its authority trusted),
/// its status line and body.
#[cfg(unix)]
fn get_tls(port: u16, ca: &Path) -> (String, String) {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, ServerName};
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from_pem_file(ca).unwrap()).unwrap();
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connection = rustls::ClientConnection::new(std::sync::Arc::new(config), ServerName::try_from("127.0.0.1").unwrap()).unwrap();
    let tcp = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    // The server may keep the connection open after answering.
    tcp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut tls = rustls::StreamOwned::new(connection, tcp);
    write!(tls, "GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").unwrap();
    let mut raw = Vec::new();
    let _ = tls.read_to_end(&mut raw);
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    (head.lines().next().unwrap_or_default().to_string(), body.to_string())
}

/// A plain HTTP POST of `{}` (with the token), its status line.
fn post(port: u16, path: &str, token: &str) -> String {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    )
    .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    raw.lines().next().unwrap_or_default().to_string()
}

/// A plain HTTP GET (with the token), its status line and body.
fn get(port: u16, path: &str, token: &str) -> (String, String) {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    (head.lines().next().unwrap_or_default().to_string(), body.to_string())
}

#[cfg(unix)]
#[test]
fn the_web_ui_from_start_to_stop() {
    let rns_port = free_port();
    let dir =
        folder("web", &format!("  [[Here]]\n    type = TCPServerInterface\n    listen_ip = 127.0.0.1\n    listen_port = {rns_port}\n"));
    let port = free_port();
    let web = Web::start(&dir, &["--web", &format!("127.0.0.1:{port}")]);
    let token = std::fs::read_to_string(dir.join("data/web_token")).unwrap().trim().to_string();
    web.wait_for(&format!("rettui web UI: http://127.0.0.1:{port}/?token={token}"));

    // It answers, and Reticulum comes up behind it.
    let (status, _) = get(port, "/api/state", "wrong");
    assert!(status.contains("401"), "{status}");
    let started = Instant::now();
    loop {
        let (status, body) = get(port, "/api/state", &token);
        assert!(status.contains("200"), "{status}");
        if body.contains("\"state\":\"online\"") {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "Reticulum didn't start: {body}");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(get(port, "/api/conversations", &token).0.contains("200"));

    // Reticulum restarted from the web UI: down, then up again.
    assert!(post(port, "/api/reticulum/restart", &token).contains("200"));
    let started = Instant::now();
    loop {
        let (_, body) = get(port, "/api/state", &token);
        if body.contains("\"state\":\"online\"") && started.elapsed() > Duration::from_millis(500) {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "Reticulum didn't start again: {body}");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(post(port, "/api/announce", &token).contains("200"));

    // The same token the next time.
    let status = web.stop();
    assert!(status.success(), "{status:?}");
    let web = Web::start(&dir, &["--web", &format!("127.0.0.1:{port}")]);
    web.wait_for(&format!("?token={token}"));
    assert!(web.stop().success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn the_web_ui_over_https_for_other_devices() {
    let dir = folder("https", "");
    let port = free_port();
    // On every address: a link for other devices (and its QR code), and
    // rettui's own certificate authority to install.
    let web = Web::start(&dir, &["--web", &format!("0.0.0.0:{port}"), "--https"]);
    let shown = web.wait_for("From another device on this network");
    assert!(shown.contains(&format!("https://127.0.0.1:{port}/?token=")), "{shown}");
    assert!(shown.contains("rettui-ca.crt") && shown.contains("anyone with the link"), "{shown}");
    assert!(dir.join("data/web-tls").is_dir());
    // Plain HTTP on the HTTPS port: sent to the https:// address.
    let (status, _) = get(port, "/api/state", "x");
    assert!(status.contains("301"), "{status}");
    let mut odd = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    odd.write_all(b"nonsense\r\n\r\n").unwrap();
    let mut answer = String::new();
    let _ = odd.read_to_string(&mut answer);
    assert!(answer.contains("400"), "{answer}");
    // Over TLS, trusting rettui's own certificate authority.
    let (status, body) = get_tls(port, &dir.join("data/web-tls/ca.pem"));
    assert!(status.contains("401") || status.contains("200"), "{status} {body}");
    assert!(web.stop().success());

    // Its own certificate and key, from files: the ones just made.
    let tls = dir.join("data/web-tls");
    let pem =
        |ends: &str| std::fs::read_dir(&tls).unwrap().map(|e| e.unwrap().path()).find(|p| p.to_string_lossy().ends_with(ends)).unwrap();
    let (cert, key) = (pem("cert.pem"), pem("cert.key"));
    let web = Web::start(
        &dir,
        &["--web", &format!("127.0.0.1:{port}"), "--tls-cert", cert.to_str().unwrap(), "--tls-key", key.to_str().unwrap()],
    );
    web.wait_for("rettui web UI: https://");
    assert!(web.stop().success());

    // A certificate file with no certificate in it.
    let empty = dir.join("empty.pem");
    std::fs::write(&empty, "nothing here").unwrap();
    let none =
        run(&dir, &["--web", &format!("127.0.0.1:{port}"), "--tls-cert", empty.to_str().unwrap(), "--tls-key", key.to_str().unwrap()]);
    assert!(!none.status.success() && said(&none).contains("no certificate"), "{}", said(&none));

    // A bad address, said.
    let bad = run(&dir, &["--web", "nowhere"]);
    assert!(!bad.status.success() && said(&bad).contains("--web expects an address"), "{}", said(&bad));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The terminal UI, in a terminal of its own (`script` makes one): it draws,
/// takes keys, and quits on q.
#[cfg(target_os = "linux")]
#[test]
fn the_terminal_ui_starts_and_quits() {
    if Command::new("script").arg("--version").output().is_err() {
        return;
    }
    let dir = folder("tui", "");
    let program = format!(
        "stty cols 120 rows 40; exec {} --data-dir {} --rns-config {}",
        env!("CARGO_BIN_EXE_rettui"),
        dir.join("data").display(),
        dir.join("rns").display()
    );
    let mut child = Command::new("script")
        .args(["-qfec", &program, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut out = child.stdout.take().unwrap();
    let screen = Arc::new(Mutex::new(Vec::new()));
    let kept = screen.clone();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = out.read(&mut chunk) {
            if n == 0 {
                break;
            }
            kept.lock().unwrap().extend_from_slice(&chunk[..n]);
        }
    });
    let shown = |text: &str| String::from_utf8_lossy(&screen.lock().unwrap()).contains(text);
    let started = Instant::now();
    while !shown("Messages") {
        assert!(started.elapsed() < Duration::from_secs(30), "the terminal UI didn't draw");
        std::thread::sleep(Duration::from_millis(50));
    }
    // A few tabs, then q.
    let mut keys = child.stdin.take().unwrap();
    for key in ["2", "3", "5", "7", "1", "\x0c"] {
        keys.write_all(key.as_bytes()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
    }
    keys.write_all(b"q").unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "the terminal UI didn't quit");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{status:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
