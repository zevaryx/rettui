//! What's heard on the network, in a database: the peers that announced
//! and the keys they announced with (`rettui.db`, SQLite, one per identity).
//!
//! Both change with every announce, several a second on a busy network.
//! Rewriting whole files for that (`peers.json.gz`, `known_identities.json`
//! before 2.0) cost a phone about 13 s of CPU and 137 MB of writes to its
//! flash an hour; here a save writes only the rows that changed.
//!
//! The database says which version of its layout it has (`user_version`).
//! An older one is brought up to date when opened; a newer one, made by a
//! later rettui, isn't opened at all, so it's never damaged. The JSON files
//! of earlier versions are read in once and renamed `*.migrated`: older
//! versions of rettui can't read the database.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};

use crate::net::PeerKind;
use crate::store::Peer;

/// The layout this version of rettui writes. Each step in [`UPGRADES`]
/// takes a database from one version to the next.
pub const VERSION: i64 = 1;

/// The steps from each version to the next: `UPGRADES[0]` makes version 1.
const UPGRADES: &[&str] = &["CREATE TABLE peers (
        dest BLOB PRIMARY KEY,
        kind TEXT NOT NULL,
        name TEXT,
        hops INTEGER NOT NULL,
        last_seen INTEGER NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE known (
        dest BLOB PRIMARY KEY,
        key BLOB NOT NULL,
        app_data BLOB
    ) WITHOUT ROWID;"];

/// The database beside the store, in the identity's data directory.
pub fn path(base: &Path) -> PathBuf {
    base.join("rettui.db")
}

/// Open the database at `path`, making it if there's none, and bring its
/// layout up to date. One made by a newer rettui is refused.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).with_context(|| format!("couldn't open {}", path.display()))?;
    // Another connection (the network's, or `rettui send`) may be writing.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    // With the log, a crash can lose only the last save, never damage the file.
    conn.execute_batch("PRAGMA synchronous=NORMAL")?;
    upgrade(&conn)?;
    Ok(conn)
}

fn upgrade(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > VERSION {
        bail!("it was made by a newer version of rettui (database version {version}; this one knows up to {VERSION})");
    }
    if version == VERSION {
        return Ok(());
    }
    // Every step and the new version in one transaction: all of it or none.
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let done = (|| {
        // Another process may have upgraded it while this one waited.
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        for step in UPGRADES.iter().skip(version.max(0) as usize).take((VERSION - version.max(0)) as usize) {
            conn.execute_batch(step)?;
        }
        conn.execute_batch(&format!("PRAGMA user_version = {VERSION}"))?;
        Ok::<(), anyhow::Error>(())
    })();
    match done {
        Ok(()) => conn.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e.context("couldn't update the database's layout"));
        }
    }
    Ok(())
}

fn kind_name(kind: PeerKind) -> &'static str {
    match kind {
        PeerKind::Lxmf => "Lxmf",
        PeerKind::Nomad => "Nomad",
        PeerKind::Propagation => "Propagation",
    }
}

fn kind_of(name: &str) -> Option<PeerKind> {
    match name {
        "Lxmf" => Some(PeerKind::Lxmf),
        "Nomad" => Some(PeerKind::Nomad),
        "Propagation" => Some(PeerKind::Propagation),
        _ => None,
    }
}

/// Every peer, by destination (hex). Rows it can't read are left out.
pub fn load_peers(conn: &Connection) -> Result<HashMap<String, Peer>> {
    let mut statement = conn.prepare("SELECT dest, kind, name, hops, last_seen FROM peers")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut peers = HashMap::new();
    for row in rows {
        let (dest, kind, name, hops, last_seen) = row?;
        if let Some(kind) = kind_of(&kind) {
            peers.insert(hex::encode(dest), Peer { kind, name, hops: hops.clamp(0, u8::MAX as i64) as u8, last_seen });
        }
    }
    Ok(peers)
}

/// Write the peers that changed and delete those forgotten, in one
/// transaction. Destinations that aren't hex are skipped.
pub fn save_peers(conn: &mut Connection, changed: &[(String, Peer)], removed: &[String]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut upsert = tx.prepare("INSERT OR REPLACE INTO peers (dest, kind, name, hops, last_seen) VALUES (?1, ?2, ?3, ?4, ?5)")?;
        for (dest, peer) in changed {
            if let Ok(dest) = hex::decode(dest) {
                upsert.execute(params![dest, kind_name(peer.kind), peer.name, peer.hops, peer.last_seen])?;
            }
        }
        let mut delete = tx.prepare("DELETE FROM peers WHERE dest = ?1")?;
        for dest in removed {
            if let Ok(dest) = hex::decode(dest) {
                delete.execute([dest])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// A public key and the announce data heard with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    pub key: Vec<u8>,
    pub app_data: Option<Vec<u8>>,
}

/// Every key kept, by destination (hex).
pub fn load_known(conn: &Connection) -> Result<HashMap<String, Known>> {
    let mut statement = conn.prepare("SELECT dest, key, app_data FROM known")?;
    let rows =
        statement.query_map([], |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, Option<Vec<u8>>>(2)?)))?;
    let mut known = HashMap::new();
    for row in rows {
        let (dest, key, app_data) = row?;
        known.insert(hex::encode(dest), Known { key, app_data });
    }
    Ok(known)
}

/// Write the keys that changed and delete those forgotten, in one transaction.
pub fn save_known(conn: &mut Connection, changed: &[(String, Known)], removed: &[String]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut upsert = tx.prepare("INSERT OR REPLACE INTO known (dest, key, app_data) VALUES (?1, ?2, ?3)")?;
        for (dest, known) in changed {
            if let Ok(dest) = hex::decode(dest) {
                upsert.execute(params![dest, known.key, known.app_data])?;
            }
        }
        let mut delete = tx.prepare("DELETE FROM known WHERE dest = ?1")?;
        for dest in removed {
            if let Ok(dest) = hex::decode(dest) {
                delete.execute([dest])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// `known_identities.json` of earlier versions: hex keys and announce data.
#[derive(serde::Deserialize)]
struct KnownJson {
    key: String,
    app_data: Option<String>,
}

/// What [`migrate_files`] did.
#[derive(Debug, Default)]
pub struct Migrated {
    /// For the log.
    pub notes: Vec<String>,
    /// Something was moved in from the files of an earlier version.
    pub moved: bool,
    /// The peers were.
    pub peers_in: bool,
}

/// Move the JSON files of earlier versions in `base` into the database:
/// `peers.json.gz` (and `extra` peers, from a store older still), and
/// `known_identities.json`. Each file is read in one transaction, and
/// renamed `*.migrated` only once that's saved; one that can't be read is
/// left as it is, to try again next time. A peer already in the database
/// keeps whichever was heard last.
pub fn migrate_files(conn: &mut Connection, base: &Path, extra: &HashMap<String, Peer>) -> Migrated {
    let mut migrated = Migrated::default();
    let peers_file = base.join("peers.json.gz");
    let mut peers: Vec<(String, Peer)> = extra.iter().map(|(k, p)| (k.clone(), p.clone())).collect();
    let from_file = peers_file.exists();
    if from_file {
        match crate::store::read_peers_file(&peers_file) {
            Ok(file) => peers.extend(file),
            Err(e) => {
                migrated.notes.push(format!("Couldn't read peers.json.gz ({e:#}): it's left as it is, and tried again next time"));
                peers.clear();
            }
        }
    }
    // The same peer in both: the one heard last.
    let mut newest: HashMap<String, Peer> = HashMap::new();
    for (dest, peer) in peers {
        if newest.get(&dest).is_none_or(|kept| kept.last_seen < peer.last_seen) {
            newest.insert(dest, peer);
        }
    }
    let peers: Vec<(String, Peer)> = newest.into_iter().collect();
    if !peers.is_empty() {
        match import_peers(conn, &peers) {
            Ok(()) => {
                migrated.moved = true;
                migrated.peers_in = true;
                migrated.notes.push(format!("Moved {} peer(s) into rettui.db", peers.len()));
                if from_file {
                    set_aside(&peers_file, &mut migrated.notes);
                }
            }
            Err(e) => migrated.notes.push(format!("Couldn't move the peers into rettui.db ({e:#}): tried again next time")),
        }
    }
    let known_file = base.join("known_identities.json");
    if known_file.exists() {
        let read = std::fs::read_to_string(&known_file)
            .map_err(anyhow::Error::from)
            .and_then(|text| Ok(serde_json::from_str::<HashMap<String, KnownJson>>(&text)?));
        match read {
            Ok(entries) => {
                let known: Vec<(String, Known)> = entries
                    .into_iter()
                    .filter_map(|(dest, entry)| {
                        let key = hex::decode(&entry.key).ok()?;
                        let app_data = entry.app_data.as_deref().and_then(|d| hex::decode(d).ok());
                        Some((dest, Known { key, app_data }))
                    })
                    .collect();
                match save_known(conn, &known, &[]) {
                    Ok(()) => {
                        migrated.moved = true;
                        migrated.notes.push(format!(
                            "Moved {} known identit{} into rettui.db",
                            known.len(),
                            if known.len() == 1 { "y" } else { "ies" }
                        ));
                        set_aside(&known_file, &mut migrated.notes);
                    }
                    Err(e) => {
                        migrated.notes.push(format!("Couldn't move the known identities into rettui.db ({e:#}): tried again next time"))
                    }
                }
            }
            Err(e) => {
                migrated.notes.push(format!("Couldn't read known_identities.json ({e:#}): it's left as it is, and tried again next time"))
            }
        }
    }
    migrated
}

/// Peers from files, merged in: the one heard last wins.
fn import_peers(conn: &mut Connection, peers: &[(String, Peer)]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut upsert = tx.prepare(
            "INSERT INTO peers (dest, kind, name, hops, last_seen) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(dest) DO UPDATE SET kind = excluded.kind, name = excluded.name, hops = excluded.hops, last_seen = excluded.last_seen
             WHERE excluded.last_seen > peers.last_seen",
        )?;
        for (dest, peer) in peers {
            if let Ok(dest) = hex::decode(dest) {
                upsert.execute(params![dest, kind_name(peer.kind), peer.name, peer.hops, peer.last_seen])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// A file moved in: renamed `<name>.migrated`, so it's still there to look at.
fn set_aside(path: &Path, notes: &mut Vec<String>) {
    let mut aside = path.as_os_str().to_owned();
    aside.push(".migrated");
    if let Err(e) = crate::config::replace(path, Path::new(&aside)) {
        // Read in again next time: harmless, as the newest of each is kept.
        notes.push(format!("Couldn't rename {} ({e})", path.display()));
    }
}

/// A copy of the database at `to`, whole and consistent while rettui is
/// writing to it (for backups). `to` mustn't exist yet.
pub fn snapshot(path: &Path, to: &Path) -> Result<()> {
    let conn = open(path)?;
    conn.execute("VACUUM INTO ?1", [to.to_string_lossy()])?;
    Ok(())
}

/// Copy the keys kept in the database at `from` into the one at `to` (a new
/// identity's), so it can write to those people at once.
pub fn copy_known(from: &Path, to: &Path) -> Result<usize> {
    let known = load_known(&open(from)?)?;
    let mut target = open(to)?;
    let known: Vec<(String, Known)> = known.into_iter().collect();
    save_known(&mut target, &known, &[])?;
    Ok(known.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn peer(name: &str, last_seen: i64) -> Peer {
        Peer { kind: PeerKind::Lxmf, name: Some(name.into()), hops: 2, last_seen }
    }

    #[test]
    fn peers_and_keys_are_saved_row_by_row() {
        let dir = temp_dir("rows");
        let mut conn = open(&path(&dir)).unwrap();
        let a = "aa".repeat(16);
        let b = "bb".repeat(16);
        save_peers(&mut conn, &[(a.clone(), peer("Amy", 10)), (b.clone(), peer("Bob", 20))], &[]).unwrap();
        save_peers(&mut conn, &[(a.clone(), peer("Amy", 30))], std::slice::from_ref(&b)).unwrap();
        let peers = load_peers(&conn).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!((peers[&a].name.as_deref(), peers[&a].last_seen), (Some("Amy"), 30));
        // Not hex: left out rather than failing the save.
        save_peers(&mut conn, &[("not hex".into(), peer("Eve", 1))], &[]).unwrap();
        assert_eq!(load_peers(&conn).unwrap().len(), 1);

        let known = Known { key: vec![7; 64], app_data: Some(b"named".to_vec()) };
        save_known(&mut conn, &[(a.clone(), known.clone())], &[]).unwrap();
        assert_eq!(load_known(&conn).unwrap()[&a], known);
        save_known(&mut conn, &[], std::slice::from_ref(&a)).unwrap();
        assert!(load_known(&conn).unwrap().is_empty());
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_database_from_a_newer_rettui_is_left_alone() {
        let dir = temp_dir("newer");
        let db = path(&dir);
        let conn = open(&db).unwrap();
        assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), VERSION);
        conn.execute_batch(&format!("PRAGMA user_version = {}", VERSION + 1)).unwrap();
        drop(conn);
        let refused = open(&db).unwrap_err().to_string();
        assert!(refused.contains("newer version of rettui"), "{refused}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_files_of_earlier_versions_move_in_once() {
        let dir = temp_dir("migrate");
        let a = "aa".repeat(16);
        let b = "bb".repeat(16);
        let peers: HashMap<String, Peer> = [(a.clone(), peer("Amy", 50))].into();
        crate::config::write_private(&dir.join("peers.json.gz"), &crate::store::encode_peers(&peers).unwrap()).unwrap();
        let known = format!(r#"{{"{a}":{{"key":"{}","app_data":"6869"}},"{b}":{{"key":"zz","app_data":null}}}}"#, "07".repeat(64));
        std::fs::write(dir.join("known_identities.json"), known).unwrap();
        // From a store older still, which held its peers itself: the newer
        // of the two is kept.
        let extra: HashMap<String, Peer> = [(a.clone(), peer("Amy (old)", 5)), (b.clone(), peer("Bob", 7))].into();

        let mut conn = open(&path(&dir)).unwrap();
        let migrated = migrate_files(&mut conn, &dir, &extra);
        assert!(migrated.moved, "{:?}", migrated.notes);
        let loaded = load_peers(&conn).unwrap();
        assert_eq!(loaded[&a].name.as_deref(), Some("Amy"));
        assert_eq!(loaded[&b].name.as_deref(), Some("Bob"));
        // The key that isn't hex is left out; the other kept with its data.
        let known = load_known(&conn).unwrap();
        assert_eq!(known.len(), 1);
        assert_eq!(known[&a].app_data.as_deref(), Some(&b"hi"[..]));
        // The files are set aside, and nothing moves next time.
        assert!(!dir.join("peers.json.gz").exists() && dir.join("peers.json.gz.migrated").exists());
        assert!(!dir.join("known_identities.json").exists() && dir.join("known_identities.json.migrated").exists());
        assert!(!migrate_files(&mut conn, &dir, &HashMap::new()).moved);
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_it_cant_read_stays_for_next_time() {
        let dir = temp_dir("unreadable");
        std::fs::write(dir.join("known_identities.json"), "{ not json").unwrap();
        std::fs::write(dir.join("peers.json.gz"), "not gzip").unwrap();
        let mut conn = open(&path(&dir)).unwrap();
        let migrated = migrate_files(&mut conn, &dir, &HashMap::new());
        assert!(!migrated.moved);
        assert_eq!(migrated.notes.len(), 2, "{:?}", migrated.notes);
        assert!(dir.join("known_identities.json").exists() && dir.join("peers.json.gz").exists());
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_snapshot_and_a_copy_of_the_keys() {
        let dir = temp_dir("copy");
        let mut conn = open(&path(&dir)).unwrap();
        let a = "aa".repeat(16);
        save_known(&mut conn, &[(a.clone(), Known { key: vec![1; 64], app_data: None })], &[]).unwrap();
        save_peers(&mut conn, &[(a.clone(), peer("Amy", 1))], &[]).unwrap();
        let copy = dir.join("snapshot.db");
        snapshot(&path(&dir), &copy).unwrap();
        assert_eq!(load_peers(&open(&copy).unwrap()).unwrap().len(), 1);
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(copy_known(&path(&dir), &path(&other)).unwrap(), 1);
        let theirs = open(&path(&other)).unwrap();
        assert_eq!(load_known(&theirs).unwrap().len(), 1);
        assert!(load_peers(&theirs).unwrap().is_empty(), "only the keys");
        drop((conn, theirs));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
