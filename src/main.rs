mod app;
mod cli;
mod config;
mod lxmf;
mod net;
mod reticulum;
mod nomad;
mod rrc;
mod store;
mod term;
mod ui;
mod web;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyEventKind,
};
use crossterm::execute;
use futures_util::StreamExt;

use crate::app::App;
use crate::config::{Paths, Settings};
use crate::store::Store;

#[derive(Parser)]
#[command(version, about = "Reticulum client for the terminal and the browser: LXMF messaging, NomadNet browsing and hosting, and RRC chat")]
struct Cli {
    /// Directory for rettui's identity, settings and messages.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Reticulum config directory (overrides settings.json).
    #[arg(long, global = true)]
    rns_config: Option<String>,
    /// Serve the web UI instead of the terminal UI (default address
    /// 127.0.0.1:8740).
    #[arg(long, value_name = "ADDRESS", num_args = 0..=1, default_missing_value = web::DEFAULT_ADDRESS)]
    web: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch a NomadNet page and print it (e.g. `<hash>:/page/index.mu`).
    Fetch {
        url: String,
        /// Print the raw Micron source instead of plain text.
        #[arg(long)]
        raw: bool,
        /// Identify to the node with this client's identity.
        #[arg(long)]
        identify: bool,
        /// Save the raw response to a file (for /file/ and /media/ paths).
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Print this client's LXMF address.
    Address,
    /// Send one LXMF message.
    Send {
        address: String,
        message: String,
        /// Attach a file (repeatable).
        #[arg(long, short)]
        attach: Vec<PathBuf>,
        /// auto (direct, then propagation node), direct or propagated.
        #[arg(long, default_value = "auto")]
        mode: String,
    },
    /// Announce and print incoming messages.
    Listen {
        /// How long to listen.
        #[arg(long, default_value_t = 300)]
        seconds: u64,
    },
    /// Download waiting messages from the propagation node.
    Sync {
        /// Propagation node to use instead of the configured one.
        #[arg(long)]
        node: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::new(cli.data_dir)?;
    let mut settings = Settings::load(&paths.settings)?;
    if cli.rns_config.is_some() {
        settings.rns_config = cli.rns_config;
    }
    if settings.rns_config.is_none() {
        settings.rns_config = config::default_rns_config();
    }

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .with_context(|| format!("opening {}", paths.log.display()))?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RETTUI_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::sync::Mutex::new(log_file))
        .with_ansi(false)
        .init();

    let identity = config::load_identity(&paths.identity)?;

    if let Some(address) = cli.web {
        if cli.command.is_some() {
            bail!("--web runs the web UI; it cannot be combined with a command");
        }
        return web::run(settings, paths, identity, &address).await;
    }

    match cli.command {
        Some(Command::Address) => {
            let hash = rns_identity::destination::Destination::hash_from_name_and_identity(
                lxmf::LXMF_ASPECT,
                Some(&identity.hash),
            );
            println!("{}", hex::encode(hash));
            Ok(())
        }
        Some(Command::Fetch {
            url,
            raw,
            identify,
            output,
        }) => cli::fetch(&settings, &url, raw, identify.then_some(identity), output).await,
        Some(Command::Send {
            address,
            message,
            attach,
            mode,
        }) => {
            let mode = match mode.as_str() {
                "auto" => lxmf::DeliveryMode::Auto,
                "direct" => lxmf::DeliveryMode::Direct,
                "propagated" => lxmf::DeliveryMode::Propagated,
                other => bail!("unknown delivery mode {other}; use auto, direct or propagated"),
            };
            cli::send(&settings, &paths, identity, &address, &message, attach, mode).await
        }
        Some(Command::Listen { seconds }) => cli::listen(&settings, &paths, identity, seconds).await,
        Some(Command::Sync { node }) => cli::sync(&settings, &paths, identity, node.as_deref()).await,
        None => run_tui(settings, paths, identity).await,
    }
}

/// Network options for an interactive session (the TUI or the web UI).
fn net_options(settings: &Settings, paths: &Paths, identity: rns_identity::identity::Identity) -> net::NetOptions {
    net::NetOptions {
        rns_config: settings.rns_config.clone(),
        identity,
        display_name: settings.display_name.clone(),
        announce_at_start: settings.announce_at_start,
        announce_interval: (settings.announce_interval_mins > 0)
            .then(|| Duration::from_secs(settings.announce_interval_mins * 60)),
        propagation_node: settings.propagation_node.as_deref().and_then(net::parse_hash),
        sync_interval: (settings.sync_interval_mins > 0)
            .then(|| Duration::from_secs(settings.sync_interval_mins * 60)),
        known_identities: paths.known_identities.clone(),
        host: nomad::host::HostConfig::from_settings(settings, paths),
    }
}

async fn run_tui(settings: Settings, paths: Paths, identity: rns_identity::identity::Identity) -> Result<()> {
    let identity_hash = identity.hash;
    let store = Store::load(&paths.store).unwrap_or_else(|e| {
        tracing::warn!("could not load store, starting empty: {e}");
        Store::default()
    });
    let (mut net_tx, mut net_rx) = net::spawn(net_options(&settings, &paths, identity.clone()));
    let mut terminal = ratatui::init();
    // Probe for Kitty graphics while in raw mode, before EventStream starts
    // reading stdin (it would swallow the terminal's replies).
    let graphics = term::images::Graphics::detect();
    let mut app = App::new(settings, paths, store, net_tx.clone(), graphics, identity_hash);
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    // ratatui's panic hook restores the terminal; also release the mouse.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        hook(info);
    }));
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut save = tokio::time::interval(Duration::from_secs(10));

    let mut decoded = app.take_decoded();
    let result = loop {
        if let Err(e) = terminal.draw(|frame| ui::draw(frame, &mut app)) {
            break Err(e.into());
        }
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => app.on_key(key),
                Some(Ok(Event::Mouse(mouse))) => app.on_mouse(mouse),
                Some(Ok(Event::Paste(text))) => app.on_paste(&text),
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(e.into()),
                None => break Ok(()),
            },
            Some(event) = net_rx.recv() => {
                app.on_net(event);
                // Apply a burst of network events before redrawing.
                net::drain_burst(&mut net_rx, |event| app.on_net(event));
            }
            Some(image) = decoded.recv() => app.on_decoded(image),
            _ = tick.tick() => app.on_tick(),
            _ = save.tick() => app.save_if_dirty(),
        }
        if app.should_quit {
            break Ok(());
        }
        if app.take_rns_restart() {
            // Show "restarting" while the old stack stops.
            let _ = terminal.draw(|frame| ui::draw(frame, &mut app));
            net::shutdown(&net_tx, &mut net_rx, net::Stop::Restart).await;
            (net_tx, net_rx) = net::spawn(net_options(&app.settings, &app.paths, identity.clone()));
            app.set_network(net_tx.clone());
        }
    };

    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    app.save_if_dirty();
    // Leave hubs and stop Reticulum properly (a few seconds at most).
    net::shutdown(&net_tx, &mut net_rx, net::Stop::Quit).await;
    app.finish_saves();
    result
}
