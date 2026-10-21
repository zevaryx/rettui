//! Earlier versions of the Reticulum config file, kept in rettui's data
//! folder (`reticulum-history/`) each time rettui saves it: what changed,
//! and the file before rettui first changed it (or as another program left
//! it), so any of them can be compared with another and put back.
//!
//! Each version is a copy of the whole file (`<id>.config`, a few KB);
//! `index.json` lists them, with the config file they're of (rettui can be
//! pointed at another Reticulum config directory) and what was done. The
//! newest [`KEPT`] of each file are kept.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Versions kept of each config file.
pub const KEPT: usize = 50;

const INDEX: &str = "index.json";

/// One saved version of a config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub id: String,
    /// When it was saved (Unix seconds).
    pub time: f64,
    /// The config file it's a version of.
    pub config: PathBuf,
    /// What was done, in a few words ("Removed interface Hilltop").
    pub what: String,
}

pub struct History {
    dir: PathBuf,
}

impl History {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn index(&self) -> Vec<Version> {
        std::fs::read(self.dir.join(INDEX)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    fn write_index(&self, versions: &[Version]) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(versions).map_err(|e| e.to_string())?;
        crate::config::write_atomic(&self.dir.join(INDEX), &json).map_err(|e| format!("Couldn't save the config history: {e}"))
    }

    /// The versions of `config`, newest first.
    pub fn list(&self, config: &Path) -> Vec<Version> {
        let mut versions: Vec<Version> = self.index().into_iter().filter(|v| v.config == config).collect();
        versions.sort_by(|a, b| b.time.total_cmp(&a.time).then_with(|| b.id.cmp(&a.id)));
        versions
    }

    /// A version's text.
    pub fn text(&self, id: &str) -> Result<String, String> {
        // Ids are made here, but come back from the web UI too.
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return Err("There's no such version".into());
        }
        std::fs::read_to_string(self.dir.join(format!("{id}.config"))).map_err(|_| "There's no such version".to_string())
    }

    /// Keep `text` as a version of `config`, unless it's the newest kept
    /// already. Older ones beyond [`KEPT`] go.
    pub fn record(&self, config: &Path, text: &str, what: &str, time: f64) -> Result<(), String> {
        let newest = self.list(config).into_iter().next();
        if newest.is_some_and(|v| self.text(&v.id).is_ok_and(|kept| kept == text)) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("Couldn't make {}: {e}", self.dir.display()))?;
        let mut id = format!("{}", (time * 1000.0) as u64);
        let mut n = 1;
        while self.dir.join(format!("{id}.config")).exists() {
            id = format!("{}-{n}", (time * 1000.0) as u64);
            n += 1;
        }
        crate::config::write_atomic(&self.dir.join(format!("{id}.config")), text.as_bytes())
            .map_err(|e| format!("Couldn't keep this version of the config: {e}"))?;
        let mut index = self.index();
        index.push(Version { id, time, config: config.to_path_buf(), what: what.to_string() });
        // The oldest of this file's beyond what's kept.
        let mut mine: Vec<&Version> = index.iter().filter(|v| v.config == config).collect();
        mine.sort_by(|a, b| b.time.total_cmp(&a.time).then_with(|| b.id.cmp(&a.id)));
        let gone: Vec<String> = mine.iter().skip(KEPT).map(|v| v.id.clone()).collect();
        for id in &gone {
            let _ = std::fs::remove_file(self.dir.join(format!("{id}.config")));
        }
        index.retain(|v| !gone.contains(&v.id));
        self.write_index(&index)
    }

    /// Before rettui saves `config`: the file as it is now (`current`, if
    /// it exists) is kept first if it isn't the newest version here, as it
    /// was before rettui first changed it, or as another program left it.
    pub fn before_saving(&self, config: &Path, current: Option<&str>, time: f64) -> Result<(), String> {
        let Some(current) = current else { return Ok(()) };
        let what = if self.list(config).is_empty() { "As it was before rettui changed it" } else { "Changed outside rettui" };
        self.record(config, current, what, time)
    }
}

/// A line of a comparison of two versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub enum DiffLine {
    Same(String),
    Added(String),
    Removed(String),
    /// Unchanged lines left out, how many.
    Skipped(usize),
}

/// Files longer than this are compared as all removed, then all added
/// (comparing line by line takes their lengths multiplied).
const LONGEST_COMPARED: usize = 5000;

/// How `old` became `new`, line by line: what's the same, removed and
/// added, with only `context` unchanged lines kept around each change
/// (the rest [`DiffLine::Skipped`]).
pub fn diff(old: &str, new: &str, context: usize) -> Vec<DiffLine> {
    let (a, b): (Vec<&str>, Vec<&str>) = (lines(old), lines(new));
    let mut out = Vec::new();
    if a.len() > LONGEST_COMPARED || b.len() > LONGEST_COMPARED {
        out.extend(a.iter().map(|l| DiffLine::Removed(l.to_string())));
        out.extend(b.iter().map(|l| DiffLine::Added(l.to_string())));
        return out;
    }
    // The longest run in common, from each pair of positions to the end.
    let (n, m) = (a.len(), b.len());
    let mut common = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            common[at(i, j)] = if a[i] == b[j] { common[at(i + 1, j + 1)] + 1 } else { common[at(i + 1, j)].max(common[at(i, j + 1)]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push(DiffLine::Same(a[i].to_string()));
            (i, j) = (i + 1, j + 1);
        } else if i < n && (j == m || common[at(i + 1, j)] >= common[at(i, j + 1)]) {
            out.push(DiffLine::Removed(a[i].to_string()));
            i += 1;
        } else {
            out.push(DiffLine::Added(b[j].to_string()));
            j += 1;
        }
    }
    trim_context(out, context)
}

fn lines(text: &str) -> Vec<&str> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() { Vec::new() } else { text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect() }
}

/// Unchanged lines more than `context` from a change, left out.
fn trim_context(lines: Vec<DiffLine>, context: usize) -> Vec<DiffLine> {
    let changed: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| !matches!(l, DiffLine::Same(_))).map(|(i, _)| i).collect();
    let near = |i: usize| changed.iter().any(|&c| c.abs_diff(i) <= context);
    let mut out = Vec::new();
    let mut skipped = 0;
    for (i, line) in lines.into_iter().enumerate() {
        if matches!(line, DiffLine::Same(_)) && !near(i) {
            skipped += 1;
            continue;
        }
        if skipped > 0 {
            out.push(DiffLine::Skipped(skipped));
            skipped = 0;
        }
        out.push(line);
    }
    if skipped > 0 {
        out.push(DiffLine::Skipped(skipped));
    }
    out
}

/// How many lines were added and removed.
pub fn counts(lines: &[DiffLine]) -> (usize, usize) {
    let added = lines.iter().filter(|l| matches!(l, DiffLine::Added(_))).count();
    let removed = lines.iter().filter(|l| matches!(l, DiffLine::Removed(_))).count();
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-rns-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn versions_kept_newest_first_without_repeats_and_pruned() {
        let dir = dir("keep");
        let history = History::new(dir.clone());
        let (config, other) = (Path::new("/a/config"), Path::new("/b/config"));
        history.before_saving(config, Some("one"), 1.0).unwrap();
        history.record(config, "two", "Set x", 2.0).unwrap();
        // The same again: not kept twice.
        history.record(config, "two", "Set x again", 3.0).unwrap();
        history.before_saving(config, Some("two"), 4.0).unwrap();
        history.record(other, "elsewhere", "Set y", 5.0).unwrap();
        // Edited outside rettui, then saved here.
        history.before_saving(config, Some("three"), 6.0).unwrap();
        history.record(config, "four", "Set z", 7.0).unwrap();
        let listed: Vec<(String, String)> = history.list(config).iter().map(|v| (v.what.clone(), history.text(&v.id).unwrap())).collect();
        assert_eq!(
            listed,
            [
                ("Set z".to_string(), "four".to_string()),
                ("Changed outside rettui".into(), "three".into()),
                ("Set x".into(), "two".into()),
                ("As it was before rettui changed it".into(), "one".into()),
            ]
        );
        assert_eq!(history.list(other).len(), 1);
        assert!(history.text("../index").is_err() && history.text("").is_err() && history.text("123").is_err());
        // Only the newest KEPT stay, and their files with them.
        for n in 0..KEPT + 5 {
            history.record(config, &format!("v{n}"), "Set", 10.0 + n as f64).unwrap();
        }
        assert_eq!(history.list(config).len(), KEPT);
        assert_eq!(history.text(&history.list(config).last().unwrap().id).unwrap(), "v5");
        let files = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(files, KEPT + 1 + 1, "this file's, the other's, and the index");
        // Saved in the same millisecond: both kept.
        history.record(other, "again", "Set", 5.0).unwrap();
        assert_eq!(history.list(other).len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn differences_line_by_line_with_context() {
        let old = "[reticulum]\n  share_instance = Yes\n\n[interfaces]\n  [[Default]]\n    type = AutoInterface\n    enabled = yes\n";
        let new = "[reticulum]\n  share_instance = Yes\n\n[interfaces]\n  [[Default]]\n    type = AutoInterface\n    enabled = no\n  [[Hill]]\n    type = TCPClientInterface\n";
        let lines = diff(old, new, 1);
        assert_eq!(
            lines,
            [
                DiffLine::Skipped(5),
                DiffLine::Same("    type = AutoInterface".into()),
                DiffLine::Removed("    enabled = yes".into()),
                DiffLine::Added("    enabled = no".into()),
                DiffLine::Added("  [[Hill]]".into()),
                DiffLine::Added("    type = TCPClientInterface".into()),
            ]
        );
        assert_eq!(counts(&lines), (3, 1));
        assert_eq!(diff("same\n", "same", 3), [DiffLine::Skipped(1)]);
        assert_eq!(diff("a\nb\nc\nd\ne", "a\nb\nc\nd\ne", 1), [DiffLine::Skipped(5)]);
        assert_eq!(diff("", "x", 3), [DiffLine::Added("x".into())]);
        // Windows line endings are the same lines.
        assert_eq!(diff("x\r\ny", "x\ny", 3), [DiffLine::Skipped(2)]);
        // A change at the end: what's after it isn't skipped.
        assert_eq!(diff("a\nb\nc", "a\nb\nC", 0), [DiffLine::Skipped(2), DiffLine::Removed("c".into()), DiffLine::Added("C".into())]);
        let long = "x\n".repeat(LONGEST_COMPARED + 1);
        assert_eq!(counts(&diff(&long, "y", 3)), (1, LONGEST_COMPARED + 1));
    }
}
