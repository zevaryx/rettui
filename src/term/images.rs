//! Terminal image rendering.
//!
//! With a terminal that speaks the Kitty graphics protocol, images are shown
//! as real pictures (via ratatui-image's unicode-placeholder mode, so they
//! scroll and clip like text). Otherwise they fall back to half blocks: each
//! cell shows two stacked pixels (`▀` with the top pixel as foreground and
//! the bottom pixel as background).

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use ratatui_image::FontSize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::sliced::{SignedPosition, SlicedImage, SlicedProtocol};

/// Decoded images are downscaled to this size; terminals never show more.
const MAX_SOURCE_PX: u32 = 1024;
/// Decoding is refused above this size to bound memory use.
pub const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;

/// Terminal graphics support, detected once at startup.
#[derive(Clone, Debug)]
pub struct Graphics {
    picker: Picker,
}

impl Graphics {
    /// Query the terminal; `None` unless it supports the Kitty protocol.
    /// Must run in raw mode, before anything else reads stdin.
    /// `RETTUI_GRAPHICS=halfblocks` forces the fallback.
    pub fn detect() -> Option<Self> {
        if std::env::var("RETTUI_GRAPHICS").is_ok_and(|v| v == "halfblocks") {
            return None;
        }
        // Built first: it detects tmux and enables passthrough, which the
        // probe needs. (Deprecated upstream in favour of its own stdio query,
        // which leaves a thread reading stdin when a terminal never answers.)
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(FontSize::new(10, 20));
        let font = probe::kitty(picker.tmux_detected(), Duration::from_millis(1500))?;
        #[allow(deprecated)]
        {
            let tmux = picker.tmux_detected();
            picker = Picker::from_fontsize(font);
            debug_assert_eq!(tmux, picker.tmux_detected());
        }
        picker.set_protocol_type(ProtocolType::Kitty);
        tracing::info!("Kitty graphics enabled, cell {}x{} px", font.width, font.height);
        Some(Self { picker })
    }

    #[cfg(test)]
    pub fn kitty_for_tests() -> Self {
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(FontSize::new(10, 20));
        picker.set_protocol_type(ProtocolType::Kitty);
        Self { picker }
    }
}

/// How an image occupies rows in a text layout.
pub enum ImageRows {
    /// Half-block text rows.
    Text(Vec<Line<'static>>),
    /// Blank rows reserved for a graphic of this size (drawn afterwards).
    Graphic(Size),
}

/// What an image being decoded in the background is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeFor {
    /// An image on the page in the browser, by its URL.
    Page(String),
    /// A message attachment on disk.
    File(PathBuf),
}

/// A finished background decode (`None` when the data isn't an image).
pub struct Decoded {
    pub what: DecodeFor,
    pub picture: Option<Picture>,
}

/// Decode on a worker thread and send the result, so large images never
/// stall drawing or input.
pub fn decode_in_background(what: DecodeFor, bytes: Option<Vec<u8>>, done: tokio::sync::mpsc::UnboundedSender<Decoded>) {
    std::thread::spawn(move || {
        let bytes = match (bytes, &what) {
            (Some(bytes), _) => bytes,
            (None, DecodeFor::File(path)) => std::fs::read(path).unwrap_or_default(),
            (None, DecodeFor::Page(_)) => Vec::new(),
        };
        let picture = Picture::decode(&bytes);
        let _ = done.send(Decoded { what, picture });
    });
}

pub struct Picture {
    image: RgbaImage,
    /// Last half-block rendering, keyed by column count.
    text_cache: RefCell<Option<(usize, Vec<Line<'static>>)>>,
    /// Last encoded graphic, keyed by the requested cell size.
    graphic_cache: RefCell<Option<(Size, SlicedProtocol)>>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}x{})", self.image.width(), self.image.height())
    }
}

impl Picture {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_IMAGE_BYTES {
            return None;
        }
        let image = image::load_from_memory(bytes).ok()?;
        let image = if image.width() > MAX_SOURCE_PX || image.height() > MAX_SOURCE_PX {
            image.thumbnail(MAX_SOURCE_PX, MAX_SOURCE_PX)
        } else {
            image
        };
        Some(Self {
            image: DynamicImage::into_rgba8(image),
            text_cache: RefCell::new(None),
            graphic_cache: RefCell::new(None),
        })
    }

    /// Lay out within `max_cols` x `max_rows` cells, keeping the aspect ratio.
    pub fn rows(&self, graphics: Option<&Graphics>, max_cols: usize, max_rows: usize) -> ImageRows {
        match graphics.and_then(|g| self.graphic_size(g, max_cols, max_rows)) {
            Some(size) => ImageRows::Graphic(size),
            None => ImageRows::Text(self.lines_within(max_cols, max_rows)),
        }
    }

    /// Encode (or reuse) the graphic for the largest fitting size and
    /// return its actual cell size.
    fn graphic_size(&self, graphics: &Graphics, max_cols: usize, max_rows: usize) -> Option<Size> {
        let font = graphics.picker.font_size();
        let (fw, fh) = (f64::from(font.width.max(1)), f64::from(font.height.max(1)));
        let (w, h) = (f64::from(self.image.width()), f64::from(self.image.height().max(1)));
        // Natural size in cells, then shrink to fit both limits.
        let mut cols = (w / fw).ceil().min(max_cols as f64).max(1.0);
        let mut rows = (cols * fw * h / w / fh).ceil();
        if rows > max_rows as f64 {
            rows = (max_rows as f64).max(1.0);
            cols = (rows * fh * w / h / fw).floor().max(1.0);
        }
        let wanted = Size::new(cols as u16, rows as u16);
        let mut cache = self.graphic_cache.borrow_mut();
        if cache.as_ref().is_none_or(|(size, _)| *size != wanted) {
            let protocol = SlicedProtocol::new(
                &graphics.picker,
                DynamicImage::ImageRgba8(self.image.clone()),
                Some(wanted),
            )
            .ok()?;
            *cache = Some((wanted, protocol));
        }
        cache.as_ref().map(|(_, protocol)| protocol.size())
    }

    /// Draw the graphic laid out by [`Picture::rows`] with its top-left at
    /// `position` relative to `area` (rows above the area are clipped).
    pub fn render_graphic(&self, area: Rect, position: SignedPosition, buf: &mut Buffer) {
        if let Some((_, protocol)) = &*self.graphic_cache.borrow() {
            SlicedImage::new(protocol, position).render(area, buf);
        }
    }

    /// Half blocks within `max_cols` columns and `max_rows` rows, keeping the
    /// aspect ratio (never upscaled past the source width).
    pub fn lines_within(&self, max_cols: usize, max_rows: usize) -> Vec<Line<'static>> {
        let (w, h) = self.image.dimensions();
        // Each row shows two pixel rows.
        let cols_for_rows = (max_rows.max(1) * 2) as f64 * w as f64 / h.max(1) as f64;
        self.lines(max_cols.min(cols_for_rows.floor().max(1.0) as usize))
    }

    /// Half blocks at most `max_cols` wide (never upscaled past the source width).
    pub fn lines(&self, max_cols: usize) -> Vec<Line<'static>> {
        let (w, h) = self.image.dimensions();
        let cols = max_cols.min(w as usize).max(1);
        if let Some((cached_cols, lines)) = &*self.text_cache.borrow()
            && *cached_cols == cols
        {
            return lines.clone();
        }
        let px_h = ((h as f64 * cols as f64 / w as f64).round() as u32).max(2);
        let rows = px_h.div_ceil(2);
        let scaled = image::imageops::resize(&self.image, cols as u32, rows * 2, FilterType::Triangle);

        let pixel = |x: u32, y: u32| {
            let p = scaled.get_pixel(x, y);
            (p[3] >= 128).then_some(Color::Rgb(p[0], p[1], p[2]))
        };
        let lines: Vec<Line<'static>> = (0..rows)
            .map(|row| {
                Line::from(
                    (0..cols as u32)
                        .map(|x| match (pixel(x, row * 2), pixel(x, row * 2 + 1)) {
                            (Some(top), Some(bottom)) => {
                                Span::styled("▀", Style::default().fg(top).bg(bottom))
                            }
                            (Some(top), None) => Span::styled("▀", Style::default().fg(top)),
                            (None, Some(bottom)) => Span::styled("▄", Style::default().fg(bottom)),
                            (None, None) => Span::raw(" "),
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        *self.text_cache.borrow_mut() = Some((cols, lines.clone()));
        lines
    }
}

/// Terminal capability probe with a hard deadline and no helper thread,
/// so nothing keeps reading stdin once the application starts.
mod probe {
    use std::time::{Duration, Instant};

    use ratatui_image::FontSize;

    /// Ask for Kitty graphics support, the cell size, and a status report
    /// (which every terminal answers, so we know when replies are complete).
    /// Returns the cell size when the terminal speaks the Kitty protocol.
    #[cfg(unix)]
    pub fn kitty(tmux: bool, timeout: Duration) -> Option<FontSize> {
        use std::io::Write;

        let graphics_query = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";
        let query = if tmux {
            // tmux passthrough: wrap and double inner escapes.
            format!("\x1bPtmux;{}\x1b\\", graphics_query.replace('\x1b', "\x1b\x1b"))
        } else {
            graphics_query.to_string()
        };
        let mut out = std::io::stdout();
        out.write_all(format!("{query}\x1b[16t\x1b[5n").as_bytes()).ok()?;
        out.flush().ok()?;

        let replies = read_until(b"\x1b[0n", timeout);
        let text = String::from_utf8_lossy(&replies);
        if !text.contains("_Gi=31;OK") {
            return None;
        }
        cell_size_reply(&text).or_else(cell_size_ioctl)
    }

    #[cfg(not(unix))]
    pub fn kitty(_tmux: bool, _timeout: Duration) -> Option<FontSize> {
        None
    }

    /// Read stdin until `end` arrives or the deadline passes.
    #[cfg(unix)]
    fn read_until(end: &[u8], timeout: Duration) -> Vec<u8> {
        let deadline = Instant::now() + timeout;
        let mut buf = Vec::new();
        while !buf.windows(end.len()).any(|w| w == end) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let mut pfd = libc::pollfd {
                fd: libc::STDIN_FILENO,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd for the duration of the call.
            let ready = unsafe { libc::poll(&mut pfd, 1, remaining.as_millis().max(1) as i32) };
            if ready <= 0 {
                break;
            }
            let mut chunk = [0u8; 512];
            // SAFETY: reading into a local buffer of the stated length.
            let n = unsafe { libc::read(libc::STDIN_FILENO, chunk.as_mut_ptr().cast(), chunk.len()) };
            if n <= 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n as usize]);
        }
        buf
    }

    /// `ESC [ 6 ; height ; width t` (reply to `ESC [ 16 t`).
    pub fn cell_size_reply(text: &str) -> Option<FontSize> {
        let start = text.find("\x1b[6;")? + 4;
        let end = start + text[start..].find('t')?;
        let (h, w) = text[start..end].split_once(';')?;
        let (h, w): (u16, u16) = (h.parse().ok()?, w.parse().ok()?);
        (h > 0 && w > 0).then(|| FontSize::new(w, h))
    }

    /// Cell size from the tty's pixel dimensions, when it reports them.
    #[cfg(unix)]
    fn cell_size_ioctl() -> Option<FontSize> {
        // SAFETY: TIOCGWINSZ fills a winsize we own.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0;
        (ok && ws.ws_col > 0 && ws.ws_row > 0 && ws.ws_xpixel > 0 && ws.ws_ypixel > 0)
            .then(|| FontSize::new(ws.ws_xpixel / ws.ws_col, ws.ws_ypixel / ws.ws_row))
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn parses_cell_size_reply() {
            let reply = "\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[0n";
            let size = super::cell_size_reply(reply).unwrap();
            assert_eq!((size.width, size.height), (10, 20));
            assert!(super::cell_size_reply("\x1b[0n").is_none());
        }
    }
}

/// A graphic reserved in a text layout: drawn over rows `row..row+height`
/// starting at column `col`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement<K> {
    pub row: usize,
    pub col: usize,
    pub size: Size,
    pub key: K,
}

/// Draw placements over a scrolled text viewport. `scroll` is the first
/// layout row shown at the top of `area`.
pub fn draw_placements<'a, K: 'a>(
    placements: impl IntoIterator<Item = &'a Placement<K>>,
    scroll: usize,
    area: Rect,
    buf: &mut Buffer,
    picture: impl Fn(&K) -> Option<&'a Picture>,
) {
    for placement in placements {
        let top = placement.row as i64 - scroll as i64;
        let bottom = top + i64::from(placement.size.height);
        if bottom <= 0 || top >= i64::from(area.height) {
            continue;
        }
        if let Some(picture) = picture(&placement.key) {
            let position = SignedPosition::from((
                placement.col.min(i16::MAX as usize) as i16,
                top.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16,
            ));
            picture.render_graphic(area, position, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut img = RgbaImage::new(w, h);
        for (_, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([if y < h / 2 { 255 } else { 0 }, 0, 0, 255]);
        }
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn renders_half_blocks_at_requested_width() {
        let picture = Picture::decode(&png(8, 8)).unwrap();
        let lines = picture.lines(4);
        assert_eq!(lines.len(), 2); // 4 cols -> 4x4 px -> 2 rows
        assert_eq!(lines[0].spans.len(), 4);
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(lines[1].spans[0].style.bg, Some(Color::Rgb(0, 0, 0)));
        // Height-bound: 1 row = 2 px tall -> 2 px wide for a square image.
        assert_eq!(picture.lines_within(8, 1)[0].spans.len(), 2);
        assert!(matches!(picture.rows(None, 4, 10), ImageRows::Text(_)));
    }

    #[test]
    fn kitty_graphics_reserve_rows_and_draw_placeholders() {
        let gfx = Graphics::kitty_for_tests();
        // 200x100 px with 10x20 px cells: natural 20 cols; limited to 10 cols
        // -> 100x50 px -> 3 rows (rounded up).
        let picture = Picture::decode(&png(200, 100)).unwrap();
        let ImageRows::Graphic(size) = picture.rows(Some(&gfx), 10, 30) else {
            panic!("expected a graphic");
        };
        assert_eq!(size.width, 10);
        assert!((2..=3).contains(&size.height), "{size:?}");

        // Scrolled one row past the image's top: only the remaining rows draw.
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        let placement = Placement { row: 0, col: 2, size, key: () };
        draw_placements([&placement], 1, area, &mut buf, |_| Some(&picture));
        let placeholder = |x, y| buf[(x, y)].symbol().contains('\u{10EEEE}');
        assert!(placeholder(2, 0));
        assert!(!placeholder(1, 0));
        assert!(!placeholder(2, size.height - 1)); // clipped row not drawn
    }
}
