//! Reticulum tab: the config file's sections, the options of the selected
//! one, a list picker for choices, and the file as text.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};

use super::editor::{Highlighted, draw_text_editor};
use super::{ACCENT, DIM, SELECTED_BG, block};
use crate::app::App;
use crate::app::reticulum::{RnsFocus, RnsRow, rns_truthy};
use crate::reticulum::schema::Kind;
use crate::reticulum::{self as rns, EXTERNAL_NOTE, RESTART_NOTE, Section};

const KEY: Color = Color::LightBlue;

pub(super) fn draw_reticulum(frame: &mut Frame, app: &mut App, area: Rect) {
    app.regions.rns_sections = Rect::default();
    app.regions.rns_options = Rect::default();
    app.regions.rns_editor = Rect::default();
    app.regions.rns_picker = Rect::default();
    if let Some(error) = &app.rns.load_error {
        let text = vec![
            Line::styled(error.clone(), Style::default().fg(Color::Red)),
            Line::raw(""),
            Line::styled("Fix the file's permissions and press R to read it again.", Style::default().fg(DIM)),
        ];
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }).block(block("Reticulum config", false)), area);
        return;
    }
    if app.rns.editor.is_some() {
        draw_text(frame, app, area);
        return;
    }
    // Borders, the file, the check, the restart note, warnings, and the
    // note about another program's shared instance.
    let status_height = 5 + app.rns.check.warnings.len().min(3) as u16 + u16::from(app.uses_external_shared_instance());
    let [status, body] = Layout::vertical([Constraint::Length(status_height), Constraint::Min(5)]).areas(area);
    draw_file_status(frame, app, status);
    let [sections, options] = Layout::horizontal([Constraint::Length(super::side_width(body.width, 34)), Constraint::Min(20)]).areas(body);
    draw_sections(frame, app, sections);
    draw_options(frame, app, options);
    draw_picker(frame, app, area);
}

/// A path cut from the left to fit `width` columns.
fn fit_path(path: &str, width: usize) -> String {
    let count = path.chars().count();
    if count <= width {
        return path.to_string();
    }
    let keep: String = path.chars().skip(count + 1 - width.max(2)).collect();
    format!("…{keep}")
}

fn draw_file_status(frame: &mut Frame, app: &App, area: Rect) {
    let state = &app.rns;
    let missing = "  not created yet: showing rsReticulum's defaults, saving creates it";
    let room = (area.width as usize).saturating_sub(8 + if state.exists { 0 } else { missing.len() });
    let mut lines = vec![Line::from(vec![
        Span::styled("File  ", Style::default().fg(DIM)),
        Span::raw(fit_path(&state.path.display().to_string(), room)),
        if state.exists { Span::raw("") } else { Span::styled(missing, Style::default().fg(Color::Yellow)) },
    ])];
    match &state.check.error {
        Some(error) => lines.push(Line::styled(format!("✗ Reticulum cannot load this file: {error}"), Style::default().fg(Color::Red))),
        None => lines.push(Line::styled("✓ The file loads", Style::default().fg(Color::Green))),
    }
    for warning in state.check.warnings.iter().take(3) {
        lines.push(Line::styled(format!("! {warning}"), Style::default().fg(Color::Yellow)));
    }
    lines.push(Line::styled(format!("{RESTART_NOTE} (Ctrl-R)"), Style::default().fg(DIM).italic()));
    if app.uses_external_shared_instance() {
        lines.push(Line::styled(EXTERNAL_NOTE, Style::default().fg(Color::Yellow).italic()));
    }
    frame.render_widget(Paragraph::new(lines).block(block("Reticulum config", false)), area);
}

fn draw_sections(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.rns.focus == RnsFocus::Sections;
    let interfaces = app.rns.sections.len().saturating_sub(2);
    let title = if area.width >= 30 {
        format!("Sections · {interfaces} interface{}", if interfaces == 1 { "" } else { "s" })
    } else {
        "Sections".to_string()
    };
    let list_block = block(&title, focused).title_bottom(Line::styled(" a add interface ", Style::default().fg(DIM)));
    app.regions.rns_sections = list_block.inner(area);
    let config = rns::check(&app.rns.text).0;
    let items: Vec<ListItem> = app
        .rns
        .sections
        .iter()
        .map(|section| match section {
            Section::Interface(name) => {
                let values = config.as_ref().and_then(|c| c.subsection("interfaces", name));
                let enabled = values
                    .and_then(|v| v.get("enabled").or_else(|| v.get("interface_enabled")))
                    .is_none_or(rns_truthy);
                let kind = values.and_then(|v| v.get("type")).unwrap_or("?").trim_end_matches("Interface").to_string();
                let (dot, color) = if enabled { ("●", Color::Green) } else { ("○", DIM) };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("  {dot} "), Style::default().fg(color)),
                    Span::raw(name.clone()),
                    Span::styled(format!("  {kind}"), Style::default().fg(DIM)),
                ]))
            }
            other => ListItem::new(Line::from(vec![Span::raw(format!(" {}", other.title())).bold()])),
        })
        .collect();
    let list = List::new(items).highlight_style(Style::default().bg(SELECTED_BG).add_modifier(Modifier::BOLD));
    frame.render_widget(list_block, area);
    frame.render_stateful_widget(list, app.regions.rns_sections, &mut app.rns.section_list);
}

fn draw_options(frame: &mut Frame, app: &mut App, area: Rect) {
    let state = &app.rns;
    let focused = state.focus == RnsFocus::Options;
    let title = state.section().map_or_else(String::new, |s| match s {
        Section::Interface(name) => format!("Interface · {name}"),
        other => other.title(),
    });
    let hints = match state.section() {
        Some(Section::Interface(_)) => " Enter edit · d default · Space on/off · r rename · x delete · t text ",
        _ => " Enter edit · d default · t edit as text ",
    };
    let options_block = block(&title, focused).title_bottom(Line::styled(hints, Style::default().fg(DIM)));
    let inner = options_block.inner(area);
    frame.render_widget(options_block, area);
    let [list_area, help_area] = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(inner);
    app.regions.rns_options = list_area;

    // Labels get at most half the width, so the values always show.
    let widest = state.options.iter().map(|o| o.label.chars().count()).max().unwrap_or(10).min(28);
    let label_width = widest.min((list_area.width as usize).saturating_sub(4) / 2).max(6) + 2;
    let items: Vec<ListItem> = state
        .rows
        .iter()
        .map(|row| match row {
            RnsRow::Group(title) => ListItem::new(Line::styled(title.to_string(), Style::default().fg(ACCENT).bold())),
            RnsRow::Option(i) => {
                let option = &state.options[*i];
                let value = match option.shown() {
                    Some(v) if option.kind == Kind::Bool => Span::raw(if rns_truthy(&v) { "yes".to_string() } else { "no".to_string() }),
                    Some(v) => Span::raw(v),
                    None if option.default.is_empty() => Span::styled("–", Style::default().fg(DIM)),
                    None if option.kind == Kind::Bool => {
                        let shown = if rns_truthy(option.default) { "yes" } else { "no" };
                        Span::styled(format!("{shown} (default)"), Style::default().fg(DIM))
                    }
                    None => Span::styled(format!("{} (default)", option.default), Style::default().fg(DIM)),
                };
                let label: String = if option.label.chars().count() > label_width - 2 {
                    option.label.chars().take(label_width - 3).chain(std::iter::once('…')).collect()
                } else {
                    option.label.clone()
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("  {label:<label_width$}"), Style::default().fg(DIM)),
                    value,
                ]))
            }
        })
        .collect();
    let list = List::new(items).highlight_style(Style::default().bg(SELECTED_BG).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, list_area, &mut app.rns.option_list);

    let help = app.rns.selected_option().map_or_else(Vec::new, |o| {
        vec![
            Line::from(vec![Span::styled(format!(" {} ", o.key), Style::default().fg(KEY)), Span::styled(o.help, Style::default().fg(DIM).italic())]),
        ]
    });
    frame.render_widget(Paragraph::new(help).wrap(Wrap { trim: false }), help_area);
}

fn draw_picker(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(picker) = &mut app.rns.picker else { return };
    const HINT: &str = " Enter pick · Esc cancel ";
    let width = picker.choices.iter().map(|(_, l)| l.chars().count()).max().unwrap_or(10).max(picker.title.chars().count()) as u16 + 8;
    let width = width.max(HINT.chars().count() as u16 + 4);
    let height = picker.choices.len() as u16 + 2;
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 3,
        width: width.min(area.width),
        height: height.min(area.height),
    };
    frame.render_widget(Clear, rect);
    let picker_block = block(&picker.title, true).title_bottom(Line::styled(HINT, Style::default().fg(DIM)));
    app.regions.rns_picker = picker_block.inner(rect);
    let items: Vec<ListItem> = picker.choices.iter().map(|(_, label)| ListItem::new(format!(" {label}"))).collect();
    let list = List::new(items).block(picker_block).highlight_style(Style::default().bg(SELECTED_BG).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, rect, &mut picker.list);
}

/// Colours for the config file: sections, keys, values and comments.
fn highlight(text: &str) -> Highlighted {
    let comment = Style::default().fg(DIM);
    text.split('\n')
        .map(|line| {
            let trimmed = line.trim_start();
            let indent = line[..line.len() - trimmed.len()].to_string();
            if trimmed.starts_with('#') {
                return vec![(comment, line.to_string())];
            }
            if trimmed.starts_with('[') {
                return vec![(Style::default(), indent), (Style::default().fg(ACCENT).bold(), trimmed.to_string())];
            }
            let Some(eq) = trimmed.find('=') else { return vec![(Style::default(), line.to_string())] };
            let (key, rest) = trimmed.split_at(eq);
            let (value, note) = match rest.find(" #") {
                Some(hash) => rest.split_at(hash),
                None => (rest, ""),
            };
            vec![
                (Style::default(), indent),
                (Style::default().fg(KEY), key.to_string()),
                (Style::default().fg(DIM), "=".to_string()),
                (Style::default(), value[1..].to_string()),
                (comment, note.to_string()),
            ]
        })
        .collect()
}

fn draw_text(frame: &mut Frame, app: &mut App, area: Rect) {
    let wrap = app.settings.wrap_lines;
    let Some(editor) = &mut app.rns.editor else { return };
    let text = editor.area.text();
    let (_, check) = rns::check(&text);
    let [editor_area, check_area] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
    let mut title = app.rns.path.display().to_string();
    if editor.dirty() {
        title.push_str(" ● modified");
    }
    let editor_block = block(&title, true)
        .title_top(Line::styled(" Ctrl-S save · Esc close · Ctrl-Z/Y undo ", Style::default().fg(DIM)).right_aligned());
    let inner = editor_block.inner(editor_area);
    frame.render_widget(editor_block, editor_area);
    app.regions.rns_editor = draw_text_editor(frame, &mut editor.area, inner, true, wrap, highlight);
    let status = match (&check.error, check.warnings.first()) {
        (Some(error), _) => Line::styled(format!(" ✗ {error} (cannot be saved like this)"), Style::default().fg(Color::Red)),
        (None, Some(warning)) => Line::styled(
            format!(" ! {warning}{}", if check.warnings.len() > 1 { format!(" (+{} more)", check.warnings.len() - 1) } else { String::new() }),
            Style::default().fg(Color::Yellow),
        ),
        (None, None) => Line::styled(" ✓ Loads", Style::default().fg(Color::Green)),
    };
    frame.render_widget(Paragraph::new(status), check_area);
}
