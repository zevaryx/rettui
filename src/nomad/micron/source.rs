//! Micron source split into tokens, for the page source view. Follows the
//! same rules as the parser so the colours match what the markup does.

use super::image_body_len;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Text,
    /// `# ...` lines, which are not shown on the page.
    Comment,
    /// `#!c=`, `#!fg=` and other page directives.
    Directive,
    /// Line-start markup: headings (`>`), depth reset (`<`), dividers (`-`).
    Structure,
    /// Inline formatting such as `` `! ``, `` `F0f0 `` or `` `` ``.
    Tag,
    Link,
    Field,
    Image,
    /// `\` and the character it escapes.
    Escape,
    /// Text inside a `` `= `` literal block.
    Literal,
}

/// How a source character is shown: tabs as spaces, and control characters
/// as visible symbols so a page cannot send escape sequences to a terminal.
pub fn visible(ch: char) -> char {
    match ch {
        '\t' => ' ',
        '\0'..='\x1f' => char::from_u32(0x2400 + ch as u32).unwrap_or('\u{fffd}'),
        '\x7f' => '\u{2421}',
        '\u{80}'..='\u{9f}' => '\u{fffd}',
        _ => ch,
    }
}

/// Tokens for each source line (split on `\n`, without line endings).
pub fn tokenize(source: &str) -> Vec<Vec<(Token, String)>> {
    let mut literal = false;
    source
        .split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line == "`=" {
                literal = !literal;
                return vec![(Token::Tag, line.to_string())];
            }
            let whole = if literal {
                Some(Token::Literal)
            } else if line.starts_with("#!") {
                Some(Token::Directive)
            } else if line.starts_with('#') {
                Some(Token::Comment)
            } else {
                None
            };
            if let Some(token) = whole {
                return vec![(token, line.to_string())];
            }
            tokenize_line(line)
        })
        .collect()
}

fn tokenize_line(line: &str) -> Vec<(Token, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<(Token, String)> = Vec::new();
    let mut push = |token: Token, text: &[char]| {
        if text.is_empty() {
            return;
        }
        match out.last_mut() {
            Some((last, s)) if *last == token => s.extend(text),
            _ => out.push((token, text.iter().collect())),
        }
    };

    let mut i = 0;
    if chars.first() == Some(&'<') {
        push(Token::Structure, &chars[..1]);
        i = 1;
    }
    match chars.get(i) {
        Some('-') => {
            push(Token::Structure, &chars[i..]);
            return out;
        }
        Some('>') => {
            let level = chars[i..].iter().take_while(|&&c| c == '>').count();
            push(Token::Structure, &chars[i..i + level]);
            i += level;
        }
        _ => {}
    }

    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            push(Token::Escape, &chars[i..i + 2]);
            i += 2;
            continue;
        }
        if c != '`' || i + 1 >= chars.len() {
            push(Token::Text, &chars[i..i + 1]);
            i += 1;
            continue;
        }
        let rest = &chars[i + 2..];
        // Length of the tag after the backtick and its code character.
        let (token, len) = match chars[i + 1] {
            'F' | 'B' => (Token::Tag, if rest.first() == Some(&'T') { 7 } else { 3 }),
            '[' => match rest.iter().position(|&c| c == ']') {
                Some(end) => (Token::Link, end + 1),
                None => (Token::Link, rest.len()),
            },
            '<' => match rest.iter().position(|&c| c == '>') {
                Some(end) => (Token::Field, end + 1),
                None => (Token::Field, rest.len()),
            },
            '(' => match image_body_len(rest) {
                Some(end) => (Token::Image, end + 1),
                None => (Token::Image, rest.len()),
            },
            _ => (Token::Tag, 0),
        };
        let end = (i + 2 + len).min(chars.len());
        push(token, &chars[i..end]);
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &[(Token, String)]) -> Vec<(Token, &str)> {
        line.iter().map(|(t, s)| (*t, s.as_str())).collect()
    }

    #[test]
    fn tokens_follow_micron_rules() {
        let lines = tokenize("#!c=0\n# note\n>>Title `!bold`!\n-=\n`=\n`!raw\n`=\nsee `[Home`:/page/index.mu] \\`x `Ff00red`f");
        assert_eq!(kinds(&lines[0]), [(Token::Directive, "#!c=0")]);
        assert_eq!(kinds(&lines[1]), [(Token::Comment, "# note")]);
        assert_eq!(
            kinds(&lines[2]),
            [
                (Token::Structure, ">>"),
                (Token::Text, "Title "),
                (Token::Tag, "`!"),
                (Token::Text, "bold"),
                (Token::Tag, "`!"),
            ]
        );
        assert_eq!(kinds(&lines[3]), [(Token::Structure, "-=")]);
        assert_eq!(kinds(&lines[5]), [(Token::Literal, "`!raw")]);
        assert_eq!(
            kinds(&lines[7]),
            [
                (Token::Text, "see "),
                (Token::Link, "`[Home`:/page/index.mu]"),
                (Token::Text, " "),
                (Token::Escape, "\\`"),
                (Token::Text, "x "),
                (Token::Tag, "`Ff00"),
                (Token::Text, "red"),
                (Token::Tag, "`f"),
            ]
        );
    }

    #[test]
    fn every_character_is_kept() {
        let source = "<>`!a`[x`y\n`(alt (1)`:/m.png) `<24|f`v> ` `FT12\r\n";
        let rebuilt: Vec<String> = tokenize(source)
            .iter()
            .map(|l| l.iter().map(|(_, s)| s.as_str()).collect())
            .collect();
        assert_eq!(rebuilt.join("\n"), source.replace('\r', ""));
    }
}
