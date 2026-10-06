//! Browser tab: NomadNet pages (from the cache when fresh), links and
//! forms, images, text selection, and the saved pages / nodes pane.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;
use ratatui::widgets::ListState;

use super::files::unique_path;
use super::{App, Prompt, PromptKind, Tab};
use crate::net::{Hash, NetCommand, PeerKind, parse_hash};
use crate::nomad::FetchedContent;
use crate::nomad::cache::Cache;
use crate::nomad::micron::{self, FieldKind, Interactive, Page};
use crate::store::{Bookmark, Peer};
use crate::term::images::{DecodeFor, Picture};
use crate::term::input::TextInput;
use crate::term::selection::Selection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub node: Hash,
    pub path: String,
    pub fields: BTreeMap<String, String>,
}

impl Location {
    pub fn url(&self) -> String {
        format!("{}:{}", hex::encode(self.node), self.path)
    }
}

pub struct Pending {
    pub id: u64,
    pub location: Location,
    pub started: Instant,
    /// Push the page being left onto history (false for back and reload).
    pub record_history: bool,
    /// Whether this request identified us (cached separately).
    identified: bool,
    /// Bypass the cache for this page's images too.
    refresh: bool,
    /// How it's going, when finding a path takes a while.
    pub status: Option<String>,
}

/// Left pane of the Browser tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowserPane {
    #[default]
    Saved,
    Nodes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowserFocus {
    #[default]
    Pane,
    Page,
}

#[derive(Default)]
pub struct Browser {
    pub location: Option<Location>,
    pub history: Vec<Location>,
    pub page: Option<Page>,
    /// Micron source of the shown page, and whether it is shown instead.
    pub source: Option<String>,
    pub view_source: bool,
    pub loading: Option<Pending>,
    pub error: Option<String>,
    pub selected: Option<usize>,
    pub scroll: usize,
    /// Rows of each interactive in the last layout, and the viewport height.
    pub item_rows: Vec<usize>,
    pub viewport: usize,
    /// Decoded page images keyed by their Micron URL.
    pub images: HashMap<String, Picture>,
    /// In-flight media requests: request id -> (Micron URL, where it lives).
    pub(super) media_requests: HashMap<u64, (String, Location, bool)>,
    /// Age of the shown page when it came from the cache.
    pub cached_age: Option<std::time::Duration>,
    pub pane: BrowserPane,
    pub focus: BrowserFocus,
    pub saved_list: ListState,
    pub nodes_list: ListState,
    /// Text selected on the page with the mouse.
    pub selection: Option<Selection>,
    pub(super) dragging: bool,
    /// The shown page's partials, by index: what loaded, and when.
    pub partials: Vec<PartialState>,
    /// In-flight partial requests: request id -> (partial, page address).
    partial_requests: HashMap<u64, (usize, String)>,
    /// The first row of each of the page's lines in the last layout.
    pub line_rows: Vec<Option<usize>>,
    /// A line to scroll to at the next layout (an anchor jumped to).
    pub jump_to: Option<usize>,
}

/// A partial of the shown page.
#[derive(Debug, Clone, Default)]
pub struct PartialState {
    /// Its content, once loaded (or why it couldn't be, as Micron).
    pub content: Option<String>,
    pub loading: bool,
    pub loaded_at: Option<Instant>,
}

/// A multi-row field's text to edit on one line (line breaks as ↵), and
/// back.
pub fn field_to_line(value: &str) -> String {
    value.replace('\n', " ↵ ")
}

pub fn field_from_line(text: &str) -> String {
    text.replace(" ↵ ", "\n").replace('↵', "\n")
}

impl App {
    pub(super) fn on_fetched(&mut self, id: u64, result: Result<FetchedContent, String>) {
        if let Some((index, page)) = self.browser.partial_requests.remove(&id) {
            return self.on_partial(index, &page, result);
        }
        if let Some((url, location, identified)) = self.browser.media_requests.remove(&id) {
            match result {
                Ok(content) => {
                    self.cache.put(location.node, &location.path, identified, &content.data, None);
                    self.decode_later(DecodeFor::Page(url), Some(content.data));
                }
                Err(e) => tracing::debug!("could not load image {url}: {e}"),
            }
            return;
        }
        if self.browser.loading.as_ref().is_none_or(|p| p.id != id) {
            return; // cancelled or superseded
        }
        let Some(pending) = self.browser.loading.take() else {
            return;
        };
        let location = &pending.location;
        match result {
            Ok(content) if location.path.starts_with(nomad_core::FILE_PREFIX) => {
                let name = nomad_core::reply_file_name(content.metadata.as_deref())
                    .or_else(|| location.path.rsplit('/').next().map(str::to_string))
                    .unwrap_or_else(|| "download".to_string());
                let saved = std::fs::create_dir_all(&self.paths.downloads).and_then(|()| {
                    let target = unique_path(&self.paths.downloads, &name);
                    std::fs::write(&target, &content.data).map(|()| target)
                });
                match saved {
                    Ok(target) => self.notify(format!("Saved {}", target.display())),
                    Err(e) => self.fail(format!("Could not save {name}: {e}")),
                }
            }
            Ok(content) => {
                if location.fields.is_empty() {
                    let ttl = micron::cache_directive(&String::from_utf8_lossy(&content.data))
                        .map(std::time::Duration::from_secs);
                    self.cache
                        .put(location.node, &location.path, pending.identified, &content.data, ttl);
                }
                self.show_page(pending, &content.data, None);
            }
            Err(e) => {
                self.log(format!("Could not load {}: {e}", location.url()));
                self.browser.error = Some(e);
            }
        }
    }

    fn show_page(&mut self, pending: Pending, data: &[u8], cached_age: Option<std::time::Duration>) {
        let source = String::from_utf8_lossy(data).into_owned();
        let page = micron::parse(&source);
        // A link's `anchor=name`: where to jump once it shows.
        self.browser.jump_to = micron::anchor_variable(&pending.location.fields).and_then(|name| page.anchor_line(name));
        // Reloading keeps the source view; another page shows the page.
        if self.browser.location.as_ref() != Some(&pending.location) {
            self.browser.view_source = false;
        }
        if let Some(previous) = self.browser.location.take()
            && pending.record_history
            && previous != pending.location
        {
            self.browser.history.push(previous);
        }
        self.browser.selected = if page.items.is_empty() { None } else { Some(0) };
        self.browser.images.clear();
        self.browser.media_requests.clear();
        let image_urls = page.image_urls();
        self.browser.page = Some(page);
        self.browser.source = Some(source);
        self.browser.location = Some(pending.location);
        self.browser.cached_age = cached_age;
        self.browser.selection = None;
        self.browser.scroll = 0;
        self.browser.error = None;
        for url in image_urls {
            self.request_image(url, pending.refresh);
        }
        self.browser.partial_requests.clear();
        let count = self.browser.page.as_ref().map_or(0, |p| p.partials.len());
        self.browser.partials = vec![PartialState::default(); count];
        for index in 0..count {
            self.request_partial(index);
        }
    }

    /// Load (or reload) one of the shown page's partials, with the fields
    /// and variables it asks for. Partials always come from the network.
    fn request_partial(&mut self, index: usize) {
        let (Some(page), Some(current)) = (&self.browser.page, &self.browser.location) else { return };
        let Some(partial) = page.partials.get(index) else { return };
        let Some(mut location) = self.resolve(&partial.url) else {
            let why = format!("`Ff66This part of the page has an address rettui can't read: {}`f", partial.url.replace('`', "'"));
            self.browser.partials[index].content = Some(why);
            return;
        };
        location.fields = page.request_fields(&partial.fields);
        let page_url = current.url();
        self.browser.partials[index].loading = true;
        let id = self.request_id();
        self.browser.partial_requests.insert(id, (index, page_url));
        if self.is_own_node(location.node) {
            let result = self.own_node_content(&location.path);
            return self.on_fetched(id, result);
        }
        let identify = self.identifies_to(location.node);
        self.send(NetCommand::Fetch { id, node: location.node, path: location.path, fields: location.fields, identify });
    }

    /// A partial loaded (or not): the page is read again with it in place,
    /// keeping what was typed and folded.
    fn on_partial(&mut self, index: usize, page_url: &str, result: Result<FetchedContent, String>) {
        if self.browser.location.as_ref().map(Location::url).as_deref() != Some(page_url) || index >= self.browser.partials.len() {
            return;
        }
        let content = match result {
            Ok(content) => String::from_utf8_lossy(&content.data).into_owned(),
            Err(e) => format!("`Ff66Could not load this part of the page: {}`f", e.replace('`', "'")),
        };
        let state = &mut self.browser.partials[index];
        (state.content, state.loading, state.loaded_at) = (Some(content), false, Some(Instant::now()));
        let (Some(source), Some(old)) = (&self.browser.source, &self.browser.page) else { return };
        let contents: Vec<Option<String>> = self.browser.partials.iter().map(|p| p.content.clone()).collect();
        let mut page = micron::parse_with(source, &contents);
        page.keep_state(old);
        let known: Vec<String> = old.image_urls();
        let new_images: Vec<String> = page.image_urls().into_iter().filter(|u| !known.contains(u)).collect();
        self.browser.selected = self.browser.selected.filter(|&s| s < page.items.len()).or((!page.items.is_empty()).then_some(0));
        self.browser.page = Some(page);
        for url in new_images {
            self.request_image(url, false);
        }
    }

    /// Reload the partials that ask to be, every so often, while the page
    /// is on screen.
    pub(super) fn refresh_partials(&mut self) {
        if self.tab != Tab::Browser || self.browser.view_source {
            return;
        }
        let Some(page) = &self.browser.page else { return };
        let due: Vec<usize> = page
            .partials
            .iter()
            .zip(&self.browser.partials)
            .enumerate()
            .filter(|(_, (partial, state))| {
                !state.loading
                    && partial.refresh.is_some_and(|every| state.loaded_at.is_some_and(|at| at.elapsed().as_secs() >= every))
            })
            .map(|(index, _)| index)
            .collect();
        for index in due {
            self.request_partial(index);
        }
    }

    /// Scroll to `line` of the page (an anchor's) at the next layout.
    fn jump_to_line(&mut self, line: Option<usize>, what: &str) {
        match line {
            Some(line) => self.browser.jump_to = Some(line),
            None => self.warn(format!("There's no {what} on this page")),
        }
    }

    /// A page image decoded in the background: show it if the page is still
    /// the one that asked for it.
    pub(super) fn on_page_image(&mut self, url: String, picture: Option<Picture>) {
        let wanted = self.browser.page.as_ref().is_some_and(|page| page.image_urls().contains(&url));
        match picture {
            Some(picture) if wanted => {
                self.browser.images.insert(url, picture);
            }
            Some(_) => {}
            None => tracing::debug!("could not decode image {url}"),
        }
    }

    fn request_image(&mut self, url: String, refresh: bool) {
        let Some(location) = self.resolve(&url) else { return };
        if !location.path.starts_with("/media/") {
            return;
        }
        let identified = self.identifies_to(location.node);
        if !refresh && let Some(cached) = self.cache.get(location.node, &location.path, identified) {
            self.decode_later(DecodeFor::Page(url), Some(cached.data));
            return;
        }
        let id = self.request_id();
        if self.is_own_node(location.node) {
            let result = self.own_node_content(&location.path);
            self.browser.media_requests.insert(id, (url, location, identified));
            return self.on_fetched(id, result);
        }
        self.send(NetCommand::Fetch {
            id,
            node: location.node,
            path: location.path.clone(),
            fields: BTreeMap::new(),
            identify: identified,
        });
        self.browser.media_requests.insert(id, (url, location, identified));
    }

    pub fn identifies_to(&self, node: Hash) -> bool {
        self.store.identified_nodes.contains(&hex::encode(node))
    }

    fn toggle_identify(&mut self) {
        let Some(location) = self.browser.location.clone() else {
            return;
        };
        let on = !self.identifies_to(location.node);
        self.set_identify(location.node, on);
        self.load(location, false, false);
    }

    pub fn navigate(&mut self, location: Location) {
        self.load(location, true, false);
    }

    /// Show a page, from the cache when possible unless `refresh`.
    /// Form submissions and file downloads always go to the network.
    fn load(&mut self, location: Location, record_history: bool, refresh: bool) {
        let identified = self.identifies_to(location.node);
        self.tab = Tab::Browser;
        self.browser.focus = BrowserFocus::Page;
        // Pages from this client's own node skip the cache, so edits show.
        let cacheable = location.fields.is_empty()
            && !location.path.starts_with(nomad_core::FILE_PREFIX)
            && !self.is_own_node(location.node);
        let id = self.request_id();
        let pending = Pending {
            id,
            location,
            started: Instant::now(),
            record_history,
            identified,
            refresh,
            status: None,
        };
        if cacheable
            && !refresh
            && let Some(cached) =
                self.cache.get(pending.location.node, &pending.location.path, identified)
        {
            self.browser.loading = None;
            self.show_page(pending, &cached.data, Some(cached.age));
            return;
        }
        self.browser.error = None;
        if self.is_own_node(pending.location.node) {
            let result = self.own_node_content(&pending.location.path);
            self.browser.loading = Some(pending);
            return self.on_fetched(id, result);
        }
        self.send(NetCommand::Fetch {
            id,
            node: pending.location.node,
            path: pending.location.path.clone(),
            fields: pending.location.fields.clone(),
            identify: identified,
        });
        self.browser.loading = Some(pending);
    }

    /// Accepts `<hash>`, `<hash>:/page/x.mu`, or `:/page/x.mu` / `/page/x.mu`
    /// relative to the current node.
    pub(super) fn resolve(&self, url: &str) -> Option<Location> {
        resolve_url(url, self.browser.location.as_ref().map(|l| l.node))
    }

    /// The page cache (shared by the TUI browser and the web UI).
    pub fn page_cache(&self) -> &Cache {
        &self.cache
    }

    /// A name for a saved page: the node's name, plus the page path.
    pub fn bookmark_name(&self, location: &Location) -> String {
        let node = self.store.display_name(&hex::encode(location.node));
        if location.path == nomad_core::DEFAULT_INDEX_ROUTE {
            node
        } else {
            format!("{node} · {}", location.path.trim_start_matches(nomad_core::PAGE_PREFIX))
        }
    }

    pub fn set_identify(&mut self, node: Hash, on: bool) {
        let key = hex::encode(node);
        let changed = if on {
            self.store.identified_nodes.insert(key.clone())
        } else {
            self.store.identified_nodes.remove(&key)
        };
        if changed {
            let name = self.store.display_name(&key);
            self.notify(if on { format!("Identifying to {name}") } else { format!("No longer identifying to {name}") });
            self.store_dirty = true;
        }
    }

    fn follow_link(&mut self, url: &str, spec: &[String]) {
        // An anchor on this page (`#` alone: the next heading).
        if let Some(name) = url.strip_prefix('#') {
            let Some(page) = &self.browser.page else { return };
            let line = if name.is_empty() {
                self.browser.selected.and_then(|item| page.next_heading_line(item))
            } else {
                page.anchor_line(name)
            };
            let what = if name.is_empty() { "heading after this link".to_string() } else { format!("anchor #{name}") };
            return self.jump_to_line(line, &what);
        }
        // Reload the partials with these ids.
        if let Some(ids) = url.strip_prefix("p:") {
            let due = self.browser.page.as_ref().map(|page| page.partials_with_ids(ids)).unwrap_or_default();
            if due.is_empty() {
                self.warn(format!("No part of this page has the id {ids}"));
            }
            for index in due {
                self.request_partial(index);
            }
            return;
        }
        if url.starts_with("rrc://") || url.starts_with("rrc@") {
            self.open_rrc_link(url);
            return;
        }
        if let Some(hash) = url
            .strip_prefix("lxmf@")
            .or_else(|| url.strip_prefix("lxmf://"))
        {
            match parse_hash(hash) {
                Some(hash) => self.open_conversation(hex::encode(hash)),
                None => self.warn(format!("Bad LXMF address: {url}")),
            }
            return;
        }
        let Some(mut location) = self.resolve(url) else {
            self.warn(format!("Cannot open link: {url}"));
            return;
        };
        if let Some(page) = &self.browser.page {
            location.fields = page.request_fields(spec);
        }
        self.navigate(location);
    }

    pub(super) fn activate(&mut self, index: usize) {
        let Some(page) = self.browser.page.as_mut() else {
            return;
        };
        self.browser.selected = Some(index);
        match page.items.get(index).cloned() {
            Some(Interactive::Link { url, fields }) => self.follow_link(&url, &fields),
            Some(Interactive::Field(f)) => {
                let field = &page.fields[f];
                if let FieldKind::Text { .. } = field.kind {
                    let (title, text) = if field.rows > 1 {
                        (format!("Field: {} (↵ starts a new line)", field.name), field_to_line(&field.value))
                    } else {
                        (format!("Field: {}", field.name), field.value.clone())
                    };
                    self.prompt = Some(Prompt {
                        kind: PromptKind::EditField(f),
                        title,
                        input: TextInput::with_text(&text),
                    });
                } else {
                    page.activate_choice(f);
                }
            }
            Some(Interactive::Fold(f)) => page.folds[f].open = !page.folds[f].open,
            None => {}
        }
    }

    fn move_selection(&mut self, forward: bool) {
        let Some(page) = &self.browser.page else { return };
        let count = page.items.len();
        if count == 0 {
            return;
        }
        let next = match self.browser.selected {
            None => 0,
            Some(i) if forward => (i + 1) % count,
            Some(i) => (i + count - 1) % count,
        };
        self.browser.selected = Some(next);
        // Scroll the selection into view.
        if let Some(&row) = self.browser.item_rows.get(next) {
            let height = self.browser.viewport.max(1);
            if row < self.browser.scroll || row >= self.browser.scroll + height {
                self.browser.scroll = row.saturating_sub(height / 3);
            }
        }
    }

    /// Index of the page text field selected in the browser, if any.
    pub(super) fn selected_text_field(&self) -> Option<usize> {
        if self.tab != Tab::Browser || self.browser.focus != BrowserFocus::Page {
            return None;
        }
        let page = self.browser.page.as_ref()?;
        match page.items.get(self.browser.selected?)? {
            Interactive::Field(f) if matches!(page.fields[*f].kind, FieldKind::Text { .. }) => Some(*f),
            _ => None,
        }
    }

    /// Absolute form of a page link (`hash:/page/x.mu`, or `lxmf@hash`).
    fn link_url(&self, url: &str) -> String {
        if url.starts_with("lxmf@") || url.starts_with("lxmf://") {
            return url.to_string();
        }
        self.resolve(url).map_or_else(|| url.to_string(), |l| l.url())
    }

    fn copy_selected_link(&mut self) {
        let link = self
            .browser
            .page
            .as_ref()
            .zip(self.browser.selected)
            .and_then(|(page, i)| match page.items.get(i) {
                Some(Interactive::Link { url, .. }) => Some(url.clone()),
                _ => None,
            });
        match link {
            Some(url) => {
                let url = self.link_url(&url);
                self.copy(&url, "link");
            }
            None => self.warn("Select a link first (Tab)"),
        }
    }

    /// NomadNet nodes heard, most recently heard first.
    pub fn browser_nodes(&self) -> Vec<(&String, &Peer)> {
        let mut nodes: Vec<_> = self
            .store
            .peers
            .iter()
            .filter(|(_, p)| p.kind == PeerKind::Nomad)
            .collect();
        nodes.sort_by(|a, b| b.1.last_seen.cmp(&a.1.last_seen).then_with(|| a.0.cmp(b.0)));
        nodes
    }

    fn pane_len(&self) -> usize {
        match self.browser.pane {
            BrowserPane::Saved => self.store.saved.len(),
            BrowserPane::Nodes => self.browser_nodes().len(),
        }
    }

    fn pane_list(&mut self) -> &mut ListState {
        match self.browser.pane {
            BrowserPane::Saved => &mut self.browser.saved_list,
            BrowserPane::Nodes => &mut self.browser.nodes_list,
        }
    }

    pub(super) fn move_pane_selection(&mut self, delta: isize) {
        let len = self.pane_len();
        if len == 0 {
            return;
        }
        let list = self.pane_list();
        let next = list.selected().map_or(0, |i| i.saturating_add_signed(delta).min(len - 1));
        list.select(Some(next));
    }

    /// Open the selected saved page or node next to the pane.
    fn open_pane_selection(&mut self) {
        let Some(index) = self.pane_list().selected() else {
            return;
        };
        let url = match self.browser.pane {
            BrowserPane::Saved => self.store.saved.get(index).map(|b| b.url.clone()),
            BrowserPane::Nodes => self.browser_nodes().get(index).map(|(k, _)| (*k).clone()),
        };
        if let Some(location) = url.and_then(|u| self.resolve(&u)) {
            self.navigate(location);
        }
    }

    fn switch_pane(&mut self, pane: BrowserPane) {
        self.browser.pane = pane;
        self.browser.focus = BrowserFocus::Pane;
        if self.pane_list().selected().is_none() && self.pane_len() > 0 {
            self.pane_list().select(Some(0));
        }
    }

    fn save_current_page(&mut self) {
        let Some(location) = self.browser.location.clone() else {
            self.log("Open a page to save it");
            return;
        };
        let url = location.url();
        if self.store.saved.iter().any(|b| b.url == url) {
            self.log("Page is already saved");
            return;
        }
        let name = self.bookmark_name(&location);
        self.log(format!("Saved {name}"));
        self.store.saved.push(Bookmark { name, url });
        self.store_dirty = true;
    }

    fn remove_saved(&mut self) {
        let Some(index) = self.browser.saved_list.selected() else {
            return;
        };
        if index < self.store.saved.len() {
            let removed = self.store.saved.remove(index);
            self.log(format!("Removed {}", removed.name));
            self.store_dirty = true;
            let len = self.store.saved.len();
            self.browser
                .saved_list
                .select((len > 0).then(|| index.min(len - 1)));
        }
    }

    pub(super) fn browser_key(&mut self, key: KeyEvent) {
        // Keys that work whichever pane has focus.
        match key.code {
            KeyCode::Char('g') | KeyCode::Char('o') => return self.open_goto(),
            KeyCode::Char('H') => {
                match self.settings.home.clone().and_then(|h| self.resolve(&h)) {
                    Some(location) => self.navigate(location),
                    None => self.log("No home page set (\"home\" in settings.json)"),
                }
                return;
            }
            KeyCode::Char('I') => return self.toggle_identify(),
            KeyCode::Backspace | KeyCode::Char('b') => return self.back(),
            KeyCode::Char('r') => return self.reload(),
            KeyCode::Char('R') => return self.clear_cache(),
            KeyCode::Char('s') => return self.save_current_page(),
            KeyCode::Char('t') => {
                let pane = match self.browser.pane {
                    BrowserPane::Saved => BrowserPane::Nodes,
                    BrowserPane::Nodes => BrowserPane::Saved,
                };
                return self.switch_pane(pane);
            }
            KeyCode::Left => {
                self.browser.focus = BrowserFocus::Pane;
                return;
            }
            KeyCode::Right if self.browser.page.is_some() => {
                self.browser.focus = BrowserFocus::Page;
                return;
            }
            KeyCode::Char('u') => return self.toggle_source(),
            KeyCode::Esc => {
                if self.browser.loading.is_none() && self.browser.view_source {
                    return self.toggle_source();
                }
                self.browser.loading = None;
                self.browser.selection = None;
                return;
            }
            KeyCode::Char('y') => {
                let address = self.browser.location.as_ref().map(Location::url).unwrap_or_default();
                return self.copy(&address, "address");
            }
            KeyCode::Char('Y') if self.browser.view_source => {
                let source = self.browser.source.clone().unwrap_or_default();
                return self.copy(&source, "page source");
            }
            KeyCode::Char('Y') => {
                // Rows with a background are padded across; the padding isn't text.
                let rows: Vec<&str> = self.regions.page_text.iter().map(|row| row.trim_end()).collect();
                let text = rows.join("\n").trim_end().to_string();
                return self.copy(&text, "page text");
            }
            KeyCode::Char('L') => return self.copy_selected_link(),
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return self.paste_from_clipboard();
            }
            _ => {}
        }
        if self.browser.focus == BrowserFocus::Pane {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => self.move_pane_selection(1),
                KeyCode::Up | KeyCode::Char('k') => self.move_pane_selection(-1),
                KeyCode::Enter => self.open_pane_selection(),
                KeyCode::Char('x') | KeyCode::Delete if self.browser.pane == BrowserPane::Saved => {
                    self.remove_saved();
                }
                _ => {}
            }
            return;
        }
        let height = self.browser.viewport.max(1);
        // The source has no links or fields to move between.
        let on_page = !self.browser.view_source;
        match key.code {
            KeyCode::Tab | KeyCode::Char('n') if on_page => self.move_selection(true),
            KeyCode::BackTab | KeyCode::Char('p') if on_page => self.move_selection(false),
            KeyCode::Enter if on_page => {
                if let Some(index) = self.browser.selected {
                    self.activate(index);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.browser.scroll += 1,
            KeyCode::Up | KeyCode::Char('k') => {
                self.browser.scroll = self.browser.scroll.saturating_sub(1);
            }
            KeyCode::PageDown | KeyCode::Char(' ') => self.browser.scroll += height,
            KeyCode::PageUp => self.browser.scroll = self.browser.scroll.saturating_sub(height),
            KeyCode::Home => self.browser.scroll = 0,
            _ => {}
        }
    }

    fn open_goto(&mut self) {
        let current = self
            .browser
            .location
            .as_ref()
            .map(Location::url)
            .unwrap_or_default();
        self.open_prompt(PromptKind::GoTo, "Go to (hash:/page/path.mu)", &current);
    }

    pub(super) fn back(&mut self) {
        if let Some(previous) = self.browser.history.pop() {
            self.load(previous, false, false);
        }
    }

    fn reload(&mut self) {
        if let Some(location) = self.browser.location.clone() {
            self.load(location, false, true);
        }
    }

    fn clear_cache(&mut self) {
        let removed = self.cache.clear();
        self.notify(format!("Cleared {removed} cached page(s) and image(s)"));
        self.reload();
    }

    /// Page layout cell under a screen position, clamped to the page area.
    pub(super) fn page_cell(&self, at: Position) -> (usize, usize) {
        let page = self.regions.page;
        let y = at.y.clamp(page.y, page.bottom().saturating_sub(1));
        let x = at.x.clamp(page.x, page.right().saturating_sub(1));
        (self.browser.scroll + (y - page.y) as usize, (x - page.x) as usize)
    }

    /// Switch between the rendered page and its Micron source.
    fn toggle_source(&mut self) {
        if self.browser.source.is_none() {
            self.warn("Open a page to view its source");
            return;
        }
        self.browser.view_source = !self.browser.view_source;
        self.browser.focus = BrowserFocus::Page;
        self.browser.selection = None;
        self.browser.scroll = 0;
    }

    pub(super) fn click_browser(&mut self, at: Position) {
        if self.regions.source_button.contains(at) {
            self.toggle_source();
            return;
        }
        if self.regions.address.contains(at) {
            self.open_goto();
            return;
        }
        if let Some(&(_, pane)) =
            self.regions.browser_tabs.iter().find(|(r, _)| r.contains(at))
        {
            self.switch_pane(pane);
            return;
        }
        let list = self.regions.browser_list;
        if list.contains(at) {
            // A single click opens the page next to the list.
            self.browser.focus = BrowserFocus::Pane;
            let offset = self.pane_list().offset();
            let index = offset + (at.y - list.y) as usize;
            if index < self.pane_len() {
                self.pane_list().select(Some(index));
                self.open_pane_selection();
            }
        }
        // Page clicks are handled on release (see `on_mouse`).
    }
}

/// Accepts `<hash>`, `<hash>:/page/x.mu`, or `:/page/x.mu` / `/page/x.mu`
/// relative to `current`.
pub fn resolve_url(url: &str, current: Option<Hash>) -> Option<Location> {
    let url = url.trim().trim_start_matches("nomadnetwork://");
    let (node, path) = match url.split_once(':') {
        Some((node, path)) => {
            let node = if node.is_empty() { current? } else { parse_hash(node)? };
            (node, path.to_string())
        }
        None if url.starts_with('/') => (current?, url.to_string()),
        None => (parse_hash(url)?, String::new()),
    };
    let path = if path.is_empty() || path == "/" {
        nomad_core::DEFAULT_INDEX_ROUTE.to_string()
    } else {
        path
    };
    Some(Location {
        node,
        path,
        fields: BTreeMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::config::Settings;
    use crate::store::Store;

    /// An app showing its own node's index page (read from its folder).
    fn showing(name: &str, pages: &[(&str, &str)]) -> App {
        let dir = std::env::temp_dir().join(format!("rettui-browser-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, Settings::default(), Store::default());
        let folder = app.paths.node.join("pages");
        std::fs::create_dir_all(&folder).unwrap();
        for (path, text) in pages {
            std::fs::write(folder.join(path), text).unwrap();
        }
        let location = Location { node: app.node.hash, path: "/page/index.mu".into(), fields: BTreeMap::new() };
        app.navigate(location);
        app
    }

    #[test]
    fn a_load_says_how_finding_a_path_goes() {
        let dir = std::env::temp_dir().join(format!("rettui-browser-progress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, Settings::default(), Store::default());
        app.navigate(Location { node: [9; 16], path: "/page/index.mu".into(), fields: BTreeMap::new() });
        let id = app.browser.loading.as_ref().expect("loading from the network").id;
        assert_eq!(app.browser.loading.as_ref().unwrap().status, None);
        app.on_net(crate::net::NetEvent::FetchProgress { id: id + 1, text: "path request 2 of 3".into() });
        assert_eq!(app.browser.loading.as_ref().unwrap().status, None, "another load's");
        app.on_net(crate::net::NetEvent::FetchProgress { id, text: "path request 2 of 3".into() });
        assert_eq!(app.browser.loading.as_ref().unwrap().status.as_deref(), Some("path request 2 of 3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partials_load_and_reload_from_p_links() {
        let mut app = showing("partials", &[
            ("index.mu", "`{:/page/count.mu`0`pid=4|name}\n`[Again`p:4]\nName: `<name`Ann>"),
            ("count.mu", "#!/bin/sh\nfirst"),
        ]);
        let page = app.browser.page.as_ref().unwrap();
        let shown = |app: &App| {
            let layout = app.browser.page.as_ref().unwrap().layout(40, None, &HashMap::new(), None);
            layout.lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>()
        };
        assert_eq!(page.partials.len(), 1);
        assert_eq!(shown(&app)[0], "first");
        // What's typed stays when it reloads (from a `p:` link).
        app.browser.page.as_mut().unwrap().fields[0].value = "Bo".into();
        std::fs::write(app.paths.node.join("pages/count.mu"), "second").unwrap();
        let link = app.browser.page.as_ref().unwrap().items.iter().position(|i| matches!(i, Interactive::Link { .. })).unwrap();
        app.activate(link);
        assert_eq!(shown(&app)[0], "second");
        assert_eq!(app.browser.page.as_ref().unwrap().fields[0].value, "Bo");
        let _ = std::fs::remove_dir_all(app.paths.node.parent().unwrap());
    }

    #[test]
    fn anchors_and_folds_from_links_and_keys() {
        let mut app = showing("anchors", &[(
            "index.mu",
            "`[Down`#end]\n`->Fold\nhidden\n<\na\nb\nc\n`:end\nlast\nName: `<5x2|notes`>",
        )]);
        app.activate(0);
        assert_eq!(app.browser.jump_to, Some(app.browser.page.as_ref().unwrap().anchor_line("end").unwrap()));
        // The fold opens with Enter on its heading.
        assert!(!app.browser.page.as_ref().unwrap().folds[0].open);
        app.browser.focus = BrowserFocus::Page;
        app.browser.selected = Some(1);
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.browser.page.as_ref().unwrap().folds[0].open);
        // A field of several rows is edited on one line, ↵ for new lines.
        app.activate(2);
        for c in "one ↵ two".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.browser.page.as_ref().unwrap().fields[0].value, "one\ntwo");
        let _ = std::fs::remove_dir_all(app.paths.node.parent().unwrap());
    }
}
