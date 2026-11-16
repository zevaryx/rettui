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

/// What a file counts for at least, against a limit on a folder's size: a
/// file takes a block on disk however small it is, and empty files are
/// otherwise free to make by the thousand.
pub(crate) const FILE_MIN_CHARGE: u64 = 4096;

/// Bytes the files under `dir` take (folders inside it counted too, to a few
/// levels), each counted for at least [`FILE_MIN_CHARGE`].
pub(crate) fn folder_size(dir: &Path) -> u64 {
    fn walk(dir: &Path, depth: usize) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
        entries
            .flatten()
            .map(|entry| match entry.metadata() {
                Ok(meta) if meta.is_file() => meta.len().max(FILE_MIN_CHARGE),
                Ok(meta) if meta.is_dir() && depth < 4 => walk(&entry.path(), depth + 1),
                _ => 0,
            })
            .sum()
    }
    walk(dir, 0)
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
    // Counting up from 1 each time is quadratic for many files of one name,
    // so past a few tries the number is random.
    for i in 1..=SEQUENTIAL_TRIES {
        let path = dir.join(format!("{stem}-{i}{ext}"));
        if !path.exists() {
            return path;
        }
    }
    loop {
        let path = dir.join(format!("{stem}-{:08x}{ext}", rand::random::<u32>()));
        if !path.exists() {
            return path;
        }
    }
}

/// How many numbered names, `name-1` on, [`unique_path`] tries before it
/// picks a random one.
const SEQUENTIAL_TRIES: u32 = 8;

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
    fn many_files_of_one_name_each_get_a_free_path_quickly() {
        let dir = std::env::temp_dir().join(format!("rettui-unique-many-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let path = unique_path(&dir, "same.txt");
            assert!(!path.exists() && path.starts_with(&dir));
            assert!(seen.insert(path.clone()), "{path:?} handed out twice");
            std::fs::write(&path, b"").unwrap();
        }
        // The first few are numbered, as before.
        assert!(dir.join("same-1.txt").exists() && dir.join("same-8.txt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folders_size_counts_what_is_inside_it() {
        let dir = std::env::temp_dir().join(format!("rettui-folder-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("x"), [0; 10]).unwrap();
        std::fs::write(dir.join("a/b/y"), [0; 5]).unwrap();
        // Each file counts for at least a block.
        assert_eq!(super::folder_size(&dir), 2 * FILE_MIN_CHARGE);
        std::fs::write(dir.join("big"), vec![0; FILE_MIN_CHARGE as usize + 7]).unwrap();
        std::fs::write(dir.join("empty"), b"").unwrap();
        assert_eq!(super::folder_size(&dir), 4 * FILE_MIN_CHARGE + 7);
        assert_eq!(super::folder_size(&dir.join("none")), 0);
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
