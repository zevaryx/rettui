//! Links in chat (see [`crate::links`]): drawn as links, and where they are
//! on their row so a click opens them. And selecting chat text with the
//! mouse: what's drawn, recorded to copy, and the selection shown.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// A link on a row: its first column, the column after it, and what it
/// opens.
pub type RowLink = (usize, usize, String);

pub fn link_style(style: Style) -> Style {
    style.fg(Color::LightBlue).add_modifier(Modifier::UNDERLINED)
}

/// The links in `found` on a row wrapped from their text: the row is
/// `piece`, from byte `offset` of the text, drawn from column `col`. A link
/// broken over rows is on each of them.
pub fn row_links(found: &[crate::links::Found], offset: usize, piece: &str, col: usize) -> Vec<RowLink> {
    let end = offset + piece.len();
    found
        .iter()
        .filter(|f| f.start < end && f.end > offset)
        .filter_map(|f| {
            let (from, to) = (f.start.max(offset) - offset, f.end.min(end) - offset);
            let (from, to) = (piece.get(..from)?.width(), piece.get(..to)?.width());
            Some((col + from, col + to, f.url.clone()))
        })
        .collect()
}

/// Plain `text` wrapped to `width`, in `style`, with the links in it drawn
/// as links (one broken over rows too), and where they are on each row.
pub fn plain_rows(text: &str, width: usize, style: Style) -> Vec<(Line<'static>, Vec<RowLink>)> {
    let found = crate::links::find(text);
    let mut cursor = 0;
    super::wrap(text, width)
        .into_iter()
        .map(|piece| {
            // Where the row is in the text (wrapping drops only spaces and
            // line breaks).
            let offset = text[cursor..].find(piece.as_str()).map_or(cursor, |i| cursor + i);
            cursor = offset + piece.len();
            let mut spans = Vec::new();
            let mut at = offset;
            for f in found.iter().filter(|f| f.start < cursor && f.end > offset) {
                let (start, end) = (f.start.max(offset), f.end.min(cursor));
                if start > at {
                    spans.push(Span::styled(text[at..start].to_string(), style));
                }
                spans.push(Span::styled(text[start..end].to_string(), link_style(style)));
                at = end;
            }
            if at < cursor {
                spans.push(Span::styled(text[at..cursor].to_string(), style));
            }
            let hits = row_links(&found, offset, &piece, 0);
            (Line::from(spans), hits)
        })
        .collect()
}

/// `line` with the links in it drawn as links, and where they are. A link
/// too long for its row (broken over two) isn't found: rows wrapped from
/// text they know use [`plain_rows`] or [`row_links`].
pub fn linkify(line: Line<'static>) -> (Line<'static>, Vec<RowLink>) {
    let text: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
    let found = crate::links::find(&text);
    if found.is_empty() {
        return (line, Vec::new());
    }
    let hits = found.iter().map(|f| (text[..f.start].width(), text[..f.end].width(), f.url.clone())).collect();
    let Line { spans, style, alignment } = line;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut at = 0;
    for span in spans {
        let content = span.content.as_ref();
        let (start, end) = (at, at + content.len());
        // Cut the span where links start and end.
        let mut cuts: Vec<usize> = vec![start, end];
        for f in &found {
            cuts.extend([f.start, f.end].into_iter().filter(|&c| c > start && c < end));
        }
        cuts.sort_unstable();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let piece = &text[pair[0]..pair[1]];
            let linked = found.iter().any(|f| f.start <= pair[0] && pair[1] <= f.end);
            let style = if linked { link_style(span.style) } else { span.style };
            out.push(Span::styled(piece.to_string(), style));
        }
        at = end;
    }
    (Line { spans: out, style, alignment }, hits)
}

/// A chat history just drawn in `area`: its rows' text as on screen (what
/// selecting copies), and the text selected in it shown reversed.
pub fn selectable(frame: &mut Frame, app: &mut crate::app::App, area: Rect) {
    let buf = frame.buffer_mut();
    app.regions.chat_text = (area.y..area.bottom())
        .map(|y| {
            let mut row = String::new();
            let mut skip = 0;
            for x in area.x..area.right() {
                // The cells a wide character covers after its own.
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let symbol = buf[(x, y)].symbol();
                skip = symbol.width().saturating_sub(1);
                row.push_str(symbol);
            }
            row
        })
        .collect();
    let Some(chat) = app.chat_selection.filter(|c| c.tab == app.tab && c.area == area && !c.selection.is_empty()) else {
        return;
    };
    for y in 0..area.height {
        for x in 0..area.width {
            if chat.selection.contains(y as usize, x as usize) {
                buf[(area.x + x, area.y + y)].set_style(Style::default().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_drawn_as_links_where_they_are() {
        let hub = format!("rrc://{}/lobby", "ab".repeat(16));
        let line = Line::from(vec![
            Span::styled("12:00 ", Style::default().fg(Color::Gray)),
            Span::raw("<é> "),
            Span::raw(format!("join {hub}, then")),
        ]);
        let (line, hits) = linkify(line);
        assert_eq!(hits, [(15, 15 + hub.len(), hub.clone())]);
        let linked: Vec<&str> =
            line.spans.iter().filter(|s| s.style.add_modifier.contains(Modifier::UNDERLINED)).map(|s| s.content.as_ref()).collect();
        assert_eq!(linked, [hub.as_str()]);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, format!("12:00 <é> join {hub}, then"));
        // Nothing to link: as it was.
        let plain = Line::raw("just words");
        assert_eq!(linkify(plain.clone()), (plain, Vec::new()));
    }

    #[test]
    fn a_link_broken_over_rows_is_one_link() {
        let page = format!("{}:/page/index.mu", "cd".repeat(16));
        let text = format!("go to {page} now");
        let rows = plain_rows(&text, 30, Style::default());
        let shown: Vec<String> = rows.iter().map(|(line, _)| line.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
        assert_eq!(shown, ["go to".to_string(), page[..30].to_string(), format!("{} now", &page[30..])]);
        // Each part opens the whole page.
        assert_eq!(rows[1].1, [(0, 30, page.clone())]);
        assert_eq!(rows[2].1, [(0, page.len() - 30, page.clone())]);
        assert!(rows[0].1.is_empty());
    }
}
