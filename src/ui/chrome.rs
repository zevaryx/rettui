//! Around the tabs: the navigation sidebar, the key-hint footer and the
//! prompt dialog.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{ACCENT, DIM, SELECTED_BG, block};
use crate::app::traffic::rate;
use crate::app::{App, BrowserFocus, NetState, NoticeKind, SyncState, Tab};
use crate::app::node::PageView;

/// The sidebar's width: full, or a rail of icons on narrow terminals.
pub(super) fn sidebar_width(total: u16) -> u16 {
    if total >= 110 { 22 } else { 6 }
}

/// Vertical navigation rail, chat-client style. On narrow terminals it is
/// icons only, with a dot for unread messages.
pub(super) fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let rail = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(DIM));
    let inner = rail.inner(area);
    frame.render_widget(rail, area);
    let width = inner.width as usize;
    let compact = width < 10;

    app.regions.version = Rect::default();
    // The logo: `rettui>_` with a cyan prompt, or just the prompt on the rail.
    let prompt = Span::styled(">_", Style::default().fg(ACCENT).bold());
    let brand = if compact {
        Line::from(vec![Span::raw(" "), prompt])
    } else {
        let mut spans = vec![Span::styled(" rettui", Style::default().bold()), prompt];
        // The version when it fits (" rettui>_ " is 10 columns); clicking
        // it opens the project page.
        let version = env!("CARGO_PKG_VERSION");
        if width >= 10 + 1 + version.len() {
            spans.push(Span::styled(format!(" {version}"), Style::default().fg(DIM)));
            app.regions.version = Rect::new(inner.x + 10, inner.y, version.len() as u16, 1);
        }
        Line::from(spans)
    };
    frame.render_widget(Paragraph::new(brand), Rect { height: 1, ..inner });
    // Tabs a row apart when there is room, else packed.
    let spacing = if inner.height >= 2 + Tab::ALL.len() as u16 * 2 + 3 { 2 } else { 1 };

    let unread: usize = app.store.conversations.values().map(|c| c.unread).sum();
    let (channel_unread, mentioned) = app.channels.total_unread();
    app.regions.tabs.clear();
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let y = inner.y + 2 + i as u16 * spacing;
        if y >= inner.bottom() {
            break;
        }
        let row = Rect::new(inner.x, y, inner.width, 1);
        app.regions.tabs.push((row, *tab));
        // No emoji-capable symbols (✉, ⚙): some terminals draw those two
        // columns wide, which shifts the rest of the row.
        let icon = match tab {
            Tab::Messages => "✎",
            Tab::Channels => "#",
            Tab::Network => "◎",
            Tab::Browser => "◈",
            Tab::Node => "⌂",
            Tab::Status => "ⓘ",
            Tab::Reticulum => "⛭",
        };
        let selected = *tab == app.tab;
        let base = if selected {
            Style::default().bg(SELECTED_BG).bold()
        } else {
            Style::default()
        };
        let mut spans = vec![
            Span::styled(if selected { "▌" } else { " " }, base.fg(ACCENT)),
            Span::styled(if compact { format!(" {icon} ") } else { format!(" {icon}  {}", tab.title()) }, base),
        ];
        let badge = match tab {
            Tab::Messages if unread > 0 => Some(format!(" {unread} ")),
            Tab::Channels if channel_unread > 0 => Some(format!(" {channel_unread} ")),
            _ => None,
        };
        // Mentions stand out from ordinary unread chat.
        let badge_bg = if *tab == Tab::Channels && mentioned { Color::LightRed } else { Color::Yellow };
        if compact {
            if badge.is_some() {
                spans.push(Span::styled("●", base.fg(badge_bg)));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)).style(base), row);
            continue;
        }
        let used: usize = spans.iter().map(Span::width).sum::<usize>()
            + badge.as_ref().map_or(0, |b| b.width() + 1);
        spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), base));
        if let Some(badge) = badge {
            spans.push(Span::styled(badge, Style::default().fg(Color::Black).bg(badge_bg).bold()));
            spans.push(Span::styled(" ", base));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), row);
    }

    // Identity and connection status at the bottom of the rail.
    if compact {
        let color = match &app.net_state {
            NetState::Starting => Color::Yellow,
            NetState::Online if app.interfaces.iter().any(|i| i.online) => Color::Green,
            NetState::Online => Color::Yellow,
            NetState::Failed(_) => Color::Red,
        };
        if inner.height > spacing * Tab::ALL.len() as u16 + 3 {
            let area = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
            frame.render_widget(Paragraph::new(Span::styled(" ●", Style::default().fg(color))), area);
        }
        return;
    }
    let mut status = vec![Line::styled(
        format!(" {}", app.settings.display_name),
        Style::default().fg(DIM),
    )];
    status.push(Line::from(match &app.net_state {
        NetState::Starting => Span::styled(" ● starting", Style::default().fg(Color::Yellow)),
        NetState::Online => {
            let online = app.interfaces.iter().filter(|i| i.online).count();
            let color = if online > 0 { Color::Green } else { Color::Yellow };
            Span::styled(
                format!(" ● {online}/{} interfaces", app.interfaces.len()),
                Style::default().fg(color),
            )
        }
        NetState::Failed(_) => Span::styled(" ● offline", Style::default().fg(Color::Red)),
    }));
    // Traffic over all interfaces, once there is a rate to show.
    if let (NetState::Online, Some((rx, tx))) = (&app.net_state, app.traffic.rates) {
        status.push(Line::from(vec![
            Span::styled(" ↓", Style::default().fg(Color::Green)),
            Span::styled(rate(rx), Style::default().fg(DIM)),
            Span::styled(" ↑", Style::default().fg(ACCENT)),
            Span::styled(rate(tx), Style::default().fg(DIM)),
        ]));
    }
    if let SyncState::Running(_) = app.sync {
        status.push(Line::styled(" ⇅ syncing", Style::default().fg(Color::Yellow)));
    }
    let height = status.len() as u16;
    if inner.height > height + 10 {
        let area = Rect::new(inner.x, inner.bottom() - height, inner.width, height);
        frame.render_widget(Paragraph::new(status), area);
    }
}

pub(super) fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    // Problems stay up a little longer than confirmations.
    if let Some(notice) = &app.notice
        && notice.at.elapsed() < std::time::Duration::from_secs(if notice.kind == NoticeKind::Done { 3 } else { 6 })
    {
        let (mark, style) = match notice.kind {
            NoticeKind::Done => (" ✓ ", Style::default().fg(Color::Black).bg(Color::Green)),
            NoticeKind::Warn => (" ! ", Style::default().fg(Color::Black).bg(Color::Yellow).bold()),
            NoticeKind::Error => (" ✗ ", Style::default().fg(Color::White).bg(Color::Red).bold()),
        };
        let text_style = if notice.kind == NoticeKind::Error { Style::default().fg(Color::LightRed) } else { Style::default() };
        let line = Line::from(vec![Span::styled(mark, style), Span::styled(format!(" {}", notice.text), text_style)]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let hints: &[(&str, &str)] = if app.prompt.is_some() {
        &[("Enter", "confirm"), ("Esc", "cancel"), ("^V", "paste")]
    } else if app.paper_view.is_some() {
        &[("y", "copy link"), ("s", "save image"), ("Esc", "close")]
    } else if app.guide.is_some() {
        &[("↑↓", "choose"), ("Enter", "change / open"), ("y", "copy link"), ("Esc", "close")]
    } else {
        match app.tab {
            Tab::Channels if app.channels.typing && app.mention_matches().is_some() => &[
                ("↑↓", "choose"),
                ("Tab/Enter", "mention"),
                ("Esc", "close list"),
            ],
            _ if app.emoji.is_some() => &[
                ("Enter", "insert"),
                ("arrows", "choose"),
                ("Tab", "group"),
                ("type", "search"),
                ("Esc", "close"),
            ],
            _ if app.shortcode_matches().is_some() => &[("↑↓", "choose"), ("Tab/Enter", "pick"), ("Esc", "close list")],
            Tab::Channels if app.channels.typing => &[
                ("Enter", "send"),
                ("Esc", "done"),
                ("@", "mention"),
                ("^E", "emoji"),
                ("/help", "commands"),
                ("^V", "paste"),
                ("PgUp/PgDn", "scroll"),
            ],
            Tab::Channels if app.channels.menu.is_some() => &[
                ("↑↓", "select"),
                ("Enter", "do"),
                ("w", "whisper"),
                ("l", "LXMF"),
                ("Esc", "close"),
            ],
            Tab::Channels if app.channels.picker.is_some() => &[("↑↓", "select"), ("Enter", "pick"), ("Esc", "close")],
            Tab::Channels => &[
                ("↑↓", "select"),
                ("Enter", "write"),
                ("m", "message user"),
                ("n", "add hub"),
                ("c", "connect"),
                ("a", "auto-connect"),
                ("J", "joins/leaves"),
                ("N", "notifications"),
                ("x", "remove"),
                ("y", "copy link"),
            ],
            Tab::Messages if app.composing && app.reply.is_some() => &[
                ("Enter", "send reply"),
                ("↑↓", "another message"),
                ("Esc", "cancel reply"),
                ("^E", "emoji"),
                ("^V", "paste"),
                ("^O", "attach"),
            ],
            Tab::Messages if app.composing => &[
                ("Enter", "send"),
                ("Esc", "done"),
                ("^R", "reply"),
                ("^E", "emoji"),
                ("^V", "paste"),
                ("^O", "attach"),
                ("^X", "clear files"),
                ("^P", "delivery mode"),
                ("PgUp/PgDn", "scroll"),
            ],
            Tab::Messages if app.contact_card.is_some() => &[
                ("r", "rename"),
                ("e", "notes"),
                ("t", "trust"),
                ("b", "block"),
                ("y", "copy address"),
                ("X", "delete conversation"),
                ("Esc", "close"),
            ],
            Tab::Messages if app.picked.is_some() => &[
                ("↑↓", "another message"),
                ("r", "reply"),
                ("e", "react"),
                ("y", "copy"),
                ("o", "open"),
                ("t", "retry"),
                ("x", "delete"),
                ("Esc", "done"),
            ],
            Tab::Messages => &[
                ("Enter", "write"),
                ("r", "reply"),
                ("m", "pick a message"),
                ("c", "contact"),
                ("n", "new"),
                ("y", "copy address"),
                ("a", "attach"),
                ("o", "open file"),
                ("d", "delivery"),
                ("p", "read paper"),
                ("P", "show paper"),
                ("N", "notifications"),
                ("S", "sync"),
                ("A", "announce"),
                ("q", "quit"),
            ],
            Tab::Network if app.net_search.typing => &[
                ("Enter", "done"),
                ("Esc", "clear"),
                ("↑↓", "select"),
                ("^V", "paste"),
            ],
            Tab::Network if !app.net_search.input.text().is_empty() => &[
                ("Enter", "open"),
                ("/", "edit search"),
                ("Esc", "clear search"),
                ("y", "copy address"),
                ("p", "use as propagation node"),
                ("f", "filter"),
            ],
            Tab::Network => &[
                ("/", "search"),
                ("Enter", "open"),
                ("y", "copy address"),
                ("p", "use as propagation node"),
                ("b", "block/unblock"),
                ("f", "filter"),
                ("S", "sync"),
                ("A", "announce"),
                ("q", "quit"),
            ],
            Tab::Browser if app.browser.search.typing => &[
                ("Enter", "done"),
                ("Esc", "clear"),
                ("↑↓", "select"),
                ("^V", "paste"),
            ],
            Tab::Browser if app.browser.focus == BrowserFocus::Pane => &[
                ("↑↓", "select"),
                ("Enter", "open"),
                ("t", "saved/nodes"),
                ("x", "remove saved"),
                ("→", "page"),
                ("y", "copy address"),
                ("g", "go to"),
                ("R", "clear cache"),
            ],
            Tab::Browser if app.browser.view_source => &[
                ("u", "back to page"),
                ("drag", "copy text"),
                ("Y", "copy source"),
                ("y", "copy address"),
                ("↑↓", "scroll"),
                ("r", "refresh"),
                ("b", "back"),
                ("←", "nodes"),
            ],
            Tab::Browser => &[
                ("drag", "copy text"),
                ("u", "view source"),
                ("y", "copy address"),
                ("Y", "copy page"),
                ("L", "copy link"),
                ("Tab", "next link"),
                ("Enter", "open"),
                ("b", "back"),
                ("r", "refresh"),
                ("R", "clear cache"),
                ("s", "save"),
                ("I", "identify"),
                ("←", "nodes"),
            ],
            Tab::Node if app.node.editing && app.node.editor.is_some() && app.node.view == PageView::Preview => &[
                ("↑↓", "scroll"),
                ("PgUp/PgDn", "page"),
                ("^P", "view"),
                ("^S", "save"),
                ("Esc", "pages"),
            ],
            Tab::Node if app.node.editing && app.node.editor.is_some() => &[
                ("^S", "save"),
                ("Esc", "pages"),
                ("Alt+key", "format (underlined)"),
                ("Shift+move", "select"),
                ("^Z/^Y", "undo/redo"),
                ("^P", "view"),
                ("^V", "paste"),
            ],
            Tab::Node => &[
                ("Enter", "edit"),
                ("n", "new"),
                ("r", "rename"),
                ("x", "delete"),
                ("h", "host on/off"),
                ("a", "announce"),
                ("b", "browse"),
                ("y", "copy address"),
                ("p", "view"),
            ],
            Tab::Reticulum if app.rns.picker.is_some() => &[("↑↓", "select"), ("Enter", "pick"), ("Esc", "cancel")],
            Tab::Reticulum if app.rns.editor.is_some() => &[
                ("^S", "save"),
                ("Esc", "close"),
                ("^Z/^Y", "undo/redo"),
                ("^V", "paste"),
            ],
            Tab::Reticulum => &[
                ("Tab", "sections/options"),
                ("Enter", "edit"),
                ("d", "default"),
                ("a", "add interface"),
                ("t", "edit as text"),
                ("^R", "restart Reticulum"),
                ("R", "reload"),
            ],
            Tab::Status if !app.first_steps().is_empty() => &[
                ("↑↓", "select setting"),
                ("Enter", "edit / toggle"),
                ("e", "edit name"),
                ("y", "copy my address"),
                ("c", "address QR code"),
                ("g", "getting started"),
                ("b", "back up identity"),
                ("i", "use another identity"),
                ("x", "hide first steps"),
                ("^R", "restart Reticulum"),
                ("S", "sync"),
                ("A", "announce"),
                ("q", "quit"),
            ],
            Tab::Status => &[
                ("↑↓", "select setting"),
                ("Enter", "edit / toggle"),
                ("e", "edit name"),
                ("y", "copy my address"),
                ("c", "address QR code"),
                ("g", "getting started"),
                ("b", "back up identity"),
                ("i", "use another identity"),
                ("^R", "restart Reticulum"),
                ("S", "sync"),
                ("A", "announce"),
                ("q", "quit"),
            ],
        }
    };
    let mut spans = Vec::new();
    for (key, action) in hints {
        spans.push(Span::styled(format!(" {key} "), Style::default().fg(Color::Black).bg(DIM)));
        spans.push(Span::raw(format!(" {action}  ")));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub(super) fn draw_prompt(frame: &mut Frame, app: &App) {
    let Some(prompt) = &app.prompt else { return };
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(76);
    // A question too long for the box's top edge goes inside it, whole,
    // above what's typed.
    let inner_width = width.saturating_sub(2) as usize;
    let question = if prompt.title.width() + 2 > inner_width { super::wrap(&prompt.title, inner_width) } else { Vec::new() };
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + area.height / 3,
        width,
        height: (3 + question.len() as u16).min(area.height),
    };
    frame.render_widget(Clear, rect);
    let prompt_block = if question.is_empty() {
        block(&prompt.title, true)
    } else {
        Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(ACCENT))
    };
    let inner = prompt_block.inner(rect);
    let cursor = prompt.input.cursor_column();
    let offset = cursor.saturating_sub(inner.width.saturating_sub(1) as usize);
    frame.render_widget(prompt_block, rect);
    let asked = question.len() as u16;
    frame.render_widget(Paragraph::new(question.join("\n")).style(Style::default().fg(super::ACCENT)), Rect { height: asked, ..inner });
    let typed = Rect { y: inner.y + asked, height: 1, ..inner };
    frame.render_widget(
        Paragraph::new(prompt.input.text()).scroll((0, offset as u16)).style(Style::default().add_modifier(Modifier::BOLD)),
        typed,
    );
    frame.set_cursor_position(Position::new(inner.x + (cursor - offset) as u16, typed.y));
}
