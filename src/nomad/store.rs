//! The files of a hosted node: `pages/` (pages and their pictures) and
//! `files/` (to download), listed, read and written by name. Every name
//! goes through [`paths::resolve`], so nothing outside the folder, hidden
//! or behind a symlink is touched.

use std::io::{Read, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;

use super::paths::{self, PathError};

/// The most a page may be (and the most a script page may print).
pub const MAX_PAGE_BYTES: usize = 512 * 1024;
/// The most a file or a picture may be.
pub const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;

/// The most files listed, and how deep folders are looked into.
const MAX_ENTRIES: usize = 10_000;
const MAX_DEPTH: usize = 32;

/// A file on the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its name: the path from `pages/` or `files/`, with `/` between folders.
    pub path: String,
    pub size: u64,
    pub modified_ms: Option<u64>,
}

/// Every file under `root`, by name. Hidden files, `.allowed` lists,
/// symlinks and names the node wouldn't serve are left out.
pub fn list(root: &Path) -> Result<Vec<Entry>, PathError> {
    let mut entries = Vec::new();
    if root.is_dir() && !std::fs::symlink_metadata(root)?.file_type().is_symlink() {
        walk(root, "", 0, &mut entries)?;
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn walk(dir: &Path, prefix: &str, depth: usize, entries: &mut Vec<Entry>) -> Result<(), PathError> {
    if depth > MAX_DEPTH {
        return Err(PathError::Invalid("its folders go too deep"));
    }
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if paths::kept_aside(&name) {
            continue;
        }
        // Of the entry itself: a symlink is neither a file nor a folder here.
        let metadata = std::fs::symlink_metadata(item.path())?;
        let path = format!("{prefix}{name}");
        if metadata.is_dir() {
            walk(&item.path(), &format!("{path}/"), depth + 1, entries)?;
        } else if metadata.is_file() && paths::check_name(&path).is_ok() {
            if entries.len() >= MAX_ENTRIES {
                return Err(PathError::Invalid("there are too many files"));
            }
            let modified_ms = metadata.modified().ok().and_then(|at| at.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64);
            entries.push(Entry { path, size: metadata.len(), modified_ms });
        }
    }
    Ok(())
}

/// The file `name` under `root`, if it's a file of `max` bytes at most.
pub fn read(root: &Path, name: &str, max: usize) -> Result<Vec<u8>, PathError> {
    let path = paths::resolve(root, name)?;
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file() {
        return Err(PathError::NotFound);
    }
    if metadata.len() > max as u64 {
        return Err(PathError::TooBig { size: metadata.len(), max });
    }
    // It may have grown since: never more than `max` is read.
    let mut data = Vec::new();
    std::fs::File::open(&path)?.take(max as u64 + 1).read_to_end(&mut data)?;
    if data.len() > max {
        return Err(PathError::TooBig { size: data.len() as u64, max });
    }
    Ok(data)
}

/// Write `name` under `root`, making its folders. The new file takes the
/// old one's place whole, so a visitor never gets half a page.
pub fn write(root: &Path, name: &str, data: &[u8], max: usize) -> Result<(), PathError> {
    if data.len() > max {
        return Err(PathError::TooBig { size: data.len() as u64, max });
    }
    let path = paths::resolve(root, name)?;
    let (Some(folder), Some(file_name)) = (path.parent(), path.file_name()) else { return Err(PathError::Invalid("it has no name")) };
    std::fs::create_dir_all(folder)?;
    // Hidden while it's written, so it's never listed or served.
    let temp = folder.join(format!(".{}.{:08x}.tmp", file_name.to_string_lossy(), rand::random::<u32>()));
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        crate::config::replace(&temp, &path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    Ok(written?)
}

/// Delete the file `name` under `root`.
pub fn delete(root: &Path, name: &str) -> Result<(), PathError> {
    let path = paths::resolve(root, name)?;
    if !std::fs::symlink_metadata(&path)?.is_file() {
        return Err(PathError::NotFound);
    }
    Ok(std::fs::remove_file(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-test-nomad-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn files_written_read_listed_and_deleted() {
        let root = temp("crud").join("pages");
        assert!(list(&root).unwrap().is_empty());
        assert!(matches!(read(&root, "index.mu", 100), Err(PathError::NotFound)));
        write(&root, "index.mu", b">Hello\n", 100).unwrap();
        write(&root, "blog/2026/one.mu", b"one", 100).unwrap();
        write(&root, "index.mu", b">Hello again\n", 100).unwrap();
        assert_eq!(read(&root, "index.mu", 100).unwrap(), b">Hello again\n");
        assert_eq!(read(&root, "/blog/2026/one.mu", 100).unwrap(), b"one");
        // Nothing is left behind by a write.
        let listed = list(&root).unwrap();
        assert_eq!(listed.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["blog/2026/one.mu", "index.mu"]);
        assert_eq!(listed[1].size, 13);
        assert!(listed[1].modified_ms.is_some());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);

        // A folder isn't a file; too big is refused both ways.
        assert!(matches!(read(&root, "blog", 100), Err(PathError::NotFound)));
        assert!(matches!(delete(&root, "blog"), Err(PathError::NotFound)));
        assert!(matches!(read(&root, "index.mu", 5), Err(PathError::TooBig { size: 13, max: 5 })));
        assert!(matches!(write(&root, "big.mu", &[0; 101], 100), Err(PathError::TooBig { .. })));
        assert!(!root.join("big.mu").exists());
        assert!(write(&root, "../out.mu", b"x", 100).is_err() && write(&root, "club.mu.allowed", b"x", 100).is_err());

        delete(&root, "index.mu").unwrap();
        assert!(matches!(delete(&root, "index.mu"), Err(PathError::NotFound)));
        assert_eq!(list(&root).unwrap().len(), 1);
        std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }

    #[test]
    fn hidden_files_lists_and_symlinks_are_left_out() {
        let dir = temp("hidden");
        let root = dir.join("pages");
        write(&root, "club.mu", b"club", 100).unwrap();
        std::fs::write(root.join("club.mu.allowed"), "abc").unwrap();
        std::fs::write(root.join(".draft.mu"), "draft").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "x").unwrap();
        std::fs::write(root.join("back\\slash.mu"), "x").unwrap_or_default();
        #[cfg(unix)]
        {
            std::fs::write(dir.join("secret"), "secret").unwrap();
            std::os::unix::fs::symlink(dir.join("secret"), root.join("link.mu")).unwrap();
            std::os::unix::fs::symlink(&dir, root.join("up")).unwrap();
            assert!(read(&root, "link.mu", 100).is_err() && read(&root, "up/secret", 100).is_err());
            assert!(write(&root, "link.mu", b"x", 100).is_err());
            assert_eq!(std::fs::read(dir.join("secret")).unwrap(), b"secret");
        }
        assert_eq!(list(&root).unwrap().iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["club.mu"]);
        assert!(read(&root, "club.mu.allowed", 100).is_err() && read(&root, ".draft.mu", 100).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
