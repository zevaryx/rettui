//! The rngit node hosted here, as the app knows it.

use std::collections::{HashMap, VecDeque};

use crossterm::event::KeyCode;
use rngit::server::{Follows, Hosted, Node};

use crate::app::App;
use crate::app::PromptKind;
use crate::app::node::NodeStatus;
use crate::app::repos::{Load, LoadingState, RepoPrompt, ReposFocus};
use crate::config::Paths;
use crate::net::NetCommand;
use crate::net::githost::{self, GitHostConfig, GitHostEvent, HostAction, HostReply};

/// How many answered requests are kept to show.
const KEPT: usize = 50;

/// One request the node answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// What it was for: `fetch`, `list`, `push`...
    pub what: String,
    /// Who asked, as the first eight hex digits of their identity; none if they didn't identify.
    pub peer: Option<String>,
    pub ok: bool,
}

pub struct GitHost {
    pub status: NodeStatus,
    /// The address in clone URLs, once it has an identity (after it first starts).
    pub address: Option<[u8; 16]>,
    /// What the network actor was last asked to run.
    config: Option<GitHostConfig>,
    /// Requests answered since it started, and how many were refused.
    pub requests: u64,
    pub refused: u64,
    /// The newest requests, newest last.
    pub recent: VecDeque<Served>,
    /// The repositories it hosts, as last listed.
    pub hosted: Load<Vec<Hosted>>,
    /// What the owner asked that hasn't come back.
    acting: HashMap<u64, HostAction>,
}

/// What the TUI's prompts about the repositories hosted here are for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostedPrompt {
    New,
    /// The source of a fork or mirror, then its name here.
    Source(Follows),
    Name {
        source: String,
        kind: Follows,
    },
    Rename {
        group: String,
        repo: String,
    },
    ConfirmDelete {
        group: String,
        repo: String,
    },
    /// The rules of a repository, or with no `repo`, of its group.
    Rules {
        group: String,
        repo: Option<String>,
    },
}

/// `group/name` (`.git` added if it isn't there) as a group and a
/// repository's folder name, if both are plain names.
pub fn parse_hosted(text: &str) -> Result<(String, String), String> {
    let text = text.trim().trim_matches('/');
    let (group, name) = text.split_once('/').ok_or("Name it group/name, such as public/demo")?;
    let name = if name.ends_with(".git") { name.to_string() } else { format!("{name}.git") };
    match rngit::server::split_repo_path(&format!("{group}/{name}")) {
        Some(_) => Ok((group.to_string(), name)),
        None => Err(format!("{group}/{name} isn't a group and a name made of letters, digits and -_.")),
    }
}

/// The lines of a rules file as one line, for a prompt: comments dropped.
fn rules_on_one_line(text: &str) -> String {
    text.split(['\n', ',']).map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect::<Vec<_>>().join(", ")
}

/// A name for a fork or mirror of `source`: its own last two parts, as
/// `group/name`, so `https://host/owner/repo.git` is `owner/repo`.
pub fn suggested_name(source: &str) -> String {
    let path = source.split_once("://").map_or(source, |(_, rest)| rest);
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').skip(1).filter(|p| !p.is_empty()).collect();
    let plain = |part: &str| -> String {
        part.trim_end_matches(".git")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
            .collect()
    };
    match parts.as_slice() {
        [.., group, name] => format!("{}/{}", plain(group).trim_start_matches('.'), plain(name).trim_start_matches('.')),
        [name] => format!("mirrors/{}", plain(name).trim_start_matches('.')),
        [] => "mirrors/".to_string(),
    }
}

impl GitHost {
    pub fn new(paths: &Paths, config: Option<GitHostConfig>) -> Self {
        Self {
            status: if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off },
            address: githost::address(&paths.git_host_identity),
            config,
            requests: 0,
            refused: 0,
            recent: VecDeque::new(),
            hosted: Load::Idle,
            acting: HashMap::new(),
        }
    }

    /// What the owner asked that is still going, as said while it goes.
    pub fn acting(&self) -> Vec<String> {
        let mut going: Vec<String> = self.acting.values().filter(|a| **a != HostAction::List).map(HostAction::what).collect();
        going.sort();
        going
    }

    /// The node's folder, as a node with no network: for its rules, which are
    /// read and written there and then.
    fn folder(&self) -> Option<Node> {
        self.config.as_ref().map(|config| Node::new(config.dir.clone()))
    }

    /// After a Reticulum restart, it starts again (with the settings).
    pub(super) fn restarting(&mut self, config: Option<GitHostConfig>) {
        self.status = if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off };
        self.config = config;
    }

    /// The URL to clone a repository from here, `rns://<address>/`.
    pub fn url(&self) -> Option<String> {
        self.address.map(|hash| format!("rns://{}", hex::encode(hash)))
    }

    /// How much it has done since it started.
    pub fn summary(&self) -> String {
        let mut text = format!("{} request{}", self.requests, if self.requests == 1 { "" } else { "s" });
        if self.refused > 0 {
            text.push_str(&format!(" · {} refused", self.refused));
        }
        text
    }
}

impl App {
    /// Ask the node to do something for its owner. It has to be running.
    pub fn host_act(&mut self, action: HostAction) -> Result<u64, String> {
        if self.git_host.status != NodeStatus::Running {
            return Err("Host rngit repositories first (Settings › Hosting)".into());
        }
        if action == HostAction::List {
            self.git_host.hosted.loading();
        }
        let id = self.request_id();
        self.git_host.acting.insert(id, action.clone());
        self.send(NetCommand::GitHostAction { id, action });
        Ok(id)
    }

    /// List what the node hosts again.
    pub fn list_hosted(&mut self) {
        if self.git_host.status == NodeStatus::Running {
            let _ = self.host_act(HostAction::List);
        }
    }

    /// The rules of a hosted repository (`repo` given) or of a group, as
    /// written in its `.allowed` file.
    pub fn hosted_rules(&self, group: &str, repo: Option<&str>) -> Result<String, String> {
        self.git_host.folder().ok_or("rngit hosting is off")?.rules(group, repo)
    }

    /// Replace them: rules one on each line, or separated by commas. Empty
    /// removes them.
    pub fn set_hosted_rules(&mut self, group: &str, repo: Option<&str>, text: &str) -> Result<(), String> {
        let lines: Vec<&str> = text.split(['\n', ',']).map(str::trim).filter(|l| !l.is_empty()).collect();
        self.git_host.folder().ok_or("rngit hosting is off")?.set_rules(group, repo, &lines.join("\n"))?;
        let whose = repo.map_or_else(|| format!("the group {group}"), |repo| format!("{group}/{repo}"));
        self.notify(format!("Saved who may use {whose}"));
        Ok(())
    }

    /// The hosted repository picked in the Repos tab.
    pub fn picked_hosted(&self) -> Option<&Hosted> {
        self.git_host.hosted.ready()?.get(self.repos.hosted.selected().unwrap_or(0))
    }

    /// Where to clone a hosted repository from.
    pub fn hosted_url(&self, hosted: &Hosted) -> Option<String> {
        Some(format!("{}/{}/{}", self.git_host.url()?, hosted.group, hosted.name.strip_suffix(".git").unwrap_or(&hosted.name)))
    }

    pub(crate) fn move_hosted(&mut self, delta: isize) {
        let len = self.git_host.hosted.ready().map_or(0, Vec::len);
        if len == 0 {
            return;
        }
        let at = self.repos.hosted.selected().unwrap_or(0).min(len - 1);
        self.repos.hosted.select(Some(at.saturating_add_signed(delta).min(len - 1)));
    }

    /// Keys of the hosted repositories' list, in the Repos tab.
    pub(super) fn hosted_key(&mut self, code: KeyCode) {
        let picked = self.picked_hosted().map(|h| (h.group.clone(), h.name.clone(), h.following.is_some()));
        let prompt = |prompt| PromptKind::Repo(RepoPrompt::Hosted(prompt));
        match code {
            KeyCode::Esc | KeyCode::BackTab | KeyCode::Tab => {
                self.repos.focus = ReposFocus::List;
                return;
            }
            KeyCode::Down | KeyCode::Char('j') => return self.move_hosted(1),
            KeyCode::Up | KeyCode::Char('k') => return self.move_hosted(-1),
            KeyCode::PageDown => return self.move_hosted(10),
            KeyCode::PageUp => return self.move_hosted(-10),
            KeyCode::Char('R') => return self.list_hosted(),
            _ => {}
        }
        let result = match (code, picked) {
            (KeyCode::Char('n'), _) => {
                self.open_prompt(prompt(HostedPrompt::New), "New empty repository: group/name (the group is made if it isn't there)", "");
                Ok(())
            }
            (KeyCode::Char(c @ ('m' | 'f')), _) => {
                let (kind, title) = if c == 'm' {
                    (Follows::Mirror, "Mirror which repository? rns://node/group/repo or https://... (followed again once a day)")
                } else {
                    (Follows::Fork, "Fork which repository? rns://node/group/repo or https://... (synced only when you press s)")
                };
                self.open_prompt(prompt(HostedPrompt::Source(kind)), title, "");
                Ok(())
            }
            (_, None) => Ok(()),
            (KeyCode::Char('s'), Some((group, repo, true))) => self.host_act(HostAction::Sync { group, repo }).map(drop),
            (KeyCode::Char('s'), Some(_)) => Err("Only a fork or a mirror syncs: this one follows nothing".into()),
            (KeyCode::Char('r'), Some((group, repo, _))) => {
                let start = format!("{group}/{}", repo.strip_suffix(".git").unwrap_or(&repo));
                self.open_prompt(prompt(HostedPrompt::Rename { group, repo }), "Rename to group/name (its releases and rules go with it)", &start);
                Ok(())
            }
            (KeyCode::Char('X'), Some((group, repo, _))) => {
                let title = format!("Delete {group}/{repo} from this node, with its releases and work documents? It can't be undone. Type y");
                self.open_prompt(prompt(HostedPrompt::ConfirmDelete { group, repo }), &title, "");
                Ok(())
            }
            (KeyCode::Char('w'), Some((group, repo, _))) => self.hosted_rules(&group, Some(&repo)).map(|rules| {
                let title = format!("Who may use {group}/{repo}: rules like r:all, rw:<identity>, adm:<identity>, separated by commas; empty for the group's");
                self.open_prompt(prompt(HostedPrompt::Rules { group, repo: Some(repo) }), &title, &rules_on_one_line(&rules));
            }),
            (KeyCode::Char('g'), Some((group, ..))) => self.hosted_rules(&group, None).map(|rules| {
                let title = format!("Who may use the group {group}: rules like r:all, c:<identity> (make repositories), separated by commas");
                self.open_prompt(prompt(HostedPrompt::Rules { group, repo: None }), &title, &rules_on_one_line(&rules));
            }),
            (KeyCode::Char('y'), Some(_)) => {
                if let Some(url) = self.picked_hosted().and_then(|h| self.hosted_url(h)) {
                    self.copy(&url, "the repository's URL");
                }
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.warn(e);
        }
    }

    pub(super) fn submit_hosted_prompt(&mut self, prompt: HostedPrompt, text: &str) -> Result<(), String> {
        let yes = text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes");
        match prompt {
            HostedPrompt::New => {
                let (group, repo) = parse_hosted(text)?;
                self.host_act(HostAction::Make { group, repo }).map(drop)
            }
            HostedPrompt::Source(kind) => {
                let source = text.trim().to_string();
                if source.is_empty() {
                    return Ok(());
                }
                let title = "Keep it here as group/name";
                self.open_prompt(
                    PromptKind::Repo(RepoPrompt::Hosted(HostedPrompt::Name { source: source.clone(), kind })),
                    title,
                    &suggested_name(&source),
                );
                Ok(())
            }
            HostedPrompt::Name { source, kind } => {
                let (group, repo) = parse_hosted(text)?;
                self.host_act(HostAction::Follow { group, repo, source, kind }).map(drop)
            }
            HostedPrompt::Rename { group, repo } => {
                let (to_group, to_repo) = parse_hosted(text)?;
                if (&to_group, &to_repo) == (&group, &repo) {
                    return Ok(());
                }
                self.host_act(HostAction::Rename { group, repo, to_group, to_repo }).map(drop)
            }
            HostedPrompt::ConfirmDelete { group, repo } if yes => self.host_act(HostAction::Delete { group, repo }).map(drop),
            HostedPrompt::ConfirmDelete { .. } => Ok(()),
            HostedPrompt::Rules { group, repo } => self.set_hosted_rules(&group, repo.as_deref(), text),
        }
    }

    /// Start, change or stop the node to match the settings.
    pub(super) fn apply_git_host_settings(&mut self) {
        let config = GitHostConfig::from_settings(&self.settings, &self.paths);
        if config == self.git_host.config {
            return;
        }
        self.git_host.status = if config.is_some() { NodeStatus::Starting } else { NodeStatus::Off };
        if config.is_none() {
            self.git_host.requests = 0;
            self.git_host.refused = 0;
            self.git_host.recent.clear();
        }
        self.git_host.config = config.clone();
        self.send(NetCommand::GitHost(config));
    }

    pub(super) fn on_git_host(&mut self, event: GitHostEvent) {
        match event {
            GitHostEvent::Started { hash } => {
                self.git_host.status = NodeStatus::Running;
                self.git_host.address = Some(hash);
                self.log(format!("Hosting rngit repositories at rns://{}", hex::encode(hash)));
                self.list_hosted();
            }
            GitHostEvent::Stopped => {
                self.git_host.status = NodeStatus::Off;
                self.git_host.hosted = Load::Idle;
                self.log("Stopped hosting rngit repositories");
            }
            GitHostEvent::Owned { id, result } => {
                let Some(action) = self.git_host.acting.remove(&id) else { return };
                match (action, result) {
                    (HostAction::List, Ok(HostReply::Listed(list))) => self.git_host.hosted = Load::Ready(list),
                    (HostAction::List, Err(e)) => self.git_host.hosted = Load::Failed(e),
                    (_, Ok(HostReply::Done(said))) => {
                        self.notify(said);
                        self.list_hosted();
                    }
                    (action, Err(e)) => {
                        self.fail(format!("{} failed: {e}", action.what()));
                        self.list_hosted();
                    }
                    (_, Ok(HostReply::Listed(_))) => {}
                }
            }
            GitHostEvent::Failed(e) => {
                self.log(format!("rngit node: {e}"));
                self.git_host.status = NodeStatus::Failed(e);
            }
            // The last of a node that has since stopped: not counted.
            GitHostEvent::Served { .. } if self.git_host.status == NodeStatus::Off => {}
            GitHostEvent::Served { path, peer, status } => {
                self.git_host.requests += 1;
                if status != 0 {
                    self.git_host.refused += 1;
                }
                let what = path.rsplit('/').next().unwrap_or(&path).chars().filter(|c| c.is_ascii_graphic()).take(16).collect();
                let peer = peer.map(|p| hex::encode(&p[..4]));
                if self.git_host.recent.len() >= KEPT {
                    self.git_host.recent.pop_front();
                }
                self.git_host.recent.push_back(Served { what, peer, ok: status == 0 });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::node::NodeStatus;
    use crate::config::Settings;
    use crate::net::NetCommand;
    use crate::net::NetEvent;
    use crate::net::githost::GitHostEvent;
    use crate::store::Store;
    use rngit::server::Follows;

    fn app(name: &str) -> (crate::app::App, tokio::sync::mpsc::UnboundedReceiver<NetCommand>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("rettui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (app, commands) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        (app, commands, dir)
    }

    #[test]
    fn turning_it_on_starts_the_node_and_off_stops_it() {
        let (mut app, mut commands, dir) = app("git-host-on");
        while commands.try_recv().is_ok() {}
        app.update_settings(&[("git_host_enabled", "true"), ("git_host_access", "r:all")]).unwrap();
        assert_eq!(app.git_host.status, NodeStatus::Starting);
        match commands.try_recv() {
            Ok(NetCommand::GitHost(Some(config))) => assert_eq!(config.access, "r:all"),
            other => panic!("{other:?}"),
        }
        app.on_net(NetEvent::GitHost(GitHostEvent::Started { hash: [7; 16] }));
        assert_eq!(app.git_host.status, NodeStatus::Running);
        assert_eq!(app.git_host.url().unwrap(), format!("rns://{}", "07".repeat(16)));
        // Once running, what it hosts is listed.
        assert!(matches!(commands.try_recv(), Ok(NetCommand::GitHostAction { action: HostAction::List, .. })));
        app.update_settings(&[("git_host_enabled", "false")]).unwrap();
        assert_eq!(app.git_host.status, NodeStatus::Off);
        assert!(matches!(commands.try_recv(), Ok(NetCommand::GitHost(None))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn requests_are_counted_and_the_newest_kept() {
        let (mut app, _commands, dir) = app("git-host-served");
        app.update_settings(&[("git_host_enabled", "true")]).unwrap();
        app.on_net(NetEvent::GitHost(GitHostEvent::Started { hash: [7; 16] }));
        for n in 0..60u8 {
            let status = if n % 10 == 0 { 3 } else { 0 };
            app.on_net(NetEvent::GitHost(GitHostEvent::Served { path: "/git/fetch\u{1b}[31m".into(), peer: Some([n; 16]), status }));
        }
        assert_eq!((app.git_host.requests, app.git_host.refused, app.git_host.recent.len()), (60, 6, 50));
        let last = app.git_host.recent.back().unwrap();
        assert_eq!((last.what.as_str(), last.peer.as_deref(), last.ok), ("fetch[31m", Some("3b3b3b3b"), true));
        app.on_net(NetEvent::GitHost(GitHostEvent::Failed("no room".into())));
        assert_eq!(app.git_host.status, NodeStatus::Failed("no room".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    use crate::net::githost::{HostAction, owner_tests};
    use crate::ui::tui_tests::Tui;
    use crossterm::event::KeyCode;

    /// Do what the app asked of the node, as the network task does, until
    /// nothing more is asked (a change lists them again).
    fn run_all(tui: &mut Tui, source: &std::path::Path) -> Vec<HostAction> {
        let mut asked = Vec::new();
        loop {
            let commands = tui.sent();
            let mut any = false;
            for command in commands {
                if let NetCommand::GitHostAction { id, action } = command {
                    any = true;
                    asked.push(action.clone());
                    let result = owner_tests::run(&tui.app.paths.git_host, source, action);
                    tui.app.on_net(NetEvent::GitHost(GitHostEvent::Owned { id, result }));
                }
            }
            if !any {
                break;
            }
        }
        tui.draw();
        asked
    }

    fn hosting(name: &str) -> (Tui, std::path::PathBuf) {
        let mut tui = Tui::with(name, |s| s.git_host_enabled = true);
        let source = owner_tests::source(&tui.dir, "public", "demo");
        tui.app.on_net(NetEvent::GitHost(GitHostEvent::Started { hash: [7; 16] }));
        assert_eq!(run_all(&mut tui, &source), [HostAction::List]);
        tui.tab(crate::app::Tab::Repos);
        (tui, source)
    }

    #[test]
    fn the_owner_mirrors_forks_and_manages_what_is_hosted_from_the_repos_tab() {
        let (mut tui, source) = hosting("git-host-manage");
        let screen = tui.key(KeyCode::Char('H')).draw();
        assert!(screen.contains("Hosted here · rns://0707") && screen.contains("Nothing hosted yet"), "{screen}");
        run_all(&mut tui, &source);

        // A mirror, named after its source unless told otherwise.
        let from = format!("rns://{}/public/demo", "ab".repeat(16));
        tui.key(KeyCode::Char('m')).answer(&from);
        assert_eq!(tui.app.prompt.as_ref().unwrap().input.text(), "public/demo");
        tui.key(KeyCode::Enter);
        let asked = run_all(&mut tui, &source);
        assert!(
            matches!(&asked[0], HostAction::Follow { group, repo, kind: Follows::Mirror, .. } if group == "public" && repo == "demo.git")
        );
        let screen = tui.draw();
        assert!(screen.contains("public/demo") && screen.contains("⇄ mirror of rns://abab"), "{screen}");
        assert!(tui.said().contains(&format!("Mirrored {from} into public/demo.git")), "{}", tui.said());

        // A fork, under another name; then an empty repository.
        tui.key(KeyCode::Char('f')).answer(&from);
        tui.app.prompt.as_mut().unwrap().input = crate::app::TextInput::with_text("forks/demo");
        tui.key(KeyCode::Enter);
        tui.key(KeyCode::Char('n')).answer("team/empty");
        run_all(&mut tui, &source);
        let names: Vec<_> = tui.app.git_host.hosted.ready().unwrap().iter().map(|h| format!("{}/{}", h.group, h.name)).collect();
        assert_eq!(names, ["forks/demo.git", "public/demo.git", "team/empty.git"]);
        assert!(tui.draw().contains("⑂ fork of"));

        // Syncing what follows a source, and not what doesn't.
        tui.key(KeyCode::Char('s'));
        assert!(matches!(run_all(&mut tui, &source)[0], HostAction::Sync { .. }));
        assert!(tui.said().contains("Synced forks/demo.git"), "{}", tui.said());
        tui.key(KeyCode::Down).key(KeyCode::Down).key(KeyCode::Char('s'));
        assert!(tui.said().contains("Only a fork or a mirror syncs"), "{}", tui.said());

        // Who may use it, and the group.
        tui.key(KeyCode::Char('w')).answer("r:all, rw:0123456789abcdef0123456789abcdef");
        let allowed = tui.app.paths.git_host.join("team/empty.git.allowed");
        assert_eq!(std::fs::read_to_string(&allowed).unwrap(), "r:all\nrw:0123456789abcdef0123456789abcdef");
        tui.key(KeyCode::Char('w'));
        assert_eq!(tui.app.prompt.as_ref().unwrap().input.text(), "r:all, rw:0123456789abcdef0123456789abcdef");
        tui.settle();
        tui.key(KeyCode::Char('w')).answer(", w:someone");
        assert!(tui.said().contains("Invalid permission"), "{}", tui.said());
        tui.key(KeyCode::Char('g')).answer("c:all");
        assert_eq!(std::fs::read_to_string(tui.app.paths.git_host.join("team.allowed")).unwrap(), "c:all");

        // Renamed, with its rules; then deleted, with them.
        tui.key(KeyCode::Char('r')).answer("2");
        run_all(&mut tui, &source);
        assert!(tui.app.paths.git_host.join("team/empty2.git.allowed").is_file() && !allowed.exists());
        tui.key(KeyCode::Char('X')).answer("n");
        assert!(tui.sent().is_empty());
        tui.key(KeyCode::Char('X')).answer("y");
        run_all(&mut tui, &source);
        assert!(!tui.app.paths.git_host.join("team/empty2.git").exists());
        assert_eq!(tui.app.git_host.hosted.ready().unwrap().len(), 2);

        // What can't be done is said.
        tui.key(KeyCode::Char('m')).answer("ftp://example.com/x.git").key(KeyCode::Enter);
        run_all(&mut tui, &source);
        assert!(tui.said().contains("Mirroring into mirrors/x.git failed: Prohibited source URL"), "{}", tui.said());
        tui.key(KeyCode::Char('n')).answer("no-group");
        assert!(tui.said().contains("Name it group/name"), "{}", tui.said());
        tui.key(KeyCode::Char('y'));
        assert!(tui.said().contains("URL"), "{}", tui.said());
        tui.key(KeyCode::Char('H'));
        assert_eq!(tui.app.repos.focus, crate::app::repos::ReposFocus::List);
    }

    #[test]
    fn nothing_is_asked_of_a_node_that_is_not_running() {
        let mut tui = Tui::new("git-host-off");
        tui.tab(crate::app::Tab::Repos).key(KeyCode::Char('H'));
        assert!(tui.draw().contains("isn't hosting rngit repositories"));
        tui.key(KeyCode::Char('n')).answer("team/x");
        assert!(tui.said().contains("Host rngit repositories first"), "{}", tui.said());
        assert!(!tui.sent().iter().any(|c| matches!(c, NetCommand::GitHostAction { .. })));
        assert!(tui.app.hosted_rules("team", None).is_err());
    }

    #[test]
    fn names_for_what_is_hosted() {
        use super::{parse_hosted, suggested_name};
        assert_eq!(parse_hosted(" /team/demo/ ").unwrap(), ("team".to_string(), "demo.git".to_string()));
        assert_eq!(parse_hosted("team/demo.git").unwrap().1, "demo.git");
        for bad in ["demo", "../x", "team/../x", ".hidden/x", "a/b/c", "team/"] {
            assert!(parse_hosted(bad).is_err(), "{bad}");
        }
        assert_eq!(suggested_name("https://github.com/zevaryx/rettui.git"), "zevaryx/rettui");
        assert_eq!(suggested_name(&format!("rns://{}/public/demo", "ab".repeat(16))), "public/demo");
        assert_eq!(suggested_name("https://example.com/one"), "mirrors/one");
        assert_eq!(suggested_name("https://example.com/a b/.x y"), "a-b/x-y");
        assert_eq!(suggested_name("nonsense"), "mirrors/");
    }

    #[test]
    fn the_web_ui_cannot_choose_the_folder_or_who_may_read() {
        let (mut app, _commands, dir) = app("git-host-web");
        assert!(app.update_settings_from_web(&[("git_host_dir", "/tmp")]).is_err());
        assert!(app.update_settings_from_web(&[("git_host_access", "r:all")]).is_err());
        app.update_settings_from_web(&[("git_host_enabled", "true")]).unwrap();
        assert!(app.git_host.status == NodeStatus::Starting);
        assert!(app.update_settings(&[("git_host_access", "nonsense")]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
