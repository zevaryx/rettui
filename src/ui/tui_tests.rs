//! The terminal UI driven as someone at the keyboard (and mouse) would:
//! keys and clicks, each followed by drawing the screen, on a test app.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::app::{App, Tab};
use crate::config::Settings;
use crate::net::NetCommand;
use crate::store::Store;

const WIDTH: u16 = 140;
const HEIGHT: u16 = 40;

/// The app, its screen and its folder.
pub(crate) struct Tui {
    pub app: App,
    terminal: Terminal<TestBackend>,
    pub dir: PathBuf,
    pub net: UnboundedReceiver<NetCommand>,
}

impl Tui {
    /// A test app with its own Reticulum config (never the user's).
    pub fn new(name: &str) -> Self {
        Self::with(name, |_| {})
    }

    pub fn with(name: &str, settings: impl FnOnce(&mut Settings)) -> Self {
        let dir = std::env::temp_dir().join(format!("rettui-tui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut chosen = Settings { rns_config: Some(dir.join("rns").display().to_string()), map_tiles: None, ..Settings::default() };
        settings(&mut chosen);
        let (app, net) = crate::app::test_app_with_net(&dir, chosen, Store::default());
        Tui { app, terminal: Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap(), dir, net }
    }

    /// Draw, and what's on screen.
    pub fn draw(&mut self) -> String {
        self.terminal.draw(|frame| super::draw(frame, &mut self.app)).unwrap();
        let buffer = self.terminal.backend().buffer();
        (0..HEIGHT).map(|y| (0..WIDTH).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    /// A key, then the screen drawn.
    pub fn key(&mut self, code: KeyCode) -> &mut Self {
        self.app.on_key(KeyEvent::from(code));
        self.draw();
        self
    }

    pub fn ctrl(&mut self, c: char) -> &mut Self {
        self.app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
        self.draw();
        self
    }

    pub fn alt(&mut self, c: char) -> &mut Self {
        self.app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT));
        self.draw();
        self
    }

    /// Keys for each character (not drawn between them).
    pub fn keys(&mut self, text: &str) -> &mut Self {
        for c in text.chars() {
            self.app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        self.draw();
        self
    }

    /// Typed into a prompt, then Enter.
    pub fn answer(&mut self, text: &str) -> &mut Self {
        self.keys(text);
        self.key(KeyCode::Enter)
    }

    fn mouse(&mut self, kind: MouseEventKind, x: u16, y: u16) {
        self.app.on_mouse(MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE });
    }

    /// A click (press and release) at a cell, then the screen drawn.
    pub fn click(&mut self, x: u16, y: u16) -> &mut Self {
        self.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        self.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
        self.draw();
        self
    }

    /// A click in the middle of an area's first row.
    pub fn click_in(&mut self, area: Rect) -> &mut Self {
        self.click(area.x + area.width / 2, area.y)
    }

    pub fn double_click(&mut self, x: u16, y: u16) -> &mut Self {
        self.click(x, y);
        self.click(x, y)
    }

    pub fn scroll(&mut self, x: u16, y: u16, down: bool) -> &mut Self {
        self.mouse(if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp }, x, y);
        self.draw();
        self
    }

    pub fn drag(&mut self, from: (u16, u16), to: (u16, u16)) -> &mut Self {
        self.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
        self.mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
        self.mouse(MouseEventKind::Up(MouseButton::Left), to.0, to.1);
        self.draw();
        self
    }

    pub fn tab(&mut self, tab: Tab) -> &mut Self {
        let number = Tab::ALL.iter().position(|t| *t == tab).unwrap() + 1;
        self.key(KeyCode::Char(char::from(b'0' + number as u8)))
    }

    /// Nothing over the tab: whatever an earlier step left open closed.
    pub fn settle(&mut self) -> &mut Self {
        let app = &mut self.app;
        (app.prompt, app.picked, app.contact_card, app.map, app.emoji) = (None, None, None, None, None);
        (app.message_search, app.archive_reader, app.forward) = (None, None, None);
        app.composing = false;
        self.draw();
        self
    }

    /// What the footer says last (notes, warnings).
    pub fn said(&self) -> String {
        self.app.log.iter().rev().take(5).cloned().collect::<Vec<_>>().join(" | ")
    }

    /// What the app told the network since last asked.
    pub fn sent(&mut self) -> Vec<NetCommand> {
        std::iter::from_fn(|| self.net.try_recv().ok()).collect()
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn the_reticulum_tab_option_by_option_and_as_text() {
    let mut tui = Tui::new("reticulum");
    tui.tab(Tab::Reticulum);
    assert!(tui.draw().contains("Reticulum"));
    // Sections, then their options: moved through and changed.
    tui.key(KeyCode::Down).key(KeyCode::Up).key(KeyCode::PageDown).key(KeyCode::PageUp);
    tui.key(KeyCode::Enter); // Options
    tui.key(KeyCode::Down).key(KeyCode::Down).key(KeyCode::PageDown).key(KeyCode::PageUp);
    tui.key(KeyCode::Enter); // toggles a bool, or asks
    if tui.app.prompt.is_some() {
        tui.answer("");
    }
    tui.key(KeyCode::Char('d')); // back to its default
    tui.key(KeyCode::Left).key(KeyCode::Tab).key(KeyCode::BackTab).key(KeyCode::Right).key(KeyCode::Esc);

    // A new interface: named, its type picked.
    tui.key(KeyCode::Char('a')).answer("Home");
    assert!(tui.app.rns.picker.is_some());
    tui.key(KeyCode::Down).key(KeyCode::Up).key(KeyCode::Enter);
    let config = std::fs::read_to_string(tui.app.rns_path()).unwrap();
    assert!(config.contains("[[Home]]"), "{config}");
    // Its options: each kind edited.
    for _ in 0..12 {
        tui.key(KeyCode::Down);
        tui.key(KeyCode::Enter);
        if tui.app.rns.picker.is_some() {
            tui.key(KeyCode::Down).key(KeyCode::Enter);
        }
        if tui.app.prompt.is_some() {
            tui.answer("1");
        }
    }
    tui.key(KeyCode::Up).key(KeyCode::Char('d'));
    // The interface: disabled, enabled, renamed, deleted.
    tui.key(KeyCode::Esc);
    tui.key(KeyCode::Char(' ')).key(KeyCode::Char(' '));
    tui.key(KeyCode::Char('r'));
    tui.app.prompt.as_mut().unwrap().input = crate::term::input::TextInput::with_text("Away");
    tui.key(KeyCode::Enter);
    assert!(std::fs::read_to_string(tui.app.rns_path()).unwrap().contains("[[Away]]"));
    tui.key(KeyCode::Char('x')).answer("y");
    assert!(!std::fs::read_to_string(tui.app.rns_path()).unwrap().contains("[[Away]]"));
    tui.key(KeyCode::Char('R'));

    // The picker, closed with Esc and by a click outside it.
    tui.key(KeyCode::Char('a')).answer("Radio");
    tui.key(KeyCode::Esc);
    tui.key(KeyCode::Char('a')).answer("Radio2");
    tui.click(0, 0);
    assert!(tui.app.rns.picker.is_none());

    // Clicks on sections and options; the wheel.
    let (sections, options) = (tui.app.regions.rns_sections, tui.app.regions.rns_options);
    tui.click_in(sections).click(options.x + 2, options.y + 1).double_click(options.x + 2, options.y + 2);
    if tui.app.prompt.is_some() {
        tui.key(KeyCode::Esc);
    }
    tui.app.rns.picker = None;
    tui.scroll(sections.x + 1, sections.y + 1, true).scroll(options.x + 1, options.y + 1, false);

    // As text: typed, saved, and Esc asks before dropping changes.
    tui.key(KeyCode::Char('t'));
    assert!(tui.app.rns.editor.is_some());
    tui.keys("# a comment\n").ctrl('s');
    assert!(std::fs::read_to_string(tui.app.rns_path()).unwrap().contains("# a comment"));
    tui.keys("x").key(KeyCode::Esc);
    assert!(tui.app.prompt.is_some());
    tui.answer("y");
    assert!(tui.app.rns.editor.is_none());
    tui.key(KeyCode::Char('t')).key(KeyCode::Esc);
    tui.key(KeyCode::Char('t'));
    let editor = tui.app.regions.rns_editor;
    tui.click(editor.x + 1, editor.y + 1).scroll(editor.x + 1, editor.y + 1, true).key(KeyCode::Esc);
    // A text that doesn't parse isn't saved.
    tui.key(KeyCode::Char('t')).keys("[[[").ctrl('s').key(KeyCode::Esc).answer("y");

    // Restarting asks first.
    tui.ctrl('r');
    tui.answer("y");
    assert!(tui.app.take_rns_restart());
}

#[test]
fn the_node_tab_pages_editor_and_files() {
    let mut tui = Tui::new("node");
    tui.tab(Tab::Node);
    assert!(tui.draw().contains("Node"));
    // A new page, in the editor: written, formatted, saved.
    tui.key(KeyCode::Char('n')).answer("about");
    assert!(tui.app.node.editing && tui.app.node.editor.is_some(), "{}", tui.said());
    tui.keys(">About\nWe are here.\n").ctrl('s');
    assert!(std::fs::read_to_string(tui.dir.join("node/pages/about.mu")).unwrap().contains("We are here"));
    for c in ['b', 'i', 'u', 'n', 'l', 'c', 'r', '1', '2', '3', 'v', 't', 'o', 'h', 'a'] {
        tui.alt(c);
        if tui.app.prompt.is_some() {
            tui.answer("x");
        }
    }
    for c in ['f', 'g', 'k', 'm', 'd'] {
        tui.alt(c);
        if tui.app.prompt.is_some() {
            tui.answer("f00");
        }
    }
    // A file from this computer, added and linked.
    let file = tui.dir.join("notes.txt");
    std::fs::write(&file, "notes").unwrap();
    tui.alt('p').answer(&file.display().to_string());
    assert!(tui.dir.join("node/files/notes.txt").is_file(), "{}", tui.said());
    tui.alt('p').answer("/no/such/file");
    tui.ctrl('s');
    // The views: editor, preview (scrolled), both.
    tui.ctrl('p');
    tui.key(KeyCode::Down).key(KeyCode::Up).key(KeyCode::PageDown).key(KeyCode::PageUp).key(KeyCode::Home);
    tui.alt('b');
    tui.ctrl('p').ctrl('p');
    tui.key(KeyCode::Esc);
    assert!(!tui.app.node.editing);

    // The list: moved through, renamed, moved to a folder, deleted.
    tui.key(KeyCode::Down).key(KeyCode::Up).key(KeyCode::Enter).key(KeyCode::Esc);
    let rows = tui.app.node_rows();
    let about = rows.iter().position(|row| tui.app.node_entry(row).is_some_and(|e| e.path == "about.mu")).unwrap();
    tui.app.node.list.select(Some(about));
    tui.key(KeyCode::Char('r'));
    tui.app.prompt.as_mut().unwrap().input = crate::term::input::TextInput::with_text("us.mu");
    tui.key(KeyCode::Enter);
    assert!(tui.dir.join("node/pages/us.mu").is_file(), "{}", tui.said());
    tui.key(KeyCode::Char('m'));
    tui.app.prompt.as_mut().unwrap().input = crate::term::input::TextInput::with_text("info");
    tui.key(KeyCode::Enter);
    assert!(tui.dir.join("node/pages/info/us.mu").is_file(), "{}", tui.said());
    // Another page with unsaved changes: opening one asks first.
    tui.key(KeyCode::Char('n')).answer("draft");
    tui.keys("unsaved").key(KeyCode::Esc);
    let rows = tui.app.node_rows();
    let moved = rows.iter().position(|row| tui.app.node_entry(row).is_some_and(|e| e.path == "info/us.mu")).unwrap();
    tui.app.node.list.select(Some(moved));
    tui.key(KeyCode::Enter);
    assert!(tui.app.prompt.is_some());
    tui.answer("y");
    tui.key(KeyCode::Esc).key(KeyCode::Char('x')).answer("y");
    assert!(!tui.dir.join("node/pages/info/us.mu").exists());
    // Hosting on and off, announcing, the address, its pages in the browser.
    tui.key(KeyCode::Char('h'));
    assert!(tui.app.settings.node_enabled);
    tui.key(KeyCode::Char('h')).key(KeyCode::Char('a')).key(KeyCode::Char('p')).key(KeyCode::Char('y'));
    assert!(tui.sent().iter().any(|c| matches!(c, NetCommand::HostAnnounce)));
    tui.key(KeyCode::Char('b'));
    assert_eq!(tui.app.tab, Tab::Browser);

    // The mouse: the list, the editor (and dragging in it), the wheel.
    tui.tab(Tab::Node);
    let (list, editor) = (tui.app.regions.node_pages, tui.app.regions.node_editor);
    tui.click_in(list).double_click(list.x + 2, list.y);
    if tui.app.prompt.is_some() {
        tui.answer("y");
    }
    tui.scroll(list.x + 1, list.y + 1, true).scroll(list.x + 1, list.y + 1, false);
    if !editor.is_empty() {
        tui.click(editor.x + 1, editor.y).drag((editor.x + 1, editor.y), (editor.x + 5, editor.y + 3));
        tui.scroll(editor.x + 1, editor.y + 1, true);
    }
    if let Some(&(area, _)) = tui.app.regions.node_ribbon.first() {
        tui.click_in(area);
        if tui.app.prompt.is_some() {
            tui.key(KeyCode::Esc);
        }
    }
}

#[test]
fn browsing_this_clients_own_node() {
    let mut tui = Tui::new("browse");
    // Pages of its own node are read from its folder: no network.
    let pages = tui.dir.join("node/pages");
    std::fs::create_dir_all(&pages).unwrap();
    std::fs::write(
        pages.join("index.mu"),
        ">Home\nSee `[the next page`:/page/next.mu] or `[Bob`lxmf@abababababababababababababababab]\n\
         Name: `<name`Bob>\n`<?|agree|yes`>I agree\n`[Send`:/page/next.mu`name|agree]\n\
         `[a hub`rrc://efefefefefefefefefefefefefefefef/lobby]\n",
    )
    .unwrap();
    std::fs::write(pages.join("next.mu"), ">Next\n".to_string() + &"line\n".repeat(80)).unwrap();
    let home = format!("{}:/page/index.mu", hex::encode(tui.app.node.hash));
    tui.tab(Tab::Browser);
    tui.key(KeyCode::Char('g'));
    tui.app.prompt.as_mut().unwrap().input = crate::term::input::TextInput::with_text(&home);
    tui.key(KeyCode::Enter);
    let shown = tui.draw();
    assert!(shown.contains("Home") && shown.contains("the next page"), "{shown}");
    // Through the links and fields: a field typed into, the checkbox.
    tui.key(KeyCode::Right);
    for _ in 0..3 {
        tui.key(KeyCode::Tab);
    }
    tui.key(KeyCode::Enter);
    if tui.app.prompt.is_some() {
        tui.answer("Alice");
    }
    tui.key(KeyCode::Tab).key(KeyCode::Enter).key(KeyCode::BackTab).key(KeyCode::Char('L'));
    // Scrolling, the source, copying.
    tui.key(KeyCode::Down).key(KeyCode::Up).key(KeyCode::PageDown).key(KeyCode::PageUp).key(KeyCode::Home);
    tui.key(KeyCode::Char('u'));
    assert!(tui.app.browser.view_source);
    tui.key(KeyCode::Char('Y')).key(KeyCode::Esc);
    tui.key(KeyCode::Char('Y')).key(KeyCode::Char('y'));
    // Following a link, back, reloading, saving.
    tui.app.browser.selected = Some(0);
    tui.key(KeyCode::Enter);
    assert!(tui.draw().contains("Next"));
    tui.key(KeyCode::Char(' ')).key(KeyCode::Char('b'));
    assert!(tui.draw().contains("Home"));
    tui.key(KeyCode::Char('r')).key(KeyCode::Char('s'));
    assert!(tui.app.store.saved.iter().any(|b| b.url.ends_with("index.mu")));
    tui.key(KeyCode::Char('R'));
    // Finding in the page.
    tui.key(KeyCode::Char('f')).keys("line").key(KeyCode::Enter).key(KeyCode::Enter).key(KeyCode::Esc);
    // The pane: saved pages and nodes, searched, a saved page removed.
    tui.key(KeyCode::Left).key(KeyCode::Down).key(KeyCode::Up).key(KeyCode::Enter);
    tui.key(KeyCode::Char('t')).key(KeyCode::Down).key(KeyCode::Char('s')).key(KeyCode::Char('t'));
    tui.key(KeyCode::Char('/')).keys("home").key(KeyCode::Enter).key(KeyCode::Esc);
    tui.key(KeyCode::Char('x'));
    // Home (none set), identifying, Esc.
    tui.key(KeyCode::Char('H')).key(KeyCode::Char('I')).key(KeyCode::Esc);
    // A link that isn't: said.
    tui.key(KeyCode::Char('g'));
    tui.app.prompt.as_mut().unwrap().input = crate::term::input::TextInput::with_text("nowhere");
    tui.key(KeyCode::Enter);
    // The mouse: the address bar, the pane's tabs and list, the page.
    tui.app.prompt = None;
    let regions = (tui.app.regions.address, tui.app.regions.browser_list, tui.app.regions.page);
    tui.click_in(regions.0);
    tui.app.prompt = None;
    if let Some(&(area, _)) = tui.app.regions.browser_tabs.first() {
        tui.click_in(area);
    }
    tui.click_in(regions.1);
    tui.click(regions.2.x + 2, regions.2.y + 1);
    tui.drag((regions.2.x + 1, regions.2.y + 1), (regions.2.x + 10, regions.2.y + 2));
    tui.scroll(regions.2.x + 1, regions.2.y + 1, true).scroll(regions.2.x + 1, regions.2.y + 1, false);
}

/// Bob, who writes.
const BOB: &str = "abababababababababababababababab";

/// A conversation with Bob: words, a picture, a location, a failed reply.
fn with_bob(tui: &mut Tui) {
    use crate::lxmf::Location;
    use crate::store::{Conversation, Message, MessageState, StoredAttachment};
    let picture = tui.dir.join("cat.png");
    image::RgbImage::from_pixel(8, 8, image::Rgb([10, 200, 10])).save(&picture).unwrap();
    let at = |n: f64| 1_800_000_000.0 + n;
    let received = MessageState::Received { verified: true };
    let messages = vec![
        Message {
            id: "a".repeat(64),
            incoming: true,
            content: "Lunch at noon? See lxmf@cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd".into(),
            timestamp: at(1.0),
            state: received.clone(),
            ..Message::default()
        },
        Message {
            id: "b".repeat(64),
            incoming: true,
            content: "A cat".into(),
            timestamp: at(2.0),
            state: received.clone(),
            attachments: vec![StoredAttachment { name: "cat.png".into(), path: picture, size: 100, image: true, voice: None }],
            ..Message::default()
        },
        Message {
            id: "c".repeat(64),
            incoming: true,
            timestamp: at(3.0),
            state: received,
            location: Some(Location {
                latitude: 51.5,
                longitude: -0.12,
                altitude: None,
                speed: None,
                bearing: None,
                accuracy: Some(9.0),
                updated: None,
            }),
            ..Message::default()
        },
        Message {
            id: "local-1".into(),
            content: "Yes!".into(),
            timestamp: at(4.0),
            state: MessageState::Failed("no path".into()),
            ..Message::default()
        },
    ];
    tui.app.store.conversations.insert(BOB.into(), Conversation { messages, unread: 2, ..Conversation::default() });
    tui.app.store.contacts.entry(BOB.into()).or_default().alias = Some("Bob".into());
}

#[test]
fn messages_at_the_keyboard_and_with_the_mouse() {
    let mut tui = Tui::new("messages");
    with_bob(&mut tui);
    tui.tab(Tab::Messages);
    tui.key(KeyCode::Down).key(KeyCode::Up);
    assert_eq!(tui.app.active_conversation.as_deref(), Some(BOB));
    assert!(tui.draw().contains("Lunch at noon"));
    // Scrolling, delivery modes, pinning, muting, read.
    for code in [
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Char('d'),
        KeyCode::Char('d'),
        KeyCode::Char('*'),
        KeyCode::Char('N'),
        KeyCode::Char('R'),
        KeyCode::Char('y'),
        KeyCode::Char('o'),
        KeyCode::Char('P'),
    ] {
        tui.key(code);
    }

    // Writing: an emoji picked, a shortcode, a reply chosen, sent (as
    // messages to Bob go: automatically, after cycling through the modes).
    tui.app.set_delivery(BOB, crate::lxmf::DeliveryMode::Auto).unwrap();
    tui.app.delivery_mode = crate::lxmf::DeliveryMode::Auto;
    tui.key(KeyCode::Enter);
    assert!(tui.app.composing);
    tui.keys("Sounds good ");
    tui.ctrl('e');
    assert!(tui.app.emoji.is_some());
    tui.key(KeyCode::Right).key(KeyCode::Down).key(KeyCode::Tab).keys("smile").key(KeyCode::Enter);
    tui.keys(" :thumbs").key(KeyCode::Tab);
    tui.ctrl('r').key(KeyCode::Up).key(KeyCode::Down);
    tui.key(KeyCode::PageUp).key(KeyCode::PageDown);
    tui.key(KeyCode::Enter);
    assert!(
        tui.sent().iter().any(|c| matches!(c, NetCommand::SendMessage { .. })),
        "composing {} emoji {} reply {:?} text {:?}: {}",
        tui.app.composing,
        tui.app.emoji.is_some(),
        tui.app.reply,
        tui.app.compose.text(),
        tui.said()
    );
    tui.ctrl('p').ctrl('p').ctrl('x');
    tui.key(KeyCode::Esc).key(KeyCode::Esc);
    tui.key(KeyCode::Char('r')).key(KeyCode::Esc).key(KeyCode::Esc);

    // A message picked: moved, replied to, reacted to, copied, forwarded,
    // opened, retried, deleted.
    tui.key(KeyCode::Char('m'));
    assert!(tui.app.picked.is_some());
    tui.key(KeyCode::Up).key(KeyCode::Up).key(KeyCode::Down);
    tui.key(KeyCode::Char('y'));
    tui.key(KeyCode::Char('e'));
    if tui.app.emoji.is_some() {
        tui.key(KeyCode::Enter);
    }
    tui.key(KeyCode::Char('m')).key(KeyCode::Up).key(KeyCode::Char('o'));
    tui.key(KeyCode::Char('m')).key(KeyCode::Char('f'));
    assert!(tui.app.forward.is_some());
    tui.key(KeyCode::Down).key(KeyCode::Up).key(KeyCode::Esc);
    tui.key(KeyCode::Char('m')).key(KeyCode::End);
    for _ in 0..3 {
        tui.key(KeyCode::Down);
    }
    tui.key(KeyCode::Char('t'));
    tui.key(KeyCode::Char('m')).key(KeyCode::Char('x')).answer("n");
    tui.key(KeyCode::Esc);

    // Searching what was said: typed, scoped, opened.
    tui.settle()
        .key(KeyCode::Char('/'))
        .keys("lunch")
        .key(KeyCode::Tab)
        .key(KeyCode::Tab)
        .key(KeyCode::Down)
        .key(KeyCode::Up)
        .key(KeyCode::Enter);
    tui.key(KeyCode::Char('/')).keys("cat");
    let results = tui.app.regions.message_search;
    tui.click(results.x + 2, results.y);
    tui.key(KeyCode::Char('/')).click(0, 0);
    // The archive (empty), the map, sharing a location, exporting.
    tui.key(KeyCode::Char('H')).key(KeyCode::Down).key(KeyCode::Esc);
    tui.key(KeyCode::Char('H')).click(0, HEIGHT - 1);
    tui.key(KeyCode::Char('M'));
    assert!(tui.app.map.is_some());
    for code in [
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::End,
        KeyCode::Home,
        KeyCode::Char('+'),
        KeyCode::Char('-'),
        KeyCode::Char('0'),
        KeyCode::Enter,
    ] {
        tui.key(code);
    }
    if tui.app.map.is_some() {
        let map = tui.app.regions.map;
        tui.scroll(map.x + 5, map.y + 5, false).scroll(map.x + 5, map.y + 5, true);
        let list = tui.app.regions.map_list;
        tui.click(list.x + 1, list.y);
        tui.click(0, 0);
    }
    tui.key(KeyCode::Char('L')).answer("51.5, -0.12");
    tui.key(KeyCode::Char('L')).answer("not a place");
    tui.key(KeyCode::Char('E'));

    // The contact card: its buttons by key and by click.
    tui.settle().key(KeyCode::Char('c'));
    assert!(tui.app.contact_card.is_some());
    tui.key(KeyCode::Char('n'));
    if tui.app.prompt.is_some() {
        tui.answer("Bobby");
    }
    tui.key(KeyCode::Char('c'));
    if let Some(&(button, _)) = tui.app.regions.card_buttons.first() {
        tui.click_in(button);
        if tui.app.prompt.is_some() {
            tui.key(KeyCode::Esc);
        }
    }
    tui.key(KeyCode::Char('c')).click(0, 0);
    assert!(tui.app.contact_card.is_none());

    // Prompts from this tab: a new conversation, a file to attach, paper.
    tui.settle().key(KeyCode::Char('n')).answer("cd".repeat(16).as_str());
    tui.key(KeyCode::Char('n')).answer("nonsense");
    let file = tui.dir.join("cat.png").display().to_string();
    tui.key(KeyCode::Char('a')).answer(&file);
    tui.key(KeyCode::Esc);
    tui.key(KeyCode::Char('p')).answer("lxm://nonsense");
    tui.key(KeyCode::Char('g'));
    tui.key(KeyCode::Esc);

    // The mouse: the list, the history (clicks and a drag), the wheel.
    tui.tab(Tab::Messages);
    let (list, history) = (tui.app.regions.conversations, tui.app.regions.history);
    tui.click_in(list).scroll(list.x + 1, list.y + 1, true).scroll(list.x + 1, list.y + 1, false);
    for row in 0..history.height.min(12) {
        tui.click(history.x + 3, history.y + row);
        tui.app.picked = None;
        tui.app.prompt = None;
        tui.app.contact_card = None;
        tui.app.map = None;
        tui.tab(Tab::Messages);
    }
    tui.drag((history.x + 1, history.y + 1), (history.x + 30, history.y + 3));
    tui.scroll(history.x + 1, history.y + 1, false).scroll(history.x + 1, history.y + 1, true);

    // Deleting the conversation asks first.
    tui.key(KeyCode::Char('X')).answer("n");
    assert!(tui.app.store.conversations.contains_key(BOB));
}
