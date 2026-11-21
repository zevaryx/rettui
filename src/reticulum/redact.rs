//! A read-only, redacted view of the Reticulum config, for remote management.
//!
//! The config holds secrets (IFAC passphrases and network names, device
//! paths, commands), so nothing is shown unless it is on a short allow-list
//! ([`SHOWN`]): every other key is listed with only whether it is set. Even
//! an allowed key shows its value only if it has the shape that key's values
//! have ([`Shape`]: a host or address, a port, yes or no, a plain word, a
//! number), so a secret written into one (`target_host = user:secret@host`)
//! stays hidden. A pipe interface, or any interface with `location_cmd` or a
//! `reachable_on` that isn't a host name or an address (Python Reticulum
//! runs such a value as a program), shows only `type` and `enabled`: not even
//! the names of its other keys.
//!
//! The text is untrusted: it is read with a size cap, parsed without
//! panicking, and the number of sections and keys and the length of names and
//! values are capped. Nothing here reports a path or an error, so a config
//! that is missing or unreadable gives `null`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::doc::{strip_comment, unquote};

/// The keys whose values are shown. Anything else shows only `set`.
pub const SHOWN: &[&str] = &[
    "type",
    "enabled",
    "interface_enabled",
    "mode",
    "interface_mode",
    "announce_cap",
    "bitrate",
    "ingress_control",
    "enable_transport",
    "share_instance",
    "listen_ip",
    "listen_port",
    "target_host",
    "target_port",
    "reachable_on",
    "discoverable",
];

/// What a shown value must look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// An IP address, v4 or v6 (an address to listen on).
    Ip,
    /// A host name or an IP address (IPv6 perhaps in brackets).
    Host,
    /// A port: 1 to 65535.
    Port,
    /// yes, no, true, false, on or off, in any case.
    Switch,
    /// A plain word (an interface type or mode): letters, digits and `_`.
    Word,
    /// A number, perhaps with a fraction.
    Number,
}

/// The shape of `key`'s values; `None` for a key not in [`SHOWN`].
fn shape(key: &str) -> Option<Shape> {
    Some(match key {
        "listen_ip" => Shape::Ip,
        "target_host" | "reachable_on" => Shape::Host,
        "listen_port" | "target_port" => Shape::Port,
        "enabled" | "interface_enabled" | "ingress_control" | "enable_transport" | "share_instance" | "discoverable" => Shape::Switch,
        "type" | "mode" | "interface_mode" => Shape::Word,
        "announce_cap" | "bitrate" => Shape::Number,
        _ => return None,
    })
}

/// A host name: dot-separated labels of letters, digits, `-` and `_`.
fn host_name(value: &str) -> bool {
    value.len() <= 253
        && value.trim_end_matches('.').split('.').all(|label| {
            !label.is_empty() && label.len() <= 63 && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

/// The keys Reticulum runs as a program when the value isn't an address
/// (an executable path gives the address on its output).
pub const REACHABLE: &[&str] = &["reachable_on", "discovery_reachable_on"];

/// Whether `value` (unquoted) is a host name or an IP address, so a
/// `reachable_on` with it names a host and not a program.
pub fn host_shaped(value: &str) -> bool {
    has_shape(value, Shape::Host)
}

/// Whether `value` has `shape`.
fn has_shape(value: &str, shape: Shape) -> bool {
    match shape {
        Shape::Ip => value.parse::<std::net::IpAddr>().is_ok(),
        Shape::Host => {
            let bare = value.strip_prefix('[').and_then(|v| v.strip_suffix(']'));
            match bare {
                Some(v6) => v6.parse::<std::net::Ipv6Addr>().is_ok(),
                None => value.parse::<std::net::IpAddr>().is_ok() || (!value.contains(':') && host_name(value)),
            }
        }
        Shape::Port => value.len() <= 5 && value.bytes().all(|b| b.is_ascii_digit()) && value.parse::<u16>().is_ok_and(|p| p > 0),
        Shape::Switch => ["yes", "no", "true", "false", "on", "off"].iter().any(|word| value.eq_ignore_ascii_case(word)),
        Shape::Word => value.len() <= MAX_NAME && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        Shape::Number => {
            let (whole, fraction) = value.split_once('.').unwrap_or((value, "0"));
            let digits = |part: &str| !part.is_empty() && part.len() <= 20 && part.bytes().all(|b| b.is_ascii_digit());
            digits(whole) && digits(fraction)
        }
    }
}

/// The only keys a pipe interface (or one with `location_cmd`) shows.
const PIPE_SHOWN: &[&str] = &["type", "enabled"];

/// The most of the file that is read.
const MAX_FILE: u64 = 1 << 20;
/// Lines looked at, and the longest part of a line looked at.
const MAX_LINES: usize = 20_000;
const MAX_LINE: usize = 2048;
/// Sections and keys per section listed, and the longest value or name.
const MAX_SECTIONS: usize = 128;
const MAX_KEYS: usize = 64;
const MAX_VALUE: usize = 256;
const MAX_NAME: usize = 64;
/// About how much of the view is kept, in bytes of JSON.
const MAX_OUTPUT: usize = 24 * 1024;
/// `[[[[x]]]]` is as deep as a section goes.
const MAX_DEPTH: usize = 4;

/// One key of a section: its value if it may be shown, and whether it is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub key: String,
    pub value: Option<String>,
    pub set: bool,
}

/// One section, by its path (`["interfaces", "Name"]`). `restricted` is an
/// interface that runs a program or has no `type`, so only its `type` and
/// `enabled` are listed (and nothing in it can be changed from afar); it
/// isn't part of the revision, which the keys listed already change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub path: Vec<String>,
    pub keys: Vec<Key>,
    pub restricted: bool,
}

/// The redacted config: sections and keys sorted, so the same config always
/// gives the same view and revision.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct View {
    pub sections: Vec<Section>,
    /// Some sections, keys or lines were left out (over a cap).
    pub truncated: bool,
}

/// A section being read.
#[derive(Default)]
struct Raw {
    /// Key to (its text if it may be shown, whether it has a value).
    keys: BTreeMap<String, (Option<String>, bool)>,
    /// Some key says this runs a program (`command`, `location_cmd`, a pipe
    /// `type`, a `reachable_on` that isn't a host). Decided as each key is read, before any cap, and never
    /// undone by a later key or a section of the same name.
    restricted: bool,
    /// A `type` with a value was read.
    typed: bool,
}

/// `text` with control and invisible characters written as `\u{..}`, cut to
/// `max` bytes (at a character boundary).
fn clean(text: &str, max: usize) -> String {
    let mut shown = String::new();
    for c in text.chars() {
        let piece = if c.is_control() || crate::stats::is_invisible(c) { c.escape_unicode().to_string() } else { c.to_string() };
        if shown.len() + piece.len() > max {
            break;
        }
        shown.push_str(&piece);
    }
    shown
}

/// The first `max` bytes of `text`, cut at a character boundary.
fn head(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn plain_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_NAME && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Whether every quote in `value` is closed.
fn balanced(value: &str) -> bool {
    let mut quote: Option<char> = None;
    for c in value.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            _ => {}
        }
    }
    quote.is_none()
}

/// `[name]`, `[[name]]`, ...: the depth and name, or `None` if it isn't one.
fn header(line: &str) -> Option<(usize, String)> {
    let depth = line.chars().take_while(|&c| c == '[').count();
    let close = line.chars().rev().take_while(|&c| c == ']').count();
    if depth == 0 || depth != close || line.len() <= depth * 2 {
        return None;
    }
    Some((depth, unquote(&line[depth..line.len() - depth])))
}

/// A line starting with `[` (comment and spaces taken off) as the view reads
/// it: the depth and name of the section it starts, or `None` if the view
/// takes it for no header (and then lists no keys until the next one).
pub fn header_of(line: &str) -> Option<(usize, String)> {
    let (depth, name) = header(line)?;
    let name = clean(name.trim(), MAX_NAME);
    (depth <= MAX_DEPTH && !name.is_empty()).then_some((depth, name))
}

/// Read `text` and redact it.
pub fn redact(text: &str) -> View {
    let mut raw: BTreeMap<Vec<String>, Raw> = BTreeMap::new();
    let mut truncated = false;
    // The names of the sections the current line is inside.
    let mut stack: Vec<String> = Vec::new();
    // The path keys go to now; `None` after a header that makes no sense.
    let mut current: Option<Vec<String>> = None;
    let mut lines = text.lines();
    let mut seen = 0;
    while let Some(line) = lines.next() {
        seen += 1;
        if seen > MAX_LINES {
            truncated = true;
            break;
        }
        // A long line is still read whole (the file is capped), as a cut
        // one could hide `location_cmd`, a quote or the start of a
        // multi-line value; it only costs the view its `complete` mark.
        if line.len() > MAX_LINE {
            truncated = true;
        }
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            current = None;
            let Some((depth, name)) = header_of(line) else { continue };
            if depth > stack.len() + 1 {
                continue;
            }
            stack.truncate(depth - 1);
            stack.push(name);
            // Sections inside an interface's own section would be hidden
            // with it, so they are not listed at all.
            if stack.first().is_some_and(|s| s == "interfaces") && depth > 2 {
                continue;
            }
            if !raw.contains_key(&stack) && raw.len() >= MAX_SECTIONS {
                truncated = true;
                continue;
            }
            raw.entry(stack.clone()).or_default();
            current = Some(stack.clone());
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let key = unquote(&line[..eq]);
        let value = line[eq + 1..].trim();
        // A value over several lines is skipped whole, as its lines are
        // not keys. One that never ends (or runs past the line limit) is
        // skipped to the end, and the view says it is cut.
        let mut multiline = false;
        for quote in ["\"\"\"", "'''"] {
            if let Some(rest) = value.strip_prefix(quote) {
                multiline = true;
                if !rest.contains(quote) {
                    let mut closed = false;
                    for next in lines.by_ref() {
                        seen += 1;
                        if next.contains(quote) {
                            closed = true;
                            break;
                        }
                        if seen > MAX_LINES {
                            break;
                        }
                    }
                    if !closed {
                        truncated = true;
                    }
                }
            }
        }
        let (Some(path), true) = (&current, plain_key(&key)) else { continue };
        let Some(section) = raw.get_mut(path) else { continue };
        let value_text = unquote(value);
        // Decided here, before the key cap can drop the key.
        let name = key.to_ascii_lowercase();
        if matches!(name.as_str(), "location_cmd" | "command")
            || (name == "type" && (multiline || value_text.to_ascii_lowercase().contains("pipe")))
            || (REACHABLE.contains(&name.as_str()) && (multiline || !value_text.is_empty() && !host_shaped(&value_text)))
        {
            section.restricted = true;
        }
        let set = multiline || !value_text.is_empty();
        if name == "type" && set {
            section.typed = true;
        }
        if !section.keys.contains_key(&key) && section.keys.len() >= MAX_KEYS {
            truncated = true;
            continue;
        }
        let text = if multiline { None } else { shown_value(&key, value) };
        section.keys.insert(key, (text, set));
    }
    let mut sections: Vec<Section> = raw.into_iter().map(|(path, section)| shown(path, section)).collect();
    truncated |= fit(&mut sections);
    View { sections, truncated }
}

/// Drop what comes after the first `MAX_OUTPUT` bytes of the view (in its
/// sorted order). Whether anything was dropped.
fn fit(sections: &mut Vec<Section>) -> bool {
    let mut used = 0;
    let mut kept = Vec::new();
    for mut section in std::mem::take(sections) {
        used += 32 + section.path.iter().map(|n| n.len() + 4).sum::<usize>();
        if used > MAX_OUTPUT {
            *sections = kept;
            return true;
        }
        let mut keys = Vec::new();
        let mut cut = false;
        for key in std::mem::take(&mut section.keys) {
            used += 40 + key.key.len() + key.value.as_ref().map_or(0, String::len);
            if used > MAX_OUTPUT {
                cut = true;
                break;
            }
            keys.push(key);
        }
        section.keys = keys;
        kept.push(section);
        if cut {
            *sections = kept;
            return true;
        }
    }
    *sections = kept;
    false
}

/// The keys of `section` that may be shown.
fn shown(path: Vec<String>, section: Raw) -> Section {
    // An interface not known to be free of programs is treated as one that
    // runs one, so a missing `type` hides everything but `enabled`.
    let in_interface = path.len() == 2 && path[0] == "interfaces";
    let restricted = in_interface && (section.restricted || !section.typed);
    let keys = section
        .keys
        .into_iter()
        .filter(|(key, _)| !restricted || PIPE_SHOWN.contains(&key.as_str()))
        .map(|(key, (text, set))| Key { value: text, key, set })
        .collect();
    Section { path, keys, restricted }
}

/// What the view shows for `key` with the raw value `raw` (as written, before
/// unquoting, without a comment): its value, if `key` is in [`SHOWN`] and the
/// value has the shape of that key's values, else `None`. A triple-quoted
/// value is never shown (the caller says so, as only it sees the lines).
pub fn shown_value(key: &str, raw: &str) -> Option<String> {
    let raw = raw.trim();
    let value = unquote(raw);
    // `reachable_on` may also name a program, which the shape keeps out.
    let may_show = SHOWN.contains(&key)
        && !raw.starts_with("\"\"\"")
        && !raw.starts_with("'''")
        && !value.is_empty()
        && balanced(raw)
        && shape(key).is_some_and(|shape| has_shape(&value, shape));
    may_show.then(|| clean(head(&value, MAX_VALUE * 2), MAX_VALUE))
}

impl View {
    /// 32 hex characters over what is shown, so it changes when the view
    /// does. A secret changed to another secret doesn't change it.
    pub fn revision(&self) -> String {
        let mut hash = Sha256::new();
        for section in &self.sections {
            hash.update(b"[");
            for name in &section.path {
                hash.update(name.as_bytes());
                hash.update([0]);
            }
            hash.update(b"]\n");
            for key in &section.keys {
                let value = key.value.as_deref().map_or("\u{1}".to_string(), |v| format!("={v}"));
                hash.update(format!("{}\0{}\0{value}\n", key.key, key.set).as_bytes());
            }
        }
        hash.update(if self.truncated { "truncated" } else { "complete" });
        hex::encode(&hash.finalize()[..16])
    }

    /// The `reticulum` object of the management state, with the sections
    /// that can be changed from afar (`editable`, which isn't part of the
    /// revision).
    pub fn to_json_with(&self, shared_instance: bool, editable: &[Vec<String>]) -> Value {
        let sections: Vec<Value> = self
            .sections
            .iter()
            .map(|s| {
                let keys: Vec<Value> = s.keys.iter().map(|k| json!({ "key": k.key, "value": k.value, "set": k.set })).collect();
                json!({ "path": s.path, "keys": keys })
            })
            .collect();
        json!({
            "revision": self.revision(),
            "shared_instance": shared_instance,
            "truncated": self.truncated,
            "sections": sections,
            "editable": editable,
        })
    }
}

/// The first 1 MiB of the file at `path` as text (bytes that aren't UTF-8
/// as U+FFFD), and whether there was more; `None` if it can't be read.
pub fn read_capped(path: &Path) -> Option<(String, bool)> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).ok()?;
    let over = bytes.len() as u64 > MAX_FILE;
    bytes.truncate(MAX_FILE as usize);
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    Some((text, over))
}

/// The redacted view of `text` (the start of a file that went on if `over`).
pub fn view_of(text: &str, over: bool) -> View {
    let mut view = redact(text);
    view.truncated |= over;
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every one of these must stay out of any output.
    const SECRETS: &[&str] = &[
        "hunter2-passphrase",
        "fake-network-name-xyz",
        "ifac-key-0123456789abcdef",
        "/dev/ttyUSB-secret-port",
        "s3cret-password-value",
        "token-abcdef-123456",
        "rm -rf /secret-command",
        "/usr/local/bin/secret-script",
        "multi-line-secret-body",
        "secret-in-comment",
    ];

    const CONFIG: &str = r#"
# a comment with secret-in-comment
[reticulum]
  enable_transport = Yes
  share_instance = No
  shared_instance_port = 37428
  instance_name = hunter2-passphrase
  panic_on_interface_error = no

[logging]
  loglevel = 4

[interfaces]
  [[Public TCP]]
    type = TCPServerInterface
    interface_enabled = yes
    listen_ip = 0.0.0.0
    listen_port = 4242
    passphrase = hunter2-passphrase
    network_name = fake-network-name-xyz
    ifac_netkey = ifac-key-0123456789abcdef
    mode = gateway
    bitrate = 62500
    announce_cap = 2
    ingress_control = true
    discoverable = yes
    reachable_on = node.example.org

  [[Radio]]
    type = RNodeInterface
    enabled = yes
    port = /dev/ttyUSB-secret-port
    password = s3cret-password-value
    api_token = token-abcdef-123456
    notes = """
multi-line-secret-body
fake = line
"""

  [[Backbone]]
    type = BackboneInterface
    target_host = hub.example.org
    target_port = 4243
    ifac_size = 16
    passphrase = ""

  [[Shell]]
    type = PipeInterface
    enabled = yes
    command = rm -rf /secret-command
    respawn_delay = 5
    mode = full

  [[Sneaky]]
    type = TCPClientInterface
    enabled = yes
    location_cmd = /usr/local/bin/secret-script
    target_host = leak.example.org
    passphrase = hunter2-passphrase
"#;

    fn find<'a>(view: &'a View, path: &[&str]) -> Option<&'a Section> {
        view.sections.iter().find(|s| s.path == path)
    }

    fn key<'a>(section: &'a Section, name: &str) -> Option<&'a Key> {
        section.keys.iter().find(|k| k.key == name)
    }

    fn assert_no_secret(text: &str) {
        for secret in SECRETS {
            assert!(!text.contains(secret), "{secret} leaked in {text}");
        }
    }

    #[test]
    fn only_allow_listed_keys_show_a_value() {
        let view = redact(CONFIG);
        let json = view.to_json_with(false, &[]).to_string();
        assert_no_secret(&json);
        for section in &view.sections {
            for k in &section.keys {
                if k.value.is_some() {
                    assert!(SHOWN.contains(&k.key.as_str()), "{} shows a value", k.key);
                }
            }
        }
        let public = find(&view, &["interfaces", "Public TCP"]).unwrap();
        assert_eq!(key(public, "listen_port").unwrap().value.as_deref(), Some("4242"));
        assert_eq!(key(public, "mode").unwrap().value.as_deref(), Some("gateway"));
        assert_eq!(key(public, "reachable_on").unwrap().value.as_deref(), Some("node.example.org"));
        let passphrase = key(public, "passphrase").unwrap();
        assert_eq!((passphrase.value.as_deref(), passphrase.set), (None, true));
        let radio = find(&view, &["interfaces", "Radio"]).unwrap();
        assert_eq!(key(radio, "port").unwrap().value, None);
        assert!(key(radio, "notes").unwrap().set);
        assert!(key(radio, "fake").is_none(), "lines of a multi-line value are not keys");
        let backbone = find(&view, &["interfaces", "Backbone"]).unwrap();
        assert!(!key(backbone, "passphrase").unwrap().set, "an empty value is not set");
        let reticulum = find(&view, &["reticulum"]).unwrap();
        assert_eq!(key(reticulum, "enable_transport").unwrap().value.as_deref(), Some("Yes"));
        assert_eq!(key(reticulum, "instance_name").unwrap().value, None);
    }

    #[test]
    fn a_pipe_or_location_cmd_interface_shows_only_type_and_enabled() {
        let view = redact(CONFIG);
        for name in ["Shell", "Sneaky"] {
            let section = find(&view, &["interfaces", name]).unwrap();
            let names: Vec<&str> = section.keys.iter().map(|k| k.key.as_str()).collect();
            assert_eq!(names, ["enabled", "type"], "{name}");
        }
        let json = view.to_json_with(false, &[]).to_string();
        for hidden in ["command", "respawn_delay", "location_cmd", "leak.example.org"] {
            assert!(!json.contains(hidden), "{hidden} in {json}");
        }
        // Spelled another way, still a pipe.
        let view = redact("[interfaces]\n[[P]]\n type = ' pipeinterface '\n command = x\n[[Q]]\n LOCATION_CMD = y\n command = z\n");
        assert_eq!(find(&view, &["interfaces", "P"]).unwrap().keys.len(), 1);
        let q = find(&view, &["interfaces", "Q"]).unwrap();
        assert!(q.keys.is_empty(), "{q:?}");
        // A pipe whose type comes after its command is hidden too.
        let view = redact("[interfaces]\n[[P]]\n command = x\n type = PipeInterface\n");
        assert_eq!(find(&view, &["interfaces", "P"]).unwrap().keys.len(), 1);
    }

    #[test]
    fn subsections_of_an_interface_are_not_listed() {
        let view = redact("[interfaces]\n[[A]]\n type = X\n[[[inner]]]\n secret_key = 1\n");
        assert!(view.sections.iter().all(|s| s.path.len() <= 2));
        assert!(!view.to_json_with(false, &[]).to_string().contains("secret_key"));
    }

    #[test]
    fn comments_odd_spacing_and_duplicates() {
        let text = "[reticulum]\n\tenable_transport=yes   # on\n  SHARE_INSTANCE   =   'No'\n[ reticulum ]\n[reticulum]\nenable_transport = no\n#[hidden]\n\
                    [interfaces]\n[[A]]\ntype=TCPClientInterface\n[[A]]\ntarget_host = \"h # not a comment\" # real one\n";
        let view = redact(text);
        let reticulum = find(&view, &["reticulum"]).unwrap();
        assert_eq!(key(reticulum, "enable_transport").unwrap().value.as_deref(), Some("no"), "the last one wins");
        assert_eq!(key(reticulum, "SHARE_INSTANCE").unwrap().value, None, "case matters: not on the list");
        assert_eq!(view.sections.iter().filter(|s| s.path == ["reticulum"]).count(), 1, "duplicate sections merge");
        assert!(find(&view, &["reticulum "]).is_none());
        assert!(find(&view, &["hidden"]).is_none());
        let a = find(&view, &["interfaces", "A"]).unwrap();
        assert_eq!(a.keys.len(), 2);
        let host = key(a, "target_host").unwrap();
        assert_eq!((host.value.as_deref(), host.set), (None, true), "a quoted # stays in the value, which isn't a host");
    }

    #[test]
    fn sections_and_keys_are_sorted_and_the_revision_is_stable() {
        let a = redact("[interfaces]\n[[B]]\nz=1\ntype=x\n[[A]]\ntype=x\nb=1\na=2\n[reticulum]\n");
        let b = redact("[reticulum]\n[interfaces]\n[[A]]\ntype=x\na=2\nb=1\n[[B]]\ntype = x\nz=9\n");
        let paths: Vec<_> = a.sections.iter().map(|s| s.path.join("/")).collect();
        assert_eq!(paths, ["interfaces", "interfaces/A", "interfaces/B", "reticulum"]);
        let keys: Vec<_> = a.sections[1].keys.iter().map(|k| k.key.as_str()).collect();
        assert_eq!(keys, ["a", "b", "type"]);
        assert_eq!(a.revision(), a.revision());
        assert_eq!(a.revision(), b.revision(), "order and a masked value don't matter");
        assert_eq!(a.revision().len(), 32);
        assert!(a.revision().chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        let c = redact("[reticulum]\nenable_transport = yes\n");
        let d = redact("[reticulum]\nenable_transport = no\n");
        assert_ne!(c.revision(), d.revision());
        let e = redact("[reticulum]\npassphrase = a\n");
        let f = redact("[reticulum]\npassphrase = b\n");
        assert_eq!(e.revision(), f.revision(), "a secret's value never reaches the hash");
        let g = redact("[reticulum]\n");
        assert_ne!(e.revision(), g.revision(), "whether it is set does");
    }

    #[test]
    fn long_or_odd_values_are_hidden() {
        let long = "a".repeat(5000);
        let view = redact(&format!("[reticulum]\nlisten_ip = {long}\ntarget_host = ok\u{1b}[31m\u{202e}x\n"));
        let r = find(&view, &["reticulum"]).unwrap();
        for name in ["listen_ip", "target_host"] {
            let k = key(r, name).unwrap();
            assert_eq!((k.value.as_deref(), k.set), (None, true), "{name}");
        }
        let json = view.to_json_with(false, &[]).to_string();
        assert!(!json.contains("ok") && !json.contains('\u{1b}') && !json.contains("aaaa"), "{json}");
        let wide = "é".repeat(500);
        let view = redact(&format!("[reticulum]\nlisten_ip = {wide}\n"));
        assert_eq!(key(&view.sections[0], "listen_ip").unwrap().value, None);
        // The longest host name there can be is shown whole.
        let longest = vec!["a".repeat(63); 3].join(".") + "." + &"b".repeat(61);
        assert_eq!(longest.len(), 253);
        let view = redact(&format!("[reticulum]\ntarget_host = {longest}\n"));
        assert_eq!(key(&view.sections[0], "target_host").unwrap().value.as_deref(), Some(longest.as_str()));
    }

    /// The value of `name` in `[reticulum]` given as `value`: what is shown.
    fn shown_as(name: &str, value: &str) -> Option<String> {
        let view = redact(&format!("[reticulum]\n{name} = {value}\n"));
        let k = key(&view.sections[0], name).unwrap().clone();
        assert!(k.set, "{name} = {value}");
        k.value
    }

    #[test]
    fn every_shown_value_has_the_shape_of_its_key() {
        // Each key on the list has a shape, and none off it does.
        for name in SHOWN {
            assert!(shape(name).is_some(), "{name} has no shape");
        }
        assert_eq!(shape("passphrase"), None);
        // The pentest's: a secret in a host's value.
        let view = redact("[interfaces]\n[[B]]\n type = BackboneInterface\n target_host = user:SECRET@host.example\n");
        let b = find(&view, &["interfaces", "B"]).unwrap();
        assert_eq!(key(b, "target_host"), Some(&Key { key: "target_host".into(), value: None, set: true }));
        assert!(!view.to_json_with(false, &[]).to_string().contains("SECRET"));
        assert_eq!(view.to_json_with(false, &[])["sections"][1]["keys"][0], json!({ "key": "target_host", "value": null, "set": true }));
        for (name, fine) in [
            ("listen_ip", "0.0.0.0"),
            ("listen_ip", "::"),
            ("target_host", "hub.example.org"),
            ("target_host", "hub.example.org."),
            ("target_host", "10.0.0.1"),
            ("target_host", "2001:db8::1"),
            ("target_host", "[2001:db8::1]"),
            ("target_host", "my_host-1"),
            ("listen_port", "4242"),
            ("target_port", "65535"),
            ("target_port", "1"),
            ("enabled", "Yes"),
            ("interface_enabled", "off"),
            ("enable_transport", "TRUE"),
            ("share_instance", "no"),
            ("discoverable", "on"),
            ("ingress_control", "false"),
            ("type", "TCPServerInterface"),
            ("mode", "access_point"),
            ("interface_mode", "gateway"),
            ("bitrate", "62500"),
            ("announce_cap", "2.5"),
        ] {
            assert_eq!(shown_as(name, fine).as_deref(), Some(fine), "{name} = {fine}");
        }
        for (name, bad) in [
            ("target_host", "user:SECRET@host.example"),
            ("target_host", "user@host"),
            ("target_host", "host:4242"),
            ("target_host", "secret:value"),
            ("target_host", "a b"),
            ("target_host", "http://host/"),
            ("target_host", "a..b"),
            ("target_host", "[not-v6]"),
            ("listen_ip", "any text at all"),
            ("listen_ip", "node.example.org"),
            ("listen_ip", "localhost"),
            ("listen_ip", "[::1]"),
            ("listen_ip", "secret-value"),
            // A zone isn't read as part of an address: hidden, never shown wrong.
            ("listen_ip", "fe80::1%eth0"),
            ("reachable_on", "/usr/local/bin/addr.sh"),
            ("listen_port", "0"),
            ("listen_port", "65536"),
            ("listen_port", "42a"),
            ("target_port", "-1"),
            ("target_port", "+80"),
            ("enabled", "maybe"),
            ("enabled", "1"),
            ("share_instance", "yes please"),
            ("type", "TCP Interface"),
            ("type", "a-b"),
            ("mode", "gate/way"),
            ("bitrate", "'anything'"),
            ("bitrate", "1e3"),
            ("bitrate", "1.2.3"),
            ("announce_cap", ".5"),
            ("announce_cap", "5."),
            ("announce_cap", "-2"),
        ] {
            assert_eq!(shown_as(name, bad), None, "{name} = {bad}");
        }
    }

    #[test]
    fn odd_names_are_escaped_or_dropped() {
        let view = redact("[reticulum]\nbad key = 1\nk\u{1b}ey = 2\n= 3\n[[]]\n[interfaces]\n[[Na\u{1b}[0mme]]\ntype = x\n");
        assert!(view.sections.iter().all(|s| s.path.iter().all(|n| !n.contains('\u{1b}'))));
        assert!(find(&view, &["reticulum"]).unwrap().keys.is_empty());
        assert!(view.sections.iter().any(|s| s.path[1..] == ["Na\\u{1b}[0mme"]));
    }

    #[test]
    fn caps_hold_on_huge_garbage_and_binary_input() {
        let mut many = String::new();
        for i in 0..1000 {
            many.push_str(&format!("[s{i}]\n"));
        }
        let view = redact(&many);
        assert_eq!(view.sections.len(), MAX_SECTIONS);
        assert!(view.truncated);
        let mut keys = String::from("[reticulum]\n");
        for i in 0..1000 {
            keys.push_str(&format!("k{i} = 1\n"));
        }
        let view = redact(&keys);
        assert_eq!(view.sections[0].keys.len(), MAX_KEYS);
        assert!(view.truncated);
        let lines = "x\n".repeat(MAX_LINES * 2);
        assert!(redact(&lines).truncated);
        for junk in
            ["", "\0\0\0", "[", "]", "[[", "[[]]", "[[[[[[a]]]]]]", "=", "== =", "[a]]", "[[a]", "\"\"\"", "k = \"\"\"", "\u{feff}[x]"]
        {
            let _ = redact(junk).to_json_with(true, &[]);
        }
        let bytes: Vec<u8> = (0..20_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let _ = redact(&String::from_utf8_lossy(&bytes)).to_json_with(false, &[]);
        let long_line = format!("[reticulum]\n{} = 1\nlisten_ip = {}\n", "k".repeat(100_000), "é".repeat(100_000));
        let view = redact(&long_line);
        assert!(view.sections[0].keys.iter().all(|k| k.key.len() <= MAX_NAME));
        let deep = "[a]\n[[b]]\n[[[c]]]\n[[[[d]]]]\n[[[[[e]]]]]\n[[[[[[f]]]]]]\n";
        assert!(redact(deep).sections.iter().all(|s| s.path.len() <= MAX_DEPTH));
    }

    #[test]
    fn an_unterminated_multiline_value_swallows_the_rest_and_leaks_nothing() {
        let view = redact("[reticulum]\nnotes = '''\npassphrase = hunter2-passphrase\n[interfaces]\n");
        let json = view.to_json_with(false, &[]).to_string();
        assert_no_secret(&json);
        assert_eq!(view.sections.len(), 1);
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-redact-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The view of the file at `path` as the management state gives it, or
    /// `null` if it can't be read.
    fn view_of_file(path: &Path, shared_instance: bool) -> Value {
        read_capped(path).map_or(Value::Null, |(text, over)| view_of(&text, over).to_json_with(shared_instance, &[]))
    }

    #[test]
    fn a_missing_unreadable_or_binary_file_gives_null_or_a_safe_view() {
        let dir = temp("file");
        assert_eq!(view_of_file(&dir.join("missing"), false), Value::Null);
        assert_eq!(view_of_file(&dir, false), Value::Null, "a folder");
        std::fs::write(dir.join("bin"), [0xff, 0xfe, 0, 1, 2, b'[', 0x80]).unwrap();
        let shown = view_of_file(&dir.join("bin"), true);
        assert_eq!(shown["shared_instance"], true);
        assert_eq!(shown["sections"], json!([]));
        std::fs::write(dir.join("ok"), CONFIG).unwrap();
        let shown = view_of_file(&dir.join("ok"), false);
        assert_no_secret(&shown.to_string());
        assert!(!shown.to_string().contains(dir.to_str().unwrap()));
        // Over the size cap: the start is read and the view says so.
        let mut big = CONFIG.to_string();
        big.push_str(&"# padding\n".repeat(120_000));
        big.push_str("[late]\n");
        std::fs::write(dir.join("big"), &big).unwrap();
        let shown = view_of_file(&dir.join("big"), false);
        assert_eq!(shown["truncated"], true);
        assert!(!shown.to_string().contains("late"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn interface_keys(text: &str, name: &str) -> Vec<String> {
        let view = redact(text);
        let section = find(&view, &["interfaces", name]).unwrap();
        section.keys.iter().map(|k| k.key.clone()).collect()
    }

    #[test]
    fn restriction_is_decided_for_every_spelling_and_order() {
        let head = "[interfaces]\n[[P]]\n";
        // Upper-case TYPE and values with "pipe" in any case.
        assert!(interface_keys(&format!("{head} TYPE = PIPEinterface\n port = x\n"), "P").is_empty(), "TYPE is not exact, so not shown");
        // Duplicate keys: the pipe is not undone by a later type.
        assert_eq!(interface_keys(&format!("{head} type = PipeInterface\n port = x\n type = TCPClientInterface\n"), "P"), ["type"]);
        assert_eq!(interface_keys(&format!("{head} type = TCPClientInterface\n type = PipeInterface\n port = x\n"), "P"), ["type"]);
        // Duplicate sections with conflicting types.
        let two = "[interfaces]\n[[P]]\n type = PipeInterface\n command = x\n[[P]]\n type = TCPClientInterface\n target_host = h\n passphrase = p\n";
        assert_eq!(interface_keys(two, "P"), ["type"]);
        let two = "[interfaces]\n[[P]]\n type = TCPClientInterface\n passphrase = p\n[[P]]\n type = PipeInterface\n";
        assert_eq!(interface_keys(two, "P"), ["type"]);
        // The caps can't hide it: 100 keys, then the type or location_cmd.
        let filler: String = (0..100).map(|i| format!(" k{i:03} = 1\n")).collect();
        for tail in [" type = PipeInterface\n", " location_cmd = /bin/x\n", " command = /bin/x\n"] {
            let view = redact(&format!("{head}{filler}{tail}"));
            let p = find(&view, &["interfaces", "P"]).unwrap();
            assert!(p.keys.len() <= 1, "{tail}: {p:?}");
            assert!(view.truncated);
        }
        // A triple-quoted type is restricted, and its text is not shown.
        let view = redact(&format!("{head} type = \"\"\"PipeInterface\"\"\"\n command = x\n enabled = yes\n"));
        let p = find(&view, &["interfaces", "P"]).unwrap();
        let names: Vec<_> = p.keys.iter().map(|k| (k.key.as_str(), k.value.as_deref())).collect();
        assert_eq!(names, [("enabled", Some("yes")), ("type", None)]);
        let view = redact(&format!("{head} type = '''\nTCPClientInterface\n'''\n passphrase = p\n"));
        assert_eq!(find(&view, &["interfaces", "P"]).unwrap().keys.len(), 1);
        // A missing or empty type is restricted too.
        assert!(interface_keys(&format!("{head} passphrase = p\n"), "P").is_empty());
        assert_eq!(interface_keys(&format!("{head} type =\n enabled = yes\n passphrase = p\n"), "P"), ["enabled", "type"]);
        // And a plain one is not.
        assert_eq!(interface_keys(&format!("{head} type = TCPClientInterface\n passphrase = p\n"), "P"), ["passphrase", "type"]);
    }

    #[test]
    fn reachable_on_shows_only_a_plain_host() {
        let host = |value: &str| {
            let view = redact(&format!("[reticulum]\nreachable_on = {value}\n"));
            let k = key(&view.sections[0], "reachable_on").unwrap().clone();
            assert!(k.set, "{value}");
            k.value
        };
        assert_eq!(host("node.example.org").as_deref(), Some("node.example.org"));
        assert_eq!(host("[2001:db8::1]").as_deref(), Some("[2001:db8::1]"));
        assert_eq!(host("10.0.0.1").as_deref(), Some("10.0.0.1"));
        for bad in ["/usr/local/bin/addr.sh", "~/addr.sh", "C:\\\\bin\\\\a.bat", "a b", "a;b", "$(x)", "\"/bin/x\""] {
            assert_eq!(host(bad), None, "{bad}");
        }
        let view = redact("[reticulum]\nreachable_on = /usr/local/bin/addr.sh\n");
        assert!(!view.to_json_with(false, &[]).to_string().contains("addr.sh"));
    }

    #[test]
    fn the_view_is_kept_to_a_byte_budget() {
        let mut text = String::from("[interfaces]\n");
        for i in 0..200 {
            text.push_str(&format!("[[Interface number {i:03} with a long name to take room]]\ntype = TCPClientInterface\n"));
            for k in 0..80 {
                text.push_str(&format!("key_number_{k:03}_with_a_long_name = {}\n", "v".repeat(200)));
            }
            text.push_str(&format!("target_host = {}\n", "h".repeat(200)));
        }
        let view = redact(&text);
        assert!(view.truncated);
        let body = view.to_json_with(false, &[]).to_string();
        assert!(body.len() < 64 * 1024, "{}", body.len());
        assert!(body.len() > 8 * 1024, "something is kept: {}", body.len());
        // What is kept is the start of the sorted view.
        assert_eq!(view.sections[0].path, ["interfaces"]);
        // A small config is whole.
        assert!(!redact(CONFIG).truncated);
    }

    #[test]
    fn what_is_cut_or_unbalanced_is_flagged_or_not_shown() {
        // A long line.
        assert!(redact(&format!("[reticulum]\n# {}\nlisten_ip = 1\n", "x".repeat(5000))).truncated);
        // A long line that opens a triple-quoted value past the cut: the
        // lines after it are not keys.
        let pad = " ".repeat(3000);
        let view = redact(&format!("[reticulum]\nnotes ={pad}\"\"\"\nlisten_port = 1\n\"\"\"\ntarget_port = 2\n"));
        let r = find(&view, &["reticulum"]).unwrap();
        assert!(view.truncated);
        assert!(key(r, "listen_port").is_none());
        assert_eq!(key(r, "target_port").unwrap().value.as_deref(), Some("2"));
        // A key at the far end of a long line is still seen.
        let view =
            redact(&format!("[interfaces]\n[[P]]\ntype = TCPClientInterface\n{}location_cmd = x\n passphrase = p\n", " ".repeat(4000)));
        assert_eq!(find(&view, &["interfaces", "P"]).unwrap().keys.len(), 1);
        // A multi-line value that runs past the line cap.
        let mut text = String::from("[reticulum]\nnotes = \"\"\"\n");
        text.push_str(&"x\n".repeat(MAX_LINES + 10));
        let view = redact(&text);
        assert!(view.truncated);
        // One that never closes.
        assert!(redact("[reticulum]\nnotes = '''\nx\n").truncated);
        assert!(!redact("[reticulum]\nnotes = '''\nx\n'''\n").truncated);
        // An allow-listed value with an open quote shows nothing.
        for open in ["\"abc", "'abc", "a\"bc", "\"a'b"] {
            let view = redact(&format!("[reticulum]\ntarget_host = {open}\n"));
            let k = key(&view.sections[0], "target_host").unwrap();
            assert_eq!((k.value.as_deref(), k.set), (None, true), "{open}");
        }
        let view = redact("[reticulum]\ntarget_host = 'a b'\nlisten_ip = \"x\"\n");
        assert_eq!(key(&view.sections[0], "target_host").unwrap().value, None, "quoted, but not a host");
        assert_eq!(key(&view.sections[0], "listen_ip").unwrap().value, None, "a listen address is an IP address");
        let view = redact("[reticulum]\nlisten_ip = \"10.0.0.1\"\n");
        assert_eq!(key(&view.sections[0], "listen_ip").unwrap().value.as_deref(), Some("10.0.0.1"));
    }

    #[test]
    fn the_json_has_the_documented_shape() {
        let shown = redact("[reticulum]\nenable_transport = yes\npassphrase = x\n").to_json_with(false, &[]);
        assert_eq!(shown["revision"].as_str().unwrap().len(), 32);
        assert_eq!(shown["shared_instance"], false);
        assert_eq!(
            shown["sections"],
            json!([{ "path": ["reticulum"], "keys": [
                { "key": "enable_transport", "value": "yes", "set": true },
                { "key": "passphrase", "value": null, "set": true },
            ] }])
        );
        assert_eq!(shown["editable"], json!([]));
    }

    #[test]
    fn restricted_interfaces_are_marked_and_the_mark_isnt_hashed() {
        let view = redact(CONFIG);
        for (name, restricted) in [("Public TCP", false), ("Radio", false), ("Backbone", false), ("Shell", true), ("Sneaky", true)] {
            assert_eq!(find(&view, &["interfaces", name]).unwrap().restricted, restricted, "{name}");
        }
        assert!(!find(&view, &["reticulum"]).unwrap().restricted);
        let untyped = redact("[interfaces]\n[[A]]\nenabled = yes\n");
        assert!(untyped.sections[1].restricted);
        let mut unmarked = untyped.clone();
        unmarked.sections[1].restricted = false;
        assert_eq!(unmarked.revision(), untyped.revision());
    }

    #[test]
    fn shown_value_is_what_the_view_shows() {
        assert_eq!(shown_value("enabled", "Yes").as_deref(), Some("Yes"));
        assert_eq!(shown_value("enabled", " 'no' ").as_deref(), Some("no"));
        assert_eq!(shown_value("bitrate", "62500").as_deref(), Some("62500"));
        for (key, raw) in [
            ("passphrase", "x"),
            ("enabled", ""),
            ("enabled", "maybe"),
            ("enabled", "'yes"),
            ("mode", "\"\"\"full\"\"\""),
            ("mode", "'''full'''"),
            ("target_host", "user:secret@host"),
        ] {
            assert_eq!(shown_value(key, raw), None, "{key} = {raw}");
        }
    }

    #[test]
    fn a_file_is_read_capped() {
        let dir = temp("capped");
        assert_eq!(read_capped(&dir.join("missing")), None);
        std::fs::write(dir.join("small"), "[reticulum]\n").unwrap();
        assert_eq!(read_capped(&dir.join("small")), Some(("[reticulum]\n".to_string(), false)));
        std::fs::write(dir.join("big"), "x".repeat(MAX_FILE as usize + 10)).unwrap();
        let (text, over) = read_capped(&dir.join("big")).unwrap();
        assert_eq!((text.len(), over), (MAX_FILE as usize, true));
        assert!(view_of(&text, over).truncated);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reachable_on_that_isnt_a_host_restricts_its_interface() {
        for value in ["/usr/local/bin/where-am-i", "~/where.sh", "'./where'", "a b", "host:4242", "'''x'''", "\"\"\"\nx\n\"\"\""] {
            for key in REACHABLE {
                let text =
                    format!("[interfaces]\n[[S]]\n type = TCPServerInterface\n enabled = no\n listen_port = 4242\n {key} = {value}\n");
                let view = redact(&text);
                let s = find(&view, &["interfaces", "S"]).unwrap();
                assert!(s.restricted, "{key} = {value}");
                let names: Vec<&str> = s.keys.iter().map(|k| k.key.as_str()).collect();
                assert_eq!(names, ["enabled", "type"], "{key} = {value}");
            }
        }
        // A host name, an address or nothing is fine.
        for value in ["node.example.org", "203.0.113.7", "[2001:db8::1]", "2001:db8::1", "''", ""] {
            let text = format!("[interfaces]\n[[S]]\n type = TCPServerInterface\n reachable_on = {value}\n");
            assert!(!find(&redact(&text), &["interfaces", "S"]).unwrap().restricted, "{value}");
        }
        // In any case of the key.
        let view = redact("[interfaces]\n[[S]]\n type = TCPServerInterface\n Reachable_On = /bin/x\n");
        assert!(find(&view, &["interfaces", "S"]).unwrap().restricted);
        assert!(host_shaped("node.example.org") && !host_shaped("/bin/x"));
    }

    #[test]
    fn headers_are_read_as_the_editor_reads_them() {
        assert_eq!(header_of("[[A]]"), Some((2, "A".into())));
        assert_eq!(header_of("[[ 'A ' ]]"), Some((2, "A".into())));
        assert_eq!(header_of("[[\u{1b}]]"), Some((2, "\\u{1b}".into())));
        for bad in ["[]", "[[A]", "[[A]] x", "[x] = 1", "[[[[[deep]]]]]", "[[ ]]"] {
            assert_eq!(header_of(bad), None, "{bad:?}");
        }
    }
}
