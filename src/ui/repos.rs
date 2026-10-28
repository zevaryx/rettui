//! Repos tab: rngit repositories. Those kept, and the rngit nodes heard, on
//! the left; the one open on the right, with its branches and tags,
//! releases, work documents and permissions, or one of them opened.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};
use rngit::client::WorkScope;
use unicode_width::UnicodeWidthStr;

use super::{accent, block, dim, human_bytes, picked_style, selected_bg};
use crate::app::App;
use crate::app::hosted_access::{AccessWho, PERMS};
use crate::app::node::NodeStatus;
use crate::app::repo_files::{Browse, BrowseView};
use crate::app::repos::{Detail, Load, OpenRepo, RepoPane, ReposFocus};
use rngit::local::EntryKind;
use rngit::server::Follows;

pub(super) fn draw_repos(frame: &mut Frame, app: &mut App, area: Rect) {
    let [left, right] = Layout::horizontal([Constraint::Length(super::side_width(app, area, 36)), Constraint::Min(20)]).areas(area);
    draw_side(frame, app, left);
    if app.repos.focus == ReposFocus::Hosted {
        draw_hosted(frame, app, right);
        draw_hosted_access(frame, app, area);
    } else {
        draw_repo(frame, app, right);
    }
}

/// The repositories the rngit node hosted here has, to manage.
fn draw_hosted(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = match app.git_host.url() {
        Some(url) if app.git_host.status == NodeStatus::Running => format!("Hosted here · {url}"),
        _ => "Hosted here".to_string(),
    };
    let hosted_block = block(&title, true);
    let inner = hosted_block.inner(area);
    frame.render_widget(hosted_block, area);
    app.regions.repo_panes.clear();
    let [about, _, body] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    app.regions.repo_list = body;
    let mut spans = Vec::new();
    for what in app.git_host.acting() {
        spans.push(Span::styled(format!("◌ {what}…  "), Style::default().fg(Color::Yellow)));
    }
    if spans.is_empty() {
        spans.push(Span::styled("m mirrors and f forks a repository here, n makes an empty one; H goes back", Style::default().fg(dim())));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), about);
    let hint = |text: String| Paragraph::new(text).style(Style::default().fg(dim())).wrap(Wrap { trim: true });
    if app.git_host.status != NodeStatus::Running {
        let text = match &app.git_host.status {
            NodeStatus::Failed(e) => format!("The rngit node failed to start: {e}"),
            NodeStatus::Starting => "The rngit node is starting…".to_string(),
            _ => "This computer isn't hosting rngit repositories. Turn on \"Host rngit repositories\" in Settings › Hosting to mirror, fork and keep repositories here.".to_string(),
        };
        return frame.render_widget(hint(text), body);
    }
    let list = match &app.git_host.hosted {
        Load::Ready(list) => list,
        Load::Failed(e) => return frame.render_widget(hint(format!("Couldn't list them: {e}")), body),
        _ => return frame.render_widget(hint("Listing…".into()), body),
    };
    if list.is_empty() {
        return frame.render_widget(
            hint(
                "Nothing hosted yet. m mirrors a repository from rns:// or https://, f forks one, n makes an empty one to push to.".into(),
            ),
            body,
        );
    }
    let items: Vec<ListItem> = list
        .iter()
        .map(|hosted| {
            let name = format!("{}/{}", hosted.group, hosted.name.strip_suffix(".git").unwrap_or(&hosted.name));
            let mut spans = vec![Span::raw(name), Span::styled(format!("  {}", human_bytes(hosted.bytes)), Style::default().fg(dim()))];
            if let Some(following) = &hosted.following {
                let (mark, how) = if following.kind == Follows::Mirror { ("⇄", "mirror of") } else { ("⑂", "fork of") };
                let synced = match following.synced {
                    0 => "never synced".to_string(),
                    at => format!("synced {} ago", super::ago(at as i64)),
                };
                spans.push(Span::styled(format!("  {mark} {how} {} · {synced}", following.source), Style::default().fg(dim())));
            }
            // A repository others can't read looks to them as if it isn't there.
            spans.push(match hosted.open {
                true => Span::styled("  anyone can clone", Style::default().fg(dim())),
                false => Span::styled("  only those its rules name can clone it (w)", Style::default().fg(Color::Yellow)),
            });
            ListItem::new(Line::from(spans))
        })
        .collect();
    if app.repos.hosted.selected().is_none_or(|at| at >= items.len()) {
        app.repos.hosted.select(Some(0));
    }
    let list = List::new(items).highlight_style(Style::default().bg(selected_bg()).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, body, &mut app.repos.hosted);
}

/// Who may use a hosted repository or group: a row for each person, a column
/// for each permission.
fn draw_hosted_access(frame: &mut Frame, app: &mut App, area: Rect) {
    let node_access = app.settings.git_host_access.clone().unwrap_or_default();
    let Some(access) = &mut app.git_host.access else { return };
    let columns = access.columns();
    let labels: Vec<&str> = PERMS.iter().take(columns).map(|(_, _, name, _)| *name).collect();
    let cell = labels.iter().map(|l| l.len()).max().unwrap_or(4) as u16 + 2;
    let name_width = 22u16;
    let width = (name_width + cell * columns as u16 + 4).max(60).min(area.width);
    let height = (access.rows.len() as u16 + 7).min(area.height.saturating_sub(2)).max(8);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 3,
        width,
        height: height.min(area.height),
    };
    frame.render_widget(Clear, rect);
    let hint = " Space tick · a add someone · Enter save · Esc cancel ";
    let title = format!("Who may use {}", access.whose());
    let popup = block(&title, true).title_bottom(Line::styled(hint, Style::default().fg(dim())));
    let inner = popup.inner(rect);
    frame.render_widget(popup, rect);
    app.regions.hosted_access = inner;
    app.regions.hosted_access_cells.clear();
    let [head, body, _, about, fallback] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(1), Constraint::Length(2)])
            .areas(inner);
    let mut spans = vec![Span::raw(" ".repeat(name_width as usize))];
    for (column, label) in labels.iter().enumerate() {
        let style = if column == access.at.1 { Style::default().fg(accent()).bold() } else { Style::default().fg(dim()) };
        spans.push(Span::styled(format!("{label:^w$}", w = cell as usize), style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), head);
    // Scrolled so the cursor's row shows.
    let shown = body.height as usize;
    let first = access.at.0.saturating_sub(shown.saturating_sub(1));
    for (i, row) in access.rows.iter().enumerate().skip(first).take(shown) {
        let y = body.y + (i - first) as u16;
        let mut name = super::clipped(&row.name, name_width as usize - 2);
        if row.key.is_none() && matches!(row.who, AccessWho::Identity(_)) {
            name = super::clipped(&format!("{} ?", row.name), name_width as usize - 2);
        }
        let name_style = if i < 2 { Style::default().bold() } else { Style::default() };
        let mut spans = vec![Span::styled(format!(" {name:<w$}", w = name_width as usize - 1), name_style)];
        for column in 0..columns {
            let mark = if row.ticks[column] { "[x]" } else { "[ ]" };
            let style = if (i, column) == access.at { picked_style() } else { Style::default() };
            let x = body.x + name_width + cell * column as u16;
            app.regions.hosted_access_cells.push((Rect { x, y, width: cell, height: 1 }.intersection(body), i, column));
            spans.push(Span::styled(format!("{mark:^w$}", w = cell as usize), style));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), Rect { y, height: 1, ..body });
    }
    let (_, word, name, what) = PERMS[access.at.1.min(columns - 1)];
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(format!(" {name} ({word}): "), Style::default().bold()), Span::raw(what)])),
        about,
    );
    let mut note = access.fallback(&node_access);
    if access.rows.iter().any(|r| r.key.is_none() && matches!(r.who, AccessWho::Identity(_))) {
        note.push_str(" ? is someone known only by identity.");
    }
    let fallback = Rect { x: fallback.x + 1, width: fallback.width.saturating_sub(2), ..fallback };
    frame.render_widget(Paragraph::new(note).style(Style::default().fg(dim())).wrap(Wrap { trim: true }), fallback);
}

/// A short form of a hash, for lists.
fn short(hash: &str) -> &str {
    &hash[..hash.len().min(8)]
}

fn draw_side(frame: &mut Frame, app: &mut App, area: Rect) {
    let side_block = block("Repos", app.repos.focus == ReposFocus::List);
    let inner = side_block.inner(area);
    frame.render_widget(side_block, area);
    app.regions.repos_side = inner;
    let open = app.repos.open.as_ref().map(|o| o.url.to_string());
    let mut items: Vec<ListItem> = app
        .store
        .repos
        .iter()
        .map(|repo| {
            let (node, name) = match crate::app::repos::parse_repo(&repo.url) {
                Ok(url) => (hex::encode(url.destination), format!("{}/{}", url.group, url.repo)),
                Err(_) => (String::new(), repo.url.clone()),
            };
            let style = if open.as_deref() == Some(repo.url.as_str()) { Style::default().fg(accent()).bold() } else { Style::default() };
            let node_name = app.store.peers.get(&node).and_then(|p| p.name.clone()).unwrap_or_else(|| short(&node).to_string());
            let mut spans = vec![Span::styled(name, style), Span::styled(format!("  {node_name}"), Style::default().fg(dim()))];
            if repo.mirror.is_some() {
                spans.push(Span::styled(" ⇩", Style::default().fg(Color::Green)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    items.extend(app.repos.nodes.iter().map(|node| {
        let hash = hex::encode(node.hash);
        let name = node.name.clone().unwrap_or_else(|| format!("node {}", short(&hash)));
        let hops = if node.hops == 1 { "1 hop".to_string() } else { format!("{} hops", node.hops) };
        ListItem::new(Line::from(vec![
            Span::styled("⌂ ", Style::default().fg(Color::LightRed)),
            Span::raw(name),
            Span::styled(format!("  {hops}, {} ago", super::ago(node.at as i64)), Style::default().fg(dim())),
        ]))
    }));
    if items.is_empty() {
        let hint = "No repositories yet. n adds one by its rns:// URL (rns://node/group/repo); a link to one in a message or page opens it. rngit nodes show here when they announce.";
        frame.render_widget(Paragraph::new(hint).style(Style::default().fg(dim())).wrap(Wrap { trim: true }), inner);
        return;
    }
    if app.repos.side.selected().is_none() {
        app.repos.side.select(Some(0));
    }
    let list = List::new(items).highlight_style(Style::default().bg(selected_bg()).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, inner, &mut app.repos.side);
}

fn draw_repo(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(open) = app.repos.open.clone() else {
        let repo_block = block("Repository", false);
        let hint = "Pick a repository on the left and press Enter, or n to add one. A node doesn't list its repositories: Enter on one opens it in the Browser, where its owner may say which it has. H shows the repositories this computer hosts.";
        frame.render_widget(Paragraph::new(hint).style(Style::default().fg(dim())).wrap(Wrap { trim: true }).block(repo_block), area);
        app.regions.repo_panes.clear();
        app.regions.repo_list = Rect::default();
        return;
    };
    let title = format!("{}/{}", open.url.group, open.url.repo);
    let repo_block = block(&title, app.repos.focus == ReposFocus::Repo);
    let inner = repo_block.inner(area);
    frame.render_widget(repo_block, area);
    let [about, tabs_row, _, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);

    // The URL, HEAD, and what's being changed.
    let mut spans = vec![Span::styled(open.url.to_string(), Style::default().fg(dim()))];
    if let Some(head) = open.refs.ready().and_then(|r| r.head.as_deref()) {
        spans.push(Span::raw(format!("  HEAD {}", head.trim_start_matches("refs/heads/"))));
    }
    for what in app.repos.writing() {
        spans.push(Span::styled(format!("  ◌ {what}…"), Style::default().fg(Color::Yellow)));
    }
    if app.repos.downloading() {
        spans.push(Span::styled("  ◌ downloading…", Style::default().fg(Color::Yellow)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), about);

    // The panes, with how many each has.
    let counts = [
        open.refs.ready().map(|r| r.refs.len()),
        open.releases.ready().map(|r| r.releases.len()),
        open.work.ready().map(|w| w.active.len() + w.proposed.len()),
        None,
    ];
    let mut spans = Vec::new();
    let mut tabs = Vec::new();
    let mut x = tabs_row.x;
    for (pane, count) in RepoPane::ALL.into_iter().zip(counts) {
        let label = match count {
            Some(n) => format!(" {} {n} ", pane.title()),
            None => format!(" {} ", pane.title()),
        };
        let width = label.width() as u16;
        tabs.push((Rect::new(x, tabs_row.y, width, 1), pane));
        x += width + 1;
        let style = if pane == app.repos.pane { Style::default().fg(Color::Black).bg(accent()).bold() } else { Style::default().fg(dim()) };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), tabs_row);
    app.regions.repo_panes = tabs;
    app.regions.repo_list = body;

    match &open.detail {
        Some(Detail::Release { tag, load }) => draw_release(frame, app, tag, load, body),
        Some(Detail::Doc { load, .. }) => draw_doc(frame, app, load, body),
        Some(Detail::Browse(browse)) => draw_browse(frame, app, browse, body),
        None => match app.repos.pane {
            RepoPane::Branches => draw_refs(frame, app, &open, body),
            RepoPane::Releases => draw_releases(frame, app, &open, body),
            RepoPane::Work => draw_work(frame, app, &open, body),
            RepoPane::Permissions => draw_text(frame, app, &open.perms, "Who may do what here, as the node's .allowed file says.", body),
        },
    }
}

/// What a list shows while it's not there: being asked, or why it isn't.
fn not_ready<T>(frame: &mut Frame, load: &Load<T>, area: Rect) -> bool {
    let (text, color) = match load {
        Load::Ready(_) => return false,
        Load::Idle | Load::Loading => ("Asking the node…".to_string(), dim()),
        Load::Failed(e) => (e.clone(), Color::Red),
    };
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(color)).wrap(Wrap { trim: true }), area);
    true
}

fn draw_list(frame: &mut Frame, app: &mut App, items: Vec<ListItem>, empty: &str, area: Rect) {
    if items.is_empty() {
        frame.render_widget(Paragraph::new(empty.to_string()).style(Style::default().fg(dim())).wrap(Wrap { trim: true }), area);
        return;
    }
    if app.repos.items.selected().is_none_or(|i| i >= items.len()) {
        app.repos.items.select(Some(0));
    }
    let list = List::new(items).highlight_style(Style::default().bg(selected_bg()).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, area, &mut app.repos.items);
}

/// What the copy on this computer is, above the branches and tags.
fn copy_line(app: &App, open: &OpenRepo) -> Line<'static> {
    let url = open.url.to_string();
    match app.store.repos.iter().find(|r| r.url == url).and_then(|r| r.mirror.as_ref()) {
        Some(m) => Line::from(vec![
            Span::styled("⇩ Copy here: ", Style::default().fg(Color::Green)),
            Span::raw(format!(
                "{} branches and {} tags, {}, updated {} ago",
                m.branches,
                m.tags,
                human_bytes(m.bytes),
                super::ago(m.updated as i64)
            )),
            Span::styled("  Enter looks inside, m updates, M deletes", Style::default().fg(dim())),
        ]),
        None => Line::from(Span::styled("No copy on this computer: m makes one", Style::default().fg(dim()))),
    }
}

fn draw_refs(frame: &mut Frame, app: &mut App, open: &OpenRepo, area: Rect) {
    let [copy, area] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    frame.render_widget(Paragraph::new(copy_line(app, open)), copy);
    app.regions.repo_list = area;
    if not_ready(frame, &open.refs, area) {
        return;
    }
    let refs = open.refs.ready().expect("checked");
    let (copy, area) = if open.from_copy {
        let [note, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
        (Some(note), rest)
    } else {
        (None, area)
    };
    if let Some(note) = copy {
        frame.render_widget(
            Paragraph::new("The node didn't answer: these are the copy's. Enter looks inside.").style(Style::default().fg(Color::Yellow)),
            note,
        );
        app.regions.repo_list = area;
    }
    let items = refs
        .refs
        .iter()
        .map(|(sha, name)| {
            let (kind, short_name) = match (name.strip_prefix("refs/heads/"), name.strip_prefix("refs/tags/")) {
                (Some(branch), _) => ("branch", branch),
                (_, Some(tag)) => ("tag   ", tag),
                _ => ("ref   ", name.as_str()),
            };
            let mut spans = vec![
                Span::styled(format!("{kind} "), Style::default().fg(dim())),
                Span::styled(short_name.to_string(), Style::default().bold()),
                Span::styled(format!("  {}", &sha.to_string()[..7]), Style::default().fg(dim())),
            ];
            if refs.head.as_deref() == Some(name.as_str()) {
                spans.push(Span::styled("  HEAD", Style::default().fg(Color::Green)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    draw_list(frame, app, items, "Nothing pushed yet.", area);
}

fn draw_releases(frame: &mut Frame, app: &mut App, open: &OpenRepo, area: Rect) {
    if not_ready(frame, &open.releases, area) {
        return;
    }
    let list = open.releases.ready().expect("checked");
    let items = list
        .releases
        .iter()
        .map(|release| {
            let mut spans = vec![Span::styled(release.tag.clone(), Style::default().bold())];
            if list.latest.as_deref() == Some(release.tag.as_str()) {
                spans.push(Span::styled(" latest", Style::default().fg(Color::Green)));
            }
            let files = if release.artifacts == 1 { "1 file".to_string() } else { format!("{} files", release.artifacts) };
            spans.push(Span::styled(format!("  {files}"), Style::default().fg(dim())));
            if release.created > 0 {
                spans.push(Span::styled(format!("  {} ago", super::ago(release.created as i64)), Style::default().fg(dim())));
            }
            let preview = crate::net::git::clean_text(&release.preview, 200).replace('\n', " ");
            spans.push(Span::raw(format!("  {preview}")));
            ListItem::new(Line::from(spans))
        })
        .collect();
    draw_list(frame, app, items, "No releases. P publishes one of a tag pushed already.", area);
}

fn draw_work(frame: &mut Frame, app: &mut App, open: &OpenRepo, area: Rect) {
    if not_ready(frame, &open.work, area) {
        return;
    }
    let work = open.work.ready().expect("checked");
    let [scopes, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let mut spans = Vec::new();
    for (scope, title, n) in [
        (WorkScope::Active, "active", work.active.len()),
        (WorkScope::Proposed, "proposed", work.proposed.len()),
        (WorkScope::Completed, "done", work.completed.len()),
    ] {
        let style = if scope == open.scope { Style::default().fg(accent()).bold() } else { Style::default().fg(dim()) };
        spans.push(Span::styled(format!("{title} {n}"), style));
        spans.push(Span::styled("  ", Style::default()));
    }
    spans.push(Span::styled("(s shows the next)", Style::default().fg(dim())));
    frame.render_widget(Paragraph::new(Line::from(spans)), scopes);
    app.regions.repo_list = list_area;
    let items = open
        .documents()
        .iter()
        .map(|doc| {
            let comments = match doc.comments {
                0 => String::new(),
                1 => "  1 comment".into(),
                n => format!("  {n} comments"),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("#{} ", doc.id), Style::default().fg(dim())),
                Span::styled(crate::net::git::clean_text(&doc.title, 200), Style::default().bold()),
                Span::styled(format!("{comments}  {} ago", super::ago(doc.edited as i64)), Style::default().fg(dim())),
            ]))
        })
        .collect();
    let empty = match open.scope {
        WorkScope::Proposed => "Nothing proposed. p proposes a document.",
        WorkScope::Completed => "Nothing done yet.",
        _ => "No work documents. w writes one (p proposes one, if you may only propose).",
    };
    draw_list(frame, app, items, empty, list_area);
}

/// Text that scrolls (a document, the permissions).
fn draw_scrolled(frame: &mut Frame, app: &mut App, lines: Vec<Line<'static>>, area: Rect) {
    let width = area.width.max(1) as usize;
    let total: usize = lines.iter().map(|l| l.width().div_ceil(width).max(1)).sum();
    let most = total.saturating_sub(area.height as usize) as u16;
    app.repos.scroll = app.repos.scroll.min(most);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.repos.scroll, 0)), area);
}

fn draw_text(frame: &mut Frame, app: &mut App, load: &Load<String>, about: &str, area: Rect) {
    if not_ready(frame, load, area) {
        return;
    }
    let text = load.ready().expect("checked");
    let mut lines = vec![Line::styled(about.to_string(), Style::default().fg(dim())), Line::default()];
    lines.extend(text.lines().map(|l| Line::raw(l.to_string())));
    if text.trim().is_empty() {
        lines.push(Line::styled("(empty)", Style::default().fg(dim())));
    }
    draw_scrolled(frame, app, lines, area);
}

fn draw_release(frame: &mut Frame, app: &mut App, tag: &str, load: &Load<rngit::client::ReleaseDetail>, area: Rect) {
    if not_ready(frame, load, area) {
        return;
    }
    let release = load.ready().expect("checked");
    let label = |s: &str| Span::styled(format!("{s:<9}"), Style::default().fg(dim()));
    let mut lines = vec![
        Line::from(vec![
            label("Release"),
            Span::styled(tag.to_string(), Style::default().bold()),
            Span::raw(format!("  {}", release.status)),
        ]),
        Line::from(vec![label("Made by"), Span::raw(app.store.display_name(&release.created_by))]),
    ];
    if !release.hash.is_empty() {
        lines.push(Line::from(vec![label("Commit"), Span::raw(release.hash.chars().take(12).collect::<String>())]));
    }
    let notes = crate::net::git::clean_text(&release.notes, 2000);
    let notes_lines: Vec<&str> = notes.lines().take(6).collect();
    for line in &notes_lines {
        lines.push(Line::raw(line.to_string()));
    }
    let height = (lines.len() as u16 + 1).min(area.height.saturating_sub(2));
    let [about, files] = Layout::vertical([Constraint::Length(height), Constraint::Min(1)]).areas(area);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), about);
    app.regions.repo_list = files;
    let items = release
        .artifacts
        .iter()
        .map(|file| {
            ListItem::new(Line::from(vec![
                Span::raw(file.name.clone()),
                Span::styled(format!("  {}", human_bytes(file.size)), Style::default().fg(dim())),
            ]))
        })
        .collect();
    draw_list(frame, app, items, "This release has no files.", files);
}

/// A branch or tag of the copy: where in it, then its folder, a file or its log.
fn draw_browse(frame: &mut Frame, app: &mut App, browse: &Browse, area: Rect) {
    let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let place = if browse.path.is_empty() { "/".to_string() } else { format!("/{}", browse.path) };
    let (what, hint) = match &browse.view {
        BrowseView::Log(_) => ("log", "Esc back"),
        BrowseView::File(_) => ("file", "Esc back"),
        _ => ("", "Enter open, L log, Esc up"),
    };
    let title = if what == "log" { format!("{} · newest commits", browse.name()) } else { format!("{} · {place}", browse.name()) };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("⇩ ", Style::default().fg(Color::Green)),
            Span::styled(title, Style::default().bold()),
            Span::styled(format!("  {hint}"), Style::default().fg(dim())),
        ])),
        head,
    );
    app.regions.repo_list = body;
    match &browse.view {
        BrowseView::Failed(e) => {
            frame.render_widget(Paragraph::new(e.clone()).style(Style::default().fg(Color::Red)).wrap(Wrap { trim: true }), body);
        }
        BrowseView::Folder(entries) => {
            let items = entries
                .iter()
                .map(|entry| {
                    let (mark, color) = match entry.kind {
                        EntryKind::Dir => ("▸ ", Color::Cyan),
                        EntryKind::Link => ("↪ ", Color::Yellow),
                        EntryKind::Submodule => ("◆ ", Color::Yellow),
                        EntryKind::File => ("  ", Color::Reset),
                    };
                    let mut spans = vec![Span::styled(format!("{mark}{}", entry.name), Style::default().fg(color))];
                    if let Some(size) = entry.size {
                        spans.push(Span::styled(format!("  {}", human_bytes(size)), Style::default().fg(dim())));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect();
            draw_list(frame, app, items, "Nothing in this folder.", body);
        }
        BrowseView::Log(commits) => {
            let items = commits
                .iter()
                .map(|c| {
                    ListItem::new(Line::from(vec![
                        Span::styled(c.id.to_string()[..7].to_string(), Style::default().fg(Color::Yellow)),
                        Span::styled(format!("  {} ago  ", super::ago(c.time)), Style::default().fg(dim())),
                        Span::raw(c.subject.clone()),
                        Span::styled(format!("  {}", crate::net::git::clean_text(&c.author, 60)), Style::default().fg(dim())),
                    ]))
                })
                .collect();
            draw_list(frame, app, items, "No commits.", body);
        }
        BrowseView::File(view) => {
            let mut lines = Vec::new();
            match &view.text {
                Some(text) => {
                    lines.extend(text.lines().map(|l| Line::raw(l.to_string())));
                    if view.truncated {
                        lines.push(Line::styled(format!("… cut here; the file is {}", human_bytes(view.size)), Style::default().fg(dim())));
                    }
                }
                None => {
                    lines.push(Line::styled(format!("Not text ({}): nothing to show.", human_bytes(view.size)), Style::default().fg(dim())))
                }
            }
            draw_scrolled(frame, app, lines, body);
        }
    }
}

fn draw_doc(frame: &mut Frame, app: &mut App, load: &Load<crate::net::git::Doc>, area: Rect) {
    if not_ready(frame, load, area) {
        return;
    }
    let doc = load.ready().expect("checked");
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("#{} ", doc.id), Style::default().fg(dim())),
        Span::styled(doc.title.clone(), Style::default().bold()),
        Span::styled(format!("  {}", doc.scope), Style::default().fg(dim())),
    ])];
    let author = app.store.display_name(&doc.author);
    let signed = match &doc.signer {
        Some(signer) if *signer == doc.author => Span::styled("  ✓ signed by them", Style::default().fg(Color::Green)),
        Some(signer) => Span::styled(format!("  signed by {}", app.store.display_name(signer)), Style::default().fg(Color::Yellow)),
        None => Span::styled("  ✗ signature doesn't check out", Style::default().fg(Color::Red)),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("by {author}, {} ago", super::ago(doc.edited as i64)), Style::default().fg(dim())),
        signed,
    ]));
    lines.push(Line::default());
    lines.extend(doc.text.lines().map(|l| Line::raw(l.to_string())));
    for comment in &doc.comments {
        lines.push(Line::default());
        let who = app.store.display_name(&comment.author);
        lines.push(Line::styled(format!("— {who}, {} ago", super::ago(comment.created as i64)), Style::default().fg(accent())));
        lines.extend(comment.text.lines().map(|l| Line::raw(l.to_string())));
    }
    draw_scrolled(frame, app, lines, area);
}
