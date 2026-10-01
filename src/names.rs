//! Names others choose, from their announces, cleaned for showing: what
//! could pass one name off as another, or bury the screen, goes. As
//! NomadNet does (and lxmf-core's `presentation`, which this follows for
//! which characters hide), but emoji stay: rettui shows them in names.
//!
//! - NFKC: fullwidth and other compatibility forms read as the letters
//!   they look like (`ＡＤＭＩＮ` is `ADMIN`).
//! - Gone: control characters, direction overrides (which can show a name
//!   backwards), zero-width and other invisible ones, private-use
//!   characters (icons only some fonts draw), and tag characters outside a
//!   flag.
//! - At most [`MAX_MARKS`] accents on one letter, so "Zalgo" text doesn't
//!   spill over the lines around it.
//! - Blank-looking characters (Hangul fillers, the blank Braille pattern)
//!   and every kind of space or line break are one space; none lead or
//!   trail.
//! - At most [`MAX_CHARS`] characters.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Characters kept of a name.
pub const MAX_CHARS: usize = 128;
/// Combining marks kept on one character (scripts that stack them, such as
/// Thai or Vietnamese, use two or three).
pub const MAX_MARKS: usize = 4;

/// A name from an announce, cleaned; `None` if nothing is left.
pub fn clean(name: &str) -> Option<String> {
    let mut out = String::new();
    let (mut kept, mut marks, mut space) = (0, 0, false);
    // The last character kept, for tag characters (only kept in a flag
    // such as 🏴󠁧󠁢󠁳󠁣󠁴󠁿, after the black flag or another tag).
    let mut last: Option<char> = None;
    for c in name.nfkc() {
        let code = c as u32;
        if c.is_whitespace() || is_blank(code) {
            space = !out.is_empty();
            continue;
        }
        let tag = (0xE0020..=0xE007F).contains(&code);
        let in_flag = last.is_some_and(|last| last == '\u{1F3F4}' || (0xE0020..=0xE007F).contains(&(last as u32)));
        if c.is_control() || is_hidden(code) || (tag && !in_flag) {
            continue;
        }
        if is_combining_mark(c) {
            marks += 1;
            if marks > MAX_MARKS || last.is_none() {
                continue;
            }
        } else {
            marks = 0;
        }
        if space {
            if kept + 2 > MAX_CHARS {
                break;
            }
            out.push(' ');
            kept += 1;
            space = false;
        }
        if kept == MAX_CHARS {
            break;
        }
        out.push(c);
        kept += 1;
        last = Some(c);
    }
    (!out.is_empty()).then_some(out)
}

/// Characters that hide, or change how the text around them shows:
/// direction controls, zero-width ones (not the joiners emoji and some
/// scripts need), invisible operators and annotations, and private use.
fn is_hidden(code: u32) -> bool {
    matches!(
        code,
        0x00AD
            | 0x061C
            | 0x180E
            | 0x200B
            | 0x200E
            | 0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE000..=0xF8FF
            | 0xF0000..=0xFFFFD
            | 0x100000..=0x10FFFD
    )
}

/// Characters drawn as nothing at all, used for names that look empty.
fn is_blank(code: u32) -> bool {
    matches!(code, 0x115F | 0x1160 | 0x3164 | 0xFFA0 | 0x2800)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keep_what_they_say_and_lose_what_hides() {
        let clean = |name: &str| clean(name).unwrap_or_default();
        // Kept: letters of any script, accents, emoji (and their joiners,
        // skin tones, keycaps and flags).
        for name in ["Alice", "🆎 Alex", "Zoë", "Nguyễn Văn Ánh", "हिन्दी", "ภาษาไทย", "فارسی‌زبان", "👩‍👩‍👧 family", "👍🏽", "1️⃣", "🇳🇴", "🏴\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}"] {
            assert_eq!(clean(name), name, "{name:?}");
        }
        // Compatibility forms read as what they look like.
        assert_eq!(clean("ＡＤＭＩＮ"), "ADMIN");
        // A name shown backwards, and invisible characters, lose them.
        assert_eq!(clean("Bob\u{202E}nimda"), "Bobnimda");
        assert_eq!(clean("Ad\u{200B}min\u{2066}\u{FEFF}"), "Admin");
        assert_eq!(clean("Eve\u{E000}\u{F8FF}"), "Eve");
        assert_eq!(clean("tag\u{E0041}\u{E0042}"), "tag", "tag characters outside a flag");
        // Stacked accents are cut down.
        let zalgo = format!("Z{}algo", "\u{0301}\u{0316}\u{0324}\u{0332}\u{0353}\u{0361}\u{0489}".repeat(4));
        assert_eq!(clean(&zalgo).chars().filter(|c| is_combining_mark(*c)).count(), MAX_MARKS);
        // (NFKC makes the first accent part of the letter: Ź.)
        assert!(clean(&zalgo).starts_with('Ź') && clean(&zalgo).ends_with("algo"));
        assert_eq!(clean(&zalgo).chars().count(), "Źalgo".chars().count() + MAX_MARKS);
        assert_eq!(clean("\u{0301}\u{0301}lead"), "lead", "marks with nothing to sit on");
        // Spaces of every kind, line breaks and blank-looking characters:
        // one space, none at the ends; a name of nothing but those is none.
        assert_eq!(clean("  Ann\u{3000}\u{2028}\n\tLee \u{3164} "), "Ann Lee");
        assert_eq!(clean("a\u{07}b"), "ab");
        assert_eq!(clean("\u{3164}\u{2800} \u{200B}"), "");
        assert_eq!(super::clean("\u{115F}"), None);
        // Long ones are cut, and never end in a space.
        assert_eq!(clean(&"x".repeat(300)).chars().count(), MAX_CHARS);
        let spaced = format!("{} y", "x".repeat(MAX_CHARS - 1));
        assert_eq!(clean(&spaced), "x".repeat(MAX_CHARS - 1));
    }
}
