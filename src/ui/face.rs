//! Someone's picture in the terminal: their icon's colours when another app
//! (Sideband, MeshChat…) gave them one, else their face, LXMFace's picture
//! of their address (see `crate::icons::face`), in half blocks: each cell
//! two of its squares, one over the other.

use ratatui::style::{Color, Style};
use ratatui::text::Span;

use crate::lxmf::fields::Appearance;

fn colour([r, g, b]: [u8; 3]) -> Color {
    Color::Rgb(r, g, b)
}

/// Rows of half blocks for a grid of colours (two of its rows a line).
fn half_blocks<const W: usize>(pixels: &[[[u8; 3]; W]]) -> Vec<Vec<Span<'static>>> {
    pixels
        .chunks(2)
        .map(|pair| {
            (0..W)
                .map(|x| {
                    let top = pair[0][x];
                    let bottom = pair.get(1).map_or(top, |row| row[x]);
                    Span::styled("▀", Style::default().fg(colour(top)).bg(colour(bottom)))
                })
                .collect()
        })
        .collect()
}

/// Their face, 8 columns by 4 lines.
pub(super) fn full(address: &str) -> Vec<Vec<Span<'static>>> {
    half_blocks(&crate::icons::face::pixels(address))
}

/// A picture for a list, 4 columns by 2 lines: their icon's colours (with
/// `letter`, their name's first, since a terminal may lack the icon's
/// glyph), or else their face, made small.
pub(super) fn small(address: &str, icon: Option<&Appearance>, letter: char) -> [Vec<Span<'static>>; 2] {
    match icon {
        Some(icon) => {
            let style = Style::default().fg(colour(icon.foreground)).bg(colour(icon.background));
            [vec![Span::styled(format!(" {letter}  "), style.bold())], vec![Span::styled("    ", style)]]
        }
        None => {
            let mut rows = half_blocks(&crate::icons::face::small(address)).into_iter();
            [rows.next().unwrap_or_default(), rows.next().unwrap_or_default()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_by_size_and_icons_first() {
        let address = "a7b3c9d1e5f20681943ab2de77fc8e01";
        let face = full(address);
        assert_eq!((face.len(), face[0].len()), (4, 8));
        let [top, bottom] = small(address, None, 'A');
        assert_eq!((top.len(), bottom.len()), (4, 4));
        assert!(top.iter().all(|s| s.content == "▀"));
        // An icon's colours win over the face.
        let icon = Appearance { icon: "radio-tower".into(), foreground: [255; 3], background: [0, 0, 128] };
        let [top, _] = small(address, Some(&icon), 'A');
        assert_eq!(top[0].content, " A  ");
        assert_eq!(top[0].style.bg, Some(Color::Rgb(0, 0, 128)));
    }
}
