//! Every option rsReticulum reads from its config file, grouped the way the
//! editors show them. Keys are the ones Python RNS documents; `aliases` are
//! other spellings rsReticulum also accepts (an existing alias is edited in
//! place rather than adding a second key).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Float,
    Text,
    /// Text that is masked when listed (passphrases, keys).
    Secret,
    /// Comma-separated values.
    List,
    Choice(&'static [&'static str]),
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Bool => "bool",
            Kind::Int => "int",
            Kind::Float => "float",
            Kind::Text => "text",
            Kind::Secret => "secret",
            Kind::List => "list",
            Kind::Choice(_) => "choice",
        }
    }

    pub fn choices(self) -> &'static [&'static str] {
        match self {
            Kind::Choice(choices) => choices,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Opt {
    pub key: &'static str,
    pub aliases: &'static [&'static str],
    pub label: &'static str,
    pub kind: Kind,
    /// What applies when the key is not set ("" when there is none).
    pub default: &'static str,
    pub help: &'static str,
}

#[derive(Debug)]
pub struct Group {
    pub title: &'static str,
    pub opts: &'static [Opt],
}

#[derive(Debug)]
pub struct InterfaceType {
    pub name: &'static str,
    pub label: &'static str,
    pub opts: &'static [Opt],
    /// Keys written when an interface of this type is added.
    pub starter: &'static [(&'static str, &'static str)],
}

const fn opt(key: &'static str, label: &'static str, kind: Kind, default: &'static str, help: &'static str) -> Opt {
    Opt {
        key,
        aliases: &[],
        label,
        kind,
        default,
        help,
    }
}

const fn alias(
    key: &'static str,
    aliases: &'static [&'static str],
    label: &'static str,
    kind: Kind,
    default: &'static str,
    help: &'static str,
) -> Opt {
    Opt {
        key,
        aliases,
        label,
        kind,
        default,
        help,
    }
}

use Kind::*;

const MODES: &[&str] = &["full", "gateway", "access_point", "roaming", "boundary"];
const PARITY: &[&str] = &["none", "even", "odd"];

pub const RETICULUM: &[Group] = &[
    Group {
        title: "General",
        opts: &[
            opt("enable_transport", "Transport node", Bool, "No", "Route traffic and answer path requests for other peers (for always-on, stationary nodes)"),
            opt("share_instance", "Share instance", Bool, "Yes", "Let other programs on this machine use this Reticulum instance"),
            opt("instance_name", "Instance name", Text, "default", "Name of the shared instance (programs sharing it must use the same name)"),
            opt("shared_instance_type", "Shared instance type", Choice(&["unix", "tcp"]), "unix", "How local programs connect to the shared instance"),
            opt("shared_instance_port", "Shared instance port", Int, "37428", "TCP port of the shared instance (tcp type)"),
            opt("instance_control_port", "Control port", Int, "37429", "TCP port for controlling the shared instance (tcp type)"),
            opt("panic_on_interface_error", "Stop on interface error", Bool, "No", "Stop Reticulum if an interface fails, instead of carrying on without it"),
            opt("respond_to_probes", "Answer probes", Bool, "No", "Answer rnprobe requests sent to this instance"),
            opt("use_implicit_proof", "Implicit proofs", Bool, "Yes", "Send short implicit packet proofs instead of explicit ones"),
            opt("link_mtu_discovery", "Link MTU discovery", Bool, "Yes", "Negotiate larger packet sizes on links where the path allows it"),
            opt("static_transport_identity", "Static transport identity", Bool, "No", "Keep the same transport identity across restarts even when not a transport node"),
            opt("local_hops_delta", "Local hops delta", Bool, "No", "Add a random hop offset to announces from this instance"),
            opt("network_identity", "Network identity", Text, "", "Identity file that signs interface discovery and blackhole lists for your network"),
            opt("force_shared_instance_bitrate", "Shared instance bitrate", Int, "", "Report this bitrate (bits/s) for the shared instance interface"),
        ],
    },
    Group {
        title: "Remote management",
        opts: &[
            opt("enable_remote_management", "Remote management", Bool, "No", "Allow the identities below to query and manage this instance"),
            opt("remote_management_allowed", "Allowed identities", List, "", "Identity hashes allowed to manage this instance, comma-separated"),
            opt("rpc_key", "RPC key", Secret, "", "Hex key for local RPC (normally derived automatically)"),
        ],
    },
    Group {
        title: "Interface discovery",
        opts: &[
            opt("discover_interfaces", "Discover interfaces", Bool, "No", "Listen for interfaces announced by other nodes"),
            alias("autoconnect_discovered_interfaces", &["discover_interfaces_autoconnect"], "Auto-connect", Int, "0", "Connect to up to this many discovered interfaces; 0 never connects"),
            alias("required_discovery_value", &["discover_interfaces_required_value"], "Required stamp value", Int, "", "Ignore discovered interfaces with a lower stamp value"),
            opt("interface_discovery_sources", "Trusted sources", List, "", "Only use interfaces announced by these network identities, comma-separated"),
            opt("bootstrap_configs", "Bootstrap configs", List, "", "Extra config files with interfaces used to bootstrap discovery"),
        ],
    },
    Group {
        title: "Blackhole",
        opts: &[
            opt("publish_blackhole", "Publish blackhole list", Bool, "No", "Publish the identities this instance blocks, for others to use"),
            opt("blackhole_sources", "Blackhole sources", List, "", "Network identities whose blackhole lists are applied, comma-separated"),
            opt("blackhole_update_interval", "Update interval (min)", Float, "", "Minutes between blackhole list updates (at least 2)"),
        ],
    },
    Group {
        title: "Announce rate defaults",
        opts: &[
            opt("default_ar_target", "Rate target (s)", Int, "", "Default minimum seconds between announces from one destination; 0 turns it off"),
            opt("default_ar_penalty", "Rate penalty (s)", Int, "", "Default extra seconds a destination waits after breaking the rate"),
            opt("default_ar_grace", "Rate grace", Int, "", "Default number of rate breaks allowed before the penalty applies"),
        ],
    },
    Group {
        title: "Ingress tuning",
        opts: INGRESS_TUNING,
    },
];

pub const LOGGING: &[Group] = &[Group {
    title: "Logging",
    opts: &[
        opt("loglevel", "Log level", Choice(&["0", "1", "2", "3", "4", "5", "6", "7"]), "4", "0 critical, 1 error, 2 warning, 3 notice, 4 info, 5 verbose, 6 debug, 7 extreme"),
        opt("logtimestamps", "Timestamps", Bool, "Yes", "Put timestamps on log lines"),
    ],
}];

const INGRESS_TUNING: &[Opt] = &[
    opt("ic_new_time", "New interface time (s)", Float, "", "How long an interface counts as new after it appears"),
    opt("ic_burst_freq_new", "Burst rate, new (/s)", Float, "", "Announces per second that count as a burst on a new interface"),
    opt("ic_burst_freq", "Burst rate (/s)", Float, "", "Announces per second that count as a burst"),
    opt("ic_pr_burst_freq_new", "Path request burst, new", Float, "", "Path requests per second that count as a burst on a new interface"),
    opt("ic_pr_burst_freq", "Path request burst", Float, "", "Path requests per second that count as a burst"),
    opt("ic_burst_hold", "Burst hold (s)", Float, "", "How long announces are held once a burst is detected"),
    opt("ic_burst_penalty", "Burst penalty (s)", Float, "", "Extra hold time after a burst ends"),
    opt("ic_max_held_announces", "Max held announces", Int, "", "Most announces held back at once"),
    opt("ic_held_release_interval", "Release interval (s)", Float, "", "Seconds between releasing held announces"),
    opt("ec_pr_freq", "Egress path requests (/s)", Float, "", "Most path requests sent per second"),
    opt("egress_control", "Egress control", Bool, "", "Limit outgoing path requests"),
];

/// Options every interface has, before its type's own options.
pub const INTERFACE_BASIC: Group = Group {
    title: "Interface",
    opts: &[
        alias("enabled", &["interface_enabled"], "Enabled", Bool, "Yes", "Start this interface with Reticulum"),
        alias("mode", &["interface_mode"], "Mode", Choice(MODES), "full", "full, gateway (serves path requests), access_point (quiet), roaming (moving), boundary (joins networks)"),
        opt("outgoing", "Outgoing", Bool, "Yes", "Send traffic on this interface (off listens only)"),
        opt("bitrate", "Bitrate (bits/s)", Int, "", "Override the interface's detected or default bitrate"),
    ],
};

/// Options every interface has, after its type's own options.
pub const INTERFACE_ADVANCED: &[Group] = &[
    Group {
        title: "Network isolation (IFAC)",
        opts: &[
            alias("network_name", &["networkname"], "Network name", Text, "", "Only peers with the same network name (and passphrase) can use this interface"),
            alias("passphrase", &["pass_phrase"], "Passphrase", Secret, "", "Passphrase for the isolated network"),
            opt("ifac_size", "IFAC size", Int, "", "Size of the access code on each packet, in bytes (1-64)"),
        ],
    },
    Group {
        title: "Announces",
        opts: &[
            opt("announce_cap", "Announce cap (%)", Float, "2", "Most of the bandwidth announces may use, in percent"),
            opt("announce_rate_target", "Rate target (s)", Int, "", "Minimum seconds between announces from one destination"),
            opt("announce_rate_grace", "Rate grace", Int, "", "Rate breaks allowed before the penalty applies"),
            opt("announce_rate_penalty", "Rate penalty (s)", Int, "", "Extra seconds a destination waits after breaking the rate"),
            opt("recursive_prs", "Recursive path requests", Bool, "No", "Always look further for unknown paths requested here"),
            opt("announces_from_internal", "Pass internal announces", Bool, "Yes", "Rebroadcast announces learned from internal interfaces"),
            opt("bootstrap_only", "Bootstrap only", Bool, "No", "Use this interface only until discovered interfaces connect (rsReticulum drops it as soon as it starts connecting to them)"),
        ],
    },
    Group {
        title: "Discovery",
        opts: &[
            opt("discoverable", "Discoverable", Bool, "No", "Announce this interface so others can find and connect to it"),
            opt("discovery_name", "Discovery name", Text, "", "Name announced for the interface; empty uses its section name"),
            alias("announce_interval", &["discovery_announce_interval"], "Announce interval (min)", Float, "", "Minutes between interface announces"),
            alias("reachable_on", &["discovery_reachable_on"], "Reachable on", Text, "", "Address others should connect to"),
            alias("discovery_stamp_value", &["stamp_value"], "Stamp value", Int, "", "Proof-of-work value of the announce"),
            opt("discovery_encrypt", "Encrypt announces", Bool, "No", "Encrypt interface announces for your network identity"),
            alias("publish_ifac", &["discovery_publish_ifac"], "Publish IFAC", Bool, "No", "Include the network name and passphrase in announces"),
            alias("latitude", &["discovery_latitude"], "Latitude", Float, "", "Location announced with the interface"),
            alias("longitude", &["discovery_longitude"], "Longitude", Float, "", "Location announced with the interface"),
            alias("height", &["discovery_height"], "Height (m)", Float, "", "Height announced with the interface"),
            opt("discovery_frequency", "Frequency (Hz)", Int, "", "Radio frequency announced with the interface"),
            opt("discovery_bandwidth", "Bandwidth (Hz)", Int, "", "Radio bandwidth announced with the interface"),
            opt("discovery_spreading_factor", "Spreading factor", Int, "", "LoRa spreading factor announced with the interface"),
            opt("discovery_coding_rate", "Coding rate", Int, "", "LoRa coding rate announced with the interface"),
            opt("discovery_modulation", "Modulation", Text, "", "Modulation announced with the interface"),
            opt("discovery_channel", "Channel", Int, "", "Channel announced with the interface"),
            opt("ignore_config_warnings", "Ignore warnings", Bool, "No", "Keep the configured mode even when discovery suggests another"),
        ],
    },
    Group {
        title: "Ingress control",
        opts: &[opt("ingress_control", "Ingress control", Bool, "Yes", "Hold back announce bursts from new or busy peers")],
    },
    Group {
        title: "Ingress tuning",
        opts: INGRESS_TUNING,
    },
];

const SERIAL_LINE: [Opt; 5] = [
    opt("port", "Port", Text, "", "Serial device, e.g. /dev/ttyUSB0"),
    alias("speed", &["baud_rate"], "Speed (baud)", Int, "", "Serial speed"),
    alias("databits", &["data_bits"], "Data bits", Int, "8", "Serial data bits"),
    opt("parity", "Parity", Choice(PARITY), "none", "Serial parity"),
    alias("stopbits", &["stop_bits"], "Stop bits", Int, "1", "Serial stop bits"),
];

const KISS: [Opt; 5] = [
    opt("preamble", "Preamble (ms)", Int, "350", "Time to wait after keying the transmitter"),
    opt("txtail", "TX tail (ms)", Int, "20", "Time to keep transmitting after the data"),
    opt("persistence", "Persistence", Int, "64", "CSMA persistence (0-255)"),
    opt("slottime", "Slot time (ms)", Int, "20", "CSMA slot time"),
    opt("flow_control", "Flow control", Bool, "No", "Wait for the modem to accept each frame"),
];

const BEACON: [Opt; 2] = [
    opt("id_callsign", "ID callsign", Text, "", "Callsign sent as an identification beacon (amateur radio)"),
    opt("id_interval", "ID interval (s)", Int, "", "Seconds between identification beacons"),
];

pub const INTERFACE_TYPES: &[InterfaceType] = &[
    InterfaceType {
        name: "AutoInterface",
        label: "Auto (local network)",
        opts: &[
            opt("group_id", "Group ID", Text, "reticulum", "Only peers with the same group ID find each other"),
            opt("discovery_scope", "Discovery scope", Choice(&["link", "admin", "site", "organisation", "global"]), "link", "How far multicast discovery reaches"),
            opt("discovery_port", "Discovery port", Int, "29716", "UDP port for peer discovery"),
            opt("data_port", "Data port", Int, "42671", "UDP port for data"),
            opt("devices", "Devices", List, "", "Only use these network devices, comma-separated"),
            opt("ignored_devices", "Ignored devices", List, "", "Never use these network devices, comma-separated"),
            opt("multicast_address_type", "Multicast address", Choice(&["temporary", "permanent"]), "temporary", "Kind of IPv6 multicast address used"),
        ],
        starter: &[],
    },
    InterfaceType {
        name: "TCPClientInterface",
        label: "TCP client",
        opts: &[
            opt("target_host", "Host", Text, "", "Host name or address to connect to"),
            opt("target_port", "Port", Int, "", "TCP port to connect to"),
            opt("kiss_framing", "KISS framing", Bool, "No", "Use KISS framing (for TNCs reached over TCP)"),
            opt("connect_timeout", "Connect timeout (s)", Int, "", "Seconds to wait when connecting"),
            opt("max_reconnect_tries", "Reconnect tries", Int, "", "Give up after this many reconnects; empty keeps trying"),
            opt("fixed_mtu", "Fixed MTU", Int, "", "Use this MTU instead of negotiating one"),
        ],
        starter: &[("target_host", "127.0.0.1"), ("target_port", "4242")],
    },
    InterfaceType {
        name: "TCPServerInterface",
        label: "TCP server",
        opts: &[
            opt("listen_ip", "Listen address", Text, "0.0.0.0", "Address to listen on"),
            opt("listen_port", "Listen port", Int, "", "TCP port to listen on"),
            opt("device", "Device", Text, "", "Listen on this network device instead of an address"),
            opt("prefer_ipv6", "Prefer IPv6", Bool, "No", "Listen on the device's IPv6 address"),
            opt("kiss_framing", "KISS framing", Bool, "No", "Use KISS framing"),
        ],
        starter: &[("listen_ip", "0.0.0.0"), ("listen_port", "4242")],
    },
    InterfaceType {
        name: "BackboneInterface",
        label: "Backbone (TCP)",
        opts: &[
            opt("listen_on", "Listen address", Text, "", "Address to listen on (server mode)"),
            alias("remote", &["target_host"], "Remote host", Text, "", "Host to connect to (client mode); empty listens instead"),
            alias("port", &["listen_port", "target_port"], "Port", Int, "", "TCP port to listen on or connect to"),
            opt("device", "Device", Text, "", "Listen on this network device"),
            opt("connect_timeout", "Connect timeout (s)", Int, "", "Seconds to wait when connecting"),
            opt("max_reconnect_tries", "Reconnect tries", Int, "", "Give up after this many reconnects; empty keeps trying"),
        ],
        starter: &[("listen_on", "0.0.0.0"), ("port", "4242")],
    },
    InterfaceType {
        name: "UDPInterface",
        label: "UDP",
        opts: &[
            opt("listen_ip", "Listen address", Text, "", "Address to listen on"),
            opt("listen_port", "Listen port", Int, "", "UDP port to listen on"),
            opt("forward_ip", "Forward address", Text, "", "Address to send to (often a broadcast address)"),
            opt("forward_port", "Forward port", Int, "", "UDP port to send to"),
            opt("device", "Device", Text, "", "Use this network device's addresses"),
        ],
        starter: &[
            ("listen_ip", "0.0.0.0"),
            ("listen_port", "4242"),
            ("forward_ip", "255.255.255.255"),
            ("forward_port", "4242"),
        ],
    },
    InterfaceType {
        name: "I2PInterface",
        label: "I2P",
        opts: &[
            opt("peers", "Peers", List, "", "I2P addresses to connect to, comma-separated"),
            opt("connectable", "Connectable", Bool, "No", "Accept incoming connections over I2P"),
            opt("i2p_sam_host", "SAM host", Text, "127.0.0.1", "Address of the I2P router's SAM bridge"),
            opt("i2p_sam_port", "SAM port", Int, "7656", "Port of the I2P router's SAM bridge"),
        ],
        starter: &[("connectable", "Yes")],
    },
    InterfaceType {
        name: "RNodeInterface",
        label: "RNode (LoRa)",
        opts: &[
            opt("port", "Port", Text, "", "Serial device, or tcp://host or ble://name"),
            opt("frequency", "Frequency (Hz)", Int, "", "Radio frequency, e.g. 867200000"),
            opt("bandwidth", "Bandwidth (Hz)", Int, "", "Radio bandwidth, e.g. 125000"),
            alias("txpower", &["tx_power"], "TX power (dBm)", Int, "", "Transmit power"),
            alias("spreadingfactor", &["spreading_factor"], "Spreading factor", Int, "", "LoRa spreading factor (5-12)"),
            alias("codingrate", &["coding_rate"], "Coding rate", Int, "", "LoRa coding rate (5-8)"),
            alias("speed", &["baud_rate"], "Speed (baud)", Int, "", "Serial speed to the RNode"),
            opt("flow_control", "Flow control", Bool, "No", "Wait for the RNode to accept each frame"),
            alias("airtime_limit_short", &["st_alock"], "Short airtime limit (%)", Float, "", "Most airtime used over 15 seconds"),
            alias("airtime_limit_long", &["lt_alock"], "Long airtime limit (%)", Float, "", "Most airtime used over an hour"),
            BEACON[0],
            BEACON[1],
        ],
        starter: &[
            ("port", "/dev/ttyUSB0"),
            ("frequency", "867200000"),
            ("bandwidth", "125000"),
            ("txpower", "7"),
            ("spreadingfactor", "8"),
            ("codingrate", "5"),
        ],
    },
    InterfaceType {
        name: "RNodeMultiInterface",
        label: "RNode multi (sub-interfaces in the text editor)",
        opts: &[
            opt("port", "Port", Text, "", "Serial device of the RNode"),
            alias("speed", &["baud_rate"], "Speed (baud)", Int, "", "Serial speed to the RNode"),
            opt("flow_control", "Flow control", Bool, "No", "Wait for the RNode to accept each frame"),
            BEACON[0],
            BEACON[1],
        ],
        starter: &[("port", "/dev/ttyACM0")],
    },
    InterfaceType {
        name: "SerialInterface",
        label: "Serial",
        opts: &[SERIAL_LINE[0], SERIAL_LINE[1], SERIAL_LINE[2], SERIAL_LINE[3], SERIAL_LINE[4]],
        starter: &[("port", "/dev/ttyUSB0"), ("speed", "115200")],
    },
    InterfaceType {
        name: "KISSInterface",
        label: "KISS (TNC)",
        opts: &[
            SERIAL_LINE[0],
            SERIAL_LINE[1],
            SERIAL_LINE[2],
            SERIAL_LINE[3],
            SERIAL_LINE[4],
            KISS[0],
            KISS[1],
            KISS[2],
            KISS[3],
            KISS[4],
            BEACON[0],
            BEACON[1],
        ],
        starter: &[("port", "/dev/ttyUSB0"), ("speed", "115200")],
    },
    InterfaceType {
        name: "AX25KISSInterface",
        label: "AX.25 KISS (TNC)",
        opts: &[
            opt("callsign", "Callsign", Text, "", "Your callsign"),
            opt("ssid", "SSID", Int, "", "AX.25 SSID (0-15)"),
            SERIAL_LINE[0],
            SERIAL_LINE[1],
            SERIAL_LINE[2],
            SERIAL_LINE[3],
            SERIAL_LINE[4],
            KISS[0],
            KISS[1],
            KISS[2],
            KISS[3],
            KISS[4],
        ],
        starter: &[("callsign", "NOCALL"), ("ssid", "0"), ("port", "/dev/ttyUSB0"), ("speed", "115200")],
    },
    InterfaceType {
        name: "PipeInterface",
        label: "Pipe (external program)",
        opts: &[
            opt("command", "Command", Text, "", "Program to run; Reticulum talks to it over stdin and stdout"),
            opt("respawn_delay", "Respawn delay (s)", Int, "5", "Seconds before restarting the program when it exits"),
        ],
        starter: &[],
    },
];

pub fn interface_type(name: &str) -> Option<&'static InterfaceType> {
    INTERFACE_TYPES.iter().find(|t| t.name == name)
}
