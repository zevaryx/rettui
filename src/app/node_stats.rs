//! What the node hosted here has served, as NomadNet counts it: pages,
//! files and pictures, kept across restarts (the node's own counts start
//! again each time it starts) until reset (`R` in the Node tab).

use std::path::Path;

use nomad_core::NomadServeStats;
use serde::{Deserialize, Serialize};

use super::{App, now};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Served {
    /// Counted since (Unix seconds).
    #[serde(default)]
    pub since: f64,
    /// Every request, found or not.
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub pages: u64,
    #[serde(default)]
    pub files: u64,
    /// Pictures shown in pages.
    #[serde(default)]
    pub media: u64,
    #[serde(default)]
    pub not_found: u64,
    /// When the last request came.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<f64>,
}

impl Served {
    /// The counts saved, or none since now.
    pub fn load(path: &Path) -> Self {
        let read = std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<Served>(&bytes).ok());
        read.unwrap_or_else(|| Served { since: now(), ..Served::default() })
    }

    /// Add what the node served between its counts `before` and `after`
    /// (from nothing, if it started again since), at `at`. Whether that
    /// was anything.
    pub fn add(&mut self, before: Option<&NomadServeStats>, after: &NomadServeStats, at: f64) -> bool {
        let zero = NomadServeStats::default();
        let before = before.filter(|b| b.request_count <= after.request_count).unwrap_or(&zero);
        let requests = after.request_count - before.request_count;
        if requests == 0 {
            return false;
        }
        self.requests += requests;
        self.pages += after.page_hits.saturating_sub(before.page_hits);
        self.files += after.file_hits.saturating_sub(before.file_hits);
        self.media += after.media_hits.saturating_sub(before.media_hits);
        self.not_found += after.not_found_count.saturating_sub(before.not_found_count);
        self.last = Some(at);
        true
    }

    /// "12 pages · 3 files · 4 pictures".
    pub fn label(&self) -> String {
        let count = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = vec![count(self.pages, "page", "pages"), count(self.files, "file", "files")];
        if self.media > 0 {
            parts.push(count(self.media, "picture", "pictures"));
        }
        parts.join(" · ")
    }
}

impl App {
    pub(super) fn on_host_stats(&mut self, stats: NomadServeStats) {
        // Counts from a node stopped since, still on their way, aren't
        // counted again: only the running node's are.
        if self.node.status != super::node::NodeStatus::Running {
            return;
        }
        if self.node.served.add(self.node.stats.as_ref(), &stats, now()) {
            self.node.served_dirty = true;
        }
        self.node.stats = Some(stats);
    }

    /// Start counting again from now.
    pub fn reset_node_stats(&mut self) {
        self.node.served = Served { since: now(), ..Served::default() };
        self.node.served_dirty = true;
        self.log("The node's statistics were reset");
    }

    /// Queue a save of the counts, if they changed.
    pub(super) fn save_node_stats(&mut self) {
        if std::mem::take(&mut self.node.served_dirty) {
            let served = self.node.served.clone();
            self.saver.write(self.paths.node_stats.clone(), "node statistics", move || Ok(serde_json::to_vec(&served)?));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(requests: u64, pages: u64, files: u64, media: u64, not_found: u64) -> NomadServeStats {
        NomadServeStats {
            request_count: requests,
            page_hits: pages,
            file_hits: files,
            media_hits: media,
            not_found_count: not_found,
            last_request_ms: None,
        }
    }

    #[test]
    fn counts_go_on_across_the_node_starting_again() {
        let mut served = Served { since: 1.0, ..Served::default() };
        assert!(served.add(None, &stats(5, 3, 1, 0, 1), 10.0));
        assert!(!served.add(Some(&stats(5, 3, 1, 0, 1)), &stats(5, 3, 1, 0, 1), 20.0));
        assert!(served.add(Some(&stats(5, 3, 1, 0, 1)), &stats(8, 4, 1, 2, 1), 30.0));
        // Started again: its counts are from nothing.
        assert!(served.add(Some(&stats(8, 4, 1, 2, 1)), &stats(2, 1, 1, 0, 0), 40.0));
        assert_eq!(served, Served { since: 1.0, requests: 10, pages: 5, files: 2, media: 2, not_found: 1, last: Some(40.0) });
        assert_eq!(served.label(), "5 pages · 2 files · 2 pictures");
        assert_eq!(Served { pages: 1, files: 1, ..Served::default() }.label(), "1 page · 1 file");

        let dir = std::env::temp_dir().join(format!("rettui-node-stats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("node-stats.json");
        assert_eq!(Served::load(&path).requests, 0);
        std::fs::write(&path, serde_json::to_vec(&served).unwrap()).unwrap();
        assert_eq!(Served::load(&path), served);
        std::fs::write(&path, "{\"pages\": 4}").unwrap();
        assert_eq!(Served::load(&path).pages, 4);
        let _ = std::fs::remove_dir_all(dir);
    }
}
