//! Backing up what rettui keeps, to one file, and restoring it: onto
//! another computer, or after a reinstall.
//!
//! A backup is a `.tar.gz` of the data directory's own files: settings,
//! contacts and conversations (the store, and its archive), the peers
//! heard and their keys (the database, copied whole while rettui runs),
//! stamp tickets, RRC chat history, the hosted
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
    DB,
    // Where versions before 2.0 kept the peers and keys: restored from their
    // backups, and moved into the database the next time rettui starts.
    "peers.json.gz",
    "known_identities.json",
    "tickets.json",
    "games.json",
    "archive",
    "rrc",
    "node",
    "rrc-hub",
];
/// The peers heard and their keys (see [`crate::db`]).
const DB: &str = "rettui.db";
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
        created: jiff::Timestamp::now().as_second(),
        identity: options.identity,
        files: options.files,
    };
    let manifest = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(jiff::Timestamp::now().as_second().max(0) as u64);
    header.set_cksum();
    tar.append_data(&mut header, format!("{ROOT}/{MANIFEST}"), manifest.as_slice()).map_err(fail)?;
    let mut count = 0;
    for name in kept(options) {
        let path = base.join(name);
        if name == DB {
            // Copied through SQLite, so a backup taken while rettui is
            // saving to it is whole: the file alone may be mid-write.
            if path.is_file() {
                let copy = std::env::temp_dir().join(format!(
                    "rettui-backup-{}-{}.db",
                    std::process::id(),
                    jiff::Timestamp::now().as_microsecond()
                ));
                let added = crate::db::snapshot(&path, &copy)
                    .map_err(|e| format!("Couldn't copy rettui.db: {e:#}"))
                    .and_then(|()| tar.append_path_with_name(&copy, format!("{ROOT}/{name}")).map_err(fail));
                let _ = std::fs::remove_file(&copy);
                added?;
                count += 1;
            }
        } else if path.is_dir() {
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
    format!("rettui-backup-{}.tar.gz", jiff::Zoned::now().strftime("%Y-%m-%d"))
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
/// The most the manifest may be when read: a real one is about 100 bytes.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
/// The settings file.
const SETTINGS: &str = "settings.json";

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
            (&mut entry).take(MAX_MANIFEST_BYTES + 1).read_to_string(&mut text).map_err(|e| bad(&e))?;
            if text.len() as u64 > MAX_MANIFEST_BYTES {
                return Err(bad(&"its rettui-backup.json is far bigger than one should be"));
            }
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

/// The settings file at `path` as JSON, if it is a normal-sized one and reads.
fn settings_json(path: &Path) -> Option<serde_json::Value> {
    if std::fs::metadata(path).ok()?.len() > crate::config::MAX_SETTINGS_BYTES {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Check the settings just restored to `path`, which came from outside:
/// ones rettui can't read are left out (it would stop at every start), and
/// the ones the web UI may not set, which decide what runs here, are put
/// back as `current` has them (see [`crate::config::withhold_web_barred`]),
/// unless the user asked for them as written (`keep_terminal_settings`:
/// then only said). What to tell the user, if anything was changed.
fn vet_settings(path: &Path, current: Option<&serde_json::Value>, keep_terminal_settings: bool) -> Option<String> {
    if let Err(e) = crate::config::Settings::load(path) {
        let _ = std::fs::remove_file(path);
        return Some(format!("its settings.json can't be read ({e:#}), so rettui starts with its default settings"));
    }
    let mut checked = settings_json(path)?;
    let withheld = crate::config::withhold_web_barred(&mut checked, current);
    if withheld.is_empty() {
        return None;
    }
    if keep_terminal_settings {
        return Some(format!(
            "restored as written, with --keep-terminal-settings, though they decide what runs on this computer: {}",
            withheld.join(", ")
        ));
    }
    // Written back the way rettui writes it.
    let written =
        serde_json::to_vec_pretty(&checked).map_err(anyhow::Error::from).and_then(|text| crate::config::write_private(path, &text));
    if let Err(e) = written {
        let _ = std::fs::remove_file(path);
        return Some(format!("its settings.json couldn't be checked ({e:#}), so rettui starts with its default settings"));
    }
    Some(format!(
        "not restored, because they decide what runs on this computer and a backup can come from anyone: {}. Set them again in the terminal UI (or in settings.json) if they're yours",
        withheld.join(", ")
    ))
}

/// Use the settings in `file`, as the settings of the data directory `base`,
/// checked as a backup's are (see [`vet_settings`]): a file that is too big or
/// doesn't read as settings is refused with the reason and nothing changes.
/// The settings it replaces are kept as `settings-before-import.json`. What
/// was done.
pub fn import_settings(file: &Path, base: &Path, keep_terminal_settings: bool) -> Result<String, String> {
    let refused = |why: &dyn std::fmt::Display| format!("{} isn't settings rettui can read: {why}", file.display());
    let bytes = crate::config::read_limited(file, crate::config::MAX_SETTINGS_BYTES).map_err(|e| refused(&e))?;
    std::fs::create_dir_all(base).map_err(|e| format!("Couldn't make {}: {e}", base.display()))?;
    let (target, stage) = (base.join(SETTINGS), base.join(format!("{SETTINGS}.importing")));
    crate::config::write_private(&stage, &bytes).map_err(|e| format!("Couldn't write {}: {e}", stage.display()))?;
    let undo = |why: String| {
        let _ = std::fs::remove_file(&stage);
        why
    };
    if let Err(e) = crate::config::Settings::load(&stage) {
        return Err(undo(refused(&format!("{e:#}").replace(&stage.display().to_string(), "the file"))));
    }
    let note = vet_settings(&stage, settings_json(&target).as_ref(), keep_terminal_settings);
    let before = base.join("settings-before-import.json");
    let kept = target.is_file() && crate::config::replace(&target, &before).is_ok();
    if let Err(e) = crate::config::replace(&stage, &target) {
        if kept {
            let _ = crate::config::replace(&before, &target);
        }
        return Err(undo(format!("Couldn't use the settings: {e}")));
    }
    let mut done = format!("Using the settings in {}", file.display());
    if let Some(note) = note {
        done.push_str("; ");
        done.push_str(&note);
    }
    if kept {
        done.push_str(&format!("; the ones replaced are kept as {}", before.display()));
    }
    Ok(done)
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
    restore_with(archive, base, force, passphrase, false)
}

/// [`restore`]; with `keep_terminal_settings`, the settings that only the
/// terminal may change are restored as the backup has them (see
/// [`vet_settings`]). Only the command line passes it: the web UI's and the
/// Android app's restores call [`restore`].
pub fn restore_with(
    archive: &Path,
    base: &Path,
    force: bool,
    passphrase: Option<&str>,
    keep_terminal_settings: bool,
) -> Result<String, String> {
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
    // The settings being replaced, for what the backup may not change.
    let current = names.iter().any(|n| n == SETTINGS).then(|| settings_json(&base.join(SETTINGS))).flatten();
    let mut aside = None;
    if !replaced.is_empty() {
        let folder = base.join(format!("before-restore-{}", jiff::Zoned::now().strftime("%Y%m%d-%H%M%S")));
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
    if names.iter().any(|n| n == SETTINGS)
        && let Some(note) = vet_settings(&base.join(SETTINGS), current.as_ref(), keep_terminal_settings)
    {
        done.push_str("; ");
        done.push_str(&note);
    }
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
    fn the_database_is_backed_up_whole_while_in_use() {
        let base = std::env::temp_dir().join(format!("rettui-backup-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (from, to) = (base.join("from"), base.join("to"));
        std::fs::create_dir_all(&from).unwrap();
        // Open, with a save not yet folded into the file itself.
        let mut db = crate::db::open(&from.join(DB)).unwrap();
        let peer = crate::store::Peer { kind: crate::net::PeerKind::Lxmf, name: Some("Amy".into()), hops: 1, last_seen: 5 };
        crate::db::save_peers(&mut db, &[("aa".repeat(16), peer)], &[]).unwrap();
        let file = base.join("backup.tar.gz");
        assert_eq!(create(&from, std::fs::File::create(&file).unwrap(), Options::default()).unwrap(), 1);
        drop(db);
        restore(&file, &to, false, None).unwrap();
        let peers = crate::db::load_peers(&crate::db::open(&to.join(DB)).unwrap()).unwrap();
        assert_eq!(peers[&"aa".repeat(16)].name.as_deref(), Some("Amy"));
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

    /// A backup of `files` (name, content) under the root, with `manifest` first.
    fn backup_of(path: &Path, manifest: &[u8], files: &[(&str, &[u8])]) {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(std::fs::File::create(path).unwrap(), flate2::Compression::fast()));
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest.len() as u64);
        header.set_cksum();
        tar.append_data(&mut header, format!("{ROOT}/{MANIFEST}"), manifest).unwrap();
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_cksum();
            tar.append_data(&mut header, format!("{ROOT}/{name}"), *content).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
    }

    const MANIFEST_JSON: &[u8] = br#"{"version":"2.0.0","created":0,"identity":false,"files":false}"#;

    #[test]
    fn a_manifest_of_gigabytes_is_refused_without_reading_it_whole() {
        let base = std::env::temp_dir().join(format!("rettui-backup-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // Spaces before the JSON: one more than it may be is enough to tell.
        let mut big = vec![b' '; MAX_MANIFEST_BYTES as usize];
        big.extend_from_slice(MANIFEST_JSON);
        let file = base.join("big.tar.gz");
        backup_of(&file, &big, &[]);
        let error = restore(&file, &base.join("to"), false, None).unwrap_err();
        assert!(error.contains("isn't a rettui backup") && error.contains("far bigger"), "{error}");
        assert!(!base.join("to").exists());
        let file = base.join("fine.tar.gz");
        backup_of(&file, MANIFEST_JSON, &[]);
        assert!(restore(&file, &base.join("to"), false, None).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn settings_the_web_ui_may_not_set_are_not_restored_as_written() {
        let base = std::env::temp_dir().join(format!("rettui-backup-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let evil = serde_json::json!({
            "display_name": "Zed",
            "voice_recorder": "sh -c 'id > /tmp/ran'",
            "print_command": "sh -c 'id > /tmp/ran'",
            "node_executable_pages": true,
            "rncp_listen": true,
            "rncp_allowed": "00".repeat(16),
            "rncp_dir": "/tmp",
            "rns_config": "/tmp/rns",
        });
        let file = base.join("evil.tar.gz");
        backup_of(&file, MANIFEST_JSON, &[(SETTINGS, evil.to_string().as_bytes())]);
        // Onto a new install: they go back to their defaults, and it says so.
        let to = base.join("to");
        let done = restore(&file, &to, false, None).unwrap();
        assert!(done.contains("not restored") && done.contains("decide what runs"), "{done}");
        let settings = crate::config::Settings::load(&to.join(SETTINGS)).unwrap();
        assert_eq!(settings.display_name, "Zed", "the rest is restored");
        assert!(settings.voice_recorder.is_none() && settings.print_command.is_none() && settings.rncp_allowed.is_none());
        assert!(settings.rncp_dir.is_none() && settings.rns_config.is_none());
        assert!(!settings.node_executable_pages && !settings.rncp_listen);
        // Onto a setup that has its own: those are kept.
        let mine = crate::config::Settings { voice_recorder: Some("arecord".into()), rncp_listen: true, ..Default::default() };
        std::fs::create_dir_all(&to).unwrap();
        mine.save(&to.join(SETTINGS)).unwrap();
        restore(&file, &to, true, None).unwrap();
        let settings = crate::config::Settings::load(&to.join(SETTINGS)).unwrap();
        assert_eq!(settings.display_name, "Zed");
        assert_eq!(settings.voice_recorder.as_deref(), Some("arecord"));
        assert!(settings.rncp_listen && settings.print_command.is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_command_line_can_ask_for_terminal_only_settings_as_written() {
        let base = std::env::temp_dir().join(format!("rettui-backup-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let mine = serde_json::json!({ "display_name": "Zed", "voice_recorder": "arecord -q", "rncp_listen": true });
        let file = base.join("mine.tar.gz");
        backup_of(&file, MANIFEST_JSON, &[(SETTINGS, mine.to_string().as_bytes())]);
        let done = restore_with(&file, &base.join("kept"), false, None, true).unwrap();
        assert!(done.contains("restored as written") && done.contains("Voice recorder"), "{done}");
        let settings = crate::config::Settings::load(&base.join("kept").join(SETTINGS)).unwrap();
        assert_eq!(settings.voice_recorder.as_deref(), Some("arecord -q"));
        assert!(settings.rncp_listen);
        // The same call without it (as the web UI and the app make it) leaves them out.
        let done = restore(&file, &base.join("left"), false, None).unwrap();
        assert!(done.contains("not restored"), "{done}");
        let settings = crate::config::Settings::load(&base.join("left").join(SETTINGS)).unwrap();
        assert!(settings.voice_recorder.is_none() && !settings.rncp_listen);
        // A file that can't be read is left out either way.
        let bad = base.join("bad.tar.gz");
        backup_of(&bad, MANIFEST_JSON, &[(SETTINGS, b"{not json")]);
        assert!(restore_with(&bad, &base.join("bad"), false, None, true).unwrap().contains("can't be read"));
        assert!(!base.join("bad").join(SETTINGS).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn ordinary_settings_come_back_untouched() {
        let base = std::env::temp_dir().join(format!("rettui-backup-settings-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (from, to) = (base.join("from"), base.join("to"));
        std::fs::create_dir_all(&from).unwrap();
        let mine = crate::config::Settings { display_name: "Amy".into(), ..Default::default() };
        mine.save(&from.join(SETTINGS)).unwrap();
        let file = base.join("backup.tar.gz");
        create(&from, std::fs::File::create(&file).unwrap(), Options::default()).unwrap();
        let done = restore(&file, &to, false, None).unwrap();
        assert!(!done.contains("not restored"), "{done}");
        assert_eq!(std::fs::read(from.join(SETTINGS)).unwrap(), std::fs::read(to.join(SETTINGS)).unwrap());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn settings_that_cant_be_read_are_left_out_so_rettui_still_starts() {
        let base = std::env::temp_dir().join(format!("rettui-backup-settings-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        for (name, text) in
            [("syntax", &b"{not json"[..]), ("type", br#"{"message_storage_mb":"x"}"#), ("huge", &vec![b' '; 2_000_000][..])]
        {
            let file = base.join(format!("{name}.tar.gz"));
            backup_of(&file, MANIFEST_JSON, &[(SETTINGS, text), ("store.json.gz", b"store")]);
            let to = base.join(name);
            let done = restore(&file, &to, false, None).unwrap();
            assert!(done.contains("settings.json can't be read"), "{name}: {done}");
            assert!(!to.join(SETTINGS).exists(), "{name}");
            assert!(to.join("store.json.gz").exists(), "{name}: the rest is restored");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn settings_brought_in_from_a_file_are_checked_as_a_backups_are() {
        let base = std::env::temp_dir().join(format!("rettui-import-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let data = base.join("data");
        let mine = crate::config::Settings { display_name: "Amy".into(), voice_recorder: Some("arecord".into()), ..Default::default() };
        std::fs::create_dir_all(&data).unwrap();
        mine.save(&data.join(SETTINGS)).unwrap();
        let before = std::fs::read(data.join(SETTINGS)).unwrap();
        let file = base.join("theirs.json");
        // A file that can't be read, or is far too big: refused with the reason, nothing changes.
        for (name, text) in
            [("syntax", &b"{not json"[..]), ("type", br#"{"message_storage_mb":"x"}"#), ("huge", &vec![b' '; 2_000_000][..])]
        {
            std::fs::write(&file, text).unwrap();
            let error = import_settings(&file, &data, false).unwrap_err();
            assert!(error.contains("isn't settings rettui can read"), "{name}: {error}");
            assert_eq!(std::fs::read(data.join(SETTINGS)).unwrap(), before, "{name}");
            assert!(!data.join("settings.json.importing").exists() && !data.join("settings-before-import.json").exists(), "{name}");
        }
        assert!(import_settings(&base.join("none.json"), &data, false).is_err());
        // One that reads: used, the ones in use kept, and what only a terminal sets stays as it was.
        let theirs =
            serde_json::json!({ "display_name": "Zed", "voice_recorder": "sh -c id", "print_command": "sh -c id", "messages_kept": 300 });
        std::fs::write(&file, theirs.to_string()).unwrap();
        let done = import_settings(&file, &data, false).unwrap();
        assert!(done.contains("not restored") && done.contains("Print command") && done.contains("settings-before-import.json"), "{done}");
        let now = crate::config::Settings::load(&data.join(SETTINGS)).unwrap();
        assert_eq!((now.display_name.as_str(), now.messages_kept), ("Zed", 300));
        assert_eq!(now.voice_recorder.as_deref(), Some("arecord"), "kept as it was");
        assert!(now.print_command.is_none());
        assert_eq!(std::fs::read(data.join("settings-before-import.json")).unwrap(), before);
        // With the terminal's flag, as written.
        let done = import_settings(&file, &data, true).unwrap();
        assert!(done.contains("restored as written"), "{done}");
        assert_eq!(crate::config::Settings::load(&data.join(SETTINGS)).unwrap().print_command.as_deref(), Some("sh -c id"));
        // Into a data directory with none.
        let fresh = base.join("fresh");
        assert!(!import_settings(&file, &fresh, false).unwrap().contains("kept as"));
        assert_eq!(crate::config::Settings::load(&fresh.join(SETTINGS)).unwrap().display_name, "Zed");
        let _ = std::fs::remove_dir_all(&base);
    }
}
