//! On-disk cache for NomadNet pages and media, to spare slow mesh links.
//!
//! Entries are keyed by node, path and whether we identified (identified
//! pages may be personalised). Form submissions and file downloads are not
//! cached. Each entry is a data file plus a small JSON sidecar.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::net::Hash;

#[derive(Serialize, Deserialize)]
struct Meta {
    url: String,
    /// Unix seconds.
    fetched: u64,
    ttl: u64,
}

pub struct Cached {
    pub data: Vec<u8>,
    pub age: Duration,
}

pub struct Cache {
    dir: PathBuf,
    default_ttl: Duration,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

impl Cache {
    pub fn new(dir: PathBuf, default_ttl: Duration) -> Self {
        let cache = Self { dir, default_ttl };
        cache.prune();
        cache
    }

    /// How long new entries stay fresh (pages can still ask for less).
    pub fn set_default_ttl(&mut self, ttl: Duration) {
        self.default_ttl = ttl;
    }

    fn key(node: Hash, path: &str, identified: bool) -> String {
        let id = format!("{}:{path}:{}", hex::encode(node), u8::from(identified));
        hex::encode(&rns_crypto::sha::full_hash(id.as_bytes())[..16])
    }

    fn files(&self, key: &str) -> (PathBuf, PathBuf) {
        (self.dir.join(format!("{key}.bin")), self.dir.join(format!("{key}.json")))
    }

    fn read_meta(path: &Path) -> Option<Meta> {
        serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
    }

    pub fn get(&self, node: Hash, path: &str, identified: bool) -> Option<Cached> {
        let (data_path, meta_path) = self.files(&Self::key(node, path, identified));
        let meta = Self::read_meta(&meta_path)?;
        let age = now().saturating_sub(meta.fetched);
        if age >= meta.ttl {
            return None;
        }
        Some(Cached { data: fs::read(data_path).ok()?, age: Duration::from_secs(age) })
    }

    /// Store a response. `ttl` overrides the default (a page's `#!c=`);
    /// zero means the page asked not to be cached.
    pub fn put(&self, node: Hash, path: &str, identified: bool, data: &[u8], ttl: Option<Duration>) {
        let ttl = ttl.unwrap_or(self.default_ttl);
        let key = Self::key(node, path, identified);
        if ttl.is_zero() {
            self.remove(&key);
            return;
        }
        let (data_path, meta_path) = self.files(&key);
        let meta = Meta { url: format!("{}:{path}", hex::encode(node)), fetched: now(), ttl: ttl.as_secs() };
        let result = fs::create_dir_all(&self.dir).and_then(|()| fs::write(&data_path, data)).and_then(|()| {
            let json = serde_json::to_string(&meta).map_err(std::io::Error::other)?;
            fs::write(&meta_path, json)
        });
        if let Err(e) = result {
            tracing::warn!("could not cache {}: {e}", meta.url);
        }
    }

    fn remove(&self, key: &str) {
        let (data_path, meta_path) = self.files(key);
        let _ = fs::remove_file(data_path);
        let _ = fs::remove_file(meta_path);
    }

    /// Remove every entry; returns how many were removed.
    pub fn clear(&self) -> usize {
        self.remove_where(|_| true)
    }

    /// Drop expired entries (run at startup).
    fn prune(&self) {
        let now = now();
        let removed = self.remove_where(|meta| meta.is_none_or(|m| now.saturating_sub(m.fetched) >= m.ttl));
        if removed > 0 {
            tracing::debug!("pruned {removed} expired cache entries");
        }
    }

    fn remove_where(&self, condition: impl Fn(Option<&Meta>) -> bool) -> usize {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                let meta = Self::read_meta(&path);
                if condition(meta.as_ref()) {
                    let _ = fs::remove_file(path.with_extension("bin"));
                    let _ = fs::remove_file(&path);
                    removed += 1;
                }
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache(name: &str) -> (Cache, PathBuf) {
        let dir = std::env::temp_dir().join(format!("rettui-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        (Cache::new(dir.clone(), Duration::from_secs(3600)), dir)
    }

    #[test]
    fn stores_and_expires_entries() {
        let (cache, dir) = temp_cache("basic");
        let node = [7; 16];
        cache.put(node, "/page/index.mu", false, b"hello", None);
        assert_eq!(cache.get(node, "/page/index.mu", false).unwrap().data, b"hello");
        // Identified and anonymous views are separate entries.
        assert!(cache.get(node, "/page/index.mu", true).is_none());

        // `#!c=0` pages are not kept.
        cache.put(node, "/page/live.mu", false, b"x", Some(Duration::ZERO));
        assert!(cache.get(node, "/page/live.mu", false).is_none());

        // A newer copy replaces the old one.
        cache.put(node, "/page/index.mu", false, b"newer", None);
        assert_eq!(cache.get(node, "/page/index.mu", false).unwrap().data, b"newer");

        cache.put(node, "/page/a.mu", false, b"a", None);
        cache.put(node, "/page/b.mu", false, b"b", Some(Duration::from_secs(1)));
        std::thread::sleep(Duration::from_millis(1100));
        assert!(cache.get(node, "/page/b.mu", false).is_none()); // expired
        assert_eq!(cache.clear(), 3);
        fs::remove_dir_all(dir).unwrap();
    }
}
