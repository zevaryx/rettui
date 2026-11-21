//! What remote management did, kept in `manage/audit.jsonl` in the data
//! directory: one JSON line per operation, with who asked (the website's
//! user), what changed from what to what, and how it ended. It holds the
//! last 2000 entries (fewer if they come to more than 2 MiB), is readable by
//! the node's user only, and is written whole each time, so a crash leaves
//! the old file or the new one. A settings change is rolled back from its
//! entry's `from` values, so entries that worked keep their values whole,
//! and refusals are the first to go when the log is full. Bursts of bad
//! signatures in a row share one entry an hour, so whoever can reach the
//! port can't push the rest out.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The most entries kept, and the most bytes they may take together (the
/// oldest go first; the newest is always kept).
pub const MAX_ENTRIES: usize = 2000;
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// The longest error kept, and the longest value kept for a change that
/// was refused or failed (it was never applied, so it's only a reminder).
const MAX_ERROR: usize = 300;
const MAX_REFUSED_VALUE: usize = 256;
/// How long an `auth_failures` entry takes in more bursts, in seconds.
const AUTH_FAILURES_WITHIN: u64 = 3600;
const AUTH_FAILURES: &str = "auth_failures";
/// The most bytes a value takes in an `/audit` answer, as JSON. A rollback
/// reads the whole value from the log, not from the answer.
const MAX_PAGE_VALUE: usize = 200;

/// One setting changed: its key, and its value before and after, as the
/// settings editors show them. A Reticulum config change has the section
/// it is in too (`["interfaces", "Name"]`), and its old value only if the
/// redacted view would show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
    pub key: String,
    pub from: String,
    pub to: String,
}

/// One line of the audit log, as `/audit` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    /// Unix seconds.
    pub time: u64,
    /// The change's id (16 hex characters), which `/rollback` takes.
    pub id: String,
    /// The website's user, or `-` for what nobody asked for (failed signatures).
    pub actor: String,
    /// `settings`, `restart`, `rollback`, `reticulum`, `confirm`,
    /// `auto_rollback` or `auth_failures`.
    pub op: String,
    #[serde(default)]
    pub changes: Vec<Change>,
    /// `ok`, `refused`, `failed` or `rolled_back`.
    pub result: String,
    #[serde(default)]
    pub error: Option<String>,
    /// For `auth_failures`: how many bad signatures it counts, in bursts
    /// from `time` on, while no other entry came between.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    /// What a Reticulum config change turned on, a sentence each (such as
    /// what it opens to others).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// How an operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Refused,
    Failed,
    /// A Reticulum config change that wasn't confirmed in time, put back.
    RolledBack,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Refused => "refused",
            Outcome::Failed => "failed",
            Outcome::RolledBack => "rolled_back",
        }
    }
}

/// What to record (the sequence number and time are added).
#[derive(Debug, Clone)]
pub struct Record<'a> {
    pub id: &'a str,
    pub actor: &'a str,
    pub op: &'static str,
    pub changes: Vec<Change>,
    pub outcome: Outcome,
    pub error: Option<&'a str>,
}

fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

/// The audit log, in memory and on disk.
#[derive(Debug)]
pub struct Audit {
    path: PathBuf,
    /// Each entry with its line in the file.
    entries: VecDeque<(Entry, String)>,
    bytes: usize,
    next_seq: u64,
    max_entries: usize,
    max_bytes: usize,
}

impl Audit {
    /// The log at `path`, as far as it can be read: a line that can't be is
    /// left out, and a file far bigger than the log ever is starts a new one.
    pub fn load(path: PathBuf) -> Self {
        Self::with_limits(path, MAX_ENTRIES, MAX_BYTES)
    }

    fn with_limits(path: PathBuf, max_entries: usize, max_bytes: usize) -> Self {
        let mut audit = Self { path, entries: VecDeque::new(), bytes: 0, next_seq: 1, max_entries, max_bytes };
        let text = match crate::config::read_limited_text(&audit.path, (max_bytes as u64).saturating_mul(2)) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return audit,
            Err(e) => {
                tracing::warn!("Remote management: the audit log can't be read, so a new one is started: {e}");
                return audit;
            }
        };
        for line in text.lines() {
            if let Ok(entry) = serde_json::from_str::<Entry>(line) {
                audit.next_seq = audit.next_seq.max(entry.seq.saturating_add(1));
                audit.push(entry, format!("{line}\n"));
            }
        }
        audit
    }

    fn push(&mut self, entry: Entry, line: String) {
        self.bytes += line.len();
        self.entries.push_back((entry, line));
        self.trim();
    }

    /// Drop entries until the log is within its limits, the oldest refusal
    /// first (it changed nothing, and a flood of them mustn't push out the
    /// changes a rollback needs), then the oldest of any. The newest is
    /// always kept.
    fn trim(&mut self) {
        while self.entries.len() > self.max_entries || (self.bytes > self.max_bytes && self.entries.len() > 1) {
            let older = self.entries.len().saturating_sub(1);
            let at = self.entries.iter().take(older).position(|(entry, _)| entry.result == "refused").unwrap_or(0);
            if let Some((_, line)) = self.entries.remove(at) {
                self.bytes -= line.len();
            }
        }
    }

    /// Add an entry and save the log. The entry is kept even if saving fails
    /// (the error says why), as what it records has happened.
    pub fn record(&mut self, record: Record<'_>, now: u64) -> Result<&Entry, String> {
        self.record_noted(record, Vec::new(), now)
    }

    /// [`Audit::record`], with `notes` on what it turned on (at most 8, each
    /// cut to the length of an error).
    pub fn record_noted(&mut self, record: Record<'_>, notes: Vec<String>, now: u64) -> Result<&Entry, String> {
        let mut changes = record.changes;
        if record.outcome != Outcome::Ok {
            for change in &mut changes {
                change.from = cut(&change.from, MAX_REFUSED_VALUE);
                change.to = cut(&change.to, MAX_REFUSED_VALUE);
            }
        }
        let entry = Entry {
            seq: self.next_seq,
            time: now,
            id: record.id.to_string(),
            actor: record.actor.to_string(),
            op: record.op.to_string(),
            changes,
            result: record.outcome.as_str().to_string(),
            error: record.error.map(|e| cut(e, MAX_ERROR)),
            count: None,
            notes: notes.iter().take(8).map(|note| cut(note, MAX_ERROR)).collect(),
        };
        self.add(entry)
    }

    fn add(&mut self, entry: Entry) -> Result<&Entry, String> {
        self.next_seq += 1;
        let line = line_of(&entry)?;
        self.push(entry, line);
        self.saved_newest()
    }

    /// Save the log; the newest entry, or why it couldn't be saved.
    fn saved_newest(&self) -> Result<&Entry, String> {
        let saved = self.save();
        let entry = self.entries.back().map(|(entry, _)| entry).ok_or("the audit log is empty")?;
        saved.map(|()| entry)
    }

    /// Record a burst of `count` bad signatures. If the newest entry is a
    /// burst too, less than an hour old, it counts this one as well instead,
    /// so a flood takes one entry an hour. A new entry gets `id`.
    pub fn auth_failures(&mut self, id: &str, count: u64, now: u64) -> Result<&Entry, String> {
        let newest =
            self.entries.back_mut().filter(|(entry, _)| entry.op == AUTH_FAILURES && now.saturating_sub(entry.time) < AUTH_FAILURES_WITHIN);
        if let Some((entry, line)) = newest {
            let total = entry.count.unwrap_or(0).saturating_add(count);
            entry.count = Some(total);
            entry.error = Some(auth_failures_text(total));
            let new_line = line_of(entry)?;
            self.bytes = self.bytes - line.len() + new_line.len();
            *line = new_line;
            self.trim();
            return self.saved_newest();
        }
        let entry = Entry {
            seq: self.next_seq,
            time: now,
            id: id.to_string(),
            actor: "-".to_string(),
            op: AUTH_FAILURES.to_string(),
            changes: Vec::new(),
            result: Outcome::Refused.as_str().to_string(),
            error: Some(auth_failures_text(count)),
            count: Some(count),
            notes: Vec::new(),
        };
        self.add(entry)
    }

    fn save(&self) -> Result<(), String> {
        let text: String = self.entries.iter().map(|(_, line)| line.as_str()).collect();
        if let Some(dir) = self.path.parent() {
            make_private_dir(dir).map_err(|e| format!("Couldn't save the audit log: {e}"))?;
        }
        crate::config::write_private(&self.path, text.as_bytes()).map_err(|e| format!("Couldn't save the audit log: {e:#}"))
    }

    /// Up to `limit` entries older than sequence number `before` (all if
    /// none), newest first, and whether there are older ones still.
    pub fn page(&self, before: Option<u64>, limit: usize) -> (Vec<&Entry>, bool) {
        let mut older = self.entries.iter().rev().map(|(entry, _)| entry).filter(|e| before.is_none_or(|before| e.seq < before));
        let page: Vec<&Entry> = older.by_ref().take(limit).collect();
        let more = older.next().is_some();
        (page, more)
    }

    /// Like [`Audit::page`], for an answer: each value cut to
    /// [`MAX_PAGE_VALUE`] bytes of JSON, and no more entries than fit in
    /// `max_bytes` of JSON (at least one, which is well under it), so a few
    /// long greetings can't make a page bigger than a website takes. Whether
    /// there are older ones still, including those that didn't fit.
    pub fn page_within(&self, before: Option<u64>, limit: usize, max_bytes: usize) -> (Vec<Entry>, bool) {
        let (entries, more) = self.page(before, limit);
        let mut page = Vec::with_capacity(entries.len());
        let mut used = 0;
        for entry in entries {
            let mut shown = entry.clone();
            for change in &mut shown.changes {
                change.from = cut_json(&change.from, MAX_PAGE_VALUE);
                change.to = cut_json(&change.to, MAX_PAGE_VALUE);
            }
            let size = serde_json::to_string(&shown).map_or(max_bytes, |line| line.len() + 1);
            if !page.is_empty() && used + size > max_bytes {
                return (page, true);
            }
            used += size;
            page.push(shown);
        }
        (page, more)
    }

    /// The settings change (or rollback) with this id that was applied.
    pub fn applied_change(&self, id: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .rev()
            .map(|(entry, _)| entry)
            .find(|e| e.id == id && e.result == "ok" && (e.op == "settings" || e.op == "rollback"))
    }

    /// Whether any entry has this id.
    pub fn has(&self, id: &str) -> bool {
        self.entries.iter().any(|(entry, _)| entry.id == id)
    }
}

/// How many bytes `c` takes in a JSON string (as serde_json writes it).
fn json_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if u32::from(c) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// `text`, cut (with an ellipsis) to take at most `max` bytes in a JSON string.
fn cut_json(text: &str, max: usize) -> String {
    if text.chars().map(json_len).sum::<usize>() <= max {
        return text.to_string();
    }
    let room = max.saturating_sub('…'.len_utf8());
    let mut used = 0;
    let mut cut: String = text
        .chars()
        .take_while(|c| {
            used += json_len(*c);
            used <= room
        })
        .collect();
    cut.push('…');
    cut
}

fn line_of(entry: &Entry) -> Result<String, String> {
    serde_json::to_string(entry).map(|line| line + "\n").map_err(|e| format!("the audit entry couldn't be written: {e}"))
}

fn auth_failures_text(count: u64) -> String {
    format!(
        "{count} requests with a bad signature since then: after each 20 within a minute, requests without a valid signature are refused for 60 seconds"
    )
}

/// Make `dir`, readable by its owner only (on Unix), as the data directory is.
pub(super) fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    if !dir.is_dir() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-manage-audit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record<'a>(id: &'a str, outcome: Outcome, changes: Vec<Change>) -> Record<'a> {
        Record { id, actor: "alice", op: "settings", changes, outcome, error: None }
    }

    fn change(key: &str, from: &str, to: &str) -> Change {
        Change { path: vec![], key: key.into(), from: from.into(), to: to.into() }
    }

    #[test]
    fn entries_are_saved_numbered_and_read_back() {
        let dir = test_dir("saved");
        let path = dir.join("manage/audit.jsonl");
        let mut audit = Audit::load(path.clone());
        assert_eq!(audit.page(None, 50), (vec![], false));
        let first = audit.record(record("a1", Outcome::Ok, vec![change("pn_name", "", "Hilltop")]), 100).unwrap().clone();
        assert_eq!((first.seq, first.time, first.result.as_str(), first.error.as_deref()), (1, 100, "ok", None));
        audit.record(Record { error: Some("no"), ..record("a2", Outcome::Refused, vec![]) }, 101).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        // Read back after a restart, numbering on from the last.
        let mut again = Audit::load(path.clone());
        let (page, more) = again.page(None, 50);
        assert_eq!(page.iter().map(|e| e.seq).collect::<Vec<_>>(), [2, 1], "newest first");
        assert!(!more);
        assert_eq!(page[1], &first);
        assert_eq!(again.record(record("a3", Outcome::Failed, vec![]), 102).unwrap().seq, 3);
        assert_eq!(again.applied_change("a1").map(|e| e.seq), Some(1));
        assert!(again.applied_change("a2").is_none(), "refused");
        assert!(again.applied_change("a3").is_none(), "failed");
        assert!(again.has("a2") && !again.has("zz"));
        // A line that can't be read is left out; the rest stay.
        std::fs::write(&path, format!("not json\n{}{{\"seq\":9}}\n", text.lines().next().unwrap().to_owned() + "\n")).unwrap();
        let read = Audit::load(path.clone());
        assert_eq!(read.page(None, 50).0.len(), 1);
        assert_eq!(read.next_seq, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pages_go_back_from_before() {
        let dir = test_dir("pages");
        let mut audit = Audit::load(dir.join("audit.jsonl"));
        for n in 0..7 {
            audit.record(record(&format!("{n:016x}"), Outcome::Ok, vec![]), n).unwrap();
        }
        let seqs = |(page, more): (Vec<&Entry>, bool)| (page.iter().map(|e| e.seq).collect::<Vec<_>>(), more);
        assert_eq!(seqs(audit.page(None, 3)), (vec![7, 6, 5], true));
        assert_eq!(seqs(audit.page(Some(5), 3)), (vec![4, 3, 2], true));
        assert_eq!(seqs(audit.page(Some(2), 3)), (vec![1], false));
        assert_eq!(seqs(audit.page(Some(1), 3)), (vec![], false));
        assert_eq!(seqs(audit.page(Some(4), 3)), (vec![3, 2, 1], false));
        assert_eq!(seqs(audit.page(Some(u64::MAX), 100)).0.len(), 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_is_capped_by_entries_and_bytes() {
        let dir = test_dir("cap");
        let path = dir.join("audit.jsonl");
        let mut audit = Audit::with_limits(path.clone(), 5, 100_000);
        for n in 0..12 {
            audit.record(record("x", Outcome::Ok, vec![]), n).unwrap();
        }
        assert_eq!(audit.page(None, 100).0.iter().map(|e| e.seq).collect::<Vec<_>>(), [12, 11, 10, 9, 8]);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 5);
        // By bytes: big entries push old ones out, but the newest is kept.
        let mut audit = Audit::with_limits(dir.join("bytes.jsonl"), 100, 3000);
        let big = "v".repeat(1000);
        for _ in 0..5 {
            audit.record(record("x", Outcome::Ok, vec![change("hub_greeting", &big, &big)]), 1).unwrap();
        }
        assert_eq!(audit.page(None, 100).0.len(), 1);
        assert!(audit.bytes <= 3000 || audit.entries.len() == 1);
        // The real limit holds 2000 ordinary entries.
        const { assert!(MAX_ENTRIES * 400 <= MAX_BYTES, "an ordinary entry is well under 400 bytes") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refusals_are_dropped_before_changes_when_the_log_is_full() {
        let dir = test_dir("trim-refused");
        let mut audit = Audit::with_limits(dir.join("audit.jsonl"), 5, 100_000);
        for n in 0..3 {
            audit.record(record(&format!("ok{n}"), Outcome::Ok, vec![]), n).unwrap();
        }
        for n in 0..10 {
            audit.record(record(&format!("no{n}"), Outcome::Refused, vec![]), 10 + n).unwrap();
        }
        let ids = |audit: &Audit| audit.page(None, 100).0.iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        // The changes stay (a rollback needs them); the newest refusals with them.
        assert_eq!(ids(&audit), ["no9", "no8", "ok2", "ok1", "ok0"]);
        assert!(audit.applied_change("ok0").is_some());
        // Bad-signature bursts are refusals too.
        for n in 0..4 {
            audit.auth_failures(&format!("af{n}"), 20, 100 + n * AUTH_FAILURES_WITHIN).unwrap();
        }
        assert_eq!(ids(&audit), ["af3", "af2", "ok2", "ok1", "ok0"]);
        // With only changes left, the oldest goes.
        audit.record(record("ok3", Outcome::Ok, vec![]), 200_000).unwrap();
        audit.record(record("ok4", Outcome::Ok, vec![]), 200_001).unwrap();
        audit.record(record("ok5", Outcome::Ok, vec![]), 200_002).unwrap();
        assert_eq!(ids(&audit), ["ok5", "ok4", "ok3", "ok2", "ok1"]);
        // A failed change is kept like one that worked.
        audit.record(record("f", Outcome::Failed, vec![]), 200_003).unwrap();
        assert_eq!(ids(&audit), ["f", "ok5", "ok4", "ok3", "ok2"]);
        // By bytes too: a big refusal is the one pushed out.
        let mut audit = Audit::with_limits(dir.join("bytes.jsonl"), 100, 1500);
        audit.record(record("ok", Outcome::Ok, vec![change("hub_greeting", "a", "b")]), 1).unwrap();
        let big = "v".repeat(200);
        for n in 0..5 {
            audit.record(record(&format!("no{n}"), Outcome::Refused, vec![change("hub_greeting", &big, &big)]), 2).unwrap();
        }
        let kept = ids(&audit);
        assert_eq!(kept.last().map(String::as_str), Some("ok"), "{kept:?}");
        assert_eq!(kept.first().map(String::as_str), Some("no4"), "{kept:?}");
        assert!(audit.bytes <= 1500, "{}", audit.bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bursts_of_bad_signatures_in_a_row_share_an_entry_an_hour() {
        let dir = test_dir("bursts");
        let path = dir.join("audit.jsonl");
        let mut audit = Audit::load(path.clone());
        let first = audit.auth_failures("a1", 20, 1000).unwrap().clone();
        assert_eq!(
            (first.op.as_str(), first.actor.as_str(), first.result.as_str(), first.count),
            ("auth_failures", "-", "refused", Some(20))
        );
        assert!(first.error.as_deref().unwrap().starts_with("20 requests"));
        // More bursts within the hour add to it, even after a restart.
        audit.auth_failures("a2", 20, 1000 + 1800).unwrap();
        let mut audit = Audit::load(path.clone());
        let same = audit.auth_failures("a3", 20, 1000 + AUTH_FAILURES_WITHIN - 1).unwrap().clone();
        assert_eq!((same.seq, same.id.as_str(), same.time, same.count), (1, "a1", 1000, Some(60)));
        assert!(same.error.as_deref().unwrap().starts_with("60 requests"));
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        assert_eq!(audit.bytes, std::fs::read_to_string(&path).unwrap().len(), "the bytes follow the rewritten line");
        // An hour on, a new entry.
        let next = audit.auth_failures("a4", 20, 1000 + AUTH_FAILURES_WITHIN).unwrap().clone();
        assert_eq!((next.seq, next.count), (2, Some(20)));
        // Another entry between them starts a new one too.
        audit.record(record("ok", Outcome::Ok, vec![]), 5000).unwrap();
        assert_eq!(audit.auth_failures("a5", 20, 5001).unwrap().seq, 4);
        assert_eq!(audit.page(None, 100).0.len(), 4);
        // An ordinary entry has no count in its line.
        assert!(!audit.entries.iter().any(|(e, line)| e.op != "auth_failures" && line.contains("count")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_change_keeps_only_the_start_of_its_values() {
        let dir = test_dir("refused");
        let mut audit = Audit::load(dir.join("audit.jsonl"));
        let long = "é".repeat(4000);
        let error = "e".repeat(1000);
        let entry = audit
            .record(Record { error: Some(&error), ..record("r", Outcome::Refused, vec![change("hub_greeting", "old", &long)]) }, 1)
            .unwrap();
        assert_eq!(entry.changes[0].to.chars().count(), MAX_REFUSED_VALUE + 1, "cut, with an ellipsis");
        assert_eq!(entry.error.as_ref().unwrap().chars().count(), MAX_ERROR + 1);
        // One that was applied keeps its values whole, to roll back from.
        let entry = audit.record(record("o", Outcome::Ok, vec![change("hub_greeting", "old", &long)]), 1).unwrap();
        assert_eq!(entry.changes[0].to, long);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_for_an_answer_is_kept_to_a_byte_budget() {
        let dir = test_dir("budget");
        let mut audit = Audit::load(dir.join("audit.jsonl"));
        // Long greetings saved over and over: each entry keeps them whole.
        let long = "g".repeat(4096);
        for n in 0..50 {
            audit.record(record(&format!("{n:016x}"), Outcome::Ok, vec![change("hub_greeting", &long, &long)]), n).unwrap();
        }
        let whole = serde_json::to_string(&audit.page(None, 50).0).unwrap();
        assert!(whole.len() > 64 * 1024, "the page as kept is too big for a website: {}", whole.len());
        let (page, more) = audit.page_within(None, 50, 48 * 1024);
        let shown = serde_json::to_string(&page).unwrap();
        assert!(shown.len() <= 48 * 1024, "{}", shown.len());
        assert!(page.len() == 50 && !more, "cut values make all 50 fit: {}", page.len());
        assert!(page.iter().all(|e| e.changes.iter().all(|c| c.to.len() <= MAX_PAGE_VALUE && c.to.ends_with('…'))));
        // The log itself keeps the whole value, for a rollback.
        assert_eq!(audit.applied_change(&format!("{:016x}", 3)).unwrap().changes[0].from, long);
        // Too many to fit: fewer, newest first, and `more` says so; the next page goes on from there.
        let (page, more) = audit.page_within(None, 50, 8 * 1024);
        assert!(more && !page.is_empty() && page.len() < 50, "{}", page.len());
        assert!(serde_json::to_string(&page).unwrap().len() <= 8 * 1024);
        let last = page.last().unwrap().seq;
        let (next, _) = audit.page_within(Some(last), 50, 8 * 1024);
        assert_eq!(next[0].seq, last - 1);
        // One entry is always given, even over the budget.
        assert_eq!(audit.page_within(None, 50, 10).0.len(), 1);
        // The end of the log: no more.
        assert_eq!(audit.page_within(Some(2), 50, 48 * 1024), (vec![page_entry(&audit, 1)], false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn page_entry(audit: &Audit, seq: u64) -> Entry {
        let mut entry = audit.page(Some(seq + 1), 1).0[0].clone();
        for change in &mut entry.changes {
            change.from = cut_json(&change.from, MAX_PAGE_VALUE);
            change.to = cut_json(&change.to, MAX_PAGE_VALUE);
        }
        entry
    }

    #[test]
    fn values_are_cut_by_their_size_in_json() {
        assert_eq!(cut_json("short", 200), "short");
        assert_eq!(cut_json("", 0), "");
        let cut = cut_json(&"a".repeat(300), 200);
        assert_eq!((cut.len(), cut.ends_with('…')), (200, true));
        // Escapes count as what they take in JSON.
        for text in ["\u{1}".repeat(300), "\"".repeat(300), "é".repeat(300), "😀".repeat(300)] {
            let cut = cut_json(&text, 200);
            let json = serde_json::to_string(&cut).unwrap();
            assert!(json.len() - 2 <= 200, "{} for {:?}", json.len(), &text[..4]);
            assert!(cut.ends_with('…'));
        }
    }

    #[test]
    fn a_log_that_cant_be_saved_still_keeps_the_entry() {
        let dir = test_dir("unsaved");
        // Its folder is a file, so it can't be made.
        std::fs::write(dir.join("manage"), "x").unwrap();
        let mut audit = Audit::load(dir.join("manage/audit.jsonl"));
        let error = audit.record(record("a", Outcome::Ok, vec![]), 1).unwrap_err();
        assert!(error.contains("Couldn't save the audit log"), "{error}");
        assert!(audit.has("a"));
        // A log too big to be one of ours starts again.
        let path = dir.join("huge.jsonl");
        std::fs::write(&path, "x".repeat(10_000)).unwrap();
        assert!(Audit::with_limits(path, 10, 1000).entries.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reticulum_change_keeps_its_path_and_older_lines_still_read() {
        let dir = test_dir("path");
        let path = dir.join("audit.jsonl");
        let mut audit = Audit::load(path.clone());
        let hill = Change { path: vec!["interfaces".into(), "Hill".into()], key: "enabled".into(), from: "yes".into(), to: "no".into() };
        audit.record(Record { op: "reticulum", ..record("r", Outcome::Ok, vec![hill.clone()]) }, 1).unwrap();
        let rolled = Record {
            op: "auto_rollback",
            error: Some("No confirm by the deadline"),
            ..record("r", Outcome::RolledBack, vec![hill.clone()])
        };
        audit.record(rolled, 2).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#""path":["interfaces","Hill"]"#), "{text}");
        assert!(text.contains(r#""result":"rolled_back""#), "{text}");
        // A settings change has no path in its line, and a line without one reads.
        audit.record(record("s", Outcome::Ok, vec![change("pn_name", "", "x")]), 3).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().lines().last().unwrap().contains("path"));
        let read = Audit::load(path);
        assert_eq!(read.page(None, 10).0[2].changes[0], hill);
        assert!(read.page(None, 10).0[0].changes[0].path.is_empty());
        // A Reticulum change isn't a settings change a rollback can undo.
        assert!(read.applied_change("r").is_none() && read.has("r"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
