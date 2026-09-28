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
    (1..)
        .map(|i| dir.join(format!("{stem}-{i}{ext}")))
        .find(|p| !p.exists())
        .expect("an unused name exists")
}

pub(super) fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => directories::BaseDirs::new()
            .map(|d| d.home_dir().join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
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
