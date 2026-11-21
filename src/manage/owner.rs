//! The management jobs, run in the node's owner loop with the app: reading
//! its state, changing settings through the same path as the web UI (behind
//! the smaller list in [`super::fields`]), restarting Reticulum and
//! watching it come back, rolling a settings change back, and the audit log.
//!
//! A Reticulum config change ([`super::rnsedit`]) is saved, and Reticulum
//! restarted with it, on probation: once Reticulum is back with every
//! interface that was online (but those the change turns off), the website
//! has `confirm_seconds` to confirm it. A change that isn't confirmed in time,
//! or with which Reticulum isn't back within [`HEALTH_WITHIN`], is put back
//! by the node itself, from the copy of the old text the config history
//! keeps (see [`super::pending`]). If it can't be written back, the node
//! tries again on every tick.
//!
//! At most [`APPLIES_PER_HOUR`] changes an hour are applied, and none for
//! [`HOLD_AFTER_ROLLBACK`] after one was put back by itself. Deadlines are
//! measured on the monotonic clock; unix times are only shown.
//!
//! Nothing Reticulum or the system says goes into an answer or the audit
//! log: they get a fixed sentence, and the node's log the detail.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::audit::{Audit, Change, Outcome, Record};
use super::fields::{self, MANAGE_FIELDS, Refusal};
use super::pending::{self, Pending, RestoreError, State};
use super::rnsedit::{self, RnsChange, RnsRefusal};
use super::{Job, Op, Reply, unix_now};
use crate::app::{App, NetState};
use crate::config::{self, FieldKind, Paths, Settings};
use crate::reticulum::doc::Doc;
use crate::reticulum::redact;

/// One Reticulum restart per this long at most (a restart, or a config
/// change; a rollback isn't held back, but counts).
pub const RESTART_EVERY: Duration = Duration::from_secs(300);
/// How long a restart, a config change or its rollback has to bring
/// Reticulum back.
pub const HEALTH_WITHIN: Duration = Duration::from_secs(90);
/// The longest message in a Reticulum change's result.
const MAX_MESSAGE: usize = 300;
/// Reticulum config changes applied in an hour at most.
pub const APPLIES_PER_HOUR: usize = 4;
/// How long the next change waits after one was put back by itself.
pub const HOLD_AFTER_ROLLBACK: Duration = Duration::from_secs(15 * 60);
/// What a Reticulum that failed is reported as (why is in the node's log,
/// and may quote the config).
const NOT_STARTED: &str = "Reticulum didn't start (see the node's log)";
/// What a file that can't be saved is answered with.
const NOT_SAVED: &str = "The change couldn't be saved on the node (see the node's log)";

/// How long things may take (shorter in tests).
#[derive(Debug, Clone, Copy)]
pub(super) struct Timing {
    /// Reticulum restarts from afar, at most one in this long.
    pub restart_every: Duration,
    /// How long a restart or a config change has to bring Reticulum back.
    pub health_within: Duration,
    /// How long a rollback has to bring Reticulum back.
    pub rollback_within: Duration,
    /// At most `applies` config changes in `apply_window`.
    pub applies: usize,
    pub apply_window: Duration,
    /// How long the next change waits after one was put back by itself.
    pub hold_after_rollback: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            restart_every: RESTART_EVERY,
            health_within: HEALTH_WITHIN,
            rollback_within: HEALTH_WITHIN,
            applies: APPLIES_PER_HOUR,
            apply_window: Duration::from_secs(3600),
            hold_after_rollback: HOLD_AFTER_ROLLBACK,
        }
    }
}
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

/// A Reticulum config change waiting to be confirmed or put back, as
/// `manage/pending.json` has it, with what is only known in this run.
#[derive(Debug)]
struct Live {
    record: Pending,
    /// When its state began, for measuring.
    since: Instant,
    /// When its probation ends.
    deadline: Option<Instant>,
    /// What keeps it from being healthy, or why it is put back.
    problems: Vec<String>,
    /// Why it is put back.
    reason: String,
    /// Being put back: whether the old text is written back yet.
    restored: bool,
    /// Whether a failure to write it back was logged (once is enough).
    told: bool,
}

impl Live {
    fn new(record: Pending) -> Self {
        Self { record, since: Instant::now(), deadline: None, problems: Vec::new(), reason: String::new(), restored: false, told: false }
    }
}

/// How the last Reticulum config change ended.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangeResult {
    id: String,
    /// `committed`, `rolled_back`, `rollback_failed` or `left`.
    outcome: &'static str,
    at: u64,
    message: String,
}

/// Runs management jobs against the app (see [`Owner::run`]).
#[derive(Debug)]
pub struct Owner {
    audit: Audit,
    restart: Option<Restart>,
    /// When Reticulum was last restarted from afar (unix seconds, and the
    /// same moment for measuring): a restart, a config change or a rollback.
    restarted: Option<(u64, Instant)>,
    pending: Option<Live>,
    result: Option<ChangeResult>,
    timing: Timing,
    /// Whether a config change may turn interfaces or transport on
    /// (`--manage-allow-enable`).
    allow_enable: bool,
    /// When the config changes in the last `apply_window` were applied.
    applies: VecDeque<Instant>,
    /// No config change until then (one was put back by itself).
    held_until: Option<Instant>,
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
        Self::with_timing(paths, Timing::default())
    }

    pub(super) fn with_timing(paths: &Paths, timing: Timing) -> Self {
        // A change the start couldn't put back (its old text couldn't be
        // written) is tried again from here.
        let pending = match pending::load(paths) {
            Ok(Some(record)) if record.state == State::RollingBack => {
                tracing::warn!("Remote management: change {} is still to be rolled back: trying again", record.id);
                let mut live = Live::new(record);
                live.reason = "The node stopped before the change was confirmed".into();
                live.problems = vec![RestoreError::Write(String::new()).message().to_string()];
                live.told = true;
                Some(live)
            }
            _ => None,
        };
        Self {
            audit: Audit::load(paths.root.join("manage").join("audit.jsonl")),
            restart: None,
            restarted: None,
            pending,
            result: None,
            timing,
            allow_enable: false,
            applies: VecDeque::new(),
            held_until: None,
        }
    }

    /// Let config changes turn interfaces and transport on (off unless the
    /// node is started with `--manage-allow-enable`).
    pub fn allow_enable(mut self, allow: bool) -> Self {
        self.allow_enable = allow;
        self
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
            Op::Reticulum { revision, confirm_seconds, changes } => self.reticulum(app, &job.actor, &revision, confirm_seconds, &changes),
            Op::Confirm { id } => self.confirm(app, &job.actor, &id),
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

    /// Watch a restart in progress, and a Reticulum config change on
    /// probation (see [`Owner::watch_change`]).
    pub fn on_tick(&mut self, app: &mut App) {
        self.watch_restart(app);
        self.watch_change(app);
    }

    /// Watch a restart in progress: `up` once the network runs and every
    /// interface that was online before it is again, `unhealthy` if
    /// Reticulum fails or that hasn't happened in [`HEALTH_WITHIN`].
    fn watch_restart(&mut self, app: &App) {
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
            NetState::Failed(e) => {
                tracing::warn!("Remote management: Reticulum didn't start after the restart {}: {}", restart.id, printable(e));
                vec![NOT_STARTED.to_string()]
            }
            _ if restart.since.elapsed() < self.timing.health_within => return,
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

    /// Whether settings must wait: Reticulum is restarting, or a config
    /// change is being applied or put back (one on probation doesn't stop them).
    fn busy(&self) -> bool {
        self.restarting() || self.pending.as_ref().is_some_and(|live| matches!(live.record.state, State::Restarting | State::RollingBack))
    }

    /// When the next restart may be asked for (unix seconds), if not now.
    fn restart_allowed_at(&self) -> Option<u64> {
        let (started, since) = self.restarted?;
        (since.elapsed() < self.timing.restart_every).then(|| started + self.timing.restart_every.as_secs())
    }

    /// Mark a restart of Reticulum from afar now.
    fn mark_restart(&mut self) {
        self.restarted = Some((unix_now(), Instant::now()));
    }

    /// Add `record` to the audit log and the node's log (where what the
    /// request sent, echoed in a key or an error, is made printable first).
    fn record(&mut self, app: &mut App, record: Record<'_>) {
        self.record_noted(app, record, Vec::new());
    }

    /// [`Owner::record`], with notes on what the change turned on.
    fn record_noted(&mut self, app: &mut App, record: Record<'_>, notes: Vec<String>) {
        let keys: Vec<String> = record
            .changes
            .iter()
            .map(|c| if c.path.is_empty() { printable(&c.key) } else { printable(&format!("{}/{}", c.path.join("/"), c.key)) })
            .collect();
        let what = match (record.outcome, record.error) {
            (Outcome::Ok, _) => "done".to_string(),
            (_, Some(error)) => format!("refused or failed: {}", printable(error)),
            (_, None) => "refused or failed".to_string(),
        };
        let keys = if keys.is_empty() { String::new() } else { format!(" ({})", keys.join(", ")) };
        app.log(format!("Remote management: {} by {}{keys}, {what}", record.op, printable(record.actor)));
        if let Err(e) = self.audit.record_noted(record, notes, unix_now()) {
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
        let reticulum = match redact::read_capped(&app.rns_path()) {
            Some((text, over)) => {
                let view = redact::view_of(&text, over);
                // A link is refused when changed, so nothing is offered.
                let editable = if shared || pending::is_link(&app.rns_path()) { Vec::new() } else { rnsedit::editable(&text, &view) };
                view.to_json_with(shared, &editable)
            }
            None => Value::Null,
        };
        let pending = self.pending.as_ref().map(|live| {
            let p = &live.record;
            json!({
                "id": p.id,
                "actor": p.actor,
                "state": p.state.as_str(),
                "applied_at": p.applied_at,
                "confirm_seconds": p.confirm_seconds,
                "healthy_at": p.healthy_at,
                "confirm_by": p.confirm_by,
                "changes": p.changes,
                "problems": live.problems,
            })
        });
        let result = self.result.as_ref().map(|r| json!({ "id": r.id, "outcome": r.outcome, "at": r.at, "message": r.message }));
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
                "pending": pending,
                "reticulum_result": result,
                "reticulum_enable_allowed": self.allow_enable,
            }),
        )
    }

    /// The changes asked for, as the audit log keeps a refused one.
    fn asked(before: &Settings, changes: &[(&str, &str)]) -> Vec<Change> {
        changes
            .iter()
            .map(|(key, value)| Change {
                path: Vec::new(),
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
        if self.busy() {
            return self.refuse(app, actor, "settings", asked, StatusCode::CONFLICT, "busy", self.busy_message(), Outcome::Refused);
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
        if self.busy() {
            return self.refuse(app, actor, "rollback", asked, StatusCode::CONFLICT, "busy", self.busy_message(), Outcome::Refused);
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
        if let Some(reply) = self.pending_refusal(app, actor, "restart", vec![]) {
            return reply;
        }
        if self.restarting() {
            let message = "Reticulum is restarting already";
            return self.refuse(app, actor, "restart", vec![], StatusCode::CONFLICT, "restarting", message, Outcome::Refused);
        }
        if let Some(at) = self.restart_allowed_at() {
            let wait = at.saturating_sub(unix_now()).max(1);
            let message = format!(
                "Reticulum can be restarted once every {} minutes: try again in {wait} seconds",
                self.timing.restart_every.as_secs() / 60
            );
            let reply =
                self.refuse(app, actor, "restart", vec![], StatusCode::TOO_MANY_REQUESTS, "rate_limited", &message, Outcome::Refused);
            return reply.with("retry_after", wait);
        }
        let id = new_id();
        let before = app.interfaces.iter().filter(|i| i.online).map(|i| i.name.clone()).collect();
        app.request_rns_restart();
        self.mark_restart();
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

    fn busy_message(&self) -> &'static str {
        if self.restarting() {
            "Reticulum is restarting: try again once it's back"
        } else {
            "A Reticulum config change is being applied or put back: try again once it's done"
        }
    }

    /// The refusal of `op` while a Reticulum config change is pending, if one is.
    fn pending_refusal(&mut self, app: &mut App, actor: &str, op: &'static str, asked: Vec<Change>) -> Option<Reply> {
        let id = self.pending.as_ref()?.record.id.clone();
        let message = "Another Reticulum config change on this node is still being tested: wait until it is kept or rolled back";
        Some(self.refuse(app, actor, op, asked, StatusCode::CONFLICT, "pending", message, Outcome::Refused).with("id", id))
    }

    /// A refusal from [`rnsedit::plan`] as an answer.
    fn rns_refusal(&mut self, app: &mut App, actor: &str, asked: Vec<Change>, refusal: RnsRefusal) -> Reply {
        let (status, code, message, extra): (StatusCode, &str, String, Vec<(&str, Value)>) = match refusal {
            RnsRefusal::NotAllowed { path, key, message } => {
                (StatusCode::FORBIDDEN, "not_allowed", message, vec![("path", json!(path)), ("key", json!(key))])
            }
            RnsRefusal::Invalid { path, key, message } => {
                (StatusCode::BAD_REQUEST, "invalid_value", message, vec![("path", json!(path)), ("key", json!(key))])
            }
            RnsRefusal::UnknownSection { path } => {
                (StatusCode::NOT_FOUND, "unknown_section", "The Reticulum config has no such section".into(), vec![("path", json!(path))])
            }
            RnsRefusal::Ambiguous { path, key, message } => {
                let mut extra = vec![("path", json!(path))];
                extra.extend(key.map(|key| ("key", json!(key))));
                (StatusCode::CONFLICT, "ambiguous", message, extra)
            }
            RnsRefusal::Stale { revision } => (
                StatusCode::CONFLICT,
                "stale_revision",
                "The Reticulum config changed since it was read: load it again".into(),
                vec![("reticulum_revision", json!(revision))],
            ),
            RnsRefusal::NoChange => (StatusCode::BAD_REQUEST, "no_change", "Every value is what it is already".into(), vec![]),
        };
        let reply = self.refuse(app, actor, "reticulum", asked, status, code, &message, Outcome::Refused);
        extra.into_iter().fold(reply, |reply, (key, value)| reply.with(key, value))
    }

    /// Change the Reticulum config (see [`rnsedit::plan`]), save it with the
    /// old text kept in the history, and restart Reticulum with it, on
    /// probation for `confirm_seconds` once it is back.
    fn reticulum(&mut self, app: &mut App, actor: &str, revision: &str, confirm_seconds: u64, changes: &[RnsChange]) -> Reply {
        let asked: Vec<Change> =
            changes.iter().map(|c| Change { path: c.path.clone(), key: c.key.clone(), from: String::new(), to: c.value.clone() }).collect();
        let refuse = |owner: &mut Self, app: &mut App, status: StatusCode, code: &str, message: &str, asked: Vec<Change>| {
            owner.refuse(app, actor, "reticulum", asked, status, code, message, Outcome::Refused)
        };
        if let Some(reply) = self.pending_refusal(app, actor, "reticulum", asked.clone()) {
            return reply;
        }
        if self.restarting() {
            return refuse(self, app, StatusCode::CONFLICT, "restarting", "Reticulum is restarting: try again once it's back", asked);
        }
        if !matches!(app.net_state, NetState::Online) {
            let message =
                "Reticulum isn't running, so its config can't be tested from afar: change it in the terminal, or try again once it runs";
            return refuse(self, app, StatusCode::CONFLICT, "busy", message, asked);
        }
        if app.uses_external_shared_instance() {
            let message = "This node uses another program's shared instance (such as rnsd), whose interfaces aren't in this config";
            return refuse(self, app, StatusCode::CONFLICT, "shared_instance", message, asked);
        }
        if let Some(at) = self.restart_allowed_at() {
            let wait = at.saturating_sub(unix_now()).max(1);
            let message = format!(
                "Reticulum can be restarted once every {} minutes, and a change restarts it: try again in {wait} seconds",
                self.timing.restart_every.as_secs() / 60
            );
            return refuse(self, app, StatusCode::TOO_MANY_REQUESTS, "rate_limited", &message, asked).with("retry_after", wait);
        }
        if let Some((wait, message)) = self.apply_wait() {
            return refuse(self, app, StatusCode::TOO_MANY_REQUESTS, "rate_limited", &message, asked).with("retry_after", wait);
        }
        let path = app.rns_path();
        let failed = |owner: &mut Self, app: &mut App, message: &str, asked: Vec<Change>| {
            owner.refuse(app, actor, "reticulum", asked, StatusCode::INTERNAL_SERVER_ERROR, "failed", message, Outcome::Failed)
        };
        // A change through a link would land somewhere else.
        if pending::is_link(&path) {
            let path = changes.first().map(|c| c.path.clone()).unwrap_or_default();
            let message = "The Reticulum config is a link; change it in the terminal".to_string();
            return self.rns_refusal(app, actor, asked, RnsRefusal::Ambiguous { path, key: None, message });
        }
        let Some((before, over)) = redact::read_capped(&path) else {
            return failed(self, app, "The Reticulum config can't be read", asked);
        };
        let planned = if over {
            Err(rnsedit::too_big(&before, revision, changes))
        } else {
            rnsedit::plan(&before, revision, changes, self.allow_enable)
        };
        let planned = match planned {
            Ok(planned) => planned,
            Err(refusal) => return self.rns_refusal(app, actor, asked, refusal),
        };
        let id = new_id();
        let now = unix_now();
        // What was online and is in this config must be back, but what the change turns off.
        let sections: HashSet<String> = Doc::new(&before).subsections("interfaces").into_iter().collect();
        let mut required: Vec<String> = Vec::new();
        for interface in app.interfaces.iter().filter(|i| i.online && sections.contains(&i.name) && !planned.disabled.contains(&i.name)) {
            if !required.contains(&interface.name) {
                required.push(interface.name.clone());
            }
        }
        let mut record = Pending {
            v: 1,
            id: id.clone(),
            actor: actor.to_string(),
            config: path.clone(),
            backup: None,
            before_sha256: pending::sha256_hex(before.as_bytes()),
            after_sha256: pending::sha256_hex(planned.after.as_bytes()),
            state: State::Restarting,
            applied_at: now,
            confirm_seconds,
            healthy_at: None,
            confirm_by: None,
            changes: planned.applied.clone(),
            required,
        };
        // Written before the config is, so a crash from here on is put right at the next start.
        if let Err(e) = pending::save(&app.paths, &record) {
            app.log(format!("Remote management: {}", printable(&e)));
            return failed(self, app, NOT_SAVED, planned.applied);
        }
        let history = app.rns_history();
        let time = crate::app::now();
        match history.before_saving(&path, Some(&before), time) {
            Ok(kept) => record.backup = kept,
            Err(e) => app.log(e),
        }
        if let Err(e) = crate::reticulum::save(&path, &planned.after) {
            app.log(format!("Remote management: {}", printable(&e)));
            if let Err(gone) = pending::remove(&app.paths) {
                app.log(gone);
            }
            return failed(self, app, NOT_SAVED, planned.applied);
        }
        let said: Vec<String> = planned.applied.iter().map(|c| format!("{}/{} = {}", c.path.join("/"), c.key, c.to)).collect();
        if let Err(e) =
            history.record(&path, &planned.after, &format!("Remote change {id} by {actor}, on probation: {}", said.join(", ")), time)
        {
            app.log(e);
        }
        if record.backup.is_some()
            && let Err(e) = pending::save(&app.paths, &record)
        {
            app.log(e);
        }
        app.rns_reload();
        app.request_rns_restart();
        self.mark_restart();
        self.applies.push_back(Instant::now());
        // Turning things on from afar opens the node to others: said loudly.
        if !planned.turned_on.is_empty() {
            let said = format!(
                "warning: Remote management: {} turned on from afar (change {id}): {}",
                printable(actor),
                printable(&planned.turned_on.join("; "))
            );
            tracing::warn!("{said}");
            app.log(said);
        }
        let entry = Record { id: &id, actor, op: "reticulum", changes: planned.applied.clone(), outcome: Outcome::Ok, error: None };
        self.record_noted(app, entry, planned.turned_on.clone());
        let body = json!({
            "id": id,
            "state": "restarting",
            "confirm_seconds": confirm_seconds,
            "applied": planned.applied,
            "notes": planned.notes,
            "reticulum_revision": redact::redact(&planned.after).revision(),
        });
        self.pending = Some(Live::new(record));
        Reply::new(StatusCode::ACCEPTED, body)
    }

    /// How long until the next config change may be applied, and why, if
    /// not now: at most `applies` in `apply_window`, and none for a while
    /// after one was put back by itself.
    fn apply_wait(&mut self) -> Option<(u64, String)> {
        let now = Instant::now();
        let window = self.timing.apply_window;
        while self.applies.front().is_some_and(|at| now.duration_since(*at) >= window) {
            self.applies.pop_front();
        }
        let seconds = |wait: Duration| wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        if let Some(until) = self.held_until.filter(|until| *until > now) {
            let wait = seconds(until - now).max(1);
            let message = format!(
                "A change was rolled back by itself a short while ago, so the next one waits {} minutes after it: try again in {wait} seconds",
                self.timing.hold_after_rollback.as_secs() / 60
            );
            return Some((wait, message));
        }
        if self.applies.len() >= self.timing.applies {
            let oldest = *self.applies.front()?;
            let wait = seconds((oldest + window).saturating_duration_since(now)).max(1);
            let message = format!(
                "At most {} Reticulum config changes are made in {} minutes: try again in {wait} seconds",
                self.timing.applies,
                window.as_secs() / 60
            );
            return Some((wait, message));
        }
        None
    }

    /// Keep the change `id`, on probation, for the actor who made it.
    fn confirm(&mut self, app: &mut App, actor: &str, id: &str) -> Reply {
        let found = self.pending.as_ref().filter(|live| live.record.id == id);
        let (state, owner, changes) = match found {
            Some(live) => (Some(live.record.state), live.record.actor.clone(), live.record.changes.clone()),
            None => (None, String::new(), Vec::new()),
        };
        let refused = |this: &mut Self, app: &mut App, status: StatusCode, code: &str, message: &str| {
            this.record(app, Record { id, actor, op: "confirm", changes: vec![], outcome: Outcome::Refused, error: Some(message) });
            Reply::error(status, code, message)
        };
        if state != Some(State::Probation) {
            let message = match state {
                None => "No change with that id is waiting to be confirmed",
                Some(_) => "That change isn't on probation: Reticulum is restarting with it, or it is being rolled back",
            };
            return refused(self, app, StatusCode::CONFLICT, "not_pending", message).with("state", state.map(State::as_str));
        }
        if owner != actor {
            return refused(self, app, StatusCode::FORBIDDEN, "not_allowed", "Only whoever made a change can confirm it");
        }
        let Some(mut live) = self.pending.take() else { return Reply::error(StatusCode::CONFLICT, "not_pending", "Nothing is pending") };
        live.record.state = State::Committed;
        // Either is enough: a file saved as committed is only tidied up at the next start.
        let saved = pending::save(&app.paths, &live.record);
        let removed = pending::remove(&app.paths);
        if let (Err(saving), Err(removing)) = (&saved, &removed) {
            app.log(format!("Remote management: {}; {}", printable(saving), printable(removing)));
            live.record.state = State::Probation;
            self.pending = Some(live);
            let message = "The change couldn't be marked as kept, so it is still on probation (see the node's log)";
            self.record(app, Record { id, actor, op: "confirm", changes, outcome: Outcome::Failed, error: Some(message) });
            return Reply::error(StatusCode::INTERNAL_SERVER_ERROR, "failed", message);
        }
        for e in [saved.err(), removed.err()].into_iter().flatten() {
            app.log(e);
        }
        self.record(app, Record { id, actor, op: "confirm", changes, outcome: Outcome::Ok, error: None });
        self.result = Some(ChangeResult { id: id.to_string(), outcome: "committed", at: unix_now(), message: "Kept".into() });
        Reply::new(StatusCode::OK, json!({ "id": id, "state": "committed" }))
    }

    /// Watch a Reticulum config change: restarting until Reticulum is back
    /// with every interface it needs (then on probation), on probation until
    /// it is confirmed or its time is up, and, if put back, until Reticulum
    /// is back with the old config.
    fn watch_change(&mut self, app: &mut App) {
        let Some(live) = self.pending.as_mut() else { return };
        let online: HashSet<&str> = app.interfaces.iter().filter(|i| i.online).map(|i| i.name.as_str()).collect();
        match live.record.state {
            State::Restarting => {
                let mut problems: Vec<String> = live
                    .record
                    .required
                    .iter()
                    .filter(|name| !online.contains(name.as_str()))
                    .map(|name| format!("{} is offline", cut(name, MAX_PROBLEM)))
                    .collect();
                if app.uses_external_shared_instance() {
                    problems.push("Another program's shared instance is in use now".into());
                }
                match &app.net_state {
                    NetState::Online if problems.is_empty() => {
                        let now = unix_now();
                        live.record.state = State::Probation;
                        live.record.healthy_at = Some(now);
                        live.record.confirm_by = Some(now.saturating_add(live.record.confirm_seconds));
                        live.since = Instant::now();
                        live.deadline = Some(live.since + Duration::from_secs(live.record.confirm_seconds));
                        live.problems.clear();
                        if let Err(e) = pending::save(&app.paths, &live.record) {
                            app.log(e);
                        }
                        tracing::info!("Remote management: Reticulum is back with change {}, on probation", live.record.id);
                    }
                    NetState::Failed(e) => {
                        tracing::warn!("Remote management: Reticulum didn't start with change {}: {}", live.record.id, printable(e));
                        let reason = format!("Reticulum isn't back as it was: {NOT_STARTED}");
                        self.roll_back(app, reason);
                    }
                    NetState::Starting if live.since.elapsed() < self.timing.health_within => {
                        live.problems = problems;
                        live.problems.insert(0, "Reticulum is starting".into());
                        live.problems.truncate(MAX_PROBLEMS);
                    }
                    _ if live.since.elapsed() < self.timing.health_within => {
                        live.problems = problems;
                        live.problems.truncate(MAX_PROBLEMS);
                    }
                    state => {
                        if matches!(state, NetState::Starting) {
                            problems.insert(0, "Reticulum is still starting".into());
                        }
                        problems.truncate(MAX_PROBLEMS);
                        let reason = format!("Reticulum isn't back as it was: {}", problems.join("; "));
                        self.roll_back(app, reason);
                    }
                }
            }
            State::Probation => {
                if let NetState::Failed(e) = &app.net_state {
                    tracing::warn!("Remote management: Reticulum stopped with change {} on probation: {}", live.record.id, printable(e));
                    self.roll_back(app, "Reticulum stopped while the change was on probation (see the node's log)".into());
                } else if live.deadline.is_none_or(|deadline| Instant::now() >= deadline) {
                    self.roll_back(app, "No confirm by the deadline".into());
                }
            }
            State::RollingBack if !live.restored => self.try_restore(app),
            State::RollingBack => {
                let reason = live.reason.clone();
                match &app.net_state {
                    NetState::Online => self.finish(app, Outcome::RolledBack, "rolled_back", &reason),
                    NetState::Failed(e) => {
                        tracing::warn!("Remote management: Reticulum didn't start with the old config: {}", printable(e));
                        let message = format!("Rolled back ({reason}), but Reticulum isn't back as it was: {NOT_STARTED}");
                        self.finish(app, Outcome::Failed, "rolled_back", &message);
                    }
                    NetState::Starting if live.since.elapsed() >= self.timing.rollback_within => {
                        let message = format!("Rolled back ({reason}), but Reticulum isn't back as it was: it is still starting");
                        self.finish(app, Outcome::Failed, "rolled_back", &message);
                    }
                    NetState::Starting => {}
                }
            }
            // Kept: nothing to watch.
            State::Committed => self.pending = None,
        }
    }

    /// Put the config from before the pending change back, for `reason`, and
    /// restart Reticulum with it (held back by no limit, but counted). The
    /// state is saved as being rolled back first, so a stop while it is
    /// written is put right at the next start; and the next change waits a
    /// while.
    fn roll_back(&mut self, app: &mut App, reason: String) {
        let Some(live) = self.pending.as_mut() else { return };
        tracing::warn!("Remote management: rolling back change {}: {}", live.record.id, printable(&reason));
        live.reason = cut(&reason, MAX_MESSAGE);
        live.problems = vec![live.reason.clone()];
        live.record.state = State::RollingBack;
        live.restored = false;
        live.since = Instant::now();
        if let Err(e) = pending::save(&app.paths, &live.record) {
            app.log(e);
        }
        self.held_until = Some(Instant::now() + self.timing.hold_after_rollback);
        self.try_restore(app);
    }

    /// Write the old text back, if the config is still as the change left it,
    /// and restart Reticulum with it. One that can't be written is tried
    /// again at the next tick; one changed on this computer meanwhile, a
    /// link, or one whose old text is nowhere is left as it is.
    fn try_restore(&mut self, app: &mut App) {
        let Some(live) = self.pending.as_mut() else { return };
        let reason = live.reason.clone();
        let sha = pending::config_sha256(&live.record.config);
        let outcome = if sha.as_deref() == Some(live.record.before_sha256.as_str()) {
            // Back already (written, but not marked so before a stop).
            Ok(())
        } else if sha.as_deref() != Some(live.record.after_sha256.as_str()) {
            let message = format!(
                "The Reticulum config was changed on this computer meanwhile, so it was left as it is (it was to be rolled back: {reason})"
            );
            self.finish_as(app, "auto_rollback", Outcome::Failed, "left", &message);
            return;
        } else {
            pending::restore(&app.rns_history(), &live.record, &reason)
        };
        match outcome {
            Ok(()) => {
                live.restored = true;
                live.since = Instant::now();
                live.problems = vec![reason];
                app.rns_reload();
                app.request_rns_restart();
                self.mark_restart();
            }
            Err(RestoreError::Write(e)) => {
                live.problems = vec![RestoreError::Write(String::new()).message().to_string()];
                if !live.told {
                    live.told = true;
                    tracing::error!("Remote management: change {} couldn't be rolled back yet: {e}", live.record.id);
                    app.log(format!(
                        "warning: Remote management: the old Reticulum config couldn't be written back for change {}: {}; trying again",
                        live.record.id,
                        printable(&e)
                    ));
                }
            }
            Err(e) => {
                tracing::error!("Remote management: change {} couldn't be rolled back: {}", live.record.id, e.message());
                app.log(format!(
                    "Remote management: CHANGE {} COULDN'T BE ROLLED BACK: {}; check the Reticulum config and its history",
                    live.record.id,
                    e.message()
                ));
                let result = if e == RestoreError::Link { "left" } else { "rollback_failed" };
                self.finish_as(app, "auto_rollback", Outcome::Failed, result, e.message());
            }
        }
    }

    /// End the pending change as an `auto_rollback` with `outcome`.
    fn finish(&mut self, app: &mut App, outcome: Outcome, result: &'static str, message: &str) {
        self.finish_as(app, "auto_rollback", outcome, result, message);
    }

    /// End the pending change: audit it as `op` with `outcome`, keep the
    /// result for `/state`, and remove `pending.json`.
    fn finish_as(&mut self, app: &mut App, op: &'static str, outcome: Outcome, result: &'static str, message: &str) {
        let Some(live) = self.pending.take() else { return };
        let p = &live.record;
        let error = (outcome != Outcome::Ok).then_some(message);
        self.record(app, Record { id: &p.id, actor: &p.actor, op, changes: p.changes.clone(), outcome, error });
        self.result = Some(ChangeResult { id: p.id.clone(), outcome: result, at: unix_now(), message: cut(message, MAX_MESSAGE) });
        if let Err(e) = pending::remove(&app.paths) {
            app.log(e);
        }
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
            let timing = Timing {
                restart_every: Duration::from_secs(300),
                health_within: Duration::from_millis(200),
                rollback_within: Duration::from_millis(200),
                hold_after_rollback: Duration::ZERO,
                ..Timing::default()
            };
            let owner = Owner::with_timing(&app.paths, timing).allow_enable(true);
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

        /// A node whose Reticulum config is `text`, running, with the
        /// interface "Hilltop TCP" online.
        fn with_rns(name: &str, text: &str) -> Self {
            let mut node = Self::new(name, Settings::default());
            let rns = node.dir.join("rns");
            std::fs::create_dir_all(&rns).unwrap();
            std::fs::write(rns.join("config"), text).unwrap();
            node.app.settings.rns_config = Some(rns.display().to_string());
            node.app.net_state = NetState::Online;
            node.app.interfaces = vec![InterfaceInfo { name: HILL.into(), online: true, ..Default::default() }];
            node
        }

        fn config_text(&self) -> String {
            std::fs::read_to_string(self.app.rns_path()).unwrap()
        }

        fn pending_file(&self) -> Option<Pending> {
            pending::load(&self.app.paths).unwrap()
        }

        fn ask_as(&mut self, actor: &str, op: Op) -> Reply {
            let (reply, answer) = oneshot::channel();
            self.owner.run(&mut self.app, Job { actor: actor.into(), op, reply });
            answer.blocking_recv().expect("every job is answered")
        }

        /// Ask for `changes` to the config, read at the revision `/state` gives.
        fn apply(&mut self, confirm_seconds: u64, changes: &[(&[&str], &str, &str)]) -> Reply {
            let revision = self.state()["reticulum"]["revision"].as_str().unwrap().to_string();
            let changes = changes
                .iter()
                .map(|(path, key, value)| RnsChange {
                    path: path.iter().map(|s| s.to_string()).collect(),
                    key: key.to_string(),
                    value: value.to_string(),
                })
                .collect();
            self.ask(Op::Reticulum { revision, confirm_seconds, changes })
        }

        /// Reticulum restarted, as the node's loop does it, and back with `online`.
        fn restarted(&mut self, online: &[&str]) {
            assert!(self.app.take_rns_restart(), "a restart was asked for");
            self.app.net_state = NetState::Online;
            self.app.interfaces =
                online.iter().map(|name| InterfaceInfo { name: name.to_string(), online: true, ..Default::default() }).collect();
        }

        fn tick(&mut self) {
            self.owner.on_tick(&mut self.app);
        }

        /// Through the restart, back with the interface, and on probation.
        fn reach_probation(&mut self, confirm_seconds: u64) -> String {
            let reply = self.apply(confirm_seconds, &[(&["reticulum"], "enable_transport", "yes")]);
            assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.body);
            self.restarted(&[HILL]);
            self.tick();
            assert_eq!(self.state()["pending"]["state"], "probation");
            reply.body["id"].as_str().unwrap().to_string()
        }
    }

    const HILL: &str = "Hilltop TCP";
    const RNS: &str = "\
[reticulum]
  enable_transport = No   # not yet
  share_instance = No

[interfaces]
  [[Hilltop TCP]]
    type = TCPServerInterface
    enabled = yes
    listen_ip = 127.0.0.1
    listen_port = 4242
    passphrase = hunter2-secret

  [[Spare]]
    type = TCPClientInterface
    enabled = no
    target_host = hub.example.org
    target_port = 4242
";

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
        node.owner.on_tick(&mut node.app);
        assert_eq!(node.state()["restart"]["state"], "restarting", "the interfaces aren't back");
        node.app.interfaces = vec![InterfaceInfo { name: "Up".into(), online: true, ..Default::default() }];
        node.owner.on_tick(&mut node.app);
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
        node.owner.timing.restart_every = Duration::ZERO;
        node.app.interfaces = vec![InterfaceInfo { name: "Uplink".into(), online: true, ..Default::default() }];
        let first = node.ask(Op::Restart);
        node.app.take_rns_restart();
        node.app.net_state = NetState::Online;
        node.owner.on_tick(&mut node.app);
        assert_eq!(node.state()["restart"]["state"], "restarting", "it has 90 seconds (here 200 ms)");
        std::thread::sleep(Duration::from_millis(250));
        node.owner.on_tick(&mut node.app);
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
        node.owner.on_tick(&mut node.app);
        assert_eq!(node.state()["restart"]["problems"][0], NOT_STARTED, "why stays in the node's log");
        assert_eq!(node.ask(Op::Restart).status, StatusCode::ACCEPTED);
        node.app.take_rns_restart();
        std::thread::sleep(Duration::from_millis(250));
        node.owner.on_tick(&mut node.app);
        assert_eq!(node.state()["restart"]["problems"][0], "Reticulum is still starting");
        // Nothing to watch: nothing happens.
        node.owner.on_tick(&mut node.app);
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
            changes: vec![Change { path: vec![], key: nasty.into(), from: String::new(), to: nasty.into() }],
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

    #[test]
    fn a_reticulum_change_is_saved_tested_and_kept_once_confirmed() {
        let mut node = Node::with_rns("rns-confirm", RNS);
        let state = node.state();
        assert_eq!(state["reticulum"]["editable"], json!([["reticulum"], ["interfaces", HILL], ["interfaces", "Spare"]]));
        assert_eq!((state["pending"].clone(), state["reticulum_result"].clone()), (Value::Null, Value::Null));
        let reply = node.apply(300, &[(&["reticulum"], "enable_transport", "yes"), (&["interfaces", "Spare"], "enabled", "yes")]);
        assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.body);
        let id = reply.body["id"].as_str().unwrap().to_string();
        assert_eq!(id.len(), 16);
        assert_eq!(reply.body["state"], "restarting");
        assert_eq!(reply.body["confirm_seconds"], 300);
        assert_eq!(reply.body["applied"][0], json!({ "path": ["reticulum"], "key": "enable_transport", "from": "No", "to": "yes" }));
        assert_eq!(reply.body["applied"][1], json!({ "path": ["interfaces", "Spare"], "key": "enabled", "from": "no", "to": "yes" }));
        assert!(reply.body["notes"].is_array());
        // Saved, with only those lines changed, and the old text in the history.
        let after = node.config_text();
        assert_eq!(
            after,
            RNS.replace("enable_transport = No   # not yet", "enable_transport = Yes # not yet")
                .replace("    enabled = no\n    target_host", "    enabled = yes\n    target_host")
        );
        assert_eq!(reply.body["reticulum_revision"], json!(redact::redact(&after).revision()));
        assert_eq!(node.state()["reticulum"]["revision"], reply.body["reticulum_revision"]);
        let versions = node.app.rns_versions();
        assert!(versions[0].what.starts_with(&format!("Remote change {id} by alice, on probation: reticulum/enable_transport = yes")));
        assert_eq!(node.app.rns_history().text(&versions[1].id).unwrap(), RNS);
        let pending = node.pending_file().unwrap();
        assert_eq!((pending.state, pending.id.as_str(), pending.actor.as_str()), (State::Restarting, id.as_str(), "alice"));
        assert_eq!(pending.required, [HILL], "online now, in the config, and not turned off");
        assert_eq!(pending.backup.as_deref(), Some(versions[1].id.as_str()));
        assert_eq!(pending.before_sha256, pending::sha256_hex(RNS.as_bytes()));
        assert_eq!(pending.after_sha256, pending::sha256_hex(after.as_bytes()));
        assert!(!std::fs::read_to_string(pending::path(&node.app.paths)).unwrap().contains("hunter2"));
        // Reticulum restarts with it; settings wait; another change or a restart is refused.
        assert!(node.app.take_rns_restart());
        let state = node.state();
        assert_eq!(state["pending"]["state"], "restarting");
        assert_eq!(state["pending"]["changes"], reply.body["applied"]);
        assert_eq!(state["pending"]["healthy_at"], Value::Null);
        assert!(state["restart_allowed_at"].as_u64().is_some());
        assert_eq!(state["restart"], Value::Null, "the restart object is for restarts asked for");
        assert_eq!(node.change(&[("pn_name", "X")]).body["error"], "busy");
        let again = node.apply(300, &[(&["interfaces", HILL], "mode", "full")]);
        assert_eq!(
            (again.status, again.body["error"].as_str(), again.body["id"].as_str()),
            (StatusCode::CONFLICT, Some("pending"), Some(id.as_str()))
        );
        assert_eq!(node.ask(Op::Restart).body["error"], "pending");
        // Starting, then running without the interface, then with it: on probation.
        node.tick();
        assert_eq!(node.state()["pending"]["problems"], json!(["Reticulum is starting", "Hilltop TCP is offline"]));
        node.app.net_state = NetState::Online;
        node.tick();
        assert_eq!(node.state()["pending"]["problems"], json!(["Hilltop TCP is offline"]));
        node.app.interfaces = vec![InterfaceInfo { name: HILL.into(), online: true, ..Default::default() }];
        node.tick();
        let state = node.state();
        assert_eq!(state["pending"]["state"], "probation");
        let healthy = state["pending"]["healthy_at"].as_u64().unwrap();
        assert_eq!(state["pending"]["confirm_by"].as_u64().unwrap(), healthy + 300);
        assert_eq!(node.pending_file().unwrap().state, State::Probation);
        // Settings may change meanwhile.
        assert_eq!(node.change(&[("pn_name", "X")]).status, StatusCode::OK);
        // Only its maker, with its id, may confirm it.
        let wrong = node.ask(Op::Confirm { id: "0123456789abcdef".into() });
        assert_eq!(
            (wrong.status, wrong.body["error"].as_str(), wrong.body["state"].clone()),
            (StatusCode::CONFLICT, Some("not_pending"), Value::Null)
        );
        let other = node.ask_as("bob", Op::Confirm { id: id.clone() });
        assert_eq!((other.status, other.body["error"].as_str()), (StatusCode::FORBIDDEN, Some("not_allowed")));
        let kept = node.ask(Op::Confirm { id: id.clone() });
        assert_eq!((kept.status, kept.body.clone()), (StatusCode::OK, json!({ "id": id, "state": "committed" })));
        assert!(node.pending_file().is_none());
        let state = node.state();
        assert_eq!(state["pending"], Value::Null);
        assert_eq!(state["reticulum_result"]["id"], json!(id));
        assert_eq!(state["reticulum_result"]["outcome"], "committed");
        assert_eq!(node.config_text(), after, "kept");
        assert_eq!(node.ask(Op::Confirm { id: id.clone() }).body["error"], "not_pending");
        // It can't be rolled back as a settings change; and the next restart waits for the limit.
        assert_eq!(node.ask(Op::Rollback { id: id.clone() }).body["error"], "unknown_change");
        assert_eq!(node.ask(Op::Restart).body["error"], "rate_limited");
        let audit = node.audit();
        let ours: Vec<(&str, &str)> =
            audit.iter().filter(|e| e["id"] == json!(id)).map(|e| (e["op"].as_str().unwrap(), e["result"].as_str().unwrap())).collect();
        // Refusals of a confirm are kept under the change's id too.
        assert_eq!(ours, [("confirm", "refused"), ("confirm", "ok"), ("confirm", "refused"), ("reticulum", "ok")]);
        assert!(audit.iter().any(|e| e["op"] == "confirm" && e["result"] == "refused" && e["actor"] == "bob"));
        assert_eq!(
            audit.iter().find(|e| e["op"] == "reticulum" && e["result"] == "ok").unwrap()["changes"][1]["path"],
            json!(["interfaces", "Spare"])
        );
        assert!(node.app.log.iter().any(|l| l.contains("reticulum by alice (reticulum/enable_transport, interfaces/Spare/enabled), done")));
    }

    /// The config and the history once a change was rolled back by itself.
    fn assert_rolled_back(node: &mut Node, id: &str, reason: &str) {
        assert_eq!(node.config_text(), RNS, "the old bytes, exactly");
        let state = node.state();
        assert_eq!(state["pending"], Value::Null);
        assert_eq!(state["reticulum_result"]["id"], json!(id));
        assert_eq!(state["reticulum_result"]["outcome"], "rolled_back");
        assert!(state["reticulum_result"]["message"].as_str().unwrap().contains(reason), "{state}");
        assert!(node.pending_file().is_none());
        assert!(node.app.rns_versions()[0].what.starts_with(&format!("Rolled back remote change {id}: ")));
        let audit = node.audit();
        assert_eq!((audit[0]["op"].as_str(), audit[0]["id"].as_str()), (Some("auto_rollback"), Some(id)));
    }

    #[test]
    fn a_change_not_confirmed_in_time_is_rolled_back_by_itself() {
        let mut node = Node::with_rns("rns-deadline", RNS);
        let id = node.reach_probation(0);
        node.tick();
        // Put back and restarting with the old config; settings wait for it.
        assert_eq!(node.config_text(), RNS);
        assert_eq!(node.state()["pending"]["state"], "rolling_back");
        assert_eq!(node.pending_file().unwrap().state, State::RollingBack);
        assert_eq!(node.change(&[("pn_name", "X")]).body["error"], "busy");
        assert_eq!(node.ask(Op::Confirm { id: id.clone() }).body["state"], "rolling_back");
        node.restarted(&[HILL]);
        node.tick();
        assert_rolled_back(&mut node, &id, "No confirm by the deadline");
        assert_eq!(node.audit()[0]["result"], "rolled_back");
        // A retry still waits for the restart limit.
        let retry = node.apply(300, &[(&["reticulum"], "enable_transport", "yes")]);
        assert_eq!((retry.status, retry.body["error"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")));
        assert!(retry.body["retry_after"].as_u64().unwrap() > 200);
    }

    #[test]
    fn a_change_reticulum_fails_with_is_rolled_back_at_once() {
        let mut node = Node::with_rns("rns-failed", RNS);
        let reply = node.apply(300, &[(&["interfaces", HILL], "mode", "boundary")]);
        let id = reply.body["id"].as_str().unwrap().to_string();
        node.app.take_rns_restart();
        node.app.net_state = NetState::Failed("bad config".into());
        node.tick();
        assert_eq!(node.config_text(), RNS);
        assert_eq!(node.state()["pending"]["problems"][0], "Reticulum isn't back as it was: Reticulum didn't start (see the node's log)");
        // Still failing with the old config: rolled back, but said.
        assert!(node.app.take_rns_restart());
        node.tick();
        assert_eq!(node.state()["pending"]["state"], "rolling_back", "restarting with the old config");
        node.app.net_state = NetState::Failed("bad config".into());
        node.tick();
        assert_rolled_back(&mut node, &id, "but Reticulum isn't back as it was: Reticulum didn't start (see the node's log)");
        assert_eq!(node.audit()[0]["result"], "failed");
        // What Reticulum said stays on the node.
        assert!(!node.state().to_string().contains("bad config"));
        assert!(!Value::Array(node.audit()).to_string().contains("bad config"));
        // On probation, a failure rolls back too.
        node.owner.timing.restart_every = Duration::ZERO;
        node.app.net_state = NetState::Online;
        let id = node.reach_probation(300);
        node.app.net_state = NetState::Failed("crashed".into());
        node.tick();
        node.restarted(&[HILL]);
        node.tick();
        assert_rolled_back(&mut node, &id, "Reticulum stopped while the change was on probation (see the node's log)");
    }

    #[test]
    fn a_change_without_an_interface_that_was_up_is_rolled_back() {
        let mut node = Node::with_rns("rns-missing", RNS);
        let reply = node.apply(300, &[(&["interfaces", HILL], "mode", "boundary")]);
        let id = reply.body["id"].as_str().unwrap().to_string();
        node.restarted(&["Spare"]);
        node.tick();
        assert_eq!(node.state()["pending"]["state"], "restarting", "it has 90 seconds (here 200 ms)");
        std::thread::sleep(Duration::from_millis(250));
        node.tick();
        assert_eq!(node.config_text(), RNS);
        // The old config isn't back within the time either.
        assert!(node.app.take_rns_restart());
        node.tick();
        assert_eq!(node.state()["pending"]["state"], "rolling_back");
        std::thread::sleep(Duration::from_millis(250));
        node.tick();
        assert_rolled_back(&mut node, &id, "Hilltop TCP is offline");
        assert!(node.state()["reticulum_result"]["message"].as_str().unwrap().contains("it is still starting"));
        // Turning the interface off doesn't need it back; another program's shared instance isn't healthy.
        node.owner.timing.restart_every = Duration::ZERO;
        node.app.net_state = NetState::Online;
        node.app.interfaces = vec![InterfaceInfo { name: HILL.into(), online: true, ..Default::default() }];
        let reply = node.apply(300, &[(&["interfaces", HILL], "enabled", "no")]);
        assert_eq!(node.pending_file().unwrap().required, Vec::<String>::new(), "{}", reply.body);
        node.restarted(&["Shared Instance[37428]"]);
        node.tick();
        assert_eq!(node.state()["pending"]["problems"], json!(["Another program's shared instance is in use now"]));
        node.app.interfaces.clear();
        node.tick();
        assert_eq!(node.state()["pending"]["state"], "probation");
    }

    #[test]
    fn a_config_changed_here_meanwhile_is_left_and_a_missing_backup_is_said() {
        let mut node = Node::with_rns("rns-left", RNS);
        let id = node.reach_probation(0);
        std::fs::write(node.app.rns_path(), "[reticulum]\n  enable_transport = yes\n").unwrap();
        node.tick();
        assert_eq!(node.config_text(), "[reticulum]\n  enable_transport = yes\n");
        assert!(!node.app.take_rns_restart(), "nothing to restart for");
        let state = node.state();
        assert_eq!((state["reticulum_result"]["outcome"].as_str(), state["pending"].clone()), (Some("left"), Value::Null));
        let audit = node.audit();
        assert_eq!(
            (audit[0]["op"].as_str(), audit[0]["result"].as_str(), audit[0]["id"].as_str()),
            (Some("auto_rollback"), Some("failed"), Some(id.as_str()))
        );
        assert!(audit[0]["error"].as_str().unwrap().contains("changed on this computer"));

        // No copy of the old text anywhere: said loudly, and the file left.
        let mut node = Node::with_rns("rns-no-backup", RNS);
        node.reach_probation(0);
        let after = node.config_text();
        std::fs::remove_dir_all(&node.app.paths.reticulum_history).unwrap();
        std::fs::write(node.app.rns_path().with_file_name("config.backup"), "other\n").unwrap();
        node.tick();
        assert_eq!(node.config_text(), after);
        assert_eq!(node.state()["reticulum_result"]["outcome"], "rollback_failed");
        assert!(node.app.log.iter().any(|l| l.contains("COULDN'T BE ROLLED BACK")));
        assert!(node.pending_file().is_none());
    }

    #[test]
    fn a_reticulum_change_is_refused_when_it_cant_be_tested_or_isnt_allowed() {
        let mut node = Node::with_rns("rns-refused", RNS);
        let ok = [(&["reticulum"][..], "enable_transport", "yes")];
        let code = |reply: &Reply| (reply.status, reply.body["error"].as_str().unwrap_or_default().to_string());
        node.app.net_state = NetState::Starting;
        assert_eq!(code(&node.apply(300, &ok)), (StatusCode::CONFLICT, "busy".into()));
        node.app.net_state = NetState::Online;
        node.app.interfaces = vec![InterfaceInfo { name: "Shared Instance[37428]".into(), online: true, ..Default::default() }];
        assert_eq!(code(&node.apply(300, &ok)), (StatusCode::CONFLICT, "shared_instance".into()));
        assert_eq!(node.state()["reticulum"]["editable"], json!([]), "nothing is editable on a shared instance");
        node.app.interfaces.clear();
        // What the change asks for.
        let reply = node.apply(300, &[(&["interfaces", HILL], "passphrase", "x")]);
        assert_eq!(code(&reply), (StatusCode::FORBIDDEN, "not_allowed".into()));
        assert_eq!((reply.body["path"].clone(), reply.body["key"].clone()), (json!(["interfaces", HILL]), json!("passphrase")));
        let reply = node.apply(300, &[(&["interfaces", HILL], "bitrate", "1")]);
        assert_eq!((code(&reply), reply.body["key"].as_str()), ((StatusCode::BAD_REQUEST, "invalid_value".into()), Some("bitrate")));
        let reply = node.apply(300, &[(&["interfaces", "Nowhere"], "enabled", "no")]);
        assert_eq!(
            (code(&reply), reply.body["path"].clone()),
            ((StatusCode::NOT_FOUND, "unknown_section".into()), json!(["interfaces", "Nowhere"]))
        );
        assert_eq!(code(&node.apply(300, &[(&["interfaces", HILL], "enabled", "yes")])), (StatusCode::BAD_REQUEST, "no_change".into()));
        let stale = node.ask(Op::Reticulum { revision: "0".repeat(32), confirm_seconds: 300, changes: vec![] });
        assert_eq!(code(&stale), (StatusCode::CONFLICT, "stale_revision".into()));
        assert_eq!(stale.body["reticulum_revision"], node.state()["reticulum"]["revision"]);
        std::fs::write(node.app.rns_path(), format!("{RNS}  [[hilltop tcp]]\n    type = UDPInterface\n")).unwrap();
        let reply = node.apply(300, &[(&["interfaces", HILL], "mode", "full")]);
        assert_eq!((code(&reply), reply.body.get("key")), ((StatusCode::CONFLICT, "ambiguous".into()), None));
        std::fs::write(node.app.rns_path(), RNS).unwrap();
        // While a restart asked for runs, and within the restart limit after it.
        assert_eq!(node.ask(Op::Restart).status, StatusCode::ACCEPTED);
        assert_eq!(code(&node.apply(300, &ok)), (StatusCode::CONFLICT, "restarting".into()));
        node.restarted(&[HILL]);
        node.tick();
        let limited = node.apply(300, &ok);
        assert_eq!(code(&limited), (StatusCode::TOO_MANY_REQUESTS, "rate_limited".into()));
        assert!(limited.body["retry_after"].as_u64().unwrap() > 200);
        // Nothing changed, nothing restarted, nothing pending; every refusal audited.
        assert_eq!(node.config_text(), RNS);
        assert!(!node.app.take_rns_restart());
        assert!(node.pending_file().is_none());
        let refused = node.audit().iter().filter(|e| e["op"] == "reticulum" && e["result"] == "refused").count();
        assert_eq!(refused, 10);
        // A config that can't be read fails plainly.
        node.owner.timing.restart_every = Duration::ZERO;
        std::fs::remove_file(node.app.rns_path()).unwrap();
        let reply = node.ask(Op::Reticulum { revision: "0".repeat(32), confirm_seconds: 300, changes: vec![] });
        assert_eq!(code(&reply), (StatusCode::INTERNAL_SERVER_ERROR, "failed".into()));
    }

    #[test]
    fn a_change_whose_files_cant_be_written_changes_nothing() {
        // pending.json can't be written: nothing else is done.
        let mut node = Node::with_rns("rns-unwritable", RNS);
        std::fs::create_dir_all(pending::path(&node.app.paths).join("x")).unwrap();
        let reply = node.apply(300, &[(&["reticulum"], "enable_transport", "yes")]);
        assert_eq!((reply.status, reply.body["error"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("failed")));
        assert_eq!(node.config_text(), RNS);
        assert!(!node.app.take_rns_restart());
        assert_eq!(node.state()["pending"], Value::Null);
        // The config can't be saved: pending.json is taken back.
        let mut node = Node::with_rns("rns-unsaved", RNS);
        std::fs::create_dir_all(node.app.rns_path().with_file_name(".config.rettui-tmp").join("x")).unwrap();
        let reply = node.apply(300, &[(&["reticulum"], "enable_transport", "yes")]);
        assert_eq!((reply.status, reply.body["error"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("failed")));
        assert_eq!(node.config_text(), RNS);
        assert!(!node.app.take_rns_restart());
        assert!(node.pending_file().is_none());
        assert_eq!(node.audit()[0]["result"], "failed");
    }

    #[test]
    fn a_confirm_that_cant_be_written_leaves_the_change_on_probation() {
        let mut node = Node::with_rns("rns-confirm-unwritable", RNS);
        let id = node.reach_probation(300);
        // pending.json can be neither replaced nor removed.
        let file = pending::path(&node.app.paths);
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir_all(file.join("x")).unwrap();
        let reply = node.ask(Op::Confirm { id: id.clone() });
        assert_eq!((reply.status, reply.body["error"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("failed")));
        assert_eq!(node.state()["pending"]["state"], "probation");
        assert_eq!(node.audit()[0]["result"], "failed");
        // Once it can be, it is kept.
        std::fs::remove_dir_all(&file).unwrap();
        assert_eq!(node.ask(Op::Confirm { id }).status, StatusCode::OK);
        assert!(node.pending_file().is_none());
    }

    #[test]
    fn turning_things_on_needs_the_flag_and_is_said_loudly() {
        let mut node = Node::with_rns("rns-enable", RNS);
        node.owner.allow_enable = false;
        assert_eq!(node.state()["reticulum_enable_allowed"], false);
        for (path, key) in [(&["reticulum"][..], "enable_transport"), (&["interfaces", "Spare"], "enabled")] {
            let reply = node.apply(300, &[(path, key, "yes")]);
            assert_eq!((reply.status, reply.body["error"].as_str()), (StatusCode::FORBIDDEN, Some("not_allowed")), "{key}");
            assert!(reply.body["message"].as_str().unwrap().contains("--manage-allow-enable"));
        }
        assert_eq!(node.config_text(), RNS);
        // Turning off, and the other keys, need no flag.
        let reply = node.apply(300, &[(&["interfaces", HILL], "mode", "boundary")]);
        assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.body);
        assert!(node.audit()[0].get("notes").is_none(), "nothing turned on");

        let mut node = Node::with_rns("rns-enable-allowed", RNS);
        assert_eq!(node.state()["reticulum_enable_allowed"], true);
        let reply = node.apply(300, &[(&["interfaces", HILL], "mode", "full"), (&["reticulum"], "enable_transport", "yes")]);
        assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.body);
        assert_eq!(reply.body["notes"][0], "This node will relay traffic for others");
        let audit = node.audit();
        assert_eq!(audit[0]["notes"], json!(["This node will relay traffic for others"]));
        assert!(node.app.log.iter().any(|l| l.contains("warning: Remote management: alice turned on from afar")), "{:?}", node.app.log);
        // An interface: what it opens, from values of the shape the view shows.
        node.owner.timing.restart_every = Duration::ZERO;
        node.restarted(&[HILL]);
        node.tick();
        let id = node.state()["pending"]["id"].as_str().unwrap().to_string();
        assert_eq!(node.ask(Op::Confirm { id }).status, StatusCode::OK);
        let reply = node.apply(300, &[(&["interfaces", "Spare"], "enabled", "yes")]);
        assert_eq!(reply.body["notes"][0], "Spare: This interface is turned on", "{}", reply.body);
    }

    #[cfg(unix)]
    #[test]
    fn a_config_that_is_a_link_is_refused() {
        let mut node = Node::with_rns("rns-link", RNS);
        let config = node.app.rns_path();
        let real = config.with_file_name("real-config");
        std::fs::rename(&config, &real).unwrap();
        std::os::unix::fs::symlink(&real, &config).unwrap();
        assert_eq!(node.state()["reticulum"]["editable"], json!([]), "nothing offered that would be refused");
        let reply = node.apply(300, &[(&["interfaces", HILL], "mode", "full")]);
        assert_eq!((reply.status, reply.body["error"].as_str()), (StatusCode::CONFLICT, Some("ambiguous")));
        assert_eq!(reply.body["message"], "The Reticulum config is a link; change it in the terminal");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), RNS);
        assert!(node.pending_file().is_none());
    }

    #[test]
    fn at_most_four_changes_an_hour_and_none_soon_after_a_rollback() {
        let mut node = Node::with_rns("rns-hourly", RNS);
        node.owner.timing.restart_every = Duration::ZERO;
        for mode in ["full", "boundary", "gateway", "roaming"] {
            let reply = node.apply(300, &[(&["interfaces", HILL], "mode", mode)]);
            assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.body);
            node.restarted(&[HILL]);
            node.tick();
            let id = reply.body["id"].as_str().unwrap().to_string();
            assert_eq!(node.ask(Op::Confirm { id }).status, StatusCode::OK);
        }
        let fifth = node.apply(300, &[(&["interfaces", HILL], "mode", "full")]);
        assert_eq!((fifth.status, fifth.body["error"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")));
        assert!(fifth.body["retry_after"].as_u64().unwrap() > 3000, "{}", fifth.body);
        assert!(fifth.body["message"].as_str().unwrap().contains("At most 4"));
        // An hour later (here: none) it goes on.
        node.owner.timing.apply_window = Duration::ZERO;
        assert_eq!(node.apply(300, &[(&["interfaces", HILL], "mode", "full")]).status, StatusCode::ACCEPTED);

        // A rollback holds the next one back for 15 minutes.
        let mut node = Node::with_rns("rns-hold", RNS);
        node.owner.timing.restart_every = Duration::ZERO;
        node.owner.timing.hold_after_rollback = HOLD_AFTER_ROLLBACK;
        let id = node.reach_probation(0);
        node.tick();
        node.restarted(&[HILL]);
        node.tick();
        assert_rolled_back(&mut node, &id, "No confirm by the deadline");
        let held = node.apply(300, &[(&["reticulum"], "enable_transport", "yes")]);
        assert_eq!((held.status, held.body["error"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited")));
        let wait = held.body["retry_after"].as_u64().unwrap();
        assert!((800..=900).contains(&wait), "{wait}");
        assert!(held.body["message"].as_str().unwrap().contains("rolled back by itself"));
    }

    #[test]
    fn the_deadline_is_measured_on_the_monotonic_clock() {
        let mut node = Node::with_rns("rns-monotonic", RNS);
        node.reach_probation(300);
        // The wall clock jumping past the deadline doesn't end it.
        node.owner.pending.as_mut().unwrap().record.confirm_by = Some(0);
        node.tick();
        assert_eq!(node.state()["pending"]["state"], "probation");
        node.owner.pending.as_mut().unwrap().deadline = Some(Instant::now());
        node.tick();
        assert_eq!(node.state()["pending"]["state"], "rolling_back");
    }

    #[test]
    fn a_rollback_that_cant_be_written_is_kept_and_tried_again() {
        let mut node = Node::with_rns("rns-retry", RNS);
        let id = node.reach_probation(0);
        let after = node.config_text();
        // The config can't be replaced for now.
        let blocker = node.app.rns_path().with_file_name(".config.rettui-tmp");
        std::fs::create_dir_all(blocker.join("x")).unwrap();
        node.tick();
        assert_eq!(node.config_text(), after);
        assert!(!node.app.take_rns_restart(), "nothing to restart with yet");
        let state = node.state();
        assert_eq!(state["pending"]["state"], "rolling_back");
        assert!(state["pending"]["problems"][0].as_str().unwrap().contains("couldn't be written back"), "{state}");
        assert!(!state.to_string().contains(".config.rettui-tmp"), "no path from the system: {state}");
        // Saved as being rolled back before the write was tried, and kept.
        assert_eq!(node.pending_file().unwrap().state, State::RollingBack);
        node.tick();
        node.tick();
        let said = node.app.log.iter().filter(|l| l.contains("couldn't be written back")).count();
        assert_eq!(said, 1, "said once: {:?}", node.app.log);
        assert_eq!(node.change(&[("pn_name", "X")]).body["error"], "busy");
        // Once it can be written, it is.
        std::fs::remove_dir_all(&blocker).unwrap();
        node.tick();
        assert_eq!(node.config_text(), RNS);
        node.restarted(&[HILL]);
        node.tick();
        assert_rolled_back(&mut node, &id, "No confirm by the deadline");
    }

    #[test]
    fn a_rollback_left_by_the_start_is_taken_up() {
        let mut node = Node::with_rns("rns-adopt", RNS);
        node.reach_probation(300);
        let mut record = node.pending_file().unwrap();
        record.state = State::RollingBack;
        pending::save(&node.app.paths, &record).unwrap();
        // A new run: the start couldn't write the old config, so it is still to be rolled back.
        node.owner = Owner::with_timing(&node.app.paths, node.owner.timing);
        assert_eq!(node.state()["pending"]["state"], "rolling_back");
        node.tick();
        assert_eq!(node.config_text(), RNS);
        node.restarted(&[HILL]);
        node.tick();
        assert_rolled_back(&mut node, &record.id, "The node stopped before the change was confirmed");
        // Anything else left over isn't taken up (the start deals with it).
        let mut node = Node::with_rns("rns-adopt-not", RNS);
        node.reach_probation(300);
        node.owner = Owner::with_timing(&node.app.paths, node.owner.timing);
        assert_eq!(node.state()["pending"], Value::Null);
    }
}
