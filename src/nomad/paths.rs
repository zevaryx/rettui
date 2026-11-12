//! NomadNet's addresses, and the names of what a node keeps.
//!
//! A node answers at the `nomadnetwork.node` destination of its identity.
//! Pages are asked for as `/page/<name>`, files as `/file/<name>`, and a
//! page's pictures at `/media`, with the name in the request. Names are
//! relative paths, as NomadNet keeps them: never `..`, never hidden, and
//! never an `.allowed` list (who may see what's beside it).

use std::path::{Component, Path, PathBuf};

/// The destination name NomadNet nodes announce under.
pub const NODE_ASPECT: &str = "nomadnetwork.node";
pub const PAGE_PREFIX: &str = "/page/";
pub const FILE_PREFIX: &str = "/file/";
/// Where a page's pictures are asked for (the name goes in the request).
pub const MEDIA_PATH: &str = "/media";
/// The page a node shows first.
pub const INDEX_PATH: &str = "/page/index.mu";

/// The most of a node's name that is announced, or kept from an announce.
pub const MAX_NAME_BYTES: usize = 256;
/// The most of some text put into a page rettui writes.
const MAX_TEXT_CHARS: usize = 128;

const MAX_PATH_BYTES: usize = 1024;
const MAX_PARTS: usize = 32;
const MAX_PART_BYTES: usize = 255;

/// Why a name or a path on the node can't be used.
#[derive(Debug)]
pub enum PathError {
    /// Not a name the node takes: why.
    Invalid(&'static str),
    /// It leads out of the node's folder (`..`, a symlink).
    Outside,
    /// Nothing by that name.
    NotFound,
    TooBig {
        size: u64,
        max: usize,
    },
    Io(std::io::Error),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathError::Invalid(why) => write!(f, "invalid path: {why}"),
            PathError::Outside => write!(f, "the path leads out of the node's folder"),
            PathError::NotFound => write!(f, "not found"),
            PathError::TooBig { size, max } => write!(f, "too large ({size} bytes, the most is {max})"),
            PathError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<std::io::Error> for PathError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::NotFound { PathError::NotFound } else { PathError::Io(e) }
    }
}

/// Whether a file is kept out of sight: a dotfile, or an `.allowed` list.
pub fn kept_aside(name: &str) -> bool {
    name.starts_with('.') || name.ends_with(".allowed")
}

/// Check a name on the node: a relative path of plain parts.
pub fn check_name(name: &str) -> Result<(), PathError> {
    let name = name.trim().trim_start_matches('/');
    if name.is_empty() {
        return Err(PathError::Invalid("it is empty"));
    }
    if name.len() > MAX_PATH_BYTES {
        return Err(PathError::Invalid("it is too long"));
    }
    if name.contains('\\') || name.chars().any(char::is_control) {
        return Err(PathError::Invalid("it has characters a name can't have"));
    }
    let mut parts = 0;
    for part in Path::new(name).components() {
        let Component::Normal(part) = part else { return Err(PathError::Outside) };
        let part = part.to_string_lossy();
        parts += 1;
        if parts > MAX_PARTS {
            return Err(PathError::Invalid("it has too many folders"));
        }
        if part.len() > MAX_PART_BYTES {
            return Err(PathError::Invalid("one of its names is too long"));
        }
        if kept_aside(part.trim()) {
            return Err(PathError::Invalid("hidden files and .allowed lists aren't served"));
        }
    }
    Ok(())
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Where `name` is under `root`, whether or not it's there yet. No part of
/// the way may be a symlink, so nothing outside `root` is ever reached.
pub fn resolve(root: &Path, name: &str) -> Result<PathBuf, PathError> {
    check_name(name)?;
    if is_symlink(root) {
        return Err(PathError::Outside);
    }
    let mut path = if root.exists() { root.canonicalize()? } else { root.to_path_buf() };
    for part in Path::new(name.trim().trim_start_matches('/')).components() {
        path.push(part);
        if is_symlink(&path) {
            return Err(PathError::Outside);
        }
    }
    Ok(path)
}

fn name_after<'a>(path: &'a str, prefix: &str) -> Result<&'a str, PathError> {
    let name = path.trim().strip_prefix(prefix).filter(|name| !name.is_empty()).ok_or(PathError::Invalid("not an address of that kind"))?;
    check_name(name)?;
    Ok(name)
}

/// The name of the page a `/page/…` path asks for.
pub fn page_name(path: &str) -> Result<&str, PathError> {
    name_after(path, PAGE_PREFIX)
}

/// The name of the file a `/file/…` path asks for.
pub fn file_name(path: &str) -> Result<&str, PathError> {
    name_after(path, FILE_PREFIX)
}

/// Text to put in a page rettui writes (a name, a path): without control
/// characters or backticks, which Micron reads as formatting, and not long.
pub fn clean_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).map(|c| if c == '`' { '\'' } else { c }).take(MAX_TEXT_CHARS).collect()
}

/// A node's name as announced: trimmed, without control characters, and
/// [`MAX_NAME_BYTES`] at most.
pub fn clamp_name(name: &str) -> String {
    let mut name: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    let mut end = name.len().min(MAX_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name.truncate(end);
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_node_takes_and_refuses() {
        for name in ["index.mu", "blog/2026/first.mu", "/images/logo.png", " notes.txt "] {
            assert!(check_name(name).is_ok(), "{name}");
        }
        for name in ["", "  ", "/", "a\\b", "a\nb", ".hidden", "blog/.git/config", "club.mu.allowed", "x/club.allowed/y"] {
            assert!(matches!(check_name(name), Err(PathError::Invalid(_))), "{name}");
        }
        for name in ["..", "../secret", "a/../../b", "./a"] {
            assert!(matches!(check_name(name), Err(PathError::Outside)), "{name}");
        }
        assert!(check_name(&"a/".repeat(MAX_PARTS + 1)).is_err());
        assert!(check_name(&"a".repeat(MAX_PART_BYTES + 1)).is_err());
        assert!(check_name(&format!("{}/x", "a/".repeat(600))).is_err());
        assert!(kept_aside(".well") && kept_aside("page.mu.allowed") && !kept_aside("allowed.mu"));
    }

    #[test]
    fn addresses_to_names() {
        assert_eq!(page_name("/page/index.mu").unwrap(), "index.mu");
        assert_eq!(page_name(" /page/blog/one.mu ").unwrap(), "blog/one.mu");
        assert_eq!(file_name("/file/manual.pdf").unwrap(), "manual.pdf");
        for path in ["/page/", "/page", "/file/x.mu", "index.mu", "/page/../x", "/page/x.allowed"] {
            assert!(page_name(path).is_err(), "{path}");
        }
        assert!(file_name("/page/x").is_err() && file_name("/file/").is_err());
    }

    #[test]
    fn paths_stay_in_the_folder() {
        let dir = std::env::temp_dir().join(format!("rettui-test-nomad-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("pages");
        // A folder that isn't there yet: where it would be.
        assert_eq!(resolve(&root, "a/b.mu").unwrap(), root.join("a/b.mu"));
        std::fs::create_dir_all(root.join("blog")).unwrap();
        std::fs::write(root.join("blog/one.mu"), "one").unwrap();
        let there = resolve(&root, "blog/one.mu").unwrap();
        assert_eq!(std::fs::read(&there).unwrap(), b"one");
        assert!(!resolve(&root, "blog/two.mu").unwrap().exists());
        assert!(matches!(resolve(&root, "../pages/blog/one.mu"), Err(PathError::Outside)));
        #[cfg(unix)]
        {
            std::fs::write(dir.join("secret"), "secret").unwrap();
            std::os::unix::fs::symlink(dir.join("secret"), root.join("link.mu")).unwrap();
            std::os::unix::fs::symlink(&dir, root.join("up")).unwrap();
            assert!(matches!(resolve(&root, "link.mu"), Err(PathError::Outside)));
            assert!(matches!(resolve(&root, "up/secret"), Err(PathError::Outside)));
            std::os::unix::fs::symlink(&root, dir.join("alias")).unwrap();
            assert!(matches!(resolve(&dir.join("alias"), "blog/one.mu"), Err(PathError::Outside)));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn text_and_names_are_cleaned() {
        assert_eq!(clean_text("a `b`\n\tc"), "a 'b'c");
        assert_eq!(clean_text(&"x".repeat(500)).len(), MAX_TEXT_CHARS);
        assert_eq!(clamp_name("  My\u{7} node \n"), "My node");
        let long = "é".repeat(200);
        let clamped = clamp_name(&long);
        assert_eq!(clamped.len(), MAX_NAME_BYTES);
        assert!(long.starts_with(&clamped));
        assert_eq!(clamp_name(&format!("a{long}")).len(), MAX_NAME_BYTES - 1);
    }
}
