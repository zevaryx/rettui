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

/// Who may see something on the node, as its `.allowed` list says (as
/// NomadNet reads it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Allowed {
    /// No list: anyone.
    Everyone,
    /// Only those identified as these identities (none: no one).
    Only(Vec<[u8; 16]>),
    /// The list is a program that prints it, run for each request: edited
    /// on the host.
    Script,
}

/// Longest `.allowed` list read, in bytes (32 hex characters a line).
const MAX_ALLOWED: u64 = 64 * 1024;

/// The identities in an `.allowed` list: lines of 32 hex characters.
fn allowed_hashes(text: &str) -> Vec<[u8; 16]> {
    let mut hashes: Vec<[u8; 16]> = Vec::new();
    for hash in text.lines().filter_map(|line| crate::net::parse_hash(line.trim())) {
        if !hashes.contains(&hash) {
            hashes.push(hash);
        }
    }
    hashes
}

/// The page for a path that isn't there. (nomad-core's own wording quotes
/// the path with backticks, which Micron reads as formatting codes.)
fn not_found(path: &str) -> String {
    let path = nomad_core::sanitize_micron_text(path);
    format!("#!c=0\n>Not found\n\nThe page {path} is not available on this node.\n")
}

/// Shown when browsing your own node's script pages while scripts are off:
/// rsNomad then sends visitors the script's text instead of running it.
const SCRIPT_NOTICE: &str = "#!c=0\n>Script page\n\nThis page is a program, but \"Run page scripts\" is off, so visitors get its text instead of what it prints. Turn the setting on in the terminal (Status tab) to run it.\n";

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

/// Where something on the node lives: with the pages (`pages/`: pages,
/// and the pictures they show) or in `files/` (to download).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    Pages,
    Files,
}

impl Root {
    pub fn id(self) -> &'static str {
        match self {
            Root::Pages => "pages",
            Root::Files => "files",
        }
    }

    pub fn from_id(id: &str) -> Option<Root> {
        match id {
            "pages" => Some(Root::Pages),
            "files" => Some(Root::Files),
            _ => None,
        }
    }
}

/// A folder as typed, for moving something into it: `blog`, `/blog/`,
/// `a/b`; empty for the top.
pub fn normalize_folder(folder: &str) -> Result<String, String> {
    let folder = folder.trim().trim_matches('/');
    if folder.is_empty() {
        return Ok(String::new());
    }
    nomad_core::validate_content_relative_path(folder).map_err(|e| format!("Not a folder name: {e}"))?;
    Ok(folder.to_string())
}

/// The folder a path is in (empty for the top).
pub fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// Every folder holding something, and those they're in, in order.
pub fn folders(entries: &[PageInfo]) -> Vec<String> {
    let mut folders: Vec<String> = Vec::new();
    for entry in entries {
        let mut folder = folder_of(&entry.path);
        while !folder.is_empty() {
            if !folders.iter().any(|f| f == folder) {
                folders.push(folder.to_string());
            }
            folder = folder_of(folder);
        }
    }
    folders.sort();
    folders
}

/// Where, in `text`, the address `from` (`:/page/a.mu`) is one on this
/// node: a link's or picture's own (right after its `` ` ``), or one naming
/// the node (`` `<node>:/page/a.mu ``), but not another node's page of the
/// same name. Only where `from` ends there (so `:/page/a.mu` isn't taken
/// for the start of `:/page/a.mu2`). Byte offsets, in order.
fn address_matches(text: &str, from: &str, node: Option<&str>) -> Vec<usize> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(start) = text[at..].find(from).map(|i| at + i) {
        let after = &text[start + from.len()..];
        let ends = after.chars().next().is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '/')));
        let before = &text[..start];
        let here = before.ends_with('`')
            || node.is_some_and(|node| {
                before.len() > node.len()
                    && before.is_char_boundary(before.len() - node.len())
                    && before[before.len() - node.len()..].eq_ignore_ascii_case(node)
                    && before[..before.len() - node.len()].ends_with('`')
            });
        if ends && here {
            found.push(start);
        }
        at = start + from.len();
    }
    found
}

/// `text` with every address `from` on this node made `to` (see
/// [`address_matches`]). How many were.
fn replace_address(text: &str, from: &str, to: &str, node: Option<&str>) -> (String, usize) {
    let found = address_matches(text, from, node);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for &start in &found {
        out.push_str(&text[at..start]);
        out.push_str(to);
        at = start + from.len();
    }
    out.push_str(&text[at..]);
    (out, found.len())
}

/// The addresses links to something in `root` use.
fn routes(root: Root) -> &'static [&'static str] {
    match root {
        Root::Pages => &[":/page/", ":/media/"],
        Root::Files => &[":/file/"],
    }
}

/// Where a place in `text` (a byte offset, such as the cursor) is once
/// links to `from` became links to `to`, as [`Pages::relink`] changes them:
/// moved along with the text before it, and to the end of an address it
/// was inside.
pub fn relinked_offset(text: &str, offset: usize, root: Root, from: &str, to: &str, node: Option<&str>) -> usize {
    let mut text = text.to_string();
    let mut offset = offset.min(text.len());
    for route in routes(root) {
        let (from, to) = (format!("{route}{from}"), format!("{route}{to}"));
        let mut moved = offset;
        for start in address_matches(&text, &from, node) {
            if start >= offset {
                break;
            }
            let end = start + from.len();
            if end <= offset {
                moved = moved + to.len() - from.len();
            } else {
                // Inside it: after the new one.
                moved = moved - (offset - start) + to.len();
            }
        }
        offset = moved;
        text = replace_address(&text, &from, &to, node).0;
    }
    offset
}

/// Pictures a page can show (NomadNet's `/media`, which reads them from
/// `pages/`).
const IMAGE_EXTENSIONS: [&str; 7] = ["webp", "png", "jpg", "jpeg", "bmp", "gif", "tiff"];

/// Where pictures added from the editors go, in `pages/`.
pub const IMAGES_FOLDER: &str = "images";

/// Whether a file of this name is a picture a page can show.
pub fn is_image_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && IMAGE_EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// A name for a file added to the node: its own, made safe for a path on
/// the node (letters, digits, `.`, `-` and `_`; spaces become `-`).
pub fn upload_name(name: &str) -> Result<String, String> {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let mut safe: String = name
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    safe = safe.trim_start_matches(['.', '-']).to_string();
    if safe.len() > 80 {
        // Keep its extension.
        let ext = safe.rsplit_once('.').map(|(_, e)| format!(".{e}")).filter(|e| e.len() <= 10).unwrap_or_default();
        safe = format!("{}{ext}", &safe[..80 - ext.len()]);
    }
    if !safe.chars().any(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("{name} needs a name with letters or digits"));
    }
    if safe.ends_with(".allowed") {
        return Err("A file can't be called .allowed (NomadNet keeps who may see a page in those)".into());
    }
    nomad_core::validate_content_relative_path(&safe).map_err(|e| format!("Not a name for the node: {e}"))?;
    Ok(safe)
}

/// `name`, or `name-2`, `name-3`... for the first that `taken` says isn't.
fn unused(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) => (stem, format!(".{ext}")),
        None => (name, String::new()),
    };
    (2..).map(|i| format!("{stem}-{i}{ext}")).find(|n| !taken(n)).expect("an unused name exists")
}

fn human_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 { format!("{} MB", bytes / (1024 * 1024)) } else { format!("{} KB", bytes.div_ceil(1024)) }
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
    /// cannot reach over Reticulum). Scripts are not run here: see
    /// [`Pages::script`].
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

    /// The script a `/page/…` path runs, if it is one.
    pub fn script(&self, path: &str) -> Option<PathBuf> {
        let rel = nomad_core::strip_page_prefix(path).ok()?;
        self.resolve(rel).ok().filter(|p| is_executable(p))
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

    /// Rename a page or picture, leaving no empty folder behind.
    fn rename_page(&self, from: &str, to: &str, scripts: bool) -> Result<(), String> {
        self.rename(from, to, scripts)?;
        self.carry_allowed(Root::Pages, from, Some(to))?;
        self.prune(Root::Pages, from);
        Ok(())
    }

    /// An image or other file from `pages/`, for editor previews.
    pub fn read_media(&self, path: &str) -> Result<Vec<u8>, String> {
        self.store.read_media_rel(path).map_err(|e| format!("{path}: {e}"))
    }

    /// What's in `files/`, to download.
    pub fn list_files(&self) -> Result<Vec<PageInfo>, String> {
        let entries = self.store.list_files().map_err(|e| e.to_string())?;
        Ok(entries
            .into_iter()
            .map(|entry| PageInfo { executable: false, text: false, path: entry.path, size: entry.size, modified_ms: entry.modified_ms })
            .collect())
    }

    fn files_dir(&self) -> &Path {
        &self.store.roots().files_dir
    }

    fn resolve_in(&self, root: Root, path: &str) -> Result<PathBuf, String> {
        let dir = match root {
            Root::Pages => self.pages_dir(),
            Root::Files => self.files_dir(),
        };
        resolve_under_root(dir, path).map_err(|e| format!("{path}: {e}"))
    }

    /// Rename (or move to another folder) something in `pages/` or in
    /// `files/`; never over something there. Scripts only with `scripts`.
    pub fn rename_in(&self, root: Root, from: &str, to: &str, scripts: bool) -> Result<(), String> {
        if root == Root::Pages {
            return self.rename_page(from, to, scripts);
        }
        nomad_core::validate_content_relative_path(to).map_err(|e| format!("Not a name for the node: {e}"))?;
        let source = self.resolve_in(root, from)?;
        if !source.is_file() {
            return Err(format!("{from} isn't a file on the node"));
        }
        let target = self.resolve_in(root, to)?;
        if target.exists() {
            return Err(format!("{to} already exists"));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&source, &target).map_err(|e| format!("Could not move {from}: {e}"))?;
        self.carry_allowed(root, from, Some(to))?;
        self.prune(root, from);
        Ok(())
    }

    /// Delete something in `pages/` or in `files/`.
    pub fn delete_in(&self, root: Root, path: &str) -> Result<(), String> {
        match root {
            Root::Pages => self.delete(path),
            Root::Files => self.store.delete_file_rel(path).map_err(|e| format!("Could not delete {path}: {e}")),
        }?;
        self.carry_allowed(root, path, None)?;
        self.prune(root, path);
        Ok(())
    }

    /// The `.allowed` list beside something on the node.
    fn allowed_path(&self, root: Root, path: &str) -> Result<PathBuf, String> {
        let target = self.resolve_in(root, path)?;
        let mut name = target.into_os_string();
        name.push(".allowed");
        Ok(PathBuf::from(name))
    }

    /// Who may see `path` (a page, picture or file).
    pub fn allowed(&self, root: Root, path: &str) -> Result<Allowed, String> {
        if !self.resolve_in(root, path)?.is_file() {
            return Err(format!("{path} isn't on the node"));
        }
        let list = self.allowed_path(root, path)?;
        let Ok(metadata) = std::fs::symlink_metadata(&list) else { return Ok(Allowed::Everyone) };
        if !metadata.is_file() || is_executable(&list) {
            return Ok(Allowed::Script);
        }
        if metadata.len() > MAX_ALLOWED {
            return Err(format!("{path}'s list of who may see it is too long to read here"));
        }
        let text = std::fs::read_to_string(&list).map_err(|e| format!("Could not read {path}'s list of who may see it: {e}"))?;
        Ok(Allowed::Only(allowed_hashes(&text)))
    }

    /// Say who may see `path`: anyone (`None`, no list), or only these
    /// identities. A list that's a program isn't changed.
    pub fn set_allowed(&self, root: Root, path: &str, only: Option<&[[u8; 16]]>) -> Result<(), String> {
        if self.allowed(root, path)? == Allowed::Script {
            return Err(format!("{path}'s list of who may see it is a program; change it on the host"));
        }
        let list = self.allowed_path(root, path)?;
        match only {
            None => match std::fs::remove_file(&list) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("Could not remove {path}'s list: {e}")),
                _ => Ok(()),
            },
            Some(hashes) => {
                let text: String = hashes.iter().map(|h| hex::encode(h) + "\n").collect();
                crate::config::write_atomic(&list, text.as_bytes()).map_err(|e| format!("Could not save {path}'s list: {e}"))
            }
        }
    }

    /// Move `from`'s `.allowed` list along with it to `to` (or, with no
    /// `to`, delete it), so what was kept to some stays so.
    fn carry_allowed(&self, root: Root, from: &str, to: Option<&str>) -> Result<(), String> {
        let list = self.allowed_path(root, from)?;
        if std::fs::symlink_metadata(&list).is_err() {
            return Ok(());
        }
        match to {
            Some(to) => crate::config::replace(&list, &self.allowed_path(root, to)?),
            None => std::fs::remove_file(&list),
        }
        .map_err(|e| format!("Could not move {from}'s list of who may see it: {e}"))
    }

    /// Remove folders left empty by moving or deleting `path` (not the
    /// roots themselves).
    fn prune(&self, root: Root, path: &str) {
        let mut folder = folder_of(path);
        while !folder.is_empty() {
            let Ok(dir) = self.resolve_in(root, folder) else { return };
            if std::fs::remove_dir(&dir).is_err() {
                return; // not empty
            }
            folder = folder_of(folder);
        }
    }

    /// After `from` became `to` (in `root`): links to it in the pages made to
    /// lead to `to` (those naming this node by its address, `node`, too).
    /// Scripts aren't changed, nor pages in `skip` (one open with changes
    /// not saved). The pages changed.
    pub fn relink(&self, root: Root, from: &str, to: &str, skip: Option<&str>, node: Option<&str>) -> Result<Vec<String>, String> {
        let routes = routes(root);
        let mut changed = Vec::new();
        for page in self.list()?.into_iter().filter(|p| p.text && !p.executable && Some(p.path.as_str()) != skip) {
            let Ok(text) = self.read(&page.path) else { continue };
            let mut updated = text.clone();
            for route in routes {
                updated = replace_address(&updated, &format!("{route}{from}"), &format!("{route}{to}"), node).0;
            }
            if updated != text {
                self.save(&page.path, &updated, false)?;
                changed.push(page.path);
            }
        }
        Ok(changed)
    }

    /// Add a picture for pages to show, in `pages/images/` (under its own
    /// name, or a free one like it: never over one there). Its path in
    /// `pages/`, which pages show as `:/media/<path>`.
    pub fn add_image(&self, name: &str, data: &[u8]) -> Result<String, String> {
        let max = self.store.roots().max_page_bytes;
        if data.len() > max {
            return Err(format!("{name} is {}: a picture in a page can be {} at most", human_size(data.len()), human_size(max)));
        }
        let rel = unused(&format!("{IMAGES_FOLDER}/{name}"), |rel| self.exists(rel));
        self.store.write_page_rel(&rel, data).map_err(|e| format!("Could not add {rel}: {e}"))?;
        Ok(rel)
    }

    /// Add a file to download, in `files/` (never over one there). Its path
    /// in `files/`, which pages link to as `:/file/<path>`.
    pub fn add_file(&self, name: &str, data: &[u8]) -> Result<String, String> {
        let roots = self.store.roots();
        if data.len() > roots.max_file_bytes {
            return Err(format!(
                "{name} is {}: a file on the node can be {} at most",
                human_size(data.len()),
                human_size(roots.max_file_bytes)
            ));
        }
        let files = roots.files_dir.clone();
        let rel = unused(name, |rel| resolve_under_root(&files, rel).is_ok_and(|p| p.exists()));
        self.store.write_file_rel(&rel, data).map_err(|e| format!("Could not add {rel}: {e}"))?;
        Ok(rel)
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
    fn files_added_are_kept_under_safe_names_never_over_others() {
        let (pages, dir) = temp_node("add");
        assert_eq!(upload_name("My Photo (1).JPG").unwrap(), "My-Photo-1.JPG");
        assert_eq!(upload_name("C:\\Users\\me\\notes.txt").unwrap(), "notes.txt");
        assert_eq!(upload_name("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(upload_name(".hidden").unwrap(), "hidden");
        assert!(upload_name("..").is_err() && upload_name("!!!").is_err() && upload_name("who.allowed").is_err());
        assert!(is_image_name("a.WEBP") && is_image_name("b.jpeg") && !is_image_name("c.pdf") && !is_image_name("png"));
        // Pictures go with the pages, files to download in files/.
        assert_eq!(pages.add_image("logo.png", b"png").unwrap(), "images/logo.png");
        assert_eq!(pages.add_image("logo.png", b"png 2").unwrap(), "images/logo-2.png");
        assert_eq!(pages.read_media("images/logo-2.png").unwrap(), b"png 2");
        assert_eq!(pages.add_file("guide.pdf", b"pdf").unwrap(), "guide.pdf");
        assert_eq!(pages.add_file("guide.pdf", b"pdf 2").unwrap(), "guide-2.pdf");
        assert_eq!(pages.serve("/file/guide-2.pdf").unwrap(), b"pdf 2");
        // Too big: said so, nothing written.
        let big = vec![0u8; 600 * 1024];
        assert!(pages.add_image("big.png", &big).unwrap_err().contains("512 KB at most"));
        assert!(!pages.exists("images/big.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_links_to_this_node_follow_a_move() {
        let own = "ab".repeat(16);
        let other = "cd".repeat(16);
        let text =
            format!("`[here`:/page/a.mu] `[mine`{own}:/page/a.mu] `[theirs`{other}:/page/a.mu] `(pic`w=40`:/page/a.mu) said :/page/a.mu");
        let (moved, count) = replace_address(&text, ":/page/a.mu", ":/page/b/a.mu", Some(&own));
        assert_eq!(count, 3);
        assert_eq!(
            moved,
            format!(
                "`[here`:/page/b/a.mu] `[mine`{own}:/page/b/a.mu] `[theirs`{other}:/page/a.mu] `(pic`w=40`:/page/b/a.mu) said :/page/a.mu"
            )
        );
        // A place in the text moves with it, and from inside a changed
        // address to its end.
        let at = |needle: &str| text.find(needle).unwrap();
        let moves = |offset| relinked_offset(&text, offset, Root::Pages, "a.mu", "b/a.mu", Some(&own));
        assert_eq!(moves(at("`[mine")), moved.find("`[mine").unwrap());
        assert_eq!(moves(at("a.mu]") + 2), moved.find("b/a.mu]").unwrap() + "b/a.mu".len());
        assert_eq!(moves(text.len()), moved.len());
        // Without the node's address, only the links without one.
        assert_eq!(replace_address(&text, ":/page/a.mu", ":/page/b.mu", None).1, 2);
    }

    #[test]
    fn moving_keeps_links_working_and_leaves_no_empty_folders() {
        let (pages, dir) = temp_node("move");
        pages
            .save(
                "index.mu",
                "`[About`:/page/about.mu] `[About 2`:/page/about.mu2] `(logo`:/media/images/logo.png) `[Get`:/file/guide.pdf]",
                false,
            )
            .unwrap();
        pages.save("about.mu", "Back: `[home`:/page/index.mu]", false).unwrap();
        pages.add_image("logo.png", b"png").unwrap();
        pages.add_file("guide.pdf", b"pdf").unwrap();
        // A page into a folder: the links to it (not to about.mu2) follow.
        pages.rename_in(Root::Pages, "about.mu", "info/about.mu", false).unwrap();
        assert_eq!(pages.relink(Root::Pages, "about.mu", "info/about.mu", None, None).unwrap(), ["index.mu"]);
        // A picture to another folder: the old one goes, being empty.
        pages.rename_in(Root::Pages, "images/logo.png", "art/logo.png", false).unwrap();
        pages.relink(Root::Pages, "images/logo.png", "art/logo.png", None, None).unwrap();
        assert!(!pages.pages_dir().join("images").exists());
        // A file into a folder of files/.
        pages.rename_in(Root::Files, "guide.pdf", "docs/guide.pdf", false).unwrap();
        pages.relink(Root::Files, "guide.pdf", "docs/guide.pdf", None, None).unwrap();
        assert_eq!(
            pages.read("index.mu").unwrap(),
            "`[About`:/page/info/about.mu] `[About 2`:/page/about.mu2] `(logo`:/media/art/logo.png) `[Get`:/file/docs/guide.pdf]"
        );
        assert_eq!(pages.serve("/file/docs/guide.pdf").unwrap(), b"pdf");
        let files: Vec<String> = pages.list_files().unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(files, ["docs/guide.pdf"]);
        assert_eq!(folders(&pages.list().unwrap()), ["art", "info"]);
        // Not over something there; a page open with changes is left be.
        pages.add_file("other.pdf", b"x").unwrap();
        assert!(pages.rename_in(Root::Files, "other.pdf", "docs/guide.pdf", false).is_err());
        pages.rename_in(Root::Pages, "index.mu", "home.mu", false).unwrap();
        assert!(pages.relink(Root::Pages, "index.mu", "home.mu", Some("info/about.mu"), None).unwrap().is_empty());
        // Deleting the last thing in a folder takes the folder too.
        pages.delete_in(Root::Files, "docs/guide.pdf").unwrap();
        assert!(!pages.files_dir().join("docs").exists());
        assert_eq!(normalize_folder("/blog/").unwrap(), "blog");
        assert_eq!(normalize_folder(" ").unwrap(), "");
        assert!(normalize_folder("../up").is_err());
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn who_may_see_follows_a_page_moved_and_goes_with_it_deleted() {
        let (pages, dir) = temp_node("allowed");
        pages.create("club.mu", ">Members").unwrap();
        assert!(pages.allowed(Root::Pages, "gone.mu").is_err());
        assert_eq!(pages.allowed(Root::Pages, "club.mu").unwrap(), Allowed::Everyone);
        let (a, b) = ([1u8; 16], [2u8; 16]);
        pages.set_allowed(Root::Pages, "club.mu", Some(&[a, b])).unwrap();
        let list = pages.pages_dir().join("club.mu.allowed");
        assert_eq!(std::fs::read_to_string(&list).unwrap(), format!("{}\n{}\n", hex::encode(a), hex::encode(b)));
        // As NomadNet writes them too: other lines, spaces and repeats.
        std::fs::write(&list, format!("# members\n  {}  \nnot a hash\n{}\n", hex::encode(b), hex::encode(b))).unwrap();
        assert_eq!(pages.allowed(Root::Pages, "club.mu").unwrap(), Allowed::Only(vec![b]));
        // The list never shows as a page of its own.
        let listed: Vec<String> = pages.list().unwrap().into_iter().map(|p| p.path).collect();
        assert_eq!(listed, ["club.mu"]);

        pages.rename_in(Root::Pages, "club.mu", "members/club.mu", false).unwrap();
        assert!(!list.exists());
        assert_eq!(pages.allowed(Root::Pages, "members/club.mu").unwrap(), Allowed::Only(vec![b]));
        pages.set_allowed(Root::Pages, "members/club.mu", Some(&[])).unwrap();
        assert_eq!(pages.allowed(Root::Pages, "members/club.mu").unwrap(), Allowed::Only(vec![]));
        pages.set_allowed(Root::Pages, "members/club.mu", None).unwrap();
        assert_eq!(pages.allowed(Root::Pages, "members/club.mu").unwrap(), Allowed::Everyone);
        pages.set_allowed(Root::Pages, "members/club.mu", Some(&[a])).unwrap();
        pages.delete_in(Root::Pages, "members/club.mu").unwrap();
        assert!(!pages.pages_dir().join("members/club.mu.allowed").exists());

        // Files to download have lists too.
        let file = pages.add_file("guide.pdf", b"pdf").unwrap();
        pages.set_allowed(Root::Files, &file, Some(&[a])).unwrap();
        pages.rename_in(Root::Files, &file, "manual.pdf", false).unwrap();
        assert_eq!(pages.allowed(Root::Files, "manual.pdf").unwrap(), Allowed::Only(vec![a]));
        pages.delete_in(Root::Files, "manual.pdf").unwrap();
        assert!(std::fs::read_dir(dir.join("files")).unwrap().next().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_list_that_is_a_program_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let (pages, dir) = temp_node("allowed-script");
        pages.create("club.mu", ">Members").unwrap();
        let list = pages.pages_dir().join("club.mu.allowed");
        std::fs::write(&list, "#!/bin/sh\necho 01010101010101010101010101010101\n").unwrap();
        std::fs::set_permissions(&list, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(pages.allowed(Root::Pages, "club.mu").unwrap(), Allowed::Script);
        assert!(pages.set_allowed(Root::Pages, "club.mu", None).is_err());
        assert!(list.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
