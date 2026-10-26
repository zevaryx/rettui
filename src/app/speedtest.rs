//! A speed test with someone (`v` in the Network tab), as NomadNet 1.4's:
//! each gives the other their speed test address, and either sends a test
//! of random bytes the other takes and times. While it's open, rettui takes
//! tests from the address entered, and no one else.

use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent};

use super::{App, PromptKind, now};
use crate::net::speedtest::{self, SpeedEvent};
use crate::net::{Hash, NetCommand, parse_hash};

/// The sizes a test may be, as NomadNet offers them (up to what's held in
/// memory).
pub const SIZES: [(&str, usize); 8] = [
    ("2 KB", 2 * 1024),
    ("8 KB", 8 * 1024),
    ("32 KB", 32 * 1024),
    ("128 KB", 128 * 1024),
    ("512 KB", 512 * 1024),
    ("1 MB", 1024 * 1024),
    ("4 MB", 4 * 1024 * 1024),
    ("16 MB", 16 * 1024 * 1024),
];
/// NomadNet's first choice, 1 MB.
const DEFAULT_SIZE: usize = 5;
/// Lines kept of what happened.
const KEPT: usize = 12;
/// Seconds a browser's asking keeps rettui listening.
const WANTED_FOR: f64 = 15.0;

#[derive(Debug)]
pub struct SpeedTest {
    /// Shown in the Network tab.
    pub open: bool,
    /// Their speed test address: tests are taken from it, and sent to it.
    pub peer: Option<Hash>,
    /// Which of [`SIZES`] is sent.
    pub size: usize,
    /// How much of one going, or coming, has.
    pub sending: Option<f32>,
    pub receiving: Option<f32>,
    /// Who the network takes tests from now, as it says.
    pub listening: Option<Hash>,
    /// What it was last asked to.
    asked: Option<Hash>,
    /// Kept listening until then, for a browser showing it.
    wanted_until: f64,
    /// What happened, the newest last.
    pub lines: VecDeque<String>,
}

impl Default for SpeedTest {
    fn default() -> Self {
        Self {
            open: false,
            peer: None,
            size: DEFAULT_SIZE,
            sending: None,
            receiving: None,
            listening: None,
            asked: None,
            wanted_until: 0.0,
            lines: VecDeque::new(),
        }
    }
}

/// Bytes a second as a bit rate, as NomadNet says it: "1.2 Mbps".
pub fn bitrate(bytes: usize, secs: f64) -> String {
    let mut bits = bytes as f64 * 8.0 / secs.max(0.001);
    for unit in ["bps", "Kbps", "Mbps", "Gbps"] {
        if bits < 1000.0 {
            return format!("{bits:.1} {unit}");
        }
        bits /= 1000.0;
    }
    format!("{bits:.1} Tbps")
}

impl SpeedTest {
    fn note(&mut self, line: String) {
        self.lines.push_back(line);
        while self.lines.len() > KEPT {
            self.lines.pop_front();
        }
    }
}

impl App {
    /// This client's speed test address, to give the other.
    pub fn speed_address(&self) -> Hash {
        speedtest::address(self.identity_hash)
    }

    /// Whose tests to take, and who to send one to.
    pub fn speed_set_peer(&mut self, text: &str) -> Result<(), String> {
        let text = text.trim().trim_start_matches('<').trim_end_matches('>');
        let peer = parse_hash(text).ok_or("A speed test address is 32 hex characters (theirs, from their speed test)")?;
        if peer == self.speed_address() {
            return Err("That's your own speed test address: enter theirs".into());
        }
        self.speed.peer = Some(peer);
        self.speed_tick();
        Ok(())
    }

    pub fn speed_set_size(&mut self, index: usize) -> Result<(), String> {
        if index >= SIZES.len() {
            return Err("Not one of the sizes".into());
        }
        self.speed.size = index;
        Ok(())
    }

    /// Send them a test.
    pub fn speed_send(&mut self) -> Result<(), String> {
        let peer = self.speed.peer.ok_or("Enter their speed test address first")?;
        if self.speed.sending.is_some() {
            return Err("A test is already being sent".into());
        }
        let (label, bytes) = SIZES[self.speed.size];
        self.speed.sending = Some(0.0);
        self.speed.note(format!("Sending {label} to {}…", hex::encode(peer)));
        self.send(NetCommand::SpeedSend { to: peer, bytes });
        Ok(())
    }

    /// A browser shows it: keep listening for a while.
    pub fn speed_wanted(&mut self) {
        self.speed.wanted_until = now() + WANTED_FOR;
        self.speed_tick();
    }

    /// Listen for their tests while it's shown, and not otherwise.
    pub(super) fn speed_tick(&mut self) {
        let shown = self.speed.open || now() < self.speed.wanted_until;
        let wanted = self.speed.peer.filter(|_| shown);
        if wanted != self.speed.asked {
            self.speed.asked = wanted;
            self.send(NetCommand::SpeedListen(wanted));
        }
    }

    pub(super) fn on_speed(&mut self, event: SpeedEvent) {
        let test = &mut self.speed;
        match event {
            SpeedEvent::Listening(Ok(from)) => {
                test.listening = from;
                if let Some(from) = from {
                    test.note(format!("Taking tests from {}", hex::encode(from)));
                }
            }
            SpeedEvent::Listening(Err(e)) => {
                test.listening = None;
                test.note(e);
            }
            SpeedEvent::Note(line) => test.note(line),
            SpeedEvent::Refused => test.note("Turned away a test from someone else".into()),
            SpeedEvent::Progress { sending: true, done } => test.sending = Some(done),
            SpeedEvent::Progress { sending: false, done } => test.receiving = Some(done),
            SpeedEvent::Done { sending, bytes, secs } => {
                let rate = bitrate(bytes, secs);
                let size = crate::ui::human_bytes(bytes as u64);
                let line = format!("{}: {rate} ({size} in {secs:.2} s)", if sending { "Upload" } else { "Download" });
                if sending {
                    test.sending = None;
                } else {
                    test.receiving = None;
                }
                test.note(line.clone());
                self.notify(format!("Speed test {line}"));
            }
            SpeedEvent::Failed { sending, why } => {
                if sending {
                    test.sending = None;
                } else {
                    test.receiving = None;
                }
                let line = format!("{} failed: {why}", if sending { "Sending" } else { "Receiving" });
                test.note(line.clone());
                self.warn(format!("Speed test: {line}"));
            }
        }
    }

    // ---- TUI ---------------------------------------------------------------

    pub(super) fn toggle_speed(&mut self) {
        self.speed.open = !self.speed.open;
        self.speed_tick();
        if self.speed.open && self.speed.peer.is_none() {
            self.ask_speed_peer();
        }
    }

    fn ask_speed_peer(&mut self) {
        let current = self.speed.peer.map(hex::encode).unwrap_or_default();
        self.open_prompt(PromptKind::SpeedPeer, "Their speed test address (from their speed test, in rettui or NomadNet)", &current);
    }

    pub(super) fn speed_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('v') => self.toggle_speed(),
            KeyCode::Char('e') => self.ask_speed_peer(),
            KeyCode::Left | KeyCode::Char('h') => self.speed.size = self.speed.size.saturating_sub(1),
            KeyCode::Right | KeyCode::Char('l') => self.speed.size = (self.speed.size + 1).min(SIZES.len() - 1),
            KeyCode::Char('y') => {
                let address = hex::encode(self.speed_address());
                self.copy(&address, "speed test address");
            }
            KeyCode::Enter => {
                if let Err(e) = self.speed_send() {
                    self.warn(e);
                }
            }
            _ => {}
        }
    }

    pub(super) fn submit_speed_prompt(&mut self, text: &str) {
        if let Err(e) = self.speed_set_peer(text) {
            self.warn(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_read_as_nomadnet_says_them() {
        assert_eq!(bitrate(125, 1.0), "1.0 Kbps");
        assert_eq!(bitrate(100, 1.0), "800.0 bps");
        assert_eq!(bitrate(1024 * 1024, 2.0), "4.2 Mbps");
        assert_eq!(bitrate(1, 0.0), "8.0 Kbps");
    }
}
