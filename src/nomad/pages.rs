//! The pages of a node hosted by rettui, for the page editors.
//!
//! Paths are relative to the node's `pages/` folder and go through
//! nomad-core's checks (no `..`, symlinks, hidden or `.allowed` files).
//! Saving keeps a page's permissions, so executable pages (scripts) stay
//! executable; the web editor is not allowed to change scripts at all.

use std::path::{Path, PathBuf};

use nomad_core::{NomadContentRoots, NomadContentStore, resolve_under_root};

/// A file in `pages/`.
#[derive(Debug, Clone)]
pub struct PageInfo {
    pub path: String,
    pub size: u64,
    pub modified_ms: Option<u64>,
    /// Runs as a program when the node allows scripts.
    pub executable: bool,
    /// Micron (or other text) that the editors can open.
    pub text: bool,
}

pub struct Pages {
    store: NomadContentStore,
}

/// Shown when browsing your own node's script pages, which only run for
/// visitors.
/// The page for a path that isn't there. (nomad-core's own wording quotes
/// the path with backticks, which Micron reads as formatting codes.)
fn not_found(path: &str) -> String {
    let path = nomad_core::sanitize_micron_text(path);
    format!("#!c=0\n>Not found\n\nThe page {path} is not available on this node.\n")
}

const SCRIPT_NOTICE: &str = ">Script page\n\nThis page runs as a program when visitors open it. Its output is not shown when you browse your own node from rettui.\n";

fn starter_index(name: &str) -> String {
    let name = nomad_core::sanitize_micron_text(name.trim());
    format!(
        ">{name}\n\n\
Welcome to this NomadNet node, hosted with rettui.\n\n\
This is the index page. Edit it in rettui's Node tab (in the terminal or the web UI) and save: \
visitors get the new version straight away.\n\n\
-\n\n\
>>Micron in brief\n\
`!Bold`!, `*italic`*, `_underlined`_ and `Ff00coloured`f text.\n\
`[A link to this page`:/page/index.mu]\n"
    )
}

/// Extensions of files served from `pages/` that are not text.
const BINARY_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"];

fn is_text_name(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    !ext.is_some_and(|e| BINARY_EXTENSIONS.contains(&e.as_str()))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

/// A page name as typed: `/page/about.mu`, `page/about`, `about` all become
/// `about.mu`; paths with another extension (images, `.txt`) are kept.
pub fn normalize_name(name: &str) -> Result<String, String> {
    let name = name.trim().trim_start_matches('/');
    let name = name.strip_prefix("page/").unwrap_or(name);
    if name.is_empty() {
        return Err("A page needs a name".into());
    }
    let file = name.rsplit('/').next().unwrap_or(name);
    let name = if file.contains('.') { name.to_string() } else { format!("{name}.mu") };
    nomad_core::validate_content_relative_path(&name).map_err(|e| format!("Not a page name: {e}"))?;
    Ok(name)
}

impl Pages {
    /// Open (creating if needed) a node folder with `pages/` and `files/`.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let store = NomadContentStore::new(NomadContentRoots::under(dir))
            .map_err(|e| format!("Could not open node folder {}: {e}", dir.display()))?;
        Ok(Self { store })
    }

    /// Write a starter `index.mu` if the node has none.
    pub fn ensure_index(&self, name: &str) -> Result<(), String> {
        if self.exists("index.mu") {
            return Ok(());
        }
        self.save("index.mu", &starter_index(name), false)
    }

    /// What the node sends for a `/page/…`, `/file/…` or `/media/…` path,
    /// read from the folder (used to browse your own node, which this client
    /// cannot reach over Reticulum). Scripts are not run.
    pub fn serve(&self, path: &str) -> Result<Vec<u8>, String> {
        if let Some(rel) = path.strip_prefix("/media/") {
            return self.read_media(rel);
        }
        if path.starts_with(nomad_core::FILE_PREFIX) {
            return self.store.read_file_route(path).map_err(|e| format!("{path}: {e}"));
        }
        let rel = nomad_core::strip_page_prefix(path).map_err(|e| format!("{path}: {e}"))?;
        if self.is_executable(rel) {
            return Ok(SCRIPT_NOTICE.as_bytes().to_vec());
        }
        match self.store.read_page_rel(rel) {
            Ok(bytes) => Ok(bytes),
            Err(nomad_core::NomadError::NotFound(_) | nomad_core::NomadError::PathTraversal) => Ok(not_found(path).into_bytes()),
            Err(e) => Err(format!("{path}: {e}")),
        }
    }

    pub fn pages_dir(&self) -> &Path {
        &self.store.roots().pages_dir
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        resolve_under_root(self.pages_dir(), path).map_err(|e| format!("{path}: {e}"))
    }

    pub fn exists(&self, path: &str) -> bool {
        self.resolve(path).is_ok_and(|p| p.is_file())
    }

    pub fn is_executable(&self, path: &str) -> bool {
        self.resolve(path).is_ok_and(|p| is_executable(&p))
    }

    pub fn list(&self) -> Result<Vec<PageInfo>, String> {
        let entries = self.store.list_pages().map_err(|e| e.to_string())?;
        Ok(entries
            .into_iter()
            .map(|entry| PageInfo {
                executable: self.is_executable(&entry.path),
                text: is_text_name(&entry.path),
                path: entry.path,
                size: entry.size,
                modified_ms: entry.modified_ms,
            })
            .collect())
    }

    /// A page's text. Binary files (images) cannot be edited.
    pub fn read(&self, path: &str) -> Result<String, String> {
        let bytes = self.store.read_page_rel(path).map_err(|e| format!("{path}: {e}"))?;
        String::from_utf8(bytes).map_err(|_| format!("{path} is not a text file"))
    }

    /// Write a page (new or existing). Existing pages keep their permissions.
    /// With `scripts` false, executable pages are refused.
    pub fn save(&self, path: &str, content: &str, scripts: bool) -> Result<(), String> {
        let target = self.resolve(path)?;
        let existing = std::fs::metadata(&target).ok().filter(|m| m.is_file());
        if !scripts && is_executable(&target) {
            return Err(format!("{path} is a script; edit it on the host"));
        }
        self.store.write_page_rel(path, content.as_bytes()).map_err(|e| format!("Could not save {path}: {e}"))?;
        if let Some(metadata) = existing {
            // The write replaces the file; put its permissions back.
            let _ = std::fs::set_permissions(&target, metadata.permissions());
        }
        Ok(())
    }

    /// Create a page that does not exist yet.
    pub fn create(&self, path: &str, content: &str) -> Result<(), String> {
        if self.exists(path) {
            return Err(format!("{path} already exists"));
        }
        self.save(path, content, false)
    }

    pub fn rename(&self, from: &str, to: &str, scripts: bool) -> Result<(), String> {
        if !scripts && self.is_executable(from) {
            return Err(format!("{from} is a script; rename it on the host"));
        }
        if self.exists(to) {
            return Err(format!("{to} already exists"));
        }
        let source = self.resolve(from)?;
        let target = self.resolve(to)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&source, &target).map_err(|e| format!("Could not rename {from}: {e}"))
    }

    pub fn delete(&self, path: &str) -> Result<(), String> {
        self.store.delete_page_rel(path).map_err(|e| format!("Could not delete {path}: {e}"))
    }

    /// An image or other file from `pages/`, for editor previews.
    pub fn read_media(&self, path: &str) -> Result<Vec<u8>, String> {
        self.store.read_media_rel(path).map_err(|e| format!("{path}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_node(name: &str) -> (Pages, PathBuf) {
        let dir = std::env::temp_dir().join(format!("rettui-pages-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Pages::open(&dir).unwrap(), dir)
    }

    #[test]
    fn names_are_normalized_and_checked() {
        assert_eq!(normalize_name("/page/about").unwrap(), "about.mu");
        assert_eq!(normalize_name("blog/post-1").unwrap(), "blog/post-1.mu");
        assert_eq!(normalize_name("img/logo.webp").unwrap(), "img/logo.webp");
        assert!(normalize_name("../secret").is_err());
        assert!(normalize_name(".hidden").is_err());
        assert!(normalize_name("x.allowed").is_err());
        assert!(normalize_name("  ").is_err());
    }

    #[test]
    fn serves_pages_files_and_media_locally() {
        let (pages, dir) = temp_node("serve");
        pages.ensure_index("My <node>").unwrap();
        let index = String::from_utf8(pages.serve("/page/index.mu").unwrap()).unwrap();
        assert!(index.starts_with(">My <node>\n"), "{index}");
        pages.ensure_index("Other").unwrap();
        assert!(pages.read("index.mu").unwrap().starts_with(">My"), "an existing index is kept");
        let missing = String::from_utf8(pages.serve("/page/nope.mu").unwrap()).unwrap();
        assert!(missing.contains("The page /page/nope.mu is not available"), "{missing}");
        std::fs::write(dir.join("files/a.txt"), "file body").unwrap();
        assert_eq!(pages.serve("/file/a.txt").unwrap(), b"file body");
        std::fs::write(dir.join("pages/logo.webp"), [1, 2, 3]).unwrap();
        assert_eq!(pages.serve("/media/logo.webp").unwrap(), [1, 2, 3]);
        // Paths outside pages/ are never read.
        std::fs::write(dir.join("secret"), "secret body").unwrap();
        let escaped = pages.serve("/page/../secret").map(|b| String::from_utf8(b).unwrap());
        assert!(!escaped.unwrap_or_default().contains("secret body"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn create_save_rename_delete() {
        let (pages, dir) = temp_node("crud");
        pages.create("blog/first.mu", ">Hello").unwrap();
        assert!(pages.create("blog/first.mu", "again").is_err());
        pages.save("blog/first.mu", ">Hello again", false).unwrap();
        assert_eq!(pages.read("blog/first.mu").unwrap(), ">Hello again");
        pages.rename("blog/first.mu", "news/first.mu", false).unwrap();
        assert!(!pages.exists("blog/first.mu") && pages.exists("news/first.mu"));
        let listed: Vec<String> = pages.list().unwrap().into_iter().map(|p| p.path).collect();
        assert_eq!(listed, ["news/first.mu"]);
        pages.delete("news/first.mu").unwrap();
        assert!(pages.list().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scripts_keep_their_permissions_and_need_permission_to_edit() {
        use std::os::unix::fs::PermissionsExt;
        let (pages, dir) = temp_node("scripts");
        pages.create("dyn.mu", "#!/bin/sh\necho hi\n").unwrap();
        let path = pages.pages_dir().join("dyn.mu");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(pages.is_executable("dyn.mu"));
        assert!(pages.save("dyn.mu", "#!/bin/sh\necho bye\n", false).is_err());
        assert!(pages.rename("dyn.mu", "other.mu", false).is_err());
        pages.save("dyn.mu", "#!/bin/sh\necho bye\n", true).unwrap();
        assert!(pages.is_executable("dyn.mu"), "saving must keep the execute bit");
        assert_eq!(pages.read("dyn.mu").unwrap(), "#!/bin/sh\necho bye\n");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
