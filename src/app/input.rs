//! Keyboard and mouse: global keys, then the focused editor or tab.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use super::{App, BrowserFocus, PromptKind, Tab};
use crate::net::NetCommand;
use crate::nomad::micron;
use crate::term::selection::Selection;

const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

impl App {
    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Char('c') && ctrl {
            self.should_quit = true;
            return;
        }
        // Repaint the whole screen, for anything a terminal drew differently
        // from what was sent (an emoji drawn wider than it should be).
        if key.code == KeyCode::Char('l') && ctrl {
            self.full_redraw = true;
            return;
        }
        if let Some(view) = &self.paper_view {
            match key.code {
                KeyCode::Char('y') => {
                    let (link, what) = match view.kind {
                        super::QrKind::Paper => (view.link.clone(), "paper message"),
                        super::QrKind::Address => (view.link.clone(), "your contact link"),
                    };
                    self.copy(&link, what);
                }
                KeyCode::Char('s') => self.save_paper_qr(),
                _ => self.paper_view = None,
            }
            return;
        }
        if self.emoji.is_some() {
            match key.code {
                KeyCode::Char('v') if ctrl => self.paste_from_clipboard(),
                _ => self.emoji_picker_key(key),
            }
            return;
        }
        if let Some(mut prompt) = self.prompt.take() {
            if key.code == KeyCode::Char('v') && ctrl {
                self.prompt = Some(prompt);
                self.paste_from_clipboard();
                return;
            }
            match key.code {
                KeyCode::Esc if prompt.kind == PromptKind::Attach => self.composing = true,
                KeyCode::Esc => {}
                KeyCode::Enter => self.submit_prompt(prompt),
                _ => {
                    prompt.input.handle(key);
                    self.prompt = Some(prompt);
                }
            }
            return;
        }
        if self.guide.is_some() {
            self.guide_key(key);
            return;
        }
        if self.contact_card.is_some() && self.tab == Tab::Messages {
            self.contact_card_key(key);
            return;
        }
        if self.tab == Tab::Channels {
            self.channels.sync_draft();
        }
        if self.tab == Tab::Channels && self.channels.menu.is_some() {
            self.user_menu_key(key);
            return;
        }
        if self.tab == Tab::Channels && self.channels.picker.is_some() {
            self.member_picker_key(key);
            return;
        }
        if self.channels.typing && self.tab == Tab::Channels {
            if self.shortcode_key(key) || self.mention_key(key) {
                return;
            }
            match key.code {
                KeyCode::Esc => self.channels.typing = false,
                KeyCode::Enter => self.submit_channel_input(),
                KeyCode::PageUp => self.channels.scroll = self.channels.scroll.saturating_add(self.channel_page()),
                KeyCode::PageDown => self.channels.scroll = self.channels.scroll.saturating_sub(self.channel_page()),
                KeyCode::Char('v') if ctrl => self.paste_from_clipboard(),
                KeyCode::Char('e') if ctrl => self.open_emoji_picker(),
                _ => {
                    self.channels.input.handle(key);
                    self.mention_typed();
                    self.shortcode_typed(key.code == KeyCode::Char(':'));
                }
            }
            return;
        }
        if self.node.editing && self.tab == Tab::Node && self.node.editor.is_some() {
            self.node_editor_key(key);
            return;
        }
        if self.tab == Tab::Reticulum && self.rns.picker.is_some() {
            self.rns_picker_key(key);
            return;
        }
        if self.tab == Tab::Reticulum && self.rns.editor.is_some() {
            self.rns_editor_key(key);
            return;
        }
        if self.net_search.typing && self.tab == Tab::Network {
            match key.code {
                KeyCode::Char('v') if ctrl => self.paste_from_clipboard(),
                _ => self.network_search_key(key),
            }
            return;
        }
        if self.browser.search.typing && self.tab == Tab::Browser {
            match key.code {
                KeyCode::Char('v') if ctrl => self.paste_from_clipboard(),
                _ => self.browser_search_key(key),
            }
            return;
        }
        if self.composing && self.tab == Tab::Messages {
            if self.shortcode_key(key) {
                return;
            }
            match key.code {
                // Esc gives up the reply first, then the writing.
                KeyCode::Esc if self.reply.is_some() => self.reply = None,
                KeyCode::Esc => self.composing = false,
                KeyCode::Char('r') if ctrl => self.move_reply(true),
                KeyCode::Up if self.reply.is_some() => self.move_reply(true),
                KeyCode::Down if self.reply.is_some() => self.move_reply(false),
                KeyCode::Enter => self.send_compose(),
                KeyCode::PageUp => self.message_scroll = self.message_scroll.saturating_add(self.history_page()),
                KeyCode::PageDown => self.message_scroll = self.message_scroll.saturating_sub(self.history_page()),
                KeyCode::Char('o') if ctrl => self.open_attach_prompt(),
                KeyCode::Char('x') if ctrl => self.attachments.clear(),
                KeyCode::Char('p') if ctrl => self.cycle_delivery(),
                KeyCode::Char('v') if ctrl => self.paste_from_clipboard(),
                KeyCode::Char('e') if ctrl => self.open_emoji_picker(),
                _ => {
                    self.compose.handle(key);
                    self.shortcode_typed(key.code == KeyCode::Char(':'));
                }
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char(c @ '1'..='7') => {
                self.switch_tab(Tab::ALL[c as usize - '1' as usize]);
                return;
            }
            KeyCode::Char('A') => {
                self.send(NetCommand::Announce);
                return;
            }
            KeyCode::Char('S') => {
                self.send(NetCommand::Sync);
                return;
            }
            _ => {}
        }

        match self.tab {
            Tab::Messages => self.messages_key(key),
            Tab::Channels => self.channels_key(key),
            Tab::Network => self.network_key(key),
            Tab::Browser => self.browser_key(key),
            Tab::Node => self.node_key(key),
            Tab::Status => self.status_key(key),
            Tab::Reticulum => self.rns_key(key),
        }
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        if self.paper_view.is_some() {
            if let MouseEventKind::Down(_) = mouse.kind {
                self.paper_view = None;
            }
            return;
        }
        if self.prompt.is_some() {
            return;
        }
        let at = Position::new(mouse.column, mouse.row);
        if self.guide.is_some() {
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                self.click_guide(at);
            }
            return;
        }
        if self.contact_card.is_some() && self.tab == Tab::Messages {
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                self.click_contact_card(at);
            }
            return;
        }
        // The emoji picker and the `:name` list take clicks and the wheel
        // over them; a click elsewhere closes the picker.
        let over_emoji = self.regions.emoji_popup.contains(at)
            && (self.emoji.is_some() || self.shortcode_matches().is_some());
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if over_emoji => {
                let delta = if mouse.kind == MouseEventKind::ScrollUp { -1 } else { 1 };
                if self.emoji.is_some() {
                    self.scroll_emoji(delta);
                }
                return;
            }
            MouseEventKind::Down(MouseButton::Left) if over_emoji => {
                if let Some(&(_, hit)) = self.regions.emoji_hits.iter().find(|(rect, _)| rect.contains(at)) {
                    self.click_emoji(hit);
                }
                return;
            }
            MouseEventKind::Down(_) => self.close_emoji_picker(),
            _ => {}
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll(at, -3),
            MouseEventKind::ScrollDown => self.scroll(at, 3),
            // On a page, press starts a text selection; a release without
            // dragging is a click (follow a link, edit a field).
            MouseEventKind::Down(MouseButton::Left)
                if self.tab == Tab::Browser && self.regions.page.contains(at) =>
            {
                let (row, col) = self.page_cell(at);
                self.browser.focus = BrowserFocus::Page;
                self.browser.selection = Some(Selection::at(row, col));
                self.browser.dragging = true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.browser.dragging => {
                // Dragging past the edges scrolls the page.
                let page = self.regions.page;
                if at.y < page.y {
                    self.browser.scroll = self.browser.scroll.saturating_sub(1);
                } else if at.y >= page.bottom() {
                    self.browser.scroll += 1;
                }
                let head = self.page_cell(at);
                if let Some(selection) = &mut self.browser.selection {
                    selection.head = head;
                }
            }
            MouseEventKind::Up(MouseButton::Left) if self.browser.dragging => {
                self.browser.dragging = false;
                match self.browser.selection {
                    Some(selection) if !selection.is_empty() => {
                        let text = selection.extract(&self.regions.page_text);
                        self.copy(&text, "selection");
                    }
                    Some(selection) => {
                        self.browser.selection = None;
                        let (row, col) = selection.anchor;
                        if let Some(item) = micron::hit_at(&self.regions.page_hits, row, col) {
                            self.activate(item);
                        }
                    }
                    None => {}
                }
            }
            // In the page editor, dragging selects text.
            MouseEventKind::Drag(MouseButton::Left) if self.tab == Tab::Node && self.node.dragging => self.drag_node(at),
            MouseEventKind::Up(MouseButton::Left) if self.node.dragging => self.node.dragging = false,
            MouseEventKind::Down(MouseButton::Left) => {
                let double = self.last_click.is_some_and(|(when, pos)| {
                    pos.y == at.y && when.elapsed() < DOUBLE_CLICK
                });
                self.last_click = Some((Instant::now(), at));
                self.click(at, double);
            }
            MouseEventKind::Down(MouseButton::Right) if self.tab == Tab::Browser => self.back(),
            _ => {}
        }
    }

    fn scroll(&mut self, at: Position, delta: isize) {
        let apply = |value: usize| value.saturating_add_signed(delta);
        match self.tab {
            Tab::Messages if self.regions.conversations.contains(at) => {
                let count = self.store.conversations.len();
                if count > 0 {
                    let i = self.conversations.selected().unwrap_or(0);
                    self.select_conversation(i.saturating_add_signed(delta.signum()).min(count - 1));
                }
            }
            // Message history is anchored at the bottom: wheel up goes back.
            Tab::Messages => self.message_scroll = self.message_scroll.saturating_add_signed(-delta),
            Tab::Channels if self.regions.channel_list.contains(at) => {
                self.move_channel_selection(delta.signum());
            }
            Tab::Channels => {
                self.channels.scroll = self.channels.scroll.saturating_add_signed(-delta);
            }
            Tab::Network => {
                let count = self.network_rows().len();
                if count > 0 {
                    let i = self.peers.selected().unwrap_or(0);
                    self.peers.select(Some(apply(i).min(count - 1)));
                }
            }
            Tab::Browser if self.regions.browser_list.contains(at) => {
                self.move_pane_selection(delta.signum());
            }
            Tab::Browser => self.browser.scroll = apply(self.browser.scroll),
            Tab::Node => self.scroll_node(at, delta),
            Tab::Status if self.regions.settings.contains(at) => self.move_setting(delta.signum()),
            Tab::Status => {}
            Tab::Reticulum => self.scroll_rns(at, delta),
        }
    }

    fn click(&mut self, at: Position, double: bool) {
        if self.regions.version.contains(at) {
            return self.open_url(crate::config::PROJECT_URL);
        }
        if let Some(&(_, tab)) = self.regions.tabs.iter().find(|(r, _)| r.contains(at)) {
            self.switch_tab(tab);
            return;
        }
        match self.tab {
            Tab::Messages => self.click_messages(at),
            Tab::Channels => self.click_channels(at),
            Tab::Network => self.click_network(at, double),
            Tab::Browser => self.click_browser(at),
            Tab::Node => self.click_node(at, double),
            Tab::Status => self.click_status(at, double),
            Tab::Reticulum if self.rns.picker.is_some() => self.click_rns_picker(at),
            Tab::Reticulum => self.click_rns(at, double),
        }
    }
}
