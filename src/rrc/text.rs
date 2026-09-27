//! Room and nick rules, mentions, message splitting and `rrc://` links.

use super::envelope::DEFAULT_ASPECT;

/// Rooms are case-insensitive; `#` is decoration.
pub fn normalize_room(room: &str) -> String {
    room.trim().trim_start_matches('#').trim().to_lowercase()
}

/// A nick the hub will accept, or `None`.
pub fn normalize_nick(nick: &str, max_bytes: usize) -> Option<String> {
    let nick = nick.trim();
    (!nick.is_empty() && nick.len() <= max_bytes && !nick.contains(['\n', '\r', '\0']))
        .then(|| nick.to_string())
}

/// Byte ranges of each `@nick` mention (as a whole word, any case) in `text`.
pub fn mention_ranges(text: &str, nick: &str) -> Vec<(usize, usize)> {
    if nick.is_empty() {
        return Vec::new();
    }
    let needle: Vec<char> = nick.chars().flat_map(char::to_lowercase).collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut ranges = Vec::new();
    for (at, _) in text.match_indices('@') {
        // Compare the following characters, lowercased, with the nick.
        let mut end = at + 1;
        let mut matched = 0;
        let mut rest = text[end..].chars();
        while matched < needle.len() {
            let Some(c) = rest.next() else { break };
            let lower: Vec<char> = c.to_lowercase().collect();
            if needle.get(matched..matched + lower.len()) != Some(&lower[..]) {
                break;
            }
            matched += lower.len();
            end += c.len_utf8();
        }
        let before = text[..at].chars().next_back();
        let after = text[end..].chars().next();
        if matched == needle.len() && !before.is_some_and(word) && !after.is_some_and(word) {
            ranges.push((at, end));
        }
    }
    ranges
}

/// Split text into parts of at most `limit` UTF-8 bytes, preferring spaces.
pub fn split_message(text: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(4);
    let mut parts = Vec::new();
    let mut rest = text.trim();
    while rest.len() > limit {
        let mut cut = limit;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        if let Some(space) = rest[..cut].rfind(' ').filter(|s| *s > limit / 2) {
            cut = space;
        }
        parts.push(rest[..cut].trim_end().to_string());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        parts.push(rest.to_string());
    }
    parts
}

/// `rrc://<hash>[:<aspect>]/<room>`, `rrc@<hash>[/<room>]` or a bare hash.
pub fn parse_link(link: &str) -> Option<(crate::net::Hash, String, Option<String>)> {
    let rest = link
        .trim()
        .strip_prefix("rrc://")
        .or_else(|| link.trim().strip_prefix("rrc@"))
        .unwrap_or(link.trim())
        .trim_start_matches('/');
    let (hub, room) = rest.split_once('/').unwrap_or((rest, ""));
    let (hex, aspect) = hub.split_once(':').unwrap_or((hub, ""));
    let hash = crate::net::parse_hash(hex)?;
    let aspect = if aspect.trim().is_empty() { DEFAULT_ASPECT } else { aspect.trim() };
    let room = normalize_room(room);
    Some((hash, aspect.to_string(), (!room.is_empty()).then_some(room)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_and_splitting() {
        assert!(!mention_ranges("hey @Zev, look", "zev").is_empty());
        assert!(mention_ranges("hey @zevaryx", "zev").is_empty());
        assert!(mention_ranges("mail me@zev.example", "zev").is_empty());
        assert_eq!(mention_ranges("hi @ZEV and @zev!", "zev"), [(3, 7), (12, 16)]);
        assert_eq!(mention_ranges("é @Ünï ok", "ünï"), [(3, 9)]);
        assert!(mention_ranges("@zevaryx", "zev").is_empty());
        let parts = split_message(&"word ".repeat(100), 50);
        assert!(parts.iter().all(|p| p.len() <= 50));
        assert_eq!(parts.join(" ").split(' ').count(), 100);
        // Never splits inside a multi-byte character.
        let parts = split_message(&"é".repeat(30), 7);
        assert!(parts.iter().all(|p| p.len() <= 7 && !p.is_empty()));
    }

    #[test]
    fn links() {
        let hash = "d765e919676aa0340412a1afae006553";
        let (h, aspect, room) = parse_link(&format!("rrc://{hash}/General")).unwrap();
        assert_eq!((hex::encode(h), aspect.as_str(), room.as_deref()), (hash.to_string(), DEFAULT_ASPECT, Some("general")));
        let (_, aspect, room) = parse_link(&format!("rrc://{hash}:rrc.hub.test")).unwrap();
        assert_eq!((aspect.as_str(), room), ("rrc.hub.test", None));
        assert!(parse_link("rrc://nothex/x").is_none());
    }
}
