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
pub(crate) const BOB: &str = "abababababababababababababababab";

/// A conversation with Bob: words, a picture, a location, a failed reply.
pub(crate) fn with_bob(tui: &mut Tui) {
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

#[test]
fn the_network_and_status_tabs() {
    use crate::net::{NetEvent, PeerKind};
    let mut tui = Tui::new("network");
    for (n, kind, name) in [(1u8, PeerKind::Lxmf, "Bob"), (2, PeerKind::Nomad, "News"), (3, PeerKind::Propagation, "Relay")] {
        tui.app.on_net(NetEvent::Announce { kind, hash: [n; 16], name: Some(name.into()), hops: n });
    }
    tui.tab(Tab::Network);
    let shown = tui.draw();
    assert!(shown.contains("Bob") && shown.contains("News") && shown.contains("Relay"), "{shown}");
    // Moved through, sorted, filtered, searched.
    for code in [
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Char('s'),
        KeyCode::Char('s'),
        KeyCode::Char('s'),
        KeyCode::Char('i'),
        KeyCode::Char('f'),
        KeyCode::Char('f'),
        KeyCode::Char('f'),
        KeyCode::Char('f'),
    ] {
        tui.key(code);
    }
    tui.key(KeyCode::Char('/')).keys("bo").key(KeyCode::Enter).key(KeyCode::Esc);
    // A conversation from the list; a node to browse; the propagation node.
    tui.tab(Tab::Network).key(KeyCode::Enter);
    tui.tab(Tab::Network).key(KeyCode::Char('m'));
    tui.tab(Tab::Network);
    let relay = hex::encode([3u8; 16]);
    tui.key(KeyCode::Char('f')); // every kind again
    for _ in 0..3 {
        if tui.app.selected_peer().is_some_and(|(key, _)| key == relay) {
            break;
        }
        tui.key(KeyCode::Down);
    }
    tui.key(KeyCode::Char('p'));
    assert_eq!(tui.app.settings.propagation_node.as_deref(), Some(relay.as_str()));
    // The announce viewer.
    tui.key(KeyCode::Char('a')).key(KeyCode::Down).key(KeyCode::Up).key(KeyCode::Char('a'));
    let list = tui.app.regions.peers;
    tui.click(list.x + 2, list.y + 1).scroll(list.x + 1, list.y + 1, true).scroll(list.x + 1, list.y + 1, false);

    // Status: the settings, by section, edited.
    tui.tab(Tab::Status);
    for code in [KeyCode::Down, KeyCode::Down, KeyCode::Up, KeyCode::Tab, KeyCode::Tab, KeyCode::BackTab, KeyCode::Right, KeyCode::Left] {
        tui.key(code);
    }
    for _ in 0..6 {
        tui.key(KeyCode::Enter);
        if tui.app.prompt.is_some() {
            tui.key(KeyCode::Esc);
        }
        tui.key(KeyCode::Down);
    }
    // The address as a QR code, the guide, backups asked for, the first
    // steps hidden, an identity to import asked for, updates.
    tui.app.on_net(NetEvent::Started { lxmf_hash: [9; 16], public_key: [1; 64], known: crate::net::remote::KnownIdentities::none() });
    tui.key(KeyCode::Char('c'));
    assert!(tui.app.paper_view.is_some());
    tui.key(KeyCode::Char('q'));
    tui.key(KeyCode::Char('g'));
    if tui.app.guide.is_some() {
        tui.key(KeyCode::Down).key(KeyCode::Up).key(KeyCode::Esc);
    }
    tui.settle();
    let backup = tui.dir.join("backup.tar.gz").display().to_string();
    tui.key(KeyCode::Char('B')).answer(&backup);
    let identity = tui.dir.join("identity-copy").display().to_string();
    tui.key(KeyCode::Char('b')).answer(&identity);
    tui.key(KeyCode::Char('i')).answer("/no/such/identity");
    tui.settle();
    tui.key(KeyCode::Char('x')).key(KeyCode::Char('u')).key(KeyCode::Char('U')).key(KeyCode::Char('y'));
    tui.settle();
    tui.key(KeyCode::Char('e'));
    tui.settle();
    // The keys, listed.
    tui.key(KeyCode::Char('?'));
    assert!(tui.app.keys_help);
    tui.key(KeyCode::Char('x'));
    tui.key(KeyCode::F(1)).click(0, 0);
    // Every tab drawn, and the sidebar clicked.
    for tab in Tab::ALL {
        tui.tab(tab);
    }
    if let Some(&(area, _)) = tui.app.regions.tabs.first() {
        tui.click_in(area);
    }
    tui.ctrl('l');
}

/// Carol's identity, on the Hilltop hub.
const CAROL: [u8; 16] = [0xcc; 16];

/// A hub, connected, with a room, a whisper conversation, people, and the
/// rooms it lists.
fn with_hilltop(tui: &mut Tui) -> crate::net::Hash {
    use crate::app::channels::{ChatLine, Hub, HubStatus, Target};
    let hash = [1; 16];
    let mut hub = Hub::new(hash, "rrc.hilltop".into(), "Hilltop".into());
    hub.status = HubStatus::Connected;
    hub.direct_notices = true;
    hub.motd = Some("Welcome to the hill".into());
    hub.available = Some(vec![("lobby".into(), Some("Hello".into())), ("dev".into(), None)]);
    hub.rooms.insert("lobby".into());
    hub.nicks.insert(CAROL.to_vec(), "carol".into());
    hub.topics.insert("lobby".into(), "All things hill".into());
    hub.members.insert("lobby".into(), [CAROL.to_vec(), vec![0xdd; 4]].into_iter().collect());
    let carol = hex::encode(CAROL);
    let lines: Vec<ChatLine> = serde_json::from_value(serde_json::json!([
        { "kind": "Msg", "src": carol, "nick": "carol", "text": "hi Tui, see rrc://0202020202020202020202020202020202/dev", "ts": 1_800_000_000_000u64, "mention": true },
        { "kind": "Action", "src": carol, "nick": "carol", "text": "waves", "ts": 1_800_000_001_000u64 },
        { "kind": "Notice", "text": "a notice", "ts": 1_800_000_002_000u64 },
        { "kind": "Error", "text": "an error", "ts": 1_800_000_003_000u64 },
        { "kind": "System", "text": "carol joined", "ts": 1_800_000_004_000u64 },
        { "kind": "Msg", "text": "mine", "ts": 1_800_000_005_000u64, "own": true },
    ]))
    .unwrap();
    hub.buffers.insert("lobby".into(), lines);
    let whisper: Vec<ChatLine> = serde_json::from_value(serde_json::json!([
        { "kind": "Private", "src": carol, "nick": "carol", "text": "psst", "ts": 1_800_000_006_000u64 },
    ]))
    .unwrap();
    hub.buffers.insert(format!("@{carol}"), whisper);
    hub.unread.insert("lobby".into(), 2);
    tui.app.channels.hubs.push(hub);
    tui.app.settings.display_name = "Tui".into();
    tui.app.channels.selected = Some(Target { hub: hash, room: Some("lobby".into()) });
    tui.tab(Tab::Channels);
    hash
}

#[test]
fn channels_at_the_keyboard_and_with_the_mouse() {
    let mut tui = Tui::new("channels");
    tui.tab(Tab::Channels);
    // No hub yet: n asks for one; nonsense is said to be.
    tui.key(KeyCode::Char('n')).answer("not a hub");
    tui.key(KeyCode::Char('m')).key(KeyCode::Char('x')).key(KeyCode::Char('y'));
    let hub = with_hilltop(&mut tui);
    let screen = tui.draw();
    assert!(screen.contains("Hilltop") && screen.contains("lobby") && screen.contains("waves"), "{screen}");

    // Moving through the list: the hub, its room, the whisper.
    tui.key(KeyCode::Char('k')).key(KeyCode::Up).key(KeyCode::Char('j')).key(KeyCode::Down).key(KeyCode::Down);
    tui.key(KeyCode::Char('k'));
    // Each setting, twice (on and off).
    for key in ['J', 'J', 'D', 'D', 'a', 'a'] {
        tui.key(KeyCode::Char(key));
    }
    // Notification levels for the room, the hub and the whisper.
    for _ in 0..4 {
        tui.key(KeyCode::Char('N'));
    }
    assert!(tui.said().contains("Notifications in #lobby"), "{}", tui.said());
    tui.key(KeyCode::Up);
    for _ in 0..3 {
        tui.key(KeyCode::Char('N'));
    }
    tui.key(KeyCode::Down).key(KeyCode::Down);
    for _ in 0..3 {
        tui.key(KeyCode::Char('N'));
    }
    tui.key(KeyCode::Up);
    // Its link, and scrolling the history.
    tui.key(KeyCode::Char('y'));
    assert_eq!(tui.app.clipboard.last.as_deref(), Some(format!("rrc://{}:rrc.hilltop/lobby", hex::encode(hub)).as_str()));
    tui.key(KeyCode::PageUp).key(KeyCode::PageDown).key(KeyCode::Home).key(KeyCode::End);

    // Members: m picks one, and their menu does what it says.
    tui.key(KeyCode::Char('m')).key(KeyCode::Char('j')).key(KeyCode::Char('k')).key(KeyCode::Down).key(KeyCode::Up);
    tui.key(KeyCode::Enter);
    assert!(tui.app.channels.menu.is_some());
    tui.key(KeyCode::Char('j')).key(KeyCode::Char('k')).key(KeyCode::Down).key(KeyCode::Up);
    tui.key(KeyCode::Char('@'));
    assert!(tui.app.channels.input.text().contains("@carol"), "{}", tui.app.channels.input.text());
    tui.key(KeyCode::Esc);
    tui.key(KeyCode::Char('m')).key(KeyCode::Enter).key(KeyCode::Char('w'));
    assert_eq!(tui.app.channels.active().map(|(_, room)| room), Some(format!("@{}", hex::encode(CAROL))));
    tui.key(KeyCode::Esc).key(KeyCode::Up);
    tui.key(KeyCode::Char('m')).key(KeyCode::Enter).key(KeyCode::Esc);
    tui.key(KeyCode::Char('m')).key(KeyCode::Esc);
    tui.key(KeyCode::Char('m')).key(KeyCode::Enter).key(KeyCode::Down).key(KeyCode::Down).key(KeyCode::Down).key(KeyCode::Enter);
    tui.key(KeyCode::Char('m'))
        .key(KeyCode::Enter)
        .key(KeyCode::Down)
        .key(KeyCode::Down)
        .key(KeyCode::Down)
        .key(KeyCode::Down)
        .key(KeyCode::Enter);
    tui.tab(Tab::Channels);
    tui.app.channels.selected = Some(crate::app::channels::Target { hub, room: Some("lobby".into()) });
    tui.key(KeyCode::Char('m')).key(KeyCode::Enter).key(KeyCode::Char('l'));
    assert_eq!(tui.app.tab, Tab::Messages);
    tui.tab(Tab::Channels);

    // Typing: a message, a command, Esc.
    tui.key(KeyCode::Enter).keys("hello hill").key(KeyCode::Enter);
    tui.keys("/topic").key(KeyCode::Enter).keys("/help").key(KeyCode::Enter);
    tui.key(KeyCode::PageUp).key(KeyCode::PageDown).key(KeyCode::Esc);

    // With the mouse: the list, the input, a listed room, a person.
    tui.draw();
    let list = tui.app.regions.channel_list;
    tui.click(list.x + 2, list.y).click(list.x + 2, list.y + 1);
    let input = tui.app.regions.channel_input;
    tui.click_in(input).key(KeyCode::Esc);
    tui.key(KeyCode::Up).draw();
    if let Some((area, _)) = tui.app.regions.channel_rooms.iter().find(|(_, room)| room == "dev").cloned() {
        tui.click_in(area);
    }
    tui.app.channels.selected = Some(crate::app::channels::Target { hub, room: Some("lobby".into()) });
    tui.draw();
    for (area, _) in tui.app.regions.channel_users.clone() {
        tui.click_in(area);
    }
    if tui.app.channels.menu.is_some() {
        let popup = tui.app.regions.channel_popup;
        tui.click(popup.x + 1, popup.y + 3);
        tui.click(0, 0);
    }
    tui.key(KeyCode::Char('m')).draw();
    let popup = tui.app.regions.channel_popup;
    tui.click(popup.x + 1, popup.y);
    tui.click(0, HEIGHT - 1);
    if let Some((area, _)) = tui.app.regions.channel_links.first().cloned() {
        tui.click_in(area);
        tui.key(KeyCode::Esc);
    }

    // Leaving: x forgets the room; on the hub it asks first.
    tui.settle().tab(Tab::Channels);
    tui.app.channels.selected = Some(crate::app::channels::Target { hub, room: Some("lobby".into()) });
    tui.key(KeyCode::Char('x'));
    assert!(!tui.app.channels.hubs[0].buffers.contains_key("lobby"));
    tui.key(KeyCode::Char('c')).key(KeyCode::Char('c'));
    tui.key(KeyCode::Char('x')).answer("y");
    assert!(tui.app.channels.hubs.is_empty());
}

impl Tui {
    /// A prompt of this kind, answered.
    pub(crate) fn ask(&mut self, kind: crate::app::PromptKind, text: &str) -> &mut Self {
        self.app.prompt = Some(crate::app::Prompt { kind, title: "Asked".into(), input: crate::term::input::TextInput::with_text(text) });
        self.draw();
        self.key(KeyCode::Enter)
    }
}

#[test]
fn prompts_answered_and_pastes_wherever_they_go() {
    use crate::app::PromptKind;
    let mut tui = Tui::new("prompts");
    with_bob(&mut tui);
    let bob = "ab".repeat(16);
    let picture = tui.dir.join("not-a-qr.png");
    image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3])).save(&picture).unwrap();

    // Each kind, answered as asked (or not).
    tui.ask(PromptKind::NewConversation, "nonsense").ask(PromptKind::NewConversation, &bob);
    tui.ask(PromptKind::GoTo, "nonsense").ask(PromptKind::GoTo, &format!("{}:/page/index.mu", "cd".repeat(16)));
    tui.ask(PromptKind::FindPath, "nonsense").ask(PromptKind::FindPath, &bob);
    tui.ask(PromptKind::ReadPaper, "").ask(PromptKind::ReadPaper, "lxm://nonsense");
    tui.ask(PromptKind::ReadPaper, picture.to_str().unwrap()).ask(PromptKind::ReadPaper, "/no/such/file.png");
    tui.ask(PromptKind::ReadPaper, "lxma://nonsense");
    tui.ask(PromptKind::ContactName(bob.clone()), "Robert").ask(PromptKind::ContactNotes(bob.clone()), "met ↵ at the park");
    assert_eq!(tui.app.store.contacts[&bob].notes, "met\nat the park");
    tui.ask(PromptKind::ContactName("nope".into()), "x").ask(PromptKind::ContactNotes("nope".into()), "x");
    tui.ask(PromptKind::GuideName, "x");
    tui.ask(PromptKind::ConfirmDeleteMessage { key: bob.clone(), id: "nope".into() }, "y");
    tui.ask(PromptKind::ConfirmDeleteMessage { key: bob.clone(), id: "a".repeat(64) }, "yes");
    tui.ask(PromptKind::ConfirmBlock("nope".into()), "y");
    tui.ask(PromptKind::ConfirmStopLive(bob.clone()), "y");
    tui.ask(PromptKind::ConfirmInstallUpdate, "y");
    tui.ask(PromptKind::Attach, "").ask(PromptKind::Attach, picture.to_str().unwrap());
    tui.ask(PromptKind::DisplayName, "").ask(PromptKind::DisplayName, "Tui");
    tui.ask(PromptKind::AddHub, "").ask(PromptKind::AddHub, "nonsense");
    tui.ask(PromptKind::ConfirmHub { hash: [3; 16], aspect: "rrc".into(), room: Some("lobby".into()) }, "y");
    tui.ask(PromptKind::ConfirmSplit { hub: [3; 16], room: "lobby".into(), parts: vec!["a".into(), "b".into()] }, "y");
    tui.ask(PromptKind::ConfirmRemoveHub([3; 16]), "y");
    tui.ask(PromptKind::ConfirmDiscardPage("index.mu".into()), "y");
    tui.ask(PromptKind::ConfirmRestartRns, "y");
    assert!(tui.app.take_rns_restart());
    tui.ask(PromptKind::NewPage, "").ask(PromptKind::NewPage, "about");
    tui.ask(PromptKind::ConfirmBlock(bob.clone()), "y");
    assert_eq!(tui.app.store.contacts[&bob].trust, crate::store::Trust::Blocked);
    tui.ask(PromptKind::ConfirmDeleteConversation(bob.clone()), "y");
    assert!(!tui.app.store.conversations.contains_key(&bob));

    // Pastes go where the typing is.
    tui.settle();
    tui.app.on_paste("nowhere");
    assert!(tui.said().contains("Nothing to paste into"), "{}", tui.said());
    tui.tab(Tab::Messages);
    tui.app.on_paste("lxm://nonsense");
    tui.app.composing = true;
    tui.app.on_paste("composed");
    tui.settle().tab(Tab::Channels);
    tui.app.channels.typing = true;
    tui.app.on_paste("chat");
    tui.app.channels.typing = false;
    tui.app.channel_search = None;
    tui.key(KeyCode::Char('/'));
    tui.app.on_paste("searched");
    tui.key(KeyCode::Esc);
    tui.tab(Tab::Network).key(KeyCode::Char('/'));
    tui.app.on_paste("peer");
    tui.key(KeyCode::Esc);
    tui.tab(Tab::Browser);
    tui.app.browser.search.typing = true;
    tui.app.on_paste("library");
    tui.app.browser.search.typing = false;
    tui.tab(Tab::Messages).key(KeyCode::Char('/'));
    tui.app.on_paste("lunch");
    tui.settle();
    tui.app.open_emoji_picker();
    tui.app.on_paste("smile");
    tui.settle();
    tui.app.on_paste("x");

    // A restart: what was in flight is said to have stopped.
    tui.app.settings.node_enabled = true;
    tui.app.request_rns_restart();
    tui.app.request_rns_restart();
    assert!(tui.app.take_rns_restart() && !tui.app.take_rns_restart());
    tui.app.on_net(crate::net::NetEvent::StartFailed("no config".into()));
    assert!(tui.draw().contains("no config") || matches!(tui.app.net_state, crate::app::NetState::Failed(_)));
    tui.ctrl('l');
    assert!(tui.app.take_full_redraw() && !tui.app.take_full_redraw());
}

#[test]
fn overlays_keys_and_the_wheel_everywhere() {
    let mut tui = Tui::new("overlays");
    with_bob(&mut tui);
    tui.tab(Tab::Messages);
    let middle = (WIDTH / 2, HEIGHT / 2);

    // A QR code on screen: y copies its link, s saves it, a key or click
    // closes it.
    tui.app.paper_view = Some(crate::app::PaperView::new("lxm://abc".into()));
    tui.draw();
    tui.key(KeyCode::Char('y'));
    assert_eq!(tui.app.clipboard.last.as_deref(), Some("lxm://abc"));
    tui.key(KeyCode::Char('s')).key(KeyCode::Char('z'));
    assert!(tui.app.paper_view.is_none());
    tui.app.paper_view = Some(crate::app::PaperView::address("lxma://abc".into()));
    tui.key(KeyCode::Char('y')).click(middle.0, middle.1);
    assert!(tui.app.paper_view.is_none());

    // Writing: every key the composer has.
    tui.key(KeyCode::Enter).keys("hello");
    tui.ctrl('v').ctrl('p').ctrl('x').ctrl('o').key(KeyCode::Esc);
    tui.key(KeyCode::Enter).ctrl('r').key(KeyCode::Up).key(KeyCode::Down).key(KeyCode::Esc);
    tui.key(KeyCode::PageUp).key(KeyCode::PageDown).ctrl('e').ctrl('v').key(KeyCode::Esc).key(KeyCode::Esc);
    tui.settle();
    tui.app.prompt = Some(crate::app::Prompt {
        kind: crate::app::PromptKind::GoTo,
        title: "Go".into(),
        input: crate::term::input::TextInput::default(),
    });
    tui.ctrl('v').click(middle.0, middle.1).key(KeyCode::Esc);

    // The searches, forwarding, the map and the archive, by mouse.
    tui.settle().key(KeyCode::Char('/')).keys("Lunch").draw();
    let hits = tui.app.regions.message_search;
    tui.click(hits.x + 1, hits.y);
    tui.settle().key(KeyCode::Char('/')).keys("Lunch").draw();
    let search_box = tui.app.regions.message_search_box;
    tui.click(search_box.x + 1, search_box.y).click(0, HEIGHT - 1);
    assert!(tui.app.message_search.is_none());
    tui.settle().key(KeyCode::Char('M')).scroll(middle.0, middle.1, false).scroll(middle.0, middle.1, true);
    let places = tui.app.regions.map_list;
    tui.click(places.x + 1, places.y).click(0, 0);
    tui.settle().key(KeyCode::Char('H')).scroll(middle.0, middle.1, false).scroll(middle.0, middle.1, true).click(0, 0);
    tui.settle();
    tui.app.forward = None;
    tui.key(KeyCode::Char('m')).key(KeyCode::Char('f')).draw();
    if tui.app.forward.is_some() {
        let forward_box = tui.app.regions.forward_box;
        tui.click(forward_box.x + 1, forward_box.y);
        let list = tui.app.regions.forward_list;
        tui.click(list.x + 1, list.y);
    }
    tui.settle();
    tui.app.forward = None;
    tui.app.picked = None;
    tui.key(KeyCode::Char('m')).key(KeyCode::Char('f')).click(0, 0).settle();

    // The emoji picker under the wheel and a click.
    tui.key(KeyCode::Enter).ctrl('e').draw();
    let popup = tui.app.regions.emoji_popup;
    tui.scroll(popup.x + 1, popup.y + 1, true).scroll(popup.x + 1, popup.y + 1, false);
    if let Some((rect, _)) = tui.app.regions.emoji_hits.first().cloned() {
        tui.click_in(rect);
    }
    tui.ctrl('e').click(0, 0).settle();

    // The wheel in each tab, over its list and elsewhere.
    for tab in Tab::ALL {
        tui.settle().tab(tab);
        for (x, y) in [(2, 5), middle] {
            tui.scroll(x, y, true).scroll(x, y, false);
        }
    }
    tui.tab(Tab::Network).key(KeyCode::Char('a'));
    tui.scroll(middle.0, middle.1, true).scroll(middle.0, middle.1, false).key(KeyCode::Esc);
    tui.tab(Tab::Network).key(KeyCode::Char('/')).ctrl('v').key(KeyCode::Esc);
    tui.tab(Tab::Browser);
    tui.app.browser.search.typing = true;
    tui.ctrl('v');
    tui.app.browser.search.typing = false;
    tui.mouse(MouseEventKind::Down(MouseButton::Right), middle.0, middle.1);

    // Keys that work anywhere.
    tui.tab(Tab::Status);
    tui.key(KeyCode::Char('?')).key(KeyCode::Char('x'));
    tui.key(KeyCode::F(1)).click(0, 0);
    tui.key(KeyCode::Char('A')).key(KeyCode::Char('S')).key(KeyCode::Char('<')).key(KeyCode::Char('>'));
    assert!(tui.sent().iter().any(|c| matches!(c, NetCommand::Sync)));
    tui.draw();
    let version = tui.app.regions.version;
    tui.click_in(version);
    tui.key(KeyCode::Char('q'));
    assert!(tui.app.should_quit);
    tui.app.should_quit = false;
    tui.ctrl('c');
    assert!(tui.app.should_quit);
}

#[test]
fn the_node_editor_wrapping_its_preview_and_the_mouse() {
    let mut tui = Tui::with("node-wrap", |s| s.wrap_lines = true);
    let pages = tui.dir.join("node/pages");
    std::fs::create_dir_all(pages.join("blog")).unwrap();
    let long = "A line much longer than the editor is wide, so that it wraps onto rows of its own. ".repeat(6);
    std::fs::write(pages.join("index.mu"), format!(">Home\n{long}\n🦀 wide characters 🦀 too\n{}", "more\n".repeat(60))).unwrap();
    std::fs::write(pages.join("blog/first.mu"), "First").unwrap();
    image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])).save(pages.join("logo.png")).unwrap();
    std::fs::create_dir_all(tui.dir.join("node/files")).unwrap();
    std::fs::write(tui.dir.join("node/files/notes.txt"), "notes").unwrap();
    tui.tab(Tab::Node);

    // Every row of the list: Enter on each (a page opens, a picture or a
    // file says what it is), n in each (a new page in its folder).
    let rows = tui.app.node_rows().len();
    for row in 0..rows {
        tui.settle();
        tui.app.node.editing = false;
        tui.app.node.list.select(Some(row));
        tui.key(KeyCode::Enter);
        if tui.app.prompt.is_some() {
            tui.answer("y");
        }
        tui.app.node.editing = false;
        tui.app.node.list.select(Some(row));
        tui.key(KeyCode::Char('n')).key(KeyCode::Esc);
    }
    assert!(tui.said().contains("is a picture") || tui.said().contains("is a file"), "{}", tui.said());

    // The home page, wrapped, worked through by keys and the mouse.
    let home = tui.app.node_rows().iter().position(|row| tui.app.node_entry(row).is_some_and(|e| e.path == "index.mu")).unwrap();
    tui.settle();
    tui.app.node.list.select(Some(home));
    tui.key(KeyCode::Enter).key(KeyCode::Right);
    for _ in 0..4 {
        tui.key(KeyCode::Down).key(KeyCode::End);
    }
    tui.ctrl('v');
    let editor = tui.app.regions.node_editor;
    assert!(!editor.is_empty());
    tui.click(editor.x + 3, editor.y + 1);
    tui.drag((editor.x + 3, editor.y + 1), (editor.x + 10, editor.y + editor.height + 2));
    tui.drag((editor.x + 3, editor.y + 3), (editor.x + 10, editor.y.saturating_sub(1)));
    tui.scroll(editor.x + 2, editor.y + 2, true).scroll(editor.x + 2, editor.y + 2, false);
    // The preview alone: keys scroll it; formatting and pasting wait.
    while tui.app.node.view != crate::app::node::PageView::Preview {
        tui.ctrl('p');
    }
    for key in [KeyCode::Char('j'), KeyCode::Char('k'), KeyCode::Char(' '), KeyCode::Down, KeyCode::Up, KeyCode::Home] {
        tui.key(key);
    }
    tui.alt('b');
    tui.app.node.editing = true;
    tui.app.on_paste("not now");
    assert!(tui.said().contains("Switch to the editor"), "{}", tui.said());
    let preview = tui.app.regions.node_preview;
    tui.scroll(preview.x + 2, preview.y + 2, true).scroll(preview.x + 2, preview.y + 2, false);
    tui.ctrl('p').ctrl('p').key(KeyCode::Esc);
    tui.ask(crate::app::PromptKind::Format(crate::app::format::Action::Upload), "/no/such/file");
}

#[test]
fn the_hub_tab_running_with_people_rooms_and_bans() {
    use crate::net::NetEvent;
    use crate::rrc::host::{HubEvent, HubPerson, HubRoom, HubSnapshot};
    let mut tui = Tui::with("hub-tab", |s| s.hub_enabled = true);
    tui.tab(Tab::Hub);
    // Not running yet: what needs it says so.
    tui.key(KeyCode::Char('a')).key(KeyCode::Char('j'));
    tui.app.on_net(NetEvent::Hub(HubEvent::Failed("port taken".into())));
    tui.draw();
    tui.app.on_net(NetEvent::Hub(HubEvent::Started { hash: [7; 16], public_key: [1; 64] }));
    let carol = "cc".repeat(16);
    let room = |name: &str, members: Vec<String>| HubRoom {
        name: name.into(),
        registered: name == "lobby",
        modes: "+nt".into(),
        topic: Some("Hello".into()),
        members,
        founder: None,
        operators: vec![carol.clone()],
        voiced: Vec::new(),
        banned: vec!["dd".repeat(16)],
        key: None,
        invited: Vec::new(),
    };
    let snapshot = HubSnapshot {
        people: vec![HubPerson {
            identity: carol.clone(),
            nick: Some("carol".into()),
            rooms: vec!["lobby".into()],
            since: 1,
            links: 1,
            operator: false,
        }],
        rooms: vec![room("lobby", vec![carol.clone()]), room("dev", vec![carol.clone(), "ee".repeat(16)])],
        klines: vec!["ff".repeat(16)],
        ..HubSnapshot::default()
    };
    tui.app.on_net(NetEvent::Hub(HubEvent::State(snapshot)));
    tui.app.on_net(NetEvent::Hub(HubEvent::Reply { text: "Stats\nmore".into(), error: false }));
    tui.app.on_net(NetEvent::Hub(HubEvent::Reply { text: "No".into(), error: true }));
    assert!(tui.draw().contains("carol"));

    // Each pane, its keys, and each prompt answered.
    use crate::app::PromptKind::Hub;
    use crate::app::hub::HubPrompt;
    tui.key(KeyCode::Char('y')).key(KeyCode::Char('a'));
    tui.key(KeyCode::Char(':')).answer("/stats");
    tui.key(KeyCode::Char('n')).answer("garden");
    for key in ['K', 'b', 'o', 'v'] {
        tui.key(KeyCode::Char(key)).answer("lobby");
    }
    tui.key(KeyCode::Char('B')).answer("y");
    tui.key(KeyCode::Char('d'));
    tui.key(KeyCode::Tab);
    tui.key(KeyCode::Char('t')).answer("New topic");
    tui.key(KeyCode::Char('m')).answer("+m");
    tui.key(KeyCode::Char('r')).key(KeyCode::Char('j')).key(KeyCode::Char('r')).key(KeyCode::Char('k'));
    tui.key(KeyCode::Char('R')).answer("hall");
    tui.key(KeyCode::Down).key(KeyCode::Char('D')).answer("y");
    tui.key(KeyCode::Char('D')).answer("n");
    tui.key(KeyCode::Right);
    tui.key(KeyCode::Char('x')).key(KeyCode::Down).key(KeyCode::Delete);
    tui.key(KeyCode::Left).key(KeyCode::BackTab).key(KeyCode::Char('h')).key(KeyCode::Char('l'));
    for prompt in [HubPrompt::Op(carol.clone()), HubPrompt::Voice(carol.clone()), HubPrompt::ConfirmKline(carol.clone())] {
        tui.ask(Hub(prompt.clone()), "dev").ask(Hub(prompt), "");
    }
    tui.ask(Hub(HubPrompt::ConfirmDelete("dev".into())), "no").ask(Hub(HubPrompt::Command), "/");
    assert!(tui.sent().iter().any(|c| matches!(c, NetCommand::HubAction(_))));

    // The mouse: each pane's heading, a row, the wheel.
    tui.draw();
    for (area, _) in tui.app.regions.hub_panes.clone() {
        tui.click_in(area);
    }
    let list = tui.app.regions.hub_list;
    tui.click(list.x + 1, list.y).scroll(list.x + 1, list.y, true).scroll(list.x + 1, list.y, false);
    // Stopped: its last state isn't shown; started again, then off by e.
    tui.app.on_net(NetEvent::Hub(HubEvent::Stopped));
    tui.app.on_net(NetEvent::Hub(HubEvent::State(HubSnapshot::default())));
    tui.key(KeyCode::Char('e')).key(KeyCode::Char('e'));
}
