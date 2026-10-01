//! JSON snapshots of the app state, shaped for the web UI's screens.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use crate::app::channels::{Hub, HubStatus, LineKind};
use crate::app::node::NodeStatus;
use crate::app::{App, NetState, SyncState, network};
use crate::config::{Effect, FIELDS, FieldKind, Settings, WebAccess};
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
        SyncState::Done(at, Ok(n)) => json!({ "state": "done", "at": at.format("%H:%M").to_string(), "count": n }),
        SyncState::Done(at, Err(e)) => json!({ "state": "failed", "at": at.format("%H:%M").to_string(), "error": e }),
    };
    let propagation = app.settings.propagation_node.as_ref().map(|hash| {
        json!({ "hash": hash, "name": app.store.display_name(hash) })
    });
    let hosting = match &app.pn.status {
        NodeStatus::Off => json!({ "state": "off" }),
        NodeStatus::Starting => json!({ "state": "starting" }),
        NodeStatus::Running => json!({ "state": "running", "hash": hex::encode(app.pn.hash), "stats": app.pn.stats }),
        NodeStatus::Failed(e) => json!({ "state": "failed", "error": e }),
    };
    let message_unread: usize = app.store.conversations.values().map(|c| c.unread).sum();
    let (channel_unread, mention) = app.channels.total_unread();
    let online = app.interfaces.iter().filter(|i| i.online).count();
    json!({
        "display_name": app.settings.display_name,
        "external_shared_instance": app.uses_external_shared_instance(),
        "wrap_lines": app.settings.wrap_lines,
        "lxmf_address": app.lxmf_hash.map(hex::encode),
        // With the public key, for others to add you (`lxma://`).
        "identity_link": app.identity_link(),
        "net": net,
        "sync": sync,
        "propagation_node": propagation,
        // The propagation node hosted here.
        "hosting": hosting,
        "rns_config": app.settings.rns_config,
        "data_dir": app.paths.store.parent().map(|p| p.display().to_string()),
        "known": app.store.peers.len(),
        "interfaces": app.interfaces.iter().map(|i| json!({
            "name": i.name, "online": i.online, "rx": i.rx_bytes, "tx": i.tx_bytes,
        })).collect::<Vec<_>>(),
        "interfaces_online": online,
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

pub fn conversations(app: &App) -> Value {
    let list: Vec<Value> = app
        .store
        .conversation_order()
        .into_iter()
        .map(|key| {
            let conversation = &app.store.conversations[&key];
            let last = conversation.messages.last().map(|m| {
                let text = if m.content.trim().is_empty() { m.opening() } else { m.content.clone() };
                json!({ "text": text, "timestamp": m.timestamp, "incoming": m.incoming })
            });
            json!({
                "key": key,
                "name": app.store.display_name(&key),
                "named": app.store.peers.get(&key).is_some_and(|p| p.name.is_some()),
                "unread": conversation.unread,
                "muted": conversation.muted,
                // Not a contact: not trusted, nor ever written to.
                "unknown": !app.is_known(&key),
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

fn message(m: &Message, conversation: &Conversation) -> Value {
    let state = message_state(&m.state);
    json!({
        "id": m.id,
        "incoming": m.incoming,
        "title": m.title,
        "content": m.content,
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
        // Older messages moved to the archive (not in `total`).
        "archived": app.store.conversations.get(key).map_or(0, |c| c.archived),
        // What you keep about them, and the name they announce.
        "contact": {
            "alias": app.store.contact(key).alias,
            "notes": app.store.contact(key).notes,
            "announced": app.store.announced_name(key),
            "trust": app.store.contact(key).trust.key(),
            "known": app.is_known(key),
            "trust_label": crate::app::contacts::trust_label(app.store.contact(key).trust, app.is_known(key)),
            // The last ping, if any.
            "ping": app.pings.get(key).map(crate::app::contacts::ping_label),
        },
        "archive": app.paths.archive.display().to_string(),
        "messages": messages,
    })
}

/// Heard peers, most recent first: those of one kind (`lxmf`, `nomad`,
/// `propagation`) if asked, matching the search words, at most `limit`.
/// `total` counts every match, so the page can offer more.
pub fn peers(app: &App, kind: Option<&str>, query: &str, limit: Option<usize>) -> Value {
    let terms = network::search_terms(query);
    let blocked = |hash: &str| app.store.contact(hash).trust == crate::store::Trust::Blocked;
    let mut peers: Vec<_> = if kind == Some("blocked") {
        // Blocked contacts, heard or not (blocking stops their announces).
        app.store
            .contacts
            .keys()
            .filter(|hash| blocked(hash))
            .map(|hash| (hash, app.store.peers.get(hash).unwrap_or(&network::UNHEARD)))
            .filter(|(hash, p)| network::matches(&terms, hash, p))
            .collect()
    } else {
        app.store
            .peers
            .iter()
            .filter(|(hash, p)| kind.is_none_or(|kind| kind_name(p.kind) == kind) && network::matches(&terms, hash, p))
            .collect()
    };
    let total = peers.len();
    peers.sort_unstable_by(|a, b| b.1.last_seen.cmp(&a.1.last_seen).then_with(|| a.0.cmp(b.0)));
    peers.truncate(limit.unwrap_or(usize::MAX));
    json!({
        "propagation_node": app.settings.propagation_node,
        "total": total,
        "peers": peers.into_iter().map(|(hash, p)| json!({
            "hash": hash, "kind": kind_name(p.kind), "name": p.name, "hops": p.hops, "last_seen": p.last_seen,
            "blocked": blocked(hash),
        })).collect::<Vec<_>>(),
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
        "members": members.iter().map(|(name, id)| json!({ "name": name, "src": hex::encode(id), "own": *id == own })).collect::<Vec<_>>(),
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
                },
                "next_start": f.effect == Effect::NextStart,
                "value": saved.field_value(f.key),
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


/// The Reticulum config for the web editor: file state, sections, and the
/// options of one section (the first when `section` is not found).
pub fn reticulum(app: &App, section: Option<&str>) -> Result<Value, String> {
    use crate::reticulum::{self as rns, Section, schema};
    let path = app.rns_path();
    let (text, exists) = rns::load(&path)?;
    let (config, check) = rns::check(&text);
    let sections = rns::sections(&text);
    let current = section
        .and_then(Section::from_id)
        .filter(|s| sections.contains(s))
        .unwrap_or(Section::Reticulum);
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
}
