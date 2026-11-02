//! Backing everything up while rettui runs (see [`crate::backup`]): once
//! what's waiting to be saved is written, on the saving thread.

use std::path::PathBuf;

use zeroize::Zeroizing;

use super::{App, PromptKind, files};
use crate::backup::{self, Options, sealed::BackupKey};

impl App {
    /// Ask where to save a backup (`B` in the Status tab).
    pub(super) fn ask_backup(&mut self) {
        let suggested = format!("~/{}", backup::default_name());
        self.open_prompt(PromptKind::BackUpAll, "Back up settings, contacts, messages and your identity to", &suggested);
    }

    pub(super) fn submit_backup_prompt(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let to = files::path_from_input(text);
        if to.exists() {
            return self.warn(format!("{} exists already: pick another name", to.display()));
        }
        // Then, if they like, a passphrase to seal it with.
        self.open_prompt(
            PromptKind::BackUpPassphrase(to),
            "A passphrase to seal it with, so only someone who knows it can restore it (Enter: none)",
            "",
        );
    }

    /// The passphrase typed (`again`: the second time, to check it).
    pub(super) fn submit_backup_passphrase(&mut self, to: PathBuf, typed: &str, again: bool) {
        let typed = Zeroizing::new(typed.to_string());
        if !again {
            if typed.is_empty() {
                return self.start_backup(to, None);
            }
            self.backup_passphrase = Some(typed);
            self.open_prompt(PromptKind::BackUpPassphraseAgain(to), "The passphrase again", "");
            return;
        }
        let first = self.backup_passphrase.take().unwrap_or_default();
        if *first != *typed {
            return self.warn("The passphrases differ: nothing was backed up. Press B to try again.");
        }
        self.start_backup(to, Some(typed));
    }

    fn start_backup(&mut self, to: PathBuf, passphrase: Option<Zeroizing<String>>) {
        match self.back_up_all(to.clone(), passphrase) {
            Ok(()) => self.confirm(format!("Backing up to {}…", to.display())),
            Err(e) => self.warn(e),
        }
    }

    /// Back up to the new file `to`, with the identity and without
    /// attachments, sealed with `passphrase` if there is one; the log says
    /// when it's done.
    pub fn back_up_all(&mut self, to: PathBuf, passphrase: Option<Zeroizing<String>>) -> Result<(), String> {
        if to.exists() {
            return Err(format!("{} exists already: pick another name", to.display()));
        }
        self.save_if_dirty();
        let base = self.paths.base.clone();
        self.saver.run("back up", move || {
            // Stretching the passphrase takes a moment: here, not on screen.
            let key = passphrase.map(|p| BackupKey::from_passphrase(&p)).transpose().map_err(anyhow::Error::msg)?;
            let count =
                backup::write_file(&base, &to, Options { identity: true, files: false }, key.as_ref()).map_err(anyhow::Error::msg)?;
            Ok(vec![if key.is_some() {
                format!("Backed up {count} files to {}, sealed with your passphrase: it can't be restored without it", to.display())
            } else {
                format!(
                    "Backed up {count} files to {}. It has your identity: keep it private, as whoever has it can pose as you",
                    to.display()
                )
            }])
        });
        Ok(())
    }

    /// A backup for the web UI to download, written to `to`: never with the
    /// identity (the web login mustn't hand out the key), nor attachments.
    /// `done` hears how many files it holds.
    /// Sealed with `passphrase`, if there is one.
    pub fn back_up_for_download(
        &mut self,
        to: PathBuf,
        passphrase: Option<Zeroizing<String>>,
        done: tokio::sync::oneshot::Sender<Result<usize, String>>,
    ) {
        self.save_if_dirty();
        let base = self.paths.base.clone();
        self.saver.run("back up", move || {
            let made = passphrase
                .map(|p| BackupKey::from_passphrase(&p))
                .transpose()
                .and_then(|key| backup::write_file(&base, &to, Options::default(), key.as_ref()));
            let note = made.as_ref().ok().map(|count| format!("A browser downloaded a backup of {count} files (without your identity)"));
            let _ = done.send(made);
            Ok(note.into_iter().collect())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backed_up_after_what_was_waiting_to_be_saved() {
        let dir = std::env::temp_dir().join(format!("rettui-app-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        std::fs::write(&app.paths.identity, [7u8; 64]).unwrap();
        app.store_dirty = true;
        let to = dir.join("out.tar.gz");
        app.back_up_all(to.clone(), None).unwrap();
        assert!(app.back_up_all(dir.join("settings.json"), None).unwrap_err().contains("exists already"));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let web = dir.join("web.tar.gz");
        app.back_up_for_download(web.clone(), None, tx);
        app.saver.finish();
        // The store was written first, so it's in both; the identity only
        // in the terminal UI's.
        let restored = dir.join("restored");
        backup::restore(&to, &restored, false, None).unwrap();
        assert!(restored.join("store.json.gz").exists() && restored.join("identity").exists());
        assert!(rx.blocking_recv().unwrap().is_ok());
        let restored = dir.join("restored-web");
        backup::restore(&web, &restored, false, None).unwrap();
        assert!(restored.join("store.json.gz").exists() && !restored.join("identity").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sealed_with_a_passphrase_typed_twice() {
        let dir = std::env::temp_dir().join(format!("rettui-app-backup-sealed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        std::fs::write(&app.paths.identity, [7u8; 64]).unwrap();
        let to = dir.join("sealed.tar.gz");
        // Typed differently the second time: nothing is made.
        app.submit_backup_passphrase(to.clone(), "one thing", false);
        assert!(matches!(app.prompt.as_ref().map(|p| &p.kind), Some(PromptKind::BackUpPassphraseAgain(_))));
        app.prompt = None;
        app.submit_backup_passphrase(to.clone(), "another", true);
        assert!(app.notice.as_ref().is_some_and(|n| n.text.contains("differ")));
        // The same twice: sealed with it.
        app.submit_backup_passphrase(to.clone(), "rosebud", false);
        app.prompt = None;
        app.submit_backup_passphrase(to.clone(), "rosebud", true);
        assert!(app.backup_passphrase.is_none());
        // None typed: a plain one.
        let plain = dir.join("plain.tar.gz");
        app.submit_backup_passphrase(plain.clone(), "", false);
        app.saver.finish();
        assert!(backup::is_sealed(&to));
        let restored = dir.join("restored");
        backup::restore(&to, &restored, false, Some("rosebud")).unwrap();
        assert!(restored.join("identity").exists());
        assert!(plain.exists() && !backup::is_sealed(&plain));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
