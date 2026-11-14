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
//!
//! A backup can be sealed with a passphrase (see [`sealed`]): then only
//! someone who knows it can restore it.

pub mod sealed;

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Everything in a backup is under this folder.
const ROOT: &str = "rettui-backup";
/// What the backup says of itself.
const MANIFEST: &str = "rettui-backup.json";
/// Kept from the data directory: files, and folders with all they hold.
const KEPT: &[&str] = &[
    "settings.json",
    "store.json.gz",
    "peers.json.gz",
    "known_identities.json",
    "tickets.json",
    "games.json",
    "archive",
    "rrc",
    "node",
    "rrc-hub",
];
/// The identity: the private key behind your addresses.
const IDENTITY: &str = "identity";
/// The hosted RRC hub's identity, behind its address: kept with yours.
const HUB_IDENTITY: &str = "rrc-hub-identity";
/// The hosted rngit node's identity, behind its address: kept with yours.
const GIT_IDENTITY: &str = "git-host-identity";
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
        names.extend([IDENTITY, HUB_IDENTITY, GIT_IDENTITY]);
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
/// only its owner can read (it may hold the identity), sealed to `key` if
/// one is given; how many files it holds. A file there already is left
/// alone, and one left unfinished is removed.
pub fn write_file(base: &Path, to: &Path, options: Options, key: Option<&sealed::BackupKey>) -> Result<usize, String> {
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let file = open.open(to).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("{} exists already: pick another name", to.display()),
        _ => format!("{}: {e}", to.display()),
    })?;
    let out = std::io::BufWriter::new(file);
    let written = match key {
        None => create(base, out, options),
        Some(key) => sealed::Sealer::new(out, key).map_err(|e| e.to_string()).and_then(|mut sealer| {
            create(base, &mut sealer, options).and_then(|count| sealer.finish().map(|_| count).map_err(|e| e.to_string()))
        }),
    };
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
    let allowed = [MANIFEST, IDENTITY, HUB_IDENTITY, GIT_IDENTITY].contains(&first) || KEPT.contains(&first) || FILES.contains(&first);
    allowed.then(|| rel.to_path_buf())
}

/// Files and folders are restored; links aren't (one could point a later
/// file outside the data directory).
fn wanted<R: Read>(entry: &tar::Entry<R>) -> bool {
    let kind = entry.header().entry_type();
    kind.is_file() || kind.is_dir()
}

/// Write the backup sealed in `archive`, opened with `passphrase`, to the new
/// file `to` (only its owner may read it): the plain backup, or with
/// `identity` only the identity in it. What was written.
pub fn unseal(archive: &Path, passphrase: &str, to: &Path, identity: bool) -> Result<&'static str, String> {
    let file = || std::fs::File::open(archive).map_err(|e| format!("Couldn't open {}: {e}", archive.display()));
    let opener = sealed::Opener::new(std::io::BufReader::new(file()?), passphrase)
        .map_err(|e| format!("Couldn't open {}: {e}", archive.display()))?;
    let mut plain = opener.open(std::io::BufReader::new(file()?)).map_err(|e| format!("Couldn't open {}: {e}", archive.display()))?;
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let write = |data: &mut dyn Read| -> Result<(), String> {
        let mut out = open.open(to).map_err(|e| format!("{}: {e}", to.display()))?;
        let copied = std::io::copy(data, &mut out).and_then(|_| out.sync_all());
        copied.map_err(|e| {
            let _ = std::fs::remove_file(to);
            format!("Couldn't open {}: {e}", archive.display())
        })
    };
    if !identity {
        write(&mut plain)?;
        return Ok("the plain backup");
    }
    // Only the identity, from the backup as it's read.
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(plain));
    let bad = |e: std::io::Error| format!("{} isn't a rettui backup: {e}", archive.display());
    for entry in tar.entries().map_err(bad)? {
        let mut entry = entry.map_err(bad)?;
        if entry.path().map_err(bad)?.as_ref() == Path::new(ROOT).join(IDENTITY) && entry.header().entry_type().is_file() {
            write(&mut entry)?;
            return Ok("the identity");
        }
    }
    Err(format!("{} has no identity in it (backed up without it, or from the web UI)", archive.display()))
}

/// Whether the file `path` is a backup sealed with a passphrase.
pub fn is_sealed(path: &Path) -> bool {
    let mut start = [0u8; sealed::MAGIC.len()];
    std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut start)).is_ok() && sealed::is_sealed(&start)
}

/// Restore the backup `archive` into the data directory `base`, with
/// `passphrase` if it's sealed. One that has an identity or a store
/// already is left alone unless `force`, and then what the backup would
/// replace is moved aside first. What was done.
/// The most a backup may hold when restored, in bytes and in entries: a
/// backup from someone else can't fill the disk.
const MAX_RESTORE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_RESTORE_ENTRIES: usize = 200_000;

/// A backup that opened and is one, before anything is restored.
struct Inspected<'a> {
    archive: &'a Path,
    opener: Option<sealed::Opener>,
    manifest: Manifest,
    /// What it would put in the data directory, by first name.
    names: Vec<String>,
    files: usize,
}

impl Inspected<'_> {
    fn open(&self) -> Result<tar::Archive<flate2::read::GzDecoder<Box<dyn Read>>>, String> {
        let file = std::fs::File::open(self.archive).map_err(|e| format!("Couldn't open {}: {e}", self.archive.display()))?;
        let input = std::io::BufReader::new(file);
        let input: Box<dyn Read> = match &self.opener {
            Some(opener) => Box::new(opener.open(input).map_err(|e| format!("Couldn't open {}: {e}", self.archive.display()))?),
            None => Box::new(input),
        };
        Ok(tar::Archive::new(flate2::read::GzDecoder::new(input)))
    }
}

/// Open the backup `archive` (with `passphrase` if it's sealed) and check
/// that it is a rettui backup, changing nothing.
fn inspect<'a>(archive: &'a Path, passphrase: Option<&str>) -> Result<Inspected<'a>, String> {
    let file = || std::fs::File::open(archive).map_err(|e| format!("Couldn't open {}: {e}", archive.display()));
    // Sealed: the passphrase is checked once (it's slow), then each reading
    // opens it again.
    let opener = if is_sealed(archive) {
        let passphrase = passphrase.ok_or_else(|| format!("{} is sealed with a passphrase: give it to restore it", archive.display()))?;
        Some(
            sealed::Opener::new(std::io::BufReader::new(file()?), passphrase)
                .map_err(|e| format!("Couldn't open {}: {e}", archive.display()))?,
        )
    } else {
        None
    };
    let mut found = Inspected {
        archive,
        opener,
        manifest: Manifest { version: String::new(), created: 0, identity: false, files: false },
        names: Vec::new(),
        files: 0,
    };
    let bad = |e: &dyn std::fmt::Display| format!("{} isn't a rettui backup: {e}", archive.display());
    // First, that it's one: its manifest, and nothing outside what a
    // backup keeps.
    let mut manifest: Option<Manifest> = None;
    let (mut bytes, mut entries) = (0u64, 0usize);
    let mut archive_entries = found.open()?;
    for entry in archive_entries.entries().map_err(|e| bad(&e))? {
        let mut entry = entry.map_err(|e| bad(&e))?;
        let path = entry.path().map_err(|e| bad(&e))?.into_owned();
        let Some(rel) = destination(&path) else {
            return Err(bad(&format!("it holds {}", path.display())));
        };
        bytes = bytes.saturating_add(entry.size());
        entries += 1;
        if bytes > MAX_RESTORE_BYTES || entries > MAX_RESTORE_ENTRIES {
            return Err(bad(&"it is bigger than a backup should be"));
        }
        if !wanted(&entry) {
            continue;
        }
        if rel == Path::new(MANIFEST) {
            let mut text = String::new();
            entry.read_to_string(&mut text).map_err(|e| bad(&e))?;
            manifest = Some(serde_json::from_str(&text).map_err(|e| bad(&e))?);
        } else {
            if entry.header().entry_type().is_file() {
                found.files += 1;
            }
            if let Some(first) = rel.components().next().and_then(|c| c.as_os_str().to_str())
                && !found.names.iter().any(|n| n == first)
            {
                found.names.push(first.to_string());
            }
        }
    }
    drop(archive_entries);
    found.manifest = manifest.ok_or_else(|| bad(&"it has no rettui-backup.json"))?;
    Ok(found)
}

/// What a backup holds, checked without restoring it.
#[derive(Debug, PartialEq, Eq)]
pub struct Contents {
    pub version: String,
    pub identity: bool,
    pub files: usize,
}

/// Open the backup `archive`, with `passphrase` if it's sealed, and say what
/// it holds. Nothing is changed.
pub fn check(archive: &Path, passphrase: Option<&str>) -> Result<Contents, String> {
    let found = inspect(archive, passphrase)?;
    Ok(Contents { version: found.manifest.version, identity: found.manifest.identity, files: found.files })
}

pub fn restore(archive: &Path, base: &Path, force: bool, passphrase: Option<&str>) -> Result<String, String> {
    let bad = |e: &dyn std::fmt::Display| format!("{} isn't a rettui backup: {e}", archive.display());
    let Inspected { opener, manifest, names, .. } = inspect(archive, passphrase)?;
    let found = Inspected { archive, opener, manifest, names, files: 0 };
    let (manifest, names) = (&found.manifest, &found.names);
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
    for entry in found.open()?.entries().map_err(|e| bad(&e))? {
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
            // Backups hold data, never programs: no execute bits, and private.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
    // Identities are the owner's to read only.
    #[cfg(unix)]
    for name in [IDENTITY, HUB_IDENTITY, GIT_IDENTITY] {
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
    fn a_sealed_backup_restores_only_with_its_passphrase() {
        let base = std::env::temp_dir().join(format!("rettui-backup-sealed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (from, to) = (base.join("from"), base.join("to"));
        data(&from);
        let file = base.join("sealed.tar.gz");
        let key = sealed::quick_key("open sesame");
        let count = write_file(&from, &file, Options { identity: true, files: false }, Some(&key)).unwrap();
        assert_eq!(count, 6);
        assert!(is_sealed(&file));
        // Nothing of it reads without the passphrase: not even as gzip.
        assert!(!std::fs::read(&file).unwrap().windows(6).any(|w| w == b"secret"));
        assert!(restore(&file, &to, false, None).unwrap_err().contains("sealed with a passphrase"));
        assert!(restore(&file, &to, false, Some("open sesame?")).unwrap_err().contains("that isn't its passphrase"));
        assert!(!to.join("identity").exists());
        let done = restore(&file, &to, false, Some("open sesame")).unwrap();
        assert!(done.starts_with("Restored 6 files"), "{done}");
        assert_eq!(std::fs::read_to_string(to.join("identity")).unwrap(), "secret");
        // A plain one needs none, and ignores one given.
        let plain = base.join("plain.tar.gz");
        write_file(&from, &plain, Options::default(), None).unwrap();
        assert!(!is_sealed(&plain));
        assert!(restore(&plain, &base.join("again"), false, Some("unused")).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_backup_is_looked_into_without_changing_anything() {
        let base = std::env::temp_dir().join(format!("rettui-backup-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let from = base.join("from");
        data(&from);
        let plain = base.join("plain.tar.gz");
        write_file(&from, &plain, Options { identity: true, files: false }, None).unwrap();
        let held = check(&plain, None).unwrap();
        assert_eq!((held.identity, held.files), (true, 6));
        let sealed = base.join("sealed.tar.gz");
        write_file(&from, &sealed, Options::default(), Some(&sealed::quick_key("pw"))).unwrap();
        assert!(check(&sealed, None).unwrap_err().contains("sealed with a passphrase"));
        assert!(check(&sealed, Some("no")).unwrap_err().contains("that isn't its passphrase"));
        assert!(!check(&sealed, Some("pw")).unwrap().identity);
        // Not a backup at all.
        let other = base.join("other");
        std::fs::write(&other, "text").unwrap();
        assert!(check(&other, None).unwrap_err().contains("isn't a rettui backup"));
        assert!(check(&base.join("missing"), None).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_sealed_backup_opened_without_restoring_it() {
        let base = std::env::temp_dir().join(format!("rettui-backup-unseal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let from = base.join("from");
        data(&from);
        let file = base.join("sealed.tar.gz");
        write_file(&from, &file, Options { identity: true, files: false }, Some(&sealed::quick_key("pw"))).unwrap();
        // The plain backup, which restores as any other.
        let plain = base.join("plain.tar.gz");
        assert_eq!(unseal(&file, "pw", &plain, false).unwrap(), "the plain backup");
        assert!(!is_sealed(&plain));
        assert!(restore(&plain, &base.join("to"), false, None).is_ok());
        // Only the identity.
        let identity = base.join("identity");
        assert_eq!(unseal(&file, "pw", &identity, true).unwrap(), "the identity");
        assert_eq!(std::fs::read_to_string(&identity).unwrap(), "secret");
        // Never over a file, nor with another passphrase; none in it, said so.
        assert!(unseal(&file, "pw", &identity, true).unwrap_err().contains("identity"));
        assert!(unseal(&file, "nope", &base.join("x"), false).unwrap_err().contains("that isn't its passphrase"));
        let without = base.join("without.tar.gz");
        write_file(&from, &without, Options::default(), Some(&sealed::quick_key("pw"))).unwrap();
        assert!(unseal(&without, "pw", &base.join("none"), true).unwrap_err().contains("no identity"));
        let _ = std::fs::remove_dir_all(&base);
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
        let done = restore(&file, &to, false, None).unwrap();
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
        let done = restore(&file, &other, false, None).unwrap();
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
        assert!(restore(&file, &to, false, None).unwrap_err().contains("--force"));
        assert_eq!(std::fs::read_to_string(to.join("identity")).unwrap(), "mine");
        let done = restore(&file, &to, true, None).unwrap();
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
        assert!(restore(&file, &base.join("to"), false, None).unwrap_err().contains("isn't a rettui backup"));
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
        restore(&file, &to, false, None).unwrap();
        assert!(!std::fs::symlink_metadata(to.join("node")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(to.join("node/index.mu")).unwrap(), "hi");
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        // Written to a new file only its owner can read, never over one.
        let saved = base.join("saved.tar.gz");
        write_file(&to, &saved, Options::default(), None).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(write_file(&to, &saved, Options::default(), None).unwrap_err().contains("exists already"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn a_backup_restores_as_data_and_one_asking_for_too_much_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("rettui-backup-modes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let manifest = br#"{"version":"1.6.0","created":0,"identity":false,"files":false}"#;
        let build = |name: &str, size: Option<u64>| {
            let file = base.join(name);
            let mut tar =
                tar::Builder::new(flate2::write::GzEncoder::new(std::fs::File::create(&file).unwrap(), flate2::Compression::fast()));
            let mut header = tar::Header::new_gnu();
            header.set_size(manifest.len() as u64);
            header.set_cksum();
            tar.append_data(&mut header, format!("{ROOT}/{MANIFEST}"), &manifest[..]).unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o4755);
            match size {
                None => {
                    header.set_size(2);
                    header.set_cksum();
                    tar.append_data(&mut header, format!("{ROOT}/node/run.sh"), &b"hi"[..]).unwrap();
                }
                Some(size) => {
                    header.set_size(size);
                    header.set_path(format!("{ROOT}/node/huge")).unwrap();
                    header.set_cksum();
                    tar.append(&header, &b""[..]).unwrap();
                }
            }
            tar.into_inner().unwrap().finish().unwrap();
            file
        };
        let to = base.join("to");
        restore(&build("fine.tar.gz", None), &to, false, None).unwrap();
        assert_eq!(std::fs::metadata(to.join("node/run.sh")).unwrap().permissions().mode() & 0o7777, 0o600);
        let error = restore(&build("huge.tar.gz", Some(MAX_RESTORE_BYTES + 1)), &base.join("to2"), false, None).unwrap_err();
        assert!(error.contains("bigger than a backup should be"), "{error}");
        assert!(!base.join("to2").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
