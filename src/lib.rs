//! rettui: the terminal UI, the web UI and the commands. The `rettui`
//! program runs [`main`]; the Android app loads this as a library and runs
//! [`run_with`] (see `android`).

// The web UI's state (`web::views`) is one `json!` of more keys than the
// default limit lets the macro expand.
#![recursion_limit = "256"]

#[cfg(target_os = "android")]
mod android;
mod app;
mod backup;
mod cli;
mod clock;
mod config;
mod db;
mod emoji;
mod games;
mod highlight;
mod https;
mod icons;
mod identities;
mod limits;
mod links;
#[cfg(test)]
mod live_tests;
mod logging;
mod lxmf;
mod markdown;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod mobile_data;
mod names;
mod net;
mod node;
mod nomad;
// Remote management of `rettui node` (src/manage/). Not `manage`, which is
// the desktop app's name for what only its own window may do (below).
mod power;
#[path = "manage/mod.rs"]
mod remote_manage;
mod reticulum;
mod rrc;
mod stats;
mod store;
mod term;
#[cfg(windows)]
pub mod toast;
mod ui;
mod update;
mod web;

/// What only the desktop app's own window may do (see its docs).
pub use web::desktop as manage;
/// What the desktop app needs to check that what holds the web UI's port is rettui before it sends the login secret there.
pub use web::{hello_matches, hello_nonce, set_port_file};

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture, Event,
    EventStream, KeyEventKind,
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
#[command(
    version,
    about = "Reticulum client for the terminal and the browser: LXMF messaging, NomadNet browsing and hosting, and RRC chat"
)]
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
    /// Serve the web UI over plain HTTP on an address other devices can
    /// reach (such as 0.0.0.0). Without this, --https or --tls-cert,
    /// rettui refuses: the login link and cookie would cross the network
    /// unencrypted. Not needed on 127.0.0.1.
    #[arg(long, requires = "web")]
    insecure_http: bool,
    /// Serve the web UI over HTTPS with this certificate (PEM, with its
    /// chain) instead of rettui's own.
    #[arg(long, value_name = "FILE", requires_all = ["web", "tls_key"])]
    tls_cert: Option<PathBuf>,
    /// The private key (PEM) of --tls-cert.
    #[arg(long, value_name = "FILE", requires_all = ["web", "tls_cert"])]
    tls_key: Option<PathBuf>,
    /// Also serve a read-only JSON summary of this node (totals only, no
    /// login) at /stats.json on this address, for a status page. Put it
    /// behind a reverse proxy; don't expose it directly.
    #[arg(long, value_name = "ADDRESS", requires = "web")]
    stats_listen: Option<String>,
    /// A file with a secret (16 to 256 printable ASCII characters except
    /// spaces, quotes and backslash) that lets a request for the stats that presents it also list every peer
    /// heard. The variable RETTUI_STATS_TOKEN does the same; not both. Not saved.
    #[arg(long, value_name = "PATH", requires = "stats_listen")]
    stats_token_file: Option<PathBuf>,
    /// Also let the stats token read the end of the log at /logs. Off unless
    /// given, because the log can hold more than you want a status page to see.
    /// Not saved.
    #[arg(long, requires = "stats_listen")]
    stats_logs: bool,
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
    /// Check for a newer release, and install it in this one's place.
    ///
    /// Only the binaries from the releases page update themselves (checked
    /// against the release's SHA256SUMS, and run once before they replace
    /// this one); a build from source, a container or a package manager's
    /// says how to update instead. Start rettui again afterwards.
    Update {
        /// Install it without asking.
        #[arg(long, short)]
        yes: bool,
    },
    /// Back up rettui's settings, contacts, messages and identity to one
    /// file, to restore on another computer or after a reinstall.
    ///
    /// The identity is the private key behind your addresses: keep the
    /// file private. Attachments are left out unless asked for; caches,
    /// logs and the web UI's login aren't kept.
    Backup {
        /// The file to write (default: rettui-backup-<date>.tar.gz here).
        file: Option<PathBuf>,
        /// Include attachments received and sent.
        #[arg(long)]
        with_files: bool,
        /// Leave the identity out (restored, rettui makes a new one, with
        /// new addresses).
        #[arg(long)]
        without_identity: bool,
        /// Seal it with a passphrase, asked for here (or taken from
        /// RETTUI_BACKUP_PASSPHRASE): it can't be restored without it. In a
        /// terminal, rettui asks for one anyway (Enter for none).
        #[arg(long, conflicts_with_all = ["key", "no_passphrase"])]
        passphrase: bool,
        /// Seal it to a backup key (from `rettui backup-key`), for backups
        /// made without anyone there to type the passphrase.
        #[arg(long, value_name = "KEY", conflicts_with = "no_passphrase")]
        key: Option<String>,
        /// Don't ask for a passphrase: the backup isn't sealed.
        #[arg(long)]
        no_passphrase: bool,
    },
    /// Open a backup sealed with a passphrase without restoring it (and
    /// without rettui running): the plain backup to OUT, or only the
    /// identity in it, for Sideband, NomadNet or another rettui. It asks for
    /// the passphrase (or takes RETTUI_BACKUP_PASSPHRASE).
    Unseal {
        /// The sealed backup.
        file: PathBuf,
        /// Where to write the plain backup (a .tar.gz).
        #[arg(required_unless_present = "identity", conflicts_with = "identity")]
        out: Option<PathBuf>,
        /// Write only the identity in it, to this file.
        #[arg(long, value_name = "FILE")]
        identity: Option<PathBuf>,
    },
    /// Turn a passphrase into a backup key, to seal backups with
    /// (`rettui backup --key`) without the passphrase at hand. Restoring one
    /// still takes the passphrase.
    BackupKey,
    /// Restore a backup into the data directory, with rettui stopped.
    ///
    /// A data directory that has an identity or messages already is left
    /// alone, unless --force: then what the backup replaces is moved aside
    /// first, into a before-restore-<time> folder. A sealed backup asks for
    /// its passphrase (or takes it from RETTUI_BACKUP_PASSPHRASE).
    ///
    /// Settings that decide which programs run here (the voice recorder and
    /// print commands, page scripts, rncp, another Reticulum config) are
    /// left out of a restore, since a backup can come from anyone: with
    /// --keep-terminal-settings they are restored as the backup has them.
    Restore {
        file: PathBuf,
        /// Restore over what's there (moved aside first).
        #[arg(long)]
        force: bool,
        /// Restore the settings that decide which programs run, as written: only for a backup you made.
        #[arg(long)]
        keep_terminal_settings: bool,
    },
    /// Save a copy of your identity, or keep another from a file.
    ///
    /// The identity is the private key behind your addresses: whoever has
    /// it can pose as you. Reticulum programs keep it in the same format,
    /// so Sideband's, NomadNet's or MeshChat's file works here, and this
    /// one there.
    Identity {
        #[command(subcommand)]
        action: IdentityAction,
    },
    /// Save a copy of the settings, or bring settings in from a file.
    ///
    /// For the Android app, whose settings file can't be reached otherwise,
    /// and for any settings file rettui set aside because it couldn't read it.
    Settings {
        #[command(subcommand)]
        action: SettingsAction,
    },
    /// Convert the propagation node's message store to the 2.0 layout, with rettui stopped.
    ///
    /// Run it once if rettui says it can't open the message store. The
    /// messages kept before are moved to a `propagation.legacy-quarantine`
    /// folder beside the store and are not handed out again: nothing can
    /// show where they came from.
    MigratePropagation,
    /// Run a public node with no TUI or web UI, as rnsd and lxmd do together.
    ///
    /// A transport node with a TCP server interface others connect to (in
    /// gateway mode, on port 4242 unless given), and a propagation node.
    /// Only what's missing is added to the Reticulum config and the
    /// settings, and kept, so the next run needs no flags. It runs until
    /// SIGINT or SIGTERM, printing its status every 10 minutes. With no
    /// --rns-config and no rns_config in settings.json, it uses the
    /// Reticulum config in reticulum/ in the data directory.
    ///
    /// Each flag but --stats-listen and --stats-token-file can also be set as a variable:
    /// RETTUI_NODE_ENTRY_POINT, RETTUI_NODE_LISTEN_IP, RETTUI_NODE_REACHABLE_ON,
    /// RETTUI_NODE_NO_ENTRY_POINT, RETTUI_NODE_NAME and
    /// RETTUI_NODE_NO_PROPAGATION (switches take yes, true or 1, and no,
    /// false or 0). A flag wins over its variable, and an empty variable
    /// counts as not set.
    Node(node::NodeArgs),
    /// Download waiting messages from the propagation node.
    Sync {
        /// Propagation node to use instead of the configured one.
        #[arg(long)]
        node: Option<String>,
    },
}

#[derive(Subcommand)]
enum IdentityAction {
    /// Save a copy of the identity in use (never over another file).
    Export { file: PathBuf },
    /// Keep the identity in a file as another identity, to switch to (in
    /// Status > Identities). The one in use stays as it is.
    Import {
        file: PathBuf,
        /// The name it announces.
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
enum SettingsAction {
    /// Save a copy of the settings to a new file: it can hold a password (an
    /// MQTT server's) and your location, so keep it private.
    Export {
        file: PathBuf,
        /// The settings file rettui last set aside because it couldn't read it, not the ones in use.
        #[arg(long)]
        unreadable: bool,
    },
    /// Use the settings in a file, with rettui stopped. They're checked as a
    /// backup's are: a file rettui can't read is refused and nothing changes,
    /// and the settings that decide which programs run are left out (see
    /// --keep-terminal-settings of restore). The settings in use are kept as
    /// settings-before-import.json.
    Import {
        file: PathBuf,
        /// Use the settings that decide which programs run as the file has them: only for a file you made.
        #[arg(long)]
        keep_terminal_settings: bool,
    },
}

/// The `rettui` program: its arguments, and what they ask for.
pub fn main() -> Result<()> {
    start(Cli::parse())
}

/// rettui run with `args` (the first being the program's name), from
/// within another program: wrong arguments are an error, rather than
/// printed with the program ended.
pub fn run_with<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    start(Cli::try_parse_from(args)?)
}

/// A stop asked for from within (the Android app's Stop), as a signal is
/// from outside. Kept if asked before it's listened for.
static STOP: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Ask the web UI to stop: it saves everything, then [`run_with`] returns.
pub fn request_stop() {
    STOP.notify_one();
}

/// The desktop app's tray settings as they stand (see [`keep_running`] and
/// [`minimize_to_tray`]), changed while it runs.
static KEEP_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static MINIMIZE_TO_TRAY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn set_tray(settings: &Settings) {
    KEEP_RUNNING.store(settings.keep_running, std::sync::atomic::Ordering::Relaxed);
    MINIMIZE_TO_TRAY.store(settings.minimize_to_tray, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the desktop app keeps running when its window is closed (the
/// Keep running when closed setting).
pub fn keep_running() -> bool {
    KEEP_RUNNING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether the desktop app hides its window to the tray when it's
/// minimized (the Minimize to tray setting).
pub fn minimize_to_tray() -> bool {
    MINIMIZE_TO_TRAY.load(std::sync::atomic::Ordering::Relaxed)
}

/// This build's version, for the desktop app's `--version`.
pub const VERSION: &str = update::VERSION;

/// Say that rettui runs inside the desktop app, before [`run_with`]:
/// installing an update then replaces it with the release's
/// `rettui-desktop`, not `rettui`.
pub fn desktop_app() {
    update::install::DESKTOP_APP.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Where the web UI keeps its login secret, for a program that runs it
/// in-process and opens it itself (the desktop app, as the Android app
/// reads its own): `data_dir` as `--data-dir` would be given.
pub fn web_token_path(data_dir: Option<PathBuf>) -> Result<PathBuf> {
    Ok(Paths::new(data_dir)?.web_token)
}

pub(crate) async fn stop_requested() {
    STOP.notified().await;
}

fn start(mut cli: Cli) -> Result<()> {
    // A node's flags may come from RETTUI_NODE_* variables too; refused here, before anything is loaded.
    if let Some(Command::Node(args)) = &mut cli.command {
        *args = std::mem::take(args).with_env().map_err(anyhow::Error::msg)?;
    }
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run(cli))
}

async fn run(cli: Cli) -> Result<()> {
    // A usage error, said before anything is loaded or made: plain HTTP on an
    // address other devices can reach needs --https or --insecure-http. (The
    // web UI's own bind checks it again.)
    if let Some(address) = &cli.web {
        let tls = cli.https || cli.tls_cert.is_some();
        web::check_plain_http(address, tls, cli.insecure_http)?;
        web::accept_plain_http(tls || cli.insecure_http);
    }
    // Before the first socket or file: room for what peers and rettui itself open.
    let open_files = limits::raise_open_files();
    // What updating on Windows left (the program it replaced).
    update::install::tidy();
    let paths = Paths::new(cli.data_dir.clone())?;
    // Before anything is loaded (and, in a new data directory, made).
    match &cli.command {
        Some(Command::Backup { file, with_files, without_identity, passphrase, key, no_passphrase }) => {
            let options = backup::Options { identity: !without_identity, files: *with_files };
            let seal = match (key, passphrase, no_passphrase) {
                (Some(key), ..) => cli::Seal::Key(key.clone()),
                (None, true, _) => cli::Seal::Passphrase,
                (None, false, true) => cli::Seal::None,
                (None, false, false) => cli::Seal::Ask,
            };
            return cli::backup(&paths, file.clone(), options, seal);
        }
        Some(Command::BackupKey) => return cli::backup_key(),
        Some(Command::Unseal { file, out, identity }) => return cli::unseal(file, out.as_deref(), identity.as_deref()),
        Some(Command::Restore { file, force, keep_terminal_settings }) => {
            return cli::restore(&paths, file, *force, *keep_terminal_settings);
        }
        Some(Command::Settings { action: SettingsAction::Export { file, unreadable } }) => {
            return cli::export_settings(&paths, file, *unreadable);
        }
        Some(Command::Settings { action: SettingsAction::Import { file, keep_terminal_settings } }) => {
            return cli::import_settings(&paths, file, *keep_terminal_settings);
        }
        Some(Command::MigratePropagation) => return cli::migrate_propagation(&paths),
        Some(Command::Identity { action: IdentityAction::Export { file } }) => return cli::export_identity(&paths, file),
        Some(Command::Identity { action: IdentityAction::Import { file, name } }) => return cli::import_identity(&paths, file, name),
        _ => {}
    }
    // A node keeps a Reticulum config of its own unless told of another.
    let rns_config = match &cli.command {
        Some(Command::Node(_)) => node::rns_config(&paths, cli.rns_config.clone())?,
        _ => cli.rns_config.clone(),
    };
    let mut settings = load_settings(&paths, rns_config)?;

    // Kept under `logging::LOG_FILE_LIMIT`, with the file before it as `rettui.log.1`.
    let log_file = logging::CappedLog::open(&paths.log).with_context(|| format!("opening {}", paths.log.display()))?;
    // Everything goes to the log file, at the level set (changed while
    // running); interface trouble also goes to the log on screen.
    let (filter, reload) = tracing_subscriber::reload::Layer::new(logging::starting_filter(&settings.log_level));
    logging::set_reloader(move |level| reload.reload(logging::filter(level)).map_err(|e| e.to_string()));
    // Once a process: run again in the same one (as in tests), it logs
    // where the first run did.
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_writer(log_file).with_ansi(false).with_filter(filter))
        .with(net::iface_log::layer())
        .try_init();

    if let Some(limit) = open_files {
        tracing::debug!(limit, "raised the open-file limit");
    }
    let identity = config::load_identity(&paths.identity)?;
    settle_name(&mut settings, &paths, &identity);

    if let Some(address) = cli.web {
        if cli.command.is_some() {
            bail!("--web runs the web UI; it cannot be combined with a command");
        }
        let https = match (cli.tls_cert, cli.tls_key) {
            (Some(cert), Some(key)) => Some(web::tls::Https::Files { cert, key }),
            _ => cli.https.then_some(web::tls::Https::Own),
        };

        // Before anything starts: a bad token stops rettui.
        let stats_token = match &cli.stats_listen {
            Some(_) => stats::load_token(cli.stats_token_file.as_deref())?,
            None => None,
        };
        let (mut settings, mut paths, mut identity) = (settings, paths, identity);
        set_tray(&settings);
        // Switching identity starts the web UI over as the other one.
        // Bound once: the port stays held while it starts over as the other one.
        let listener = web::bind(&address).await?;
        let stats =
            || cli.stats_listen.as_deref().map(|address| stats::Listen { address, token: stats_token.clone(), logs: cli.stats_logs });
        while web::run(listener.try_clone()?, settings, paths, identity, https.clone(), stats()).await?.is_some() {
            (settings, paths, identity) = switched(&cli.data_dir, cli.rns_config.clone())?;
            set_tray(&settings);
        }
        return Ok(());
    }

    match cli.command {
        Some(Command::Address { link }) => {
            let hash = rns_identity::destination::Destination::hash_from_name_and_identity(lxmf::LXMF_ASPECT, Some(&identity.hash));
            if link {
                println!("{}", lxmf::identity_link(hash, &identity.get_public_key()));
            } else {
                println!("{}", hex::encode(hash));
            }
            Ok(())
        }
        Some(Command::Fetch { url, raw, identify, output }) => cli::fetch(&settings, &url, raw, identify.then_some(identity), output).await,
        Some(Command::Send { address, message, attach, mode }) => {
            let Some(mode) = lxmf::DeliveryMode::parse(&mode) else {
                bail!("unknown delivery mode {mode}; use auto, direct, propagated or paper");
            };
            cli::send(&settings, &paths, identity, &address, &message, attach, mode).await
        }
        Some(Command::Listen { seconds }) => cli::listen(&settings, &paths, identity, seconds).await,
        Some(Command::Sync { node }) => cli::sync(&settings, &paths, identity, node.as_deref()).await,
        Some(Command::Ping { address }) => cli::ping(&settings, &paths, identity, &address).await,
        Some(Command::Path { table: true, .. }) => cli::path_table(&settings).await,
        Some(Command::Path { address, drop, .. }) => cli::path(&settings, address.as_deref().unwrap_or_default(), drop).await,
        Some(Command::Probe { address, name }) => cli::probe(&settings, &paths, &address, name.as_deref()).await,
        Some(Command::Update { yes }) => cli::update(&settings, yes).await,
        Some(Command::Node(args)) => node::run(settings, paths, identity, &args).await,
        Some(
            Command::Backup { .. }
            | Command::BackupKey
            | Command::Unseal { .. }
            | Command::Restore { .. }
            | Command::MigratePropagation
            | Command::Settings { .. }
            | Command::Identity { .. },
        ) => {
            unreachable!("handled before loading")
        }
        None => {
            let (mut settings, mut paths, mut identity) = (settings, paths, identity);
            // Switching identity starts over as the other one.
            while run_tui(settings, paths, identity).await?.is_some() {
                (settings, paths, identity) = switched(&cli.data_dir, cli.rns_config.clone())?;
            }
            Ok(())
        }
    }
}

/// The settings in `paths` (with the Reticulum config given on the command
/// line, if one was), applied to what's global.
fn load_settings(paths: &Paths, rns_config: Option<String>) -> Result<Settings> {
    // In the Android app nobody can mend a settings file by hand, so one that
    // can't be read is set aside and the defaults used (the log says so).
    let (mut settings, note) = Settings::load_or_aside(&paths.settings, cfg!(target_os = "android"))?;
    if let Some(note) = note {
        config::note_at_start(note);
    }
    if rns_config.is_some() {
        settings.rns_config = rns_config;
    }
    if settings.rns_config.is_none() {
        settings.rns_config = config::default_rns_config();
    }
    clock::set(&settings.clock, &settings.date_style);
    ui::set_theme(&settings.tui_theme);
    // (Not before logging starts, which takes the level itself.)
    let _ = logging::set_level(&settings.log_level);
    Ok(settings)
}

/// A new identity, or one from before names were made from addresses, still
/// has "rettui user": give it the name its address makes, and keep it.
fn settle_name(settings: &mut Settings, paths: &Paths, identity: &rns_identity::identity::Identity) {
    if settings.name_by_address(&app::identity::lxmf_address(identity))
        && let Err(e) = settings.save(&paths.settings)
    {
        tracing::warn!("couldn't save the name {}: {e:#}", settings.display_name);
    }
}

/// After switching identity: the identity now in use, its data directory
/// and settings.
fn switched(data_dir: &Option<PathBuf>, rns_config: Option<String>) -> Result<(Settings, Paths, rns_identity::identity::Identity)> {
    let paths = Paths::new(data_dir.clone())?;
    let mut settings = load_settings(&paths, rns_config)?;
    let identity = config::load_identity(&paths.identity)?;
    settle_name(&mut settings, &paths, &identity);
    tracing::info!("now using the identity in {}", paths.base.display());
    Ok((settings, paths, identity))
}

/// Network options for an interactive session (the TUI or the web UI).
fn net_options(settings: &Settings, paths: &Paths, identity: rns_identity::identity::Identity, blocked: store::Blocked) -> net::NetOptions {
    let hub = rrc::host::HubHostConfig::from_settings(settings, paths, identity.hash);
    let git_host = net::githost::GitHostConfig::from_settings(settings, paths);
    net::NetOptions {
        blocked,
        rns_config: settings.rns_config.clone(),
        identity,
        display_name: settings.display_name.clone(),
        announce_at_start: settings.announce_at_start,
        announce: settings.announce_schedule(),
        propagation_node: settings.propagation_node.as_deref().and_then(net::parse_hash),
        auto_propagation: settings.auto_propagation_node,
        sync_interval: (settings.sync_interval_mins > 0).then(|| Duration::from_secs(settings.sync_interval_mins * 60)),
        db: paths.db.clone(),
        host: nomad::host::HostConfig::from_settings(settings, paths),
        propagation: lxmf::pn::PnConfig::from_settings(settings, paths),
        hub,
        git_host,
        rncp_listen: net::rncplisten::RncpListenConfig::from_settings(settings, paths),
        stamp_cost: lxmf::policy::stamp_cost(settings.stamp_cost),
        max_message_bytes: settings.max_message_kb * 1000,
        tickets: paths.tickets.clone(),
        ratchets: paths.ratchets.clone(),
        discovered_entry_points: settings.discovered_entry_points,
        disconnect_bootstrap_only: settings.disconnect_bootstrap_only,
        notes: android_start(settings, paths),
        downloading: paths.downloading.clone(),
    }
}

/// On a phone, before Reticulum starts: AutoInterface kept off mobile data
/// (its multicast there kept the mobile radio awake), and the locks the app
/// holds for rettui (see [`power`]). What was done, for the log.
fn android_start(settings: &Settings, paths: &Paths) -> Vec<String> {
    if !cfg!(target_os = "android") {
        return Vec::new();
    }
    let notes = app::reticulum::leave_out_mobile_data(settings.rns_config.as_deref(), paths.reticulum_history.clone());
    power::set_wake(settings.keep_awake);
    let text = reticulum::load(&reticulum::config_path(settings.rns_config.as_deref())).map(|(text, _)| text).unwrap_or_default();
    power::set_multicast(reticulum::has_auto_interface(&text));
    notes.into_iter().collect()
}

/// The terminal UI, until it quits; or the name of the identity it's
/// switching to.
async fn run_tui(settings: Settings, paths: Paths, identity: rns_identity::identity::Identity) -> Result<Option<String>> {
    let identity_hash = identity.hash;
    let store = Store::load_with_db(&paths.store, &paths.db);
    store.sync_blocked();
    let (mut net_tx, mut net_rx) = net::spawn(net_options(&settings, &paths, identity.clone(), store.blocked.clone()));
    // ratatui's panic hook gives the terminal back on a panic in any thread, but only the
    // UI thread's panic ends the UI: a worker's must not wreck the screen.
    let default_hook: std::sync::Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync> = std::sync::Arc::from(std::panic::take_hook());
    let kept = default_hook.clone();
    std::panic::set_hook(Box::new(move |info| kept(info)));
    let mut terminal = ratatui::init();
    let ratatui_hook = std::panic::take_hook();
    // Probe for Kitty graphics while in raw mode, before EventStream starts
    // reading stdin (it would swallow the terminal's replies).
    let graphics = term::images::Graphics::detect();
    let mut app = App::new(settings, paths, store, net_tx.clone(), Some(graphics), identity_hash);
    app.in_terminal = true;
    app.open_newest_conversation();
    // Focus reports (where the terminal sends them) tell notifications
    // whether the user is looking.
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste, EnableFocusChange)?;
    let ui_thread = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == ui_thread {
            let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste, DisableFocusChange);
            ratatui_hook(info);
        } else {
            // On the screen it would only be garbage; the log has it.
            tracing::error!("a thread panicked: {info}");
        }
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
                None => break Ok(None),
            },
            Some(event) = net_rx.recv() => {
                app.on_net(event);
                // Apply a burst of network events before redrawing.
                net::drain_burst(&mut net_rx, |event| app.on_net(event));
            }
            Some(image) = decoded.recv() => app.on_decoded(image),
            _ = tick.tick() => app.on_tick(),
            _ = save.tick() => app.save_if_dirty(),
            () = app.save_timer() => app.save_when_due(),
        }
        for notification in app.take_notifications() {
            desktop.show(notification);
        }
        // Reads close notifications in the web UI's browsers, and on the
        // desktop.
        for tag in app.take_reads() {
            desktop.withdraw(tag);
        }
        app.open_paper_read();
        if let Some(e) = desktop.failure() {
            app.log(format!("Could not show a desktop notification: {e}"));
        }
        while let Some(response) = desktop.response() {
            if let Err(e) = app.respond(response) {
                app.log(format!("A notification's action didn't work: {e}"));
            }
        }
        if app.should_quit {
            break Ok(None);
        }
        if let Some(name) = app.take_switch() {
            break Ok(Some(name));
        }
        if app.take_rns_restart() {
            // Show "restarting" while the old stack stops.
            let _ = terminal.draw(|frame| ui::draw(frame, &mut app));
            net::shutdown(&net_tx, &mut net_rx, net::Stop::Restart).await;
            (net_tx, net_rx) = net::spawn(net_options(&app.settings, &app.paths, identity.clone(), app.store.blocked.clone()));
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

#[cfg(test)]
mod tests {
    #[test]
    fn a_new_identity_is_named_from_its_address_and_keeps_it() {
        let dir = std::env::temp_dir().join(format!("rettui-settle-name-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = super::Paths::new(Some(dir.clone())).unwrap();
        let identity = super::config::load_identity(&paths.identity).unwrap();
        let mut settings = super::config::Settings::load(&paths.settings).unwrap();
        assert_eq!(settings.display_name, "rettui user");
        super::settle_name(&mut settings, &paths, &identity);
        let name = super::config::default_name(&super::app::identity::lxmf_address(&identity));
        assert_eq!(settings.display_name, name);
        assert_eq!(super::config::Settings::load(&paths.settings).unwrap().display_name, name);
        // Someone who chose a name keeps it.
        settings.display_name = "Zev".into();
        settings.save(&paths.settings).unwrap();
        let mut again = super::config::Settings::load(&paths.settings).unwrap();
        super::settle_name(&mut again, &paths, &identity);
        assert_eq!(super::config::Settings::load(&paths.settings).unwrap().display_name, "Zev");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn node_takes_its_flags_and_refuses_ones_that_disagree() {
        use clap::Parser;
        let node = |args: &[&str]| super::Cli::try_parse_from(["rettui", "node"].iter().chain(args)).map(|cli| (cli.command, cli.data_dir));
        let Ok((Some(super::Command::Node(args)), None)) = node(&[]) else { panic!("no flags is a node") };
        assert_eq!(args, super::node::NodeArgs::default());
        let Ok((Some(super::Command::Node(args)), Some(dir))) = node(&[
            "--entry-point",
            "4965",
            "--listen-ip",
            "::",
            "--reachable-on",
            "rns.example.org",
            "--name",
            "Hilltop",
            "--stats-listen",
            "127.0.0.1:8741",
            "--stats-token-file",
            "/run/secrets/stats",
            "--stats-logs",
            "--manage-listen",
            "10.200.0.2:8750",
            "--manage-secret-file",
            "/run/secrets/manage",
            "--data-dir",
            "/srv/rettui",
        ]) else {
            panic!("every flag")
        };
        assert_eq!(args.stats_listen.as_deref(), Some("127.0.0.1:8741"));
        assert_eq!(args.stats_token_file, Some(std::path::PathBuf::from("/run/secrets/stats")));
        assert!(args.stats_logs);
        assert_eq!(args.manage_listen.as_deref(), Some("10.200.0.2:8750"));
        assert_eq!(args.manage_secret_file, Some(std::path::PathBuf::from("/run/secrets/manage")));
        assert_eq!(dir, std::path::PathBuf::from("/srv/rettui"));
        assert_eq!(args.entry_point, Some(4965));
        assert_eq!(args.listen_ip, Some("::".parse().unwrap()));
        assert_eq!((args.reachable_on.as_deref(), args.name.as_deref()), (Some("rns.example.org"), Some("Hilltop")));
        for wrong in [
            &["--entry-point", "0"][..],
            &["--entry-point", "70000"],
            &["--listen-ip", "everywhere"],
            &["--no-entry-point", "--entry-point", "4242"],
            &["--no-entry-point", "--reachable-on", "x"],
            &["--no-propagation", "--name", "x"],
            &["--reachable-on", "a#b"],
            &["--reachable-on", "\"quoted\""],
            &["--stats-token-file", "/x"],
            &["--manage-listen", "127.0.0.1:8750"],
            &["--manage-secret-file", "/x"],
            &["--stats-logs"],
        ] {
            assert!(node(wrong).is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn the_desktop_app_finds_the_login_secret_in_the_data_folder() {
        let dir = std::env::temp_dir().join(format!("rettui-token-path-{}", std::process::id()));
        assert_eq!(super::web_token_path(Some(dir.clone())).unwrap(), dir.join("web_token"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_web_ui_stops_when_asked_from_within() {
        let dir = std::env::temp_dir().join(format!("rettui-run-with-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("rns")).unwrap();
        std::fs::write(dir.join("rns/config"), "[reticulum]\n  share_instance = No\n\n[interfaces]\n").unwrap();
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/settings.json"), r#"{"update_check": false, "welcomed": true}"#).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let args: Vec<String> =
            ["rettui", "--data-dir", &dir.join("data").display().to_string(), "--rns-config", &dir.join("rns").display().to_string()]
                .into_iter()
                .map(String::from)
                .chain(["--web".into(), format!("127.0.0.1:{port}")])
                .collect();
        let running = std::thread::spawn(move || super::run_with(args));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(std::time::Instant::now() < until && !running.is_finished(), "the web UI didn't start");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        super::request_stop();
        running.join().unwrap().unwrap();
        // Wrong arguments are an error, not the end of the program.
        assert!(super::run_with(["rettui", "--no-such-flag"]).is_err());
        // The stats endpoint is part of the web UI's run: alone, it's refused.
        let error = super::run_with(["rettui", "--stats-listen", "127.0.0.1:8741"]).unwrap_err();
        assert!(error.to_string().contains("--web"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_stats_token_stops_the_web_ui_before_it_starts() {
        let dir = std::env::temp_dir().join(format!("rettui-web-bad-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (file, data) = (dir.join("token"), dir.join("data").display().to_string());
        std::fs::write(&file, "short\n").unwrap();
        let file = file.display().to_string();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let web = format!("127.0.0.1:{port}");
        let error =
            super::run_with(["rettui", "--data-dir", &data, "--web", &web, "--stats-listen", "127.0.0.1:0", "--stats-token-file", &file])
                .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("--stats-token-file") && message.contains("at least 16") && !message.contains("short\n"), "{message}");
        // The token file needs the stats address.
        assert!(super::run_with(["rettui", "--web", &web, "--stats-token-file", &file]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
