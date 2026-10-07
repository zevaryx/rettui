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
//! - [`notify`]: notifications for what arrives while the user looks
//!   elsewhere.

mod browser;
pub mod contacts;
pub mod emoji;
pub mod format;
pub mod channels;
pub mod guide;
pub(crate) mod files;
pub mod identity;
mod input;
mod messages;
pub mod network;
pub mod notify;
pub mod paths;
pub mod reach;
mod saver;
pub mod node;
pub mod reticulum;
pub mod search;
pub mod shrink;
mod settings;
pub mod traffic;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{Paths, Settings};
use crate::lxmf::{Delivered, DeliveryMode};
use crate::net::{Hash, NetCommand, NetEvent, PeerKind, parse_hash};
use crate::nomad::cache::Cache;
use crate::store::{MessageState, Peer, Store};
use crate::term::clipboard::Clipboard;
use crate::term::images::{DecodeFor, Decoded, Graphics, Picture, decode_in_background};
use crate::term::input::TextInput;

pub use browser::{Browser, BrowserFocus, BrowserPane, Location, resolve_url};

/// An app for tests: its data in `dir` (settings saved there too), and no
/// network.
#[cfg(test)]
pub(crate) fn test_app(dir: &std::path::Path, settings: Settings, store: Store) -> App {
    test_app_with_net(dir, settings, store).0
}

/// [`test_app`], with what it tells the network.
#[cfg(test)]
pub(crate) fn test_app_with_net(
    dir: &std::path::Path,
    settings: Settings,
    store: Store,
) -> (App, tokio::sync::mpsc::UnboundedReceiver<NetCommand>) {
    let paths = Paths::new(Some(dir.to_path_buf())).unwrap();
    settings.save(&paths.settings).unwrap();
    let (net, commands) = tokio::sync::mpsc::unbounded_channel();
    (App::new(settings, paths, store, net, None, [0; 16]), commands)
}
pub use network::{NetFilter, NetSearch, match_mask};

const LOG_LINES: usize = 500;

/// Scrolled as far back as it goes (Home): drawing stops at the top. Far
/// from `usize::MAX`, since drawing adds the view's height to it.
pub(crate) const SCROLL_TOP: usize = usize::MAX / 4;

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
    /// An address to find a path to (see [`paths`]).
    FindPath,
    EditField(usize),
    /// A `settings.json` entry, by key.
    EditSetting(&'static str),
    DisplayName,
    Attach,
    /// Read in a paper message (its `lxm://` link).
    ReadPaper,
    /// Your own name for a contact (by address), and notes about them.
    ContactName(String),
    ContactNotes(String),
    /// The name chosen in the getting-started guide.
    GuideName,
    /// An identity file from another program, to use from the next start;
    /// then, which (its address shown), confirmed.
    ImportIdentity,
    ConfirmImportIdentity(PathBuf),
    /// Where to save a copy of the identity.
    BackUpIdentity,
    ConfirmDeleteMessage { key: String, id: String },
    ConfirmDeleteConversation(String),
    ConfirmBlock(String),
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
    /// The page editor's formatting that needs an answer (a colour, an
    /// address, a field name).
    Format(format::Action),
}

/// What a QR code over the tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrKind {
    /// A paper message.
    Paper,
    /// Your address and key (an `lxma://` link), to be added as a contact.
    Address,
}

/// A paper message, or your address, shown as a QR code over the tab
/// (terminal UI).
pub struct PaperView {
    pub kind: QrKind,
    pub link: String,
    /// Made once, not every frame (the largest takes a few milliseconds).
    pub qr: Result<crate::lxmf::paper::Qr, String>,
}

impl PaperView {
    pub(crate) fn new(link: String) -> Self {
        let qr = crate::lxmf::paper::Qr::new(&link);
        Self { kind: QrKind::Paper, link, qr }
    }

    pub(crate) fn address(link: String) -> Self {
        Self { kind: QrKind::Address, ..Self::new(link) }
    }
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
    /// The getting-started guide's rows.
    pub guide_rows: Vec<(Rect, guide::GuideRow)>,
    /// The version beside the name, top left (opens the project page).
    pub version: Rect,
    /// Browser pane sub-tabs and list.
    pub browser_tabs: Vec<(Rect, BrowserPane)>,
    pub browser_search: Rect,
    /// The message search's results, and the first one shown.
    pub message_search: Rect,
    pub message_search_first: usize,
    pub message_search_box: Rect,
    pub browser_list: Rect,
    pub conversations: Rect,
    pub history: Rect,
    /// What each visible history row opens when clicked.
    pub history_rows: Vec<Option<HistoryHit>>,
    /// The picked message's buttons, where they are.
    pub history_buttons: Vec<(Rect, HistoryHit)>,
    /// The contact card, and its buttons.
    pub card: Rect,
    pub card_buttons: Vec<(Rect, contacts::CardAction)>,
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
    /// The page editor's formatting ribbon: each button and its action.
    pub node_ribbon: Vec<(Rect, format::Action)>,
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
    /// Names in the list `@` opens while typing.
    pub channel_mentions: Vec<(Rect, String)>,
    /// The emoji picker (or the `:name` list) and what's in it.
    pub emoji_popup: Rect,
    pub emoji_hits: Vec<(Rect, emoji::EmojiHit)>,
}

/// What a click on a history row does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryHit {
    /// Open an attachment.
    File(PathBuf),
    /// Pick the message (its index), or put it down: its first row.
    Pick(usize),
    /// Show the message a reply answers (its index): the reply's quote.
    Original(usize),
    /// Open a link (a location, on a map).
    Link(String),
    /// Do something with a message (its index): the picked message's
    /// buttons.
    Action(usize, MessageAction),
}

/// What can be done with a picked message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageAction {
    Reply,
    React,
    Copy,
    /// Send a message that failed again.
    Retry,
    Delete,
}

impl MessageAction {
    /// The button's label, and its key.
    pub fn label(self) -> (&'static str, char) {
        match self {
            MessageAction::Reply => ("↩ Reply", 'r'),
            MessageAction::React => ("🙂 React", 'e'),
            MessageAction::Copy => ("⧉ Copy", 'y'),
            MessageAction::Retry => ("↻ Retry", 't'),
            MessageAction::Delete => ("✕ Delete", 'x'),
        }
    }
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
    /// The peers heard changed (they're saved apart from the store).
    pub(crate) peers_dirty: bool,
    net: UnboundedSender<NetCommand>,
    /// This client's identity hash (RRC sender id).
    pub identity_hash: Hash,
    pub channels: crate::app::channels::Channels,
    /// The hosted node and the page editor.
    pub node: node::Node,
    /// The propagation node hosted here.
    pub pn: node::PnHost,
    /// The Reticulum config editor.
    pub rns: reticulum::RnsState,
    pub net_state: NetState,
    pub lxmf_hash: Option<Hash>,
    /// This client's identity's public key (known once Reticulum is up).
    pub public_key: Option<[u8; 64]>,
    pub interfaces: Vec<crate::net::InterfaceInfo>,
    /// Bytes in and out over all interfaces, and their rates.
    pub traffic: traffic::Traffic,
    pub log: VecDeque<String>,
    pub should_quit: bool,
    /// Repaint the whole screen at the next draw (Ctrl-L).
    pub full_redraw: bool,
    /// The list of keys is open (`?`, or F1).
    pub keys_help: bool,
    /// What the screen was showing at the last draw (see [`App::view_key`]).
    shown_view: u64,
    pub sync: SyncState,

    pub conversations: ListState,
    pub active_conversation: Option<String>,
    pub compose: TextInput,
    pub composing: bool,
    /// The message being replied to (its id), in the open conversation.
    pub reply: Option<String>,
    /// The message picked in the open conversation (its id), to do
    /// something with it (`m`, or a click on it).
    pub picked: Option<String>,
    /// The message the emoji picker reacts to (its id), in the open
    /// conversation.
    pub reacting: Option<String>,
    /// The contact card shown over the Messages tab (an address).
    pub contact_card: Option<String>,
    /// The getting-started guide, when it's open (terminal UI).
    pub guide: Option<guide::Guide>,
    /// The propagation node picked automatically, and why the last try
    /// didn't pick one (said once).
    pub auto_pick: Option<crate::net::autopn::Pick>,
    auto_error: Option<String>,
    /// Pings to contacts (by address): waiting, or how they went.
    pub pings: HashMap<String, contacts::PingState>,
    /// Paths looked for on request, and probes, by address (see
    /// [`paths`]).
    pub path_lookups: HashMap<String, paths::PathLookup>,
    pub probes: HashMap<String, paths::ProbeState>,
    /// The contacts last told to the network actor (trusted, and spared the
    /// stamp), so it's told only of changes.
    policy_sent: Option<(Vec<Hash>, Vec<Hash>)>,
    /// A message (its index) to bring into view at the next draw.
    pub scroll_to: Option<usize>,
    /// The message search (`/` in Messages), while open.
    pub message_search: Option<search::MessageSearch>,
    /// The emoji picker, over the input being written in.
    pub emoji: Option<emoji::EmojiPicker>,
    /// The `:name` list while typing one.
    pub shortcode: emoji::Shortcode,
    pub message_scroll: usize,
    pub attachments: Vec<PathBuf>,
    /// What's written and attached in the conversations not open.
    drafts: HashMap<String, (TextInput, Vec<PathBuf>, Option<String>)>,
    pub delivery_mode: DeliveryMode,
    /// Failed messages sent again on their own when their recipient
    /// announced, and when (see [`App::resend_on_announce`]).
    auto_resent: HashMap<String, Instant>,
    /// Decoded image attachments by file path (`None` if undecodable).
    pictures: HashMap<PathBuf, Option<Picture>>,
    /// Images being decoded in the background, and where results arrive
    /// (the event loop takes the receiver with [`App::take_decoded`]).
    decoding: std::collections::HashSet<PathBuf>,
    decoded_tx: tokio::sync::mpsc::UnboundedSender<Decoded>,
    decoded_rx: Option<tokio::sync::mpsc::UnboundedReceiver<Decoded>>,
    /// Writes the store and chat history in the background.
    saver: saver::Saver,

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
    /// The entry points the guide added (their interfaces' names), and
    /// when: said as each connects (see [`App::watch_connection`]).
    connect_watch: Option<(Vec<String>, Instant)>,
    /// Whether the guide's entry points answer, tried as it opens.
    pub(crate) entry_reach: reach::Reachability,
    /// The LXMF address of an identity chosen to use from the next start
    /// (see [`identity`]).
    pub identity_pending: Option<Hash>,
    /// This client announced since rettui started.
    pub announced: bool,
    /// What interface discovery does was said, since Reticulum started.
    discovery_noted: bool,
    /// Hubs to reconnect once Reticulum is back after a restart.
    rejoin_hubs: Vec<Hash>,
    /// Notifications waiting to be shown (see [`App::take_notifications`]).
    notifications: Vec<notify::Notification>,
    /// What was read since last asked, by notification tag (the web UI
    /// closes their notifications in every browser).
    reads: Vec<String>,
    /// A paper message shown as a QR code (terminal UI).
    pub paper_view: Option<PaperView>,
    /// The conversation a paper message was just read into.
    pub paper_read: Option<String>,
    /// Whether the window has the focus (as far as the terminal says; the
    /// web UI's browsers each know their own).
    pub focused: bool,
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
        let pn_hash = rns_identity::destination::Destination::hash_from_name_and_identity(
            crate::lxmf::PROPAGATION_ASPECT,
            Some(&identity_hash),
        );
        let pn = node::PnHost::new(pn_hash, crate::lxmf::pn::PnConfig::from_settings(&settings, &paths));
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
            store_dirty: store.rewrite_store,
            peers_dirty: store.rewrite_peers,
            store,
            net,
            identity_hash,
            channels,
            node,
            pn,
            rns: reticulum::RnsState::default(),
            net_state: NetState::Starting,
            lxmf_hash: None,
            public_key: None,
            interfaces: Vec::new(),
            traffic: traffic::Traffic::default(),
            log: VecDeque::new(),
            should_quit: false,
            full_redraw: false,
            keys_help: false,
            shown_view: 0,
            sync: SyncState::Idle,
            conversations: ListState::default(),
            active_conversation: None,
            compose: TextInput::default(),
            composing: false,
            reply: None,
            picked: None,
            reacting: None,
            contact_card: None,
            guide: None,
            pings: HashMap::new(),
            path_lookups: HashMap::new(),
            probes: HashMap::new(),
            auto_pick: None,
            auto_error: None,
            policy_sent: None,
            scroll_to: None,
            message_search: None,
            emoji: None,
            shortcode: emoji::Shortcode::default(),
            message_scroll: 0,
            attachments: Vec::new(),
            drafts: HashMap::new(),
            delivery_mode: DeliveryMode::Auto,
            auto_resent: HashMap::new(),
            pictures: HashMap::new(),
            decoding: std::collections::HashSet::new(),
            decoded_tx,
            decoded_rx: Some(decoded_rx),
            saver: saver::Saver::new(),
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
            connect_watch: None,
            entry_reach: reach::Reachability::default(),
            identity_pending: None,
            announced: false,
            discovery_noted: true,
            rejoin_hubs: Vec::new(),
            notifications: Vec::new(),
            reads: Vec::new(),
            paper_view: None,
            paper_read: None,
            focused: true,
        };
        for note in std::mem::take(&mut app.store.migration_notes) {
            app.log(note);
        }
        // Stores of earlier versions (or a lower "Messages kept") may hold
        // more than each conversation keeps.
        app.archive_overflow_now();
        // A new install: help to reach others first.
        if !app.settings.welcomed {
            app.open_guide();
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

    /// Interfaces going offline or coming online get a line in the log (why
    /// one dropped is usually the line before, from the interface itself).
    fn log_interface_changes(&mut self, interfaces: &[crate::net::InterfaceInfo]) {
        let changes: Vec<String> = interfaces
            .iter()
            .filter_map(|now| {
                let before = self.interfaces.iter().find(|i| i.name == now.name)?;
                match (before.online, now.online) {
                    (true, false) => Some(format!("Interface {} went offline", now.name)),
                    (false, true) => Some(format!("Interface {} is online", now.name)),
                    _ => None,
                }
            })
            .collect();
        for line in changes {
            self.log(line);
        }
    }

    /// Queue saves of whatever changed; they are written in the background
    /// (only the snapshot is taken here, a millisecond or two).
    pub fn save_if_dirty(&mut self) {
        self.save_rrc_history();
        // Peers first: a store of an earlier version still holds them until
        // it's written again.
        if self.peers_dirty {
            let peers = self.store.peers.clone();
            let path = crate::store::peers_path(&self.paths.store);
            self.saver.write(path, "peers", move || crate::store::encode_peers(&peers));
            self.peers_dirty = false;
        }
        if self.store_dirty {
            // Only messages change the store's size: archive what's over
            // the limit first (it's queued before the store, so a message
            // is never only in memory).
            let (archived, _) = self.archive_overflow();
            let snapshot = self.store.snapshot();
            self.saver.write(self.paths.store.clone(), "store", move || snapshot.encode());
            self.store_dirty = false;
            if archived > 0 {
                self.keep_within_storage(false);
            }
        }
    }

    /// Save what changed and wait until everything is written, before exiting.
    pub fn finish_saves(&mut self) {
        self.save_if_dirty();
        self.saver.finish();
    }

    /// The window gained or lost the focus (in terminals that say). Coming
    /// back reads the conversation or room on screen: what arrived there
    /// meanwhile counted as unread.
    pub fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
        if focused {
            match self.tab {
                Tab::Messages => self.sync_active_conversation(),
                Tab::Channels => self.mark_channel_read(),
                _ => {}
            }
        }
    }

    /// A page of the message history, or a channel's: its height, less a
    /// line that stays in view.
    pub(crate) fn history_page(&self) -> usize {
        (self.regions.history.height as usize).saturating_sub(1).max(1)
    }

    pub(crate) fn channel_page(&self) -> usize {
        (self.regions.channel_history.height as usize).saturating_sub(1).max(1)
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
        if let Some(picker) = &mut self.emoji {
            picker.search.insert_str(text);
            (picker.pick, picker.top) = (0, 0);
        } else if let Some(prompt) = &mut self.prompt {
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
        } else if self.message_search.is_some() && self.tab == Tab::Messages {
            self.paste_message_search(text);
        } else if self.browser.search.typing && self.tab == Tab::Browser {
            self.paste_browser_search(text);
        } else if let Some(field) = self.selected_text_field() {
            // Paste into the selected page field by opening its editor.
            let page = self.browser.page.as_ref().expect("field implies a page");
            let current = &page.fields[field];
            let editing = if current.rows > 1 { browser::field_to_line(&current.value) } else { current.value.clone() };
            let mut input = TextInput::with_text(&editing);
            input.insert_str(text);
            self.prompt = Some(Prompt {
                kind: PromptKind::EditField(field),
                title: format!("Field: {}", current.name),
                input,
            });
        } else if self.tab == Tab::Messages && text.trim().starts_with("lxm://") {
            if let Err(e) = self.read_paper(text) {
                self.warn(e);
            }
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
            NetEvent::Started { lxmf_hash, public_key } => {
                self.net_state = NetState::Online;
                self.lxmf_hash = Some(lxmf_hash);
                self.public_key = Some(public_key);
                self.log(format!("Reticulum ready; LXMF address {}", hex::encode(lxmf_hash)));
                // Said once its interfaces are known (see note_discovery).
                self.discovery_noted = false;
                // A new network actor knows no contacts yet.
                self.update_policy(true);
                if self.settings.auto_propagation_node {
                    self.apply_auto_propagation();
                }
                self.reapply_blocks();
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
            NetEvent::Pn(event) => self.on_pn(event),
            NetEvent::StartFailed(e) => {
                self.log(format!("Network failed: {e}"));
                self.net_state = NetState::Failed(e);
            }
            NetEvent::Announce { kind, hash, name, hops } => {
                let key = hex::encode(hash);
                if kind == PeerKind::Lxmf {
                    self.resend_on_announce(&key);
                }
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
                self.peers_dirty = true;
            }
            NetEvent::Announced => {
                self.announced = true;
                self.log("Announced LXMF destination");
            }
            NetEvent::Message(message) => self.on_message(*message),
            NetEvent::Delivery { id, result } => {
                // Handing a message to a node picked automatically failed:
                // it may have gone.
                if self.settings.auto_propagation_node && result.as_ref().is_err_and(|e| e.contains("propagation")) {
                    self.send(NetCommand::RecheckPropagation);
                }
                self.on_delivery(id, result);
            }
            NetEvent::Pinged { to, result } => self.on_pinged(to, result),
            NetEvent::Path { to, result } => self.on_path(to, result),
            NetEvent::PathProgress { to, text } => self.on_path_progress(to, text),
            NetEvent::PathForgotten { to, had } => self.on_path_forgotten(to, had),
            NetEvent::Probed { to, result } => self.on_probed(to, result),
            NetEvent::PropagationPicked(result) => self.on_propagation_picked(result),
            NetEvent::Fetched { id, result } => self.on_fetched(id, result),
            NetEvent::FetchProgress { id, text } => {
                if let Some(pending) = self.browser.loading.as_mut().filter(|pending| pending.id == id) {
                    pending.status = Some(text);
                }
            }
            NetEvent::Paper { id, result } => self.on_paper(id, result),
            NetEvent::SyncStarted => self.sync = SyncState::Running(Instant::now()),
            NetEvent::Synced(result) => {
                match &result {
                    Ok(0) => {}
                    Ok(n) => self.log(format!("Downloaded {n} message(s) from propagation node")),
                    Err(e) => {
                        self.log(format!("Sync failed: {e}"));
                        // A node picked automatically may have gone.
                        if self.settings.auto_propagation_node {
                            self.send(NetCommand::RecheckPropagation);
                        }
                    }
                }
                self.sync = SyncState::Done(chrono::Local::now(), result);
            }
            NetEvent::Interfaces(interfaces) => {
                self.traffic.update(&interfaces, Instant::now());
                self.log_interface_changes(&interfaces);
                self.interfaces = interfaces;
                self.note_discovery();
                self.watch_connection();
            }
            NetEvent::Log(line) => self.log(line),
            NetEvent::Stopped => {}
        }
    }

    /// A message (or a reaction) sent, or not.
    fn on_delivery(&mut self, id: u64, result: Result<crate::lxmf::Sent, String>) {
        let local = format!("local-{id}");
        let state = match &result {
            Ok(sent) if sent.delivered == Delivered::Direct => MessageState::Delivered,
            Ok(_) => MessageState::Propagated,
            Err(e) => MessageState::Failed(e.clone()),
        };
        if let Some(message) = self.store.find_message_mut(&local) {
            message.state = state;
            // What replies (and reactions) to it will name it by.
            if let Ok(sent) = &result {
                message.hash = Some(hex::encode(sent.hash));
                // Reactions to it that came before this did.
                for conversation in self.store.conversations.values_mut() {
                    if conversation.messages.iter().any(|m| m.id == local) {
                        conversation.adopt_strays();
                    }
                }
            }
            self.store_dirty = true;
        } else if let Some(reaction) = self.store.find_reaction_mut(&local) {
            reaction.state = state;
            self.store_dirty = true;
            if let Err(e) = &result {
                return self.log(format!("Reaction not sent: {e}"));
            }
            return;
        }
        match result {
            Ok(sent) if sent.delivered == Delivered::Propagated => self.log("Message handed to propagation node"),
            Ok(_) => {}
            Err(e) => self.log(format!("Delivery failed: {e}")),
        }
    }

    fn submit_prompt(&mut self, prompt: Prompt) {
        let text = prompt.input.text().trim().to_string();
        match prompt.kind {
            PromptKind::NewConversation => match self.add_contact(&text) {
                Ok(key) => self.open_conversation(key),
                Err(e) => self.warn(e),
            },
            PromptKind::GoTo => match self.resolve(&text) {
                Some(location) => self.navigate(location),
                None => self.warn(format!("Not a NomadNet address: {text}")),
            },
            PromptKind::FindPath => {
                if let Err(e) = self.find_path_to(&text) {
                    self.warn(e);
                }
            }
            PromptKind::EditSetting(key) => self.update_setting(key, prompt.input.text()),
            PromptKind::EditField(f) => {
                if let Some(page) = &mut self.browser.page
                    && let Some(field) = page.fields.get_mut(f)
                {
                    field.value = if field.rows > 1 {
                        browser::field_from_line(prompt.input.text())
                    } else {
                        prompt.input.text().to_string()
                    };
                }
            }
            // A contact's link (or its QR code) works here too.
            PromptKind::ReadPaper if text.starts_with("lxma://") => match self.add_contact(&text) {
                Ok(key) => self.open_conversation(key),
                Err(e) => self.warn(e),
            },
            PromptKind::ReadPaper if !text.is_empty() => {
                // A link, or a picture of its QR code.
                let link = if text.starts_with("lxm://") {
                    Ok(text)
                } else {
                    let path = files::path_from_input(&text);
                    std::fs::read(&path)
                        .map_err(|e| format!("Not an lxm:// link, and can't read {}: {e}", path.display()))
                        .and_then(|picture| crate::lxmf::paper::scan(&picture))
                };
                let read = link.and_then(|link| match link.starts_with("lxma://") {
                    true => self.add_contact(&link).map(|key| self.open_conversation(key)),
                    false => self.read_paper(&link),
                });
                if let Err(e) = read {
                    self.warn(e);
                }
            }
            PromptKind::ReadPaper => {}
            PromptKind::ContactName(key) => {
                if let Err(e) = self.set_contact_name(&key, &text) {
                    self.warn(e);
                }
            }
            PromptKind::GuideName => {
                if let Some(guide) = &mut self.guide {
                    guide.choices.name = text.to_string();
                }
            }
            kind @ (PromptKind::ImportIdentity | PromptKind::ConfirmImportIdentity(_) | PromptKind::BackUpIdentity) => {
                self.submit_identity_prompt(kind, &text)
            }
            PromptKind::ContactNotes(key) => {
                // Lines were shown as ↵ to edit on one line.
                let notes = prompt.input.text().replace(" ↵ ", "\n").replace('↵', "\n");
                if let Err(e) = self.set_contact_notes(&key, &notes) {
                    self.warn(e);
                }
            }
            PromptKind::ConfirmDeleteMessage { key, id } => {
                if text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes") {
                    match self.delete_message(&key, &id) {
                        Ok(()) => self.confirm("Deleted the message"),
                        Err(e) => self.warn(e),
                    }
                }
            }
            PromptKind::ConfirmBlock(key) => {
                if text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes") {
                    let name = self.store.display_name(&key);
                    match self.block_contact(&key) {
                        Ok(()) => {
                            self.contact_card = None;
                            self.notify(format!("Blocked {name}"));
                        }
                        Err(e) => self.warn(e),
                    }
                }
            }
            PromptKind::ConfirmDeleteConversation(key) => {
                if text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes") {
                    let name = self.store.display_name(&key);
                    if self.delete_conversation(&key) {
                        self.contact_card = None;
                        self.notify(format!("Deleted the conversation with {name}"));
                    }
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
            PromptKind::Format(action) => self.node_format_answer(action, &text),
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
        // Pings waiting belong to the old stack.
        self.pings.retain(|_, ping| *ping != contacts::PingState::Waiting);
        self.pn.restarting(crate::lxmf::pn::PnConfig::from_settings(&self.settings, &self.paths));
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

    /// Which view is on screen: the tab and its panes, focus, menus and
    /// prompts (not list positions). When it changes, most of the screen does.
    fn view_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        use std::mem::discriminant;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        discriminant(&self.tab).hash(&mut h);
        (self.composing, self.prompt.is_some(), self.net_search.typing, self.keys_help).hash(&mut h);
        self.message_search.is_some().hash(&mut h);
        // Emoji over the view, which some terminals draw narrower than
        // they should (see `take_full_redraw`).
        if let Some(picker) = &self.emoji {
            (picker.tab, picker.top, picker.search.text()).hash(&mut h);
        }
        if let Some((at, found)) = self.shortcode_matches() {
            (at, found.iter().map(|e| e.as_str()).collect::<Vec<_>>()).hash(&mut h);
        }
        (self.channels.typing, self.channels.menu.is_some(), self.channels.picker.is_some()).hash(&mut h);
        (discriminant(&self.browser.pane), discriminant(&self.browser.focus), self.browser.source.is_some()).hash(&mut h);
        self.browser.search.typing.hash(&mut h);
        (self.node.editing, discriminant(&self.node.view), self.node.editor.is_some()).hash(&mut h);
        (discriminant(&self.rns.focus), self.rns.picker.is_some(), self.rns.editor.is_some()).hash(&mut h);
        h.finish()
    }

    /// Whether to repaint the whole screen before this draw: asked for
    /// (Ctrl-L), or on switching tabs and panes. Only changed cells are sent
    /// otherwise, so a character a terminal drew wider than expected (some
    /// emoji) could be left behind as the view changes.
    pub fn take_full_redraw(&mut self) -> bool {
        let view = self.view_key();
        let changed = view != self.shown_view;
        self.shown_view = view;
        std::mem::take(&mut self.full_redraw) || changed
    }

    /// Periodic work (called about twice a second).
    pub fn on_tick(&mut self) {
        self.channels_tick();
        self.watch_connection();
        self.refresh_partials();
        for report in self.saver.reports() {
            match report {
                saver::Report::Failed(e) => self.fail(e),
                saver::Report::Note(note) => self.log(note),
            }
        }
    }
}
