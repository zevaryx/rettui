//! Application state and input handling.
//!
//! [`App`] holds everything the UI shows. Each tab's state changes live in
//! their own module, mirroring `ui/`:
//!
//! - [`messages`]: LXMF conversations, compose and attachments.
//! - [`channels`]: RRC hubs and rooms.
//! - [`network`]: announced peers, nodes and propagation nodes.
//! - [`browser`]: NomadNet pages, saved pages and the page cache.
//! - [`node`]: hosting a NomadNet node and editing its pages.
//! - [`reticulum`]: editing the Reticulum config file.
//! - [`input`]: routing keys and mouse events to the above.

mod browser;
pub mod channels;
pub(crate) mod files;
mod input;
mod messages;
mod network;
pub mod node;
pub mod reticulum;
mod settings;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{Paths, Settings};
use crate::lxmf::{Delivered, DeliveryMode};
use crate::net::{Hash, NetCommand, NetEvent, parse_hash};
use crate::nomad::cache::Cache;
use crate::store::{MessageState, Peer, Store};
use crate::term::clipboard::Clipboard;
use crate::term::images::{DecodeFor, Decoded, Graphics, Picture, decode_in_background};
use crate::term::input::TextInput;

pub use browser::{Browser, BrowserFocus, BrowserPane, Location, resolve_url};
pub use network::{NetFilter, NetSearch, match_mask};

const LOG_LINES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Messages,
    Channels,
    Network,
    Browser,
    Node,
    Status,
    Reticulum,
}

impl Tab {
    pub const ALL: [Tab; 7] = [
        Tab::Messages,
        Tab::Channels,
        Tab::Network,
        Tab::Browser,
        Tab::Node,
        Tab::Status,
        Tab::Reticulum,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Messages => "Messages",
            Tab::Channels => "Channels",
            Tab::Network => "Network",
            Tab::Browser => "Browser",
            Tab::Node => "Node",
            Tab::Status => "Status",
            Tab::Reticulum => "Reticulum",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    NewConversation,
    GoTo,
    EditField(usize),
    /// A `settings.json` entry, by key.
    EditSetting(&'static str),
    DisplayName,
    Attach,
    /// Add an RRC hub by address or `rrc://` link.
    AddHub,
    /// Opening a new hub from a page link reveals our identity: confirm.
    ConfirmHub {
        hash: Hash,
        aspect: String,
        room: Option<String>,
    },
    ConfirmRemoveHub(Hash),
    ConfirmSplit {
        hub: Hash,
        room: String,
        parts: Vec<String>,
    },
    /// Hosted node pages.
    NewPage,
    RenamePage(String),
    ConfirmDeletePage(String),
    /// Open this page, dropping unsaved changes to the open one.
    ConfirmDiscardPage(String),
    /// Reticulum config: interfaces, an option's value, leaving the text.
    NewInterface,
    RenameInterface(String),
    ConfirmDeleteInterface(String),
    RnsOption(crate::reticulum::Section, String),
    ConfirmDiscardRns,
    /// Restart the Reticulum stack.
    ConfirmRestartRns,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub title: String,
    pub input: TextInput,
}

pub enum NetState {
    Starting,
    Online,
    Failed(String),
}

/// Screen areas from the last frame, used to route mouse events.
#[derive(Default)]
pub struct Regions {
    pub tabs: Vec<(Rect, Tab)>,
    /// Browser pane sub-tabs and list.
    pub browser_tabs: Vec<(Rect, BrowserPane)>,
    pub browser_list: Rect,
    pub conversations: Rect,
    pub history: Rect,
    /// Attachment shown on each visible history row.
    pub history_rows: Vec<Option<PathBuf>>,
    pub compose: Rect,
    pub peers: Rect,
    /// Network tab search box.
    pub net_search: Rect,
    pub address: Rect,
    pub page: Rect,
    /// Clickable page cells: (row, first column, end column, item).
    pub page_hits: Vec<(usize, usize, usize, usize)>,
    /// Plain text of every laid-out page row, as drawn (for copying).
    pub page_text: Vec<String>,
    /// The page / source toggle in the page's title bar.
    pub source_button: Rect,
    /// Status tab: the settings list.
    pub settings: Rect,
    /// Node tab: page list and editor text area.
    pub node_pages: Rect,
    pub node_editor: Rect,
    pub node_preview: Rect,
    /// Reticulum tab: sections, options, the text editor and the picker.
    pub rns_sections: Rect,
    pub rns_options: Rect,
    pub rns_editor: Rect,
    pub rns_picker: Rect,
    /// Channels tab: hub/room list, message history, input, and the
    /// clickable public rooms shown for a hub.
    pub channel_list: Rect,
    pub channel_history: Rect,
    pub channel_input: Rect,
    pub channel_rooms: Vec<(Rect, String)>,
    /// Clickable users: names in the history and the members list.
    pub channel_users: Vec<(Rect, Vec<u8>)>,
    /// The user menu or member picker's list.
    pub channel_popup: Rect,
}

/// How a footer notice reads: done, a hint that something can't be done
/// now, or a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Done,
    Warn,
    Error,
}

pub struct Notice {
    pub text: String,
    pub kind: NoticeKind,
    pub at: Instant,
}

pub enum SyncState {
    Idle,
    Running(Instant),
    Done(chrono::DateTime<chrono::Local>, Result<usize, String>),
}

pub struct App {
    pub tab: Tab,
    /// Settings in effect (including session-only values such as
    /// `--rns-config`).
    pub settings: Settings,
    /// Settings as saved in `settings.json`, shown by the settings editor.
    pub settings_file: Settings,
    /// Selected row of the settings editor (Status tab).
    pub settings_list: ListState,
    pub paths: Paths,
    pub store: Store,
    pub(crate) store_dirty: bool,
    net: UnboundedSender<NetCommand>,
    /// This client's identity hash (RRC sender id).
    pub identity_hash: Hash,
    pub channels: crate::app::channels::Channels,
    /// The hosted node and the page editor.
    pub node: node::Node,
    /// The Reticulum config editor.
    pub rns: reticulum::RnsState,
    pub net_state: NetState,
    pub lxmf_hash: Option<Hash>,
    pub interfaces: Vec<crate::net::InterfaceInfo>,
    pub log: VecDeque<String>,
    pub should_quit: bool,
    pub sync: SyncState,

    pub conversations: ListState,
    pub active_conversation: Option<String>,
    pub compose: TextInput,
    pub composing: bool,
    pub message_scroll: usize,
    pub attachments: Vec<PathBuf>,
    pub delivery_mode: DeliveryMode,
    /// Decoded image attachments by file path (`None` if undecodable).
    pictures: HashMap<PathBuf, Option<Picture>>,
    /// Images being decoded in the background, and where results arrive
    /// (the event loop takes the receiver with [`App::take_decoded`]).
    decoding: std::collections::HashSet<PathBuf>,
    decoded_tx: tokio::sync::mpsc::UnboundedSender<Decoded>,
    decoded_rx: Option<tokio::sync::mpsc::UnboundedReceiver<Decoded>>,

    pub peers: ListState,
    pub net_filter: NetFilter,
    pub net_search: NetSearch,

    pub browser: Browser,
    pub prompt: Option<Prompt>,
    pub regions: Regions,
    /// Kitty graphics support, if the terminal has it.
    pub graphics: Option<Graphics>,
    clipboard: Clipboard,
    /// Short confirmation shown in the footer (e.g. "Copied address").
    pub notice: Option<Notice>,
    cache: Cache,
    /// Time and cell of the previous left click, for double-click detection.
    last_click: Option<(Instant, Position)>,
    next_request: u64,
    /// A Reticulum restart the event loop should carry out.
    restart_pending: bool,
    /// Hubs to reconnect once Reticulum is back after a restart.
    rejoin_hubs: Vec<Hash>,
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

impl App {
    pub fn new(
        settings: Settings,
        paths: Paths,
        store: Store,
        net: UnboundedSender<NetCommand>,
        graphics: Option<Graphics>,
        identity_hash: Hash,
    ) -> Self {
        let channels = crate::app::channels::Channels::load(&store.rrc_hubs, &paths.rrc_history);
        // The node uses this client's identity, so its address is known now.
        let node_hash = rns_identity::destination::Destination::hash_from_name_and_identity(
            nomad_core::NOMAD_NODE_ASPECT,
            Some(&identity_hash),
        );
        let node = node::Node::new(node_hash, settings.node_enabled);
        let cache = Cache::new(
            paths.cache.clone(),
            std::time::Duration::from_secs(settings.cache_hours * 3600),
        );
        let settings_file = Settings::load(&paths.settings).unwrap_or_else(|_| settings.clone());
        let (decoded_tx, decoded_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = Self {
            tab: Tab::Messages,
            settings,
            settings_file,
            settings_list: ListState::default().with_selected(Some(0)),
            paths,
            store,
            store_dirty: false,
            net,
            identity_hash,
            channels,
            node,
            rns: reticulum::RnsState::default(),
            net_state: NetState::Starting,
            lxmf_hash: None,
            interfaces: Vec::new(),
            log: VecDeque::new(),
            should_quit: false,
            sync: SyncState::Idle,
            conversations: ListState::default(),
            active_conversation: None,
            compose: TextInput::default(),
            composing: false,
            message_scroll: 0,
            attachments: Vec::new(),
            delivery_mode: DeliveryMode::Auto,
            pictures: HashMap::new(),
            decoding: std::collections::HashSet::new(),
            decoded_tx,
            decoded_rx: Some(decoded_rx),
            peers: ListState::default(),
            net_filter: NetFilter::All,
            net_search: NetSearch::default(),
            browser: Browser::default(),
            prompt: None,
            regions: Regions::default(),
            graphics,
            clipboard: Clipboard::new(),
            notice: None,
            cache,
            last_click: None,
            next_request: 1,
            restart_pending: false,
            rejoin_hubs: Vec::new(),
        };
        if !app.store.conversations.is_empty() {
            app.conversations.select(Some(0));
            app.sync_active_conversation();
        }
        app
    }

    pub fn log(&mut self, line: impl Into<String>) {
        let stamp = chrono::Local::now().format("%H:%M:%S");
        self.log.push_back(format!("{stamp}  {}", line.into()));
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }

    pub fn save_if_dirty(&mut self) {
        self.save_rrc_history();
        if self.store_dirty {
            if let Err(e) = self.store.save(&self.paths.store) {
                self.log(format!("Could not save store: {e}"));
            }
            self.store_dirty = false;
        }
    }

    /// Save a new display name and announce it.
    pub fn set_display_name(&mut self, name: String) -> Result<(), String> {
        self.update_settings(&[("display_name", &name)]).map(|_| ())
    }

    pub(crate) fn send(&mut self, command: NetCommand) {
        if self.net.send(command).is_err() {
            self.log("Network task is not running");
        }
    }

    fn request_id(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }

    pub fn propagation_node(&self) -> Option<Hash> {
        self.settings.propagation_node.as_deref().and_then(parse_hash)
    }

    fn show(&mut self, text: String, kind: NoticeKind) {
        self.notice = Some(Notice {
            text,
            kind,
            at: Instant::now(),
        });
    }

    /// Something done, in the log and the footer.
    pub(crate) fn notify(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.log(text.clone());
        self.show(text, NoticeKind::Done);
    }

    /// Something done, in the footer only (the log has the details).
    pub(crate) fn confirm(&mut self, text: impl Into<String>) {
        self.show(text.into(), NoticeKind::Done);
    }

    /// Something that can't be done right now, or done with a caveat.
    pub(crate) fn warn(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.log(text.clone());
        self.show(text, NoticeKind::Warn);
    }

    /// Something that failed.
    pub(crate) fn fail(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.log(text.clone());
        self.show(text, NoticeKind::Error);
    }

    fn copy(&mut self, text: &str, what: &str) {
        if text.is_empty() {
            self.warn(format!("Nothing to copy ({what} is empty)"));
            return;
        }
        self.clipboard.copy(text);
        let chars = text.chars().count();
        self.notify(format!("Copied {what} ({chars} character{})", if chars == 1 { "" } else { "s" }));
    }

    /// Text pasted from the terminal (bracketed paste) or the clipboard,
    /// inserted into whatever is being edited.
    pub fn on_paste(&mut self, text: &str) {
        if let Some(prompt) = &mut self.prompt {
            prompt.input.insert_str(text);
        } else if self.composing && self.tab == Tab::Messages {
            self.compose.insert_str(text);
        } else if self.channels.typing && self.tab == Tab::Channels {
            self.channels.input.insert_str(text);
        } else if self.node.editing && self.tab == Tab::Node {
            self.node_paste(text);
        } else if self.rns.editor.is_some() && self.tab == Tab::Reticulum {
            self.rns_paste(text);
        } else if self.net_search.typing && self.tab == Tab::Network {
            self.paste_network_search(text);
        } else if let Some(field) = self.selected_text_field() {
            // Paste into the selected page field by opening its editor.
            let page = self.browser.page.as_ref().expect("field implies a page");
            let current = &page.fields[field];
            let mut input = TextInput::with_text(&current.value);
            input.insert_str(text);
            self.prompt = Some(Prompt {
                kind: PromptKind::EditField(field),
                title: format!("Field: {}", current.name),
                input,
            });
        } else {
            self.warn("Nothing to paste into: start writing a message or select a field");
        }
    }

    fn paste_from_clipboard(&mut self) {
        match self.clipboard.paste() {
            Some(text) => self.on_paste(&text),
            None => self.warn("Clipboard unavailable: use your terminal's paste shortcut"),
        }
    }

    pub fn on_net(&mut self, event: NetEvent) {
        match event {
            NetEvent::Started { lxmf_hash } => {
                self.net_state = NetState::Online;
                self.lxmf_hash = Some(lxmf_hash);
                self.log(format!("Reticulum ready; LXMF address {}", hex::encode(lxmf_hash)));
                self.start_channels();
                // Hubs that were connected before a restart, auto or not.
                for hash in std::mem::take(&mut self.rejoin_hubs) {
                    if let Some(index) = self.channels.hub_index(hash) {
                        self.connect_hub(index);
                    }
                }
            }
            NetEvent::Rrc { hub, event } => self.on_rrc(hub, event),
            NetEvent::Host(event) => self.on_host(event),
            NetEvent::StartFailed(e) => {
                self.log(format!("Network failed: {e}"));
                self.net_state = NetState::Failed(e);
            }
            NetEvent::Announce { kind, hash, name, hops } => {
                let key = hex::encode(hash);
                let peer = self.store.peers.entry(key).or_insert(Peer {
                    kind,
                    name: None,
                    hops,
                    last_seen: 0,
                });
                peer.kind = kind;
                peer.hops = hops;
                peer.last_seen = now() as i64;
                if name.is_some() {
                    peer.name = name;
                }
                self.store_dirty = true;
            }
            NetEvent::Announced => self.log("Announced LXMF destination"),
            NetEvent::Message(message) => self.on_message(message),
            NetEvent::Delivery { id, result } => {
                let local = format!("local-{id}");
                if let Some(message) = self.store.find_message_mut(&local) {
                    message.state = match &result {
                        Ok(Delivered::Direct) => MessageState::Delivered,
                        Ok(Delivered::Propagated) => MessageState::Propagated,
                        Err(e) => MessageState::Failed(e.clone()),
                    };
                    self.store_dirty = true;
                }
                match result {
                    Ok(Delivered::Propagated) => self.log("Message handed to propagation node"),
                    Ok(Delivered::Direct) => {}
                    Err(e) => self.log(format!("Delivery failed: {e}")),
                }
            }
            NetEvent::Fetched { id, result } => self.on_fetched(id, result),
            NetEvent::SyncStarted => self.sync = SyncState::Running(Instant::now()),
            NetEvent::Synced(result) => {
                match &result {
                    Ok(0) => {}
                    Ok(n) => self.log(format!("Downloaded {n} message(s) from propagation node")),
                    Err(e) => self.log(format!("Sync failed: {e}")),
                }
                self.sync = SyncState::Done(chrono::Local::now(), result);
            }
            NetEvent::Interfaces(interfaces) => self.interfaces = interfaces,
            NetEvent::Log(line) => self.log(line),
            NetEvent::Stopped => {}
        }
    }

    fn submit_prompt(&mut self, prompt: Prompt) {
        let text = prompt.input.text().trim().to_string();
        match prompt.kind {
            PromptKind::NewConversation => match parse_hash(&text) {
                Some(hash) => self.open_conversation(hex::encode(hash)),
                None => self.warn("An LXMF address is 32 hex characters"),
            },
            PromptKind::GoTo => match self.resolve(&text) {
                Some(location) => self.navigate(location),
                None => self.warn(format!("Not a NomadNet address: {text}")),
            },
            PromptKind::EditSetting(key) => self.update_setting(key, prompt.input.text()),
            PromptKind::EditField(f) => {
                if let Some(page) = &mut self.browser.page
                    && let Some(field) = page.fields.get_mut(f)
                {
                    field.value = prompt.input.text().to_string();
                }
            }
            PromptKind::Attach if !text.is_empty() => self.add_attachment(&text),
            PromptKind::Attach => self.composing = true,
            PromptKind::DisplayName if !text.is_empty() => {
                if let Err(e) = self.set_display_name(text) {
                    self.fail(e);
                }
            }
            PromptKind::DisplayName => {}
            PromptKind::AddHub => match crate::rrc::parse_link(&text) {
                Some((hash, aspect, room)) => self.add_hub(hash, &aspect, room),
                None if text.is_empty() => {}
                None => self.warn("A hub address is 32 hex characters (or an rrc:// link)"),
            },
            PromptKind::ConfirmHub { hash, aspect, room } => {
                if text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes") {
                    self.add_hub(hash, &aspect, room);
                }
            }
            PromptKind::ConfirmRemoveHub(hash) => {
                if text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes") {
                    self.remove_hub(hash);
                }
            }
            PromptKind::ConfirmSplit { hub, room, parts } => {
                if text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes") {
                    self.confirm_split(hub, &room, &parts);
                }
            }
            PromptKind::ConfirmDiscardPage(path) => {
                if text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes") {
                    self.discard_and_open(&path);
                }
            }
            PromptKind::ConfirmRestartRns => {
                if text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes") {
                    self.request_rns_restart();
                }
            }
            kind @ (PromptKind::NewInterface
            | PromptKind::RenameInterface(_)
            | PromptKind::ConfirmDeleteInterface(_)
            | PromptKind::RnsOption(..)
            | PromptKind::ConfirmDiscardRns) => {
                let text = prompt.input.text().to_string();
                self.submit_rns_prompt(kind, &text);
            }
            kind @ (PromptKind::NewPage | PromptKind::RenamePage(_) | PromptKind::ConfirmDeletePage(_)) => {
                if !text.is_empty() {
                    self.submit_page_prompt(kind, &text);
                }
            }
        }
    }

    fn open_prompt(&mut self, kind: PromptKind, title: &str, initial: &str) {
        self.prompt = Some(Prompt {
            kind,
            title: title.to_string(),
            input: TextInput::with_text(initial),
        });
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.tab = tab;
        match tab {
            Tab::Messages => self.sync_active_conversation(),
            Tab::Channels => self.mark_channel_read(),
            Tab::Node => self.refresh_pages(),
            // The file may have been edited elsewhere.
            Tab::Reticulum if self.rns.editor.is_none() => self.rns_reload(),
            // Show the settings file as it is now (it may be edited by hand).
            Tab::Status => {
                if let Ok(saved) = self.saved_settings() {
                    self.settings_file = saved;
                }
            }
            _ => {}
        }
    }

    /// Restart the Reticulum stack (to apply config changes, or recover): the
    /// event loop stops the network actor and starts a new one with the same
    /// identity. Links and transfers in progress end; hubs reconnect and the
    /// hosted node starts again once it is back.
    pub fn request_rns_restart(&mut self) {
        if self.restart_pending {
            return;
        }
        self.restart_pending = true;
        self.log("Restarting Reticulum…");
        self.net_state = NetState::Starting;
        self.interfaces.clear();
        if matches!(self.sync, SyncState::Running(_)) {
            self.sync = SyncState::Idle;
        }
        self.rejoin_hubs = self.channels_before_restart();
        if self.settings.node_enabled {
            self.node.status = node::NodeStatus::Starting;
        }
        self.node.stats = None;
        if self.browser.loading.take().is_some() {
            self.browser.error = Some("Reticulum restarted while loading; load the page again".into());
        }
        self.browser.media_requests.clear();
        // Deliveries in progress belong to the old stack.
        for conversation in self.store.conversations.values_mut() {
            for message in &mut conversation.messages {
                if message.state == MessageState::Sending {
                    message.state = MessageState::Failed("interrupted by a Reticulum restart; send it again".into());
                    self.store_dirty = true;
                }
            }
        }
        self.confirm("Restarting Reticulum…");
    }

    /// Whether a restart was asked for (the event loop carries it out).
    pub fn take_rns_restart(&mut self) -> bool {
        std::mem::take(&mut self.restart_pending)
    }

    /// Ask before restarting Reticulum (Ctrl-R in the Status and Reticulum tabs).
    pub(super) fn open_restart_prompt(&mut self) {
        let title = if self.uses_external_shared_instance() {
            "Reconnect to the shared instance? (Its own program applies interface changes.) Type y"
        } else {
            "Restart Reticulum? Links and transfers stop, hubs reconnect. Type y"
        };
        self.open_prompt(PromptKind::ConfirmRestartRns, title, "");
    }

    /// The new network actor after a restart.
    pub fn set_network(&mut self, net: UnboundedSender<NetCommand>) {
        self.net = net;
    }

    /// rettui is a client of another program's shared instance (rnsd,
    /// NomadNet…): interfaces in the config belong to that program, and a
    /// restart here only reconnects to it.
    pub fn uses_external_shared_instance(&self) -> bool {
        self.interfaces.iter().any(|i| i.name.starts_with("Shared Instance["))
    }

    /// Where background image decodes report back; the event loop owns it.
    pub fn take_decoded(&mut self) -> tokio::sync::mpsc::UnboundedReceiver<Decoded> {
        self.decoded_rx.take().expect("the decode receiver is taken once")
    }

    fn decode_later(&self, what: DecodeFor, bytes: Option<Vec<u8>>) {
        decode_in_background(what, bytes, self.decoded_tx.clone());
    }

    /// A background decode finished.
    pub fn on_decoded(&mut self, decoded: Decoded) {
        match decoded.what {
            DecodeFor::File(path) => {
                self.decoding.remove(&path);
                self.pictures.insert(path, decoded.picture);
            }
            DecodeFor::Page(url) => self.on_page_image(url, decoded.picture),
        }
    }

    /// Periodic work (called about twice a second).
    pub fn on_tick(&mut self) {
        self.channels_tick();
    }
}
