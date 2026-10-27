//! Reading the copy of a repository on this computer in the Repos tab: its
//! files, a file's text and the newest commits of a branch or tag. It works
//! with no node, since the copy is a repository on disk (see
//! [`rngit::local`]).

use std::path::PathBuf;

use rngit::local::{CommitInfo, EntryKind, FileView, TreeEntry};

use super::App;
use super::repos::Detail;
use crate::net::git::{clean_text, describe};

/// Most commits the log shows.
const LOG_COMMITS: usize = 200;
/// Most of a file's text read.
pub const FILE_BYTES: usize = 256 * 1024;

/// A branch or tag of the copy, opened.
#[derive(Debug, Clone, PartialEq)]
pub struct Browse {
    /// The full name of the branch or tag.
    pub refname: String,
    /// The folder (or file) shown, from the top; empty at the top.
    pub path: String,
    pub view: BrowseView,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BrowseView {
    Folder(Vec<TreeEntry>),
    File(FileView),
    Log(Vec<CommitInfo>),
    Failed(String),
}

impl Browse {
    /// The short name of the branch or tag.
    pub fn name(&self) -> &str {
        self.refname.trim_start_matches("refs/heads/").trim_start_matches("refs/tags/")
    }
}

/// `path` and a name in it.
pub fn join(path: &str, name: &str) -> String {
    if path.is_empty() { name.to_string() } else { format!("{path}/{name}") }
}

/// The folder a path is in.
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// A file's text as shown: no control characters but line breaks and tabs.
fn cleaned(mut view: FileView) -> FileView {
    view.text = view.text.map(|text| clean_text(&text, FILE_BYTES));
    view
}

/// What the copy in `dir` shows for this: a folder, a file or the log.
pub fn read(dir: &std::path::Path, refname: &str, path: &str, file: bool, log: bool) -> BrowseView {
    let result = if log {
        rngit::local::log(dir, refname, LOG_COMMITS).map(|commits| {
            BrowseView::Log(
                commits
                    .into_iter()
                    .map(|c| CommitInfo { subject: clean_text(&c.subject, 200), author: clean_text(&c.author, 100), ..c })
                    .collect(),
            )
        })
    } else if file {
        rngit::local::file(dir, refname, path, FILE_BYTES).map(|view| BrowseView::File(cleaned(view)))
    } else {
        rngit::local::tree(dir, refname, path)
            .map(|entries| BrowseView::Folder(entries.into_iter().map(|e| TreeEntry { name: clean_text(&e.name, 200), ..e }).collect()))
    };
    result.unwrap_or_else(|e| BrowseView::Failed(describe(e)))
}

impl App {
    /// The folder of a repository's copy, if it has one.
    pub fn copy_dir(&self, url: &rngit::RepoUrl) -> Result<PathBuf, String> {
        let kept = url.to_string();
        if !self.store.repos.iter().any(|r| r.url == kept && r.mirror.is_some()) {
            return Err("There is no copy of it on this computer: m makes one".into());
        }
        self.mirror_dir_for(url)
    }

    /// The folder of the open repository's copy, if it has one.
    fn open_copy(&self) -> Result<(PathBuf, String), String> {
        let url = self.repos.open.as_ref().map(|o| o.url.clone()).ok_or("No repository is open")?;
        Ok((self.copy_dir(&url)?, url.to_string()))
    }

    fn show_browse(&mut self, browse: Browse) {
        if let Some(open) = &mut self.repos.open {
            open.detail = Some(Detail::Browse(browse));
        }
        self.repos.items.select(Some(0));
        self.repos.scroll = 0;
    }

    /// Open a branch or tag of the copy at its top folder.
    pub fn browse_ref(&mut self, refname: &str) -> Result<(), String> {
        let (dir, _) = self.open_copy()?;
        let view = read(&dir, refname, "", false, false);
        self.show_browse(Browse { refname: refname.to_string(), path: String::new(), view });
        Ok(())
    }

    fn browsing(&self) -> Option<Browse> {
        match &self.repos.open.as_ref()?.detail {
            Some(Detail::Browse(browse)) => Some(browse.clone()),
            _ => None,
        }
    }

    /// Enter on the row picked: a folder is opened, a file read.
    pub fn browse_enter(&mut self, row: usize) -> Result<(), String> {
        let Some(browse) = self.browsing() else { return Ok(()) };
        let BrowseView::Folder(entries) = &browse.view else { return Ok(()) };
        let Some(entry) = entries.get(row) else { return Ok(()) };
        let path = join(&browse.path, &entry.name);
        let (dir, _) = self.open_copy()?;
        let view = match entry.kind {
            EntryKind::Dir => read(&dir, &browse.refname, &path, false, false),
            EntryKind::File | EntryKind::Link => read(&dir, &browse.refname, &path, true, false),
            EntryKind::Submodule => return Err("That is another repository: its files aren't in this copy".into()),
        };
        self.show_browse(Browse { path, view, ..browse });
        Ok(())
    }

    /// Back one step: from a file or the log to its folder, from a folder to
    /// the one above it, and from the top to the list of branches and tags.
    pub fn browse_up(&mut self) {
        let Some(browse) = self.browsing() else { return };
        let folder = match &browse.view {
            BrowseView::File(_) => parent(&browse.path).to_string(),
            // The log is of the branch; going back returns to the folder it was opened from.
            BrowseView::Log(_) => browse.path.clone(),
            BrowseView::Folder(_) | BrowseView::Failed(_) if !browse.path.is_empty() => parent(&browse.path).to_string(),
            BrowseView::Folder(_) | BrowseView::Failed(_) => {
                if let Some(open) = &mut self.repos.open {
                    open.detail = None;
                }
                self.repos.scroll = 0;
                return;
            }
        };
        let Ok((dir, _)) = self.open_copy() else { return };
        let view = read(&dir, &browse.refname, &folder, false, false);
        self.show_browse(Browse { path: folder, view, ..browse });
    }

    /// The newest commits of the branch or tag open.
    pub fn browse_log(&mut self) {
        let Some(browse) = self.browsing() else { return };
        if matches!(browse.view, BrowseView::Log(_)) {
            return;
        }
        let Ok((dir, _)) = self.open_copy() else { return };
        let view = read(&dir, &browse.refname, "", false, true);
        self.show_browse(Browse { view, ..browse });
    }
}
