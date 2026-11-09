//! Writing Micron: text escaped so it shows as written, links, and source
//! code coloured by [`crate::highlight`], for the pages rettui makes itself.

use crate::highlight::{self, Lang, Token};

/// Text as Micron shows it literally: its backslashes and backticks escaped,
/// tabs as spaces and control characters as visible symbols.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | '`' => {
                out.push('\\');
                out.push(c);
            }
            '\t' => out.push_str("    "),
            c => out.push(super::source::visible(c)),
        }
    }
    out
}

/// A whole line of text, escaped, including what would mean something at
/// the start of a line (headings, comments, dividers, section ends).
pub fn line(text: &str) -> String {
    let escaped = escape(text);
    if escaped.starts_with(['#', '>', '<', '-']) { format!("\\{escaped}") } else { escaped }
}

/// A link label: Micron ends one at a backtick or `]`.
fn label(text: &str) -> String {
    escape(&text.replace(['`', ']'], "'"))
}

/// Whether a link variable's value can go in a link as it is: a link's
/// fields end at `]` and split at `|`, `` ` `` and `=`, and are trimmed.
fn plain_value(value: &str) -> bool {
    value.trim() == value && !value.contains(['`', '|', ']', '=', '\\']) && !value.chars().any(char::is_control)
}

/// A link to `path` (`:/page/x.mu`, or another node's) with page variables.
/// A value a link can't carry as it is goes as hex, in `<key>_x`, which
/// [`var`] reads back.
pub fn link(text: &str, path: &str, vars: &[(&str, &str)]) -> String {
    let mut out = format!("`[{}`{path}", label(text));
    for (i, (key, value)) in vars.iter().enumerate() {
        out.push(if i == 0 { '`' } else { '|' });
        if plain_value(value) {
            out.push_str(&format!("{key}={value}"));
        } else {
            out.push_str(&format!("{key}_x={}", hex::encode(value)));
        }
    }
    out.push(']');
    out
}

/// A page variable from a request: `var_<key>`, or its hex form from [`link`].
pub fn var<'a>(vars: &'a std::collections::BTreeMap<String, String>, key: &str) -> Option<std::borrow::Cow<'a, str>> {
    if let Some(value) = vars.get(&format!("var_{key}")) {
        return Some(value.as_str().into());
    }
    let bytes = hex::decode(vars.get(&format!("var_{key}_x"))?).ok()?;
    String::from_utf8(bytes).ok().map(Into::into)
}

/// The colour a kind of source is shown in (Micron's three hex digits), and
/// whether it's italic.
fn colour(token: Token) -> Option<(&'static str, bool)> {
    Some(match token {
        Token::Plain => return None,
        Token::Keyword => ("c8f", false),
        Token::Type => ("fc6", false),
        Token::Constant | Token::Number => ("f96", false),
        Token::String | Token::Inserted => ("9d7", false),
        Token::Comment => ("888", true),
        Token::Function => ("7bf", false),
        Token::Meta => ("e87", false),
        Token::Deleted => ("f77", false),
    })
}

/// `text` in a kind's colour.
fn coloured(token: Token, text: &str) -> String {
    match colour(token) {
        Some((rgb, true)) => format!("`F{rgb}`*{}`*`f", escape(text)),
        Some((rgb, false)) => format!("`F{rgb}{}`f", escape(text)),
        None => escape(text),
    }
}

/// Micron's own source, coloured as the page source view colours it.
fn micron_lines(text: &str) -> Vec<String> {
    use super::source::Token as M;
    super::source::tokenize(text)
        .into_iter()
        .map(|tokens| {
            tokens
                .into_iter()
                .map(|(token, text)| {
                    let token = match token {
                        M::Text => Token::Plain,
                        M::Comment => Token::Comment,
                        M::Directive | M::Tag => Token::Meta,
                        M::Structure => Token::Keyword,
                        M::Link => Token::Function,
                        M::Field => Token::Type,
                        M::Image | M::Escape => Token::Constant,
                        M::Literal => Token::String,
                    };
                    coloured(token, &text)
                })
                .collect()
        })
        .collect()
}

/// Source code as Micron lines, coloured for `lang` (as plain text when there
/// is none). Each line is escaped, but not for the start of a line: put
/// something before it, or use [`line`]'s escaping, as needed.
pub fn code(lang: Option<&Lang>, text: &str) -> Vec<String> {
    let Some(lang) = lang else {
        return text.split('\n').map(|l| escape(l.strip_suffix('\r').unwrap_or(l))).collect();
    };
    if std::ptr::eq(lang, &highlight::MICRON) {
        return micron_lines(text);
    }
    let lines: Vec<&str> = text.split('\n').collect();
    highlight::highlight(lang, text)
        .into_iter()
        .zip(lines)
        .map(|(spans, line)| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let mut out = String::new();
            let mut at = 0;
            for (token, range) in spans {
                // Ranges come in order and inside their line.
                let (Some(before), Some(piece)) = (line.get(at..range.start), line.get(range.clone())) else { continue };
                out.push_str(&escape(before));
                out.push_str(&coloured(token, piece));
                at = range.end;
            }
            out.push_str(&escape(line.get(at..).unwrap_or_default()));
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::nomad::micron;

    /// What a line of Micron shows.
    fn shown(source: &str) -> String {
        let page = micron::parse(source);
        page.lines
            .iter()
            .filter_map(|line| match line {
                micron::MLine::Text { spans, .. } => Some(spans.iter().map(|s| s.text.as_str()).collect::<String>()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn escaped_text_shows_as_written() {
        for text in ["a `!bold`! b", "back\\slash `` end", "# not a comment", "> not a heading", "- not a rule", "<not reset", "`="] {
            assert_eq!(shown(&line(text)), text, "{text}");
        }
        assert_eq!(escape("a\tb\u{1b}[2J"), "a    b\u{241b}[2J");
    }

    #[test]
    fn links_carry_values_they_cannot_hold_as_hex() {
        let made = link("a]b`c", ":/page/blob.mu", &[("g", "public"), ("path", "dir/we|ird]=name ")]);
        assert!(made.starts_with("`[a'b'c`:/page/blob.mu`g=public|path_x="), "{made}");
        let page = micron::parse(&made);
        let fields: BTreeMap<String, String> = page
            .items
            .iter()
            .find_map(|item| match item {
                micron::Interactive::Link { url, fields, .. } => {
                    assert_eq!(url, ":/page/blob.mu");
                    Some(page.request_fields(fields))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(var(&fields, "g").as_deref(), Some("public"));
        assert_eq!(var(&fields, "path").as_deref(), Some("dir/we|ird]=name "));
        assert_eq!(var(&fields, "nope"), None);
        let bad = BTreeMap::from([("var_p_x".to_string(), "zz".to_string())]);
        assert_eq!(var(&bad, "p"), None);
    }

    #[test]
    fn code_is_coloured_and_still_reads_the_same() {
        let rust = highlight::language("main.rs", "");
        let lines = code(rust, "fn main() { let s = \"`x`\\\\\"; } // hi\r\n\tok");
        assert!(lines[0].contains("`Fc8ffn`f") && lines[0].contains("`F888`*// hi`*`f"), "{}", lines[0]);
        assert_eq!(shown(&lines[0]), "fn main() { let s = \"`x`\\\\\"; } // hi");
        assert_eq!(shown(&lines[1]), "    ok");
        assert_eq!(code(None, "plain `text`"), ["plain \\`text\\`"]);
        let mu = code(highlight::language("index.mu", ""), ">Title\n`!bold`!");
        assert!(mu[0].starts_with("`Fc8f>") && mu[1].contains("`Fe87"), "{mu:?}");
        assert_eq!(shown(&format!(" {}", mu[1])), " `!bold`!");
    }
}
