//! Micron pages as HTML, for the web UI.
//!
//! Everything taken from the page is escaped. Links have no `href`: their
//! target goes in `data-url` (and the fields they submit in `data-fields`)
//! for the web UI's script to follow, and images point wherever `media`
//! says, so a page cannot link or load anything by itself.

use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};

use super::source::{Token, tokenize, visible};
use super::{FieldKind, Interactive, MLine, MSpan, Page, Table};

/// Divider lines repeat their character; CSS clips the excess. Enough for
/// the widest panes: about 9,700 px at the page's font size, wider than a
/// browser window on an ultrawide or 8K screen.
const DIVIDER_CHARS: usize = 1200;

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(visible(c)),
        }
    }
    out
}

/// xterm's 256-colour palette.
fn indexed(n: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match n {
        0..16 => BASE[n as usize],
        16..232 => {
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            let n = n - 16;
            (level(n / 36), level(n / 6 % 6), level(n % 6))
        }
        _ => {
            let v = 8 + (n - 232) * 10;
            (v, v, v)
        }
    }
}

fn css_color(color: Color) -> Option<String> {
    let (r, g, b) = match color {
        Color::Reset => return None,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(n) => indexed(n),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::Gray => (204, 204, 204),
        Color::DarkGray => (118, 118, 118),
        Color::LightRed => (241, 76, 76),
        Color::LightGreen => (35, 209, 139),
        Color::LightYellow => (245, 245, 67),
        Color::LightBlue => (108, 182, 255),
        Color::LightMagenta => (214, 112, 214),
        Color::LightCyan => (41, 184, 219),
        Color::White => (255, 255, 255),
    };
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn css(style: Style) -> String {
    let mut out = String::new();
    if let Some(fg) = style.fg.and_then(css_color) {
        out.push_str(&format!("color:{fg};"));
    }
    if let Some(bg) = style.bg.and_then(css_color) {
        out.push_str(&format!("background:{bg};"));
    }
    if style.add_modifier.contains(Modifier::BOLD) {
        out.push_str("font-weight:bold;");
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        out.push_str("font-style:italic;");
    }
    if style.add_modifier.contains(Modifier::UNDERLINED) {
        out.push_str("text-decoration:underline;");
    }
    out
}

fn align_css(align: Alignment) -> &'static str {
    match align {
        Alignment::Left => "left",
        Alignment::Center => "center",
        Alignment::Right => "right",
    }
}

impl Page {
    /// The page as HTML. `link` turns a link's URL into the target the web UI
    /// follows (an absolute NomadNet address, `lxmf@…` or `rrc://…`; `#name`
    /// jumps to an anchor and `p:id` reloads partials); `media` turns an
    /// inline image's URL into its `src`, or `None` to show its alt text.
    ///
    /// Anchors are empty `m-anchor` spans (`data-anchor`), a collapsible
    /// heading is an `m-fold-head` (`data-fold`) followed by the `m-fold`
    /// holding its section, and a partial is an `m-partial` with its
    /// request in `data-url`, `data-fields`, `data-refresh` and `data-pid`
    /// for the script to load.
    pub fn to_html(&self, link: impl Fn(&str) -> String, media: impl Fn(&str) -> Option<String>) -> String {
        let mut out = format!("<div class=\"micron\" style=\"{}\">", css(self.base_style));
        // Open sections that fold: their heading's level.
        let mut folds: Vec<usize> = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            if let MLine::Text { section: Some(section), .. } = line {
                while folds.last().is_some_and(|level| section.level <= *level) {
                    folds.pop();
                    out.push_str("</div>");
                }
            }
            if let MLine::SectionEnd = line {
                for _ in folds.drain(..) {
                    out.push_str("</div>");
                }
            }
            for (name, _) in self.anchors.iter().filter(|(_, at)| *at == index) {
                out.push_str(&format!("<span class=\"m-anchor\" data-anchor=\"{}\"></span>", escape(name)));
            }
            match line {
                MLine::Text { indent, align, fill, spans, section } => {
                    let fill = fill.map(css).unwrap_or_default();
                    let fold = section.and_then(|s| s.fold);
                    let class = match (section, fold) {
                        (_, Some(_)) => "m-line m-heading m-fold-head",
                        (Some(_), None) => "m-line m-heading",
                        _ => "m-line",
                    };
                    let data = fold
                        .map(|f| {
                            format!(
                                " data-fold=\"{f}\" data-open=\"{}\" data-closed=\"{}\"",
                                escape(self.fold_mark(true)),
                                escape(self.fold_mark(false))
                            )
                        })
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "<div class=\"{class}\"{data} style=\"padding-left:{indent}ch;text-align:{};{fill}\">",
                        align_css(*align)
                    ));
                    out.push_str(&self.spans_html(spans, &link));
                    out.push_str("</div>");
                    if let (Some(fold), Some(section)) = (fold, section) {
                        let hidden = if self.folds[fold].open { "" } else { " hidden" };
                        out.push_str(&format!("<div class=\"m-fold{hidden}\" data-fold=\"{fold}\">"));
                        folds.push(section.level);
                    }
                }
                MLine::Divider { indent, ch, style } => {
                    let rule: String = std::iter::repeat_n(*ch, DIVIDER_CHARS).collect();
                    out.push_str(&format!(
                        "<div class=\"m-divider\" style=\"padding-left:{indent}ch;{}\">{}</div>",
                        css(*style),
                        escape(&rule)
                    ));
                }
                MLine::Image { indent, align, url, alt, width } => {
                    let place = format!("padding-left:{indent}ch;text-align:{}", align_css(*align));
                    match media(url) {
                        Some(src) => {
                            // The page's width when there is room, else the
                            // pane's (the stylesheet's 100%, which this replaces).
                            let size = width.map(|w| format!("max-width:min({w}ch,100%);")).unwrap_or_default();
                            out.push_str(&format!(
                                "<div class=\"m-image\" style=\"{place}\"><img src=\"{}\" alt=\"{}\" title=\"{}\" loading=\"lazy\" style=\"{size}\"></div>",
                                escape(&src),
                                escape(alt),
                                escape(alt),
                            ));
                        }
                        None => out.push_str(&format!("<div class=\"m-image\" style=\"{place}\">[image: {}]</div>", escape(alt))),
                    }
                }
                MLine::Table(table) => out.push_str(&self.table_html(table, &link)),
                MLine::FieldBlock { indent, item, style } => {
                    if let Interactive::Field(f) = self.items[*item] {
                        out.push_str(&format!(
                            "<div class=\"m-line\" style=\"padding-left:{indent}ch\">{}</div>",
                            self.field_html(f, &css(*style))
                        ));
                    }
                }
                MLine::Partial { indent, index } => {
                    let partial = &self.partials[*index];
                    out.push_str(&format!(
                        "<div class=\"m-partial\" style=\"padding-left:{indent}ch\" data-url=\"{}\" data-fields=\"{}\" data-refresh=\"{}\" data-pid=\"{}\"><span class=\"m-loading\">loading…</span></div>",
                        escape(&link(&partial.url)),
                        escape(&partial.fields.join("|")),
                        partial.refresh.unwrap_or(0),
                        escape(partial.pid.as_deref().unwrap_or_default()),
                    ));
                }
                MLine::SectionEnd => {}
            }
        }
        for _ in folds {
            out.push_str("</div>");
        }
        out.push_str("</div>");
        out
    }

    /// A run of spans: text, links (inert, see [`Page::to_html`]), fields and
    /// a fold's mark.
    fn spans_html(&self, spans: &[MSpan], link: &impl Fn(&str) -> String) -> String {
        let mut out = String::new();
        for span in spans {
            let style = css(span.style);
            match span.item.and_then(|i| self.items.get(i)) {
                Some(Interactive::Link { url, fields }) => out.push_str(&format!(
                    "<a class=\"m-link\" data-url=\"{}\" data-fields=\"{}\" style=\"{style}\">{}</a>",
                    escape(&link(url)),
                    escape(&fields.join("|")),
                    escape(&span.text),
                )),
                Some(Interactive::Field(f)) => out.push_str(&self.field_html(*f, &style)),
                Some(Interactive::Fold(f)) if span.text.is_empty() => {
                    out.push_str(&format!("<span class=\"m-fold-mark\">{} </span>", escape(self.fold_mark(self.folds[*f].open))));
                }
                _ => out.push_str(&format!("<span style=\"{style}\">{}</span>", escape(&span.text))),
            }
        }
        out
    }

    fn table_html(&self, table: &Table, link: &impl Fn(&str) -> String) -> String {
        let place = format!("padding-left:{}ch;text-align:{}", table.indent, align_css(table.align));
        let size = table.max_width.map(|w| format!("max-width:{w}ch")).unwrap_or_default();
        let mut out = format!("<div class=\"m-table-wrap\" style=\"{place}\"><table class=\"m-table\" style=\"{size}\">");
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0).max(table.columns.len());
        for (r, row) in table.rows.iter().enumerate() {
            let cell = if table.header && r == 0 { "th" } else { "td" };
            out.push_str("<tr>");
            for c in 0..columns {
                let align = align_css(table.columns.get(c).copied().unwrap_or(Alignment::Left));
                let spans = row.get(c).map(Vec::as_slice).unwrap_or_default();
                out.push_str(&format!("<{cell} style=\"text-align:{align}\">{}</{cell}>", self.spans_html(spans, link)));
            }
            out.push_str("</tr>");
        }
        out.push_str("</table></div>");
        out
    }

    fn field_html(&self, index: usize, style: &str) -> String {
        let field = &self.fields[index];
        let name = escape(&field.name);
        let value = escape(&field.value);
        let checked = if field.checked { " checked" } else { "" };
        match field.kind {
            FieldKind::Text { masked: false } if field.rows > 1 => format!(
                "<textarea class=\"m-field\" name=\"{name}\" cols=\"{}\" rows=\"{}\" style=\"{style}\">{value}</textarea>",
                field.width, field.rows,
            ),
            FieldKind::Text { masked } => format!(
                "<input class=\"m-field\" type=\"{}\" name=\"{name}\" size=\"{}\" value=\"{value}\" style=\"{style}\">",
                if masked { "password" } else { "text" },
                field.width,
            ),
            FieldKind::Checkbox | FieldKind::Radio => format!(
                "<label class=\"m-choice\" style=\"{style}\"><input type=\"{}\" name=\"{name}\" value=\"{value}\"{checked}> {}</label>",
                if field.kind == FieldKind::Checkbox { "checkbox" } else { "radio" },
                escape(&field.label),
            ),
        }
    }
}

/// Micron source with line numbers and the markup coloured by CSS class.
pub fn source_html(source: &str) -> String {
    let mut out = String::from("<div class=\"m-source\">");
    for (number, tokens) in tokenize(source).iter().enumerate() {
        out.push_str(&format!("<div class=\"s-line\"><span class=\"s-num\">{}</span><span class=\"s-text\">", number + 1));
        for (token, text) in tokens {
            let class = match token {
                Token::Text => "",
                Token::Comment => "t-comment",
                Token::Directive => "t-directive",
                Token::Structure => "t-structure",
                Token::Tag => "t-tag",
                Token::Link => "t-link",
                Token::Field => "t-field",
                Token::Image => "t-image",
                Token::Escape => "t-escape",
                Token::Literal => "t-literal",
            };
            let text = escape(&text.replace('\t', "    "));
            if class.is_empty() {
                out.push_str(&text);
            } else {
                out.push_str(&format!("<span class=\"{class}\">{text}</span>"));
            }
        }
        out.push_str("</span></div>");
    }
    out.push_str("</div>");
    out
}

#[cfg(test)]
mod tests {
    use super::super::parse;
    use super::*;

    #[test]
    fn page_markup_is_escaped_and_links_are_inert() {
        let page = parse(
            ">Title <b>\n`[Click \"><script>`:/page/x.mu`name|var=1]\nName: `<8|name`<v>\n`<?|agree|yes`I <agree>>\n`(Logo`:/media/l.png)",
        );
        let html = page.to_html(|url| format!("abc:{url}"), |url| Some(format!("/api/media?u={url}")));
        assert!(!html.contains("<script>") && !html.contains("<b>") && !html.contains("<v>"));
        assert!(!html.contains("href"));
        assert!(html.contains("&lt;b&gt;"));
        assert!(html.contains("data-url=\"abc::/page/x.mu\""));
        assert!(html.contains("data-fields=\"name|var=1\""));
        assert!(html.contains("<input class=\"m-field\" type=\"text\" name=\"name\" size=\"8\""));
        assert!(html.contains("type=\"checkbox\" name=\"agree\" value=\"yes\""));
        assert!(html.contains("src=\"/api/media?u=:/media/l.png\""));
    }

    #[test]
    fn image_widths_never_exceed_the_pane() {
        let page = parse("`(Logo`w=100`:/media/l.png)\n`(Any`:/media/l.png)");
        let html = page.to_html(|url| url.to_string(), |url| Some(url.to_string()));
        assert!(html.contains("style=\"max-width:min(100ch,100%);\""));
        // Without a width the stylesheet's 100% applies.
        assert!(html.contains("loading=\"lazy\" style=\"\""));
    }

    #[test]
    fn line_backgrounds_fill_their_rows() {
        let html = parse("`B000\n`c\nx\n`b").to_html(|url| url.to_string(), |_| None);
        // Only "x" is a row, and it takes the background across.
        assert_eq!(html.matches("m-line").count(), 1);
        assert!(html.contains("<div class=\"m-line\" style=\"padding-left:0ch;text-align:center;background:#000000;\">"));
    }

    #[test]
    fn control_characters_are_shown_not_sent() {
        let html = source_html("a\u{1b}[31mb\n`!x");
        assert!(!html.contains('\u{1b}'));
        assert!(html.contains("a\u{241b}[31mb"));
        assert!(html.contains("<span class=\"t-tag\">`!</span>"));
    }

    #[test]
    fn colours_become_css() {
        assert_eq!(css_color(Color::Rgb(1, 2, 255)).as_deref(), Some("#0102ff"));
        assert_eq!(css_color(Color::Indexed(196)).as_deref(), Some("#ff0000"));
        assert_eq!(css_color(Color::Reset), None);
    }
}
