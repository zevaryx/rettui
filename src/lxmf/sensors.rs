//! Sensor readings in telemetry (`FIELD_TELEMETRY`), as Sideband sends
//! them: besides the time and the location (see `super::fields`), a
//! battery, pressure, temperature, humidity, light, motion, the radio link,
//! the processor, memory and storage, tanks and fuel, power used and made,
//! and readings of the sender's own. Each is a msgpack value at its sensor's
//! number; rettui shows them in words, as Sideband's own list does.
//!
//! What this computer can say of itself goes the other way, with a
//! location sent (the `telemetry_sensors` setting): its battery (Linux),
//! processor load, memory (Linux) and the storage rettui keeps its data on
//! (Unix), as Sideband sends its own.

use std::path::Path;

use rmpv::Value;
use serde::{Deserialize, Serialize};

/// Sensor numbers (Sideband's).
const PRESSURE: u64 = 0x03;
const BATTERY: u64 = 0x04;
const PHYSICAL_LINK: u64 = 0x05;
const ACCELERATION: u64 = 0x06;
const TEMPERATURE: u64 = 0x07;
const HUMIDITY: u64 = 0x08;
const MAGNETIC_FIELD: u64 = 0x09;
const AMBIENT_LIGHT: u64 = 0x0A;
const GRAVITY: u64 = 0x0B;
const ANGULAR_VELOCITY: u64 = 0x0C;
const PROXIMITY: u64 = 0x0E;
const INFORMATION: u64 = 0x0F;
const RECEIVED: u64 = 0x10;
const POWER_CONSUMPTION: u64 = 0x11;
const POWER_PRODUCTION: u64 = 0x12;
const PROCESSOR: u64 = 0x13;
const RAM: u64 = 0x14;
const NVM: u64 = 0x15;
const TANK: u64 = 0x16;
const FUEL: u64 = 0x17;
const LXMF_PROPAGATION: u64 = 0x18;
const RNS_TRANSPORT: u64 = 0x19;
const CONNECTION_MAP: u64 = 0x1A;
const CUSTOM: u64 = 0xFF;

/// Most readings kept from one message (the rest are left out).
const MOST: usize = 40;
/// Longest text kept of a reading.
const LONGEST: usize = 200;

/// One reading, in words: the sensor (or what it measures, for those with
/// several entries), and its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading {
    pub sensor: String,
    pub value: String,
}

/// A number, shown with at most `places` decimals and no trailing zeros.
fn number(value: f64, places: i32) -> String {
    let scale = 10f64.powi(places);
    let rounded = (value * scale).round() / scale;
    let text = format!("{rounded:.*}", places.max(0) as usize);
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text };
    if text == "-0" { "0".into() } else { text }
}

fn float(value: &Value) -> Option<f64> {
    let n = match value {
        Value::F64(v) => *v,
        Value::F32(v) => f64::from(*v),
        Value::Integer(n) => n.as_f64()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

/// Text a sender gave (a label or a unit), tidied: no control characters,
/// and not too long.
fn text(value: &Value) -> Option<String> {
    let s = match value {
        Value::String(s) => s.as_str()?.to_string(),
        Value::Binary(b) => String::from_utf8(b.clone()).ok()?,
        _ => return None,
    };
    let s: String = s.chars().filter(|c| !c.is_control()).take(LONGEST).collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// What a value is, as shown: text as it is, numbers tidied, anything else
/// briefly.
fn any(value: &Value) -> String {
    match value {
        Value::Boolean(b) => if *b { "yes" } else { "no" }.into(),
        Value::Nil => "none".into(),
        v if float(v).is_some() => number(float(v).unwrap_or_default(), 2),
        v => text(v).unwrap_or_else(|| "(not shown)".into()),
    }
}

/// `[x, y, z]`, with a unit.
fn axes(value: &Value, unit: &str) -> Option<String> {
    let Value::Array(parts) = value else { return None };
    let [x, y, z] = parts.as_slice() else { return None };
    Some(format!("x {}, y {}, z {} {unit}", number(float(x)?, 2), number(float(y)?, 2), number(float(z)?, 2)))
}

/// A size in bytes, as people read it.
fn size(bytes: f64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1000.0 && unit < units.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{} {}", number(value, if unit == 0 { 0 } else { 1 }), units[unit])
}

/// Power in watts, with a prefix.
fn watts(value: f64) -> String {
    let (scaled, prefix) = match value.abs() {
        v if v >= 1e9 => (value / 1e9, "G"),
        v if v >= 1e6 => (value / 1e6, "M"),
        v if v >= 1e3 => (value / 1e3, "k"),
        _ => (value, ""),
    };
    format!("{} {prefix}W", number(scaled, 2))
}

/// The entries of a sensor with several (`[[label, [values…]], …]`): each
/// with its label (`0`, or none, is the sensor's own name).
fn entries<'a>(value: &'a Value, own: &str) -> Vec<(String, &'a [Value])> {
    let Value::Array(list) = value else { return Vec::new() };
    list.iter()
        .filter_map(|entry| {
            let Value::Array(pair) = entry else { return None };
            let [label, Value::Array(values)] = pair.as_slice() else { return None };
            Some((text(label).unwrap_or_else(|| own.to_string()), values.as_slice()))
        })
        .collect()
}

/// Used of a capacity, both in bytes: "3.2 GB of 8 GB (40%)".
fn used_of(values: &[Value]) -> Option<String> {
    let capacity = float(values.first()?)?;
    let used = float(values.get(1)?)?;
    let percent = if capacity > 0.0 { format!(" ({}%)", number(used / capacity * 100.0, 1)) } else { String::new() };
    Some(format!("{} used of {}{percent}", size(used), size(capacity)))
}

/// The readings in a telemetry map, in the order of their sensors, but the
/// time and the location (shown apart).
pub fn read(sensors: &[(Value, Value)]) -> Vec<Reading> {
    let mut sorted: Vec<(u64, &Value)> = sensors.iter().filter_map(|(k, v)| Some((k.as_u64()?, v))).collect();
    sorted.sort_by_key(|(sid, _)| *sid);
    let mut readings = Vec::new();
    let mut add = |sensor: &str, value: Option<String>| {
        if let Some(value) = value {
            readings.push(Reading { sensor: sensor.chars().take(LONGEST).collect(), value });
        }
    };
    for (sid, value) in sorted {
        if value.is_nil() {
            continue;
        }
        match sid {
            PRESSURE => add("Pressure", float(value).map(|v| format!("{} mbar", number(v, 1)))),
            BATTERY => add("Battery", {
                let Value::Array(parts) = value else { continue };
                parts.first().and_then(float).map(|percent| {
                    let mut said = format!("{}%", number(percent, 1));
                    if parts.get(1).and_then(Value::as_bool) == Some(true) {
                        said.push_str(", charging");
                    }
                    if let Some(temperature) = parts.get(2).and_then(float) {
                        said.push_str(&format!(", {} °C", number(temperature, 1)));
                    }
                    said
                })
            }),
            PHYSICAL_LINK => add("Radio link", {
                let Value::Array(parts) = value else { continue };
                let part = |i: usize| parts.get(i).and_then(float);
                let said: Vec<String> = [
                    part(2).map(|q| format!("quality {}%", number(q, 1))),
                    part(0).map(|rssi| format!("RSSI {} dBm", number(rssi, 1))),
                    part(1).map(|snr| format!("SNR {} dB", number(snr, 1))),
                ]
                .into_iter()
                .flatten()
                .collect();
                (!said.is_empty()).then(|| said.join(", "))
            }),
            ACCELERATION => add("Acceleration", axes(value, "m/s²")),
            TEMPERATURE => add("Temperature", float(value).map(|c| format!("{} °C", number(c, 1)))),
            HUMIDITY => add("Humidity", float(value).map(|p| format!("{}%", number(p, 1)))),
            MAGNETIC_FIELD => add("Magnetic field", axes(value, "µT")),
            AMBIENT_LIGHT => add("Light", float(value).map(|lux| format!("{} lux", number(lux, 1)))),
            GRAVITY => add("Gravity", axes(value, "m/s²")),
            ANGULAR_VELOCITY => add("Rotation", axes(value, "rad/s")),
            PROXIMITY => add(
                "Proximity",
                match value {
                    Value::Boolean(near) => Some(if *near { "something near" } else { "nothing near" }.into()),
                    v => float(v).map(|cm| format!("{} cm", number(cm, 1))),
                },
            ),
            INFORMATION => add("Information", text(value)),
            RECEIVED => add("Received", {
                let Value::Array(parts) = value else { continue };
                let hash = |v: Option<&Value>| match v {
                    Some(Value::Binary(b)) if !b.is_empty() => Some(hex::encode(&b[..b.len().min(8)])),
                    _ => None,
                };
                let mut said: Vec<String> = Vec::new();
                if let Some(by) = hash(parts.first()) {
                    said.push(format!("by {by}"));
                }
                if let Some(via) = hash(parts.get(1)) {
                    said.push(format!("via {via}"));
                }
                if let Some(distance) = parts.get(2).and_then(float) {
                    said.push(if distance >= 1000.0 {
                        format!("{} km away", number(distance / 1000.0, 1))
                    } else {
                        format!("{} m away", number(distance, 0))
                    });
                }
                (!said.is_empty()).then(|| said.join(", "))
            }),
            POWER_CONSUMPTION | POWER_PRODUCTION => {
                let (own, verb) = if sid == POWER_CONSUMPTION { ("Power used", "using") } else { ("Power made", "making") };
                for (label, values) in entries(value, own) {
                    add(&label, values.first().and_then(float).map(|w| format!("{verb} {}", watts(w))));
                }
            }
            PROCESSOR => {
                for (label, values) in entries(value, "Processor") {
                    let load = values.first().and_then(float).map(|l| format!("{}% busy", number(l * 100.0, 1)));
                    let averages = match values.get(1) {
                        Some(Value::Array(avgs)) if avgs.len() == 3 => {
                            let avgs: Option<Vec<String>> = avgs.iter().map(|a| float(a).map(|a| number(a, 2))).collect();
                            avgs.map(|a| format!("load averages {}", a.join(", ")))
                        }
                        _ => None,
                    };
                    let clock = values.get(2).and_then(float).map(|hz| match hz {
                        hz if hz >= 1e9 => format!("at {} GHz", number(hz / 1e9, 2)),
                        hz => format!("at {} MHz", number(hz / 1e6, 0)),
                    });
                    let said: Vec<String> = [load, clock, averages].into_iter().flatten().collect();
                    add(&label, (!said.is_empty()).then(|| said.join(", ")));
                }
            }
            RAM | NVM => {
                let own = if sid == RAM { "Memory" } else { "Storage" };
                for (label, values) in entries(value, own) {
                    add(&label, used_of(values));
                }
            }
            TANK | FUEL => {
                let own = if sid == TANK { "Tank" } else { "Fuel" };
                for (label, values) in entries(value, own) {
                    let capacity = values.first().and_then(float);
                    let level = values.get(1).and_then(float);
                    let unit = values.get(2).and_then(text).unwrap_or_default();
                    add(
                        &label,
                        level.map(|level| {
                            let percent = match capacity {
                                Some(c) if c > 0.0 => format!(" ({}%)", number(level / c * 100.0, 1)),
                                _ => String::new(),
                            };
                            let of = capacity.map(|c| format!(" of {}", number(c, 1))).unwrap_or_default();
                            format!("{}{of} {unit}{percent}", number(level, 1)).replace("  ", " ")
                        }),
                    );
                }
            }
            CUSTOM => {
                for (label, values) in entries(value, "Reading") {
                    add(&label, values.first().map(any));
                }
            }
            RNS_TRANSPORT => add("Reticulum transport", Some("statistics (shown in Sideband)".into())),
            LXMF_PROPAGATION => add("Propagation node", Some("statistics (shown in Sideband)".into())),
            CONNECTION_MAP => add("Connection map", Some("(shown in Sideband)".into())),
            // The time and the location, and sensors not known.
            _ => {}
        }
    }
    readings.truncate(MOST);
    readings
}

/// What this computer can say of itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Local {
    /// Percent charged, and whether it's charging.
    pub battery: Option<(f64, bool)>,
    /// How busy the processor is (0 to 1), and its load averages over 1, 5
    /// and 15 minutes.
    pub processor: Option<(f64, [f64; 3])>,
    /// Memory: its size and how much is used, in bytes.
    pub memory: Option<(u64, u64)>,
    /// The storage holding `dir`: its size and how much is used.
    pub storage: Option<(u64, u64)>,
}

impl Local {
    /// The readings, now, with `dir` the data directory (whose storage is
    /// said).
    pub fn now(dir: &Path) -> Self {
        Self { battery: battery(), processor: processor(), memory: memory(), storage: storage(dir) }
    }

    /// As telemetry entries (Sideband's), to go in the map with the time
    /// and the location.
    pub fn entries(&self) -> Vec<(Value, Value)> {
        let mut out = Vec::new();
        let one = |label: &str, values: Vec<Value>| Value::Array(vec![Value::Array(vec![Value::from(label), Value::Array(values)])]);
        if let Some((percent, charging)) = self.battery {
            out.push((
                Value::from(BATTERY),
                Value::Array(vec![Value::from((percent * 10.0).round() / 10.0), Value::from(charging), Value::Nil]),
            ));
        }
        if let Some((load, averages)) = self.processor {
            let averages = Value::Array(averages.iter().map(|a| Value::from((a * 100.0).round() / 100.0)).collect());
            out.push((Value::from(PROCESSOR), one("Processor", vec![Value::from((load * 1000.0).round() / 1000.0), averages, Value::Nil])));
        }
        if let Some((capacity, used)) = self.memory {
            out.push((Value::from(RAM), one("Memory", vec![Value::from(capacity), Value::from(used)])));
        }
        if let Some((capacity, used)) = self.storage {
            out.push((Value::from(NVM), one("Storage", vec![Value::from(capacity), Value::from(used)])));
        }
        out
    }
}

/// The first battery the system has, on Linux.
fn battery() -> Option<(f64, bool)> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let supplies = std::fs::read_dir("/sys/class/power_supply").ok()?;
    supplies.flatten().find_map(|entry| {
        let path = entry.path();
        let read = |name: &str| std::fs::read_to_string(path.join(name)).ok().map(|s| s.trim().to_string());
        if read("type")? != "Battery" {
            return None;
        }
        let percent: f64 = read("capacity")?.parse().ok()?;
        let charging = read("status").is_some_and(|s| s == "Charging" || s == "Full");
        Some((percent.clamp(0.0, 100.0), charging))
    })
}

/// The load averages, and from the last minute's, how busy the processor
/// is (shared between its cores).
#[cfg(unix)]
fn processor() -> Option<(f64, [f64; 3])> {
    let mut averages = [0f64; 3];
    // SAFETY: getloadavg writes at most the 3 values asked for.
    let got = unsafe { libc::getloadavg(averages.as_mut_ptr(), 3) };
    if got != 3 {
        return None;
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    Some(((averages[0] / cores).clamp(0.0, 1.0), averages))
}

#[cfg(not(unix))]
fn processor() -> Option<(f64, [f64; 3])> {
    None
}

/// Memory, on Linux: its size, and what's used (not free for programs).
fn memory() -> Option<(u64, u64)> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = |name: &str| -> Option<u64> {
        let line = info.lines().find(|l| l.starts_with(name))?;
        line[name.len()..].trim().trim_end_matches("kB").trim().parse::<u64>().ok().map(|n| n * 1024)
    };
    let total = kb("MemTotal:")?;
    let available = kb("MemAvailable:")?;
    Some((total, total.saturating_sub(available)))
}

/// The storage `dir` is on: its size, and what's used.
#[cfg(unix)]
fn storage(dir: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: an all-zero statvfs is a valid value for it to fill in.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a NUL-terminated string and `stats` is writable.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    let block = stats.f_frsize as u64;
    let total = stats.f_blocks as u64 * block;
    let free = stats.f_bfree as u64 * block;
    (total > 0).then(|| (total, total.saturating_sub(free)))
}

#[cfg(not(unix))]
fn storage(_dir: &Path) -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: Vec<(u64, Value)>) -> Vec<(Value, Value)> {
        entries.into_iter().map(|(k, v)| (Value::from(k), v)).collect()
    }

    fn entry(label: Value, values: Vec<Value>) -> Value {
        Value::Array(vec![Value::Array(vec![label, Value::Array(values)])])
    }

    #[test]
    fn sidebands_sensors_in_words() {
        let sensors = map(vec![
            (0x01, Value::from(1_790_000_000)),
            (BATTERY, Value::Array(vec![Value::from(87.25), Value::from(true), Value::from(31.4)])),
            (PRESSURE, Value::from(1013.25)),
            (PHYSICAL_LINK, Value::Array(vec![Value::from(-97), Value::from(8.5), Value::from(80)])),
            (ACCELERATION, Value::Array(vec![Value::from(0.1), Value::from(-0.0), Value::from(9.81)])),
            (TEMPERATURE, Value::from(21.55)),
            (HUMIDITY, Value::from(40)),
            (MAGNETIC_FIELD, Value::Array(vec![Value::from(1), Value::from(2), Value::from(3)])),
            (AMBIENT_LIGHT, Value::from(310.0)),
            (GRAVITY, Value::Array(vec![Value::from(0), Value::from(0), Value::from(9.8)])),
            (ANGULAR_VELOCITY, Value::Array(vec![Value::from(0), Value::from(0.5), Value::from(0)])),
            (PROXIMITY, Value::from(true)),
            (INFORMATION, Value::from("On the hill\u{7}")),
            (
                RECEIVED,
                Value::Array(vec![Value::Binary(vec![0xab; 16]), Value::Binary(vec![0xcd; 16]), Value::from(1520.0), Value::from(1500.0)]),
            ),
            (POWER_CONSUMPTION, entry(Value::from(0), vec![Value::from(2500.0), Value::Nil])),
            (POWER_PRODUCTION, entry(Value::from("Solar"), vec![Value::from(12.5), Value::from("solar-power")])),
            (
                PROCESSOR,
                entry(
                    Value::from(0),
                    vec![Value::from(0.25), Value::Array(vec![Value::from(1.0), Value::from(0.5), Value::from(0.25)]), Value::from(1.8e9)],
                ),
            ),
            (RAM, entry(Value::from(0), vec![Value::from(8_000_000_000u64), Value::from(3_200_000_000u64)])),
            (NVM, entry(Value::from("SD card"), vec![Value::from(64_000_000_000u64), Value::from(16_000_000_000u64)])),
            (TANK, entry(Value::from("Water"), vec![Value::from(200.0), Value::from(50.0), Value::from("L"), Value::Nil])),
            (FUEL, entry(Value::from(0), vec![Value::from(40), Value::from(30), Value::Nil, Value::Nil])),
            (RNS_TRANSPORT, Value::Map(Vec::new())),
            (CUSTOM, entry(Value::from("Door"), vec![Value::from("open"), Value::Nil])),
            (0x99, Value::from("unknown sensor")),
            (TEMPERATURE + 0x100, Value::Nil),
        ]);
        let said: Vec<String> = read(&sensors).iter().map(|r| format!("{}: {}", r.sensor, r.value)).collect();
        assert_eq!(
            said,
            [
                "Pressure: 1013.3 mbar",
                "Battery: 87.3%, charging, 31.4 °C",
                "Radio link: quality 80%, RSSI -97 dBm, SNR 8.5 dB",
                "Acceleration: x 0.1, y 0, z 9.81 m/s²",
                "Temperature: 21.6 °C",
                "Humidity: 40%",
                "Magnetic field: x 1, y 2, z 3 µT",
                "Light: 310 lux",
                "Gravity: x 0, y 0, z 9.8 m/s²",
                "Rotation: x 0, y 0.5, z 0 rad/s",
                "Proximity: something near",
                "Information: On the hill",
                "Received: by abababababababab, via cdcdcdcdcdcdcdcd, 1.5 km away",
                "Power used: using 2.5 kW",
                "Solar: making 12.5 W",
                "Processor: 25% busy, at 1.8 GHz, load averages 1, 0.5, 0.25",
                "Memory: 3.2 GB used of 8 GB (40%)",
                "SD card: 16 GB used of 64 GB (25%)",
                "Water: 50 of 200 L (25%)",
                "Fuel: 30 of 40 (75%)",
                "Reticulum transport: statistics (shown in Sideband)",
                "Door: open",
            ]
        );
        // Odd values are left out rather than shown wrong.
        let odd =
            map(vec![(BATTERY, Value::from("full")), (PROCESSOR, Value::from(3)), (ACCELERATION, Value::Array(vec![Value::from(1)]))]);
        assert!(read(&odd).is_empty());
    }

    #[test]
    fn this_computers_readings_go_as_sideband_reads_them() {
        let local = Local {
            battery: Some((54.0, false)),
            processor: Some((0.125, [0.5, 0.4, 0.3])),
            memory: Some((16_000_000_000, 4_000_000_000)),
            storage: Some((500_000_000_000, 250_000_000_000)),
        };
        let said: Vec<String> = read(&local.entries()).iter().map(|r| format!("{}: {}", r.sensor, r.value)).collect();
        assert_eq!(
            said,
            [
                "Battery: 54%",
                "Processor: 12.5% busy, load averages 0.5, 0.4, 0.3",
                "Memory: 4 GB used of 16 GB (25%)",
                "Storage: 250 GB used of 500 GB (50%)",
            ]
        );
        assert!(Local::default().entries().is_empty());
        // What this computer has, here: nothing that isn't so.
        let here = Local::now(&std::env::temp_dir());
        if let Some((total, used)) = here.storage {
            assert!(used <= total);
        }
        if let Some((load, _)) = here.processor {
            assert!((0.0..=1.0).contains(&load));
        }
    }
}
