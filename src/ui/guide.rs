//! The getting-started guide over the screen (see [`crate::app::guide`]):
//! one line a choice, with what the chosen one does at the bottom.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{DIM, PICKED, block, wrap};
use crate::app::App;
use crate::app::guide::{self, Guide, GuideRow, LINKS};

/// The guide's widest, in columns.
const WIDTH: u16 = 84;

fn check(on: bool) -> &'static str {
    if on { "[x]" } else { "[ ]" }
}

/// What a row does, shown under the guide when it's picked.
fn help(row: Option<GuideRow>) -> String {
    match row {
        Some(GuideRow::Name) => guide::NAME_HELP.to_string(),
        Some(GuideRow::Identity) => guide::IDENTITY_HELP.to_string(),
        Some(GuideRow::Connect) => guide::CONNECT_HELP.to_string(),
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

pub(super) fn draw_guide(frame: &mut Frame, app: &mut App) {
    app.regions.guide_rows.clear();
    let Some(state) = app.guide.clone() else { return };
    let view = app.guide_view();
    let rows = Guide::rows(&view);
    let area = frame.area();
    let width = WIDTH.min(area.width.saturating_sub(2));
    let inner_width = width.saturating_sub(4) as usize;
    let selected = rows.get(state.row).copied();
    let choices = &state.choices;

    let intro: Vec<Line<'static>> = wrap(guide::INTRO, inner_width).into_iter().map(|l| Line::styled(l, Style::default().fg(DIM))).collect();
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
        let style = if Some(row) == selected { Style::default().bg(super::SELECTED_BG).add_modifier(Modifier::BOLD) } else { Style::default() };
        placed.push((body.len(), row));
        body.push(Line::from(spans).style(style));
    };
    row_line(&mut body, GuideRow::Name, vec![Span::styled("Your name   ", Style::default().fg(DIM)), Span::raw(choices.name.clone())]);
    let identity = match (view.identity_pending, view.address) {
        (Some(next), _) => vec![Span::raw(format!("{} from the next start", hex::encode(next)))],
        (None, address) => vec![
            Span::raw(address.map(hex::encode).unwrap_or_else(|| "(starting)".into())),
            Span::styled("  · use one you already have", Style::default().fg(DIM)),
        ],
    };
    row_line(&mut body, GuideRow::Identity, [vec![Span::styled("Identity    ", Style::default().fg(DIM))], identity].concat());
    if view.external {
        for line in wrap(&format!("{}: add entry points in that program's Reticulum config.", crate::reticulum::EXTERNAL_NOTE), inner_width) {
            body.push(Line::styled(line, Style::default().fg(DIM)));
        }
    } else {
        let connect = if view.has_entry_point {
            "[x] Connect through RMAP World (in your Reticulum config already)".to_string()
        } else {
            format!("{} Connect through RMAP World ({}:{}), a community entry point", check(choices.connect), guide::ENTRY_HOST, guide::ENTRY_PORT)
        };
        row_line(&mut body, GuideRow::Connect, vec![Span::raw(connect)]);
        let discover = if view.has_discovery {
            "[x] Find entry points near you over time (on already)".to_string()
        } else {
            format!("{} Also find entry points near you over time (interface discovery)", check(choices.discover))
        };
        row_line(&mut body, GuideRow::Discover, vec![Span::raw(discover)]);
    }
    row_line(&mut body, GuideRow::AutoPropagation, vec![Span::raw(format!("{} Pick a propagation node automatically", check(choices.auto_propagation)))]);
    body.push(Line::styled("Learn more", Style::default().fg(DIM)));
    for (i, (title, ..)) in LINKS.iter().enumerate() {
        row_line(&mut body, GuideRow::Link(i), vec![Span::raw("↗ "), Span::styled(*title, Style::default().add_modifier(Modifier::UNDERLINED))]);
    }

    // The two buttons share a line.
    let button = |label: &str, row: GuideRow| {
        let style = if Some(row) == selected { PICKED } else { Style::default().fg(Color::Black).bg(DIM) };
        Span::styled(format!(" {label} "), style)
    };
    let (apply, later) = (" Apply ", " Not now ");
    let buttons = Line::from(vec![button(apply.trim(), GuideRow::Apply), Span::raw("  "), button(later.trim(), GuideRow::Later)]);

    // What the chosen row does, with room for the longest so the guide
    // keeps its size moving between rows.
    let help_lines = wrap(&help(selected), inner_width);
    let help_room = rows.iter().map(|row| wrap(&help(Some(*row)), inner_width).len()).max().unwrap_or(0).max(help_lines.len());

    // On a short screen, the intro and then the blank lines between parts
    // are left out before anything is cut off.
    let mut spare = (area.height.saturating_sub(2) as usize).saturating_sub(1 + body.len() + 1 + help_room);
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
    let body_at = lines.len();
    lines.extend(body);
    if gap_before_buttons {
        lines.push(Line::default());
    }
    let buttons_at = lines.len();
    lines.push(buttons);
    if gap_before_help {
        lines.push(Line::default());
    }
    let help_at = lines.len();
    lines.extend(help_lines.into_iter().map(|line| Line::styled(line, Style::default().fg(DIM).add_modifier(Modifier::ITALIC))));
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
        if at(body_at + line) < inner.bottom() {
            app.regions.guide_rows.push((Rect::new(inner.x, at(body_at + line), inner.width, 1), row));
        }
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
        assert!(screen.contains("Getting started") && screen.contains("Reticulum reaches others") && screen.contains("Not connected to anyone yet"), "{screen}");
        assert!(screen.contains("[x] Connect through RMAP World") && screen.contains("Using a LoRa radio (RNode)") && screen.contains("Words you'll meet") && screen.contains("Using rettui"), "{screen}");
        assert!(screen.contains("Apply") && screen.contains("Not now") && screen.contains("The name sent"), "{screen}");
        let first = apply_at(&app);
        // The longest help is shown whole, and the guide keeps its size.
        let rows = crate::app::guide::Guide::rows(&app.guide_view());
        for (index, row) in rows.iter().enumerate() {
            app.guide.as_mut().unwrap().row = index;
            let screen = draw(&mut app);
            assert_eq!(apply_at(&app), first, "{row:?}");
            match row {
                GuideRow::AutoPropagation => assert!(screen.contains("read them."), "{screen}"),
                // A link's note, then where it goes, whole.
                GuideRow::Link(i) => {
                    let (_, url, _) = crate::app::guide::LINKS[*i];
                    assert!(screen.contains(url) && screen.contains("copies"), "{screen}")
                }
                _ => {}
            }
        }
        app.guide.as_mut().unwrap().row = 0;
        draw(&mut app);
        // A click on the discovery row ticks it.
        let (rect, _) = *app.regions.guide_rows.iter().find(|(_, row)| *row == GuideRow::Discover).unwrap();
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: rect.y, modifiers: KeyModifiers::NONE });
        assert!(app.guide.as_ref().unwrap().choices.discover);
        // Not now closes it for good.
        let (rect, _) = *app.regions.guide_rows.iter().find(|(_, row)| *row == GuideRow::Later).unwrap();
        app.on_mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: rect.x + 1, row: rect.y, modifiers: KeyModifiers::NONE });
        assert!(app.guide.is_none() && app.saved_settings().unwrap().welcomed);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
