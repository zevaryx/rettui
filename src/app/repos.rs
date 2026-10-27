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
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;
use ratatui::widgets::ListState;
use rngit::RepoUrl;
use rngit::client::{ReleaseDetail, ReleaseList, WorkKind, WorkList, WorkScope, WorkSummary};

use super::{App, PromptKind, Tab, now};
use crate::net::git::{Doc, GitReply, GitRequest};
use crate::net::heard::{Heard, Kind};
use crate::net::{Hash, NetCommand};
use crate::store::SavedRepo;

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
    Release { tag: String, load: Load<ReleaseDetail> },
    Doc { id: u64, scope: WorkScope, load: Load<Doc> },
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
        self.asked.values().filter_map(|a| if let Asked::Write(_, what) = a { Some(what.clone()) } else { None }).collect()
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
    ConfirmRemove(String),
    NewTitle(bool),
    NewText { propose: bool, title: String },
    Comment { id: u64, scope: WorkScope },
    EditTitle { id: u64, scope: WorkScope },
    EditText { id: u64, scope: WorkScope, title: String },
    ConfirmDeleteDoc { id: u64, scope: WorkScope },
    ReleaseTag,
    ReleaseNotes(String),
    ReleaseFiles { tag: String, notes: String },
    ConfirmDeleteRelease(String),
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
            self.store.repos.push(SavedRepo { url: url.clone() });
            self.store_dirty = true;
        }
        Ok(url)
    }

    pub fn remove_repo(&mut self, url: &str) {
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
            None => {}
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
        let Some(asked) = self.repos.asked.remove(&id) else { return };
        let open = self.repos.open.as_mut();
        let is_open = |repo: &RepoUrl, open: &Option<&mut OpenRepo>| open.as_ref().is_some_and(|o| o.url == *repo);
        match (asked, result) {
            (Asked::Refs(repo), result) if is_open(&repo, &open) => {
                let open = open.expect("checked");
                open.refs = into_load(result, |r| if let GitReply::Refs(refs) = r { Some(refs) } else { None });
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
            (None, RepoPane::Branches) => open.refs.ready().map_or(0, |r| r.refs.len()),
            (None, RepoPane::Releases) => open.releases.ready().map_or(0, |r| r.releases.len()),
            (None, RepoPane::Work) => open.documents().len(),
            (None, RepoPane::Permissions) => 0,
        }
    }

    fn move_repo_items(&mut self, delta: isize) {
        if matches!(self.repos.open.as_ref().and_then(|o| o.detail.as_ref()), Some(Detail::Doc { .. }))
            || (self.repos.pane == RepoPane::Permissions && self.repos.open.as_ref().is_some_and(|o| o.detail.is_none()))
        {
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
                self.open_prompt(PromptKind::Repo(RepoPrompt::Add), "Add a repository (rns://node/group/repo)", "rns://")
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

    /// Enter on the left: a repository opens; a node asks which of its
    /// repositories (a node doesn't list them).
    fn open_side(&mut self) {
        let Some(selected) = self.repos.side.selected() else { return };
        if let Some(repo) = self.store.repos.get(selected).cloned() {
            if let Err(e) = self.open_repo(&repo.url) {
                self.warn(e);
            }
        } else if let Some(node) = self.repos.nodes.get(selected - self.store.repos.len()).cloned() {
            let start = format!("rns://{}/", hex::encode(node.hash));
            self.open_prompt(PromptKind::Repo(RepoPrompt::Add), "Which repository on this node? (rns://node/group/repo)", &start);
        }
    }

    fn repo_key(&mut self, code: KeyCode) {
        let Some(open) = self.repos.open.clone() else {
            self.repos.focus = ReposFocus::List;
            return;
        };
        match code {
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
        assert_eq!(tui.app.store.repos, [SavedRepo { url: format!("rns://{NODE}/public/demo") }]);
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
        // An rngit node heard: listed, and Enter asks which repository.
        let hash = [7; 16];
        let heard = Heard { hash, kind: Kind::Git, aspect: Some(crate::net::git::GIT_ASPECT), name: None, hops: 2 };
        tui.app.on_net(NetEvent::Heard(heard));
        assert!(tui.draw().contains("node 07070707"));
        tui.key(KeyCode::Enter).answer("public/site");
        let url = format!("rns://{}/public/site", hex::encode(hash));
        assert_eq!(tui.app.store.repos[0].url, url);
        // Asked as rngit's own identity, as the settings say.
        let sent = tui.sent();
        assert!(sent.iter().all(|c| matches!(c, NetCommand::Git { identity_file: Some(_), .. })));
        assert!(!sent.is_empty());

        // A link to one opens it.
        tui.app.open_address(&format!("RNS://{NODE}/public/demo"));
        assert_eq!(tui.app.tab, Tab::Repos);
        assert_eq!(tui.app.store.repos.len(), 2);
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
}
