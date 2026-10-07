mod app;
mod cli;
mod config;
mod emoji;
mod icons;
mod lxmf;
mod markdown;
mod names;
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
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange,
    EnableMouseCapture, Event, EventStream, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use futures_util::StreamExt;
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

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
    /// Serve the web UI over HTTPS, with rettui's own certificate (devices
    /// install its certificate authority, from /rettui-ca.crt). Without
    /// this, or --tls-cert, it's plain HTTP.
    #[arg(long, requires = "web")]
    https: bool,
    /// Serve the web UI over HTTPS with this certificate (PEM, with its
    /// chain) instead of rettui's own.
    #[arg(long, value_name = "FILE", requires_all = ["web", "tls_key"])]
    tls_cert: Option<PathBuf>,
    /// The private key (PEM) of --tls-cert.
    #[arg(long, value_name = "FILE", requires_all = ["web", "tls_cert"])]
    tls_key: Option<PathBuf>,
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
    Address {
        /// Print it as an lxma:// link with your public key (as Columba
        /// shares contacts), for others to add you.
        #[arg(long)]
        link: bool,
    },
    /// Send one LXMF message.
    Send {
        /// LXMF address, or an lxma:// link (with their key).
        address: String,
        message: String,
        /// Attach a file (repeatable).
        #[arg(long, short)]
        attach: Vec<PathBuf>,
        /// auto (direct, then propagation node), direct, propagated, or
        /// paper (not sent: prints an lxm:// link and its QR code).
        #[arg(long, default_value = "auto")]
        mode: String,
    },
    /// Announce and print incoming messages.
    Listen {
        /// How long to listen.
        #[arg(long, default_value_t = 300)]
        seconds: u64,
    },
    /// Ping an LXMF address: how long a Link to it takes to set up, and
    /// how many hops away it is.
    Ping { address: String },
    /// Find the path to any destination and print it, as rnpath does.
    ///
    /// One is asked for if none is known, up to three times over a minute.
    /// With rettui or rnsd running as the shared instance, these are its
    /// paths; else, those the last run with this Reticulum config saved.
    Path {
        /// The destination's address (LXMF, NomadNet node, propagation
        /// node, RRC hub…).
        #[arg(required_unless_present = "table")]
        address: Option<String>,
        /// Forget the path instead, so the next use asks for a fresh one
        /// (for one gone stale).
        #[arg(long, short, requires = "address")]
        drop: bool,
        /// Print every path known.
        #[arg(long, short, conflicts_with_all = ["address", "drop"])]
        table: bool,
    },
    /// Probe any destination, as rnprobe does: how long it takes to answer.
    ///
    /// LXMF addresses are sent a probe packet, as rnprobe sends. NomadNet
    /// and propagation nodes and RRC hubs don't answer those, so setting up
    /// a Link to one is timed instead.
    Probe {
        /// The destination's address (LXMF, NomadNet node, propagation
        /// node, RRC hub…).
        address: String,
        /// The destination's full name (e.g. rnsh.listen), for kinds
        /// rettui can't tell; it's sent a probe packet it must prove.
        #[arg(long)]
        name: Option<String>,
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
    // Everything goes to the log file; interface trouble also goes to the
    // log on screen.
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::sync::Mutex::new(log_file))
                .with_ansi(false)
                .with_filter(
                    tracing_subscriber::EnvFilter::try_from_env("RETTUI_LOG")
                        .unwrap_or_else(|_| "warn".into()),
                ),
        )
        .with(net::iface_log::layer())
        .init();

    let identity = config::load_identity(&paths.identity)?;

    if let Some(address) = cli.web {
        if cli.command.is_some() {
            bail!("--web runs the web UI; it cannot be combined with a command");
        }
        let https = match (cli.tls_cert, cli.tls_key) {
            (Some(cert), Some(key)) => Some(web::tls::Https::Files { cert, key }),
            _ => cli.https.then_some(web::tls::Https::Own),
        };
        return web::run(settings, paths, identity, &address, https).await;
    }

    match cli.command {
        Some(Command::Address { link }) => {
            let hash = rns_identity::destination::Destination::hash_from_name_and_identity(
                lxmf::LXMF_ASPECT,
                Some(&identity.hash),
            );
            if link {
                println!("{}", lxmf::identity_link(hash, &identity.get_public_key()));
            } else {
                println!("{}", hex::encode(hash));
            }
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
            let Some(mode) = lxmf::DeliveryMode::parse(&mode) else {
                bail!("unknown delivery mode {mode}; use auto, direct, propagated or paper");
            };
            cli::send(&settings, &paths, identity, &address, &message, attach, mode).await
        }
        Some(Command::Listen { seconds }) => cli::listen(&settings, &paths, identity, seconds).await,
        Some(Command::Sync { node }) => cli::sync(&settings, &paths, identity, node.as_deref()).await,
        Some(Command::Ping { address }) => cli::ping(&settings, &paths, identity, &address).await,
        Some(Command::Path { table: true, .. }) => cli::path_table(&settings).await,
        Some(Command::Path { address, drop, .. }) => {
            cli::path(&settings, address.as_deref().unwrap_or_default(), drop).await
        }
        Some(Command::Probe { address, name }) => cli::probe(&settings, &paths, &address, name.as_deref()).await,
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
        announce: settings.announce_schedule(),
        propagation_node: settings.propagation_node.as_deref().and_then(net::parse_hash),
        auto_propagation: settings.auto_propagation_node,
        sync_interval: (settings.sync_interval_mins > 0)
            .then(|| Duration::from_secs(settings.sync_interval_mins * 60)),
        known_identities: paths.known_identities.clone(),
        host: nomad::host::HostConfig::from_settings(settings, paths),
        propagation: lxmf::pn::PnConfig::from_settings(settings, paths),
        stamp_cost: lxmf::policy::stamp_cost(settings.stamp_cost),
        max_message_bytes: settings.max_message_kb * 1000,
        tickets: paths.tickets.clone(),
        ratchets: paths.ratchets.clone(),
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
    let mut app = App::new(settings, paths, store, net_tx.clone(), Some(graphics), identity_hash);
    app.open_newest_conversation();
    // Focus reports (where the terminal sends them) tell notifications
    // whether the user is looking.
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste, EnableFocusChange)?;
    // ratatui's panic hook restores the terminal; also release the mouse.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste, DisableFocusChange);
        hook(info);
    }));
    let desktop = term::desktop::Desktop::start(&app.paths.cache);
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut save = tokio::time::interval(Duration::from_secs(10));

    let mut decoded = app.take_decoded();
    let result = loop {
        // A full repaint is sent as one synchronized update, so terminals
        // that support it show no blank frame in between.
        let full = app.take_full_redraw();
        if full {
            let _ = execute!(std::io::stdout(), BeginSynchronizedUpdate);
            // Resizing to the same size clears the screen and forgets what
            // was drawn, so every cell is sent. (`clear` would first ask the
            // terminal for the cursor position, racing the input reader.)
            if let Ok(size) = terminal.size() {
                let _ = terminal.resize(ratatui::layout::Rect::new(0, 0, size.width, size.height));
            }
        }
        let drawn = terminal.draw(|frame| ui::draw(frame, &mut app));
        if full {
            let _ = execute!(std::io::stdout(), EndSynchronizedUpdate);
        }
        if let Err(e) = drawn {
            break Err(e.into());
        }
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => app.on_key(key),
                Some(Ok(Event::Mouse(mouse))) => app.on_mouse(mouse),
                Some(Ok(Event::Paste(text))) => app.on_paste(&text),
                Some(Ok(Event::FocusGained)) => app.set_focus(true),
                Some(Ok(Event::FocusLost)) => app.set_focus(false),
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
        for notification in app.take_notifications() {
            desktop.show(notification);
        }
        // Reads close notifications in the web UI's browsers; the desktop
        // keeps its own.
        app.take_reads();
        app.open_paper_read();
        if let Some(e) = desktop.failure() {
            app.log(format!("Could not show a desktop notification: {e}"));
        }
        while let Some(target) = desktop.clicked() {
            app.open_notified(target);
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

    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste, DisableFocusChange);
    ratatui::restore();
    app.save_if_dirty();
    // Leave hubs and stop Reticulum properly (a few seconds at most).
    net::shutdown(&net_tx, &mut net_rx, net::Stop::Quit).await;
    app.finish_saves();
    result
}
