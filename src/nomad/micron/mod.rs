//! Micron markup (NomadNet page format) parsing and terminal layout.
//!
//! Parsing produces a width-independent [`Page`]; [`Page::layout`] wraps it
//! to the viewport and applies the current selection, form values and
//! folded sections. [`html`] renders the same page for the web UI.
//!
//! Besides text formatting, links, fields and images, it reads what
//! NomadNet's Micron guide describes for:
//!
//! - tables, between `` `t `` lines (`` `tc30 ``: centred, at most 30
//!   columns), one row a line as `| a | b |`, with a `| --- | :-: | --: |`
//!   row setting the columns' alignment (and making the row above it a
//!   header);
//! - partials, `` `{url`refresh`fields} ``: part of the page loaded (and
//!   reloaded every `refresh` seconds) on its own, which links to `p:id`
//!   reload (`pid=id` among its fields);
//! - collapsible headings, `` `+> `` (open) and `` `-> `` (folded);
//! - anchors: every heading's text as a slug, and `` `:name ``, which links
//!   to `#name` (or `#`, the next heading) jump to;
//! - text fields of several rows, `` `<40x5|notes`> ``.

use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};

pub mod html;
pub(crate) mod layout;
pub mod source;

pub use layout::hit_at;

const INDENT: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interactive {
    Link {
        url: String,
        fields: Vec<String>,
    },
    Field(usize),
    /// A collapsible heading (its fold): activating it folds or opens it.
    Fold(usize),
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
    /// Rows a text field takes: more than one for longer, multi-line text.
    pub rows: usize,
}

#[derive(Debug, Clone)]
struct MSpan {
    text: String,
    style: Style,
    item: Option<usize>,
}

/// A heading: its level, and its fold if it's collapsible.
#[derive(Debug, Clone, Copy)]
struct Section {
    level: usize,
    fold: Option<usize>,
}

/// A table: where it sits, how wide it may be, its columns' alignment, and
/// its rows of cells (the first one a header when a separator follows it).
#[derive(Debug, Clone)]
struct Table {
    indent: usize,
    align: Alignment,
    max_width: Option<usize>,
    columns: Vec<Alignment>,
    header: bool,
    rows: Vec<Vec<Vec<MSpan>>>,
}

#[derive(Debug, Clone)]
enum MLine {
    Text {
        indent: usize,
        align: Alignment,
        fill: Option<Style>,
        spans: Vec<MSpan>,
        /// Set on a heading.
        section: Option<Section>,
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
    Table(Table),
    /// A text field of several rows (its item), on rows of its own.
    FieldBlock {
        indent: usize,
        item: usize,
        style: Style,
    },
    /// A partial not loaded (yet): its index in [`Page::partials`].
    Partial {
        indent: usize,
        index: usize,
    },
    /// A `<` line: sections before it end (and folding with them).
    SectionEnd,
}

/// Part of a page loaded on its own (`` `{url`refresh`fields} ``).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partial {
    pub url: String,
    /// Seconds between reloads, if it reloads.
    pub refresh: Option<u64>,
    /// Fields (and variables) sent with its request, as a link's.
    pub fields: Vec<String>,
    /// Its id, from a `pid=` variable: `p:id` links reload it.
    pub pid: Option<String>,
}

/// A collapsible heading's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fold {
    pub open: bool,
    /// Its heading's text (to keep it open or folded when the page is read
    /// again).
    pub title: String,
}

#[derive(Debug, Clone, Default)]
pub struct Page {
    lines: Vec<MLine>,
    /// Page colours from `#!fg=` / `#!bg=` directives.
    pub base_style: Style,
    pub items: Vec<Interactive>,
    pub fields: Vec<Field>,
    pub folds: Vec<Fold>,
    /// The marks of open and folded sections (`#!fold` sets them).
    pub fold_marks: Option<(String, String)>,
    /// Anchors and the line each is on, first declared first.
    anchors: Vec<(String, usize)>,
    pub partials: Vec<Partial>,
    /// The line each item is on.
    item_lines: Vec<usize>,
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

/// NomadNet's heading colours (its dark theme), which pages are made for.
fn heading_style(level: usize) -> Style {
    let (fg, bg) = match level {
        1 => (0x22, 0xbb),
        2 => (0x11, 0x99),
        _ => (0x00, 0x77),
    };
    Style::default().fg(Color::Rgb(fg, fg, fg)).bg(Color::Rgb(bg, bg, bg))
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

/// A heading's text as an anchor name: lowercase, each run of anything
/// but ASCII letters and digits as one hyphen, none at either end.
pub fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut gap = false;
    for c in text.chars().map(|c| c.to_ascii_lowercase()) {
        if c.is_ascii_alphanumeric() {
            if gap && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(c);
            gap = false;
        } else {
            gap = true;
        }
    }
    slug
}

/// Whether `rest` (after `` `t ``) opens or closes a table: an alignment
/// letter and a width, both optional.
fn table_options(rest: &str) -> Option<(Alignment, Option<usize>)> {
    let rest = rest.trim_end();
    let (align, width) = match rest.chars().next() {
        Some('l') => (Alignment::Left, &rest[1..]),
        Some('c') => (Alignment::Center, &rest[1..]),
        Some('r') => (Alignment::Right, &rest[1..]),
        _ => (Alignment::Left, rest),
    };
    if width.is_empty() {
        return Some((align, None));
    }
    width.parse().ok().map(|w: usize| (align, Some(w.max(1))))
}

/// A table row's cells: split at `|`, except inside links and fields (whose
/// parts are `|`-separated too) and where escaped.
fn table_cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = if line.ends_with('|') && !line.ends_with("\\|") { &line[..line.len() - 1] } else { line };
    let mut cells = vec![String::new()];
    let mut closing: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        let cell = cells.last_mut().expect("there is a cell");
        match c {
            '\\' => {
                cell.push(c);
                if let Some(next) = chars.next() {
                    cell.push(next);
                }
            }
            '`' if closing.is_none() => {
                cell.push(c);
                match chars.peek() {
                    Some('[') => closing = Some(']'),
                    Some('<') => closing = Some('>'),
                    _ => {}
                }
            }
            '|' if closing.is_none() => cells.push(String::new()),
            c => {
                if closing == Some(c) {
                    closing = None;
                }
                cell.push(c);
            }
        }
    }
    cells
}

/// A separator cell's column alignment (`---`, `:--`, `:-:`, `--:`).
fn separator(cell: &str) -> Option<Alignment> {
    let cell = cell.trim();
    if !cell.contains('-') || !cell.chars().all(|c| c == '-' || c == ':') {
        return None;
    }
    Some(match (cell.starts_with(':'), cell.ends_with(':')) {
        (true, true) => Alignment::Center,
        (false, true) => Alignment::Right,
        _ => Alignment::Left,
    })
}

struct Parser<'a> {
    page: Page,
    /// Images (and other blocks) found mid-line, emitted after that line.
    images: Vec<MLine>,
    format: Format,
    align: Alignment,
    depth: usize,
    literal: bool,
    /// The table being read, between its `` `t `` lines.
    table: Option<Table>,
    /// Partials' contents, by index, where they've loaded.
    loaded: &'a [Option<String>],
    /// Partials found on the line being read, to put after it.
    partials_due: Vec<usize>,
    /// Reading a partial's content: partials in it aren't loaded.
    nested: bool,
}

/// Parse a page whose partials haven't loaded.
pub fn parse(source: &str) -> Page {
    parse_with(source, &[])
}

/// Parse a page with its partials' contents where they've loaded (by
/// index; see [`Page::partials`]), in their places.
pub fn parse_with(source: &str, loaded: &[Option<String>]) -> Page {
    let mut parser = Parser {
        page: Page::default(),
        images: Vec::new(),
        format: Format::default(),
        align: Alignment::Left,
        depth: 0,
        literal: false,
        table: None,
        loaded,
        partials_due: Vec::new(),
        nested: false,
    };
    for raw in source.lines() {
        parser.line(raw.trim_end_matches('\r'));
    }
    parser.end_table();
    parser.page
}

/// Parse a partial's content on its own (for the web UI): partials in it
/// aren't loaded.
pub fn parse_partial(source: &str) -> Page {
    let mut page = {
        let mut parser = Parser {
            page: Page::default(),
            images: Vec::new(),
            format: Format::default(),
            align: Alignment::Left,
            depth: 0,
            literal: false,
            table: None,
            loaded: &[],
            partials_due: Vec::new(),
            nested: true,
        };
        for raw in source.lines() {
            parser.line(raw.trim_end_matches('\r'));
        }
        parser.end_table();
        parser.page
    };
    page.partials.clear();
    page
}

impl Parser<'_> {
    fn content_indent(&self) -> usize {
        self.depth.saturating_sub(1) * INDENT
    }

    /// A row takes the background in effect where its line ends, across the
    /// whole width (as NomadNet), so `B000 on a line colours all of it.
    fn row_fill(&self) -> Option<Style> {
        self.format.bg.map(|bg| Style::default().bg(bg))
    }

    fn line(&mut self, line: &str) {
        let before = self.page.lines.len();
        self.parse_line(line);
        if !self.images.is_empty() {
            // A line holding only an image should not leave a blank row behind.
            if self.page.lines.len() == before + 1
                && let Some(MLine::Text { spans, .. }) = self.page.lines.last()
                && spans.iter().all(|s| s.text.trim().is_empty() && s.item.is_none())
            {
                self.page.lines.pop();
            }
            self.page.lines.append(&mut self.images);
        }
        for index in std::mem::take(&mut self.partials_due) {
            self.place_partial(index);
        }
    }

    /// A partial in its place: its content if it has loaded, else a
    /// placeholder. Its content is read as a page of its own (formatting
    /// starts afresh, and ends with it).
    fn place_partial(&mut self, index: usize) {
        let Some(content) = self.loaded.get(index).cloned().flatten() else {
            let indent = self.content_indent();
            self.page.lines.push(MLine::Partial { indent, index });
            return;
        };
        let saved = (self.format, self.align, self.depth, self.literal, self.nested);
        (self.format, self.align, self.literal, self.nested) = (Format::default(), Alignment::Left, false, true);
        for raw in content.lines() {
            self.line(raw.trim_end_matches('\r'));
        }
        self.end_table();
        (self.format, self.align, self.depth, self.literal, self.nested) = saved;
    }

    fn end_table(&mut self) {
        if let Some(table) = self.table.take() {
            self.page.lines.push(MLine::Table(table));
        }
    }

    /// A line between a table's `` `t `` lines: a row, or the separator.
    fn table_row(&mut self, line: &str) {
        let cells = table_cells(line);
        let Some(table) = self.table.as_mut() else { return };
        if let Some(columns) = cells.iter().map(|c| separator(c)).collect::<Option<Vec<_>>>() {
            table.columns = columns;
            table.header = table.rows.len() == 1;
            return;
        }
        // Each cell starts from the formatting in effect at the row.
        let saved = self.format;
        let mut row = Vec::new();
        for cell in &cells {
            self.format = saved;
            row.push(self.inline(cell.trim(), Style::default()));
        }
        self.format = saved;
        // Images aren't drawn in cells.
        self.images.clear();
        if let Some(table) = self.table.as_mut() {
            table.rows.push(row);
        }
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
                fill: self.row_fill(),
                spans: vec![MSpan { text: line.replace('\t', "    "), style: self.format.style(), item: None }],
                section: None,
            });
            return;
        }
        if let Some((align, max_width)) = line.strip_prefix("`t").and_then(table_options) {
            match self.table.take() {
                Some(table) => self.page.lines.push(MLine::Table(table)),
                None => {
                    let table =
                        Table { indent: self.content_indent(), align, max_width, columns: Vec::new(), header: false, rows: Vec::new() };
                    self.table = Some(table);
                }
            }
            return;
        }
        if self.table.is_some() {
            self.table_row(line);
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
                _ => {
                    // `#!fold [-] [+]`: the open mark, then the folded one.
                    if let Some(marks) = directive.strip_prefix("fold ") {
                        let mut marks = marks.split_whitespace();
                        if let (Some(open), Some(folded)) = (marks.next(), marks.next()) {
                            self.page.fold_marks = Some((open.to_string(), folded.to_string()));
                        }
                    }
                }
            }
            return;
        }
        if line.starts_with('#') {
            return;
        }
        if let Some(rest) = line.strip_prefix('<') {
            self.depth = 0;
            self.page.lines.push(MLine::SectionEnd);
            if rest.is_empty() {
                return;
            }
            line = rest;
        }
        // A collapsible heading: `+> open, `-> folded.
        let fold = match line.get(..3) {
            Some("`+>") => Some(true),
            Some("`->") => Some(false),
            _ => None,
        };
        if fold.is_some() {
            line = &line[2..];
        }
        if line.starts_with('>') {
            let level = line.chars().take_while(|&c| c == '>').count();
            self.depth = level;
            let text = line[level..].trim();
            if text.is_empty() {
                return;
            }
            let style = heading_style(level);
            // Headings have their own colours, whatever the lines before set;
            // codes inside one apply to it alone.
            let saved = std::mem::take(&mut self.format);
            let mut spans = self.inline(text, style);
            self.format = saved;
            let title: String = spans.iter().map(|s| s.text.as_str()).collect();
            let at = self.page.lines.len();
            self.anchor(slugify(&title), at);
            // The row takes the background its text starts with.
            let fill = match spans.first().and_then(|s| s.style.bg) {
                Some(bg) => style.bg(bg),
                None => style,
            };
            // A collapsible one: its mark first, and the whole heading
            // (but its links) folds or opens it.
            let fold = fold.map(|open| {
                let index = self.page.folds.len();
                self.page.folds.push(Fold { open, title: title.trim().to_string() });
                let item = self.item(Interactive::Fold(index));
                for span in spans.iter_mut().filter(|s| s.item.is_none()) {
                    span.item = Some(item);
                }
                spans.insert(0, MSpan { text: String::new(), style, item: Some(item) });
                index
            });
            self.page.lines.push(MLine::Text {
                indent: level.saturating_sub(1) * INDENT,
                align: self.align,
                fill: Some(fill),
                spans,
                section: Some(Section { level, fold }),
            });
            return;
        }
        if let Some(rest) = line.strip_prefix('-') {
            // "-=" etc. repeat the given character; anything else ("-",
            // "---") is a thin rule.
            let mut chars = rest.chars();
            let ch = match (chars.next(), chars.next()) {
                (Some(ch), None) if !ch.is_control() => ch,
                _ => '\u{2500}',
            };
            self.page.lines.push(MLine::Divider { indent: self.content_indent(), ch, style: self.format.style() });
            return;
        }
        let spans = self.inline(line, Style::default());
        // A line of only codes (`c, `Bddd) sets them for the lines after it
        // but isn't shown; an empty line is a blank row.
        if spans.is_empty() && !line.is_empty() {
            return;
        }
        // Fields of several rows go on rows of their own, between the
        // text before and after them.
        let tall: Vec<bool> = spans
            .iter()
            .map(|span| match span.item.map(|i| &self.page.items[i]) {
                Some(Interactive::Field(f)) => self.page.fields[*f].rows > 1,
                _ => false,
            })
            .collect();
        if !tall.contains(&true) {
            self.page.lines.push(MLine::Text {
                indent: self.content_indent(),
                align: self.align,
                fill: self.row_fill(),
                spans,
                section: None,
            });
            return;
        }
        // The text either side, without the spaces that set the field apart.
        let mut run: Vec<MSpan> = Vec::new();
        let push_run = |parser: &mut Self, mut run: Vec<MSpan>| {
            if let Some(first) = run.first_mut().filter(|s| s.item.is_none()) {
                first.text = first.text.trim_start().to_string();
            }
            if let Some(last) = run.last_mut().filter(|s| s.item.is_none()) {
                last.text = last.text.trim_end().to_string();
            }
            if run.iter().any(|s| !s.text.is_empty() || s.item.is_some()) {
                let (indent, align, fill) = (parser.content_indent(), parser.align, parser.row_fill());
                parser.page.lines.push(MLine::Text { indent, align, fill, spans: run, section: None });
            }
        };
        for (span, tall) in spans.into_iter().zip(tall) {
            if !tall {
                run.push(span);
                continue;
            }
            push_run(self, std::mem::take(&mut run));
            let item = span.item.expect("a field has an item");
            self.page.lines.push(MLine::FieldBlock { indent: self.content_indent(), item, style: span.style });
        }
        push_run(self, run);
    }

    /// Add an interactive item, on the line being read.
    fn item(&mut self, item: Interactive) -> usize {
        self.page.items.push(item);
        self.page.item_lines.push(self.page.lines.len());
        self.page.items.len() - 1
    }

    /// Declare an anchor at `line` (the first of a name wins).
    fn anchor(&mut self, name: String, line: usize) {
        if !name.is_empty() && !self.page.anchors.iter().any(|(n, _)| *n == name) {
            self.page.anchors.push((name, line));
        }
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
                    spans.push(MSpan { text: std::mem::take(&mut buf), style: base.patch(self.format.style()), item: None });
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
                // An anchor: a name of letters, digits, `_` and `-`, taking
                // no room.
                ':' => {
                    let len = chars[i..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == '_' || **c == '-').count();
                    let name: String = chars[i..i + len].iter().collect();
                    i += len;
                    let line = self.page.lines.len();
                    self.anchor(name, line);
                }
                // A partial: `{url`refresh`fields}, loaded after the page.
                '{' => {
                    flush!();
                    let Some(len) = chars[i..].iter().position(|&c| c == '}') else {
                        break;
                    };
                    let body: String = chars[i..i + len].iter().collect();
                    i += len + 1;
                    if !self.nested {
                        let mut parts = body.split('`');
                        let url = parts.next().unwrap_or_default().trim().to_string();
                        let refresh = parts.next().and_then(|r| r.trim().parse().ok()).filter(|r: &u64| *r > 0);
                        let fields: Vec<String> = parts
                            .next()
                            .unwrap_or_default()
                            .split('|')
                            .map(str::trim)
                            .filter(|f| !f.is_empty())
                            .map(str::to_string)
                            .collect();
                        let pid = fields.iter().find_map(|f| f.strip_prefix("pid=")).map(str::to_string);
                        if !url.is_empty() {
                            self.page.partials.push(Partial { url, refresh, fields, pid });
                            self.partials_due.push(self.page.partials.len() - 1);
                        }
                    }
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
                            "a=c" | "a=center" => align = Alignment::Center,
                            "a=r" | "a=right" => align = Alignment::Right,
                            "a=l" | "a=left" => align = Alignment::Left,
                            _ => {}
                        }
                    }
                    let alt = parts.first().copied().unwrap_or_default();
                    let alt = if alt.is_empty() { "image" } else { alt };
                    let width = parts.iter().find_map(|p| p.strip_prefix("w=")?.parse().ok());
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
        let fields = fields.split('|').map(str::trim).filter(|f| !f.is_empty()).map(str::to_string).collect();
        let mut style = base.patch(self.format.style()).add_modifier(Modifier::UNDERLINED);
        if self.format.fg.is_none() {
            style = style.fg(Color::LightBlue);
        }
        let item = self.item(Interactive::Link { url: url.to_string(), fields });
        MSpan { text: label.to_string(), style, item: Some(item) }
    }

    /// Fields look like `name`default`, `24|name`default`, `!24|pass``,
    /// `40x5|notes`` (several rows) or `?|name|value|*`label` (checkbox) /
    /// `^|name|value|*`label` (radio).
    fn field(&mut self, body: &str, base: Style) -> MSpan {
        let (spec, default) = body.split_once('`').unwrap_or((body, ""));
        let parts: Vec<&str> = spec.split('|').collect();
        let field = match parts.first().copied() {
            Some(kind @ ("?" | "^")) => Field {
                name: parts.get(1).copied().unwrap_or_default().to_string(),
                kind: if kind == "?" { FieldKind::Checkbox } else { FieldKind::Radio },
                value: parts.get(2).copied().unwrap_or_default().to_string(),
                checked: parts.get(3).is_some_and(|p| *p == "*"),
                label: default.to_string(),
                width: 0,
                rows: 1,
            },
            _ => {
                let name = parts.last().copied().unwrap_or_default();
                let flags = if parts.len() > 1 { parts[0] } else { "" };
                let masked = flags.contains('!');
                let size = flags.trim_matches('!');
                let (width, rows) = size.split_once('x').unwrap_or((size, ""));
                Field {
                    name: name.to_string(),
                    kind: FieldKind::Text { masked },
                    value: default.to_string(),
                    label: String::new(),
                    checked: false,
                    width: width.parse().unwrap_or(24usize).clamp(1, 256),
                    rows: rows.parse().unwrap_or(1usize).clamp(1, 50),
                }
            }
        };
        self.page.fields.push(field);
        let item = self.item(Interactive::Field(self.page.fields.len() - 1));
        MSpan {
            text: String::new(), // rendered from live field state in layout()
            style: base.patch(self.format.style()),
            item: Some(item),
        }
    }
}

/// Seconds from a page's `#!c=N` cache directive (`0` means do not cache).
pub fn cache_directive(source: &str) -> Option<u64> {
    source.lines().take_while(|line| line.starts_with("#!")).find_map(|line| line.strip_prefix("#!c=")?.trim().parse().ok())
}

/// A request's `var_anchor` (a link's `anchor=name`): where on the page it
/// loads to jump.
pub fn anchor_variable(fields: &std::collections::BTreeMap<String, String>) -> Option<&str> {
    fields.get("var_anchor").map(String::as_str).filter(|a| !a.is_empty())
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

    /// The marks of an open and a folded section.
    pub fn fold_mark(&self, open: bool) -> &str {
        match (&self.fold_marks, open) {
            (Some((mark, _)), true) => mark,
            (Some((_, mark)), false) => mark,
            (None, true) => "▾",
            (None, false) => "▸",
        }
    }

    /// The line an anchor is on (`name`, or with `#` before it).
    pub fn anchor_line(&self, name: &str) -> Option<usize> {
        let name = name.trim_start_matches('#');
        self.anchors.iter().find(|(n, _)| n == name).map(|(_, line)| *line)
    }

    /// The first heading after item `item` (where a link to `#` jumps).
    pub fn next_heading_line(&self, item: usize) -> Option<usize> {
        let from = *self.item_lines.get(item)?;
        self.lines
            .iter()
            .enumerate()
            .skip(from + 1)
            .find(|(_, line)| matches!(line, MLine::Text { section: Some(_), .. }))
            .map(|(at, _)| at)
    }

    /// Partials whose id is one of `ids` (a `p:` link's, `|`- or
    /// `,`-separated).
    pub fn partials_with_ids(&self, ids: &str) -> Vec<usize> {
        let ids: Vec<&str> = ids.split(['|', ',']).map(str::trim).filter(|i| !i.is_empty()).collect();
        self.partials.iter().enumerate().filter(|(_, p)| p.pid.as_deref().is_some_and(|pid| ids.contains(&pid))).map(|(i, _)| i).collect()
    }

    /// Keep what was typed, ticked and folded on the page this one replaces
    /// (the same page read again with a partial loaded): fields by name and
    /// kind, folds by heading, in order.
    pub fn keep_state(&mut self, old: &Page) {
        let mut used = vec![false; old.fields.len()];
        for field in &mut self.fields {
            // Choices of one name differ by value; text fields by order.
            let same = old.fields.iter().enumerate().find(|(i, f)| {
                !used[*i]
                    && f.name == field.name
                    && f.kind == field.kind
                    && (matches!(f.kind, FieldKind::Text { .. }) || f.value == field.value)
            });
            if let Some((i, old)) = same {
                used[i] = true;
                field.value = old.value.clone();
                field.checked = old.checked;
            }
        }
        let mut used = vec![false; old.folds.len()];
        for fold in &mut self.folds {
            if let Some((i, old)) = old.folds.iter().enumerate().find(|(i, f)| !used[*i] && f.title == fold.title) {
                used[i] = true;
                fold.open = old.open;
            }
        }
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
        layout.lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn headings_dividers_and_comments() {
        let page = parse("#!c=0\n>Title\nbody\n-\n>>Sub\nmore\n<---\n-=");
        let out = text(&page.layout(10, None, &HashMap::new(), None));
        assert_eq!(out[0], "Title     ");
        assert_eq!(out[1], "body");
        assert_eq!(out[2], "\u{2500}".repeat(10));
        assert_eq!(out[3], "  Sub     ");
        assert_eq!(out[4], "  more");
        // "---" is a thin rule too; "-=" repeats "=".
        assert_eq!(out[5], "\u{2500}".repeat(10));
        assert_eq!(out[6], "=".repeat(10));
    }

    /// As NomadNet draws them (a page banner: a black band, a centred
    /// title, then a heading).
    #[test]
    fn rows_take_the_background_their_line_ends_with() {
        let black = Some(Color::Rgb(0, 0, 0));
        let page = parse("`Fddd`B000\n---\n`c\nLogo\n`a\n\n>>Head\nbody`b\n<plain");
        let layout = page.layout(10, None, &HashMap::new(), None);
        // Lines of only codes are not rows; an empty line is.
        let out = text(&layout);
        let rule = "\u{2500}".repeat(10);
        let blank = " ".repeat(10);
        assert_eq!(out, [&rule, "   Logo   ", &blank, "  Head    ", "  body", "plain"]);
        // The background fills the whole row, the rule's too.
        for row in 0..3 {
            assert!(layout.lines[row].spans.iter().all(|s| s.style.bg == black), "row {row}");
        }
        // A heading keeps its own colours over the page's, across the
        // row and its indent.
        let head = &layout.lines[3].spans;
        assert!(head.iter().all(|s| s.style.bg == Some(Color::Rgb(0x99, 0x99, 0x99))));
        assert_eq!(head[1].style.fg, Some(Color::Rgb(0x11, 0x11, 0x11)));
        // The background ended with the line, so only the text has it.
        assert_eq!(layout.lines[4].spans[1].style.bg, black);
        assert_eq!(layout.lines[4].width(), 6);
        // Text with no background of its own shows its row's.
        let page = parse(" `Ff00x`B00fy");
        let layout = page.layout(4, None, &HashMap::new(), None);
        let blue = Some(Color::Rgb(0, 0, 255));
        assert!(layout.lines[0].spans.iter().all(|s| s.style.bg == blue));
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
        let page = parse("`[Home`:/page/index.mu] `[abc`deadbeef:/page/x.mu`q|v=1]\n`<16|q`hi> `<?|opt|yes|*`Opt>");
        assert_eq!(page.items.len(), 4);
        assert_eq!(page.items[0], Interactive::Link { url: ":/page/index.mu".into(), fields: vec![] });
        let Interactive::Link { fields, .. } = &page.items[1] else { panic!() };
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
        image::DynamicImage::new_rgba8(200, 100).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
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
        let page = parse("`(Swapfest, Aug 23 (photo: JohnC)`w=80`a=c`:/media/img/a.webp) after\n`(broken (x)");
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
            '`', '[', ']', '(', ')', '<', '>', '!', '*', '_', 'F', 'f', 'B', 'b', 'T', 'g', 'c', 'l', 'r', 'a', '=', '|', '?', '^', '\\',
            '#', '-', '0', '9', 'e', ':', '/', ' ', '\n', 'é', '全', 't', '{', '}', '+', 'x', '5',
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
            let loaded = [Some(text.clone())];
            let mut page = parse_with(&text, &loaded);
            for fold in &mut page.folds {
                fold.open = !fold.open;
            }
            for width in [1, 7, 40] {
                let layout = page.layout(width, Some(0), &HashMap::new(), None);
                let _ = hit_at(&layout.hits, 0, 0);
            }
            let _ = page.to_html(|u| u.to_string(), |_| None);
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

    #[test]
    fn tables_with_aligned_columns_and_a_header() {
        let source =
            "`t\n| Name | Price | Qty |\n| ---- | :---: | --: |\n| `[Apple`:/a] | Free | `!5`! |\n| Orange | Ask, nicely | 3 |\n`t\nafter";
        let page = parse(source);
        let layout = page.layout(40, None, &HashMap::new(), None);
        let out = text(&layout);
        assert_eq!(out[0], "┌────────┬─────────────┬─────┐");
        assert_eq!(out[1], "│ Name   │    Price    │ Qty │");
        assert_eq!(out[2], "├────────┼─────────────┼─────┤");
        assert_eq!(out[3], "│ Apple  │    Free     │   5 │");
        assert_eq!(out[4], "│ Orange │ Ask, nicely │   3 │");
        assert_eq!(out[5], "└────────┴─────────────┴─────┘");
        assert_eq!(out[6], "after");
        // The header is bold; a link in a cell is clickable where drawn.
        assert!(layout.lines[1].spans.iter().any(|s| s.content == "Name" && s.style.add_modifier.contains(Modifier::BOLD)));
        assert_eq!(hit_at(&layout.hits, 3, 2), Some(0));
        assert_eq!(layout.item_rows, vec![3]);
        // Narrower: the widest column gives way and its cells wrap.
        let out = text(&page.layout(26, None, &HashMap::new(), None));
        assert!(out.iter().all(|row| row.chars().count() <= 26), "{out:#?}");
        assert!(out.iter().any(|row| row.contains("Ask,")) && out.iter().any(|row| row.contains("nicely")), "{out:#?}");
        // `tc30: centred, and no wider than 30.
        let page = parse("`tc20\n| a long cell of text |\n`t");
        let out = text(&page.layout(40, None, &HashMap::new(), None));
        assert!(out.iter().all(|row| row.trim().chars().count() <= 20), "{out:#?}");
        assert!(out[0].starts_with("          ┌"), "{out:#?}");
        // Pipes inside links and fields don't split cells.
        let page = parse("`t\n| `[Go`:/x`a|b] | `<?|opt|1`> |\n`t");
        assert_eq!(page.items.len(), 2);
        let Interactive::Link { fields, .. } = &page.items[0] else { panic!() };
        assert_eq!(fields, &["a", "b"]);
    }

    #[test]
    fn collapsible_headings_fold_their_section() {
        let mut page = parse("`+>Open\none\n>>Inside\ntwo\n`->Folded\nthree\n>>Under\nfour\n>Next\nfive\n<\n`->Last\nsix\n<\nseven");
        let out = text(&page.layout(20, None, &HashMap::new(), None));
        let shown: Vec<&str> = out.iter().map(|l| l.trim_end()).collect();
        assert_eq!(shown, ["▾ Open", "one", "  Inside", "  two", "▸ Folded", "Next", "five", "▸ Last", "seven"]);
        // The heading opens (and folds) it.
        assert_eq!(page.items, [Interactive::Fold(0), Interactive::Fold(1), Interactive::Fold(2)]);
        page.folds[1].open = true;
        page.folds[0].open = false;
        let out = text(&page.layout(20, None, &HashMap::new(), None));
        let shown: Vec<&str> = out.iter().map(|l| l.trim_end()).collect();
        assert_eq!(shown, ["▸ Open", "▾ Folded", "three", "  Under", "  four", "Next", "five", "▸ Last", "seven"]);
        // A page may choose its own marks.
        let page = parse("#!fold [-] [+]\n`->Hidden\nx");
        assert_eq!(text(&page.layout(20, None, &HashMap::new(), None))[0].trim_end(), "[+] Hidden");
    }

    #[test]
    fn anchors_from_headings_and_tags() {
        let page = parse(">Hello World\n`[Down`#notes] `[Next`#]\ntext `:mid-line here\n`:notes\n>>Introduction & Setup!\nend");
        assert_eq!(slugify("Introduction & Setup!"), "introduction-setup");
        let anchors: Vec<(&str, usize)> = page.anchors.iter().map(|(n, l)| (n.as_str(), *l)).collect();
        assert_eq!(anchors, [("hello-world", 0), ("mid-line", 2), ("notes", 3), ("introduction-setup", 3)]);
        assert_eq!(page.anchor_line("#notes"), Some(3));
        // `#` alone: the next heading after the link.
        assert_eq!(page.next_heading_line(1), Some(3));
        let layout = page.layout(40, None, &HashMap::new(), None);
        assert_eq!(layout.line_rows[3], Some(3));
        // The tag takes no room.
        assert_eq!(text(&layout)[2], "text  here");
        let fields = std::collections::BTreeMap::from([("var_anchor".to_string(), "notes".to_string())]);
        assert_eq!(anchor_variable(&fields), Some("notes"));
    }

    #[test]
    fn partials_load_in_their_place() {
        let source = "top\n`{abcd:/page/p.mu`10`pid=32|name}\n`[Hi`p:32]\nName: `<name`Jo>";
        let page = parse(source);
        assert_eq!(
            page.partials,
            [Partial {
                url: "abcd:/page/p.mu".into(),
                refresh: Some(10),
                fields: vec!["pid=32".into(), "name".into()],
                pid: Some("32".into())
            }]
        );
        assert_eq!(page.partials_with_ids("31|32"), [0]);
        assert_eq!(text(&page.layout(40, None, &HashMap::new(), None))[1], "loading…");
        // Loaded: its lines in place, its links and fields part of the page,
        // and partials in it not loaded.
        let mut typed = page.clone();
        typed.fields[0].value = "Ann".into();
        let loaded = [Some(">Inside\n`[More`:/m] `<age`3>\n`{abcd:/page/deeper.mu}".to_string())];
        let mut again = parse_with(source, &loaded);
        again.keep_state(&typed);
        let out = text(&again.layout(40, None, &HashMap::new(), None));
        assert_eq!(out[1].trim_end(), "Inside");
        assert_eq!(out[2], format!("More 3{}", "_".repeat(23)));
        assert_eq!(again.partials.len(), 1, "the one inside isn't loaded");
        assert_eq!(again.fields.iter().map(|f| f.value.as_str()).collect::<Vec<_>>(), ["3", "Ann"], "what was typed stays");
        let html = again.to_html(|u| u.to_string(), |_| None);
        assert!(!html.contains("m-partial"));
        let html = page.to_html(|u| format!("x{u}"), |_| None);
        assert!(html.contains("<div class=\"m-partial\" style=\"padding-left:0ch\" data-url=\"xabcd:/page/p.mu\" data-fields=\"pid=32|name\" data-refresh=\"10\" data-pid=\"32\">"), "{html}");
        assert!(parse_partial("`{abcd:/x}\nhi").partials.is_empty());
    }

    #[test]
    fn text_fields_of_several_rows() {
        let mut page = parse("Notes: `<40x3|notes`first> after\n`<!8x2|secret`>");
        assert_eq!((page.fields[0].width, page.fields[0].rows), (40, 3));
        assert_eq!(page.fields[1].kind, FieldKind::Text { masked: true });
        page.fields[0].value = "line one\nline two is long enough to wrap around\nthree\nfour".into();
        let layout = page.layout(30, Some(0), &HashMap::new(), None);
        let out = text(&layout);
        assert_eq!(out[0], "Notes:");
        assert_eq!(out[1].trim_end(), "line one");
        assert_eq!(out[2].trim_end(), "line two is long enough to");
        assert_eq!(out[3].trim_end(), "wrap around…");
        assert_eq!(out[4], "after");
        // Every row of it is the field's, and it's selected.
        assert_eq!(hit_at(&layout.hits, 2, 5), Some(0));
        assert!(layout.lines[1].spans[1].style.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(page.request_fields(&["notes".into()])["field_notes"], page.fields[0].value);
        let html = page.to_html(|u| u.to_string(), |_| None);
        assert!(html.contains("<textarea class=\"m-field\" name=\"notes\" cols=\"40\" rows=\"3\""), "{html}");
        assert!(html.contains("type=\"password\" name=\"secret\""));
    }
}
