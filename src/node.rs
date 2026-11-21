//! `rettui node`: a public node with no TUI and no web UI, as `rnsd` (a
//! transport node with a TCP server others connect to, in gateway mode)
//! and `lxmd` (a propagation node) are together.
//!
//! It sets the node up first, adding only what's missing to the Reticulum
//! config and the settings (never removing or changing what's there, beyond
//! turning on what a node is), so a later run with no flags is the same node.
//! Then it runs as the web UI does, without the HTTP server, until SIGINT
//! or SIGTERM, and saves everything on the way out. It has no login and
//! reads no input. The one thing it listens on that can change anything is
//! remote management, off unless `--manage-listen` and
//! `--manage-secret-file` are both given: signed requests only, for a few
//! settings and a Reticulum restart, and never what it runs (see
//! [`crate::remote_manage`]).

use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use rns_identity::identity::Identity;
use rns_runtime::config::{Config, ConfigSection};

use crate::app::App;
use crate::app::node::NodeStatus;
use crate::config::{Paths, Settings};
use crate::reticulum::doc::Doc;
use crate::store::Store;
use crate::{net, reticulum};

/// The port a new entry point listens on, Reticulum's usual one.
pub const DEFAULT_PORT: u16 = 4242;
/// The name of the TCP server interface the node adds (with a number after
/// it if that's taken), as the Entry point button names it.
const INTERFACE_NAME: &str = "Public entry point";
/// How often the status line is printed while it runs.
const STATUS_EVERY: Duration = Duration::from_secs(10 * 60);

/// What `rettui node` was asked to set up. Every flag but the stats and
/// manage ones is kept in the Reticulum config or the settings, so the next run needs
/// none of them. Each of those can also be given as a `RETTUI_NODE_*`
/// variable (see [`NodeArgs::with_vars`]), which a flag overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq, clap::Args)]
pub struct NodeArgs {
    /// The port others connect to. A TCP server interface on it is added
    /// unless one is there (default: an existing TCP server, else 4242).
    #[arg(long, value_name = "PORT", value_parser = clap::value_parser!(u16).range(1..))]
    pub entry_point: Option<u16>,
    /// The address a new TCP server interface listens on (default 0.0.0.0, every address).
    #[arg(long, value_name = "IP")]
    pub listen_ip: Option<IpAddr>,
    /// A host name or address others reach this node at: the TCP server
    /// interface is announced with it, for interface discovery.
    #[arg(long, value_name = "HOST", value_parser = parse_host)]
    pub reachable_on: Option<String>,
    /// Add no TCP server interface: others connect over the interfaces
    /// already in the Reticulum config.
    #[arg(long, conflicts_with_all = ["entry_point", "listen_ip", "reachable_on"])]
    pub no_entry_point: bool,
    /// The name the propagation node announces (default: the display name).
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
    /// Don't turn on the propagation node (one turned on before stays on:
    /// switch it off with pn_enabled in settings.json).
    #[arg(long, conflicts_with = "name")]
    pub no_propagation: bool,
    /// Also serve a read-only JSON summary of this node (totals only, no
    /// login) at /stats.json on this address, for a status page. Put it
    /// behind a reverse proxy; don't expose it directly. Not saved: give it
    /// on every run.
    #[arg(long, value_name = "ADDRESS")]
    pub stats_listen: Option<String>,
    /// A file with a secret (16 to 256 printable ASCII characters except
    /// spaces, quotes and backslash) that lets a request for the stats that presents it also list every peer
    /// heard. The variable RETTUI_STATS_TOKEN does the same; not both. Not saved.
    #[arg(long, value_name = "PATH", requires = "stats_listen")]
    pub stats_token_file: Option<std::path::PathBuf>,
    /// Also let the stats token read the end of the log at /logs. Off unless
    /// given, because the log can hold more than you want a status page to see.
    /// Not saved.
    #[arg(long, requires = "stats_listen")]
    pub stats_logs: bool,
    /// Let a website that holds the manage secret change a few settings and
    /// restart Reticulum, with signed requests to this address (a loopback
    /// or private one; 0.0.0.0 with a warning). Not saved: give it on every run.
    #[arg(long, value_name = "ADDRESS", requires = "manage_secret_file")]
    pub manage_listen: Option<String>,
    /// A file with the manage secret (32 to 256 printable ASCII characters
    /// except spaces, quotes and backslash), shared with the website only.
    /// Not saved.
    #[arg(long, value_name = "PATH", requires = "manage_listen")]
    pub manage_secret_file: Option<std::path::PathBuf>,
    /// Take management connections only from this address or network (as
    /// 10.0.0.5 or 10.0.0.0/24), the website's. Give it once for each; any
    /// other is closed before anything is read. Not saved.
    #[arg(long, value_name = "ADDRESS", requires = "manage_listen", value_parser = crate::remote_manage::Network::parse)]
    pub manage_allow: Vec<crate::remote_manage::Network>,
}

/// The variables that stand for the node's flags (not `--stats-listen`, which is for one run).
const ENV_ENTRY_POINT: &str = "RETTUI_NODE_ENTRY_POINT";
const ENV_LISTEN_IP: &str = "RETTUI_NODE_LISTEN_IP";
const ENV_REACHABLE_ON: &str = "RETTUI_NODE_REACHABLE_ON";
const ENV_NO_ENTRY_POINT: &str = "RETTUI_NODE_NO_ENTRY_POINT";
const ENV_NAME: &str = "RETTUI_NODE_NAME";
const ENV_NO_PROPAGATION: &str = "RETTUI_NODE_NO_PROPAGATION";

/// Which flags the command line gave, to say where a conflict came from.
struct Given {
    entry_point: bool,
    listen_ip: bool,
    reachable_on: bool,
    name: bool,
    no_entry_point: bool,
    no_propagation: bool,
}

impl NodeArgs {
    /// These flags, with what the command line left out taken from the
    /// `RETTUI_NODE_*` environment variables.
    pub fn with_env(self) -> Result<Self, String> {
        self.with_vars(|name| std::env::var_os(name))
    }

    /// Like [`NodeArgs::with_env`], reading variables with `get`. A flag wins
    /// over its variable; a variable that is empty counts as not set (Docker
    /// Compose passes those); a bad value is refused with the variable's
    /// name; and the flags that can't go together can't when they come from
    /// the environment, or one from each, either.
    ///
    /// This isn't clap's own `env`, which takes an empty value as a value,
    /// only reads `true` and `false` for a switch, and names the flag, not the
    /// variable, when one is bad.
    fn with_vars(mut self, get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<Self, String> {
        let var = |name: &str| -> Result<Option<String>, String> {
            match get(name) {
                None => Ok(None),
                Some(value) => match value.into_string() {
                    Ok(value) if value.is_empty() => Ok(None),
                    Ok(value) => Ok(Some(value)),
                    Err(_) => Err(format!("{name} isn't valid text")),
                },
            }
        };
        // Which of them the command line gave.
        let flags = Given {
            entry_point: self.entry_point.is_some(),
            listen_ip: self.listen_ip.is_some(),
            reachable_on: self.reachable_on.is_some(),
            name: self.name.is_some(),
            no_entry_point: self.no_entry_point,
            no_propagation: self.no_propagation,
        };
        let bad = |name: &str, value: &str, why: &str| format!("{name} is \"{value}\", but {why}");
        if self.entry_point.is_none() {
            self.entry_point = var(ENV_ENTRY_POINT)?
                .map(|value| match value.parse::<u16>() {
                    Ok(port) if port > 0 => Ok(port),
                    _ => Err(bad(ENV_ENTRY_POINT, &value, "it takes a port number from 1 to 65535")),
                })
                .transpose()?;
        }
        if self.listen_ip.is_none() {
            self.listen_ip = var(ENV_LISTEN_IP)?
                .map(|value| value.parse::<IpAddr>().map_err(|_| bad(ENV_LISTEN_IP, &value, "it takes an IP address")))
                .transpose()?;
        }
        if self.reachable_on.is_none() {
            self.reachable_on =
                var(ENV_REACHABLE_ON)?.map(|value| parse_host(&value).map_err(|why| bad(ENV_REACHABLE_ON, &value, &why))).transpose()?;
        }
        if self.name.is_none() {
            self.name = var(ENV_NAME)?;
        }
        for (on, name) in [(&mut self.no_entry_point, ENV_NO_ENTRY_POINT), (&mut self.no_propagation, ENV_NO_PROPAGATION)] {
            if !*on && let Some(value) = var(name)? {
                *on = match value.to_ascii_lowercase().as_str() {
                    "yes" | "true" | "1" => true,
                    "no" | "false" | "0" => false,
                    _ => return Err(bad(name, &value, "it takes yes, true or 1, or no, false or 0")),
                };
            }
        }
        // The command line's clap rules, for what the environment gave too.
        let origin = |flag: &str, name: &str, from_flag: bool| if from_flag { format!("--{flag}") } else { name.to_string() };
        let conflict = |a: String, b: String| Err(format!("{a} and {b} can't be used together"));
        if self.no_entry_point {
            let a = origin("no-entry-point", ENV_NO_ENTRY_POINT, flags.no_entry_point);
            let others = [
                (self.entry_point.is_some(), "entry-point", ENV_ENTRY_POINT, flags.entry_point),
                (self.listen_ip.is_some(), "listen-ip", ENV_LISTEN_IP, flags.listen_ip),
                (self.reachable_on.is_some(), "reachable-on", ENV_REACHABLE_ON, flags.reachable_on),
            ];
            if let Some((_, flag, name, from_flag)) = others.into_iter().find(|other| other.0) {
                return conflict(a, origin(flag, name, from_flag));
            }
        }
        if self.no_propagation && self.name.is_some() {
            return conflict(origin("no-propagation", ENV_NO_PROPAGATION, flags.no_propagation), origin("name", ENV_NAME, flags.name));
        }
        Ok(self)
    }
}

/// The Reticulum config directory the node uses when neither
/// `--rns-config` nor `rns_config` in settings.json names one: its own, in
/// the data directory, so making this a node changes no other program's
/// config (as Docker's entrypoint does with /data/reticulum).
pub fn own_rns_config(paths: &Paths) -> Result<Option<String>> {
    let saved = Settings::load(&paths.settings)?;
    Ok(saved.rns_config.is_none().then(|| own_dir(paths)))
}

/// The node's own Reticulum config directory, in the data directory.
fn own_dir(paths: &Paths) -> String {
    paths.root.join("reticulum").to_string_lossy().into_owned()
}

/// The interfaces in `config` that others connect to over TCP: TCP servers,
/// and Backbone interfaces with no remote. Their name, section and port.
fn tcp_listeners(config: &Config) -> Vec<(&str, &ConfigSection, Option<u64>)> {
    config
        .subsections("interfaces")
        .into_iter()
        .filter(|(_, section)| match section.get("type") {
            Some("TCPServerInterface") => true,
            Some("BackboneInterface") => section.get("remote").or_else(|| section.get("target_host")).is_none(),
            _ => false,
        })
        .map(|(name, section)| (name, section, section.get_uint("listen_port").or_else(|| section.get_uint("port"))))
        .collect()
}

/// The Reticulum config made a node's (see [`prepare_config`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigPlan {
    /// The new text.
    pub text: String,
    /// What was changed in it.
    pub edits: Vec<String>,
    /// What's worth knowing that wasn't changed (said every run).
    pub notices: Vec<String>,
}

/// Whether `host` can be written as `reachable_on`: a host name or an IP
/// address (an IPv6 one perhaps in brackets), nothing that could end the
/// value or start a comment or section in the config file.
fn valid_host(host: &str) -> bool {
    let inner = match host.strip_prefix('[') {
        Some(rest) => match rest.strip_suffix(']') {
            Some(inner) => inner,
            None => return false,
        },
        None => host,
    };
    !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '_' | '-'))
}

/// `--reachable-on` as the command line gives it: refused before anything
/// is loaded if it isn't a host.
fn parse_host(text: &str) -> Result<String, String> {
    let args = NodeArgs { reachable_on: Some(text.to_string()), ..NodeArgs::default() };
    reachable_on(&args).map(|host| host.unwrap_or_default().to_string())
}

/// `--reachable-on`, trimmed, if it was given and is a host.
fn reachable_on(args: &NodeArgs) -> Result<Option<&str>, String> {
    match args.reachable_on.as_deref().map(str::trim) {
        Some(host) if !valid_host(host) => Err(format!(
            "it takes a host name or an IP address (letters, digits, '.', '-', '_' and ':', \
             with [ ] only around an IPv6 address), not \"{host}\""
        )),
        host => Ok(host),
    }
}

/// Make the Reticulum config `text` a node's, as `args` asks: Transport node
/// on, and a TCP server interface in gateway mode (unless there is one, or
/// `--no-entry-point`). Only what's missing is added.
pub fn prepare_config(text: &str, args: &NodeArgs) -> Result<ConfigPlan, String> {
    let host = reachable_on(args)?;
    let config = Config::parse(text).map_err(|e| format!("The Reticulum config can't be read: {e}"))?;
    let mut doc = Doc::new(text);
    let mut changes = Vec::new();
    let mut notices = Vec::new();
    if config.section("reticulum").and_then(|r| r.get_bool("enable_transport")) != Some(true) {
        doc.set(&["reticulum"], "enable_transport", &[], Some("Yes"));
        changes.push("Transport node turned on (enable_transport = Yes)".to_string());
    }
    if args.no_entry_point {
        return Ok(ConfigPlan { text: doc.text(), edits: changes, notices });
    }
    let listeners = tcp_listeners(&config);
    let port = args.entry_point.map(u64::from);
    // One already on the port asked for; with none asked for, any that's on,
    // else one (perhaps off) on the default port.
    let found = listeners
        .iter()
        .find(|(_, section, on)| section.interface_enabled() && (port.is_none() || *on == port))
        .or_else(|| listeners.iter().find(|(_, _, on)| *on == Some(port.unwrap_or(u64::from(DEFAULT_PORT)))));
    match found {
        Some((name, section, _)) if !section.interface_enabled() => {
            notices.push(format!("{name} listens on that port but is off (enabled = No): left off, so others can't connect yet"));
        }
        Some((name, section, on)) => {
            let path = ["interfaces", *name];
            if doc.present_key(&path, &["mode", "interface_mode"]).is_none() {
                doc.set(&path, "mode", &["interface_mode"], Some("gateway"));
                changes.push(format!("{name}: mode = gateway, so it answers path requests for those connecting"));
            }
            if let Some(host) = host {
                if doc.present_key(&path, &["reachable_on", "discovery_reachable_on"]).is_none() {
                    doc.set(&path, "reachable_on", &["discovery_reachable_on"], Some(host));
                    changes.push(format!("{name}: reachable_on = {host}"));
                }
                if doc.present_key(&path, &["discoverable"]).is_none() {
                    doc.set(&path, "discoverable", &[], Some("Yes"));
                    changes.push(format!("{name}: discoverable = Yes"));
                }
            }
            if let Some(ip) = args.listen_ip
                && section.get("listen_ip").is_some_and(|was| was.parse::<IpAddr>().ok() != Some(ip))
            {
                notices.push(format!(
                    "{name} listens on {} already: --listen-ip left it as it is",
                    section.get("listen_ip").unwrap_or_default()
                ));
            }
            if let Some(on) = on {
                notices.push(format!("Entry point: {name}, on port {on}"));
            }
        }
        None => {
            let taken = doc.subsections("interfaces");
            let name = std::iter::once(INTERFACE_NAME.to_string())
                .chain((2..).map(|n| format!("{INTERFACE_NAME} {n}")))
                .find(|candidate| !taken.contains(candidate))
                .expect("some name is free");
            let port = args.entry_point.unwrap_or(DEFAULT_PORT).to_string();
            let ip = args.listen_ip.map_or_else(|| "0.0.0.0".to_string(), |ip| ip.to_string());
            let mut keys = vec![
                ("type", "TCPServerInterface"),
                ("enabled", "Yes"),
                ("mode", "gateway"),
                ("listen_ip", ip.as_str()),
                ("listen_port", port.as_str()),
            ];
            if let Some(host) = host {
                keys.push(("discoverable", "Yes"));
                keys.push(("reachable_on", host));
            }
            doc.add_subsection("interfaces", &name, &keys)?;
            changes.push(format!("Added {name}: a TCP server on {ip} port {port}, in gateway mode"));
        }
    }
    Ok(ConfigPlan { text: doc.text(), edits: changes, notices })
}

/// Make `settings` a node's, as `args` asks: the propagation node on (unless
/// `--no-propagation`), with its name. What changed, to print.
pub fn prepare_settings(settings: &mut Settings, args: &NodeArgs) -> Result<Vec<String>, String> {
    let mut changes = Vec::new();
    if !args.no_propagation && !settings.pn_enabled {
        settings.pn_enabled = true;
        changes.push("Propagation node turned on (pn_enabled)".to_string());
    }
    if let Some(name) = &args.name {
        let before = settings.pn_name.clone();
        settings.set_field("pn_name", name)?;
        if settings.pn_name != before {
            changes.push(format!("Propagation node named {}", settings.pn_name.as_deref().unwrap_or_default()));
        }
    }
    Ok(changes)
}

/// What setting the node up did: the changes saved (each after the file
/// it was saved in), and notices on what was left as it was.
#[derive(Debug, Default, PartialEq, Eq)]
struct Prepared {
    changes: Vec<String>,
    notices: Vec<String>,
}

/// Set up the node's Reticulum config (the one `settings` names) and its
/// settings, saving what changed.
fn prepare(settings: &mut Settings, paths: &Paths, args: &NodeArgs) -> Result<Prepared> {
    // Wrong flags before anything is touched.
    reachable_on(args).map_err(|e| anyhow!("--reachable-on: {e}"))?;
    let path = reticulum::config_path(settings.rns_config.as_deref());
    // A new config starts from rettui's default, as for the other modes.
    reticulum::seed_default(settings.rns_config.as_deref());
    let (before, existed) = reticulum::load(&path).map_err(|e| anyhow!(e))?;
    let plan = prepare_config(&before, args).map_err(|e| anyhow!("{e} ({})", path.display()))?;
    let (after, notices) = (plan.text, plan.notices);
    let mut changes = Vec::new();
    if after != before || !existed {
        // Restricted, as the web UI is: the node never turns on a program
        // Reticulum runs (a pipe interface).
        reticulum::validate(&before, &after, true).map_err(|e| anyhow!("{e} ({})", path.display()))?;
        let history = reticulum::history::History::new(paths.reticulum_history.clone());
        let now = crate::app::now();
        if let Err(e) = history.before_saving(&path, existed.then_some(before.as_str()), now) {
            tracing::warn!("{e}");
        }
        reticulum::save(&path, &after).map_err(|e| anyhow!(e))?;
        if let Err(e) = history.record(&path, &after, "Set up as a node (rettui node)", now) {
            tracing::warn!("{e}");
        }
        changes = plan.edits.into_iter().map(|c| format!("{}: {c}", path.display())).collect();
    }
    // The settings as saved, so values only in effect for this run (such as
    // --rns-config) aren't written to the file.
    let mut saved = Settings::load(&paths.settings)?;
    let mut said = prepare_settings(&mut saved, args).map_err(|e| anyhow!(e))?;
    // The node's own Reticulum config (see [`own_rns_config`]) is kept, so
    // that the TUI and the web UI on this data directory use it too.
    let own = own_dir(paths);
    if saved.rns_config.is_none() && settings.rns_config.as_deref() == Some(own.as_str()) {
        said.push(format!("rns_config = {own}, so rettui and rettui --web with this data directory use the node's Reticulum config too"));
        saved.rns_config = Some(own);
    }
    if !said.is_empty() {
        saved.save(&paths.settings)?;
        prepare_settings(settings, args).map_err(|e| anyhow!(e))?;
        changes.extend(said.into_iter().map(|c| format!("{}: {c}", paths.settings.display())));
    }
    Ok(Prepared { changes, notices })
}

/// Services the settings have on, which this node serves to anyone who
/// connects: one line each, with how to turn it off.
fn exposed_services(settings: &Settings) -> Vec<String> {
    let off = |key: &str| format!("set {key} to false in settings.json, with rettui stopped, to turn it off");
    let mut lines = Vec::new();
    if settings.node_enabled {
        lines.push(format!("Serving a NomadNet node (node_enabled): its pages and files are public; {}", off("node_enabled")));
        if settings.node_executable_pages {
            lines.push(format!(
                "Page scripts are on (node_executable_pages): they run on this computer for any peer who asks for them; {}",
                off("node_executable_pages")
            ));
        }
    }
    if settings.hub_enabled {
        lines.push(format!("Hosting an RRC hub (hub_enabled): anyone can join its rooms; {}", off("hub_enabled")));
    }
    if settings.git_host_enabled {
        lines.push(format!(
            "Hosting rngit repositories (git_host_enabled): cloned by whoever their .allowed files and the access rule let; {}",
            off("git_host_enabled")
        ));
    }
    if settings.rncp_listen {
        lines.push(format!("Receiving files with rncp (rncp_listen): from the identities in rncp_allowed; {}", off("rncp_listen")));
    }
    lines
}

/// Said on a run that set the node up: what it makes public.
fn public_note(settings: &Settings, identity: &Identity) -> String {
    let lxmf = hex::encode(crate::app::identity::lxmf_address(identity));
    let pn = match settings.pn_name.as_deref().filter(|n| !n.trim().is_empty()) {
        Some(name) => format!("the propagation node announces itself as \"{name}\""),
        None => "the propagation node announces the display name too, unless --name gives it one of its own".to_string(),
    };
    format!(
        "Note: a node announces itself: its identity, LXMF address {lxmf} and display name \"{}\" are public, and {pn}",
        settings.display_name
    )
}

/// The Reticulum config directory a node uses: the one given on the command
/// line, else the one in settings.json, else its own (see [`own_rns_config`]).
pub fn rns_config(paths: &Paths, given: Option<String>) -> Result<Option<String>> {
    match given {
        Some(given) => Ok(Some(given)),
        None => own_rns_config(paths),
    }
}

/// `rettui node`: set it up, then run it until SIGINT or SIGTERM.
pub async fn run(mut settings: Settings, paths: Paths, identity: Identity, args: &NodeArgs) -> Result<()> {
    // Before anything starts: a bad token stops the node.
    let stats = match args.stats_listen.as_deref() {
        Some(address) => Some(crate::stats::Listen {
            address,
            token: crate::stats::load_token(args.stats_token_file.as_deref())?,
            logs: args.stats_logs,
        }),
        None => None,
    };
    let manage = match (&args.manage_listen, &args.manage_secret_file) {
        (Some(address), Some(file)) => {
            // A public address stops the node here too, before it is set up.
            if let Ok(address) = address.parse() {
                crate::remote_manage::check_bind(&address)?;
            }
            Some(crate::remote_manage::Listen {
                address: address.clone(),
                secret: crate::remote_manage::load_secret(file)?,
                allow: args.manage_allow.clone(),
            })
        }
        _ => None,
    };
    check_secrets_differ(stats.as_ref(), manage.as_ref())?;
    // Listened for from now, so a stop while it starts is a clean one.
    let stop = crate::web::shutdown_signal();
    let prepared = prepare(&mut settings, &paths, args)?;
    for change in &prepared.changes {
        println!("{change}");
    }
    for notice in &prepared.notices {
        println!("{notice}");
    }
    if !prepared.changes.is_empty() {
        println!("{}", public_note(&settings, &identity));
    }
    for line in exposed_services(&settings) {
        println!("{line}");
    }
    serve(settings, paths, identity, stop, STATUS_EVERY, Listeners { stats, manage }, |_| {}).await
}

/// Refuse a manage secret that is the stats token too. The stats token is
/// sent with every stats request and may sit in a proxy's config, so it
/// mustn't also sign management requests. The error never repeats either.
fn check_secrets_differ(stats: Option<&crate::stats::Listen<'_>>, manage: Option<&crate::remote_manage::Listen>) -> Result<()> {
    if let (Some(token), Some(manage)) = (stats.and_then(|s| s.token.as_ref()), manage)
        && manage.secret.is_token(token)
    {
        bail!(
            "--manage-secret-file holds the same text as the stats token: use a different secret for each, so whoever has the stats token can't manage the node"
        );
    }
    Ok(())
}

/// What the node listens on besides Reticulum: the stats page and remote
/// management, each if asked for.
#[derive(Debug, Default)]
pub(crate) struct Listeners<'a> {
    pub stats: Option<crate::stats::Listen<'a>>,
    pub manage: Option<crate::remote_manage::Listen>,
}

/// The node, until `stop`: what the web UI runs, without its HTTP server.
/// The status line is printed at the start, then every `every`. `stopping`
/// is given the app once it's stopped, before it saves (for tests). With
/// `stats` (an address, and perhaps a token), the stats page is served there (see [`crate::stats`]),
/// and with `manage` remote management (see [`crate::remote_manage`]), whose jobs run here.
pub(crate) async fn serve(
    settings: Settings,
    paths: Paths,
    identity: Identity,
    stop: impl std::future::Future<Output = ()>,
    every: Duration,
    listeners: Listeners<'_>,
    stopping: impl FnOnce(&mut App),
) -> Result<()> {
    let Listeners { stats, manage } = listeners;
    let rns_path = reticulum::config_path(settings.rns_config.as_deref());
    let listening =
        std::fs::read_to_string(&rns_path).ok().and_then(|text| Config::parse(&text).ok()).map(|c| listening(&c)).unwrap_or_default();
    // Before anything starts, so a bad or taken address stops rettui.
    let started = std::time::Instant::now();
    let (stats_tx, stats_rx) = crate::stats::channel(Default::default());
    let disk = crate::stats::Disk::default();
    let with_peers = stats.as_ref().is_some_and(|listen| listen.token.is_some());
    let with_logs = with_peers && stats.as_ref().is_some_and(|listen| listen.logs);
    let stats_server = match stats {
        Some(listen) => {
            let (bound, task) = crate::stats::serve(
                listen.address,
                listen.token,
                stats_rx,
                with_logs.then(|| crate::stats::LogFiles { log: paths.log.clone(), web_token: Some(paths.web_token.clone()) }),
            )
            .await?;
            println!("{}", crate::stats::started_line(bound, with_peers, with_logs));
            Some(task)
        }
        None => None,
    };
    let (manage_server, mut manage_jobs, mut owner) = match manage {
        Some(listen) => {
            let allow = listen.allow.clone();
            let (bound, task, jobs) = crate::remote_manage::serve(listen, &paths).await?;
            println!("{}", crate::remote_manage::started_line(bound, &allow));
            (Some(task), Some(jobs), Some(crate::remote_manage::Owner::new(&paths)))
        }
        None => (None, None, None),
    };
    let identity_hash = identity.hash;
    let store = Store::load_with_db(&paths.store, &paths.db);
    store.sync_blocked();
    let (mut net_tx, mut net_rx) = net::spawn(crate::net_options(&settings, &paths, identity.clone(), store.blocked.clone()));
    let mut app = App::new(settings, paths, store, net_tx.clone(), None, identity_hash);
    // No tab is on screen, as in the web UI.
    app.tab = crate::app::Tab::Status;
    println!("rettui node {} started, with the Reticulum config in {}", crate::VERSION, rns_path.display());
    println!("{}", status_line(&app, &identity, &listening));

    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut save = tokio::time::interval(Duration::from_secs(10));
    let mut status = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    let mut stats_tick = tokio::time::interval(Duration::from_secs(1));
    tokio::pin!(stop);
    let mut decoded = app.take_decoded();
    loop {
        tokio::select! {
            Some(event) = net_rx.recv() => {
                app.on_net(event);
                net::drain_burst(&mut net_rx, |event| app.on_net(event));
            }
            Some(image) = decoded.recv() => app.on_decoded(image),
            _ = tick.tick() => {
                app.on_tick();
                if let Some(owner) = &mut owner {
                    owner.on_tick(&app);
                }
            }
            Some(job) = crate::remote_manage::next_job(&mut manage_jobs) => {
                if let Some(owner) = &mut owner {
                    owner.run(&mut app, job);
                }
            }
            _ = save.tick() => app.save_if_dirty(),
            () = app.save_timer() => app.save_when_due(),
            _ = status.tick() => println!("{}", status_line(&app, &identity, &listening)),
            _ = stats_tick.tick(), if stats_server.is_some() => {
                stats_tx.send_replace(crate::stats::snapshot(&app, started, with_peers, &disk));
            }
            () = &mut stop => break,
        }
        let stopped = if stats_server.as_ref().is_some_and(|task| task.is_finished()) {
            Some("the stats server stopped")
        } else if manage_server.as_ref().is_some_and(|task| task.is_finished()) {
            Some("the remote management listener stopped")
        } else {
            None
        };
        if let Some(stopped) = stopped {
            app.save_if_dirty();
            net::shutdown(&net_tx, &mut net_rx, net::Stop::Quit).await;
            app.finish_saves();
            return Err(anyhow!(stopped));
        }
        print_log(&mut app);
        // Nobody is there to be shown these. Switching identity and quitting
        // are only asked for from a UI, and `watched` stays false (no one is
        // looking, so peers are saved less often), so those aren't checked.
        app.take_notifications();
        app.take_reads();
        app.take_autoplays();
        app.take_cleanup_news();
        app.paper_read = None;
        if app.take_rns_restart() {
            net::shutdown(&net_tx, &mut net_rx, net::Stop::Restart).await;
            (net_tx, net_rx) = net::spawn(crate::net_options(&app.settings, &app.paths, identity.clone(), app.store.blocked.clone()));
            app.set_network(net_tx.clone());
        }
    }
    for task in stats_server.iter().chain(&manage_server) {
        task.abort();
    }
    println!("rettui node stopping: saving, and leaving the network");
    stopping(&mut app);
    app.save_if_dirty();
    net::shutdown(&net_tx, &mut net_rx, net::Stop::Quit).await;
    app.finish_saves();
    print_log(&mut app);
    println!("rettui node stopped");
    Ok(())
}

/// What the app logged (shown on Status in the UIs), for the service's log.
fn print_log(app: &mut App) {
    for line in app.log.drain(..) {
        println!("{line}");
    }
}

/// The TCP servers in the config that are on: where each listens.
fn listening(config: &Config) -> Vec<String> {
    tcp_listeners(config)
        .into_iter()
        .filter(|(_, section, _)| section.interface_enabled())
        .map(|(name, section, port)| {
            let ip = section.get("listen_ip").or_else(|| section.get("listen_on")).or_else(|| section.get("device")).unwrap_or("0.0.0.0");
            format!("{name} on {ip}:{}", port.map_or_else(|| "?".to_string(), |p| p.to_string()))
        })
        .collect()
}

/// One line on how the node is: its addresses, the propagation node, where
/// it listens and its interfaces.
fn status_line(app: &App, identity: &Identity, listening: &[String]) -> String {
    let lxmf = hex::encode(crate::app::identity::lxmf_address(identity));
    let pn = match (&app.pn.status, &app.pn.stats) {
        (NodeStatus::Off, _) => "off".to_string(),
        (NodeStatus::Starting, _) => format!("{} (starting)", hex::encode(app.pn.hash)),
        (NodeStatus::Running, Some(stats)) => format!(
            "{} ({} messages held, {} peers connected, peered with {})",
            hex::encode(app.pn.hash),
            stats.messages,
            stats.peers,
            stats.peered
        ),
        (NodeStatus::Running, None) => format!("{} (running)", hex::encode(app.pn.hash)),
        (NodeStatus::Failed(e), _) => format!("{} (failed: {e})", hex::encode(app.pn.hash)),
    };
    let listening = if listening.is_empty() { "nowhere (no TCP server interface is on)".to_string() } else { listening.join(", ") };
    let interfaces = if app.interfaces.is_empty() {
        "not up yet".to_string()
    } else {
        let online = app.interfaces.iter().filter(|i| i.online).count();
        let clients: u64 = app.interfaces.iter().filter_map(|i| i.clients).sum();
        format!("{online} of {} online, {clients} clients connected", app.interfaces.len())
    };
    format!("rettui node: LXMF {lxmf}; propagation node {pn}; listening {listening}; interfaces {interfaces}")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The node's flags with `vars` as the environment.
    fn with_vars(args: NodeArgs, vars: &[(&str, &str)]) -> Result<NodeArgs, String> {
        args.with_vars(|name| vars.iter().find(|(var, _)| *var == name).map(|(_, value)| value.into()))
    }

    fn env(vars: &[(&str, &str)]) -> Result<NodeArgs, String> {
        with_vars(NodeArgs::default(), vars)
    }

    #[test]
    fn each_variable_sets_its_flag() {
        assert_eq!(env(&[]).unwrap(), NodeArgs::default());
        assert_eq!(env(&[(ENV_ENTRY_POINT, "4965")]).unwrap().entry_point, Some(4965));
        assert_eq!(env(&[(ENV_ENTRY_POINT, "65535")]).unwrap().entry_point, Some(65535));
        assert_eq!(env(&[(ENV_LISTEN_IP, "::1")]).unwrap().listen_ip, Some("::1".parse().unwrap()));
        assert_eq!(env(&[(ENV_REACHABLE_ON, "rns.example.org")]).unwrap().reachable_on.as_deref(), Some("rns.example.org"));
        assert_eq!(env(&[(ENV_NAME, "Hilltop node")]).unwrap().name.as_deref(), Some("Hilltop node"));
        assert!(env(&[(ENV_NO_ENTRY_POINT, "yes")]).unwrap().no_entry_point);
        assert!(env(&[(ENV_NO_PROPAGATION, "TRUE")]).unwrap().no_propagation);
        for on in ["yes", "Yes", "true", "TRUE", "1"] {
            assert!(env(&[(ENV_NO_PROPAGATION, on)]).unwrap().no_propagation, "{on}");
        }
        for off in ["no", "No", "false", "FALSE", "0"] {
            assert!(!env(&[(ENV_NO_PROPAGATION, off)]).unwrap().no_propagation, "{off}");
        }
        // A switch that is off lets the others stand.
        let args = env(&[(ENV_NO_PROPAGATION, "no"), (ENV_NAME, "x"), (ENV_NO_ENTRY_POINT, "0"), (ENV_ENTRY_POINT, "5")]).unwrap();
        assert_eq!((args.name.as_deref(), args.entry_point), (Some("x"), Some(5)));
    }

    #[test]
    fn a_flag_beats_its_variable() {
        let flags = NodeArgs {
            entry_point: Some(1),
            listen_ip: Some("10.0.0.1".parse().unwrap()),
            reachable_on: Some("flag.example".into()),
            name: Some("flag".into()),
            ..NodeArgs::default()
        };
        let vars = [(ENV_ENTRY_POINT, "2"), (ENV_LISTEN_IP, "10.0.0.2"), (ENV_REACHABLE_ON, "env.example"), (ENV_NAME, "env")];
        assert_eq!(with_vars(flags.clone(), &vars).unwrap(), flags);
        // Even a bad variable is not looked at when the flag was given.
        assert_eq!(with_vars(flags.clone(), &[(ENV_ENTRY_POINT, "nope")]).unwrap(), flags);
        let on = NodeArgs { no_propagation: true, ..NodeArgs::default() };
        assert_eq!(with_vars(on.clone(), &[(ENV_NO_PROPAGATION, "no")]).unwrap(), on);
    }

    #[test]
    fn an_empty_variable_is_not_set() {
        let vars = [
            (ENV_ENTRY_POINT, ""),
            (ENV_LISTEN_IP, ""),
            (ENV_REACHABLE_ON, ""),
            (ENV_NO_ENTRY_POINT, ""),
            (ENV_NAME, ""),
            (ENV_NO_PROPAGATION, ""),
        ];
        assert_eq!(env(&vars).unwrap(), NodeArgs::default());
        // So an empty one doesn't clash with a set one.
        assert!(env(&[(ENV_NO_PROPAGATION, "yes"), (ENV_NAME, "")]).unwrap().no_propagation);
        assert_eq!(env(&[(ENV_NO_ENTRY_POINT, "yes"), (ENV_ENTRY_POINT, ""), (ENV_LISTEN_IP, "")]).unwrap().entry_point, None);
    }

    #[test]
    fn a_bad_variable_is_refused_by_name() {
        for (name, value) in [
            (ENV_ENTRY_POINT, "0"),
            (ENV_ENTRY_POINT, "65536"),
            (ENV_ENTRY_POINT, "-1"),
            (ENV_ENTRY_POINT, "http"),
            (ENV_ENTRY_POINT, " 4242"),
            (ENV_LISTEN_IP, "localhost"),
            (ENV_LISTEN_IP, "300.1.1.1"),
            (ENV_REACHABLE_ON, "bad host"),
            (ENV_REACHABLE_ON, "a#b"),
            (ENV_REACHABLE_ON, "[::1"),
            (ENV_NO_ENTRY_POINT, "maybe"),
            (ENV_NO_PROPAGATION, "on"),
            (ENV_NO_PROPAGATION, "2"),
        ] {
            let error = env(&[(name, value)]).unwrap_err();
            assert!(error.contains(name) && error.contains(value), "{name}={value}: {error}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_variable_that_is_not_text_is_refused() {
        use std::os::unix::ffi::OsStringExt;
        let error =
            NodeArgs::default().with_vars(|name| (name == ENV_NAME).then(|| std::ffi::OsString::from_vec(vec![0xff, 0xfe]))).unwrap_err();
        assert!(error.contains(ENV_NAME), "{error}");
    }

    #[test]
    fn variables_that_disagree_are_refused() {
        let error = env(&[(ENV_NO_ENTRY_POINT, "yes"), (ENV_ENTRY_POINT, "4242")]).unwrap_err();
        assert!(error.contains(ENV_NO_ENTRY_POINT) && error.contains(ENV_ENTRY_POINT), "{error}");
        assert!(env(&[(ENV_NO_ENTRY_POINT, "1"), (ENV_LISTEN_IP, "0.0.0.0")]).is_err());
        assert!(env(&[(ENV_NO_ENTRY_POINT, "1"), (ENV_REACHABLE_ON, "a.example")]).is_err());
        let error = env(&[(ENV_NO_PROPAGATION, "yes"), (ENV_NAME, "Hilltop")]).unwrap_err();
        assert!(error.contains(ENV_NO_PROPAGATION) && error.contains(ENV_NAME), "{error}");
        // One from the command line and one from the environment, either way round.
        let flag = NodeArgs { no_entry_point: true, ..NodeArgs::default() };
        let error = with_vars(flag, &[(ENV_ENTRY_POINT, "4242")]).unwrap_err();
        assert!(error.contains("--no-entry-point") && error.contains(ENV_ENTRY_POINT), "{error}");
        let flag = NodeArgs { entry_point: Some(4242), ..NodeArgs::default() };
        let error = with_vars(flag, &[(ENV_NO_ENTRY_POINT, "yes")]).unwrap_err();
        assert!(error.contains(ENV_NO_ENTRY_POINT) && error.contains("--entry-point"), "{error}");
        let flag = NodeArgs { name: Some("x".into()), ..NodeArgs::default() };
        assert!(with_vars(flag, &[(ENV_NO_PROPAGATION, "true")]).is_err());
        let flag = NodeArgs { no_propagation: true, ..NodeArgs::default() };
        let error = with_vars(flag, &[(ENV_NAME, "x")]).unwrap_err();
        assert!(error.contains("--no-propagation") && error.contains(ENV_NAME), "{error}");
        // A switch turned off doesn't clash.
        assert!(env(&[(ENV_NO_PROPAGATION, "no"), (ENV_NAME, "x")]).is_ok());
    }

    #[test]
    fn the_real_environment_is_read() {
        // What the variables are here is anyone's guess; this only checks the wiring.
        if let Err(error) = NodeArgs::default().with_env() {
            assert!(error.contains("RETTUI_NODE_"), "{error}");
        }
    }

    /// Where a test's node keeps everything.
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rettui-node-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const BASE: &str = "[reticulum]\n  enable_transport = False\n  share_instance = No\n\n[interfaces]\n  [[Default Interface]]\n    type = AutoInterface\n    enabled = Yes\n";

    fn parsed(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    fn interface<'a>(config: &'a Config, name: &str) -> &'a ConfigSection {
        config.subsection("interfaces", name).unwrap()
    }

    #[test]
    fn a_config_becomes_a_gateway_entry_point_on_4242_once() {
        let ConfigPlan { text, edits, notices } = prepare_config(BASE, &NodeArgs::default()).unwrap();
        assert_eq!(edits.len(), 2, "{edits:?}");
        assert!(notices.is_empty(), "{notices:?}");
        let config = parsed(&text);
        assert_eq!(config.section("reticulum").unwrap().get_bool("enable_transport"), Some(true));
        let server = interface(&config, "Public entry point");
        assert_eq!(server.get("type"), Some("TCPServerInterface"));
        assert_eq!(server.get("mode"), Some("gateway"));
        assert_eq!(server.get("listen_ip"), Some("0.0.0.0"));
        assert_eq!(server.get_uint("listen_port"), Some(4242));
        assert!(server.interface_enabled());
        assert!(server.get("reachable_on").is_none());
        assert!(text.contains("AutoInterface"), "what was there stays");
        assert!(reticulum::validate(BASE, &text, true).is_ok());
        // Run again, nothing more is added.
        let again = prepare_config(&text, &NodeArgs::default()).unwrap();
        assert_eq!(again.text, text);
        assert!(again.edits.is_empty(), "{:?}", again.edits);
        assert_eq!(again.notices, vec!["Entry point: Public entry point, on port 4242".to_string()]);
    }

    #[test]
    fn flags_set_the_port_address_and_host_and_a_later_run_without_them_keeps_them() {
        let args = NodeArgs {
            entry_point: Some(4965),
            listen_ip: Some("192.0.2.7".parse().unwrap()),
            reachable_on: Some(" rns.example.org ".into()),
            ..NodeArgs::default()
        };
        let text = prepare_config(BASE, &args).unwrap().text;
        let config = parsed(&text);
        let server = interface(&config, "Public entry point");
        assert_eq!(server.get_uint("listen_port"), Some(4965));
        assert_eq!(server.get("listen_ip"), Some("192.0.2.7"));
        assert_eq!(server.get("reachable_on"), Some("rns.example.org"));
        assert_eq!(server.get_bool("discoverable"), Some(true));
        // No flags: the server there is the entry point, not a new one on 4242.
        assert_eq!(prepare_config(&text, &NodeArgs::default()).unwrap().text, text);
        // The same port again changes nothing either.
        assert_eq!(prepare_config(&text, &args).unwrap().text, text);
    }

    #[test]
    fn what_the_operator_set_on_a_server_stays_and_only_whats_missing_is_added() {
        let own = format!(
            "{BASE}  [[My server]]\n    type = TCPServerInterface\n    interface_enabled = Yes\n    interface_mode = boundary\n    listen_ip = 10.0.0.1\n    port = 5000\n    reachable_on = mine.example\n"
        );
        let args =
            NodeArgs { listen_ip: Some("0.0.0.0".parse().unwrap()), reachable_on: Some("other.example".into()), ..NodeArgs::default() };
        let ConfigPlan { text, edits, notices } = prepare_config(&own, &args).unwrap();
        let config = parsed(&text);
        assert_eq!(config.subsections("interfaces").len(), 2, "no second server");
        let server = interface(&config, "My server");
        assert_eq!(server.get("interface_mode"), Some("boundary"));
        assert!(server.get("mode").is_none());
        assert_eq!(server.get("listen_ip"), Some("10.0.0.1"));
        assert_eq!(server.get("reachable_on"), Some("mine.example"));
        // Only discoverable was missing.
        assert_eq!(server.get_bool("discoverable"), Some(true));
        assert_eq!(edits.last().map(String::as_str), Some("My server: discoverable = Yes"));
        assert_eq!(edits.len(), 2, "transport, and discoverable: {edits:?}");
        assert!(notices.iter().any(|c| c.contains("left it as it is")), "{notices:?}");

        // Without a mode, it's given gateway.
        let plain = own.replace("    interface_mode = boundary\n", "");
        let text = prepare_config(&plain, &NodeArgs::default()).unwrap().text;
        assert_eq!(interface(&parsed(&text), "My server").get("mode"), Some("gateway"));
    }

    #[test]
    fn another_port_gets_a_server_of_its_own_and_the_first_is_kept() {
        let first = prepare_config(BASE, &NodeArgs::default()).unwrap().text;
        let ConfigPlan { text, edits: changes, .. } =
            prepare_config(&first, &NodeArgs { entry_point: Some(4243), ..NodeArgs::default() }).unwrap();
        let config = parsed(&text);
        assert_eq!(interface(&config, "Public entry point").get_uint("listen_port"), Some(4242));
        assert_eq!(interface(&config, "Public entry point 2").get_uint("listen_port"), Some(4243));
        assert!(changes[0].starts_with("Added Public entry point 2"), "{changes:?}");
    }

    #[test]
    fn a_server_turned_off_is_left_off_and_not_doubled() {
        let off = format!("{BASE}  [[Off]]\n    type = TCPServerInterface\n    enabled = No\n    listen_port = 4242\n");
        let ConfigPlan { text, edits, notices } = prepare_config(&off, &NodeArgs::default()).unwrap();
        let config = parsed(&text);
        assert_eq!(config.subsections("interfaces").len(), 2);
        assert!(!interface(&config, "Off").interface_enabled());
        // Transport is the only edit; the server left off is a notice.
        assert_eq!(edits.len(), 1, "{edits:?}");
        assert!(notices.iter().any(|c| c.contains("left off")), "{notices:?}");
        // Said again on a run that changes nothing.
        let again = prepare_config(&text, &NodeArgs::default()).unwrap();
        assert!(again.edits.is_empty() && again.notices.iter().any(|c| c.contains("left off")), "{again:?}");
    }

    #[test]
    fn a_backbone_listener_counts_as_the_entry_point() {
        let backbone =
            format!("{BASE}  [[Backbone]]\n    type = BackboneInterface\n    enabled = Yes\n    listen_on = 0.0.0.0\n    port = 4242\n");
        let text = prepare_config(&backbone, &NodeArgs::default()).unwrap().text;
        assert_eq!(parsed(&text).subsections("interfaces").len(), 2);
        assert_eq!(listening(&parsed(&text)), vec!["Backbone on 0.0.0.0:4242".to_string()]);
    }

    #[test]
    fn no_entry_point_only_turns_transport_on() {
        let args = NodeArgs { no_entry_point: true, ..NodeArgs::default() };
        let plan = prepare_config(BASE, &args).unwrap();
        assert_eq!(plan.edits.len(), 1);
        assert_eq!(parsed(&plan.text).subsections("interfaces").len(), 1);
        let on = BASE.replace("enable_transport = False", "enable_transport = Yes");
        assert_eq!(prepare_config(&on, &args).unwrap(), ConfigPlan { text: on.clone(), ..ConfigPlan::default() });
    }

    #[test]
    fn bad_input_is_refused() {
        for bad in ["two words", "a#b", "a\"b", "a'b", "a=b", "[a", "a]", "[]", "[x]y", "", "  ", "a\nb", "ünï"] {
            let args = NodeArgs { reachable_on: Some(bad.into()), ..NodeArgs::default() };
            assert!(prepare_config(BASE, &args).unwrap_err().contains("takes a host name or an IP address"), "{bad:?}");
        }
        for good in ["rns.example.org", "my-node_1.example", "192.0.2.7", "2001:db8::1", "[2001:db8::1]"] {
            let args = NodeArgs { reachable_on: Some(good.into()), ..NodeArgs::default() };
            let text = prepare_config(BASE, &args).unwrap().text;
            assert_eq!(interface(&parsed(&text), "Public entry point").get("reachable_on"), Some(good), "{good}");
        }
        assert!(prepare_config("[reticulum\n", &NodeArgs::default()).is_err());
        let mut settings = Settings::default();
        let long = NodeArgs { name: Some("x".repeat(500)), ..NodeArgs::default() };
        assert!(prepare_settings(&mut settings, &long).is_err());
    }

    #[test]
    fn settings_turn_the_propagation_node_on_and_name_it_once() {
        let mut settings = Settings::default();
        let args = NodeArgs { name: Some("Hilltop".into()), ..NodeArgs::default() };
        assert_eq!(prepare_settings(&mut settings, &args).unwrap().len(), 2);
        assert!(settings.pn_enabled);
        assert_eq!(settings.pn_name.as_deref(), Some("Hilltop"));
        assert!(prepare_settings(&mut settings, &args).unwrap().is_empty());
        // No flags keep the name; --no-propagation turns nothing off.
        assert!(prepare_settings(&mut settings, &NodeArgs::default()).unwrap().is_empty());
        assert_eq!(settings.pn_name.as_deref(), Some("Hilltop"));
        let mut off = Settings::default();
        assert!(prepare_settings(&mut off, &NodeArgs { no_propagation: true, ..NodeArgs::default() }).unwrap().is_empty());
        assert!(!off.pn_enabled);
        settings.pn_enabled = true;
        assert!(prepare_settings(&mut settings, &NodeArgs { no_propagation: true, ..NodeArgs::default() }).unwrap().is_empty());
        assert!(settings.pn_enabled);
    }

    #[test]
    fn preparing_saves_the_config_and_settings_and_a_second_run_changes_nothing() {
        let dir = test_dir("prepare");
        let paths = Paths::new(Some(dir.join("data"))).unwrap();
        let mut settings = Settings::load(&paths.settings).unwrap();
        settings.rns_config = own_rns_config(&paths).unwrap();
        assert_eq!(settings.rns_config.as_deref().map(PathBuf::from), Some(dir.join("data/reticulum")));
        let prepared = prepare(&mut settings, &paths, &NodeArgs::default()).unwrap();
        assert_eq!(prepared.changes.len(), 4, "{prepared:?}");
        assert!(prepared.changes.iter().all(|c| c.starts_with(&dir.display().to_string())), "{prepared:?}");
        assert!(prepared.notices.is_empty(), "{prepared:?}");
        assert!(settings.pn_enabled);
        let saved = Settings::load(&paths.settings).unwrap();
        assert!(saved.pn_enabled);
        // The node's own config is kept, for the UIs on this data directory.
        assert_eq!(saved.rns_config, settings.rns_config);
        let text = std::fs::read_to_string(dir.join("data/reticulum/config")).unwrap();
        assert_eq!(interface(&parsed(&text), "Public entry point").get("mode"), Some("gateway"));
        assert!(!reticulum::history::History::new(paths.reticulum_history.clone()).list(&dir.join("data/reticulum/config")).is_empty());
        // A second run changes nothing, and says what it uses, not as a change.
        let again = prepare(&mut settings, &paths, &NodeArgs::default()).unwrap();
        assert!(again.changes.is_empty(), "{again:?}");
        assert_eq!(again.notices, vec!["Entry point: Public entry point, on port 4242".to_string()]);
        // A config named on the command line, then in settings.json, is the one used.
        assert_eq!(rns_config(&paths, Some("/srv/rns".into())).unwrap().as_deref(), Some("/srv/rns"));
        assert_eq!(own_rns_config(&paths).unwrap(), None);
        assert_eq!(rns_config(&paths, None).unwrap(), None, "the one in settings.json is used as for any run");
        // One given on the command line is used for the run, not kept.
        let mut given = Settings { rns_config: Some(dir.join("given").display().to_string()), ..saved.clone() };
        let mut unnamed = saved;
        unnamed.rns_config = None;
        unnamed.save(&paths.settings).unwrap();
        prepare(&mut given, &paths, &NodeArgs::default()).unwrap();
        assert!(dir.join("given/config").is_file());
        assert_eq!(Settings::load(&paths.settings).unwrap().rns_config, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_config_with_a_pipe_interface_is_set_up_and_its_pipe_left_as_it_was() {
        let dir = test_dir("pipe");
        let paths = Paths::new(Some(dir.join("data"))).unwrap();
        std::fs::create_dir_all(dir.join("rns")).unwrap();
        let pipe = format!("{BASE}  [[Pipe]]\n    type = PipeInterface\n    enabled = No\n    command = cat\n");
        std::fs::write(dir.join("rns/config"), &pipe).unwrap();
        let mut settings = Settings { rns_config: Some(dir.join("rns").display().to_string()), ..Settings::default() };
        prepare(&mut settings, &paths, &NodeArgs::default()).unwrap();
        let config = parsed(&std::fs::read_to_string(dir.join("rns/config")).unwrap());
        assert!(!interface(&config, "Pipe").interface_enabled());
        assert!(config.subsection("interfaces", "Public entry point").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_status_line_says_the_addresses_and_where_it_listens() {
        let dir = test_dir("status");
        let mut app = crate::app::test_app(&dir, Settings { pn_enabled: true, ..Settings::default() }, Store::default());
        let identity = Identity::new();
        let line = status_line(&app, &identity, &["Public entry point on 0.0.0.0:4242".into()]);
        assert!(line.contains(&hex::encode(crate::app::identity::lxmf_address(&identity))), "{line}");
        assert!(line.contains(&format!("{} (starting)", hex::encode(app.pn.hash))), "{line}");
        assert!(line.contains("0.0.0.0:4242") && line.contains("not up yet"), "{line}");
        app.pn.status = NodeStatus::Failed("no disk".into());
        assert!(status_line(&app, &identity, &[]).contains("failed: no disk"));
        app.pn.status = NodeStatus::Running;
        assert!(status_line(&app, &identity, &[]).contains("(running)"));
        app.pn.stats = Some(crate::lxmf::pn::PnStats { messages: 3, peers: 1, peered: 2, ..Default::default() });
        assert!(status_line(&app, &identity, &[]).contains("3 messages held, 1 peers connected, peered with 2"));
        app.pn.status = NodeStatus::Off;
        assert!(status_line(&app, &identity, &[]).contains("propagation node off; listening nowhere"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn services_served_to_anyone_are_named_with_how_to_turn_them_off() {
        assert!(exposed_services(&Settings::default()).is_empty());
        let scripts = Settings { node_executable_pages: true, ..Settings::default() };
        assert!(exposed_services(&scripts).is_empty(), "scripts only run on a node that's on");
        let all = Settings {
            node_enabled: true,
            node_executable_pages: true,
            hub_enabled: true,
            git_host_enabled: true,
            rncp_listen: true,
            ..Settings::default()
        };
        let lines = exposed_services(&all);
        for key in ["node_enabled", "node_executable_pages", "hub_enabled", "git_host_enabled", "rncp_listen"] {
            assert!(lines.iter().any(|l| l.contains(&format!("set {key} to false in settings.json"))), "{key}: {lines:?}");
        }
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn the_first_run_says_what_a_node_makes_public() {
        let identity = Identity::new();
        let settings = Settings { display_name: "Zev".into(), ..Settings::default() };
        let note = public_note(&settings, &identity);
        assert!(note.contains(&hex::encode(crate::app::identity::lxmf_address(&identity))) && note.contains("\"Zev\""), "{note}");
        assert!(note.contains("unless --name"), "{note}");
        let named = Settings { pn_name: Some("Hilltop".into()), ..settings };
        assert!(public_note(&named, &identity).contains("announces itself as \"Hilltop\""));
    }

    #[test]
    fn listening_names_the_servers_that_are_on() {
        let text = format!(
            "{BASE}  [[A]]\n    type = TCPServerInterface\n    enabled = Yes\n    listen_ip = 127.0.0.1\n    listen_port = 1\n  [[B]]\n    type = TCPServerInterface\n    enabled = No\n    listen_port = 2\n"
        );
        assert_eq!(listening(&parsed(&text)), vec!["A on 127.0.0.1:1".to_string()]);
    }

    const STATS_SECRET: &str = "node-test-stats-secret";

    /// The body of `/stats.json` on `port`, if it answers 200, asked with `token` if given.
    fn get_stats(port: u16, token: Option<&str>) -> Option<String> {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
        let header = token.map(|t| format!("X-Stats-Token: {t}\r\n")).unwrap_or_default();
        stream.write_all(format!("GET /stats.json HTTP/1.1\r\nHost: x\r\n{header}Connection: close\r\n\r\n").as_bytes()).ok()?;
        let mut text = String::new();
        stream.read_to_string(&mut text).ok()?;
        let (head, body) = text.split_once("\r\n\r\n")?;
        head.starts_with("HTTP/1.1 200").then(|| body.to_string())
    }

    /// The node runs, as `rettui node` runs it, until stopped from within:
    /// its entry point takes connections, its propagation node starts, and
    /// it stops cleanly, saving what changed on the way out.
    /// A stats address that can't be used stops the node before it starts.
    #[test]
    fn a_bad_stats_address_stops_the_node() {
        let dir = test_dir("bad-stats");
        let paths = Paths::new(Some(dir.join("data"))).unwrap();
        let identity = crate::config::load_identity(&paths.identity).unwrap();
        let result = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(serve(
            Settings::default(),
            paths,
            identity,
            std::future::pending::<()>(),
            Duration::from_secs(60),
            Listeners { stats: Some(crate::stats::Listen { address: "not an address", token: None, logs: false }), manage: None },
            |_| {},
        ));
        assert!(format!("{:#}", result.unwrap_err()).contains("--stats-listen expects"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A token that can't be used stops the node before it changes anything.
    #[test]
    fn a_bad_stats_token_stops_the_node_before_it_starts() {
        let dir = test_dir("bad-token");
        let paths = Paths::new(Some(dir.join("data"))).unwrap();
        let identity = crate::config::load_identity(&paths.identity).unwrap();
        let file = dir.join("token");
        std::fs::write(&file, "too short\n").unwrap();
        let args = NodeArgs { stats_listen: Some("127.0.0.1:0".into()), stats_token_file: Some(file.clone()), ..NodeArgs::default() };
        let run = |args: NodeArgs, paths: Paths, identity: Identity| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move { run(Settings::default(), paths, identity, &args).await })
        };
        let message = format!("{:#}", run(args.clone(), paths.clone(), identity.clone()).unwrap_err());
        assert!(message.contains("--stats-token-file") && message.contains("at least 16") && !message.contains("too short"), "{message}");
        // A file that isn't there is refused too, and nothing was set up on the way.
        let args = NodeArgs { stats_token_file: Some(dir.join("missing")), ..args };
        let message = format!("{:#}", run(args, paths.clone(), identity).unwrap_err());
        assert!(message.contains("could not read the stats token file"), "{message}");
        assert!(!paths.settings.exists(), "the node changed its settings before refusing the token");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A manage secret that can't be used stops the node before it changes anything.
    #[test]
    fn a_bad_manage_secret_stops_the_node_before_it_starts() {
        let dir = test_dir("bad-manage");
        let paths = Paths::new(Some(dir.join("data"))).unwrap();
        let identity = crate::config::load_identity(&paths.identity).unwrap();
        let file = dir.join("secret");
        let write = |text: &str| crate::config::write_private(&file, text.as_bytes()).unwrap();
        write("too short to be a secret\n");
        let args = NodeArgs { manage_listen: Some("127.0.0.1:0".into()), manage_secret_file: Some(file.clone()), ..NodeArgs::default() };
        let run = |args: NodeArgs| {
            let (paths, identity) = (paths.clone(), identity.clone());
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move { run(Settings::default(), paths, identity, &args).await })
        };
        let message = format!("{:#}", run(args.clone()).unwrap_err());
        assert!(message.contains("--manage-secret-file") && message.contains("at least 32") && !message.contains("too short"), "{message}");
        // A public address is refused too, before anything is set up.
        write("a-manage-secret-long-enough-0123456789");
        let public = NodeArgs { manage_listen: Some("8.8.8.8:8750".into()), ..args.clone() };
        assert!(format!("{:#}", run(public).unwrap_err()).contains("refuses a public address"));
        // The stats token can't be the manage secret too; the error doesn't repeat either.
        let token = dir.join("token");
        std::fs::write(&token, "a-manage-secret-long-enough-0123456789\n").unwrap();
        let shared = NodeArgs { stats_listen: Some("127.0.0.1:0".into()), stats_token_file: Some(token.clone()), ..args.clone() };
        let message = format!("{:#}", run(shared).unwrap_err());
        assert!(message.contains("same text as the stats token") && !message.contains("long-enough"), "{message}");
        // A secret its group or others can read is refused, saying how to fix it.
        #[cfg(unix)]
        for mode in [0o640, 0o604, 0o644] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            let message = format!("{:#}", run(args.clone()).unwrap_err());
            assert!(message.contains("can be read or changed by other users") && message.contains("chmod 600"), "{mode:o}: {message}");
            assert!(!message.contains("long-enough"), "{message}");
        }
        assert!(!paths.settings.exists(), "the node changed its settings before refusing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only the same text in both is refused; either alone, or two different ones, are fine.
    #[test]
    fn the_manage_secret_and_the_stats_token_must_differ() {
        let stats = |token: &str| crate::stats::Listen {
            address: "127.0.0.1:0",
            token: Some(crate::stats::Token::parse(token).unwrap()),
            logs: false,
        };
        let manage = |secret: &str| crate::remote_manage::Listen {
            address: "127.0.0.1:0".into(),
            secret: crate::remote_manage::Secret::parse(secret).unwrap(),
            allow: vec![],
        };
        let same = check_secrets_differ(Some(&stats(MANAGE_SECRET)), Some(&manage(MANAGE_SECRET))).unwrap_err().to_string();
        assert!(same.contains("same text as the stats token") && !same.contains(MANAGE_SECRET), "{same}");
        assert!(check_secrets_differ(Some(&stats("another-stats-token-0123456789abcdef")), Some(&manage(MANAGE_SECRET))).is_ok());
        // A token that is the start of the secret is another text.
        assert!(check_secrets_differ(Some(&stats(&MANAGE_SECRET[..20])), Some(&manage(MANAGE_SECRET))).is_ok());
        assert!(check_secrets_differ(Some(&stats(MANAGE_SECRET)), None).is_ok());
        assert!(check_secrets_differ(None, Some(&manage(MANAGE_SECRET))).is_ok());
        let untokened = crate::stats::Listen { address: "127.0.0.1:0", token: None, logs: false };
        assert!(check_secrets_differ(Some(&untokened), Some(&manage(MANAGE_SECRET))).is_ok());
    }

    const MANAGE_SECRET: &str = "node-test-manage-secret-0123456789abcdef";

    /// A signed request to the node's management listener on `port`: the
    /// status, and the body if the answer is signed for it.
    fn manage(port: u16, method: &str, target: &str, body: &str) -> Option<(u16, serde_json::Value)> {
        use crate::remote_manage::wire;
        use std::io::{Read, Write};
        let secret = crate::remote_manage::Secret::parse(MANAGE_SECRET).unwrap();
        let nonce = hex::encode(rand::random::<[u8; 16]>());
        let ts = crate::remote_manage::unix_now().to_string();
        let signature = wire::sign(&secret, &wire::canonical(&ts, &nonce, method, target, "alice", body.as_bytes()));
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
        let request = format!(
            "{method} {target} HTTP/1.1\r\nHost: x\r\nX-Rettui-Timestamp: {ts}\r\nX-Rettui-Nonce: {nonce}\r\nX-Rettui-Actor: alice\r\n\
             X-Rettui-Signature: v1={signature}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).ok()?;
        // The node closes the connection after each answer.
        let mut text = String::new();
        stream.read_to_string(&mut text).ok()?;
        let (head, body) = text.split_once("\r\n\r\n")?;
        let status: u16 = head.split(' ').nth(1)?.parse().ok()?;
        let signed = head.lines().find_map(|line| line.strip_prefix("x-rettui-signature: "))?;
        if signed != wire::response_signature(&secret, &nonce, status, body.as_bytes()) {
            return None;
        }
        Some((status, serde_json::from_str(body).ok()?))
    }

    /// Remote management through the node as `rettui node` runs it: the
    /// state, a settings change that the node applies and saves, and the
    /// audit log, each answer signed.
    #[test]
    fn the_node_is_managed_with_signed_requests() {
        let dir = test_dir("manage");
        for attempt in 0..3 {
            let root = dir.join(attempt.to_string());
            std::fs::create_dir_all(root.join("rns")).unwrap();
            std::fs::write(root.join("rns/config"), "[reticulum]\n  share_instance = No\n\n[interfaces]\n").unwrap();
            let paths = Paths::new(Some(root.join("data"))).unwrap();
            let settings = Settings {
                update_check: false,
                welcomed: true,
                rns_config: Some(root.join("rns").display().to_string()),
                ..Settings::default()
            };
            settings.save(&paths.settings).unwrap();
            let identity = crate::config::load_identity(&paths.identity).unwrap();
            let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            let listen = crate::remote_manage::Listen {
                address: format!("127.0.0.1:{port}"),
                secret: crate::remote_manage::Secret::parse(MANAGE_SECRET).unwrap(),
                allow: vec![crate::remote_manage::Network::parse("127.0.0.0/8").unwrap()],
            };
            let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
            let settings_file = paths.settings.clone();
            let running = std::thread::spawn(move || {
                tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(serve(
                    settings,
                    paths,
                    identity,
                    async move {
                        let _ = stop_rx.await;
                    },
                    Duration::from_secs(60),
                    Listeners { stats: None, manage: Some(listen) },
                    |_| {},
                ))
            });
            let until = std::time::Instant::now() + Duration::from_secs(20);
            let mut state = None;
            while std::time::Instant::now() < until && !running.is_finished() {
                // A request signed in the second the listener started is refused (`before_start`): sign another.
                if let Some(answer) = manage(port, "GET", "/manage/v1/state", "").filter(|(_, json)| json["error"] != "before_start") {
                    state = Some(answer);
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let Some((status, state)) = state else {
                let _ = stop_tx.send(());
                let _ = running.join();
                continue;
            };
            assert_eq!(status, 200, "{state}");
            assert_eq!(state["version"], crate::VERSION);
            let revision = state["settings_revision"].as_str().unwrap();
            let body = serde_json::json!({ "revision": revision, "changes": { "pn_name": "Managed" } }).to_string();
            let (status, changed) = manage(port, "POST", "/manage/v1/settings", &body).expect("the change wasn't answered");
            assert_eq!(status, 200, "{changed}");
            assert_eq!(changed["applied"][0]["to"], "Managed");
            let (status, audit) = manage(port, "GET", "/manage/v1/audit?limit=5", "").expect("the audit log wasn't answered");
            assert_eq!(status, 200);
            assert_eq!(audit["entries"][0]["id"], changed["id"]);
            stop_tx.send(()).unwrap();
            running.join().unwrap().unwrap();
            assert_eq!(Settings::load(&settings_file).unwrap().pn_name.as_deref(), Some("Managed"));
            let saved = std::fs::read_to_string(root.join("data/manage/audit.jsonl")).unwrap();
            assert!(saved.contains("\"actor\":\"alice\"") && !saved.contains(MANAGE_SECRET), "{saved}");
            // The time of the last request is kept for the next run.
            let mark: u64 = std::fs::read_to_string(root.join("data/manage/last-request")).unwrap().trim().parse().unwrap();
            assert!(mark.abs_diff(crate::remote_manage::unix_now()) < 60, "{mark}");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        panic!("the node didn't start on any of three ports");
    }

    #[test]
    fn the_node_listens_hosts_and_stops_when_asked() {
        let dir = test_dir("serve");
        // A port found free can be taken by another program before the node
        // binds it: then try again, with another.
        for attempt in 0..3 {
            let root = dir.join(attempt.to_string());
            std::fs::create_dir_all(root.join("rns")).unwrap();
            std::fs::write(root.join("rns/config"), "[reticulum]\n  share_instance = No\n\n[interfaces]\n").unwrap();
            let paths = Paths::new(Some(root.join("data"))).unwrap();
            Settings { update_check: false, welcomed: true, ..Settings::default() }.save(&paths.settings).unwrap();
            let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            let args = NodeArgs { entry_point: Some(port), listen_ip: Some("127.0.0.1".parse().unwrap()), ..NodeArgs::default() };
            let mut settings = Settings::load(&paths.settings).unwrap();
            settings.rns_config = Some(root.join("rns").display().to_string());
            let identity = crate::config::load_identity(&paths.identity).unwrap();
            prepare(&mut settings, &paths, &args).unwrap();
            let stats_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            let stats_address = format!("127.0.0.1:{stats_port}");
            let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
            let stop = async move {
                let _ = stop_rx.await;
            };
            // Changed only once it's stopping: saved by the save on the way out.
            let stopping = |app: &mut App| {
                app.store.next_local_id = 424_242;
                app.store_dirty = true;
            };
            let (propagation, store) = (paths.propagation.clone(), paths.store.clone());
            let running = std::thread::spawn(move || {
                let stats_address = stats_address;
                tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(serve(
                    settings,
                    paths,
                    identity,
                    stop,
                    Duration::from_millis(200),
                    Listeners {
                        stats: Some(crate::stats::Listen {
                            address: stats_address.as_str(),
                            token: Some(crate::stats::Token::parse(STATS_SECRET).unwrap()),
                            logs: true,
                        }),
                        manage: None,
                    },
                    stopping,
                ))
            });
            let until = std::time::Instant::now() + Duration::from_secs(20);
            let started = loop {
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() && propagation.exists() {
                    break true;
                }
                assert!(!running.is_finished(), "the node stopped by itself");
                if std::time::Instant::now() > until {
                    break false;
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            // The stats page, from the node's own app: the snapshot is made
            // about once a second, so the first answer may be "starting".
            let mut stats = None;
            while started && std::time::Instant::now() < until {
                // The interfaces are known once the network says so.
                if let Some(text) = get_stats(stats_port, None) {
                    let json = serde_json::from_str::<serde_json::Value>(&text).ok();
                    let total = json.as_ref().and_then(|j| j["interfaces"]["total"].as_u64());
                    // The disk is measured in the background and the node's numbers come a little after it starts.
                    let complete = json.as_ref().is_some_and(|j| j.get("disk").is_some() && j.get("propagation").is_some());
                    stats = Some(text);
                    if total.is_some_and(|n| n >= 1) && complete {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            // With the token, the same page also lists the peers heard; without it, not.
            let listed = started.then(|| get_stats(stats_port, Some(STATS_SECRET)).expect("the stats page didn't answer the token"));
            let wrong = started.then(|| get_stats(stats_port, Some("not-the-secret-at-all")).expect("the stats page didn't answer"));
            stop_tx.send(()).unwrap();
            running.join().unwrap().unwrap();
            if let (Some(listed), Some(wrong)) = (listed, wrong) {
                let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
                assert_eq!(listed["authorised"], true, "{listed}");
                assert!(listed["peers"].is_array(), "{listed}");
                assert!(listed["propagation_peers"].is_array(), "{listed}");
                let wrong: serde_json::Value = serde_json::from_str(&wrong).unwrap();
                assert!(wrong.get("peers").is_none() && wrong.get("authorised").is_none(), "{wrong}");
            }
            if started {
                let json: serde_json::Value = serde_json::from_str(&stats.expect("the stats page didn't answer")).unwrap();
                assert_eq!(json["version"], 1);
                assert!(json["interfaces"]["total"].as_u64().unwrap() >= 1, "{json}");
                assert!(json["interfaces"].get("list").is_none(), "{json}");
                assert!(json["peers_heard"]["total"].is_u64() && json["announces_last_minute"].is_u64(), "{json}");
                assert_eq!(json["propagation"]["enabled"], true, "{json}");
                assert_eq!(json["propagation"]["stored_messages"], 0, "{json}");
                assert!(json["propagation"]["storage_limit_bytes"].as_u64().unwrap() > 0, "{json}");
                assert!(json["disk"]["data_bytes"].as_u64().unwrap() > 0, "{json}");
                assert!(json.get("propagation_peers").is_none(), "no list without the token: {json}");
            }
            if !started {
                continue;
            }
            assert!(Settings::load(&root.join("data/settings.json")).unwrap().pn_enabled);
            assert_eq!(Store::load(&store).unwrap().next_local_id, 424_242, "the store wasn't saved on the way out");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        panic!("the node didn't start on any of three ports");
    }
}
