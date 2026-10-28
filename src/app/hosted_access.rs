//! Who may do what with a repository the rngit node hosted here keeps, or with
//! its group: its `.allowed` rules as a table of people and permissions, in
//! the Repos tab's hosted list (Enter, a click, `w` or `g`) and the web UI.
//! The node reads the rules for each request, so a change applies at once.

use crossterm::event::KeyCode;
use ratatui::layout::Position;
use rngit::server::{Perm, Rule, Who};

use super::App;
use super::allowed::MAX_LISTED;
use crate::app::PromptKind;
use crate::app::git_host::HostedPrompt;
use crate::app::repos::RepoPrompt;
use crate::net::{Hash, parse_hash};
use crate::store::Trust;

/// The permissions a table has a column for: the rules' word for it, its
/// name, and what it lets someone do. Creating is a group's only.
pub const PERMS: [(Perm, &str, &str, &str); 8] = [
    (Perm::Read, "r", "Read", "clone and fetch it"),
    (Perm::Write, "w", "Push", "push branches and tags"),
    (Perm::Release, "rel", "Releases", "publish and delete releases"),
    (Perm::Issues, "i", "Issues", "open and comment on work documents"),
    (Perm::Proposals, "p", "Propose", "propose work documents"),
    (Perm::Stats, "s", "Stats", "see how much it's used"),
    (Perm::Admin, "adm", "Admin", "all of these, and change who may"),
    (Perm::Create, "c", "Create", "make repositories in the group"),
];

/// Who a row of the table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessWho {
    Anyone,
    /// `none`: no one at all, whatever the group or the node's settings say.
    NoOne,
    Identity(Hash),
}

impl AccessWho {
    /// As the rules write it: `all`, `none` or an identity's hash.
    pub fn word(&self) -> String {
        match self {
            AccessWho::Anyone => "all".into(),
            AccessWho::NoOne => "none".into(),
            AccessWho::Identity(hash) => hex::encode(hash),
        }
    }

    pub fn parse(text: &str) -> Option<AccessWho> {
        match text.trim().to_ascii_lowercase().as_str() {
            "all" => Some(AccessWho::Anyone),
            "none" => Some(AccessWho::NoOne),
            hash => parse_hash(hash).map(AccessWho::Identity),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccessRow {
    pub who: AccessWho,
    pub name: String,
    /// Their LXMF address, if they're someone rettui knows.
    pub key: Option<String>,
    /// One for each of the table's columns.
    pub ticks: Vec<bool>,
}

/// A repository's or a group's rules, as a table.
#[derive(Debug, Clone, PartialEq)]
pub struct HostedAccess {
    pub group: String,
    /// None for the group's own rules.
    pub repo: Option<String>,
    /// Anyone, no one, then people.
    pub rows: Vec<AccessRow>,
    /// The TUI's cursor: a row, and a column.
    pub at: (usize, usize),
}

impl HostedAccess {
    /// Its columns, as indexes into [`PERMS`].
    pub fn columns(&self) -> usize {
        if self.repo.is_some() { PERMS.len() - 1 } else { PERMS.len() }
    }

    /// Whose rules these are, as the dialog's title says.
    pub fn whose(&self) -> String {
        match &self.repo {
            Some(repo) => format!("{}/{}", self.group, repo.strip_suffix(".git").unwrap_or(repo)),
            None => format!("the group {}", self.group),
        }
    }

    /// What a column with nothing ticked comes to: what is above these rules.
    pub fn fallback(&self, node_access: &str) -> String {
        let node = if node_access.trim().is_empty() { "no one".to_string() } else { node_access.trim().to_string() };
        match self.repo {
            Some(_) => format!("A permission with nothing ticked is the group's, then Settings › Hosting › rngit access ({node})."),
            None => format!("A permission with nothing ticked is Settings › Hosting › rngit access ({node})."),
        }
    }

    /// Tick or untick a cell. Anyone and no one stand for the whole column,
    /// so ticking one of them clears the rest, and ticking someone clears them.
    pub fn toggle(&mut self, row: usize, column: usize) {
        if column >= self.columns() || row >= self.rows.len() {
            return;
        }
        let on = !self.rows[row].ticks[column];
        if on {
            let whole = !matches!(self.rows[row].who, AccessWho::Identity(_));
            for other in &mut self.rows {
                if whole || !matches!(other.who, AccessWho::Identity(_)) {
                    other.ticks[column] = false;
                }
            }
        }
        self.rows[row].ticks[column] = on;
    }

    /// Add someone, by identity, to tick permissions for.
    pub fn add(&mut self, identity: Hash, name: String, key: Option<String>) -> Result<usize, String> {
        if let Some(at) = self.rows.iter().position(|r| r.who == AccessWho::Identity(identity)) {
            return Ok(at);
        }
        if self.rows.len() >= MAX_LISTED + 2 {
            return Err(format!("A table has at most {MAX_LISTED} people"));
        }
        self.rows.push(AccessRow { who: AccessWho::Identity(identity), name, key, ticks: vec![false; PERMS.len()] });
        Ok(self.rows.len() - 1)
    }

    /// The rules the table comes to, a line for each tick.
    pub fn rules(&self) -> String {
        let mut lines = Vec::new();
        for (column, (_, word, ..)) in PERMS.iter().enumerate().take(self.columns()) {
            for row in self.rows.iter().filter(|r| r.ticks[column]) {
                lines.push(format!("{word}:{}", row.who.word()));
            }
        }
        lines.join("\n")
    }
}

/// The column a rule's permission has, if the table shows it.
fn column_of(perm: Perm) -> Option<usize> {
    PERMS.iter().position(|(p, ..)| *p == perm)
}

impl App {
    /// The rules of a hosted repository (or with no `repo`, of its group) as a
    /// table: anyone, no one, your contacts and those you've talked with
    /// (whose keys are known), and anyone else the rules name.
    pub fn hosted_access(&self, group: &str, repo: Option<&str>) -> Result<HostedAccess, String> {
        let rules = Rule::parse(&self.hosted_rules(group, repo)?);
        let blank = || vec![false; PERMS.len()];
        let mut access = HostedAccess {
            group: group.to_string(),
            repo: repo.map(str::to_string),
            rows: vec![
                AccessRow { who: AccessWho::Anyone, name: "Anyone".into(), key: None, ticks: blank() },
                AccessRow { who: AccessWho::NoOne, name: "No one".into(), key: None, ticks: blank() },
            ],
            at: (0, 0),
        };
        let mut keys: Vec<&String> = self.store.contacts.keys().chain(self.store.conversations.keys()).collect();
        keys.sort();
        keys.dedup();
        let mut people: Vec<AccessRow> = keys
            .into_iter()
            .filter(|key| self.store.contact(key).trust != Trust::Blocked)
            .filter_map(|key| {
                let identity = self.identity_of(key)?;
                Some(AccessRow {
                    who: AccessWho::Identity(identity),
                    name: self.store.display_name(key),
                    key: Some(key.clone()),
                    ticks: blank(),
                })
            })
            .collect();
        people.sort_by_key(|p| p.name.to_lowercase());
        people.dedup_by_key(|p| p.who);
        access.rows.extend(people);
        for rule in rules {
            let who = match rule.who {
                Who::Everyone => AccessWho::Anyone,
                Who::Nobody => AccessWho::NoOne,
                Who::Identity(id) => AccessWho::Identity(id),
            };
            let row = match access.rows.iter().position(|r| r.who == who) {
                Some(row) => row,
                None => match who {
                    AccessWho::Identity(id) => access.add(id, format!("<{}>", &hex::encode(id)[..12]), None)?,
                    _ => continue,
                },
            };
            let columns = access.columns();
            for column in rule.perms.into_iter().filter_map(column_of).filter(|c| *c < columns) {
                access.rows[row].ticks[column] = true;
            }
        }
        // Those named first, so they're seen.
        let named = |r: &AccessRow| r.ticks.iter().any(|t| *t);
        access.rows[2..].sort_by_key(|r| (!named(r), r.name.to_lowercase()));
        Ok(access)
    }

    /// Save a table as the rules, and list what's hosted again (whether
    /// anyone may clone each).
    pub fn set_hosted_access(&mut self, access: &HostedAccess) -> Result<(), String> {
        self.set_hosted_rules(&access.group, access.repo.as_deref(), &access.rules())?;
        self.list_hosted();
        Ok(())
    }

    /// Someone to add to a table, by LXMF address (once their key is known)
    /// or identity hash.
    pub fn access_person(&self, text: &str) -> Result<(Hash, String, Option<String>), String> {
        let text = text.trim().trim_start_matches('<').trim_end_matches('>');
        if let Some(identity) = self.identity_of(text) {
            return Ok((identity, self.store.display_name(text), Some(text.to_string())));
        }
        let identity = parse_hash(text).ok_or("That isn't an LXMF address or an identity hash (32 hex digits)")?;
        Ok((identity, format!("<{}>", &text[..12.min(text.len())]), None))
    }

    // ---- TUI ---------------------------------------------------------------

    /// Open the table for the picked hosted repository, or its group.
    pub(super) fn open_hosted_access(&mut self, group: bool) {
        let Some(hosted) = self.picked_hosted() else { return };
        let (name, repo) = (hosted.group.clone(), hosted.name.clone());
        match self.hosted_access(&name, (!group).then_some(repo.as_str())) {
            Ok(access) => self.git_host.access = Some(access),
            Err(e) => self.warn(e),
        }
    }

    pub(super) fn hosted_access_key(&mut self, code: KeyCode) {
        let Some(access) = self.git_host.access.as_mut() else { return };
        let (row, column) = access.at;
        let (rows, columns) = (access.rows.len(), access.columns());
        match code {
            KeyCode::Esc => self.git_host.access = None,
            KeyCode::Down | KeyCode::Char('j') => access.at.0 = (row + 1).min(rows - 1),
            KeyCode::Up | KeyCode::Char('k') => access.at.0 = row.saturating_sub(1),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => access.at.1 = (column + 1).min(columns - 1),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => access.at.1 = column.saturating_sub(1),
            KeyCode::Home => access.at.0 = 0,
            KeyCode::End => access.at.0 = rows - 1,
            KeyCode::Char(' ') => access.toggle(row, column),
            KeyCode::Char('a') => self.open_prompt(
                PromptKind::Repo(RepoPrompt::Hosted(HostedPrompt::AddPerson)),
                "Add who? Their LXMF address (once their key is known) or identity hash",
                "",
            ),
            KeyCode::Enter => {
                let Some(access) = self.git_host.access.take() else { return };
                if let Err(e) = self.set_hosted_access(&access) {
                    self.git_host.access = Some(access);
                    self.fail(e);
                }
            }
            _ => {}
        }
    }

    /// Add someone typed into the prompt to the open table.
    pub(super) fn add_access_person(&mut self, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let (identity, name, key) = self.access_person(text)?;
        let access = self.git_host.access.as_mut().ok_or("The table was closed")?;
        let row = access.add(identity, name, key)?;
        access.at.0 = row;
        Ok(())
    }

    /// A click on the table ticks the cell; outside it, closes it.
    pub(super) fn click_hosted_access(&mut self, at: Position) {
        let area = self.regions.hosted_access;
        let Some(access) = self.git_host.access.as_mut() else { return };
        if !area.contains(at) {
            self.git_host.access = None;
            return;
        }
        let hit = self.regions.hosted_access_cells.iter().find(|(rect, ..)| rect.contains(at)).map(|(_, row, column)| (*row, *column));
        if let Some((row, column)) = hit {
            access.at = (row, column);
            access.toggle(row, column);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(repo: bool) -> HostedAccess {
        let row = |who, name: &str| AccessRow { who, name: name.into(), key: None, ticks: vec![false; PERMS.len()] };
        HostedAccess {
            group: "public".into(),
            repo: repo.then(|| "demo.git".into()),
            rows: vec![row(AccessWho::Anyone, "Anyone"), row(AccessWho::NoOne, "No one"), row(AccessWho::Identity([7; 16]), "Ben")],
            at: (0, 0),
        }
    }

    #[test]
    fn anyone_and_no_one_stand_for_the_whole_column() {
        let mut t = table(true);
        t.toggle(2, 0);
        t.toggle(0, 0);
        assert_eq!((t.rows[0].ticks[0], t.rows[2].ticks[0]), (true, false));
        t.toggle(2, 0);
        assert_eq!((t.rows[0].ticks[0], t.rows[2].ticks[0]), (false, true));
        t.toggle(1, 1);
        t.toggle(2, 6);
        assert_eq!(t.rules(), format!("r:{0}\nw:none\nadm:{0}", hex::encode([7; 16])));
        // A repository has no Create column; a group does.
        t.toggle(0, 7);
        assert!(!t.rules().contains("c:"));
        let mut g = table(false);
        g.toggle(0, 7);
        assert_eq!(g.rules(), "c:all");
    }

    #[test]
    fn someone_is_added_once() {
        let mut t = table(true);
        assert_eq!(t.add([7; 16], "Ben again".into(), None), Ok(2));
        assert_eq!(t.add([8; 16], "Cy".into(), None), Ok(3));
        assert_eq!(AccessWho::parse(" ALL "), Some(AccessWho::Anyone));
        assert_eq!(AccessWho::parse(&hex::encode([8; 16])), Some(AccessWho::Identity([8; 16])));
        assert_eq!(AccessWho::parse("ben"), None);
        assert!(t.fallback("").contains("the group's") && t.fallback("").contains("(no one)"));
        assert!(table(false).fallback("r:all").contains("(r:all)"));
    }
}
