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
