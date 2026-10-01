//! Emoji in what's being written: the picker (`Ctrl-E`), and `:name`
//! completion as it's typed, in the message box and the channel input.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use emojis::Emoji;

use super::{App, Tab};
use crate::emoji;
use crate::term::input::TextInput;

/// Which input emoji go into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmojiTarget {
    Compose,
    Channel,
    /// A reaction to the message in [`App::reacting`].
    Reaction,
}

/// The picker: a search, a tab for the recently used emoji and one for each
/// group, and a grid of the emoji shown.
#[derive(Debug)]
pub struct EmojiPicker {
    pub target: EmojiTarget,
    pub search: TextInput,
    /// The tab shown: 0 is the recently used, then [`emoji::groups`].
    pub tab: usize,
    /// The chosen emoji in [`App::emoji_choices`].
    pub pick: usize,
    /// The grid's first row on screen, and its size as last drawn (for
    /// moving by rows and pages).
    pub top: usize,
    pub columns: usize,
    pub rows: usize,
}

impl EmojiPicker {
    /// Scroll the grid so the chosen emoji is on screen.
    pub fn follow(&mut self) {
        let row = self.pick / self.columns.max(1);
        let rows = self.rows.max(1);
        if row < self.top {
            self.top = row;
        } else if row >= self.top + rows {
            self.top = row + 1 - rows;
        }
    }
}

/// What a click on the picker or the `:name` list lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmojiHit {
    /// An emoji in the picker's grid (its index) or the `:name` list.
    Pick(usize),
    Tab(usize),
}

impl App {
    /// The input being written in, if emoji can go into it.
    pub fn emoji_target(&self) -> Option<EmojiTarget> {
        match self.tab {
            Tab::Messages if self.composing && self.active_conversation.is_some() => Some(EmojiTarget::Compose),
            Tab::Channels if self.channels.typing => Some(EmojiTarget::Channel),
            _ => None,
        }
    }

    /// The input emoji go into (a reaction's picker opens over the message
    /// box).
    pub fn emoji_input(&self, target: EmojiTarget) -> &TextInput {
        match target {
            EmojiTarget::Compose | EmojiTarget::Reaction => &self.compose,
            EmojiTarget::Channel => &self.channels.input,
        }
    }

    fn emoji_input_mut(&mut self, target: EmojiTarget) -> &mut TextInput {
        match target {
            EmojiTarget::Compose | EmojiTarget::Reaction => &mut self.compose,
            EmojiTarget::Channel => &mut self.channels.input,
        }
    }

    /// Open the picker over the input being written in, at the recently
    /// used if there are any.
    pub fn open_emoji_picker(&mut self) {
        if let Some(target) = self.emoji_target() {
            self.open_emoji_picker_for(target);
        }
    }

    pub(super) fn open_emoji_picker_for(&mut self, target: EmojiTarget) {
        let tab = if emoji::recent(&self.store.recent_emoji).is_empty() { 1 } else { 0 };
        self.emoji = Some(EmojiPicker { target, search: TextInput::default(), tab, pick: 0, top: 0, columns: 1, rows: 1 });
    }

    /// What the picker shows: what matches the search, or the tab's emoji.
    pub fn emoji_choices(&self) -> Vec<&'static Emoji> {
        let Some(picker) = &self.emoji else { return Vec::new() };
        if !picker.search.text().trim().is_empty() {
            return emoji::search(picker.search.text());
        }
        match picker.tab {
            0 => emoji::recent(&self.store.recent_emoji),
            tab => emoji::groups().get(tab - 1).map(|g| g.emoji.clone()).unwrap_or_default(),
        }
    }

    /// Keys while the picker is open: arrows and pages move in the grid,
    /// `Tab` / `Shift-Tab` change tab, `Enter` puts the emoji in, `Esc` (or
    /// `Ctrl-E`) closes it, and the rest go to the search.
    pub(crate) fn emoji_picker_key(&mut self, key: KeyEvent) {
        let count = self.emoji_choices().len();
        let tabs = emoji::groups().len() + 1;
        let Some(picker) = &mut self.emoji else { return };
        let (columns, page) = (picker.columns.max(1), picker.columns.max(1) * picker.rows.max(1));
        let last = count.saturating_sub(1);
        match key.code {
            KeyCode::Esc => self.close_emoji_picker(),
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => self.close_emoji_picker(),
            KeyCode::Enter => {
                let chosen = self.emoji_choices().get(self.emoji.as_ref().map_or(0, |p| p.pick)).copied();
                if let Some(chosen) = chosen {
                    let target = self.emoji.take().map(|p| p.target);
                    if let Some(target) = target {
                        self.insert_emoji(target, chosen.as_str());
                    }
                }
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let step = if key.code == KeyCode::Tab { 1 } else { tabs - 1 };
                // Tab from a search goes to the tab it was on.
                if picker.search.text().is_empty() {
                    picker.tab = (picker.tab + step) % tabs;
                }
                picker.search = TextInput::default();
                (picker.pick, picker.top) = (0, 0);
            }
            KeyCode::Left => picker.pick = picker.pick.saturating_sub(1),
            KeyCode::Right => picker.pick = (picker.pick + 1).min(last),
            KeyCode::Up => picker.pick = picker.pick.saturating_sub(columns),
            KeyCode::Down if picker.pick + columns <= last => picker.pick += columns,
            KeyCode::Down => {}
            KeyCode::PageUp => picker.pick = picker.pick.saturating_sub(page),
            KeyCode::PageDown => picker.pick = (picker.pick + page).min(last),
            KeyCode::Home => picker.pick = 0,
            KeyCode::End => picker.pick = last,
            _ => {
                if picker.search.handle(key) {
                    (picker.pick, picker.top) = (0, 0);
                }
            }
        }
        if let Some(picker) = &mut self.emoji {
            picker.follow();
        }
    }

    /// A click on the picker: an emoji goes in, a tab is shown.
    pub(crate) fn click_emoji(&mut self, hit: EmojiHit) {
        match hit {
            EmojiHit::Pick(index) if self.emoji.is_some() => {
                if let Some(chosen) = self.emoji_choices().get(index).copied() {
                    let target = self.emoji.take().map(|p| p.target);
                    if let Some(target) = target {
                        self.insert_emoji(target, chosen.as_str());
                    }
                }
            }
            EmojiHit::Pick(index) => {
                if let Some((at, found)) = self.shortcode_matches() {
                    self.pick_shortcode(at, found[index.min(found.len() - 1)].as_str());
                }
            }
            EmojiHit::Tab(tab) => {
                if let Some(picker) = &mut self.emoji {
                    picker.tab = tab;
                    picker.search = TextInput::default();
                    (picker.pick, picker.top) = (0, 0);
                }
            }
        }
    }

    /// The wheel over the picker moves a row at a time.
    pub(crate) fn scroll_emoji(&mut self, delta: isize) {
        let last = self.emoji_choices().len().saturating_sub(1);
        if let Some(picker) = &mut self.emoji {
            let row = picker.columns.max(1);
            picker.pick = if delta < 0 { picker.pick.saturating_sub(row) } else { (picker.pick + row).min(last) };
            picker.follow();
        }
    }

    /// Close the picker without picking (nor reacting).
    pub(crate) fn close_emoji_picker(&mut self) {
        self.emoji = None;
        self.reacting = None;
    }

    /// Put `emoji` in at the input's cursor, and remember it (or react
    /// with it).
    fn insert_emoji(&mut self, target: EmojiTarget, emoji: &str) {
        if target == EmojiTarget::Reaction {
            return self.react_with(emoji);
        }
        self.emoji_input_mut(target).insert_str(emoji);
        self.remember_emoji(emoji);
    }

    /// Put `emoji` first among the recently used (saved with the store).
    pub fn remember_emoji(&mut self, emoji: &str) {
        emoji::remember(&mut self.store.recent_emoji, emoji);
        self.store_dirty = true;
    }

    /// While typing `:name` in the input being written in: where its `:` is
    /// and the emoji it could be. `None` when no name is being typed,
    /// nothing matches, Esc closed the list for this `:`, or the picker is
    /// open.
    pub fn shortcode_matches(&self) -> Option<(usize, Vec<&'static Emoji>)> {
        if self.emoji.is_some() {
            return None;
        }
        let input = self.emoji_input(self.emoji_target()?);
        let (at, found) = emoji::completions(input.before_cursor())?;
        (self.shortcode.dismissed != Some(at)).then_some((at, found))
    }

    /// Keys for the `:name` list while it's open (Up and Down choose, Tab or
    /// Enter picks, Esc closes it); true when the key was one of those.
    pub(crate) fn shortcode_key(&mut self, key: KeyEvent) -> bool {
        let Some((at, found)) = self.shortcode_matches() else { return false };
        let count = found.len();
        let pick = self.shortcode.pick.min(count - 1);
        match key.code {
            KeyCode::Down => self.shortcode.pick = (pick + 1) % count,
            KeyCode::Up => self.shortcode.pick = (pick + count - 1) % count,
            KeyCode::Tab | KeyCode::Enter => self.pick_shortcode(at, found[pick].as_str()),
            KeyCode::Esc => self.shortcode.dismissed = Some(at),
            _ => return false,
        }
        true
    }

    /// After a key in the input: a `:name:` finished by typing its `:`
    /// (`colon`) becomes its emoji, and a new `:` gets its list back.
    pub(crate) fn shortcode_typed(&mut self, colon: bool) {
        let Some(target) = self.emoji_target() else { return };
        self.shortcode.pick = 0;
        let before = self.emoji_input(target).before_cursor();
        if let Some((at, found)) = emoji::finished(before).filter(|_| colon) {
            let end = before.len();
            self.emoji_input_mut(target).replace(at, end, found.as_str());
            self.remember_emoji(found.as_str());
            return;
        }
        let at = emoji::completions(before).map(|(at, _)| at);
        if at != self.shortcode.dismissed {
            self.shortcode.dismissed = None;
        }
    }

    /// Put `emoji` in place of the `:name` typed from byte `at`.
    fn pick_shortcode(&mut self, at: usize, emoji: &str) {
        let Some(target) = self.emoji_target() else { return };
        let input = self.emoji_input_mut(target);
        let end = input.before_cursor().len();
        input.replace(at, end, emoji);
        self.shortcode.pick = 0;
        self.remember_emoji(emoji);
    }
}

/// The `:name` list's state: the emoji chosen in it, and the `:` whose
/// list Esc closed.
#[derive(Debug, Default)]
pub struct Shortcode {
    pub pick: usize,
    pub dismissed: Option<usize>,
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyEvent, KeyModifiers};

    use super::*;

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn composing(name: &str) -> App {
        let dir = std::env::temp_dir().join(format!("rettui-emoji-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        app.active_conversation = Some("ab".repeat(16));
        app.tab = Tab::Messages;
        app.composing = true;
        app
    }

    #[test]
    fn a_finished_name_becomes_its_emoji() {
        let mut app = composing("finished");
        typed(&mut app, "here be :dragon: ok");
        assert_eq!(app.compose.text(), "here be 🐉 ok");
        assert_eq!(app.store.recent_emoji, ["🐉"]);
        // Not a time, or a word that isn't a shortcode.
        typed(&mut app, " 12:30: :nope:");
        assert_eq!(app.compose.text(), "here be 🐉 ok 12:30: :nope:");
        // Only typing the `:` does it: a pasted one stays as it is, even
        // with the cursor moved back to it.
        app.on_paste(" :tada:");
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Right);
        assert!(app.compose.text().ends_with(" :tada:"));
    }

    #[test]
    fn names_are_completed_from_a_list() {
        let mut app = composing("list");
        typed(&mut app, "hi :drag");
        let (at, found) = app.shortcode_matches().unwrap();
        assert_eq!((at, found[0].as_str()), (3, "🐉"));
        // Down chooses the next; Tab puts it in for what was typed.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.compose.text(), format!("hi {}", found[1].as_str()));
        // Esc closes the list for that `:` (and not the input).
        typed(&mut app, " :thumbs");
        assert!(app.shortcode_matches().is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.shortcode_matches().is_none());
        assert!(app.composing);
        typed(&mut app, "u");
        assert!(app.shortcode_matches().is_none());
        // A new `:` gets it back.
        typed(&mut app, " :thumbs");
        assert!(app.shortcode_matches().is_some());
    }

    #[test]
    fn the_channel_input_takes_emoji_too() {
        let mut app = composing("channel");
        app.tab = Tab::Channels;
        app.channels.typing = true;
        typed(&mut app, "gg :tada: ");
        assert_eq!(app.channels.input.text(), "gg 🎉 ");
        app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert_eq!(app.emoji.as_ref().map(|p| p.target), Some(EmojiTarget::Channel));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.channels.input.text(), "gg 🎉 🎉");
    }

    #[test]
    fn the_picker_puts_the_chosen_emoji_in() {
        let mut app = composing("picker");
        typed(&mut app, "a ");
        app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        // Nothing used yet: it opens at the smileys.
        assert_eq!(app.emoji.as_ref().map(|p| p.tab), Some(1));
        app.emoji.as_mut().unwrap().columns = 4;
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Down);
        let chosen = app.emoji_choices()[5].as_str();
        press(&mut app, KeyCode::Enter);
        assert!(app.emoji.is_none());
        assert_eq!(app.compose.text(), format!("a {chosen}"));
        // Typing searches; it opens at the recently used now.
        app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        assert_eq!(app.emoji.as_ref().map(|p| p.tab), Some(0));
        assert_eq!(app.emoji_choices()[0].as_str(), chosen);
        typed(&mut app, "dragon");
        assert_eq!(app.emoji_choices()[0].as_str(), "🐉");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.compose.text(), format!("a {chosen}🐉"));
        assert_eq!(app.store.recent_emoji[0], "🐉");
        // Tab moves through the tabs; Esc closes without putting one in.
        app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL));
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.emoji.as_ref().map(|p| p.tab), Some(1));
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.emoji.as_ref().map(|p| p.tab), Some(emoji::groups().len()));
        press(&mut app, KeyCode::Esc);
        assert!(app.emoji.is_none() && app.composing);
        assert_eq!(app.compose.text(), format!("a {chosen}🐉"));
    }
}
