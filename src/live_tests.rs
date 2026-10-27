//! Two rettuis on a real network: each its own app and Reticulum stack (as
//! `rettui` runs them), joined over TCP on this computer, talking as two
//! people's would. Slower than the other tests (Reticulum takes its time
//! to find paths), and what they check doesn't go without the network.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rns_identity::identity::Identity;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::app::App;
use crate::config::{Paths, Settings};
use crate::lxmf::DeliveryMode;
use crate::net::{self, NetEvent};
use crate::store::{MessageState, Store};

/// Longest a step may take: paths are asked for and announces go round.
const PATIENCE: Duration = Duration::from_secs(60);

/// Every interface here takes in announces as they come. With ingress
/// control on, three of Amy's announces within a second (as when Ben
/// connects) start a burst on Ben's new interface, which holds announces
/// of destinations he doesn't know yet. On a link this quiet the burst
/// never ends in rsReticulum (ending it needs six recent announces), so a
/// held one, released, is held again: Ben might never hear Amy's node.
const TAKE_ALL: &str = "    ingress_control = No\n";

/// One rettui: its app, what its network tells it, and its folder.
struct Peer {
    app: App,
    events: UnboundedReceiver<NetEvent>,
    dir: PathBuf,
    identity: Identity,
}

/// A free port on this computer, for Amy's TCP server.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

impl Peer {
    /// A rettui in a fresh folder named for it, with `interface` as its only
    /// Reticulum interface (no shared instance, nothing else on the
    /// machine).
    fn start(name: &str, interface: &str, settings: impl FnOnce(&mut Settings)) -> Self {
        Self::start_in(name, "", interface, settings)
    }

    /// [`Peer::start`], with more for the config's `[reticulum]` section.
    fn start_in(name: &str, reticulum: &str, interface: &str, settings: impl FnOnce(&mut Settings)) -> Self {
        let dir = std::env::temp_dir().join(format!("rettui-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rns = dir.join("rns");
        std::fs::create_dir_all(&rns).unwrap();
        let config = format!(
            "[reticulum]\n  enable_transport = No\n  share_instance = No\n  panic_on_interface_error = No\n{reticulum}\n\
             [logging]\n  loglevel = 2\n\n[interfaces]\n{interface}"
        );
        std::fs::write(rns.join("config"), config).unwrap();
        let paths = Paths::new(Some(dir.clone())).unwrap();
        let mut chosen = Settings {
            display_name: name.to_string(),
            rns_config: Some(rns.display().to_string()),
            announce_at_start: true,
            welcomed: true,
            update_check: false,
            map_tiles: None,
            ..Settings::default()
        };
        settings(&mut chosen);
        chosen.save(&paths.settings).unwrap();
        let identity = crate::config::load_identity(&paths.identity).unwrap();
        let (net, events) = net::spawn(crate::net_options(&chosen, &paths, identity.clone()));
        let mut app = App::new(chosen, paths, Store::default(), net, None, identity.hash);
        app.updates.fetch = |_| Err("no update checks in tests".into());
        Peer { app, events, dir, identity }
    }

    /// Take in what the network said.
    fn pump(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            if let NetEvent::StartFailed(e) = &event {
                panic!("Reticulum didn't start: {e}");
            }
            self.app.on_net(event);
        }
        self.app.on_tick();
    }

    /// The LXMF address, once Reticulum is up.
    fn address(&self) -> String {
        hex::encode(self.app.lxmf_hash.expect("started"))
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.app.send(net::NetCommand::Shutdown(net::Stop::Quit));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Pump both until `done` says so (or it's taken too long: `what` didn't
/// happen).
async fn until(amy: &mut Peer, ben: &mut Peer, what: &str, mut done: impl FnMut(&Peer, &Peer) -> bool) {
    let started = Instant::now();
    loop {
        amy.pump();
        ben.pump();
        if done(amy, ben) {
            return;
        }
        if started.elapsed() > PATIENCE {
            let log = |p: &Peer| p.app.log.iter().rev().take(15).cloned().collect::<Vec<_>>().join("\n  ");
            // What each saw in channels, if anything.
            let chat = |p: &Peer| {
                let lines = p.app.channels.hubs.iter().flat_map(|h| {
                    h.buffers.iter().flat_map(move |(room, lines)| lines.iter().map(move |l| format!("[{}] {room}: {}", h.name, l.text)))
                });
                lines.collect::<Vec<_>>().join("\n  ")
            };
            panic!(
                "{what} didn't happen in time.\nAmy's log:\n  {}\nBen's log:\n  {}\nAmy's channels:\n  {}\nBen's channels:\n  {}",
                log(amy),
                log(ben),
                chat(amy),
                chat(ben)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Amy runs a TCP server (and, as `amy` says, a node, a propagation node, a
/// hub); Ben connects to it. Both started, and each heard the other.
async fn pair(name: &str, amy: impl FnOnce(&mut Settings)) -> (Peer, Peer) {
    let port = free_port();
    let server = format!(
        "  [[Server]]\n    type = TCPServerInterface\n    enabled = Yes\n    listen_ip = 127.0.0.1\n    listen_port = {port}\n{TAKE_ALL}"
    );
    let client = format!(
        "  [[To Amy]]\n    type = TCPClientInterface\n    enabled = Yes\n    target_host = 127.0.0.1\n    target_port = {port}\n{TAKE_ALL}"
    );
    let mut amy = Peer::start(&format!("{name}-amy"), &server, amy);
    // Amy listening before Ben connects.
    let started = Instant::now();
    while amy.app.lxmf_hash.is_none() {
        amy.pump();
        assert!(started.elapsed() < PATIENCE, "Amy didn't start");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut ben = Peer::start(&format!("{name}-ben"), &client, |_| {});
    until(&mut amy, &mut ben, "Ben starting", |_, b| b.app.lxmf_hash.is_some()).await;
    // Each announces until the other has heard: the first may go out before
    // the TCP connection is up.
    let mut asked = Instant::now() - Duration::from_secs(10);
    let heard = |a: &Peer, b: &Peer| a.app.store.peers.contains_key(&b.address()) && b.app.store.peers.contains_key(&a.address());
    while !heard(&amy, &ben) {
        if asked.elapsed() > Duration::from_secs(2) {
            amy.app.send(net::NetCommand::Announce);
            ben.app.send(net::NetCommand::Announce);
            asked = Instant::now();
        }
        until(&mut amy, &mut ben, "Amy and Ben hearing each other", |a, b| heard(a, b) || asked.elapsed() > Duration::from_secs(2)).await;
    }
    (amy, ben)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_rettuis_message_each_other_and_find_their_way() {
    let (mut amy, mut ben) = pair("messages", |_| {}).await;
    let (to_ben, to_amy) = (ben.address(), amy.address());
    assert_eq!(amy.app.store.display_name(&to_ben), "messages-ben");

    // A message each way, directly: received, and its sender told it was.
    let id = amy.app.send_message(to_ben.clone(), "Hello Ben".into(), Vec::new(), DeliveryMode::Direct, None).unwrap();
    until(&mut amy, &mut ben, "Ben getting Amy's message", |_, b| {
        b.app.store.conversations.get(&to_amy).is_some_and(|c| c.messages.iter().any(|m| m.content == "Hello Ben"))
    })
    .await;
    let local = format!("local-{id}");
    until(&mut amy, &mut ben, "Amy hearing it was delivered", |a, _| {
        a.app.store.conversations[&to_ben].messages.iter().any(|m| m.id == local && m.state == MessageState::Delivered)
    })
    .await;
    ben.app.send_message(to_amy.clone(), "Hi Amy".into(), Vec::new(), DeliveryMode::Auto, None).unwrap();
    until(&mut amy, &mut ben, "Amy getting Ben's answer", |a, _| {
        a.app.store.conversations.get(&to_ben).is_some_and(|c| c.messages.iter().any(|m| m.content == "Hi Amy"))
    })
    .await;
    // Received messages are checked: Ben's key is known to Amy.
    let theirs = amy.app.store.conversations[&to_ben].messages.iter().find(|m| m.content == "Hi Amy").unwrap().clone();
    assert!(matches!(theirs.state, MessageState::Received { verified: true }));

    // A reaction, a file and a reply.
    let file = amy.dir.join("note.txt");
    std::fs::write(&file, "a note").unwrap();
    amy.app.send_message(to_ben.clone(), "With a file".into(), vec![file], DeliveryMode::Direct, Some(theirs.id.clone())).unwrap();
    amy.app.send_reaction(&to_ben, &theirs.id, "👍").unwrap();
    until(&mut amy, &mut ben, "Ben getting the file and the reaction", |_, b| {
        let conversation = &b.app.store.conversations[&to_amy];
        let file = conversation.messages.iter().any(|m| m.content == "With a file" && m.attachments.len() == 1 && m.reply.is_some());
        let reacted = conversation.messages.iter().any(|m| m.reactions.iter().any(|r| r.emoji == "👍"));
        file && reacted
    })
    .await;

    // Ping, the path, a probe.
    amy.app.ping(&to_ben).unwrap();
    until(&mut amy, &mut ben, "Amy's ping answered", |a, _| {
        matches!(a.app.pings.get(&to_ben), Some(crate::app::contacts::PingState::Done { result: Ok(_), .. }))
    })
    .await;
    amy.app.find_path_to(&to_ben).unwrap();
    until(&mut amy, &mut ben, "Amy finding the path", |a, _| {
        matches!(a.app.path_lookups.get(&to_ben), Some(crate::app::paths::PathLookup::Done(Ok(_))))
    })
    .await;
    amy.app.probe(&to_ben).unwrap();
    until(&mut amy, &mut ben, "Amy's probe answered", |a, _| {
        matches!(a.app.probes.get(&to_ben), Some(crate::app::paths::ProbeState::Done(_)))
    })
    .await;
    // Whose Ben's address is: his identity, and his other addresses.
    amy.app.whois(&to_ben).unwrap();
    until(&mut amy, &mut ben, "Amy finding whose it is", |a, _| matches!(a.app.whois.looked_up, Some((_, Some(_))))).await;
    let Some((_, Some(Ok(found)))) = &amy.app.whois.looked_up else { panic!("{:?}", amy.app.whois.looked_up) };
    assert_eq!((found.identity, found.kind), (ben.app.identity_hash, Some("LXMF address")));
    assert!(found.others.iter().any(|(kind, hash)| *kind == "speed test" && *hash == ben.app.speed_address()));
    amy.app.forget_path(&to_ben).unwrap();
    until(&mut amy, &mut ben, "the path forgotten", |a, _| !a.app.path_lookups.contains_key(&to_ben)).await;
    // Both see their interface up.
    assert!(amy.app.interfaces.iter().any(|i| i.online) && ben.app.interfaces.iter().any(|i| i.online));
    let _ = (&amy.identity, &ben.identity);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn amys_node_propagation_node_and_hub() {
    use crate::app::node::NodeStatus;
    let (mut amy, mut ben) = pair("hosting", |s| {
        s.node_enabled = true;
        s.node_name = Some("Amy's node".into());
        s.pn_enabled = true;
        s.hub_enabled = true;
        s.hub_name = Some("Amy's hub".into());
    })
    .await;
    until(&mut amy, &mut ben, "Amy's node, propagation node and hub running", |a, _| {
        a.app.node.status == NodeStatus::Running && a.app.pn.status == NodeStatus::Running && a.app.hub.status == NodeStatus::Running
    })
    .await;
    let (to_amy, to_ben) = (amy.address(), ben.address());

    // Ben browses Amy's node, once he's heard it.
    let node = hex::encode(amy.app.node.hash);
    amy.app.send(net::NetCommand::HostAnnounce);
    until(&mut amy, &mut ben, "Ben hearing Amy's node", |_, b| b.app.store.peers.contains_key(&node)).await;
    let index = crate::app::resolve_url(&format!("{node}:/page/index.mu"), None).unwrap();
    ben.app.navigate(index);
    until(&mut amy, &mut ben, "Ben loading Amy's page", |_, b| b.app.browser.tab.loading.is_none() && b.app.browser.tab.source.is_some())
        .await;
    assert!(ben.app.browser.tab.source.as_deref().unwrap().contains("Amy's node"), "{:?}", ben.app.browser.tab.source);
    // A page that isn't there.
    ben.app.navigate(crate::app::resolve_url(&format!("{node}:/page/nope.mu"), None).unwrap());
    until(&mut amy, &mut ben, "Ben told the page isn't there", |_, b| b.app.browser.tab.loading.is_none()).await;

    // Amy blocks Ben (his identity known, so in Reticulum too), then lets
    // him back.
    amy.app.block_contact(&to_ben).unwrap();
    until(&mut amy, &mut ben, "Ben blocked in Reticulum", |a, _| a.app.log.iter().any(|l| l.contains("Blocked") && l.contains("identity")))
        .await;
    amy.app.unblock_contact(&to_ben).unwrap();
    until(&mut amy, &mut ben, "Ben unblocked in Reticulum", |a, _| {
        a.app.log.iter().any(|l| l.contains("Unblocked") && l.contains("identity"))
    })
    .await;

    // Through Amy's propagation node: Ben leaves a message there for Amy,
    // who collects it by syncing.
    let pn = hex::encode(amy.app.pn.hash);
    amy.app.send(net::NetCommand::Announce);
    until(&mut amy, &mut ben, "Ben hearing Amy's propagation node", |_, b| b.app.store.peers.contains_key(&pn)).await;
    ben.app.set_propagation_node(&pn);
    amy.app.set_propagation_node(&pn);
    ben.app.send_message(to_amy.clone(), "Left at your node".into(), Vec::new(), DeliveryMode::Propagated, None).unwrap();
    until(&mut amy, &mut ben, "Ben's message handed to the propagation node", |_, b| {
        b.app.store.conversations[&to_amy].messages.iter().any(|m| m.state == MessageState::Propagated)
    })
    .await;
    amy.app.send(net::NetCommand::Sync);
    until(&mut amy, &mut ben, "Amy collecting it", |a, _| {
        a.app.store.conversations.get(&to_ben).is_some_and(|c| c.messages.iter().any(|m| m.content == "Left at your node"))
    })
    .await;
    // The other way: Amy leaves one there for Ben, who fetches it over a
    // Link to her node.
    amy.app.send_message(to_ben.clone(), "One for you".into(), Vec::new(), DeliveryMode::Propagated, None).unwrap();
    until(&mut amy, &mut ben, "Amy's message on her node", |a, _| {
        a.app.store.conversations[&to_ben].messages.iter().any(|m| m.content == "One for you" && m.state == MessageState::Propagated)
    })
    .await;
    ben.app.send(net::NetCommand::Sync);
    until(&mut amy, &mut ben, "Ben collecting it", |_, b| {
        b.app.store.conversations.get(&to_amy).is_some_and(|c| c.messages.iter().any(|m| m.content == "One for you"))
    })
    .await;

    // Amy's hub: Ben joins a room and says something; Amy, in the same
    // room, hears it.
    let (hub, _) = amy.app.hub.address.expect("the hub's address");
    amy.app.send(net::NetCommand::HubAction(crate::rrc::host::HubAction::Announce));
    ben.app.add_hub(hub, crate::rrc::DEFAULT_ASPECT, Some("lobby".into()));
    let ben_hub = ben.app.channels.hub_index(hub).unwrap();
    until(&mut amy, &mut ben, "Ben in the lobby", |_, b| b.app.channels.hubs[ben_hub].rooms.contains("lobby")).await;
    let amy_hub = amy.app.channels.hub_index(hub).expect("Amy's client on her own hub");
    until(&mut amy, &mut ben, "Amy connected to her hub", |a, _| {
        matches!(a.app.channels.hubs[amy_hub].status, crate::app::channels::HubStatus::Connected)
    })
    .await;
    amy.app.join_room(amy_hub, "lobby");
    until(&mut amy, &mut ben, "Amy in the lobby", |a, _| a.app.channels.hubs[amy_hub].rooms.contains("lobby")).await;
    until(&mut amy, &mut ben, "Amy seeing Ben join", |a, _| {
        a.app.channels.hubs[amy_hub].buffers.get("lobby").is_some_and(|lines| lines.iter().any(|l| l.text.contains("hosting-ben joined")))
    })
    .await;
    ben.app.channel_submit(ben_hub, "lobby", "hello from ben");
    until(&mut amy, &mut ben, "Amy hearing Ben in the lobby", |a, _| {
        a.app.channels.hubs[amy_hub].buffers.get("lobby").is_some_and(|lines| lines.iter().any(|l| l.text.contains("hello from ben")))
    })
    .await;
    // Who's there, from the hub's side.
    until(&mut amy, &mut ben, "the hub counting two", |a, _| a.app.hub.snapshot.people.len() >= 2).await;
    let _ = to_ben;
}

/// Amy hosts a repository; Ben, over Reticulum, reads its branches and
/// makes a copy, and is turned away from one he isn't allowed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn amys_repositories_read_by_ben() {
    use crate::app::node::NodeStatus;
    let repos = std::env::temp_dir().join(format!("rettui-live-gitrepos-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repos);
    let git = git_in;
    let work = repos.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    std::fs::write(work.join("README.md"), "hello\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "first"]);
    git(&work, &["tag", "v1"]);
    for name in ["open", "closed"] {
        let bare = repos.join("served/public").join(format!("{name}.git"));
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "-q", "--bare", "-b", "main"]);
        git(&work, &["push", "-q", bare.to_str().unwrap(), "main", "--tags"]);
    }
    // Only the first can be read, by anyone.
    std::fs::write(repos.join("served/public/open.git.allowed"), "r:all").unwrap();
    let served = repos.join("served").display().to_string();
    let (mut amy, mut ben) = pair("githost", |s| {
        s.git_host_enabled = true;
        s.git_host_dir = Some(served);
    })
    .await;
    until(&mut amy, &mut ben, "Amy's repositories hosted", |a, _| a.app.git_host.status == NodeStatus::Running).await;
    let node = hex::encode(amy.app.git_host.address.expect("the node's address"));

    // Ben reads its branches and tags (waiting for the announce that gives him a path).
    ben.app.open_repo(&format!("rns://{node}/public/open")).unwrap();
    let wait = Instant::now();
    loop {
        until(&mut amy, &mut ben, "Ben's answer", |_, b| {
            !matches!(b.app.repos.open.as_ref().unwrap().refs, crate::app::repos::Load::Loading)
        })
        .await;
        match &ben.app.repos.open.as_ref().unwrap().refs {
            crate::app::repos::Load::Ready(refs) => {
                let names: Vec<&str> = refs.refs.iter().map(|(_, n)| n.as_str()).collect();
                assert_eq!(names, ["refs/heads/main", "refs/tags/v1"]);
                break;
            }
            // The path may not be known yet: ask again.
            _ if wait.elapsed() < PATIENCE => {
                tokio::time::sleep(Duration::from_secs(2)).await;
                ben.app.reload_repo();
            }
            other => panic!("Ben couldn't read the repository: {other:?}"),
        }
    }
    // A copy of it, which has the history.
    ben.app.mirror_repo().unwrap();
    let url = ben.app.repos.open.as_ref().unwrap().url.clone();
    until(&mut amy, &mut ben, "Ben's copy", |_, b| b.app.copy_refs(&url).is_some()).await;
    assert_eq!(ben.app.copy_refs(&url).unwrap().refs.len(), 2);
    // The one nobody may read is not there, as far as Ben can tell.
    ben.app.open_repo(&format!("rns://{node}/public/closed")).unwrap();
    until(&mut amy, &mut ben, "Ben refused", |_, b| matches!(b.app.repos.open.as_ref().unwrap().refs, crate::app::repos::Load::Failed(_)))
        .await;
    assert!(amy.app.git_host.requests >= 2 && amy.app.git_host.refused >= 1, "{} {}", amy.app.git_host.requests, amy.app.git_host.refused);
    let _ = std::fs::remove_dir_all(&repos);
}

/// Run git in `dir` as a nobody with no settings, and say what it printed.
fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Ben sends a copy of one of Amy's repositories to a new one on her node,
/// which his identity may make, and it holds the history.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ben_pushes_to_amys_repositories() {
    use crate::app::node::NodeStatus;
    let repos = std::env::temp_dir().join(format!("rettui-live-gitpush-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repos);
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let work = repos.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    for n in 0..2 {
        std::fs::write(work.join(format!("f{n}")), format!("file {n}\n")).unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", &format!("commit {n}")]);
    }
    git(&work, &["tag", "v1"]);
    let source = repos.join("served/public/source.git");
    std::fs::create_dir_all(&source).unwrap();
    git(&source, &["init", "-q", "--bare", "-b", "main"]);
    git(&work, &["push", "-q", source.to_str().unwrap(), "main", "--tags"]);
    let served = repos.join("served").display().to_string();
    let (mut amy, mut ben) = pair("gitpush", |s| {
        s.git_host_enabled = true;
        s.git_host_dir = Some(served);
        // Anyone may read, write and make repositories.
        s.git_host_access = Some("rw:all, c:all".into());
    })
    .await;
    until(&mut amy, &mut ben, "Amy's repositories hosted", |a, _| a.app.git_host.status == NodeStatus::Running).await;
    let node = hex::encode(amy.app.git_host.address.expect("the node's address"));
    // Ben's copy of the source (waiting for the announce that gives him a path).
    ben.app.open_repo(&format!("rns://{node}/public/source")).unwrap();
    let from = ben.app.repos.open.as_ref().unwrap().url.clone();
    let wait = Instant::now();
    loop {
        // Asked again until the path is known (an answer comes, or two seconds pass).
        ben.app.mirror_repo().ok();
        let asked = Instant::now();
        until(&mut amy, &mut ben, "Ben's copy", |_, b| b.app.copy_refs(&from).is_some() || asked.elapsed() > Duration::from_secs(4)).await;
        if ben.app.copy_refs(&from).is_some() {
            break;
        }
        assert!(wait.elapsed() < PATIENCE, "Ben couldn't copy the repository");
    }
    // Sent on to a repository that isn't there yet.
    let to = crate::app::repos::parse_repo(&format!("rns://{node}/public/made")).unwrap();
    ben.app.push_copy(&to, crate::app::repos::PushHow::parse("add").unwrap()).unwrap();
    let made = repos.join("served/public/made.git");
    until(&mut amy, &mut ben, "the new repository holding the history", |_, _| made.join("refs/heads/main").exists()).await;
    assert_eq!(git(&made, &["rev-parse", "main"]), git(&work, &["rev-parse", "main"]));
    assert_eq!(git(&made, &["rev-parse", "v1^{commit}"]), git(&work, &["rev-parse", "v1^{commit}"]));
    git(&made, &["fsck", "--strict"]);
    assert!(amy.app.git_host.requests >= 5, "{}", amy.app.git_host.requests);
    let _ = std::fs::remove_dir_all(&repos);
}

/// Ben publishes a release of one of Amy's repositories, reads it back, and
/// deletes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ben_publishes_a_release_on_amys_repository() {
    use crate::app::node::NodeStatus;
    let repos = std::env::temp_dir().join(format!("rettui-live-gitrelease-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repos);
    let work = repos.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git_in(&work, &["init", "-q", "-b", "main"]);
    std::fs::write(work.join("f"), "file\n").unwrap();
    git_in(&work, &["add", "."]);
    git_in(&work, &["commit", "-q", "-m", "one"]);
    git_in(&work, &["tag", "v1"]);
    let source = repos.join("served/public/source.git");
    std::fs::create_dir_all(&source).unwrap();
    git_in(&source, &["init", "-q", "--bare", "-b", "main"]);
    git_in(&work, &["push", "-q", source.to_str().unwrap(), "main", "--tags"]);
    let file = repos.join("app.bin");
    std::fs::write(&file, b"the app").unwrap();
    let served = repos.join("served").display().to_string();
    let (mut amy, mut ben) = pair("gitrelease", |s| {
        s.git_host_enabled = true;
        s.git_host_dir = Some(served);
        s.git_host_access = Some("rw:all, rel:all".into());
    })
    .await;
    until(&mut amy, &mut ben, "Amy's repositories hosted", |a, _| a.app.git_host.status == NodeStatus::Running).await;
    let node = hex::encode(amy.app.git_host.address.expect("the node's address"));
    ben.app.open_repo(&format!("rns://{node}/public/source")).unwrap();

    // Asked again until Amy's path is known, and the (empty) list of releases comes.
    let wait = Instant::now();
    loop {
        ben.app.reload_repo();
        let asked = Instant::now();
        until(&mut amy, &mut ben, "the releases", |_, b| {
            b.app.repos.open.as_ref().is_some_and(|o| o.releases.ready().is_some()) || asked.elapsed() > Duration::from_secs(4)
        })
        .await;
        if ben.app.repos.open.as_ref().is_some_and(|o| o.releases.ready().is_some()) {
            break;
        }
        assert!(wait.elapsed() < PATIENCE, "Ben couldn't read the releases");
    }
    ben.app.publish_release_paths("v1", "First release", &file.display().to_string()).unwrap();
    let kept = repos.join("served/public/source.git.releases/v1");
    until(&mut amy, &mut ben, "the release published", |_, _| {
        std::fs::read_to_string(kept.join("META")).is_ok_and(|m| m.contains("published"))
    })
    .await;
    assert_eq!(std::fs::read(kept.join("artifacts/app.bin")).unwrap(), b"the app");
    assert!(kept.join("artifacts/manifest.rsm").is_file() && kept.join("artifacts/app.bin.rsg").is_file());

    // Ben reads it back from Amy's node.
    let listed = |b: &Peer| b.app.repos.open.as_ref().and_then(|o| o.releases.ready().map(|r| (r.releases.len(), r.latest.clone())));
    ben.app.reload_repo();
    until(&mut amy, &mut ben, "the release listed", |_, b| listed(b) == Some((1, Some("v1".into())))).await;
    ben.app.delete_release("v1").unwrap();
    until(&mut amy, &mut ben, "the release deleted", |_, _| !kept.exists()).await;
    let _ = std::fs::remove_dir_all(&repos);
}

/// Ben proposes a work document on one of Amy's repositories, and comments on
/// it; Amy's node checks his signature, and what he reads back verifies.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ben_proposes_a_work_document_on_amys_repository() {
    use crate::app::node::NodeStatus;
    use crate::app::repos::Detail;
    use rngit::client::WorkScope;
    let repos = std::env::temp_dir().join(format!("rettui-live-gitwork-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repos);
    let source = repos.join("served/public/source.git");
    std::fs::create_dir_all(&source).unwrap();
    git_in(&source, &["init", "-q", "--bare", "-b", "main"]);
    let served = repos.join("served").display().to_string();
    let (mut amy, mut ben) = pair("gitwork", |s| {
        s.git_host_enabled = true;
        s.git_host_dir = Some(served);
        s.git_host_access = Some("r:all, p:all, i:all".into());
    })
    .await;
    until(&mut amy, &mut ben, "Amy's repositories hosted", |a, _| a.app.git_host.status == NodeStatus::Running).await;
    let node = hex::encode(amy.app.git_host.address.expect("the node's address"));
    ben.app.open_repo(&format!("rns://{node}/public/source")).unwrap();

    // Asked again until Amy's path is known, and the (empty) list of documents comes.
    let wait = Instant::now();
    loop {
        ben.app.reload_repo();
        let asked = Instant::now();
        until(&mut amy, &mut ben, "the documents", |_, b| {
            b.app.repos.open.as_ref().is_some_and(|o| o.work.ready().is_some()) || asked.elapsed() > Duration::from_secs(4)
        })
        .await;
        if ben.app.repos.open.as_ref().is_some_and(|o| o.work.ready().is_some()) {
            break;
        }
        assert!(wait.elapsed() < PATIENCE, "Ben couldn't read the documents");
    }
    ben.app.new_doc(true, "Idea", "Build a thing\n").unwrap();
    let proposed = repos.join("served/public/source.git.work/proposed/1/root");
    until(&mut amy, &mut ben, "the proposal kept", |_, _| proposed.is_file()).await;
    assert!(repos.join("served/public/source.git.work/1.allowed").is_file());

    // Read back: listed, and signed by Ben's key.
    let ben_hash = hex::encode(ben.identity.hash);
    ben.app.reload_repo();
    until(&mut amy, &mut ben, "the proposal listed", |_, b| {
        b.app.repos.open.as_ref().and_then(|o| o.work.ready()).is_some_and(|w| w.proposed.len() == 1)
    })
    .await;
    ben.app.open_doc(1, WorkScope::Proposed);
    until(&mut amy, &mut ben, "the proposal read", |_, b| {
        matches!(b.app.repos.open.as_ref().and_then(|o| o.detail.as_ref()), Some(Detail::Doc { load, .. }) if load.ready().is_some())
    })
    .await;
    let doc = match ben.app.repos.open.as_ref().and_then(|o| o.detail.clone()) {
        Some(Detail::Doc { load, .. }) => load.ready().cloned().unwrap(),
        _ => unreachable!(),
    };
    assert_eq!((doc.title.as_str(), doc.text.as_str(), doc.author.as_str()), ("Idea", "Build a thing", ben_hash.as_str()));
    assert_eq!(doc.signer.as_deref(), Some(ben_hash.as_str()));

    // A comment goes in too.
    ben.app.comment_doc(1, WorkScope::Proposed, "Soon").unwrap();
    let comment = repos.join("served/public/source.git.work/proposed/1/1");
    until(&mut amy, &mut ben, "the comment kept", |_, _| comment.is_file()).await;
    let _ = std::fs::remove_dir_all(&repos);
}

/// Run `work`, taking in what Amy's network says meanwhile (as her rettui
/// would, running).
async fn while_running<T>(amy: &mut Peer, work: impl std::future::Future<Output = T>) -> T {
    tokio::pin!(work);
    loop {
        tokio::select! {
            out = &mut work => return out,
            () = tokio::time::sleep(Duration::from_millis(50)) => amy.pump(),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_command_line_against_a_running_rettui() {
    use crate::app::node::NodeStatus;
    let port = free_port();
    let server = format!("  [[Server]]\n    type = TCPServerInterface\n    listen_ip = 127.0.0.1\n    listen_port = {port}\n{TAKE_ALL}");
    let mut amy = Peer::start("cli-amy", &server, |s| {
        s.node_enabled = true;
        s.node_name = Some("Amy's node".into());
        s.pn_enabled = true;
    });
    let started = Instant::now();
    while !(amy.app.node.status == NodeStatus::Running && amy.app.pn.status == NodeStatus::Running) {
        amy.pump();
        assert!(started.elapsed() < PATIENCE, "Amy's node didn't start");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    amy.app.send(net::NetCommand::Announce);
    amy.app.send(net::NetCommand::HostAnnounce);
    let link = crate::lxmf::identity_link(amy.app.lxmf_hash.unwrap(), &amy.identity.get_public_key());
    let (node, pn) = (hex::encode(amy.app.node.hash), hex::encode(amy.app.pn.hash));

    // The command line's own folder and Reticulum, a client of Amy's.
    let dir = std::env::temp_dir().join(format!("rettui-live-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let rns = dir.join("rns");
    std::fs::create_dir_all(&rns).unwrap();
    std::fs::write(
        rns.join("config"),
        format!(
            "[reticulum]\n  share_instance = No\n\n[interfaces]\n  [[To Amy]]\n    type = TCPClientInterface\n    \
             target_host = 127.0.0.1\n    target_port = {port}\n{TAKE_ALL}"
        ),
    )
    .unwrap();
    let paths = Paths::new(Some(dir.clone())).unwrap();
    let settings = Settings { rns_config: Some(rns.display().to_string()), display_name: "cli".into(), ..Settings::default() };
    settings.save(&paths.settings).unwrap();
    let identity = crate::config::load_identity(&paths.identity).unwrap();

    // `rettui fetch`: a page, as text, as Micron, and to a file.
    let url = format!("{node}:/page/index.mu");
    while_running(&mut amy, crate::cli::fetch(&settings, &url, false, None, None)).await.unwrap();
    while_running(&mut amy, crate::cli::fetch(&settings, &url, true, Some(identity.clone()), None)).await.unwrap();
    let saved = dir.join("index.mu");
    while_running(&mut amy, crate::cli::fetch(&settings, &url, true, None, Some(saved.clone()))).await.unwrap();
    assert!(std::fs::read_to_string(&saved).unwrap().contains("Amy's node"));
    assert!(crate::cli::fetch(&settings, "nonsense", false, None, None).await.is_err());

    // `rettui send` to Amy (by her link, so her key is known at once), and
    // on paper; `rettui ping`.
    while_running(
        &mut amy,
        crate::cli::send(&settings, &paths, identity.clone(), &link, "From the command line", Vec::new(), DeliveryMode::Direct),
    )
    .await
    .unwrap();
    let from_cli =
        hex::encode(rns_identity::destination::Destination::hash_from_name_and_identity(crate::lxmf::LXMF_ASPECT, Some(&identity.hash)));
    let started = Instant::now();
    while !amy.app.store.conversations.get(&from_cli).is_some_and(|c| c.messages.iter().any(|m| m.content == "From the command line")) {
        amy.pump();
        assert!(started.elapsed() < PATIENCE, "Amy didn't get the command line's message");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    while_running(&mut amy, crate::cli::send(&settings, &paths, identity.clone(), &link, "On paper", Vec::new(), DeliveryMode::Paper))
        .await
        .unwrap();
    assert!(
        crate::cli::send(&settings, &paths, identity.clone(), &link, "x", vec![dir.join("missing")], DeliveryMode::Direct).await.is_err()
    );
    while_running(&mut amy, crate::cli::ping(&settings, &paths, identity.clone(), &link)).await.unwrap();

    // `rettui path` (and forgetting it), `rettui path --table`, `rettui probe`.
    let amy_address = hex::encode(amy.app.lxmf_hash.unwrap());
    while_running(&mut amy, crate::cli::path(&settings, &amy_address, false)).await.unwrap();
    while_running(&mut amy, crate::cli::path_table(&settings)).await.unwrap();
    // The path and announce-rate tables, as the Network tab reads them.
    amy.app.tables_wanted();
    let started = Instant::now();
    while !matches!(&amy.app.tables.read, Some(Ok(_))) {
        amy.pump();
        assert!(started.elapsed() < PATIENCE, "Amy's tables weren't read: {:?}", amy.app.tables.read);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = while_running(&mut amy, crate::cli::path(&settings, &amy_address, true)).await;
    while_running(&mut amy, crate::cli::probe(&settings, &paths, &node, None)).await.unwrap();
    assert!(crate::cli::path(&settings, "nope", false).await.is_err());

    // `rettui sync` with Amy's propagation node, and `rettui listen` while
    // she writes.
    while_running(&mut amy, crate::cli::sync(&settings, &paths, identity.clone(), Some(&pn))).await.unwrap();
    assert!(crate::cli::sync(&settings, &paths, identity.clone(), None).await.is_err());
    amy.app.send_message(from_cli.clone(), "Heard while listening".into(), Vec::new(), DeliveryMode::Direct, None).unwrap();
    while_running(&mut amy, crate::cli::listen(&settings, &paths, identity.clone(), 4)).await.unwrap();

    // `rettui backup` and `rettui restore`.
    let backup = dir.join("backup.tar.gz");
    crate::cli::backup(&paths, Some(backup.clone()), crate::backup::Options { identity: true, files: false }).unwrap();
    assert!(backup.is_file());
    let empty = Paths::new(Some(dir.join("empty"))).unwrap();
    assert!(crate::cli::backup(&empty, Some(dir.join("none.tar.gz")), crate::backup::Options { identity: false, files: false }).is_err());
    crate::cli::restore(&empty, &backup, false).unwrap();
    assert!(empty.settings.is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

impl Peer {
    /// A key, as the terminal UI takes it, then the screen drawn.
    fn key(&mut self, code: crossterm::event::KeyCode) {
        self.app.on_key(crossterm::event::KeyEvent::from(code));
        self.draw();
    }

    /// Typed, then Enter.
    fn line(&mut self, text: &str) {
        for c in text.chars() {
            self.app.on_key(crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(c)));
        }
        self.key(crossterm::event::KeyCode::Enter);
    }

    fn draw(&mut self) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(120, 36)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, &mut self.app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..36).map(|y| (0..120).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n").collect()
    }

    /// Lines of a hub's room (by the hub's address), as text.
    fn heard(&self, hub: crate::net::Hash, room: &str) -> String {
        let Some(index) = self.app.channels.hub_index(hub) else { return String::new() };
        self.app.channels.hubs[index]
            .buffers
            .get(room)
            .map(|lines| lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn talking_in_a_hub_at_the_keyboard() {
    use crossterm::event::KeyCode;
    let (mut amy, mut ben) = pair("chat", |s| {
        s.hub_enabled = true;
        s.hub_name = Some("Amy's hub".into());
        // Longer than a packet: it comes as a Resource.
        s.hub_greeting = Some(format!("Welcome to the hill. {}", "This hub is for talking about hills. ".repeat(40)));
    })
    .await;
    until(&mut amy, &mut ben, "Amy's hub running", |a, _| a.app.hub.status == crate::app::node::NodeStatus::Running).await;
    let (hub, _) = amy.app.hub.address.unwrap();
    let amy_hub = loop {
        if let Some(index) = amy.app.channels.hub_index(hub) {
            break index;
        }
        amy.pump();
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    until(&mut amy, &mut ben, "Amy connected to her hub", |a, _| {
        matches!(a.app.channels.hubs[amy_hub].status, crate::app::channels::HubStatus::Connected)
    })
    .await;
    amy.app.join_room(amy_hub, "lobby");

    // Ben adds the hub by the Channels tab's n, with a room in the link.
    ben.key(KeyCode::Char('3'));
    ben.key(KeyCode::Char('n'));
    ben.line(&format!("rrc://{}/lobby", hex::encode(hub)));
    until(&mut amy, &mut ben, "Ben joining the lobby", |a, _| a.heard(hub, "lobby").contains("chat-ben joined")).await;
    ben.app.channels.selected = Some(crate::app::channels::Target { hub, room: Some("lobby".into()) });
    ben.draw();

    // Writing: plain, an action, mentions completed with Tab.
    ben.key(KeyCode::Enter);
    assert!(ben.app.channels.typing);
    ben.line("hello everyone");
    ben.line("/me waves");
    for c in "@chat-a".chars() {
        ben.app.on_key(crossterm::event::KeyEvent::from(KeyCode::Char(c)));
    }
    ben.key(KeyCode::Tab);
    ben.line("are you there?");
    until(&mut amy, &mut ben, "Amy hearing Ben", |a, _| {
        let heard = a.heard(hub, "lobby");
        heard.contains("hello everyone") && heard.contains("waves") && heard.contains("are you there?")
    })
    .await;
    // Commands: help, who's here, rooms, a ping, another room, a nick,
    // a whisper, an LXMF message, cleared.
    for command in ["/help", "/who", "/list", "/ping", "/join garden", "/part garden", "/nick benny", "/raw nonsense"] {
        ben.line(command);
    }
    let amy_nick = amy.app.channels.hubs[amy_hub].nick.clone().unwrap_or_else(|| "chat-amy".into());
    ben.line(&format!("/msg {amy_nick} just for you"));
    until(&mut amy, &mut ben, "Amy getting Ben's whisper", |a, _| {
        a.app.channels.hubs[amy_hub]
            .buffers
            .iter()
            .any(|(room, lines)| room.starts_with('@') && lines.iter().any(|l| l.text.contains("just for you")))
    })
    .await;
    ben.line(&format!("/dm {amy_nick} over LXMF"));
    let to_ben = ben.address();
    until(&mut amy, &mut ben, "Amy getting Ben's LXMF message", |a, _| {
        a.app.store.conversations.get(&to_ben).is_some_and(|c| c.messages.iter().any(|m| m.content == "over LXMF"))
    })
    .await;
    ben.line("/clear");
    ben.key(KeyCode::Esc);

    // The members: picked, and what can be done with one.
    ben.key(KeyCode::Char('m'));
    if ben.app.channels.picker.is_some() {
        ben.key(KeyCode::Down);
        ben.key(KeyCode::Up);
        ben.key(KeyCode::Enter);
    }
    if ben.app.channels.menu.is_some() {
        for _ in 0..4 {
            ben.key(KeyCode::Down);
        }
        ben.key(KeyCode::Up);
        ben.key(KeyCode::Esc);
    }
    for action in crate::app::channels::users::UserAction::ALL {
        let amy_identity = amy.app.identity_hash.to_vec();
        ben.app.open_user_menu(&amy_identity);
        let Some(menu) = ben.app.channels.menu.as_mut() else { break };
        let index = crate::app::channels::users::UserAction::ALL.iter().position(|a| *a == action).unwrap();
        menu.list.select(Some(index));
        ben.key(KeyCode::Enter);
        if ben.app.channels.typing {
            ben.key(KeyCode::Esc);
        }
        if ben.app.prompt.is_some() {
            ben.key(KeyCode::Esc);
        }
    }
    // The tab's other keys: joins and dates shown or not, notifications,
    // scrolling, searching, connecting.
    ben.app.channels.selected = Some(crate::app::channels::Target { hub, room: Some("lobby".into()) });
    for code in [
        KeyCode::Char('J'),
        KeyCode::Char('J'),
        KeyCode::Char('D'),
        KeyCode::Char('D'),
        KeyCode::Char('N'),
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Char('y'),
        KeyCode::Down,
        KeyCode::Up,
    ] {
        ben.key(code);
    }
    ben.key(KeyCode::Char('/'));
    ben.line("hello");
    ben.key(KeyCode::Esc);
    ben.key(KeyCode::Char('a'));
    ben.key(KeyCode::Char('a'));

    // Amy, the hub's operator: her panel's actions on Ben.
    let ben_identity = hex::encode(ben.app.identity_hash);
    amy.app.hub_topic("lobby", "Welcome all").unwrap();
    amy.app.hub_modes("lobby", "+m").unwrap();
    amy.app.hub_modes("lobby", "-m").unwrap();
    amy.app.hub_register("lobby", true).unwrap();
    amy.app.hub_run("/stats").unwrap();
    let who = ben_identity.clone();
    amy.app.hub_voice("lobby", &who, true).unwrap();
    amy.app.hub_op("lobby", &who, true).unwrap();
    amy.app.hub_op("lobby", &who, false).unwrap();
    until(&mut amy, &mut ben, "Ben seeing the topic", |_, b| b.heard(hub, "lobby").contains("Welcome all")).await;
    until(&mut amy, &mut ben, "Ben getting the long greeting", |_, b| {
        let index = b.app.channels.hub_index(hub).unwrap();
        b.app.channels.hubs[index].motd.as_deref().is_some_and(|m| m.contains("talking about hills"))
            || b.app.channels.hubs[index].buffers.values().flatten().any(|l| l.text.contains("talking about hills"))
    })
    .await;
    // Ben pings the hub; Amy renames it as it runs (nobody disconnected).
    let ben_index = ben.app.channels.hub_index(hub).unwrap();
    ben.app.channel_submit(ben_index, "lobby", "/ping");
    until(&mut amy, &mut ben, "Ben's pong", |_, b| b.heard(hub, "lobby").contains("Pong from hub")).await;
    amy.app.update_settings(&[("hub_name", "Amy's renamed hub")]).unwrap();
    until(&mut amy, &mut ben, "the hub renamed", |a, _| a.app.hub.replies.iter().any(|(text, _)| text.to_lowercase().contains("reload")))
        .await;
    amy.key(KeyCode::Char('9'));
    for code in [KeyCode::Tab, KeyCode::Down, KeyCode::Up, KeyCode::Tab, KeyCode::Down, KeyCode::BackTab, KeyCode::Char('y')] {
        amy.key(code);
    }
    amy.app.hub_kick("lobby", &who).unwrap();
    amy.app.hub_ban(Some("lobby"), &who, true).unwrap();
    amy.app.hub_ban(Some("lobby"), &who, false).unwrap();
    amy.app.hub_invite("lobby", &who, true).unwrap();
    amy.app.hub_room_key("lobby", Some("secret")).unwrap();
    amy.app.hub_room_key("lobby", None).unwrap();
    amy.app.hub_rename_room("lobby", "hall").unwrap();
    amy.app.hub_delete_room("hall").unwrap();
    amy.app.hub_disconnect(&who).unwrap();
    until(&mut amy, &mut ben, "Ben disconnected by the hub", |_, b| {
        let index = b.app.channels.hub_index(hub).unwrap();
        !matches!(b.app.channels.hubs[index].status, crate::app::channels::HubStatus::Connected)
    })
    .await;
    // Ben's /disconnect and /connect.
    let index = ben.app.channels.hub_index(hub).unwrap();
    ben.app.channel_submit(index, "lobby", "/connect");
    ben.app.channel_submit(index, "lobby", "/disconnect");
    amy.pump();
    ben.pump();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn games_over_the_network_at_the_keyboard() {
    use crossterm::event::KeyCode;
    let (mut amy, mut ben) = pair("games", |_| {}).await;
    let (to_amy, to_ben) = (amy.address(), ben.address());
    // Amy challenges Ben to Tic-Tac-Toe from the Games tab: n, who, which.
    amy.app.add_contact(&to_ben).unwrap();
    amy.key(KeyCode::Char('2'));
    amy.key(KeyCode::Char('n'));
    let who = amy.app.game_menu_items().iter().position(|i| matches!(i, crate::app::games::MenuItem::Contact(k) if *k == to_ben)).unwrap();
    for _ in 0..who {
        amy.key(KeyCode::Down);
    }
    amy.key(KeyCode::Enter);
    amy.key(KeyCode::Enter); // Tic-Tac-Toe, the first
    until(&mut amy, &mut ben, "Ben challenged", |_, b| b.app.games.waiting(None) == 1).await;
    // Ben accepts on the board.
    ben.key(KeyCode::Char('2'));
    ben.key(KeyCode::Enter);
    assert!(ben.app.game_playing);
    ben.key(KeyCode::Char('a'));
    until(&mut amy, &mut ben, "Amy's move", |a, _| a.app.game_list().first().is_some_and(|g| g.yours)).await;
    // Turn about: Amy by cursor, Ben by number, Amy by a click...
    amy.key(KeyCode::Enter);
    assert!(amy.app.game_playing);
    amy.key(KeyCode::Left);
    amy.key(KeyCode::Up);
    amy.key(KeyCode::Enter); // top left
    until(&mut amy, &mut ben, "Ben's move", |_, b| b.app.game_list().first().is_some_and(|g| g.yours)).await;
    ben.key(KeyCode::Char('5'));
    until(&mut amy, &mut ben, "Amy's second move", |a, _| a.app.game_list().first().is_some_and(|g| g.yours)).await;
    amy.draw();
    let board = amy.app.regions.game_board;
    let (w, h) = board.cell;
    amy.app.on_mouse(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: board.origin.x + w, // the top middle
        row: board.origin.y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    until(&mut amy, &mut ben, "Ben's second move", |_, b| b.app.game_list().first().is_some_and(|g| g.yours)).await;
    // Ben offers a draw; Amy takes it.
    ben.key(KeyCode::Char('D'));
    until(&mut amy, &mut ben, "Amy offered a draw", |a, _| a.app.game_list().first().is_some_and(|g| g.draw_offer == Some(true))).await;
    amy.key(KeyCode::Char('y'));
    until(&mut amy, &mut ben, "the game over", |_, b| b.app.game_list().first().is_some_and(|g| g.status == "completed")).await;
    let _ = (h, &to_amy);

    // Chess, typed moves and a resignation; Four in a Row by number.
    let session = amy.app.start_game(&to_ben, "chess").unwrap();
    until(&mut amy, &mut ben, "Ben challenged to chess", |_, b| b.app.games.view("chess", &session).is_some()).await;
    ben.app.game_action("chess", &session, "accept", None).unwrap();
    until(&mut amy, &mut ben, "the chess game begun", |a, _| a.app.games.view("chess", &session).is_some_and(|g| g.status == "active"))
        .await;
    let white_is_amy = amy.app.games.view("chess", &session).unwrap().yours;
    let (white, black) = if white_is_amy { (&mut amy, &mut ben) } else { (&mut ben, &mut amy) };
    white.app.show_game("chess", &session);
    white.key(KeyCode::Enter);
    white.key(KeyCode::Char('t'));
    white.line("e2e4");
    white.key(KeyCode::Esc);
    let _ = black;
    until(&mut amy, &mut ben, "the first chess move arriving", |a, b| {
        let moved = |p: &Peer| {
            p.app
                .games
                .view("chess", &session)
                .is_some_and(|g| matches!(&g.board, crate::games::Board::Chess { moves, .. } if !moves.is_empty()))
        };
        moved(a) && moved(b)
    })
    .await;
    // The black side: the board turned round, the cursor moved, a piece picked.
    let black = if white_is_amy { &mut ben } else { &mut amy };
    black.app.show_game("chess", &session);
    black.key(KeyCode::Enter);
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right, KeyCode::Enter, KeyCode::Enter] {
        black.key(code);
    }
    black.key(KeyCode::Char('R'));
    black.line("y");
    until(&mut amy, &mut ben, "the resignation arriving", |a, b| {
        [a, b].iter().all(|p| p.app.games.view("chess", &session).is_some_and(|g| g.status == "completed"))
    })
    .await;
    let session = ben.app.start_game(&to_amy, "four_in_a_row").unwrap();
    until(&mut amy, &mut ben, "Amy challenged to Four in a Row", |a, _| a.app.games.view("four_in_a_row", &session).is_some()).await;
    amy.app.show_game("four_in_a_row", &session);
    amy.key(KeyCode::Enter);
    amy.key(KeyCode::Char('d')); // declined
    until(&mut amy, &mut ben, "Ben hearing it was declined", |_, b| {
        b.app.games.view("four_in_a_row", &session).is_some_and(|g| g.status == "declined")
    })
    .await;
    // The list: filtered, moved through, a game forgotten.
    amy.key(KeyCode::Esc);
    for code in [
        KeyCode::Char('f'),
        KeyCode::Char('f'),
        KeyCode::Char('f'),
        KeyCode::Char('f'),
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::End,
        KeyCode::Home,
    ] {
        amy.key(code);
    }
    amy.key(KeyCode::Char('x'));
    amy.line("y");
    amy.key(KeyCode::Char('c'));
    assert_eq!(amy.app.tab, crate::app::Tab::Messages);
}

/// Pump one until `done` (or it's taken too long: `what` didn't happen).
async fn until_one(peer: &mut Peer, what: &str, mut done: impl FnMut(&Peer) -> bool) {
    let started = Instant::now();
    loop {
        peer.pump();
        if done(peer) {
            return;
        }
        if started.elapsed() > PATIENCE {
            let log = peer.app.log.iter().rev().take(20).cloned().collect::<Vec<_>>().join("\n  ");
            panic!("{what} didn't happen in time. The log:\n  {log}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_network_actor_command_by_command() {
    use net::NetCommand;
    let port = free_port();
    // Interface discovery on, with an entry point to bring back (one that
    // isn't there) and one that can't be.
    let interfaces = format!(
        "  [[Server]]\n    type = TCPServerInterface\n    listen_ip = 127.0.0.1\n    listen_port = {port}\n\
         \n  [[Entry]]\n    type = TCPClientInterface\n    bootstrap_only = Yes\n    target_host = 127.0.0.1\n    target_port = 1\n    networkname = net\n    passphrase = pass\n\
         \n  [[Odd]]\n    type = UDPInterface\n    bootstrap_only = Yes\n    listen_ip = 127.0.0.1\n    listen_port = {}\n    forward_ip = 127.0.0.1\n    forward_port = {}\n",
        free_port(),
        free_port()
    );
    let mut amy = Peer::start_in("actor", "  discover_interfaces = Yes\n", &interfaces, |s| s.discovered_entry_points = 1);
    until_one(&mut amy, "Amy starting", |a| a.app.lxmf_hash.is_some()).await;
    let me = amy.app.lxmf_hash.unwrap();
    let send = |amy: &mut Peer, command: NetCommand| amy.app.send(command);

    // A paper message to itself, written and read back in.
    let paper = crate::lxmf::paper::Paper { to: me, content: "On paper".into(), timestamp: 1_800_000_000.0, reply: None, format: None };
    let public_key = amy.app.public_key.unwrap();
    send(&mut amy, NetCommand::Remember { to: me, public_key });
    send(&mut amy, NetCommand::WritePaper { id: 1, paper });
    let started = Instant::now();
    let link = loop {
        let mut found = None;
        while let Ok(event) = amy.events.try_recv() {
            match event {
                net::NetEvent::Paper { result, .. } => found = Some(result.expect("written")),
                other => amy.app.on_net(other),
            }
        }
        if let Some((link, _)) = found {
            break link;
        }
        assert!(started.elapsed() < PATIENCE, "the paper message wasn't written");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    send(&mut amy, NetCommand::ReadPaper(link));
    send(&mut amy, NetCommand::ReadPaper("lxm://nonsense".into()));
    until_one(&mut amy, "the paper message read back in", |a| {
        a.app.store.conversations.values().any(|c| c.messages.iter().any(|m| m.content == "On paper"))
    })
    .await;
    // Saying who it is, and what it takes.
    send(&mut amy, NetCommand::SetDisplayName("Amy again".into()));
    send(&mut amy, NetCommand::SetPolicy { stamp_cost: Some(9), max_bytes: 1_000_000 });
    send(&mut amy, NetCommand::SetPolicy { stamp_cost: Some(9), max_bytes: 2_000_000 });
    send(&mut amy, NetCommand::SetContacts { trusted: vec![[5; 16]], exempt: vec![[6; 16]] });
    send(&mut amy, NetCommand::Remember { to: [5; 16], public_key: [1; 64] });
    send(&mut amy, NetCommand::Blackhole { to: [7; 16], block: true, quiet: false });
    send(&mut amy, NetCommand::Blackhole { to: [7; 16], block: false, quiet: true });
    // Timers, and picking a propagation node.
    send(
        &mut amy,
        NetCommand::SetIntervals {
            announce: net::AnnounceSchedule::Every(Duration::from_secs(3600)),
            sync: Some(Duration::from_secs(600)),
        },
    );
    send(&mut amy, NetCommand::SetPropagationNode(Some([8; 16])));
    send(&mut amy, NetCommand::Sync);
    send(&mut amy, NetCommand::AutoPropagation { enabled: true, known: vec![([8; 16], 1, 1_800_000_000)] });
    send(&mut amy, NetCommand::RecheckPropagation);
    send(&mut amy, NetCommand::AutoPropagation { enabled: false, known: Vec::new() });
    send(&mut amy, NetCommand::SetDiscoveredEntryPoints(2));
    send(&mut amy, NetCommand::SetDiscoveredEntryPoints(1));
    // Nothing hosted: said so.
    send(&mut amy, NetCommand::HostReload);
    send(&mut amy, NetCommand::HostAnnounce);
    send(&mut amy, NetCommand::Host(None));
    send(&mut amy, NetCommand::HubAction(crate::rrc::host::HubAction::Announce));
    send(&mut amy, NetCommand::Hub(None));
    send(&mut amy, NetCommand::Propagation(None));
    until_one(&mut amy, "the node and hub said not to be running", |a| {
        a.app.log.iter().any(|l| l.contains("Node announce failed"))
            && a.app.hub.replies.iter().any(|(text, _)| text.contains("isn't running"))
    })
    .await;

    // Bootstrap only: the entry point that can't be brought back is said.
    until_one(&mut amy, "the Bootstrap only note", |a| a.app.log.iter().any(|l| l.contains("Odd is Bootstrap only"))).await;
    // A few looks for entry points (none are found here), once started.
    let started = Instant::now();
    until_one(&mut amy, "a few looks for entry points", |_| started.elapsed() > Duration::from_secs(8)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovered_entry_points_left_to_the_config() {
    // The config connects to what it finds itself, or doesn't look: rettui
    // says why its own setting does nothing.
    for (name, reticulum, why) in [
        ("auto", "  discover_interfaces = Yes\n  autoconnect_discovered_interfaces = 2\n", "connects to entry points it finds itself"),
        ("off", "", "interface discovery is off"),
    ] {
        let mut peer = Peer::start_in(&format!("discovery-{name}"), reticulum, "", |s| s.discovered_entry_points = 1);
        until_one(&mut peer, why, |p| p.app.log.iter().any(|l| l.contains(why))).await;
        peer.app.send(net::NetCommand::SetDiscoveredEntryPoints(3));
        until_one(&mut peer, "the setting said to wait", |p| p.app.log.iter().any(|l| l.contains("takes effect once"))).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_sent_to_an_rncp_listener() {
    use rns_runtime::rncp::{RncpListenerConfig, spawn_rncp_listener};
    use rns_transport::messages::{OutboundRequest, TransportMessage};
    let port = free_port();
    let server = format!(
        "  [[Server]]\n    type = TCPServerInterface\n    enabled = Yes\n    listen_ip = 127.0.0.1\n    listen_port = {port}\n{TAKE_ALL}"
    );
    let mut amy = Peer::start("rncp-amy", &server, |_| {});
    until_one(&mut amy, "Amy starting", |a| a.app.lxmf_hash.is_some()).await;

    // Someone running `rncp -l` on another Reticulum, connected to Amy's:
    // one listener lets Amy send, the other lets no one.
    let dir = std::env::temp_dir().join(format!("rettui-live-rncp-listener-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (rns, saved) = (dir.join("rns"), dir.join("received"));
    std::fs::create_dir_all(&rns).unwrap();
    std::fs::create_dir_all(&saved).unwrap();
    std::fs::write(
        rns.join("config"),
        format!(
            "[reticulum]\n  enable_transport = No\n  share_instance = No\n\n[interfaces]\n  [[To Amy]]\n    type = TCPClientInterface\n    enabled = Yes\n    target_host = 127.0.0.1\n    target_port = {port}\n{TAKE_ALL}"
        ),
    )
    .unwrap();
    let runtime = net::start_runtime(Some(&rns.display().to_string())).await.unwrap();
    let listen = |allowed: Vec<[u8; 16]>| {
        let identity = Identity::new();
        let config = RncpListenerConfig { identity: identity.clone(), save_dir: saved.clone(), allowed, ..RncpListenerConfig::default() };
        let (events, _) = tokio::sync::mpsc::channel(64);
        let transport_tx = runtime.transport_tx.clone();
        async move { (spawn_rncp_listener(transport_tx, config, events).await.unwrap(), identity) }
    };
    let (open, open_identity) = listen(vec![amy.identity.hash]).await;
    let (closed, closed_identity) = listen(Vec::new()).await;
    // Each announces, as `rncp -l -b` does, until Amy has sent.
    let announcing = {
        let transport_tx = runtime.transport_tx.clone();
        let listeners = [(open_identity, open.destination_hash()), (closed_identity, closed.destination_hash())];
        tokio::spawn(async move {
            loop {
                for (identity, hash) in &listeners {
                    let mut destination = rns_identity::destination::Destination::new(
                        Some(identity),
                        rns_identity::destination::Direction::In,
                        rns_identity::destination::DestType::Single,
                        rns_runtime::rncp::DEFAULT_RNCP_APP_NAME,
                    )
                    .unwrap();
                    let raw = destination.announce_packet(identity, None, None, false, None, crate::app::now()).unwrap();
                    let _ =
                        transport_tx.send(TransportMessage::Outbound(OutboundRequest { raw: raw.into(), destination_hash: *hash })).await;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        })
    };

    let to = hex::encode(open.destination_hash());
    amy.app.rncp_send(&to, "notes.txt", b"Meet at the hill at noon".to_vec()).unwrap();
    assert!(amy.app.rncp_send(&to, "notes.txt", b"again".to_vec()).unwrap_err().contains("already on its way"));
    let refused = hex::encode(closed.destination_hash());
    amy.app.rncp_send(&refused, "secret.txt", b"no".to_vec()).unwrap();
    until_one(&mut amy, "both sends finishing", |a| a.app.rncp.iter().all(|s| !matches!(s.state, net::RncpState::Sending(_)))).await;
    assert!(matches!(amy.app.rncp[0].state, net::RncpState::Sent { bytes: 24, .. }), "{:?}", amy.app.rncp[0].state);
    assert_eq!(std::fs::read_to_string(saved.join("notes.txt")).unwrap(), "Meet at the hill at noon");
    assert!(
        matches!(&amy.app.rncp[1].state, net::RncpState::Failed(e) if e.contains("may not allow your identity")),
        "{:?}",
        amy.app.rncp[1].state
    );
    assert!(!saved.join("secret.txt").exists());
    assert!(amy.app.log.iter().any(|l| l.contains("Sent notes.txt (24 B)")), "{:?}", amy.app.log);

    announcing.abort();
    open.shutdown().await;
    closed.shutdown().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_speed_test_between_two_rettuis() {
    let (mut amy, mut ben) = pair("speed", |_| {}).await;
    let (amys, bens) = (amy.app.speed_address(), ben.app.speed_address());
    // Ben takes tests from Amy only; Amy's is the one he entered.
    ben.app.speed.open = true;
    ben.app.speed_set_peer(&hex::encode(amys)).unwrap();
    until(&mut amy, &mut ben, "Ben listening", |_, b| b.app.speed.listening == Some(amys)).await;
    amy.app.speed_set_peer(&hex::encode(bens)).unwrap();
    amy.app.speed_set_size(1).unwrap();
    amy.app.speed_send().unwrap();
    until(&mut amy, &mut ben, "the test arriving", |a, b| {
        a.app.speed.sending.is_none() && b.app.speed.lines.iter().any(|l| l.starts_with("Download"))
    })
    .await;
    assert!(amy.app.speed.lines.iter().any(|l| l.starts_with("Upload") && l.contains("8.2 KB")), "{:?}", amy.app.speed.lines);
    assert!(ben.app.speed.lines.iter().any(|l| l.starts_with("Download") && l.contains("8.2 KB")), "{:?}", ben.app.speed.lines);

    // Someone else's test is turned away: Ben now takes them from another.
    ben.app.speed_set_peer(&"ab".repeat(16)).unwrap();
    until(&mut amy, &mut ben, "Ben listening for someone else", |_, b| b.app.speed.listening.is_some_and(|l| l != amys)).await;
    amy.app.speed_send().unwrap();
    until(&mut amy, &mut ben, "Amy's test refused", |a, _| a.app.speed.sending.is_none()).await;
    assert!(amy.app.speed.lines.back().is_some_and(|l| l.starts_with("Sending failed")), "{:?}", amy.app.speed.lines);
    // Closed, it stops listening.
    ben.app.speed.open = false;
    until(&mut amy, &mut ben, "Ben no longer listening", |_, b| b.app.speed.listening.is_none()).await;
}
