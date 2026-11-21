//! A Reticulum config change made from afar that isn't confirmed yet, kept
//! in `manage/pending.json` (readable by the node's user only) from before
//! the config is saved until the change is kept or put back. It holds the
//! SHA-256 of the file before and after the change (never the text, which
//! the config history keeps), so the old text can be found and checked, and
//! a file changed on this computer meanwhile is left alone.
//!
//! An unconfirmed change never outlives the run that made it: [`recover`]
//! runs when rettui starts (as a node, the TUI or the web UI), before
//! Reticulum does, and puts the old text back if the file is still as the
//! change left it. If the old text can't be written back, the file stays (as
//! being rolled back), and the next start, or the node's owner loop, tries
//! again; only a change whose old text is nowhere to be found is given up.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::audit::{Audit, Change, Outcome, Record};
use super::{lower_hex, unix_now};
use crate::config::Paths;
use crate::reticulum::history::History;

/// The file, in the management folder.
pub const FILE: &str = "pending.json";
/// The most of it that is read.
const MAX_FILE: u64 = 64 * 1024;
/// The most of the Reticulum config read to compare it (more than any
/// config a change is made to).
const MAX_CONFIG: u64 = 2 * 1024 * 1024;

/// Where a change is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Saved; Reticulum is restarting with it.
    Restarting,
    /// Reticulum is back with it; waiting for the website to confirm it.
    Probation,
    /// Not confirmed in time, or Reticulum isn't back: the old text is put back.
    RollingBack,
    /// Confirmed: the change is kept (the file is removed next).
    Committed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Restarting => "restarting",
            State::Probation => "probation",
            State::RollingBack => "rolling_back",
            State::Committed => "committed",
        }
    }

    /// The state in a sentence: "while the change was {words}".
    fn words(self) -> &'static str {
        match self {
            State::Restarting => "restarting Reticulum",
            State::Probation => "on probation",
            State::RollingBack => "being rolled back",
            State::Committed => "kept",
        }
    }
}

/// A change waiting to be confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// The layout of this file: 1.
    pub v: u32,
    /// The change's id (16 hex), as the audit log has it.
    pub id: String,
    /// The website's user who made it, the only one who may confirm it.
    pub actor: String,
    /// The Reticulum config file it changed.
    pub config: PathBuf,
    /// The config history's id of the text before the change, if it was kept.
    pub backup: Option<String>,
    pub before_sha256: String,
    pub after_sha256: String,
    pub state: State,
    /// Unix seconds.
    pub applied_at: u64,
    pub confirm_seconds: u64,
    pub healthy_at: Option<u64>,
    pub confirm_by: Option<u64>,
    pub changes: Vec<Change>,
    /// The interfaces that must be online again for the change to be healthy.
    pub required: Vec<String>,
}

/// The file's place.
pub fn path(paths: &Paths) -> PathBuf {
    paths.root.join("manage").join(FILE)
}

/// SHA-256 of `bytes`, as 64 lowercase hex characters.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The pending change, if there is one. A file that can't be read, isn't
/// JSON or doesn't look like one of ours is an error.
pub fn load(paths: &Paths) -> Result<Option<Pending>, String> {
    let text = match crate::config::read_limited_text(&path(paths), MAX_FILE) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("it can't be read: {e}")),
    };
    let pending: Pending = serde_json::from_str(&text).map_err(|e| format!("it isn't a pending change: {e}"))?;
    let fine = pending.v == 1
        && lower_hex(&pending.id, 16)
        && lower_hex(&pending.before_sha256, 64)
        && lower_hex(&pending.after_sha256, 64)
        && !pending.actor.is_empty()
        && pending.actor.len() <= 32;
    if !fine {
        return Err("it isn't a pending change this version knows".into());
    }
    Ok(Some(pending))
}

/// Save the pending change, readable by the node's user only.
pub fn save(paths: &Paths, pending: &Pending) -> Result<(), String> {
    let file = path(paths);
    if let Some(dir) = file.parent() {
        super::audit::make_private_dir(dir).map_err(|e| format!("Couldn't save the pending change: {e}"))?;
    }
    let json = serde_json::to_vec_pretty(pending).map_err(|e| format!("Couldn't save the pending change: {e}"))?;
    crate::config::write_private(&file, &json).map_err(|e| format!("Couldn't save the pending change: {e:#}"))
}

/// Remove the pending change's file (there being none is fine).
pub fn remove(paths: &Paths) -> Result<(), String> {
    match std::fs::remove_file(path(paths)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Couldn't remove the pending change ({}): {e}", path(paths).display())),
    }
}

/// The SHA-256 of the config file as it is now, if it can be read.
pub fn config_sha256(config: &Path) -> Option<String> {
    crate::config::read_limited(config, MAX_CONFIG).ok().map(|bytes| sha256_hex(&bytes))
}

/// The text the config had before the change: the version the history kept
/// for it, else any version of that file with the same SHA-256, else
/// `config.backup` if it still holds it. Checked against `before_sha256`, so
/// nothing else is ever put back.
fn backup_text(history: &History, pending: &Pending) -> Option<String> {
    let matches = |text: &String| sha256_hex(text.as_bytes()) == pending.before_sha256;
    let kept = pending.backup.iter().cloned().chain(history.list(&pending.config).into_iter().map(|v| v.id));
    for id in kept {
        if let Some(text) = history.text(&id).ok().filter(matches) {
            return Some(text);
        }
    }
    let backup = pending.config.with_file_name("config.backup");
    crate::config::read_limited_text(&backup, MAX_CONFIG).ok().filter(matches)
}

/// Why the old text couldn't be put back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreError {
    /// No copy of it was found: there is nothing to try again with.
    NoCopy,
    /// The config is a symbolic link, which isn't followed.
    Link,
    /// It couldn't be written (why, for the node's log only): try again.
    Write(String),
}

impl RestoreError {
    /// What the audit log and the answers say (nothing from the system).
    pub fn message(&self) -> &'static str {
        match self {
            RestoreError::NoCopy => "No copy of the config from before the change was found, so it was left as it is",
            RestoreError::Link => "The Reticulum config is a link, so it was left as it is: change it in the terminal",
            RestoreError::Write(_) => "The config from before the change couldn't be written back (see the node's log); trying again",
        }
    }
}

/// Whether `config` is a symbolic link (a change through it would land
/// elsewhere).
pub fn is_link(config: &Path) -> bool {
    std::fs::symlink_metadata(config).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Put the text from before the change back (without restarting anything),
/// keeping it in the history as rolled back for `reason`.
pub fn restore(history: &History, pending: &Pending, reason: &str) -> Result<(), RestoreError> {
    if is_link(&pending.config) {
        return Err(RestoreError::Link);
    }
    let Some(text) = backup_text(history, pending) else { return Err(RestoreError::NoCopy) };
    crate::reticulum::save(&pending.config, &text).map_err(RestoreError::Write)?;
    if let Err(e) =
        history.record(&pending.config, &text, &format!("Rolled back remote change {}: {reason}", pending.id), crate::app::now())
    {
        tracing::warn!("{e}");
    }
    Ok(())
}

/// The id the audit log and `/state` give a change whose file couldn't be read.
const UNKNOWN_ID: &str = "0000000000000000";

/// What the start found out about the last change, for `/state` until the
/// next change ends (`manage/start-result.json`).
#[derive(Debug, Serialize, Deserialize)]
pub struct StartResult {
    pub id: String,
    /// `rolled_back` or `left`.
    pub outcome: String,
    pub at: u64,
    pub message: String,
}

fn start_result_path(paths: &Paths) -> PathBuf {
    paths.root.join("manage").join("start-result.json")
}

fn save_start_result(paths: &Paths, result: &StartResult) {
    let written = serde_json::to_vec(result)
        .map_err(|e| e.to_string())
        .and_then(|bytes| crate::config::write_private(&start_result_path(paths), &bytes).map_err(|e| e.to_string()));
    if let Err(e) = written {
        tracing::warn!("Remote management: couldn't save what the start found: {e}");
    }
}

/// What the start found out, once: the file is removed as it is read.
pub fn take_start_result(paths: &Paths) -> Option<StartResult> {
    let path = start_result_path(paths);
    let bytes = crate::config::read_limited(&path, MAX_FILE).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_slice(&bytes).ok()
}

/// At the start, before Reticulum: what to do with a change left pending by
/// the last run (it was never confirmed, so it isn't kept). The config is put
/// back if it is as the change left it; anything else is left as it is and
/// said. `rns_config` is the Reticulum config directory in use. Returns what
/// to tell the operator; the audit log says it too.
pub fn recover(paths: &Paths, rns_config: Option<&str>) -> Vec<String> {
    let pending = match load(paths) {
        Ok(Some(pending)) => pending,
        Ok(None) => return Vec::new(),
        Err(why) => {
            let aside = path(paths).with_file_name(format!("{FILE}.unreadable-{}", unix_now()));
            let moved = match std::fs::rename(path(paths), &aside) {
                Ok(()) => format!("moved it to {}", aside.display()),
                Err(e) => format!("and it couldn't be moved aside: {e}"),
            };
            // What the change was can't be known, and it was never confirmed: the config may
            // still hold it. So the next change waits, as after a rollback, and the website is told.
            super::recent::ChangeLimits::hold_saved(paths, super::owner::HOLD_AFTER_ROLLBACK);
            let message = format!(
                "The file of a Reticulum change left by the last run couldn't be read, so that change, if it was never confirmed, may still be in the Reticulum config: check the config against its history. The next change waits {} minutes",
                super::owner::HOLD_AFTER_ROLLBACK.as_secs() / 60
            );
            let mut audit = Audit::load(paths.root.join("manage").join("audit.jsonl"));
            let record = Record {
                id: UNKNOWN_ID,
                actor: "node",
                op: "auto_rollback",
                changes: Vec::new(),
                outcome: Outcome::Failed,
                error: Some(&message),
            };
            if let Err(e) = audit.record(record, unix_now()) {
                tracing::warn!("Remote management: {e}");
            }
            save_start_result(
                paths,
                &StartResult { id: UNKNOWN_ID.into(), outcome: "left".into(), at: unix_now(), message: message.clone() },
            );
            let line = format!(
                "warning: remote management: {} was left by the last run, but {why}; {moved}. A change that was never confirmed may still be in the Reticulum config: check it against its history. The next change waits {} minutes",
                path(paths).display(),
                super::owner::HOLD_AFTER_ROLLBACK.as_secs() / 60
            );
            tracing::warn!("{line}");
            return vec![line];
        }
    };
    let mut said = Vec::new();
    let mut keep = false;
    let mut hold = false;
    if pending.state == State::Committed {
        if let Err(e) = remove(paths) {
            said.push(format!("warning: remote management: {e}"));
        }
        return said;
    }
    let mut audit = Audit::load(paths.root.join("manage").join("audit.jsonl"));
    let mut record = |op: &'static str, outcome: Outcome, error: &str| {
        let record = Record { id: &pending.id, actor: &pending.actor, op, changes: pending.changes.clone(), outcome, error: Some(error) };
        if let Err(e) = audit.record(record, unix_now()) {
            tracing::warn!("Remote management: {e}");
        }
    };
    let current = crate::reticulum::config_path(rns_config);
    let line = if pending.config != current {
        record("auto_rollback", Outcome::Failed, "The node uses another Reticulum config now, so the change was left as it is");
        format!(
            "warning: remote management: change {} to {} wasn't confirmed, but the node uses {} now: left it as it is",
            pending.id,
            pending.config.display(),
            current.display()
        )
    } else {
        let sha = config_sha256(&pending.config);
        if sha.as_deref() == Some(pending.after_sha256.as_str()) {
            let reason = format!("The node stopped while the change was {}", pending.state.words());
            match restore(&History::new(paths.reticulum_history.clone()), &pending, &reason) {
                Ok(()) => {
                    // The next change waits, as after one the node put back while it ran.
                    hold = true;
                    record("auto_rollback", Outcome::RolledBack, &reason);
                    save_start_result(
                        paths,
                        &StartResult {
                            id: pending.id.clone(),
                            outcome: "rolled_back".into(),
                            at: unix_now(),
                            message: "The node stopped before the change was confirmed: the Reticulum config was put back as it was".into(),
                        },
                    );
                    format!(
                        "Remote management: change {} wasn't confirmed before the node stopped: put the Reticulum config back as it was",
                        pending.id
                    )
                }
                Err(RestoreError::Write(e)) => {
                    // Kept, as being rolled back, for the next try.
                    keep = true;
                    hold = true;
                    let rolling = Pending { state: State::RollingBack, ..pending.clone() };
                    if let Err(e) = save(paths, &rolling) {
                        tracing::warn!("Remote management: {e}");
                    }
                    tracing::error!("Remote management: change {} wasn't confirmed and couldn't be rolled back yet: {e}", pending.id);
                    format!(
                        "warning: remote management: change {} wasn't confirmed, and the old Reticulum config couldn't be written back: {e}. It is tried again at the next start (and by the node meanwhile)",
                        pending.id
                    )
                }
                Err(e) => {
                    record("auto_rollback", Outcome::Failed, e.message());
                    tracing::error!(
                        "Remote management: change {} wasn't confirmed and couldn't be rolled back: {}",
                        pending.id,
                        e.message()
                    );
                    format!(
                        "warning: remote management: change {} wasn't confirmed and couldn't be rolled back: {}",
                        pending.id,
                        e.message()
                    )
                }
            }
        } else if sha.as_deref() == Some(pending.before_sha256.as_str()) {
            if pending.state == State::Restarting {
                record("reticulum", Outcome::Failed, "The node stopped before the change was saved");
                format!("Remote management: change {} wasn't saved before the node stopped: nothing to put back", pending.id)
            } else {
                let reason = format!("The node stopped while the change was {}; the old config was back already", pending.state.words());
                hold = true;
                record("auto_rollback", Outcome::RolledBack, &reason);
                format!("Remote management: change {} wasn't confirmed; the Reticulum config was back as it was already", pending.id)
            }
        } else {
            record(
                "auto_rollback",
                Outcome::Failed,
                "The Reticulum config was changed on this computer meanwhile, so it was left as it is",
            );
            format!(
                "warning: remote management: change {} wasn't confirmed, but the Reticulum config was changed since: left it as it is",
                pending.id
            )
        }
    };
    tracing::warn!("{line}");
    said.push(line);
    if hold {
        super::recent::ChangeLimits::hold_saved(paths, super::owner::HOLD_AFTER_ROLLBACK);
    }
    if !keep && let Err(e) = remove(paths) {
        said.push(format!("warning: remote management: {e}"));
    }
    said
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const BEFORE: &str = "[reticulum]\n  enable_transport = no\n";
    const AFTER: &str = "[reticulum]\n  enable_transport = yes\n";

    struct Node {
        dir: PathBuf,
        paths: Paths,
        config: PathBuf,
    }

    impl Drop for Node {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl Node {
        /// A data directory whose Reticulum config was `BEFORE`, kept in the
        /// history, and is now `AFTER`.
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rettui-manage-pending-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let paths = Paths::new(Some(dir.join("data"))).unwrap();
            let config = crate::reticulum::config_path(Some(dir.join("rns").to_str().unwrap()));
            std::fs::create_dir_all(config.parent().unwrap()).unwrap();
            std::fs::write(&config, BEFORE).unwrap();
            let node = Self { dir, paths, config };
            let history = node.history();
            let backup = history.before_saving(&node.config, Some(BEFORE), 1.0).unwrap();
            crate::reticulum::save(&node.config, AFTER).unwrap();
            history.record(&node.config, AFTER, "Remote change", 2.0).unwrap();
            save(&node.paths, &node.pending(State::Probation, backup)).unwrap();
            node
        }

        fn rns(&self) -> String {
            self.dir.join("rns").display().to_string()
        }

        fn history(&self) -> History {
            History::new(self.paths.reticulum_history.clone())
        }

        fn pending(&self, state: State, backup: Option<String>) -> Pending {
            Pending {
                v: 1,
                id: "0123456789abcdef".into(),
                actor: "alice".into(),
                config: self.config.clone(),
                backup,
                before_sha256: sha256_hex(BEFORE.as_bytes()),
                after_sha256: sha256_hex(AFTER.as_bytes()),
                state,
                applied_at: 100,
                confirm_seconds: 300,
                healthy_at: None,
                confirm_by: None,
                changes: vec![Change {
                    path: vec!["reticulum".into()],
                    key: "enable_transport".into(),
                    from: "no".into(),
                    to: "yes".into(),
                }],
                required: vec![],
            }
        }

        fn set_state(&self, state: State) {
            let mut pending = load(&self.paths).unwrap().unwrap();
            pending.state = state;
            save(&self.paths, &pending).unwrap();
        }

        fn recover(&self) -> Vec<String> {
            recover(&self.paths, Some(&self.rns()))
        }

        fn config_text(&self) -> String {
            std::fs::read_to_string(&self.config).unwrap()
        }

        fn audit(&self) -> Vec<(String, String, String)> {
            let audit = Audit::load(self.paths.root.join("manage/audit.jsonl"));
            audit.page(None, 100).0.iter().map(|e| (e.op.clone(), e.result.clone(), e.error.clone().unwrap_or_default())).collect()
        }
    }

    #[test]
    fn saved_privately_and_read_back() {
        let node = Node::new("saved");
        let pending = load(&node.paths).unwrap().unwrap();
        assert_eq!(pending.state, State::Probation);
        assert!(pending.backup.is_some());
        let text = std::fs::read_to_string(path(&node.paths)).unwrap();
        assert!(text.contains("\"state\": \"probation\"") && !text.contains("enable_transport = yes"), "no config text: {text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(path(&node.paths)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        remove(&node.paths).unwrap();
        assert_eq!(load(&node.paths), Ok(None));
        remove(&node.paths).unwrap();
        // Nothing pending: recovery does nothing.
        assert!(node.recover().is_empty());
        assert_eq!(node.config_text(), AFTER);
        assert!(node.audit().is_empty());
    }

    #[test]
    fn a_change_on_probation_or_restarting_is_rolled_back_at_the_start() {
        for state in [State::Restarting, State::Probation, State::RollingBack] {
            let node = Node::new(&format!("after-{}", state.as_str()));
            node.set_state(state);
            let said = node.recover();
            assert_eq!(node.config_text(), BEFORE, "{state:?}");
            assert!(said[0].contains("put the Reticulum config back"), "{said:?}");
            let audit = node.audit();
            assert_eq!((audit[0].0.as_str(), audit[0].1.as_str()), ("auto_rollback", "rolled_back"), "{audit:?}");
            assert!(audit[0].2.contains(&format!("while the change was {}", state.words())), "{audit:?}");
            assert_eq!(load(&node.paths), Ok(None));
            assert!(node.history().list(&node.config)[0].what.starts_with("Rolled back remote change 0123456789abcdef"));
            // The next change waits, as after a rollback the running node made.
            let limits =
                super::super::recent::ChangeLimits::load(&node.paths, Duration::from_secs(3600), super::super::owner::HOLD_AFTER_ROLLBACK);
            let left = limits.held_until.expect("held").saturating_duration_since(std::time::Instant::now());
            assert!(left > Duration::from_secs(850), "{state:?}: {left:?}");
            // Once only.
            assert!(node.recover().is_empty());
        }
    }

    #[test]
    fn the_backup_is_found_by_its_hash_wherever_it_is() {
        // No history id: any version with the same text.
        let node = Node::new("by-hash");
        let mut pending = load(&node.paths).unwrap().unwrap();
        pending.backup = Some("999".into());
        save(&node.paths, &pending).unwrap();
        node.recover();
        assert_eq!(node.config_text(), BEFORE);
        // No history at all: config.backup, if it holds the old text.
        let node = Node::new("config-backup");
        std::fs::remove_dir_all(&node.paths.reticulum_history).unwrap();
        node.recover();
        assert_eq!(node.config_text(), BEFORE);
        // Neither: left as it is, and said.
        let node = Node::new("no-backup");
        std::fs::remove_dir_all(&node.paths.reticulum_history).unwrap();
        std::fs::write(node.config.with_file_name("config.backup"), "something else\n").unwrap();
        let said = node.recover();
        assert_eq!(node.config_text(), AFTER);
        assert!(said[0].contains("couldn't be rolled back"), "{said:?}");
        let audit = node.audit();
        assert_eq!((audit[0].0.as_str(), audit[0].1.as_str()), ("auto_rollback", "failed"));
        assert!(audit[0].2.contains("No copy"), "{audit:?}");
        assert_eq!(load(&node.paths), Ok(None));
    }

    #[test]
    fn a_config_as_it_was_or_changed_since_is_left_alone() {
        // Stopped before the change was saved.
        let node = Node::new("before");
        node.set_state(State::Restarting);
        std::fs::write(&node.config, BEFORE).unwrap();
        node.recover();
        assert_eq!(node.config_text(), BEFORE);
        assert_eq!(node.audit()[0], ("reticulum".into(), "failed".into(), "The node stopped before the change was saved".into()));
        // On probation, but the old text is back already (as a rollback leaves it).
        let node = Node::new("back-already");
        std::fs::write(&node.config, BEFORE).unwrap();
        node.recover();
        assert_eq!(node.audit()[0].1, "rolled_back");
        assert!(node.audit()[0].2.contains("back already"));
        // Changed on this computer since.
        let node = Node::new("other");
        std::fs::write(&node.config, "[reticulum]\n  enable_transport = maybe\n").unwrap();
        let said = node.recover();
        assert_eq!(node.config_text(), "[reticulum]\n  enable_transport = maybe\n");
        assert!(said[0].contains("changed since"), "{said:?}");
        assert_eq!(node.audit()[0].1, "failed");
        assert!(node.audit()[0].2.contains("changed on this computer"));
        // Gone altogether.
        let node = Node::new("gone");
        std::fs::remove_file(&node.config).unwrap();
        node.recover();
        assert!(!node.config.exists());
        assert_eq!(node.audit()[0].1, "failed");
        assert_eq!(load(&node.paths), Ok(None));
    }

    #[test]
    fn a_rollback_that_cant_be_written_is_kept_for_the_next_start() {
        let node = Node::new("unwritable");
        let blocker = node.config.with_file_name(".config.rettui-tmp");
        std::fs::create_dir_all(blocker.join("x")).unwrap();
        let said = node.recover();
        assert_eq!(node.config_text(), AFTER);
        assert!(said[0].contains("couldn't be written back") && said[0].contains("tried again"), "{said:?}");
        // Kept, as being rolled back; nothing in the audit log yet, as it isn't over.
        assert_eq!(load(&node.paths).unwrap().unwrap().state, State::RollingBack);
        assert!(node.audit().is_empty());
        // The next start puts it back.
        std::fs::remove_dir_all(&blocker).unwrap();
        let said = node.recover();
        assert!(said[0].contains("put the Reticulum config back"), "{said:?}");
        assert_eq!(node.config_text(), BEFORE);
        assert_eq!(load(&node.paths), Ok(None));
        assert_eq!(node.audit()[0].1, "rolled_back");
    }

    #[cfg(unix)]
    #[test]
    fn a_config_that_is_a_link_is_left_alone() {
        let node = Node::new("link");
        let real = node.config.with_file_name("real-config");
        std::fs::rename(&node.config, &real).unwrap();
        std::os::unix::fs::symlink(&real, &node.config).unwrap();
        assert!(is_link(&node.config) && !is_link(&real));
        let said = node.recover();
        assert!(said[0].contains("is a link"), "{said:?}");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), AFTER);
        assert!(is_link(&node.config));
        assert_eq!(node.audit()[0].1, "failed");
        assert_eq!(load(&node.paths), Ok(None));
    }

    #[test]
    fn a_change_to_another_config_is_left_alone() {
        let node = Node::new("elsewhere");
        let said = recover(&node.paths, Some(node.dir.join("other-rns").to_str().unwrap()));
        assert_eq!(node.config_text(), AFTER);
        assert!(said[0].contains("uses") && said[0].contains("left it as it is"), "{said:?}");
        let audit = node.audit();
        assert_eq!((audit[0].0.as_str(), audit[0].1.as_str()), ("auto_rollback", "failed"));
        assert!(!audit[0].2.contains(node.dir.to_str().unwrap()), "no path in the audit log: {audit:?}");
        assert_eq!(load(&node.paths), Ok(None));
    }

    #[test]
    fn a_committed_change_is_only_tidied_up() {
        let node = Node::new("committed");
        node.set_state(State::Committed);
        assert!(node.recover().is_empty());
        assert_eq!(node.config_text(), AFTER);
        assert!(node.audit().is_empty());
        assert_eq!(load(&node.paths), Ok(None));
    }

    #[test]
    fn an_unreadable_file_is_moved_aside_and_nothing_else_is_done() {
        for junk in ["not json", "{}", "{\"v\":2}", &"x".repeat(70_000)] {
            let node = Node::new("unreadable");
            let pending = load(&node.paths).unwrap().unwrap();
            std::fs::write(path(&node.paths), junk).unwrap();
            assert!(load(&node.paths).is_err());
            let said = node.recover();
            assert!(said[0].contains("moved it to") && said[0].contains(".unreadable-"), "{said:?}");
            assert_eq!(node.config_text(), AFTER, "left as it is");
            assert!(!path(&node.paths).exists());
            let aside: Vec<_> = std::fs::read_dir(node.paths.root.join("manage"))
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with("pending.json.unreadable-"))
                .collect();
            assert_eq!(aside.len(), 1);
            // A well-formed file with odd values is refused too.
            for bad in [
                Pending { v: 2, ..pending.clone() },
                Pending { id: "XYZ".into(), ..pending.clone() },
                Pending { after_sha256: "00".into(), ..pending.clone() },
                Pending { actor: String::new(), ..pending.clone() },
            ] {
                save(&node.paths, &bad).unwrap();
                assert!(load(&node.paths).is_err(), "{bad:?}");
            }
        }
    }
}
