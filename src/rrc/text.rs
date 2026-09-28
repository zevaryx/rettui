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

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Characters that join the parts of a name (`o'brien`, `bob-smith`,
/// `zev.bot`) but can also be punctuation after one (`@zev.`, `@zev's`).
fn is_joiner(c: char) -> bool {
    matches!(c, '-' | '\'' | '\u{2019}' | '.')
}

/// Whether `rest`, the text after a name, leaves it whole: `@zev` is not
/// the start of `@zevaryx` or `@zev-bot`, but is `zev` in `@zev's`,
/// `@zev.` and `@zev-`.
fn ends_name(rest: &str) -> bool {
    let mut chars = rest.chars();
    match chars.next() {
        Some(c) if is_word(c) => false,
        Some(c) if is_joiner(c) => match chars.next() {
            // A possessive: 's and then the end of the word.
            Some('s' | 'S') if matches!(c, '\'' | '\u{2019}') => !chars.next().is_some_and(is_word),
            Some(next) => !is_word(next),
            None => true,
        },
        _ => true,
    }
}

/// Whether an `@` at byte `at` starts a word (not an address like a@b).
fn starts_mention(text: &str, at: usize) -> bool {
    !text[..at].chars().next_back().is_some_and(is_word)
}

/// Where a mention of `needle` (a lowercased nick) starting with the `@` at
/// byte `at` ends, if the text there is that nick as a whole name.
fn mention_end(text: &str, at: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() || !starts_mention(text, at) {
        return None;
    }
    // Compare the following characters, lowercased, with the nick.
    let mut end = at + 1;
    let mut matched = 0;
    let mut rest = text[end..].chars();
    while matched < needle.len() {
        let c = rest.next()?;
        let lower: Vec<char> = c.to_lowercase().collect();
        if needle.get(matched..matched + lower.len()) != Some(&lower[..]) {
            return None;
        }
        matched += lower.len();
        end += c.len_utf8();
    }
    ends_name(&text[end..]).then_some(end)
}

fn lowercase(nick: &str) -> Vec<char> {
    nick.chars().flat_map(char::to_lowercase).collect()
}

/// Byte ranges of each `@nick` mention (as a whole word, any case) in `text`.
pub fn mention_ranges(text: &str, nick: &str) -> Vec<(usize, usize)> {
    let needle = lowercase(nick);
    text.match_indices('@')
        .filter_map(|(at, _)| Some((at, mention_end(text, at, &needle)?)))
        .collect()
}

/// Byte ranges of mentions of any of `names` in `text`, each with the index
/// of the name. Where several names match (`@ann` and `@ann b`), the
/// longest wins.
pub fn user_mentions(text: &str, names: &[&str]) -> Vec<(usize, usize, usize)> {
    if !text.contains('@') {
        return Vec::new();
    }
    let needles: Vec<Vec<char>> = names.iter().map(|n| lowercase(n)).collect();
    let mut found = Vec::new();
    let mut after = 0;
    for (at, _) in text.match_indices('@') {
        if at < after {
            continue;
        }
        let longest = needles
            .iter()
            .enumerate()
            .filter_map(|(i, needle)| Some((mention_end(text, at, needle)?, i)))
            .max_by_key(|(end, _)| *end);
        if let Some((end, i)) = longest {
            found.push((at, end, i));
            after = end;
        }
    }
    found
}

/// The `@name` being typed just before the cursor (`before` is the text up
/// to it): the byte where its `@` is, and what follows it. The `@` starts a
/// word (so addresses like a@b don't count). Names can have spaces
/// (`@ann b`), so what follows may too, but not at its end: after a space
/// it's up to the next letter whether a longer name is meant.
pub fn mention_prefix(before: &str) -> Option<(usize, &str)> {
    // The last `@` that starts a word: an `@` inside a name (`@a@b`) doesn't.
    let (at, _) = before.rmatch_indices('@').find(|&(at, _)| starts_mention(before, at))?;
    let partial = &before[at + 1..];
    (!partial.ends_with(char::is_whitespace)).then_some((at, partial))
}

/// `names` matching a partly typed one: those starting with it first, then
/// those containing it, ignoring case (each in the order given). Once it
/// has a space, only names starting with it (it's no longer a search).
pub fn complete_names<'a, T>(names: &'a [(String, T)], partial: &str) -> Vec<&'a (String, T)> {
    let partial = partial.to_lowercase();
    let search = !partial.contains(char::is_whitespace);
    let (mut starts, mut contains) = (Vec::new(), Vec::new());
    for entry in names {
        let name = entry.0.to_lowercase();
        if name.starts_with(&partial) {
            starts.push(entry);
        } else if search && name.contains(&partial) {
            contains.push(entry);
        }
    }
    starts.extend(contains);
    starts
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
        // Any of several users: the longest name wins, case doesn't matter.
        let names = ["ann", "Ann B", "bob"];
        assert_eq!(user_mentions("@ann b and @BOB, @ann", &names), [(0, 6, 1), (11, 15, 2), (17, 21, 0)]);
        assert!(user_mentions("@annie me@bob", &names).is_empty());
        assert!(user_mentions("no mentions", &names).is_empty());
        // What is being typed at the cursor.
        assert_eq!(mention_prefix("hi @an"), Some((3, "an")));
        assert_eq!(mention_prefix("@"), Some((0, "")));
        assert_eq!(mention_prefix("mail me@an"), None);
        assert_eq!(mention_prefix("hi @ann "), None);
        assert_eq!(mention_prefix("hi @ann b"), Some((3, "ann b")));
        assert_eq!(mention_prefix("(@o'br"), Some((1, "o'br")));
        assert_eq!(mention_prefix("@a@b"), Some((0, "a@b")));
        let people = [("Bob".to_string(), 1), ("annie".to_string(), 2), ("Joanna".to_string(), 3), ("zed".to_string(), 4)];
        let names: Vec<&str> = complete_names(&people, "AN").iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["annie", "Joanna"]);
        assert_eq!(complete_names(&people, "").len(), 4);
        // With a space it's a name being finished, not a search.
        let people = [("ann b".to_string(), 1), ("joann b".to_string(), 2)];
        assert_eq!(complete_names(&people, "ann b").len(), 1);
        assert_eq!(complete_names(&people, "nn").len(), 2);
        let parts = split_message(&"word ".repeat(100), 50);
        assert!(parts.iter().all(|p| p.len() <= 50));
        assert_eq!(parts.join(" ").split(' ').count(), 100);
        // Never splits inside a multi-byte character.
        let parts = split_message(&"é".repeat(30), 7);
        assert!(parts.iter().all(|p| p.len() <= 7 && !p.is_empty()));
    }

    #[test]
    fn names_with_punctuation() {
        // Joiners inside a name are part of it.
        assert_eq!(mention_ranges("hi @o'brien!", "o'brien"), [(3, 11)]);
        assert_eq!(mention_ranges("hi @Bob-Smith, @bob.jr", "bob-smith"), [(3, 13)]);
        assert_eq!(mention_ranges("hi @bob.jr", "bob.jr"), [(3, 10)]);
        // A shorter name isn't the start of a longer one...
        assert!(mention_ranges("@zev-bot @zev.bot @zev'sbot @zev_2 @zev-2", "zev").is_empty());
        // ...but punctuation after a name leaves it whole.
        for text in ["@zev.", "@zev-", "@zev's idea", "@zev\u{2019}s", "@zev, hi", "(@zev)", "@zev...", "@zev!", "@zev: hi", "\"@zev\""] {
            assert_eq!(mention_ranges(text, "zev").len(), 1, "{text}");
        }
        // Names ending or starting in punctuation, other scripts and emoji.
        assert_eq!(mention_ranges("@-dash- hi", "-dash-"), [(0, 7)]);
        assert_eq!(mention_ranges("yo @zev. ok", "zev."), [(3, 8)]);
        assert_eq!(mention_ranges("@José, @ÉLODIE", "élodie"), [(8, 16)]);
        assert_eq!(mention_ranges("@rat🐀 hi", "rat🐀"), [(0, 8)]);
        assert_eq!(mention_ranges("@名前さん", "名前さん"), [(0, 13)]);
        // Several users: each whole, the longest winning.
        let names = ["bob", "bob-smith", "o'brien", "ann b"];
        assert_eq!(
            user_mentions("@bob-smith, @bob's, @O'Brien and @ann b.", &names),
            [(0, 10, 1), (12, 16, 0), (20, 28, 2), (33, 39, 3)]
        );
        assert!(user_mentions("@bob-jones @bobby", &names).is_empty());
        assert_eq!(user_mentions("hi @a@b!", &["a@b", "b"]), [(3, 7, 0)]);
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
