//! Browser tab: saved pages / nodes pane, address bar and the page itself.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{ACCENT, DIM, SELECTED_BG, ago, ago_secs, block};
use crate::app::{App, BrowserFocus, BrowserPane};
use crate::nomad::micron::source::{Token, tokenize, visible};
use crate::term::images::draw_placements;

pub(super) fn draw_browser(frame: &mut Frame, app: &mut App, area: Rect) {
    let [pane, content] =
        Layout::horizontal([Constraint::Length(super::side_width(area.width, 32)), Constraint::Min(20)]).areas(area);
    draw_browser_pane(frame, app, pane);

    let [address, body] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(content);
    app.regions.address = address;

    let identified = app
        .browser
        .location
        .as_ref()
        .is_some_and(|l| app.store.identified_nodes.contains(&hex::encode(l.node)));
    let (url_text, url_style) = match (&app.browser.loading, &app.browser.location) {
        (Some(pending), _) => (
            format!(
                "{}  loading {}s…{}  (Esc to cancel)",
                pending.location.url(),
                pending.started.elapsed().as_secs(),
                pending.status.as_deref().map(|status| format!("  {status}…")).unwrap_or_default()
            ),
            Style::default().fg(Color::Yellow),
        ),
        (None, Some(location)) => {
            let name = app.store.display_name(&hex::encode(location.node));
            (format!("{name}  {}", location.url()), Style::default())
        }
        (None, None) => (
            "Pick a node on the left, or press g / click here to enter an address".to_string(),
            Style::default().fg(DIM),
        ),
    };
    // Badges live in the border so a long address cannot push them away.
    let mut badges = Vec::new();
    if app.browser.loading.is_none()
        && let Some(age) = app.browser.cached_age
    {
        badges.push(Span::styled(
            format!(" cached {} ago · r refresh ", ago_secs(age.as_secs())),
            Style::default().fg(DIM),
        ));
    }
    if identified {
        badges.push(Span::styled(" identified ", Style::default().fg(Color::LightYellow)));
    }
    frame.render_widget(
        Paragraph::new(Span::styled(url_text, url_style))
            .block(block("Address", false).title_top(Line::from(badges).right_aligned())),
        address,
    );

    let view_source = app.browser.view_source;
    let mut body_block = block(
        if view_source { "Source" } else { "Page" },
        app.browser.focus == BrowserFocus::Page,
    );
    app.regions.source_button = Rect::default();
    if app.browser.source.is_some() {
        // A button in the title bar; `u` does the same.
        let (label, style) = if view_source {
            (" ◂ back to page ", Style::default().fg(Color::Black).bg(ACCENT))
        } else {
            (" </> view source ", Style::default().fg(Color::White).bg(DIM))
        };
        let width = label.width() as u16;
        // Right-aligned titles end just inside the top-right corner.
        let x = body.right().saturating_sub(width + 1).max(body.x + 1);
        app.regions.source_button = Rect::new(x, body.y, width.min(body.width.saturating_sub(2)), 1);
        body_block = body_block.title_top(Line::from(Span::styled(label, style)).right_aligned());
    }
    let inner = body_block.inner(body);
    frame.render_widget(body_block, body);
    app.browser.viewport = inner.height as usize;

    let mut page_area = inner;
    if let Some(error) = &app.browser.error {
        let [err, rest] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
        frame.render_widget(
            Paragraph::new(format!("✗ {error}")).style(Style::default().fg(Color::Red)),
            err,
        );
        page_area = rest;
    }
    app.regions.page = page_area;
    if view_source {
        draw_source(frame, app, page_area);
        return;
    }
    let Some(page) = &app.browser.page else {
        app.regions.page_hits.clear();
        return;
    };

    let layout = page.layout(
        page_area.width as usize,
        app.browser.selected,
        &app.browser.images,
        app.graphics.as_ref(),
    );
    let height = page_area.height as usize;
    let max_scroll = layout.lines.len().saturating_sub(height);
    // An anchor jumped to: its row at the top (a folded one's heading).
    if let Some(line) = app.browser.jump_to.take() {
        let row = layout.line_rows.iter().take(line + 1).rev().find_map(|row| *row);
        if let Some(row) = row {
            app.browser.scroll = row;
        }
    }
    app.browser.scroll = app.browser.scroll.min(max_scroll);
    app.browser.item_rows = layout.item_rows;
    app.browser.line_rows = layout.line_rows;
    app.regions.page_hits = layout.hits;
    // Plain text of every row as drawn (alignment applied), for copying.
    let width = page_area.width as usize;
    app.regions.page_text = layout
        .lines
        .iter()
        .map(|line| {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let pad = match line.alignment {
                Some(Alignment::Center) => width.saturating_sub(line.width()) / 2,
                Some(Alignment::Right) => width.saturating_sub(line.width()),
                _ => 0,
            };
            format!("{}{text}", " ".repeat(pad))
        })
        .collect();
    let visible: Vec<Line> = layout
        .lines
        .into_iter()
        .skip(app.browser.scroll)
        .take(height)
        .collect();
    frame.render_widget(Paragraph::new(visible).style(page.base_style), page_area);
    draw_selection(frame, app, page_area);
    let images = &app.browser.images;
    draw_placements(&layout.placements, app.browser.scroll, page_area, frame.buffer_mut(), |url| {
        images.get(url)
    });
}

/// Reverse the text selected with the mouse.
fn draw_selection(frame: &mut Frame, app: &App, area: Rect) {
    let Some(selection) = app.browser.selection.filter(|s| !s.is_empty()) else {
        return;
    };
    let buf = frame.buffer_mut();
    for y in 0..area.height {
        let row = app.browser.scroll + y as usize;
        for x in 0..area.width {
            if selection.contains(row, x as usize)
                && let Some(cell) = buf.cell_mut((area.x + x, area.y + y))
            {
                cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

pub(super) fn token_style(token: Token) -> Style {
    let style = Style::default();
    match token {
        Token::Text => style,
        Token::Comment => style.fg(DIM).add_modifier(Modifier::ITALIC),
        Token::Directive => style.fg(Color::LightMagenta),
        Token::Structure => style.fg(ACCENT).add_modifier(Modifier::BOLD),
        Token::Tag => style.fg(Color::Yellow),
        Token::Link => style.fg(Color::LightBlue),
        Token::Field => style.fg(Color::LightGreen),
        Token::Image => style.fg(Color::LightMagenta),
        Token::Escape => style.fg(DIM),
        Token::Literal => style.fg(Color::Gray),
    }
}

/// The page's Micron source, highlighted and wrapped, with line numbers.
/// Rows are recorded in `page_text` so selecting and copying work as on
/// the page.
fn draw_source(frame: &mut Frame, app: &mut App, area: Rect) {
    app.regions.page_hits.clear();
    app.regions.page_text.clear();
    let Some(source) = &app.browser.source else { return };
    let lines = tokenize(source);
    let gutter_width = lines.len().to_string().len() as u16 + 2;
    if area.width <= gutter_width + 1 {
        return;
    }
    let [gutter, text_area] =
        Layout::horizontal([Constraint::Length(gutter_width), Constraint::Min(1)]).areas(area);
    app.regions.page = text_area;
    let width = text_area.width as usize;

    // (source line number on its first row, spans, plain text)
    let mut rows: Vec<(Option<usize>, Vec<Span<'static>>, String)> = Vec::new();
    for (number, tokens) in lines.iter().enumerate() {
        let mut spans = Vec::new();
        let mut plain = String::new();
        let mut used = 0;
        let mut label = Some(number + 1);
        for (token, text) in tokens {
            let style = token_style(*token);
            let mut chunk = String::new();
            let chars = text.chars().flat_map(|c| {
                let count = if c == '\t' { 4 } else { 1 };
                std::iter::repeat_n(visible(c), count)
            });
            for ch in chars {
                let w = ch.width().unwrap_or(0);
                if used + w > width && used > 0 {
                    spans.push(Span::styled(std::mem::take(&mut chunk), style));
                    rows.push((label.take(), std::mem::take(&mut spans), std::mem::take(&mut plain)));
                    used = 0;
                }
                chunk.push(ch);
                plain.push(ch);
                used += w;
            }
            if !chunk.is_empty() {
                spans.push(Span::styled(chunk, style));
            }
        }
        rows.push((label, spans, plain));
    }

    let height = text_area.height as usize;
    app.browser.scroll = app.browser.scroll.min(rows.len().saturating_sub(height));
    let scroll = app.browser.scroll;
    let numbers: Vec<Line> = rows
        .iter()
        .skip(scroll)
        .take(height)
        .map(|(n, ..)| {
            let text = n.map(|n| format!("{n} ")).unwrap_or_default();
            Line::from(Span::styled(text, Style::default().fg(DIM))).right_aligned()
        })
        .collect();
    frame.render_widget(Paragraph::new(numbers), gutter);
    let visible: Vec<Line> = rows
        .iter()
        .skip(scroll)
        .take(height)
        .map(|(_, spans, _)| Line::from(spans.clone()))
        .collect();
    frame.render_widget(Paragraph::new(visible), text_area);
    app.regions.page_text = rows.into_iter().map(|(.., plain)| plain).collect();
    draw_selection(frame, app, text_area);
}

/// Saved pages and heard nodes, beside the page.
fn draw_browser_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.browser.focus == BrowserFocus::Pane;
    let pane_block = block("Browse", focused);
    let inner = pane_block.inner(area);
    frame.render_widget(pane_block, area);
    if inner.height < 2 {
        return;
    }
    let [tabs_row, list_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);

    app.regions.browser_tabs.clear();
    let mut x = tabs_row.x;
    let mut spans = Vec::new();
    for (pane, label) in [
        (BrowserPane::Saved, format!(" Saved {} ", app.store.saved.len())),
        (BrowserPane::Nodes, format!(" Nodes {} ", app.browser_nodes().len())),
    ] {
        let width = label.width() as u16;
        app.regions.browser_tabs.push((Rect::new(x, tabs_row.y, width, 1), pane));
        x += width + 1;
        let style = if pane == app.browser.pane {
            Style::default().fg(Color::Black).bg(ACCENT).bold()
        } else {
            Style::default().fg(DIM)
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), tabs_row);
    app.regions.browser_list = list_area;

    let current = app.browser.location.as_ref().map(|l| (l.url(), hex::encode(l.node)));
    let (items, empty_hint): (Vec<ListItem>, &str) = match app.browser.pane {
        BrowserPane::Saved => (
            app.store
                .saved
                .iter()
                .map(|b| {
                    let open = current.as_ref().is_some_and(|(url, _)| *url == b.url);
                    let marker = if open { "▸ " } else { "  " };
                    ListItem::new(Line::from(vec![
                        Span::styled(marker, Style::default().fg(ACCENT)),
                        Span::raw(b.name.clone()),
                    ]))
                })
                .collect(),
            "Nothing saved yet. Press s on a page to save it.",
        ),
        BrowserPane::Nodes => (
            app.browser_nodes()
                .iter()
                .map(|(hash, peer)| {
                    let open = current.as_ref().is_some_and(|(_, node)| node == *hash);
                    let marker = if open { "▸ " } else { "  " };
                    let name = peer.name.clone().unwrap_or_else(|| format!("<{}>", &hash[..12]));
                    ListItem::new(Line::from(vec![
                        Span::styled(marker, Style::default().fg(ACCENT)),
                        Span::raw(name),
                        Span::styled(format!("  {}", ago(peer.last_seen)), Style::default().fg(DIM)),
                    ]))
                })
                .collect(),
            "No NomadNet nodes heard yet.",
        ),
    };
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(empty_hint)
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(DIM)),
            list_area,
        );
        return;
    }
    let list = List::new(items).highlight_style(Style::default().bg(SELECTED_BG).bold());
    let state = match app.browser.pane {
        BrowserPane::Saved => &mut app.browser.saved_list,
        BrowserPane::Nodes => &mut app.browser.nodes_list,
    };
    if state.selected().is_none() {
        state.select(Some(0));
    }
    frame.render_stateful_widget(list, list_area, state);
}
