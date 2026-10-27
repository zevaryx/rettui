//! Hosting rngit repositories: a `git.repositories` destination that answers
//! clones and fetches from a folder (see [`rngit::server`]).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rngit::server::{Node, Rule};
use rns_identity::identity::Identity;
use rns_runtime::prelude::*;
use tokio::sync::mpsc;

use super::NetEvent;
use crate::config::{Paths, Settings, expand_home};

/// How often the node announces, as stock rngit does.
const ANNOUNCE_EVERY: Duration = Duration::from_secs(6 * 3600);
/// Stock waits a few seconds so the interfaces are up before the first announce.
const FIRST_ANNOUNCE: Duration = Duration::from_secs(5);

/// Mirrors are looked at hourly and followed again once a day old, as stock does.
const MIRROR_CHECK_EVERY: Duration = Duration::from_secs(3600);
const MIRROR_OLDER_THAN: Duration = Duration::from_secs(24 * 3600);

/// What the git node runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHostConfig {
    /// Holds `group/name.git` repositories.
    pub dir: PathBuf,
    pub identity: PathBuf,
    pub name: Option<String>,
    /// Rules for every group, as in a `.allowed` file.
    pub access: String,
    /// Largest push taken or clone sent, and what a repository may come to, in bytes.
    pub push_bytes: usize,
    pub repo_bytes: u64,
    /// Whether a sync drops what its source dropped.
    pub prune: bool,
}

impl GitHostConfig {
    pub fn from_settings(settings: &Settings, paths: &Paths) -> Option<Self> {
        settings.git_host_enabled.then(|| Self {
            dir: settings.git_host_dir.as_deref().map_or_else(|| paths.git_host.clone(), expand_home),
            identity: paths.git_host_identity.clone(),
            name: settings.git_host_name.clone().filter(|n| !n.trim().is_empty()),
            access: settings.git_host_access.clone().unwrap_or_default(),
            push_bytes: usize::try_from(settings.git_host_push_mb.saturating_mul(1_000_000)).unwrap_or(usize::MAX),
            repo_bytes: settings.git_host_repo_mb.saturating_mul(1_000_000),
            prune: settings.git_host_prune,
        })
    }
}

/// What the git node reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHostEvent {
    Started {
        hash: [u8; 16],
    },
    Stopped,
    Failed(String),
    /// A request was answered: its path, who asked (if they identified) and
    /// the status byte of the answer (`0` is success).
    Served {
        path: String,
        peer: Option<[u8; 16]>,
        status: u8,
    },
}

/// The git node's identity, made the first time.
fn identity(path: &std::path::Path) -> Result<Identity, String> {
    if path.exists() {
        return Identity::from_file(path).map_err(|e| format!("{} isn't an identity file: {e}", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't make {}: {e}", parent.display()))?;
    }
    let identity = Identity::new();
    identity.to_file(path).map_err(|e| format!("Couldn't save the git node's identity to {}: {e}", path.display()))?;
    Ok(identity)
}

/// The git node's address, once it has an identity (before it first starts,
/// it has none).
pub fn address(path: &std::path::Path) -> Option<[u8; 16]> {
    let identity = Identity::from_file(path).ok()?;
    Some(Destination::hash_from_name_and_identity(rngit::serve::ASPECT, Some(&identity.hash)))
}

/// A running git node.
pub struct HostedGit {
    pub hash: [u8; 16],
    destination: RegisteredDestination,
    announcer: tokio::task::JoinHandle<()>,
    mirrors: tokio::task::JoinHandle<()>,
}

impl HostedGit {
    pub async fn start(runtime: &ReticulumHandle, config: &GitHostConfig, ev: mpsc::UnboundedSender<NetEvent>) -> Result<Self, String> {
        let (path, dir) = (config.identity.clone(), config.dir.clone());
        let identity = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
            identity(&path)
        })
        .await
        .map_err(|e| e.to_string())??;
        let mut node = Node::new(config.dir.clone());
        node.default_rules = Rule::parse(&config.access);
        node.max_bundle_bytes = config.push_bytes;
        node.quotas.max_push_bytes = config.push_bytes;
        node.quotas.max_repo_bytes = config.repo_bytes;
        node.prune_synced = config.prune;
        node.upstream = Some(rngit::serve::upstream(runtime.clone(), identity.clone(), rngit::client::Limits::default()));
        let node = Arc::new(node);
        let activity: rngit::serve::Activity = {
            let ev = ev.clone();
            Arc::new(move |path, peer, status| {
                let _ = ev.send(NetEvent::GitHost(GitHostEvent::Served { path: path.to_string(), peer, status }));
            })
        };
        let app_data = config.name.as_ref().map(|n| n.as_bytes().to_vec());
        let destination =
            rngit::serve::serve(runtime, identity, node.clone(), app_data.clone(), Some(activity)).await.map_err(|e| e.to_string())?;
        let hash = destination.handle.destination_hash();
        let handle = destination.handle.clone();
        let announcer = tokio::spawn(async move {
            tokio::time::sleep(FIRST_ANNOUNCE).await;
            loop {
                let options = DestinationAnnounceOptions { app_data: app_data.clone(), ..DestinationAnnounceOptions::default() };
                let _ = handle.announce(options).await;
                tokio::time::sleep(ANNOUNCE_EVERY).await;
            }
        });
        let mirrors = rngit::serve::keep_mirrors(node, MIRROR_CHECK_EVERY, MIRROR_OLDER_THAN);
        Ok(Self { hash, destination, announcer, mirrors })
    }

    /// Stop answering, closing the links so clients see it go.
    pub async fn stop(self) {
        self.announcer.abort();
        self.mirrors.abort();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.destination.close()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_node_runs_only_when_asked_and_with_what_was_set() {
        let dir = std::env::temp_dir().join(format!("rettui-githost-{}", std::process::id()));
        let paths = Paths::new(Some(dir.clone())).unwrap();
        let mut settings = Settings::default();
        assert_eq!(GitHostConfig::from_settings(&settings, &paths), None);
        settings.git_host_enabled = true;
        let config = GitHostConfig::from_settings(&settings, &paths).unwrap();
        assert_eq!((config.dir.as_path(), config.name.as_deref(), config.access.as_str()), (paths.git_host.as_path(), None, ""));
        assert_eq!((config.push_bytes, config.repo_bytes, config.prune), (256_000_000, 1_024_000_000, false));
        settings.git_host_name = Some("  ".into());
        settings.git_host_access = Some("r:all".into());
        settings.git_host_dir = Some(dir.join("repos").display().to_string());
        let config = GitHostConfig::from_settings(&settings, &paths).unwrap();
        assert_eq!((config.dir, config.name, config.access), (dir.join("repos"), None, "r:all".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_identity_is_made_once_and_gives_a_stable_address() {
        let dir = std::env::temp_dir().join(format!("rettui-githost-id-{}", std::process::id()));
        let path = dir.join("nested/git-host-identity");
        assert_eq!(address(&path), None);
        let first = identity(&path).unwrap();
        let again = identity(&path).unwrap();
        assert_eq!(first.hash, again.hash);
        assert!(address(&path).is_some());
        std::fs::write(&path, b"junk").unwrap();
        assert!(identity(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
