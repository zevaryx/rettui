//! The rngit node hosted here, as the app knows it.

use std::collections::VecDeque;

use crate::app::App;
use crate::app::node::NodeStatus;
use crate::config::Paths;
use crate::net::NetCommand;
use crate::net::githost::{self, GitHostConfig, GitHostEvent};

/// How many answered requests are kept to show.
const KEPT: usize = 50;

/// One request the node answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// What it was for: `fetch`, `list`, `push`...
    pub what: String,
    /// Who asked, as the first eight hex digits of their identity; none if they didn't identify.
    pub peer: Option<String>,
    pub ok: bool,
}

pub struct GitHost {
    pub status: NodeStatus,
    /// The address in clone URLs, once it has an identity (after it first starts).
    pub address: Option<[u8; 16]>,
    /// What the network actor was last asked to run.
    config: Option<GitHostConfig>,
    /// Requests answered since it started, and how many were refused.
    pub requests: u64,
    pub refused: u64,
    /// The newest requests, newest last.
    pub recent: VecDeque<Served>,
}

impl GitHost {
    pub fn new(paths: &Paths, config: Option<GitHostConfig>) -> Self {
        Self {
            status: if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off },
            address: githost::address(&paths.git_host_identity),
            config,
            requests: 0,
            refused: 0,
            recent: VecDeque::new(),
        }
    }

    /// After a Reticulum restart, it starts again (with the settings).
    pub(super) fn restarting(&mut self, config: Option<GitHostConfig>) {
        self.status = if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off };
        self.config = config;
    }

    /// The URL to clone a repository from here, `rns://<address>/`.
    pub fn url(&self) -> Option<String> {
        self.address.map(|hash| format!("rns://{}", hex::encode(hash)))
    }

    /// How much it has done since it started.
    pub fn summary(&self) -> String {
        let mut text = format!("{} request{}", self.requests, if self.requests == 1 { "" } else { "s" });
        if self.refused > 0 {
            text.push_str(&format!(" · {} refused", self.refused));
        }
        text
    }
}

impl App {
    /// Start, change or stop the node to match the settings.
    pub(super) fn apply_git_host_settings(&mut self) {
        let config = GitHostConfig::from_settings(&self.settings, &self.paths);
        if config == self.git_host.config {
            return;
        }
        self.git_host.status = if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off };
        if config.is_none() {
            self.git_host.requests = 0;
            self.git_host.refused = 0;
            self.git_host.recent.clear();
        }
        self.git_host.config = config.clone();
        self.send(NetCommand::GitHost(config));
    }

    pub(super) fn on_git_host(&mut self, event: GitHostEvent) {
        match event {
            GitHostEvent::Started { hash } => {
                self.git_host.status = NodeStatus::Running;
                self.git_host.address = Some(hash);
                self.log(format!("Hosting rngit repositories at rns://{}", hex::encode(hash)));
            }
            GitHostEvent::Stopped => {
                self.git_host.status = NodeStatus::Off;
                self.log("Stopped hosting rngit repositories");
            }
            GitHostEvent::Failed(e) => {
                self.log(format!("rngit node: {e}"));
                self.git_host.status = NodeStatus::Failed(e);
            }
            // The last of a node that has since stopped: not counted.
            GitHostEvent::Served { .. } if self.git_host.status == NodeStatus::Off => {}
            GitHostEvent::Served { path, peer, status } => {
                self.git_host.requests += 1;
                if status != 0 {
                    self.git_host.refused += 1;
                }
                let what = path.rsplit('/').next().unwrap_or(&path).chars().filter(|c| c.is_ascii_graphic()).take(16).collect();
                let peer = peer.map(|p| hex::encode(&p[..4]));
                if self.git_host.recent.len() >= KEPT {
                    self.git_host.recent.pop_front();
                }
                self.git_host.recent.push_back(Served { what, peer, ok: status == 0 });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::node::NodeStatus;
    use crate::config::Settings;
    use crate::net::NetCommand;
    use crate::net::NetEvent;
    use crate::net::githost::GitHostEvent;
    use crate::store::Store;

    fn app(name: &str) -> (crate::app::App, tokio::sync::mpsc::UnboundedReceiver<NetCommand>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("rettui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (app, commands) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        (app, commands, dir)
    }

    #[test]
    fn turning_it_on_starts_the_node_and_off_stops_it() {
        let (mut app, mut commands, dir) = app("git-host-on");
        while commands.try_recv().is_ok() {}
        app.update_settings(&[("git_host_enabled", "true"), ("git_host_access", "r:all")]).unwrap();
        assert_eq!(app.git_host.status, NodeStatus::Starting);
        match commands.try_recv() {
            Ok(NetCommand::GitHost(Some(config))) => assert_eq!(config.access, "r:all"),
            other => panic!("{other:?}"),
        }
        app.on_net(NetEvent::GitHost(GitHostEvent::Started { hash: [7; 16] }));
        assert_eq!(app.git_host.status, NodeStatus::Running);
        assert_eq!(app.git_host.url().unwrap(), format!("rns://{}", "07".repeat(16)));
        app.update_settings(&[("git_host_enabled", "false")]).unwrap();
        assert_eq!(app.git_host.status, NodeStatus::Off);
        assert!(matches!(commands.try_recv(), Ok(NetCommand::GitHost(None))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn requests_are_counted_and_the_newest_kept() {
        let (mut app, _commands, dir) = app("git-host-served");
        app.update_settings(&[("git_host_enabled", "true")]).unwrap();
        app.on_net(NetEvent::GitHost(GitHostEvent::Started { hash: [7; 16] }));
        for n in 0..60u8 {
            let status = if n % 10 == 0 { 3 } else { 0 };
            app.on_net(NetEvent::GitHost(GitHostEvent::Served { path: "/git/fetch\u{1b}[31m".into(), peer: Some([n; 16]), status }));
        }
        assert_eq!((app.git_host.requests, app.git_host.refused, app.git_host.recent.len()), (60, 6, 50));
        let last = app.git_host.recent.back().unwrap();
        assert_eq!((last.what.as_str(), last.peer.as_deref(), last.ok), ("fetch[31m", Some("3b3b3b3b"), true));
        app.on_net(NetEvent::GitHost(GitHostEvent::Failed("no room".into())));
        assert_eq!(app.git_host.status, NodeStatus::Failed("no room".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_web_ui_cannot_choose_the_folder_or_who_may_read() {
        let (mut app, _commands, dir) = app("git-host-web");
        assert!(app.update_settings_from_web(&[("git_host_dir", "/tmp")]).is_err());
        assert!(app.update_settings_from_web(&[("git_host_access", "r:all")]).is_err());
        app.update_settings_from_web(&[("git_host_enabled", "true")]).unwrap();
        assert!(app.git_host.status == NodeStatus::Starting);
        assert!(app.update_settings(&[("git_host_access", "nonsense")]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
