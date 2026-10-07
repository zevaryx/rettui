//! The keys of each mode, the most used first: the footer shows those that
//! fit, and `?` (or F1, which works while typing too) lists them all.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{accent, dim, block};
use crate::app::node::PageView;
use crate::app::{App, BrowserFocus, BrowserPane, Tab};

type KeyList = &'static [(&'static str, &'static str)];

/// One mode's keys.
pub struct Keys {
    /// What the mode is, for the list's title.
    pub title: &'static str,
    pub keys: KeyList,
    /// Keys go to a text box or a dialog here, `?` too: F1 lists them.
    pub typing: bool,
}

/// Keys that work in every tab, unless typing.
const EVERYWHERE: KeyList = &[("1–7", "tabs"), ("A", "announce"), ("S", "sync"), ("^L", "redraw"), ("q", "quit"), ("?", "keys")];
/// Keys that work while typing, too.
const ALWAYS: KeyList = &[("F1", "keys"), ("^L", "redraw"), ("^C", "quit")];

fn mode(title: &'static str, keys: KeyList) -> Keys {
    Keys { title, keys, typing: false }
}

fn typing(title: &'static str, keys: KeyList) -> Keys {
    Keys { title, keys, typing: true }
}

/// The keys of the mode on screen.
pub fn keys(app: &App) -> Keys {
    if app.prompt.is_some() {
        return typing("Prompt", &[("Enter", "ok"), ("Esc", "cancel"), ("^V", "paste")]);
    }
    if app.paper_view.is_some() {
        return typing("QR code", &[("y", "copy link"), ("s", "save"), ("Esc", "close")]);
    }
    if app.guide.is_some() {
        return typing("Getting started", &[("Enter", "change"), ("↑↓", "choose"), ("y", "copy link"), ("Esc", "close")]);
    }
    match app.tab {
        Tab::Channels if app.channels.typing && app.mention_matches().is_some() => {
            typing("Mentioning", &[("Tab/Enter", "mention"), ("↑↓", "choose"), ("Esc", "close")])
        }
        _ if app.emoji.is_some() => typing(
            "Emoji",
            &[("Enter", "insert"), ("arrows", "choose"), ("Tab", "group"), ("type", "search"), ("Esc", "close")],
        ),
        _ if app.shortcode_matches().is_some() => {
            typing("Emoji by name", &[("Tab/Enter", "pick"), ("↑↓", "choose"), ("Esc", "close")])
        }
        Tab::Channels if app.channels.typing => typing(
            "Writing in a channel",
            &[
                ("Enter", "send"),
                ("Esc", "done"),
                ("@", "mention"),
                ("^E", "emoji"),
                ("/help", "commands"),
                ("^V", "paste"),
                ("PgUp/Dn", "scroll"),
            ],
        ),
        Tab::Channels if app.channels.menu.is_some() => typing(
            "User menu",
            &[("Enter", "do"), ("w", "whisper"), ("l", "LXMF"), ("↑↓", "select"), ("Esc", "close")],
        ),
        Tab::Channels if app.channels.picker.is_some() => {
            typing("Members", &[("Enter", "pick"), ("↑↓", "select"), ("Esc", "close")])
        }
        Tab::Channels => mode(
            "Channels",
            &[
                ("Enter", "write"),
                ("m", "message"),
                ("n", "add hub"),
                ("c", "connect"),
                ("a", "auto"),
                ("J", "joins"),
                ("N", "notify"),
                ("x", "remove"),
                ("y", "copy link"),
                ("↑↓", "select"),
                ("PgUp/Dn", "scroll"),
                ("Home/End", "first/last"),
            ],
        ),
        Tab::Messages if app.message_search.is_some() => typing(
            "Search messages",
            &[("Enter", "open"), ("↑↓", "select"), ("Tab", "here/all"), ("Esc", "close"), ("^V", "paste")],
        ),
        Tab::Messages if app.forward.is_some() => typing(
            "Forward to",
            &[("Enter", "forward"), ("↑↓", "select"), ("Esc", "cancel"), ("^V", "paste")],
        ),
        Tab::Messages if app.map.is_some() => mode(
            "Map",
            &[
                ("↑↓", "pick"),
                ("Enter", "open"),
                ("+/-", "zoom"),
                ("0", "all"),
                ("o", "OpenStreetMap"),
                ("Esc", "close"),
            ],
        ),
        Tab::Messages if app.archive_reader.is_some() => mode(
            "Archive",
            &[("↑↓", "scroll"), ("PgUp/Dn", "page"), ("Home/End", "oldest/newest"), ("Esc", "close")],
        ),
        Tab::Messages if app.composing && app.reply.is_some() => typing(
            "Replying",
            &[
                ("Enter", "send"),
                ("↑↓", "other"),
                ("Esc", "no reply"),
                ("^E", "emoji"),
                ("^O", "attach"),
                ("^V", "paste"),
            ],
        ),
        Tab::Messages if app.composing => typing(
            "Writing",
            &[
                ("Enter", "send"),
                ("Esc", "done"),
                ("^R", "reply"),
                ("^E", "emoji"),
                ("^O", "attach"),
                ("^V", "paste"),
                ("^X", "detach"),
                ("^P", "delivery"),
                (":name", "emoji"),
                ("PgUp/Dn", "scroll"),
            ],
        ),
        Tab::Messages if app.contact_card.is_some() => typing(
            "Contact",
            &[
                ("r", "rename"),
                ("e", "notes"),
                ("t", "trust"),
                ("b", "block"),
                ("y", "copy"),
                ("l", "leave as is"),
                ("X", "delete"),
                ("Esc", "close"),
            ],
        ),
        Tab::Messages if app.picked.is_some() => mode(
            "Picked message",
            &[
                ("r", "reply"),
                ("e", "react"),
                ("y", "copy"),
                ("f", "forward"),
                ("o", "open"),
                ("t", "retry"),
                ("x", "delete"),
                ("↑↓", "other"),
                ("Esc", "done"),
            ],
        ),
        Tab::Messages => mode(
            "Messages",
            &[
                ("Enter", "write"),
                ("/", "search"),
                ("r", "reply"),
                ("m", "pick"),
                ("c", "contact"),
                ("n", "new"),
                ("y", "copy"),
                ("a", "attach"),
                ("o", "open"),
                ("d", "delivery"),
                ("p", "read paper"),
                ("P", "show paper"),
                ("N", "notify"),
                ("H", "archive"),
                ("L", "share location"),
                ("M", "map"),
                ("*", "pin"),
                ("R", "all read"),
                ("E", "export"),
                ("X", "delete"),
                ("↑↓", "select"),
                ("PgUp/Dn", "scroll"),
                ("Home/End", "first/last"),
            ],
        ),
        Tab::Network if app.net_search.typing => {
            typing("Network search", &[("Enter", "done"), ("Esc", "clear"), ("↑↓", "select"), ("^V", "paste")])
        }
        Tab::Network if !app.net_search.input.text().is_empty() => mode(
            "Network, searched",
            &[
                ("Enter", "open"),
                ("P", "path"),
                ("T", "probe"),
                ("Esc", "clear"),
                ("/", "edit"),
                ("y", "copy"),
                ("p", "sync via"),
                ("b", "block"),
                ("f", "filter"),
                ("s", "sort"),
                ("i", "interface"),
                ("D", "forget path"),
                ("↑↓", "select"),
            ],
        ),
        Tab::Network => mode(
            "Network",
            &[
                ("Enter", "open"),
                ("/", "search"),
                ("P", "path"),
                ("T", "probe"),
                ("y", "copy"),
                ("p", "sync via"),
                ("b", "block"),
                ("f", "filter"),
                ("s", "sort"),
                ("i", "interface"),
                ("D", "forget path"),
                ("↑↓", "select"),
            ],
        ),
        Tab::Browser if app.browser.find.is_some() => {
            typing("Find in page", &[("Enter", "next"), ("↑↓", "previous/next"), ("Esc", "done"), ("^V", "paste")])
        }
        Tab::Browser if app.browser.search.typing => {
            typing("Browser search", &[("Enter", "done"), ("Esc", "clear"), ("↑↓", "select"), ("^V", "paste")])
        }
        // A search in effect: what's for the rows found comes first.
        Tab::Browser
            if app.browser.focus == BrowserFocus::Pane
                && !app.browser.search.input.text().is_empty()
                && app.browser.pane == BrowserPane::Nodes =>
        {
            mode(
                "Browser, nodes searched",
                &[
                    ("Enter", "open"),
                    ("s", "save"),
                    ("Esc", "clear"),
                    ("/", "edit"),
                    ("t", "saved/nodes"),
                    ("→", "page"),
                    ("g", "go to"),
                    ("↑↓", "select"),
                ],
            )
        }
        Tab::Browser if app.browser.focus == BrowserFocus::Pane && !app.browser.search.input.text().is_empty() => mode(
            "Browser, saved searched",
            &[
                ("Enter", "open"),
                ("x", "remove"),
                ("Esc", "clear"),
                ("/", "edit"),
                ("t", "saved/nodes"),
                ("→", "page"),
                ("g", "go to"),
                ("↑↓", "select"),
            ],
        ),
        Tab::Browser if app.browser.focus == BrowserFocus::Pane && app.browser.pane == BrowserPane::Nodes => mode(
            "Browser, nodes",
            &[
                ("Enter", "open"),
                ("/", "search"),
                ("s", "save"),
                ("t", "saved/nodes"),
                ("→", "page"),
                ("g", "go to"),
                ("y", "copy"),
                ("b", "back"),
                ("r", "refresh"),
                ("H", "home"),
                ("R", "clear cache"),
                ("↑↓", "select"),
            ],
        ),
        Tab::Browser if app.browser.focus == BrowserFocus::Pane => mode(
            "Browser, saved",
            &[
                ("Enter", "open"),
                ("/", "search"),
                ("x", "remove"),
                ("t", "saved/nodes"),
                ("→", "page"),
                ("g", "go to"),
                ("y", "copy"),
                ("b", "back"),
                ("r", "refresh"),
                ("H", "home"),
                ("R", "clear cache"),
                ("↑↓", "select"),
            ],
        ),
        Tab::Browser if app.browser.view_source => mode(
            "Browser, source",
            &[
                ("u", "page"),
                ("f", "find"),
                ("Y", "copy source"),
                ("y", "copy"),
                ("r", "refresh"),
                ("b", "back"),
                ("←", "list"),
                ("/", "search"),
                ("drag", "copy text"),
                ("↑↓", "scroll"),
            ],
        ),
        Tab::Browser => mode(
            "Browser",
            &[
                ("Tab", "next"),
                ("Enter", "open"),
                ("f", "find"),
                ("b", "back"),
                ("r", "refresh"),
                ("s", "save"),
                ("g", "go to"),
                ("y", "copy"),
                ("Y", "copy page"),
                ("L", "copy link"),
                ("u", "source"),
                ("I", "identify"),
                ("H", "home"),
                ("←", "list"),
                ("/", "search"),
                ("R", "clear cache"),
                ("drag", "copy text"),
                ("^V", "paste"),
                ("↑↓", "scroll"),
                ("PgUp/Dn", "page"),
            ],
        ),
        Tab::Node if app.node.editing && app.node.editor.is_some() && app.node.view == PageView::Preview => typing(
            "Preview",
            &[("^S", "save"), ("Esc", "pages"), ("^P", "view"), ("↑↓", "scroll"), ("PgUp/Dn", "page")],
        ),
        Tab::Node if app.node.editing && app.node.editor.is_some() => typing(
            "Editing a page",
            &[
                ("^S", "save"),
                ("Esc", "pages"),
                ("Alt+key", "format"),
                ("^P", "view"),
                ("^Z/^Y", "undo/redo"),
                ("Shift+←→", "select"),
                ("^A", "all"),
                ("^V", "paste"),
            ],
        ),
        Tab::Node => mode(
            "Node",
            &[
                ("Enter", "edit"),
                ("n", "new"),
                ("r", "rename"),
                ("x", "delete"),
                ("h", "host"),
                ("a", "announce"),
                ("b", "browse"),
                ("y", "copy"),
                ("p", "view"),
                ("↑↓", "select"),
            ],
        ),
        Tab::Reticulum if app.rns.picker.is_some() => {
            typing("Choose", &[("Enter", "pick"), ("↑↓", "select"), ("Esc", "cancel")])
        }
        Tab::Reticulum if app.rns.editor.is_some() => typing(
            "Config as text",
            &[("^S", "save"), ("Esc", "close"), ("^Z/^Y", "undo/redo"), ("^V", "paste")],
        ),
        Tab::Reticulum => mode(
            "Reticulum",
            &[
                ("Enter", "edit"),
                ("Tab", "section"),
                ("a", "add"),
                ("Space", "on/off"),
                ("r", "rename"),
                ("x", "delete"),
                ("d", "default"),
                ("t", "as text"),
                ("R", "reload"),
                ("^R", "restart"),
                ("↑↓", "select"),
            ],
        ),
        Tab::Status if !app.first_steps().is_empty() => mode(
            "Status",
            &[
                ("Enter", "edit"),
                ("e", "name"),
                ("y", "copy"),
                ("c", "QR code"),
                ("g", "guide"),
                ("x", "hide steps"),
                ("b", "back up"),
                ("i", "identity"),
                ("^R", "restart"),
                ("↑↓", "select"),
            ],
        ),
        Tab::Status => mode(
            "Status",
            &[
                ("Enter", "edit"),
                ("e", "name"),
                ("y", "copy"),
                ("c", "QR code"),
                ("g", "guide"),
                ("b", "back up"),
                ("i", "identity"),
                ("^R", "restart"),
                ("↑↓", "select"),
            ],
        ),
    }
}

fn keycap(key: &str) -> Span<'static> {
    Span::styled(key.to_string(), Style::default().fg(Color::Black).bg(dim()))
}

/// One hint's width: the key, a space, what it does, and two spaces.
fn hint_width(key: &str, action: &str) -> usize {
    key.width() + 1 + action.width() + 2
}

/// The footer: as many of the mode's keys as fit whole, and at the right
/// edge, how to list them all.
pub(super) fn draw_hints(frame: &mut Frame, app: &App, area: Rect) {
    let keys = keys(app);
    let more = if keys.typing { "F1" } else { "?" };
    let room = (area.width as usize).saturating_sub(hint_width(more, "keys"));
    let mut spans = Vec::new();
    let mut used = 0;
    for (key, action) in keys.keys {
        used += hint_width(key, action);
        if used > room {
            break;
        }
        spans.push(keycap(key));
        spans.push(Span::raw(format!(" {action}  ")));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    let all = Line::from(vec![keycap(more), Span::styled(" keys", Style::default().fg(dim()))]).right_aligned();
    frame.render_widget(Paragraph::new(all), area);
}

/// Every key of the mode on screen, and those that work everywhere, in
/// columns as the screen allows. Any key or click closes it.
pub(super) fn draw_keys(frame: &mut Frame, app: &App) {
    let keys = keys(app);
    let everywhere = if keys.typing { ALWAYS } else { EVERYWHERE };
    let all = keys.keys.iter().chain(everywhere);
    let key_width = all.clone().map(|(key, _)| key.width()).max().unwrap_or(0);
    let action_width = all.map(|(_, action)| action.width()).max().unwrap_or(0);
    let column = key_width + 1 + action_width + 3;
    let area = frame.area();
    let inner_room = (area.width as usize).saturating_sub(6);
    let columns = (inner_room / column).clamp(1, 3);
    let rows = |list: KeyList| -> Vec<Line<'static>> {
        list.chunks(list.len().div_ceil(columns).max(1))
            .fold(Vec::<Vec<Span<'static>>>::new(), |mut lines, chunk| {
                for (row, (key, action)) in chunk.iter().enumerate() {
                    if lines.len() <= row {
                        lines.push(Vec::new());
                    }
                    let pad = key_width - key.width();
                    lines[row].push(Span::styled(format!("{}{key}", " ".repeat(pad)), Style::default().fg(accent()).bold()));
                    lines[row].push(Span::raw(format!(" {action:<action_width$}   ")));
                }
                lines
            })
            .into_iter()
            .map(Line::from)
            .collect()
    };
    let mut lines = rows(keys.keys);
    lines.push(Line::default());
    let heading = if keys.typing { "While typing, too" } else { "In every tab" };
    lines.push(Line::from(Span::styled(heading, Style::default().fg(dim()))));
    lines.extend(rows(everywhere));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Any key closes this list", Style::default().fg(dim()))));

    let width = (column * columns + 4).min(area.width as usize) as u16;
    let height = (lines.len() + 2).min(area.height as usize) as u16;
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let title = format!("Keys · {}", keys.title);
    frame.render_widget(Paragraph::new(lines).block(block(&title, true).padding(ratatui::widgets::Padding::horizontal(1))), rect);
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::{App, Tab};

    fn screen(app: &mut App, width: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..24).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect()).collect()
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn footers_show_whole_hints_and_how_to_see_the_rest() {
        let dir = std::env::temp_dir().join(format!("rettui-keys-footer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        for width in [60, 80, 120, 200] {
            for tab in Tab::ALL {
                app.tab = tab;
                let footer = screen(&mut app, width).pop().unwrap();
                // Right of the sidebar: the first hints, each whole, then
                // how to see the rest.
                let hints = footer.split('│').nth(1).unwrap().trim_end();
                let hints = hints.strip_suffix("? keys").unwrap_or_else(|| panic!("{width} {tab:?}: {footer}")).trim_end();
                let first = super::keys(&app).keys.iter().map(|(key, action)| format!("{key} {action}"));
                let whole = first.scan(String::new(), |line, hint| {
                    if !line.is_empty() {
                        line.push_str("  ");
                    }
                    line.push_str(&hint);
                    Some(line.clone())
                });
                assert!(whole.into_iter().any(|line| line == hints), "{width} {tab:?}: {footer}");
            }
        }
        // The new keys show at 80 columns.
        app.tab = Tab::Network;
        let footer = screen(&mut app, 80).pop().unwrap();
        assert!(footer.contains("P path  T probe"), "{footer}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_list_of_keys_opens_with_a_question_mark_or_f1() {
        let dir = std::env::temp_dir().join(format!("rettui-keys-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        app.tab = Tab::Network;
        press(&mut app, KeyCode::Char('?'));
        let shown = screen(&mut app, 100).join("\n");
        assert!(shown.contains("Keys · Network") && shown.contains("D forget path") && shown.contains("In every tab"), "{shown}");
        assert!(shown.contains("q quit") && shown.contains("1–7 tabs"), "{shown}");
        // Any key closes it, and does nothing else.
        press(&mut app, KeyCode::Char('f'));
        assert!(!app.keys_help);
        assert_eq!(app.net_filter, crate::app::NetFilter::All);
        // While typing, ? is typed; F1 lists the keys, with those that
        // work while typing.
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.net_search.input.text(), "?");
        assert!(!app.keys_help);
        let footer = screen(&mut app, 100).pop().unwrap();
        assert!(footer.trim_end().ends_with("F1 keys"), "{footer}");
        press(&mut app, KeyCode::F(1));
        let shown = screen(&mut app, 100).join("\n");
        assert!(shown.contains("Keys · Network search") && shown.contains("While typing, too") && shown.contains("^C quit"), "{shown}");
        // A click closes it too.
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 1, row: 1, modifiers: KeyModifiers::NONE });
        assert!(!app.keys_help);
        assert_eq!(app.net_search.input.text(), "?");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_browser_hints_searching_and_saving_nodes() {
        let dir = std::env::temp_dir().join(format!("rettui-keys-browser-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        app.tab = Tab::Browser;
        app.browser.pane = crate::app::BrowserPane::Nodes;
        let footer = |app: &mut App| screen(app, 80).pop().unwrap();
        let nodes = footer(&mut app);
        assert!(nodes.contains("Enter open  / search  s save") && nodes.trim_end().ends_with("? keys"), "{nodes}");
        app.browser.pane = crate::app::BrowserPane::Saved;
        assert!(footer(&mut app).contains("/ search  x remove"));
        // While typing, ? is typed: F1 lists the keys.
        press(&mut app, KeyCode::Char('/'));
        let typing = footer(&mut app);
        assert!(typing.contains("Enter done  Esc clear") && typing.trim_end().ends_with("F1 keys"), "{typing}");
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.browser.search.input.text(), "?");
        // A search in effect: saving or removing what was found, and
        // clearing it.
        press(&mut app, KeyCode::Enter);
        assert!(footer(&mut app).contains("Enter open  x remove  Esc clear  / edit"));
        app.browser.pane = crate::app::BrowserPane::Nodes;
        assert!(footer(&mut app).contains("Enter open  s save  Esc clear  / edit"));
        press(&mut app, KeyCode::Char('?'));
        assert!(screen(&mut app, 100).join("\n").contains("Keys · Browser, nodes searched"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
