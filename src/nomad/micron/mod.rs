//! Micron markup (NomadNet page format) parsing and terminal layout.
//!
//! Parsing produces a width-independent [`Page`]; [`Page::layout`] wraps it
//! to the viewport and applies the current selection and form values.
//! [`html`] renders the same page for the web UI.

use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};

pub mod html;
mod layout;
pub mod source;

pub use layout::hit_at;

const INDENT: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interactive {
    Link { url: String, fields: Vec<String> },
    Field(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    Text { masked: bool },
    Checkbox,
    Radio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
    /// Current text for text fields; submitted value for checkboxes/radios.
    pub value: String,
    pub label: String,
    pub checked: bool,
    pub width: usize,
}

#[derive(Debug, Clone)]
struct MSpan {
    text: String,
    style: Style,
    item: Option<usize>,
}

#[derive(Debug, Clone)]
enum MLine {
    Text {
        indent: usize,
        align: Alignment,
        fill: Option<Style>,
        spans: Vec<MSpan>,
    },
    Divider {
        indent: usize,
        ch: char,
        style: Style,
    },
    /// NomadNet 1.4 inline image, rendered on its own rows.
    Image {
        indent: usize,
        align: Alignment,
        url: String,
        alt: String,
        width: Option<usize>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Page {
    lines: Vec<MLine>,
    /// Page colours from `#!fg=` / `#!bg=` directives.
    pub base_style: Style,
    pub items: Vec<Interactive>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Format {
    bold: bool,
    italic: bool,
    underline: bool,
    fg: Option<Color>,
    bg: Option<Color>,
}

impl Format {
    fn style(self) -> Style {
        let mut style = Style::default();
        if let Some(fg) = self.fg {
            style = style.fg(fg);
        }
        if let Some(bg) = self.bg {
            style = style.bg(bg);
        }
        if self.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.underline {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        style
    }
}

/// Length of an inline image body (`alt`opts`url`, up to its closing `)`).
///
/// Alt text may itself contain parentheses (`photo (by Jo)`), so the body
/// ends at the first `)` that follows a backtick-separated URL, falling back
/// to the first `)` for malformed markup.
fn image_body_len(chars: &[char]) -> Option<usize> {
    let closes = chars.iter().enumerate().filter(|(_, c)| **c == ')').map(|(i, _)| i);
    let mut first = None;
    for end in closes {
        first.get_or_insert(end);
        let body = &chars[..end];
        if let Some(tick) = body.iter().rposition(|&c| c == '`') {
            let url = &body[tick + 1..];
            if url.iter().any(|&c| c == ':' || c == '/') {
                return Some(end);
            }
        }
    }
    first
}

fn heading_style(level: usize) -> Style {
    match level {
        1 => Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(0xbb, 0xbb, 0xbb))
            .add_modifier(Modifier::BOLD),
        2 => Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(0x99, 0x99, 0x99))
            .add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(Color::Rgb(0xee, 0xee, 0xee))
            .bg(Color::Rgb(0x55, 0x55, 0x55))
            .add_modifier(Modifier::BOLD),
    }
}

/// Parse a Micron colour: three hex digits (`f80`), grayscale (`g50`), or
/// 24-bit (`T86efac`; bare six digits in `#!fg=` page directives).
fn parse_color(code: &str) -> Option<Color> {
    let hex = code.strip_prefix('T').unwrap_or(code);
    if hex.len() == 6 {
        let v = u32::from_str_radix(hex, 16).ok()?;
        return Some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
    }
    let chars: Vec<char> = code.chars().collect();
    if chars.len() != 3 {
        return None;
    }
    if chars[0] == 'g' {
        let level: u32 = code[1..].parse().ok()?;
        let v = (level.min(99) * 255 / 99) as u8;
        return Some(Color::Rgb(v, v, v));
    }
    let digit = |c: char| c.to_digit(16).map(|d| (d * 17) as u8);
    Some(Color::Rgb(digit(chars[0])?, digit(chars[1])?, digit(chars[2])?))
}

struct Parser {
    page: Page,
    /// Images found mid-line, emitted after that line.
    images: Vec<MLine>,
    format: Format,
    align: Alignment,
    depth: usize,
    literal: bool,
}

pub fn parse(source: &str) -> Page {
    let mut parser = Parser {
        page: Page::default(),
        images: Vec::new(),
        format: Format::default(),
        align: Alignment::Left,
        depth: 0,
        literal: false,
    };
    for raw in source.lines() {
        parser.line(raw.trim_end_matches('\r'));
    }
    parser.page
}

impl Parser {
    fn content_indent(&self) -> usize {
        self.depth.saturating_sub(1) * INDENT
    }

    fn line(&mut self, line: &str) {
        let before = self.page.lines.len();
        self.parse_line(line);
        if self.images.is_empty() {
            return;
        }
        // A line holding only an image should not leave a blank row behind.
        if self.page.lines.len() == before + 1
            && let Some(MLine::Text { spans, .. }) = self.page.lines.last()
            && spans.iter().all(|s| s.text.trim().is_empty() && s.item.is_none())
        {
            self.page.lines.pop();
        }
        self.page.lines.append(&mut self.images);
    }

    fn parse_line(&mut self, mut line: &str) {
        if line == "`=" {
            self.literal = !self.literal;
            return;
        }
        if self.literal {
            self.page.lines.push(MLine::Text {
                indent: self.content_indent(),
                align: Alignment::Left,
                fill: None,
                spans: vec![MSpan {
                    text: line.replace('\t', "    "),
                    style: self.format.style(),
                    item: None,
                }],
            });
            return;
        }
        if let Some(directive) = line.strip_prefix("#!") {
            match directive.split_once('=') {
                Some(("fg", color)) => {
                    if let Some(c) = parse_color(color.trim()) {
                        self.page.base_style = self.page.base_style.fg(c);
                    }
                }
                Some(("bg", color)) => {
                    if let Some(c) = parse_color(color.trim()) {
                        self.page.base_style = self.page.base_style.bg(c);
                    }
                }
                _ => {}
            }
            return;
        }
        if line.starts_with('#') {
            return;
        }
        if let Some(rest) = line.strip_prefix('<') {
            self.depth = 0;
            if rest.is_empty() {
                return;
            }
            line = rest;
        }
        if line.starts_with('>') {
            let level = line.chars().take_while(|&c| c == '>').count();
            self.depth = level;
            let text = line[level..].trim();
            if text.is_empty() {
                return;
            }
            let style = heading_style(level);
            let saved = self.format;
            let spans = self.inline(text, style);
            self.format = saved;
            self.page.lines.push(MLine::Text {
                indent: level.saturating_sub(1) * INDENT,
                align: self.align,
                fill: Some(style),
                spans,
            });
            return;
        }
        if let Some(rest) = line.strip_prefix('-') {
            let ch = rest.chars().next().unwrap_or('\u{2500}');
            // A lone "-" is a thin rule; "-=" etc. repeat the given character.
            let ch = if ch.is_whitespace() { '\u{2500}' } else { ch };
            self.page.lines.push(MLine::Divider {
                indent: self.content_indent(),
                ch,
                style: self.format.style(),
            });
            return;
        }
        let spans = self.inline(line, Style::default());
        self.page.lines.push(MLine::Text {
            indent: self.content_indent(),
            align: self.align,
            fill: None,
            spans,
        });
    }

    /// Parse inline formatting. `base` is patched under the running format.
    fn inline(&mut self, text: &str, base: Style) -> Vec<MSpan> {
        let chars: Vec<char> = text.chars().collect();
        let mut spans = Vec::new();
        let mut buf = String::new();
        let mut i = 0;

        macro_rules! flush {
            () => {
                if !buf.is_empty() {
                    spans.push(MSpan {
                        text: std::mem::take(&mut buf),
                        style: base.patch(self.format.style()),
                        item: None,
                    });
                }
            };
        }

        while i < chars.len() {
            let c = chars[i];
            if c == '\\' && i + 1 < chars.len() {
                buf.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c != '`' || i + 1 >= chars.len() {
                buf.push(if c == '\t' { ' ' } else { c });
                i += 1;
                continue;
            }
            let code = chars[i + 1];
            i += 2;
            match code {
                '!' => {
                    flush!();
                    self.format.bold = !self.format.bold;
                }
                '*' => {
                    flush!();
                    self.format.italic = !self.format.italic;
                }
                '_' => {
                    flush!();
                    self.format.underline = !self.format.underline;
                }
                'F' | 'B' => {
                    flush!();
                    // `FT` / `BT` take 24-bit colour (six hex digits).
                    let len = if chars.get(i) == Some(&'T') { 7 } else { 3 };
                    let end = (i + len).min(chars.len());
                    let color = parse_color(&chars[i..end].iter().collect::<String>());
                    if code == 'F' {
                        self.format.fg = color;
                    } else {
                        self.format.bg = color;
                    }
                    i = end;
                }
                'f' => {
                    flush!();
                    self.format.fg = None;
                }
                'b' => {
                    flush!();
                    self.format.bg = None;
                }
                '`' => {
                    flush!();
                    self.format = Format::default();
                    self.align = Alignment::Left;
                }
                'c' => self.align = Alignment::Center,
                'l' | 'a' => self.align = Alignment::Left,
                'r' => self.align = Alignment::Right,
                '[' => {
                    flush!();
                    let Some(len) = chars[i..].iter().position(|&c| c == ']') else {
                        break;
                    };
                    let body: String = chars[i..i + len].iter().collect();
                    i += len + 1;
                    spans.push(self.link(&body, base));
                }
                // NomadNet 1.4 inline image: `(alt`w=60`a=c`:/media/x.webp)
                '(' => {
                    flush!();
                    let Some(len) = image_body_len(&chars[i..]) else {
                        break;
                    };
                    let body: String = chars[i..i + len].iter().collect();
                    i += len + 1;
                    let parts: Vec<&str> = body.split('`').collect();
                    // Options sit between the alt text and the URL. The
                    // alignment is the image's own; later lines keep theirs.
                    let mut align = self.align;
                    for part in parts.iter().skip(1).take(parts.len().saturating_sub(2)) {
                        match *part {
                            "a=c" => align = Alignment::Center,
                            "a=r" => align = Alignment::Right,
                            "a=l" => align = Alignment::Left,
                            _ => {}
                        }
                    }
                    let alt = parts.first().copied().unwrap_or_default();
                    let alt = if alt.is_empty() { "image" } else { alt };
                    let width = parts
                        .iter()
                        .find_map(|p| p.strip_prefix("w=")?.parse().ok());
                    let url = if parts.len() > 1 { parts[parts.len() - 1] } else { "" };
                    self.images.push(MLine::Image {
                        indent: self.content_indent(),
                        align,
                        url: url.to_string(),
                        alt: alt.to_string(),
                        width,
                    });
                }
                '<' => {
                    flush!();
                    let Some(len) = chars[i..].iter().position(|&c| c == '>') else {
                        break;
                    };
                    let body: String = chars[i..i + len].iter().collect();
                    i += len + 1;
                    spans.push(self.field(&body, base));
                }
                _ => {}
            }
        }
        flush!();
        spans
    }

    fn link(&mut self, body: &str, base: Style) -> MSpan {
        let parts: Vec<&str> = body.split('`').collect();
        let (label, url, fields) = match parts.as_slice() {
            [url] => (*url, *url, ""),
            [label, url] => (*label, *url, ""),
            [label, url, fields, ..] => (*label, *url, *fields),
            [] => ("", "", ""),
        };
        let label = if label.is_empty() { url } else { label };
        let fields = fields
            .split('|')
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(str::to_string)
            .collect();
        let mut style = base.patch(self.format.style()).add_modifier(Modifier::UNDERLINED);
        if self.format.fg.is_none() {
            style = style.fg(Color::LightBlue);
        }
        self.page.items.push(Interactive::Link {
            url: url.to_string(),
            fields,
        });
        MSpan {
            text: label.to_string(),
            style,
            item: Some(self.page.items.len() - 1),
        }
    }

    /// Fields look like `name`default`, `24|name`default`, `!24|pass`` or
    /// `?|name|value|*`label` (checkbox) / `^|name|value|*`label` (radio).
    fn field(&mut self, body: &str, base: Style) -> MSpan {
        let (spec, default) = body.split_once('`').unwrap_or((body, ""));
        let parts: Vec<&str> = spec.split('|').collect();
        let field = match parts.first().copied() {
            Some(kind @ ("?" | "^")) => Field {
                name: parts.get(1).copied().unwrap_or_default().to_string(),
                kind: if kind == "?" {
                    FieldKind::Checkbox
                } else {
                    FieldKind::Radio
                },
                value: parts.get(2).copied().unwrap_or_default().to_string(),
                checked: parts.get(3).is_some_and(|p| *p == "*"),
                label: default.to_string(),
                width: 0,
            },
            _ => {
                let name = parts.last().copied().unwrap_or_default();
                let flags = if parts.len() > 1 { parts[0] } else { "" };
                let masked = flags.contains('!');
                let width = flags
                    .trim_matches('!')
                    .parse()
                    .unwrap_or(24usize)
                    .clamp(1, 256);
                Field {
                    name: name.to_string(),
                    kind: FieldKind::Text { masked },
                    value: default.to_string(),
                    label: String::new(),
                    checked: false,
                    width,
                }
            }
        };
        self.page.fields.push(field);
        self.page
            .items
            .push(Interactive::Field(self.page.fields.len() - 1));
        MSpan {
            text: String::new(), // rendered from live field state in layout()
            style: base.patch(self.format.style()),
            item: Some(self.page.items.len() - 1),
        }
    }
}

/// Seconds from a page's `#!c=N` cache directive (`0` means do not cache).
pub fn cache_directive(source: &str) -> Option<u64> {
    source
        .lines()
        .take_while(|line| line.starts_with("#!"))
        .find_map(|line| line.strip_prefix("#!c=")?.trim().parse().ok())
}

impl Page {
    /// URLs of inline images, in page order.
    pub fn image_urls(&self) -> Vec<String> {
        self.lines
            .iter()
            .filter_map(|line| match line {
                MLine::Image { url, .. } if !url.is_empty() => Some(url.clone()),
                _ => None,
            })
            .collect()
    }

    /// Form values submitted with a link, as NomadNet `field_*` / `var_*` keys.
    pub fn request_fields(&self, spec: &[String]) -> std::collections::BTreeMap<String, String> {
        let mut out = std::collections::BTreeMap::new();
        let all = spec.iter().any(|s| s == "*");
        for field in &self.fields {
            if !(all || spec.iter().any(|s| s == &field.name)) {
                continue;
            }
            let key = format!("field_{}", field.name);
            match field.kind {
                FieldKind::Text { .. } => {
                    out.insert(key, field.value.clone());
                }
                FieldKind::Checkbox if field.checked => {
                    out.entry(key)
                        .and_modify(|v: &mut String| {
                            v.push(',');
                            v.push_str(&field.value);
                        })
                        .or_insert_with(|| field.value.clone());
                }
                FieldKind::Radio if field.checked => {
                    out.insert(key, field.value.clone());
                }
                _ => {}
            }
        }
        for var in spec {
            if let Some((k, v)) = var.split_once('=') {
                out.insert(format!("var_{k}"), v.to_string());
            }
        }
        out
    }

    /// Toggle a checkbox, or select a radio and clear its siblings.
    pub fn activate_choice(&mut self, index: usize) {
        match self.fields[index].kind {
            FieldKind::Checkbox => self.fields[index].checked = !self.fields[index].checked,
            FieldKind::Radio => {
                let name = self.fields[index].name.clone();
                for field in &mut self.fields {
                    if field.kind == FieldKind::Radio && field.name == name {
                        field.checked = false;
                    }
                }
                self.fields[index].checked = true;
            }
            FieldKind::Text { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::layout::Layout;
    use super::*;
    use crate::term::images::{Graphics, Picture};

    fn text(layout: &Layout) -> Vec<String> {
        layout
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn headings_dividers_and_comments() {
        let page = parse("#!c=0\n>Title\nbody\n-\n>>Sub\nmore");
        let out = text(&page.layout(10, None, &HashMap::new(), None));
        assert_eq!(out[0], "Title     ");
        assert_eq!(out[1], "body");
        assert_eq!(out[2], "\u{2500}".repeat(10));
        assert_eq!(out[3], "  Sub     ");
        assert_eq!(out[4], "  more");
    }

    #[test]
    fn inline_formatting_splits_spans() {
        let page = parse("plain `!bold`! `Ff00red`f done");
        let layout = page.layout(80, None, &HashMap::new(), None);
        let spans = &layout.lines[0].spans;
        assert_eq!(spans[0].content, "plain ");
        assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[3].style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(spans[4].content, " done");
    }

    #[test]
    fn links_and_fields() {
        let page = parse(
            "`[Home`:/page/index.mu] `[abc`deadbeef:/page/x.mu`q|v=1]\n`<16|q`hi> `<?|opt|yes|*`Opt>",
        );
        assert_eq!(page.items.len(), 4);
        assert_eq!(
            page.items[0],
            Interactive::Link {
                url: ":/page/index.mu".into(),
                fields: vec![]
            }
        );
        let Interactive::Link { fields, .. } = &page.items[1] else {
            panic!()
        };
        let submitted = page.request_fields(fields);
        assert_eq!(submitted.get("field_q").map(String::as_str), Some("hi"));
        assert_eq!(submitted.get("var_v").map(String::as_str), Some("1"));
        assert!(page.fields[1].checked);
        let layout = page.layout(80, Some(2), &HashMap::new(), None);
        assert_eq!(layout.item_rows, vec![0, 0, 1, 1]);
        // "Home" starts at column 0; the second link follows "Home ".
        assert_eq!(hit_at(&layout.hits, 0, 0), Some(0));
        assert_eq!(hit_at(&layout.hits, 0, 5), Some(1));
        assert_eq!(hit_at(&layout.hits, 0, 4), None);
    }

    #[test]
    fn literal_blocks_are_not_parsed() {
        let page = parse("`=\n>not a heading `!x\n`=\n>real");
        let out = text(&page.layout(20, None, &HashMap::new(), None));
        assert_eq!(out[0], ">not a heading `!x");
        assert!(out[1].starts_with("real"));
    }

    #[test]
    fn centered_heading_and_image_placeholder() {
        let page = parse(">`cMid\n`(Logo`w=30`a=c`:/media/logo.webp)");
        let layout = page.layout(15, None, &HashMap::new(), None);
        assert_eq!(text(&layout)[0], "      Mid      ");
        assert_eq!(text(&layout)[1], "[image: Logo]");
        assert_eq!(layout.lines[1].alignment, Some(Alignment::Center));
        assert_eq!(page.image_urls(), vec![":/media/logo.webp".to_string()]);
        assert_eq!(layout.lines.len(), 2); // no blank row left by the image line
        // The image's alignment doesn't carry over to the lines after it.
        let page = parse("`(Logo`a=c`:/media/logo.webp)\nafter");
        let layout = page.layout(15, None, &HashMap::new(), None);
        assert_eq!(layout.lines[1].alignment, Some(Alignment::Left));
    }

    #[test]
    fn truecolor_and_page_directives() {
        let page = parse("#!bg=020617\n`FT86efacGo`f `F0f0x");
        assert_eq!(page.base_style.bg, Some(Color::Rgb(0x02, 0x06, 0x17)));
        let layout = page.layout(40, None, &HashMap::new(), None);
        let spans = &layout.lines[0].spans;
        assert_eq!(spans[0].content, "Go");
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(0x86, 0xef, 0xac)));
        assert_eq!(spans[2].style.fg, Some(Color::Rgb(0, 255, 0)));
    }

    #[test]
    fn centered_links_are_clickable_where_drawn() {
        let page = parse("`c`[ab`:/x]");
        let layout = page.layout(10, None, &HashMap::new(), None);
        assert_eq!(hit_at(&layout.hits, 0, 4), Some(0));
        assert_eq!(hit_at(&layout.hits, 0, 5), Some(0));
        assert_eq!(hit_at(&layout.hits, 0, 3), None);
    }

    #[test]
    fn cache_directive_is_read_from_the_header() {
        assert_eq!(cache_directive("#!c=0\n>Hi"), Some(0));
        assert_eq!(cache_directive("#!bg=000\n#!c=3600\ntext"), Some(3600));
        assert_eq!(cache_directive(">Hi\n#!c=5"), None);
    }

    #[test]
    fn kitty_images_reserve_rows() {
        let mut png = Vec::new();
        image::DynamicImage::new_rgba8(200, 100)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let page = parse("top\n`c`(Pic`:/media/p.png)\nbottom");
        let images = HashMap::from([(":/media/p.png".to_string(), Picture::decode(&png).unwrap())]);
        let gfx = Graphics::for_tests(ratatui_image::picker::ProtocolType::Kitty);
        let layout = page.layout(40, None, &images, Some(&gfx));
        let placement = &layout.placements[0];
        assert_eq!(placement.row, 1);
        // 20 cols wide, centred in 40.
        assert_eq!((placement.col, placement.size.width), (10, 20));
        let bottom = 1 + usize::from(placement.size.height);
        assert_eq!(text(&layout)[bottom], "bottom");
    }

    #[test]
    fn image_alt_text_may_contain_parentheses() {
        let page = parse(
            "`(Swapfest, Aug 23 (photo: JohnC)`w=80`a=c`:/media/img/a.webp) after\n`(broken (x)",
        );
        assert_eq!(page.image_urls(), vec![":/media/img/a.webp".to_string()]);
        let out = text(&page.layout(80, None, &HashMap::new(), None));
        assert!(out.iter().any(|l| l.contains("[image: Swapfest, Aug 23 (photo: JohnC)]")));
        assert!(out.iter().any(|l| l.contains("after")));
    }

    /// Malformed markup must never panic: parse and lay out random mixes
    /// of Micron control characters.
    #[test]
    fn random_markup_never_panics() {
        const ALPHABET: &[char] = &[
            '`', '[', ']', '(', ')', '<', '>', '!', '*', '_', 'F', 'f', 'B', 'b', 'T', 'g', 'c',
            'l', 'r', 'a', '=', '|', '?', '^', '\\', '#', '-', '0', '9', 'e', ':', '/', ' ',
            '\n', 'é', '全',
        ];
        let mut seed: u64 = 0x5eed;
        for _ in 0..3000 {
            let mut text = String::new();
            for _ in 0..(seed % 60) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                text.push(ALPHABET[(seed % ALPHABET.len() as u64) as usize]);
            }
            let mut page = parse(&text);
            for width in [1, 7, 40] {
                let layout = page.layout(width, Some(0), &HashMap::new(), None);
                let _ = hit_at(&layout.hits, 0, 0);
            }
            for f in 0..page.fields.len() {
                page.activate_choice(f);
            }
            let specs: Vec<String> = vec!["*".into(), "a=b".into()];
            let _ = page.request_fields(&specs);
            let _ = cache_directive(&text);
        }
    }

    #[test]
    fn long_lines_wrap() {
        let page = parse("abcdefghij");
        assert_eq!(text(&page.layout(4, None, &HashMap::new(), None)), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn lines_wrap_between_words() {
        let page = parse("Welcome visitors to the node");
        let out = text(&page.layout(12, None, &HashMap::new(), None));
        assert_eq!(out, vec!["Welcome", "visitors to", "the node"]);
        // Words keep their styles across a break, and leading spaces stay.
        let page = parse("  a `!bold words`! end");
        let layout = page.layout(8, None, &HashMap::new(), None);
        assert_eq!(text(&layout), vec!["  a bold", "words", "end"]);
        assert!(layout.lines[1].spans[0].style.add_modifier.contains(Modifier::BOLD));
        // A link that moves down is clickable (and selected) on its new row.
        let page = parse("some text `[a link`:/x]");
        let layout = page.layout(12, None, &HashMap::new(), None);
        assert_eq!(text(&layout), vec!["some text a", "link"]);
        assert_eq!(layout.item_rows, vec![0]);
        assert_eq!(hit_at(&layout.hits, 1, 0), Some(0));
        let page = parse("some text here `[link`:/x]");
        let layout = page.layout(12, None, &HashMap::new(), None);
        assert_eq!(layout.item_rows, vec![1]);
        // Wide characters count double.
        let page = parse("全全 全全");
        assert_eq!(text(&page.layout(5, None, &HashMap::new(), None)), vec!["全全", "全全"]);
    }

    #[test]
    fn emoji_sequences_are_measured_as_drawn() {
        // ❤️ (with its emoji selector), 👍🏽 and a family are two columns each,
        // as terminals draw them, not the sum of their parts (1, 4 and 6).
        // (Counted by parts, "ab ❤️❤️" was 5 columns and drawn 7 wide in 6.)
        let page = parse("ab ❤️❤️ cd");
        assert_eq!(text(&page.layout(6, None, &HashMap::new(), None)), vec!["ab", "❤️❤️", "cd"]);
        let page = parse("👍🏽 👨‍👩‍👧");
        assert_eq!(text(&page.layout(5, None, &HashMap::new(), None)), vec!["👍🏽 👨‍👩‍👧"]);
        // Its drawn width is what centring uses, and it is never split.
        let page = parse("`c❤️");
        let layout = page.layout(10, None, &HashMap::new(), None);
        assert_eq!(layout.lines[0].width(), 2);
        let page = parse("👨‍👩‍👧👨‍👩‍👧");
        assert_eq!(text(&page.layout(3, None, &HashMap::new(), None)), vec!["👨‍👩‍👧", "👨‍👩‍👧"]);
    }
}

