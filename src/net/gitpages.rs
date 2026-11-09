//! The NomadNet pages of the rngit node hosted here: its repositories to
//! browse, as stock rngit's `serve_nomadnet` has them, at the same paths and
//! with the same page variables, so links between nodes work both ways.
//! Source files are coloured by [`crate::highlight`], and READMEs and other
//! Markdown are shown formatted.
//!
//! A visitor sees what they may clone: the node's rules are checked for each
//! page, and a repository they may not read looks like one that isn't there.
//! Everything is read with [`rngit::local`], which caps what it reads.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rngit::local::{self, ChangeKind, EntryKind};
use rngit::server::{NULL_IDENT, Node};

use crate::highlight;
use crate::nomad::micron::write::{self, escape, line, var};
use crate::ui::human_bytes;

pub const INDEX: &str = "/page/index.mu";
pub const GROUP: &str = "/page/group.mu";
pub const REPO: &str = "/page/repo.mu";
pub const TREE: &str = "/page/tree.mu";
pub const BLOB: &str = "/page/blob.mu";
pub const COMMITS: &str = "/page/commits.mu";
pub const COMMIT: &str = "/page/commit.mu";
pub const REFS: &str = "/page/refs.mu";
pub const DOWNLOAD: &str = "/file/download";

/// Every path the pages answer.
pub const PATHS: [&str; 9] = [INDEX, GROUP, REPO, TREE, BLOB, COMMITS, COMMIT, REFS, DOWNLOAD];

/// Most lines, and bytes of a file, shown on one page; the rest is a page on.
const PAGE_LINES: usize = 1000;
const PAGE_BYTES: usize = 128 * 1024;
/// Largest file read to show; a bigger one can be downloaded.
const MAX_SHOWN: usize = 8 * 1024 * 1024;
/// Largest file coloured; a bigger one is shown plain.
const MAX_COLOURED: usize = 1024 * 1024;
/// Most of a README shown under a repository's files.
const MAX_README: usize = 64 * 1024;
/// Largest file downloaded from the pages, whatever the node sends in a clone.
const MAX_DOWNLOAD: usize = 64 * 1024 * 1024;
/// Commits listed a page.
const LOG_PAGE: usize = 50;
/// Entries of a folder, and branches or tags, listed.
const MAX_LISTED: usize = 1000;
/// Repositories listed on the index or a group's page, and branches or
/// tags shown with their newest commit: each one is opened to read it.
const MAX_DETAILED: usize = 200;

/// What the pages need to know of the node they're for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// The name it announces.
    pub name: Option<String>,
    /// Its `git.repositories` address, for clone commands.
    pub address: [u8; 16],
}

impl Site {
    fn title(&self) -> &str {
        self.name.as_deref().unwrap_or("rngit node")
    }
}

/// What a page request gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Page(String),
    File { name: String, data: Vec<u8> },
}

/// The status a reply is counted with (as a protocol reply's first byte):
/// 0 for a page or file, 3 for one not found.
pub type Status = u8;
const OK: Status = 0;
const NOT_FOUND: Status = 3;

type Vars = BTreeMap<String, String>;

/// A failed page: what to tell the visitor.
struct Missing(String);

impl<E: std::fmt::Display> From<E> for Missing {
    fn from(error: E) -> Self {
        Missing(error.to_string())
    }
}

/// Answer one request for a page or a file, from `peer` (`None` if they
/// didn't identify).
pub fn answer(node: &Node, site: &Site, path: &str, vars: &Vars, peer: Option<[u8; 16]>) -> (Reply, Status) {
    let peer = peer.unwrap_or(NULL_IDENT);
    let pages = Pages { node, site, vars, peer };
    if node.blocked.contains(&peer) {
        let why = if peer == NULL_IDENT {
            "This node shows its repositories only to visitors who identify themselves. Identify to this node, then load the page again."
        } else {
            "This node doesn't show its repositories to you."
        };
        return (Reply::Page(pages.frame(&[], ">Not available", &format!("{why}\n"))), NOT_FOUND);
    }
    let result = match path {
        INDEX => pages.index(),
        GROUP => pages.group(),
        REPO => pages.repo(),
        TREE => pages.tree(),
        BLOB => pages.blob(),
        COMMITS => pages.commits(),
        COMMIT => pages.commit(),
        REFS => pages.refs(),
        DOWNLOAD => return pages.download().map_or_else(|Missing(why)| (Reply::Page(pages.missing(&why)), NOT_FOUND), |r| (r, OK)),
        _ => Err(Missing("There's no such page.".into())),
    };
    match result {
        Ok(page) => (Reply::Page(page), OK),
        Err(Missing(why)) => (Reply::Page(pages.missing(&why)), NOT_FOUND),
    }
}

/// A repository a visitor asked for and may read.
struct Repo {
    group: String,
    /// Its folder, `name.git`.
    folder: String,
    dir: PathBuf,
}

impl Repo {
    /// Its name as a clone URL has it.
    fn name(&self) -> &str {
        self.folder.strip_suffix(".git").unwrap_or(&self.folder)
    }

    /// The variables a link to it carries (with more after them).
    fn vars<'a>(&'a self, more: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut vars = vec![("g", self.group.as_str()), ("r", self.name())];
        vars.extend_from_slice(more);
        vars
    }
}

/// A branch, tag or commit a page is at.
struct At {
    /// What a link carries to come back to it: a short name or a hash.
    var: String,
    /// What `rngit::local` reads it by: a full ref name or a hash.
    rev: String,
    /// What it's called on the page.
    label: String,
}

struct Pages<'a> {
    node: &'a Node,
    site: &'a Site,
    vars: &'a Vars,
    peer: [u8; 16],
}

/// A link to one of these pages (or the download) on this node.
fn link(text: &str, path: &str, vars: &[(&str, &str)]) -> String {
    write::link(text, &format!(":{path}"), vars)
}

/// A date, as visitors anywhere read it.
fn date(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0).map_or_else(|| "?".into(), |d| d.format("%Y-%m-%d %H:%M UTC").to_string())
}

fn short(id: &impl std::fmt::Display) -> String {
    id.to_string().chars().take(8).collect()
}

/// The branch a repository shows first: HEAD's, or (when HEAD names one
/// that isn't there, as a new node's `master` with `main` pushed) the first.
fn default_branch(refs: &rngit::RemoteRefs) -> Option<String> {
    let head = refs.head.clone().filter(|h| refs.get(h).is_some());
    head.or_else(|| refs.refs.iter().find(|(_, n)| n.starts_with("refs/heads/")).map(|(_, n)| n.clone()))
}

/// A path in a repository joined to a relative link in a file at `from`
/// (`/` starts at the top); `None` if it leaves the repository.
fn relative(from: &str, link: &str) -> Option<String> {
    let link = link.split(['#', '?']).next().unwrap_or_default();
    if link.is_empty() {
        return None;
    }
    let mut parts: Vec<&str> = if link.starts_with('/') { Vec::new() } else { from.split('/').filter(|p| !p.is_empty()).collect() };
    if !link.starts_with('/') {
        parts.pop();
    }
    for part in link.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

impl Pages<'_> {
    fn var(&self, key: &str) -> Option<String> {
        var(self.vars, key).map(|v| v.into_owned()).filter(|v| !v.is_empty())
    }

    /// A page: the node's name, where the visitor is, and the content.
    fn frame(&self, crumbs: &[String], title: &str, body: &str) -> String {
        let mut out = format!("#!c=0\n`c`!{}`!\n`a\n", escape(self.site.title()));
        let mut trail = vec![link("Node", INDEX, &[])];
        trail.extend_from_slice(crumbs);
        out.push_str(&trail.join(" `F888/`f "));
        out.push_str("\n-\n");
        out.push_str(title);
        out.push('\n');
        out.push_str(body);
        out
    }

    fn missing(&self, why: &str) -> String {
        self.frame(&[], ">Not found", &format!("{}\n", line(why)))
    }

    fn not_found() -> Missing {
        Missing("This node has no such repository, or you may not read it.".into())
    }

    /// The repository the page is for, if the visitor may read it.
    fn repository(&self) -> Result<Repo, Missing> {
        let (Some(group), Some(name)) = (self.var("g"), self.var("r")) else { return Err(Self::not_found()) };
        let folder = if name.ends_with(".git") { name } else { format!("{name}.git") };
        rngit::server::split_repo_path(&format!("{group}/{folder}")).ok_or_else(Self::not_found)?;
        let dir = self.node.root.join(&group).join(&folder);
        if !dir.is_dir() || !self.node.allowed(&group, &folder, &self.peer, rngit::server::Perm::Read) {
            return Err(Self::not_found());
        }
        Ok(Repo { group, folder, dir })
    }

    /// Where the page is in its history: `ref` as a branch, a tag, a full
    /// ref name or a commit's hash; HEAD's branch without one. `None` for a
    /// repository with nothing in it.
    fn at(&self, repo: &Repo) -> Result<Option<At>, Missing> {
        let refs = local::refs(&repo.dir)?;
        let wanted = self.var("ref").filter(|r| r != "HEAD");
        let Some(wanted) = wanted else {
            let Some(name) = default_branch(&refs) else { return Ok(None) };
            let label = name.trim_start_matches("refs/heads/").to_string();
            return Ok(Some(At { var: label.clone(), rev: name, label }));
        };
        if wanted.len() == 40 && wanted.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(Some(At { label: format!("commit {}", short(&wanted)), rev: wanted.to_ascii_lowercase(), var: wanted }));
        }
        let full = [wanted.clone(), format!("refs/heads/{wanted}"), format!("refs/tags/{wanted}")]
            .into_iter()
            .find(|name| name.starts_with("refs/") && refs.get(name).is_some())
            .ok_or_else(|| Missing(format!("This repository has no branch or tag {wanted}.")))?;
        let label = full.trim_start_matches("refs/heads/").trim_start_matches("refs/tags/").to_string();
        Ok(Some(At { var: label.clone(), rev: full, label }))
    }

    fn repo_crumbs(&self, repo: &Repo) -> Vec<String> {
        vec![link(&repo.group, GROUP, &[("g", &repo.group)]), link(repo.name(), REPO, &repo.vars(&[]))]
    }

    /// The trail to a folder or file: each folder on the way links to it.
    fn path_crumbs(&self, repo: &Repo, at: &At, path: &str) -> Vec<String> {
        let mut crumbs = self.repo_crumbs(repo);
        crumbs.push(link(&at.label, TREE, &repo.vars(&[("ref", &at.var)])));
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        for (i, part) in parts.iter().enumerate() {
            if i + 1 == parts.len() {
                crumbs.push(escape(part));
            } else {
                let to = parts[..=i].join("/");
                crumbs.push(link(part, TREE, &repo.vars(&[("ref", &at.var), ("path", &to)])));
            }
        }
        crumbs
    }

    /// The links along the top of a repository's pages.
    fn repo_links(&self, repo: &Repo, at: Option<&At>) -> String {
        let at = at.map(|at| at.var.as_str()).unwrap_or("HEAD");
        let links = [
            link("Files", TREE, &repo.vars(&[("ref", at)])),
            link("Commits", COMMITS, &repo.vars(&[("ref", at)])),
            link("Branches and tags", REFS, &repo.vars(&[])),
        ];
        links.join("  `F888·`f  ")
    }

    /// The text of a repository's `description` file, unless it's git's placeholder.
    fn description(repo: &Repo) -> Option<String> {
        use std::io::Read as _;
        let mut text = String::new();
        std::fs::File::open(repo.dir.join("description")).ok()?.take(300).read_to_string(&mut text).ok()?;
        let text = text.trim();
        (!text.is_empty() && !text.starts_with("Unnamed repository")).then(|| text.lines().next().unwrap_or_default().to_string())
    }

    /// One line about a repository, for the lists of them.
    fn listed(&self, group: &str, folder: &str) -> String {
        let repo = Repo { group: group.to_string(), folder: folder.to_string(), dir: self.node.root.join(group).join(folder) };
        let mut out = format!("  {}", link(repo.name(), REPO, &repo.vars(&[])));
        if let Some(description) = Self::description(&repo) {
            out.push_str(&format!(" `F888—`f {}", escape(&description)));
        }
        let newest = self.at(&repo).ok().flatten().and_then(|at| local::log(&repo.dir, &at.rev, 1).ok()).and_then(|l| l.into_iter().next());
        match newest {
            Some(commit) => out.push_str(&format!(" `F888· {}`f", date(commit.time))),
            None => out.push_str(" `F888· empty`f"),
        }
        out.push('\n');
        out
    }

    fn index(&self) -> Result<String, Missing> {
        let readable = self.node.readable_by(&self.peer);
        let mut body = String::new();
        if readable.is_empty() {
            body.push_str("There are no repositories here you may read.\n");
        }
        let mut group: Option<&str> = None;
        for (g, folder) in readable.iter().take(MAX_DETAILED) {
            if group != Some(g) {
                body.push_str(&format!("\n>>{}\n", link(g, GROUP, &[("g", g)])));
                group = Some(g);
            }
            body.push_str(&self.listed(g, folder));
        }
        if readable.len() > MAX_DETAILED {
            body.push_str(&format!("\n<\n`F888and {} more, in their groups`f\n", readable.len() - MAX_DETAILED));
        }
        body.push_str(&format!(
            "\n<\n`F888Clone any of them with`f `Fdd6git clone rns://{}/<group>/<name>`f `F888(with git-remote-rns)`f\n",
            hex::encode(self.site.address)
        ));
        Ok(self.frame(&[], ">Repositories", &body))
    }

    fn group(&self) -> Result<String, Missing> {
        let group = self.var("g").ok_or_else(Self::not_found)?;
        let mine: Vec<_> = self.node.readable_by(&self.peer).into_iter().filter(|(g, _)| *g == group).collect();
        if mine.is_empty() {
            return Err(Missing("This node has no such group, or nothing in it you may read.".into()));
        }
        let mut body: String = mine.iter().take(MAX_DETAILED).map(|(g, folder)| self.listed(g, folder)).collect();
        if mine.len() > MAX_DETAILED {
            body.push_str(&format!("  `F888and {} more`f\n", mine.len() - MAX_DETAILED));
        }
        Ok(self.frame(&[escape(&group)], &format!(">{}", escape(&group)), &body))
    }

    fn clone_line(&self, repo: &Repo) -> String {
        format!("`F888Clone:`f `Fdd6git clone rns://{}/{}/{}`f\n", hex::encode(self.site.address), escape(&repo.group), escape(repo.name()))
    }

    fn repo(&self) -> Result<String, Missing> {
        let repo = self.repository()?;
        let at = self.at(&repo)?;
        let mut body = String::new();
        if let Some(description) = Self::description(&repo) {
            body.push_str(&format!("{}\n\n", line(&description)));
        }
        body.push_str(&self.clone_line(&repo));
        let Some(at) = at else {
            body.push_str(&format!(
                "\nNothing has been pushed here yet. Push to it with `Fdd6git push rns://{}/{}/{} main`f.\n",
                hex::encode(self.site.address),
                escape(&repo.group),
                escape(repo.name())
            ));
            return Ok(self.frame(&self.repo_crumbs(&repo)[..1], &format!(">{}", escape(repo.name())), &body));
        };
        body.push_str(&format!("`F888On:`f {}\n", escape(&at.label)));
        body.push_str(&self.repo_links(&repo, Some(&at)));
        body.push('\n');
        if let Some(commit) = local::log(&repo.dir, &at.rev, 1)?.into_iter().next() {
            let id = commit.id.to_string();
            body.push_str(&format!(
                "`F888Latest:`f {} {} `F888— {}, {}`f\n",
                link(&short(&id), COMMIT, &repo.vars(&[("ref", &at.var), ("h", &id)])),
                escape(&commit.subject),
                escape(&commit.author),
                date(commit.time)
            ));
        }
        let entries = local::tree(&repo.dir, &at.rev, "")?;
        body.push_str("\n>>Files\n");
        body.push_str(&self.listing(&repo, &at, "", &entries));
        let readme = entries.iter().filter(|e| e.kind == EntryKind::File).find(|e| {
            let lower = e.name.to_ascii_lowercase();
            ["readme.md", "readme.markdown", "readme.mu", "readme", "readme.txt"].contains(&lower.as_str())
        });
        if let Some(readme) = readme
            && let Ok(view) = local::file(&repo.dir, &at.rev, &readme.name, MAX_README)
            && let Some(text) = view.text
        {
            body.push_str(&format!("\n>>{}\n", escape(&readme.name)));
            // Its headings under the README's own.
            body.push_str(&self.shown(&repo, &at, &readme.name, &text, 3));
            body.push('\n');
            if view.truncated {
                body.push_str(&format!("\n{}\n", link("Read on", BLOB, &repo.vars(&[("ref", &at.var), ("path", &readme.name)]))));
            }
        }
        Ok(self.frame(&self.repo_crumbs(&repo)[..1], &format!(">{}", escape(repo.name())), &body))
    }

    /// A folder's entries, each a link to it.
    fn listing(&self, repo: &Repo, at: &At, path: &str, entries: &[local::TreeEntry]) -> String {
        let mut out = String::new();
        if !path.is_empty() {
            let up = path.rsplit_once('/').map_or("", |(up, _)| up);
            out.push_str(&format!("  {}\n", link("..", TREE, &repo.vars(&[("ref", &at.var), ("path", up)]))));
        }
        for entry in entries.iter().take(MAX_LISTED) {
            let full = if path.is_empty() { entry.name.clone() } else { format!("{path}/{}", entry.name) };
            let to = |page| link(&entry.name, page, &repo.vars(&[("ref", &at.var), ("path", &full)]));
            match entry.kind {
                EntryKind::Dir => out.push_str(&format!("  `F7bf▸`f {}`F888/`f\n", to(TREE))),
                EntryKind::File => out.push_str(&format!("    {} `F888{}`f\n", to(BLOB), human_bytes(entry.size.unwrap_or(0)))),
                EntryKind::Link => out.push_str(&format!("    {} `F888(link)`f\n", to(BLOB))),
                EntryKind::Submodule => out.push_str(&format!("    {} `F888(another repository)`f\n", escape(&entry.name))),
            }
        }
        if entries.len() > MAX_LISTED {
            out.push_str(&format!("  `F888and {} more`f\n", entries.len() - MAX_LISTED));
        }
        if entries.is_empty() {
            out.push_str("  `F888Empty`f\n");
        }
        out
    }

    fn path(&self) -> Result<String, Missing> {
        let path = self.var("path").unwrap_or_default();
        Ok(path.trim_matches('/').to_string())
    }

    fn tree(&self) -> Result<String, Missing> {
        let repo = self.repository()?;
        let at = self.at(&repo)?.ok_or_else(|| Missing("Nothing has been pushed to this repository yet.".into()))?;
        let path = self.path()?;
        if local::kind(&repo.dir, &at.rev, &path)?.is_some_and(|kind| kind != EntryKind::Dir) {
            return self.blob();
        }
        let entries = local::tree(&repo.dir, &at.rev, &path)?;
        let title = if path.is_empty() { format!(">{}", escape(repo.name())) } else { format!(">{}", escape(&path)) };
        let mut body = format!("{}\n\n", self.repo_links(&repo, Some(&at)));
        body.push_str(&self.listing(&repo, &at, &path, &entries));
        Ok(self.frame(&self.path_crumbs(&repo, &at, &path), &title, &body))
    }

    /// A text file shown whole: Markdown (its headings from `level` `>`s)
    /// and Micron formatted, anything else as numbered, coloured lines.
    fn shown(&self, repo: &Repo, at: &At, path: &str, text: &str, level: usize) -> String {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".md") || lower.ends_with(".markdown") {
            let to = |said: &str, url: &str| {
                if url.contains("://") || url.starts_with("mailto:") || url.starts_with('#') {
                    return None;
                }
                let target = relative(path, url)?;
                Some(link(said, BLOB, &repo.vars(&[("ref", &at.var), ("path", &target)])))
            };
            return crate::markdown::micron(text, level, &to);
        }
        if lower.ends_with(".mu") {
            // A page in the repository, as it would show on a node; `<` keeps its sections apart from this page's.
            return format!("<\n{text}\n<");
        }
        let first = text.lines().next().unwrap_or_default();
        let lang = highlight::language(path, first).filter(|_| text.len() <= MAX_COLOURED);
        let lines = write::code(lang, text);
        let width = lines.len().to_string().len();
        lines.iter().enumerate().map(|(i, l)| format!("`F666 {:>width$}`f  {l}\n", i + 1)).collect()
    }

    fn blob(&self) -> Result<String, Missing> {
        let repo = self.repository()?;
        let at = self.at(&repo)?.ok_or_else(|| Missing("Nothing has been pushed to this repository yet.".into()))?;
        let path = self.path()?;
        match local::kind(&repo.dir, &at.rev, &path)? {
            None | Some(EntryKind::Dir) => return self.tree(),
            _ => {}
        }
        let crumbs = self.path_crumbs(&repo, &at, &path);
        let name = path.rsplit('/').next().unwrap_or(&path);
        let download = link("Download", DOWNLOAD, &repo.vars(&[("ref", &at.var), ("path", &path)]));
        let data = match local::read(&repo.dir, &at.rev, &path, MAX_SHOWN) {
            Ok(data) => data,
            Err(rngit::Error::TooLarge { size, .. }) => {
                let body = format!("{} is {}, too big to show here.\n\n{download}\n", escape(name), human_bytes(size as u64));
                return Ok(self.frame(&crumbs, &format!(">{}", escape(name)), &body));
            }
            Err(e) => return Err(e.into()),
        };
        let Some(text) = std::str::from_utf8(&data).ok().filter(|t| !t.contains('\0')) else {
            let body = format!("A binary file of {}.\n\n{download}\n", human_bytes(data.len() as u64));
            return Ok(self.frame(&crumbs, &format!(">{}", escape(name)), &body));
        };
        let lower = path.to_ascii_lowercase();
        let renders = [".md", ".markdown", ".mu"].iter().any(|ext| lower.ends_with(ext));
        let rendered = renders && self.var("raw").is_none();
        let total = text.lines().count();
        let mut about = vec![human_bytes(data.len() as u64), format!("{total} line{}", if total == 1 { "" } else { "s" })];
        if let Some(lang) = highlight::language(&path, text.lines().next().unwrap_or_default()) {
            about.push(lang.name.to_string());
        }
        let mut links = vec![download];
        if renders {
            let other = if rendered {
                link("Source", BLOB, &repo.vars(&[("ref", &at.var), ("path", &path), ("raw", "y")]))
            } else {
                link("Formatted", BLOB, &repo.vars(&[("ref", &at.var), ("path", &path), ("render", "y")]))
            };
            links.insert(0, other);
        }
        let mut body = format!("`F888{}`f  {}\n\n", about.join(" · "), links.join("  `F888·`f  "));
        if rendered {
            body.push_str(&self.shown(&repo, &at, &path, text, 2));
            body.push('\n');
            return Ok(self.frame(&crumbs, &format!(">{}", escape(name)), &body));
        }
        // A page of lines at a time, numbered as in the whole file.
        let from = self.var("from").and_then(|f| f.parse::<usize>().ok()).unwrap_or(1).clamp(1, total.max(1));
        let start = text.split_inclusive('\n').take(from - 1).map(str::len).sum::<usize>();
        let (mut end, mut shown) = (start, 0);
        for piece in text[start..].split_inclusive('\n') {
            if shown == PAGE_LINES || (shown > 0 && end + piece.len() - start > PAGE_BYTES) {
                break;
            }
            end += piece.len();
            shown += 1;
        }
        // Coloured from the top, so a comment or string begun above the page shows as one.
        let lang = highlight::language(&path, text.lines().next().unwrap_or_default()).filter(|_| text.len() <= MAX_COLOURED);
        let lines = write::code(lang, &text[..end]);
        let width = (from + shown).to_string().len();
        for (i, l) in lines.iter().enumerate().skip(from - 1).take(shown) {
            body.push_str(&format!("`F666 {:>width$}`f  {l}\n", i + 1));
        }
        if shown < total.saturating_sub(from - 1) || from > 1 {
            let mut pager = Vec::new();
            if from > 1 {
                let back = from.saturating_sub(PAGE_LINES).max(1).to_string();
                pager.push(link("Earlier lines", BLOB, &repo.vars(&[("ref", &at.var), ("path", &path), ("raw", "y"), ("from", &back)])));
            }
            if from + shown <= total {
                let next = (from + shown).to_string();
                pager.push(link("Later lines", BLOB, &repo.vars(&[("ref", &at.var), ("path", &path), ("raw", "y"), ("from", &next)])));
            }
            body.push_str(&format!("\n`F888Lines {from} to {} of {total}`f  {}\n", from + shown - 1, pager.join("  ")));
        }
        Ok(self.frame(&crumbs, &format!(">{}", escape(name)), &body))
    }

    fn commits(&self) -> Result<String, Missing> {
        let repo = self.repository()?;
        let at = self.at(&repo)?.ok_or_else(|| Missing("Nothing has been pushed to this repository yet.".into()))?;
        let skip = self.var("skip").and_then(|s| s.parse::<usize>().ok()).unwrap_or(0).min(local::MAX_SKIP);
        let (commits, more) = local::log_page(&repo.dir, &at.rev, skip, LOG_PAGE)?;
        let mut body = format!("{}\n\n", self.repo_links(&repo, Some(&at)));
        for commit in &commits {
            let id = commit.id.to_string();
            body.push_str(&format!(
                "`F888{}`f {}\n   `F888{}, {}`f\n",
                short(&id),
                link(&commit.subject, COMMIT, &repo.vars(&[("ref", &at.var), ("h", &id)])),
                escape(&commit.author),
                date(commit.time)
            ));
        }
        let mut pager = Vec::new();
        if skip > 0 {
            let newer = skip.saturating_sub(LOG_PAGE).to_string();
            pager.push(link("Newer", COMMITS, &repo.vars(&[("ref", &at.var), ("skip", &newer)])));
        }
        if more && skip + LOG_PAGE <= local::MAX_SKIP {
            let older = (skip + LOG_PAGE).to_string();
            pager.push(link("Older", COMMITS, &repo.vars(&[("ref", &at.var), ("skip", &older)])));
        }
        if !pager.is_empty() {
            body.push_str(&format!("\n{}\n", pager.join("  ")));
        }
        let mut crumbs = self.repo_crumbs(&repo);
        crumbs.push("commits".into());
        Ok(self.frame(&crumbs, &format!(">Commits on {}", escape(&at.label)), &body))
    }

    fn commit(&self) -> Result<String, Missing> {
        let repo = self.repository()?;
        let hash = self.var("h").filter(|h| h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit())).ok_or_else(Self::not_found)?;
        let detail =
            local::commit(&repo.dir, &hash.to_ascii_lowercase()).map_err(|_| Missing("This repository has no such commit.".into()))?;
        let id = detail.info.id.to_string();
        let mut body = format!(
            "`F888Author:`f {} <{}>\n`F888Date:`f   {}\n`F888Commit:`f {id}\n",
            escape(&detail.info.author),
            escape(&detail.email),
            date(detail.info.time)
        );
        for parent in &detail.parents {
            let parent = parent.to_string();
            body.push_str(&format!("`F888Parent:`f {}\n", link(&parent, COMMIT, &repo.vars(&[("h", &parent)]))));
        }
        body.push_str(&format!("{}\n", link("Browse the files at this commit", TREE, &repo.vars(&[("ref", &id)]))));
        let message = detail.message.trim_end();
        let rest = message.split_once('\n').map(|(_, rest)| rest.trim()).unwrap_or_default();
        if !rest.is_empty() {
            body.push('\n');
            for l in rest.lines() {
                body.push_str(&format!("{}\n", line(l)));
            }
        }
        body.push_str(&format!("\n>>Changed ({}{})\n", detail.changes.len(), if detail.more_changes { "+" } else { "" }));
        let before = detail.parents.first().map(ToString::to_string);
        for change in &detail.changes {
            let (mark, at) = match change.kind {
                ChangeKind::Added => ("`F9d7A`f", Some(id.as_str())),
                ChangeKind::Modified => ("`Ffc6M`f", Some(id.as_str())),
                ChangeKind::Deleted => ("`Ff77D`f", before.as_deref()),
            };
            let shown = match at {
                Some(at) => link(&change.path, BLOB, &repo.vars(&[("ref", at), ("path", &change.path)])),
                None => escape(&change.path),
            };
            body.push_str(&format!("  {mark} {shown}\n"));
        }
        if detail.more_changes {
            body.push_str("  `F888and more`f\n");
        }
        let mut crumbs = self.repo_crumbs(&repo);
        crumbs.push(format!("commit {}", short(&id)));
        Ok(self.frame(&crumbs, &format!(">{}", escape(&detail.info.subject)), &body))
    }

    fn refs(&self) -> Result<String, Missing> {
        let repo = self.repository()?;
        let refs = local::refs(&repo.dir)?;
        let default = default_branch(&refs);
        let mut body = format!("{}\n", self.repo_links(&repo, None));
        for (title, prefix) in [("Branches", "refs/heads/"), ("Tags", "refs/tags/")] {
            body.push_str(&format!("\n>>{title}\n"));
            let names: Vec<&str> = refs.refs.iter().filter_map(|(_, n)| n.strip_prefix(prefix)).collect();
            if names.is_empty() {
                body.push_str("  `F888None`f\n");
            }
            for (i, name) in names.into_iter().take(MAX_LISTED).enumerate() {
                let head = default.as_deref() == Some(&format!("{prefix}{name}"));
                let newest = (i < MAX_DETAILED)
                    .then(|| local::log(&repo.dir, &format!("{prefix}{name}"), 1).ok())
                    .flatten()
                    .and_then(|l| l.into_iter().next());
                body.push_str(&format!("  {}", link(name, TREE, &repo.vars(&[("ref", name)]))));
                if head {
                    body.push_str(" `F9d7(default)`f");
                }
                if let Some(commit) = newest {
                    body.push_str(&format!(" `F888· {} · {}`f", escape(&commit.subject), date(commit.time)));
                }
                body.push('\n');
            }
        }
        let mut crumbs = self.repo_crumbs(&repo);
        crumbs.push("branches and tags".into());
        Ok(self.frame(&crumbs, ">Branches and tags", &body))
    }

    fn download(&self) -> Result<Reply, Missing> {
        let repo = self.repository()?;
        let at = self.at(&repo)?.ok_or_else(|| Missing("Nothing has been pushed to this repository yet.".into()))?;
        let path = self.path()?;
        let most = self.node.max_bundle_bytes.min(MAX_DOWNLOAD);
        let data = local::read(&repo.dir, &at.rev, &path, most).map_err(|e| match e {
            rngit::Error::TooLarge { size, .. } => {
                Missing(format!("That file is {}, more than this node sends.", human_bytes(size as u64)))
            }
            e => e.into(),
        })?;
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        Ok(Reply::File { name, data })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::*;

    const ME: [u8; 16] = [5; 16];

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.name=Ann", "-c", "user.email=ann@example.org", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A node with `public/demo.git` (anyone may read it) and
    /// `team/secret.git` (only [`ME`]).
    fn node(name: &str) -> (Node, PathBuf) {
        let dir = std::env::temp_dir().join(format!("rettui-gitpages-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let work = dir.join("work");
        std::fs::create_dir_all(work.join("src")).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        std::fs::write(
            work.join("README.md"),
            "# Demo\n\nSee [the code](src/main.rs) and [the web](https://example.org).\n\n```rust\nfn main() {}\n```\n",
        )
        .unwrap();
        std::fs::write(work.join("src/main.rs"), "// The start.\nfn main() {\n    println!(\"hi `there`\");\n}\n").unwrap();
        std::fs::write(work.join("long.txt"), (1..=2500).map(|n| format!("line {n}\n")).collect::<String>()).unwrap();
        std::fs::write(work.join("blob.bin"), [0u8, 1, 2]).unwrap();
        std::fs::write(work.join("we|ird name.txt"), "odd").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "First\n\nWith a body."]);
        std::fs::write(work.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::remove_file(work.join("blob.bin")).unwrap();
        git(&work, &["commit", "-q", "-am", "Second"]);
        git(&work, &["tag", "v1"]);
        let root = dir.join("repos");
        let node = Node::new(root.clone());
        node.make_repository("public", "demo.git", None).unwrap();
        node.make_repository("team", "secret.git", None).unwrap();
        node.make_repository("public", "empty.git", None).unwrap();
        git(&work, &["push", "-q", root.join("public/demo.git").to_str().unwrap(), "main", "v1"]);
        git(&work, &["push", "-q", root.join("team/secret.git").to_str().unwrap(), "main"]);
        std::fs::write(root.join("public/demo.git/description"), "A demo of the pages\n").unwrap();
        node.set_rules("public", None, "r:all").unwrap();
        node.set_rules("team", None, &format!("r:{}", hex::encode(ME))).unwrap();
        (node, dir)
    }

    fn site() -> Site {
        Site { name: Some("Moon forge".into()), address: [7; 16] }
    }

    fn vars(pairs: &[(&str, &str)]) -> Vars {
        pairs.iter().map(|(k, v)| (format!("var_{k}"), v.to_string())).collect()
    }

    fn page(node: &Node, path: &str, pairs: &[(&str, &str)], peer: Option<[u8; 16]>) -> (String, Status) {
        match answer(node, &site(), path, &vars(pairs), peer) {
            (Reply::Page(page), status) => (page, status),
            (Reply::File { .. }, _) => panic!("a file, not a page"),
        }
    }

    /// The links on a page: where each goes and with what variables.
    fn links(page: &str) -> Vec<(String, Vars)> {
        let parsed = crate::nomad::micron::parse(page);
        parsed
            .items
            .iter()
            .filter_map(|item| match item {
                crate::nomad::micron::Interactive::Link { url, fields } => Some((url.clone(), parsed.request_fields(fields))),
                _ => None,
            })
            .collect()
    }

    /// Follow the first link on `page` whose variables include `pair`.
    fn follow(node: &Node, page: &str, path: &str, key: &str, value: &str) -> String {
        let (_, fields) = links(page)
            .into_iter()
            .find(|(url, fields)| url == &format!(":{path}") && var(fields, key).as_deref() == Some(value))
            .unwrap_or_else(|| panic!("no link to {path} with {key}={value} in\n{page}"));
        match answer(node, &site(), path, &fields, None) {
            (Reply::Page(page), 0) => page,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_visitor_browses_from_the_index_to_a_file() {
        let (node, dir) = node("browse");
        let (index, status) = page(&node, INDEX, &[], None);
        assert_eq!(status, OK);
        assert!(index.starts_with("#!c=0\n`c`!Moon forge`!"), "{index}");
        assert!(index.contains("A demo of the pages") && index.contains("empty"), "{index}");
        assert!(index.contains(&format!("rns://{}/<group>/<name>", hex::encode([7u8; 16]))));
        // The team's repository isn't there for someone who may not read it.
        assert!(!index.contains("secret") && !index.contains("team"), "{index}");
        let repo = follow(&node, &index, REPO, "r", "demo");
        assert!(repo.contains(&format!("git clone rns://{}/public/demo", hex::encode([7u8; 16]))), "{repo}");
        assert!(repo.contains("`F888On:`f main") && repo.contains("Second"), "{repo}");
        // The README, formatted, with its relative link made a page link and its code coloured.
        assert!(repo.contains(">>README.md\n>>>Demo"), "{repo}");
        assert!(repo.contains("`[the code`:/page/blob.mu`g=public|r=demo|ref=main|path=src/main.rs]"), "{repo}");
        assert!(repo.contains("the web (https://example.org)") && repo.contains("`Fc8ffn`f"), "{repo}");
        let tree = follow(&node, &repo, TREE, "path", "src");
        assert!(tree.contains("`[..`:/page/tree.mu"), "{tree}");
        let blob = follow(&node, &tree, BLOB, "path", "src/main.rs");
        assert!(blob.contains("Rust") && blob.contains("`F666 1`f  `Fc8ffn`f `F7bfmain`f() {}"), "{blob}");
        // The file at the first commit, from its history.
        let commits = follow(&node, &repo, COMMITS, "ref", "main");
        let first = follow(&node, &commits, COMMIT, "h", &git(&node.root.join("public/demo.git"), &["rev-parse", "main~1"]));
        assert!(first.contains(">First") && first.contains("\nWith a body.\n"), "{first}");
        assert!(first.contains("`F9d7A`f `[src/main.rs`"), "{first}");
        let old = follow(&node, &first, BLOB, "path", "src/main.rs");
        assert!(old.contains("`F888`*// The start.`*`f") && old.contains("hi \\`there\\`"), "{old}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rules_hide_repositories_and_blocked_visitors_see_nothing() {
        let (mut node, dir) = node("rules");
        let secret = [("g", "team"), ("r", "secret")];
        let (page_for_anyone, status) = page(&node, REPO, &secret, None);
        assert_eq!(status, NOT_FOUND);
        assert!(page_for_anyone.contains(">Not found"));
        let (mine, status) = page(&node, REPO, &secret, Some(ME));
        assert_eq!(status, OK, "{mine}");
        assert!(page(&node, INDEX, &[], Some(ME)).0.contains("secret"));
        // Every page of it is hidden, and so is its group.
        for path in [TREE, BLOB, COMMITS, REFS, GROUP] {
            assert_eq!(page(&node, path, &[("g", "team"), ("r", "secret"), ("path", "README.md")], None).1, NOT_FOUND, "{path}");
        }
        assert!(matches!(
            answer(&node, &site(), DOWNLOAD, &vars(&[("g", "team"), ("r", "secret"), ("path", "README.md")]), None).1,
            NOT_FOUND
        ));
        // Names that aren't a group and repository, or no page at all.
        for bad in [[("g", ".."), ("r", "demo")], [("g", "public"), ("r", "../team/secret")], [("g", "public"), ("r", "")]] {
            assert_eq!(page(&node, REPO, &bad, Some(ME)).1, NOT_FOUND, "{bad:?}");
        }
        assert_eq!(page(&node, "/page/other.mu", &[], None).1, NOT_FOUND);
        node.blocked.insert(NULL_IDENT);
        let (page_for_anyone, status) = page(&node, INDEX, &[], None);
        assert!(status == NOT_FOUND && page_for_anyone.contains("identify"), "{page_for_anyone}");
        node.blocked.insert(ME);
        assert!(page(&node, INDEX, &[], Some(ME)).0.contains("doesn't show"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn files_page_by_lines_and_odd_ones_say_what_they_are() {
        let (node, dir) = node("files");
        fn demo<'a>(more: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
            let mut all = vec![("g", "public"), ("r", "demo")];
            all.extend_from_slice(more);
            all
        }
        let (long, _) = page(&node, BLOB, &demo(&[("path", "long.txt")]), None);
        assert!(long.contains("2500 lines") && long.contains("line 1000\n") && !long.contains("line 1001\n"), "{}", &long[..300]);
        assert!(long.contains("Lines 1 to 1000 of 2500"));
        let later = follow(&node, &long, BLOB, "from", "1001");
        assert!(later.contains("`F666 1001`f  line 1001") && later.contains("Earlier lines"), "{}", &later[..400]);
        let last = follow(&node, &later, BLOB, "from", "2001");
        assert!(last.contains("Lines 2001 to 2500 of 2500") && !last.contains("Later lines"));
        // A binary file at the tag (removed since), and a name links can't carry as it is.
        let (binary, _) = page(&node, BLOB, &demo(&[("ref", "v1~"), ("path", "blob.bin")]), None);
        assert!(binary.contains("no branch or tag"), "{binary}");
        let first = git(&node.root.join("public/demo.git"), &["rev-parse", "main~1"]);
        let (binary, _) = page(&node, BLOB, &demo(&[("ref", &first), ("path", "blob.bin")]), None);
        assert!(binary.contains("A binary file of 3 B"), "{binary}");
        let (top, _) = page(&node, TREE, &demo(&[]), None);
        let weird = follow(&node, &top, BLOB, "path", "we|ird name.txt");
        assert!(weird.contains("odd"), "{weird}");
        // Markdown is formatted unless its source is asked for.
        let (readme, _) = page(&node, BLOB, &demo(&[("path", "README.md")]), None);
        assert!(readme.contains(">>Demo") && readme.contains("`[Source`"), "{readme}");
        let source = follow(&node, &readme, BLOB, "raw", "y");
        assert!(source.contains("`F666 1`f  `Fc8f# Demo`f"), "{source}");
        // A folder asked for as a file shows the folder, and the other way round.
        assert!(page(&node, BLOB, &demo(&[("path", "src")]), None).0.contains("main.rs"));
        assert!(page(&node, TREE, &demo(&[("path", "src/main.rs")]), None).0.contains("`F666 1`f"));
        assert_eq!(page(&node, TREE, &demo(&[("path", "../x")]), None).1, NOT_FOUND);
        match answer(&node, &site(), DOWNLOAD, &vars(&demo(&[("path", "src/main.rs")])), None) {
            (Reply::File { name, data }, OK) => assert_eq!((name.as_str(), data.as_slice()), ("main.rs", b"fn main() {}\n".as_slice())),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_branches_and_empty_repositories() {
        let (node, dir) = node("history");
        let (refs, _) = page(&node, REFS, &[("g", "public"), ("r", "demo.git")], None);
        assert!(refs.contains(">>Branches\n  `[main`") && refs.contains("(default)") && refs.contains(">>Tags\n  `[v1`"), "{refs}");
        let tag = follow(&node, &refs, TREE, "ref", "v1");
        assert!(tag.contains("README.md"), "{tag}");
        let (commits, _) = page(&node, COMMITS, &[("g", "public"), ("r", "demo")], None);
        assert!(commits.contains(">Commits on main") && commits.contains("Second") && commits.contains("First"));
        assert!(!commits.contains("Older") && !commits.contains("Newer"));
        let (empty, status) = page(&node, REPO, &[("g", "public"), ("r", "empty")], None);
        assert!(status == OK && empty.contains("Nothing has been pushed here yet"), "{empty}");
        assert_eq!(page(&node, TREE, &[("g", "public"), ("r", "empty")], None).1, NOT_FOUND);
        assert_eq!(page(&node, COMMIT, &[("g", "public"), ("r", "demo"), ("h", &"0".repeat(40))], None).1, NOT_FOUND);
        assert_eq!(page(&node, COMMIT, &[("g", "public"), ("r", "demo"), ("h", "main")], None).1, NOT_FOUND);
        let (group, _) = page(&node, GROUP, &[("g", "public")], None);
        assert!(group.contains("demo") && group.contains("empty"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn relative_links_stay_in_the_repository() {
        assert_eq!(relative("README.md", "docs/a.md").as_deref(), Some("docs/a.md"));
        assert_eq!(relative("docs/a.md", "../b.md#part").as_deref(), Some("b.md"));
        assert_eq!(relative("docs/a.md", "/c/d.rs?x").as_deref(), Some("c/d.rs"));
        assert_eq!(relative("docs/a.md", "./e.md").as_deref(), Some("docs/e.md"));
        assert_eq!(relative("a.md", "../../etc/passwd"), None);
        assert_eq!(relative("a.md", "#top"), None);
    }
}
