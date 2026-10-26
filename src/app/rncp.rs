//! Sending a file to an `rncp` listener (`F` in the Network tab), as
//! `rncp FILE ADDRESS` does: someone running `rncp -l` gets it in their
//! folder. The listener lets in only the identities it allows (`rncp -a`),
//! unless it lets anyone send (`rncp -l -n`).

use super::{App, PromptKind, now};
use crate::net::{NetCommand, RncpState, parse_hash};

/// Largest file sent, in bytes (it's held in memory while it goes).
pub const MAX_SEND: usize = 32 * 1024 * 1024;
/// Sends kept to show, the newest last.
const KEPT: usize = 10;

#[derive(Debug, Clone)]
pub struct RncpSend {
    pub to: String,
    pub name: String,
    pub bytes: usize,
    /// When it was asked for (Unix seconds).
    pub at: f64,
    pub state: RncpState,
}

/// The name a file goes as: its own, without the folders it's in.
fn send_name(name: &str) -> Result<String, String> {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    if name.is_empty() || name == "." || name == ".." {
        return Err("The file needs a name".into());
    }
    if name.chars().count() > 255 || name.chars().any(char::is_control) {
        return Err("A file's name is at most 255 characters, on one line".into());
    }
    Ok(name.to_string())
}

impl App {
    /// Send `data` as the file `name` to the `rncp` listener at `to`.
    pub fn rncp_send(&mut self, to: &str, name: &str, data: Vec<u8>) -> Result<(), String> {
        let hash = parse_hash(to.trim()).ok_or("An rncp address is 32 hex characters")?;
        let name = send_name(name)?;
        if data.len() > MAX_SEND {
            return Err(format!("A file sent with rncp is at most {} MB", MAX_SEND / (1024 * 1024)));
        }
        let to = hex::encode(hash);
        if self.rncp.iter().any(|s| s.to == to && s.name == name && matches!(s.state, RncpState::Sending(_))) {
            return Err(format!("{name} is already on its way there"));
        }
        self.rncp.push(RncpSend { to: to.clone(), name: name.clone(), bytes: data.len(), at: now(), state: RncpState::Sending(0.0) });
        if self.rncp.len() > KEPT {
            let gone = self.rncp.len() - KEPT;
            self.rncp.drain(..gone);
        }
        self.send(NetCommand::RncpSend { to: hash, name: name.clone(), data });
        self.confirm(format!("Sending {name} to {}…", self.store.display_name(&to)));
        Ok(())
    }

    /// Send a file on this computer (the terminal only: the web UI sends
    /// what its browser picks).
    pub(super) fn rncp_send_path(&mut self, to: &str, path: &str) -> Result<(), String> {
        let path = super::files::expand_home(path.trim());
        let size = std::fs::metadata(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?.len();
        if !path.is_file() {
            return Err(format!("{} isn't a file", path.display()));
        }
        if size > MAX_SEND as u64 {
            return Err(format!("A file sent with rncp is at most {} MB", MAX_SEND / (1024 * 1024)));
        }
        let data = std::fs::read(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.rncp_send(to, &name, data)
    }

    pub(super) fn on_rncp(&mut self, to: crate::net::Hash, name: String, state: RncpState) {
        let to = hex::encode(to);
        let who = self.store.display_name(&to);
        let Some(send) = self.rncp.iter_mut().rev().find(|s| s.to == to && s.name == name && matches!(s.state, RncpState::Sending(_)))
        else {
            return;
        };
        send.state = state.clone();
        match state {
            RncpState::Sending(done) if done > 0.0 => self.confirm(format!("Sending {name} to {who}: {:.0}%", done * 100.0)),
            RncpState::Sending(_) => {}
            RncpState::Sent { bytes, secs } => {
                let size = crate::ui::human_bytes(bytes as u64);
                self.log(format!("Sent {name} ({size}) to {who} with rncp in {secs:.1} s"));
                self.notify(format!("Sent {name} to {who}"));
            }
            RncpState::Failed(e) => {
                self.log(format!("Could not send {name} to {who} with rncp: {e}"));
                self.fail(format!("Could not send {name}: {e}"));
            }
        }
    }

    /// Ask for the listener's address, then the file (`F`).
    pub(super) fn ask_rncp(&mut self, address: Option<String>) {
        let mine = hex::encode(self.identity_hash);
        let title = format!("Send a file to which rncp listener? (it must allow your identity {mine}, or run rncp -l -n)");
        self.open_prompt(PromptKind::RncpTo, &title, &address.unwrap_or_default());
    }

    pub(super) fn submit_rncp_prompt(&mut self, kind: PromptKind, text: &str) {
        match kind {
            PromptKind::RncpTo => match parse_hash(text.trim()) {
                Some(hash) => {
                    let to = hex::encode(hash);
                    let title = format!("Send which file to {}? (its path, up to 32 MB)", self.store.display_name(&to));
                    self.open_prompt(PromptKind::RncpFile(to), &title, "");
                }
                None => self.warn("An rncp address is 32 hex characters"),
            },
            PromptKind::RncpFile(to) => {
                if let Err(e) = self.rncp_send_path(&to, text) {
                    self.warn(e);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_goes_by_its_own_name() {
        assert_eq!(send_name("/home/me/notes.txt").unwrap(), "notes.txt");
        assert_eq!(send_name("C:\\me\\photo 1.jpg").unwrap(), "photo 1.jpg");
        assert!(send_name("dir/").is_err() && send_name("..").is_err() && send_name("a\nb").is_err());
        assert!(send_name(&"x".repeat(256)).is_err());
    }
}
