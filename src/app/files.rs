//! Files on disk: saving downloads without overwriting, and opening them.

use std::path::{Path, PathBuf};

use super::App;

/// A path in `dir` for `name` that does not overwrite anything. The remote
/// name is reduced to its final component so it cannot escape `dir`.
pub(crate) fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let name = Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .unwrap_or_else(|| "download".to_string());
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
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
