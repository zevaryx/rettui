//! Non-interactive commands: send, listen, sync, fetch, path and probe
//! from the shell.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use rns_identity::identity::Identity;
use tokio::sync::mpsc::UnboundedReceiver;
use unicode_width::UnicodeWidthStr;

use crate::config::{Paths, Settings};
use crate::lxmf::{DeliveryMode, InboundMessage};
use crate::net::{self, Hash, NetCommand, NetEvent, NetOptions};
use crate::nomad::{self, micron};

fn options(
    settings: &Settings,
    paths: &Paths,
    identity: Identity,
    announce: bool,
    node: Option<Hash>,
) -> NetOptions {
    NetOptions {
        rns_config: settings.rns_config.clone(),
        identity,
        display_name: settings.display_name.clone(),
        announce_at_start: announce,
        announce: crate::net::AnnounceSchedule::Off,
        propagation_node: node,
        auto_propagation: false,
        sync_interval: None,
        known_identities: paths.known_identities.clone(),
        host: None,
        propagation: None,
        stamp_cost: crate::lxmf::policy::stamp_cost(settings.stamp_cost),
        max_message_bytes: settings.max_message_kb * 1000,
        tickets: paths.tickets.clone(),
        ratchets: paths.ratchets.clone(),
    }
}

async fn wait_started(events: &mut UnboundedReceiver<NetEvent>) -> Result<Hash> {
    while let Some(event) = events.recv().await {
        match event {
            NetEvent::Started { lxmf_hash, .. } => return Ok(lxmf_hash),
            NetEvent::StartFailed(e) => bail!(e),
            // Interfaces that fail to start say so before Reticulum is up.
            NetEvent::Log(line) => eprintln!("{line}"),
            _ => {}
        }
    }
    bail!("network task stopped")
}

fn print_message(message: &InboundMessage, downloads: &Path) {
    let when = chrono::DateTime::from_timestamp(message.timestamp as i64, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    let verified = if message.verified { "" } else { " (unverified)" };
    println!("From {} at {when}{verified}", hex::encode(message.source));
    if !message.title.is_empty() {
        println!("  {}", message.title);
    }
    for line in message.content.lines() {
        println!("  {line}");
    }
    let extras = &message.extras;
    if let Some(reaction) = &extras.reaction {
        println!("  reacted {} to message {}", reaction.emoji, hex::encode(reaction.to));
    }
    let location = extras.telemetry.as_ref().and_then(|t| t.location);
    if let Some(location) = location {
        println!("  location: {}  {}", location.label(), location.map_url());
    }
    let bare = message.content.trim().is_empty() && message.attachments.is_empty() && extras.audio.is_none() && location.is_none();
    for note in extras.notes(bare) {
        println!("  ({note})");
    }
    let audio = extras.audio.as_ref().map(|a| (a.file_name(), a.data.as_slice(), format!("voice message, {}", a.codec())));
    let files = message.attachments.iter().map(|a| (a.name.clone(), a.data.as_slice(), "attachment".to_string()));
    for (name, data, what) in files.chain(audio) {
        let name = Path::new(&name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".into());
        let path = downloads.join(&name);
        let saved = std::fs::create_dir_all(downloads).and_then(|()| std::fs::write(&path, data));
        match saved {
            Ok(()) => println!("  {what}: {} ({} bytes)", path.display(), data.len()),
            Err(e) => println!("  {what} {name}: could not save: {e}"),
        }
    }
}

pub async fn send(
    settings: &Settings,
    paths: &Paths,
    identity: Identity,
    to: &str,
    message: &str,
    attachments: Vec<PathBuf>,
    mode: DeliveryMode,
) -> Result<()> {
    let to = crate::net::parse_hash(to).ok_or_else(|| anyhow!("expected a 32 hex char LXMF address"))?;
    if let Some(missing) = attachments.iter().find(|path| !path.is_file()) {
        bail!("{} is not a file", missing.display());
    }
    if mode == DeliveryMode::Paper && !attachments.is_empty() {
        bail!("a paper message carries text only");
    }
    let node = settings.propagation_node.as_deref().and_then(crate::net::parse_hash);
    let (commands, mut events) = net::spawn(options(settings, paths, identity, false, node));
    wait_started(&mut events).await?;
    let content = message.to_string();
    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let _ = commands.send(if mode == DeliveryMode::Paper {
        NetCommand::WritePaper { id: 1, paper: crate::lxmf::paper::Paper { to, content, timestamp, reply: None, format: None } }
    } else {
        NetCommand::SendMessage { id: 1, message: crate::lxmf::Outgoing::text(to, content, attachments, mode, timestamp) }
    });
    while let Some(event) = events.recv().await {
        match event {
            NetEvent::Delivery { result: Ok(sent), .. } => {
                println!("Delivered ({:?})", sent.delivered);
                return Ok(());
            }
            NetEvent::Delivery { result: Err(e), .. } => bail!("delivery failed: {e}"),
            NetEvent::Paper { result: Ok((link, _)), .. } => {
                print_paper(&link);
                return Ok(());
            }
            NetEvent::Paper { result: Err(e), .. } => bail!("could not write the paper message: {e}"),
            NetEvent::Log(line) => eprintln!("{line}"),
            _ => {}
        }
    }
    bail!("network task stopped")
}

/// A paper message written: its QR code, then its link.
fn print_paper(link: &str) {
    print_qr(link);
    println!("{link}");
}

/// The QR code of `text`, on a terminal wide enough (two modules a
/// character, black on white whatever the theme); nothing when output
/// isn't a terminal.
pub fn print_qr(text: &str) {
    use std::io::IsTerminal;
    let width = crossterm::terminal::size().map_or(0, |(cols, _)| cols as usize);
    match crate::lxmf::paper::Qr::new(text) {
        Ok(qr) if std::io::stdout().is_terminal() && qr.size() <= width => {
            let colour = |dark: bool| if dark { 16 } else { 231 };
            for y in (0..qr.size()).step_by(2) {
                let mut line = String::new();
                for x in 0..qr.size() {
                    let (top, bottom) = (colour(qr.is_dark(x, y)), colour(qr.is_dark(x, y + 1)));
                    line.push_str(&format!("\x1b[38;5;{top}m\x1b[48;5;{bottom}m▀"));
                }
                println!("{line}\x1b[0m");
            }
        }
        Ok(qr) if std::io::stdout().is_terminal() => {
            eprintln!("(the QR code needs a terminal {} columns wide)", qr.size());
        }
        _ => {}
    }
}

/// Announce, then print incoming messages until the time runs out.
pub async fn listen(settings: &Settings, paths: &Paths, identity: Identity, seconds: u64) -> Result<()> {
    let (_commands, mut events) = net::spawn(options(settings, paths, identity, true, None));
    let address = wait_started(&mut events).await?;
    println!("Listening on {} for {seconds}s", hex::encode(address));
    let deadline = tokio::time::sleep(Duration::from_secs(seconds));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => return Ok(()),
            event = events.recv() => match event {
                Some(NetEvent::Message(message)) => print_message(&message, &paths.downloads),
                Some(NetEvent::Announced) => println!("Announced"),
                Some(NetEvent::Log(line)) => eprintln!("{line}"),
                Some(_) => {}
                None => bail!("network task stopped"),
            }
        }
    }
}

/// Ping an LXMF address (or `lxma://` link) and print how it went.
pub async fn ping(settings: &Settings, paths: &Paths, identity: Identity, address: &str) -> Result<()> {
    let (to, key) = crate::lxmf::parse_contact(address).map_err(|e| anyhow!(e))?;
    let (commands, mut events) = net::spawn(options(settings, paths, identity, false, None));
    wait_started(&mut events).await?;
    if let Some(public_key) = key {
        let _ = commands.send(NetCommand::Remember { to, public_key });
    }
    let _ = commands.send(NetCommand::Ping(to));
    while let Some(event) = events.recv().await {
        match event {
            NetEvent::Pinged { result: Ok(ping), .. } => {
                let hops = ping.hops.map_or_else(String::new, |h| format!(", {h} hop{} away", if h == 1 { "" } else { "s" }));
                let signal = ping.signal.map_or_else(String::new, |s| format!(", heard at {}", s.label()));
                println!("{} answered in {} ms{hops}{signal}", hex::encode(to), ping.rtt.as_millis());
                return Ok(());
            }
            NetEvent::Pinged { result: Err(e), .. } => bail!("{e}"),
            NetEvent::Log(line) => eprintln!("{line}"),
            _ => {}
        }
    }
    bail!("network task stopped")
}

fn parse_destination(address: &str) -> Result<Hash> {
    net::parse_hash(address).ok_or_else(|| anyhow!("{address} isn't an address: one is 32 hex characters"))
}

/// Say so when a path has to be asked for: the first request's answer can
/// take a while, and further ones are reported as they go.
async fn say_if_asking(runtime: &rns_runtime::reticulum::ReticulumHandle, to: Hash) {
    if !runtime.has_path(to).await.unwrap_or(false) {
        eprintln!("No path known; asking for one…");
    }
}

/// Find the path to a destination and print it, as `rnpath` does, asking
/// for one if none is known; or with `drop`, forget it.
pub async fn path(settings: &Settings, address: &str, drop: bool) -> Result<()> {
    let to = parse_destination(address)?;
    let runtime = net::start_runtime(settings.rns_config.as_deref())
        .await
        .map_err(anyhow::Error::msg)?;
    if drop {
        // Without a shared instance, the paths are those the last run with
        // this config saved, and this run saves them again without it.
        let had = net::drop_path(&runtime, to).await;
        runtime.shutdown_and_wait().await;
        if !had {
            bail!("No path to {} was known", hex::encode(to));
        }
        println!("Forgot the path to {}: the next use asks for a fresh one", hex::encode(to));
        return Ok(());
    }
    say_if_asking(&runtime, to).await;
    let result = net::trace_path(&runtime, to, &|text| eprintln!("{text}…")).await;
    runtime.shutdown_and_wait().await;
    let info = result.map_err(anyhow::Error::msg)?;
    let hops = crate::app::paths::hops_label(info.hops);
    let via = info.via.map(|via| format!(" via {}", hex::encode(via))).unwrap_or_default();
    let kept = crate::app::paths::kept_label(info.expires, chrono::Utc::now().timestamp());
    println!("{}: {hops}{via} on {}, kept for {kept}", hex::encode(to), info.interface);
    Ok(())
}

/// Print every path known, as `rnpath -t` does: the shared instance's, if
/// rettui or rnsd runs as one.
pub async fn path_table(settings: &Settings) -> Result<()> {
    let runtime = net::start_runtime(settings.rns_config.as_deref())
        .await
        .map_err(anyhow::Error::msg)?;
    let shared = runtime.instance_mode == rns_runtime::reticulum::InstanceMode::Client;
    let paths = net::paths(&runtime).await;
    runtime.shutdown_and_wait().await;
    let mut paths = paths.map_err(anyhow::Error::msg)?;
    if !shared {
        eprintln!("No shared instance (rettui or rnsd) runs here: these are the paths the last run saved, and any heard since");
    }
    if paths.is_empty() {
        println!("No paths known");
        return Ok(());
    }
    paths.sort_by(|(a, a_info), (b, b_info)| (a_info.hops, &a_info.interface, a).cmp(&(b_info.hops, &b_info.interface, b)));
    let now = chrono::Utc::now().timestamp();
    println!("{:<32}  {:>4}  {:<32}  {:<8}  Interface", "Address", "Hops", "Via", "Kept for");
    for (hash, info) in paths {
        let via = info.via.map_or_else(|| "-".to_string(), hex::encode);
        let kept = crate::app::paths::kept_label(info.expires, now);
        println!("{}  {:>4}  {via:<32}  {kept:<8}  {}", hex::encode(hash), info.hops, info.interface);
    }
    Ok(())
}

/// Probe a destination and print how it answered, as `rnprobe` does.
/// `name` (e.g. `rnsh.listen`) is for destinations rettui can't tell the
/// kind of, which are sent a probe packet they must prove.
pub async fn probe(settings: &Settings, paths: &Paths, address: &str, name: Option<&str>) -> Result<()> {
    let to = parse_destination(address)?;
    let known = net::KnownIdentities::load(&paths.known_identities);
    let runtime = net::start_runtime(settings.rns_config.as_deref())
        .await
        .map_err(anyhow::Error::msg)?;
    say_if_asking(&runtime, to).await;
    let result = net::probe(&runtime, &known, to, name, &|text| eprintln!("{text}…")).await;
    runtime.shutdown_and_wait().await;
    let probe = result.map_err(anyhow::Error::msg)?;
    println!("{}: {}", hex::encode(to), crate::app::paths::probe_label(&probe));
    Ok(())
}

/// Download waiting messages from the propagation node and print them.
pub async fn sync(settings: &Settings, paths: &Paths, identity: Identity, node: Option<&str>) -> Result<()> {
    let node = node
        .or(settings.propagation_node.as_deref())
        .and_then(crate::net::parse_hash)
        .ok_or_else(|| anyhow!("no propagation node: pass --node or set propagation_node"))?;
    let (commands, mut events) = net::spawn(options(settings, paths, identity, false, Some(node)));
    wait_started(&mut events).await?;
    let _ = commands.send(NetCommand::Sync);
    let mut messages = Vec::new();
    while let Some(event) = events.recv().await {
        match event {
            NetEvent::Message(message) => messages.push(message),
            NetEvent::Synced(Ok(count)) => {
                // Messages are parsed before the result is reported.
                println!("{count} message(s) downloaded");
                for message in &messages {
                    print_message(message, &paths.downloads);
                }
                return Ok(());
            }
            NetEvent::Synced(Err(e)) => bail!("sync failed: {e}"),
            NetEvent::Log(line) => eprintln!("{line}"),
            _ => {}
        }
    }
    bail!("network task stopped")
}

pub async fn fetch(
    settings: &Settings,
    url: &str,
    raw: bool,
    identity: Option<Identity>,
    output: Option<PathBuf>,
) -> Result<()> {
    let (node, path) = url.split_once(':').unwrap_or((url, ""));
    let Some(node) = net::parse_hash(node) else {
        bail!("expected <32 hex char node hash>[:/page/path.mu]");
    };
    let path = if path.is_empty() { nomad_core::DEFAULT_INDEX_ROUTE } else { path };
    let runtime = net::start_runtime(settings.rns_config.as_deref())
        .await
        .map_err(anyhow::Error::msg)?;
    let result = nomad::fetch_once(&runtime, node, path, identity, &|text| eprintln!("{text}")).await;
    runtime.shutdown_and_wait().await;
    let content = result.map_err(anyhow::Error::msg)?;
    if let Some(output) = output {
        std::fs::write(&output, &content.data)?;
        println!("Saved {} bytes to {}", content.data.len(), output.display());
        return Ok(());
    }
    let text = String::from_utf8_lossy(&content.data);
    if raw {
        println!("{text}");
    } else {
        const WIDTH: usize = 100;
        let layout = micron::parse(&text).layout(WIDTH, None, &Default::default(), None);
        for line in layout.lines {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let text = text.trim_end();
            // Centred and right-aligned lines, placed as the TUI draws them.
            let pad = match line.alignment {
                Some(ratatui::layout::Alignment::Center) => WIDTH.saturating_sub(text.width()) / 2,
                Some(ratatui::layout::Alignment::Right) => WIDTH.saturating_sub(text.width()),
                _ => 0,
            };
            println!("{}{text}", " ".repeat(pad));
        }
    }
    Ok(())
}
