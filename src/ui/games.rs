//! The Games tab: your games with everyone, newest first, and the
//! selected one's board, with where it stands and what can be done; and
//! starting a new one (`n`, or `g` in a conversation). See
//! `crate::app::games`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::{accent, block, dim, selected_bg};
use crate::app::App;
use crate::app::games::{BoardRegion, GameFilter, GameMenu, MenuItem};
use crate::games::{Board, GameView};

/// The other player's pieces (yours are in the accent colour).
const THEIRS: Color = Color::Yellow;
/// A chess board's squares, which look the same in every theme.
const LIGHT_SQUARE: Color = Color::Rgb(0xb5, 0x88, 0x63);
const DARK_SQUARE: Color = Color::Rgb(0x7a, 0x55, 0x3a);
const LAST_MOVE: Color = Color::Rgb(0x9a, 0x9a, 0x4a);
/// A Reversi board's green.
const FELT: Color = Color::Rgb(0x2e, 0x6b, 0x3a);
/// Discs and stones that are black or white whatever the theme.
const WHITE_PIECE: Color = Color::Rgb(0xff, 0xff, 0xff);
const BLACK_PIECE: Color = Color::Rgb(0x10, 0x10, 0x10);

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let (width, height) = (width.min(area.width), height.min(area.height));
    Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 2, width, height }
}

/// A row padded to `width`, so a selected one is highlighted across.
fn padded(mut spans: Vec<Span<'static>>, width: usize, style: Style) -> Line<'static> {
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    if used < width {
        spans.push(Span::styled(" ".repeat(width - used), style));
    }
    Line::from(spans)
}

/// The Games tab.
pub(super) fn draw_games(frame: &mut Frame, app: &mut App, area: Rect) {
    app.sync_game_selection();
    let [list_area, board_area] =
        Layout::horizontal([Constraint::Length(super::side_width(app, area, 34)), Constraint::Min(20)]).areas(area);
    draw_list(frame, app, list_area);
    draw_board_pane(frame, app, board_area);
}

/// Your games: their picture (icon or face), the game and who with, and
/// where it stands; two rows each.
fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let games = app.game_list();
    let outer = block("Games", !app.game_playing);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    // The filters, a line above the list.
    app.regions.games_filters.clear();
    let mut x = inner.x;
    let mut filters = Vec::new();
    for filter in GameFilter::ALL {
        let label = format!(" {} ", filter.label());
        let width = (label.width() as u16).min(inner.right().saturating_sub(x));
        app.regions.games_filters.push((Rect { x, y: inner.y, width, height: 1 }, filter));
        x += width;
        let style =
            if filter == app.game_filter { Style::default().fg(Color::Black).bg(accent()).bold() } else { Style::default().fg(dim()) };
        filters.push(Span::styled(label, style));
    }
    frame.render_widget(Paragraph::new(Line::from(filters)), Rect { height: 1, ..inner });
    let inner = Rect { y: inner.y + 2, height: inner.height.saturating_sub(2), ..inner };
    app.regions.games_list = inner;
    if games.is_empty() && app.game_filter != GameFilter::All {
        let hint = format!("No {} games.\n\nPress f for the others, or n for a new game.", app.game_filter.label().to_lowercase());
        frame.render_widget(Paragraph::new(hint).wrap(Wrap { trim: true }).style(Style::default().fg(dim())), inner);
        return;
    }
    if games.is_empty() {
        let hint = "No games yet.\n\nPress n to challenge someone to Tic-Tac-Toe, Four in a Row, Dots and Boxes, Gomoku, Reversi, \
                    Mancala, Checkers, Chess or Hex (or g in a conversation). \
                    Each move goes as a message, so they can answer whenever they're next online.";
        frame.render_widget(Paragraph::new(hint).wrap(Wrap { trim: true }).style(Style::default().fg(dim())), inner);
        return;
    }
    // Two lines a game; the last may show only its first.
    let rows = (inner.height as usize).div_ceil(2);
    if rows == 0 {
        return;
    }
    let selected = app.game_selected.min(games.len() - 1);
    let first = app.regions.games_first.min(selected).max((selected + 1).saturating_sub(rows));
    app.regions.games_first = first;
    let pictures = inner.width >= 24;
    let width = inner.width as usize;
    for (i, game) in games.iter().enumerate().skip(first).take(rows) {
        let y = inner.y + ((i - first) * 2) as u16;
        let base = if i == selected { Style::default().bg(selected_bg()) } else { Style::default() };
        let gap = if pictures { "     " } else { " " };
        let name = app.store.display_name(&game.peer);
        let mark = if game.yours { Span::styled("●", base.fg(accent())) } else { Span::styled(" ", base) };
        let top = padded(
            vec![Span::styled(gap, base), Span::styled(game.name.clone(), base.bold()), Span::styled(format!(" · {name} "), base), mark],
            width,
            base,
        );
        let summary = if game.yours { base.fg(accent()) } else { base.fg(dim()) };
        let bottom = padded(vec![Span::styled(gap, base), Span::styled(game.summary.clone(), summary)], width, base);
        let buffer = frame.buffer_mut();
        buffer.set_line(inner.x, y, &top, inner.width);
        if y + 1 < inner.bottom() {
            buffer.set_line(inner.x, y + 1, &bottom, inner.width);
        }
        if pictures {
            let letter = name.chars().next().and_then(|c| c.to_uppercase().next()).unwrap_or('?');
            let icon = app.store.contacts.get(&game.peer).and_then(|c| c.icon.as_ref());
            for (dy, line) in super::face::small(&game.peer, icon, letter).into_iter().enumerate() {
                if y + (dy as u16) < inner.bottom() {
                    buffer.set_line(inner.x, y + dy as u16, &Line::from(line), 4);
                }
            }
        }
    }
}

/// The selected game: where it stands, its board, and what can be done.
fn draw_board_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    app.regions.game_board = BoardRegion::default();
    let game = app.game_view.clone().and_then(|screen| app.games.view(&screen.app, &screen.session).map(|game| (screen, game)));
    let Some((screen, game)) = game else {
        frame.render_widget(block("", false), area);
        return;
    };
    let name = app.store.display_name(&game.peer);
    let title = format!("{} with {name}", game.name);
    let keys = if app.game_playing {
        hints(&game, app.game_unsent(&game))
    } else {
        "Enter (or a click) to play · n new game · f filter · c conversation · x forget".to_string()
    };
    let outer = block(&title, app.game_playing).title_bottom(Line::styled(format!(" {keys} "), Style::default().fg(dim())));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let (board_w, board_h) = match &game.board {
        Board::TicTacToe { .. } => (17, 5),
        Board::FourInARow { .. } => (30, 7),
        Board::Chess { .. } | Board::Checkers { .. } | Board::Reversi { .. } => (28, 9),
        Board::Hex { .. } => (38, 13),
        Board::Gomoku { .. } => (33, 16),
        Board::DotsAndBoxes { .. } => (23, 12),
        Board::Mancala { .. } => (36, 5),
    };
    // The board in the middle, where it stands above it, what you are
    // below, and how your last move went (and what came with theirs).
    let notes = app.game_notes(&game);
    let notes_h = if notes.is_empty() { 0 } else { notes.len() as u16 + 1 };
    let column = centered(inner, inner.width, board_h + 4 + notes_h);
    let status = if game.yours { Style::default().fg(accent()).bold() } else { Style::default().bold() };
    frame.render_widget(Paragraph::new(Span::styled(game.summary.clone(), status)).centered(), Rect { height: 1, ..column });
    let board_area =
        Rect { x: column.x + column.width.saturating_sub(board_w) / 2, y: column.y + 2, width: board_w.min(column.width), height: board_h };
    // The cursor only shows while you're on the board.
    let cursor = if app.game_playing { screen.cursor } else { usize::MAX };
    let region = match &game.board {
        Board::TicTacToe { cells, mine } => draw_tic_tac_toe(frame, board_area, cells, *mine, cursor),
        Board::FourInARow { cells, mine, last } => {
            draw_four_in_a_row(frame, board_area, cells, *mine, *last, cursor, game.yours && app.game_playing)
        }
        Board::Chess { fen, white, last, .. } => draw_chess(frame, board_area, fen, *white, last.as_deref(), cursor, screen.from),
        Board::Checkers { cells, black, last, .. } => {
            draw_checkers(frame, board_area, cells, *black, last.as_deref(), cursor, &screen.path)
        }
        Board::Hex { cells, black, last, .. } => draw_hex(frame, board_area, cells, *black, last.as_deref(), cursor),
        Board::Gomoku { cells, black, last, .. } => draw_gomoku(frame, board_area, cells, *black, last.as_deref(), cursor),
        Board::Reversi { cells, legal, last, .. } => draw_reversi(frame, board_area, cells, legal, last.as_deref(), cursor),
        Board::DotsAndBoxes { rows, black, last, .. } => draw_dots(frame, board_area, rows, *black, last.as_deref(), cursor),
        Board::Mancala { holes, black, legal, last, .. } => draw_mancala(frame, board_area, holes, *black, legal, last.as_deref(), cursor),
    };
    app.regions.game_board = BoardRegion { area: inner, ..region };
    let below = Rect { y: board_area.y + board_area.height + 1, height: 1, ..column };
    frame.render_widget(Paragraph::new(Span::styled(footnote(&game), Style::default().fg(dim()))).centered(), below);
    if !notes.is_empty() {
        let area = Rect { y: below.y + 2, height: inner.bottom().saturating_sub(below.y + 2), ..column };
        let lines: Vec<Line> = notes.into_iter().map(|note| Line::styled(note, Style::default().fg(THEIRS))).collect();
        frame.render_widget(Paragraph::new(lines).centered().wrap(Wrap { trim: true }), area);
    }
}

/// Starting a new game: who with (your conversations), then which game.
pub(super) fn draw_game_menu(frame: &mut Frame, app: &mut App) {
    let Some(menu) = app.game_menu.clone() else { return };
    let items = app.game_menu_items();
    let area = frame.area();
    let rect = centered(area, 60, (items.len() as u16).clamp(1, area.height.saturating_sub(8)) + 4);
    let title = match &menu {
        GameMenu { peer: Some(peer), .. } => format!("New game with {}", app.store.display_name(peer)),
        GameMenu { peer: None, .. } => "New game: who with?".to_string(),
    };
    let outer = block(&title, true).title_bottom(Line::styled(" Enter pick · ↑↓ choose · Esc cancel ", Style::default().fg(dim())));
    let inner = outer.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(outer, rect);
    app.regions.game_menu_box = rect;
    let list = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    app.regions.game_menu = list;
    let width = list.width as usize;
    let first = menu.selected.saturating_sub((list.height as usize).saturating_sub(1));
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .skip(first)
        .take(list.height as usize)
        .map(|(i, item)| {
            let base = if i == menu.selected { Style::default().bg(selected_bg()) } else { Style::default() };
            let spans = match item {
                MenuItem::Contact(key) => {
                    vec![Span::styled(app.store.display_name(key), base.bold()), Span::styled(format!("  {}", &key[..12]), base.fg(dim()))]
                }
                MenuItem::New(app) => vec![Span::styled(crate::games::game_name(app).to_string(), base.bold())],
            };
            padded(spans, width, base)
        })
        .collect();
    app.regions.game_menu = Rect { height: lines.len() as u16, ..list };
    let hint = match (&menu.peer, items.is_empty()) {
        (None, true) => "No conversations yet: start one in Messages (n) first",
        (None, false) => "Your conversations",
        (Some(_), _) => "Your challenge goes as a message; they accept or decline",
    };
    frame.render_widget(Paragraph::new(Span::styled(hint, Style::default().fg(dim()))), Rect { height: 1, ..inner });
    frame.render_widget(Paragraph::new(lines), list);
}

/// The keys that do something now, on the board.
fn hints(game: &GameView, unsent: bool) -> String {
    let mut keys: Vec<&str> = Vec::new();
    if unsent {
        keys.push("r send again");
    }
    let can = |command: &str| game.actions.iter().any(|a| a == command);
    if can("accept") {
        keys.extend(["a accept", "d decline"]);
    }
    if game.yours && game.status == "active" && game.draw_offer != Some(true) {
        keys.push(match game.app.as_str() {
            "chess" | "checkers" => "Enter pick/move · t type",
            "hex" | "gomoku" | "reversi" | "dots_and_boxes" => "Enter play · t type",
            "mancala" => "Enter or 1–6 sow",
            _ => "Enter play",
        });
        if matches!(&game.board, Board::Hex { legal, .. } if legal.iter().any(|m| m == "swap")) {
            keys.push("s swap");
        }
    }
    if can("draw_accept") {
        keys.extend(["y accept draw", "n refuse"]);
    }
    if can("draw_offer") {
        keys.push("D offer draw");
    }
    if can(crate::games::DRAW_CLAIM) {
        keys.push("C claim draw");
    }
    if can("resign") {
        keys.push("R resign");
    }
    if !matches!(game.status.as_str(), "pending" | "active") {
        keys.push("x forget");
    }
    keys.push("Esc list");
    keys.join(" · ")
}

fn footnote(game: &GameView) -> String {
    match &game.board {
        Board::TicTacToe { mine, .. } => format!("You're {mine}"),
        Board::FourInARow { .. } => "You're ● in your colour; they're ● in yellow".to_string(),
        Board::Chess { white, moves, .. } => {
            let side = if *white { "White" } else { "Black" };
            let shown: Vec<String> = moves
                .chunks(2)
                .enumerate()
                .map(|(i, pair)| format!("{}. {}", i + 1, pair.join(" ")))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .take(4)
                .rev()
                .collect();
            if shown.is_empty() { format!("You're {side}") } else { format!("You're {side} · {}", shown.join("  ")) }
        }
        Board::Checkers { black, last, must_capture, legal, .. } => {
            let mut said = vec![if *black { "You're Black (dark pieces)" } else { "You're White (light pieces)" }.to_string()];
            if *must_capture && !legal.is_empty() {
                said.push("you must jump".into());
            }
            if let Some(last) = last {
                said.push(format!("last {last}"));
            }
            said.join(" · ")
        }
        Board::Hex { black, last, .. } => {
            let side = if *black { "You're Black: join top and bottom" } else { "You're White: join left and right" };
            with_last(side.to_string(), last)
        }
        Board::Gomoku { black, last, .. } => {
            let side = if *black { "You're Black" } else { "You're White" };
            with_last(format!("{side}, in your colour · five in a row wins"), last)
        }
        Board::Reversi { black, last, black_count, white_count, .. } => {
            let (side, mine, theirs) = if *black { ("Black", black_count, white_count) } else { ("White", white_count, black_count) };
            with_last(format!("You're {side} · discs: you {mine}, them {theirs}"), last)
        }
        Board::DotsAndBoxes { black, last, black_boxes, white_boxes, .. } => {
            let (first, mine, theirs) =
                if *black { ("you went first", black_boxes, white_boxes) } else { ("they went first", white_boxes, black_boxes) };
            with_last(format!("Boxes: you {mine}, them {theirs} · {first}"), last)
        }
        Board::Mancala { black, last, holes, .. } => {
            let (mine, theirs) = if *black { (holes[6], holes[13]) } else { (holes[13], holes[6]) };
            let first = if *black { "you went first" } else { "they went first" };
            with_last(format!("Your pits below, your store on the right: {mine} to {theirs} · {first}"), last)
        }
    }
}

fn with_last(said: String, last: &Option<String>) -> String {
    match last {
        Some(last) => format!("{said} · last {last}"),
        None => said,
    }
}

/// The colours of the pieces of the side moving first (Black) and second,
/// as yours or theirs.
fn side_colours(black: bool) -> (Color, Color) {
    if black { (accent(), THEIRS) } else { (THEIRS, accent()) }
}

/// A stone's colour: Black's (`b`) or White's.
fn stone_colour(stone: char, black: bool) -> Color {
    let (first, second) = side_colours(black);
    if stone == 'b' { first } else { second }
}

/// A Hex board, a rhombus: each row set right of the one above. Black's
/// edges (top and bottom) are lettered in Black's colour, White's (left and
/// right) numbered in White's.
fn draw_hex(frame: &mut Frame, area: Rect, cells: &[char], black: bool, last: Option<&str>, cursor: usize) -> BoardRegion {
    let (black_colour, white_colour) = side_colours(black);
    let last = last.and_then(|m| crate::games::grid_square_index(m, 11));
    let letters: String = (0..11).map(|f| format!("{} ", (b'a' + f) as char)).collect();
    let mut lines = vec![Line::styled(format!("   {letters}"), Style::default().fg(black_colour))];
    for row in 0..11 {
        let mut spans = vec![Span::raw(" ".repeat(row)), Span::styled(format!("{:>2} ", 11 - row), Style::default().fg(white_colour))];
        for col in 0..11 {
            let i = row * 11 + col;
            let stone = cells.get(i).copied().unwrap_or(' ');
            let mut style = if stone == ' ' { Style::default().fg(dim()) } else { Style::default().fg(stone_colour(stone, black)).bold() };
            if last == Some(i) {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if i == cursor {
                style = style.bg(selected_bg());
            }
            spans.push(Span::styled(if stone == ' ' { "·" } else { "●" }, style));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(format!("{}", 11 - row), Style::default().fg(white_colour)));
        lines.push(Line::from(spans));
    }
    lines.push(Line::styled(format!("{}   {letters}", " ".repeat(11)), Style::default().fg(black_colour)));
    frame.render_widget(Paragraph::new(lines), area);
    BoardRegion { origin: Position::new(area.x + 3, area.y + 1), cell: (2, 1), across: 11, down: 11, skew: 1, area, ..Default::default() }
}

/// A Gomoku board: fifteen points by fifteen.
fn draw_gomoku(frame: &mut Frame, area: Rect, cells: &[char], black: bool, last: Option<&str>, cursor: usize) -> BoardRegion {
    let last = last.and_then(|m| crate::games::grid_square_index(m, 15));
    let mut lines = Vec::new();
    for row in 0..15 {
        let mut spans = vec![Span::styled(format!("{:>2} ", 15 - row), Style::default().fg(dim()))];
        for col in 0..15 {
            let i = row * 15 + col;
            let stone = cells.get(i).copied().unwrap_or(' ');
            let mut style = if stone == ' ' { Style::default().fg(dim()) } else { Style::default().fg(stone_colour(stone, black)).bold() };
            if last == Some(i) {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if i == cursor {
                style = style.bg(selected_bg());
            }
            spans.push(Span::styled(if stone == ' ' { "·" } else { "●" }, style));
            spans.push(Span::raw(" "));
        }
        lines.push(Line::from(spans));
    }
    let letters: String = (0..15).map(|f| format!("{} ", (b'a' + f) as char)).collect();
    lines.push(Line::styled(format!("   {letters}"), Style::default().fg(dim())));
    frame.render_widget(Paragraph::new(lines), area);
    BoardRegion { origin: Position::new(area.x + 3, area.y), cell: (2, 1), across: 15, down: 15, area, ..Default::default() }
}

/// A Reversi board: black and white discs on green, and (your move) dots
/// where a disc can go.
fn draw_reversi(frame: &mut Frame, area: Rect, cells: &[char], legal: &[String], last: Option<&str>, cursor: usize) -> BoardRegion {
    let last = last.and_then(|m| crate::games::grid_square_index(m, 8));
    let mut lines = Vec::new();
    for row in 0..8 {
        let mut spans = vec![Span::styled(format!("{} ", 8 - row), Style::default().fg(dim()))];
        for col in 0..8 {
            let i = row * 8 + col;
            let disc = cells.get(i).copied().unwrap_or(' ');
            let mut bg = FELT;
            if last == Some(i) {
                bg = LAST_MOVE;
            }
            if i == cursor {
                bg = accent();
            }
            let open = legal.contains(&crate::games::grid_square_name(i, 8));
            let (glyph, fg) = match disc {
                'b' => ('●', BLACK_PIECE),
                'w' => ('●', WHITE_PIECE),
                _ if open => ('·', WHITE_PIECE),
                _ => (' ', WHITE_PIECE),
            };
            spans.push(Span::styled(format!(" {glyph} "), Style::default().fg(fg).bg(bg).bold()));
        }
        lines.push(Line::from(spans));
    }
    let files: String = (0..8).map(|c| format!(" {} ", (b'a' + c) as char)).collect();
    lines.push(Line::styled(format!("  {files}"), Style::default().fg(dim())));
    frame.render_widget(Paragraph::new(lines), area);
    BoardRegion { origin: Position::new(area.x + 2, area.y), cell: (3, 1), across: 8, down: 8, area, ..Default::default() }
}

/// A Dots and Boxes grid: dots one wide, the lines and boxes between three;
/// boxes claimed are filled in their owner's colour.
fn draw_dots(frame: &mut Frame, area: Rect, rows: &[String], black: bool, last: Option<&str>, cursor: usize) -> BoardRegion {
    let last = last.and_then(crate::games::dots_place);
    let mut lines = Vec::new();
    for (row, text) in rows.iter().enumerate().take(11) {
        let label = if row % 2 == 0 { format!("{} ", 6 - row / 2) } else { "  ".to_string() };
        let mut spans = vec![Span::styled(label, Style::default().fg(dim()))];
        for (col, c) in text.chars().enumerate().take(11) {
            let here = row * 11 + col == cursor;
            let mut style = if last == Some((row, col)) { Style::default().fg(accent()).bold() } else { Style::default() };
            let shown = match (row % 2, col % 2) {
                (0, 0) => "•".to_string(),
                (0, _) if c == '-' => "───".to_string(),
                (0, _) if here => {
                    style = style.fg(dim());
                    "───".to_string()
                }
                (0, _) => "   ".to_string(),
                (_, 0) if c == '|' => "│".to_string(),
                (_, 0) if here => {
                    style = style.fg(dim());
                    "│".to_string()
                }
                (_, 0) => " ".to_string(),
                _ if c == 'b' || c == 'w' => {
                    style = style.fg(stone_colour(c, black));
                    " ■ ".to_string()
                }
                _ => "   ".to_string(),
            };
            if here {
                style = style.bg(selected_bg());
            }
            spans.push(Span::styled(shown, style));
        }
        lines.push(Line::from(spans));
    }
    let files: String = (0..6).map(|c| format!("{}   ", (b'a' + c) as char)).collect();
    lines.push(Line::styled(format!("  {files}"), Style::default().fg(dim())));
    frame.render_widget(Paragraph::new(lines), area);
    BoardRegion {
        origin: Position::new(area.x + 2, area.y),
        cell: (3, 1),
        across: 11,
        down: 11,
        narrow_even: true,
        area,
        ..Default::default()
    }
}

/// A Mancala board: your pits below, from your left, and your store on
/// the right; theirs above, the other way round, their store on the left.
fn draw_mancala(
    frame: &mut Frame,
    area: Rect,
    holes: &[u64],
    black: bool,
    legal: &[String],
    last: Option<&str>,
    cursor: usize,
) -> BoardRegion {
    let seeds = |hole: usize| holes.get(hole).copied().unwrap_or(0);
    let mine = crate::games::mancala_pits(black);
    let theirs = crate::games::mancala_pits(!black);
    let (my_store, their_store) = if black { (6, 13) } else { (13, 6) };
    let pit = |(letter, hole): (char, usize), at: Option<usize>| -> Span<'static> {
        let mut style = if seeds(hole) == 0 { Style::default().fg(dim()) } else { Style::default().bold() };
        if last.is_some_and(|m| m.starts_with(letter)) {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        if at.is_some_and(|i| i == cursor) {
            style = style.bg(selected_bg()).fg(if legal.contains(&letter.to_string()) { accent() } else { dim() });
        }
        Span::styled(format!("[{:>2}]", seeds(hole)), style)
    };
    let gap = Span::raw("      ");
    let label = |letter: char| Span::styled(format!("  {letter} "), Style::default().fg(dim()));
    let mut top = vec![gap.clone()];
    let mut top_labels = vec![gap.clone()];
    for &(letter, hole) in theirs.iter().rev() {
        top.push(pit((letter, hole), None).patch_style(Style::default().fg(THEIRS)));
        top_labels.push(label(letter));
    }
    let mut bottom = vec![gap.clone()];
    let mut bottom_labels = vec![gap];
    for (i, &(letter, hole)) in mine.iter().enumerate() {
        bottom.push(pit((letter, hole), Some(i)));
        bottom_labels.push(label(letter));
    }
    let stores = Line::from(vec![
        Span::styled(format!("[{:>2}]", seeds(their_store)), Style::default().fg(THEIRS).bold()),
        Span::raw(" ".repeat(28)),
        Span::styled(format!("[{:>2}]", seeds(my_store)), Style::default().fg(accent()).bold()),
    ]);
    let lines = vec![Line::from(top_labels), Line::from(top), stores, Line::from(bottom), Line::from(bottom_labels)];
    frame.render_widget(Paragraph::new(lines), area);
    BoardRegion { origin: Position::new(area.x + 6, area.y + 3), cell: (4, 1), across: 6, down: 1, area, ..Default::default() }
}

fn piece_style(piece: char, mine: char) -> Style {
    if piece == mine { Style::default().fg(accent()).bold() } else { Style::default().fg(THEIRS).bold() }
}

fn draw_tic_tac_toe(frame: &mut Frame, area: Rect, cells: &[char], mine: char, cursor: usize) -> BoardRegion {
    // Cells five wide, a line between them: 17 by 5 (and two lines spare).
    let mut lines = Vec::new();
    for row in 0..3 {
        let mut spans = Vec::new();
        for col in 0..3 {
            let i = row * 3 + col;
            let piece = cells.get(i).copied().unwrap_or(' ');
            let mut style = piece_style(piece, mine);
            if i == cursor {
                style = style.bg(selected_bg());
            }
            let shown = if piece == ' ' { format!("  {}  ", i + 1) } else { format!("  {piece}  ") };
            let style = if piece == ' ' { style.fg(dim()).remove_modifier(Modifier::BOLD) } else { style };
            spans.push(Span::styled(shown, style));
            if col < 2 {
                spans.push(Span::styled("│", Style::default().fg(dim())));
            }
        }
        lines.push(Line::from(spans));
        if row < 2 {
            lines.push(Line::styled("─────┼─────┼─────", Style::default().fg(dim())));
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
    // Clicks: three across, three down, every other row a cell.
    BoardRegion { origin: Position::new(area.x, area.y), cell: (6, 2), across: 3, down: 3, area, ..Default::default() }
}

fn draw_four_in_a_row(
    frame: &mut Frame,
    area: Rect,
    cells: &[char],
    mine: char,
    last: Option<usize>,
    cursor: usize,
    yours: bool,
) -> BoardRegion {
    // Columns four wide: the drop marker over the cursor's, then six rows.
    let column = cursor % 7;
    let mut lines = vec![Line::from(
        (0..7)
            .map(|c| {
                if c == column && yours {
                    Span::styled(format!(" {}▼ ", c + 1), Style::default().fg(accent()).bold())
                } else {
                    Span::styled(format!(" {}  ", c + 1), Style::default().fg(dim()))
                }
            })
            .collect::<Vec<_>>(),
    )];
    for row in 0..6 {
        let spans: Vec<Span> = (0..7)
            .map(|c| {
                let i = row * 7 + c;
                let piece = cells.get(i).copied().unwrap_or(' ');
                let mut style = if piece == ' ' { Style::default().fg(dim()) } else { piece_style(piece, mine) };
                if c == column && yours {
                    style = style.bg(selected_bg());
                }
                if last == Some(i) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                Span::styled(if piece == ' ' { " ·  " } else { " ●  " }, style)
            })
            .collect();
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);
    // A click anywhere in a column drops into it.
    BoardRegion { origin: Position::new(area.x, area.y), cell: (4, 7), across: 7, down: 1, area, ..Default::default() }
}

/// A chess piece's glyph: the filled ones for both sides, told apart by
/// colour (they read better than the outlined ones in a terminal). But
/// the pawn's outlined: the filled one (U+265F) is an emoji too, which
/// terminals may draw from an emoji font, in its own colours (both sides'
/// pawns alike), whatever colour it's given.
fn chess_glyph(piece: char) -> char {
    match piece.to_ascii_lowercase() {
        'k' => '♚',
        'q' => '♛',
        'r' => '♜',
        'b' => '♝',
        'n' => '♞',
        'p' => '♙',
        _ => ' ',
    }
}

fn draw_chess(
    frame: &mut Frame,
    area: Rect,
    fen: &str,
    white: bool,
    last: Option<&str>,
    cursor: usize,
    from: Option<usize>,
) -> BoardRegion {
    let squares = crate::games::chess_squares(fen);
    let last: Vec<usize> = last
        .map(|m| {
            let at = |s: &[u8]| (8 - (s[1] - b'0') as usize) * 8 + (s[0] - b'a') as usize;
            let b = m.as_bytes();
            if b.len() >= 4 { vec![at(&b[0..2]), at(&b[2..4])] } else { Vec::new() }
        })
        .unwrap_or_default();
    // Each square three wide; ranks on the left, files below. Black sees
    // the board turned round.
    let mut lines = Vec::new();
    for shown_row in 0..8 {
        let mut spans = Vec::new();
        let row = if white { shown_row } else { 7 - shown_row };
        spans.push(Span::styled(format!("{} ", 8 - row), Style::default().fg(dim())));
        for shown_col in 0..8 {
            let col = if white { shown_col } else { 7 - shown_col };
            let i = row * 8 + col;
            let piece = squares[i];
            let mut bg = if (row + col) % 2 == 0 { LIGHT_SQUARE } else { DARK_SQUARE };
            if last.contains(&i) {
                bg = LAST_MOVE;
            }
            if from == Some(i) {
                bg = Color::Rgb(0x5a, 0x8a, 0x3a);
            }
            if i == cursor {
                bg = accent();
            }
            let fg = if piece.is_ascii_uppercase() { Color::Rgb(0xff, 0xff, 0xff) } else { Color::Rgb(0x10, 0x10, 0x10) };
            spans.push(Span::styled(format!(" {} ", chess_glyph(piece)), Style::default().fg(fg).bg(bg).bold()));
        }
        lines.push(Line::from(spans));
    }
    let files: String = (0..8).map(|c| format!(" {} ", (b'a' + if white { c } else { 7 - c }) as char)).collect();
    lines.push(Line::styled(format!("  {files}"), Style::default().fg(dim())));
    frame.render_widget(Paragraph::new(lines), area);
    BoardRegion { origin: Position::new(area.x + 2, area.y), cell: (3, 1), across: 8, down: 8, area, ..Default::default() }
}

/// A checkers board: Black (dark pieces) at the top, as it starts, unless
/// you're Black. Men are discs, kings ringed ones.
fn draw_checkers(
    frame: &mut Frame,
    area: Rect,
    cells: &[char],
    black: bool,
    last: Option<&str>,
    cursor: usize,
    path: &[usize],
) -> BoardRegion {
    let index = |square: &str| {
        let b = square.as_bytes();
        (b.len() == 2).then(|| (8 - (b[1] - b'0') as usize) * 8 + (b[0] - b'a') as usize)
    };
    let last: Vec<usize> = last.map(|m| m.split(' ').filter_map(index).collect()).unwrap_or_default();
    let mut lines = Vec::new();
    for shown_row in 0..8 {
        let mut spans = Vec::new();
        let row = if black { 7 - shown_row } else { shown_row };
        spans.push(Span::styled(format!("{} ", 8 - row), Style::default().fg(dim())));
        for shown_col in 0..8 {
            let col = if black { 7 - shown_col } else { shown_col };
            let i = row * 8 + col;
            let piece = cells.get(i).copied().unwrap_or(' ');
            let mut bg = if (row + col) % 2 == 0 { LIGHT_SQUARE } else { DARK_SQUARE };
            if last.contains(&i) {
                bg = LAST_MOVE;
            }
            if path.contains(&i) {
                bg = Color::Rgb(0x5a, 0x8a, 0x3a);
            }
            if i == cursor {
                bg = accent();
            }
            let fg = if piece.eq_ignore_ascii_case(&'w') { Color::Rgb(0xff, 0xff, 0xff) } else { Color::Rgb(0x10, 0x10, 0x10) };
            let glyph = match piece {
                'b' | 'w' => '●',
                'B' | 'W' => '◉',
                _ => ' ',
            };
            spans.push(Span::styled(format!(" {glyph} "), Style::default().fg(fg).bg(bg).bold()));
        }
        lines.push(Line::from(spans));
    }
    let files: String = (0..8).map(|c| format!(" {} ", (b'a' + if black { 7 - c } else { c }) as char)).collect();
    lines.push(Line::styled(format!("  {files}"), Style::default().fg(dim())));
    frame.render_widget(Paragraph::new(lines), area);
    BoardRegion { origin: Position::new(area.x + 2, area.y), cell: (3, 1), across: 8, down: 8, area, ..Default::default() }
}

#[cfg(test)]
mod tests {
    #[test]
    fn no_chess_piece_is_drawn_as_an_emoji() {
        // The black pawn (U+265F) is the one chess symbol that's an emoji
        // too: drawn from an emoji font, its colour would be lost.
        for piece in "kqrbnpKQRBNP".chars() {
            assert_ne!(super::chess_glyph(piece), '\u{265F}', "{piece}");
        }
    }

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::config::Settings;
    use crate::store::Store;

    fn screen(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_games_tab_from_a_conversation_to_a_board() {
        use crate::app::Tab;
        use crossterm::event::KeyCode;
        let dir = std::env::temp_dir().join(format!("rettui-ui-games-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, _net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        app.games.set_me(&[0xa1; 16]);
        let peer = hex::encode([0xb2; 16]);
        app.add_contact(&peer).unwrap();
        app.active_conversation = Some(peer.clone());
        let mut terminal = Terminal::new(TestBackend::new(110, 34)).unwrap();
        let key = |app: &mut crate::app::App, code| app.on_key(crossterm::event::KeyEvent::from(code));
        let draw = |terminal: &mut Terminal<TestBackend>, app: &mut crate::app::App| {
            terminal.draw(|f| super::super::draw(f, app)).unwrap();
            screen(terminal)
        };

        // g in a conversation with no game yet: the Games tab, and a new
        // game with them to pick.
        key(&mut app, KeyCode::Char('g'));
        assert_eq!(app.tab, Tab::Games);
        let shown = draw(&mut terminal, &mut app);
        assert!(shown.contains("New game with") && shown.contains("Four in a Row") && shown.contains("⚄  Games"), "{shown}");
        // The second: Four in a Row.
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert!(app.game_menu.is_none());
        let shown = draw(&mut terminal, &mut app);
        assert!(shown.contains("Four in a Row with") && shown.contains("Waiting for them to accept"), "{shown}");
        assert!(shown.contains("Four in a Row ·"), "listed: {shown}");
        // The challenge went as a message, kept with the games' moves, not
        // in the conversation; being sent, it says so.
        let conversation = &app.store.conversations[&peer];
        assert!(conversation.messages.is_empty());
        assert_eq!(conversation.game_moves.last().unwrap().opening(), "🎲 Four in a Row · Sent a challenge!");
        assert!(shown.contains("Your last move is being sent"), "{shown}");
        // Filtered: it's pending, not active.
        key(&mut app, KeyCode::Char('f'));
        let shown = draw(&mut terminal, &mut app);
        assert!(shown.contains("No active games") && app.game_view.is_none(), "{shown}");
        key(&mut app, KeyCode::Char('f'));
        let shown = draw(&mut terminal, &mut app);
        assert!(shown.contains("Four in a Row ·") && app.game_view.is_some(), "pending: {shown}");
        key(&mut app, KeyCode::Char('f'));
        key(&mut app, KeyCode::Char('f'));
        assert_eq!(app.game_filter, crate::app::games::GameFilter::All);
        // Onto the board and back to the list.
        key(&mut app, KeyCode::Enter);
        assert!(app.game_playing);
        key(&mut app, KeyCode::Esc);
        assert!(!app.game_playing && app.game_view.is_some());
        // n: who with, from your conversations.
        key(&mut app, KeyCode::Char('n'));
        let shown = draw(&mut terminal, &mut app);
        assert!(shown.contains("New game: who with?"), "{shown}");
        key(&mut app, KeyCode::Esc);
        assert!(app.game_menu.is_none());
        // Numbers switch tabs from the list: Games is 2.
        key(&mut app, KeyCode::Char('1'));
        assert_eq!(app.tab, Tab::Messages);
        key(&mut app, KeyCode::Char('2'));
        assert_eq!(app.tab, Tab::Games);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `app`, a game begun between you (`0xa1…`) and Ben, with these moves
    /// made (by whoever's turn it is), drawn on the board.
    fn drawn(app_id: &str, moves: &[&str], dir: &str) -> (String, crate::app::App, String) {
        use std::collections::HashMap;

        use crate::games::{Games, fields, move_payload};
        let dir = std::env::temp_dir().join(format!("rettui-ui-{dir}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, _net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        let (me, them) = ([0xa1; 16], [0xb2; 16]);
        app.games.set_me(&me);
        let mut ben = Games::default();
        ben.set_me(&them);
        let wire = |prepared: &crate::games::Prepared| -> HashMap<u8, Vec<u8>> {
            fields(&hex::decode(prepared.game.envelope.as_ref().unwrap()).unwrap()).into_iter().collect()
        };
        let challenge = app.games.challenge(app_id, &hex::encode(them)).unwrap();
        let session = challenge.game.session.clone();
        ben.receive(&hex::encode(me), &wire(&challenge));
        app.games.receive(&hex::encode(them), &wire(&ben.act(app_id, &session, "accept", HashMap::new()).unwrap()));
        for m in moves {
            if app.games.view(app_id, &session).unwrap().yours {
                ben.receive(&hex::encode(me), &wire(&app.games.act(app_id, &session, "move", move_payload(app_id, m).unwrap()).unwrap()));
            } else {
                app.games.receive(&hex::encode(them), &wire(&ben.act(app_id, &session, "move", move_payload(app_id, m).unwrap()).unwrap()));
            }
        }
        app.show_game(app_id, &session);
        app.game_playing = true;
        let mut terminal = Terminal::new(TestBackend::new(110, 34)).unwrap();
        terminal.draw(|f| super::super::draw(f, &mut app)).unwrap();
        let _ = std::fs::remove_dir_all(dir);
        (screen(&terminal), app, session)
    }

    #[test]
    fn the_new_boards_drawn() {
        let (shown, ..) = drawn("hex", &["f6", "swap"], "hex");
        assert!(shown.contains("Hex with") && shown.contains("a b c d e f g h i j k"), "{shown}");
        assert!(shown.contains("join top and bottom") || shown.contains("join left and right"), "{shown}");
        // Stones on the board, right of the list.
        let stones = |shown: &str| shown.lines().map(|l| l.chars().skip(52).filter(|c| *c == '●').count()).sum::<usize>();
        assert_eq!(stones(&shown), 1, "{shown}");
        let (shown, ..) = drawn("gomoku", &["h8", "h9"], "gomoku");
        assert!(shown.contains("15 ·") && shown.contains("five in a row wins"), "{shown}");
        assert_eq!(stones(&shown), 2, "{shown}");
        let (shown, ..) = drawn("reversi", &["d3"], "reversi");
        assert!(shown.contains("discs: you 4, them 1") || shown.contains("discs: you 1, them 4"), "{shown}");
        let (shown, ..) = drawn("dots_and_boxes", &["a1h", "a1v", "a2h", "b1v"], "dots");
        assert!(shown.contains("•───•") && shown.contains("│ ■ │") && shown.contains("last b1v"), "{shown}");
        let (shown, ..) = drawn("mancala", &["c"], "mancala");
        assert!(shown.contains("[ 1]") && shown.contains("[ 5]") && shown.contains("your store on the right"), "{shown}");
        if std::env::var("SHOW_BOARDS").is_ok() {
            for (app, moves) in [
                ("hex", &["f6", "e7"][..]),
                ("gomoku", &["h8", "h9"]),
                ("reversi", &["d3", "c3"]),
                ("dots_and_boxes", &["a1h", "a1v", "a2h", "b1v"]),
                ("mancala", &["c", "a"]),
            ] {
                println!("{}", drawn(app, moves, "show").0);
            }
        }
    }

    #[test]
    fn the_games_tab_in_a_window_a_line_or_two_high() {
        let (_, mut app, _) = drawn("ttt", &[], "short");
        for height in 1..=4 {
            let mut terminal = Terminal::new(TestBackend::new(60, height)).unwrap();
            terminal.draw(|f| super::super::draw(f, &mut app)).unwrap();
        }
    }

    #[test]
    fn a_checkers_board_drawn_for_either_side() {
        use std::collections::HashMap;

        use crate::games::{Games, fields};
        let dir = std::env::temp_dir().join(format!("rettui-ui-checkers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, _net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        let (me, them) = ([0xa1; 16], [0xb2; 16]);
        app.games.set_me(&me);
        let mut ben = Games::default();
        ben.set_me(&them);
        let wire = |prepared: &crate::games::Prepared| -> HashMap<u8, Vec<u8>> {
            fields(&hex::decode(prepared.game.envelope.as_ref().unwrap()).unwrap()).into_iter().collect()
        };
        let challenge = app.games.challenge("checkers", &hex::encode(them)).unwrap();
        let session = challenge.game.session.clone();
        ben.receive(&hex::encode(me), &wire(&challenge));
        app.games.receive(&hex::encode(them), &wire(&ben.act("checkers", &session, "accept", HashMap::new()).unwrap()));
        app.show_game("checkers", &session);
        app.game_playing = true;
        let mut terminal = Terminal::new(TestBackend::new(110, 34)).unwrap();
        terminal.draw(|f| super::super::draw(f, &mut app)).unwrap();
        let shown = screen(&terminal);
        let view = app.games.view("checkers", &session).unwrap();
        let crate::games::Board::Checkers { black, .. } = view.board else { panic!() };
        let side = if black { "You're Black (dark pieces)" } else { "You're White (light pieces)" };
        assert!(shown.contains(side) && shown.contains("Checkers with"), "{shown}");
        // Your pieces are at the bottom, whichever side you are; the files
        // read the other way for Black.
        let files = if black { " h  g  f  e  d  c  b  a " } else { " a  b  c  d  e  f  g  h " };
        assert!(shown.contains(files), "{shown}");
        // Six rows of four, twelve pieces a side.
        assert_eq!(shown.lines().filter(|l| l.matches('●').count() == 4).count(), 6, "{shown}");
        if black {
            assert!(shown.contains("Enter pick/move · t type"), "{shown}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
