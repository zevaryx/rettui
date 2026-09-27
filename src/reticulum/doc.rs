//! Editing a Reticulum config file in place: values are changed, added or
//! removed line by line, so comments, ordering and the rest of the file stay
//! as they were. Reading values is left to rns-runtime's own parser.

/// What a line of the file is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// Blank, comment, or anything else.
    Other,
    /// `[name]` (depth 1), `[[name]]` (depth 2), ...
    Header { depth: usize, name: String },
    /// `key = value`, possibly continued (multi-line values) up to `last`.
    Key { key: String, last: usize },
}

/// The text before a `#` that is not inside quotes.
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '#') => return &line[..i],
            _ => {}
        }
    }
    line
}

fn unquote(text: &str) -> String {
    let text = text.trim();
    for q in ['"', '\''] {
        if text.len() >= 2 && text.starts_with(q) && text.ends_with(q) {
            return text[1..text.len() - 1].to_string();
        }
    }
    text.to_string()
}

fn scan(lines: &[String]) -> Vec<Kind> {
    let mut kinds = vec![Kind::Other; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        let line = strip_comment(&lines[i]).trim();
        if line.starts_with('[') && line.ends_with(']') {
            let depth = line.chars().take_while(|&c| c == '[').count();
            let close = line.chars().rev().take_while(|&c| c == ']').count();
            if depth == close && line.len() > depth * 2 {
                let name = unquote(&line[depth..line.len() - depth]);
                kinds[i] = Kind::Header { depth, name };
            }
        } else if let Some(eq) = line.find('=') {
            let key = unquote(&line[..eq]);
            let value = line[eq + 1..].trim();
            let mut last = i;
            for quote in ["\"\"\"", "'''"] {
                if let Some(rest) = value.strip_prefix(quote)
                    && !rest.contains(quote)
                {
                    while last + 1 < lines.len() {
                        last += 1;
                        if lines[last].contains(quote) {
                            break;
                        }
                    }
                }
            }
            kinds[i] = Kind::Key { key, last };
            i = last;
        }
        i += 1;
    }
    kinds
}

/// Write a value the way ConfigObj reads it back.
pub fn format_value(value: &str) -> String {
    let needs_quotes = value.contains(['#', ',', '\n', '"', '\''])
        || value != value.trim()
        || value.is_empty();
    if !needs_quotes {
        value.to_string()
    } else if value.contains('\n') || (value.contains('"') && value.contains('\'')) {
        // The closing quotes must not run into the value's own.
        if value.contains("'''") || value.ends_with('\'') {
            format!("\"\"\"{value}\"\"\"")
        } else {
            format!("'''{value}'''")
        }
    } else if value.contains('"') {
        format!("'{value}'")
    } else {
        format!("\"{value}\"")
    }
}

/// A list value, `a, b, c`.
pub fn format_list(items: &[String]) -> String {
    match items {
        [] => "\"\"".to_string(),
        // A trailing comma keeps a single item a list for ConfigObj.
        [one] => format!("{},", format_value(one)),
        _ => items.iter().map(|i| format_value(i)).collect::<Vec<_>>().join(", "),
    }
}

pub struct Doc {
    lines: Vec<String>,
    kinds: Vec<Kind>,
    trailing_newline: bool,
}

/// Where a section's lines are: its header, the end of its own keys (before
/// any subsection), and the end of everything under it.
struct Span {
    header: usize,
    own_end: usize,
    end: usize,
    depth: usize,
}

impl Doc {
    pub fn new(text: &str) -> Self {
        let trailing_newline = text.ends_with('\n') || text.is_empty();
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let kinds = scan(&lines);
        Self {
            lines,
            kinds,
            trailing_newline,
        }
    }

    pub fn text(&self) -> String {
        let mut text = self.lines.join("\n");
        if self.trailing_newline && !text.is_empty() {
            text.push('\n');
        }
        text
    }

    fn rescan(&mut self) {
        self.kinds = scan(&self.lines);
    }

    fn header(&self, i: usize) -> Option<(usize, &str)> {
        match &self.kinds[i] {
            Kind::Header { depth, name } => Some((*depth, name)),
            _ => None,
        }
    }

    /// The lines of the section at `path` (`["reticulum"]`,
    /// `["interfaces", "My TCP"]`).
    fn find(&self, path: &[&str]) -> Option<Span> {
        let mut current: Vec<String> = Vec::new();
        let mut found: Option<usize> = None;
        for i in 0..self.lines.len() {
            let Some((depth, name)) = self.header(i) else { continue };
            if let Some(header) = found {
                if depth <= path.len() {
                    return Some(self.span(header, i, path.len()));
                }
                continue;
            }
            current.truncate(depth - 1);
            if current.len() == depth - 1 {
                current.push(name.to_string());
            }
            if current.len() == path.len() && current.iter().zip(path).all(|(a, b)| a == b) {
                found = Some(i);
            }
        }
        found.map(|header| self.span(header, self.lines.len(), path.len()))
    }

    fn span(&self, header: usize, end: usize, depth: usize) -> Span {
        let own_end = (header + 1..end).find(|&i| self.header(i).is_some()).unwrap_or(end);
        Span {
            header,
            own_end,
            end,
            depth,
        }
    }

    /// The key lines directly in a section.
    fn keys(&self, span: &Span) -> impl Iterator<Item = (usize, &str, usize)> {
        (span.header + 1..span.own_end).filter_map(|i| match &self.kinds[i] {
            Kind::Key { key, last } => Some((i, key.as_str(), *last)),
            _ => None,
        })
    }

    /// Names of the subsections of a top-level section, in file order.
    pub fn subsections(&self, section: &str) -> Vec<String> {
        let Some(span) = self.find(&[section]) else { return Vec::new() };
        (span.header + 1..span.end)
            .filter_map(|i| match self.header(i) {
                Some((2, name)) => Some(name.to_string()),
                _ => None,
            })
            .collect()
    }

    pub fn has_section(&self, path: &[&str]) -> bool {
        self.find(path).is_some()
    }

    /// Which of `keys` is set in the section (the first one found).
    pub fn present_key(&self, path: &[&str], keys: &[&str]) -> Option<String> {
        let span = self.find(path)?;
        let found: Vec<&str> = self.keys(&span).map(|(_, k, _)| k).collect();
        keys.iter().find(|k| found.contains(k)).map(|k| k.to_string())
    }

    /// The raw value text of a key (before unquoting), for keeping its style.
    pub fn raw_value(&self, path: &[&str], key: &str) -> Option<String> {
        let span = self.find(path)?;
        let (i, ..) = self.keys(&span).find(|(_, k, _)| *k == key)?;
        let line = strip_comment(&self.lines[i]);
        line.find('=').map(|eq| line[eq + 1..].trim().to_string())
    }

    fn indent_of(&self, i: usize) -> String {
        let line = &self.lines[i];
        line[..line.len() - line.trim_start().len()].to_string()
    }

    /// Indentation for a new key: that of the section's keys, else that of
    /// keys in sections at the same depth.
    fn indent_for(&self, span: &Span) -> String {
        if let Some((i, ..)) = self.keys(span).next() {
            return self.indent_of(i);
        }
        let sibling = (0..self.lines.len())
            .filter(|&i| self.header(i).is_some_and(|(d, _)| d == span.depth))
            .find_map(|h| {
                let next = self.lines.get(h + 1).map(|_| h + 1)?;
                matches!(self.kinds[next], Kind::Key { .. }).then(|| self.indent_of(next))
            });
        sibling.unwrap_or_else(|| "    ".repeat(span.depth.saturating_sub(1)))
    }

    fn header_line(&self, depth: usize, name: &str) -> String {
        let indent = (0..self.lines.len())
            .find(|&i| self.header(i).is_some_and(|(d, _)| d == depth))
            .map_or_else(|| "  ".repeat(depth.saturating_sub(1)), |i| self.indent_of(i));
        format!("{indent}{}{name}{}", "[".repeat(depth), "]".repeat(depth))
    }

    /// Add a section (its parent must exist for subsections), at the end of
    /// its parent.
    fn add_section(&mut self, path: &[&str]) {
        let at = match path {
            [_] => self.lines.len(),
            _ => match self.find(&path[..path.len() - 1]) {
                Some(parent) => parent.end,
                None => {
                    self.add_section(&path[..path.len() - 1]);
                    self.lines.len()
                }
            },
        };
        // Go in after the parent's last line, before any blank lines.
        let mut at = at;
        while at > 0 && self.lines[at - 1].trim().is_empty() {
            at -= 1;
        }
        let mut new = Vec::new();
        if at > 0 {
            new.push(String::new());
        }
        new.push(self.header_line(path.len(), path[path.len() - 1]));
        if at < self.lines.len() && !self.lines[at].trim().is_empty() {
            new.push(String::new());
        }
        self.lines.splice(at..at, new);
        self.rescan();
    }

    /// Set `key` (or whichever of `aliases` is already there) in a section,
    /// creating the section if needed. `None` removes the key.
    pub fn set(&mut self, path: &[&str], key: &str, aliases: &[&str], value: Option<&str>) {
        if self.find(path).is_none() {
            if value.is_none() {
                return;
            }
            self.add_section(path);
        }
        let span = self.find(path).expect("section exists");
        let existing = self
            .keys(&span)
            .find(|(_, k, _)| *k == key || aliases.contains(k))
            .map(|(i, k, last)| (i, k.to_string(), last));
        match (existing, value) {
            (Some((i, found, last)), Some(value)) => {
                let line = &self.lines[i];
                let indent = &line[..line.len() - line.trim_start().len()];
                let comment = &line[strip_comment(line).len()..];
                let comment = if comment.is_empty() || last != i { String::new() } else { format!(" {comment}") };
                let replacement = format!("{indent}{found} = {value}{comment}");
                self.lines.splice(i..=last, [replacement]);
            }
            (Some((i, _, last)), None) => {
                self.lines.drain(i..=last);
            }
            (None, Some(value)) => {
                let indent = self.indent_for(&span);
                let after = self.keys(&span).last().map_or(span.header, |(_, _, last)| last);
                self.lines.insert(after + 1, format!("{indent}{key} = {value}"));
            }
            (None, None) => return,
        }
        self.rescan();
    }

    /// Add `[[name]]` under `section` with some starting keys.
    pub fn add_subsection(&mut self, section: &str, name: &str, keys: &[(&str, &str)]) -> Result<(), String> {
        validate_name(name)?;
        if self.find(&[section, name]).is_some() {
            return Err(format!("There is already a section named {name}"));
        }
        self.add_section(&[section, name]);
        for (key, value) in keys {
            self.set(&[section, name], key, &[], Some(value));
        }
        Ok(())
    }

    /// Remove a section with its keys and subsections. Comments after its
    /// last key are left, as they usually introduce what follows.
    pub fn remove(&mut self, path: &[&str]) -> Result<(), String> {
        let span = self.find(path).ok_or_else(|| format!("No section {}", path.join("/")))?;
        let last_key = (span.header..span.end)
            .rev()
            .find(|&i| !matches!(self.kinds[i], Kind::Other))
            .map_or(span.header, |i| match self.kinds[i] {
                Kind::Key { last, .. } => last,
                _ => i,
            });
        let mut end = last_key + 1;
        while end < span.end && self.lines[end].trim().is_empty() {
            end += 1;
        }
        self.lines.drain(span.header..end);
        self.rescan();
        Ok(())
    }

    pub fn rename(&mut self, path: &[&str], name: &str) -> Result<(), String> {
        validate_name(name)?;
        let mut target: Vec<&str> = path.to_vec();
        *target.last_mut().expect("a path") = name;
        if self.find(&target).is_some() {
            return Err(format!("There is already a section named {name}"));
        }
        let span = self.find(path).ok_or_else(|| format!("No section {}", path.join("/")))?;
        let line = &self.lines[span.header];
        let indent = &line[..line.len() - line.trim_start().len()];
        let depth = span.depth;
        self.lines[span.header] = format!("{indent}{}{name}{}", "[".repeat(depth), "]".repeat(depth));
        self.rescan();
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("A name is needed".into());
    }
    if name != name.trim() || name.contains(['[', ']', '#', '"', '\'', '\n']) {
        return Err("Names cannot contain brackets, quotes or #, or start or end with spaces".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Reticulum config
[reticulum]
  enable_transport = False   # not a router
  share_instance = Yes

[logging]
  loglevel = 4

[interfaces]
  # The local network
  [[Default Interface]]
    type = AutoInterface
    enabled = Yes

  # A remote hub
  [[Hub]]
    type = TCPClientInterface
    target_host = example.org
    target_port = 4242
";

    #[test]
    fn values_change_in_place_with_their_comments() {
        let mut doc = Doc::new(SAMPLE);
        doc.set(&["reticulum"], "enable_transport", &[], Some("True"));
        doc.set(&["interfaces", "Hub"], "target_port", &[], Some("4965"));
        let text = doc.text();
        assert!(text.contains("  enable_transport = True # not a router\n"), "{text}");
        assert!(text.contains("    target_port = 4965\n"));
        assert_eq!(text.lines().count(), SAMPLE.lines().count());
    }

    #[test]
    fn keys_are_added_to_the_right_section_and_removed() {
        let mut doc = Doc::new(SAMPLE);
        doc.set(&["interfaces", "Default Interface"], "group_id", &[], Some("home"));
        doc.set(&["reticulum"], "instance_name", &[], Some("my instance"));
        let text = doc.text();
        assert!(text.contains("    enabled = Yes\n    group_id = home\n\n  # A remote hub"), "{text}");
        assert!(text.contains("  share_instance = Yes\n  instance_name = my instance\n"));
        doc.set(&["interfaces", "Hub"], "target_host", &[], None);
        assert!(!doc.text().contains("example.org"));
        // Aliases are edited where they are.
        doc.set(&["interfaces", "Hub"], "mode", &["interface_mode"], Some("gateway"));
        doc.set(&["interfaces", "Hub"], "interface_mode", &["mode"], Some("boundary"));
        assert!(doc.text().contains("    mode = boundary\n"));
        let parsed = rns_runtime::config::Config::parse(&doc.text()).unwrap();
        assert_eq!(parsed.subsection("interfaces", "Hub").unwrap().get("mode"), Some("boundary"));
    }

    #[test]
    fn sections_are_added_renamed_and_removed() {
        let mut doc = Doc::new(SAMPLE);
        doc.add_subsection("interfaces", "UDP", &[("type", "UDPInterface"), ("listen_port", "4242")]).unwrap();
        assert!(doc.add_subsection("interfaces", "UDP", &[]).is_err());
        assert_eq!(doc.subsections("interfaces"), ["Default Interface", "Hub", "UDP"]);
        doc.rename(&["interfaces", "Hub"], "Remote hub").unwrap();
        doc.remove(&["interfaces", "Default Interface"]).unwrap();
        let text = doc.text();
        assert!(!text.contains("AutoInterface"));
        // The comment introducing the next section stays.
        assert!(text.contains("  # A remote hub\n  [[Remote hub]]\n"), "{text}");
        let parsed = rns_runtime::config::Config::parse(&text).unwrap();
        let names: Vec<&str> = parsed.subsections("interfaces").iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["Remote hub", "UDP"]);
        assert_eq!(parsed.subsection("interfaces", "UDP").unwrap().get("listen_port"), Some("4242"));
        // Missing sections are created.
        let mut empty = Doc::new("");
        empty.set(&["logging"], "loglevel", &[], Some("6"));
        empty.add_subsection("interfaces", "Auto", &[("type", "AutoInterface")]).unwrap();
        let parsed = rns_runtime::config::Config::parse(&empty.text()).unwrap();
        assert_eq!(parsed.section("logging").unwrap().get("loglevel"), Some("6"));
        assert!(parsed.subsection("interfaces", "Auto").is_some());
        assert!(doc.rename(&["interfaces", "UDP"], "bad]name").is_err());
    }

    #[test]
    fn values_are_quoted_when_needed() {
        assert_eq!(format_value("plain"), "plain");
        assert_eq!(format_value("a#b"), "\"a#b\"");
        assert_eq!(format_value("say \"hi\""), "'say \"hi\"'");
        assert_eq!(format_list(&["a".into(), "b c".into()]), "a, b c");
        assert_eq!(format_list(&["only".into()]), "only,");
        let mut doc = Doc::new("[reticulum]\n");
        for value in ["x, y # z", "say \"hi\"", "it's", "both ' and \""] {
            doc.set(&["reticulum"], "instance_name", &[], Some(&format_value(value)));
            let parsed = rns_runtime::config::Config::parse(&doc.text()).unwrap();
            assert_eq!(parsed.section("reticulum").unwrap().get("instance_name"), Some(value));
        }
    }

    #[test]
    fn multi_line_values_are_replaced_whole() {
        let mut doc = Doc::new("[a]\nk = \"\"\"one\ntwo\"\"\"\nother = 1\n");
        doc.set(&["a"], "k", &[], Some("single"));
        assert_eq!(doc.text(), "[a]\nk = single\nother = 1\n");
    }
}
