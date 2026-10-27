//! The Repos tab: repositories on rngit nodes (Git over Reticulum, see
//! [`crate::net::git`]). Those kept, and the rngit nodes heard, on the left;
//! the one open on the right, with its branches and tags, releases (whose
//! files download checked against the release's signature), work
//! documents and permissions. Documents and releases can be written too,
//! signed with the identity the node knows you by.
//!
//! A node doesn't list its repositories: they're added by their
//! `rns://<node>/<group>/<repo>` URL, or opened from a link to one (in a
//! message, a chat or a page).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;
use ratatui::widgets::ListState;
use rngit::RepoUrl;
use rngit::client::{ReleaseDetail, ReleaseList, WorkKind, WorkList, WorkScope, WorkSummary};

use super::{App, PromptKind, Tab, now};
use crate::app::browser::Location;
use crate::app::repo_files::{Browse, BrowseView};
use crate::net::git::{Doc, GitReply, GitRequest, Mirrored, Pushed};
use crate::net::heard::{Heard, Kind};
use crate::net::{self, Hash, NetCommand, PeerKind};
use crate::store::{SavedMirror, SavedRepo};

/// Most repositories kept.
pub const MAX_REPOS: usize = 200;
/// Most rngit nodes listed (the nearest kept).
const MAX_NODES: usize = 100;
/// Largest release published, its files together, in bytes (they're held
/// in memory, and sent from the web UI in one go).
pub const MAX_RELEASE: usize = 32 * 1024 * 1024;
/// Longest title, and text, of a document or comment, in characters.
pub const MAX_TITLE: usize = 200;
pub const MAX_TEXT: usize = crate::net::git::MAX_TEXT;

/// What the open repository shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepoPane {
    #[default]
    Branches,
    Releases,
    Work,
    Permissions,
}

impl RepoPane {
    pub const ALL: [RepoPane; 4] = [RepoPane::Branches, RepoPane::Releases, RepoPane::Work, RepoPane::Permissions];

    pub fn title(self) -> &'static str {
        match self {
            RepoPane::Branches => "Branches and tags",
            RepoPane::Releases => "Releases",
            RepoPane::Work => "Work",
            RepoPane::Permissions => "Permissions",
        }
    }
}

/// Where keys go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReposFocus {
    #[default]
    List,
    Repo,
}

/// Something asked of a node: not yet, on its way, come, or failed.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Load<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(value) => Some(value),
            _ => None,
        }
    }
}

/// An rngit node heard announcing.
#[derive(Debug, Clone, PartialEq)]
pub struct GitNode {
    pub hash: Hash,
    pub name: Option<String>,
    pub hops: u8,
    /// When last heard (Unix seconds).
    pub at: f64,
}

/// One thing of the open repository, opened.
#[derive(Debug, Clone, PartialEq)]
pub enum Detail {
    Release {
        tag: String,
        load: Load<ReleaseDetail>,
    },
    Doc {
        id: u64,
        scope: WorkScope,
        load: Load<Doc>,
    },
    /// A branch or tag of the copy on this computer.
    Browse(Browse),
}

/// The repository open on the right.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRepo {
    pub url: RepoUrl,
    pub refs: Load<rngit::RemoteRefs>,
    pub releases: Load<ReleaseList>,
    pub work: Load<WorkList>,
    pub perms: Load<String>,
    /// Which work documents the Work pane lists.
    pub scope: WorkScope,
    pub detail: Option<Detail>,
    /// Whether the branches and tags are the copy's, because the node didn't answer.
    pub from_copy: bool,
}

impl OpenRepo {
    fn new(url: RepoUrl) -> Self {
        OpenRepo {
            url,
            refs: Load::Idle,
            releases: Load::Idle,
            work: Load::Idle,
            perms: Load::Idle,
            scope: WorkScope::Active,
            detail: None,
            from_copy: false,
        }
    }

    /// The work documents the Work pane lists.
    pub fn documents(&self) -> &[WorkSummary] {
        let Some(work) = self.work.ready() else { return &[] };
        match self.scope {
            WorkScope::Completed => &work.completed,
            WorkScope::Proposed => &work.proposed,
            WorkScope::Active | WorkScope::All => &work.active,
        }
    }
}

/// What an answer is for.
#[derive(Debug, Clone)]
enum Asked {
    Refs(RepoUrl),
    Releases(RepoUrl),
    Work(RepoUrl),
    Perms(RepoUrl),
    Release(RepoUrl, String),
    Doc(RepoUrl, u64),
    Artifact,
    /// A change, and what it is (said while it goes).
    Write(RepoUrl, String),
    /// A local copy being made or brought up to date.
    Mirror(RepoUrl),
    /// A local copy being sent to this repository.
    Push(RepoUrl),
}

#[derive(Debug, Default)]
pub struct Repos {
    pub focus: ReposFocus,
    pub pane: RepoPane,
    /// The kept repositories, then the nodes heard.
    pub side: ListState,
    /// The open pane's list (a release's files, when one is open).
    pub items: ListState,
    /// How far an open document is scrolled.
    pub scroll: u16,
    pub open: Option<OpenRepo>,
    /// rngit nodes heard, nearest first.
    pub nodes: Vec<GitNode>,
    asked: HashMap<u64, Asked>,
}

impl Repos {
    /// Changes being made, and what each is.
    pub fn writing(&self) -> Vec<String> {
        self.asked
            .values()
            .filter_map(|a| match a {
                Asked::Write(_, what) => Some(what.clone()),
                Asked::Mirror(repo) => Some(format!("Copying {}/{}", repo.group, repo.repo)),
                Asked::Push(repo) => Some(format!("Sending to {}/{}", repo.group, repo.repo)),
                _ => None,
            })
            .collect()
    }

    /// A release file being downloaded.
    pub fn downloading(&self) -> bool {
        self.asked.values().any(|a| matches!(a, Asked::Artifact))
    }
}

/// What to add: an `rns://` URL, or one without its `rns://`.
pub fn parse_repo(text: &str) -> Result<RepoUrl, String> {
    let text = text.trim();
    let url = if text.to_ascii_lowercase().starts_with("rns://") { format!("rns://{}", &text[6..]) } else { format!("rns://{text}") };
    url.trim_end_matches('/')
        .parse()
        .map_err(|_| "A repository is rns://<node>/<group>/<repo>: the node's 32 hex characters, then two names".to_string())
}

/// Paths written in a prompt, separated by spaces (one with spaces in it
/// in quotes).
pub fn split_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() {
                    paths.push(std::mem::take(&mut current));
                }
            }
            (_, c) => current.push(c),
        }
    }
    if !current.is_empty() {
        paths.push(current);
    }
    paths
}

/// Where stock rngit keeps its identity: `$RNGIT_CONFIG/client_identity`,
/// or `~/.rngit/client_identity`.
pub fn rngit_identity_path() -> Option<PathBuf> {
    match std::env::var_os("RNGIT_CONFIG") {
        Some(dir) => Some(PathBuf::from(dir).join("client_identity")),
        None => directories::BaseDirs::new().map(|d| d.home_dir().join(".rngit").join("client_identity")),
    }
}

/// A name Windows won't make a file or folder of, whatever follows its dot.
fn reserved_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or_default().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit())
}

/// The folder a repository's copy lives in, `<repos>/<node>/<group>/<name>.git`.
/// Its names come from a URL and may come from the network, so they're
/// checked to be names a folder can have on every system.
pub fn mirror_dir(repos: &Path, url: &RepoUrl) -> Result<PathBuf, String> {
    for part in [&url.group, &url.repo] {
        if part.ends_with('.') || reserved_name(part) {
            return Err(format!("{part:?} can't be the name of a folder here"));
        }
    }
    let dir = repos.join(hex::encode(url.destination)).join(&url.group).join(format!("{}.git", url.repo));
    if !dir.starts_with(repos) {
        return Err("That repository's name leads out of the folder for copies".into());
    }
    Ok(dir)
}

/// Delete a copy's folder, and the folders around it that are empty then.
fn remove_mirror_dir(repos: &Path, dir: &Path) -> std::io::Result<()> {
    if !dir.starts_with(repos) || dir == repos {
        return Ok(());
    }
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    // The group's folder, then the node's, when nothing else is in them.
    for parent in dir.ancestors().skip(1).take(2) {
        if parent == repos || std::fs::remove_dir(parent).is_err() {
            break;
        }
    }
    Ok(())
}

/// What to do with a branch the node has that differs from the copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushHow {
    /// Add what is new; leave branches that differ.
    Add,
    /// Also overwrite branches that differ.
    Overwrite,
    /// Also overwrite them, and delete what the copy lacks, so the node ends up the same.
    Same,
}

impl PushHow {
    /// `force` and `prune` for the library.
    pub fn flags(self) -> (bool, bool) {
        (self != PushHow::Add, self == PushHow::Same)
    }

    pub fn parse(text: &str) -> Option<PushHow> {
        match text.trim().to_lowercase().as_str() {
            "" | "add" | "a" => Some(PushHow::Add),
            "overwrite" | "o" => Some(PushHow::Overwrite),
            "same" | "s" => Some(PushHow::Same),
            _ => None,
        }
    }
}

/// What sending a copy to a repository is told as.
pub fn push_said(p: &Pushed) -> String {
    let name = format!("{}/{}", p.repo.group, p.repo.repo);
    let mut said = format!("Sent the copy to {name}: {} sent, {} already there", p.pushed, p.up_to_date);
    if p.created {
        said = format!("Made {name} and sent the copy: {} branches and tags", p.pushed);
    }
    if p.deleted > 0 {
        said.push_str(&format!(", {} removed there", p.deleted));
    }
    if p.rejected_count > 0 {
        said.push_str(&format!(". Left alone because they differ there: {}", p.rejected.join(", ")));
        if p.rejected_count > p.rejected.len() {
            said.push_str(&format!(" and {} more", p.rejected_count - p.rejected.len()));
        }
    }
    said
}

/// What a copy made or updated is told as.
pub fn mirror_said(m: &Mirrored) -> String {
    let what = format!("{} branches and {} tags, {}", m.branches, m.tags, crate::ui::human_bytes(m.bytes));
    let name = format!("{}/{}", m.repo.group, m.repo.repo);
    if m.first {
        format!("Copied {name}: {what}")
    } else if m.removed > 0 {
        format!("Updated the copy of {name}: {what}; {} gone from the node removed", m.removed)
    } else {
        format!("Updated the copy of {name}: {what}")
    }
}

/// A work document's title and text as they go: trimmed, and not too long.
fn check_doc(title: &str, text: &str) -> Result<(String, String), String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("A document needs a title".into());
    }
    if title.chars().count() > MAX_TITLE {
        return Err(format!("A title is at most {MAX_TITLE} characters"));
    }
    if text.chars().count() > MAX_TEXT {
        return Err(format!("A document is at most {MAX_TEXT} characters"));
    }
    Ok((title.to_string(), text.to_string()))
}

/// A new work document (proposed, for those who may only propose).
pub fn new_doc_request(propose: bool, title: &str, text: &str) -> Result<GitRequest, String> {
    let (title, text) = check_doc(title, text)?;
    Ok(GitRequest::WorkNew { kind: if propose { WorkKind::Propose } else { WorkKind::Create }, title, text })
}

pub fn comment_request(id: u64, scope: WorkScope, text: &str) -> Result<GitRequest, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("A comment needs some text".into());
    }
    if text.chars().count() > MAX_TEXT {
        return Err(format!("A comment is at most {MAX_TEXT} characters"));
    }
    Ok(GitRequest::WorkComment { id, scope, text: text.to_string() })
}

pub fn edit_request(id: u64, scope: WorkScope, title: &str, text: &str) -> Result<GitRequest, String> {
    let (title, text) = check_doc(title, text)?;
    Ok(GitRequest::WorkEdit { id, scope, title, text })
}

/// A release of a tag already pushed, with these files.
pub fn release_request(tag: &str, notes: &str, files: Vec<(String, Vec<u8>)>) -> Result<GitRequest, String> {
    let tag = tag.trim();
    if tag.is_empty() || tag.chars().count() > 128 || tag.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("A release is of a tag: its name, with no spaces".into());
    }
    if files.is_empty() {
        return Err("A release needs at least one file".into());
    }
    if files.iter().map(|(_, data)| data.len()).sum::<usize>() > MAX_RELEASE {
        return Err(format!("A release's files are at most {} MB together", MAX_RELEASE / (1024 * 1024)));
    }
    for (name, _) in &files {
        rngit::client::check_artifact_name(name).map_err(|_| format!("{name:?} can't be a release file's name"))?;
        if name == "manifest.rsm" || name.ends_with(".rsg") {
            return Err(format!("{name} is a name rngit keeps for signatures"));
        }
    }
    if notes.chars().count() > MAX_TEXT {
        return Err(format!("Release notes are at most {MAX_TEXT} characters"));
    }
    Ok(GitRequest::ReleaseNew { tag: tag.to_string(), notes: notes.to_string(), files })
}

/// What the TUI's prompts in the Repos tab are for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoPrompt {
    Add,
    /// The node with an rngit node's pages.
    PageNode(Hash),
    ConfirmRemove(String),
    NewTitle(bool),
    NewText {
        propose: bool,
        title: String,
    },
    Comment {
        id: u64,
        scope: WorkScope,
    },
    EditTitle {
        id: u64,
        scope: WorkScope,
    },
    EditText {
        id: u64,
        scope: WorkScope,
        title: String,
    },
    ConfirmDeleteDoc {
        id: u64,
        scope: WorkScope,
    },
    ReleaseTag,
    ReleaseNotes(String),
    ReleaseFiles {
        tag: String,
        notes: String,
    },
    ConfirmDeleteRelease(String),
    ConfirmRemoveMirror(String),
    /// Copy a repository a link opened.
    ConfirmCopyLinked,
    /// Where to send the copy.
    PushTo,
    /// How: what to do with branches that differ there.
    PushHow(RepoUrl),
}

/// A work list by name.
pub fn parse_scope(text: &str) -> Option<WorkScope> {
    match text {
        "active" => Some(WorkScope::Active),
        "completed" => Some(WorkScope::Completed),
        "proposed" => Some(WorkScope::Proposed),
        "all" => Some(WorkScope::All),
        _ => None,
    }
}

impl App {
    /// The identity file requests go as, when it's not rettui's own.
    pub fn rngit_identity_file(&self) -> Option<PathBuf> {
        self.settings.rngit_identity.then(rngit_identity_path).flatten()
    }

    /// Ask (or tell) a repository something; the answer comes as a
    /// [`crate::net::NetEvent::Git`] with the id given back.
    pub fn git(&mut self, repo: &RepoUrl, request: GitRequest) -> u64 {
        let id = self.request_id();
        let timeout = std::time::Duration::from_secs(self.settings.rngit_timeout_secs.max(1));
        self.send(NetCommand::Git { id, repo: repo.clone(), request, identity_file: self.rngit_identity_file(), timeout });
        id
    }

    /// Keep a repository (by its URL, with or without `rns://`). Its URL as
    /// kept.
    pub fn add_repo(&mut self, text: &str) -> Result<String, String> {
        let url = parse_repo(text)?.to_string();
        if !self.store.repos.iter().any(|r| r.url == url) {
            if self.store.repos.len() >= MAX_REPOS {
                return Err(format!("At most {MAX_REPOS} repositories are kept: remove one first"));
            }
            self.store.repos.push(SavedRepo { url: url.clone(), mirror: None });
            self.store_dirty = true;
        }
        Ok(url)
    }

    /// The folder for a repository's copy, if another kept repository's copy
    /// isn't in the same one (on a system that doesn't tell capitals apart
    /// `Demo` and `demo` are).
    pub(super) fn mirror_dir_for(&self, url: &RepoUrl) -> Result<PathBuf, String> {
        let dir = mirror_dir(&self.paths.repos, url)?;
        let same = |other: &Path| other.to_string_lossy().to_lowercase() == dir.to_string_lossy().to_lowercase();
        let wanted = url.to_string();
        let clash = self
            .store
            .repos
            .iter()
            .filter(|r| r.url != wanted && r.mirror.is_some())
            .find(|r| parse_repo(&r.url).ok().and_then(|u| mirror_dir(&self.paths.repos, &u).ok()).is_some_and(|other| same(&other)));
        match clash {
            Some(other) => Err(format!("{} has its copy in the same folder, as capitals don't count in a folder's name", other.url)),
            None => Ok(dir),
        }
    }

    /// The branches and tags of a repository's copy, if it has one that can be read.
    pub fn copy_refs(&self, url: &RepoUrl) -> Option<rngit::RemoteRefs> {
        let kept = url.to_string();
        self.store.repos.iter().find(|r| r.url == kept)?.mirror.as_ref()?;
        rngit::local::refs(&self.mirror_dir_for(url).ok()?).ok()
    }

    /// The request that makes (or updates) a repository's copy. The folder
    /// is rettui's choice; no UI picks one.
    pub fn mirror_request(&self, url: &RepoUrl) -> Result<GitRequest, String> {
        Ok(GitRequest::Mirror { dir: self.mirror_dir_for(url)? })
    }

    /// Make the open repository's copy, or bring it up to date.
    pub fn mirror_repo(&mut self) -> Result<u64, String> {
        let repo = self.repos.open.as_ref().map(|o| o.url.clone()).ok_or("No repository is open")?;
        if self.repos.asked.values().any(|a| matches!(a, Asked::Mirror(r) if *r == repo)) {
            return Err("It is being copied already".into());
        }
        let request = self.mirror_request(&repo)?;
        let id = self.git(&repo, request);
        self.repos.asked.insert(id, Asked::Mirror(repo.clone()));
        self.notify(format!("Copying {}/{}…", repo.group, repo.repo));
        Ok(id)
    }

    /// The request that sends a repository's copy to another repository.
    /// The copy is the one `m` made; the folder is rettui's choice.
    pub fn push_copy_request(&self, from: &RepoUrl, to: &RepoUrl, how: PushHow) -> Result<GitRequest, String> {
        if from == to {
            return Err("That is the repository the copy is of: send it to another one".into());
        }
        let kept = from.to_string();
        if !self.store.repos.iter().any(|r| r.url == kept && r.mirror.is_some()) {
            return Err("There is no copy of it on this computer: m makes one".into());
        }
        let (force, prune) = how.flags();
        Ok(GitRequest::PushCopy { dir: self.mirror_dir_for(from)?, force, prune })
    }

    /// Send the open repository's copy to `to`.
    pub fn push_copy(&mut self, to: &RepoUrl, how: PushHow) -> Result<u64, String> {
        let from = self.repos.open.as_ref().map(|o| o.url.clone()).ok_or("No repository is open")?;
        if self.repos.asked.values().any(|a| matches!(a, Asked::Push(r) if r == to)) {
            return Err("A copy is being sent there already".into());
        }
        let request = self.push_copy_request(&from, to, how)?;
        let id = self.git(to, request);
        self.repos.asked.insert(id, Asked::Push(to.clone()));
        self.notify(format!("Sending the copy to {}/{}…", to.group, to.repo));
        Ok(id)
    }

    /// Note a copy made: kept with the repository. One for a repository no
    /// longer kept is deleted again.
    pub fn record_mirror(&mut self, m: &Mirrored) {
        let url = m.repo.to_string();
        match self.store.repos.iter_mut().find(|r| r.url == url) {
            Some(kept) => {
                kept.mirror = Some(SavedMirror { updated: m.at, branches: m.branches, tags: m.tags, bytes: m.bytes, head: m.head.clone() });
                self.store_dirty = true;
            }
            None => {
                if let Ok(dir) = mirror_dir(&self.paths.repos, &m.repo) {
                    let _ = remove_mirror_dir(&self.paths.repos, &dir);
                }
            }
        }
    }

    /// Delete a repository's copy from this computer. Nothing on the node
    /// changes.
    pub fn remove_mirror(&mut self, url: &str) -> Result<(), String> {
        let repo = parse_repo(url)?;
        if !self.store.repos.iter().any(|r| r.url == url && r.mirror.is_some()) {
            return Err("There is no copy of it on this computer".into());
        }
        let dir = mirror_dir(&self.paths.repos, &repo)?;
        remove_mirror_dir(&self.paths.repos, &dir).map_err(|e| format!("Could not delete {}: {e}", dir.display()))?;
        if let Some(kept) = self.store.repos.iter_mut().find(|r| r.url == url) {
            kept.mirror = None;
            self.store_dirty = true;
        }
        Ok(())
    }

    pub fn remove_repo(&mut self, url: &str) {
        if self.store.repos.iter().any(|r| r.url == url && r.mirror.is_some())
            && let Err(e) = self.remove_mirror(url)
        {
            self.warn(e);
        }
        let before = self.store.repos.len();
        self.store.repos.retain(|r| r.url != url);
        if self.store.repos.len() != before {
            self.store_dirty = true;
            if self.repos.open.as_ref().is_some_and(|o| o.url.to_string() == url) {
                self.repos.open = None;
                self.repos.focus = ReposFocus::List;
            }
            let len = self.side_len();
            if self.repos.side.selected().is_some_and(|i| i >= len) {
                self.repos.side.select(len.checked_sub(1));
            }
        }
    }

    /// Open a repository in the Repos tab (kept, if it isn't), and read
    /// what it has.
    pub fn open_repo(&mut self, text: &str) -> Result<(), String> {
        let url = self.add_repo(text)?;
        let repo = parse_repo(&url)?;
        self.tab = Tab::Repos;
        self.repos.focus = ReposFocus::Repo;
        self.repos.items.select(Some(0));
        if let Some(index) = self.store.repos.iter().position(|r| r.url == url) {
            self.repos.side.select(Some(index));
        }
        if self.repos.open.as_ref().is_none_or(|o| o.url != repo) {
            self.repos.open = Some(OpenRepo::new(repo));
            self.repos.pane = RepoPane::Branches;
        }
        self.reload_repo();
        Ok(())
    }

    /// Read the open repository's lists again (and what's open of it).
    pub fn reload_repo(&mut self) {
        let Some(open) = &self.repos.open else { return };
        let repo = open.url.clone();
        let detail = open.detail.clone();
        let asks = [
            (GitRequest::Refs, Asked::Refs(repo.clone())),
            (GitRequest::Releases, Asked::Releases(repo.clone())),
            (GitRequest::Work(WorkScope::All), Asked::Work(repo.clone())),
            (GitRequest::Perms, Asked::Perms(repo.clone())),
        ];
        for (request, asked) in asks {
            let id = self.git(&repo, request);
            self.repos.asked.insert(id, asked);
        }
        if let Some(open) = &mut self.repos.open {
            for load in [&mut open.refs as &mut dyn LoadingState, &mut open.releases, &mut open.work, &mut open.perms] {
                load.loading();
            }
        }
        match detail {
            Some(Detail::Release { tag, .. }) => self.open_release(&tag),
            Some(Detail::Doc { id, scope, .. }) => self.open_doc(id, scope),
            // The copy is on this computer: asking the node again changes nothing in it.
            Some(Detail::Browse(_)) | None => {}
        }
    }

    /// Open one release of the open repository.
    pub fn open_release(&mut self, tag: &str) {
        let Some(open) = &mut self.repos.open else { return };
        let repo = open.url.clone();
        open.detail = Some(Detail::Release { tag: tag.to_string(), load: Load::Loading });
        self.repos.items.select(Some(0));
        let id = self.git(&repo, GitRequest::Release(tag.to_string()));
        self.repos.asked.insert(id, Asked::Release(repo, tag.to_string()));
    }

    /// Open one work document of the open repository.
    pub fn open_doc(&mut self, doc: u64, scope: WorkScope) {
        let Some(open) = &mut self.repos.open else { return };
        let repo = open.url.clone();
        open.detail = Some(Detail::Doc { id: doc, scope, load: Load::Loading });
        self.repos.scroll = 0;
        let id = self.git(&repo, GitRequest::WorkView { id: doc, scope });
        self.repos.asked.insert(id, Asked::Doc(repo, doc));
    }

    /// Download a file of a release into the downloads folder.
    pub fn download_artifact(&mut self, tag: &str, name: &str) {
        let Some(open) = &self.repos.open else { return };
        let repo = open.url.clone();
        let id = self.git(&repo, GitRequest::Artifact { tag: tag.to_string(), name: name.to_string() });
        self.repos.asked.insert(id, Asked::Artifact);
        self.notify(format!("Downloading {name}…"));
    }

    /// Change something in the open repository (`what` is said meanwhile);
    /// its lists are read again after.
    pub fn repo_write(&mut self, request: GitRequest, what: &str) -> Result<u64, String> {
        let repo = self.repos.open.as_ref().map(|o| o.url.clone()).ok_or("No repository is open")?;
        let id = self.git(&repo, request);
        self.repos.asked.insert(id, Asked::Write(repo, what.to_string()));
        self.notify(format!("{what}…"));
        Ok(id)
    }

    /// A new work document (proposed, for those who may only propose).
    pub fn new_doc(&mut self, propose: bool, title: &str, text: &str) -> Result<u64, String> {
        let request = new_doc_request(propose, title, text)?;
        let what = if propose { format!("Proposing {}", title.trim()) } else { format!("Making {}", title.trim()) };
        self.repo_write(request, &what)
    }

    pub fn comment_doc(&mut self, id: u64, scope: WorkScope, text: &str) -> Result<u64, String> {
        self.repo_write(comment_request(id, scope, text)?, &format!("Commenting on #{id}"))
    }

    pub fn edit_doc(&mut self, id: u64, scope: WorkScope, title: &str, text: &str) -> Result<u64, String> {
        self.repo_write(edit_request(id, scope, title, text)?, &format!("Changing #{id}"))
    }

    /// Mark a document done, or (`done` false) active again.
    pub fn finish_doc(&mut self, id: u64, done: bool) -> Result<u64, String> {
        if done {
            self.repo_write(GitRequest::WorkComplete(id), &format!("Marking #{id} done"))
        } else {
            self.repo_write(GitRequest::WorkActivate(id), &format!("Making #{id} active"))
        }
    }

    pub fn delete_doc(&mut self, id: u64, scope: WorkScope) -> Result<u64, String> {
        let sent = self.repo_write(GitRequest::WorkDelete { id, scope }, &format!("Deleting #{id}"))?;
        self.close_detail();
        Ok(sent)
    }

    /// Back from a document or release that's going away, so the reload doesn't ask for it.
    fn close_detail(&mut self) {
        if let Some(open) = &mut self.repos.open {
            open.detail = None;
        }
    }

    /// Publish a release of a tag already pushed, with these files.
    pub fn publish_release(&mut self, tag: &str, notes: &str, files: Vec<(String, Vec<u8>)>) -> Result<u64, String> {
        let request = release_request(tag, notes, files)?;
        self.repo_write(request, &format!("Publishing {}", tag.trim()))
    }

    /// [`App::publish_release`] with files read from this computer.
    pub(super) fn publish_release_paths(&mut self, tag: &str, notes: &str, paths: &str) -> Result<u64, String> {
        let mut files = Vec::new();
        let mut total = 0u64;
        for text in split_paths(paths) {
            let path = super::files::expand_home(&text);
            let size = std::fs::metadata(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?.len();
            total += size;
            if !path.is_file() {
                return Err(format!("{} isn't a file", path.display()));
            }
            if total > MAX_RELEASE as u64 {
                return Err(format!("A release's files are at most {} MB together", MAX_RELEASE / (1024 * 1024)));
            }
            let data = std::fs::read(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
            files.push((path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), data));
        }
        self.publish_release(tag, notes, files)
    }

    pub fn delete_release(&mut self, tag: &str) -> Result<u64, String> {
        let sent = self.repo_write(GitRequest::ReleaseDelete(tag.to_string()), &format!("Deleting the release {tag}"))?;
        self.close_detail();
        Ok(sent)
    }

    pub(super) fn on_git(&mut self, id: u64, result: Result<GitReply, String>) {
        if let Ok(GitReply::Mirrored(copy)) = &result {
            self.record_mirror(copy);
        }
        let Some(asked) = self.repos.asked.remove(&id) else { return };
        // The node can't be asked: the copy on this computer still has the branches and tags.
        let from_copy = match (&asked, &result) {
            (Asked::Refs(repo), Err(_)) => self.copy_refs(repo),
            _ => None,
        };
        let open = self.repos.open.as_mut();
        let is_open = |repo: &RepoUrl, open: &Option<&mut OpenRepo>| open.as_ref().is_some_and(|o| o.url == *repo);
        match (asked, result) {
            (Asked::Refs(repo), result) if is_open(&repo, &open) => {
                let open = open.expect("checked");
                open.from_copy = from_copy.is_some();
                open.refs = match from_copy {
                    Some(refs) => Load::Ready(refs),
                    None => into_load(result, |r| if let GitReply::Refs(refs) = r { Some(refs) } else { None }),
                };
            }
            (Asked::Releases(repo), result) if is_open(&repo, &open) => {
                let open = open.expect("checked");
                open.releases = into_load(result, |r| if let GitReply::Releases(list) = r { Some(list) } else { None });
            }
            (Asked::Work(repo), result) if is_open(&repo, &open) => {
                let open = open.expect("checked");
                open.work = into_load(result, |r| if let GitReply::Work(list) = r { Some(list) } else { None });
            }
            (Asked::Perms(repo), result) if is_open(&repo, &open) => {
                let open = open.expect("checked");
                open.perms = into_load(result, |r| if let GitReply::Perms(text) = r { Some(text) } else { None });
            }
            (Asked::Release(repo, tag), result) if is_open(&repo, &open) => {
                let open = open.expect("checked");
                if let Some(Detail::Release { tag: shown, load }) = &mut open.detail
                    && *shown == tag
                {
                    *load = into_load(result, |r| if let GitReply::Release(detail) = r { Some(detail) } else { None });
                }
            }
            (Asked::Doc(repo, doc), result) if is_open(&repo, &open) => {
                let open = open.expect("checked");
                if let Some(Detail::Doc { id, load, .. }) = &mut open.detail
                    && *id == doc
                {
                    *load = into_load(result, |r| if let GitReply::Doc(doc) = r { Some(doc) } else { None });
                }
            }
            (Asked::Artifact, Ok(GitReply::Artifact { name, data, signer })) => self.save_artifact(&name, &data, &signer),
            (Asked::Artifact, Err(e)) => self.fail(format!("Couldn't download: {e}")),
            (Asked::Write(repo, _), Ok(GitReply::Done(said))) => {
                self.notify(said);
                if self.repos.open.as_ref().is_some_and(|o| o.url == repo) {
                    self.reload_repo();
                }
            }
            (Asked::Write(_, what), Err(e)) => self.fail(format!("{what} failed: {e}")),
            (Asked::Mirror(_), Ok(GitReply::Mirrored(copy))) => self.notify(mirror_said(&copy)),
            (Asked::Mirror(repo), Err(e)) => self.fail(format!("Copying {}/{} failed: {e}", repo.group, repo.repo)),
            (Asked::Push(_), Ok(GitReply::Pushed(sent))) => self.notify(push_said(&sent)),
            (Asked::Push(repo), Err(e)) => self.fail(format!("Sending the copy to {}/{} failed: {e}", repo.group, repo.repo)),
            // For a repository no longer open, or an answer of another kind.
            _ => {}
        }
    }

    fn save_artifact(&mut self, name: &str, data: &[u8], signer: &str) {
        let saved = std::fs::create_dir_all(&self.paths.downloads).map_err(anyhow::Error::from).and_then(|()| {
            let target = super::files::unique_path(&self.paths.downloads, name);
            crate::config::write_atomic(&target, data).map(|()| target)
        });
        match saved {
            Ok(target) => self.notify(format!("Saved {} (signed by {})", target.display(), self.store.display_name(signer))),
            Err(e) => self.fail(format!("Could not save {name}: {e}")),
        }
    }

    /// An announce heard: rngit nodes are listed, nearest first.
    pub(super) fn heard_git_node(&mut self, heard: &Heard) {
        if heard.kind != Kind::Git {
            return;
        }
        let node = GitNode { hash: heard.hash, name: heard.name.clone(), hops: heard.hops, at: now() };
        match self.repos.nodes.iter_mut().find(|n| n.hash == node.hash) {
            Some(known) => *known = node,
            None => self.repos.nodes.push(node),
        }
        self.repos.nodes.sort_by_key(|n| n.hops);
        self.repos.nodes.truncate(MAX_NODES);
    }

    /// Rows of the list on the left: kept repositories, then nodes.
    pub(crate) fn side_len(&self) -> usize {
        self.store.repos.len() + self.repos.nodes.len()
    }

    fn move_repos_side(&mut self, delta: isize) {
        let len = self.side_len();
        if len == 0 {
            return;
        }
        let at = self.repos.side.selected().unwrap_or(0).min(len - 1);
        self.repos.side.select(Some(at.saturating_add_signed(delta).min(len - 1)));
    }

    /// How many rows the open pane lists.
    pub(crate) fn repo_items(&self) -> usize {
        let Some(open) = &self.repos.open else { return 0 };
        match (&open.detail, self.repos.pane) {
            (Some(Detail::Release { load, .. }), _) => load.ready().map_or(0, |d| d.artifacts.len()),
            (Some(Detail::Doc { .. }), _) => 0,
            (Some(Detail::Browse(browse)), _) => match &browse.view {
                BrowseView::Folder(entries) => entries.len(),
                BrowseView::Log(commits) => commits.len(),
                BrowseView::File(_) | BrowseView::Failed(_) => 0,
            },
            (None, RepoPane::Branches) => open.refs.ready().map_or(0, |r| r.refs.len()),
            (None, RepoPane::Releases) => open.releases.ready().map_or(0, |r| r.releases.len()),
            (None, RepoPane::Work) => open.documents().len(),
            (None, RepoPane::Permissions) => 0,
        }
    }

    fn move_repo_items(&mut self, delta: isize) {
        let reading = matches!(
            self.repos.open.as_ref().and_then(|o| o.detail.as_ref()),
            Some(Detail::Doc { .. } | Detail::Browse(Browse { view: BrowseView::File(_) | BrowseView::Failed(_), .. }))
        );
        if reading || (self.repos.pane == RepoPane::Permissions && self.repos.open.as_ref().is_some_and(|o| o.detail.is_none())) {
            self.repos.scroll = self.repos.scroll.saturating_add_signed(delta as i16);
            return;
        }
        let len = self.repo_items();
        if len == 0 {
            return;
        }
        let at = self.repos.items.selected().unwrap_or(0).min(len - 1);
        self.repos.items.select(Some(at.saturating_add_signed(delta).min(len - 1)));
    }

    fn switch_repo_pane(&mut self, forward: bool) {
        let at = RepoPane::ALL.iter().position(|p| *p == self.repos.pane).unwrap_or(0);
        let n = RepoPane::ALL.len();
        self.repos.pane = RepoPane::ALL[if forward { (at + 1) % n } else { (at + n - 1) % n }];
        self.repos.items.select(Some(0));
        self.repos.scroll = 0;
        if let Some(open) = &mut self.repos.open {
            open.detail = None;
        }
    }

    pub(crate) fn repos_key(&mut self, key: KeyEvent) {
        match (self.repos.focus, key.code) {
            (_, KeyCode::Char('n')) => {
                // On a node heard, the prompt starts with its address.
                let selected = self.repos.side.selected().unwrap_or(0);
                let node = selected.checked_sub(self.store.repos.len()).and_then(|i| self.repos.nodes.get(i));
                let start = node.map_or_else(|| "rns://".to_string(), |n| format!("rns://{}/", hex::encode(n.hash)));
                self.open_prompt(PromptKind::Repo(RepoPrompt::Add), "Add a repository (rns://node/group/repo)", &start)
            }
            (ReposFocus::List, KeyCode::Char('b')) => {
                let selected = self.repos.side.selected().unwrap_or(0);
                match selected.checked_sub(self.store.repos.len()).and_then(|i| self.repos.nodes.get(i)).map(|n| n.hash) {
                    Some(hash) => self.browse_git_node(hash, true),
                    None => self.warn("b is for a node: select one below the repositories"),
                }
            }
            (ReposFocus::List, KeyCode::Down | KeyCode::Char('j')) => self.move_repos_side(1),
            (ReposFocus::List, KeyCode::Up | KeyCode::Char('k')) => self.move_repos_side(-1),
            (ReposFocus::List, KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')) => self.open_side(),
            (ReposFocus::List, KeyCode::Tab) if self.repos.open.is_some() => self.repos.focus = ReposFocus::Repo,
            (ReposFocus::List, KeyCode::Char('D')) => {
                if let Some(repo) = self.repos.side.selected().and_then(|i| self.store.repos.get(i)).cloned() {
                    let title = format!("Stop keeping {}? Nothing on the node changes. Type y", repo.url);
                    self.open_prompt(PromptKind::Repo(RepoPrompt::ConfirmRemove(repo.url)), &title, "");
                }
            }
            (ReposFocus::List, KeyCode::Char('y')) => {
                let selected = self.repos.side.selected().unwrap_or(0);
                match self.store.repos.get(selected).cloned() {
                    Some(repo) => self.copy(&repo.url, "the repository's URL"),
                    None => {
                        let node = selected.checked_sub(self.store.repos.len()).and_then(|i| self.repos.nodes.get(i));
                        if let Some(hash) = node.map(|n| hex::encode(n.hash)) {
                            self.copy(&hash, "the node's address");
                        }
                    }
                }
            }
            (ReposFocus::Repo, code) => self.repo_key(code),
            _ => {}
        }
    }

    /// Follow a link to a repository: it opens, and if it hasn't been copied
    /// to this computer, asks whether to copy it.
    pub fn follow_repo_link(&mut self, url: &str) {
        if let Err(e) = self.open_repo(url) {
            return self.warn(e);
        }
        let kept = self.repos.open.as_ref().map(|o| o.url.to_string());
        if kept.is_some_and(|kept| !self.store.repos.iter().any(|r| r.url == kept && r.mirror.is_some())) {
            self.open_prompt(
                PromptKind::Repo(RepoPrompt::ConfirmCopyLinked),
                "Copy it to this computer, to read or clone from? Nothing on the node changes. Type y",
                "",
            );
        }
    }

    /// Enter on the left: a repository opens; a node opens in the Browser,
    /// where its pages are (a node doesn't list its repositories: `n` adds
    /// one by name).
    fn open_side(&mut self) {
        let Some(selected) = self.repos.side.selected() else { return };
        if let Some(repo) = self.store.repos.get(selected).cloned() {
            if let Err(e) = self.open_repo(&repo.url) {
                self.warn(e);
            }
        } else if let Some(node) = self.repos.nodes.get(selected - self.store.repos.len()).cloned() {
            self.browse_git_node(node.hash, false);
        }
    }

    /// The node with an rngit node's pages, if one is kept and still known.
    pub fn page_node_for(&self, git: Hash) -> Option<Hash> {
        let kept = self.store.git_pages.get(&hex::encode(git))?;
        net::parse_hash(kept)
    }

    /// Nomad nodes that look like the pages of an rngit node: the same name
    /// (capitals and spaces don't count), nearest first.
    pub fn page_node_suggestions(&self, git: Hash) -> Vec<(Hash, String)> {
        let plain = |name: &str| name.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        let Some(wanted) = self.repos.nodes.iter().find(|n| n.hash == git).and_then(|n| n.name.as_deref()).map(plain) else {
            return Vec::new();
        };
        if wanted.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<_> = self
            .store
            .peers
            .iter()
            .filter(|(_, p)| p.kind == PeerKind::Nomad)
            .filter_map(|(hash, p)| {
                let name = p.name.as_deref()?;
                let plain = plain(name);
                let near = !plain.is_empty() && (plain.contains(&wanted) || wanted.contains(&plain));
                near.then_some(()).and(net::parse_hash(hash)).map(|hash| (hash, name.to_string(), p.hops))
            })
            .collect();
        found.sort_by_key(|(_, _, hops)| *hops);
        found.truncate(5);
        found.into_iter().map(|(hash, name, _)| (hash, name)).collect()
    }

    /// Open the pages of an rngit node. A node that serves them from another
    /// identity is asked for once, with a guess by name; after that the
    /// choice is kept. `change` asks again.
    pub fn browse_git_node(&mut self, git: Hash, change: bool) {
        if let Some(pages) = self.page_node_for(git).filter(|_| !change) {
            return self.navigate(Location { node: pages, path: "/page/index.mu".into(), fields: Default::default() });
        }
        let suggestions = self.page_node_suggestions(git);
        let start = self.page_node_for(git).or(suggestions.first().map(|(hash, _)| *hash)).map(hex::encode).unwrap_or_default();
        let mut title = "Which NomadNet node has its pages? Its address; empty to try this node".to_string();
        if !suggestions.is_empty() {
            let names: Vec<_> = suggestions.iter().map(|(hash, name)| format!("{name} {}", &hex::encode(hash)[..8])).collect();
            title = format!("{title} (like: {})", names.join(", "));
        }
        self.open_prompt(PromptKind::Repo(RepoPrompt::PageNode(git)), &title, &start);
    }

    /// Keep (or, with `None`, forget) the node with an rngit node's pages.
    pub fn set_page_node(&mut self, git: Hash, pages: Option<Hash>) {
        let key = hex::encode(git);
        let changed = match pages {
            Some(pages) => self.store.git_pages.insert(key, hex::encode(pages)) != Some(hex::encode(pages)),
            None => self.store.git_pages.remove(&key).is_some(),
        };
        if changed {
            self.store_dirty = true;
        }
    }

    /// The repositories a page's Micron source links to, in order, each once.
    pub fn repos_linked(source: &str) -> Vec<RepoUrl> {
        let mut seen = Vec::new();
        for link in crate::links::find(source).into_iter().filter(|l| l.kind == crate::links::LinkKind::Git) {
            if let Ok(repo) = parse_repo(&link.url)
                && !seen.contains(&repo)
            {
                seen.push(repo);
            }
        }
        seen
    }

    /// Keep every repository a page links to. How many were new.
    pub fn add_repos_linked(&mut self, source: &str) -> Result<usize, String> {
        let repos = Self::repos_linked(source);
        if repos.is_empty() {
            return Err("This page has no links to repositories (rns://<node>/<group>/<repo>)".into());
        }
        let mut added = 0;
        for repo in repos {
            let url = repo.to_string();
            if self.store.repos.iter().any(|r| r.url == url) {
                continue;
            }
            self.add_repo(&url)?;
            added += 1;
        }
        Ok(added)
    }

    /// Keep every repository the shown page links to.
    pub fn add_page_repos(&mut self) -> Result<usize, String> {
        let source = self.browser.tab.source.clone().unwrap_or_default();
        self.add_repos_linked(&source)
    }

    fn repo_key(&mut self, code: KeyCode) {
        let Some(open) = self.repos.open.clone() else {
            self.repos.focus = ReposFocus::List;
            return;
        };
        match code {
            KeyCode::Esc | KeyCode::Backspace if matches!(open.detail, Some(Detail::Browse(_))) => self.browse_up(),
            KeyCode::Esc | KeyCode::Backspace if open.detail.is_some() => {
                if let Some(open) = &mut self.repos.open {
                    open.detail = None;
                }
                self.repos.scroll = 0;
            }
            KeyCode::Esc | KeyCode::BackTab => self.repos.focus = ReposFocus::List,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => self.switch_repo_pane(true),
            KeyCode::Left | KeyCode::Char('h') => self.switch_repo_pane(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_repo_items(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_repo_items(-1),
            KeyCode::PageDown => self.move_repo_items(10),
            KeyCode::PageUp => self.move_repo_items(-10),
            KeyCode::Char('r') => self.reload_repo(),
            KeyCode::Char('m') => {
                if let Err(e) = self.mirror_repo() {
                    self.warn(e);
                }
            }
            KeyCode::Char('M') => {
                let url = open.url.to_string();
                if self.store.repos.iter().any(|r| r.url == url && r.mirror.is_some()) {
                    let title = format!("Delete the copy of {url} on this computer? Nothing on the node changes. Type y");
                    self.open_prompt(PromptKind::Repo(RepoPrompt::ConfirmRemoveMirror(url)), &title, "");
                } else {
                    self.warn("There is no copy of it on this computer: m makes one");
                }
            }
            KeyCode::Char('u') => {
                let url = open.url.to_string();
                if self.store.repos.iter().any(|r| r.url == url && r.mirror.is_some()) {
                    self.open_prompt(
                        PromptKind::Repo(RepoPrompt::PushTo),
                        "Send this copy to which repository? (rns://<node>/<group>/<repo>; made if it isn't there)",
                        "",
                    );
                } else {
                    self.warn("There is no copy of it on this computer: m makes one");
                }
            }
            KeyCode::Char('y') => self.copy(&open.url.to_string(), "the repository's URL"),
            _ => self.repo_pane_key(&open, code),
        }
    }

    /// Keys of what the open pane shows.
    fn repo_pane_key(&mut self, open: &OpenRepo, code: KeyCode) {
        let selected = self.repos.items.selected().unwrap_or(0);
        let result = match (&open.detail, self.repos.pane, code) {
            (Some(Detail::Release { tag, load }), _, KeyCode::Enter | KeyCode::Char('g')) => {
                if let Some(file) = load.ready().and_then(|d| d.artifacts.get(selected)) {
                    self.download_artifact(tag, &file.name);
                }
                Ok(())
            }
            (Some(Detail::Release { tag, .. }), _, KeyCode::Char('X')) => {
                let title = format!("Delete the release {tag} from the node (its tag stays)? Type y");
                self.open_prompt(PromptKind::Repo(RepoPrompt::ConfirmDeleteRelease(tag.clone())), &title, "");
                Ok(())
            }
            (Some(Detail::Browse(_)), _, KeyCode::Enter) => self.browse_enter(selected),
            (Some(Detail::Browse(_)), _, KeyCode::Char('L')) => {
                self.browse_log();
                Ok(())
            }
            (Some(Detail::Browse(_)), _, _) => Ok(()),
            (None, RepoPane::Branches, KeyCode::Enter) => match open.refs.ready().and_then(|r| r.refs.get(selected)) {
                Some((_, name)) => self.browse_ref(&name.clone()),
                None => Ok(()),
            },
            (Some(Detail::Doc { id, scope, load }), _, code) => {
                let (id, scope) = (*id, *scope);
                match code {
                    KeyCode::Char('c') => {
                        self.open_prompt(PromptKind::Repo(RepoPrompt::Comment { id, scope }), &format!("Comment on #{id}"), "");
                        Ok(())
                    }
                    KeyCode::Char('e') => {
                        let title = load.ready().map(|d| d.title.clone()).unwrap_or_default();
                        self.open_prompt(PromptKind::Repo(RepoPrompt::EditTitle { id, scope }), &format!("Title of #{id}"), &title);
                        Ok(())
                    }
                    KeyCode::Char('x') => match scope {
                        WorkScope::Completed => self.finish_doc(id, false).map(drop),
                        _ => self.finish_doc(id, true).map(drop),
                    },
                    KeyCode::Char('X') => {
                        let title = format!("Delete #{id} from the node? Type y");
                        self.open_prompt(PromptKind::Repo(RepoPrompt::ConfirmDeleteDoc { id, scope }), &title, "");
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
            (None, RepoPane::Releases, KeyCode::Enter) => {
                if let Some(release) = open.releases.ready().and_then(|r| r.releases.get(selected)) {
                    self.open_release(&release.tag.clone());
                }
                Ok(())
            }
            (None, RepoPane::Releases, KeyCode::Char('P')) => {
                self.open_prompt(PromptKind::Repo(RepoPrompt::ReleaseTag), "Publish a release of which tag? (pushed already)", "");
                Ok(())
            }
            (None, RepoPane::Work, KeyCode::Enter) => {
                if let Some(doc) = open.documents().get(selected) {
                    self.open_doc(doc.id, open.scope);
                }
                Ok(())
            }
            (None, RepoPane::Work, KeyCode::Char('s')) => {
                if let Some(open) = &mut self.repos.open {
                    open.scope = match open.scope {
                        WorkScope::Active => WorkScope::Proposed,
                        WorkScope::Proposed => WorkScope::Completed,
                        WorkScope::Completed | WorkScope::All => WorkScope::Active,
                    };
                }
                self.repos.items.select(Some(0));
                Ok(())
            }
            (None, RepoPane::Work, KeyCode::Char('w')) => {
                self.open_prompt(PromptKind::Repo(RepoPrompt::NewTitle(false)), "New work document: its title", "");
                Ok(())
            }
            (None, RepoPane::Work, KeyCode::Char('p')) => {
                self.open_prompt(PromptKind::Repo(RepoPrompt::NewTitle(true)), "Propose a work document: its title", "");
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.warn(e);
        }
    }

    pub(super) fn submit_repo_prompt(&mut self, prompt: RepoPrompt, text: &str) {
        let yes = text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes");
        let result = match prompt {
            RepoPrompt::Add => self.open_repo(text),
            RepoPrompt::PageNode(git) => {
                let text = text.trim().trim_matches(['<', '>']);
                if text.is_empty() {
                    self.set_page_node(git, None);
                    self.navigate(Location { node: git, path: "/page/index.mu".into(), fields: Default::default() });
                    Ok(())
                } else if let Some(pages) = net::parse_hash(text) {
                    self.set_page_node(git, Some(pages));
                    self.navigate(Location { node: pages, path: "/page/index.mu".into(), fields: Default::default() });
                    Ok(())
                } else {
                    Err("An address is 32 hex characters".into())
                }
            }
            RepoPrompt::ConfirmRemove(url) if yes => {
                self.remove_repo(&url);
                Ok(())
            }
            RepoPrompt::NewTitle(propose) => match check_doc(text, "") {
                Ok((title, _)) => {
                    let kind = RepoPrompt::NewText { propose, title };
                    self.open_prompt(PromptKind::Repo(kind), "Its text (Markdown)", "");
                    Ok(())
                }
                Err(e) => Err(e),
            },
            RepoPrompt::NewText { propose, title } => self.new_doc(propose, &title, text).map(drop),
            RepoPrompt::Comment { id, scope } => self.comment_doc(id, scope, text).map(drop),
            RepoPrompt::EditTitle { id, scope } => match check_doc(text, "") {
                Ok((title, _)) => {
                    let body = self.open_doc_text(id).unwrap_or_default();
                    self.open_prompt(
                        PromptKind::Repo(RepoPrompt::EditText { id, scope, title }),
                        &format!("Text of #{id} (Markdown)"),
                        &body,
                    );
                    Ok(())
                }
                Err(e) => Err(e),
            },
            RepoPrompt::EditText { id, scope, title } => self.edit_doc(id, scope, &title, text).map(drop),
            RepoPrompt::ConfirmDeleteDoc { id, scope } if yes => {
                if let Some(open) = &mut self.repos.open {
                    open.detail = None;
                }
                self.delete_doc(id, scope).map(drop)
            }
            RepoPrompt::ReleaseTag => {
                let tag = text.trim().to_string();
                if tag.is_empty() {
                    return;
                }
                self.open_prompt(PromptKind::Repo(RepoPrompt::ReleaseNotes(tag.clone())), &format!("Notes for {tag} (Markdown)"), "");
                Ok(())
            }
            RepoPrompt::ReleaseNotes(tag) => {
                let title = format!("Files of {tag}: their paths, separated by spaces (quote one with spaces)");
                self.open_prompt(PromptKind::Repo(RepoPrompt::ReleaseFiles { tag, notes: text.to_string() }), &title, "");
                Ok(())
            }
            RepoPrompt::ReleaseFiles { tag, notes } => self.publish_release_paths(&tag, &notes, text).map(drop),
            RepoPrompt::ConfirmDeleteRelease(tag) if yes => {
                if let Some(open) = &mut self.repos.open {
                    open.detail = None;
                }
                self.delete_release(&tag).map(drop)
            }
            RepoPrompt::PushTo => match parse_repo(text) {
                Ok(to) => {
                    let title = "If a branch there differs: add (leave it), overwrite it, or same (overwrite, and remove what the copy lacks). Enter adds";
                    self.open_prompt(PromptKind::Repo(RepoPrompt::PushHow(to)), title, "");
                    Ok(())
                }
                Err(e) => Err(e),
            },
            RepoPrompt::PushHow(to) => match PushHow::parse(text) {
                Some(how) => self.push_copy(&to, how).map(drop),
                None => Err("Type add, overwrite or same".into()),
            },
            RepoPrompt::ConfirmCopyLinked if yes => self.mirror_repo().map(drop),
            RepoPrompt::ConfirmRemoveMirror(url) if yes => self.remove_mirror(&url).map(|()| self.notify("Deleted the copy")),
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.warn(e);
        }
    }

    /// The text of the document open, if it's `id`.
    fn open_doc_text(&self, id: u64) -> Option<String> {
        match &self.repos.open.as_ref()?.detail {
            Some(Detail::Doc { id: shown, load: Load::Ready(doc), .. }) if *shown == id => Some(doc.text.clone()),
            _ => None,
        }
    }

    pub(crate) fn click_repos(&mut self, at: Position) {
        if let Some((_, pane)) = self.regions.repo_panes.iter().find(|(rect, _)| rect.contains(at)) {
            self.repos.focus = ReposFocus::Repo;
            if *pane != self.repos.pane {
                self.repos.pane = *pane;
                self.repos.items.select(Some(0));
                if let Some(open) = &mut self.repos.open {
                    open.detail = None;
                }
            }
            return;
        }
        let side = self.regions.repos_side;
        if side.contains(at) {
            self.repos.focus = ReposFocus::List;
            let index = self.repos.side.offset() + (at.y - side.y) as usize;
            if index < self.side_len() {
                self.repos.side.select(Some(index));
            }
            return;
        }
        let list = self.regions.repo_list;
        if list.contains(at) {
            self.repos.focus = ReposFocus::Repo;
            let index = self.repos.items.offset() + (at.y - list.y) as usize;
            if index < self.repo_items() {
                self.repos.items.select(Some(index));
            }
        }
    }

    pub(crate) fn scroll_repos(&mut self, delta: isize) {
        match self.repos.focus {
            ReposFocus::List => self.move_repos_side(delta),
            ReposFocus::Repo => self.move_repo_items(delta),
        }
    }
}

/// A list's state can be set loading, whatever it holds.
trait LoadingState {
    fn loading(&mut self);
}

impl<T> LoadingState for Load<T> {
    /// What's there stays shown while it's read again.
    fn loading(&mut self) {
        if !matches!(self, Load::Ready(_)) {
            *self = Load::Loading;
        }
    }
}

fn into_load<T>(result: Result<GitReply, String>, take: impl FnOnce(GitReply) -> Option<T>) -> Load<T> {
    match result {
        Ok(reply) => take(reply).map_or_else(|| Load::Failed("The node answered something else".into()), Load::Ready),
        Err(e) => Load::Failed(e),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crossterm::event::KeyCode;
    use rns_identity::identity::Identity;

    use super::*;
    use crate::net::NetEvent;
    use crate::net::git::tests::{Canned, answer, demo_node};
    use crate::ui::tui_tests::Tui;
    use rngit::Response;

    const NODE: &str = "0123456789abcdef0123456789abcdef";

    /// Answer what the app asked of rngit nodes as `node` would, and say
    /// what that was.
    fn answer_all(tui: &mut Tui, node: &Arc<Canned>) -> Vec<GitRequest> {
        // Answers may ask again (a write reloads), so until nothing new goes.
        let mut asked = Vec::new();
        loop {
            let mut any = false;
            for command in tui.sent() {
                if let NetCommand::Git { id, repo, request, .. } = command {
                    any = true;
                    asked.push(request.clone());
                    let result = answer(node, &repo, request);
                    tui.app.on_net(NetEvent::Git { id, result });
                }
            }
            if !any {
                break;
            }
        }
        tui.draw();
        asked
    }

    fn opened(tui: &mut Tui) -> String {
        tui.draw()
    }

    #[test]
    fn a_repository_read_from_its_node() {
        let author = Identity::new();
        let node = Arc::new(demo_node(&author));
        let mut tui = Tui::new("repos-read");
        tui.tab(Tab::Repos);
        assert!(opened(&mut tui).contains("No repositories yet"));

        // Added by its URL: kept, and everything asked for.
        tui.key(KeyCode::Char('n')).answer(&format!("{NODE}/public/demo.git"));
        assert_eq!(tui.app.store.repos, [SavedRepo { url: format!("rns://{NODE}/public/demo"), mirror: None }]);
        let asked = answer_all(&mut tui, &node);
        assert_eq!(asked.len(), 4, "{asked:?}");
        let screen = tui.draw();
        assert!(screen.contains("public/demo") && screen.contains("main") && screen.contains("HEAD main"), "{screen}");

        // Releases: one opened, its file downloaded (checked first).
        let screen = tui.key(KeyCode::Right).draw();
        assert!(screen.contains("v1 latest") && screen.contains("First"), "{screen}");
        tui.key(KeyCode::Enter);
        answer_all(&mut tui, &node);
        assert!(tui.draw().contains("app.txt"));
        tui.key(KeyCode::Enter);
        assert!(tui.draw().contains("downloading"));
        answer_all(&mut tui, &node);
        assert_eq!(std::fs::read(tui.app.paths.downloads.join("app.txt")).unwrap(), b"the app");
        assert!(tui.said().contains("Saved") || tui.app.notice.as_ref().is_some_and(|n| n.text.contains("Saved")));

        // Work: each list, and a document with its comment and signature.
        tui.key(KeyCode::Esc).key(KeyCode::Right);
        assert!(tui.draw().contains("Speed"));
        assert!(tui.key(KeyCode::Char('s')).draw().contains("Idea"));
        assert!(tui.key(KeyCode::Char('s')).draw().contains("Old"));
        tui.key(KeyCode::Char('s')).key(KeyCode::Enter);
        answer_all(&mut tui, &node);
        let screen = tui.draw();
        assert!(screen.contains("Make it faster") && screen.contains("On it") && screen.contains("✓ signed by them"), "{screen}");
        tui.key(KeyCode::Down).key(KeyCode::Up);

        // Permissions, as the node has them.
        tui.key(KeyCode::Esc).key(KeyCode::Right);
        let screen = tui.draw();
        assert!(screen.contains("r:all"), "{screen}");
        tui.key(KeyCode::Char('y'));

        // Reloaded; and a node that can't be asked says why.
        tui.key(KeyCode::Char('r'));
        let empty = Arc::new(Canned::default());
        answer_all(&mut tui, &empty);
        assert!(tui.draw().contains("nothing canned"));
    }

    #[test]
    fn writing_documents_and_releases() {
        let author = Identity::new();
        let node = Arc::new(demo_node(&author));
        let mut tui = Tui::new("repos-write");
        tui.app.open_repo(&format!("rns://{NODE}/public/demo")).unwrap();
        answer_all(&mut tui, &node);
        let writes = |tui: &mut Tui| -> Vec<GitRequest> {
            let node = node.clone();
            answer_all(tui, &node)
                .into_iter()
                .filter(|r| {
                    !matches!(
                        r,
                        GitRequest::Refs
                            | GitRequest::Releases
                            | GitRequest::Work(_)
                            | GitRequest::Perms
                            | GitRequest::WorkView { .. }
                            | GitRequest::Release(_)
                    )
                })
                .collect()
        };

        // A new document, and a proposed one.
        tui.key(KeyCode::Right).key(KeyCode::Right);
        tui.key(KeyCode::Char('w')).answer("New").answer("Body");
        assert!(tui.draw().contains("Making New"));
        assert_eq!(writes(&mut tui), [GitRequest::WorkNew { kind: WorkKind::Create, title: "New".into(), text: "Body".into() }]);
        assert!(tui.said().contains("Made #1"), "{}", tui.said());
        tui.key(KeyCode::Char('p')).answer("Idea").answer("Maybe");
        assert!(matches!(&writes(&mut tui)[..], [GitRequest::WorkNew { kind: WorkKind::Propose, .. }]));
        // No title: nothing goes.
        tui.key(KeyCode::Char('w')).answer("");
        assert!(writes(&mut tui).is_empty());

        // On a document: a comment, done, changed, deleted.
        tui.key(KeyCode::Enter);
        answer_all(&mut tui, &node);
        tui.key(KeyCode::Char('c')).answer("Nice");
        assert_eq!(writes(&mut tui), [GitRequest::WorkComment { id: 1, scope: WorkScope::Active, text: "Nice".into() }]);
        tui.key(KeyCode::Char('x'));
        assert_eq!(writes(&mut tui), [GitRequest::WorkComplete(1)]);
        tui.key(KeyCode::Char('e')).answer("").answer("");
        assert_eq!(
            writes(&mut tui),
            [GitRequest::WorkEdit { id: 1, scope: WorkScope::Active, title: "Speed".into(), text: "Make it faster".into() }]
        );
        tui.key(KeyCode::Char('X')).answer("y");
        assert_eq!(writes(&mut tui), [GitRequest::WorkDelete { id: 1, scope: WorkScope::Active }]);

        // A release, with a file from this computer; a file that isn't there.
        let file = tui.dir.join("app.tar.gz");
        std::fs::write(&file, b"tarball").unwrap();
        tui.key(KeyCode::Left);
        tui.key(KeyCode::Char('P')).answer("v2").answer("Notes").answer(&format!("\"{}\"", file.display()));
        assert_eq!(
            writes(&mut tui),
            [GitRequest::ReleaseNew { tag: "v2".into(), notes: "Notes".into(), files: vec![("app.tar.gz".into(), b"tarball".to_vec())] }]
        );
        tui.key(KeyCode::Char('P')).answer("v3").answer("").answer("/no/such/file");
        assert!(writes(&mut tui).is_empty());
        assert!(tui.said().contains("Could not read"), "{}", tui.said());
        // And one deleted.
        tui.key(KeyCode::Enter);
        answer_all(&mut tui, &node);
        tui.key(KeyCode::Char('X')).answer("y");
        assert_eq!(writes(&mut tui), [GitRequest::ReleaseDelete("v1".into())]);

        // A change the node refuses is said.
        tui.app.repo_write(GitRequest::WorkComplete(9), "Marking #9 done").unwrap();
        answer_all(&mut tui, &Arc::new(Canned::default()));
        assert!(tui.said().contains("Marking #9 done failed"), "{}", tui.said());
    }

    #[test]
    fn nodes_links_and_the_list() {
        let mut tui = Tui::with("repos-list", |s| s.rngit_identity = true);
        tui.tab(Tab::Repos);
        // An rngit node heard: listed. Enter opens it in the Browser; n adds a repository on it.
        let hash = [7; 16];
        let heard = Heard { hash, kind: Kind::Git, aspect: Some(crate::net::git::GIT_ASPECT), name: None, hops: 2 };
        tui.app.on_net(NetEvent::Heard(heard));
        assert!(tui.draw().contains("node 07070707"));
        // With no other node to guess, the prompt starts empty; empty tries the node itself.
        tui.key(KeyCode::Enter).answer("");
        assert_eq!(tui.app.tab, Tab::Browser);
        let asked = tui.sent();
        assert!(
            asked.iter().any(|c| matches!(c, NetCommand::Fetch { node, path, .. } if *node == hash && path == "/page/index.mu")),
            "{asked:?}"
        );
        tui.tab(Tab::Repos);
        tui.key(KeyCode::Char('n')).answer("public/site");
        let url = format!("rns://{}/public/site", hex::encode(hash));
        assert_eq!(tui.app.store.repos[0].url, url);
        // Asked as rngit's own identity, as the settings say.
        let sent = tui.sent();
        assert!(sent.iter().all(|c| matches!(c, NetCommand::Git { identity_file: Some(_), .. })));
        assert!(!sent.is_empty());

        // A link to one opens it, and asks whether to copy it; anything but y leaves it be.
        tui.app.open_address(&format!("RNS://{NODE}/public/demo"));
        assert_eq!(tui.app.tab, Tab::Repos);
        assert_eq!(tui.app.store.repos.len(), 2);
        assert!(tui.draw().contains("Copy it to this computer"));
        tui.sent();
        tui.answer("n");
        assert!(!tui.sent().iter().any(|c| matches!(c, NetCommand::Git { request: GitRequest::Mirror { .. }, .. })));
        // Once copied, a link doesn't ask again.
        tui.app.open_address("rns://nothex/a/b");
        assert!(tui.said().contains("rns://<node>/<group>/<repo>"), "{}", tui.said());

        // Clicked: a pane, a row; then removed from the list.
        let pane = tui.app.regions.repo_panes[1].0;
        tui.click_in(pane);
        assert_eq!(tui.app.repos.pane, RepoPane::Releases);
        let side = tui.app.regions.repos_side;
        tui.click(side.x + 1, side.y);
        assert_eq!(tui.app.repos.focus, ReposFocus::List);
        tui.scroll(side.x + 1, side.y, true);
        tui.key(KeyCode::Up).key(KeyCode::Char('y'));
        tui.key(KeyCode::Char('D')).answer("y");
        assert_eq!(tui.app.store.repos.len(), 1);
        tui.key(KeyCode::Char('D')).answer("y");
        assert!(tui.app.store.repos.is_empty() && tui.app.repos.open.is_none());
        assert!(tui.draw().contains("Pick a repository"));
    }

    #[test]
    fn a_node_with_its_pages_on_another_identity() {
        use crate::net::PeerKind;
        use crate::store::Peer;
        let mut tui = Tui::with("repos-pages", |_| {});
        tui.tab(Tab::Repos);
        let (git, pages, other) = ([7; 16], [9; 16], [8; 16]);
        let heard =
            Heard { hash: git, kind: Kind::Git, aspect: Some(crate::net::git::GIT_ASPECT), name: Some("Moon Forge".into()), hops: 2 };
        tui.app.on_net(NetEvent::Heard(heard));
        let nomad = |name: &str, hops| Peer { kind: PeerKind::Nomad, name: Some(name.into()), hops, last_seen: 1 };
        tui.app.store.peers.insert(hex::encode(pages), nomad("moon forge pages", 3));
        tui.app.store.peers.insert(hex::encode(other), nomad("Somewhere else", 1));
        // Only the node that has the name is guessed.
        let guessed = tui.app.page_node_suggestions(git);
        assert_eq!(guessed.len(), 1);
        assert_eq!(guessed[0].0, pages);

        // The prompt starts with the guess; the choice is kept and the pages open.
        tui.draw();
        tui.key(KeyCode::Enter);
        let screen = tui.draw();
        assert!(screen.contains(&hex::encode(pages)), "{screen}");
        tui.key(KeyCode::Enter);
        assert_eq!(tui.app.tab, Tab::Browser);
        assert_eq!(tui.app.page_node_for(git), Some(pages));
        assert!(tui.sent().iter().any(|c| matches!(c, NetCommand::Fetch { node, .. } if *node == pages)));

        // Next time it opens without asking; `b` asks again, and a mistake is refused.
        tui.tab(Tab::Repos);
        tui.key(KeyCode::Enter);
        assert_eq!(tui.app.tab, Tab::Browser);
        assert!(tui.sent().iter().any(|c| matches!(c, NetCommand::Fetch { node, .. } if *node == pages)));
        tui.tab(Tab::Repos);
        tui.key(KeyCode::Char('b')).answer("nonsense");
        assert!(tui.said().contains("32 hex"), "{}", tui.said());
        assert_eq!(tui.app.page_node_for(git), Some(pages));
        let clear = |tui: &mut Tui| {
            for _ in 0..40 {
                tui.key(KeyCode::Backspace);
            }
        };
        tui.key(KeyCode::Char('b'));
        clear(&mut tui);
        tui.answer(&hex::encode(other));
        assert_eq!(tui.app.page_node_for(git), Some(other));
        // Empty forgets it.
        tui.tab(Tab::Repos);
        tui.key(KeyCode::Char('b'));
        clear(&mut tui);
        tui.key(KeyCode::Enter);
        assert_eq!(tui.app.page_node_for(git), None);
        // It's saved with the rest.
        tui.app.set_page_node(git, Some(pages));
        assert_eq!(tui.app.store.snapshot().git_pages.len(), 1);
    }

    #[test]
    fn every_repository_on_a_page_is_kept() {
        let mut tui = Tui::with("repos-page", |_| {});
        tui.tab(Tab::Browser);
        assert!(tui.app.add_page_repos().is_err());
        let a = format!("rns://{NODE}/public/a");
        let b = format!("rns://{NODE}/public/b.git");
        tui.app.browser.tab.source = Some(format!("`[clone]`{a}\nSee {a} and {b}, or `[x`{NODE}:/page/index.mu]\nnot rns://nowhere/x/y"));
        assert_eq!(tui.app.add_page_repos(), Ok(2));
        let kept: Vec<_> = tui.app.store.repos.iter().map(|r| r.url.clone()).collect();
        assert_eq!(kept, [a, format!("rns://{NODE}/public/b")]);
        // Again: nothing new.
        tui.key(KeyCode::Char('a'));
        assert!(tui.said().contains("kept already"), "{}", tui.said());
        assert_eq!(tui.app.store.repos.len(), 2);
    }

    #[test]
    fn what_goes_is_checked_first() {
        assert!(parse_repo("rns://abc/a/b").is_err());
        assert_eq!(parse_repo(&format!("rns://{NODE}/a/b/")).unwrap().to_string(), format!("rns://{NODE}/a/b"));
        assert_eq!(split_paths("a 'b c' \"d e\"  f"), ["a", "b c", "d e", "f"]);
        assert!(new_doc_request(false, " ", "").is_err());
        assert!(new_doc_request(false, &"t".repeat(MAX_TITLE + 1), "").is_err());
        assert!(comment_request(1, WorkScope::Active, "  ").is_err());
        let file = || vec![("a.txt".to_string(), vec![1])];
        assert!(release_request("v 1", "", file()).is_err());
        assert!(release_request("v1", "", Vec::new()).is_err());
        assert!(release_request("v1", "", vec![("../a".into(), vec![1])]).is_err());
        assert!(release_request("v1", "", vec![("manifest.rsm".into(), vec![1])]).is_err());
        assert!(release_request("v1", "", vec![("big".into(), vec![0; MAX_RELEASE + 1])]).is_err());
        assert!(release_request("v1", "notes", file()).is_ok());
        assert_eq!(parse_scope("proposed"), Some(WorkScope::Proposed));
        assert_eq!(parse_scope("nope"), None);
        unsafe { std::env::set_var("RNGIT_CONFIG", "/tmp/rngit-here") };
        assert_eq!(rngit_identity_path(), Some(PathBuf::from("/tmp/rngit-here/client_identity")));
        unsafe { std::env::remove_var("RNGIT_CONFIG") };
    }

    use crate::net::git::tests::Source;

    /// The app asked to copy: answer as the node holding `source` would.
    fn copy_from(tui: &mut Tui, source: &Source) {
        let node = Arc::new(source.node());
        answer_all(tui, &node);
    }

    /// A repository kept, opened, and everything it asked for answered by nobody.
    fn open_demo(tui: &mut Tui) {
        tui.tab(Tab::Repos);
        tui.key(KeyCode::Char('n')).answer(&format!("{NODE}/public/demo"));
        tui.sent();
    }

    fn demo_url() -> RepoUrl {
        parse_repo(&format!("{NODE}/public/demo")).unwrap()
    }

    #[test]
    fn a_repository_is_copied_to_this_computer_and_the_copy_is_kept_track_of() {
        let mut tui = Tui::new("repos-copy");
        let source = Source::new(&tui.dir.join("source"));
        source.git(&["tag", "v1"]);
        open_demo(&mut tui);
        assert!(tui.draw().contains("No copy on this computer"));

        tui.key(KeyCode::Char('m'));
        let screen = tui.draw();
        assert!(screen.contains("Copying public/demo"), "{screen}");
        // The folder is the app's choice, under its data folder.
        let sent = tui.sent();
        let [NetCommand::Git { request: GitRequest::Mirror { dir }, .. }] = &sent[..] else { panic!("{sent:?}") };
        let dir = dir.clone();
        assert_eq!(dir, tui.app.paths.repos.join(NODE).join("public").join("demo.git"));
        // Asked again while it goes: refused, not sent twice.
        tui.key(KeyCode::Char('m'));
        assert!(tui.sent().is_empty());
        assert!(tui.said().contains("being copied already"), "{}", tui.said());

        // Answered: kept with the repository, and shown.
        let NetCommand::Git { id, repo, request, .. } = sent.into_iter().next().unwrap() else { unreachable!() };
        let result = answer(&Arc::new(source.node()), &repo, request);
        tui.app.on_net(NetEvent::Git { id, result });
        let kept = tui.app.store.repos[0].mirror.clone().expect("recorded");
        assert_eq!((kept.branches, kept.tags, kept.head.as_deref()), (1, 1, Some("main")));
        assert!(kept.bytes > 0 && kept.updated > 0.0);
        assert!(dir.join("HEAD").exists());
        let screen = tui.draw();
        assert!(screen.contains("Copy here: 1 branches and 1 tags") && screen.contains("⇩"), "{screen}");
        assert!(
            tui.app.notice.as_ref().is_some_and(|n| n.text.starts_with("Copied public/demo")),
            "{:?}",
            tui.app.notice.as_ref().map(|n| n.text.clone())
        );
        assert!(tui.app.store_dirty);

        // Updated: new work comes, and a gone tag goes.
        source.commit("two");
        source.git(&["tag", "-d", "v1"]);
        tui.key(KeyCode::Char('m'));
        copy_from(&mut tui, &source);
        let kept = tui.app.store.repos[0].mirror.clone().unwrap();
        assert_eq!((kept.branches, kept.tags), (1, 0));
        assert!(
            tui.app.notice.as_ref().is_some_and(|n| n.text.contains("Updated the copy") && n.text.contains("1 gone")),
            "{:?}",
            tui.app.notice.as_ref().map(|n| n.text.clone())
        );
    }

    fn other_url() -> RepoUrl {
        parse_repo(&format!("{NODE}/public/elsewhere")).unwrap()
    }

    #[test]
    fn a_copy_is_sent_on_to_another_repository() {
        let mut tui = Tui::new("repos-push");
        let source = Source::new(&tui.dir.join("source"));
        source.git(&["tag", "v1"]);
        open_demo(&mut tui);

        // Without a copy there is nothing to send.
        tui.key(KeyCode::Char('u'));
        assert!(tui.said().contains("no copy"), "{}", tui.said());
        tui.key(KeyCode::Char('m'));
        copy_from(&mut tui, &source);
        assert!(tui.app.store.repos[0].mirror.is_some());

        // Asked where, then how; not to itself, and only to a repository that can be one.
        tui.key(KeyCode::Char('u')).answer("nowhere");
        assert!(tui.said().contains("rns://<node>/<group>/<repo>"), "{}", tui.said());
        tui.key(KeyCode::Char('u')).answer(&format!("{NODE}/public/demo")).key(KeyCode::Enter);
        assert!(tui.said().contains("send it to another one"), "{}", tui.said());
        tui.key(KeyCode::Char('u')).answer(&format!("{NODE}/public/elsewhere")).answer("sideways");
        assert!(tui.said().contains("Type add, overwrite or same"), "{}", tui.said());
        assert!(tui.sent().is_empty());

        // Sent: to that repository, from the copy's folder, which is the app's choice.
        tui.key(KeyCode::Char('u')).answer(&format!("{NODE}/public/elsewhere")).answer("same");
        let sent = tui.sent();
        let [NetCommand::Git { repo, request: GitRequest::PushCopy { dir, force: true, prune: true }, .. }] = &sent[..] else {
            panic!("{sent:?}")
        };
        assert_eq!(*repo, other_url());
        assert_eq!(*dir, tui.app.paths.repos.join(NODE).join("public").join("demo.git"));
        let screen = tui.draw();
        assert!(screen.contains("Sending to public/else"), "{screen}");
        // Asked again while it goes: refused.
        tui.key(KeyCode::Char('u')).answer(&format!("{NODE}/public/elsewhere")).key(KeyCode::Enter);
        assert!(tui.said().contains("being sent there already"), "{}", tui.said());
        assert!(tui.sent().is_empty());

        // Answered: told what happened.
        let NetCommand::Git { id, repo, request, .. } = sent.into_iter().next().unwrap() else { unreachable!() };
        let node =
            Arc::new(Canned::default().reply("/git/list", "", Response::Packet(vec![0])).reply("/git/push", "", Response::Packet(vec![0])));
        let result = answer(&node, &repo, request);
        tui.app.on_net(NetEvent::Git { id, result });
        let said = tui.app.notice.as_ref().map(|n| n.text.clone()).unwrap_or_default();
        assert!(said.starts_with("Sent the copy to public/elsewhere: 2 sent"), "{said}");
        assert!(!tui.draw().contains("Sending to"));

        // Refused by the node: said, and it can be tried again.
        tui.key(KeyCode::Char('u')).answer(&format!("{NODE}/public/elsewhere")).key(KeyCode::Enter);
        answer_all(&mut tui, &Arc::new(Canned::default()));
        assert!(tui.said().contains("Sending the copy to public/elsewhere failed"), "{}", tui.said());
        tui.key(KeyCode::Char('u')).answer(&format!("{NODE}/public/elsewhere")).key(KeyCode::Enter);
        assert_eq!(tui.sent().len(), 1);
    }

    #[test]
    fn a_link_to_a_repository_offers_a_copy_once() {
        let mut tui = Tui::new("repos-link-copy");
        let source = Source::new(&tui.dir.join("source"));
        tui.tab(Tab::Repos);
        let link = format!("rns://{NODE}/public/demo");
        tui.app.open_address(&link);
        tui.sent();
        assert!(tui.draw().contains("Copy it to this computer"));
        tui.answer("y");
        let sent = tui.sent();
        assert!(matches!(&sent[..], [NetCommand::Git { request: GitRequest::Mirror { .. }, .. }]), "{sent:?}");
        for command in sent {
            let NetCommand::Git { id, repo, request, .. } = command else { unreachable!() };
            let result = answer(&Arc::new(source.node()), &repo, request);
            tui.app.on_net(NetEvent::Git { id, result });
        }
        assert!(tui.app.store.repos[0].mirror.is_some());
        // Copied already: the link just opens it.
        tui.app.open_address(&link);
        assert!(!tui.draw().contains("Copy it to this computer"));
        // And a link that isn't one says so, without asking anything.
        tui.app.open_address("rns://nothex/a/b");
        assert!(!tui.draw().contains("Copy it to this computer"));
    }

    /// A repository with a folder, a text file, a binary one and a file with control characters, copied.
    fn copied_with_files(name: &str) -> (Tui, Source) {
        let mut tui = Tui::new(name);
        let source = Source::new(&tui.dir.join("source"));
        std::fs::create_dir_all(source.0.join("docs/deep")).unwrap();
        std::fs::write(source.0.join("docs/guide.md"), "# Guide\nline two\n").unwrap();
        std::fs::write(source.0.join("docs/deep/x.txt"), "deep \x1b[31mred\x1b[0m\n").unwrap();
        std::fs::write(source.0.join("blob.bin"), [0u8, 1, 2]).unwrap();
        source.git(&["add", "."]);
        source.git(&["commit", "-q", "-m", "files and folders"]);
        source.git(&["tag", "v1"]);
        open_demo(&mut tui);
        tui.key(KeyCode::Char('m'));
        copy_from(&mut tui, &source);
        assert!(tui.app.store.repos[0].mirror.is_some());
        (tui, source)
    }

    #[test]
    fn the_copy_is_read_folder_by_folder_without_the_node() {
        let (mut tui, _source) = copied_with_files("repos-browse");
        // The node can't be asked, so the copy's branches show; Enter on one opens the top folder of
        // the copy: folders first, then files with sizes.
        tui.key(KeyCode::Char('r'));
        answer_all(&mut tui, &Arc::new(Canned::default()));
        tui.key(KeyCode::Enter);
        let screen = tui.draw();
        assert!(screen.contains("main · /") && screen.contains("▸ docs") && screen.contains("blob.bin  3 B"), "{screen}");
        assert!(screen.find("▸ docs").unwrap() < screen.find("blob.bin").unwrap());
        assert!(tui.sent().is_empty(), "nothing was asked of the node");

        // Into a folder, into another, and a file's text with the escape codes taken out.
        tui.key(KeyCode::Enter);
        assert!(tui.draw().contains("main · /docs"));
        tui.key(KeyCode::Enter);
        tui.key(KeyCode::Enter);
        let screen = tui.draw();
        assert!(screen.contains("main · /docs/deep/x.txt") && screen.contains("deep [31mred[0m") && !screen.contains('\u{1b}'), "{screen}");
        // Esc goes up one step at a time, then to the branches.
        tui.key(KeyCode::Esc);
        assert!(tui.draw().contains("main · /docs/deep") && tui.draw().contains("x.txt  "));
        tui.key(KeyCode::Backspace);
        tui.key(KeyCode::Esc);
        assert!(tui.draw().contains("main · /"));
        tui.key(KeyCode::Esc);
        let screen = tui.draw();
        assert!(screen.contains("branch main") && !screen.contains("▸ docs"), "{screen}");

        // The log of a tag.
        tui.key(KeyCode::Down).key(KeyCode::Down).key(KeyCode::Enter).key(KeyCode::Char('L'));
        let screen = tui.draw();
        assert!(screen.contains("v1 · newest commits") && screen.contains("files and folders") && screen.contains("one"), "{screen}");
        tui.key(KeyCode::Esc);
        assert!(tui.draw().contains("v1 · /"));
        // A binary file says so.
        tui.key(KeyCode::Down).key(KeyCode::Enter);
        let screen = tui.draw();
        assert!(screen.contains("Not text"), "{screen}");
    }

    #[test]
    fn looking_inside_needs_a_copy_and_a_node_that_does_not_answer_shows_the_copy() {
        let mut tui = Tui::new("repos-offline");
        open_demo(&mut tui);
        // No copy: nothing to look inside, and the node's refs are the only ones.
        let node = Arc::new(demo_node(&Identity::new()));
        tui.key(KeyCode::Char('r'));
        answer_all(&mut tui, &node);
        tui.key(KeyCode::Enter);
        let screen = tui.draw();
        assert!(tui.said().contains("no copy"), "{} {screen}", tui.said());

        // A copy, and a node that can't be reached: the copy's branches and tags show instead.
        let source = Source::new(&tui.dir.join("source"));
        source.git(&["tag", "v1"]);
        tui.key(KeyCode::Char('m'));
        copy_from(&mut tui, &source);
        tui.key(KeyCode::Char('r'));
        answer_all(&mut tui, &Arc::new(Canned::default()));
        let screen = tui.draw();
        assert!(screen.contains("The node didn't answer: these are the copy's"), "{screen}");
        assert!(screen.contains("branch main") && screen.contains("tag    v1") && screen.contains("HEAD"), "{screen}");
        tui.key(KeyCode::Enter);
        assert!(tui.draw().contains("main · /"));
        // A node that answers again replaces them.
        tui.key(KeyCode::Esc).key(KeyCode::Char('r'));
        answer_all(&mut tui, &Arc::new(demo_node(&Identity::new())));
        assert!(!tui.draw().contains("The node didn't answer"));
    }

    #[test]
    fn what_is_said_of_a_copy_sent() {
        let sent = |created, rejected: &[&str], more| Pushed {
            repo: other_url(),
            created,
            pushed: 3,
            up_to_date: 1,
            rejected: rejected.iter().map(|s| s.to_string()).collect(),
            rejected_count: rejected.len() + more,
            deleted: 0,
        };
        assert_eq!(push_said(&sent(false, &[], 0)), "Sent the copy to public/elsewhere: 3 sent, 1 already there");
        assert_eq!(push_said(&sent(true, &[], 0)), "Made public/elsewhere and sent the copy: 3 branches and tags");
        let said = push_said(&Pushed { deleted: 2, ..sent(false, &["main", "dev"], 3) });
        assert!(said.contains("2 removed there") && said.contains("differ there: main, dev and 3 more"), "{said}");
        assert_eq!(PushHow::parse(""), Some(PushHow::Add));
        assert_eq!(PushHow::parse(" Overwrite "), Some(PushHow::Overwrite));
        assert_eq!(
            (PushHow::Add.flags(), PushHow::Overwrite.flags(), PushHow::Same.flags()),
            ((false, false), (true, false), (true, true))
        );
    }

    #[test]
    fn a_copy_that_fails_says_so_and_keeps_nothing() {
        let mut tui = Tui::new("repos-copy-fails");
        open_demo(&mut tui);
        tui.key(KeyCode::Char('m'));
        answer_all(&mut tui, &Arc::new(Canned::default()));
        assert!(tui.said().contains("Copying public/demo failed"), "{}", tui.said());
        assert!(tui.app.store.repos[0].mirror.is_none());
        assert!(!tui.app.paths.repos.join(NODE).exists());
        // And it can be tried again.
        tui.key(KeyCode::Char('m'));
        assert_eq!(tui.sent().len(), 1);
    }

    #[test]
    fn the_copy_is_deleted_on_its_own_and_with_the_repository() {
        let mut tui = Tui::new("repos-copy-delete");
        let source = Source::new(&tui.dir.join("source"));
        open_demo(&mut tui);
        tui.key(KeyCode::Char('M'));
        assert!(tui.said().contains("no copy"), "{}", tui.said());
        tui.key(KeyCode::Char('m'));
        copy_from(&mut tui, &source);
        let dir = tui.app.paths.repos.join(NODE).join("public").join("demo.git");
        assert!(dir.exists());

        // Not unless it's confirmed.
        tui.key(KeyCode::Char('M')).answer("n");
        assert!(dir.exists() && tui.app.store.repos[0].mirror.is_some());
        tui.key(KeyCode::Char('M')).answer("y");
        assert!(!dir.exists() && tui.app.store.repos[0].mirror.is_none());
        // The folders it was in go with it; the repository is still kept.
        assert!(!tui.app.paths.repos.join(NODE).exists());
        assert_eq!(tui.app.store.repos.len(), 1);

        // Removing the repository from the list takes a copy with it.
        tui.key(KeyCode::Char('m'));
        copy_from(&mut tui, &source);
        assert!(dir.exists());
        tui.key(KeyCode::Esc).key(KeyCode::Char('D')).answer("y");
        assert!(tui.app.store.repos.is_empty() && !dir.exists());
    }

    #[test]
    fn a_copy_finished_for_a_repository_no_longer_kept_is_deleted() {
        let mut tui = Tui::new("repos-copy-orphan");
        let source = Source::new(&tui.dir.join("source"));
        open_demo(&mut tui);
        tui.key(KeyCode::Char('m'));
        let sent = tui.sent();
        let NetCommand::Git { id, repo, request, .. } = sent.into_iter().next().unwrap() else { panic!() };
        // Dropped while it was being copied.
        tui.app.remove_repo(&repo.to_string());
        let result = answer(&Arc::new(source.node()), &repo, request);
        tui.app.on_net(NetEvent::Git { id, result });
        assert!(tui.app.store.repos.is_empty());
        assert!(!tui.app.paths.repos.join(NODE).exists());
    }

    #[test]
    fn a_repository_called_dot_git_is_added_and_copied_like_any_other() {
        let text = format!("rns://{NODE}/BASH_BLOG/.git");
        let url = parse_repo(&text).unwrap();
        assert_eq!((url.group.as_str(), url.repo.as_str(), url.repo_path().as_str()), ("BASH_BLOG", ".git", "BASH_BLOG/.git"));
        assert_eq!(url.to_string(), text);
        assert_eq!(parse_repo(&text.replace("rns://", "")).unwrap(), url);
        let dir = mirror_dir(Path::new("/data/repos"), &url).unwrap();
        assert_eq!(dir, Path::new("/data/repos").join(NODE).join("BASH_BLOG").join(".git.git"));
        let mut tui = Tui::new("repos-dot-git");
        assert_eq!(tui.app.add_repo(&text).unwrap(), text);
        assert_eq!(tui.app.add_repo(&format!("{text}/")).unwrap(), text, "kept once");
        assert_eq!(tui.app.store.repos.len(), 1);
    }

    #[test]
    fn copies_go_in_folders_every_system_can_have() {
        let repos = Path::new("/data/repos");
        let url = |group: &str, repo: &str| RepoUrl::new([1; 16], group, repo).unwrap();
        let hex = "01".repeat(16);
        assert_eq!(mirror_dir(repos, &url("public", "demo")).unwrap(), repos.join(&hex).join("public").join("demo.git"));
        assert_eq!(mirror_dir(repos, &url("public", "demo.git")).unwrap(), mirror_dir(repos, &url("public", "demo")).unwrap());
        for bad in ["con", "CON", "nul", "aux.txt", "com1", "LPT9", "ends.", "prn."] {
            assert!(mirror_dir(repos, &url(bad, "demo")).is_err(), "{bad}");
            assert!(mirror_dir(repos, &url("public", bad)).is_err(), "{bad}");
        }
        for fine in ["console", "com10", "com", "a.b", "-x", "_y"] {
            assert!(mirror_dir(repos, &url("public", fine)).is_ok(), "{fine}");
        }
        // Names that would lead out of the folder never become a repository.
        for escape in ["..", ".", "a/b", "a\\b", ""] {
            assert!(RepoUrl::new([1; 16], "public", escape).is_err(), "{escape}");
            assert!(RepoUrl::new([1; 16], escape, "demo").is_err(), "{escape}");
        }
    }

    #[test]
    fn two_repositories_are_not_copied_into_one_folder() {
        let mut tui = Tui::new("repos-copy-clash");
        let source = Source::new(&tui.dir.join("source"));
        open_demo(&mut tui);
        tui.key(KeyCode::Char('m'));
        copy_from(&mut tui, &source);
        // The same name in other capitals would share the folder where capitals
        // don't count, so it isn't copied while the first is.
        tui.app.add_repo(&format!("{NODE}/Public/Demo")).unwrap();
        let other = parse_repo(&format!("{NODE}/Public/Demo")).unwrap();
        let clash = tui.app.mirror_request(&other).unwrap_err();
        assert!(clash.contains("same folder"), "{clash}");
        assert!(tui.app.mirror_request(&demo_url()).is_ok());
        // Once the first copy is gone, the other may be made.
        tui.app.remove_mirror(&demo_url().to_string()).unwrap();
        assert!(tui.app.mirror_request(&other).is_ok());
    }

    #[test]
    fn what_is_known_of_a_copy_is_saved_and_older_files_still_load() {
        let kept = SavedRepo {
            url: "rns://x/g/r".into(),
            mirror: Some(SavedMirror { updated: 5.0, branches: 2, tags: 1, bytes: 9, head: Some("main".into()) }),
        };
        let json = serde_json::to_string(&kept).unwrap();
        assert_eq!(serde_json::from_str::<SavedRepo>(&json).unwrap(), kept);
        // Without a copy nothing is written, so earlier versions read the file as ever.
        let plain = serde_json::to_string(&SavedRepo { url: "rns://x/g/r".into(), mirror: None }).unwrap();
        assert_eq!(plain, r#"{"url":"rns://x/g/r"}"#);
        assert_eq!(serde_json::from_str::<SavedRepo>(&plain).unwrap().mirror, None);
        // A copy's record without a HEAD, from a node that didn't say.
        let bare = r#"{"url":"u","mirror":{"updated":1.0,"branches":0,"tags":0,"bytes":0}}"#;
        assert_eq!(serde_json::from_str::<SavedRepo>(bare).unwrap().mirror.unwrap().head, None);
    }
}
