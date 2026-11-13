//! Files on disk: saving downloads without overwriting, and opening them.

use std::path::{Path, PathBuf};

use super::App;

/// A file name someone else chose, made safe to save: its final component
/// only (the `/` and `\\` of either system), without control or direction-
/// changing characters (which can make `txt.exe` read as `exe.txt`), without
/// characters Windows refuses, not hidden, not a Windows device name, and at
/// most 120 characters with its extension kept.
pub(crate) fn safe_file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let hidden = |c: char| c.is_control() || matches!(c as u32, 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2069 | 0xFEFF);
    let cleaned: String = last.chars().filter(|&c| !hidden(c)).map(|c| if ":*?\"<>|".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() || cleaned.starts_with('.') {
        return "download".to_string();
    }
    let (stem, ext) = match cleaned.rsplit_once('.') {
        Some((stem, ext)) if ext.chars().count() <= 16 => (stem.to_string(), format!(".{ext}")),
        _ => (cleaned.clone(), String::new()),
    };
    let stem: String = stem.chars().take(120usize.saturating_sub(ext.chars().count()).max(1)).collect();
    let device = stem.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let reserved = matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (device.len() == 4 && (device.starts_with("COM") || device.starts_with("LPT")) && device.as_bytes()[3].is_ascii_digit());
    if reserved { format!("_{stem}{ext}") } else { format!("{stem}{ext}") }
}

/// A path in `dir` for `name` that does not overwrite anything. The remote
/// name is made safe first (see [`safe_file_name`]).
pub(crate) fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let name = safe_file_name(name);
    let candidate = dir.join(&name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) => (stem.to_string(), format!(".{ext}")),
        None => (name.clone(), String::new()),
    };
    (1..).map(|i| dir.join(format!("{stem}-{i}{ext}"))).find(|p| !p.exists()).expect("an unused name exists")
}

/// A path typed, pasted or dropped into a prompt. A terminal given a file
/// dragged onto it may quote it (`'…'` or `"…"`), escape its spaces and
/// such with `\` (on Unix), or give it as a `file://` address: those are
/// undone. Text that names a file as it is stays as it is, so a path that
/// worked before still does.
pub(crate) fn path_from_input(text: &str) -> PathBuf {
    let text = text.trim();
    let literal = expand_home(text);
    if text.is_empty() || literal.exists() {
        return literal;
    }
    let mut path = text.to_string();
    for quote in ['\'', '"'] {
        if path.len() >= 2 && path.starts_with(quote) && path.ends_with(quote) {
            path = path[1..path.len() - 1].to_string();
            break;
        }
    }
    if let Some(address) = path.strip_prefix("file://") {
        // file:///home/… (or file://localhost/home/…), with %20 for spaces.
        let address = address.strip_prefix("localhost").unwrap_or(address);
        path = percent_decode(address);
        // file:///C:/… on Windows.
        if cfg!(windows) && path.as_bytes().get(2) == Some(&b':') && path.starts_with('/') {
            path.remove(0);
        }
    } else if cfg!(unix) && path.contains('\\') {
        let mut unescaped = String::with_capacity(path.len());
        let mut chars = path.chars();
        while let Some(c) = chars.next() {
            unescaped.push(if c == '\\' { chars.next().unwrap_or(c) } else { c });
        }
        path = unescaped;
    }
    expand_home(&path)
}

/// `%XX` escapes undone, as in a `file://` address.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            (b, ..) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(super) fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => directories::BaseDirs::new().map(|d| d.home_dir().join(rest)).unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

/// Open a file or web address with the desktop's default application.
fn open_external(target: impl AsRef<std::ffi::OsStr>) -> std::io::Result<()> {
    // Tests open nothing on the computer running them.
    if cfg!(test) {
        let _ = target;
        return Ok(());
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

impl App {
    /// Ask before opening a file someone sent, with the program the system
    /// uses for its type: the program may do more than show it.
    pub(super) fn ask_open_file(&mut self, path: &Path) {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let question = format!("Open {name} with the program your system uses for it? (y/n)");
        self.open_prompt(super::PromptKind::ConfirmOpenFile(path.to_path_buf()), &question, "");
    }

    pub(super) fn open_file(&mut self, path: &Path) {
        match open_external(path) {
            Ok(()) => self.log(format!("Opened {}", path.display())),
            Err(e) => self.log(format!("Could not open {}: {e}", path.display())),
        }
    }

    /// Open a web address in the desktop's browser.
    pub(super) fn open_url(&mut self, url: &str) {
        match open_external(url) {
            Ok(()) => self.notify(format!("Opened {url} in your browser")),
            Err(e) => self.fail(format!("Could not open {url}: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn paths_dropped_onto_a_terminal() {
        let dir = std::env::temp_dir().join(format!("rettui-dropped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("my identity (1)");
        std::fs::write(&file, b"x").unwrap();
        let shown = file.display().to_string();
        // As it is, quoted, escaped (Unix), or as a file:// address.
        assert_eq!(path_from_input(&shown), file);
        assert_eq!(path_from_input(&format!("'{shown}'")), file);
        assert_eq!(path_from_input(&format!("\"{shown}\" ")), file);
        assert_eq!(path_from_input(&format!("file://{}", shown.replace(' ', "%20").replace('(', "%28").replace(')', "%29"))), file);
        if cfg!(unix) {
            assert_eq!(path_from_input(&shown.replace(' ', "\\ ").replace('(', "\\(").replace(')', "\\)")), file);
        }
        // A file whose name really has quotes is taken as it is.
        let quoted = dir.join("'odd'");
        std::fs::write(&quoted, b"x").unwrap();
        assert_eq!(path_from_input(&quoted.display().to_string()), quoted);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    use super::*;

    #[test]
    fn unique_path_never_overwrites_or_escapes() {
        let dir = std::env::temp_dir().join(format!("rettui-unique-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a-1.txt"));
        assert_eq!(unique_path(&dir, "../../etc/passwd"), dir.join("passwd"));
        assert_eq!(unique_path(&dir, ".bashrc"), dir.join("download"));
        assert_eq!(unique_path(&dir, "..\\..\\evil.exe"), dir.join("evil.exe"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names_from_others_are_made_safe() {
        for (name, safe) in [
            ("notes\u{202e}txt.exe", "notestxt.exe"),
            ("a:b*c?.txt", "a_b_c_.txt"),
            ("report.pdf. . ", "report.pdf"),
            ("NUL.txt", "_NUL.txt"),
            ("com1", "_com1"),
            ("lpt10.txt", "lpt10.txt"),
            ("  ", "download"),
            ("", "download"),
            ("\u{0}\u{7}x.bin", "x.bin"),
        ] {
            assert_eq!(super::safe_file_name(name), safe, "{name:?}");
        }
        let long = format!("{}.jpeg", "a".repeat(400));
        let safe = super::safe_file_name(&long);
        assert!(safe.chars().count() <= 120 && safe.ends_with(".jpeg"), "{safe}");
        let dir = std::env::temp_dir().join(format!("rettui-safe-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
