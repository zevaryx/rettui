//! The app's own workings that the terminal UI reaches only through
//! keys: driven directly, on the terminal UI's test app (see
//! [`crate::ui::tui_tests`]).

use crossterm::event::KeyCode;

use crate::app::Tab;
use crate::ui::tui_tests::{BOB, Tui, with_bob};

#[test]
fn picked_messages_retries_paper_and_location_requests() {
    use crate::lxmf::DeliveryMode;
    use crate::lxmf::fields::Command;
    use crate::store::MessageState;
    let mut tui = Tui::new("picked");
    with_bob(&mut tui);
    tui.tab(Tab::Messages);
    tui.app.open_conversation(BOB.into());
    tui.draw();

    // A message picked: each of its keys.
    for keys in [
        &[KeyCode::Char('m'), KeyCode::Char('k'), KeyCode::Char('j'), KeyCode::Char('o')][..],
        &[KeyCode::Char('m'), KeyCode::Up, KeyCode::Char('o')],
        &[KeyCode::Char('m'), KeyCode::Up, KeyCode::Up, KeyCode::Char('o')],
        &[KeyCode::Char('m'), KeyCode::Up, KeyCode::Up, KeyCode::Up, KeyCode::Char('o')],
        &[KeyCode::Char('m'), KeyCode::Char('t')],
        &[KeyCode::Char('m'), KeyCode::Up, KeyCode::Char('e')],
        &[KeyCode::Char('m'), KeyCode::Up, KeyCode::Char('y')],
        &[KeyCode::Char('m'), KeyCode::Char('z'), KeyCode::Esc],
    ] {
        tui.settle();
        tui.app.picked = None;
        for key in keys {
            tui.key(*key);
        }
    }
    // A reaction picked: sent (Bob's message has an LXMF hash).
    tui.settle();
    tui.app.picked = None;
    tui.key(KeyCode::Char('m')).key(KeyCode::Up).key(KeyCode::Up).key(KeyCode::Up).key(KeyCode::Char('e'));
    tui.app.react_with("👍");
    tui.settle();

    // Sending again: each reason it can't, then by paper and directly.
    let retry = |tui: &mut Tui, id: &str, mode: DeliveryMode| tui.app.retry_message(BOB, id, mode);
    assert!(retry(&mut tui, "local-1", DeliveryMode::Propagated).unwrap_err().contains("propagation node"));
    assert!(retry(&mut tui, &"a".repeat(64), DeliveryMode::Direct).unwrap_err().contains("Only your own"));
    assert!(retry(&mut tui, "local-99", DeliveryMode::Direct).unwrap_err().contains("isn't in this conversation"));
    assert!(tui.app.retry_message("nope", "local-1", DeliveryMode::Direct).is_err());
    let failed = |tui: &mut Tui| {
        let conversation = tui.app.store.conversations.get_mut(BOB).unwrap();
        conversation.messages.last_mut().unwrap().state = MessageState::Failed("x".into());
    };
    let reply = crate::store::ReplyTo { hash: "a".repeat(64), quote: Some("Lunch".into()) };
    tui.app.store.conversations.get_mut(BOB).unwrap().messages.last_mut().unwrap().reply = Some(reply);
    failed(&mut tui);
    retry(&mut tui, "local-1", DeliveryMode::Paper).unwrap();
    assert!(retry(&mut tui, "local-1", DeliveryMode::Direct).unwrap_err().contains("Only a message that failed"));
    failed(&mut tui);
    retry(&mut tui, "local-1", DeliveryMode::Direct).unwrap();
    // A paper message written, or not.
    tui.app.on_paper(1, Ok(("lxm://written".into(), [7; 32])));
    assert_eq!(tui.app.store.conversations[BOB].messages.last().unwrap().paper.as_deref(), Some("lxm://written"));
    assert!(tui.app.paper_view.is_some());
    tui.key(KeyCode::Esc).key(KeyCode::Char('P'));
    assert!(tui.app.paper_view.is_some());
    tui.key(KeyCode::Esc);
    tui.app.on_paper(1, Err("no key".into()));
    tui.app.on_paper(99, Err("no such".into()));
    // A message with a location, or an attachment, can't go on paper.
    failed(&mut tui);
    let location = tui.app.store.conversations[BOB].messages[2].location;
    tui.app.store.conversations.get_mut(BOB).unwrap().messages.last_mut().unwrap().location = location;
    assert!(retry(&mut tui, "local-1", DeliveryMode::Paper).unwrap_err().contains("text only"));
    // Read in: someone else's, or not one.
    tui.app.lxmf_hash = Some([0x11; 16]);
    assert!(tui.app.read_paper("nonsense").is_err());
    tui.app.paper_read = Some(BOB.into());
    tui.app.open_paper_read();
    tui.app.store.conversations.get_mut(BOB).unwrap().messages.iter_mut().for_each(|m| m.paper = None);
    tui.settle().key(KeyCode::Char('P'));

    // Drafts stay with the conversation they were written in.
    tui.app.add_contact(&"cd".repeat(16)).unwrap();
    tui.app.open_conversation(BOB.into());
    tui.key(KeyCode::Enter).keys("half written").key(KeyCode::Esc);
    tui.app.open_conversation("cd".repeat(16));
    assert!(tui.app.compose.text().is_empty());
    tui.app.open_conversation(BOB.into());
    assert_eq!(tui.app.compose.text(), "half written");
    // A folder isn't attached.
    tui.app.add_attachment(tui.dir.to_str().unwrap());
    assert!(tui.said().contains("Not a file"), "{}", tui.said());

    // Asked for this station's location: answered as the setting says.
    let ask = |tui: &mut Tui, n: u8| {
        let mut message = crate::lxmf::InboundMessage {
            id: Some([n; 32]),
            source: [0xab; 16],
            timestamp: 1_800_000_100.0 + f64::from(n),
            verified: true,
            ..Default::default()
        };
        message.extras.commands = vec![Command::TelemetryRequest, Command::Ping];
        tui.app.on_message(message);
        tui.app.store.conversations[BOB].messages.last().unwrap().notes.join(" ")
    };
    tui.app.settings.location_requests = "off".into();
    assert!(ask(&mut tui, 50).contains("Location requests is off"));
    tui.app.settings.location_requests = "trusted".into();
    assert!(ask(&mut tui, 51).contains("trusted only"));
    tui.app.settings.location_requests = "contacts".into();
    assert!(ask(&mut tui, 52).contains("Location isn't set"));
    tui.app.settings.location = Some("51.5, -0.1".into());
    tui.app.settings.answer_commands = "contacts".into();
    assert!(ask(&mut tui, 53).contains("Answered with this station's location"));
    assert!(ask(&mut tui, 54).contains("less than a minute ago"));
}

#[test]
fn every_channel_command_said_well_and_badly() {
    use crate::app::channels::HubStatus;
    use crate::ui::tui_tests::{CAROL, with_hilltop};
    let mut tui = Tui::new("commands");
    let hub = with_hilltop(&mut tui);
    let index = tui.app.channels.hub_index(hub).unwrap();
    let whisper = format!("@{}", hex::encode(CAROL));
    let say = |tui: &mut Tui, room: &str, text: &str| {
        tui.app.channel_submit(index, room, text);
    };
    let errors = |tui: &Tui, room: &str| {
        tui.app.channels.hubs[index]
            .buffers
            .get(room)
            .map_or(0, |lines| lines.iter().filter(|l| l.kind == crate::app::channels::LineKind::Error).count())
    };

    for text in [
        "/help",
        "/join",
        "/join #garden secret",
        "/me",
        "/me waves",
        "/nick",
        "/nick bad\nnick",
        "/nick Tui2",
        "/nick",
        "/ping",
        "/list",
        "/quote",
        "/quote /stats",
        "/who",
        "/names lobby",
        "/topic Hills",
        "/msg",
        "/msg nobody hi",
        "/msg carol",
        "/msg carol hello",
        "/dm",
        "/dm nobody",
        "/dm carol",
        "/dm carol over LXMF",
        "/clear",
    ] {
        tui.app.tab = Tab::Channels;
        say(&mut tui, "lobby", text);
    }
    let long = "x".repeat(tui.app.channels.hubs[index].limits.max_msg_bytes + 10);
    assert!(tui.app.channel_submit(index, "lobby", &long).is_some());
    assert!(tui.app.channel_submit(index, &whisper, &long).is_some());
    say(&mut tui, "lobby", &format!("/me {long}"));
    // In a whisper conversation, room commands make no sense.
    say(&mut tui, &whisper, "/me hi");
    say(&mut tui, &whisper, "/quote hi");
    say(&mut tui, &whisper, "a whisper");
    assert!(errors(&tui, &whisper) >= 1);
    // The hub's own buffer: not a room to talk in.
    say(&mut tui, "", "hello");
    say(&mut tui, "", "/part");
    assert!(errors(&tui, "") >= 2);
    // Private notices where the hub doesn't pass them.
    tui.app.channels.hubs[index].direct_notices = false;
    say(&mut tui, "lobby", "/msg carol hi");
    // Disconnected: talking connects; hub commands wait.
    tui.app.channels.hubs[index].status = HubStatus::Disconnected;
    say(&mut tui, "lobby", "/stats");
    say(&mut tui, "lobby", "anyone?");
    say(&mut tui, "lobby", "/disconnect");
    say(&mut tui, "lobby", "/connect");
    say(&mut tui, "lobby", "/part");
    tui.app.channels.hubs[index].status = HubStatus::Connected;
    tui.app.confirm_split(hub, "lobby", &["one".into(), "two".into()]);
    tui.app.confirm_split(hub, &whisper, &["one".into()]);
    tui.app.confirm_split([9; 16], "lobby", &["none".into()]);
    assert!(errors(&tui, "lobby") >= 3, "{}", errors(&tui, "lobby"));
}

#[test]
fn markdown_as_html_with_every_kind_of_markup() {
    let html = crate::markdown::html(
        "# One\n## Two\n### Three\n\n> quoted\n\n```\ncode\n```\n\n    indented\n\n1. first\n2. second\n\n- a\n- b\n\n\
         *em* **strong** ~~gone~~ `inline`  \nbreak\n\n---\n\n[safe](https://example.org) [unsafe](javascript:alert(1)) ![pic](https://e.org/p.png) <b>raw</b>",
    );
    for part in [
        "md-h1",
        "md-h2",
        "md-h3",
        "<blockquote>",
        "<pre><code>",
        "<ol start=\"1\">",
        "<ul>",
        "<li>",
        "<em>",
        "<strong>",
        "<del>",
        "<code>",
        "<br>",
        "<hr>",
        "href=\"https://example.org\"",
        "md-link",
        "&lt;b&gt;",
    ] {
        assert!(html.contains(part), "{part}: {html}");
    }
}

#[test]
fn forwarding_and_exporting_a_conversation() {
    use crate::store::{Message, MessageState};
    let mut tui = Tui::new("forwarding");
    with_bob(&mut tui);
    let carol = "cd".repeat(16);
    tui.app.add_contact(&carol).unwrap();
    // What can't be forwarded.
    assert!(tui.app.forward_message(BOB, "nope", &carol).unwrap_err().contains("isn't here"));
    let empty = Message { id: "local-9".into(), state: MessageState::Delivered, timestamp: 1.0, ..Message::default() };
    tui.app.store.conversations.get_mut(BOB).unwrap().messages.push(empty);
    assert!(tui.app.forward_message(BOB, "local-9", &carol).unwrap_err().contains("nothing in that message"));
    // A picture's file gone: said.
    let gone = tui.dir.join("gone.png");
    let mut with_file = tui.app.store.conversations[BOB].messages[1].clone();
    with_file.id = "e".repeat(64);
    with_file.attachments[0].path = gone;
    tui.app.store.conversations.get_mut(BOB).unwrap().messages.push(with_file);
    assert!(tui.app.forward_message(BOB, &"e".repeat(64), &carol).unwrap_err().contains("Couldn't copy"));

    // The picker: typed into, pasted into, moved through, picked.
    tui.tab(Tab::Messages);
    tui.app.open_newest_conversation();
    tui.app.forward = None;
    tui.key(KeyCode::Char('m')).key(KeyCode::Up).key(KeyCode::Up).key(KeyCode::Up).key(KeyCode::Up).key(KeyCode::Char('f'));
    if tui.app.forward.is_some() {
        tui.keys("cd").key(KeyCode::Down).key(KeyCode::Up).ctrl('v');
        tui.app.on_paste("cdcd\nline");
        tui.app.forward.as_mut().unwrap().input = crate::term::input::TextInput::with_text(&format!("lxmf@{}", "ef".repeat(16)));
        assert_eq!(tui.app.forward_targets()[0], "ef".repeat(16));
        tui.key(KeyCode::Enter);
        tui.key(KeyCode::Esc);
    }
    // Exported with every state and what came with each.
    let mut odd = tui.app.store.conversations[BOB].messages[0].clone();
    odd.id = "f".repeat(64);
    odd.title = "A title".into();
    odd.state = MessageState::Received { verified: false };
    odd.notes = vec!["a note".into()];
    odd.reactions = vec![crate::store::Reaction {
        emoji: "👍".into(),
        incoming: false,
        timestamp: 2.0,
        id: "local-3".into(),
        state: MessageState::Delivered,
    }];
    let sending = Message { id: "local-8".into(), content: "on its way".into(), timestamp: 3.0, ..Message::default() };
    let conversation = tui.app.store.conversations.get_mut(BOB).unwrap();
    conversation.messages.push(odd);
    conversation.messages.push(sending);
    let path = tui.app.export_conversation(BOB).unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    for part in
        ["A title", "(unverified)", "(not sent yet)", "(not sent: no path)", "[a note]", "[file: cat.png]", "[location:", "👍 (You)"]
    {
        assert!(text.contains(part), "{part}: {text}");
    }
}

#[test]
fn sharing_locations_live_and_the_map_by_keys() {
    use crate::app::PromptKind;
    use crate::app::live::Source;
    use crate::lxmf::{DeliveryMode, Location};
    let mut tui = Tui::new("live");
    with_bob(&mut tui);
    tui.tab(Tab::Messages);
    tui.app.open_newest_conversation();
    // A location typed, and each way that isn't one.
    let ask = |tui: &mut Tui, text: &str| tui.ask(PromptKind::ShareLocation(BOB.into()), text).said();
    assert!(ask(&mut tui, "nowhere").contains("A location is latitude"));
    assert!(ask(&mut tui, "live soon").contains("How long to share live"));
    assert!(ask(&mut tui, "live 1h").contains("Set this station's Location"));
    tui.ask(PromptKind::ShareLocation(BOB.into()), "");
    tui.app.delivery_mode = DeliveryMode::Paper;
    assert!(ask(&mut tui, "51.5, -0.1").contains("paper message carries text only"));
    tui.app.delivery_mode = DeliveryMode::Auto;
    ask(&mut tui, "51.5, -0.1");
    assert!(tui.app.store.conversations[BOB].messages.last().unwrap().location.is_some());
    // This station's, live: for a while, then until stopped.
    tui.app.settings.location = Some("51.5, -0.1".into());
    ask(&mut tui, "live 15m");
    assert!(tui.app.live_share(BOB).unwrap().until.is_some());
    assert!(tui.app.start_live(BOB, 100_000, Source::Station).unwrap_err().contains("a week"));
    ask(&mut tui, "live on");
    let share = tui.app.live_share(BOB).unwrap();
    assert_eq!(crate::app::live::label(share), "📍 live");
    assert_eq!(crate::app::live::describe(BOB, share)["source"], "station");
    tui.settle().key(KeyCode::Char('L'));
    assert!(matches!(tui.app.prompt.as_ref().map(|p| &p.kind), Some(PromptKind::ConfirmStopLive(_))));
    tui.answer("y");
    assert!(tui.app.stop_live(BOB).unwrap_err().contains("aren't sharing"));
    // A device's, as it moves; a share that's ended stops by itself.
    tui.app.start_live(BOB, 0, Source::Device).unwrap();
    tui.app.device_position(Location::parse("51.6, -0.2").unwrap());
    tui.app.live.shares.get_mut(BOB).unwrap().until = Some(1.0);
    tui.app.on_tick();
    assert!(tui.app.live_share(BOB).is_none());
    // Propagated with no node: said.
    tui.app.store.contacts.entry(BOB.into()).or_default().delivery = Some(DeliveryMode::Propagated);
    tui.app.start_live(BOB, 0, Source::Station).unwrap();
    tui.app.stop_live(BOB).unwrap();
    assert!(tui.said().contains("no propagation node"), "{}", tui.said());
    tui.app.store.contacts.entry(BOB.into()).or_default().delivery = None;

    // The map, by its keys.
    tui.settle().key(KeyCode::Char('M'));
    for key in ['j', 'k', '+', '+', '=', '-', '-', '0', 'a', 'o'] {
        tui.key(KeyCode::Char(key));
    }
    tui.key(KeyCode::Home).key(KeyCode::End).key(KeyCode::Down).key(KeyCode::Up).key(KeyCode::Enter);
    tui.settle().key(KeyCode::Char('M')).key(KeyCode::Char('q'));
    let mut empty = Tui::new("no-map");
    empty.tab(Tab::Messages).key(KeyCode::Char('M'));
    assert!(empty.app.map.is_none());
}

/// Sideband's commands sent: typed after `K`, and the contact card's "Ask
/// where they are"; each goes as a message with no text, and says what it
/// asked.
#[test]
fn sending_sideband_commands_by_keys_and_the_contact_card() {
    use crate::app::PromptKind;
    use crate::lxmf::DeliveryMode;
    use crate::lxmf::fields::Command;
    use crate::net::NetCommand;
    let mut tui = Tui::new("send-commands");
    with_bob(&mut tui);
    tui.tab(Tab::Messages);
    tui.app.open_newest_conversation();
    tui.sent();
    tui.settle().key(KeyCode::Char('K'));
    assert!(matches!(tui.app.prompt.as_ref().map(|p| &p.kind), Some(PromptKind::Command(_))));
    tui.answer("dance");
    assert!(tui.said().contains("ping, echo <text>, signal or location"), "{}", tui.said());
    let sent = |tui: &mut Tui| -> Vec<(Vec<Command>, String)> {
        tui.sent()
            .into_iter()
            .filter_map(|c| match c {
                NetCommand::SendMessage { message, .. } => Some((message.commands.clone(), message.content.clone())),
                _ => None,
            })
            .collect()
    };
    assert!(sent(&mut tui).is_empty());
    // Paper carries text only, so a command goes the usual way.
    tui.app.delivery_mode = DeliveryMode::Paper;
    tui.ask(PromptKind::Command(BOB.into()), "echo are you there?");
    assert_eq!(sent(&mut tui), [(vec![Command::Echo("are you there?".into())], String::new())]);
    let last = tui.app.store.conversations[BOB].messages.last().unwrap().clone();
    assert!(!last.incoming && last.content.is_empty());
    assert_eq!(last.notes, ["Asked for an echo of “are you there?”"]);
    assert!(tui.draw().contains("Asked for an echo"), "{}", tui.draw());
    tui.app.delivery_mode = DeliveryMode::Auto;
    tui.ask(PromptKind::Command(BOB.into()), "sig");
    assert_eq!(sent(&mut tui)[0].0, [Command::SignalReport]);
    // The contact card: a location request, and a command typed.
    tui.settle().key(KeyCode::Char('c')).key(KeyCode::Char('w'));
    assert_eq!(sent(&mut tui)[0].0, [Command::TelemetryRequest]);
    let notice = tui.app.notice.as_ref().map(|n| n.text.clone()).unwrap_or_default();
    assert!(notice.contains("their answer goes on the map"), "{notice}");
    tui.key(KeyCode::Char('k'));
    assert!(matches!(tui.app.prompt.as_ref().map(|p| &p.kind), Some(PromptKind::Command(_))));
    tui.answer("ping");
    assert_eq!(sent(&mut tui)[0].0, [Command::Ping]);
    assert!(tui.app.send_command("nobody", Command::Ping).is_err());
    // One that failed goes again, as it was.
    let id = tui.app.store.conversations[BOB].messages.last().unwrap().id.clone();
    tui.app.store.conversations.get_mut(BOB).unwrap().find_mut(&id).unwrap().state = crate::store::MessageState::Failed("no path".into());
    tui.app.retry_message(BOB, &id, DeliveryMode::Paper).unwrap();
    assert_eq!(sent(&mut tui), [(vec![Command::Ping], String::new())]);
    // Through a propagation node, there must be one.
    tui.app.delivery_mode = DeliveryMode::Propagated;
    assert!(tui.app.send_command(BOB, Command::Ping).unwrap_err().contains("propagation node"));
}

/// Tracking someone from their contact card (`f`): asked every so many
/// minutes, said in the conversation's header, and stopped the same way.
#[test]
fn tracking_a_contact_from_the_card() {
    use crate::app::PromptKind;
    use crate::app::contacts::CardAction;
    use crate::lxmf::fields::Command;
    use crate::net::NetCommand;
    let mut tui = Tui::new("track");
    with_bob(&mut tui);
    tui.tab(Tab::Messages);
    tui.app.open_newest_conversation();
    tui.sent();
    tui.settle().key(KeyCode::Char('c'));
    assert!(tui.app.card_actions(BOB).contains(&CardAction::Track));
    tui.key(KeyCode::Char('f'));
    assert!(matches!(tui.app.prompt.as_ref().map(|p| &p.kind), Some(PromptKind::Track(_))));
    tui.answer("often");
    assert!(tui.said().contains("number of minutes"), "{}", tui.said());
    let shown = tui.app.store.conversations[BOB].messages.len();
    tui.ask(PromptKind::Track(BOB.into()), "10");
    let notice = tui.app.notice.as_ref().map(|n| n.text.clone()).unwrap_or_default();
    assert!(notice.contains("every 10 min"), "{notice}");
    let asked: Vec<NetCommand> = tui.sent();
    assert!(asked.iter().any(|c| matches!(c, NetCommand::SendMessage { message, .. } if message.commands == [Command::TelemetryRequest])));
    assert_eq!(tui.app.store.conversations[BOB].messages.len(), shown, "requests aren't shown");
    tui.app.contact_card = None;
    assert!(tui.draw().contains("asking every 10 min"), "{}", tui.draw());
    tui.settle().key(KeyCode::Char('c'));
    assert!(tui.app.card_actions(BOB).contains(&CardAction::StopTracking));
    tui.key(KeyCode::Char('f'));
    assert!(tui.app.track(BOB).is_none());
}

/// A message being sent says how it goes, and can be cancelled (`c` on a
/// picked one); cancelled, it isn't sent again by itself, but can be by
/// hand. A sync says how many of those waiting came.
#[test]
fn send_progress_cancelling_and_sync_progress() {
    use crate::app::SyncState;
    use crate::lxmf::{CANCELLED, DeliveryMode, WAITING_FOR_DELIVERY};
    use crate::net::{NetCommand, NetEvent};
    use crate::store::MessageState;
    let mut tui = Tui::new("cancel");
    with_bob(&mut tui);
    tui.tab(Tab::Messages);
    tui.app.open_newest_conversation();
    tui.sent();
    let id = tui.app.send_message(BOB.into(), "a long one".into(), Vec::new(), DeliveryMode::Auto, None).unwrap();
    let local = format!("local-{id}");
    assert!(tui.sent().iter().any(|c| matches!(c, NetCommand::SendMessage { id: sent, .. } if *sent == id)));
    tui.app.on_net(NetEvent::SendProgress { id, text: "42%".into() });
    assert_eq!(tui.app.send_progress_of(&local), Some("42%"));
    assert!(tui.draw().contains("sending… 42%"), "{}", tui.draw());
    // Picked, then cancelled.
    tui.settle().key(KeyCode::Char('m'));
    while tui.app.picked_message().is_some_and(|(_, m)| m.id != local) {
        tui.key(KeyCode::Down);
    }
    tui.key(KeyCode::Char('c'));
    assert!(tui.sent().iter().any(|c| matches!(c, NetCommand::CancelSend(sent) if *sent == id)));
    // It says it's being cancelled until the send ends, whatever else it hears.
    tui.app.on_net(NetEvent::SendProgress { id, text: "43%".into() });
    assert_eq!(tui.app.send_progress_of(&local), Some("cancelling"));
    assert!(tui.app.cancel_send(BOB, &local).unwrap_err().contains("being cancelled already"));
    tui.app.on_net(NetEvent::Delivery { id, result: Err(CANCELLED.into()) });
    assert_eq!(tui.app.send_progress_of(&local), None);
    assert!(tui.draw().contains(" cancelled"), "{}", tui.draw());
    assert!(tui.app.cancel_send(BOB, &local).unwrap_err().contains("Only a message being sent"));
    assert!(tui.app.cancel_send(BOB, "aa").is_err());
    // Not sent again when they announce, but by hand, yes.
    tui.app.settings.resend_on_announce = true;
    tui.app.resend_on_announce(BOB);
    assert!(!tui.sent().iter().any(|c| matches!(c, NetCommand::SendMessage { id: sent, .. } if *sent == id)));
    tui.app.retry_message(BOB, &local, DeliveryMode::Auto).unwrap();
    assert!(tui.sent().iter().any(|c| matches!(c, NetCommand::SendMessage { id: sent, .. } if *sent == id)));
    // One packet on its way can't be called back.
    tui.app.on_net(NetEvent::SendProgress { id, text: WAITING_FOR_DELIVERY.into() });
    assert!(tui.app.cancel_send(BOB, &local).unwrap_err().contains("can't be called back"));
    tui.app.on_net(NetEvent::Delivery { id, result: Err("no path".into()) });
    let message = tui.app.store.conversations[BOB].messages.iter().find(|m| m.id == local).unwrap();
    assert_eq!(message.state, MessageState::Failed("no path".into()));

    // A sync says how it's going, in Status.
    tui.app.on_net(NetEvent::SyncStarted);
    tui.app.on_net(NetEvent::SyncProgress("3 of 10 downloaded".into()));
    tui.settle().tab(Tab::Status);
    assert!(tui.draw().contains("3 of 10 downloaded"), "{}", tui.draw());
    tui.app.on_net(NetEvent::Synced(Ok(10)));
    assert!(matches!(tui.app.sync, SyncState::Done(..)) && tui.app.sync_progress.is_none());
}

#[test]
fn every_setting_changed_here_and_from_the_web() {
    let mut tui = Tui::new("every-setting");
    with_bob(&mut tui);
    tui.app.channels.hubs.push(crate::app::channels::Hub::new([1; 16], "rrc".into(), "Hub".into()));
    // Each changed (a switch the other way, a number one more), from the
    // terminal UI, then back from the web UI where it may be.
    for field in crate::config::FIELDS {
        let before = tui.app.settings.field_value(field.key);
        let changed = match before.as_str() {
            "true" => "false".to_string(),
            "false" => "true".to_string(),
            number if number.parse::<u64>().is_ok() => (number.parse::<u64>().unwrap() + 1).to_string(),
            _ => continue,
        };
        let _ = tui.app.update_settings(&[(field.key, &changed)]);
        let _ = tui.app.update_settings_from_web(&[(field.key, &before)]);
        let _ = tui.app.update_settings_from_web(&[(field.key, &changed)]);
    }
    // Text ones: an icon, a home page, a time and date style.
    for (key, value) in [
        ("icon", "account"),
        ("icon_color", "#ff0000"),
        ("home", &format!("{}:/page/index.mu", "cd".repeat(16))),
        ("answer_commands", "contacts"),
        ("unknown_senders", "requests"),
    ] {
        let _ = tui.app.update_settings(&[(key, value)]);
    }
    assert!(tui.app.update_settings(&[("no_such_setting", "1")]).is_err());
    tui.app.update_setting("display_name", "Renamed");
    tui.app.update_setting("stamp_cost", "not a number");
    assert_eq!(tui.app.settings.display_name, "Renamed");
}

#[test]
fn a_contact_card_every_button_and_ping_answers() {
    use crate::app::contacts::{CardAction, PingState, ping_label, trust_label};
    use crate::net::NetEvent;
    use crate::net::remote::Ping;
    use crate::store::Trust;
    let mut tui = Tui::new("card");
    with_bob(&mut tui);
    tui.tab(Tab::Messages);
    for action in [
        CardAction::Rename,
        CardAction::Notes,
        CardAction::Copy,
        CardAction::Ping,
        CardAction::Ping,
        CardAction::Trust,
        CardAction::Untrust,
        CardAction::LeaveAsIs,
        CardAction::Block,
        CardAction::Unblock,
        CardAction::DeleteConversation,
        CardAction::Close,
    ] {
        assert!(!action.label().0.is_empty());
        tui.app.contact_card = Some(BOB.into());
        tui.app.prompt = None;
        tui.app.card_action(action);
    }
    tui.app.block_contact(BOB).unwrap();
    tui.app.contact_card = Some(BOB.into());
    tui.app.card_action(CardAction::Unblock);
    tui.app.contact_card = Some(BOB.into());
    tui.app.card_action(CardAction::Unblock);
    assert!(tui.app.unblock_contact(BOB).unwrap_err().contains("aren't blocked"));
    assert!(tui.app.set_contact_notes(BOB, &"x".repeat(crate::store::MAX_NOTES + 1)).is_err());
    tui.app.lxmf_hash = Some([0x11; 16]);
    assert!(tui.app.add_contact(&"11".repeat(16)).unwrap_err().contains("own address"));
    // Every trust, and the buttons each has.
    for trust in [Trust::Trusted, Trust::Blocked, Trust::Untrusted, Trust::Unknown] {
        assert!(!trust_label(trust, false).is_empty() && !trust_label(trust, true).is_empty());
        tui.app.store.update_contact(BOB, |c| c.trust = trust);
        assert!(tui.app.card_actions(BOB).contains(&CardAction::Close));
    }
    tui.app.set_trust(BOB, Trust::Blocked).unwrap();
    tui.app.set_trust(BOB, Trust::Trusted).unwrap();
    // Pings answered, near and far, and not.
    let hash = [0xab; 16];
    for (hops, result) in [(Some(1), true), (Some(3), true), (None, true), (None, false)] {
        let result =
            if result { Ok(Ping { rtt: std::time::Duration::from_millis(50), hops, signal: None }) } else { Err("no answer".into()) };
        tui.app.on_net(NetEvent::Pinged { to: hash, result });
    }
    assert_eq!(ping_label(&PingState::Waiting), "waiting for an answer…");
    tui.app.reapply_blocks();
}
