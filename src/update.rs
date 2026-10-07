//! Update checks: once a day, the newest release on GitHub, and whether
//! it's newer than this build (the Check for updates setting, on unless
//! turned off). The answer is kept in `update-check.json`, so a restart
//! doesn't ask again, and an update found is still shown offline.
//!
//! The question is one HTTPS request to GitHub's API, through the proxy
//! set in the environment if any, saying only that it's rettui and which
//! version. Nothing is downloaded or installed: the UIs say a newer release
//! is out and link to its page.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The newest release (GitHub leaves out drafts and prereleases).
const LATEST: &str = "https://api.github.com/repos/zevaryx/rettui/releases/latest";
/// How often it asks, and how soon it asks again after it couldn't.
pub const EVERY: Duration = Duration::from_secs(24 * 3600);
pub const RETRY: Duration = Duration::from_secs(3600);

/// A release: its version (without the `v`) and its page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub url: String,
}

/// What the last check found, and when (Unix seconds).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    pub at: i64,
    pub latest: Option<Release>,
}

impl Checked {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|data| serde_json::from_slice(&data).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
    }

    /// The release found, if it's newer than `current`.
    pub fn newer_than(&self, current: &str) -> Option<&Release> {
        self.latest.as_ref().filter(|release| newer(current, &release.version))
    }
}

/// A version's numbers and what follows a `-` (a prerelease's `rc.1`).
fn parts(version: &str) -> Option<([u64; 3], Option<&str>)> {
    let version = version.trim().trim_start_matches('v');
    let version = version.split('+').next()?;
    let (numbers, pre) = match version.split_once('-') {
        Some((numbers, pre)) => (numbers, Some(pre)),
        None => (version, None),
    };
    let mut numbers = numbers.split('.').map(|n| n.parse::<u64>().ok());
    let found = [numbers.next()??, numbers.next().unwrap_or(Some(0))?, numbers.next().unwrap_or(Some(0))?];
    numbers.next().is_none().then_some((found, pre))
}

/// Whether `candidate` is a later version than `current`: by its numbers,
/// and a release is later than its own prereleases (`1.7.0` after
/// `1.7.0-rc.2`). Anything that isn't a version is never newer.
pub fn newer(current: &str, candidate: &str) -> bool {
    let (Some((now, now_pre)), Some((then, then_pre))) = (parts(current), parts(candidate)) else { return false };
    match then.cmp(&now) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => now_pre.is_some() && then_pre.is_none(),
    }
}

/// The newest release, from GitHub. Blocking: for a thread of its own.
pub fn fetch() -> Result<Release, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent(&format!("rettui/{VERSION} (+https://github.com/zevaryx/rettui)"))
        .try_proxy_from_env(true)
        .build();
    let fail = |e: String| format!("Couldn't check for updates: {e}");
    let response = agent.get(LATEST).set("Accept", "application/vnd.github+json").call().map_err(|e| fail(e.to_string()))?;
    let body = response.into_string().map_err(|e| fail(e.to_string()))?;
    latest_of(&body).map_err(fail)
}

/// The release in GitHub's answer for the latest one.
fn latest_of(body: &str) -> Result<Release, String> {
    #[derive(Deserialize)]
    struct Latest {
        tag_name: String,
        html_url: String,
    }
    let latest: Latest = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let version = latest.tag_name.trim_start_matches('v').to_string();
    if parts(&version).is_none() {
        return Err(format!("the newest release is called {:?}", latest.tag_name));
    }
    // Only GitHub's own pages are linked to.
    let url = if latest.html_url.starts_with("https://github.com/zevaryx/rettui/") {
        latest.html_url
    } else {
        format!("https://github.com/zevaryx/rettui/releases/tag/{}", latest.tag_name)
    };
    Ok(Release { version, url })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_versions_are_newer() {
        assert!(newer("1.6.0", "1.7.0"));
        assert!(newer("1.6.0", "v1.6.1"));
        assert!(newer("1.6.9", "1.10.0"));
        assert!(newer("1.6.0", "2.0"));
        assert!(!newer("1.6.0", "1.6.0"));
        assert!(!newer("1.6.0", "1.5.1"));
        assert!(!newer("1.10.0", "1.9.9"));
        // A release is newer than its prereleases, not the other way round.
        assert!(newer("1.7.0-rc.1", "1.7.0"));
        assert!(!newer("1.7.0", "1.7.0-rc.1"));
        assert!(!newer("1.7.0-rc.1", "1.6.0"));
        assert!(newer("1.6.0+build.5", "1.6.1"));
        // Not versions: never newer.
        for wrong in ["", "latest", "1.x", "1.2.3.4", "v"] {
            assert!(!newer("1.6.0", wrong), "{wrong}");
        }
        assert!(!newer("dev", "1.7.0"));
    }

    #[test]
    fn githubs_answer() {
        // As GitHub answered for v1.5.1 (the fields that matter, and some).
        let body = r#"{"url": "https://api.github.com/repos/zevaryx/rettui/releases/405055621",
            "html_url": "https://github.com/zevaryx/rettui/releases/tag/v1.5.1", "id": 405055621, "tag_name": "v1.5.1",
            "target_commitish": "main", "name": "rettui v1.5.1", "draft": false, "prerelease": false,
            "created_at": "2026-10-06T18:38:15Z", "published_at": "2026-10-06T19:08:15Z"}"#;
        let release = latest_of(body).unwrap();
        assert_eq!(release, Release { version: "1.5.1".into(), url: "https://github.com/zevaryx/rettui/releases/tag/v1.5.1".into() });
        // A page elsewhere isn't linked to; a tag that isn't a version, or
        // an answer that isn't one, is an error.
        let elsewhere = body.replace("https://github.com/zevaryx/rettui/releases/tag/v1.5.1", "https://example.com/x");
        assert_eq!(latest_of(&elsewhere).unwrap().url, "https://github.com/zevaryx/rettui/releases/tag/v1.5.1");
        assert!(latest_of(&body.replace("\"v1.5.1\"", "\"nightly\"")).is_err());
        assert!(latest_of(r#"{"message": "API rate limit exceeded"}"#).is_err());
    }

    #[test]
    fn kept_between_runs() {
        let path = std::env::temp_dir().join(format!("rettui-update-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert_eq!(Checked::load(&path), Checked::default());
        let release = Release { version: "1.7.0".into(), url: "https://github.com/zevaryx/rettui/releases/tag/v1.7.0".into() };
        let checked = Checked { at: 1_790_000_000, latest: Some(release.clone()) };
        checked.save(&path).unwrap();
        let loaded = Checked::load(&path);
        assert_eq!(loaded, checked);
        assert_eq!(loaded.newer_than("1.6.0"), Some(&release));
        assert_eq!(loaded.newer_than("1.7.0"), None);
        // A file that isn't one is as good as none.
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(Checked::load(&path), Checked::default());
        let _ = std::fs::remove_file(&path);
    }
}
