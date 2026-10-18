//! Someone's face: the picture LXMFace draws from an LXMF address (an
//! identicon, as Ratspeak shows them), for whoever hasn't set an icon of
//! their own (see `super`). The same address gives the same face in every
//! app that uses LXMFace.
//!
//! It's an 8×8 grid of three colours, symmetric left to right, in a
//! circle. The web UI shows LXMFace's own SVG; the terminal draws the grid
//! in half blocks.

/// The face for `address` (hex) as an SVG image, `size` pixels square.
pub fn svg(address: &str, size: u32) -> String {
    lxmface::lxmface_svg(&address.to_ascii_lowercase(), size)
}

/// The face for `address` (hex): its 8×8 grid of colours, top row first.
pub fn pixels(address: &str) -> [[[u8; 3]; 8]; 8] {
    let data = lxmface::lxmface_data(&address.to_ascii_lowercase());
    let colours = [rgb(&data.bgcolor), rgb(&data.color), rgb(&data.spotcolor)];
    data.grid.map(|row| row.map(|cell| colours[usize::from(cell.min(2))]))
}

/// The face halved to 4×4, for a terminal's list: each 2×2 square's
/// colour is the one most of it has, and of a tie the brightest's (the
/// shape over the background).
pub fn small(address: &str) -> [[[u8; 3]; 4]; 4] {
    let data = lxmface::lxmface_data(&address.to_ascii_lowercase());
    let colours = [rgb(&data.bgcolor), rgb(&data.color), rgb(&data.spotcolor)];
    let mut out = [[[0; 3]; 4]; 4];
    for (y, row) in out.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            let mut count = [0u8; 3];
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                count[usize::from(data.grid[y * 2 + dy][x * 2 + dx].min(2))] += 1;
            }
            // Most first; of a tie, spot over primary over background.
            let pick = (0..3).max_by_key(|&i| (count[i], i)).unwrap_or(0);
            *cell = colours[pick];
        }
    }
    out
}

/// An HSL colour (hue in degrees, saturation and lightness in percent) as
/// RGB.
fn rgb(colour: &lxmface::HslColor) -> [u8; 3] {
    let (h, s, l) = (f64::from(colour.h) / 360.0, colour.s / 100.0, colour.l / 100.0);
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return [v; 3];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let channel = |t: f64| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    [channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDRESS: &str = "a7b3c9d1e5f20681943ab2de77fc8e01";

    #[test]
    fn the_same_address_the_same_face_whatever_its_case() {
        assert_eq!(svg(ADDRESS, 64), svg(&ADDRESS.to_uppercase(), 64));
        assert!(svg(ADDRESS, 64).starts_with("<svg"));
        assert_ne!(pixels(ADDRESS), pixels("00000000000000000000000000000001"));
        // Symmetric, left to right.
        for row in pixels(ADDRESS) {
            assert_eq!(row[0], row[7]);
            assert_eq!(row[3], row[4]);
        }
        let small = small(ADDRESS);
        for row in small {
            assert_eq!(row[0], row[3]);
        }
    }

    #[test]
    fn hsl_to_rgb() {
        let colour = |h, s, l| rgb(&lxmface::HslColor { h, s, l });
        assert_eq!(colour(0, 100.0, 50.0), [255, 0, 0]);
        assert_eq!(colour(120, 100.0, 50.0), [0, 255, 0]);
        assert_eq!(colour(240, 100.0, 25.0), [0, 0, 128]);
        assert_eq!(colour(0, 50.0, 100.0), [255, 255, 255]);
        assert_eq!(colour(60, 100.0, 0.0), [0, 0, 0]);
    }
}
