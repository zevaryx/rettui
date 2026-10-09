//! Node tab: hosting a NomadNet node and editing its pages; and hosting a
//! propagation node (its status is on the Status tab).
//!
//! The page operations (`node_*`) are shared by the TUI and the web UI.
//! Saving writes the file the node serves, so edits are live straight away;
//! added, renamed and deleted pages also make the node reload its routes.
//!
//! Pictures and files are added from the editors (shown in the page, or
//! linked to download), and anything can be moved between the folders of
//! its root (`pages/` or `files/`): links to it in the pages follow.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nomad_core::NomadServeStats;
use ratatui::layout::Position;
use ratatui::widgets::ListState;

use super::{App, PromptKind, format};
use crate::lxmf::pn::{PnConfig, PnStats};
use crate::net::{Hash, HostEvent, NetCommand, PnEvent};
use crate::nomad::host::HostConfig;
use crate::nomad::pages::{self, PageInfo, Pages, Root};
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
    /// What's in `pages/` (pages, and pictures they show) and in `files/`
    /// (to download).
    pub pages: Vec<PageInfo>,
    pub files: Vec<PageInfo>,
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
            files: Vec::new(),
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

/// A row of the list of what's on the node: a folder (its path in its
/// root; empty for the root itself), or something in one (its index in
/// `pages` or `files`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeRow {
    Folder { root: Root, path: String, depth: usize },
    Item { root: Root, index: usize, depth: usize },
}

impl NodeRow {
    pub fn root(&self) -> Root {
        match self {
            NodeRow::Folder { root, .. } | NodeRow::Item { root, .. } => *root,
        }
    }
}

/// `entries` as a tree under `folder`: what's in it, then its folders.
fn tree(root: Root, entries: &[PageInfo], folder: &str, depth: usize, rows: &mut Vec<NodeRow>) {
    for index in (0..entries.len()).filter(|&i| pages::folder_of(&entries[i].path) == folder) {
        rows.push(NodeRow::Item { root, index, depth });
    }
    let mut subfolders: Vec<&str> = entries
        .iter()
        .filter_map(|e| {
            let rest = if folder.is_empty() { Some(e.path.as_str()) } else { e.path.strip_prefix(folder)?.strip_prefix('/') };
            rest?.split_once('/').map(|(sub, _)| sub)
        })
        .collect();
    subfolders.sort_unstable();
    subfolders.dedup();
    for sub in subfolders {
        let path = if folder.is_empty() { sub.to_string() } else { format!("{folder}/{sub}") };
        rows.push(NodeRow::Folder { root, path: path.clone(), depth });
        tree(root, entries, &path, depth + 1, rows);
    }
}

/// Something added to the node from an editor.
#[derive(Debug, Clone)]
pub struct Added {
    /// Where pages find it: `:/media/images/…` (a picture), `:/file/…`.
    pub address: String,
    pub image: bool,
    /// Made smaller to show in a page (and to cross the mesh).
    pub shrunk: bool,
}

/// A file to add to the node, its name made safe and a picture made
/// smaller.
pub struct Upload {
    name: String,
    data: Vec<u8>,
    image: bool,
    shrunk: bool,
}

impl Upload {
    pub fn prepare(name: &str, data: &[u8]) -> Result<Self, String> {
        let name = pages::upload_name(name)?;
        if !pages::is_image_name(&name) {
            return Ok(Self { name, data: data.to_vec(), image: false, shrunk: false });
        }
        Ok(match super::shrink::shrink(data, "medium") {
            Some(small) => {
                let stem = name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem);
                Self { name: format!("{stem}.{}", small.extension), data: small.data, image: true, shrunk: true }
            }
            None => Self { name, data: data.to_vec(), image: true, shrunk: false },
        })
    }
}

/// The propagation node hosted here.
pub struct PnHost {
    pub status: NodeStatus,
    /// Its address (known from the identity before it starts).
    pub hash: Hash,
    pub stats: Option<PnStats>,
    /// What the network actor was last asked to run.
    config: Option<PnConfig>,
}

impl PnHost {
    pub fn new(hash: Hash, config: Option<PnConfig>) -> Self {
        let status = if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off };
        Self { status, hash, stats: None, config }
    }

    /// After a Reticulum restart, it starts again (with the settings).
    pub(super) fn restarting(&mut self, config: Option<PnConfig>) {
        *self = Self::new(self.hash, config);
    }
}

impl App {
    /// Start, restart or stop the propagation node to match the settings
    /// (nothing, if what it runs with didn't change).
    pub(super) fn apply_pn_settings(&mut self) {
        let config = PnConfig::from_settings(&self.settings, &self.paths);
        if config == self.pn.config {
            return;
        }
        self.pn.status = if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off };
        self.pn.stats = None;
        self.pn.config = config.clone();
        self.send(NetCommand::Propagation(config));
    }

    pub(super) fn on_pn(&mut self, event: PnEvent) {
        match event {
            PnEvent::Started { hash } => {
                self.pn.status = NodeStatus::Running;
                self.pn.hash = hash;
                // Listed under its name, to be picked as yours (a client never
                // hears its own announce).
                let name = self.pn.config.as_ref().map_or_else(|| self.settings.display_name.clone(), |c| c.name.clone());
                self.store.peers.insert(
                    hex::encode(hash),
                    crate::store::Peer {
                        kind: crate::net::PeerKind::Propagation,
                        name: Some(name),
                        hops: 0,
                        last_seen: chrono::Utc::now().timestamp(),
                    },
                );
                self.peers_dirty = true;
                self.log(format!("Hosting propagation node {}", hex::encode(hash)));
            }
            PnEvent::Stopped => {
                self.pn.status = NodeStatus::Off;
                self.log("Stopped hosting the propagation node");
            }
            PnEvent::Failed(e) => {
                self.log(format!("Propagation node: {e}"));
                self.pn.status = NodeStatus::Failed(e);
            }
            PnEvent::Stats(stats) => self.pn.stats = Some(stats),
        }
    }

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

    /// Re-read what's in the node folder (creating it, with a starter
    /// index page, the first time).
    pub fn refresh_pages(&mut self) {
        let name = self.settings.node_name.clone().unwrap_or_else(|| self.settings.display_name.clone());
        let listed = self.node_store().and_then(|store| {
            store.ensure_index(&name)?;
            Ok((store.list()?, store.list_files()?))
        });
        match listed {
            Ok((pages, files)) => {
                self.node.pages = pages;
                self.node.files = files;
                self.node.error = None;
            }
            Err(e) => {
                self.node.pages.clear();
                self.node.files.clear();
                self.node.error = Some(e);
            }
        }
        let len = self.node_rows().len();
        let selected = self.node.list.selected().map(|i| i.min(len.saturating_sub(1)));
        self.node.list.select(if len == 0 { None } else { selected.or(Some(1)) });
    }

    /// What's on the node as the list shows it: `pages/`, then `files/`,
    /// each with its folders.
    pub fn node_rows(&self) -> Vec<NodeRow> {
        let mut rows = Vec::new();
        for (root, entries) in [(Root::Pages, &self.node.pages), (Root::Files, &self.node.files)] {
            rows.push(NodeRow::Folder { root, path: String::new(), depth: 0 });
            tree(root, entries, "", 1, &mut rows);
        }
        rows
    }

    /// Something on the node, by its row.
    pub fn node_entry(&self, row: &NodeRow) -> Option<&PageInfo> {
        match row {
            NodeRow::Item { root: Root::Pages, index, .. } => self.node.pages.get(*index),
            NodeRow::Item { root: Root::Files, index, .. } => self.node.files.get(*index),
            NodeRow::Folder { .. } => None,
        }
    }

    /// The folders of a root, for moving something to.
    pub fn node_folders(&self, root: Root) -> Vec<String> {
        pages::folders(match root {
            Root::Pages => &self.node.pages,
            Root::Files => &self.node.files,
        })
    }

    /// Add a file to the node for its pages: a picture goes with the pages
    /// (in `images/`, shown with `:/media/…`), made smaller to suit the mesh
    /// and without its metadata (such as where a photo was taken); anything
    /// else is a file to download (in `files/`, linked as `:/file/…`).
    /// Never over something there.
    pub fn node_add_file(&mut self, name: &str, data: &[u8]) -> Result<Added, String> {
        let upload = Upload::prepare(name, data)?;
        self.node_add_upload(upload)
    }

    /// [`App::node_add_file`] for an upload already prepared (away from the
    /// app, since making a picture smaller takes a moment).
    pub fn node_add_upload(&mut self, upload: Upload) -> Result<Added, String> {
        let store = self.node_store()?;
        let added = if upload.image {
            let path = store.add_image(&upload.name, &upload.data)?;
            Added { address: format!(":/media/{path}"), image: true, shrunk: upload.shrunk }
        } else {
            let path = store.add_file(&upload.name, &upload.data)?;
            Added { address: format!(":/file/{path}"), image: false, shrunk: false }
        };
        self.send(NetCommand::HostReload);
        self.refresh_pages();
        self.log(format!("Added {} to the node", added.address.trim_start_matches(':')));
        Ok(added)
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

    /// Rename (or move) something on the node, in its root: a page keeps
    /// its `.mu`. Links to it in the pages follow, but for one open in the
    /// terminal UI with changes not saved (it's said). Its new path.
    pub fn node_rename_in(&mut self, root: Root, from: &str, to: &str, scripts: bool) -> Result<String, String> {
        let to = match root {
            Root::Pages => pages::normalize_name(to)?,
            Root::Files => {
                let to = to.trim().trim_matches('/').to_string();
                nomad_core::validate_content_relative_path(&to).map_err(|e| format!("Not a name for the node: {e}"))?;
                to
            }
        };
        if to == from {
            return Ok(to);
        }
        let store = self.node_store()?;
        store.rename_in(root, from, &to, scripts)?;
        // The page open here, renamed or with changes not saved.
        let open = self.node.editor.as_ref().map(|e| (e.path.clone(), e.dirty()));
        if root == Root::Pages
            && let Some(editor) = self.node.editor.as_mut().filter(|e| e.path == from)
        {
            editor.path = to.clone();
        }
        let unsaved = open.as_ref().filter(|(_, dirty)| *dirty).map(|(path, _)| if path == from { to.clone() } else { path.clone() });
        let relinked = store.relink(root, from, &to, unsaved.as_deref())?;
        // The open page, its links changed on disk: shown as saved.
        if let Some(editor) = self.node.editor.as_mut().filter(|e| !e.dirty() && relinked.contains(&e.path))
            && let Ok(text) = store.read(&editor.path)
        {
            let cursor = editor.area.cursor();
            editor.area = TextArea::new(&text);
            editor.area.set_cursor(cursor.0, cursor.1);
            editor.saved = text;
        }
        self.send(NetCommand::HostReload);
        self.refresh_pages();
        let mut line = format!("Moved {from} to {to}");
        if !relinked.is_empty() {
            line.push_str(&format!(" (links to it updated in {})", relinked.join(", ")));
        }
        self.log(line);
        if let Some(page) = unsaved {
            self.warn(format!("Links in {page}, open with changes not saved, weren't updated"));
        }
        Ok(to)
    }

    /// Move something into a folder of its root (empty: the top), keeping
    /// its name. Its new path.
    pub fn node_move(&mut self, root: Root, path: &str, folder: &str, scripts: bool) -> Result<String, String> {
        let folder = pages::normalize_folder(folder)?;
        let name = path.rsplit('/').next().unwrap_or(path);
        let to = if folder.is_empty() { name.to_string() } else { format!("{folder}/{name}") };
        self.node_rename_in(root, path, &to, scripts)
    }

    pub fn node_delete_in(&mut self, root: Root, path: &str) -> Result<(), String> {
        self.node_store()?.delete_in(root, path)?;
        self.send(NetCommand::HostReload);
        self.refresh_pages();
        self.log(format!("Deleted {path} from {}/", root.id()));
        Ok(())
    }

    /// `hash:/page/<path>` on this client's node.
    pub fn node_page_url(&self, path: &str) -> String {
        format!("{}:/page/{path}", hex::encode(self.node.hash))
    }

    // ---- TUI ---------------------------------------------------------------

    fn selected_row(&self) -> Option<NodeRow> {
        self.node_rows().into_iter().nth(self.node.list.selected()?)
    }

    fn selected_page(&self) -> Option<&PageInfo> {
        match self.selected_row()? {
            NodeRow::Item { root: Root::Pages, index, .. } => self.node.pages.get(index),
            _ => None,
        }
    }

    /// What's selected: its root and path (a folder's, for a folder).
    fn selected_item(&self) -> Option<(Root, String, bool)> {
        let row = self.selected_row()?;
        match &row {
            NodeRow::Folder { root, path, .. } => Some((*root, path.clone(), true)),
            NodeRow::Item { root, .. } => Some((*root, self.node_entry(&row)?.path.clone(), false)),
        }
    }

    fn open_page(&mut self, path: &str) {
        match self.node_read(path) {
            Ok((text, executable)) => {
                self.node.editor = Some(Editor { path: path.to_string(), area: TextArea::new(&text), saved: text, executable });
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
            PromptKind::RenamePage(root, from) => match self.node_rename_in(root, &from, text, true) {
                Ok(to) => self.select_item(root, &to),
                Err(e) => self.fail(e),
            },
            PromptKind::ConfirmDeletePage(root, path) if text.eq_ignore_ascii_case("y") => match self.node_delete_in(root, &path) {
                Ok(()) => {
                    if root == Root::Pages && self.node.editor.as_ref().is_some_and(|e| e.path == path) {
                        self.node.editor = None;
                        self.node.editing = false;
                    }
                }
                Err(e) => self.fail(e),
            },
            _ => {}
        }
    }

    fn select_page(&mut self, path: &str) {
        self.select_item(Root::Pages, path);
    }

    fn select_item(&mut self, root: Root, path: &str) {
        let rows = self.node_rows();
        if let Some(i) = rows.iter().position(|row| row.root() == root && self.node_entry(row).is_some_and(|e| e.path == path)) {
            self.node.list.select(Some(i));
        }
    }

    pub(super) fn submit_move(&mut self, root: Root, path: &str, folder: &str) {
        match self.node_move(root, path, folder, true) {
            Ok(to) => {
                self.select_item(root, &to);
                self.confirm(format!("Moved to {}/{to}", root.id()));
            }
            Err(e) => self.fail(e),
        }
    }

    /// The editor's Upload: a file from this computer added to the node,
    /// and shown or linked to where the cursor is.
    fn node_upload_path(&mut self, answer: &str) {
        let path = super::files::path_from_input(answer);
        if answer.trim().is_empty() {
            return;
        }
        let data = match std::fs::metadata(&path) {
            Ok(meta) if !meta.is_file() => return self.warn(format!("{} isn't a file", path.display())),
            // Pictures may be made much smaller; files are 32 MB at most.
            Ok(meta) if meta.len() > 64 * 1024 * 1024 => return self.warn(format!("{} is too big for the node", path.display())),
            Ok(_) => match std::fs::read(&path) {
                Ok(data) => data,
                Err(e) => return self.warn(format!("Couldn't read {}: {e}", path.display())),
            },
            Err(e) => return self.warn(format!("Couldn't read {}: {e}", path.display())),
        };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match self.node_add_file(&name, &data) {
            Ok(added) => {
                if let Some(editor) = &mut self.node.editor {
                    format::added(&mut editor.area, &added.address, added.image);
                    self.node.editing = true;
                }
                let how = if added.image { "shown in the page" } else { "linked to download" };
                let smaller = if added.shrunk { ", made smaller" } else { "" };
                self.confirm(format!("Added {}{smaller}: {how}", added.address.trim_start_matches(':')));
            }
            Err(e) => self.warn(e),
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
        if action == format::Action::Upload {
            return self.node_upload_path(answer);
        }
        let Some(editor) = &mut self.node.editor else { return };
        self.node.editing = true;
        if let Err(e) = format::apply(&mut editor.area, action, answer) {
            self.warn(e);
        }
    }

    pub(super) fn node_key(&mut self, key: KeyEvent) {
        let count = self.node_rows().len();
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
                    self.warn(format!("{path} is a picture: pages show it (Alt+M or Alt+P in a page)"));
                }
                None => {
                    if let Some((Root::Files, path, false)) = self.selected_item() {
                        self.warn(format!("{path} is a file to download: pages link to it as :/file/{path}"));
                    }
                }
            },
            KeyCode::Right | KeyCode::Char('i') if self.node.editor.is_some() => self.node.editing = true,
            KeyCode::Char('n') => {
                // In the folder picked (of pages/).
                let folder = match self.selected_item() {
                    Some((Root::Pages, path, true)) if !path.is_empty() => format!("{path}/"),
                    Some((Root::Pages, path, false)) => match pages::folder_of(&path) {
                        "" => String::new(),
                        folder => format!("{folder}/"),
                    },
                    _ => String::new(),
                };
                self.open_prompt(PromptKind::NewPage, "New page (e.g. about or blog/first)", &folder);
            }
            KeyCode::Char('r') => {
                if let Some((root, path, false)) = self.selected_item() {
                    self.open_prompt(PromptKind::RenamePage(root, path.clone()), &format!("Rename {path} to (links to it follow)"), &path);
                }
            }
            KeyCode::Char('m') => {
                if let Some((root, path, false)) = self.selected_item() {
                    let folders = self.node_folders(root);
                    let there = if folders.is_empty() { String::new() } else { format!("; there are {}", folders.join(", ")) };
                    let title = format!("Move {path} to which folder of {}/? (empty: the top{there}; links to it follow)", root.id());
                    self.open_prompt(PromptKind::MoveNodeItem(root, path.clone()), &title, pages::folder_of(&path));
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some((root, path, false)) = self.selected_item() {
                    let title = format!("Delete {path} from {}/? Type y", root.id());
                    self.open_prompt(PromptKind::ConfirmDeletePage(root, path), &title, "");
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
            if index < self.node_rows().len() {
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
        } else {
            let rows = self.node_rows().len();
            let i = self.node.list.selected().unwrap_or(0);
            let next = i.saturating_add_signed(delta.signum()).min(rows.saturating_sub(1));
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{ImageFormat, Rgb, RgbImage};

    use super::*;
    use crate::app::Tab;
    use crate::config::Settings;
    use crate::store::Store;

    fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.on_key(KeyEvent::new(code, modifiers));
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            key(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    /// A noisy photo (so it doesn't compress to nothing), as a PNG.
    fn photo(width: u32, height: u32) -> Vec<u8> {
        let picture = RgbImage::from_fn(width, height, |x, y| {
            let n = x.wrapping_mul(2_654_435_761).wrapping_add(y.wrapping_mul(40_503)) as u8;
            Rgb([n, n.wrapping_mul(3), (x + y) as u8])
        });
        let mut png = Vec::new();
        picture.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        png
    }

    /// The Node tab, with the starter page open in the editor.
    fn editing(name: &str) -> (App, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("rettui-node-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, Settings::default(), Store::default());
        app.tab = Tab::Node;
        app.refresh_pages();
        app.open_page("index.mu");
        (app, dir)
    }

    fn upload(app: &mut App, path: &std::path::Path) {
        key(app, KeyCode::Char('p'), KeyModifiers::ALT);
        assert!(app.prompt.is_some(), "Alt+P asks for the file");
        type_text(app, &path.display().to_string());
        key(app, KeyCode::Enter, KeyModifiers::NONE);
    }

    fn screen(app: &mut App) -> String {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..24).map(|y| (0..120).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    fn page_text(app: &App) -> String {
        app.node.editor.as_ref().unwrap().area.text()
    }

    #[test]
    fn pictures_and_files_added_from_the_editor_are_shown_and_linked() {
        let (mut app, dir) = editing("upload");
        let node = HostConfig::dir(&app.settings, &app.paths);
        let picture = dir.join("Holiday photo.png");
        std::fs::write(&picture, photo(2000, 1500)).unwrap();
        upload(&mut app, &picture);
        assert!(page_text(&app).contains("`(Holiday-photo`:/media/images/Holiday-photo.jpg)"), "{}", page_text(&app));
        // Made smaller for the mesh, with the pages, and listed in its folder.
        let shown = image::open(node.join("pages/images/Holiday-photo.jpg")).unwrap();
        assert!(shown.width() <= 1024 && shown.height() <= 1024);
        let rows = app.node_rows();
        assert!(rows.contains(&NodeRow::Folder { root: Root::Pages, path: "images".into(), depth: 1 }));

        let notes = dir.join("notes.txt");
        std::fs::write(&notes, "hello").unwrap();
        upload(&mut app, &notes);
        assert!(page_text(&app).contains("`[notes.txt`:/file/notes.txt]"));
        assert_eq!(std::fs::read_to_string(node.join("files/notes.txt")).unwrap(), "hello");
        // Never over something already there.
        let again = app.node_add_file("notes.txt", b"other").unwrap();
        assert_eq!(again.address, ":/file/notes-2.txt");
        assert_eq!(std::fs::read_to_string(node.join("files/notes.txt")).unwrap(), "hello");
        let files: Vec<&str> = app.node.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(files, ["notes-2.txt", "notes.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moving_to_a_folder_keeps_the_pages_links() {
        let (mut app, dir) = editing("move");
        let node = HostConfig::dir(&app.settings, &app.paths);
        let notes = dir.join("notes.txt");
        std::fs::write(&notes, "hello").unwrap();
        upload(&mut app, &notes);
        key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL);
        // From the list: m, and the folder.
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        app.select_item(Root::Files, "notes.txt");
        key(&mut app, KeyCode::Char('m'), KeyModifiers::NONE);
        type_text(&mut app, "docs");
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(node.join("files/docs/notes.txt").exists());
        assert!(!node.join("files/notes.txt").exists());
        let index = std::fs::read_to_string(node.join("pages/index.mu")).unwrap();
        assert!(index.contains("`[notes.txt`:/file/docs/notes.txt]"), "{index}");
        // The page open (saved) shows its links as changed.
        assert_eq!(page_text(&app), index);
        assert_eq!(app.selected_item(), Some((Root::Files, "docs/notes.txt".into(), false)));
        assert!(app.node_rows().contains(&NodeRow::Folder { root: Root::Files, path: "docs".into(), depth: 1 }));
        let shown = screen(&mut app);
        for line in ["pages/", "index.mu", "files/", "docs/", "notes.txt"] {
            assert!(shown.contains(line), "{line} isn't listed");
        }

        // Changes not saved: that page is left as typed (and it's said), and
        // a folder left empty goes.
        let area = &mut app.node.editor.as_mut().unwrap().area;
        area.set_cursor(0, 0);
        area.insert_str("more\n");
        app.node_move(Root::Files, "docs/notes.txt", "", true).unwrap();
        assert!(!node.join("files/docs").exists());
        assert!(page_text(&app).contains(":/file/docs/notes.txt"));
        assert!(std::fs::read_to_string(node.join("pages/index.mu")).unwrap().contains(":/file/docs/notes.txt"));
        assert!(app.node_rows().iter().all(|row| !matches!(row, NodeRow::Folder { path, .. } if path == "docs")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
