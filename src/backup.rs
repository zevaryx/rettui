//! Backing up what rettui keeps, to one file, and restoring it: onto
//! another computer, or after a reinstall.
//!
//! A backup is a `.tar.gz` of the data directory's own files: settings,
//! contacts and conversations (the store, and its archive), the peers
//! heard, known identities, stamp tickets, RRC chat history, the hosted
//! node's pages (its default folder) and the hosted RRC hub's settings and
//! rooms; the identity and the hub's, unless left out (the web UI always
//! leaves them out); and with `files`, attachments received and sent. Caches, logs, the web UI's login and certificates, ratchets and
//! the propagation node's messages aren't kept: they're made again.
//!
//! Restoring puts them in a data directory while rettui isn't running
//! there: one that already has an identity or a store is left alone unless
//! asked (`force`), and then what was there is moved aside first.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Everything in a backup is under this folder.
const ROOT: &str = "rettui-backup";
/// What the backup says of itself.
const MANIFEST: &str = "rettui-backup.json";
/// Kept from the data directory: files, and folders with all they hold.
const KEPT: &[&str] =
    &["settings.json", "store.json.gz", "peers.json.gz", "known_identities.json", "tickets.json", "archive", "rrc", "node", "rrc-hub"];
/// The identity: the private key behind your addresses.
const IDENTITY: &str = "identity";
/// The hosted RRC hub's identity, behind its address: kept with yours.
const HUB_IDENTITY: &str = "rrc-hub-identity";
/// Attachments received and sent.
const FILES: &[&str] = &["downloads", "uploads"];

/// What goes in a backup besides the rest.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub identity: bool,
    pub files: bool,
}

/// What a backup says of itself.
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    version: String,
    /// Unix seconds.
    created: i64,
    identity: bool,
    files: bool,
}

/// What's in the data directory `base` that a backup with `options` keeps.
fn kept(options: Options) -> Vec<&'static str> {
    let mut names: Vec<&str> = KEPT.to_vec();
    if options.identity {
        names.extend([IDENTITY, HUB_IDENTITY]);
    }
    if options.files {
        names.extend(FILES);
    }
    names
}

/// Write a backup of the data directory `base` to `out`; how many files
/// it holds.
pub fn create(base: &Path, out: impl Write, options: Options) -> Result<usize, String> {
    let fail = |e: std::io::Error| e.to_string();
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(out, flate2::Compression::default()));
    tar.follow_symlinks(false);
    let manifest = Manifest {
        version: crate::update::VERSION.into(),
        created: chrono::Utc::now().timestamp(),
        identity: options.identity,
        files: options.files,
    };
    let manifest = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(chrono::Utc::now().timestamp().max(0) as u64);
    header.set_cksum();
    tar.append_data(&mut header, format!("{ROOT}/{MANIFEST}"), manifest.as_slice()).map_err(fail)?;
    let mut count = 0;
    for name in kept(options) {
        let path = base.join(name);
        if path.is_dir() {
            tar.append_dir_all(format!("{ROOT}/{name}"), &path).map_err(fail)?;
            count += walk_count(&path);
        } else if path.is_file() {
            tar.append_path_with_name(&path, format!("{ROOT}/{name}")).map_err(fail)?;
            count += 1;
        }
    }
    tar.into_inner().map_err(fail)?.finish().map_err(fail)?.flush().map_err(fail)?;
    Ok(count)
}

/// Write a backup of the data directory `base` to the new file `to`, which
/// only its owner can read (it may hold the identity); how many files it
/// holds. A file there already is left alone, and one left unfinished is
/// removed.
pub fn write_file(base: &Path, to: &Path, options: Options) -> Result<usize, String> {
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let file = open.open(to).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("{} exists already: pick another name", to.display()),
        _ => format!("{}: {e}", to.display()),
    })?;
    let written = create(base, std::io::BufWriter::new(file), options);
    if written.is_err() {
        let _ = std::fs::remove_file(to);
    }
    written
}

/// The file a backup made today is saved to, unless another is named.
pub fn default_name() -> String {
    format!("rettui-backup-{}.tar.gz", chrono::Local::now().format("%Y-%m-%d"))
}

fn walk_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| if entry.path().is_dir() { walk_count(&entry.path()) } else { 1 })
        .sum()
}

/// Where an entry of a backup goes, relative to the data directory: only
/// what a backup keeps, and never outside it.
fn destination(path: &Path) -> Option<PathBuf> {
    let rel = path.strip_prefix(ROOT).ok()?;
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let first = rel.components().next()?.as_os_str().to_str()?;
    let allowed = [MANIFEST, IDENTITY, HUB_IDENTITY].contains(&first) || KEPT.contains(&first) || FILES.contains(&first);
    allowed.then(|| rel.to_path_buf())
}

/// Files and folders are restored; links aren't (one could point a later
/// file outside the data directory).
fn wanted<R: Read>(entry: &tar::Entry<R>) -> bool {
    let kind = entry.header().entry_type();
    kind.is_file() || kind.is_dir()
}

/// Restore the backup `archive` into the data directory `base`. One that
/// has an identity or a store already is left alone unless `force`, and
/// then what the backup would replace is moved aside first. What was done.
pub fn restore(archive: &Path, base: &Path, force: bool) -> Result<String, String> {
    let open = || -> Result<tar::Archive<flate2::read::GzDecoder<std::fs::File>>, String> {
        let file = std::fs::File::open(archive).map_err(|e| format!("Couldn't open {}: {e}", archive.display()))?;
        Ok(tar::Archive::new(flate2::read::GzDecoder::new(file)))
    };
    let bad = |e: &dyn std::fmt::Display| format!("{} isn't a rettui backup: {e}", archive.display());
    // First, that it's one: its manifest, and nothing outside what a
    // backup keeps.
    let mut manifest: Option<Manifest> = None;
    let mut names: Vec<String> = Vec::new();
    for entry in open()?.entries().map_err(|e| bad(&e))? {
        let mut entry = entry.map_err(|e| bad(&e))?;
        let path = entry.path().map_err(|e| bad(&e))?.into_owned();
        let Some(rel) = destination(&path) else {
            return Err(bad(&format!("it holds {}", path.display())));
        };
        if !wanted(&entry) {
            continue;
        }
        if rel == Path::new(MANIFEST) {
            let mut text = String::new();
            entry.read_to_string(&mut text).map_err(|e| bad(&e))?;
            manifest = Some(serde_json::from_str(&text).map_err(|e| bad(&e))?);
        } else if let Some(first) = rel.components().next().and_then(|c| c.as_os_str().to_str())
            && !names.iter().any(|n| n == first)
        {
            names.push(first.to_string());
        }
    }
    let manifest = manifest.ok_or_else(|| bad(&"it has no rettui-backup.json"))?;
    // What's here already.
    std::fs::create_dir_all(base).map_err(|e| format!("Couldn't make {}: {e}", base.display()))?;
    let set_up = base.join(IDENTITY).exists() || base.join("store.json.gz").exists();
    let replaced: Vec<&String> = names.iter().filter(|n| base.join(n).exists()).collect();
    if set_up && !force {
        return Err(format!(
            "{} has rettui's identity or messages already: restore into another data directory (--data-dir), or with --force, which moves what's there aside first",
            base.display()
        ));
    }
    let mut aside = None;
    if !replaced.is_empty() {
        let folder = base.join(format!("before-restore-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        std::fs::create_dir_all(&folder).map_err(|e| format!("Couldn't make {}: {e}", folder.display()))?;
        for name in replaced {
            std::fs::rename(base.join(name), folder.join(name)).map_err(|e| format!("Couldn't move {name} aside: {e}"))?;
        }
        aside = Some(folder);
    }
    // Then everything in it.
    let mut count = 0;
    for entry in open()?.entries().map_err(|e| bad(&e))? {
        let mut entry = entry.map_err(|e| bad(&e))?;
        let path = entry.path().map_err(|e| bad(&e))?.into_owned();
        let Some(rel) = destination(&path) else { continue };
        if rel == Path::new(MANIFEST) || !wanted(&entry) {
            continue;
        }
        let target = base.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't make {}: {e}", parent.display()))?;
        }
        entry.unpack(&target).map_err(|e| format!("Couldn't restore {}: {e}", rel.display()))?;
        if entry.header().entry_type().is_file() {
            count += 1;
        }
    }
    // Identities are the owner's to read only.
    #[cfg(unix)]
    for name in [IDENTITY, HUB_IDENTITY] {
        use std::os::unix::fs::PermissionsExt;
        if base.join(name).is_file() {
            let _ = std::fs::set_permissions(base.join(name), std::fs::Permissions::from_mode(0o600));
        }
    }
    let mut done = format!("Restored {count} files from rettui {}'s backup into {}", manifest.version, base.display());
    if !manifest.identity {
        done.push_str(" (it has no identity: rettui makes a new one, with new addresses, unless you restore yours)");
    }
    if let Some(folder) = aside {
        done.push_str(&format!("; what was there is in {}", folder.display()));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(dir: &Path) {
        std::fs::create_dir_all(dir.join("archive")).unwrap();
        std::fs::create_dir_all(dir.join("downloads/abc")).unwrap();
        std::fs::create_dir_all(dir.join("cache")).unwrap();
        std::fs::create_dir_all(dir.join("rrc-hub")).unwrap();
        for (name, text) in [
            ("identity", "secret"),
            ("rrc-hub-identity", "hub secret"),
            ("rrc-hub/config.yaml", "hub: {}"),
            ("settings.json", "{}"),
            ("store.json.gz", "store"),
            ("archive/2026-10.jsonl", "old"),
            ("downloads/abc/photo.jpg", "jpeg"),
            ("cache/page.bin", "cached"),
            ("rettui.log", "log"),
            ("web_token", "token"),
        ] {
            std::fs::write(dir.join(name), text).unwrap();
        }
    }

    #[test]
    fn backed_up_and_restored_elsewhere_without_what_isnt_kept() {
        let base = std::env::temp_dir().join(format!("rettui-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (from, to) = (base.join("from"), base.join("to"));
        data(&from);
        let file = base.join("backup.tar.gz");
        let count = create(&from, std::fs::File::create(&file).unwrap(), Options { identity: true, files: false }).unwrap();
        assert_eq!(count, 6, "identities, settings, store, archive, the hub's config");
        let done = restore(&file, &to, false).unwrap();
        assert!(done.starts_with("Restored 6 files"), "{done}");
        assert_eq!(std::fs::read_to_string(to.join("identity")).unwrap(), "secret");
        assert_eq!(std::fs::read_to_string(to.join("rrc-hub-identity")).unwrap(), "hub secret");
        assert_eq!(std::fs::read_to_string(to.join("rrc-hub/config.yaml")).unwrap(), "hub: {}");
        assert_eq!(std::fs::read_to_string(to.join("archive/2026-10.jsonl")).unwrap(), "old");
        for missing in ["downloads", "cache", "rettui.log", "web_token"] {
            assert!(!to.join(missing).exists(), "{missing}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(to.join("identity")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // With attachments, without the identity.
        let file = base.join("files.tar.gz");
        create(&from, std::fs::File::create(&file).unwrap(), Options { identity: false, files: true }).unwrap();
        let other = base.join("other");
        let done = restore(&file, &other, false).unwrap();
        assert!(done.contains("no identity"), "{done}");
        assert!(other.join("downloads/abc/photo.jpg").exists() && !other.join("identity").exists());
        assert!(other.join("rrc-hub/config.yaml").exists() && !other.join("rrc-hub-identity").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_existing_setup_is_kept_unless_forced_and_then_moved_aside() {
        let base = std::env::temp_dir().join(format!("rettui-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (from, to) = (base.join("from"), base.join("to"));
        data(&from);
        let file = base.join("backup.tar.gz");
        create(&from, std::fs::File::create(&file).unwrap(), Options { identity: true, files: false }).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(to.join("identity"), "mine").unwrap();
        assert!(restore(&file, &to, false).unwrap_err().contains("--force"));
        assert_eq!(std::fs::read_to_string(to.join("identity")).unwrap(), "mine");
        let done = restore(&file, &to, true).unwrap();
        assert!(done.contains("before-restore-"), "{done}");
        assert_eq!(std::fs::read_to_string(to.join("identity")).unwrap(), "secret");
        let aside =
            std::fs::read_dir(&to).unwrap().flatten().find(|e| e.file_name().to_string_lossy().starts_with("before-restore-")).unwrap();
        assert_eq!(std::fs::read_to_string(aside.path().join("identity")).unwrap(), "mine");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn only_backups_and_only_into_the_data_directory() {
        assert_eq!(destination(Path::new("rettui-backup/archive/x.jsonl")), Some(PathBuf::from("archive/x.jsonl")));
        assert_eq!(destination(Path::new("rettui-backup/identity")), Some(PathBuf::from("identity")));
        for wrong in
            ["rettui-backup/../etc/passwd", "rettui-backup/web_token", "other/identity", "/etc/passwd", "rettui-backup/archive/../../x"]
        {
            assert_eq!(destination(Path::new(wrong)), None, "{wrong}");
        }
        let base = std::env::temp_dir().join(format!("rettui-not-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // A .tar.gz that isn't one: refused, nothing written.
        let file = base.join("x.tar.gz");
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(std::fs::File::create(&file).unwrap(), flate2::Compression::fast()));
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_cksum();
        tar.append_data(&mut header, "notes/a.txt", &b"hi"[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        assert!(restore(&file, &base.join("to"), false).unwrap_err().contains("isn't a rettui backup"));
        assert!(!base.join("to").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn links_in_a_backup_are_left_out() {
        let base = std::env::temp_dir().join(format!("rettui-backup-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        // A backup whose node folder is a link out of the data directory,
        // then a file "in" it.
        let file = base.join("links.tar.gz");
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(std::fs::File::create(&file).unwrap(), flate2::Compression::fast()));
        let manifest = br#"{"version":"1.6.0","created":0,"identity":false,"files":false}"#;
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest.len() as u64);
        header.set_cksum();
        tar.append_data(&mut header, format!("{ROOT}/{MANIFEST}"), &manifest[..]).unwrap();
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        tar.append_link(&mut link, format!("{ROOT}/node"), &outside).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, format!("{ROOT}/node/index.mu"), &b"hi"[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let to = base.join("to");
        restore(&file, &to, false).unwrap();
        assert!(!std::fs::symlink_metadata(to.join("node")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(to.join("node/index.mu")).unwrap(), "hi");
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        // Written to a new file only its owner can read, never over one.
        let saved = base.join("saved.tar.gz");
        write_file(&to, &saved, Options::default()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(write_file(&to, &saved, Options::default()).unwrap_err().contains("exists already"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
