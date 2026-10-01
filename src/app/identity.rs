//! Using an identity from another Reticulum program, and backing this one
//! up. An identity is the private key behind your address: lost, the
//! address is gone for good; brought along, contacts reach you here at the
//! address they know. Reticulum programs keep it in the same format (the
//! raw 64-byte key Python's Reticulum writes), so Sideband's, NomadNet's or
//! MeshChat's file works here, and this one there.
//!
//! The terminal UI only: anyone holding the web login link could otherwise
//! copy the key, and pose as you for good.
//!
//! A new identity is used from the next start: the node, the propagation
//! node and RRC all take their addresses from it.

use std::path::{Path, PathBuf};

use rns_identity::identity::Identity;

use super::{App, PromptKind, files};
use crate::net::Hash;

/// Identity files are 64 bytes (or a little more, in rsReticulum's own
/// envelope); anything this big is something else.
const MOST: u64 = 1024;

/// The LXMF address an identity has.
pub fn lxmf_address(identity: &Identity) -> Hash {
    rns_identity::destination::Destination::hash_from_name_and_identity(crate::lxmf::LXMF_ASPECT, Some(&identity.hash))
}

/// The identity in a file, if it is one.
pub fn read_identity(path: &Path) -> Result<Identity, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("Can't read {}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MOST || meta.len() < 64 {
        return Err(format!("{} isn't an identity file (those are 64 bytes)", path.display()));
    }
    Identity::from_file(path).map_err(|e| format!("{} isn't an identity file: {e}", path.display()))
}

/// Write a file only its owner may read, never over another.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

impl App {
    /// Where the identity in use is kept.
    fn identity_file(&self) -> &Path {
        &self.paths.identity
    }

    /// Ask which file to use (the guide's "Use an identity you already
    /// have", or `i` in the Status tab).
    pub(super) fn ask_identity_file(&mut self) {
        self.open_prompt(PromptKind::ImportIdentity, "Identity file to use (from Sideband, NomadNet, MeshChat or another rettui)", "");
    }

    /// The file chosen: check it, and ask before using it.
    pub(super) fn chose_identity_file(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let path = files::path_from_input(text);
        let identity = match read_identity(&path) {
            Ok(identity) => identity,
            Err(e) => return self.warn(e),
        };
        if identity.hash == self.identity_hash && self.identity_pending.is_none() {
            return self.warn("That's the identity rettui uses already");
        }
        let address = hex::encode(lxmf_address(&identity));
        // What else takes its address from the identity, and changes too.
        let mut also = Vec::new();
        if self.settings.node_enabled {
            also.push("your node's address");
        }
        if self.settings.pn_enabled {
            also.push("your propagation node's address");
        }
        if !self.channels.hubs.is_empty() {
            also.push("who you are to RRC hubs");
        }
        let also = match also.as_slice() {
            [] => String::new(),
            [one] => format!(" It changes {one} too."),
            [rest @ .., last] => format!(" It changes {} and {last} too.", rest.join(", ")),
        };
        let title = format!("Use the identity with LXMF address {address}?{also} Yours is kept beside it. Type y");
        self.open_prompt(PromptKind::ConfirmImportIdentity(path), &title, "");
    }

    /// Use the identity in `path` from the next start, keeping the one in
    /// use beside it (`identity.previous-<when>`). Its LXMF address.
    pub fn import_identity(&mut self, path: &Path) -> Result<Hash, String> {
        let identity = read_identity(path)?;
        let target = self.identity_file().to_path_buf();
        let fail = |e: std::io::Error| format!("Couldn't save the identity: {e}");
        if let Ok(current) = std::fs::read(&target) {
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let kept = target.with_file_name(format!("identity.previous-{stamp}"));
            write_private(&kept, &current).map_err(fail)?;
        }
        // Rewritten in Python's format, whatever it came in.
        identity.to_file(&target).map_err(|e| format!("Couldn't save the identity: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).map_err(fail)?;
        }
        let address = lxmf_address(&identity);
        self.identity_pending = (identity.hash != self.identity_hash).then_some(address);
        Ok(address)
    }

    /// Ask where to save a copy of the identity (`b` in the Status tab).
    pub(super) fn ask_identity_backup(&mut self) {
        if self.identity_pending.is_some() {
            return self.warn("Another identity is waiting for rettui to start again: back up after that");
        }
        let short = self.lxmf_hash.map(|hash| hex::encode(&hash[..4])).unwrap_or_default();
        let suggested = format!("~/rettui-identity-{short}");
        self.open_prompt(PromptKind::BackUpIdentity, "Save a copy of your identity to", &suggested);
    }

    /// Save a copy of the identity in use to `to`, readable by its owner
    /// only, and never over another file.
    pub fn back_up_identity(&mut self, to: &Path) -> Result<PathBuf, String> {
        if to.exists() {
            return Err(format!("{} exists already: pick another name", to.display()));
        }
        let key = std::fs::read(self.identity_file()).map_err(|e| format!("Couldn't read your identity: {e}"))?;
        write_private(to, &key).map_err(|e| format!("Couldn't save {}: {e}", to.display()))?;
        self.identity_backed_up();
        Ok(to.to_path_buf())
    }

    pub(super) fn submit_identity_prompt(&mut self, kind: PromptKind, text: &str) {
        let yes = text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes");
        match kind {
            PromptKind::ImportIdentity => self.chose_identity_file(text),
            PromptKind::ConfirmImportIdentity(path) if yes => match self.import_identity(&path) {
                Ok(address) => self.notify(format!(
                    "Saved. Quit and start rettui again to use it ({}). Don't run it in another program at the same time: messages to it would be split between them",
                    hex::encode(address)
                )),
                Err(e) => self.fail(e),
            },
            PromptKind::BackUpIdentity if !text.is_empty() => match self.back_up_identity(&files::path_from_input(text)) {
                Ok(path) => self.notify(format!(
                    "Saved your identity to {}. Keep it private: whoever has it can pose as you and read what's sent to you",
                    path.display()
                )),
                Err(e) => self.warn(e),
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use crate::store::Store;

    #[test]
    fn an_identity_comes_from_another_program_and_is_backed_up() {
        let dir = std::env::temp_dir().join(format!("rettui-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = crate::app::test_app(&dir, Settings::default(), Store::default());
        // The one in use, as rettui made it at first start.
        let ours = Identity::new();
        ours.to_file(&app.paths.identity).unwrap();
        app.identity_hash = ours.hash;
        // Another program's, in Python's format (the raw 64-byte key).
        let theirs = Identity::new();
        let theirs_path = dir.join("sideband-identity");
        std::fs::write(&theirs_path, *theirs.get_private_key().unwrap()).unwrap();
        assert_eq!(read_identity(&theirs_path).unwrap().hash, theirs.hash);
        // Not identities.
        std::fs::write(dir.join("short"), b"nope").unwrap();
        assert!(read_identity(&dir.join("short")).is_err());
        assert!(read_identity(&dir).is_err());
        assert!(read_identity(&dir.join("missing")).is_err());

        // Backed up: the same key, private, and never over another file.
        let backup = dir.join("backup");
        app.back_up_identity(&backup).unwrap();
        assert_eq!(read_identity(&backup).unwrap().hash, ours.hash);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(app.back_up_identity(&backup).is_err());

        // Imported: used from the next start, the old one kept beside it.
        let address = app.import_identity(&theirs_path).unwrap();
        assert_eq!(address, lxmf_address(&theirs));
        assert_eq!(app.identity_pending, Some(address));
        assert_eq!(read_identity(&app.paths.identity).unwrap().hash, theirs.hash);
        let kept: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("identity.previous-"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(read_identity(&kept[0].path()).unwrap().hash, ours.hash);
        // The question says what else changes, and shows whole.
        app.identity_pending = None;
        app.settings.node_enabled = true;
        app.add_hub([0xcd; 16], "rrc.hub", None);
        app.chose_identity_file(theirs_path.to_str().unwrap());
        let title = app.prompt.as_ref().map(|p| p.title.clone()).unwrap_or_default();
        assert!(title.contains("It changes your node's address and who you are to RRC hubs too."), "{title}");
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = (0..24).map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect();
        assert!(screen.contains("Type y") && screen.contains("RRC hubs"), "{screen}");
        app.prompt = None;
        app.identity_pending = Some(address);
        // Backing up waits for the restart (the file holds the new one).
        app.ask_identity_backup();
        assert!(app.prompt.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
