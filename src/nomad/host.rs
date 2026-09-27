//! Hosting a NomadNet node with nomad-core, run by the network actor.
//!
//! The node serves `pages/` and `files/` from its folder, reading each page
//! from disk per request, so saved edits are live straight away. New,
//! renamed and deleted pages need [`NomadNode::reload_routes`].

use std::path::PathBuf;
use std::time::Duration;

use nomad_core::{NomadContentRoots, NomadContentStore, NomadNode, NomadNodeConfig};
use rns_runtime::prelude::*;

use crate::config::{Paths, Settings, expand_home};

/// How the node should run, from the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfig {
    pub dir: PathBuf,
    pub name: String,
    pub announce_interval: Option<Duration>,
    pub executable_pages: bool,
}

impl HostConfig {
    /// The node folder the settings point at (used even when not hosting,
    /// so pages can be written before the node is switched on).
    pub fn dir(settings: &Settings, paths: &Paths) -> PathBuf {
        settings.node_dir.as_deref().map_or_else(|| paths.node.clone(), expand_home)
    }

    /// `None` when hosting is off.
    pub fn from_settings(settings: &Settings, paths: &Paths) -> Option<Self> {
        settings.node_enabled.then(|| Self {
            dir: Self::dir(settings, paths),
            name: settings.node_name.clone().unwrap_or_else(|| settings.display_name.clone()),
            announce_interval: (settings.node_announce_interval_mins > 0)
                .then(|| Duration::from_secs(settings.node_announce_interval_mins * 60)),
            executable_pages: settings.node_executable_pages,
        })
    }
}

/// Start the node on the running Reticulum instance.
pub async fn start(runtime: &ReticulumHandle, identity: &Identity, config: &HostConfig) -> Result<NomadNode, String> {
    // A starter page of ours instead of nomad-core's placeholder.
    crate::nomad::pages::Pages::open(&config.dir)?.ensure_index(&config.name)?;
    let store = NomadContentStore::new(NomadContentRoots::under(&config.dir))
        .map_err(|e| format!("Could not open node folder {}: {e}", config.dir.display()))?;
    NomadNode::spawn(
        runtime.transport_tx.clone(),
        identity.clone(),
        store,
        NomadNodeConfig {
            display_name: config.name.clone(),
            announce_interval: config.announce_interval,
            announce_at_start: true,
            allow_executable_pages: config.executable_pages,
            ..NomadNodeConfig::default()
        },
    )
    .await
    .map_err(|e| format!("Could not start the node: {e}"))
}
