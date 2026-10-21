//! Several identities (see `crate::identities`): the list in the Status
//! tab (`I`), keeping another, switching, renaming, deleting and sharing
//! one's address as a QR code. Switching saves everything, then starts
//! over as the other identity (its settings, conversations and network).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;

use super::{App, PromptKind, files};
use crate::identities::{self, Profile};

/// The identities list, open over the Status tab.
#[derive(Debug, Default)]
pub struct IdentitiesView {
    pub selected: usize,
    /// The first shown (the list scrolls).
    pub top: usize,
    pub list: Vec<Profile>,
}

impl App {
    /// The identities kept here, the one in use marked.
    pub fn identities(&self) -> Vec<Profile> {
        identities::list(&self.paths.root)
    }

    /// Keep another identity named `name`: a new one, or the one in the
    /// file `from` (as Sideband, NomadNet, MeshChat or rettui keep them).
    pub fn add_identity(&mut self, name: &str, from: Option<&std::path::Path>) -> Result<Profile, String> {
        let identity = from.map(super::identity::read_identity).transpose()?;
        let profile = identities::add(&self.paths, &self.settings, name, identity)?;
        self.log(format!("Kept another identity, {} ({})", profile.name, hex::encode(profile.lxmf)));
        Ok(profile)
    }

    /// Switch to the identity `id`: everything is saved, then rettui starts
    /// over as it.
    pub fn switch_identity(&mut self, id: &str) -> Result<Profile, String> {
        let profile = identities::find(&self.paths.root, id)?;
        if profile.active {
            return Err(format!("{} is the identity in use already", profile.name));
        }
        let profile = identities::choose(&self.paths.root, id)?;
        self.save_if_dirty();
        self.log(format!("Switching to {} ({})", profile.name, hex::encode(profile.lxmf)));
        self.switching = Some(profile.name.clone());
        Ok(profile)
    }

    /// The identity to switch to, once: the run loop starts over as it.
    pub fn take_switch(&mut self) -> Option<String> {
        self.switching.take()
    }

    /// Rename the identity `id`: the one in use by its display name.
    pub fn rename_identity(&mut self, id: &str, name: &str) -> Result<(), String> {
        let profile = identities::find(&self.paths.root, id)?;
        if profile.active {
            return self.update_settings(&[("display_name", name)]).map(|_| ());
        }
        identities::rename(&self.paths.root, id, name)
    }

    /// Delete the identity `id` (not the first, nor the one in use).
    pub fn delete_identity(&mut self, id: &str) -> Result<Profile, String> {
        let profile = identities::delete(&self.paths.root, id)?;
        self.log(format!("Deleted the identity {} ({})", profile.name, hex::encode(profile.lxmf)));
        Ok(profile)
    }

    // ---- The list (terminal UI) ---------------------------------------------

    /// `I` in the Status tab.
    pub(super) fn open_identities(&mut self) {
        let list = self.identities();
        let selected = list.iter().position(|p| p.active).unwrap_or(0);
        self.identities_view = Some(IdentitiesView { selected, top: 0, list });
    }

    fn refresh_identities(&mut self) {
        let list = self.identities();
        if let Some(view) = &mut self.identities_view {
            view.selected = view.selected.min(list.len().saturating_sub(1));
            view.list = list;
        }
    }

    fn selected_identity(&self) -> Option<Profile> {
        let view = self.identities_view.as_ref()?;
        view.list.get(view.selected).cloned()
    }

    pub(super) fn identities_key(&mut self, key: KeyEvent) {
        let Some(view) = &mut self.identities_view else { return };
        let last = view.list.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.identities_view = None,
            KeyCode::Down | KeyCode::Char('j') => view.selected = (view.selected + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => view.selected = view.selected.saturating_sub(1),
            KeyCode::Home => view.selected = 0,
            KeyCode::End => view.selected = last,
            KeyCode::Char('n') => self.open_prompt(PromptKind::NewIdentity(None), "Name of the new identity (the one it announces)", ""),
            KeyCode::Char('i') => self.open_prompt(
                PromptKind::IdentityFromFile,
                "Identity file to keep as another identity (from Sideband, NomadNet, MeshChat or another rettui)",
                "",
            ),
            _ => {
                let Some(profile) = self.selected_identity() else { return };
                match key.code {
                    KeyCode::Enter if profile.active => self.warn(format!("{} is the identity in use", profile.name)),
                    KeyCode::Enter => {
                        let title = format!(
                            "Switch to {} ({})? Everything is saved, and rettui starts again as it. Type y",
                            profile.name,
                            hex::encode(profile.lxmf)
                        );
                        self.open_prompt(PromptKind::ConfirmSwitchIdentity(profile.id), &title, "");
                    }
                    KeyCode::Char('r') => {
                        self.open_prompt(PromptKind::RenameIdentity(profile.id.clone()), "Name (the one it announces)", &profile.name)
                    }
                    KeyCode::Char('c') => self.paper_view = Some(super::PaperView::address(profile.link)),
                    KeyCode::Char('y') => self.copy(&profile.link, &format!("{}'s LXMF address, with its key", profile.name)),
                    KeyCode::Char('x') | KeyCode::Delete if profile.id.is_empty() => {
                        self.warn("The first identity stays (it's the data directory's own)")
                    }
                    KeyCode::Char('x') | KeyCode::Delete if profile.active => {
                        self.warn("That's the identity in use: switch to another first")
                    }
                    KeyCode::Char('x') | KeyCode::Delete => {
                        let title = format!(
                            "Delete {} ({}) for good, with its key, conversations and contacts? Its address is gone unless it's backed up. Type its name",
                            profile.name,
                            hex::encode(profile.lxmf)
                        );
                        self.open_prompt(PromptKind::ConfirmDeleteIdentity(profile.id), &title, "");
                    }
                    _ => {}
                }
            }
        }
    }

    pub(super) fn click_identities(&mut self, at: Position, double: bool) {
        let list = self.regions.identities_list;
        if !list.contains(at) {
            return;
        }
        let Some(view) = &mut self.identities_view else { return };
        // Each identity has two rows.
        let index = view.top + (at.y - list.y) as usize / 2;
        if index < view.list.len() {
            view.selected = index;
            if double {
                self.identities_key(KeyEvent::from(KeyCode::Enter));
            }
        }
    }

    pub(super) fn submit_identities_prompt(&mut self, kind: PromptKind, text: &str) {
        let yes = text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes");
        match kind {
            PromptKind::NewIdentity(from) => match self.add_identity(text, from.as_deref()) {
                Ok(profile) => {
                    self.refresh_identities();
                    if let Some(view) = &mut self.identities_view {
                        view.selected = view.list.iter().position(|p| p.id == profile.id).unwrap_or(view.selected);
                    }
                    self.confirm(format!("Kept {} ({}): Enter switches to it", profile.name, hex::encode(profile.lxmf)));
                }
                Err(e) => self.warn(e),
            },
            PromptKind::IdentityFromFile if !text.is_empty() => {
                let path = files::path_from_input(text);
                match super::identity::read_identity(&path) {
                    Ok(_) => self.open_prompt(PromptKind::NewIdentity(Some(path)), "Its name (the one it announces)", ""),
                    Err(e) => self.warn(e),
                }
            }
            PromptKind::RenameIdentity(id) => match self.rename_identity(&id, text) {
                Ok(()) => {
                    self.refresh_identities();
                    self.confirm("Renamed");
                }
                Err(e) => self.warn(e),
            },
            PromptKind::ConfirmSwitchIdentity(id) if yes => {
                if let Err(e) = self.switch_identity(&id) {
                    self.warn(e);
                }
            }
            PromptKind::ConfirmDeleteIdentity(id) => {
                let named = self.identities().into_iter().find(|p| p.id == id).is_some_and(|p| p.name == text.trim());
                if !named {
                    return self.warn("Not deleted: that isn't its name");
                }
                match self.delete_identity(&id) {
                    Ok(profile) => {
                        self.refresh_identities();
                        self.confirm(format!("Deleted {}", profile.name));
                    }
                    Err(e) => self.warn(e),
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent};

    use crate::app::{App, Tab};
    use crate::config::Settings;
    use crate::store::Store;

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::from(code));
    }

    fn answer(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
        press(app, KeyCode::Enter);
    }

    #[test]
    fn identities_kept_and_switched_from_the_status_tab() {
        let dir = std::env::temp_dir().join(format!("rettui-app-identities-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = Settings { display_name: "Amy".into(), ..Settings::default() };
        let (mut app, _net) = crate::app::test_app_with_net(&dir, settings, Store::default());
        crate::config::load_identity(&app.paths.identity).unwrap();
        app.tab = Tab::Status;
        press(&mut app, KeyCode::Char('I'));
        assert_eq!(app.identities_view.as_ref().unwrap().list.len(), 1);
        // The first can't go, and is in use.
        press(&mut app, KeyCode::Char('x'));
        assert!(app.prompt.is_none());
        press(&mut app, KeyCode::Enter);
        assert!(app.prompt.is_none() && app.take_switch().is_none());
        // A new one, then one from a file.
        press(&mut app, KeyCode::Char('n'));
        answer(&mut app, "Amy at work");
        let file = dir.join("brought");
        rns_identity::identity::Identity::new().to_file(&file).unwrap();
        press(&mut app, KeyCode::Char('i'));
        answer(&mut app, &file.display().to_string());
        answer(&mut app, "Bee");
        let names: Vec<String> = app.identities_view.as_ref().unwrap().list.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, ["Amy", "Amy at work", "Bee"]);
        assert_eq!(app.identities_view.as_ref().unwrap().selected, 2, "the one just kept");
        // Its QR code, renamed, deleted (by its name only).
        press(&mut app, KeyCode::Char('c'));
        assert!(app.paper_view.take().is_some_and(|v| v.link.starts_with("lxma://")));
        press(&mut app, KeyCode::Char('r'));
        for _ in 0..3 {
            press(&mut app, KeyCode::Backspace);
        }
        answer(&mut app, "Bea");
        press(&mut app, KeyCode::Char('x'));
        answer(&mut app, "Bee");
        assert_eq!(app.identities().len(), 3, "not its name");
        press(&mut app, KeyCode::Char('x'));
        answer(&mut app, "Bea");
        assert_eq!(app.identities().len(), 2);
        // The one in use is renamed through its display name.
        app.rename_identity("", "Amy A").unwrap();
        assert_eq!((app.settings.display_name.as_str(), app.identities()[0].name.as_str()), ("Amy A", "Amy A"));
        // Switched: asked, then the run loop is told.
        assert_eq!(app.identities_view.as_ref().unwrap().selected, 1);
        press(&mut app, KeyCode::Enter);
        answer(&mut app, "y");
        assert_eq!(app.take_switch().as_deref(), Some("Amy at work"));
        assert_eq!(crate::identities::active_dir(&app.paths.root), app.identities()[1].dir);
        press(&mut app, KeyCode::Esc);
        assert!(app.identities_view.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
