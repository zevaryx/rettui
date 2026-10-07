//! Backing everything up while rettui runs (see [`crate::backup`]): once
//! what's waiting to be saved is written, on the saving thread.

use std::path::PathBuf;

use super::{App, PromptKind, files};
use crate::backup::{self, Options};

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
        match self.back_up_all(to.clone()) {
            Ok(()) => self.confirm(format!("Backing up to {}…", to.display())),
            Err(e) => self.warn(e),
        }
    }

    /// Back up to the new file `to`, with the identity and without
    /// attachments; the log says when it's done.
    pub fn back_up_all(&mut self, to: PathBuf) -> Result<(), String> {
        if to.exists() {
            return Err(format!("{} exists already: pick another name", to.display()));
        }
        self.save_if_dirty();
        let base = self.paths.base.clone();
        self.saver.run("back up", move || {
            let count = backup::write_file(&base, &to, Options { identity: true, files: false }).map_err(anyhow::Error::msg)?;
            Ok(vec![format!(
                "Backed up {count} files to {}. It has your identity: keep it private, as whoever has it can pose as you",
                to.display()
            )])
        });
        Ok(())
    }

    /// A backup for the web UI to download, written to `to`: never with the
    /// identity (the web login mustn't hand out the key), nor attachments.
    /// `done` hears how many files it holds.
    pub fn back_up_for_download(&mut self, to: PathBuf, done: tokio::sync::oneshot::Sender<Result<usize, String>>) {
        self.save_if_dirty();
        let base = self.paths.base.clone();
        self.saver.run("back up", move || {
            let made = backup::write_file(&base, &to, Options::default());
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
        app.back_up_all(to.clone()).unwrap();
        assert!(app.back_up_all(dir.join("settings.json")).unwrap_err().contains("exists already"));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let web = dir.join("web.tar.gz");
        app.back_up_for_download(web.clone(), tx);
        app.saver.finish();
        // The store was written first, so it's in both; the identity only
        // in the terminal UI's.
        let restored = dir.join("restored");
        backup::restore(&to, &restored, false).unwrap();
        assert!(restored.join("store.json.gz").exists() && restored.join("identity").exists());
        assert!(rx.blocking_recv().unwrap().is_ok());
        let restored = dir.join("restored-web");
        backup::restore(&web, &restored, false).unwrap();
        assert!(restored.join("store.json.gz").exists() && !restored.join("identity").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
