//! The getting-started guide over the screen (see [`crate::app::guide`]):
//! one line a choice, with what the chosen one does at the bottom. The
//! choices scroll when they don't all fit.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use unicode_width::UnicodeWidthStr;

use super::{dim, picked_style, block, wrap};
use crate::app::App;
use crate::app::guide::{self, ENTRY_POINTS, Guide, GuideRow, GuideView, LINKS};
use crate::app::reach::Reach;

/// The guide's widest, in columns.
const WIDTH: u16 = 84;

fn check(on: bool) -> &'static str {
    if on { "[x]" } else { "[ ]" }
}

/// What a row does, shown under the guide when it's picked; for an entry
/// point, by what trying it found.
fn help(row: Option<GuideRow>, reach: impl Fn(usize) -> Reach) -> String {
    match row {
        Some(GuideRow::Name) => guide::NAME_HELP.to_string(),
        Some(GuideRow::Identity) => guide::IDENTITY_HELP.to_string(),
        Some(GuideRow::Connect(i)) => {
            let entry = ENTRY_POINTS[i];
            match reach(i) {
                Reach::Down(why) => format!("{}:{}. {}", entry.host, entry.port, guide::down_help(why)),
                _ => format!("{}:{}. {}", entry.host, entry.port, guide::CONNECT_HELP),
            }
        }
        Some(GuideRow::Discover) => guide::DISCOVER_HELP.to_string(),
        Some(GuideRow::AutoPropagation) => guide::AUTO_PROPAGATION_HELP.to_string(),
        Some(GuideRow::Link(i)) => match LINKS[i] {
            (_, url, Some(note)) => format!("{note} {url} · Enter opens it, y copies it"),
            (_, url, None) => format!("{url} · Enter opens it, y copies it"),
        },
        Some(GuideRow::Apply) => guide::APPLY_HELP.to_string(),
        Some(GuideRow::Later) | None => guide::LATER_HELP.to_string(),
    }
}

/// Every entry point that isn't in the config already was tried, and none
/// answered.
fn none_answered(view: &GuideView) -> bool {
    let offered: Vec<Reach> = view.reach.iter().zip(&view.has_entry_point).filter(|(_, has)| !**has).map(|(reach, _)| *reach).collect();
    !offered.is_empty() && offered.iter().all(|reach| reach.is_down())
}

pub(super) fn draw_guide(frame: &mut Frame, app: &mut App) {
    app.regions.guide_rows.clear();
    let Some(state) = app.guide.clone() else { return };
    let view = app.guide_view();
    let rows = Guide::rows(&view);
    let area = frame.area();
    let width = WIDTH.min(area.width.saturating_sub(2));
    let inner_width = width.saturating_sub(4) as usize;
    let selected = rows.get(state.row).copied();
    let choices = &state.shown(&view);

    let intro: Vec<Line<'static>> = wrap(guide::INTRO, inner_width).into_iter().map(|l| Line::styled(l, Style::default().fg(dim()))).collect();
    let status = match (view.interfaces_online, view.heard) {
        (0, _) => Line::styled("○ Not connected to anyone yet", Style::default().fg(Color::Yellow)),
        (n, 0) => Line::styled(format!("◌ {n} interface{} online, nobody heard yet", if n == 1 { "" } else { "s" }), Style::default().fg(Color::Yellow)),
        (n, heard) => Line::styled(
            format!("● Connected: {n} interface{} online, {heard} peers and nodes heard", if n == 1 { "" } else { "s" }),
            Style::default().fg(Color::Green),
        ),
    };

    // The choices and links, and which line each row is on.
    let mut body: Vec<Line<'static>> = Vec::new();
    let mut placed: Vec<(usize, GuideRow)> = Vec::new();
    let mut row_line = |body: &mut Vec<Line<'static>>, row: GuideRow, spans: Vec<Span<'static>>| {
        let style = if Some(row) == selected { Style::default().bg(super::selected_bg()).add_modifier(Modifier::BOLD) } else { Style::default() };
        placed.push((body.len(), row));
        body.push(Line::from(spans).style(style));
    };
    if view.shared_config && !view.external {
        for line in wrap(guide::SHARED_NOTE, inner_width) {
            body.push(Line::styled(line, Style::default().fg(Color::Yellow)));
        }
    }
    row_line(&mut body, GuideRow::Name, vec![Span::styled("Your name   ", Style::default().fg(dim())), Span::raw(choices.name.clone())]);
    let identity = match (view.identity_pending, view.address) {
        (Some(next), _) => vec![Span::raw(format!("{} from the next start", hex::encode(next)))],
        (None, address) => vec![
            Span::raw(address.map(hex::encode).unwrap_or_else(|| "(starting)".into())),
            Span::styled("  · use one you already have", Style::default().fg(dim())),
        ],
    };
    row_line(&mut body, GuideRow::Identity, [vec![Span::styled("Identity    ", Style::default().fg(dim()))], identity].concat());
    if view.external {
        for line in wrap(&format!("{}: add entry points in that program's Reticulum config.", crate::reticulum::EXTERNAL_NOTE), inner_width) {
            body.push(Line::styled(line, Style::default().fg(dim())));
        }
    } else {
        // Under their regions' headings, each with what trying it found;
        // one that didn't answer is greyed out, and can't be ticked.
        let name_width = ENTRY_POINTS.iter().map(|entry| entry.name.width()).max().unwrap_or(0);
        let mut region = "";
        for (i, entry) in ENTRY_POINTS.iter().enumerate() {
            if entry.region != region {
                region = entry.region;
                body.push(Line::styled(format!("Entry points · {region}"), Style::default().fg(dim())));
            }
            let name = format!("{}{}  ", entry.name, " ".repeat(name_width - entry.name.width()));
            let spans = match view.reach[i] {
                _ if view.has_entry_point[i] => {
                    vec![Span::raw(format!("[x] {name}")), Span::styled("in your Reticulum config already", Style::default().fg(dim()))]
                }
                reach @ Reach::Down(_) => vec![Span::styled(format!("[ ] {name}{}", reach.label()), Style::default().fg(dim()))],
                reach => {
                    let color = if matches!(reach, Reach::Up(_)) { Color::Green } else { dim() };
                    vec![Span::raw(format!("{} {name}", check(choices.connect[i]))), Span::styled(reach.label(), Style::default().fg(color))]
                }
            };
            row_line(&mut body, GuideRow::Connect(i), spans);
        }
        if none_answered(&view) {
            for line in wrap(guide::NONE_ANSWERED, inner_width) {
                body.push(Line::styled(line, Style::default().fg(Color::Yellow)));
            }
        }
        let discover = if view.has_discovery {
            "[x] Find entry points near you over time (on already)".to_string()
        } else {
            format!("{} Also find entry points near you over time (interface discovery)", check(choices.discover))
        };
        row_line(&mut body, GuideRow::Discover, vec![Span::raw(discover)]);
    }
    row_line(&mut body, GuideRow::AutoPropagation, vec![Span::raw(format!("{} Pick a propagation node automatically", check(choices.auto_propagation)))]);
    // The last line of a row with a warning under it, to keep in view
    // with it.
    let mut warned: Option<(GuideRow, usize)> = None;
    if choices.auto_propagation {
        for line in wrap(guide::AUTO_PROPAGATION_WARNING, inner_width.saturating_sub(4)) {
            body.push(Line::styled(format!("    {line}"), Style::default().fg(Color::Yellow)));
        }
        warned = Some((GuideRow::AutoPropagation, body.len() - 1));
    }
    body.push(Line::styled("Learn more", Style::default().fg(dim())));
    for (i, (title, ..)) in LINKS.iter().enumerate() {
        row_line(&mut body, GuideRow::Link(i), vec![Span::raw("↗ "), Span::styled(*title, Style::default().add_modifier(Modifier::UNDERLINED))]);
    }

    // The two buttons share a line.
    let button = |label: &str, row: GuideRow| {
        let style = if Some(row) == selected { picked_style() } else { Style::default().fg(Color::Black).bg(dim()) };
        Span::styled(format!(" {label} "), style)
    };
    let (apply, later) = (" Apply ", " Not now ");
    let buttons = Line::from(vec![button(apply.trim(), GuideRow::Apply), Span::raw("  "), button(later.trim(), GuideRow::Later)]);

    // What the chosen row does, with room for the longest so the guide
    // keeps its size moving between rows (and as entry points answer).
    let help_lines = wrap(&help(selected, |i| view.reach[i]), inner_width);
    let help_room = rows
        .iter()
        .flat_map(|row| [help(Some(*row), |_| Reach::Checking), help(Some(*row), |_| Reach::Down("name not found"))])
        .map(|text| wrap(&text, inner_width).len())
        .max()
        .unwrap_or(0)
        .max(help_lines.len());

    // On a short screen, the intro and then the blank lines between parts
    // are left out; then the choices scroll.
    let height = area.height.saturating_sub(2) as usize;
    let shown = body.len().min(height.saturating_sub(1 + 1 + help_room).max(1));
    let mut spare = height.saturating_sub(1 + shown + 1 + help_room);
    let mut fits = |lines: usize| {
        let fits = spare >= lines;
        if fits {
            spare -= lines;
        }
        fits
    };
    let show_intro = fits(intro.len());
    let (gap_before_help, gap_before_buttons, gap_after_status) = (fits(1), fits(1), fits(1));

    let mut lines = if show_intro { intro } else { Vec::new() };
    lines.push(status);
    if gap_after_status {
        lines.push(Line::default());
    }
    // The picked row stays in view, with the line above it (perhaps its
    // heading) and any warning under it.
    let body_len = body.len();
    let mut top = state.scroll;
    if let Some(&(line, row)) = placed.iter().find(|(_, row)| Some(*row) == selected) {
        let last = warned.filter(|(warned, _)| *warned == row).map_or(line, |(_, last)| last);
        if line <= top {
            top = line.saturating_sub(1);
        } else if last >= top + shown {
            top = (last + 1 - shown).min(line);
        }
    }
    let top = top.min(body_len - shown);
    if let Some(guide) = &mut app.guide {
        guide.scroll = top;
    }
    let body_at = lines.len();
    lines.extend(body.into_iter().skip(top).take(shown));
    if gap_before_buttons {
        lines.push(Line::default());
    }
    let buttons_at = lines.len();
    lines.push(buttons);
    if gap_before_help {
        lines.push(Line::default());
    }
    let help_at = lines.len();
    lines.extend(help_lines.into_iter().map(|line| Line::styled(line, Style::default().fg(dim()).add_modifier(Modifier::ITALIC))));
    lines.resize(help_at + help_room, Line::default());

    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let guide_block = block("Getting started", true).padding(Padding::horizontal(1));
    let inner = guide_block.inner(rect);
    frame.render_widget(Paragraph::new(lines).block(guide_block), rect);
    let at = |line: usize| inner.y + line as u16;
    for (line, row) in placed {
        if (top..top + shown).contains(&line) && at(body_at + line - top) < inner.bottom() {
            app.regions.guide_rows.push((Rect::new(inner.x, at(body_at + line - top), inner.width, 1), row));
        }
    }
    if shown < body_len {
        let bar = Rect::new(rect.right().saturating_sub(1), at(body_at), 1, (shown as u16).min(inner.bottom().saturating_sub(at(body_at))));
        let mut position = ScrollbarState::new(body_len - shown + 1).position(top).viewport_content_length(shown);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(None).end_symbol(None).track_symbol(Some("│"));
        frame.render_stateful_widget(scrollbar, bar, &mut position);
    }
    if at(buttons_at) < inner.bottom() {
        let apply_width = apply.width() as u16;
        app.regions.guide_rows.push((Rect::new(inner.x, at(buttons_at), apply_width, 1), GuideRow::Apply));
        app.regions.guide_rows.push((Rect::new(inner.x + apply_width + 2, at(buttons_at), later.width() as u16, 1), GuideRow::Later));
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::guide::GuideRow;
    use crate::app::reach::Reach;
    use crate::config::Settings;
    use crate::store::Store;

    #[test]
    fn the_guide_fits_a_small_terminal_and_takes_clicks() {
        let dir = std::env::temp_dir().join(format!("rettui-guide-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = Settings { welcomed: false, rns_config: Some(dir.join("rns").display().to_string()), ..Settings::default() };
        let mut app = crate::app::test_app(&dir, settings, Store::default());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut draw = |app: &mut crate::app::App| {
            terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
            let buffer = terminal.backend().buffer();
            (0..24).map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect::<String>()
        };
        let apply_at = |app: &crate::app::App| app.regions.guide_rows.iter().find(|(_, row)| *row == GuideRow::Apply).unwrap().0;
        let screen = draw(&mut app);
        // At 80×24 the intro gives way, and the choices scroll.
        assert!(screen.contains("Getting started") && screen.contains("Not connected to anyone yet"), "{screen}");
        assert!(screen.contains("Entry points · Primary & global backbone"), "{screen}");
        // While they're tried, the first 3 are ticked.
        assert!(screen.contains("[x] RNS Between The Borders   checking…"), "{screen}");
        assert!(screen.contains("[x] RNS Simply Equipped       checking…"), "{screen}");
        assert!(screen.contains("[ ] RNS Beleth                checking…"), "{screen}");
        assert!(screen.contains("Apply") && screen.contains("Not now") && screen.contains("The name sent"), "{screen}");
        assert!(!screen.contains("Using rettui"), "below, for now: {screen}");
        let first = apply_at(&app);
        // Every row comes into view when picked, where a click picks it;
        // the longest help is shown whole, and the guide keeps its size.
        let rows = crate::app::guide::Guide::rows(&app.guide_view());
        for (index, row) in rows.iter().enumerate() {
            app.guide.as_mut().unwrap().row = index;
            let screen = draw(&mut app);
            assert_eq!(apply_at(&app), first, "{row:?}");
            assert!(app.regions.guide_rows.iter().any(|(_, shown)| shown == row), "{row:?} in view: {screen}");
            match row {
                // Ticked the first time, with the warning under it.
                GuideRow::AutoPropagation => {
                    assert!(screen.contains("[x] Pick a propagation node automatically"), "{screen}");
                    assert!(screen.contains("answers fastest.") && screen.contains("Warning: anyone can run a"), "{screen}");
                    assert!(screen.contains("the Network tab."), "the warning whole: {screen}");
                    let space = || crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char(' '), KeyModifiers::NONE);
                    app.on_key(space());
                    let screen = draw(&mut app);
                    assert!(screen.contains("[ ] Pick a propagation node automatically") && !screen.contains("Warning:"), "{screen}");
                    app.on_key(space());
                }
                GuideRow::Connect(i) => {
                    let entry = crate::app::guide::ENTRY_POINTS[*i];
                    assert!(screen.contains(&format!("{}:{}. A public transport node", entry.host, entry.port)), "{screen}");
                }
                // A link's note, then where it goes, whole.
                GuideRow::Link(i) => {
                    let (title, url, _) = crate::app::guide::LINKS[*i];
                    assert!(screen.contains(title) && screen.contains(url) && screen.contains("copies"), "{screen}")
                }
                _ => {}
            }
        }
        // As the entry points answer: how quickly, the 3 fastest ticked;
        // or greyed out and not to be ticked.
        let count = crate::app::guide::ENTRY_POINTS.len();
        let answered = |ms: [u64; 6]| ms.map(|ms| if ms == 0 { Reach::Down("refused") } else { Reach::Up(std::time::Duration::from_millis(ms)) }).to_vec();
        app.entry_reach.set(answered([0, 120, 250, 382, 600, 90]));
        app.guide.as_mut().unwrap().row = 2;
        let screen = draw(&mut app);
        assert!(screen.contains("[ ] RNS Beleth                up · 382 ms"), "{screen}");
        assert!(screen.contains("[ ] RNS Between The Borders   down · refused"), "{screen}");
        assert!(screen.contains("[x] RMAP World                up · 120 ms"), "{screen}");
        assert!(screen.contains("[x] RNS Simply Equipped       up · 250 ms"), "{screen}");
        assert!(screen.contains("[x] Ratspeak & Colorado Mesh  up · 90 ms"), "{screen}");
        let (rect, _) = *app.regions.guide_rows.iter().find(|(_, row)| *row == GuideRow::Connect(0)).unwrap();
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: rect.y, modifiers: KeyModifiers::NONE });
        assert!(draw(&mut app).contains("[ ] RNS Between The Borders   down · refused"));
        assert!(draw(&mut app).contains("It didn't answer just now (refused)"));
        // None answering: said.
        app.entry_reach.set(vec![Reach::Down("no answer"); count]);
        assert!(draw(&mut app).contains("None of the entry points answered"));
        app.guide.as_mut().unwrap().row = 0;
        draw(&mut app);
        // Discovery is ticked to start with; a click on its row unticks it.
        assert!(screen.contains("[x] Also find entry points"), "{screen}");
        let (rect, _) = *app.regions.guide_rows.iter().find(|(_, row)| *row == GuideRow::Discover).unwrap();
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: rect.y, modifiers: KeyModifiers::NONE });
        assert!(!app.guide.as_ref().unwrap().choices.discover);
        // Not now closes it for good.
        let (rect, _) = *app.regions.guide_rows.iter().find(|(_, row)| *row == GuideRow::Later).unwrap();
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: rect.y, modifiers: KeyModifiers::NONE });
        assert!(app.guide.is_none() && app.saved_settings().unwrap().welcomed);
        // Taller, it all fits, the intro too.
        app.open_guide();
        let mut taller = Terminal::new(TestBackend::new(80, 46)).unwrap();
        taller.draw(|frame| crate::ui::draw(frame, &mut app)).unwrap();
        let buffer = taller.backend().buffer();
        let screen: String = (0..46).map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect();
        assert!(screen.contains("Reticulum reaches others") && screen.contains("Using rettui") && screen.contains("Not now"), "{screen}");
        assert_eq!(app.regions.guide_rows.len(), crate::app::guide::Guide::rows(&app.guide_view()).len(), "{screen}");
        // Apply adds the ones ticked, as shown.
        app.entry_reach.set(answered([0, 120, 250, 382, 600, 90]));
        let (rect, _) = *app.regions.guide_rows.iter().find(|(_, row)| *row == GuideRow::Apply).unwrap();
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: rect.y, modifiers: KeyModifiers::NONE });
        let config = std::fs::read_to_string(dir.join("rns").join("config")).unwrap();
        let added: Vec<&str> = crate::app::guide::ENTRY_POINTS.iter().map(|entry| entry.host).filter(|host| config.contains(&format!("target_host = {host}"))).collect();
        assert_eq!(added, ["rmap.world", "rns.simplyequipped.com", "rns.ratspeak.org"], "{config}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
