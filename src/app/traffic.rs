//! Network traffic over all interfaces: bytes in and out, and how fast they
//! are moving (from how the interfaces' counters changed between updates);
//! and each interface's recent rates, for its graph.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Instant;

use crate::net::InterfaceInfo;

#[derive(Debug, Default, Clone)]
pub struct Traffic {
    /// Bytes received and sent since the interfaces started.
    pub rx_total: u64,
    pub tx_total: u64,
    /// Bytes per second over the last update (none until there have been
    /// two).
    pub rates: Option<(f64, f64)>,
    last: Option<(Instant, u64, u64)>,
    /// Each interface's rates (bytes per second in and out) at each update,
    /// oldest first: the last ten minutes.
    pub history: BTreeMap<String, VecDeque<(f64, f64)>>,
    /// Each interface's counters at the last update.
    counters: HashMap<String, (u64, u64)>,
}

/// How many rates each interface's history keeps: ten minutes of updates
/// (one every five seconds).
pub const HISTORY: usize = 120;

impl Traffic {
    /// Take in the interfaces' counters, read at `now`.
    pub fn update(&mut self, interfaces: &[InterfaceInfo], now: Instant) {
        let rx: u64 = interfaces.iter().map(|i| i.rx_bytes).sum();
        let tx: u64 = interfaces.iter().map(|i| i.tx_bytes).sum();
        if let Some((then, last_rx, last_tx)) = self.last {
            let secs = now.duration_since(then).as_secs_f64();
            if secs > 0.0 {
                // Counters start again when Reticulum restarts (or an
                // interface goes away): that update has no rate to give.
                let rate = |now: u64, then: u64| now.checked_sub(then).map_or(0.0, |d| d as f64 / secs);
                self.rates = Some((rate(rx, last_rx), rate(tx, last_tx)));
                for interface in interfaces {
                    let Some(&(last_rx, last_tx)) = self.counters.get(&interface.name) else { continue };
                    let rates = (rate(interface.rx_bytes, last_rx), rate(interface.tx_bytes, last_tx));
                    let history = self.history.entry(interface.name.clone()).or_default();
                    history.push_back(rates);
                    if history.len() > HISTORY {
                        history.pop_front();
                    }
                }
            }
        }
        // Those gone are forgotten.
        self.history.retain(|name, _| interfaces.iter().any(|i| &i.name == name));
        self.counters = interfaces.iter().map(|i| (i.name.clone(), (i.rx_bytes, i.tx_bytes))).collect();
        self.rx_total = rx;
        self.tx_total = tx;
        self.last = Some((now, rx, tx));
    }
}

/// Rates as a row of bars, one character each, the newest last: as tall as
/// they are against `peak`. Nothing moving is a space.
pub fn sparkline(rates: impl Iterator<Item = f64>, peak: f64) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    rates
        .map(|rate| {
            if rate <= 0.0 || peak <= 0.0 {
                ' '
            } else {
                BARS[(((rate / peak) * BARS.len() as f64).ceil() as usize).clamp(1, BARS.len()) - 1]
            }
        })
        .collect()
}

/// A rate, short: `0 B/s`, `340 B/s`, `1.2 KB/s`, `18 KB/s`, `1.1 MB/s`.
pub fn rate(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut value = bytes_per_sec.max(0.0);
    let mut unit = 0;
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 || value >= 9.95 { format!("{value:.0} {}", UNITS[unit]) } else { format!("{value:.1} {}", UNITS[unit]) }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn iface(rx: u64, tx: u64) -> InterfaceInfo {
        InterfaceInfo { name: "i".into(), online: true, rx_bytes: rx, tx_bytes: tx, ..Default::default() }
    }

    #[test]
    fn rates_come_from_the_change_between_updates() {
        let start = Instant::now();
        let mut traffic = Traffic::default();
        traffic.update(&[iface(1000, 500), iface(0, 100)], start);
        assert_eq!((traffic.rx_total, traffic.tx_total), (1000, 600));
        assert!(traffic.rates.is_none());
        traffic.update(&[iface(6000, 500), iface(0, 1100)], start + Duration::from_secs(5));
        assert_eq!(traffic.rates, Some((1000.0, 200.0)));
        // Counters that went back (Reticulum restarted) give no rate.
        traffic.update(&[iface(10, 10)], start + Duration::from_secs(10));
        assert_eq!(traffic.rates, Some((0.0, 0.0)));
    }

    #[test]
    fn each_interface_keeps_its_recent_rates() {
        let start = Instant::now();
        let mut traffic = Traffic::default();
        let named =
            |name: &str, rx, tx| InterfaceInfo { name: name.into(), online: true, rx_bytes: rx, tx_bytes: tx, ..Default::default() };
        traffic.update(&[named("LoRa", 0, 0), named("TCP", 0, 0)], start);
        assert!(traffic.history.is_empty());
        traffic.update(&[named("LoRa", 500, 50), named("TCP", 5000, 0)], start + Duration::from_secs(5));
        assert_eq!(traffic.history["LoRa"], [(100.0, 10.0)]);
        assert_eq!(traffic.history["TCP"], [(1000.0, 0.0)]);
        // One gone is forgotten; one new starts with its next update.
        traffic.update(&[named("LoRa", 500, 50), named("Auto", 70, 0)], start + Duration::from_secs(10));
        assert_eq!(traffic.history.keys().collect::<Vec<_>>(), ["LoRa"]);
        assert_eq!(traffic.history["LoRa"].len(), 2);
        for i in 0..HISTORY as u64 * 2 {
            traffic.update(&[named("LoRa", 500 + i, 50)], start + Duration::from_secs(15 + i * 5));
        }
        assert_eq!(traffic.history["LoRa"].len(), HISTORY);
    }

    #[test]
    fn sparklines_are_as_tall_as_the_rates() {
        assert_eq!(sparkline([0.0, 1.0, 50.0, 100.0].into_iter(), 100.0), " ▁▄█");
        assert_eq!(sparkline([0.0, 0.0].into_iter(), 0.0), "  ");
    }

    #[test]
    fn rates_read_short() {
        assert_eq!(rate(0.0), "0 B/s");
        assert_eq!(rate(340.4), "340 B/s");
        assert_eq!(rate(1234.0), "1.2 KB/s");
        assert_eq!(rate(18_400.0), "18 KB/s");
        assert_eq!(rate(999_700.0), "1.0 MB/s");
        assert_eq!(rate(1_100_000.0), "1.1 MB/s");
    }
}
