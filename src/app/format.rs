//! The page editor's formatting actions (its ribbon and Alt-key shortcuts):
//! Micron markup put around the selection, on the selected lines, or at the
//! cursor. The web UI's editor has the same actions and keys.

use crate::term::textarea::TextArea;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Bold,
    Italic,
    Underline,
    /// Remove formatting from the selection, or reset it at the cursor.
    Normal,
    Foreground,
    Background,
    Left,
    Center,
    Right,
    Heading(u8),
    Divider,
    Literal,
    Comment,
    Link,
    Image,
    /// Add a picture or a file to the node, and show it or link to it.
    Upload,
    Field,
    Checkbox,
    Radio,
}

/// The ribbon's buttons, in groups.
pub const RIBBON: [&[Action]; 6] = [
    &[Action::Bold, Action::Italic, Action::Underline, Action::Normal],
    &[Action::Foreground, Action::Background],
    &[Action::Left, Action::Center, Action::Right],
    &[Action::Heading(1), Action::Heading(2), Action::Heading(3)],
    &[Action::Divider, Action::Literal, Action::Comment],
    &[Action::Link, Action::Image, Action::Upload, Action::Field, Action::Checkbox, Action::Radio],
];

impl Action {
    /// The Alt-key shortcut; each label contains it.
    pub fn key(self) -> char {
        match self {
            Action::Bold => 'b',
            Action::Italic => 'i',
            Action::Underline => 'u',
            Action::Normal => 'n',
            Action::Foreground => 'f',
            Action::Background => 'g',
            Action::Left => 'l',
            Action::Center => 'c',
            Action::Right => 'r',
            Action::Heading(level) => char::from(b'0' + level.clamp(1, 3)),
            Action::Divider => 'v',
            Action::Literal => 't',
            Action::Comment => 'o',
            Action::Link => 'k',
            Action::Image => 'm',
            Action::Upload => 'p',
            Action::Field => 'd',
            Action::Checkbox => 'h',
            Action::Radio => 'a',
        }
    }

    pub fn from_key(key: char) -> Option<Action> {
        let key = key.to_ascii_lowercase();
        RIBBON.iter().flat_map(|group| group.iter()).copied().find(|action| action.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::Bold => "Bold",
            Action::Italic => "Italic",
            Action::Underline => "Underline",
            Action::Normal => "Normal",
            Action::Foreground => "Fg",
            Action::Background => "Bg",
            Action::Left => "Left",
            Action::Center => "Centre",
            Action::Right => "Right",
            Action::Heading(1) => "H1",
            Action::Heading(2) => "H2",
            Action::Heading(_) => "H3",
            Action::Divider => "Divider",
            Action::Literal => "Literal",
            Action::Comment => "Comment",
            Action::Link => "Link",
            Action::Image => "Image",
            Action::Upload => "Upload",
            Action::Field => "Field",
            Action::Checkbox => "Check",
            Action::Radio => "Radio",
        }
    }

    /// A shorter label for narrow editors (still containing the key).
    pub fn short(self) -> &'static str {
        match self {
            Action::Bold => "B",
            Action::Italic => "I",
            Action::Underline => "U",
            Action::Normal => "N",
            Action::Left => "L",
            Action::Center => "C",
            Action::Right => "R",
            Action::Divider => "Div",
            Action::Literal => "Lit",
            Action::Comment => "Com",
            Action::Image => "Img",
            Action::Upload => "Up",
            Action::Field => "Fld",
            Action::Checkbox => "Chk",
            Action::Radio => "Rad",
            other => other.label(),
        }
    }

    /// The question asked before applying, for actions that need an answer.
    pub fn question(self) -> Option<&'static str> {
        Some(match self {
            Action::Foreground => "Text colour: 3 or 6 hex digits (f80, 00aaff)",
            Action::Background => "Background colour: 3 or 6 hex digits (224, 003366)",
            Action::Link => "Link to (a page like :/page/about.mu, node:/page/…, lxmf@…, rrc://…)",
            Action::Image => "Image address (like :/media/logo.png)",
            Action::Upload => "File to add to the node (its path): a picture is shown in the page, any other file linked to download",
            Action::Field => "Field name",
            Action::Checkbox => "Checkbox name",
            Action::Radio => "Radio group name",
            _ => return None,
        })
    }
}

/// Apply an action to the editor; `answer` is the reply to its question.
pub fn apply(area: &mut TextArea, action: Action, answer: &str) -> Result<(), String> {
    let answer = answer.trim();
    match action {
        Action::Bold => toggle(area, "`!"),
        Action::Italic => toggle(area, "`*"),
        Action::Underline => toggle(area, "`_"),
        Action::Normal => normal(area),
        Action::Foreground => wrap(area, &format!("`F{}", color(answer)?), "`f"),
        Action::Background => wrap(area, &format!("`B{}", color(answer)?), "`b"),
        Action::Left => align(area, 'a'),
        Action::Center => align(area, 'c'),
        Action::Right => align(area, 'r'),
        Action::Heading(level) => heading(area, level.clamp(1, 3) as usize),
        Action::Divider => divider(area),
        Action::Literal => literal(area),
        Action::Comment => comment(area),
        Action::Link => {
            if answer.is_empty() {
                return Err("A link needs an address".into());
            }
            let label = single_line(&area.selected_text()).unwrap_or_else(|| answer.to_string());
            area.replace_selection(&format!("`[{}`{answer}]", clean(&label)));
        }
        Action::Image => {
            if answer.is_empty() {
                return Err("An image needs an address".into());
            }
            let alt = single_line(&area.selected_text()).unwrap_or_else(|| "image".into());
            area.replace_selection(&format!("`({}`{answer})", clean(&alt)));
        }
        // The app adds the file to the node, then puts in its markup (see
        // `added`).
        Action::Upload => return Err("A file is added by the editor".into()),
        Action::Field | Action::Checkbox | Action::Radio => {
            let name = field_name(answer)?;
            let chosen = single_line(&area.selected_text()).map(|s| clean(&s));
            let markup = match action {
                Action::Field => format!("`<24|{name}`{}>", chosen.unwrap_or_default()),
                Action::Checkbox => format!("`<?|{name}|yes`{}>", chosen.unwrap_or(name.clone())),
                _ => {
                    let label = chosen.unwrap_or_else(|| "Option".into());
                    format!("`<^|{name}|{label}`{label}>")
                }
            };
            area.replace_selection(&markup);
        }
    }
    Ok(())
}

/// Markup for a file just added to the node, around the selection (its
/// text) or at the cursor: a picture shown (`address` like
/// `:/media/images/logo.jpg`), or a link to download a file (like
/// `:/file/guide.pdf`). Without a selection, the file's name.
pub fn added(area: &mut TextArea, address: &str, image: bool) {
    let name = address.rsplit('/').next().unwrap_or(address);
    let named = if image { name.rsplit_once('.').map_or(name, |(stem, _)| stem) } else { name };
    let label = clean(&single_line(&area.selected_text()).unwrap_or_else(|| named.to_string()));
    area.replace_selection(&if image { format!("`({label}`{address})") } else { format!("`[{label}`{address}]") });
}

/// The selection when it is on one line and not empty.
fn single_line(text: &str) -> Option<String> {
    (!text.is_empty() && !text.contains('\n')).then(|| text.to_string())
}

/// Text inside link, image and field markup can't hold its delimiters.
fn clean(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '`' | '[' | ']' | '(' | ')' | '<' | '>' | '|')).collect()
}

fn field_name(name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("Fields need a name".into());
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return Err("Field names use letters, digits, - and _".into());
    }
    Ok(name.to_string())
}

/// `f80` or `#f80` as Micron's `f80`; six digits as `T00aaff`.
fn color(text: &str) -> Result<String, String> {
    let hex = text.trim_start_matches('#').to_ascii_lowercase();
    match hex.len() {
        3 | 6 if hex.chars().all(|c| c.is_ascii_hexdigit()) => Ok(if hex.len() == 6 { format!("T{hex}") } else { hex }),
        _ => Err(format!("\"{text}\" is not a colour: use 3 or 6 hex digits, like f80")),
    }
}

/// Step a position `n` chars along its line.
fn shift((row, col): (usize, usize), n: isize) -> (usize, usize) {
    (row, col.saturating_add_signed(n))
}

/// Put `open` and `close` around the selection (keeping the text selected),
/// or insert both with the cursor between them.
fn wrap(area: &mut TextArea, open: &str, close: &str) {
    let open_len = open.chars().count() as isize;
    let close_len = close.chars().count() as isize;
    match area.selection() {
        Some((start, _)) => {
            let text = area.selected_text();
            area.replace_selection(&format!("{open}{text}{close}"));
            let end = shift(area.cursor(), -close_len);
            area.select(shift(start, open_len), end);
        }
        None => {
            area.replace_selection(&format!("{open}{close}"));
            let (row, col) = shift(area.cursor(), -close_len);
            area.set_cursor(row, col);
        }
    }
}

/// Bold, italic and underline switch on and off with the same tag: wrap the
/// selection, or unwrap it when it is already wrapped.
fn toggle(area: &mut TextArea, tag: &str) {
    let len = tag.chars().count();
    if let Some((start, end)) = area.selection() {
        let text = area.selected_text();
        let chars = text.chars().count();
        if chars >= 2 * len && text.starts_with(tag) && text.ends_with(tag) {
            let inner: String = text.chars().skip(len).take(chars - 2 * len).collect();
            area.replace_selection(&inner);
            area.select(start, area.cursor());
            return;
        }
        let lines = area.lines();
        let before: String = lines[start.0].chars().take(start.1).collect();
        let after: String = lines[end.0].chars().skip(end.1).collect();
        if before.ends_with(tag) && after.starts_with(tag) {
            let outer = shift(start, -(len as isize));
            area.select(outer, shift(end, len as isize));
            area.replace_selection(&text);
            area.select(outer, area.cursor());
            return;
        }
    }
    wrap(area, tag, tag);
}

/// Formatting tags in some Micron text: `!`, `*`, `_`, colours and resets.
fn strip_formatting(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let hex = |from: usize, n: usize| chars.get(from..from + n).is_some_and(|s| s.iter().all(char::is_ascii_hexdigit));
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`' {
            match chars.get(i + 1) {
                Some('!' | '*' | '_' | 'f' | 'b' | '`') => {
                    i += 2;
                    continue;
                }
                Some('F' | 'B') if chars.get(i + 2) == Some(&'T') && hex(i + 3, 6) => {
                    i += 9;
                    continue;
                }
                Some('F' | 'B') if hex(i + 2, 3) => {
                    i += 5;
                    continue;
                }
                _ => {}
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn normal(area: &mut TextArea) {
    match area.selection() {
        Some((start, _)) => {
            let plain = strip_formatting(&area.selected_text());
            area.replace_selection(&plain);
            area.select(start, area.cursor());
        }
        // A reset: formatting and alignment back to the page's defaults.
        None => area.replace_selection("``"),
    }
}

/// The lines an action on lines applies to: the selected ones (not the
/// last when the selection ends at its very start), or the cursor's.
fn line_range(area: &TextArea) -> (usize, usize) {
    match area.selection() {
        Some(((r0, _), (r1, 0))) if r1 > r0 => (r0, r1 - 1),
        Some(((r0, _), (r1, _))) => (r0, r1),
        None => (area.cursor().0, area.cursor().0),
    }
}

/// A heading's `>` marks and the rest of the line.
fn split_heading(line: &str) -> (&str, &str) {
    let marks = line.len() - line.trim_start_matches('>').len();
    line.split_at(marks)
}

/// A line's text without the alignment tags at its start.
fn without_alignment(mut text: &str) -> &str {
    while let Some(rest) = ["`c", "`l", "`r", "`a"].iter().find_map(|tag| text.strip_prefix(tag)) {
        text = rest;
    }
    text
}

fn starts_aligned(text: &str) -> bool {
    without_alignment(text).len() != text.len()
}

/// Align the lines: the tag goes at the start of the first, and the line
/// after goes back to the left (alignment carries on in Micron).
fn align(area: &mut TextArea, tag: char) {
    let (first, last) = line_range(area);
    let (row, col) = area.cursor();
    let lines = area.lines();
    let mut new: Vec<String> = (first..=last)
        .map(|i| {
            let (marks, rest) = split_heading(&lines[i]);
            let tagged = if i == first { format!("`{tag}") } else { String::new() };
            format!("{marks}{tagged}{}", without_alignment(rest))
        })
        .collect();
    let mut replace_to = last;
    if tag != 'a'
        && let Some(next) = lines.get(last + 1)
    {
        let (marks, rest) = split_heading(next);
        if !starts_aligned(rest) {
            new.push(format!("{marks}`a{rest}"));
            replace_to = last + 1;
        }
    }
    area.replace_lines(first, replace_to, new);
    area.set_cursor(row, col);
}

/// Make the lines headings of a level, or plain text when they already are.
fn heading(area: &mut TextArea, level: usize) {
    let (first, last) = line_range(area);
    let lines = area.lines();
    let already = (first..=last).all(|i| split_heading(&lines[i]).0.len() == level);
    let new = (first..=last)
        .map(|i| {
            let (_, rest) = split_heading(&lines[i]);
            if already { rest.to_string() } else { format!("{}{rest}", ">".repeat(level)) }
        })
        .collect();
    area.replace_lines(first, last, new);
    let row = area.cursor().0;
    area.set_cursor(row, usize::MAX);
}

/// A divider on the cursor's line if it is empty, or on a new line after it.
fn divider(area: &mut TextArea) {
    let row = area.cursor().0;
    let line = area.lines()[row].clone();
    if line.trim().is_empty() {
        area.replace_lines(row, row, vec!["-".into()]);
        area.set_cursor(row, 1);
    } else {
        area.replace_lines(row, row, vec![line, "-".into()]);
        area.set_cursor(row + 1, 1);
    }
}

/// Shown exactly as written: `` `= `` lines around the selected lines, or an
/// empty block to type into.
fn literal(area: &mut TextArea) {
    let (first, last) = line_range(area);
    let lines = area.lines();
    if area.selection().is_none() && lines[first].trim().is_empty() {
        area.replace_lines(first, first, vec!["`=".into(), String::new(), "`=".into()]);
        area.set_cursor(first + 1, 0);
        return;
    }
    let mut new = vec!["`=".to_string()];
    new.extend(lines[first..=last].iter().cloned());
    new.push("`=".into());
    area.replace_lines(first, last, new);
    area.set_cursor(last + 1, usize::MAX);
}

/// Comment the lines out (`#`), or back in when they all are.
fn comment(area: &mut TextArea) {
    let (first, last) = line_range(area);
    let lines = area.lines();
    let commented = |l: &str| l.starts_with('#') && !l.starts_with("#!");
    let all = (first..=last).all(|i| commented(&lines[i]));
    let new = (first..=last)
        .map(|i| {
            let line = &lines[i];
            if all { line.strip_prefix("# ").or_else(|| line.strip_prefix('#')).unwrap_or(line).to_string() } else { format!("# {line}") }
        })
        .collect();
    let (row, _) = area.cursor();
    area.replace_lines(first, last, new);
    area.set_cursor(row, usize::MAX);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_selection(text: &str, from: (usize, usize), to: (usize, usize)) -> TextArea {
        let mut area = TextArea::new(text);
        area.select(from, to);
        area
    }

    #[test]
    fn inline_formatting_wraps_and_unwraps() {
        let mut area = with_selection("say hello there", (0, 4), (0, 9));
        apply(&mut area, Action::Bold, "").unwrap();
        assert_eq!(area.text(), "say `!hello`! there");
        assert_eq!(area.selected_text(), "hello");
        // Again: the same selection is unwrapped.
        apply(&mut area, Action::Bold, "").unwrap();
        assert_eq!(area.text(), "say hello there");
        assert_eq!(area.selected_text(), "hello");
        // With the tags selected too.
        let mut area = with_selection("`*word`*", (0, 0), (0, 8));
        apply(&mut area, Action::Italic, "").unwrap();
        assert_eq!(area.text(), "word");
        // Nothing selected: a pair with the cursor inside, ready to type.
        let mut area = TextArea::new("ab");
        area.set_cursor(0, 1);
        apply(&mut area, Action::Underline, "").unwrap();
        assert_eq!(area.text(), "a`_`_b");
        assert_eq!(area.cursor(), (0, 3));
        // Undo takes it all back at once.
        area.handle(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('z'), crossterm::event::KeyModifiers::CONTROL));
        assert_eq!(area.text(), "ab");
    }

    #[test]
    fn colours_and_normal() {
        let mut area = with_selection("red text", (0, 0), (0, 3));
        apply(&mut area, Action::Foreground, "#F00").unwrap();
        assert_eq!(area.text(), "`Ff00red`f text");
        let mut area = with_selection("sea", (0, 0), (0, 3));
        apply(&mut area, Action::Background, "003366").unwrap();
        assert_eq!(area.text(), "`BT003366sea`b");
        assert!(apply(&mut area, Action::Foreground, "red").is_err());
        let mut area = with_selection("`!`Ff00bold red`f`! and `B123back`b``", (0, 0), (0, 99));
        apply(&mut area, Action::Normal, "").unwrap();
        assert_eq!(area.text(), "bold red and back");
    }

    #[test]
    fn lines_align_head_and_comment() {
        let mut area = with_selection("one\ntwo\nthree", (0, 1), (1, 1));
        apply(&mut area, Action::Center, "").unwrap();
        assert_eq!(area.text(), "`cone\ntwo\n`athree");
        // Aligning again replaces the tag rather than adding another.
        let mut area = with_selection("`cone\ntwo\n`athree", (0, 0), (1, 1));
        apply(&mut area, Action::Right, "").unwrap();
        assert_eq!(area.text(), "`rone\ntwo\n`athree");
        // Headings keep their marks first, and toggle.
        let mut area = TextArea::new(">Title\nbody");
        apply(&mut area, Action::Center, "").unwrap();
        assert_eq!(area.text(), ">`cTitle\n`abody");
        let mut area = TextArea::new("Title");
        apply(&mut area, Action::Heading(2), "").unwrap();
        assert_eq!(area.text(), ">>Title");
        apply(&mut area, Action::Heading(2), "").unwrap();
        assert_eq!(area.text(), "Title");
        let mut area = with_selection("a\nb\n#!c=0", (0, 0), (2, 0));
        apply(&mut area, Action::Comment, "").unwrap();
        assert_eq!(area.text(), "# a\n# b\n#!c=0");
        let mut area = with_selection("# a\n# b", (0, 0), (1, 1));
        apply(&mut area, Action::Comment, "").unwrap();
        assert_eq!(area.text(), "a\nb");
    }

    #[test]
    fn blocks_and_inserts() {
        let mut area = TextArea::new("text");
        apply(&mut area, Action::Divider, "").unwrap();
        assert_eq!(area.text(), "text\n-");
        let mut area = TextArea::new("");
        apply(&mut area, Action::Literal, "").unwrap();
        assert_eq!(area.text(), "`=\n\n`=");
        assert_eq!(area.cursor(), (1, 0));
        let mut area = with_selection("see about", (0, 4), (0, 9));
        apply(&mut area, Action::Link, ":/page/about.mu").unwrap();
        assert_eq!(area.text(), "see `[about`:/page/about.mu]");
        let mut area = TextArea::new("");
        apply(&mut area, Action::Image, ":/media/logo.png").unwrap();
        assert_eq!(area.text(), "`(image`:/media/logo.png)");
        let mut area = TextArea::new("");
        apply(&mut area, Action::Field, "name").unwrap();
        assert_eq!(area.text(), "`<24|name`>");
        let mut area = with_selection("Subscribe", (0, 0), (0, 9));
        apply(&mut area, Action::Checkbox, "sub").unwrap();
        assert_eq!(area.text(), "`<?|sub|yes`Subscribe>");
        let mut area = with_selection("Blue", (0, 0), (0, 4));
        apply(&mut area, Action::Radio, "colour").unwrap();
        assert_eq!(area.text(), "`<^|colour|Blue`Blue>");
        assert!(apply(&mut area, Action::Field, "no spaces").is_err());
        assert!(apply(&mut area, Action::Link, "").is_err());
        // Every key is unique, and in its labels.
        let all: Vec<Action> = RIBBON.iter().flat_map(|g| g.iter()).copied().collect();
        for action in &all {
            assert_eq!(Action::from_key(action.key()), Some(*action));
            assert!(action.label().to_ascii_lowercase().contains(action.key()), "{action:?}");
            assert!(action.short().to_ascii_lowercase().contains(action.key()), "{action:?}");
        }
    }
}
