//! Terminal image rendering, with ratatui-image.
//!
//! Every image is drawn by ratatui-image in the best protocol the terminal
//! has: Kitty graphics (unicode placeholders), Sixel, iTerm2 inline images,
//! or half blocks (each cell two stacked pixels) everywhere else. Its
//! "sliced" images scroll and clip row by row like text. Windows Terminal
//! always gets Sixel.
//!
//! The terminal is probed by us rather than by ratatui-image: its query
//! leaves a thread reading stdin when a terminal never answers, which would
//! swallow the first key presses.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use image::{DynamicImage, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::layout::{Rect, Size};
use ratatui::widgets::Widget;
use ratatui_image::FontSize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::sliced::{SignedPosition, SlicedImage, SlicedProtocol};

/// Decoded images are downscaled to this size; terminals never show more.
const MAX_SOURCE_PX: u32 = 1024;
/// Decoding is refused above this size to bound memory use.
pub const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
/// Cell size when the terminal doesn't say (the usual 1:2 shape).
const DEFAULT_CELL: FontSize = FontSize::new(10, 20);

/// Terminal graphics support, detected once at startup.
#[derive(Clone, Debug)]
pub struct Graphics {
    picker: Picker,
}

/// Half blocks, for layouts drawn without a terminal's answers (tests).
static HALFBLOCKS: LazyLock<Graphics> = LazyLock::new(Graphics::halfblocks);

/// What a terminal says it can do, and where it runs.
#[derive(Debug, Default, Clone)]
pub struct Terminal {
    /// Answered the Kitty graphics query.
    pub kitty: bool,
    /// Lists Sixel (4) in its device attributes.
    pub sixel: bool,
    /// Cell size in pixels, when known.
    pub cell: Option<FontSize>,
    /// `WT_SESSION` is set (Windows Terminal, also inside WSL).
    pub windows_terminal: bool,
    /// iTerm2 or WezTerm (`TERM_PROGRAM`): iTerm2 inline images.
    pub iterm2: bool,
    /// Konsole: its Sixel and Kitty placeholders are unreliable.
    pub konsole: bool,
    /// `RETTUI_GRAPHICS`: `kitty`, `sixel`, `iterm2` or `halfblocks`.
    pub forced: Option<ProtocolType>,
}

impl Terminal {
    fn from_env() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let program = var("TERM_PROGRAM").unwrap_or_default();
        Self {
            windows_terminal: var("WT_SESSION").is_some(),
            iterm2: program == "iTerm.app" || program == "WezTerm" || var("WEZTERM_EXECUTABLE").is_some(),
            konsole: var("KONSOLE_VERSION").is_some(),
            forced: var("RETTUI_GRAPHICS").and_then(|v| match v.to_ascii_lowercase().as_str() {
                "kitty" => Some(ProtocolType::Kitty),
                "sixel" => Some(ProtocolType::Sixel),
                "iterm2" => Some(ProtocolType::Iterm2),
                "halfblocks" | "none" | "off" => Some(ProtocolType::Halfblocks),
                _ => None,
            }),
            cell: var("RETTUI_CELL_SIZE").and_then(|v| parse_cell_size(&v)),
            ..Self::default()
        }
    }

    /// The protocol to draw with: a forced one; Sixel in Windows Terminal;
    /// then Kitty, iTerm2 and Sixel as the terminal supports them (not
    /// Konsole's Kitty or Sixel, nor WezTerm's Kitty); else half blocks.
    pub fn protocol(&self) -> ProtocolType {
        if let Some(forced) = self.forced {
            return forced;
        }
        if self.windows_terminal {
            return ProtocolType::Sixel;
        }
        if self.kitty && !self.konsole && !self.iterm2 {
            return ProtocolType::Kitty;
        }
        if self.iterm2 {
            return ProtocolType::Iterm2;
        }
        if self.sixel && !self.konsole {
            return ProtocolType::Sixel;
        }
        ProtocolType::Halfblocks
    }
}

/// `9x19` as a cell size.
fn parse_cell_size(text: &str) -> Option<FontSize> {
    let (w, h) = text.trim().split_once(['x', 'X'])?;
    let (w, h): (u16, u16) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (w > 0 && h > 0).then(|| FontSize::new(w, h))
}

impl Graphics {
    /// Probe the terminal and pick how to draw images. Must run in raw
    /// mode, before anything else reads stdin.
    pub fn detect() -> Self {
        let mut terminal = Terminal::from_env();
        // Skip the probe when the answer is already decided.
        let decided = terminal.forced.is_some() || terminal.windows_terminal;
        #[allow(deprecated)]
        let tmux = Picker::from_fontsize(DEFAULT_CELL).tmux_detected();
        if let Some(answers) = probe::query(tmux, Duration::from_millis(if decided { 400 } else { 1500 })) {
            terminal.kitty = answers.kitty;
            terminal.sixel = answers.sixel;
            terminal.cell = terminal.cell.or(answers.cell);
        }
        let graphics = Self::for_terminal(&terminal);
        tracing::info!(
            "terminal graphics: {:?}, cell {}x{} px ({terminal:?})",
            graphics.picker.protocol_type(),
            graphics.picker.font_size().width,
            graphics.picker.font_size().height,
        );
        graphics
    }

    pub fn for_terminal(terminal: &Terminal) -> Self {
        let protocol = terminal.protocol();
        if protocol == ProtocolType::Halfblocks {
            return Self::halfblocks();
        }
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(terminal.cell.unwrap_or(DEFAULT_CELL));
        picker.set_protocol_type(protocol);
        Self { picker }
    }

    pub fn halfblocks() -> Self {
        Self { picker: Picker::halfblocks() }
    }

    pub fn protocol(&self) -> ProtocolType {
        self.picker.protocol_type()
    }

    #[cfg(test)]
    pub fn for_tests(protocol: ProtocolType) -> Self {
        Self::for_terminal(&Terminal {
            forced: Some(protocol),
            cell: Some(DEFAULT_CELL),
            ..Terminal::default()
        })
    }
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
    /// Last encoded graphic, keyed by protocol and cell size.
    graphic_cache: RefCell<Option<(ProtocolType, Size, SlicedProtocol)>>,
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
            graphic_cache: RefCell::new(None),
        })
    }

    /// Lay out within `max_cols` x `max_rows` cells, keeping the aspect
    /// ratio: encode (or reuse) the graphic and return its size in cells,
    /// or `None` if it can't be encoded. Without `graphics`, half blocks.
    pub fn rows(&self, graphics: Option<&Graphics>, max_cols: usize, max_rows: usize) -> Option<Size> {
        let graphics = graphics.unwrap_or(&HALFBLOCKS);
        let protocol = graphics.protocol();
        // A half-block cell shows 1x2 pixels, so pictures can be as many
        // cells wide as they have pixels; real graphics use the cell size.
        let font = if protocol == ProtocolType::Halfblocks {
            FontSize::new(1, 2)
        } else {
            graphics.picker.font_size()
        };
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
        if cache.as_ref().is_none_or(|(p, size, _)| *p != protocol || *size != wanted) {
            let sliced = SlicedProtocol::new(&graphics.picker, DynamicImage::ImageRgba8(self.image.clone()), Some(wanted)).ok()?;
            *cache = Some((protocol, wanted, sliced));
        }
        cache.as_ref().map(|(_, _, sliced)| sliced.size())
    }

    /// Draw the graphic laid out by [`Picture::rows`] with its top-left at
    /// `position` relative to `area` (rows outside the area are clipped).
    pub fn render_graphic(&self, area: Rect, position: SignedPosition, buf: &mut Buffer) {
        if let Some((_, _, sliced)) = &*self.graphic_cache.borrow() {
            SlicedImage::new(sliced, position).render(area, buf);
        }
    }
}

/// Terminal capability probe with a hard deadline and no helper thread,
/// so nothing keeps reading stdin once the application starts.
mod probe {
    // Replies are only read on Unix; elsewhere the environment decides.
    #![cfg_attr(not(unix), allow(dead_code))]

    use std::time::Duration;
    #[cfg(unix)]
    use std::time::Instant;

    use ratatui_image::FontSize;

    /// What the terminal answered.
    #[derive(Debug, Default)]
    pub struct Answers {
        pub kitty: bool,
        pub sixel: bool,
        pub cell: Option<FontSize>,
    }

    /// Ask for Kitty graphics support, the cell size and the device
    /// attributes (which list Sixel), then a status report that every
    /// terminal answers, so we know when the replies are complete.
    #[cfg(unix)]
    pub fn query(tmux: bool, timeout: Duration) -> Option<Answers> {
        use std::io::Write;

        let graphics_query = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";
        let query = if tmux {
            // tmux passthrough: wrap and double inner escapes.
            format!("\x1bPtmux;{}\x1b\\", graphics_query.replace('\x1b', "\x1b\x1b"))
        } else {
            graphics_query.to_string()
        };
        let mut out = std::io::stdout();
        out.write_all(format!("{query}\x1b[16t\x1b[c\x1b[5n").as_bytes()).ok()?;
        out.flush().ok()?;

        let replies = read_until(b"\x1b[0n", timeout);
        let text = String::from_utf8_lossy(&replies);
        Some(parse(&text).with_cell_fallback(cell_size_ioctl))
    }

    #[cfg(not(unix))]
    pub fn query(_tmux: bool, _timeout: Duration) -> Option<Answers> {
        None
    }

    impl Answers {
        fn with_cell_fallback(mut self, fallback: impl FnOnce() -> Option<FontSize>) -> Self {
            self.cell = self.cell.or_else(fallback);
            self
        }
    }

    /// The replies to the queries.
    pub fn parse(text: &str) -> Answers {
        Answers {
            kitty: text.contains("_Gi=31;OK"),
            sixel: device_attributes(text).is_some_and(|attrs| attrs.contains(&"4")),
            cell: cell_size_reply(text),
        }
    }

    /// `ESC [ ? 62 ; 4 ; … c`: the primary device attributes.
    fn device_attributes(text: &str) -> Option<Vec<&str>> {
        let start = text.find("\x1b[?")? + 3;
        let end = start + text[start..].find('c')?;
        let body = &text[start..end];
        body.chars().all(|c| c.is_ascii_digit() || c == ';').then(|| body.split(';').collect())
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
        fn parses_replies() {
            let answers = super::parse("\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[?62;4;22c\x1b[0n");
            assert!(answers.kitty && answers.sixel);
            let cell = answers.cell.unwrap();
            assert_eq!((cell.width, cell.height), (10, 20));
            // A terminal without Sixel (no 4) or Kitty.
            let answers = super::parse("\x1b[?62;22;42c\x1b[0n");
            assert!(!answers.kitty && !answers.sixel && answers.cell.is_none());
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
    use ratatui::style::Color;

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

    /// Lay out a 200x100 px picture in at most 10 columns and draw it one
    /// row scrolled off the top of a 20x5 area at column 2.
    fn drawn(graphics: Option<&Graphics>) -> (Size, Buffer) {
        let picture = Picture::decode(&png(200, 100)).unwrap();
        let size = picture.rows(graphics, 10, 30).expect("laid out");
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        let placement = Placement { row: 0, col: 2, size, key: () };
        draw_placements([&placement], 1, area, &mut buf, |_| Some(&picture));
        (size, buf)
    }

    #[test]
    fn half_blocks_without_terminal_graphics() {
        // 1x2 px per cell: 10 columns show 10x5 px, in 3 rows (rounded up).
        let (size, buf) = drawn(None);
        assert_eq!(size.width, 10);
        assert!((2..=3).contains(&size.height), "{size:?}");
        let cell = &buf[(2, 0)];
        assert!(["▀", "▄", "█"].contains(&cell.symbol()) || cell.bg != Color::Reset, "{cell:?}");
        assert_eq!(buf[(1, 0)].symbol(), " ");
    }

    #[test]
    fn kitty_graphics_draw_placeholders() {
        // 10x20 px cells: natural 20 cols, limited to 10 -> 100x50 px -> 3 rows.
        let (size, buf) = drawn(Some(&Graphics::for_tests(ProtocolType::Kitty)));
        assert_eq!(size.width, 10);
        assert!((2..=3).contains(&size.height), "{size:?}");
        let placeholder = |x, y| buf[(x, y)].symbol().contains('\u{10EEEE}');
        assert!(placeholder(2, 0));
        assert!(!placeholder(1, 0));
        assert!(!placeholder(2, size.height - 1)); // clipped row not drawn
    }

    #[test]
    fn sixel_and_iterm2_graphics() {
        let all = |buf: &Buffer| buf.content().iter().map(|c| c.symbol()).collect::<String>();
        let (_, buf) = drawn(Some(&Graphics::for_tests(ProtocolType::Sixel)));
        assert!(all(&buf).contains("\x1bP"), "a sixel sequence is drawn");
        let (_, buf) = drawn(Some(&Graphics::for_tests(ProtocolType::Iterm2)));
        assert!(all(&buf).contains("\x1b]1337;File="), "an iTerm2 image is drawn");
    }

    #[test]
    fn protocol_choice() {
        let t = |f: fn(&mut Terminal)| {
            let mut terminal = Terminal::default();
            f(&mut terminal);
            terminal.protocol()
        };
        assert_eq!(t(|_| {}), ProtocolType::Halfblocks);
        assert_eq!(t(|t| t.kitty = true), ProtocolType::Kitty);
        assert_eq!(t(|t| t.sixel = true), ProtocolType::Sixel);
        assert_eq!(t(|t| t.iterm2 = true), ProtocolType::Iterm2);
        // Windows Terminal always gets Sixel, whatever it answered.
        assert_eq!(t(|t| t.windows_terminal = true), ProtocolType::Sixel);
        assert_eq!(t(|t| { t.windows_terminal = true; t.kitty = true }), ProtocolType::Sixel);
        // WezTerm: iTerm2 rather than its Kitty; Konsole: neither of its own.
        assert_eq!(t(|t| { t.iterm2 = true; t.kitty = true }), ProtocolType::Iterm2);
        assert_eq!(t(|t| { t.konsole = true; t.kitty = true; t.sixel = true }), ProtocolType::Halfblocks);
        // RETTUI_GRAPHICS wins.
        assert_eq!(t(|t| { t.windows_terminal = true; t.forced = Some(ProtocolType::Halfblocks) }), ProtocolType::Halfblocks);
        let graphics = Graphics::for_terminal(&Terminal { windows_terminal: true, ..Terminal::default() });
        assert_eq!(graphics.protocol(), ProtocolType::Sixel);
    }

    #[test]
    fn cell_size_setting() {
        let size = parse_cell_size("9x19").unwrap();
        assert_eq!((size.width, size.height), (9, 19));
        assert!(parse_cell_size("9").is_none());
        assert!(parse_cell_size("0x19").is_none());
    }
}
