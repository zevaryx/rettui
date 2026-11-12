//! Micron in chat messages, as NomadNet writes and draws it in RRC rooms.
//!
//! NomadNet runs a message through Markdown before Micron, and Markdown takes
//! a backtick for code, so a message writes Micron's tag character as `¦`
//! (U+00A6, broken bar) instead: `¦F0f0` for green, `¦f` back to the default.
//! Only those tags are read. A backtick on its own stays one, since people
//! write them in ordinary messages, and a message without a `¦` tag is plain
//! text.
//!
//! The tags read are text formatting only: colours (`¦Fxxx` `¦Bxxx`,
//! `¦FTxxxxxx` `¦BTxxxxxx`, `¦f` `¦b`), bold `¦!`, italic `¦*`, underline
//! `¦_` and `` ¦` `` (all back to the default, as Micron's double backtick).
//! Alignment, links and the rest (`¦<` `¦>` `¦{` `¦c` `¦l` `¦r` `¦a` `¦=`)
//! are left out rather than shown.

use ratatui::style::Style;

use super::Format;

/// The tag character in a chat message.
pub const TAG: char = '\u{a6}';

/// A message's text without its tags, and how it is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatted {
    pub text: String,
    /// Byte ranges of `text` and their styles, in order, where they differ
    /// from the default.
    pub spans: Vec<(usize, usize, Style)>,
    /// Whether it sets a colour. Coloured messages are often art (block
    /// pictures, letters drawn in characters), which wrapping would break, so
    /// they are drawn line for line.
    pub colour: bool,
}

/// What a tag at the start of `rest` (just after a `¦`) does, and how many
/// bytes it takes.
enum Tag {
    /// The default foreground, and the background too for `¦f`b`.
    Fg {
        both: bool,
    },
    Bg,
    Colour {
        background: bool,
        code: usize,
    },
    Bold,
    Italic,
    Underline,
    Reset,
    Dropped,
}

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The tag at the start of `rest` and its length, if it is one.
fn tag(rest: &str) -> Option<(Tag, usize)> {
    let first = rest.chars().next()?;
    Some(match first {
        'F' | 'B' => {
            let background = first == 'B';
            if rest.get(1..2) == Some("T") && rest.get(2..8).is_some_and(is_hex) {
                (Tag::Colour { background, code: 6 }, 8)
            } else if rest.get(1..4).is_some_and(is_hex) {
                (Tag::Colour { background, code: 3 }, 4)
            } else {
                return None;
            }
        }
        // NomadNet's own pair for both defaults.
        'f' if rest[1..].starts_with("`b") => (Tag::Fg { both: true }, 3),
        'f' => (Tag::Fg { both: false }, 1),
        'b' => (Tag::Bg, 1),
        '!' => (Tag::Bold, 1),
        '*' => (Tag::Italic, 1),
        '_' => (Tag::Underline, 1),
        '`' => (Tag::Reset, 1),
        '<' | '>' | '{' | 'c' | 'l' | 'r' | 'a' | '=' => (Tag::Dropped, 1),
        _ => return None,
    })
}

/// A chat message's Micron, or `None` when it has no `¦` tag (and so is
/// plain text).
pub fn parse(message: &str) -> Option<Formatted> {
    let mut text = String::with_capacity(message.len());
    let mut spans: Vec<(usize, usize, Style)> = Vec::new();
    let mut format = Format::default();
    let (mut tagged, mut colour) = (false, false);
    let mut rest = message;
    while let Some(at) = rest.find(TAG) {
        let (before, after) = (&rest[..at], &rest[at + TAG.len_utf8()..]);
        push(&mut text, &mut spans, before, format.style());
        let Some((tag, len)) = tag(after) else {
            push(&mut text, &mut spans, &rest[at..at + TAG.len_utf8()], format.style());
            rest = after;
            continue;
        };
        tagged = true;
        match tag {
            Tag::Colour { background, code } => {
                let color = super::parse_color(&after[len - code..len]);
                colour |= color.is_some();
                if background {
                    format.bg = color;
                } else {
                    format.fg = color;
                }
            }
            Tag::Fg { both } => {
                format.fg = None;
                if both {
                    format.bg = None;
                }
            }
            Tag::Bg => format.bg = None,
            Tag::Bold => format.bold = !format.bold,
            Tag::Italic => format.italic = !format.italic,
            Tag::Underline => format.underline = !format.underline,
            Tag::Reset => format = Format::default(),
            Tag::Dropped => {}
        }
        rest = &after[len..];
    }
    push(&mut text, &mut spans, rest, format.style());
    tagged.then_some(Formatted { text, spans, colour })
}

/// Add `piece` in `style` to the text, joining the span before it when the
/// style is the same.
fn push(text: &mut String, spans: &mut Vec<(usize, usize, Style)>, piece: &str, style: Style) {
    let start = text.len();
    text.push_str(piece);
    if piece.is_empty() || style == Style::default() {
        return;
    }
    match spans.last_mut() {
        Some((_, end, last)) if *end == start && *last == style => *end = text.len(),
        _ => spans.push((start, text.len(), style)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    #[test]
    fn plain_messages_and_backticks_are_not_micron() {
        assert_eq!(parse("hello there"), None);
        assert_eq!(parse("run `F00 and `!this`!"), None);
        // A broken bar that starts no tag is just a character.
        assert_eq!(parse("a ¦ b ¦x ¦Fzz"), None);
    }

    #[test]
    fn colours_and_text_styles() {
        let f = parse("¦F0f0go¦f ¦BTff8000hot¦b ¦!bold¦! ¦*it¦* ¦_un¦_").unwrap();
        assert_eq!(f.text, "go hot bold it un");
        assert!(f.colour);
        assert_eq!(
            f.spans,
            vec![
                (0, 2, Style::default().fg(Color::Rgb(0, 255, 0))),
                (3, 6, Style::default().bg(Color::Rgb(0xff, 0x80, 0))),
                (7, 11, Style::default().add_modifier(Modifier::BOLD)),
                (12, 14, Style::default().add_modifier(Modifier::ITALIC)),
                (15, 17, Style::default().add_modifier(Modifier::UNDERLINED)),
            ]
        );
    }

    #[test]
    fn layout_tags_are_dropped_and_reset_clears() {
        let f = parse("¦c¦<¦>¦{¦l¦r¦a¦=mid ¦F00f¦!x¦`y").unwrap();
        assert_eq!(f.text, "mid xy");
        assert_eq!(f.spans, vec![(4, 5, Style::default().fg(Color::Rgb(0, 0, 255)).add_modifier(Modifier::BOLD))]);
        // Formatting alone (no colour) is not art.
        assert!(!parse("¦!loud¦!").unwrap().colour);
        // Two bars are no tag: both shown, and the one after a tag is read.
        assert_eq!(parse("a¦¦ b"), None);
        let f = parse("¦¦F0f0x").unwrap();
        assert_eq!((f.text.as_str(), f.spans.len()), ("¦x", 1));
    }

    #[test]
    fn spaces_are_kept_and_unknown_tags_shown() {
        let f = parse("¦B000  ¦f`b x ¦q").unwrap();
        assert_eq!(f.text, "   x ¦q");
        assert_eq!(f.spans, vec![(0, 2, Style::default().bg(Color::Rgb(0, 0, 0)))]);
    }

    #[test]
    fn cut_off_tags_never_panic() {
        for message in ["¦", "¦F", "¦F0", "¦FT12345", "¦B12", "¦f`", "x¦\u{301}", "¦FT¦F0f0"] {
            let _ = parse(message);
        }
        assert_eq!(parse("¦FT¦F0f0x").unwrap().text, "¦FTx");
    }
}
