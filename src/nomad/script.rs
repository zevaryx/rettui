//! Running a hosted node's script pages for its owner, the way visitors get
//! them.
//!
//! rsNomad runs a visitor's script page itself (when "Run page scripts" is
//! on), but a client can't reach the node it hosts over Reticulum, so the
//! Browser runs the owner's own script pages here. It copies how rsNomad runs
//! them, so what the owner sees is what visitors get: no shell, nothing on
//! standard input, a cleared environment with only `PATH` (`/usr/bin:/bin`),
//! `link_id`, `remote_identity` and the request's `field_*` and `var_*`
//! values, as NomadNet passes them, and 10 seconds to finish. When visitors
//! would get nothing, the page says why, with what the script wrote to its
//! error output, which visitors never see.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

/// How long a script may run (rsNomad's limit for visitors).
#[cfg(unix)]
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Where scripts find programs (rsNomad's, for visitors).
pub const PATH: &str = "/usr/bin:/bin";

/// The most of a script's error output that is kept: its end.
#[cfg(unix)]
const MAX_STDERR: usize = 4 * 1024;

/// How a script run went.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// What it printed: the page visitors get.
    pub stdout: Vec<u8>,
    /// The end of its error output.
    pub stderr: Vec<u8>,
    /// Why visitors get nothing at all (it didn't start, ran too long or
    /// printed too much).
    pub failure: Option<String>,
}

/// Run a script page with a request's values, as rsNomad would for a
/// visitor. `remote_identity` is the identity hash of a visitor who
/// identified.
#[cfg(unix)]
pub fn run(script: &Path, remote_identity: Option<[u8; 16]>, fields: &BTreeMap<String, String>, max_stdout: usize) -> Run {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let mut command = Command::new(script);
    command.env_clear().env("PATH", PATH).env("link_id", hex::encode(rand::random::<[u8; 16]>()));
    if let Some(hash) = remote_identity {
        command.env("remote_identity", hex::encode(hash));
    }
    for (key, value) in fields.iter().filter(|(k, _)| k.starts_with("field_") || k.starts_with("var_")) {
        command.env(key, value);
    }
    // Its own process group, so whatever it starts is stopped with it.
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return Run { failure: Some(format!("it could not be started ({e})")), ..Run::default() },
    };
    let (Some(mut out), Some(mut err)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Run { failure: Some("its output could not be read".into()), ..Run::default() };
    };
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut out).take(max_stdout as u64 + 1).read_to_end(&mut buf);
        buf
    });
    let error_reader = std::thread::spawn(move || {
        // Read it all (so the script never stalls on a full pipe), keeping the end.
        let mut kept = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n) = err.read(&mut chunk) {
            if n == 0 {
                break;
            }
            kept.extend_from_slice(&chunk[..n]);
            if kept.len() > MAX_STDERR {
                kept.drain(..kept.len() - MAX_STDERR);
            }
        }
        kept
    });

    let start = Instant::now();
    let mut failure = None;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() >= TIMEOUT => {
                failure = Some(format!("it ran for longer than {} seconds and was stopped", TIMEOUT.as_secs()));
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                failure = Some(format!("waiting for it failed ({e})"));
                break;
            }
        }
    }
    // Stop it and anything it started, which could hold the pipes open.
    // SAFETY: the child leads its own process group (`process_group(0)`).
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    let stdout = reader.join().unwrap_or_default();
    let stderr = error_reader.join().unwrap_or_default();
    if failure.is_none() && stdout.len() > max_stdout {
        failure = Some(format!("it printed more than {} KB, the most a page may be", max_stdout / 1024));
    }
    if failure.is_some() {
        return Run { stdout: Vec::new(), stderr, failure };
    }
    Run { stdout, stderr, failure }
}

/// Scripts don't run on this system: rsNomad runs them only on Unix.
#[cfg(not(unix))]
pub fn run(_script: &Path, _remote_identity: Option<[u8; 16]>, _fields: &BTreeMap<String, String>, _max_stdout: usize) -> Run {
    Run { failure: Some("scripts only run on Linux, macOS and other Unix systems".into()), ..Run::default() }
}

/// The page the owner sees for a script page: what it printed, or, when
/// visitors would get nothing, why.
pub fn preview(script: &Path, remote_identity: Option<[u8; 16]>, fields: &BTreeMap<String, String>) -> Vec<u8> {
    let run = run(script, remote_identity, fields, nomad_core::DEFAULT_MAX_PAGE_BYTES);
    if run.failure.is_none() && !run.stdout.is_empty() {
        return run.stdout;
    }
    explain(script, &run).into_bytes()
}

fn explain(script: &Path, run: &Run) -> String {
    let why = run.failure.as_deref().unwrap_or("it printed nothing");
    let mut page = format!("#!c=0\n>Script page\n\nVisitors get nothing from this page: {}.\n", nomad_core::sanitize_micron_text(why));
    if let Some(hint) = interpreter_hint(script, std::env::var_os("PATH").as_deref()) {
        page.push_str(&format!("\n{}\n", nomad_core::sanitize_micron_text(&hint)));
    }
    let stderr = String::from_utf8_lossy(&run.stderr);
    if !stderr.trim().is_empty() {
        // Shown as it is; a backtick would end the literal block.
        page.push_str(&format!("\n>>What it wrote to its error output\n\n`=\n{}\n`=\n", stderr.trim_end().replace('`', "'")));
    }
    page.push_str("\nOnly you see this page: visitors never see a script's error output.\n");
    page
}

/// A fix for a first line naming a program that scripts can't find, as
/// `#!/usr/bin/env python3` does when Python isn't in `/usr/bin` or `/bin`.
/// `host_path` is this computer's own `PATH`, to say where the program is.
pub fn interpreter_hint(script: &Path, host_path: Option<&std::ffi::OsStr>) -> Option<String> {
    let mut first = [0u8; 256];
    let n = std::fs::File::open(script).and_then(|mut f| std::io::Read::read(&mut f, &mut first)).ok()?;
    let line = String::from_utf8_lossy(&first[..n]);
    let line = line.lines().next()?.strip_prefix("#!")?.trim();
    let mut words = line.split_whitespace();
    let program = words.next()?;
    if Path::new(program).file_name().is_some_and(|name| name == "env") {
        let name = words.find(|w| !w.starts_with('-'))?;
        if std::env::split_paths(PATH).any(|dir| dir.join(name).is_file()) {
            return None;
        }
        let found: Option<PathBuf> =
            host_path.and_then(|p| std::env::split_paths(p).map(|dir| dir.join(name)).find(|candidate| candidate.is_file()));
        return Some(match found {
            Some(at) => format!(
                "Scripts look for programs only in {PATH}, and {name} isn't there. It's at {}: start the page with #!{} instead.",
                at.display(),
                at.display()
            ),
            None => format!(
                "Scripts look for programs only in {PATH}, and {name} isn't there. Install it, or name it in full on the first line."
            ),
        });
    }
    (!Path::new(program).is_file()).then(|| format!("The first line names {program}, which isn't on this computer."))
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-script-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_script_gets_what_nomadnet_passes_and_nothing_else() {
        let dir = temp_dir("env");
        let page = script(
            &dir,
            "env.mu",
            "#!/bin/sh\nprintf '%s|%s|%s|%s|%s|%s\\n' \"$field_name\" \"$var_x\" \"$other\" \"$remote_identity\" \"$PATH\" \"$HOME\"\nprintf '%s' \"$link_id\" | wc -c\n",
        );
        let fields: BTreeMap<String, String> =
            [("field_name", "Zeva"), ("var_x", "1"), ("other", "no")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let out = run(&page, Some([0xab; 16]), &fields, 1024);
        assert_eq!(out.failure, None);
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.starts_with(&format!("Zeva|1||{}|{PATH}|\n", "ab".repeat(16))), "{text}");
        assert_eq!(text.lines().nth(1).map(str::trim), Some("32"), "a link id, as hex");
    }

    #[test]
    fn what_a_script_prints_is_the_page() {
        let dir = temp_dir("hello");
        let page = script(&dir, "hello.mu", "#!/bin/sh\necho '>Hello'\necho oops >&2\nexit 3\n");
        // Visitors still get what a failing script printed.
        assert_eq!(preview(&page, None, &BTreeMap::new()), b">Hello\n".to_vec());
        assert_eq!(run(&page, None, &BTreeMap::new(), 1024).stderr, b"oops\n");
    }

    #[test]
    fn a_script_that_prints_nothing_says_why_with_its_errors() {
        let dir = temp_dir("broken");
        let page = script(&dir, "broken.mu", "#!/bin/sh\necho 'Traceback: `bad`' >&2\nexit 1\n");
        let text = String::from_utf8(preview(&page, None, &BTreeMap::new())).unwrap();
        assert!(text.contains("Visitors get nothing from this page: it printed nothing."), "{text}");
        assert!(text.contains("`=\nTraceback: 'bad'\n`="), "{text}");
    }

    #[test]
    fn a_script_that_runs_too_long_is_stopped_with_what_it_started() {
        let dir = temp_dir("slow");
        // The background sleep holds the output open: it must be stopped too.
        let page = script(&dir, "slow.mu", "#!/bin/sh\necho partial\nsleep 60 &\nsleep 60\n");
        let started = std::time::Instant::now();
        let out = run(&page, None, &BTreeMap::new(), 1024);
        assert!(started.elapsed() < TIMEOUT + Duration::from_secs(5));
        assert!(out.failure.as_deref().is_some_and(|f| f.contains("longer than 10 seconds")), "{out:?}");
        assert!(out.stdout.is_empty(), "visitors get nothing when a script is stopped");
    }

    #[test]
    fn a_script_that_prints_too_much_gives_visitors_nothing() {
        let dir = temp_dir("big");
        let page = script(&dir, "big.mu", "#!/bin/sh\nhead -c 5000 /dev/zero\n");
        let out = run(&page, None, &BTreeMap::new(), 2048);
        assert_eq!(out.failure.as_deref(), Some("it printed more than 2 KB, the most a page may be"));
        assert!(out.stdout.is_empty());
    }

    #[test]
    fn a_script_that_cant_start_says_so() {
        let dir = temp_dir("missing");
        let page = script(&dir, "missing.mu", "#!/no/such/python3\nprint('hi')\n");
        let text = String::from_utf8(preview(&page, None, &BTreeMap::new())).unwrap();
        assert!(text.contains("it could not be started"), "{text}");
        assert!(text.contains("The first line names /no/such/python3, which isn't on this computer."), "{text}");
    }

    #[test]
    fn an_interpreter_outside_the_scripts_path_gets_a_fix() {
        let dir = temp_dir("hint");
        let bin = dir.join("bin");
        std::fs::create_dir(&bin).unwrap();
        script(&bin, "rettui-test-python", "#!/bin/sh\n");
        let page = script(&dir, "py.mu", "#!/usr/bin/env -S rettui-test-python -u\nprint('hi')\n");
        let host_path = std::env::join_paths([bin.clone()]).unwrap();
        let hint = interpreter_hint(&page, Some(&host_path)).unwrap();
        let at = bin.join("rettui-test-python");
        assert!(hint.contains(&format!("It's at {}: start the page with #!{} instead.", at.display(), at.display())), "{hint}");
        assert!(interpreter_hint(&page, None).unwrap().contains("Install it, or name it in full"));
        // sh is in /bin (or /usr/bin) everywhere: nothing to fix.
        let fine = script(&dir, "sh.mu", "#!/usr/bin/env sh\necho hi\n");
        assert_eq!(interpreter_hint(&fine, Some(&host_path)), None);
        let plain = script(&dir, "plain.mu", "#!/bin/sh\necho hi\n");
        assert_eq!(interpreter_hint(&plain, None), None);
    }
}
