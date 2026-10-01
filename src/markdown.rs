//! Formatted message text. LXMF's renderer field (`FIELD_RENDERER`) says a
//! message is written in Markdown (as Sideband and NomadNet compose them)
//! or in Micron; rettui marks the messages it sends as Markdown.
//!
//! Markdown is parsed with pulldown-cmark into blocks, which the terminal
//! draws as styled, wrapped lines and the web UI gets as HTML. Line breaks
//! are kept, as in a chat. Raw HTML in a message is shown as text, never
//! passed through, and only web and mail links become links.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

use crate::nomad::micron::html::escape;

/// How a message's text is written, from its renderer field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextFormat {
    Markdown,
    Micron,
}

impl TextFormat {
    /// From LXMF's renderer field value (plain text and BBCode are shown as
    /// they are).
    pub fn from_renderer(renderer: u8) -> Option<Self> {
        match renderer {
            lxmf_core::constants::RENDERER_MARKDOWN => Some(TextFormat::Markdown),
            lxmf_core::constants::RENDERER_MICRON => Some(TextFormat::Micron),
            _ => None,
        }
    }
}

/// How a run of text looks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Look {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Text,
    Heading,
    Code,
    Rule,
}

/// A block of text: a paragraph, a heading, a list item, a code block or a
/// rule, inside `quote` levels of quotes.
#[derive(Debug, Clone)]
struct Block {
    kind: Kind,
    quote: usize,
    /// List depth (1 for a top-level item), and its marker (`• `, `1. `).
    list: usize,
    marker: Option<String>,
    /// Its text: runs of one look, a line break as `\n`.
    runs: Vec<(String, Look)>,
}

fn parse(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut open: Option<Block> = None;
    let mut look = Look::default();
    let mut links: Vec<String> = Vec::new();
    let (mut quote, mut lists): (usize, Vec<Option<u64>>) = (0, Vec::new());
    let mut marker: Option<String> = None;
    let start = |kind: Kind, quote: usize, lists: &[Option<u64>], marker: &mut Option<String>| Block {
        kind,
        quote,
        list: lists.len(),
        marker: marker.take(),
        runs: Vec::new(),
    };
    let finish = |open: &mut Option<Block>, blocks: &mut Vec<Block>| {
        if let Some(block) = open.take()
            && (!block.runs.is_empty() || block.kind == Kind::Rule)
        {
            blocks.push(block);
        }
    };
    for event in Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Start(Tag::Paragraph) => {
                finish(&mut open, &mut blocks);
                open = Some(start(Kind::Text, quote, &lists, &mut marker));
            }
            Event::Start(Tag::Heading { .. }) => {
                finish(&mut open, &mut blocks);
                open = Some(start(Kind::Heading, quote, &lists, &mut marker));
            }
            Event::Start(Tag::CodeBlock(_)) => {
                finish(&mut open, &mut blocks);
                open = Some(start(Kind::Code, quote, &lists, &mut marker));
            }
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item) => {
                finish(&mut open, &mut blocks);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                finish(&mut open, &mut blocks);
                quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                finish(&mut open, &mut blocks);
                quote = quote.saturating_sub(1);
            }
            Event::Start(Tag::List(first)) => {
                finish(&mut open, &mut blocks);
                lists.push(first);
            }
            Event::End(TagEnd::List(_)) => {
                finish(&mut open, &mut blocks);
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                finish(&mut open, &mut blocks);
                marker = Some(match lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => "• ".to_string(),
                });
            }
            Event::Start(Tag::Emphasis) => look.italic = true,
            Event::End(TagEnd::Emphasis) => look.italic = false,
            Event::Start(Tag::Strong) => look.bold = true,
            Event::End(TagEnd::Strong) => look.bold = false,
            Event::Start(Tag::Strikethrough) => look.strike = true,
            Event::End(TagEnd::Strikethrough) => look.strike = false,
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                links.push(dest_url.to_string());
                look.link = true;
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                look.link = false;
                // Where it goes, after its text if that doesn't say.
                if let Some(url) = links.pop()
                    && let Some(block) = &mut open
                {
                    let said: String = block.runs.iter().rev().take_while(|(_, l)| l.link).map(|(t, _)| t.as_str()).collect();
                    if said.trim() != url && !url.is_empty() {
                        block.runs.push((format!(" ({url})"), Look::default()));
                    }
                }
            }
            Event::Rule => {
                finish(&mut open, &mut blocks);
                blocks.push(start(Kind::Rule, quote, &lists, &mut marker));
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                // Text of a list item without a paragraph (a tight list).
                let block = open.get_or_insert_with(|| start(Kind::Text, quote, &lists, &mut marker));
                block.runs.push((text.to_string(), look));
            }
            Event::Code(text) => {
                let block = open.get_or_insert_with(|| start(Kind::Text, quote, &lists, &mut marker));
                block.runs.push((text.to_string(), Look { code: true, ..look }));
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(block) = &mut open {
                    block.runs.push(("\n".into(), look));
                }
            }
            _ => {}
        }
    }
    finish(&mut open, &mut blocks);
    blocks
}

/// Inline code and code blocks.
const CODE: Color = Color::Yellow;
const QUOTE: Color = Color::DarkGray;

fn style_of(look: Look, kind: &Kind) -> Style {
    let mut style = Style::default();
    if look.bold || *kind == Kind::Heading {
        style = style.add_modifier(Modifier::BOLD);
    }
    if look.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if look.strike {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    if look.link {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if look.code || *kind == Kind::Code {
        style = style.fg(CODE);
    }
    style
}

/// Markdown `text` as lines at most `width` columns wide, for the terminal.
pub fn lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut previous: Option<usize> = None;
    for block in parse(text) {
        // A blank line between blocks, but not between a list's items.
        if previous.is_some_and(|list| list == 0 || block.list == 0) {
            out.push(Line::default());
        }
        previous = Some(block.list);
        let bars = "▎ ".repeat(block.quote);
        let indent = "  ".repeat(block.list.saturating_sub(1));
        let marker = block.marker.clone().unwrap_or_default();
        let first = format!("{indent}{marker}");
        let rest = " ".repeat(first.width());
        let avail = width.saturating_sub(bars.width() + first.width()).max(1);
        if block.kind == Kind::Rule {
            out.push(Line::from(vec![Span::styled(bars.clone(), Style::default().fg(QUOTE)), Span::styled("─".repeat(avail), Style::default().fg(QUOTE))]));
            continue;
        }
        // Each line of the block (split at its breaks), wrapped.
        let mut rows = Vec::new();
        let mut pieces: Vec<(String, Style, Option<usize>)> = Vec::new();
        let flush = |pieces: &mut Vec<(String, Style, Option<usize>)>, rows: &mut Vec<Vec<Span<'static>>>| {
            let (wrapped, _) = crate::nomad::micron::layout::wrap(pieces, avail);
            rows.extend(wrapped.into_iter().map(|row| row.into_iter().map(|(span, _)| span).collect::<Vec<_>>()));
            pieces.clear();
        };
        for (text, look) in &block.runs {
            let style = style_of(*look, &block.kind);
            let mut parts = text.split('\n').peekable();
            while let Some(part) = parts.next() {
                if !part.is_empty() {
                    pieces.push((part.to_string(), style, None));
                }
                if parts.peek().is_some() {
                    flush(&mut pieces, &mut rows);
                }
            }
        }
        flush(&mut pieces, &mut rows);
        // A code block's last line break leaves an empty row.
        if block.kind == Kind::Code && rows.last().is_some_and(|r| r.is_empty()) {
            rows.pop();
        }
        for (i, row) in rows.into_iter().enumerate() {
            let lead = if i == 0 { first.clone() } else { rest.clone() };
            let mut spans = vec![Span::styled(bars.clone(), Style::default().fg(QUOTE)), Span::raw(lead)];
            spans.extend(row);
            out.push(Line::from(spans));
        }
    }
    out
}

/// Markdown `text` without its markup, for previews and notifications:
/// what it says, a line per line (break) or block.
pub fn plain(text: &str) -> String {
    parse(text)
        .iter()
        .map(|block| {
            let words: String = block.runs.iter().map(|(text, _)| text.as_str()).collect();
            format!("{}{words}", block.marker.as_deref().unwrap_or(""))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a link may be followed from the web UI.
fn safe_link(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:"].iter().any(|scheme| lower.starts_with(scheme))
}

/// Markdown `text` as HTML for the web UI: everything escaped, raw HTML
/// shown as text, only web and mail links as links.
pub fn html(text: &str) -> String {
    let mut out = String::new();
    let mut links: Vec<bool> = Vec::new();
    for event in Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => out.push_str("<p>"),
                Tag::Heading { level, .. } => {
                    let n = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        _ => 3,
                    };
                    out.push_str(&format!("<p class=\"md-h{n}\">"));
                }
                Tag::BlockQuote(_) => out.push_str("<blockquote>"),
                Tag::CodeBlock(CodeBlockKind::Fenced(_) | CodeBlockKind::Indented) => out.push_str("<pre><code>"),
                Tag::List(Some(first)) => out.push_str(&format!("<ol start=\"{first}\">")),
                Tag::List(None) => out.push_str("<ul>"),
                Tag::Item => out.push_str("<li>"),
                Tag::Emphasis => out.push_str("<em>"),
                Tag::Strong => out.push_str("<strong>"),
                Tag::Strikethrough => out.push_str("<del>"),
                Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                    let safe = safe_link(&dest_url);
                    links.push(safe);
                    if safe {
                        out.push_str(&format!("<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">", escape(&dest_url)));
                    } else {
                        out.push_str(&format!("<span class=\"md-link\" title=\"{}\">", escape(&dest_url)));
                    }
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph | TagEnd::Heading(_) => out.push_str("</p>"),
                TagEnd::BlockQuote(_) => out.push_str("</blockquote>"),
                TagEnd::CodeBlock => out.push_str("</code></pre>"),
                TagEnd::List(true) => out.push_str("</ol>"),
                TagEnd::List(false) => out.push_str("</ul>"),
                TagEnd::Item => out.push_str("</li>"),
                TagEnd::Emphasis => out.push_str("</em>"),
                TagEnd::Strong => out.push_str("</strong>"),
                TagEnd::Strikethrough => out.push_str("</del>"),
                TagEnd::Link | TagEnd::Image => out.push_str(if links.pop().unwrap_or(false) { "</a>" } else { "</span>" }),
                _ => {}
            },
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => out.push_str(&escape(&text)),
            Event::Code(text) => out.push_str(&format!("<code>{}</code>", escape(&text))),
            Event::SoftBreak | Event::HardBreak => out.push_str("<br>"),
            Event::Rule => out.push_str("<hr>"),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(text: &str, width: usize) -> Vec<String> {
        lines(text, width).iter().map(|l| l.to_string().trim_end().to_string()).collect()
    }

    #[test]
    fn markdown_in_the_terminal() {
        let text = "# Plans\nMeet at **noon**, not *ten*.\nBring `coffee`.\n\n- one\n- two\n  1. nested\n\n> quoted\n> still\n\n[map](https://example.org) and <b>raw</b>\n\n---\n\n```\nlet x = 1;\n```";
        assert_eq!(shown(text, 40), [
            "Plans",
            "",
            "Meet at noon, not ten.",
            "Bring coffee.",
            "",
            "• one",
            "• two",
            "  1. nested",
            "",
            "▎ quoted",
            "▎ still",
            "",
            "map (https://example.org) and <b>raw</b>",
            "",
            "────────────────────────────────────────",
            "",
            "let x = 1;",
        ]);
        let styled = lines("**bold** _it_ ~~gone~~ `code`", 40);
        let style = |text: &str| styled[0].spans.iter().find(|s| s.content == text).unwrap().style;
        assert!(style("bold").add_modifier.contains(Modifier::BOLD));
        assert!(style("it").add_modifier.contains(Modifier::ITALIC));
        assert!(style("gone").add_modifier.contains(Modifier::CROSSED_OUT));
        assert_eq!(style("code").fg, Some(CODE));
        // Wrapped under the marker.
        assert_eq!(shown("- a list item long enough to wrap", 16), ["• a list item", "  long enough to", "  wrap"]);
    }

    #[test]
    fn markdown_for_the_web_is_escaped() {
        let html = html("**hi** <script>alert(1)</script>\n[ok](https://example.org) [no](javascript:alert(1))");
        assert!(html.contains("<strong>hi</strong>"));
        assert!(html.contains("&lt;script&gt;") && !html.contains("<script>"), "{html}");
        assert!(html.contains("<a href=\"https://example.org\" target=\"_blank\" rel=\"noopener noreferrer\">ok</a>"));
        assert!(!html.contains("href=\"javascript"), "{html}");
        assert!(html.contains("<br>"), "line breaks are kept: {html}");
        assert_eq!(TextFormat::from_renderer(2), Some(TextFormat::Markdown));
        assert_eq!(TextFormat::from_renderer(0), None);
    }

    #[test]
    fn previews_leave_the_markup_out() {
        assert_eq!(plain("# Plans\n**noon**, `here`\n\n- one\n- two"), "Plans\nnoon, here\n• one\n• two");
    }
}
