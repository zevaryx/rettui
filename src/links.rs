//! Addresses written in chat, found so both UIs can link them: NomadNet
//! pages and files (`<node>:/page/index.mu`, also after `nomadnetwork://`),
//! LXMF addresses (`lxmf@<address>`, `lxmf://<address>`, and `lxma://`
//! links with a public key) and RRC hubs (`rrc://<hub>/room`, `rrc@<hub>`).
//!
//! A bare hash isn't linked: it could be any destination. What a link ends
//! with is trimmed of the punctuation a sentence puts after it. The web UI
//! has the same rules in `findLinks` (`app.js`).

/// What a link opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// A page or file, in the Browser.
    Nomad,
    /// A conversation (an `lxma://` link brings the public key too).
    Lxmf,
    /// A hub, and maybe a room on it.
    Rrc,
}

/// A link found in some text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Where it is (byte offsets).
    pub start: usize,
    pub end: usize,
    pub kind: LinkKind,
    /// What opening it opens: the text as written, but for a page or file
    /// without `nomadnetwork://`.
    pub url: String,
}

/// Punctuation a sentence may put after a link, which isn't part of it.
const TRAILING: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '\'', '"', '*', '_', '`'];

/// Every link in `text`, in order.
pub fn find(text: &str) -> Vec<Found> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        // A link starts a word.
        let starts_word = at == 0 || !is_word(bytes[at - 1]);
        if starts_word && let Some(link) = link_at(text, at) {
            at = link.end;
            found.push(link);
            continue;
        }
        at += text[at..].chars().next().map_or(1, char::len_utf8);
    }
    found
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `text` from `at` starts with `prefix` (in any case).
fn has_prefix(text: &str, at: usize, prefix: &str) -> bool {
    text.get(at..at + prefix.len()).is_some_and(|s| s.eq_ignore_ascii_case(prefix))
}

/// 32 hex digits at `at`, and no more: where they end.
fn hash_at(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let end = at + 32;
    let hex = bytes.get(at..end)?;
    (hex.iter().all(u8::is_ascii_hexdigit) && bytes.get(end).is_none_or(|b| !b.is_ascii_alphanumeric())).then_some(end)
}

/// Where a run of characters that aren't whitespace ends, from `at`, less
/// the punctuation after it (never before `min`).
fn run_end(text: &str, at: usize, min: usize) -> usize {
    let end = text[at..].find(char::is_whitespace).map_or(text.len(), |i| at + i);
    let trimmed = text[min..end].trim_end_matches(TRAILING);
    min + trimmed.len()
}

fn link_at(text: &str, at: usize) -> Option<Found> {
    let found = |end: usize, kind: LinkKind, url: &str| Some(Found { start: at, end, kind, url: url.to_string() });
    // RRC: a hub, maybe its aspect, maybe a room.
    for prefix in ["rrc://", "rrc@"] {
        if has_prefix(text, at, prefix) {
            let hash_end = hash_at(text, at + prefix.len())?;
            let follows = text[hash_end..].chars().next();
            let end = if matches!(follows, Some(':' | '/')) { run_end(text, hash_end, hash_end) } else { hash_end };
            return found(end, LinkKind::Rrc, &text[at..end]);
        }
    }
    // An address with its public key.
    if has_prefix(text, at, "lxma://") {
        let hash_end = hash_at(text, at + 7)?;
        let key = hash_end + 1;
        let bytes = text.as_bytes();
        if !matches!(bytes.get(hash_end), Some(b':' | b'/')) {
            return None;
        }
        let key_end = key + 128;
        if !bytes.get(key..key_end)?.iter().all(u8::is_ascii_hexdigit) || bytes.get(key_end).is_some_and(u8::is_ascii_alphanumeric) {
            return None;
        }
        return found(key_end, LinkKind::Lxmf, &text[at..key_end]);
    }
    for prefix in ["lxmf://", "lxmf@"] {
        if has_prefix(text, at, prefix) {
            let end = hash_at(text, at + prefix.len())?;
            return found(end, LinkKind::Lxmf, &text[at..end]);
        }
    }
    // A page or file: `<node>:/page/…` or `:/file/…`.
    let node = if has_prefix(text, at, "nomadnetwork://") { at + 15 } else { at };
    let hash_end = hash_at(text, node)?;
    let route = ["page/", "file/"].iter().find(|route| text[hash_end..].starts_with(&format!(":/{route}")))?;
    let path = hash_end + 2 + route.len();
    let end = run_end(text, path, path);
    (end > path).then(|| Found { start: at, end, kind: LinkKind::Nomad, url: text[node..end].to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODE: &str = "c0ffee00c0ffee00c0ffee00c0ffee00";

    fn urls(text: &str) -> Vec<(LinkKind, String)> {
        find(text).into_iter().map(|f| (f.kind, f.url)).collect()
    }

    #[test]
    fn pages_addresses_and_hubs_in_what_people_write() {
        let key = "ab".repeat(64);
        let text = format!(
            "See {NODE}:/page/index.mu, or nomadnetwork://{NODE}:/file/manual.pdf! Write to lxmf@{NODE} (or lxma://{NODE}:{key}). \
             Chat: rrc://{NODE}/lobby. And rrc@{NODE}."
        );
        assert_eq!(
            urls(&text),
            [
                (LinkKind::Nomad, format!("{NODE}:/page/index.mu")),
                (LinkKind::Nomad, format!("{NODE}:/file/manual.pdf")),
                (LinkKind::Lxmf, format!("lxmf@{NODE}")),
                (LinkKind::Lxmf, format!("lxma://{NODE}:{key}")),
                (LinkKind::Rrc, format!("rrc://{NODE}/lobby")),
                (LinkKind::Rrc, format!("rrc@{NODE}")),
            ]
        );
        // Where each is, as written (with nomadnetwork://).
        let second = &find(&text)[1];
        assert_eq!(&text[second.start..second.end], format!("nomadnetwork://{NODE}:/file/manual.pdf"));
        // Paths keep what's in them, sentences lose what's after.
        assert_eq!(urls(&format!("({NODE}:/page/a/b.mu`q=1)."))[0].1, format!("{NODE}:/page/a/b.mu`q=1"));
        assert_eq!(urls(&format!("LXMF://{}", NODE.to_uppercase()))[0].1, format!("LXMF://{}", NODE.to_uppercase()));
    }

    #[test]
    fn not_links() {
        // A bare hash, a hash too long or short, inside a word, no path.
        for text in [
            NODE.to_string(),
            format!("lxmf@{NODE}0"),
            format!("lxmf@{}", &NODE[1..]),
            format!("xlxmf@{NODE}"),
            format!("{NODE}:/page/"),
            format!("{NODE}:/other/x"),
            format!("a{NODE}:/page/x.mu"),
            format!("lxma://{NODE}:short"),
            "rrc://nothex".to_string(),
        ] {
            assert!(find(&text).is_empty(), "{text}");
        }
        // Next to non-ASCII text, still found.
        assert_eq!(urls(&format!("é→lxmf@{NODE}✓")).len(), 1);
    }
}
