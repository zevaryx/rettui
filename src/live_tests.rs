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
        app.updates.fetch = || Err("no update checks in tests".into());
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
    let server =
        format!("  [[Server]]\n    type = TCPServerInterface\n    enabled = Yes\n    listen_ip = 127.0.0.1\n    listen_port = {port}\n");
    let client =
        format!("  [[To Amy]]\n    type = TCPClientInterface\n    enabled = Yes\n    target_host = 127.0.0.1\n    target_port = {port}\n");
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
    until(&mut amy, &mut ben, "Ben loading Amy's page", |_, b| b.app.browser.loading.is_none() && b.app.browser.source.is_some()).await;
    assert!(ben.app.browser.source.as_deref().unwrap().contains("Amy's node"), "{:?}", ben.app.browser.source);
    // A page that isn't there.
    ben.app.navigate(crate::app::resolve_url(&format!("{node}:/page/nope.mu"), None).unwrap());
    until(&mut amy, &mut ben, "Ben told the page isn't there", |_, b| b.app.browser.loading.is_none()).await;

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
    let server = format!("  [[Server]]\n    type = TCPServerInterface\n    listen_ip = 127.0.0.1\n    listen_port = {port}\n");
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
             target_host = 127.0.0.1\n    target_port = {port}\n"
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
