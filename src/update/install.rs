//! Installing a newer release in this one's place (`rettui update`, `U` in
//! Status, the web UI's Install), only when asked: for the binaries the
//! releases page has, not a build from source, a container's, or one a
//! package manager installed (each says how to update it instead).
//!
//! The release's archive for this system is downloaded from GitHub with
//! its `SHA256SUMS`, checked against them, and its `rettui` unpacked beside
//! this one. That's run (`--version`) to be sure it works here, then put in
//! this one's place; rettui carries on as it was until it's started again.
//! The checksums come from the same release: they catch a download gone
//! wrong, not a release that isn't GitHub's.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::{Release, VERSION};

/// The target a release build was made for (the Build workflow sets it);
/// none for a build from source.
const RELEASE_TARGET: Option<&str> = option_env!("RETTUI_RELEASE_TARGET");
/// Where releases' files are.
const RELEASES: &str = "https://github.com/zevaryx/rettui/releases/download";
/// The most taken of an archive (they're 10–15 MB), and of a program
/// unpacked from one.
const MAX_ARCHIVE: u64 = 200_000_000;
/// How long a download may take.
const DOWNLOAD: Duration = Duration::from_secs(600);

/// How this copy of rettui is updated: in place (where it is, and what it
/// was built for), or not, and how instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    InPlace { exe: PathBuf, target: &'static str },
    Elsewhere(String),
}

impl Install {
    /// How this copy is updated (looked at once a run).
    pub fn here() -> Self {
        let exe = std::env::current_exe().and_then(|p| p.canonicalize());
        Self::from(RELEASE_TARGET, in_container(), exe)
    }

    fn from(target: Option<&'static str>, container: bool, exe: std::io::Result<PathBuf>) -> Self {
        let Some(target) = target else {
            return Self::Elsewhere("Built from source: update it with git pull --recurse-submodules, then build it again".into());
        };
        if container {
            return Self::Elsewhere("In a container: pull the new image (docker compose pull, then docker compose up -d)".into());
        }
        let exe = match exe {
            Ok(exe) => exe,
            Err(e) => return Self::Elsewhere(format!("Couldn't tell where rettui is ({e}): update it from the releases page")),
        };
        if managed(&exe) {
            return Self::Elsewhere("Installed by a package manager: update it with that".into());
        }
        match exe.parent() {
            Some(dir) if writable(dir) => Self::InPlace { exe, target },
            Some(dir) => Self::Elsewhere(format!("{} can't be written to: update it from the releases page", dir.display())),
            None => Self::Elsewhere("Update it from the releases page".into()),
        }
    }

    pub fn in_place(&self) -> bool {
        matches!(self, Self::InPlace { .. })
    }
}

/// In a container, where the program is the image's: replaced, it would
/// be lost with the container.
fn in_container() -> bool {
    std::env::var_os("RETTUI_CONTAINER").is_some() || Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists()
}

/// Where package managers keep the programs they install (Homebrew's
/// links lead into its Cellar).
fn managed(exe: &Path) -> bool {
    let text = exe.to_string_lossy();
    let system = ["/usr/", "/bin/", "/sbin/"].iter().any(|p| text.starts_with(p)) && !text.starts_with("/usr/local/");
    system || ["/Cellar/", "/nix/store/", "/snap/"].iter().any(|p| text.contains(p))
}

/// Whether a file can be made in `dir`.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".rettui-update-probe-{}", std::process::id()));
    let made = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    made
}

/// A release's archive for `target`, as the Build workflow names it.
fn archive_name(version: &str, target: &str) -> String {
    let kind = if target.contains("windows") { "zip" } else { "tar.gz" };
    format!("rettui-v{version}-{target}.{kind}")
}

/// The SHA-256 that `SHA256SUMS` (as sha256sum writes it) gives `name`.
fn checksum_for(sums: &str, name: &str) -> Option<[u8; 32]> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        if file != name {
            return None;
        }
        hex::decode(hash).ok()?.try_into().ok()
    })
}

/// The `rettui` program in a release's archive (a `.zip` for Windows).
fn unpack(archive: &[u8], windows: bool) -> Result<Vec<u8>, String> {
    let name = if windows { "rettui.exe" } else { "rettui" };
    let is_it = |path: &Path| path.file_name().is_some_and(|f| f == name);
    let read = |reader: &mut dyn Read| -> Result<Vec<u8>, String> {
        let mut data = Vec::new();
        reader.take(MAX_ARCHIVE).read_to_end(&mut data).map_err(|e| format!("Couldn't unpack the release: {e}"))?;
        Ok(data)
    };
    let bad = |e: &dyn std::fmt::Display| format!("Couldn't unpack the release: {e}");
    if windows {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).map_err(|e| bad(&e))?;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(|e| bad(&e))?;
            if file.is_file() && file.enclosed_name().is_some_and(|p| is_it(&p)) {
                return read(&mut file);
            }
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
        for entry in tar.entries().map_err(|e| bad(&e))? {
            let mut entry = entry.map_err(|e| bad(&e))?;
            let path = entry.path().map_err(|e| bad(&e))?.into_owned();
            if entry.header().entry_type().is_file() && is_it(&path) {
                return read(&mut entry);
            }
        }
    }
    Err(format!("There's no {name} in the release's archive"))
}

/// A file from GitHub, of at most `limit` bytes.
fn download(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let response = agent.get(url).call().map_err(|e| format!("Couldn't download the update: {e}"))?;
    let mut data = Vec::new();
    response.into_reader().take(limit + 1).read_to_end(&mut data).map_err(|e| format!("Couldn't download the update: {e}"))?;
    if data.len() as u64 > limit {
        return Err(format!("Couldn't download the update: {url} is bigger than it should be"));
    }
    Ok(data)
}

/// Install `release` in this copy's place, if it's newer and this copy is
/// updated in place. What was done, to say. Blocking, and a download of
/// 10–15 MB: for a thread of its own.
pub fn install_release(release: &Release) -> Result<String, String> {
    let (exe, target) = match Install::here() {
        Install::InPlace { exe, target } => (exe, target),
        Install::Elsewhere(how) => return Err(how),
    };
    if !super::newer(VERSION, &release.version) {
        return Err(format!("rettui {VERSION} is as new as {} already", release.version));
    }
    let program = fetch_program(&release.version, target)?;
    replace(&exe, &program, &release.version, run_version)?;
    Ok(format!("Installed rettui {}: start rettui again to use it", release.version))
}

/// A release's program for `target`, downloaded and checked against its
/// `SHA256SUMS`.
fn fetch_program(version: &str, target: &str) -> Result<Vec<u8>, String> {
    let agent = super::agent(DOWNLOAD);
    let name = archive_name(version, target);
    let base = format!("{RELEASES}/v{version}");
    let sums = download(&agent, &format!("{base}/SHA256SUMS"), 1_000_000)?;
    let expected = checksum_for(&String::from_utf8_lossy(&sums), &name)
        .ok_or_else(|| format!("rettui {version} has no {name} for this system: update it from the releases page"))?;
    let archive = download(&agent, &format!("{base}/{name}"), MAX_ARCHIVE)?;
    let found: [u8; 32] = Sha256::digest(&archive).into();
    if found != expected {
        return Err(format!("The download isn't what the release's SHA256SUMS says {name} is, so it isn't installed"));
    }
    unpack(&archive, target.contains("windows"))
}

/// What a program says it is (`--version`).
fn run_version(program: &Path) -> Result<String, String> {
    let output = std::process::Command::new(program).arg("--version").output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Put `program` in `exe`'s place, once it's run beside it and said it's
/// `version` (`check` runs it). On Windows, where a running program can't
/// be replaced, this one is moved aside (see [`tidy`]).
fn replace(exe: &Path, program: &[u8], version: &str, check: impl Fn(&Path) -> Result<String, String>) -> Result<(), String> {
    let dir = exe.parent().ok_or("rettui isn't in a folder")?;
    let new = dir.join(if cfg!(windows) { format!(".rettui-{version}-new.exe") } else { format!(".rettui-{version}-new") });
    let fail = |what: &str, e: &dyn std::fmt::Display| {
        let _ = std::fs::remove_file(&new);
        format!("Couldn't {what}: {e}")
    };
    std::fs::write(&new, program).map_err(|e| fail("write the new rettui", &e))?;
    // As this one is: runnable, by whoever could run it.
    if let Ok(meta) = std::fs::metadata(exe) {
        let _ = std::fs::set_permissions(&new, meta.permissions());
    }
    match check(&new) {
        Ok(said) if said.split_whitespace().any(|word| word == version) => {}
        Ok(said) => return Err(fail("install the new rettui", &format!("it says it's {said:?}, not {version}"))),
        Err(e) => return Err(fail("run the new rettui here, so it isn't installed", &e)),
    }
    if cfg!(windows) {
        let old = aside(exe);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old).map_err(|e| fail("move the old rettui aside", &e))?;
        if let Err(e) = std::fs::rename(&new, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(fail("put the new rettui in place", &e));
        }
    } else {
        std::fs::rename(&new, exe).map_err(|e| fail("put the new rettui in place", &e))?;
    }
    Ok(())
}

/// Where Windows' old program is moved while it runs.
fn aside(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

/// Remove the old program an update on Windows moved aside (at start, once
/// it's no longer running).
pub fn tidy() {
    if cfg!(windows)
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = std::fs::remove_file(aside(&exe));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_updates_in_place() {
        let dir = std::env::temp_dir().join(format!("rettui-install-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("rettui");
        let linux = "x86_64-unknown-linux-gnu";
        let target = Some(linux);
        assert_eq!(Install::from(target, false, Ok(exe.clone())), Install::InPlace { exe: exe.clone(), target: linux });
        let how = |install: Install| match install {
            Install::Elsewhere(how) => how,
            other => panic!("{other:?}"),
        };
        assert!(how(Install::from(None, false, Ok(exe.clone()))).starts_with("Built from source"));
        assert!(how(Install::from(target, true, Ok(exe.clone()))).starts_with("In a container"));
        for managed in ["/usr/bin/rettui", "/opt/homebrew/Cellar/rettui/1.6.0/bin/rettui", "/nix/store/abc-rettui/bin/rettui"] {
            assert!(how(Install::from(target, false, Ok(managed.into()))).contains("package manager"), "{managed}");
        }
        assert!(!managed(Path::new("/usr/local/bin/rettui")) && !managed(Path::new("/home/me/.local/bin/rettui")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archives_and_their_checksums() {
        assert_eq!(archive_name("1.7.0", "aarch64-apple-darwin"), "rettui-v1.7.0-aarch64-apple-darwin.tar.gz");
        assert_eq!(archive_name("1.7.0", "x86_64-pc-windows-msvc"), "rettui-v1.7.0-x86_64-pc-windows-msvc.zip");
        let a = "ab".repeat(32);
        let sums =
            format!("{a}  rettui-v1.7.0-x86_64-unknown-linux-gnu.tar.gz\n{}  rettui-v1.7.0-x86_64-pc-windows-msvc.zip\n", "cd".repeat(32));
        assert_eq!(checksum_for(&sums, "rettui-v1.7.0-x86_64-unknown-linux-gnu.tar.gz"), Some([0xab; 32]));
        assert_eq!(checksum_for(&sums, "rettui-v1.7.0-x86_64-pc-windows-msvc.zip"), Some([0xcd; 32]));
        // sha256sum's binary mode marks names with *.
        assert_eq!(checksum_for(&format!("{a} *x.tar.gz"), "x.tar.gz"), Some([0xab; 32]));
        assert_eq!(checksum_for(&sums, "rettui-v1.7.0-aarch64-apple-darwin.tar.gz"), None);
        assert_eq!(checksum_for("not hex  x.tar.gz", "x.tar.gz"), None);

        // As the Build workflow packs them: a folder with README.md and
        // the program.
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, data) in [("rettui-v1.7.0-x/README.md", &b"read me"[..]), ("rettui-v1.7.0-x/rettui", b"the program")] {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, path, data).unwrap();
        }
        let packed = tar.into_inner().unwrap().finish().unwrap();
        assert_eq!(unpack(&packed, false).unwrap(), b"the program");
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("rettui-v1.7.0-x/README.md", options).unwrap();
        std::io::Write::write_all(&mut zip, b"read me").unwrap();
        zip.start_file("rettui-v1.7.0-x/rettui.exe", options).unwrap();
        std::io::Write::write_all(&mut zip, b"the windows program").unwrap();
        let packed = zip.finish().unwrap().into_inner();
        assert_eq!(unpack(&packed, true).unwrap(), b"the windows program");
        assert!(unpack(&packed, false).is_err());
        assert!(unpack(b"not an archive", false).is_err());
    }

    #[test]
    fn put_in_place_only_once_it_runs() {
        let dir = std::env::temp_dir().join(format!("rettui-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("rettui");
        std::fs::write(&exe, b"old").unwrap();
        // It runs, and says it's the version: in place, nothing left over.
        replace(&exe, b"new", "1.7.0", |_| Ok("rettui 1.7.0".into())).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), if cfg!(windows) { 2 } else { 1 });
        // It doesn't run here, or says it's something else: left as it was.
        let refused = replace(&exe, b"newer", "1.8.0", |_| Err("Exec format error".into())).unwrap_err();
        assert!(refused.contains("isn't installed") && refused.contains("Exec format error"), "{refused}");
        let wrong = replace(&exe, b"newer", "1.8.0", |_| Ok("rettui 1.7.0".into())).unwrap_err();
        assert!(wrong.contains("not 1.8.0"), "{wrong}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert!(!std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains("1.8.0")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_new_program_is_run_to_check_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rettui-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("rettui");
        std::fs::write(&exe, b"#!/bin/sh\necho old\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A program that runs and says its version, made runnable as the
        // old one was.
        replace(&exe, b"#!/bin/sh\necho rettui 1.7.0\n", "1.7.0", run_version).unwrap();
        assert_eq!(run_version(&exe).unwrap(), "rettui 1.7.0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real release: downloaded, checked against its SHA256SUMS, unpacked
    /// and run in place of a stand-in. Online, so only when asked
    /// (`cargo test -- --ignored`).
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    #[ignore]
    fn a_real_release_downloads_checks_and_runs() {
        let program = fetch_program("1.5.1", "x86_64-unknown-linux-gnu").unwrap();
        let dir = std::env::temp_dir().join(format!("rettui-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("rettui");
        std::fs::write(&exe, b"stand-in").unwrap();
        replace(&exe, &program, "1.5.1", run_version).unwrap();
        assert_eq!(run_version(&exe).unwrap(), "rettui 1.5.1");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
