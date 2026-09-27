//! Non-interactive commands: send, listen, sync and fetch from the shell.

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
        announce_interval: None,
        propagation_node: node,
        sync_interval: None,
        known_identities: paths.known_identities.clone(),
        host: None,
    }
}

async fn wait_started(events: &mut UnboundedReceiver<NetEvent>) -> Result<Hash> {
    while let Some(event) = events.recv().await {
        match event {
            NetEvent::Started { lxmf_hash } => return Ok(lxmf_hash),
            NetEvent::StartFailed(e) => bail!(e),
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
    for attachment in &message.attachments {
        let name = Path::new(&attachment.name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".into());
        let path = downloads.join(&name);
        let saved = std::fs::create_dir_all(downloads).and_then(|()| std::fs::write(&path, &attachment.data));
        match saved {
            Ok(()) => println!("  attachment: {} ({} bytes)", path.display(), attachment.data.len()),
            Err(e) => println!("  attachment {name}: could not save: {e}"),
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
    let node = settings.propagation_node.as_deref().and_then(crate::net::parse_hash);
    let (commands, mut events) = net::spawn(options(settings, paths, identity, false, node));
    wait_started(&mut events).await?;
    let _ = commands.send(NetCommand::SendMessage {
        id: 1,
        to,
        content: message.to_string(),
        attachments,
        mode,
    });
    while let Some(event) = events.recv().await {
        match event {
            NetEvent::Delivery { result: Ok(how), .. } => {
                println!("Delivered ({how:?})");
                return Ok(());
            }
            NetEvent::Delivery { result: Err(e), .. } => bail!("delivery failed: {e}"),
            NetEvent::Log(line) => eprintln!("{line}"),
            _ => {}
        }
    }
    bail!("network task stopped")
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
    let result = nomad::fetch_once(&runtime, node, path, identity).await;
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
