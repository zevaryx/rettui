//! Network tab: everything heard announcing, filterable by kind and
//! searchable by name or address.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};

use super::{DIM, SELECTED_BG, ago, block};
use crate::app::{App, NetFilter, match_mask};
use crate::net::PeerKind;

/// Longest name shown before the address column.
const NAME_WIDTH: usize = 32;

/// `text` (at most `limit` chars) with search matches highlighted.
fn highlighted(text: &str, limit: usize, terms: &[Vec<char>], style: Style) -> Vec<Span<'static>> {
    let hit = Style::default().fg(Color::Black).bg(Color::Yellow);
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut on = false;
    for (c, matched) in text.chars().zip(match_mask(text, terms)).take(limit) {
        if matched != on && !run.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut run), if on { hit } else { style }));
        }
        on = matched;
        run.push(c);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if on { hit } else { style }));
    }
    spans
}

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
                    spans.extend(highlighted(name, NAME_WIDTH, &terms, Style::default()));
                    name.chars().count().min(NAME_WIDTH)
                }
                None => {
                    spans.push(Span::styled("(unnamed)", Style::default().fg(DIM)));
                    "(unnamed)".len()
                }
            };
            spans.push(Span::raw(" ".repeat(NAME_WIDTH - shown + 1)));
            spans.extend(highlighted(hash, usize::MAX, &terms, Style::default().fg(DIM)));
            spans.extend([
                Span::styled(
                    format!("  {} hop{}", peer.hops, if peer.hops == 1 { "" } else { "s" }),
                    Style::default().fg(DIM),
                ),
                Span::styled(format!("  {} ago", ago(peer.last_seen)), Style::default().fg(DIM)),
            ]);
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
        let message = if terms.is_empty() {
            "Listening for announces… peers, NomadNet nodes and propagation nodes appear here as they are heard."
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
