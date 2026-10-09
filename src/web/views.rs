//! JSON snapshots of the app state, shaped for the web UI's screens.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use crate::app::channels::{Hub, HubStatus, LineKind};
use crate::app::node::NodeStatus;
use crate::app::{App, NetState, SyncState, network};
use crate::config::{Effect, FIELDS, FieldKind, Section, Settings, WebAccess};
use crate::net::PeerKind;
use crate::rrc;
use crate::store::{Conversation, Message, MessageState, NotifyLevel};

fn kind_name(kind: PeerKind) -> &'static str {
    match kind {
        PeerKind::Lxmf => "lxmf",
        PeerKind::Nomad => "nomad",
        PeerKind::Propagation => "propagation",
    }
}

/// Identity, network and sync state, the log, and the sidebar badges.
pub fn state(app: &App) -> Value {
    let net = match &app.net_state {
        NetState::Starting => json!({ "state": "starting" }),
        NetState::Online => json!({ "state": "online" }),
        NetState::Failed(e) => json!({ "state": "failed", "error": e }),
    };
    let sync = match &app.sync {
        SyncState::Idle => json!({ "state": "idle" }),
        SyncState::Running(started) => json!({ "state": "running", "secs": started.elapsed().as_secs() }),
        SyncState::Done(at, Ok(n)) => json!({ "state": "done", "at": crate::clock::time(at, false), "count": n }),
        SyncState::Done(at, Err(e)) => json!({ "state": "failed", "at": crate::clock::time(at, false), "error": e }),
    };
    let propagation = app.settings.propagation_node.as_ref().map(|hash| json!({ "hash": hash, "name": app.store.display_name(hash) }));
    // How it was picked, when it's picked automatically.
    let auto_propagation = app.auto_pick_label();
    let hosting = match &app.pn.status {
        NodeStatus::Off => json!({ "state": "off" }),
        NodeStatus::Starting => json!({ "state": "starting" }),
        NodeStatus::Running => json!({ "state": "running", "hash": hex::encode(app.pn.hash), "stats": app.pn.stats }),
        NodeStatus::Failed(e) => json!({ "state": "failed", "error": e }),
    };
    // The RRC hub hosted here (the Hub section has the rest).
    let hub = match &app.hub.status {
        NodeStatus::Off => json!({ "state": "off" }),
        NodeStatus::Starting => json!({ "state": "starting" }),
        NodeStatus::Running => json!({
            "state": "running",
            "hash": app.hub.address.map(|(hash, _)| hex::encode(hash)),
            "people": app.hub.snapshot.people.len(),
            "rooms": app.hub.snapshot.rooms.len(),
        }),
        NodeStatus::Failed(e) => json!({ "state": "failed", "error": e }),
    };
    let message_unread = app.unread_messages();
    let (channel_unread, mention) = app.channels.total_unread();
    let online = app.interfaces.iter().filter(|i| i.online).count();
    json!({
        "display_name": app.settings.display_name,
        "icon": icon(app.own_appearance().as_ref()),
        "external_shared_instance": app.uses_external_shared_instance(),
        "wrap_lines": app.settings.wrap_lines,
        // How times and dates read (see `clock`).
        "clock": app.settings.clock,
        "date_style": app.settings.date_style,
        // How big pictures sent may be (the composer says they'll shrink).
        "picture_size": app.settings.picture_size,
        // A newer release, if one's out and update checks are on.
        "update": app.update_available().map(|r| {
            let how = match &app.updates.install {
                crate::update::install::Install::InPlace { .. } => None,
                crate::update::install::Install::Elsewhere(how) => Some(how.clone()),
            };
            json!({
                "version": r.version, "url": r.url,
                // Whether Install can put it in this one's place, else how
                // to update instead; and how installing it is going.
                "installable": how.is_none(), "how": how,
                "installing": app.updates.installing(),
                "installed": app.updates.installed.as_deref() == Some(r.version.as_str()),
            })
        }),
        // This build, and what's known of newer releases.
        "version": {
            "current": crate::update::VERSION,
            "build": crate::update::install::build_label(),
            "status": app.version_status(),
            "checking": app.updates.checking(),
            "error": app.updates.error.is_some(),
            "newer": app.update_available().is_some(),
        },
        // This station's location, to share (see the Location setting).
        "location": app.settings.own_location().map(|l| json!({ "latitude": l.latitude, "longitude": l.longitude })),
        // Locations shared live, and from where (a browser posts its
        // device's position while any is the device's).
        "live": app.live.shares.iter().map(|(key, share)| crate::app::live::describe(key, share)).collect::<Vec<_>>(),
        "lxmf_address": app.lxmf_hash.map(hex::encode),
        // With the public key, for others to add you (`lxma://`).
        "identity_link": app.identity_link(),
        "net": net,
        "sync": sync,
        "propagation_node": propagation,
        "auto_propagation": auto_propagation,
        // A new install: the getting-started guide opens by itself.
        "welcome": !app.settings.welcomed,
        // The propagation node hosted here.
        "hosting": hosting,
        "hub": hub,
        "rns_config": app.settings.rns_config,
        "data_dir": app.paths.store.parent().map(|p| p.display().to_string()),
        "known": app.store.peers.len(),
        "interfaces": app.interfaces.iter().map(|i| json!({
            "name": i.name, "online": i.online, "rx": i.rx_bytes, "tx": i.tx_bytes,
            // Its rate, MTU and the rest, as rnstatus shows them.
            "details": i.details(),
            // Bytes per second in and out at each update, oldest first.
            "history": app.traffic.history.get(&i.name).map(|history| {
                history.iter().map(|(rx, tx)| [rx.round(), tx.round()]).collect::<Vec<_>>()
            }),
        })).collect::<Vec<_>>(),
        "interfaces_online": online,
        // Until they're all taken.
        "first_steps": app.first_steps().iter().map(|step| json!({
            "label": step.label, "done": step.done, "how": step.how_web,
        })).collect::<Vec<_>>(),
        // Over all interfaces: totals, and bytes per second once known.
        "traffic": {
            "rx": app.traffic.rx_total,
            "tx": app.traffic.tx_total,
            "rx_rate": app.traffic.rates.map(|r| r.0),
            "tx_rate": app.traffic.rates.map(|r| r.1),
        },
        "log": app.log.iter().collect::<Vec<_>>(),
        "unread": { "messages": message_unread, "channels": channel_unread, "mention": mention },
    })
}

/// The getting-started guide: what's set up, the choices to start from,
/// and its words and links.
pub fn guide(app: &App) -> Value {
    use crate::app::guide;
    let view = app.guide_view();
    let defaults = app.guide_defaults();
    json!({
        "name": view.name,
        // Each entry point offered, whether the config has it, whether
        // it's ticked to start with, and whether it answered.
        "entries": guide::ENTRY_POINTS.iter().enumerate().map(|(i, entry)| json!({
            "region": entry.region, "name": entry.name, "host": entry.host, "port": entry.port,
            "present": view.has_entry_point[i], "on": defaults.connect[i],
            "reach": reach(view.reach[i]),
        })).collect::<Vec<_>>(),
        "has_discovery": view.has_discovery,
        "external": view.external,
        "external_note": crate::reticulum::EXTERNAL_NOTE,
        "shared_note": (view.shared_config && !view.external).then_some(guide::SHARED_NOTE),
        "identity_file": app.paths.identity.display().to_string(),
        "interfaces_online": view.interfaces_online,
        "heard": view.heard,
        "defaults": {
            "discover": defaults.discover, "auto_propagation": defaults.auto_propagation,
            "update_check": defaults.update_check,
        },
        "help": {
            "intro": guide::INTRO, "name": guide::NAME_HELP, "connect": guide::CONNECT_HELP,
            "identity": guide::IDENTITY_WEB_NOTE,
            "discover": guide::DISCOVER_HELP, "auto_propagation": guide::AUTO_PROPAGATION_HELP,
            "auto_propagation_warning": guide::AUTO_PROPAGATION_WARNING, "apply": guide::APPLY_HELP,
            "update_check": guide::UPDATE_CHECK_HELP,
            "none_answered": guide::NONE_ANSWERED,
        },
        "links": guide::LINKS.iter().map(|(title, url, note)| json!({ "title": title, "url": url, "note": note })).collect::<Vec<_>>(),
    })
}

/// Whether each of the guide's entry points answered, as they're tried,
/// and whether it's ticked to start with (the fastest are, on a fresh
/// install).
pub fn guide_reach(app: &App) -> Value {
    let view = app.guide_view();
    let picked = crate::app::guide::picked(&view);
    Value::Array(
        view.reach
            .into_iter()
            .zip(picked)
            .map(|(r, on)| {
                let mut entry = reach(r);
                entry["on"] = on.into();
                entry
            })
            .collect(),
    )
}

fn reach(reach: crate::app::reach::Reach) -> Value {
    use crate::app::reach::Reach;
    match reach {
        Reach::Checking => json!({ "state": "checking", "label": reach.label() }),
        Reach::Up(_) => json!({ "state": "up", "label": reach.label() }),
        Reach::Down(why) => json!({ "state": "down", "label": reach.label(), "help": crate::app::guide::down_help(why) }),
    }
}

/// An icon to show: its character (none if rettui doesn't know the name),
/// its name and colours.
pub fn icon(appearance: Option<&crate::lxmf::fields::Appearance>) -> Value {
    let Some(a) = appearance else { return Value::Null };
    json!({
        "glyph": crate::icons::glyph(&a.icon).map(String::from),
        "name": a.icon,
        "fg": crate::icons::hex_colour(a.foreground),
        "bg": crate::icons::hex_colour(a.background),
    })
}

pub fn conversations(app: &App) -> Value {
    let list: Vec<Value> = app
        .conversation_order()
        .into_iter()
        .map(|key| {
            let conversation = &app.store.conversations[&key];
            let last = conversation.messages.last().map(|m| {
                let text = if m.content.trim().is_empty() { m.opening() } else { m.text() };
                json!({ "text": text, "timestamp": m.timestamp, "incoming": m.incoming })
            });
            json!({
                "key": key,
                "name": app.store.display_name(&key),
                "named": app.store.peers.get(&key).is_some_and(|p| p.name.is_some()),
                "unread": conversation.unread,
                "muted": conversation.muted,
                "pinned": conversation.pinned,
                // Not a contact: not trusted, nor ever written to.
                "unknown": !app.is_known(&key),
                // A message request (see App::is_request): listed apart.
                "request": app.is_request(&key),
                "icon": icon(app.store.contacts.get(&key).and_then(|c| c.icon.as_ref())),
                "last": last,
            })
        })
        .collect();
    json!(list)
}

/// A message, and what it answers if it's a reply (in `conversation`).
fn message_state(state: &MessageState) -> Value {
    match state {
        MessageState::Received { verified } => json!({ "kind": "received", "verified": verified }),
        MessageState::Sending => json!({ "kind": "sending" }),
        MessageState::Delivered => json!({ "kind": "delivered" }),
        MessageState::Propagated => json!({ "kind": "propagated" }),
        MessageState::Failed(e) => json!({ "kind": "failed", "error": e }),
    }
}

/// A formatted message's text as HTML (escaped: no HTML of the sender's
/// gets through), or `None` for plain text.
fn formatted(m: &Message) -> Option<String> {
    match m.format? {
        crate::markdown::TextFormat::Markdown => Some(crate::markdown::html(&m.content)),
        crate::markdown::TextFormat::Micron => {
            Some(crate::nomad::micron::parse(&m.content).to_html(|url| url.trim().to_string(), |_| None))
        }
    }
}

fn message(m: &Message, conversation: &Conversation) -> Value {
    let state = message_state(&m.state);
    json!({
        "id": m.id,
        "incoming": m.incoming,
        "title": m.title,
        "content": m.content,
        // Its text formatted (Markdown or Micron), if it's marked so.
        "html": formatted(m),
        "timestamp": m.timestamp,
        "state": state,
        "attachments": m.attachments.iter().enumerate().map(|(index, a)| json!({
            "index": index, "name": a.name, "size": a.size, "image": a.image,
            "exists": a.path.exists(),
            // A voice message: its codec, and whether a browser can play it.
            "voice": a.voice, "playable": a.playable(),
        })).collect::<Vec<_>>(),
        // Reactions to it, oldest first: yours with how sending went.
        "reactions": m.reactions.iter().map(|r| json!({
            "emoji": r.emoji, "incoming": r.incoming, "state": message_state(&r.state),
        })).collect::<Vec<_>>(),
        "location": m.location.map(|l| json!({
            "latitude": l.latitude, "longitude": l.longitude, "accuracy": l.accuracy,
            "label": l.label(), "map": l.map_url(),
        })),
        "notes": m.notes,
        // How it was heard, over a radio that says.
        "signal": m.signal.map(|s| json!({ "short": s.short(), "label": s.label() })),
        // A paper message written: its lxm:// link.
        "paper": m.paper,
        // Whether it can be replied to (it has an LXMF hash).
        "can_reply": m.lxmf_hash().is_some(),
        // What it answers: that message's id if it's here, who wrote it
        // (`incoming`, if known) and the start of it.
        "reply": m.reply.as_ref().map(|reply| {
            let quoted = conversation.quoted(reply);
            json!({
                "id": quoted.index.map(|i| conversation.messages[i].id.clone()),
                "incoming": quoted.incoming,
                "text": quoted.text,
            })
        }),
    })
}

/// Archived messages (oldest first), as a conversation's are shown, with
/// replies found among them.
pub fn archived(messages: Vec<Message>) -> Value {
    let conversation = Conversation { messages, ..Conversation::default() };
    let shown: Vec<Value> = conversation.messages.iter().map(|m| message(m, &conversation)).collect();
    json!({ "messages": shown })
}

/// A conversation: its newest `last` messages if asked (what the page
/// shows; live updates refetch it), and how many there are in all.
pub fn conversation(app: &App, key: &str, last: Option<usize>) -> Value {
    let empty = Conversation::default();
    let conversation = app.store.conversations.get(key).unwrap_or(&empty);
    let all = conversation.messages.as_slice();
    let messages: Vec<Value> = newest(all, last).iter().map(|m| message(m, conversation)).collect();
    json!({
        "key": key,
        "total": all.len(),
        "name": app.store.display_name(key),
        "unread": app.store.conversations.get(key).map_or(0, |c| c.unread),
        "muted": app.store.conversations.get(key).is_some_and(|c| c.muted),
        "pinned": app.store.conversations.get(key).is_some_and(|c| c.pinned),
        "icon": icon(app.store.contacts.get(key).and_then(|c| c.icon.as_ref())),
        // Older messages moved to the archive (not in `total`).
        "archived": app.store.conversations.get(key).map_or(0, |c| c.archived),
        // What you keep about them, and the name they announce.
        "contact": {
            "alias": app.store.contact(key).alias,
            "notes": app.store.contact(key).notes,
            "announced": app.store.announced_name(key),
            "trust": app.store.contact(key).trust.key(),
            "known": app.is_known(key),
            "request": app.is_request(key),
            "trust_label": crate::app::contacts::trust_label(app.store.contact(key).trust, app.is_known(key)),
            // The last ping, if any.
            "ping": app.pings.get(key).map(crate::app::contacts::ping_label),
            // How messages to them go.
            "delivery": app.delivery_for(key).label(),
        },
        "archive": app.paths.archive.display().to_string(),
        // Your location, shared with them live.
        "live": app.live_share(key).map(|share| crate::app::live::describe(key, share)),
        "messages": messages,
    })
}

/// Everyone's newest location, for the map (see `App::locations`): this
/// station's first if it's set, then the newest; and whether there are
/// tiles to draw it on, and whose they are.
pub fn locations(app: &App) -> Value {
    let here = app.settings.own_location();
    let places: Vec<Value> = app
        .locations()
        .iter()
        .map(|p| {
            let l = &p.location;
            json!({
                "key": p.key, "name": p.name, "at": p.at,
                "icon": p.key.as_deref().map_or_else(|| icon(app.own_appearance().as_ref()), |k| icon(app.store.contact(k).icon.as_ref())),
                "latitude": l.latitude, "longitude": l.longitude, "accuracy": l.accuracy, "altitude": l.altitude,
                "label": l.label(), "map": l.map_url(),
                // How far and which way from this station.
                "away": here.filter(|_| p.key.is_some()).map(|h| h.away(l)),
            })
        })
        .collect();
    let tiles = app.settings.map_tiles.as_deref();
    let file = app.settings.map_tiles_file.as_deref().map(std::path::Path::new);
    let web_credit = match tiles {
        None => None,
        Some(url) if url.contains("openstreetmap.org") => Some("© OpenStreetMap contributors".to_string()),
        Some(url) => url.split("//").nth(1).and_then(|rest| rest.split('/').next()).map(|host| format!("Tiles: {host}")),
    };
    // The offline map's own credit (or its name), then the web's.
    let file_credit = file.map(|file| {
        crate::web::mbtiles::credit(file)
            .unwrap_or_else(|| format!("Tiles: {}", file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
    });
    let credit = match (file_credit, web_credit) {
        (Some(file), Some(web)) if file != web => Some(format!("{file} · {web}")),
        (file, web) => file.or(web),
    };
    json!({
        "places": places,
        "tiles": tiles.is_some() || file.is_some(),
        "credit": credit,
        "here": here.map(|h| json!({ "latitude": h.latitude, "longitude": h.longitude })),
    })
}

/// Messages found by a search: each with its conversation, when it was,
/// who sent it, how many newer follow it there (to know whether it's among
/// those loaded), and a line of it around what matched.
pub fn search(app: &App, query: &str, within: Option<&str>) -> Value {
    let hits = crate::app::search::search(&app.store, query, within);
    let full = hits.len() >= crate::app::search::MAX_HITS;
    let found: Vec<Value> = hits
        .iter()
        .filter_map(|hit| {
            let message = app.store.conversations.get(&hit.key)?.messages.get(hit.index)?;
            Some(json!({
                "key": hit.key,
                "name": app.store.display_name(&hit.key),
                "id": hit.id,
                "timestamp": hit.timestamp,
                "incoming": hit.incoming,
                "newer": hit.newer,
                "snippet": crate::app::search::snippet(message, query, 80),
            }))
        })
        .collect();
    json!({ "query": query, "hits": found, "full": full })
}

/// Channel lines found by a search: each with its hub and room (as the
/// page names them), when, who said it, and a line of it around what
/// matched.
pub fn channel_search(app: &App, query: &str, within: Option<(crate::net::Hash, &str)>) -> Value {
    let hits = crate::app::channels::search::search(&app.channels, query, within);
    let full = hits.len() >= crate::app::channels::search::MAX_HITS;
    let found: Vec<Value> = hits
        .iter()
        .filter_map(|hit| {
            let hub = app.channels.hubs.iter().find(|h| h.hash == hit.hub)?;
            let line = hub.buffers.get(&hit.room)?.get(hit.index)?;
            let whisper = hub.whispers().into_iter().find(|(key, _)| *key == hit.room).map(|(_, name)| name);
            Some(json!({
                "hub": hex::encode(hit.hub),
                "hub_name": hub.name,
                "room": hit.room,
                "whisper": whisper,
                "ts": hit.ts,
                "nick": if line.own { Some("You".to_string()) } else { line.nick.clone() },
                "snippet": crate::app::search::snippet_of(&[line.text.as_str()], query, 80),
            }))
        })
        .collect();
    json!({ "query": query, "hits": found, "full": full })
}

/// Heard peers, most recent first: those of one kind (`lxmf`, `nomad`,
/// `propagation`) if asked, matching the search words, at most `limit`.
/// `total` counts every match, so the page can offer more, and `heard`
/// every one of the kind, matching or not.
pub fn peers(app: &App, kind: Option<&str>, query: &str, limit: Option<usize>, sort: network::NetSort, via: Option<&str>) -> Value {
    let terms = network::search_terms(query);
    let blocked = |hash: &str| app.store.contact(hash).trust == crate::store::Trust::Blocked;
    let of_kind: Vec<_> = if kind == Some("blocked") {
        // Blocked contacts, heard or not (blocking stops their announces).
        app.store
            .contacts
            .keys()
            .filter(|hash| blocked(hash))
            .map(|hash| (hash, app.store.peers.get(hash).unwrap_or(&network::UNHEARD)))
            .collect()
    } else {
        app.store.peers.iter().filter(|(_, p)| kind.is_none_or(|kind| kind_name(p.kind) == kind)).collect()
    };
    let heard = of_kind.len();
    let mut peers: Vec<_> = of_kind.into_iter().filter(|(hash, p)| network::matches(&terms, hash, p) && app.goes_via(hash, via)).collect();
    let total = peers.len();
    sort.sort(&mut peers);
    peers.truncate(limit.unwrap_or(usize::MAX));
    json!({
        "propagation_node": app.settings.propagation_node,
        "total": total,
        "heard": heard,
        // Interfaces with paths through them, to keep the list to one.
        "interfaces": app.route_interfaces(),
        "peers": peers.into_iter().map(|(hash, p)| json!({
            "hash": hash, "kind": kind_name(p.kind), "name": p.name, "hops": p.hops, "last_seen": p.last_seen,
            "blocked": blocked(hash),
            // The interface its path goes through, if one's known.
            "via": app.routes.get(hash.as_str()),
        })).collect::<Vec<_>>(),
    })
}

/// Announces heard after `after`, of a kind, matching a search and kept to
/// an interface, oldest first; `last` is the newest heard, to ask after.
pub fn announces(app: &App, after: Option<u64>, filter: crate::app::announces::HeardFilter, query: &str, via: Option<&str>) -> Value {
    let terms = network::search_terms(query);
    let log = &app.announce_log;
    let rows: Vec<Value> = log
        .after(after)
        .filter(|entry| filter.shows(entry.heard.kind))
        .filter_map(|entry| {
            let hash = entry.address();
            let name = entry.heard.name.as_deref();
            (network::matches_text(&terms, &[name.unwrap_or_default(), &hash]) && app.goes_via(&hash, via)).then(|| {
                json!({
                    "seq": entry.seq, "at": entry.at, "kind": entry.heard.kind.key(), "tag": entry.heard.kind.tag(),
                    "aspect": entry.heard.aspect, "name": name, "hash": hash, "hops": entry.heard.hops,
                    "via": app.routes.get(hash.as_str()),
                })
            })
        })
        .collect();
    json!({
        "announces": rows,
        "last": log.entries.back().map(|entry| entry.seq),
        "last_minute": log.last_minute(),
        "kept": crate::app::announces::KEPT,
        "interfaces": app.route_interfaces(),
    })
}

fn hub_status(status: &HubStatus) -> Value {
    match status {
        HubStatus::Connected => json!({ "kind": "connected" }),
        HubStatus::Connecting(step) => json!({ "kind": "connecting", "text": step }),
        HubStatus::Failed(e) => json!({ "kind": "failed", "text": e }),
        HubStatus::Disconnected => json!({ "kind": "disconnected" }),
    }
}

fn link(hub: &Hub, room: &str) -> String {
    let mut link = format!("rrc://{}", hex::encode(hub.hash));
    if hub.aspect != rrc::DEFAULT_ASPECT {
        link.push(':');
        link.push_str(&hub.aspect);
    }
    if !room.is_empty() {
        link.push('/');
        link.push_str(room);
    }
    link
}

/// Hubs and their rooms, for the channel list and hub views.
pub fn channels(app: &App) -> Value {
    let hubs: Vec<Value> = app
        .channels
        .hubs
        .iter()
        .map(|hub| {
            let rooms: Vec<Value> = hub
                .listed_rooms()
                .into_iter()
                .map(|room| {
                    json!({
                        "name": room,
                        "joined": hub.rooms.contains(&room),
                        "unread": hub.unread.get(&room).copied().unwrap_or(0),
                        "mention": hub.mentions.contains(&room),
                        "link": link(hub, &room),
                        "notify": hub.notify_level(&room).key(),
                    })
                })
                .collect();
            json!({
                "hash": hex::encode(hub.hash),
                "aspect": hub.aspect,
                "name": hub.hub_name.clone().unwrap_or_else(|| hub.name.clone()),
                "status": hub_status(&hub.status),
                "nick": hub.nick,
                "display_name": app.settings.display_name,
                "auto_connect": hub.auto_connect,
                "limits": { "message_bytes": hub.limits.max_msg_bytes, "rooms": hub.limits.max_rooms },
                "motd": hub.motd,
                "available": hub.available.as_ref().map(|rooms| rooms.iter().map(|(name, topic)| json!({
                    "name": name, "topic": topic, "joined": hub.rooms.contains(name),
                })).collect::<Vec<_>>()),
                "unread": hub.unread.get("").copied().unwrap_or(0),
                "mention": hub.mentions.contains(""),
                "link": link(hub, ""),
                // What its rooms notify of, unless set otherwise.
                "notify": hub.notify.unwrap_or(NotifyLevel::Mentions).key(),
                "rooms": rooms,
                "whispers": hub.whispers().into_iter().map(|(key, name)| json!({
                    "key": key,
                    "name": name,
                    "unread": hub.unread.get(&key).copied().unwrap_or(0),
                    "notify": hub.notify_level(&key).key(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!(hubs)
}

fn line_kind(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Msg => "msg",
        LineKind::Action => "action",
        LineKind::Notice => "notice",
        LineKind::Private => "private",
        LineKind::Error => "error",
        LineKind::System => "system",
    }
}

/// Byte ranges of `text` as UTF-16 offsets (what JavaScript strings index).
fn utf16_ranges(text: &str, ranges: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let at = |byte: usize| text.get(..byte).map_or(0, |t| t.encode_utf16().count());
    ranges.iter().map(|&(start, end)| (at(start), at(end))).collect()
}

/// The newest `last` of `items` (all of them without a limit).
fn newest<T>(items: &[T], last: Option<usize>) -> &[T] {
    &items[last.map_or(0, |n| items.len().saturating_sub(n))..]
}

/// One room's (or the hub buffer's, `room` empty) messages and members: the
/// newest `last` lines if asked, and how many there are in all.
pub fn room(app: &App, hub: &Hub, room: &str, last: Option<usize>) -> Value {
    use crate::app::channels::users::lxmf_address;
    let own_nick = hub.nick.clone().unwrap_or_else(|| app.settings.display_name.clone());
    let buffer = hub.buffers.get(room).map(Vec::as_slice).unwrap_or_default();
    let people = hub.mentionable(room);
    let shown: Vec<_> = buffer.iter().filter(|line| line.shown(app.settings.show_joins)).collect();
    let lines: Vec<Value> = newest(&shown, last)
        .iter()
        .map(|line| {
            let highlights = line.highlights(&own_nick);
            // `@name` mentions of others, drawn in that person's colour.
            let mentions: Vec<Value> = line
                .user_mentions(&people, &highlights)
                .into_iter()
                .map(|(start, end, id)| {
                    let (start, end) = utf16_ranges(&line.text, &[(start, end)])[0];
                    json!([start, end, hex::encode(id)])
                })
                .collect();
            json!({
                "kind": line_kind(line.kind),
                "src": line.src,
                "nick": line.nick.clone().or_else(|| line.own.then(|| own_nick.clone())).or_else(|| {
                    line.src.as_deref().and_then(|s| hex::decode(s).ok()).map(|h| hub.name_of(&h))
                }),
                "text": line.text,
                "ts": line.ts,
                "mention": line.mention,
                "highlights": utf16_ranges(&line.text, &highlights),
                "mentions": mentions,
                "own": line.own,
                "pending": line.pending.is_some(),
            })
        })
        .collect();
    let own = app.identity_hash.to_vec();
    let members = hub.members_of(room);
    // Everyone shown (members and senders), for the user menu.
    let mut ids: BTreeMap<String, Vec<u8>> = members.iter().map(|(_, id)| (hex::encode(id), id.clone())).collect();
    ids.extend(buffer.iter().filter_map(|l| l.src.clone()).filter_map(|s| hex::decode(&s).ok().map(|id| (s, id))));
    let seen: HashMap<&str, &String> =
        buffer.iter().filter(|l| !l.own).filter_map(|l| Some((l.src.as_deref()?, l.nick.as_ref()?))).collect();
    let users: serde_json::Map<String, Value> = ids
        .into_iter()
        .filter(|(_, id)| *id != own)
        .filter_map(|(key, id)| {
            let lxmf = hex::encode(lxmf_address(&id)?);
            let known = app.store.peers.get(&lxmf).is_some_and(|p| p.kind == crate::net::PeerKind::Lxmf)
                || app.store.conversations.contains_key(&lxmf);
            // Their nick now, else the one on their latest line here.
            let name = hub.nicks.get(&id).or_else(|| seen.get(key.as_str()).copied()).cloned().unwrap_or_else(|| hub.name_of(&id));
            Some((key, json!({ "name": name, "lxmf": lxmf, "lxmf_known": known })))
        })
        .collect();
    json!({
        "hub": hex::encode(hub.hash),
        "room": room,
        "total_lines": shown.len(),
        "joined": hub.rooms.contains(room),
        "topic": hub.topics.get(room),
        // Someone known by the start of their identity only (until they
        // say something) has no user menu.
        "members": members.iter().map(|(name, id)| json!({ "name": name, "src": hex::encode(id), "own": *id == own, "partial": id.len() < 16 })).collect::<Vec<_>>(),
        "users": users,
        // Who `@` offers (members and whoever has spoken here).
        "mentionable": people.iter().map(|(name, id)| json!({ "name": name, "src": hex::encode(id), "own": *id == own })).collect::<Vec<_>>(),
        "whisper": hub.direct_notices,
        // This view is a whisper conversation: who with.
        "whisper_with": crate::app::channels::whisper_peer(room).map(|peer| json!({
            "src": hex::encode(&peer), "name": hub.whisper_name(room),
        })),
        "nick": own_nick,
        "own_src": hex::encode(&own),
        // Notifications here: its own level ("default" follows the hub's),
        // and what that comes to.
        "notify": hub.room_notify.get(room).map_or("default", |level| level.key()),
        "notify_level": hub.notify_level(room).key(),
        "hub_notify": hub.notify.unwrap_or(NotifyLevel::Mentions).key(),
        // People joining and leaving are left out when this is off.
        "show_joins": app.settings.show_joins,
        "lines": lines,
    })
}

pub fn saved(app: &App) -> Value {
    json!(app.store.saved.iter().map(|b| json!({ "name": b.name, "url": b.url })).collect::<Vec<_>>())
}

/// `settings.json` for the settings editor, with what each field is.
pub fn settings(app: &App, saved: &Settings) -> Value {
    let fields: Vec<Value> = FIELDS
        .iter()
        .map(|f| {
            json!({
                "key": f.key,
                "label": f.label,
                "help": f.help,
                "kind": match f.kind {
                    FieldKind::Text => "text",
                    FieldKind::Optional => "optional",
                    FieldKind::Toggle => "toggle",
                    FieldKind::Number => "number",
                    FieldKind::Choice(_) => "choice",
                    FieldKind::Color => "color",
                },
                "choices": match f.kind {
                    FieldKind::Choice(choices) => choices.to_vec(),
                    _ => Vec::new(),
                },
                "next_start": f.effect == Effect::NextStart,
                "value": saved.field_value(f.key),
                // The group it's shown in.
                "section": f.section().id(),
                // What the web UI may do with it.
                "web": match f.web_access() {
                    WebAccess::Change => "change",
                    WebAccess::TurnOffOnly => "turn_off_only",
                    WebAccess::TerminalOnly => "terminal_only",
                },
            })
        })
        .collect();
    json!({
        "path": app.paths.settings.display().to_string(),
        "sections": Section::ALL.iter().map(|s| json!({ "id": s.id(), "title": s.title() })).collect::<Vec<_>>(),
        "fields": fields,
        // What this session actually uses (may come from --rns-config or
        // the standard locations).
        "session_rns_config": app.settings.rns_config,
    })
}

/// The hosted node and its pages.
pub fn node(app: &App) -> Value {
    use crate::app::node::NodeStatus;
    let (status, error) = match &app.node.status {
        NodeStatus::Off => ("off", None),
        NodeStatus::Starting => ("starting", None),
        NodeStatus::Running => ("running", None),
        NodeStatus::Failed(e) => ("failed", Some(e.clone())),
    };
    json!({
        "status": status,
        "error": error,
        "address": hex::encode(app.node.hash),
        "name": app.settings.node_name.clone().unwrap_or_else(|| app.settings.display_name.clone()),
        "dir": crate::nomad::host::HostConfig::dir(&app.settings, &app.paths).display().to_string(),
        "scripts": app.settings.node_executable_pages,
        "stats": app.node.stats.as_ref().map(|s| json!({
            "requests": s.request_count, "pages": s.page_hits, "files": s.file_hits, "not_found": s.not_found_count,
        })),
        "list_error": app.node.error,
        "pages": app.node.pages.iter().map(|p| json!({
            "path": p.path, "size": p.size, "modified_ms": p.modified_ms, "executable": p.executable, "text": p.text,
        })).collect::<Vec<_>>(),
    })
}

/// The RRC hub hosted here, for the Hub section: how it's doing, who's on
/// it, its rooms and bans, and what its commands came to.
pub fn hub(app: &App) -> Value {
    use crate::app::node::NodeStatus;
    let hub = &app.hub;
    let (status, error) = match &hub.status {
        NodeStatus::Off => ("off", None),
        NodeStatus::Starting => ("starting", None),
        NodeStatus::Running => ("running", None),
        NodeStatus::Failed(e) => ("failed", Some(e.clone())),
    };
    let name = hub
        .name()
        .map(str::to_string)
        .unwrap_or_else(|| app.settings.hub_name.clone().unwrap_or_else(|| app.settings.display_name.clone()));
    json!({
        "status": status,
        "error": error,
        "address": hub.address.map(|(hash, _)| hex::encode(hash)),
        "link": hub.link(),
        "name": name,
        "greeting": app.settings.hub_greeting,
        "open_rooms": app.settings.hub_open_rooms,
        "announce_mins": app.settings.hub_announce_interval_mins,
        "dir": app.paths.rrc_hub.display().to_string(),
        "you": hex::encode(app.identity_hash),
        "people": hub.snapshot.people,
        "rooms": hub.snapshot.rooms,
        "bans": hub.bans().iter().map(|ban| json!({
            "room": ban.room, "identity": ban.identity, "name": hub.name_of(&ban.identity),
        })).collect::<Vec<_>>(),
        "unidentified": hub.snapshot.unidentified,
        "stats": hub.snapshot.stats,
        "replies": hub.replies.iter().map(|(text, error)| json!({ "text": text, "error": error })).collect::<Vec<_>>(),
        "replied": hub.replied,
    })
}

/// The Reticulum config for the web editor: file state, sections, and the
/// options of one section (the first when `section` is not found).
pub fn reticulum(app: &App, section: Option<&str>) -> Result<Value, String> {
    use crate::reticulum::{self as rns, Section, schema};
    let path = app.rns_path();
    let (text, exists) = rns::load(&path)?;
    let (config, check) = rns::check(&text);
    let sections = rns::sections(&text);
    let current = section.and_then(Section::from_id).filter(|s| sections.contains(s)).unwrap_or(Section::Reticulum);
    let describe = |s: &Section| {
        let values = match s {
            Section::Interface(name) => config.as_ref().and_then(|c| c.subsection("interfaces", name)),
            _ => None,
        };
        json!({
            "id": s.id(),
            "title": s.title(),
            "interface": matches!(s, Section::Interface(_)),
            "type": values.and_then(|v| v.get("type")),
            "enabled": values.map(|v| v.get("enabled").or_else(|| v.get("interface_enabled")).is_none_or(crate::app::reticulum::rns_truthy)),
        })
    };
    let options: Vec<Value> = rns::options(config.as_ref(), &current)
        .into_iter()
        .map(|o| {
            json!({
                "group": o.group, "key": o.key, "label": o.label, "kind": o.kind.name(),
                "choices": o.kind.choices(), "default": o.default, "help": o.help, "value": o.value,
            })
        })
        .collect();
    Ok(json!({
        "path": path.display().to_string(),
        "exists": exists,
        "text": text,
        "error": check.error,
        "warnings": check.warnings,
        "note": rns::RESTART_NOTE,
        "external_note": app.uses_external_shared_instance().then_some(rns::EXTERNAL_NOTE),
        "sections": sections.iter().map(describe).collect::<Vec<_>>(),
        "section": current.id(),
        "options": options,
        "types": schema::INTERFACE_TYPES.iter().map(|t| json!({ "name": t.name, "label": t.label })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_part_of_a_history() {
        let items = [1, 2, 3, 4, 5];
        assert_eq!(newest(&items, Some(2)), [4, 5]);
        assert_eq!(newest(&items, Some(9)), items);
        assert_eq!(newest(&items, None), items);
        assert!(newest(&items, Some(0)).is_empty());
    }

    #[test]
    fn peers_count_the_matches_and_all_of_the_kind() {
        use crate::net::PeerKind;
        use crate::store::{Peer, Store};
        let dir = std::env::temp_dir().join(format!("rettui-views-peers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::default();
        for (i, name) in ["Alpha Library", "Beta Wiki", "Gamma"].into_iter().enumerate() {
            store.peers.insert(format!("{i:032x}"), Peer { kind: PeerKind::Nomad, name: Some(name.into()), hops: 1, last_seen: i as i64 });
        }
        store.peers.insert("ab".repeat(16), Peer { kind: PeerKind::Lxmf, name: Some("Alpha person".into()), hops: 1, last_seen: 9 });
        let mut app = crate::app::test_app(&dir, crate::config::Settings::default(), store);
        let heard = network::NetSort::Heard;
        let found = peers(&app, Some("nomad"), "alpha", Some(10), heard, None);
        assert_eq!((found["total"].as_u64(), found["heard"].as_u64()), (Some(1), Some(3)));
        assert_eq!(found["peers"][0]["name"], "Alpha Library");
        let newest = peers(&app, Some("nomad"), "", Some(2), heard, None);
        assert_eq!((newest["total"].as_u64(), newest["heard"].as_u64()), (Some(3), Some(3)));
        assert_eq!(newest["peers"].as_array().unwrap().len(), 2);
        // By name, or kept to those whose path goes through an interface.
        let names =
            |found: &Value| found["peers"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(
            names(&peers(&app, None, "", None, network::NetSort::Name, None)),
            ["Alpha Library", "Alpha person", "Beta Wiki", "Gamma"]
        );
        app.routes.insert("ab".repeat(16), "RNode LoRa".into());
        let via = peers(&app, None, "", None, heard, Some("RNode LoRa"));
        assert_eq!((names(&via), via["peers"][0]["via"].as_str()), (vec!["Alpha person".to_string()], Some("RNode LoRa")));
        assert_eq!(via["interfaces"], json!(["RNode LoRa"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_say_their_group() {
        let dir = std::env::temp_dir().join(format!("rettui-views-groups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app = crate::app::test_app(&dir, crate::config::Settings::default(), crate::store::Store::default());
        let view = settings(&app, &app.settings);
        assert_eq!(view["sections"][0], json!({ "id": "profile", "title": "Profile" }));
        let section = |key: &str| view["fields"].as_array().unwrap().iter().find(|f| f["key"] == key).unwrap()["section"].clone();
        assert_eq!(
            (section("display_name"), section("hub_enabled"), section("quiet_hours")),
            (json!("profile"), json!("hosting"), json!("notifications"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_hub_section_has_its_people_rooms_bans_and_replies() {
        use crate::net::NetEvent;
        use crate::rrc::host::{HubEvent, HubRoom, HubSnapshot};
        let dir = std::env::temp_dir().join(format!("rettui-views-hub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = crate::config::Settings { hub_enabled: true, ..crate::config::Settings::default() };
        let mut app = crate::app::test_app(&dir, settings, crate::store::Store::default());
        assert_eq!((hub(&app)["status"].as_str(), hub(&app)["link"].as_str()), (Some("starting"), None));
        assert_eq!(state(&app)["hub"]["state"], "starting");
        app.on_net(NetEvent::Hub(HubEvent::Started { hash: [7; 16], public_key: [1; 64] }));
        let room = HubRoom {
            name: "lobby".into(),
            registered: true,
            modes: "+nrt".into(),
            topic: None,
            members: Vec::new(),
            founder: None,
            operators: Vec::new(),
            voiced: Vec::new(),
            banned: vec!["c".repeat(32)],
            key: None,
            invited: Vec::new(),
        };
        app.on_net(NetEvent::Hub(HubEvent::State(HubSnapshot { rooms: vec![room], ..HubSnapshot::default() })));
        app.on_net(NetEvent::Hub(HubEvent::Reply { text: "no such room".into(), error: true }));
        let view = hub(&app);
        assert_eq!(
            (view["status"].as_str(), view["link"].as_str()),
            (Some("running"), Some(format!("rrc://{}", hex::encode([7; 16])).as_str()))
        );
        assert_eq!(view["rooms"][0]["name"], "lobby");
        assert_eq!(view["bans"][0], json!({ "room": "lobby", "identity": "c".repeat(32), "name": "cccccccccccc" }));
        assert_eq!((view["replies"][0]["error"].as_bool(), view["replied"].as_u64()), (Some(true), Some(1)));
        assert_eq!(state(&app)["hub"]["rooms"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
