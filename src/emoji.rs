//! Emoji for the pickers in both UIs: Unicode's list (its names, groups and
//! order, from the `emojis` crate) with GitHub's shortcodes, searched by
//! either, and the ones used lately.

use std::sync::LazyLock;

use emojis::Emoji;

/// The newest Unicode version whose emoji are offered. Systems draw newer
/// ones as boxes, or as their parts side by side, until their fonts catch up.
const NEWEST: (u32, u32) = (15, 0);

/// How many recently used emoji are kept.
pub const RECENT: usize = 30;

pub struct Group {
    pub name: &'static str,
    /// The emoji on its tab.
    pub icon: &'static str,
    pub emoji: Vec<&'static Emoji>,
}

/// Each emoji offered, with the words it's found by (lowercase): those of
/// its name and its shortcodes, and each shortcode whole (`+1`).
struct Indexed {
    emoji: &'static Emoji,
    words: Vec<String>,
}

static GROUPS: LazyLock<Vec<Group>> = LazyLock::new(|| {
    use emojis::Group as G;
    emojis::Group::iter()
        .map(|group| {
            let (name, icon) = match group {
                G::SmileysAndEmotion => ("Smileys & emotion", "😀"),
                G::PeopleAndBody => ("People & body", "👋"),
                G::AnimalsAndNature => ("Animals & nature", "🐻"),
                G::FoodAndDrink => ("Food & drink", "🍔"),
                G::TravelAndPlaces => ("Travel & places", "🚗"),
                G::Activities => ("Activities", "⚽"),
                G::Objects => ("Objects", "💡"),
                G::Symbols => ("Symbols", "🔣"),
                G::Flags => ("Flags", "🏁"),
            };
            Group { name, icon, emoji: group.emojis().filter(|e| offered(e)).collect() }
        })
        .collect()
});

static INDEX: LazyLock<Vec<Indexed>> = LazyLock::new(|| {
    GROUPS
        .iter()
        .flat_map(|group| &group.emoji)
        .map(|&emoji| {
            let name = emoji.name().to_lowercase();
            let mut words: Vec<String> = split(&name).map(str::to_string).collect();
            for code in emoji.shortcodes() {
                words.push(code.to_string());
                words.extend(split(code).map(str::to_string));
            }
            Indexed { emoji, words }
        })
        .collect()
});

fn offered(emoji: &Emoji) -> bool {
    let version = emoji.unicode_version();
    (version.major(), version.minor()) <= NEWEST
}

fn split(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty())
}

/// The groups, in Unicode's order (smileys first, flags last).
pub fn groups() -> &'static [Group] {
    &GROUPS
}

/// Emoji matching `query`, best first. Each of its words must start a word
/// of an emoji's name or shortcodes (`thumbs up`, `+1`, `:tada:`); emoji
/// with the query as a shortcode come first, then those with a shortcode
/// starting with it, then the rest in Unicode's order.
pub fn search(query: &str) -> Vec<&'static Emoji> {
    let query = query.trim().trim_matches(':').to_lowercase();
    let terms: Vec<&str> = query.split(|c: char| c.is_whitespace() || c == '_').filter(|t| !t.is_empty()).collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let code = terms.join("_");
    let mut found: Vec<((u8, usize), &'static Emoji)> = INDEX
        .iter()
        .filter(|entry| terms.iter().all(|term| entry.words.iter().any(|word| word.starts_with(term))))
        .map(|entry| {
            let codes = entry.emoji.shortcodes();
            // Shorter shortcodes first (`dragon` before `dragon_face`).
            let rank = match codes.filter(|c| c.starts_with(&code)).map(str::len).min() {
                Some(len) if len == code.len() => (0, 0),
                Some(len) => (1, len),
                None => (2, 0),
            };
            (rank, entry.emoji)
        })
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found.into_iter().map(|(_, emoji)| emoji).collect()
}

/// The emoji `text` is, if it's one offered.
pub fn get(text: &str) -> Option<&'static Emoji> {
    emojis::get(text).filter(|e| offered(e))
}

/// Most emoji the `:name` list shows at once.
pub const SHORTCODE_ROWS: usize = 8;

/// The `:name` being typed at the end of `before` (the text before the
/// cursor): the byte its `:` is at, and the name so far. The `:` starts a
/// word (so not `12:30` or `http://`), and the name is a shortcode's
/// characters, at least `min` of them (`:)` and `:D` aren't names).
fn shortcode_at(before: &str, min: usize) -> Option<(usize, &str)> {
    let at = before.rfind(':')?;
    let name = &before[at + 1..];
    let starts_word = before[..at].chars().next_back().is_none_or(char::is_whitespace);
    let named = name.len() >= min && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-'));
    (starts_word && named).then_some((at, name))
}

/// While typing `:name`: where its `:` is in `before` (the text before the
/// cursor) and the emoji it could be, best first, at most
/// [`SHORTCODE_ROWS`]. `None` with fewer than two characters typed, or no
/// emoji to match.
pub fn completions(before: &str) -> Option<(usize, Vec<&'static Emoji>)> {
    let (at, name) = shortcode_at(before, 2)?;
    let mut found = search(name);
    found.truncate(SHORTCODE_ROWS);
    (!found.is_empty()).then_some((at, found))
}

/// A `:name:` just finished at the end of `before`: where its first `:` is,
/// and the emoji it names.
pub fn finished(before: &str) -> Option<(usize, &'static Emoji)> {
    let (at, name) = shortcode_at(before.strip_suffix(':')?, 1)?;
    let emoji = emojis::get_by_shortcode(&name.to_ascii_lowercase()).filter(|e| offered(e))?;
    Some((at, emoji))
}

/// The shortcode to show for `emoji` in a list for `typed`: the first
/// starting with what was typed, else its first.
pub fn shortcode_for(emoji: &Emoji, typed: &str) -> Option<&'static str> {
    let typed = typed.trim_matches(':').to_ascii_lowercase();
    let emoji: &'static Emoji = emojis::get(emoji.as_str())?;
    emoji.shortcodes().find(|c| c.starts_with(&typed)).or_else(|| emoji.shortcode())
}

/// Put `emoji` first among the recently used, keeping the newest [`RECENT`].
pub fn remember(recent: &mut Vec<String>, emoji: &str) {
    recent.retain(|e| e != emoji);
    recent.insert(0, emoji.to_string());
    recent.truncate(RECENT);
}

/// The recently used emoji, newest first (any no longer offered left out).
pub fn recent(recent: &[String]) -> Vec<&'static Emoji> {
    recent.iter().filter_map(|e| get(e)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(query: &str) -> Vec<&'static str> {
        search(query).iter().map(|e| e.as_str()).collect()
    }

    #[test]
    fn emoji_are_found_by_name_and_shortcode() {
        // A shortcode that is the query comes first.
        assert_eq!(found("+1")[0], "👍");
        assert_eq!(found(":tada:")[0], "🎉");
        assert_eq!(found("thumbs up")[0], "👍");
        // Every word has to match the start of one.
        assert!(found("heart").contains(&"❤️"));
        assert!(found("red heart").contains(&"❤️"));
        assert!(!found("blue heart").contains(&"❤️"));
        // Words are matched from their start.
        assert!(found("ear").contains(&"👂") && !found("ear").contains(&"❤️"));
        assert!(found("zqx").is_empty());
        assert!(found("  ").is_empty());
        // Flags by country name or code.
        assert!(found("flag united states").contains(&"🇺🇸"));
    }

    #[test]
    fn names_after_a_colon_are_completed() {
        let (at, found) = completions("here be :dra").unwrap();
        assert_eq!(at, 8);
        assert_eq!(found[0].as_str(), "🐉");
        assert_eq!(shortcode_for(found[0], "dra"), Some("dragon"));
        assert!(found.len() <= SHORTCODE_ROWS);
        // A word of the name finds it too, shown by its shortcode.
        let (_, found) = completions(":thumbs").unwrap();
        assert!(found.iter().any(|e| e.as_str() == "👍"));
        assert_eq!(shortcode_for(emojis::get("👍").unwrap(), "thumbs"), Some("thumbsup"));
        // Not inside a word, a time or an address; not a smiley; not
        // with fewer than two characters; not once a space follows.
        for text in ["dra:gon", "at 12:30", "http://x", "ok :)", ":D", ":d", ":dragon ", ""] {
            assert!(completions(text).is_none(), "{text:?}");
        }
        // Finished, it's the emoji, if a shortcode is what it names.
        assert_eq!(finished("a :dragon:").map(|(at, e)| (at, e.as_str())), Some((2, "🐉")));
        assert_eq!(finished(":+1:").map(|(_, e)| e.as_str()), Some("👍"));
        assert_eq!(finished(":Dragon:").map(|(_, e)| e.as_str()), Some("🐉"));
        assert!(finished(":dragons:").is_none());
        assert!(finished("12:30:").is_none());
        assert!(finished(":dragon").is_none());
    }

    #[test]
    fn only_emoji_systems_can_draw_are_offered() {
        let all: Vec<&Emoji> = groups().iter().flat_map(|g| g.emoji.iter().copied()).collect();
        assert!(all.len() > 1800);
        assert!(all.iter().all(|e| offered(e)));
        // Unicode 15.1's phoenix is left out.
        assert!(get("🐦‍🔥").is_none());
        assert_eq!(groups().len(), 9);
    }

    #[test]
    fn recent_emoji_are_newest_first_and_kept_short() {
        let mut recent = Vec::new();
        for emoji in ["😀", "👍", "😀"] {
            remember(&mut recent, emoji);
        }
        assert_eq!(recent, ["😀", "👍"]);
        for emoji in groups()[0].emoji.iter().take(40) {
            remember(&mut recent, emoji.as_str());
        }
        assert_eq!(recent.len(), RECENT);
        // Anything that isn't an emoji offered is left out.
        recent.insert(0, "x".into());
        assert_eq!(super::recent(&recent).len(), RECENT);
    }
}
