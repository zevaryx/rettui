//! Node tab: hosting a NomadNet node and editing its pages.
//!
//! The page operations (`node_*`) are shared by the TUI and the web UI.
//! Saving writes the file the node serves, so edits are live straight away;
//! added, renamed and deleted pages also make the node reload its routes.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nomad_core::NomadServeStats;
use ratatui::layout::Position;
use ratatui::widgets::ListState;

use super::{App, PromptKind, format};
use crate::net::{Hash, HostEvent, NetCommand};
use crate::nomad::host::HostConfig;
use crate::nomad::pages::{self, PageInfo, Pages};
use crate::term::textarea::TextArea;

/// Starting content for a new page.
const NEW_PAGE: &str = ">New page\n\nWrite your page here in Micron.\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeStatus {
    Off,
    Starting,
    Running,
    Failed(String),
}

/// A page open in the TUI editor.
pub struct Editor {
    pub path: String,
    pub area: TextArea,
    /// Text as last saved, to tell whether there are unsaved changes.
    saved: String,
    pub executable: bool,
}

/// What the right-hand side shows for an open page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageView {
    Editor,
    Split,
    Preview,
}

impl PageView {
    pub fn next(self) -> Self {
        match self {
            PageView::Split => PageView::Editor,
            PageView::Editor => PageView::Preview,
            PageView::Preview => PageView::Split,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PageView::Editor => "editor only",
            PageView::Split => "editor and preview",
            PageView::Preview => "preview only",
        }
    }
}

impl Editor {
    pub fn dirty(&self) -> bool {
        self.area.text() != self.saved
    }
}

/// The preview as last laid out, and for what text and width: drawn again as
/// it is until either changes (not parsed and laid out on every frame).
pub struct Preview {
    pub text: String,
    pub width: usize,
    pub lines: Vec<ratatui::text::Line<'static>>,
    pub style: ratatui::style::Style,
}

pub struct Node {
    pub status: NodeStatus,
    /// This client's node address (the same identity as LXMF).
    pub hash: Hash,
    pub stats: Option<NomadServeStats>,
    pub pages: Vec<PageInfo>,
    /// Why the page list could not be read.
    pub error: Option<String>,
    pub list: ListState,
    pub editor: Option<Editor>,
    /// Keys go to the editor.
    pub editing: bool,
    /// A mouse press in the editor, selecting as it drags.
    pub dragging: bool,
    /// Editor, preview, or both side by side.
    pub view: PageView,
    /// First row shown by the preview when it is shown alone.
    pub preview_scroll: usize,
    pub preview: Option<Preview>,
}

impl Node {
    pub fn new(hash: Hash, enabled: bool) -> Self {
        Self {
            status: if enabled { NodeStatus::Starting } else { NodeStatus::Off },
            hash,
            stats: None,
            pages: Vec::new(),
            error: None,
            list: ListState::default(),
            editor: None,
            editing: false,
            dragging: false,
            view: PageView::Split,
            preview_scroll: 0,
            preview: None,
        }
    }
}

impl App {
    fn node_store(&self) -> Result<Pages, String> {
        Pages::open(&HostConfig::dir(&self.settings, &self.paths))
    }

    /// Start, restart or stop the node to match the settings.
    pub(super) fn apply_node_settings(&mut self) {
        let config = HostConfig::from_settings(&self.settings, &self.paths);
        self.node.status = if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off };
        self.node.stats = None;
        self.send(NetCommand::Host(config));
        self.refresh_pages();
    }

    pub(super) fn on_host(&mut self, event: HostEvent) {
        match event {
            HostEvent::Started { hash } => {
                self.node.status = NodeStatus::Running;
                self.node.hash = hash;
                // A client never hears its own announce: list the node by name.
                let name = self.settings.node_name.clone().unwrap_or_else(|| self.settings.display_name.clone());
                self.store.peers.insert(
                    hex::encode(hash),
                    crate::store::Peer {
                        kind: crate::net::PeerKind::Nomad,
                        name: Some(name),
                        hops: 0,
                        last_seen: chrono::Utc::now().timestamp(),
                    },
                );
                self.peers_dirty = true;
                self.log(format!("Hosting NomadNet node {}", hex::encode(hash)));
            }
            HostEvent::Stopped => {
                self.node.status = NodeStatus::Off;
                self.log("Stopped hosting the node");
            }
            HostEvent::Failed(e) => {
                self.log(format!("Node: {e}"));
                self.node.status = NodeStatus::Failed(e);
            }
            HostEvent::Stats(stats) => self.node.stats = Some(stats),
        }
    }

    /// Re-read the page list from the node folder (creating it, with a
    /// starter index page, the first time).
    pub fn refresh_pages(&mut self) {
        let name = self.settings.node_name.clone().unwrap_or_else(|| self.settings.display_name.clone());
        let listed = self.node_store().and_then(|store| {
            store.ensure_index(&name)?;
            store.list()
        });
        match listed {
            Ok(pages) => {
                self.node.pages = pages;
                self.node.error = None;
            }
            Err(e) => {
                self.node.pages.clear();
                self.node.error = Some(e);
            }
        }
        let len = self.node.pages.len();
        let selected = self.node.list.selected().map(|i| i.min(len.saturating_sub(1)));
        self.node.list.select(if len == 0 { None } else { selected.or(Some(0)) });
    }

    /// A page's text, and whether it is an executable script.
    pub fn node_read(&self, path: &str) -> Result<(String, bool), String> {
        let store = self.node_store()?;
        Ok((store.read(path)?, store.is_executable(path)))
    }

    pub fn node_media(&self, path: &str) -> Result<Vec<u8>, String> {
        self.node_store()?.read_media(path)
    }

    /// Save a page (new or existing); `scripts` allows changing executable
    /// pages (never from the web UI).
    pub fn node_save(&mut self, path: &str, content: &str, scripts: bool) -> Result<(), String> {
        let store = self.node_store()?;
        let existed = store.exists(path);
        store.save(path, content, scripts)?;
        if !existed {
            self.send(NetCommand::HostReload);
            self.refresh_pages();
        }
        Ok(())
    }

    pub fn node_create(&mut self, name: &str) -> Result<String, String> {
        let path = pages::normalize_name(name)?;
        self.node_store()?.create(&path, NEW_PAGE)?;
        self.send(NetCommand::HostReload);
        self.refresh_pages();
        self.log(format!("Created page {path}"));
        Ok(path)
    }

    pub fn node_rename(&mut self, from: &str, to: &str, scripts: bool) -> Result<String, String> {
        let to = pages::normalize_name(to)?;
        self.node_store()?.rename(from, &to, scripts)?;
        self.send(NetCommand::HostReload);
        self.refresh_pages();
        self.log(format!("Renamed page {from} to {to}"));
        Ok(to)
    }

    pub fn node_delete(&mut self, path: &str) -> Result<(), String> {
        self.node_store()?.delete(path)?;
        self.send(NetCommand::HostReload);
        self.refresh_pages();
        self.log(format!("Deleted page {path}"));
        Ok(())
    }

    /// `hash:/page/<path>` on this client's node.
    pub fn node_page_url(&self, path: &str) -> String {
        format!("{}:/page/{path}", hex::encode(self.node.hash))
    }

    // ---- TUI ---------------------------------------------------------------

    fn selected_page(&self) -> Option<&PageInfo> {
        self.node.pages.get(self.node.list.selected()?)
    }

    fn open_page(&mut self, path: &str) {
        match self.node_read(path) {
            Ok((text, executable)) => {
                self.node.editor = Some(Editor {
                    path: path.to_string(),
                    area: TextArea::new(&text),
                    saved: text,
                    executable,
                });
                self.node.editing = true;
            }
            Err(e) => self.fail(e),
        }
    }

    /// Open a page, asking first if that would drop unsaved changes.
    fn open_page_checked(&mut self, path: String) {
        match &self.node.editor {
            Some(editor) if editor.path == path => self.node.editing = true,
            Some(editor) if editor.dirty() => {
                let title = format!("Discard unsaved changes to {}? Type y", editor.path);
                self.open_prompt(PromptKind::ConfirmDiscardPage(path), &title, "");
            }
            _ => self.open_page(&path),
        }
    }

    /// From the discard prompt.
    pub(super) fn discard_and_open(&mut self, path: &str) {
        self.node.editor = None;
        self.open_page(path);
    }

    fn save_editor(&mut self) {
        let Some(editor) = &self.node.editor else { return };
        let (path, text) = (editor.path.clone(), editor.area.text());
        // The TUI runs as the local user, who may edit scripts.
        match self.node_save(&path, &text, true) {
            Ok(()) => {
                if let Some(editor) = &mut self.node.editor {
                    editor.saved = text;
                    editor.area.break_undo_group();
                }
                let live = if self.node.status == NodeStatus::Running { " (live)" } else { "" };
                self.confirm(format!("Saved {path}{live}"));
                self.refresh_pages();
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn submit_page_prompt(&mut self, kind: PromptKind, text: &str) {
        match kind {
            PromptKind::NewPage => match self.node_create(text) {
                Ok(path) => {
                    self.select_page(&path);
                    self.open_page(&path);
                }
                Err(e) => self.fail(e),
            },
            PromptKind::RenamePage(from) => match self.node_rename(&from, text, true) {
                Ok(to) => {
                    if let Some(editor) = self.node.editor.as_mut().filter(|e| e.path == from) {
                        editor.path = to.clone();
                    }
                    self.select_page(&to);
                }
                Err(e) => self.fail(e),
            },
            PromptKind::ConfirmDeletePage(path) if text.eq_ignore_ascii_case("y") => {
                match self.node_delete(&path) {
                    Ok(()) => {
                        if self.node.editor.as_ref().is_some_and(|e| e.path == path) {
                            self.node.editor = None;
                            self.node.editing = false;
                        }
                    }
                    Err(e) => self.fail(e),
                }
            }
            _ => {}
        }
    }

    fn select_page(&mut self, path: &str) {
        if let Some(i) = self.node.pages.iter().position(|p| p.path == path) {
            self.node.list.select(Some(i));
        }
    }

    pub(super) fn node_editor_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => self.node.editing = false,
            // Formatting: Alt and the key underlined in the ribbon.
            KeyCode::Char(c) if alt && !ctrl && format::Action::from_key(c).is_some() => {
                if let Some(action) = format::Action::from_key(c) {
                    self.node_format(action);
                }
            }
            KeyCode::Char('s') if ctrl => self.save_editor(),
            KeyCode::Char('p') if ctrl => self.cycle_page_view(),
            // The preview alone: keys scroll it, nothing is typed unseen.
            _ if self.node.view == PageView::Preview => match key.code {
                KeyCode::Down | KeyCode::Char('j') => self.scroll_preview(1),
                KeyCode::Up | KeyCode::Char('k') => self.scroll_preview(-1),
                KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_preview(20),
                KeyCode::PageUp => self.scroll_preview(-20),
                KeyCode::Home => self.node.preview_scroll = 0,
                _ => {}
            },
            KeyCode::Char('v') if ctrl => self.paste_from_clipboard(),
            _ => {
                if let Some(editor) = &mut self.node.editor {
                    editor.area.handle(key);
                }
            }
        }
    }

    /// A formatting action from the ribbon or its key: applied at once, or
    /// after asking for what it needs.
    pub(super) fn node_format(&mut self, action: format::Action) {
        if self.node.view == PageView::Preview {
            return self.warn("Switch to the editor (Ctrl-P) to format");
        }
        let Some(editor) = &mut self.node.editor else { return };
        self.node.editing = true;
        match action.question() {
            Some(question) => self.open_prompt(PromptKind::Format(action), question, ""),
            None => {
                if let Err(e) = format::apply(&mut editor.area, action, "") {
                    self.warn(e);
                }
            }
        }
    }

    pub(super) fn node_format_answer(&mut self, action: format::Action, answer: &str) {
        let Some(editor) = &mut self.node.editor else { return };
        self.node.editing = true;
        if let Err(e) = format::apply(&mut editor.area, action, answer) {
            self.warn(e);
        }
    }

    pub(super) fn node_key(&mut self, key: KeyEvent) {
        let count = self.node.pages.len();
        match key.code {
            KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                let i = self.node.list.selected().map_or(0, |i| (i + 1).min(count - 1));
                self.node.list.select(Some(i));
            }
            KeyCode::Up | KeyCode::Char('k') if count > 0 => {
                let i = self.node.list.selected().map_or(0, |i| i.saturating_sub(1));
                self.node.list.select(Some(i));
            }
            KeyCode::Enter => match self.selected_page() {
                Some(page) if page.text => {
                    let path = page.path.clone();
                    self.open_page_checked(path);
                }
                Some(page) => {
                    let path = page.path.clone();
                    self.warn(format!("{path} is not a text file"));
                }
                None => {}
            },
            KeyCode::Right | KeyCode::Char('i') if self.node.editor.is_some() => self.node.editing = true,
            KeyCode::Char('n') => self.open_prompt(PromptKind::NewPage, "New page (e.g. about or blog/first)", ""),
            KeyCode::Char('r') => {
                if let Some(path) = self.selected_page().map(|p| p.path.clone()) {
                    self.open_prompt(PromptKind::RenamePage(path.clone()), &format!("Rename {path} to"), &path);
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some(path) = self.selected_page().map(|p| p.path.clone()) {
                    let title = format!("Delete {path}? Type y");
                    self.open_prompt(PromptKind::ConfirmDeletePage(path), &title, "");
                }
            }
            KeyCode::Char('h') => {
                let on = !self.settings.node_enabled;
                self.update_setting("node_enabled", if on { "true" } else { "false" });
            }
            KeyCode::Char('a') => self.send(NetCommand::HostAnnounce),
            KeyCode::Char('p') => self.cycle_page_view(),
            KeyCode::Char('y') => {
                let address = hex::encode(self.node.hash);
                self.copy(&address, "node address");
            }
            KeyCode::Char('b') => {
                let path = self.node.editor.as_ref().map(|e| e.path.clone());
                let url = path.map_or_else(|| hex::encode(self.node.hash), |p| self.node_page_url(&p));
                if let Some(location) = self.resolve(&url) {
                    self.navigate(location);
                }
            }
            _ => {}
        }
    }

    fn cycle_page_view(&mut self) {
        self.node.view = self.node.view.next();
        self.node.preview_scroll = 0;
        self.confirm(format!("Showing {}", self.node.view.label()));
    }

    /// Scroll the preview shown alone (the drawing clamps it to the page).
    fn scroll_preview(&mut self, delta: isize) {
        self.node.preview_scroll = self.node.preview_scroll.saturating_add_signed(delta);
    }

    pub(super) fn node_paste(&mut self, text: &str) {
        if self.node.view == PageView::Preview {
            return self.warn("Switch to the editor (Ctrl-P) to paste");
        }
        if let Some(editor) = &mut self.node.editor {
            editor.area.insert_str(text);
        }
    }

    pub(super) fn click_node(&mut self, at: Position, double: bool) {
        let list = self.regions.node_pages;
        let editor = self.regions.node_editor;
        if let Some(&(_, action)) = self.regions.node_ribbon.iter().find(|(area, _)| area.contains(at)) {
            self.node_format(action);
        } else if list.contains(at) {
            self.node.editing = false;
            let index = self.node.list.offset() + (at.y - list.y) as usize;
            if index < self.node.pages.len() {
                self.node.list.select(Some(index));
                if double {
                    self.node_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                }
            }
        } else if editor.contains(at)
            && let Some(state) = &mut self.node.editor
        {
            self.node.editing = true;
            self.node.dragging = true;
            let (row, col) = state.area.position_at((at.y - editor.y) as usize, (at.x - editor.x) as usize);
            state.area.clear_selection();
            state.area.set_cursor(row, col);
        }
    }

    /// Select from where the press was to the pointer (held inside the text).
    pub(super) fn drag_node(&mut self, at: Position) {
        let editor = self.regions.node_editor;
        let Some(state) = &mut self.node.editor else { return };
        if editor.is_empty() {
            return;
        }
        let x = at.x.clamp(editor.x, editor.right() - 1) - editor.x;
        let y = at.y.clamp(editor.y, editor.bottom() - 1) - editor.y;
        let (mut row, col) = state.area.position_at(y as usize, x as usize);
        // Past the top or bottom: a line further, so the view scrolls on.
        if at.y < editor.y {
            row = row.saturating_sub(1);
        } else if at.y >= editor.bottom() {
            row += 1;
        }
        state.area.extend_to(row, col);
    }

    pub(super) fn scroll_node(&mut self, at: Position, delta: isize) {
        if self.regions.node_preview.contains(at) && self.node.view == PageView::Preview {
            self.scroll_preview(delta);
        } else if self.regions.node_editor.contains(at)
            && let Some(editor) = &mut self.node.editor
        {
            let (row, col) = editor.area.cursor();
            editor.area.set_cursor(row.saturating_add_signed(delta), col);
        } else if !self.node.pages.is_empty() {
            let i = self.node.list.selected().unwrap_or(0);
            let next = i.saturating_add_signed(delta.signum()).min(self.node.pages.len() - 1);
            self.node.list.select(Some(next));
        }
    }

    /// Whether a location is on this client's own node (never cached, so
    /// edits show at once).
    pub fn is_own_node(&self, node: Hash) -> bool {
        node == self.node.hash
    }

    /// A path on this client's own node, read from the node folder: a client
    /// cannot open a Link to a destination it hosts itself.
    pub fn own_node_content(&self, path: &str) -> Result<crate::nomad::FetchedContent, String> {
        let data = self.node_store()?.serve(path)?;
        Ok(crate::nomad::FetchedContent { data, metadata: None })
    }
}
