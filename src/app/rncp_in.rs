//! Files received with `rncp`, as `rncp -l` does: someone sends a file to
//! this identity's rncp address (shown in the Network tab) and it is saved
//! in a folder. Only the identities in "Who may send files" are let in.

use std::path::PathBuf;

use super::App;
use crate::net::rncplisten::{RncpListenConfig, RncpListenEvent};
use crate::net::{Hash, NetCommand};

/// Files kept to show, the newest last.
const KEPT: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Received {
    pub name: String,
    pub bytes: usize,
    pub path: PathBuf,
    /// Who sent it (identity hash, hex), if they said.
    pub from: Option<String>,
    /// When it arrived (Unix seconds).
    pub at: f64,
}

/// What the listener is doing.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Listening {
    #[default]
    Off,
    Starting,
    On(Hash),
    Failed(String),
}

#[derive(Debug, Default)]
pub struct RncpIn {
    pub config: Option<RncpListenConfig>,
    pub status: Listening,
    pub received: Vec<Received>,
    /// Attempts by identities not on the list, since it started.
    pub refused: u64,
}

impl RncpIn {
    pub fn new(config: Option<RncpListenConfig>) -> Self {
        Self { status: if config.is_some() { Listening::Starting } else { Listening::Off }, config, ..Self::default() }
    }

    /// After a Reticulum restart, it starts again (with the settings).
    pub(super) fn restarting(&mut self, config: Option<RncpListenConfig>) {
        self.status = if config.is_some() { Listening::Starting } else { Listening::Off };
        self.config = config;
    }
}

impl App {
    /// Start, change or stop listening to match the settings.
    pub(super) fn apply_rncp_settings(&mut self) {
        let config = RncpListenConfig::from_settings(&self.settings, &self.paths);
        if config == self.rncp_in.config {
            return;
        }
        self.rncp_in.status = if config.is_some() { Listening::Starting } else { Listening::Off };
        if config.is_none() {
            self.rncp_in.refused = 0;
        }
        self.rncp_in.config = config.clone();
        self.send(NetCommand::RncpListen(config));
    }

    pub(super) fn on_rncp_listen(&mut self, event: RncpListenEvent) {
        match event {
            RncpListenEvent::Started { hash } => {
                // Listening for nobody is easy to do by mistake.
                let nobody = self.rncp_in.config.as_ref().is_some_and(|c| c.allowed.is_empty());
                self.rncp_in.status = Listening::On(hash);
                self.log(format!(
                    "Receiving files with rncp at {}{}",
                    hex::encode(hash),
                    if nobody { " (no identity may send yet: add one in Settings)" } else { "" }
                ));
            }
            RncpListenEvent::Stopped => {
                if self.rncp_in.config.is_none() {
                    self.rncp_in.status = Listening::Off;
                }
            }
            RncpListenEvent::Failed(e) => {
                self.log(e.clone());
                self.rncp_in.status = Listening::Failed(e);
            }
            RncpListenEvent::Received { name, bytes, path, from } => {
                let from = from.map(hex::encode);
                let who = from.as_deref().map_or_else(|| "someone".to_string(), |from| self.store.display_name(from));
                let size = crate::ui::human_bytes(bytes as u64);
                self.log(format!("Received {name} ({size}) from {who} with rncp, saved as {}", path.display()));
                self.notify(format!("{who} sent you {name}"));
                self.rncp_in.received.push(Received { name, bytes, path, from, at: super::now() });
                if self.rncp_in.received.len() > KEPT {
                    let gone = self.rncp_in.received.len() - KEPT;
                    self.rncp_in.received.drain(..gone);
                }
            }
            RncpListenEvent::Denied { from } => {
                self.rncp_in.refused += 1;
                let who = hex::encode(from);
                self.log(format!(
                    "Refused a file from {} with rncp: it isn't allowed to send",
                    if from == [0; 16] { "someone unknown" } else { &who }
                ));
            }
            RncpListenEvent::WriteFailed { name, reason } => {
                self.log(format!("Could not save {name} received with rncp: {reason}"));
                self.fail(format!("Could not save {name}: {reason}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::rncplisten::RncpListenEvent as E;

    #[test]
    fn it_follows_the_settings_and_says_what_arrived() {
        let dir = std::env::temp_dir().join(format!("rettui-rncp-in-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut commands) =
            crate::app::test_app_with_net(&dir, crate::config::Settings::default(), crate::store::Store::default());
        assert_eq!(app.rncp_in.status, Listening::Off);
        let who = "ab".repeat(16);
        app.update_settings(&[("rncp_allowed", &who), ("rncp_listen", "true")]).unwrap();
        assert_eq!(app.rncp_in.status, Listening::Starting);
        let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
        assert!(sent.iter().any(|c| matches!(c, NetCommand::RncpListen(Some(config)) if config.allowed == vec![[0xab; 16]])));
        app.on_net(crate::net::NetEvent::RncpListen(E::Started { hash: [7; 16] }));
        assert_eq!(app.rncp_in.status, Listening::On([7; 16]));
        let path = dir.join("received").join("notes.txt");
        app.on_net(crate::net::NetEvent::RncpListen(E::Received {
            name: "notes.txt".into(),
            bytes: 5,
            path: path.clone(),
            from: Some([0xab; 16]),
        }));
        app.on_net(crate::net::NetEvent::RncpListen(E::Denied { from: [0xcd; 16] }));
        assert_eq!((app.rncp_in.received.len(), app.rncp_in.refused), (1, 1));
        assert_eq!(app.rncp_in.received[0].from.as_deref(), Some(who.as_str()));
        assert!(app.log.iter().any(|l| l.contains("Received notes.txt")) && app.log.iter().any(|l| l.contains("Refused a file")));
        // Many files: the newest are kept.
        for n in 0..30 {
            app.on_net(crate::net::NetEvent::RncpListen(E::Received { name: format!("f{n}"), bytes: 1, path: path.clone(), from: None }));
        }
        assert_eq!((app.rncp_in.received.len(), app.rncp_in.received.last().unwrap().name.as_str()), (KEPT, "f29"));
        // Off again.
        app.update_settings(&[("rncp_listen", "false")]).unwrap();
        assert_eq!((app.rncp_in.status.clone(), app.rncp_in.refused), (Listening::Off, 0));
        app.on_net(crate::net::NetEvent::RncpListen(E::Failed("no".into())));
        assert_eq!(app.rncp_in.status, Listening::Failed("no".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
