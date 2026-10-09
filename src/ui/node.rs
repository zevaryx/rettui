//! Node tab: hosting status, the page list, and the page editor with a live
//! Micron preview.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};

use super::browser::token_style;
use super::editor::draw_text_editor;
use super::{accent, block, dim, human_bytes, selected_bg};
use crate::app::App;
use crate::app::format::{Action, RIBBON};
use crate::app::node::{NodeRow, NodeStatus, PageView, Preview};
use crate::nomad::host::HostConfig;
use crate::nomad::micron;
use crate::nomad::micron::source::tokenize;
use crate::nomad::pages::Root;

pub(super) fn draw_node(frame: &mut Frame, app: &mut App, area: Rect) {
    // Wide enough for the 32-character node address.
    let [left, right] = Layout::horizontal([Constraint::Length(super::side_width(area.width, 43)), Constraint::Min(20)]).areas(area);
    let [status_area, list_area] = Layout::vertical([Constraint::Length(7), Constraint::Min(3)]).areas(left);
    draw_status(frame, app, status_area);
    draw_pages(frame, app, list_area);
    draw_editor(frame, app, right);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let label = |s: &str| Span::styled(format!("{s:<9}"), Style::default().fg(dim()));
    let state = match &app.node.status {
        NodeStatus::Running => Span::styled("● hosting", Style::default().fg(Color::Green)),
        NodeStatus::Starting => Span::styled("◌ starting", Style::default().fg(Color::Yellow)),
        // `h` starts it (the footer and the editor pane say so).
        NodeStatus::Off => Span::styled("○ off", Style::default().fg(dim())),
        NodeStatus::Failed(e) => Span::styled(format!("✗ {e}"), Style::default().fg(Color::Red)),
    };
    let running = app.node.status == NodeStatus::Running;
    let name = app.settings.node_name.clone().unwrap_or_else(|| app.settings.display_name.clone());
    let requests = app
        .node
        .stats
        .as_ref()
        .map_or_else(|| "–".to_string(), |s| format!("{} ({} pages, {} files)", s.request_count, s.page_hits, s.file_hits));
    let dir = HostConfig::dir(&app.settings, &app.paths).display().to_string();
    let lines = vec![
        Line::from(vec![label("Node"), state]),
        Line::from(vec![
            label("Address"),
            Span::styled(hex::encode(app.node.hash), Style::default().fg(if running { accent() } else { dim() })),
        ]),
        Line::from(vec![label("Name"), Span::raw(name)]),
        Line::from(vec![label("Requests"), Span::raw(requests)]),
        Line::from(vec![label("Folder"), Span::styled(dir, Style::default().fg(dim()))]),
    ];
    frame.render_widget(Paragraph::new(lines).block(block("Hosting", false)), area);
}

/// What's on the node: `pages/` and `files/`, each with its folders.
fn draw_pages(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = format!("Pages · {} · Files · {}", app.node.pages.len(), app.node.files.len());
    let list_block = block(&title, !app.node.editing);
    app.regions.node_pages = list_block.inner(area);
    if let Some(error) = &app.node.error {
        frame.render_widget(
            Paragraph::new(error.as_str()).style(Style::default().fg(Color::Red)).wrap(Wrap { trim: true }).block(list_block),
            area,
        );
        return;
    }
    let open = app.node.editor.as_ref().map(|e| (e.path.clone(), e.dirty()));
    let rows = app.node_rows();
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let indent = |depth: usize| "  ".repeat(depth);
            let Some(entry) = app.node_entry(row) else {
                let NodeRow::Folder { root, path, depth } = row else { return ListItem::new("") };
                // A root, or a folder in one (by its own name).
                let name =
                    if path.is_empty() { format!("{}/", root.id()) } else { format!("{}/", path.rsplit('/').next().unwrap_or(path)) };
                let style = if path.is_empty() { Style::default().fg(accent()).bold() } else { Style::default().fg(accent()) };
                return ListItem::new(Line::from(vec![Span::raw(indent(*depth)), Span::styled(name, style)]));
            };
            let NodeRow::Item { root, depth, .. } = row else { return ListItem::new("") };
            let page = *root == Root::Pages;
            let (marker, marker_style) = match &open {
                Some((path, true)) if page && *path == entry.path => ("● ", Style::default().fg(Color::Yellow)),
                Some((path, false)) if page && *path == entry.path => ("▸ ", Style::default().fg(accent())),
                _ => ("  ", Style::default()),
            };
            let name = entry.path.rsplit('/').next().unwrap_or(&entry.path).to_string();
            let name_style = if page && entry.text { Style::default() } else { Style::default().fg(dim()) };
            let mut spans =
                vec![Span::raw(indent(depth.saturating_sub(1))), Span::styled(marker, marker_style), Span::styled(name, name_style)];
            if entry.executable {
                spans.push(Span::styled(" script", Style::default().fg(Color::LightMagenta)));
            } else if page && !entry.text {
                spans.push(Span::styled(" picture", Style::default().fg(Color::LightBlue)));
            }
            spans.push(Span::styled(format!("  {}", human_bytes(entry.size)), Style::default().fg(dim())));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items).block(list_block).highlight_style(Style::default().bg(selected_bg()).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, area, &mut app.node.list);
}

fn draw_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    app.regions.node_editor = Rect::default();
    app.regions.node_preview = Rect::default();
    let editing = app.node.editing;
    let view = app.node.view;
    let wrap = app.settings.wrap_lines;
    let Some(editor) = &mut app.node.editor else {
        let help = vec![
            Line::raw("Pick a page and press Enter to edit it, or n to create one."),
            Line::raw(""),
            Line::raw("Ctrl-S saves the page into the node folder. While the node is hosting,"),
            Line::raw("visitors get the new version straight away."),
            Line::raw(""),
            Line::styled("h hosts or stops the node · b opens it in the Browser", Style::default().fg(dim())),
        ];
        frame.render_widget(Paragraph::new(help).wrap(Wrap { trim: false }).block(block("Editor", false)), area);
        return;
    };

    let (editor_area, preview_area) = match view {
        PageView::Editor => (Some(area), None),
        PageView::Preview => (None, Some(area)),
        PageView::Split if area.width >= 100 => {
            let [a, b] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area);
            (Some(a), Some(b))
        }
        PageView::Split => {
            let [a, b] = Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);
            (Some(a), Some(b))
        }
    };

    let mut title = editor.path.clone();
    if editor.executable {
        title.push_str(" (script)");
    }
    if editor.dirty() {
        title.push_str(" ● modified");
    }
    let hints = Line::styled(format!(" Ctrl-S save · Ctrl-P {} · Esc pages ", view.next().label()), Style::default().fg(dim()));
    let text = editor.area.text();
    if let Some(editor_area) = editor_area {
        // Key hints only when they fit beside the page name.
        let room = editor_area.width as usize > title.chars().count() + hints.width() + 6;
        let editor_block = if room { block(&title, editing).title_top(hints.clone().right_aligned()) } else { block(&title, editing) };
        let mut inner = editor_block.inner(editor_area);
        frame.render_widget(editor_block, editor_area);
        app.regions.node_ribbon.clear();
        if inner.height >= 4 {
            let [ribbon, text] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
            draw_ribbon(frame, ribbon, &mut app.regions.node_ribbon);
            inner = text;
        }
        app.regions.node_editor = draw_text_editor(frame, &mut editor.area, inner, editing, wrap, |text| {
            tokenize(text).into_iter().map(|line| line.into_iter().map(|(token, text)| (token_style(token), text)).collect()).collect()
        });
    }

    let Some(preview_area) = preview_area else { return };
    // Shown alone, the preview carries the page's title and keys.
    let preview_title = if editor_area.is_some() { "Preview".to_string() } else { format!("{title} · preview") };
    let preview_block =
        if editor_area.is_some() { block(&preview_title, false) } else { block(&preview_title, editing).title_top(hints.right_aligned()) };
    let preview_inner = preview_block.inner(preview_area);
    frame.render_widget(preview_block, preview_area);
    app.regions.node_preview = preview_inner;
    let width = preview_inner.width as usize;
    if !app.node.preview.as_ref().is_some_and(|p| p.width == width && p.text == text) {
        let page = micron::parse(&text);
        let layout = page.layout(width, None, &HashMap::new(), None);
        app.node.preview = Some(Preview { text, width, lines: layout.lines, style: page.base_style });
    }
    let preview = app.node.preview.as_ref().expect("laid out above");
    let total = preview.lines.len();
    let height = preview_inner.height as usize;
    let most = total.saturating_sub(height);
    let scroll = if editor_area.is_none() {
        app.node.preview_scroll = app.node.preview_scroll.min(most);
        app.node.preview_scroll
    } else {
        // Keep the preview roughly level with the part being edited.
        let (top, lines) = (editor.area.top, editor.area.row_count());
        (if lines > 1 { top * total / lines } else { 0 }).min(most)
    };
    let visible: Vec<Line> = preview.lines.iter().skip(scroll).take(height).cloned().collect();
    frame.render_widget(Paragraph::new(visible).style(preview.style), preview_inner);
}

/// The formatting ribbon: its buttons, each with its Alt-key letter
/// underlined, in groups. Short labels when the full ones don't fit, and
/// whatever fits after that.
fn draw_ribbon(frame: &mut Frame, area: Rect, buttons: &mut Vec<(Rect, Action)>) {
    const SEPARATOR: &str = " │ ";
    let width = |short: bool| -> usize {
        RIBBON
            .iter()
            .map(|group| group.iter().map(|a| if short { a.short() } else { a.label() }.len() + 1).sum::<usize>() - 1)
            .sum::<usize>()
            + SEPARATOR.len() * (RIBBON.len() - 1)
    };
    let short = width(false) > area.width as usize;
    let key_style = Style::default().fg(accent()).add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
    let mut spans = Vec::new();
    let mut x = area.x;
    'groups: for (g, group) in RIBBON.iter().enumerate() {
        for (i, action) in group.iter().enumerate() {
            let gap = if i > 0 {
                " "
            } else if g > 0 {
                SEPARATOR
            } else {
                ""
            };
            let label = if short { action.short() } else { action.label() };
            let needed = (gap.chars().count() + label.len()) as u16;
            if x + needed > area.right() {
                break 'groups;
            }
            spans.push(Span::styled(gap, Style::default().fg(dim())));
            x += gap.chars().count() as u16;
            // The shortcut's letter, underlined (labels are ASCII).
            let at = label.to_ascii_lowercase().find(action.key()).unwrap_or(0);
            spans.push(Span::raw(&label[..at]));
            spans.push(Span::styled(&label[at..=at], key_style));
            spans.push(Span::raw(&label[at + 1..]));
            buttons.push((Rect::new(x, area.y, label.len() as u16, 1), *action));
            x += label.len() as u16;
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
