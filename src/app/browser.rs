//! Browser tab: NomadNet pages (from the cache when fresh), links and
//! forms, images, text selection, and the saved pages / nodes pane.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;
use ratatui::widgets::ListState;

use super::files::unique_path;
use super::network::{NetSearch, matches, matches_text};
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

/// The most Browser tabs open at once: each keeps its page and images.
pub const MAX_TABS: usize = 12;

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
    /// The tab on screen; the others wait in `parked`, in order, with this
    /// one at `current` among them.
    pub tab: PageTab,
    pub(super) parked: Vec<PageTab>,
    pub current: usize,
    /// Rows of the page area in the last layout.
    pub viewport: usize,
    pub pane: BrowserPane,
    pub focus: BrowserFocus,
    pub saved_list: ListState,
    pub nodes_list: ListState,
    /// Narrows both lists: nodes by name or address, saved pages by name
    /// or address.
    pub search: NetSearch,
    pub(super) dragging: bool,
}

/// One tab of the Browser: a page, how it got there, and what's loading.
#[derive(Default)]
pub struct PageTab {
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
    /// Rows of each interactive in the last layout.
    pub item_rows: Vec<usize>,
    /// Decoded page images keyed by their Micron URL.
    pub images: HashMap<String, Picture>,
    /// In-flight media requests: request id -> (Micron URL, where it lives).
    pub(super) media_requests: HashMap<u64, (String, Location, bool)>,
    /// Age of the shown page when it came from the cache.
    pub cached_age: Option<std::time::Duration>,
    /// Text selected on the page with the mouse.
    pub selection: Option<Selection>,
    /// The shown page's partials, by index: what loaded, and when.
    pub partials: Vec<PartialState>,
    /// In-flight partial requests: request id -> (partial, page address).
    partial_requests: HashMap<u64, (usize, String)>,
    /// The first row of each of the page's lines in the last layout.
    pub line_rows: Vec<Option<usize>>,
    /// A line to scroll to at the next layout (an anchor jumped to).
    pub jump_to: Option<usize>,
    /// Finding text in the page (`f`).
    pub find: Option<super::find::PageFind>,
}

impl Browser {
    /// Reticulum restarted: what any tab was loading is gone with it.
    pub(super) fn restarted(&mut self) {
        for tab in std::iter::once(&mut self.tab).chain(self.parked.iter_mut()) {
            if tab.loading.take().is_some() {
                tab.error = Some("Reticulum restarted while loading; load the page again".into());
            }
            tab.media_requests.clear();
        }
    }
}

impl PageTab {
    /// Whether a reply to request `id` is this tab's.
    fn awaits(&self, id: u64) -> bool {
        self.loading.as_ref().is_some_and(|p| p.id == id)
            || self.media_requests.contains_key(&id)
            || self.partial_requests.contains_key(&id)
    }
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
        // A tab out of sight still gets what it asked for.
        if !self.browser.tab.awaits(id)
            && let Some(index) = self.browser_tab_awaiting(id)
        {
            return self.in_browser_tab(index, |app| app.on_fetched(id, result));
        }
        if let Some((index, page)) = self.browser.tab.partial_requests.remove(&id) {
            return self.on_partial(index, &page, result);
        }
        if let Some((url, location, identified)) = self.browser.tab.media_requests.remove(&id) {
            match result {
                Ok(content) => {
                    self.cache.put(location.node, &location.path, identified, &content.data, None);
                    self.decode_later(DecodeFor::Page(url), Some(content.data));
                }
                Err(e) => tracing::debug!("could not load image {url}: {e}"),
            }
            return;
        }
        if self.browser.tab.loading.as_ref().is_none_or(|p| p.id != id) {
            return; // cancelled or superseded
        }
        let Some(pending) = self.browser.tab.loading.take() else {
            return;
        };
        let location = &pending.location;
        match result {
            Ok(content) if location.path.starts_with(nomad_core::FILE_PREFIX) => {
                let name = nomad_core::reply_file_name(content.metadata.as_deref())
                    .or_else(|| location.path.rsplit('/').next().map(str::to_string))
                    .unwrap_or_else(|| "download".to_string());
                let folder =
                    self.settings.nomad_files_dir.as_deref().map_or_else(|| self.paths.downloads.clone(), super::files::path_from_input);
                let saved = std::fs::create_dir_all(&folder).and_then(|()| {
                    let target = unique_path(&folder, &name);
                    std::fs::write(&target, &content.data).map(|()| target)
                });
                match saved {
                    Ok(target) => self.notify(format!("Saved {}", target.display())),
                    Err(e) => self.fail(format!("Could not save {name}: {e}")),
                }
            }
            Ok(content) => {
                if location.fields.is_empty() {
                    let ttl = micron::cache_directive(&String::from_utf8_lossy(&content.data)).map(std::time::Duration::from_secs);
                    self.cache.put(location.node, &location.path, pending.identified, &content.data, ttl);
                }
                self.show_page(pending, &content.data, None);
            }
            Err(e) => {
                self.log(format!("Could not load {}: {e}", location.url()));
                self.browser.tab.error = Some(e);
            }
        }
    }

    fn show_page(&mut self, pending: Pending, data: &[u8], cached_age: Option<std::time::Duration>) {
        let source = String::from_utf8_lossy(data).into_owned();
        let page = micron::parse(&source);
        // A link's `anchor=name`: where to jump once it shows.
        self.browser.tab.jump_to = micron::anchor_variable(&pending.location.fields).and_then(|name| page.anchor_line(name));
        // Reloading keeps the source view; another page shows the page.
        if self.browser.tab.location.as_ref() != Some(&pending.location) {
            self.browser.tab.view_source = false;
        }
        if let Some(previous) = self.browser.tab.location.take()
            && pending.record_history
            && previous != pending.location
        {
            self.browser.tab.history.push(previous);
        }
        self.browser.tab.selected = if page.items.is_empty() { None } else { Some(0) };
        self.browser.tab.images.clear();
        self.browser.tab.media_requests.clear();
        let image_urls = page.image_urls();
        self.browser.tab.page = Some(page);
        self.browser.tab.source = Some(source);
        self.browser.tab.location = Some(pending.location);
        self.browser.tab.cached_age = cached_age;
        self.browser.tab.selection = None;
        self.browser.tab.scroll = 0;
        self.browser.tab.error = None;
        for url in image_urls {
            self.request_image(url, pending.refresh);
        }
        self.browser.tab.partial_requests.clear();
        let count = self.browser.tab.page.as_ref().map_or(0, |p| p.partials.len());
        self.browser.tab.partials = vec![PartialState::default(); count];
        for index in 0..count {
            self.request_partial(index);
        }
    }

    /// Load (or reload) one of the shown page's partials, with the fields
    /// and variables it asks for. Partials always come from the network.
    fn request_partial(&mut self, index: usize) {
        let (Some(page), Some(current)) = (&self.browser.tab.page, &self.browser.tab.location) else { return };
        let Some(partial) = page.partials.get(index) else { return };
        let Some(mut location) = self.resolve(&partial.url) else {
            let why = format!("`Ff66This part of the page has an address rettui can't read: {}`f", partial.url.replace('`', "'"));
            self.browser.tab.partials[index].content = Some(why);
            return;
        };
        location.fields = page.request_fields(&partial.fields);
        let page_url = current.url();
        self.browser.tab.partials[index].loading = true;
        let id = self.request_id();
        self.browser.tab.partial_requests.insert(id, (index, page_url));
        let identify = self.identifies_to(location.node);
        if self.is_own_node(location.node) {
            if let Some(result) = self.own_node_content(id, &location, identify) {
                self.on_fetched(id, result);
            }
            return;
        }
        self.send(NetCommand::Fetch { id, node: location.node, path: location.path, fields: location.fields, identify });
    }

    /// A partial loaded (or not): the page is read again with it in place,
    /// keeping what was typed and folded.
    fn on_partial(&mut self, index: usize, page_url: &str, result: Result<FetchedContent, String>) {
        if self.browser.tab.location.as_ref().map(Location::url).as_deref() != Some(page_url) || index >= self.browser.tab.partials.len() {
            return;
        }
        let content = match result {
            Ok(content) => String::from_utf8_lossy(&content.data).into_owned(),
            Err(e) => format!("`Ff66Could not load this part of the page: {}`f", e.replace('`', "'")),
        };
        let state = &mut self.browser.tab.partials[index];
        (state.content, state.loading, state.loaded_at) = (Some(content), false, Some(Instant::now()));
        let (Some(source), Some(old)) = (&self.browser.tab.source, &self.browser.tab.page) else { return };
        let contents: Vec<Option<String>> = self.browser.tab.partials.iter().map(|p| p.content.clone()).collect();
        let mut page = micron::parse_with(source, &contents);
        page.keep_state(old);
        let known: Vec<String> = old.image_urls();
        let new_images: Vec<String> = page.image_urls().into_iter().filter(|u| !known.contains(u)).collect();
        self.browser.tab.selected = self.browser.tab.selected.filter(|&s| s < page.items.len()).or((!page.items.is_empty()).then_some(0));
        self.browser.tab.page = Some(page);
        for url in new_images {
            self.request_image(url, false);
        }
    }

    /// Reload the partials that ask to be, every so often, while the page
    /// is on screen.
    pub(super) fn refresh_partials(&mut self) {
        if self.tab != Tab::Browser || self.browser.tab.view_source {
            return;
        }
        let Some(page) = &self.browser.tab.page else { return };
        let due: Vec<usize> = page
            .partials
            .iter()
            .zip(&self.browser.tab.partials)
            .enumerate()
            .filter(|(_, (partial, state))| {
                !state.loading && partial.refresh.is_some_and(|every| state.loaded_at.is_some_and(|at| at.elapsed().as_secs() >= every))
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
            Some(line) => self.browser.tab.jump_to = Some(line),
            None => self.warn(format!("There's no {what} on this page")),
        }
    }

    /// A page image decoded in the background: show it if the page is still
    /// the one that asked for it.
    pub(super) fn on_page_image(&mut self, url: String, picture: Option<Picture>) {
        let Some(picture) = picture else {
            tracing::debug!("could not decode image {url}");
            return;
        };
        // The tab whose page shows it, on screen or not.
        let wants = |tab: &PageTab| tab.page.as_ref().is_some_and(|page| page.image_urls().contains(&url));
        let browser = &mut self.browser;
        if let Some(tab) = std::iter::once(&mut browser.tab).chain(browser.parked.iter_mut()).find(|tab| wants(tab)) {
            tab.images.insert(url, picture);
        }
    }

    /// How finding a path for a page is going, in whichever tab loads it.
    pub(super) fn on_fetch_progress(&mut self, id: u64, text: String) {
        let browser = &mut self.browser;
        let loading = std::iter::once(&mut browser.tab).chain(browser.parked.iter_mut()).filter_map(|tab| tab.loading.as_mut());
        if let Some(pending) = loading.into_iter().find(|pending| pending.id == id) {
            pending.status = Some(text);
        }
    }

    /// How many Browser tabs there are (one at least).
    pub fn browser_tab_count(&self) -> usize {
        self.browser.parked.len() + 1
    }

    /// The tabs in order, the one on screen among them.
    pub fn browser_tabs(&self) -> Vec<&PageTab> {
        let mut tabs: Vec<&PageTab> = self.browser.parked.iter().collect();
        tabs.insert(self.browser.current, &self.browser.tab);
        tabs
    }

    /// Show tab `index` (in tab order).
    pub fn switch_browser_tab(&mut self, index: usize) {
        let browser = &mut self.browser;
        if index == browser.current || index > browser.parked.len() {
            return;
        }
        let shown = std::mem::take(&mut browser.tab);
        browser.parked.insert(browser.current, shown);
        browser.tab = browser.parked.remove(index);
        browser.current = index;
        browser.dragging = false;
        browser.focus = if browser.tab.page.is_some() { BrowserFocus::Page } else { BrowserFocus::Pane };
    }

    /// A new tab after the one on screen, opening `location` if there is
    /// one.
    pub fn new_browser_tab(&mut self, location: Option<Location>) {
        if self.browser_tab_count() >= MAX_TABS {
            self.warn(format!("{MAX_TABS} tabs are open already: close one (W) first"));
            return;
        }
        let browser = &mut self.browser;
        let shown = std::mem::take(&mut browser.tab);
        browser.parked.insert(browser.current, shown);
        browser.current += 1;
        browser.dragging = false;
        browser.focus = BrowserFocus::Pane;
        if let Some(location) = location {
            self.navigate(location);
        }
    }

    /// Close the tab on screen (what it was loading is let go) and show
    /// the next one, or the one before if it was the last. The only tab is
    /// emptied instead.
    pub fn close_browser_tab(&mut self) {
        let browser = &mut self.browser;
        if browser.parked.is_empty() {
            browser.tab = PageTab::default();
        } else {
            if browser.current == browser.parked.len() {
                browser.current -= 1;
            }
            browser.tab = browser.parked.remove(browser.current);
        }
        browser.dragging = false;
        browser.focus = if browser.tab.page.is_some() { BrowserFocus::Page } else { BrowserFocus::Pane };
    }

    /// Close tab `index` (in tab order). The tab on screen stays, unless it
    /// is the one closed.
    pub fn close_browser_tab_at(&mut self, index: usize) {
        let browser = &mut self.browser;
        if index == browser.current {
            return self.close_browser_tab();
        }
        if index > browser.parked.len() {
            return;
        }
        if index < browser.current {
            browser.parked.remove(index);
            browser.current -= 1;
        } else {
            browser.parked.remove(index - 1);
        }
    }

    /// A middle-click, or with `close` false a Ctrl+click: a link, saved
    /// page or node opens in a new tab (in the list, the other way from a
    /// click; see `browser_new_tabs`), and a tab middle-clicked closes.
    pub(super) fn open_in_new_tab_at(&mut self, at: Position, close: bool) {
        if let Some(&(_, index)) = self.regions.page_tabs.iter().find(|(r, _)| r.contains(at)) {
            return if close { self.close_browser_tab_at(index) } else { self.switch_browser_tab(index) };
        }
        if self.regions.page.contains(at) {
            let (row, col) = self.page_cell(at);
            if let Some(item) = micron::hit_at(&self.regions.page_hits, row, col) {
                self.open_item_in_new_tab(item);
            }
            return;
        }
        let list = self.regions.browser_list;
        if list.contains(at) {
            let index = self.pane_list().offset() + (at.y - list.y) as usize;
            if index < self.pane_len() {
                self.browser.focus = BrowserFocus::Pane;
                self.browser.search.typing = false;
                self.pane_list().select(Some(index));
                if let Some(location) = self.pane_selection() {
                    self.open_in_tab(location, !self.settings.browser_new_tabs);
                }
            }
            return;
        }
        self.click_browser(at);
    }

    /// Page item `item` in a new tab if it's a link to a page; anything
    /// else does what a click does.
    pub(super) fn open_item_in_new_tab(&mut self, item: usize) {
        self.browser.focus = BrowserFocus::Page;
        self.browser.tab.selected = Some(item);
        match self.selected_link_location() {
            Some(location) => self.new_browser_tab(Some(location)),
            None => self.activate(item),
        }
    }

    /// The tab out of sight (its place in tab order) waiting for request
    /// `id`.
    fn browser_tab_awaiting(&self, id: u64) -> Option<usize> {
        let index = self.browser.parked.iter().position(|tab| tab.awaits(id))?;
        Some(if index < self.browser.current { index } else { index + 1 })
    }

    /// Run `f` with tab `index` on screen, then put back the one that was,
    /// as it was.
    fn in_browser_tab(&mut self, index: usize, f: impl FnOnce(&mut App)) {
        let (shown, focus) = (self.browser.current, self.browser.focus);
        self.switch_browser_tab(index);
        f(self);
        self.switch_browser_tab(shown);
        self.browser.focus = focus;
    }

    /// The selected link's page in a new tab. In the list, the selected
    /// saved page or node the other way from Enter (see
    /// `browser_new_tabs`). Without one, the page on screen again.
    fn open_in_new_tab(&mut self) {
        let location = if self.browser.focus == BrowserFocus::Pane {
            match self.pane_selection() {
                Some(location) => return self.open_in_tab(location, !self.settings.browser_new_tabs),
                None => None,
            }
        } else {
            self.selected_link_location()
        };
        match location.or_else(|| self.browser.tab.location.clone()) {
            Some(location) => self.new_browser_tab(Some(location)),
            None => self.new_browser_tab(None),
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
            let result = self.own_node_content(id, &location, identified);
            self.browser.tab.media_requests.insert(id, (url, location, identified));
            if let Some(result) = result {
                self.on_fetched(id, result);
            }
            return;
        }
        self.send(NetCommand::Fetch {
            id,
            node: location.node,
            path: location.path.clone(),
            fields: BTreeMap::new(),
            identify: identified,
        });
        self.browser.tab.media_requests.insert(id, (url, location, identified));
    }

    pub fn identifies_to(&self, node: Hash) -> bool {
        self.store.identified_nodes.contains(&hex::encode(node))
    }

    fn toggle_identify(&mut self) {
        let Some(location) = self.browser.tab.location.clone() else {
            return;
        };
        let on = !self.identifies_to(location.node);
        self.set_identify(location.node, on);
        self.load(location, false, false);
    }

    /// A link written in a message (see [`crate::links`]): a page or file
    /// opens in the Browser, an address its conversation, a hub in
    /// Channels (asking first, for a hub not added yet).
    pub fn open_address(&mut self, url: &str) {
        let url = url.trim();
        // Schemes in any case, as they're found.
        let scheme = url.find("://").or_else(|| url.find('@')).filter(|&i| url[..i].chars().all(|c| c.is_ascii_alphabetic()));
        let url = match scheme {
            Some(i) => format!("{}{}", url[..i].to_ascii_lowercase(), &url[i..]),
            None => url.to_string(),
        };
        if url.starts_with("rrc://") || url.starts_with("rrc@") {
            return self.open_rrc_link(&url);
        }
        if url.starts_with("lxma://") {
            return match self.add_contact(&url) {
                Ok(key) => self.open_conversation(key),
                Err(e) => self.warn(e),
            };
        }
        if let Some(hash) = url.strip_prefix("lxmf@").or_else(|| url.strip_prefix("lxmf://")) {
            return match parse_hash(hash) {
                Some(hash) => self.open_conversation(hex::encode(hash)),
                None => self.warn(format!("Bad LXMF address: {url}")),
            };
        }
        match self.resolve(&url) {
            Some(location) => self.navigate(location),
            None => self.warn(format!("Cannot open {url}")),
        }
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
        let cacheable =
            location.fields.is_empty() && !location.path.starts_with(nomad_core::FILE_PREFIX) && !self.is_own_node(location.node);
        let id = self.request_id();
        let pending = Pending { id, location, started: Instant::now(), record_history, identified, refresh, status: None };
        if cacheable
            && !refresh
            && let Some(cached) = self.cache.get(pending.location.node, &pending.location.path, identified)
        {
            self.browser.tab.loading = None;
            self.show_page(pending, &cached.data, Some(cached.age));
            return;
        }
        self.browser.tab.error = None;
        if self.is_own_node(pending.location.node) {
            let result = self.own_node_content(id, &pending.location, identified);
            self.browser.tab.loading = Some(pending);
            if let Some(result) = result {
                self.on_fetched(id, result);
            }
            return;
        }
        self.send(NetCommand::Fetch {
            id,
            node: pending.location.node,
            path: pending.location.path.clone(),
            fields: pending.location.fields.clone(),
            identify: identified,
        });
        self.browser.tab.loading = Some(pending);
    }

    /// Accepts `<hash>`, `<hash>:/page/x.mu`, or `:/page/x.mu` / `/page/x.mu`
    /// relative to the current node.
    pub(super) fn resolve(&self, url: &str) -> Option<Location> {
        resolve_url(url, self.browser.tab.location.as_ref().map(|l| l.node))
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
        let changed = if on { self.store.identified_nodes.insert(key.clone()) } else { self.store.identified_nodes.remove(&key) };
        if changed {
            let name = self.store.display_name(&key);
            self.notify(if on { format!("Identifying to {name}") } else { format!("No longer identifying to {name}") });
            self.store_dirty = true;
        }
    }

    fn follow_link(&mut self, url: &str, spec: &[String]) {
        // An anchor on this page (`#` alone: the next heading).
        if let Some(name) = url.strip_prefix('#') {
            let Some(page) = &self.browser.tab.page else { return };
            let line = if name.is_empty() {
                self.browser.tab.selected.and_then(|item| page.next_heading_line(item))
            } else {
                page.anchor_line(name)
            };
            let what = if name.is_empty() { "heading after this link".to_string() } else { format!("anchor #{name}") };
            return self.jump_to_line(line, &what);
        }
        // Reload the partials with these ids.
        if let Some(ids) = url.strip_prefix("p:") {
            let due = self.browser.tab.page.as_ref().map(|page| page.partials_with_ids(ids)).unwrap_or_default();
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
        if let Some(hash) = url.strip_prefix("lxmf@").or_else(|| url.strip_prefix("lxmf://")) {
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
        if let Some(page) = &self.browser.tab.page {
            location.fields = page.request_fields(spec);
        }
        self.navigate(location);
    }

    pub(super) fn activate(&mut self, index: usize) {
        let Some(page) = self.browser.tab.page.as_mut() else {
            return;
        };
        self.browser.tab.selected = Some(index);
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
                    self.prompt = Some(Prompt { kind: PromptKind::EditField(f), title, input: TextInput::with_text(&text) });
                } else {
                    page.activate_choice(f);
                }
            }
            Some(Interactive::Fold(f)) => page.folds[f].open = !page.folds[f].open,
            None => {}
        }
    }

    fn move_selection(&mut self, forward: bool) {
        let Some(page) = &self.browser.tab.page else { return };
        let count = page.items.len();
        if count == 0 {
            return;
        }
        let next = match self.browser.tab.selected {
            None => 0,
            Some(i) if forward => (i + 1) % count,
            Some(i) => (i + count - 1) % count,
        };
        self.browser.tab.selected = Some(next);
        // Scroll the selection into view.
        if let Some(&row) = self.browser.tab.item_rows.get(next) {
            let height = self.browser.viewport.max(1);
            if row < self.browser.tab.scroll || row >= self.browser.tab.scroll + height {
                self.browser.tab.scroll = row.saturating_sub(height / 3);
            }
        }
    }

    /// Index of the page text field selected in the browser, if any.
    pub(super) fn selected_text_field(&self) -> Option<usize> {
        if self.tab != Tab::Browser || self.browser.focus != BrowserFocus::Page {
            return None;
        }
        let page = self.browser.tab.page.as_ref()?;
        match page.items.get(self.browser.tab.selected?)? {
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
        let link = self.browser.tab.page.as_ref().zip(self.browser.tab.selected).and_then(|(page, i)| match page.items.get(i) {
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

    /// NomadNet nodes heard that match the search, most recently heard
    /// first.
    pub fn browser_nodes(&self) -> Vec<(&String, &Peer)> {
        let terms = self.browser.search.terms();
        let mut nodes: Vec<_> = self.store.peers.iter().filter(|(hash, p)| p.kind == PeerKind::Nomad && matches(&terms, hash, p)).collect();
        nodes.sort_by(|a, b| b.1.last_seen.cmp(&a.1.last_seen).then_with(|| a.0.cmp(b.0)));
        nodes
    }

    /// Saved pages that match the search, with where each is kept.
    pub fn browser_saved(&self) -> Vec<(usize, &Bookmark)> {
        let terms = self.browser.search.terms();
        let saved = self.store.saved.iter().enumerate();
        saved.filter(|(_, b)| matches_text(&terms, &[&b.name, &b.url])).collect()
    }

    fn pane_len(&self) -> usize {
        match self.browser.pane {
            BrowserPane::Saved => self.browser_saved().len(),
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
        if let Some(location) = self.pane_selection() {
            self.open_in_tab(location, self.settings.browser_new_tabs);
        }
    }

    /// Open `location` in a new tab, or in the tab shown. An empty tab
    /// shown takes it either way.
    fn open_in_tab(&mut self, location: Location, new: bool) {
        let tab = &self.browser.tab;
        if new && (tab.location.is_some() || tab.loading.is_some()) {
            self.new_browser_tab(Some(location));
        } else {
            self.navigate(location);
        }
    }

    /// The saved page or node selected in the list.
    fn pane_selection(&mut self) -> Option<Location> {
        let index = self.pane_list().selected()?;
        let url = match self.browser.pane {
            BrowserPane::Saved => self.browser_saved().get(index).map(|(_, b)| b.url.clone()),
            BrowserPane::Nodes => self.browser_nodes().get(index).map(|(k, _)| (*k).clone()),
        };
        url.and_then(|u| self.resolve(&u))
    }

    /// Where the selected link leads, if it's a page or file (with what it
    /// sends from the page's fields).
    fn selected_link_location(&self) -> Option<Location> {
        let page = self.browser.tab.page.as_ref()?;
        let Some(Interactive::Link { url, fields }) = page.items.get(self.browser.tab.selected?) else {
            return None;
        };
        if ["#", "p:", "rrc://", "rrc@", "lxmf@", "lxmf://"].iter().any(|prefix| url.starts_with(prefix)) {
            return None;
        }
        let mut location = self.resolve(url)?;
        location.fields = page.request_fields(fields);
        Some(location)
    }

    fn switch_pane(&mut self, pane: BrowserPane) {
        self.browser.pane = pane;
        self.browser.focus = BrowserFocus::Pane;
        if self.pane_list().selected().is_none() && self.pane_len() > 0 {
            self.pane_list().select(Some(0));
        }
    }

    fn save_current_page(&mut self) {
        let Some(location) = self.browser.tab.location.clone() else {
            self.log("Open a page to save it");
            return;
        };
        self.save(location);
    }

    /// Save the home page of the node selected in the Nodes list, without
    /// opening it: the same as saving that page once open.
    fn save_selected_node(&mut self) {
        let index = self.browser.nodes_list.selected();
        let node = index.and_then(|i| self.browser_nodes().get(i).map(|(hash, _)| (*hash).clone()));
        if let Some(location) = node.and_then(|hash| resolve_url(&hash, None)) {
            self.save(location);
        }
    }

    fn save(&mut self, location: Location) {
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
        let Some(kept_at) = self.browser_saved().get(index).map(|&(at, _)| at) else {
            return;
        };
        let removed = self.store.saved.remove(kept_at);
        self.log(format!("Removed {}", removed.name));
        self.store_dirty = true;
        let len = self.browser_saved().len();
        self.browser.saved_list.select((len > 0).then(|| index.min(len - 1)));
    }

    /// Keys while the pane's search box has focus.
    pub(super) fn browser_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.clear_browser_search(),
            KeyCode::Enter => self.browser.search.typing = false,
            KeyCode::Up => self.move_pane_selection(-1),
            KeyCode::Down => self.move_pane_selection(1),
            _ => {
                if self.browser.search.input.handle(key) {
                    self.browser_search_changed();
                }
            }
        }
    }

    pub(super) fn paste_browser_search(&mut self, text: &str) {
        self.browser.search.input.insert_str(text);
        self.browser_search_changed();
    }

    fn search_browser(&mut self) {
        self.browser.search.typing = true;
        self.browser.focus = BrowserFocus::Pane;
    }

    fn clear_browser_search(&mut self) {
        self.browser.search.input.take();
        self.browser.search.typing = false;
        self.browser_search_changed();
    }

    /// Show the best match first in both lists whenever the search changes.
    fn browser_search_changed(&mut self) {
        let saved = !self.browser_saved().is_empty();
        let nodes = !self.browser_nodes().is_empty();
        self.browser.saved_list = ListState::default().with_selected(saved.then_some(0));
        self.browser.nodes_list = ListState::default().with_selected(nodes.then_some(0));
    }

    pub(super) fn browser_key(&mut self, key: KeyEvent) {
        // Finding in the page: keys go to what's looked for.
        if self.browser.tab.find.is_some() {
            return self.find_key(key);
        }
        // Keys that work whichever pane has focus.
        match key.code {
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => return self.open_find(),
            KeyCode::Char('f') => return self.open_find(),
            KeyCode::Char('g') | KeyCode::Char('o') => return self.open_goto(),
            KeyCode::Char('/') => return self.search_browser(),
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
            // With the Nodes list in hand, the node selected; else the page.
            KeyCode::Char('s') if self.browser.focus == BrowserFocus::Pane && self.browser.pane == BrowserPane::Nodes => {
                return self.save_selected_node();
            }
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
            KeyCode::Right if self.browser.tab.page.is_some() => {
                self.browser.focus = BrowserFocus::Page;
                return;
            }
            KeyCode::Char('u') => return self.toggle_source(),
            KeyCode::Char(c @ '1'..='9') if key.modifiers.contains(KeyModifiers::ALT) => {
                return self.switch_browser_tab(c as usize - '1' as usize);
            }
            KeyCode::Char('T') => return self.open_in_new_tab(),
            KeyCode::Char('W') => return self.close_browser_tab(),
            KeyCode::Char(']') => return self.switch_browser_tab((self.browser.current + 1) % self.browser_tab_count()),
            KeyCode::Char('[') => {
                let count = self.browser_tab_count();
                return self.switch_browser_tab((self.browser.current + count - 1) % count);
            }
            KeyCode::Esc => {
                if self.browser.tab.loading.is_none() && self.browser.tab.view_source {
                    return self.toggle_source();
                }
                let searching = !self.browser.search.input.text().is_empty();
                if self.browser.tab.loading.is_none() && self.browser.focus == BrowserFocus::Pane && searching {
                    return self.clear_browser_search();
                }
                self.browser.tab.loading = None;
                self.browser.tab.selection = None;
                return;
            }
            KeyCode::Char('y') => {
                let address = self.browser.tab.location.as_ref().map(Location::url).unwrap_or_default();
                return self.copy(&address, "address");
            }
            KeyCode::Char('Y') if self.browser.tab.view_source => {
                let source = self.browser.tab.source.clone().unwrap_or_default();
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
        let on_page = !self.browser.tab.view_source;
        match key.code {
            KeyCode::Tab | KeyCode::Char('n') if on_page => self.move_selection(true),
            KeyCode::BackTab | KeyCode::Char('p') if on_page => self.move_selection(false),
            KeyCode::Enter if on_page => {
                if let Some(index) = self.browser.tab.selected {
                    self.activate(index);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.browser.tab.scroll += 1,
            KeyCode::Up | KeyCode::Char('k') => {
                self.browser.tab.scroll = self.browser.tab.scroll.saturating_sub(1);
            }
            KeyCode::PageDown | KeyCode::Char(' ') => self.browser.tab.scroll += height,
            KeyCode::PageUp => self.browser.tab.scroll = self.browser.tab.scroll.saturating_sub(height),
            KeyCode::Home => self.browser.tab.scroll = 0,
            _ => {}
        }
    }

    fn open_goto(&mut self) {
        let current = self.browser.tab.location.as_ref().map(Location::url).unwrap_or_default();
        self.open_prompt(PromptKind::GoTo, "Go to (hash:/page/path.mu)", &current);
    }

    pub(super) fn back(&mut self) {
        if let Some(previous) = self.browser.tab.history.pop() {
            self.load(previous, false, false);
        }
    }

    fn reload(&mut self) {
        if let Some(location) = self.browser.tab.location.clone() {
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
        (self.browser.tab.scroll + (y - page.y) as usize, (x - page.x) as usize)
    }

    /// Switch between the rendered page and its Micron source.
    fn toggle_source(&mut self) {
        if self.browser.tab.source.is_none() {
            self.warn("Open a page to view its source");
            return;
        }
        self.browser.tab.view_source = !self.browser.tab.view_source;
        self.browser.focus = BrowserFocus::Page;
        self.browser.tab.selection = None;
        self.browser.tab.scroll = 0;
    }

    pub(super) fn click_browser(&mut self, at: Position) {
        if self.regions.source_button.contains(at) {
            self.toggle_source();
            return;
        }
        if let Some(&(_, index)) = self.regions.page_tab_close.iter().find(|(r, _)| r.contains(at)) {
            return self.close_browser_tab_at(index);
        }
        if self.regions.new_page_tab.contains(at) {
            return self.new_browser_tab(None);
        }
        if let Some(&(_, index)) = self.regions.page_tabs.iter().find(|(r, _)| r.contains(at)) {
            return self.switch_browser_tab(index);
        }
        if self.regions.address.contains(at) {
            self.open_goto();
            return;
        }
        if let Some(&(_, pane)) = self.regions.browser_tabs.iter().find(|(r, _)| r.contains(at)) {
            self.switch_pane(pane);
            return;
        }
        if self.regions.browser_search.contains(at) {
            return self.search_browser();
        }
        let list = self.regions.browser_list;
        if list.contains(at) {
            // A single click opens the page next to the list.
            self.browser.focus = BrowserFocus::Pane;
            self.browser.search.typing = false;
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
    let path = if path.is_empty() || path == "/" { nomad_core::DEFAULT_INDEX_ROUTE.to_string() } else { path };
    Some(Location { node, path, fields: BTreeMap::new() })
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
        let id = app.browser.tab.loading.as_ref().expect("loading from the network").id;
        assert_eq!(app.browser.tab.loading.as_ref().unwrap().status, None);
        app.on_net(crate::net::NetEvent::FetchProgress { id: id + 1, text: "path request 2 of 3".into() });
        assert_eq!(app.browser.tab.loading.as_ref().unwrap().status, None, "another load's");
        app.on_net(crate::net::NetEvent::FetchProgress { id, text: "path request 2 of 3".into() });
        assert_eq!(app.browser.tab.loading.as_ref().unwrap().status.as_deref(), Some("path request 2 of 3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partials_load_and_reload_from_p_links() {
        let mut app = showing(
            "partials",
            &[("index.mu", "`{:/page/count.mu`0`pid=4|name}\n`[Again`p:4]\nName: `<name`Ann>"), ("count.mu", "#!/bin/sh\nfirst")],
        );
        let page = app.browser.tab.page.as_ref().unwrap();
        let shown = |app: &App| {
            let layout = app.browser.tab.page.as_ref().unwrap().layout(40, None, &HashMap::new(), None);
            layout.lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>()
        };
        assert_eq!(page.partials.len(), 1);
        assert_eq!(shown(&app)[0], "first");
        // What's typed stays when it reloads (from a `p:` link).
        app.browser.tab.page.as_mut().unwrap().fields[0].value = "Bo".into();
        std::fs::write(app.paths.node.join("pages/count.mu"), "second").unwrap();
        let link = app.browser.tab.page.as_ref().unwrap().items.iter().position(|i| matches!(i, Interactive::Link { .. })).unwrap();
        app.activate(link);
        assert_eq!(shown(&app)[0], "second");
        assert_eq!(app.browser.tab.page.as_ref().unwrap().fields[0].value, "Bo");
        let _ = std::fs::remove_dir_all(app.paths.node.parent().unwrap());
    }

    #[test]
    fn anchors_and_folds_from_links_and_keys() {
        let mut app = showing("anchors", &[("index.mu", "`[Down`#end]\n`->Fold\nhidden\n<\na\nb\nc\n`:end\nlast\nName: `<5x2|notes`>")]);
        app.activate(0);
        assert_eq!(app.browser.tab.jump_to, Some(app.browser.tab.page.as_ref().unwrap().anchor_line("end").unwrap()));
        // The fold opens with Enter on its heading.
        assert!(!app.browser.tab.page.as_ref().unwrap().folds[0].open);
        app.browser.focus = BrowserFocus::Page;
        app.browser.tab.selected = Some(1);
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.browser.tab.page.as_ref().unwrap().folds[0].open);
        // A field of several rows is edited on one line, ↵ for new lines.
        app.activate(2);
        for c in "one ↵ two".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.browser.tab.page.as_ref().unwrap().fields[0].value, "one\ntwo");
        let _ = std::fs::remove_dir_all(app.paths.node.parent().unwrap());
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    /// An app with three nodes, an LXMF peer and two saved pages, showing
    /// the Browser tab.
    fn with_nodes(name: &str) -> App {
        let dir = std::env::temp_dir().join(format!("rettui-browser-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::default();
        let node = |name: Option<&str>, last_seen| Peer { kind: PeerKind::Nomad, name: name.map(str::to_string), hops: 1, last_seen };
        store.peers.insert("aa".repeat(16), node(Some("Alpha Library"), 30));
        store.peers.insert("bb".repeat(16), node(Some("Beta Wiki"), 20));
        store.peers.insert("cc".repeat(16), node(None, 10));
        store.peers.insert("ab".repeat(16), Peer { kind: PeerKind::Lxmf, name: Some("Alpha person".into()), hops: 1, last_seen: 40 });
        store.saved.push(Bookmark { name: "Alpha Library".into(), url: format!("{}:/page/index.mu", "aa".repeat(16)) });
        store.saved.push(Bookmark { name: "Gamma · news.mu".into(), url: format!("{}:/page/news.mu", "dd".repeat(16)) });
        let mut app = crate::app::test_app(&dir, Settings::default(), store);
        app.tab = Tab::Browser;
        app
    }

    fn node_names(app: &App) -> Vec<String> {
        app.browser_nodes().iter().map(|(hash, p)| p.name.clone().unwrap_or_else(|| (*hash).clone())).collect()
    }

    fn saved_names(app: &App) -> Vec<String> {
        app.browser_saved().iter().map(|(_, b)| b.name.clone()).collect()
    }

    #[test]
    fn searching_the_pane_narrows_both_lists() {
        let mut app = with_nodes("search");
        app.switch_pane(BrowserPane::Nodes);
        // Only nodes, newest first.
        assert_eq!(node_names(&app), ["Alpha Library", "Beta Wiki", &"cc".repeat(16)]);
        // `/` from the page too: the pane takes focus, and keys go to the box.
        app.browser.focus = BrowserFocus::Page;
        press(&mut app, KeyCode::Char('/'));
        assert!(app.browser.search.typing);
        assert_eq!(app.browser.focus, BrowserFocus::Pane);
        type_text(&mut app, "ALPHA lib");
        assert_eq!(node_names(&app), ["Alpha Library"]);
        assert_eq!(saved_names(&app), ["Alpha Library"]);
        assert_eq!(app.browser.nodes_list.selected(), Some(0));
        // Enter keeps the search; keys are the pane's again.
        press(&mut app, KeyCode::Enter);
        assert!(!app.browser.search.typing);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.browser.search.input.text(), "ALPHA lib");
        // Esc in the pane clears it.
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.browser.search.input.text(), "");
        assert_eq!(node_names(&app).len(), 3);
        // Addresses match, pasted as `<hash>` too.
        press(&mut app, KeyCode::Char('/'));
        app.on_paste(&format!("<{}>", "cc".repeat(16)));
        assert_eq!(node_names(&app), ["cc".repeat(16)]);
        // Esc while typing clears it as well.
        press(&mut app, KeyCode::Esc);
        assert!(!app.browser.search.typing);
        assert_eq!(node_names(&app).len(), 3);
        // Saved pages are found by address, and the one shown is removed.
        app.switch_pane(BrowserPane::Saved);
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "dddd");
        press(&mut app, KeyCode::Enter);
        assert_eq!(saved_names(&app), ["Gamma · news.mu"]);
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.store.saved.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Alpha Library"]);
        assert!(saved_names(&app).is_empty());
        let _ = std::fs::remove_dir_all(app.paths.node.parent().unwrap());
    }

    #[test]
    fn s_in_the_nodes_list_saves_the_node_selected() {
        let mut app = with_nodes("save-node");
        app.switch_pane(BrowserPane::Nodes);
        press(&mut app, KeyCode::Down);
        assert_eq!(node_names(&app)[app.browser.nodes_list.selected().unwrap()], "Beta Wiki");
        press(&mut app, KeyCode::Char('s'));
        let home = format!("{}:/page/index.mu", "bb".repeat(16));
        let saved = |app: &App| app.store.saved.iter().map(|b| (b.name.clone(), b.url.clone())).collect::<Vec<_>>();
        assert_eq!(saved(&app).last(), Some(&("Beta Wiki".to_string(), home.clone())));
        // Once only, and the same bookmark as saving the page once open.
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(saved(&app).len(), 3);
        assert_eq!(resolve_url(&"bb".repeat(16), None).unwrap().url(), home);
        // Found by a search, too.
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "cccc");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(saved(&app).last().unwrap().1, format!("{}:/page/index.mu", "cc".repeat(16)));
        // With the page in hand, s saves the page.
        app.browser.tab.location = Some(Location { node: [0xee; 16], path: "/page/about.mu".into(), fields: BTreeMap::new() });
        app.browser.focus = BrowserFocus::Page;
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(saved(&app).last().unwrap().1, format!("{}:/page/about.mu", "ee".repeat(16)));
        let _ = std::fs::remove_dir_all(app.paths.node.parent().unwrap());
    }

    #[test]
    fn every_kind_of_link_and_key_on_a_page() {
        let page = "`[Next heading`#]\n`[Nowhere`#gone]\n`[Reload none`p:9]\n`[Hub`rrc://0101010101010101010101010101010101]\n\
                    `[Bob`lxmf@abababababababababababababababab]\n`[Bad`lxmf@zz]\n`[Broken`nonsense link]\n\
                    `[About`:/page/about.mu`name]\n>Heading\n`<?|agree|yes`Agree> `<^|pick|a`A> `<^|pick|b`B>\nName: `<name`Ann>\n\
                    `(pic`:/media/pic.png)";
        let mut app = showing("links", &[("index.mu", page), ("about.mu", "About us")]);
        std::fs::create_dir_all(app.paths.node.join("media")).unwrap();
        let mut png = Vec::new();
        image::RgbImage::from_pixel(4, 4, image::Rgb([9, 9, 9]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(app.paths.node.join("media/pic.png"), png).unwrap();
        app.tab = Tab::Browser;
        app.browser.focus = BrowserFocus::Page;
        let items = app.browser.tab.page.as_ref().unwrap().items.len();
        // Each item activated in turn, back to the page each time.
        for index in 0..items {
            if app.browser.tab.location.as_ref().is_some_and(|l| l.path != "/page/index.mu") {
                app.back();
            }
            app.tab = Tab::Browser;
            app.prompt = None;
            app.activate(index);
        }
        assert!(app.log.iter().any(|l| l.contains("There's no anchor #gone")), "{:?}", app.log);
        assert!(app.log.iter().any(|l| l.contains("No part of this page has the id 9")));
        assert!(app.log.iter().any(|l| l.contains("Bad LXMF address")));
        assert!(app.store.conversations.contains_key(&"ab".repeat(16)));
        app.prompt = None;
        app.tab = Tab::Browser;
        app.back();
        app.browser.focus = BrowserFocus::Page;
        // Its picture decoded and shown (and one no longer on the page not).
        let url = format!("{}:/media/pic.png", hex::encode(app.node.hash));
        let picture = || Picture::decode(&std::fs::read(app.paths.node.join("media/pic.png")).unwrap());
        let (one, two) = (picture(), picture());
        app.on_page_image(url.clone(), one);
        app.on_page_image("elsewhere".into(), two);
        app.on_page_image(url, None);

        // The page's keys.
        for code in [
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Char('n'),
            KeyCode::Char('p'),
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::PageDown,
            KeyCode::Char(' '),
            KeyCode::PageUp,
            KeyCode::Home,
            KeyCode::Char('L'),
            KeyCode::Char('y'),
            KeyCode::Char('Y'),
            KeyCode::Char('u'),
            KeyCode::Char('Y'),
            KeyCode::Esc,
            KeyCode::Char('I'),
            KeyCode::Char('I'),
            KeyCode::Char('H'),
            KeyCode::Char('r'),
            KeyCode::Char('s'),
            KeyCode::Char('t'),
            KeyCode::Char('t'),
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Char('R'),
        ] {
            app.prompt = None;
            app.tab = Tab::Browser;
            press(&mut app, code);
        }
        app.browser.tab.selected = None;
        press(&mut app, KeyCode::Char('L'));
        app.settings.home = Some(format!("{}:/page/about.mu", hex::encode(app.node.hash)));
        press(&mut app, KeyCode::Char('H'));
        press(&mut app, KeyCode::Char('b'));
        press(&mut app, KeyCode::Char('g'));
        app.prompt = None;
        // The pane: saved pages listed, opened and removed.
        app.browser.focus = BrowserFocus::Pane;
        app.browser.pane = BrowserPane::Saved;
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Enter);
        app.browser.focus = BrowserFocus::Pane;
        press(&mut app, KeyCode::Char('x'));
        type_text(&mut app, "/");
        press(&mut app, KeyCode::Esc);
        let _ = std::fs::remove_dir_all(app.paths.node.parent().unwrap());
    }

    #[test]
    fn files_pictures_and_failures_from_other_nodes() {
        let dir = std::env::temp_dir().join(format!("rettui-browser-remote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        let node = [0xcd; 16];
        let fetched = |net: &mut tokio::sync::mpsc::UnboundedReceiver<NetCommand>| {
            std::iter::from_fn(|| net.try_recv().ok())
                .filter_map(|c| match c {
                    NetCommand::Fetch { id, path, .. } => Some((id, path)),
                    _ => None,
                })
                .last()
        };
        // A file: saved in Downloads under the name it came with.
        app.navigate(Location { node, path: "/file/notes.txt".into(), fields: BTreeMap::new() });
        let (id, _) = fetched(&mut net).unwrap();
        app.on_fetched(id, Ok(FetchedContent { data: b"notes".to_vec(), metadata: None }));
        assert_eq!(std::fs::read(app.paths.downloads.join("notes.txt")).unwrap(), b"notes");
        // With a folder chosen: saved there instead, made if it's new.
        let chosen = app.paths.base.join("elsewhere");
        app.update_settings(&[("nomad_files_dir", &chosen.display().to_string())]).unwrap();
        app.navigate(Location { node, path: "/file/more.txt".into(), fields: BTreeMap::new() });
        let (id, _) = fetched(&mut net).unwrap();
        app.on_fetched(id, Ok(FetchedContent { data: b"more".to_vec(), metadata: None }));
        assert_eq!(std::fs::read(chosen.join("more.txt")).unwrap(), b"more");
        assert!(!app.paths.downloads.join("more.txt").exists());
        assert!(app.update_settings(&[("nomad_files_dir", "/no/such/place/at/all")]).is_err());
        // A page that didn't come: said; one superseded: dropped.
        app.navigate(Location { node, path: "/page/gone.mu".into(), fields: BTreeMap::new() });
        let (id, _) = fetched(&mut net).unwrap();
        app.on_fetched(id + 100, Ok(FetchedContent { data: b"late".to_vec(), metadata: None }));
        app.on_fetched(id, Err("no path".into()));
        assert_eq!(app.browser.tab.error.as_deref(), Some("no path"));
        // A page with a picture: fetched, kept, and asked for once.
        app.navigate(Location { node, path: "/page/pic.mu".into(), fields: BTreeMap::new() });
        let (id, _) = fetched(&mut net).unwrap();
        app.on_fetched(id, Ok(FetchedContent { data: b"`(pic`:/media/a.png)".to_vec(), metadata: None }));
        let (image, path) = fetched(&mut net).unwrap();
        assert_eq!(path, "/media/a.png");
        app.on_fetched(image, Ok(FetchedContent { data: vec![1, 2, 3], metadata: None }));
        app.navigate(Location { node, path: "/page/pic2.mu".into(), fields: BTreeMap::new() });
        let (id, _) = fetched(&mut net).unwrap();
        app.on_fetched(id, Ok(FetchedContent { data: b"`(pic`:/media/b.png)".to_vec(), metadata: None }));
        let (image, _) = fetched(&mut net).unwrap();
        app.on_fetched(image, Err("gone".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tabs_keep_their_pages_and_get_what_they_asked_for() {
        let dir = std::env::temp_dir().join(format!("rettui-browser-tabs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        app.tab = Tab::Browser;
        let node = [0xcd; 16];
        let at = |path: &str| Location { node, path: path.into(), fields: BTreeMap::new() };
        let fetched = |net: &mut tokio::sync::mpsc::UnboundedReceiver<NetCommand>| {
            std::iter::from_fn(|| net.try_recv().ok())
                .filter_map(|c| match c {
                    NetCommand::Fetch { id, path, .. } => Some((id, path)),
                    _ => None,
                })
                .last()
        };
        let page = |data: &str| Ok(FetchedContent { data: data.as_bytes().to_vec(), metadata: None });
        app.navigate(at("/page/index.mu"));
        let (id, _) = fetched(&mut net).unwrap();
        app.on_fetched(id, page("`[About`:/page/about.mu]"));
        // The selected link, in a new tab after this one, which shows it.
        app.browser.focus = BrowserFocus::Page;
        app.browser.tab.selected = Some(0);
        press(&mut app, KeyCode::Char('T'));
        assert_eq!((app.browser_tab_count(), app.browser.current), (2, 1));
        let (about, path) = fetched(&mut net).unwrap();
        assert_eq!(path, "/page/about.mu");
        // Back in the first before it comes: it goes to the second tab,
        // which keeps the first's history out of it.
        press(&mut app, KeyCode::Char('['));
        assert_eq!(app.browser.tab.location, Some(at("/page/index.mu")));
        app.on_fetched(about, page("About"));
        assert_eq!(app.browser.tab.location, Some(at("/page/index.mu")));
        assert_eq!(app.browser.focus, BrowserFocus::Page);
        let tabs = app.browser_tabs();
        assert_eq!(
            (tabs[1].location.clone(), tabs[1].source.as_deref(), tabs[1].history.len()),
            (Some(at("/page/about.mu")), Some("About"), 0)
        );
        // How finding a path goes, for a tab out of sight.
        app.navigate(at("/page/slow.mu"));
        let (slow, _) = fetched(&mut net).unwrap();
        press(&mut app, KeyCode::Char(']'));
        app.on_net(crate::net::NetEvent::FetchProgress { id: slow, text: "finding a path".into() });
        assert_eq!(app.browser_tabs()[0].loading.as_ref().and_then(|p| p.status.as_deref()), Some("finding a path"));
        // Reticulum restarting stops it there too.
        app.browser.restarted();
        assert!(app.browser_tabs()[0].loading.is_none() && app.browser_tabs()[0].error.is_some());
        // Round the end both ways; a click on a tab; nothing to switch to.
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.browser.current, 0);
        press(&mut app, KeyCode::Char('['));
        assert_eq!(app.browser.current, 1);
        app.switch_browser_tab(7);
        assert_eq!(app.browser.current, 1);
        // Without a link selected: the page shown, again; then from the list.
        app.browser.tab.selected = None;
        press(&mut app, KeyCode::Char('T'));
        assert_eq!((app.browser_tab_count(), app.browser.current), (3, 2));
        app.store.saved.push(Bookmark { name: "Index".into(), url: at("/page/index.mu").url() });
        app.browser.focus = BrowserFocus::Pane;
        app.browser.pane = BrowserPane::Saved;
        app.browser.saved_list.select(Some(0));
        // Picked from the list: a new tab (as set by default), or with T the
        // tab shown.
        press(&mut app, KeyCode::Enter);
        assert_eq!((app.browser_tab_count(), app.browser.current), (4, 3));
        app.browser.focus = BrowserFocus::Pane;
        press(&mut app, KeyCode::Char('T'));
        assert_eq!((app.browser_tab_count(), app.browser.current), (4, 3));
        // The other way round when set so.
        app.settings.browser_new_tabs = false;
        app.browser.focus = BrowserFocus::Pane;
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.browser_tab_count(), 4);
        app.browser.focus = BrowserFocus::Pane;
        press(&mut app, KeyCode::Char('T'));
        assert_eq!((app.browser_tab_count(), app.browser.current), (5, 4));
        app.settings.browser_new_tabs = true;
        // An empty tab takes what's picked rather than another opening.
        app.new_browser_tab(None);
        assert_eq!(app.browser_tab_count(), 6);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.browser_tab_count(), 6);
        assert_eq!(app.browser.tab.location, Some(at("/page/index.mu")));
        for _ in 0..2 {
            app.close_browser_tab();
        }
        app.switch_browser_tab(3);
        // Closing: the next one shows, or the one before after the last.
        press(&mut app, KeyCode::Char('W'));
        assert_eq!((app.browser_tab_count(), app.browser.current), (3, 2));
        app.switch_browser_tab(0);
        press(&mut app, KeyCode::Char('W'));
        assert_eq!((app.browser_tab_count(), app.browser.current), (2, 0));
        assert_eq!(app.browser.tab.location, Some(at("/page/about.mu")));
        // So many open: no more.
        for _ in 0..MAX_TABS {
            app.new_browser_tab(None);
        }
        assert_eq!(app.browser_tab_count(), MAX_TABS);
        // Down to one, which closing empties.
        while app.browser_tab_count() > 1 {
            app.close_browser_tab();
        }
        app.navigate(at("/page/index.mu"));
        app.close_browser_tab();
        assert!(app.browser.tab.location.is_none() && app.browser.tab.loading.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partials_from_other_nodes_the_cache_and_addresses_in_messages() {
        let dir = std::env::temp_dir().join(format!("rettui-browser-partials-elsewhere-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        let node = [0xcd; 16];
        let fetches = |net: &mut tokio::sync::mpsc::UnboundedReceiver<NetCommand>| -> Vec<(u64, String)> {
            std::iter::from_fn(|| net.try_recv().ok())
                .filter_map(|c| match c {
                    NetCommand::Fetch { id, path, .. } => Some((id, path)),
                    _ => None,
                })
                .collect()
        };
        // A page with a partial from the same node, and one at an address
        // that can't be read.
        let page = Location { node, path: "/page/parts.mu".into(), fields: BTreeMap::new() };
        app.navigate(page.clone());
        let (id, _) = fetches(&mut net)[0].clone();
        app.on_fetched(id, Ok(FetchedContent { data: b"top\n`{:/page/p.mu`5}\n`{not an address}\nend".to_vec(), metadata: None }));
        let asked = fetches(&mut net);
        let (part, _) = asked.iter().find(|(_, path)| path == "/page/p.mu").cloned().unwrap();
        app.on_fetched(part, Ok(FetchedContent { data: b"`(pic`:/media/in.png)\nfrom the part".to_vec(), metadata: None }));
        assert!(app.browser.tab.partials[0].content.as_deref() == Some("`(pic`:/media/in.png)\nfrom the part"));
        assert!(app.browser.tab.partials[1].content.as_deref().unwrap().contains("can't read"));
        assert!(fetches(&mut net).iter().any(|(_, path)| path == "/media/in.png"));
        // Reloaded: failed this time.
        app.browser.tab.partials[0].loaded_at = Some(Instant::now() - std::time::Duration::from_secs(60));
        app.refresh_partials();
        let again = fetches(&mut net);
        if let Some((part, _)) = again.iter().find(|(_, path)| path == "/page/p.mu") {
            app.on_fetched(*part, Err("gone".into()));
            assert!(app.browser.tab.partials[0].content.as_deref().unwrap().contains("Could not load"));
        }
        // Opened again: from the cache, no request.
        app.navigate(Location { node, path: "/page/other.mu".into(), fields: BTreeMap::new() });
        let (other, _) = fetches(&mut net)[0].clone();
        app.on_fetched(other, Ok(FetchedContent { data: b"other".to_vec(), metadata: None }));
        app.navigate(page.clone());
        let _ = fetches(&mut net);
        app.navigate(Location { node, path: "/page/other.mu".into(), fields: BTreeMap::new() });
        assert!(fetches(&mut net).iter().all(|(_, path)| path != "/page/other.mu"), "{:?}", app.log);
        assert!(app.browser.tab.cached_age.is_some());
        // Identifying to the node reloads it; the source, with and without
        // a page.
        app.tab = Tab::Browser;
        app.browser.focus = BrowserFocus::Page;
        press(&mut app, KeyCode::Char('I'));
        assert!(app.identifies_to(node));
        press(&mut app, KeyCode::Char('I'));
        app.browser.tab.source = None;
        press(&mut app, KeyCode::Char('u'));

        // Addresses in messages, each kind.
        for (address, said) in [
            ("lxmf@zz", "Bad LXMF address"),
            ("lxma://nonsense", ""),
            ("not anything", "Cannot open"),
            ("rrc://0101010101010101010101010101010101", ""),
        ] {
            app.open_address(address);
            if !said.is_empty() {
                assert!(app.log.iter().any(|l| l.contains(said)), "{address}: {:?}", app.log);
            }
            app.prompt = None;
        }
        app.open_address(&format!("lxmf@{}", "ab".repeat(16)));
        assert_eq!(app.active_conversation.as_deref(), Some("ab".repeat(16).as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
