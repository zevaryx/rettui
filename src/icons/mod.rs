//! A person's icon, as Sideband, Columba and MeshChat show one: a Material
//! Design Icon by name, in a colour on a colour (LXMF's icon appearance
//! field). Each name's character is where Nerd Font has the icon, so the
//! web UI shows it with the font it bundles (see `README.md`).

use std::collections::HashMap;
use std::sync::LazyLock;

/// `name code` lines, by name (see `README.md`).
static TABLE: &str = include_str!("mdi.txt");

static ICONS: LazyLock<HashMap<&'static str, char>> = LazyLock::new(|| {
    TABLE
        .lines()
        .filter_map(|line| {
            let (name, code) = line.split_once(' ')?;
            Some((name, char::from_u32(u32::from_str_radix(code, 16).ok()?)?))
        })
        .collect()
});

/// Where to find an icon's name.
pub const LIBRARY_URL: &str = "https://pictogrammers.com/library/mdi/";

/// Longest icon name taken (the longest is about 40 characters).
pub const MAX_NAME: usize = 64;

/// The character of the icon called `name`, if there's one.
pub fn glyph(name: &str) -> Option<char> {
    ICONS.get(name).copied()
}

/// Icons whose names hold every word of `query` (in any order), shortest
/// names first, at most `limit`.
pub fn search(query: &str, limit: usize) -> Vec<(&'static str, char)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<(&'static str, char)> = ICONS
        .iter()
        .filter(|(name, _)| words.iter().all(|w| name.contains(w.as_str())))
        .map(|(name, glyph)| (*name, *glyph))
        .collect();
    found.sort_by_key(|(name, _)| (name.len(), *name));
    found.truncate(limit);
    found
}

/// A colour as `#rrggbb`.
pub fn hex_colour(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// A colour from `#rrggbb` (or `rrggbb`, or `#rgb`).
pub fn parse_colour(text: &str) -> Option<[u8; 3]> {
    let text = text.trim().trim_start_matches('#');
    let digits: Vec<u8> = match text.len() {
        3 => text.chars().map(|c| c.to_digit(16).map(|d| d as u8 * 17)).collect::<Option<_>>()?,
        6 => (0..3).map(|i| u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()).collect::<Option<_>>()?,
        _ => return None,
    };
    digits.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_find_their_icons() {
        assert_eq!(glyph("account"), Some('\u{F0004}'));
        assert_eq!(glyph("radio-tower"), Some('\u{F043B}'));
        assert_eq!(glyph("no-such-icon"), None);
        assert!(ICONS.len() > 6000);
        let found = search("radio tower", 10);
        assert_eq!(found.first().map(|(n, _)| *n), Some("radio-tower"));
        assert!(found.iter().all(|(n, _)| n.contains("radio") && n.contains("tower")));
        assert_eq!(search("account", 3).len(), 3);
        assert_eq!(search("account", 3)[0].0, "account");
    }

    #[test]
    fn colours_both_ways() {
        assert_eq!(parse_colour("#1a2B3c"), Some([0x1a, 0x2b, 0x3c]));
        assert_eq!(parse_colour("fff"), Some([255, 255, 255]));
        assert_eq!(parse_colour("#12345"), None);
        assert_eq!(parse_colour("#gg0000"), None);
        assert_eq!(hex_colour([0x1a, 0x2b, 0x3c]), "#1a2b3c");
    }
}
