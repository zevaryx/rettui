//! `rettui node`: a public node with no TUI and no web UI, as `rnsd` (a
//! transport node with a TCP server others connect to, in gateway mode)
//! and `lxmd` (a propagation node) are together.
//!
//! It sets the node up first, adding only what's missing to the Reticulum
//! config and the settings (never removing or changing what's there, beyond
//! turning on what a node is), so a later run with no flags is the same node.
//! Then it runs as the web UI does, without the HTTP server, until SIGINT
//! or SIGTERM, and saves everything on the way out. Nothing it listens on
//! can change what it runs: it has no login and reads no input.

use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Result, anyhow};
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

/// What `rettui node` was asked to set up. Every flag is kept in the
/// Reticulum config or the settings, so the next run needs none of them.
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
    #[arg(long, value_name = "HOST")]
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
}

/// The Reticulum config directory the node uses when neither
/// `--rns-config` nor `rns_config` in settings.json names one: its own, in
/// the data directory, so making this a node changes no other program's
/// config (as Docker's entrypoint does with /data/reticulum).
pub fn own_rns_config(paths: &Paths) -> Result<Option<String>> {
    let saved = Settings::load(&paths.settings)?;
    Ok(saved.rns_config.is_none().then(|| paths.root.join("reticulum").to_string_lossy().into_owned()))
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

/// Make the Reticulum config `text` a node's, as `args` asks: Transport node
/// on, and a TCP server interface in gateway mode (unless there is one, or
/// `--no-entry-point`). Only what's missing is added. The new text, and
/// what changed, to print.
pub fn prepare_config(text: &str, args: &NodeArgs) -> Result<(String, Vec<String>), String> {
    let config = Config::parse(text).map_err(|e| format!("The Reticulum config can't be read: {e}"))?;
    let mut doc = Doc::new(text);
    let mut changes = Vec::new();
    if config.section("reticulum").and_then(|r| r.get_bool("enable_transport")) != Some(true) {
        doc.set(&["reticulum"], "enable_transport", &[], Some("Yes"));
        changes.push("Transport node turned on (enable_transport = Yes)".to_string());
    }
    if args.no_entry_point {
        return Ok((doc.text(), changes));
    }
    let host = args.reachable_on.as_deref().map(str::trim).filter(|h| !h.is_empty());
    if host.is_some_and(|h| h.chars().any(|c| c.is_whitespace() || c.is_control())) {
        return Err("The host name can't have spaces".into());
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
            changes.push(format!("{name} listens on that port but is off (enabled = No): left off, so others can't connect yet"));
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
                changes.push(format!(
                    "{name} listens on {} already: --listen-ip left it as it is",
                    section.get("listen_ip").unwrap_or_default()
                ));
            }
            if let Some(on) = on {
                changes.push(format!("Entry point: {name}, on port {on}"));
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
    Ok((doc.text(), changes))
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

/// Set up the node's Reticulum config (the one `settings` names) and its
/// settings, saving what changed. What changed, to print.
fn prepare(settings: &mut Settings, paths: &Paths, args: &NodeArgs) -> Result<Vec<String>> {
    let path = reticulum::config_path(settings.rns_config.as_deref());
    // A new config starts from rettui's default, as for the other modes.
    reticulum::seed_default(settings.rns_config.as_deref());
    let (before, existed) = reticulum::load(&path).map_err(|e| anyhow!(e))?;
    let (after, mut changes) = prepare_config(&before, args).map_err(|e| anyhow!("{e} ({})", path.display()))?;
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
        changes = changes.into_iter().map(|c| format!("{}: {c}", path.display())).collect();
    } else {
        changes.clear();
    }
    // The settings as saved, so values only in effect for this run (such as
    // --rns-config) aren't written to the file.
    let mut saved = Settings::load(&paths.settings)?;
    let said = prepare_settings(&mut saved, args).map_err(|e| anyhow!(e))?;
    if !said.is_empty() {
        saved.save(&paths.settings)?;
        prepare_settings(settings, args).map_err(|e| anyhow!(e))?;
        changes.extend(said.into_iter().map(|c| format!("{}: {c}", paths.settings.display())));
    }
    Ok(changes)
}

/// `rettui node`: set it up, then run it until SIGINT or SIGTERM.
pub async fn run(mut settings: Settings, paths: Paths, identity: Identity, args: &NodeArgs) -> Result<()> {
    // Listened for from now, so a stop while it starts is a clean one.
    let stop = crate::web::shutdown_signal();
    for change in prepare(&mut settings, &paths, args)? {
        println!("{change}");
    }
    serve(settings, paths, identity, stop, STATUS_EVERY).await
}

/// The node, until `stop`: what the web UI runs, without its HTTP server.
/// The status line is printed at the start, then every `every`.
pub(crate) async fn serve(
    settings: Settings,
    paths: Paths,
    identity: Identity,
    stop: impl std::future::Future<Output = ()>,
    every: Duration,
) -> Result<()> {
    let rns_path = reticulum::config_path(settings.rns_config.as_deref());
    let listening =
        std::fs::read_to_string(&rns_path).ok().and_then(|text| Config::parse(&text).ok()).map(|c| listening(&c)).unwrap_or_default();
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
    tokio::pin!(stop);
    let mut decoded = app.take_decoded();
    loop {
        tokio::select! {
            Some(event) = net_rx.recv() => {
                app.on_net(event);
                net::drain_burst(&mut net_rx, |event| app.on_net(event));
            }
            Some(image) = decoded.recv() => app.on_decoded(image),
            _ = tick.tick() => app.on_tick(),
            _ = save.tick() => app.save_if_dirty(),
            () = app.save_timer() => app.save_when_due(),
            _ = status.tick() => println!("{}", status_line(&app, &identity, &listening)),
            () = &mut stop => break,
        }
        print_log(&mut app);
        // Nobody is there to be shown these.
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
    println!("rettui node stopping: saving, and leaving the network");
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
            let ip = section.get("listen_ip").or_else(|| section.get("device")).unwrap_or("0.0.0.0");
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
        let (text, changes) = prepare_config(BASE, &NodeArgs::default()).unwrap();
        assert_eq!(changes.len(), 2, "{changes:?}");
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
        let (again, changes) = prepare_config(&text, &NodeArgs::default()).unwrap();
        assert_eq!(again, text);
        assert_eq!(changes, vec!["Entry point: Public entry point, on port 4242".to_string()]);
    }

    #[test]
    fn flags_set_the_port_address_and_host_and_a_later_run_without_them_keeps_them() {
        let args = NodeArgs {
            entry_point: Some(4965),
            listen_ip: Some("192.0.2.7".parse().unwrap()),
            reachable_on: Some(" rns.example.org ".into()),
            ..NodeArgs::default()
        };
        let (text, _) = prepare_config(BASE, &args).unwrap();
        let config = parsed(&text);
        let server = interface(&config, "Public entry point");
        assert_eq!(server.get_uint("listen_port"), Some(4965));
        assert_eq!(server.get("listen_ip"), Some("192.0.2.7"));
        assert_eq!(server.get("reachable_on"), Some("rns.example.org"));
        assert_eq!(server.get_bool("discoverable"), Some(true));
        // No flags: the server there is the entry point, not a new one on 4242.
        let (again, _) = prepare_config(&text, &NodeArgs::default()).unwrap();
        assert_eq!(again, text);
        // The same port again changes nothing either.
        assert_eq!(prepare_config(&text, &args).unwrap().0, text);
    }

    #[test]
    fn what_the_operator_set_on_a_server_stays_and_only_whats_missing_is_added() {
        let own = format!(
            "{BASE}  [[My server]]\n    type = TCPServerInterface\n    interface_enabled = Yes\n    interface_mode = boundary\n    listen_ip = 10.0.0.1\n    port = 5000\n    reachable_on = mine.example\n"
        );
        let args =
            NodeArgs { listen_ip: Some("0.0.0.0".parse().unwrap()), reachable_on: Some("other.example".into()), ..NodeArgs::default() };
        let (text, changes) = prepare_config(&own, &args).unwrap();
        let config = parsed(&text);
        assert_eq!(config.subsections("interfaces").len(), 2, "no second server");
        let server = interface(&config, "My server");
        assert_eq!(server.get("interface_mode"), Some("boundary"));
        assert!(server.get("mode").is_none());
        assert_eq!(server.get("listen_ip"), Some("10.0.0.1"));
        assert_eq!(server.get("reachable_on"), Some("mine.example"));
        // Only discoverable was missing.
        assert_eq!(server.get_bool("discoverable"), Some(true));
        assert!(changes.iter().any(|c| c.contains("left it as it is")), "{changes:?}");

        // Without a mode, it's given gateway.
        let plain = own.replace("    interface_mode = boundary\n", "");
        let (text, _) = prepare_config(&plain, &NodeArgs::default()).unwrap();
        assert_eq!(interface(&parsed(&text), "My server").get("mode"), Some("gateway"));
    }

    #[test]
    fn another_port_gets_a_server_of_its_own_and_the_first_is_kept() {
        let (first, _) = prepare_config(BASE, &NodeArgs::default()).unwrap();
        let (text, changes) = prepare_config(&first, &NodeArgs { entry_point: Some(4243), ..NodeArgs::default() }).unwrap();
        let config = parsed(&text);
        assert_eq!(interface(&config, "Public entry point").get_uint("listen_port"), Some(4242));
        assert_eq!(interface(&config, "Public entry point 2").get_uint("listen_port"), Some(4243));
        assert!(changes[0].starts_with("Added Public entry point 2"), "{changes:?}");
    }

    #[test]
    fn a_server_turned_off_is_left_off_and_not_doubled() {
        let off = format!("{BASE}  [[Off]]\n    type = TCPServerInterface\n    enabled = No\n    listen_port = 4242\n");
        let (text, changes) = prepare_config(&off, &NodeArgs::default()).unwrap();
        let config = parsed(&text);
        assert_eq!(config.subsections("interfaces").len(), 2);
        assert!(!interface(&config, "Off").interface_enabled());
        assert!(changes.iter().any(|c| c.contains("left off")), "{changes:?}");
    }

    #[test]
    fn a_backbone_listener_counts_as_the_entry_point() {
        let backbone =
            format!("{BASE}  [[Backbone]]\n    type = BackboneInterface\n    enabled = Yes\n    listen_on = 0.0.0.0\n    port = 4242\n");
        let (text, _) = prepare_config(&backbone, &NodeArgs::default()).unwrap();
        assert_eq!(parsed(&text).subsections("interfaces").len(), 2);
    }

    #[test]
    fn no_entry_point_only_turns_transport_on() {
        let args = NodeArgs { no_entry_point: true, ..NodeArgs::default() };
        let (text, changes) = prepare_config(BASE, &args).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(parsed(&text).subsections("interfaces").len(), 1);
        let on = BASE.replace("enable_transport = False", "enable_transport = Yes");
        assert_eq!(prepare_config(&on, &args).unwrap(), (on.clone(), Vec::new()));
    }

    #[test]
    fn bad_input_is_refused() {
        let spaced = NodeArgs { reachable_on: Some("two words".into()), ..NodeArgs::default() };
        assert!(prepare_config(BASE, &spaced).unwrap_err().contains("spaces"));
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
        let changes = prepare(&mut settings, &paths, &NodeArgs::default()).unwrap();
        assert_eq!(changes.len(), 3, "{changes:?}");
        assert!(settings.pn_enabled);
        let saved = Settings::load(&paths.settings).unwrap();
        assert!(saved.pn_enabled);
        // The config directory used for this run isn't written to settings.json.
        assert!(saved.rns_config.is_none());
        let text = std::fs::read_to_string(dir.join("data/reticulum/config")).unwrap();
        assert_eq!(interface(&parsed(&text), "Public entry point").get("mode"), Some("gateway"));
        assert!(!reticulum::history::History::new(paths.reticulum_history.clone()).list(&dir.join("data/reticulum/config")).is_empty());
        assert!(prepare(&mut settings, &paths, &NodeArgs::default()).unwrap().is_empty());
        // A config named in settings.json is the one used.
        let mut named = saved;
        named.rns_config = Some(dir.join("elsewhere").display().to_string());
        named.save(&paths.settings).unwrap();
        assert_eq!(own_rns_config(&paths).unwrap(), None);
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
    fn listening_names_the_servers_that_are_on() {
        let text = format!(
            "{BASE}  [[A]]\n    type = TCPServerInterface\n    enabled = Yes\n    listen_ip = 127.0.0.1\n    listen_port = 1\n  [[B]]\n    type = TCPServerInterface\n    enabled = No\n    listen_port = 2\n"
        );
        assert_eq!(listening(&parsed(&text)), vec!["A on 127.0.0.1:1".to_string()]);
    }

    /// The node runs, as `rettui node` runs it, until stopped from within:
    /// its entry point takes connections, its propagation node starts, and
    /// it stops cleanly, saving its settings.
    #[test]
    fn the_node_listens_hosts_and_stops_when_asked() {
        let dir = test_dir("serve");
        std::fs::create_dir_all(dir.join("rns")).unwrap();
        std::fs::write(dir.join("rns/config"), "[reticulum]\n  share_instance = No\n\n[interfaces]\n").unwrap();
        let paths = Paths::new(Some(dir.join("data"))).unwrap();
        Settings { update_check: false, welcomed: true, ..Settings::default() }.save(&paths.settings).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let args = NodeArgs { entry_point: Some(port), listen_ip: Some("127.0.0.1".parse().unwrap()), ..NodeArgs::default() };
        let mut settings = Settings::load(&paths.settings).unwrap();
        settings.rns_config = Some(dir.join("rns").display().to_string());
        let identity = crate::config::load_identity(&paths.identity).unwrap();
        prepare(&mut settings, &paths, &args).unwrap();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let stop = async move {
            let _ = stop_rx.await;
        };
        let propagation = paths.propagation.clone();
        let running = std::thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(serve(
                settings,
                paths,
                identity,
                stop,
                Duration::from_millis(200),
            ))
        });
        let until = std::time::Instant::now() + Duration::from_secs(30);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() || !propagation.exists() {
            assert!(std::time::Instant::now() < until && !running.is_finished(), "the node didn't start");
            std::thread::sleep(Duration::from_millis(50));
        }
        stop_tx.send(()).unwrap();
        running.join().unwrap().unwrap();
        assert!(Settings::load(&dir.join("data/settings.json")).unwrap().pn_enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
