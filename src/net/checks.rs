//! The entry points in the Reticulum config, for the Tools tab to test the
//! reach of (see [`crate::app::reach`]).
//!
//! The hosts come from the config, never from the caller, so the web UI
//! can't be made to connect to an address of its own choosing.

use rns_runtime::config::Config;

/// An interface in the config that connects to a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryPoint {
    /// The interface's name in the config.
    pub name: String,
    /// Where to connect: the host it connects to, or for one that listens,
    /// this computer's address for its port.
    pub host: String,
    pub port: u16,
    /// It waits for others to connect (a server interface) instead of
    /// connecting out. `host` is then where it's tried from here.
    pub listens: bool,
    pub enabled: bool,
    /// It has a network name or passphrase, so its far end must have the
    /// same (IFAC). The passphrase itself is never kept here.
    pub ifac: bool,
}

/// The interfaces in the config text that connect to a host and port, or
/// listen on a port.
pub fn entry_points(text: &str) -> Vec<EntryPoint> {
    let Ok(config) = Config::parse(text) else { return Vec::new() };
    config
        .subsections("interfaces")
        .into_iter()
        .filter_map(|(name, section)| {
            let (host, port, listens) = match (section.get("target_host"), section.get_uint("target_port")) {
                (Some(host), Some(port)) => (host.trim().to_string(), u16::try_from(port).ok()?, false),
                _ => {
                    let port = u16::try_from(section.get_uint("listen_port")?).ok()?;
                    (local_address(section.get("listen_ip").unwrap_or_default()), port, true)
                }
            };
            let has = |keys: &[&str]| keys.iter().any(|key| section.get(key).is_some_and(|v| !v.trim().is_empty()));
            Some(EntryPoint {
                name: name.to_string(),
                host,
                port,
                listens,
                enabled: section.interface_enabled(),
                ifac: has(&["networkname", "network_name", "passphrase", "pass_phrase"]),
            })
        })
        .collect()
}

/// Where on this computer a server interface listening on `ip` can be
/// reached: all addresses (or none given) means this computer's own.
fn local_address(ip: &str) -> String {
    match ip.trim().trim_matches(['[', ']']) {
        "" | "0.0.0.0" => "127.0.0.1".into(),
        "::" => "::1".into(),
        ip => ip.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_points_come_from_the_config_without_their_passphrases() {
        let text = "[interfaces]\n  [[Testnet]]\n    type = TCPClientInterface\n    enabled = yes\n    target_host = example.org\n    target_port = 4242\n    passphrase = secret\n  [[Off]]\n    type = TCPClientInterface\n    enabled = no\n    target_host = 192.0.2.1\n    target_port = 1\n  [[Rettui Public]]\n    type = TCPServerInterface\n    enabled = Yes\n    listen_ip = 0.0.0.0\n    listen_port = 4242\n  [[Auto]]\n    type = AutoInterface\n";
        let found = entry_points(text);
        assert_eq!(found.len(), 3);
        // One that listens is tried from this computer, on its own address.
        assert_eq!(
            found[2],
            EntryPoint { name: "Rettui Public".into(), host: "127.0.0.1".into(), port: 4242, listens: true, enabled: true, ifac: false }
        );
        assert_eq!(local_address("::"), "::1");
        assert_eq!(local_address("192.0.2.7"), "192.0.2.7");
        assert_eq!(
            found[0],
            EntryPoint { name: "Testnet".into(), host: "example.org".into(), port: 4242, listens: false, enabled: true, ifac: true }
        );
        assert!(!found[1].enabled && !found[1].ifac);
        assert!(entry_points("not a config [[").is_empty());
    }
}
