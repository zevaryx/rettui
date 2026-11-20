//! The `/logs` answer of the stats server: the tail of `rettui.log` as JSON,
//! for a status page, behind the stats token. The file is untrusted text (peers
//! and the libraries put what they like in log lines), so it is read in a
//! bounded window, decoded leniently, redacted and capped before it is sent.
//!
//! A line is as `tracing_subscriber`'s default format writes it, without colour:
//! `2025-01-31T12:34:56.123456Z  INFO rettui::net: text`: a timestamp, the level
//! in capitals (padded on the left to 5), then the rest, which is the text.
//! A line that doesn't start like that (a continuation, such as the second line of a message) has a null
//! time and level, and is shown if the last line that did parse is shown. Before the first line that parses, continuations are
//! hidden, as the tail window may start in the middle of an entry. rettui now
//! writes each entry on one line (see `crate::logging`), so such lines come from
//! older logs.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::logging::LEVELS;

/// The most lines one answer has, and how many it has if not asked.
pub const LINES_MAX: usize = 500;
pub const LINES_DEFAULT: usize = 200;
/// The most of the end of the file that is looked at.
const SCAN_MAX: u64 = 1024 * 1024;
/// The longest line sent, in characters.
const LINE_MAX: usize = 1000;
/// The most of a line that is redacted: a bit more than is sent, so a token that
/// ends up inside the part sent is whole in it, and a hostile line costs no more than this.
const REDACT_MAX: usize = LINE_MAX + 2 * super::TOKEN_MAX;
/// The index in [`LEVELS`] of the most detailed level that is served (`debug`).
const LEVEL_MAX: usize = 3;
/// The most bytes of lines in one answer.
const RESPONSE_MAX: usize = 256 * 1024;
/// What stands for what was removed.
const REDACTED: &str = "[redacted]";
/// The most of the web UI's login token file that is read.
const WEB_TOKEN_MAX: u64 = 1024;

/// What a request asks for: how many lines, and the least severe level shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Query {
    pub lines: usize,
    /// An index into [`LEVELS`]: this one and the ones before it are shown.
    pub level: usize,
}

impl Default for Query {
    fn default() -> Self {
        Self { lines: LINES_DEFAULT, level: 2 }
    }
}

impl Query {
    /// The query string's `lines` and `level`; other keys (such as a `token`, which is never read from here) are ignored.
    /// The error is the plain message for a 400.
    pub fn parse(query: Option<&str>) -> Result<Self, &'static str> {
        let mut parsed = Self::default();
        for pair in query.unwrap_or_default().split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match key {
                "lines" => {
                    parsed.lines = value
                        .parse::<usize>()
                        .ok()
                        .filter(|n| (1..=LINES_MAX).contains(n))
                        .ok_or("lines must be a number from 1 to 500\n")?;
                }
                "level" => {
                    parsed.level = LEVELS
                        .iter()
                        .position(|name| name.eq_ignore_ascii_case(value))
                        .ok_or("level must be one of error, warn, info, debug\n")?;
                    if parsed.level > LEVEL_MAX {
                        return Err("trace is not served; level must be one of error, warn, info, debug\n");
                    }
                }
                _ => {}
            }
        }
        Ok(parsed)
    }
}

/// What the answer is made from.
#[derive(Clone)]
pub struct Source {
    /// `rettui.log`. None reads as a missing file.
    pub path: Option<PathBuf>,
    /// The stats token, to take out of what is sent.
    pub secret: String,
    /// The web UI's login token file, when the web UI runs: its token is taken out too. Read for each
    /// answer, so a new token (after "Sign out the others") is taken out as well.
    pub web_token: Option<PathBuf>,
    /// The home directory, which becomes `~`.
    pub home: Option<String>,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("path", &self.path)
            .field("web_token", &self.web_token)
            .field("home", &self.home)
            .finish_non_exhaustive()
    }
}

impl Source {
    pub fn new(path: Option<PathBuf>, secret: &str, web_token: Option<PathBuf>) -> Self {
        let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_string_lossy().into_owned());
        Self { path, secret: secret.to_string(), web_token, home }
    }

    /// The JSON answer. Reads the file, so call it where blocking is fine.
    pub fn answer(&self, query: Query) -> String {
        self.answer_within(query, SCAN_MAX, RESPONSE_MAX)
    }

    fn answer_within(&self, query: Query, scan_max: u64, response_max: usize) -> String {
        let (bytes, window_cut) = self.path.as_deref().map(|path| read_tail(path, scan_max)).unwrap_or_default();
        let web_token = self.web_token.as_deref().and_then(read_secret);
        let secrets = [Some(self.secret.as_str()), web_token.as_deref()];
        let secrets: Vec<&str> = secrets.into_iter().flatten().collect();
        let (mut clipped, mut visible) = (false, false);
        let mut found: Vec<(Option<String>, Option<usize>, String)> = Vec::new();
        let mut raws = bytes.split(|&b| b == b'\n').peekable();
        while let Some(raw) = raws.next() {
            // Bytes after the last line break are a line still being written: like a cut one, it can
            // end in part of a secret.
            let unfinished = raws.peek().is_none();
            let line = String::from_utf8_lossy(raw);
            let line = line.strip_suffix('\r').unwrap_or(&line);
            if line.is_empty() {
                continue;
            }
            // Cut before redacting, so a huge line costs no more than a long one.
            let (line, cut) = cut_chars(line, REDACT_MAX);
            let cut = cut || unfinished;
            let line = redact(line, &secrets, self.home.as_deref(), cut);
            let (time, level, text) = match parse_line(&line) {
                Some((time, level, text)) => {
                    visible = level <= query.level;
                    (Some(time), Some(level), text)
                }
                None => (None, None, line.as_str()),
            };
            if !visible {
                continue;
            }
            let (text, text_cut) = cut_chars(text, LINE_MAX);
            clipped |= cut || text_cut;
            let text = text.to_string();
            found.push((time.map(str::to_string), level, text));
        }
        // From the newest back: what fits in the budget, up to the count asked for.
        let (mut used, mut over) = (0usize, false);
        let mut chosen = Vec::new();
        for (time, level, text) in found.into_iter().rev().take(query.lines) {
            let entry = json!({ "time": time, "level": level.map(|i| LEVELS[i]), "text": text });
            let size = entry.to_string().len() + 1;
            if used.saturating_add(size) > response_max {
                over = true;
                break;
            }
            used += size;
            chosen.push(entry);
        }
        chosen.reverse();
        // A cut window that gave fewer lines than asked for means older ones were not looked at.
        let short = window_cut && chosen.len() < query.lines;
        json!({
            "level": LEVELS[query.level],
            "truncated": clipped || over || short,
            "lines": Value::Array(chosen),
        })
        .to_string()
    }
}

/// The last `max` bytes of the file, from the start of a whole line, and whether older bytes were left out.
/// A file that can't be read is empty.
fn read_tail(path: &Path, max: u64) -> (Vec<u8>, bool) {
    let read = || -> std::io::Result<(Vec<u8>, bool)> {
        let mut file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        let start = len.saturating_sub(max);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(max).read_to_end(&mut bytes)?;
        if start > 0 {
            // The window starts inside a line: drop that piece.
            let from = bytes.iter().position(|&b| b == b'\n').map_or(bytes.len(), |i| i + 1);
            bytes.drain(..from);
        }
        Ok((bytes, start > 0))
    };
    read().unwrap_or_default()
}

/// The web UI's login token, from its file, if there is one that is long enough to be one.
fn read_secret(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(WEB_TOKEN_MAX).read_to_end(&mut bytes).ok()?;
    // A file that fills the cap is not a token (the web UI refuses one that long); one that isn't text is no token either.
    if bytes.len() as u64 >= WEB_TOKEN_MAX {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?.trim();
    (text.len() >= super::TOKEN_MIN).then(|| text.to_string())
}

/// The time, the level (an index into [`LEVELS`]) and the text of a line, if it starts as the logger writes one.
fn parse_line(line: &str) -> Option<(&str, usize, &str)> {
    let (time, rest) = line.split_once(' ')?;
    let looks_like_time = (10..=40).contains(&time.len())
        && time.starts_with(|c: char| c.is_ascii_digit())
        && time.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'-' | b':' | b'.' | b'T' | b'Z' | b'+'));
    if !looks_like_time {
        return None;
    }
    let rest = rest.trim_start_matches(' ');
    let (word, text) = rest.split_once(' ').unwrap_or((rest, ""));
    let level = LEVELS.iter().position(|name| name.eq_ignore_ascii_case(word))?;
    // The level is written in capitals only.
    word.bytes().all(|b| b.is_ascii_uppercase()).then_some((time, level, text))
}

/// The first `max` characters of `text`, and whether there was more.
fn cut_chars(text: &str, max: usize) -> (&str, bool) {
    match text.char_indices().nth(max) {
        Some((at, _)) => (&text[..at], true),
        None => (text, false),
    }
}

/// `text` without a piece at its end that starts the token: a cut line can end halfway through one.
fn drop_partial_tail<'a>(text: &'a str, secret: &str) -> &'a str {
    (1..secret.len()).rev().filter(|&n| secret.is_char_boundary(n)).find_map(|n| text.strip_suffix(&secret[..n])).unwrap_or(text)
}

/// `line` without the secrets, URL queries and passwords, what follows a word such as `authorization` or `token`,
/// or the home directory. `cut` is whether the line was cut at its end (or is unfinished), which can leave part
/// of a secret there.
fn redact(line: &str, secrets: &[&str], home: Option<&str>, cut: bool) -> String {
    let mut text = line.to_string();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        // As it is written, and as a debug print writes it (`\"` for a quote, and so on).
        let escaped = secret.escape_debug().to_string();
        for form in [*secret, escaped.as_str()] {
            // Whole tokens first: a token at the end whose last characters equal its first would otherwise be
            // taken for a partial one and only partly stripped.
            text = text.replace(form, REDACTED);
            if cut {
                text.truncate(drop_partial_tail(&text, form).len());
            }
            if escaped == *secret {
                break;
            }
        }
    }
    if let Some(home) = home.filter(|home| home.len() > 1) {
        text = text.replace(home, "~");
        let slashed = home.replace('\\', "/");
        if slashed != home {
            text = text.replace(&slashed, "~");
        }
    }
    redact_after_words(&redact_urls(&text))
}

/// Whether `c` ends a URL or a path in a line.
fn ends_url(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>')
}

/// Each URL or path (`https://host/x?key=...`, or `/x?key=...` in `GET /x?key=... HTTP/1.1`) with its query,
/// up to a space or a quote, replaced by `?[redacted]`, and without a `user:password@` before its host.
/// Libraries log the URLs they fetch, and a query can hold a key (a map tile service's, say).
fn redact_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.find(ends_url).unwrap_or(rest.len());
        let (word, after) = rest.split_at(end);
        // A `?` after a `/`, with something after it, starts a query.
        let query = word.find('/').and_then(|slash| word[slash..].find('?').map(|q| slash + q)).filter(|&q| q + 1 < word.len());
        let mut kept = query.map_or(word, |q| &word[..=q]);
        while let Some(scheme) = kept.find("://") {
            out.push_str(&kept[..scheme + 3]);
            kept = &kept[scheme + 3..];
            let host_end = kept.find(['/', '?', '#']).unwrap_or(kept.len());
            if let Some(at) = kept[..host_end].rfind('@') {
                kept = &kept[at + 1..];
            }
        }
        out.push_str(kept);
        if query.is_some() {
            out.push_str(REDACTED);
        }
        let delimiter = after.chars().next().map_or(0, char::len_utf8);
        out.push_str(&after[..delimiter]);
        rest = &after[delimiter..];
    }
    out
}

/// Words after which the rest of a line is taken out: header names (`Authorization`, `Proxy-Authorization`,
/// `X-Stats-Token`, `Cookie`, `Set-Cookie`), what keys are called (`api_key`, `token=`), and `Bearer`.
const SECRET_WORDS: [&str; 10] =
    ["authorization", "token", "cookie", "api-key", "api_key", "apikey", "secret", "password", "passwd", "bearer"];

/// `text` with everything after the first of the [`SECRET_WORDS`] (and the `:`, `=`, quotes or
/// spaces after it) replaced by `[redacted]`. However the value is printed (`name: value`, `("name", "value")`,
/// `name => Some("value")`, quoted or not), it goes. Words are found in any case, through characters that
/// draw nothing and through `%`-encoding (`authorization%3A%20...`). Taking the whole rest of the line
/// takes more than needed at times, but a value can't slip past where it ends.
fn redact_after_words(text: &str) -> String {
    // A copy to search, lower case, without control or invisible characters and with `%XX` decoded; `from` is
    // where each of its bytes came from in `text`. Other characters are a NUL each, which no word holds.
    let (mut folded, mut from) = (String::with_capacity(text.len()), Vec::with_capacity(text.len()));
    let bytes = text.as_bytes();
    let mut chars = text.char_indices();
    while let Some((at, c)) = chars.next() {
        let decoded = (c == '%')
            .then(|| bytes.get(at + 1..at + 3))
            .flatten()
            .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        let c = match decoded {
            Some(byte) => {
                // The two digits are ASCII: skipping them stays on character boundaries.
                chars.nth(1);
                char::from(byte)
            }
            None => c,
        };
        if c.is_control() || super::is_invisible(c) {
            continue;
        }
        folded.push(if c.is_ascii() { c.to_ascii_lowercase() } else { '\0' });
        from.push(at);
    }
    let Some((start, len)) = SECRET_WORDS.iter().filter_map(|word| folded.find(word).map(|at| (at, word.len()))).min() else {
        return text.to_string();
    };
    let word_end = from.get(start + len).copied().unwrap_or(text.len());
    let value = text[word_end..]
        .char_indices()
        .find(|&(_, c)| {
            !(c.is_whitespace()
                || c.is_control()
                || super::is_invisible(c)
                || matches!(c, ':' | '=' | '>' | '"' | '\'' | '\\' | ',' | '(' | '[' | '{'))
        })
        .map(|(at, _)| word_end + at);
    match value {
        Some(value) => format!("{}{REDACTED}", &text[..value]),
        // Only the word, at the end: nothing to take out.
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-stats-logs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn line(level: &str, text: &str) -> String {
        format!("2025-01-31T12:34:56.123456Z {level:>5} rettui::net: {text}\n")
    }

    fn answer_of(content: &[u8], query: &str, secret: &str, home: Option<&str>) -> Value {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = folder(&format!("a{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let path = dir.join("rettui.log");
        std::fs::write(&path, content).unwrap();
        let source = Source { path: Some(path), secret: secret.into(), web_token: None, home: home.map(String::from) };
        let value = serde_json::from_str(&source.answer(Query::parse(Some(query)).unwrap())).unwrap();
        let _ = std::fs::remove_dir_all(dir);
        value
    }

    fn texts(value: &Value) -> Vec<&str> {
        value["lines"].as_array().unwrap().iter().map(|l| l["text"].as_str().unwrap()).collect()
    }

    #[test]
    fn the_query_has_defaults_and_refuses_bad_values() {
        assert_eq!(Query::parse(None), Ok(Query { lines: 200, level: 2 }));
        assert_eq!(Query::parse(Some("lines=1&level=error")), Ok(Query { lines: 1, level: 0 }));
        assert_eq!(Query::parse(Some("lines=500&level=DEBUG")), Ok(Query { lines: 500, level: 3 }));
        assert_eq!(Query::parse(Some("token=abc&other=1&&")), Ok(Query::default()));
        for bad in ["lines=0", "lines=501", "lines=-1", "lines=", "lines=x", "lines=1.5", "lines=99999999999999999999999"] {
            assert!(Query::parse(Some(bad)).unwrap_err().starts_with("lines must"), "{bad}");
        }
        let trace = Query::parse(Some("level=trace")).unwrap_err();
        assert!(trace.starts_with("trace is not served"), "{trace}");
        assert!(Query::parse(Some("lines=5&level=TRACE")).is_err());
        for bad in ["level=", "level=fatal", "level=%65rror", "level=info,warn"] {
            assert!(Query::parse(Some(bad)).unwrap_err().starts_with("level must"), "{bad}");
        }
    }

    #[test]
    fn a_line_is_read_as_the_logger_writes_it() {
        let parsed = parse_line("2025-01-31T12:34:56.123456Z  INFO rettui::net: peer up x=1");
        assert_eq!(parsed, Some(("2025-01-31T12:34:56.123456Z", 2, "rettui::net: peer up x=1")));
        assert_eq!(parse_line("2025-01-31T12:34:56Z ERROR boom").map(|p| p.1), Some(0));
        assert_eq!(parse_line("2025-01-31T12:34:56Z TRACE").map(|p| (p.1, p.2)), Some((4, "")));
        for not in
            ["", "  at src/main.rs:1", "INFO x", "2025-01-31T12:34:56Z info x", "2025-01-31T12:34:56Z LOUD x", "abc WARN x", "12 WARN x"]
        {
            assert_eq!(parse_line(not), None, "{not:?}");
        }
    }

    #[test]
    fn the_level_is_a_minimum_and_lines_are_oldest_first() {
        let log = [line("TRACE", "t"), line("DEBUG", "d"), line("INFO", "i"), line("WARN", "w"), line("ERROR", "e")].concat();
        let by_default = answer_of(log.as_bytes(), "", "unused-secret-value", None);
        assert_eq!(by_default["level"], "info");
        assert_eq!(by_default["truncated"], false);
        assert_eq!(texts(&by_default), ["rettui::net: i", "rettui::net: w", "rettui::net: e"]);
        let first = &by_default["lines"][0];
        assert_eq!((first["time"].as_str(), first["level"].as_str()), (Some("2025-01-31T12:34:56.123456Z"), Some("info")));
        assert_eq!(texts(&answer_of(log.as_bytes(), "level=error", "unused-secret-value", None)), ["rettui::net: e"]);
        assert_eq!(texts(&answer_of(log.as_bytes(), "level=debug", "unused-secret-value", None)).len(), 4);
        let last_two = answer_of(log.as_bytes(), "level=debug&lines=2", "unused-secret-value", None);
        assert_eq!(texts(&last_two), ["rettui::net: w", "rettui::net: e"]);
        assert_eq!(last_two["truncated"], false, "the count asked for is not a cut");
    }

    #[test]
    fn a_continuation_follows_the_line_before_it() {
        let log = format!(
            "  stray start\n{}second line of a warning\n{}more\n{}after an info\r\n",
            line("WARN", "w"),
            line("INFO", "i"),
            line("DEBUG", "d")
        );
        let warn_only = answer_of(log.as_bytes(), "level=warn", "unused-secret-value", None);
        assert_eq!(texts(&warn_only), ["rettui::net: w", "second line of a warning"]);
        assert_eq!(warn_only["lines"][1]["level"], Value::Null);
        assert_eq!(warn_only["lines"][1]["time"], Value::Null);
        let all = answer_of(log.as_bytes(), "level=debug", "unused-secret-value", None);
        assert_eq!(texts(&all).len(), 6, "the stray one at the start is hidden: {all}");
        assert!(texts(&all).iter().all(|t| !t.contains("stray")));
        assert_eq!(*texts(&all).last().unwrap(), "after an info");
    }

    #[test]
    fn a_missing_file_or_none_is_an_empty_list() {
        let dir = folder("missing");
        let source = Source { path: Some(dir.join("nope.log")), secret: "s".into(), web_token: None, home: None };
        let value: Value = serde_json::from_str(&source.answer(Query::default())).unwrap();
        assert_eq!(value, json!({ "level": "info", "truncated": false, "lines": [] }));
        let none = Source { path: None, ..source.clone() };
        assert_eq!(serde_json::from_str::<Value>(&none.answer(Query::default())).unwrap(), value);
        // A folder where the file should be.
        let folder_instead = Source { path: Some(dir.clone()), ..source };
        assert_eq!(serde_json::from_str::<Value>(&folder_instead.answer(Query::default())).unwrap(), value);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn garbage_is_read_without_a_panic() {
        let mut bytes = line("INFO", "good").into_bytes();
        bytes.extend_from_slice(b"2025-01-31T12:34:56Z  WARN bad \xff\xfe\xc3\x28 utf8\n\0\0\0\n\n\r\n");
        bytes.extend((0..=255u8).cycle().take(5000));
        bytes.extend_from_slice(b"\n2025-01-31T12:34:56Z ERROR \xe2\x82");
        let value = answer_of(&bytes, "level=debug", "unused-secret-value", Some("/home/x"));
        let found = texts(&value);
        assert!(found.contains(&"rettui::net: good") && found.iter().any(|t| t.contains("bad \u{fffd}")), "{found:?}");
        assert!(value["lines"].as_array().unwrap().iter().any(|l| l["level"] == "error"));
        // Only the file's own bytes, and nothing else, so an empty file works too.
        assert_eq!(answer_of(b"", "", "unused-secret-value", None)["lines"], json!([]));
    }

    #[test]
    fn the_token_headers_and_home_are_taken_out() {
        let secret = "correct-horse-battery-staple";
        let home = "/home/alice";
        let log = [
            line("INFO", &format!("request with {secret} in it, and {secret}{secret}")),
            line("INFO", r#"headers: {"authorization": "Bearer other-credential", "x-stats-token": "another-one"} done"#),
            line("INFO", "Authorization: Basic dXNlcjpwYXNz"),
            line("INFO", "X-Stats-Token=plain-value; done"),
            line("INFO", "AUTHORIZATION: Bearer ABC.def-ghi"),
            line("INFO", "no authorization given, authorization is needed"),
            line("INFO", &format!("opened {home}/.local/share/rettui/store.json and {home}")),
        ]
        .concat();
        let value = answer_of(log.as_bytes(), "", secret, Some(home));
        let text = value.to_string();
        for leaked in [secret, "other-credential", "another-one", "dXNlcjpwYXNz", "plain-value", "ABC.def-ghi", "alice"] {
            assert!(!text.contains(leaked), "{leaked} leaked: {text}");
        }
        let found = texts(&value);
        assert_eq!(found[0], "rettui::net: request with [redacted] in it, and [redacted][redacted]");
        assert_eq!(found[1], r#"rettui::net: headers: {"authorization": "[redacted]"#);
        assert_eq!(found[2], "rettui::net: Authorization: [redacted]");
        assert_eq!(found[3], "rettui::net: X-Stats-Token=[redacted]");
        assert_eq!(found[4], "rettui::net: AUTHORIZATION: [redacted]");
        assert_eq!(found[5], "rettui::net: no authorization [redacted]", "more than needed, rather than too little");
        assert_eq!(found[6], "rettui::net: opened ~/.local/share/rettui/store.json and ~");
    }

    #[test]
    fn a_home_that_is_the_root_or_empty_is_left_alone() {
        assert_eq!(redact("/usr/bin/x", &["unused-secret-value"], Some("/"), false), "/usr/bin/x");
        assert_eq!(redact("/usr/bin/x", &["unused-secret-value"], Some(""), false), "/usr/bin/x");
        assert_eq!(redact("C:\\Users\\bob\\a C:/Users/bob/b", &["unused-secret-value"], Some("C:\\Users\\bob"), false), "~\\a ~/b");
        assert_eq!(redact("x authorization: ", &["unused-secret-value"], None, false), "x authorization: ", "nothing to redact");
        assert_eq!(redact("x authorization: \u{200b}\t", &["unused-secret-value"], None, false), "x authorization: \u{200b}\t");
        assert_eq!(
            redact("token \u{e9}\u{e9} authorization: \u{e9}\u{e9}x end", &["unused-secret-value"], None, false),
            "token [redacted]"
        );
    }

    #[test]
    fn a_long_line_is_cut_and_says_so() {
        let log = [line("INFO", &"x".repeat(5000)), line("INFO", "short")].concat();
        let value = answer_of(log.as_bytes(), "", "unused-secret-value", None);
        assert_eq!(value["truncated"], true);
        let found = texts(&value);
        assert_eq!(found[0].chars().count(), 1000);
        assert_eq!(found[1], "rettui::net: short");
        // Exactly at the limit is not a cut.
        let exact = format!("2025-01-31T12:34:56Z  INFO {}\n", "y".repeat(1000));
        assert_eq!(answer_of(exact.as_bytes(), "", "unused-secret-value", None)["truncated"], false);
        let long_multibyte = format!("2025-01-31T12:34:56Z  INFO {}\n", "\u{e9}".repeat(1200));
        let value = answer_of(long_multibyte.as_bytes(), "", "unused-secret-value", None);
        assert_eq!((value["truncated"].as_bool(), texts(&value)[0].chars().count()), (Some(true), 1000));
    }

    #[test]
    fn the_answer_is_capped_in_size_keeping_the_newest() {
        let dir = folder("cap");
        let path = dir.join("rettui.log");
        let mut log = String::new();
        for i in 0..400 {
            log.push_str(&line("INFO", &format!("{i:04} {}", "z".repeat(900))));
        }
        std::fs::write(&path, &log).unwrap();
        let source = Source { path: Some(path), secret: "unused-secret-value".into(), web_token: None, home: None };
        let body = source.answer(Query { lines: 400, level: 2 });
        assert!(body.len() <= RESPONSE_MAX + 1024, "{} bytes", body.len());
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["truncated"], true);
        let found = texts(&value);
        assert!(found.len() < 400 && found.len() > 100, "{}", found.len());
        assert!(found.last().unwrap().contains("0399 "), "the newest is kept");
        assert!(found[0].contains(&format!("{:04} ", 400 - found.len())), "a run of the newest, in order");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_the_end_of_a_big_file_is_read() {
        let dir = folder("scan");
        let path = dir.join("rettui.log");
        let mut log = String::new();
        for i in 0..200 {
            log.push_str(&line("INFO", &format!("{i:04}")));
        }
        std::fs::write(&path, &log).unwrap();
        let source = Source { path: Some(path.clone()), secret: "unused-secret-value".into(), web_token: None, home: None };
        // A window of about 10 lines (each 52 bytes): the first, partial, line is dropped.
        let value: Value = serde_json::from_str(&source.answer_within(Query::default(), 520, RESPONSE_MAX)).unwrap();
        let found = texts(&value);
        assert!((8..=10).contains(&found.len()), "{found:?}");
        assert!(found.last().unwrap().ends_with("0199") && found.iter().all(|t| t.starts_with("rettui::net: 0")));
        assert_eq!(value["truncated"], true, "older lines were not looked at");
        // Enough lines found in the window: no cut to report.
        let value: Value = serde_json::from_str(&source.answer_within(Query { lines: 3, level: 2 }, 520, RESPONSE_MAX)).unwrap();
        assert_eq!((texts(&value).len(), &value["truncated"]), (3, &json!(false)));
        // A window with no newline at all has no whole line.
        let value: Value = serde_json::from_str(&source.answer_within(Query::default(), 20, RESPONSE_MAX)).unwrap();
        assert_eq!(value["lines"], json!([]));
        // The real window is 1 MiB: a bigger file gives its end only.
        let mut big = String::new();
        while big.len() < 3 * 1024 * 1024 {
            big.push_str(&line("INFO", &"b".repeat(200)));
        }
        big.push_str(&line("INFO", "the very end"));
        std::fs::write(&path, &big).unwrap();
        let value: Value = serde_json::from_str(&source.answer(Query { lines: 500, level: 2 })).unwrap();
        assert!(texts(&value).last().unwrap().ends_with("the very end"));
        assert_eq!(texts(&value).len(), 500);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_escaped_form_of_the_token_is_taken_out_too() {
        let secret = "ab\"cd\\ef-0123456789";
        let escaped = secret.escape_debug().to_string();
        assert_ne!(escaped, secret);
        let text = redact(&format!("a {secret} b {escaped} c {secret:?}"), &[secret], None, false);
        assert_eq!(text, "a [redacted] b [redacted] c \"[redacted]\"");
    }

    /// What `redact` makes of `line`, with no secret or home to take out.
    fn redacted(line: &str) -> String {
        redact(line, &["unused-secret-value"], None, false)
    }

    #[test]
    fn quoted_headers_and_schemes_lose_all_that_follows() {
        assert_eq!(redacted(r#"x-stats-token: "two words" after"#), r#"x-stats-token: "[redacted]"#);
        assert_eq!(redacted(r#"{"x-stats-token": "a b;c)d", "host": "h"}"#), r#"{"x-stats-token": "[redacted]"#);
        assert_eq!(redacted(r#"authorization: "Bearer a b c" next"#), r#"authorization: "[redacted]"#);
        assert_eq!(redacted(r#"{\"authorization\": \"Bearer tok\", \"a\": 1}"#), r#"{\"authorization\": \"[redacted]"#);
        assert_eq!(redacted("authorization: x-stats-token: 'unclosed value"), "authorization: [redacted]");
        let vapid = "sent Authorization: vapid t=eyJhbGciOi.payload.sig, k=BPublicKey-1234 to the service";
        assert_eq!(redacted(vapid), "sent Authorization: [redacted]");
    }

    /// The ways the penetration test got a header's value past the first redaction, each a line as it was logged.
    #[test]
    fn every_way_of_printing_a_secret_value_loses_it() {
        let cases = [
            (r#"x-stats-token: Some("OTHER-TOKEN-01") rust Option debug"#, "OTHER-TOKEN-01"),
            (r#"headers [("authorization", "Bearer LEAK-BEARER-02"), ("x-stats-token", "LEAK-XST-02")]"#, "LEAK-"),
            ("GET /?q=authorization%3A%20Bearer%20LEAK-URLENC-03", "LEAK-URLENC-03"),
            ("authorization%3A%20Bearer%20LEAK-URLENC-03b", "LEAK-URLENC-03b"),
            ("%61uthorization%3a LEAK-URLENC-03c", "LEAK-URLENC-03c"),
            (r#"{:x-stats-token => "LEAK-RUBY-04"}"#, "LEAK-RUBY-04"),
            ("x-stats-token => LEAK-ARROW-04b", "LEAK-ARROW-04b"),
            (r#"json {"Authorization":"Bearer LEAK-JSON-05"}"#, "LEAK-JSON-05"),
            ("cookie: session=LEAK-COOKIE-06; api_key=LEAK-APIKEY-06 token=LEAK-QTOKEN-06", "LEAK-"),
            ("Set-Cookie: id=LEAK-SETCOOKIE; HttpOnly", "LEAK-SETCOOKIE"),
            ("authorization:Bearer LEAK-NOSPACE-15", "LEAK-NOSPACE-15"),
            ("Authorization\t:\tBearer LEAK-TAB-16", "LEAK-TAB-16"),
            ("proxy-authorization: Basic LEAK-PROXY-17", "LEAK-PROXY-17"),
            ("x-stats-token: 'q1' LEAK-AFTER-QUOTE-18", "LEAK-AFTER-QUOTE-18"),
            ("X-Stats-Token:\u{a0}LEAK-NBSP-19", "LEAK-NBSP-19"),
            ("authorization\u{200b}: Bearer LEAK-ZWSP-20", "LEAK-ZWSP-20"),
            ("auth\u{200d}ori\u{ad}zation\u{7}: LEAK-INVISIBLE-20b", "LEAK-INVISIBLE-20b"),
            ("x\u{2010}stats\u{2010}token: LEAK-UNIDASH-21", "LEAK-UNIDASH-21"),
            ("bearer LEAK-BAREBEARER-23", "LEAK-BAREBEARER-23"),
            ("apikey=LEAK-APIKEY-24 api-key: LEAK-APIKEY-25", "LEAK-APIKEY"),
            ("password=hunter2-LEAK passwd hunter3-LEAK", "LEAK"),
            ("client_secret: LEAK-SECRET-26", "LEAK-SECRET-26"),
        ];
        for (line, leak) in cases {
            let text = redacted(line);
            assert!(!text.contains(leak), "{line:?} leaked as {text:?}");
            assert!(text.ends_with(REDACTED), "{line:?} became {text:?}");
        }
        assert_eq!(redacted(r#"x-stats-token: Some("X")"#), "x-stats-token: [redacted]");
        assert_eq!(redacted(r#"[("authorization", "Bearer X")]"#), r#"[("authorization", "[redacted]"#);
        assert_eq!(redacted("Authorization\t:\tBearer X"), "Authorization\t:\t[redacted]");
        assert_eq!(redacted("a\u{200b}uthorization: X"), "a\u{200b}uthorization: [redacted]");
        // A broken `%` sequence or a multi-byte character after one is only text.
        assert_eq!(redacted("100% sure, %zz, %\u{e9}\u{e9}, %4"), "100% sure, %zz, %\u{e9}\u{e9}, %4");
    }

    #[test]
    fn urls_lose_their_query_and_password() {
        // As ureq writes a request at `debug`.
        let key = "LEAK-TILE-KEY-0123";
        for line in [
            format!("ureq::unit: sending request GET https://tiles.example.com/3/2/1.png?apikey={key}"),
            format!("ureq::stream: writing prelude: GET /3/2/1.png?apikey={key} HTTP/1.1"),
            format!("ureq::unit: response 200 to GET https://tiles.example.com/3/2/1.png?style=x&k={key}"),
            format!("url=\"https://tiles.example.com/x?{key}\" done"),
            format!("fetching HTTPS://h.example?key={key}"),
        ] {
            let text = redacted(&line);
            assert!(!text.contains(key) && text.contains("?[redacted]"), "{line:?} became {text:?}");
        }
        assert_eq!(
            redacted("ureq::unit: response 200 to GET https://tiles.example.com/1.png?k=K for a tile"),
            "ureq::unit: response 200 to GET https://tiles.example.com/1.png?[redacted] for a tile"
        );
        assert_eq!(redacted("GET /3/2/1.png?k=K HTTP/1.1"), "GET /3/2/1.png?[redacted] HTTP/1.1");
        assert_eq!(redacted("'/a?b' \"/c?d\" `/e?f` </g?h>"), "'/a?[redacted]' \"/c?[redacted]\" `/e?[redacted]` </g?[redacted]>");
        assert_eq!(redacted("Basic in url https://user:LEAK-PASS@example.invalid/x"), "Basic in url https://example.invalid/x");
        assert_eq!(redacted("http://a:b@c@host.example?x=1 and ftp://u:p@h"), "http://host.example?[redacted] and ftp://h");
        // An `@` past the host is the path's, and stays.
        assert_eq!(redacted("https://host.example/@user/x"), "https://host.example/@user/x");
        // No query, or a `?` at the end of a sentence or before any `/`: nothing to take out.
        for kept in ["https://host.example/a/b", "why a/b?", "what? a/b", "is it?/x", "a ?", "://", "\u{e9}/\u{e9}?"] {
            assert_eq!(redacted(kept), kept);
        }
        assert_eq!(redacted("\u{e9}/\u{e9}?\u{e9} x"), "\u{e9}/\u{e9}?[redacted] x");
    }

    #[test]
    fn the_web_login_token_is_taken_out_too() {
        let dir = folder("web-token");
        let (log, web_token) = (dir.join("rettui.log"), dir.join("web_token"));
        let login = "0123456789abcdef0123456789abcdef0123456789abcdef";
        std::fs::write(&log, line("INFO", &format!("login link opened with {login} at last"))).unwrap();
        std::fs::write(&web_token, format!("{login}\n")).unwrap();
        let source = Source { path: Some(log), secret: "unused-secret-value".into(), web_token: Some(web_token.clone()), home: None };
        let value: Value = serde_json::from_str(&source.answer(Query::default())).unwrap();
        assert_eq!(texts(&value), ["rettui::net: login link opened with [redacted] at last"]);
        // A file that is too short to be a token, or none, takes nothing out.
        std::fs::write(&web_token, "short").unwrap();
        assert_eq!(read_secret(&web_token), None);
        assert_eq!(read_secret(&dir.join("missing")), None);
        // Neither does a file that isn't text, or one that fills the cap.
        std::fs::write(&web_token, [0xffu8; 40]).unwrap();
        assert_eq!(read_secret(&web_token), None);
        std::fs::write(&web_token, "a".repeat(WEB_TOKEN_MAX as usize + 10)).unwrap();
        assert_eq!(read_secret(&web_token), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_whole_token_at_the_end_of_a_cut_line_is_taken_out() {
        let secret = "a0123456789abcdefa";
        let out = redact(&format!("x {secret}"), &[secret], None, true);
        assert!(!out.contains("a0123"), "{out}");
        assert_eq!(out, format!("x {REDACTED}"));
    }

    #[test]
    fn an_unfinished_last_line_does_not_leave_part_of_the_token() {
        let secret = "correct-horse-battery-staple";
        // The newest line, still being written, stops halfway through the token.
        let log = format!("{}2025-01-31T12:34:56Z  INFO rettui::net: sent {}", line("INFO", "before"), &secret[..14]);
        let value = answer_of(log.as_bytes(), "", secret, None);
        assert_eq!(texts(&value), ["rettui::net: before", "rettui::net: sent "]);
        assert_eq!(value["truncated"], true);
        // The same text on a finished line is only text.
        let value = answer_of(format!("{log}\n").as_bytes(), "", secret, None);
        assert_eq!(texts(&value)[1], format!("rettui::net: sent {}", &secret[..14]));
        assert_eq!(value["truncated"], false);
    }

    #[test]
    fn a_line_break_inside_an_event_cannot_forge_an_entry() {
        // Written as the log writer writes an event with a peer's text holding a line break and a fake entry.
        let dir = folder("forge");
        let path = dir.join("rettui.log");
        let log = crate::logging::CappedLog::open(&path).unwrap();
        let forged = "2025-01-31T12:34:56Z  INFO forged: injected";
        for event in [
            format!("2025-01-31T12:34:55Z DEBUG rettui::net: a peer said hi\n{forged}\n"),
            format!("2025-01-31T12:34:55Z TRACE rettui::net: secret TRACE-ONLY\r\n{forged} TRACE-TAIL\n"),
            line("INFO", "real"),
        ] {
            std::io::Write::write_all(&mut tracing_subscriber::fmt::MakeWriter::make_writer(&log), event.as_bytes()).unwrap();
        }
        let source = Source { path: Some(path), secret: "unused-secret-value".into(), web_token: None, home: None };
        let value: Value = serde_json::from_str(&source.answer(Query::default())).unwrap();
        assert_eq!(texts(&value), ["rettui::net: real"], "the forged INFO lines stay inside the DEBUG and TRACE entries");
        let debug: Value = serde_json::from_str(&source.answer(Query { lines: 10, level: 3 })).unwrap();
        assert_eq!(texts(&debug)[0], format!("rettui::net: a peer said hi\\n{forged}"));
        assert!(!debug.to_string().contains("TRACE-"), "{debug}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_log_from_an_older_version_with_line_breaks_still_reads() {
        // Before events were kept on one line, a message's second line was a line of its own.
        let log = format!("{}  second line\n", line("WARN", "first"));
        assert_eq!(texts(&answer_of(log.as_bytes(), "level=warn", "unused-secret-value", None)), ["rettui::net: first", "  second line"]);
    }

    #[test]
    fn a_megabyte_line_of_header_words_is_quick_and_cut() {
        let long = "authorization ".repeat(74_000);
        let plain = format!("2025-01-31T12:34:56Z  INFO {}\n", "x ".repeat(500_000));
        let value = answer_of(plain.as_bytes(), "", "unused-secret-value", None);
        assert_eq!((value["truncated"].as_bool(), texts(&value)[0].chars().count()), (Some(true), 1000));
        assert!(long.len() > 1_000_000 && long.len() < 1024 * 1024 - 1024, "about a MiB, and inside the window read");
        let log = format!("2025-01-31T12:34:56Z  INFO {long}\n{}", line("INFO", "after"));
        let started = std::time::Instant::now();
        let value = answer_of(log.as_bytes(), "", "unused-secret-value", Some("/home/x"));
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(1), "{took:?}");
        assert_eq!(value["truncated"], true);
        assert_eq!(texts(&value)[0], "authorization [redacted]");
        assert_eq!(texts(&value)[1], "rettui::net: after");
        // With signs too, a hostile line is still a single pass.
        let signs = "authorization: x-stats-token=".repeat(40_000);
        let started = std::time::Instant::now();
        let redacted = redact(super::cut_chars(&signs, REDACT_MAX).0, &["unused-secret-value"], None, true);
        assert!(started.elapsed() < std::time::Duration::from_secs(1) && redacted.contains(REDACTED));
        // A whole megabyte through each step, as if nothing had cut it: still one pass.
        let mixed = "authorizatio %4 https://a/b?c=d x\u{200b} ".repeat(25_000);
        let started = std::time::Instant::now();
        let _ = redact_after_words(&redact_urls(&mixed));
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    }

    #[test]
    fn a_token_cut_in_two_by_the_cut_does_not_leave_its_start() {
        let secret = "correct-horse-battery-staple";
        let cut = format!("{}{}", "x".repeat(10), &secret[..12]);
        assert_eq!(redact(&cut, &[secret], None, true), "xxxxxxxxxx");
        // The same text, when the line was whole, is just text.
        assert_eq!(redact(&cut, &[secret], None, false), cut);
    }

    #[test]
    fn debug_of_a_source_does_not_show_the_secret() {
        let source = Source {
            path: Some(PathBuf::from("/x/rettui.log")),
            secret: "correct-horse-battery-staple".into(),
            web_token: None,
            home: None,
        };
        let shown = format!("{source:?} {source:#?}");
        assert!(!shown.contains("correct-horse") && !shown.contains("secret") && shown.contains("rettui.log"), "{shown}");
    }
}
