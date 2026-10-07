//! Clipboard access: the system clipboard where there is one, and the
//! terminal's OSC 52 clipboard escape (which also works over SSH).

use std::io::Write;

use base64::Engine;

/// OSC 52 payloads beyond this are commonly truncated or refused.
const MAX_OSC52_BYTES: usize = 100_000;

pub struct Clipboard {
    /// Kept alive for the whole session: on X11 and Wayland the copying
    /// process serves the clipboard contents.
    system: Option<arboard::Clipboard>,
}

impl Clipboard {
    /// `RETTUI_CLIPBOARD=osc52` skips the system clipboard.
    pub fn new() -> Self {
        let osc52_only = std::env::var("RETTUI_CLIPBOARD").is_ok_and(|v| v == "osc52");
        let system = if osc52_only {
            None
        } else {
            arboard::Clipboard::new().inspect_err(|e| tracing::info!("no system clipboard ({e}); using OSC 52")).ok()
        };
        Self { system }
    }

    /// Copy text. Always also sent as OSC 52, so it lands in the local
    /// clipboard even when rettui runs over SSH.
    pub fn copy(&mut self, text: &str) {
        if let Some(system) = &mut self.system
            && let Err(e) = system.set_text(text.to_string())
        {
            tracing::warn!("system clipboard copy failed: {e}");
        }
        if text.len() <= MAX_OSC52_BYTES {
            let encoded = base64::engine::general_purpose::STANDARD.encode(text);
            let mut out = std::io::stdout();
            let _ = write!(out, "\x1b]52;c;{encoded}\x07");
            let _ = out.flush();
        }
    }

    /// Text from the system clipboard. (Terminal paste arrives separately
    /// as a bracketed-paste event.)
    pub fn paste(&mut self) -> Option<String> {
        self.system.as_mut()?.get_text().ok()
    }
}
