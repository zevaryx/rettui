//! The management jobs, run in the node's owner loop with the app: reading
//! its state, changing settings through the same path as the web UI (behind
//! the smaller list in [`super::fields`]), restarting Reticulum and
//! watching it come back, rolling a settings change back, and the audit log.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::audit::{Audit, Change, Outcome, Record};
use super::fields::{self, MANAGE_FIELDS, Refusal};
use super::{Job, Op, Reply, unix_now};
use crate::app::{App, NetState};
use crate::config::{self, FieldKind, Paths, Settings};
use crate::reticulum::redact;

/// One Reticulum restart per this long at most.
pub const RESTART_EVERY: Duration = Duration::from_secs(300);
/// How long a restart has to bring every interface that was online back.
pub const HEALTH_WITHIN: Duration = Duration::from_secs(90);
/// The most problems a restart reports, and the longest each.
const MAX_PROBLEMS: usize = 32;
const MAX_PROBLEM: usize = 120;
/// The most bytes of entries in an `/audit` answer: well under the 64 KiB a
/// website takes, with room for the rest of the answer.
const MAX_AUDIT_PAGE: usize = 48 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Health {
    Restarting,
    Up,
    Unhealthy,
}

impl Health {
    fn as_str(self) -> &'static str {
        match self {
            Health::Restarting => "restarting",
            Health::Up => "up",
            Health::Unhealthy => "unhealthy",
        }
    }
}

/// The last restart asked for, and how it went.
#[derive(Debug)]
struct Restart {
    id: String,
    actor: String,
    health: Health,
    /// Unix seconds, and the same moment for measuring.
    started: u64,
    since: Instant,
    /// The interfaces online before it, which must all be back.
    before: Vec<String>,
    problems: Vec<String>,
}

/// Runs management jobs against the app (see [`Owner::run`]).
#[derive(Debug)]
pub struct Owner {
    audit: Audit,
    restart: Option<Restart>,
    restart_every: Duration,
    health_within: Duration,
}

/// A new change id: 16 hex characters.
fn new_id() -> String {
    hex::encode(rand::random::<[u8; 8]>())
}

fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// `text` for the node's log: control and invisible characters (an escape,
/// a line break, a NUL, a bidi override) written out as `\u{..}`, so what a
/// request sent can't split a line, colour the terminal or reorder the text.
fn printable(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() || crate::stats::is_invisible(c) {
            shown.extend(c.escape_unicode());
        } else {
            shown.push(c);
        }
    }
    shown
}

impl Owner {
    /// The owner of the node at `paths`, with its audit log (in the data
    /// directory, `manage/audit.jsonl`).
    pub fn new(paths: &Paths) -> Self {
        Self::with_timing(paths, RESTART_EVERY, HEALTH_WITHIN)
    }

    fn with_timing(paths: &Paths, restart_every: Duration, health_within: Duration) -> Self {
        Self { audit: Audit::load(paths.root.join("manage").join("audit.jsonl")), restart: None, restart_every, health_within }
    }

    /// Run `job` and answer it. A request whose answer is no longer awaited
    /// (it timed out) has still been carried out.
    pub fn run(&mut self, app: &mut App, job: Job) {
        let reply = match job.op {
            Op::State => self.state(app),
            Op::Settings { revision, changes } => {
                let pairs: Vec<(&str, &str)> = changes.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                self.settings(app, &job.actor, &revision, &pairs)
            }
            Op::Restart => self.restart(app, &job.actor),
            Op::Rollback { id } => self.rollback(app, &job.actor, &id),
            Op::Audit { before, limit } => self.audit_page(before, limit),
            Op::AuthFailures { count } => {
                app.log(format!(
                    "Remote management: {count} requests with a bad signature; after 20 within a minute, requests without a valid signature are refused for 60 seconds (logged again in 10 minutes at most while they go on)"
                ));
                if let Err(e) = self.audit.auth_failures(&new_id(), count as u64, unix_now()) {
                    tracing::warn!("Remote management: {e}");
                    app.log(printable(&e));
                }
                Reply::new(StatusCode::OK, json!({}))
            }
        };
        let _ = job.reply.send(reply);
    }

    /// Watch a restart in progress: `up` once the network runs and every
    /// interface that was online before it is again, `unhealthy` if
    /// Reticulum fails or that hasn't happened in [`HEALTH_WITHIN`].
    pub fn on_tick(&mut self, app: &App) {
        let Some(restart) = self.restart.as_mut().filter(|r| r.health == Health::Restarting) else { return };
        let online: HashSet<&str> = app.interfaces.iter().filter(|i| i.online).map(|i| i.name.as_str()).collect();
        let missing: Vec<String> = restart
            .before
            .iter()
            .filter(|name| !online.contains(name.as_str()))
            .map(|name| format!("{} is offline", cut(name, MAX_PROBLEM)))
            .collect();
        let mut problems = match &app.net_state {
            NetState::Online if missing.is_empty() => {
                restart.health = Health::Up;
                tracing::info!("Remote management: Reticulum is back after the restart {}", restart.id);
                return;
            }
            NetState::Failed(e) => vec![format!("Reticulum didn't start: {}", cut(e, MAX_PROBLEM))],
            _ if restart.since.elapsed() < self.health_within => return,
            NetState::Starting => vec!["Reticulum is still starting".to_string()],
            NetState::Online => vec![],
        };
        problems.extend(missing);
        problems.truncate(MAX_PROBLEMS);
        restart.health = Health::Unhealthy;
        restart.problems = problems;
        let error = format!("Reticulum isn't back as it was: {}", restart.problems.join("; "));
        tracing::warn!("Remote management: {}", printable(&error));
        let (id, actor) = (restart.id.clone(), restart.actor.clone());
        let record = Record { id: &id, actor: &actor, op: "restart", changes: vec![], outcome: Outcome::Failed, error: Some(&error) };
        if let Err(e) = self.audit.record(record, unix_now()) {
            tracing::warn!("Remote management: {e}");
        }
    }

    fn restarting(&self) -> bool {
        self.restart.as_ref().is_some_and(|r| r.health == Health::Restarting)
    }

    /// When the next restart may be asked for (unix seconds), if not now.
    fn restart_allowed_at(&self) -> Option<u64> {
        let last = self.restart.as_ref()?;
        (last.since.elapsed() < self.restart_every).then(|| last.started + self.restart_every.as_secs())
    }

    /// Add `record` to the audit log and the node's log (where what the
    /// request sent, echoed in a key or an error, is made printable first).
    fn record(&mut self, app: &mut App, record: Record<'_>) {
        let keys: Vec<String> = record.changes.iter().map(|c| printable(&c.key)).collect();
        let what = match (record.outcome, record.error) {
            (Outcome::Ok, _) => "done".to_string(),
            (_, Some(error)) => format!("refused or failed: {}", printable(error)),
            (_, None) => "refused or failed".to_string(),
        };
        let keys = if keys.is_empty() { String::new() } else { format!(" ({})", keys.join(", ")) };
        app.log(format!("Remote management: {} by {}{keys}, {what}", record.op, printable(record.actor)));
        if let Err(e) = self.audit.record(record, unix_now()) {
            tracing::warn!("Remote management: {e}");
            app.log(printable(&e));
        }
    }

    /// Refuse a write: record it, and answer `status` with `code`.
    #[allow(clippy::too_many_arguments)] // Each is a different part of the answer and the record.
    fn refuse(
        &mut self,
        app: &mut App,
        actor: &str,
        op: &'static str,
        changes: Vec<Change>,
        status: StatusCode,
        code: &str,
        message: &str,
        outcome: Outcome,
    ) -> Reply {
        self.record(app, Record { id: &new_id(), actor, op, changes, outcome, error: Some(message) });
        Reply::error(status, code, message)
    }

    fn state(&self, app: &App) -> Reply {
        let saved = match app.saved_settings() {
            Ok(saved) => saved,
            Err(e) => return Reply::error(StatusCode::INTERNAL_SERVER_ERROR, "failed", e),
        };
        let settings: Vec<Value> = MANAGE_FIELDS
            .iter()
            .filter_map(|(key, access)| {
                let field = config::field(key)?;
                let mut entry = json!({
                    "key": key,
                    "label": field.label,
                    "help": field.help,
                    "kind": fields::kind_name(field.kind),
                    "access": access.as_str(),
                    "value": saved.field_value(key),
                });
                if let (FieldKind::Choice(choices), Some(map)) = (field.kind, entry.as_object_mut()) {
                    map.insert("choices".into(), json!(choices));
                }
                Some(entry)
            })
            .collect();
        let network = match app.net_state {
            NetState::Starting => "starting",
            NetState::Online => "running",
            NetState::Failed(_) => "failed",
        };
        let restart =
            self.restart.as_ref().map(|r| json!({ "id": r.id, "state": r.health.as_str(), "started": r.started, "problems": r.problems }));
        // The redacted Reticulum config; `null` when it can't be read. The
        // read and the parse run here in the owner loop, and are capped (1
        // MiB read, a small number of lines, sections and keys).
        let shared = app.uses_external_shared_instance();
        let reticulum = redact::view_of_file(&crate::reticulum::config_path(app.settings.rns_config.as_deref()), shared);
        Reply::new(
            StatusCode::OK,
            json!({
                "v": 1,
                "node_time": unix_now(),
                "version": crate::VERSION,
                "settings_revision": fields::revision(&saved),
                "settings": settings,
                "network": {
                    "state": network,
                    "shared_instance": shared,
                    "interfaces_total": app.interfaces.len(),
                    "interfaces_online": app.interfaces.iter().filter(|i| i.online).count(),
                },
                "reticulum": reticulum,
                "restart": restart,
                "restart_allowed_at": self.restart_allowed_at(),
                "pending": null,
            }),
        )
    }

    /// The changes asked for, as the audit log keeps a refused one.
    fn asked(before: &Settings, changes: &[(&str, &str)]) -> Vec<Change> {
        changes
            .iter()
            .map(|(key, value)| Change {
                key: cut(key, 64),
                from: fields::access(key).map(|_| before.field_value(key)).unwrap_or_default(),
                to: value.to_string(),
            })
            .collect()
    }

    /// Apply `changes` (already checked), and record them as `op` with `id`.
    fn apply(&mut self, app: &mut App, actor: &str, op: &'static str, applied: Vec<Change>, changes: &[(&str, &str)]) -> Reply {
        let id = new_id();
        match app.update_settings_from_web(changes) {
            Ok(notes) => {
                let revision = fields::revision(&app.settings_file);
                let body = json!({ "id": id, "applied": applied, "notes": notes, "settings_revision": revision });
                self.record(app, Record { id: &id, actor, op, changes: applied, outcome: Outcome::Ok, error: None });
                Reply::new(StatusCode::OK, body)
            }
            Err(e) => {
                self.record(app, Record { id: &id, actor, op, changes: applied, outcome: Outcome::Failed, error: Some(&e) });
                Reply::error(StatusCode::INTERNAL_SERVER_ERROR, "failed", e)
            }
        }
    }

    /// A refusal from [`fields::check`] as an answer.
    fn refusal(&mut self, app: &mut App, actor: &str, op: &'static str, asked: Vec<Change>, refusal: Refusal) -> Reply {
        let (status, code, key, message) = match refusal {
            Refusal::NotAllowed { key, message } => (StatusCode::FORBIDDEN, "not_allowed", Some(key), message),
            Refusal::Invalid { key, message } => (StatusCode::BAD_REQUEST, "invalid_value", Some(key), message),
            Refusal::NoChange => (StatusCode::BAD_REQUEST, "no_change", None, "Every value is what it is already".to_string()),
        };
        let reply = self.refuse(app, actor, op, asked, status, code, &message, Outcome::Refused);
        match key {
            Some(key) => reply.with("key", key),
            None => reply,
        }
    }

    fn settings(&mut self, app: &mut App, actor: &str, revision: &str, changes: &[(&str, &str)]) -> Reply {
        let before = match app.saved_settings() {
            Ok(before) => before,
            Err(e) => return self.refuse(app, actor, "settings", vec![], StatusCode::INTERNAL_SERVER_ERROR, "failed", &e, Outcome::Failed),
        };
        let asked = Self::asked(&before, changes);
        let current = fields::revision(&before);
        if revision != current {
            let message = "The settings changed since they were read: load them again";
            let reply = self.refuse(app, actor, "settings", asked, StatusCode::CONFLICT, "stale_revision", message, Outcome::Refused);
            return reply.with("settings_revision", current);
        }
        if self.restarting() {
            let message = "Reticulum is restarting: try again once it's back";
            return self.refuse(app, actor, "settings", asked, StatusCode::CONFLICT, "busy", message, Outcome::Refused);
        }
        match fields::check(&before, changes) {
            Ok(applied) => self.apply(app, actor, "settings", applied, changes),
            Err(refusal) => self.refusal(app, actor, "settings", asked, refusal),
        }
    }

    fn rollback(&mut self, app: &mut App, actor: &str, id: &str) -> Reply {
        let Some(entry) = self.audit.applied_change(id).cloned() else {
            let message = if self.audit.has(id) {
                "That change wasn't a settings change that was applied, so there's nothing to roll back"
            } else {
                "No change with that id is in the audit log"
            };
            return self.refuse(app, actor, "rollback", vec![], StatusCode::NOT_FOUND, "unknown_change", message, Outcome::Refused);
        };
        let before = match app.saved_settings() {
            Ok(before) => before,
            Err(e) => return self.refuse(app, actor, "rollback", vec![], StatusCode::INTERNAL_SERVER_ERROR, "failed", &e, Outcome::Failed),
        };
        let inverse: Vec<(&str, &str)> = entry.changes.iter().map(|c| (c.key.as_str(), c.from.as_str())).collect();
        let asked = Self::asked(&before, &inverse);
        if self.restarting() {
            let message = "Reticulum is restarting: try again once it's back";
            return self.refuse(app, actor, "rollback", asked, StatusCode::CONFLICT, "busy", message, Outcome::Refused);
        }
        // Only what is still as that change left it can be put back.
        let changed: Vec<String> = entry.changes.iter().filter(|c| before.field_value(&c.key) != c.to).map(|c| c.key.clone()).collect();
        if !changed.is_empty() {
            let message = format!("Changed again since then: {}", changed.join(", "));
            let reply = self.refuse(app, actor, "rollback", asked, StatusCode::CONFLICT, "changed_since", &message, Outcome::Refused);
            return reply.with("keys", changed);
        }
        // Through the same list as any change, so a service that change
        // turned off isn't turned back on.
        match fields::check(&before, &inverse) {
            Ok(applied) => {
                let mut reply = self.apply(app, actor, "rollback", applied, &inverse);
                // The rollback's answer has no notes (they were the change's).
                if let Some(map) = reply.body.as_object_mut() {
                    map.remove("notes");
                }
                reply
            }
            Err(refusal) => self.refusal(app, actor, "rollback", asked, refusal),
        }
    }

    fn restart(&mut self, app: &mut App, actor: &str) -> Reply {
        if self.restarting() {
            let message = "Reticulum is restarting already";
            return self.refuse(app, actor, "restart", vec![], StatusCode::CONFLICT, "restarting", message, Outcome::Refused);
        }
        if let Some(at) = self.restart_allowed_at() {
            let wait = at.saturating_sub(unix_now()).max(1);
            let message =
                format!("Reticulum can be restarted once every {} minutes: try again in {wait} seconds", self.restart_every.as_secs() / 60);
            let reply =
                self.refuse(app, actor, "restart", vec![], StatusCode::TOO_MANY_REQUESTS, "rate_limited", &message, Outcome::Refused);
            return reply.with("retry_after", wait);
        }
        let id = new_id();
        let before = app.interfaces.iter().filter(|i| i.online).map(|i| i.name.clone()).collect();
        app.request_rns_restart();
        self.restart = Some(Restart {
            id: id.clone(),
            actor: actor.to_string(),
            health: Health::Restarting,
            started: unix_now(),
            since: Instant::now(),
            before,
            problems: Vec::new(),
        });
        self.record(app, Record { id: &id, actor, op: "restart", changes: vec![], outcome: Outcome::Ok, error: None });
        Reply::new(StatusCode::ACCEPTED, json!({ "id": id, "state": "restarting" }))
    }

    fn audit_page(&self, before: Option<u64>, limit: usize) -> Reply {
        let (entries, more) = self.audit.page_within(before, limit, MAX_AUDIT_PAGE);
        Reply::new(StatusCode::OK, json!({ "entries": entries, "more": more }))
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::*;
    use crate::net::InterfaceInfo;
    use crate::store::Store;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-manage-owner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    struct Node {
        app: App,
        owner: Owner,
        dir: std::path::PathBuf,
    }

    impl Node {
        fn new(name: &str, settings: Settings) -> Self {
            let dir = test_dir(name);
            let (app, _commands) = crate::app::test_app_with_net(&dir, settings, Store::default());
            let owner = Owner::with_timing(&app.paths, Duration::from_secs(300), Duration::from_millis(200));
            Self { app, owner, dir }
        }

        fn ask(&mut self, op: Op) -> Reply {
            let (reply, answer) = oneshot::channel();
            self.owner.run(&mut self.app, Job { actor: "alice".into(), op, reply });
            answer.blocking_recv().expect("every job is answered")
        }

        fn state(&mut self) -> Value {
            let reply = self.ask(Op::State);
            assert_eq!(reply.status, StatusCode::OK);
            reply.body
        }

        fn revision(&mut self) -> String {
            self.state()["settings_revision"].as_str().unwrap().to_string()
        }

        fn change(&mut self, changes: &[(&str, &str)]) -> Reply {
            let revision = self.revision();
            let changes = changes.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            self.ask(Op::Settings { revision, changes })
        }

        fn audit(&mut self) -> Vec<Value> {
            self.ask(Op::Audit { before: None, limit: 100 }).body["entries"].as_array().unwrap().clone()
        }
    }

    impl Drop for Node {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn the_state_lists_the_managed_settings_and_the_network() {
        let mut node = Node::new("state", Settings { pn_enabled: true, ..Settings::default() });
        node.app.interfaces = vec![
            InterfaceInfo { name: "A".into(), online: true, ..Default::default() },
            InterfaceInfo { name: "B".into(), online: false, ..Default::default() },
        ];
        let state = node.state();
        assert_eq!(state["v"], 1);
        assert_eq!(state["version"], crate::VERSION);
        assert!(state["node_time"].as_u64().unwrap() > 1_700_000_000);
        assert_eq!(state["pending"], Value::Null);
        assert_eq!(state["reticulum"], Value::Null, "no config file to read here");
        assert_eq!(state["restart"], Value::Null);
        assert_eq!(state["restart_allowed_at"], Value::Null);
        assert_eq!(
            state["network"],
            json!({ "state": "starting", "shared_instance": false, "interfaces_total": 2, "interfaces_online": 1 })
        );
        let settings = state["settings"].as_array().unwrap();
        assert_eq!(settings.len(), MANAGE_FIELDS.len());
        let pn = settings.iter().find(|s| s["key"] == "pn_enabled").unwrap();
        assert_eq!(
            (pn["kind"].as_str(), pn["access"].as_str(), pn["value"].as_str()),
            (Some("toggle"), Some("turn_off_only"), Some("true"))
        );
        assert!(pn["label"].is_string() && pn["help"].is_string() && pn.get("choices").is_none());
        let level = settings.iter().find(|s| s["key"] == "log_level").unwrap();
        assert_eq!(level["kind"], "choice");
        assert_eq!(level["choices"], json!(crate::logging::LEVELS));
        let name = settings.iter().find(|s| s["key"] == "pn_name").unwrap();
        assert_eq!((name["kind"].as_str(), name["access"].as_str(), name["value"].as_str()), (Some("optional"), Some("change"), Some("")));
        // Toggles are the strings "true" and "false", which a change takes back as they are.
        for toggle in settings.iter().filter(|s| s["kind"] == "toggle") {
            let value = toggle["value"].as_str().unwrap();
            assert!(value == "true" || value == "false", "{toggle}");
            let mut saved = node.app.saved_settings().unwrap();
            for text in ["true", "false"] {
                saved.set_field(toggle["key"].as_str().unwrap(), text).unwrap();
                assert_eq!(saved.field_value(toggle["key"].as_str().unwrap()), text);
            }
        }
        // Nothing outside the list.
        assert!(!settings.iter().any(|s| s["key"] == "rns_config" || s["key"] == "node_dir"));
        node.app.net_state = NetState::Online;
        assert_eq!(node.state()["network"]["state"], "running");
        node.app.net_state = NetState::Failed("no".into());
        assert_eq!(node.state()["network"]["state"], "failed");
    }

    #[test]
    fn the_state_carries_the_redacted_reticulum_config() {
        let mut node = Node::new("reticulum", Settings::default());
        let dir = node.dir.clone();
        let rns = dir.join("rns");
        std::fs::create_dir_all(&rns).unwrap();
        let text = "[reticulum]\n enable_transport = yes\n[interfaces]\n [[TCP]]\n  type = TCPServerInterface\n  listen_port = 4242\n  passphrase = fake-secret-passphrase\n  network_name = fake-secret-network\n";
        std::fs::write(rns.join("config"), text).unwrap();
        node.app.settings.rns_config = Some(rns.display().to_string());
        node.app.interfaces = vec![InterfaceInfo { name: "Shared Instance[37428]".into(), online: true, ..Default::default() }];
        let state = node.state();
        let shown = state["reticulum"].to_string();
        assert!(!shown.contains("fake-secret"), "{shown}");
        assert!(!shown.contains(dir.to_str().unwrap()), "{shown}");
        assert_eq!(state["reticulum"]["shared_instance"], true);
        assert_eq!(state["reticulum"]["revision"].as_str().unwrap().len(), 32);
        let tcp = &state["reticulum"]["sections"].as_array().unwrap().iter().find(|s| s["path"] == json!(["interfaces", "TCP"])).unwrap();
        assert!(tcp["keys"].as_array().unwrap().contains(&json!({ "key": "passphrase", "value": null, "set": true })));
        assert!(tcp["keys"].as_array().unwrap().contains(&json!({ "key": "listen_port", "value": "4242", "set": true })));
        // The same config gives the same revision; the audit log has nothing from it.
        assert_eq!(node.state()["reticulum"]["revision"], state["reticulum"]["revision"]);
        assert!(node.audit().is_empty());
    }

    #[test]
    fn a_change_is_applied_saved_audited_and_moves_the_revision() {
        let mut node = Node::new("change", Settings::default());
        let first = node.revision();
        let reply = node.change(&[("pn_name", "Hilltop"), ("pn_max_peers", "9"), ("display_name", "rettui user")]);
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
        let id = reply.body["id"].as_str().unwrap().to_string();
        assert_eq!(id.len(), 16);
        let applied = reply.body["applied"].as_array().unwrap();
        assert_eq!(applied.len(), 2, "the display name was the same: {applied:?}");
        assert_eq!(applied[0], json!({ "key": "pn_name", "from": "", "to": "Hilltop" }));
        assert_ne!(reply.body["settings_revision"], json!(first));
        assert_eq!(reply.body["settings_revision"], json!(node.revision()));
        assert!(reply.body["notes"].is_array());
        // Saved and running.
        assert_eq!(node.app.saved_settings().unwrap().pn_name.as_deref(), Some("Hilltop"));
        assert_eq!(node.app.settings.pn_max_peers, 9);
        let audit = node.audit();
        assert_eq!(audit.len(), 1);
        assert_eq!(
            (audit[0]["op"].as_str(), audit[0]["result"].as_str(), audit[0]["actor"].as_str()),
            (Some("settings"), Some("ok"), Some("alice"))
        );
        assert_eq!(audit[0]["id"], json!(id));
        assert_eq!(audit[0]["changes"][0], json!({ "key": "pn_name", "from": "", "to": "Hilltop" }));
        assert!(
            node.app.log.iter().any(|l| l.contains("Remote management: settings by alice (pn_name, pn_max_peers), done")),
            "{:?}",
            node.app.log
        );
    }

    #[test]
    fn a_stale_revision_or_a_change_made_meanwhile_is_refused() {
        let mut node = Node::new("stale", Settings::default());
        let revision = node.revision();
        let changes = |name: &str| vec![("pn_name".to_string(), name.to_string())];
        let first = node.ask(Op::Settings { revision: revision.clone(), changes: changes("One") });
        assert_eq!(first.status, StatusCode::OK);
        // A second change made from the same revision, as two people at once.
        let second = node.ask(Op::Settings { revision: revision.clone(), changes: changes("Two") });
        assert_eq!(second.status, StatusCode::CONFLICT);
        assert_eq!(second.body["error"], "stale_revision");
        assert_eq!(second.body["settings_revision"], first.body["settings_revision"]);
        assert_eq!(node.app.saved_settings().unwrap().pn_name.as_deref(), Some("One"));
        // A change in the terminal moves the revision too.
        node.app.update_settings(&[("pn_name", "Three")]).unwrap();
        let third = node.ask(Op::Settings { revision: first.body["settings_revision"].as_str().unwrap().into(), changes: changes("Four") });
        assert_eq!(third.body["error"], "stale_revision");
        let audit = node.audit();
        assert_eq!(audit[0]["result"], "refused");
        assert_eq!(audit[0]["changes"][0], json!({ "key": "pn_name", "from": "Three", "to": "Four" }));
    }

    #[test]
    fn refusals_say_why_and_are_audited() {
        let mut node = Node::new("refusals", Settings { hub_enabled: true, ..Settings::default() });
        let reply = node.change(&[("rns_config", "/tmp/x")]);
        assert_eq!(
            (reply.status, reply.body["error"].as_str(), reply.body["key"].as_str()),
            (StatusCode::FORBIDDEN, Some("not_allowed"), Some("rns_config"))
        );
        let reply = node.change(&[("pn_enabled", "true")]);
        assert_eq!((reply.status, reply.body["error"].as_str()), (StatusCode::FORBIDDEN, Some("not_allowed")));
        assert!(reply.body["message"].as_str().unwrap().contains("only be turned on in the terminal"));
        let reply = node.change(&[("pn_stamp_cost", "nope")]);
        assert_eq!(
            (reply.status, reply.body["error"].as_str(), reply.body["key"].as_str()),
            (StatusCode::BAD_REQUEST, Some("invalid_value"), Some("pn_stamp_cost"))
        );
        let reply = node.change(&[("hub_enabled", "true")]);
        assert_eq!(reply.body["error"], "no_change");
        assert!(!node.app.saved_settings().unwrap().pn_enabled);
        // Turning a service off works.
        assert_eq!(node.change(&[("hub_enabled", "false")]).status, StatusCode::OK);
        assert!(!node.app.settings.hub_enabled);
        let audit = node.audit();
        assert_eq!(audit.len(), 5);
        assert_eq!(audit.iter().filter(|e| e["result"] == "refused").count(), 4);
        assert!(audit.iter().all(|e| e["error"].is_null() == (e["result"] == "ok")));
    }

    #[test]
    fn a_rollback_puts_the_values_back_but_never_turns_a_service_on() {
        let mut node = Node::new("rollback", Settings { pn_enabled: true, ..Settings::default() });
        let change = node.change(&[("pn_name", "Hilltop"), ("pn_max_peers", "9")]);
        let id = change.body["id"].as_str().unwrap().to_string();
        let reply = node.ask(Op::Rollback { id: id.clone() });
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
        let new_id = reply.body["id"].as_str().unwrap();
        assert_ne!(new_id, id);
        assert_eq!(reply.body["applied"][0], json!({ "key": "pn_name", "from": "Hilltop", "to": "" }));
        assert!(reply.body.get("notes").is_none());
        let saved = node.app.saved_settings().unwrap();
        assert_eq!((saved.pn_name, saved.pn_max_peers), (None, Settings::default().pn_max_peers));
        assert_eq!(reply.body["settings_revision"], json!(node.revision()));
        // Rolled back already: what it changed has changed since.
        let again = node.ask(Op::Rollback { id: id.clone() });
        assert_eq!((again.status, again.body["error"].as_str()), (StatusCode::CONFLICT, Some("changed_since")));
        assert_eq!(again.body["keys"], json!(["pn_name", "pn_max_peers"]));
        // A rollback can be rolled back.
        assert_eq!(node.ask(Op::Rollback { id: new_id.to_string() }).status, StatusCode::OK);
        assert_eq!(node.app.saved_settings().unwrap().pn_name.as_deref(), Some("Hilltop"));

        // Turning the propagation node off can't be rolled back remotely.
        let off = node.change(&[("pn_enabled", "false")]);
        assert_eq!(off.status, StatusCode::OK);
        let reply = node.ask(Op::Rollback { id: off.body["id"].as_str().unwrap().into() });
        assert_eq!(
            (reply.status, reply.body["error"].as_str(), reply.body["key"].as_str()),
            (StatusCode::FORBIDDEN, Some("not_allowed"), Some("pn_enabled"))
        );
        assert!(!node.app.saved_settings().unwrap().pn_enabled);

        // An id nobody gave, or one that wasn't an applied settings change.
        let reply = node.ask(Op::Rollback { id: "0123456789abcdef".into() });
        assert_eq!((reply.status, reply.body["error"].as_str()), (StatusCode::NOT_FOUND, Some("unknown_change")));
        let refused = node.change(&[("pn_enabled", "true")]);
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        let refused_id = node.audit()[0]["id"].as_str().unwrap().to_string();
        let reply = node.ask(Op::Rollback { id: refused_id });
        assert_eq!(reply.body["error"], "unknown_change");
        assert!(reply.body["message"].as_str().unwrap().contains("nothing to roll back"));
        // A change made by hand in between stops it.
        let change = node.change(&[("hub_name", "Chat")]);
        node.app.update_settings(&[("hub_name", "By hand")]).unwrap();
        let reply = node.ask(Op::Rollback { id: change.body["id"].as_str().unwrap().into() });
        assert_eq!(reply.body["keys"], json!(["hub_name"]));
    }

    #[test]
    fn a_restart_is_rate_limited_and_watched_until_it_is_back() {
        let mut node = Node::new("restart", Settings::default());
        node.app.net_state = NetState::Online;
        node.app.interfaces = vec![
            InterfaceInfo { name: "Up".into(), online: true, ..Default::default() },
            InterfaceInfo { name: "Down".into(), online: false, ..Default::default() },
        ];
        let reply = node.ask(Op::Restart);
        assert_eq!(reply.status, StatusCode::ACCEPTED);
        assert_eq!(reply.body["state"], "restarting");
        assert!(node.app.take_rns_restart(), "the restart is asked for");
        let state = node.state();
        assert_eq!(state["restart"]["state"], "restarting");
        assert_eq!(state["restart"]["id"], reply.body["id"]);
        let allowed = state["restart_allowed_at"].as_u64().unwrap();
        assert_eq!(allowed, state["restart"]["started"].as_u64().unwrap() + 300);
        // One at a time; and settings wait for it.
        assert_eq!(node.ask(Op::Restart).body["error"], "restarting");
        assert_eq!(node.change(&[("pn_name", "X")]).body["error"], "busy");
        // Back once the network runs with the interface that was up.
        node.app.net_state = NetState::Online;
        node.owner.on_tick(&node.app);
        assert_eq!(node.state()["restart"]["state"], "restarting", "the interfaces aren't back");
        node.app.interfaces = vec![InterfaceInfo { name: "Up".into(), online: true, ..Default::default() }];
        node.owner.on_tick(&node.app);
        assert_eq!(node.state()["restart"]["state"], "up");
        // The next one waits five minutes.
        let limited = node.ask(Op::Restart);
        assert_eq!((limited.status, limited.body["error"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")));
        assert!(limited.body["retry_after"].as_u64().unwrap() > 200);
        let audit = node.audit();
        assert_eq!(audit.iter().filter(|e| e["op"] == "restart" && e["result"] == "ok").count(), 1);
    }

    #[test]
    fn a_restart_that_doesnt_come_back_is_unhealthy() {
        let mut node = Node::new("unhealthy", Settings::default());
        node.owner.restart_every = Duration::ZERO;
        node.app.interfaces = vec![InterfaceInfo { name: "Uplink".into(), online: true, ..Default::default() }];
        let first = node.ask(Op::Restart);
        node.app.take_rns_restart();
        node.app.net_state = NetState::Online;
        node.owner.on_tick(&node.app);
        assert_eq!(node.state()["restart"]["state"], "restarting", "it has 90 seconds (here 200 ms)");
        std::thread::sleep(Duration::from_millis(250));
        node.owner.on_tick(&node.app);
        let state = node.state();
        assert_eq!(state["restart"]["state"], "unhealthy");
        assert_eq!(state["restart"]["problems"], json!(["Uplink is offline"]));
        let audit = node.audit();
        assert_eq!((audit[0]["op"].as_str(), audit[0]["result"].as_str()), (Some("restart"), Some("failed")));
        assert_eq!(audit[0]["id"], first.body["id"]);
        // Reticulum that fails is unhealthy at once; one still starting after the wait says so.
        assert_eq!(node.ask(Op::Restart).status, StatusCode::ACCEPTED);
        node.app.take_rns_restart();
        node.app.net_state = NetState::Failed("bad config".into());
        node.owner.on_tick(&node.app);
        assert_eq!(node.state()["restart"]["problems"][0], "Reticulum didn't start: bad config");
        assert_eq!(node.ask(Op::Restart).status, StatusCode::ACCEPTED);
        node.app.take_rns_restart();
        std::thread::sleep(Duration::from_millis(250));
        node.owner.on_tick(&node.app);
        assert_eq!(node.state()["restart"]["problems"][0], "Reticulum is still starting");
        // Nothing to watch: nothing happens.
        node.owner.on_tick(&node.app);
    }

    #[test]
    fn failed_signatures_are_audited_without_an_actor() {
        let mut node = Node::new("failures", Settings::default());
        let reply = node.ask(Op::AuthFailures { count: 20 });
        assert_eq!(reply.status, StatusCode::OK);
        let audit = node.audit();
        assert_eq!(
            (audit[0]["op"].as_str(), audit[0]["actor"].as_str(), audit[0]["result"].as_str()),
            (Some("auth_failures"), Some("-"), Some("refused"))
        );
        assert!(audit[0]["error"].as_str().unwrap().starts_with("20 requests"));
        assert_eq!(audit[0]["count"], 20);
        assert!(node.app.log.iter().any(|l| l.contains("20 requests with a bad signature; after 20 within a minute")));
        // The next burst counts in the same entry.
        node.ask(Op::AuthFailures { count: 20 });
        let audit = node.audit();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0]["count"], 40);
        assert!(audit[0]["error"].as_str().unwrap().starts_with("40 requests"));
    }

    #[test]
    fn what_a_request_sent_is_made_printable_in_the_log() {
        let mut node = Node::new("printable", Settings::default());
        let nasty = "a\u{1b}[31mred\nfake line\0\u{202e}desrever";
        let record = Record {
            id: "0123456789abcdef",
            actor: "alice\u{1b}",
            op: "settings",
            changes: vec![Change { key: nasty.into(), from: String::new(), to: nasty.into() }],
            outcome: Outcome::Refused,
            error: Some(nasty),
        };
        node.owner.record(&mut node.app, record);
        let line = node.app.log.iter().find(|l| l.contains("Remote management: settings")).unwrap().clone();
        for bad in ['\u{1b}', '\n', '\0', '\u{202e}'] {
            assert!(!line.contains(bad), "{bad:?} in {line:?}");
        }
        assert!(line.contains(r"a\u{1b}[31mred\u{a}fake line\u{0}\u{202e}desrever"), "{line}");
        assert!(line.contains(r"by alice\u{1b} ("), "{line}");
        // The audit log keeps the text as it was (JSON escapes it).
        assert_eq!(node.audit()[0]["error"], nasty);
        // Ordinary text is left as it is.
        assert_eq!(printable("Hilltop: at most 64 characters, ünïcode"), "Hilltop: at most 64 characters, ünïcode");
    }

    #[test]
    fn settings_that_cant_be_read_or_saved_fail_plainly() {
        let mut node = Node::new("unreadable", Settings::default());
        let revision = node.revision();
        // A folder where settings.json was: it can be neither read nor replaced.
        let path = node.app.paths.settings.clone();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir_all(path.join("x")).unwrap();
        let reply = node.ask(Op::State);
        assert_eq!((reply.status, reply.body["error"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("failed")));
        let reply = node.ask(Op::Settings { revision, changes: vec![("pn_name".into(), "X".into())] });
        assert_eq!(reply.body["error"], "failed");
        assert_eq!(node.ask(Op::Rollback { id: "0123456789abcdef".into() }).body["error"], "unknown_change");
        assert_eq!(node.audit()[1]["result"], "failed");
    }
}
