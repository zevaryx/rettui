//! Network tab: everything heard announcing, filterable by kind and
//! searchable by name or address.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};

use super::{DIM, SELECTED_BG, ago, block, highlighted};
use crate::app::{App, NetFilter};
use crate::net::PeerKind;

/// Longest name shown before the address column.
const NAME_WIDTH: usize = 32;
/// Narrowest the name column gets (in narrow terminals, before addresses
/// would be cut off).
const MIN_NAME_WIDTH: usize = 12;
/// A row besides its name: the selection mark (2), the tag (6), spaces
/// around the name (2) and the address (32).
const ROW_WITHOUT_NAME: usize = 42;

fn draw_search(frame: &mut Frame, app: &mut App, area: Rect) {
    let search = &app.net_search;
    let search_block = block("Search", search.typing);
    let inner = search_block.inner(area);
    app.regions.net_search = area;
    let text = search.input.text();
    let content = if text.is_empty() && !search.typing {
        Line::from(Span::styled("Press / or click here to find by name or address", Style::default().fg(DIM)))
    } else {
        Line::from(text.to_string())
    };
    let cursor = search.input.cursor_column();
    let offset = cursor.saturating_sub(inner.width.saturating_sub(1) as usize);
    frame.render_widget(Paragraph::new(content).scroll((0, offset as u16)).block(search_block), area);
    if search.typing {
        frame.set_cursor_position(Position::new(inner.x + (cursor - offset) as u16, inner.y));
    }
}

pub(super) fn draw_network(frame: &mut Frame, app: &mut App, area: Rect) {
    let [search_area, area] = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(area);
    draw_search(frame, app, search_area);
    let terms = app.net_search.terms();

    let filter = match app.net_filter {
        NetFilter::All => "all",
        NetFilter::Peers => "LXMF peers",
        NetFilter::Nodes => "NomadNet nodes",
        NetFilter::Propagation => "propagation nodes",
        NetFilter::Blocked => "blocked",
    };
    let propagation = app.settings.propagation_node.clone();
    let rows = app.network_rows();
    let total = rows.len();
    let title = if terms.is_empty() {
        format!("Heard announces · {filter} · {}", rows.len())
    } else {
        format!("Heard announces · {filter} · {} matching", rows.len())
    };
    // Only the rows on screen get drawn (there can be thousands): keep the
    // selection in view, then build items for that window.
    let height = area.height.saturating_sub(2) as usize;
    // Names give way before addresses do.
    let name_width = (area.width.saturating_sub(2) as usize).saturating_sub(ROW_WITHOUT_NAME).clamp(MIN_NAME_WIDTH, NAME_WIDTH);
    let selected = app.peers.selected().unwrap_or(0).min(total.saturating_sub(1));
    let mut offset = app.peers.offset().min(total.saturating_sub(height.max(1)));
    if selected < offset {
        offset = selected;
    } else if height > 0 && selected >= offset + height {
        offset = selected + 1 - height;
    }
    let items: Vec<ListItem> = rows
        .iter()
        .skip(offset)
        .take(height)
        .map(|(hash, peer)| {
            let (tag, color) = match peer.kind {
                PeerKind::Lxmf => ("PEER", Color::LightMagenta),
                PeerKind::Nomad => ("NODE", Color::LightGreen),
                PeerKind::Propagation => ("PROP", Color::LightYellow),
            };
            let mut spans = vec![
                Span::styled(format!(" {tag} "), Style::default().fg(Color::Black).bg(color)),
                Span::raw(" "),
            ];
            let shown = match &peer.name {
                Some(name) => {
                    spans.extend(highlighted(name, name_width, &terms, Style::default()));
                    name.chars().count().min(name_width)
                }
                None => {
                    spans.push(Span::styled("(unnamed)", Style::default().fg(DIM)));
                    "(unnamed)".len()
                }
            };
            spans.push(Span::raw(" ".repeat(name_width.saturating_sub(shown) + 1)));
            spans.extend(highlighted(hash, usize::MAX, &terms, Style::default().fg(DIM)));
            spans.extend([
                Span::styled(
                    format!("  {} hop{}", peer.hops, if peer.hops == 1 { "" } else { "s" }),
                    Style::default().fg(DIM),
                ),
                Span::styled(
                    if peer.last_seen == 0 { "  not heard".to_string() } else { format!("  {} ago", ago(peer.last_seen)) },
                    Style::default().fg(DIM),
                ),
            ]);
            if app.store.contacts.get(hash.as_str()).is_some_and(|c| c.trust == crate::store::Trust::Blocked) {
                spans.push(Span::styled("  ⛔ blocked", Style::default().fg(Color::LightRed)));
            }
            if propagation.as_deref() == Some(hash.as_str()) {
                spans.push(Span::styled("  ★ outbound", Style::default().fg(Color::LightYellow)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let empty = items.is_empty();
    let list_block = block(&title, true);
    app.regions.peers = list_block.inner(area);
    if empty {
        let message = if app.net_filter == NetFilter::Blocked && terms.is_empty() {
            "Nobody is blocked. Block someone from their contact card (c in Messages), or with b on an LXMF peer here.".to_string()
        } else if terms.is_empty() && !app.interfaces.iter().any(|i| i.online) {
            "Not connected to anyone yet, so nothing can be heard. The getting-started guide (g in the Status tab) adds an entry point to connect through."
                .to_string()
        } else if terms.is_empty() {
            "Listening for announces… peers, NomadNet nodes and propagation nodes appear here as they are heard. Each announces on its own schedule, many only every few hours, so the list fills over the first hours. Press A to announce yourself, so others can find you too."
                .to_string()
        } else {
            format!("Nothing heard matches “{}”. Esc clears the search.", app.net_search.input.text().trim())
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::default().fg(DIM))
                .wrap(Wrap { trim: true })
                .block(list_block),
            area,
        );
        return;
    }
    app.peers.select(Some(selected));
    *app.peers.offset_mut() = offset;
    // The window's own state: the selection relative to its first row.
    let mut window = ListState::default().with_selected(Some(selected - offset));
    let list = List::new(items)
        .block(list_block)
        .highlight_style(Style::default().bg(SELECTED_BG).bold())
        .highlight_symbol("▌");
    frame.render_stateful_widget(list, area, &mut window);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::config::Settings;
    use crate::net::InterfaceInfo;
    use crate::store::Store;

    fn screen(app: &mut crate::app::App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..30).map(|y| (0..120).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    #[test]
    fn an_empty_list_says_why_and_what_to_expect() {
        let dir = std::env::temp_dir().join(format!("rettui-net-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = crate::app::test_app(&dir, Settings::default(), Store::default());
        app.tab = crate::app::Tab::Network;
        let shown = screen(&mut app);
        assert!(shown.contains("Not connected to anyone yet") && shown.contains("getting-started guide"), "{shown}");
        app.interfaces.push(InterfaceInfo { name: "RMAP World".into(), online: true, rx_bytes: 0, tx_bytes: 0 });
        let shown = screen(&mut app);
        assert!(shown.contains("Listening for announces") && shown.contains("every few hours") && shown.contains("Press A"), "{shown}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
