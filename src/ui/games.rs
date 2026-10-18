//! Games over the conversation: the games menu (`g`), and a game's board
//! with where it stands and what can be done (see `crate::app::games`).

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{accent, block, dim, selected_bg};
use crate::app::App;
use crate::app::games::{BoardRegion, MenuItem};
use crate::games::{Board, GameView};

/// The other player's pieces (yours are in the accent colour).
const THEIRS: Color = Color::Yellow;
/// A chess board's squares, which look the same in every theme.
const LIGHT_SQUARE: Color = Color::Rgb(0xb5, 0x88, 0x63);
const DARK_SQUARE: Color = Color::Rgb(0x7a, 0x55, 0x3a);
const LAST_MOVE: Color = Color::Rgb(0x9a, 0x9a, 0x4a);

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

pub(super) fn draw_game_menu(frame: &mut Frame, app: &mut App) {
    let Some(menu) = app.game_menu.clone() else { return };
    let items = app.game_menu_items();
    let area = frame.area();
    let rect = centered(area, 60, items.len() as u16 + 4);
    let name = app.store.display_name(&menu.peer);
    let title = format!("Games with {name}");
    let outer = block(&title, true).title_bottom(Line::styled(" Enter open · ↑↓ choose · Esc close ", Style::default().fg(dim())));
    let inner = outer.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(outer, rect);
    app.regions.game_menu_box = rect;
    let list = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    app.regions.game_menu = list;
    let width = list.width as usize;
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let base = if i == menu.selected { Style::default().bg(selected_bg()) } else { Style::default() };
            let spans = match item {
                MenuItem::Game(game) => {
                    let mark = if game.yours { Span::styled("● ", base.fg(accent())) } else { Span::styled("  ", base) };
                    let summary = if game.yours { base.fg(accent()) } else { base.fg(dim()) };
                    vec![mark, Span::styled(game.name.clone(), base.bold()), Span::styled(format!("  {}", game.summary), summary)]
                }
                MenuItem::New(app) => {
                    vec![Span::styled("+ ", base.fg(dim())), Span::styled(format!("New game of {}", crate::games::game_name(app)), base)]
                }
            };
            padded(spans, width, base)
        })
        .collect();
    let hint = if items.iter().any(|i| matches!(i, MenuItem::Game(_))) { "Your games, newest first" } else { "No games with them yet" };
    frame.render_widget(Paragraph::new(Span::styled(hint, Style::default().fg(dim()))), Rect { height: 1, ..inner });
    frame.render_widget(Paragraph::new(lines), list);
}

pub(super) fn draw_game(frame: &mut Frame, app: &mut App) {
    app.place_game_cursor();
    let Some(screen) = app.game_view.clone() else { return };
    let Some(game) = app.games.view(&screen.app, &screen.session) else { return };
    let (board_w, board_h) = match &game.board {
        Board::TicTacToe { .. } => (17, 5),
        Board::FourInARow { .. } => (30, 7),
        Board::Chess { .. } => (28, 9),
    };
    let area = frame.area();
    let hints = format!(" {} ", hints(&game));
    let footnote = footnote(&game);
    // Wide enough for the keys and the line below the board, if it can be.
    let width = (board_w + 4).max(48).max(hints.width() as u16 + 4).max(footnote.width() as u16 + 4);
    let rect = centered(area, width, board_h + 7);
    let name = app.store.display_name(&game.peer);
    let title = format!("{} with {name}", game.name);
    let outer = block(&title, true).title_bottom(Line::styled(hints, Style::default().fg(dim())));
    let inner = outer.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(outer, rect);

    let status = if game.yours { Style::default().fg(accent()).bold() } else { Style::default().bold() };
    frame.render_widget(Paragraph::new(Span::styled(game.summary.clone(), status)).centered(), Rect { height: 1, ..inner });
    let board_area = Rect { x: inner.x + inner.width.saturating_sub(board_w) / 2, y: inner.y + 2, width: board_w, height: board_h };
    let region = match &game.board {
        Board::TicTacToe { cells, mine } => draw_tic_tac_toe(frame, board_area, cells, *mine, screen.cursor),
        Board::FourInARow { cells, mine, last } => draw_four_in_a_row(frame, board_area, cells, *mine, *last, screen.cursor, game.yours),
        Board::Chess { fen, white, last, .. } => draw_chess(frame, board_area, fen, *white, last.as_deref(), screen.cursor, screen.from),
    };
    app.regions.game_board = BoardRegion { area: rect, ..region };
    // Below: what you are, and for chess the moves so far.
    let below = Rect { y: board_area.y + board_area.height + 1, height: 1, ..inner };
    frame.render_widget(Paragraph::new(Span::styled(footnote, Style::default().fg(dim()))).centered(), below);
}

/// The keys that do something now.
fn hints(game: &GameView) -> String {
    let mut keys: Vec<&str> = Vec::new();
    let can = |command: &str| game.actions.iter().any(|a| a == command);
    if can("accept") {
        keys.extend(["a accept", "d decline"]);
    }
    if game.yours && game.status == "active" && game.draw_offer != Some(true) {
        keys.push(if game.app == "chess" { "Enter pick/move · t type" } else { "Enter play" });
    }
    if can("draw_accept") {
        keys.extend(["y accept draw", "n refuse"]);
    }
    if can("draw_offer") {
        keys.push("D offer draw");
    }
    if can("resign") {
        keys.push("R resign");
    }
    if !matches!(game.status.as_str(), "pending" | "active") {
        keys.push("x forget");
    }
    keys.push("Esc close");
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
    }
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
    BoardRegion { origin: Position::new(area.x, area.y), cell: (6, 2), across: 3, down: 3, area }
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
    BoardRegion { origin: Position::new(area.x, area.y), cell: (4, 7), across: 7, down: 1, area }
}

/// A chess piece's glyph: the filled ones for both sides, told apart by
/// colour (they read better than the outlined ones in a terminal).
fn chess_glyph(piece: char) -> char {
    match piece.to_ascii_lowercase() {
        'k' => '♚',
        'q' => '♛',
        'r' => '♜',
        'b' => '♝',
        'n' => '♞',
        'p' => '♟',
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
    BoardRegion { origin: Position::new(area.x + 2, area.y), cell: (3, 1), across: 8, down: 8, area }
}

#[cfg(test)]
mod tests {
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
    fn menu_then_a_board_drawn_and_played_by_key_and_click() {
        let dir = std::env::temp_dir().join(format!("rettui-ui-games-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, _net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        app.games.set_me(&[0xa1; 16]);
        let peer = hex::encode([0xb2; 16]);
        app.add_contact(&peer).unwrap();
        app.active_conversation = Some(peer.clone());
        let mut terminal = Terminal::new(TestBackend::new(110, 34)).unwrap();
        let key = |app: &mut crate::app::App, code| app.on_key(crossterm::event::KeyEvent::from(code));

        key(&mut app, crossterm::event::KeyCode::Char('g'));
        terminal.draw(|f| super::super::draw(f, &mut app)).unwrap();
        let shown = screen(&terminal);
        assert!(shown.contains("No games with them yet") && shown.contains("New game of Four in a Row"), "{shown}");
        // The second new game: Four in a Row.
        key(&mut app, crossterm::event::KeyCode::Down);
        key(&mut app, crossterm::event::KeyCode::Enter);
        assert!(app.game_menu.is_none());
        terminal.draw(|f| super::super::draw(f, &mut app)).unwrap();
        let shown = screen(&terminal);
        assert!(shown.contains("Four in a Row with") && shown.contains("Waiting for them to accept"), "{shown}");
        // The challenge is in the conversation, as a game's move.
        let opening = app.store.conversations[&peer].messages.last().unwrap().opening();
        assert_eq!(opening, "🎲 Four in a Row · Sent a challenge!");
        // Not playable yet: a click on the board does nothing but say so.
        let board = app.regions.game_board;
        app.on_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: board.origin.x + 1,
            row: board.origin.y + 3,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert!(app.game_view.is_some());
        key(&mut app, crossterm::event::KeyCode::Esc);
        assert!(app.game_view.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
