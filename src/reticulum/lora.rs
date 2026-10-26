//! What an RNode's radio settings come to: its data rate, how long a full
//! packet is on air, how faint a signal it hears, and its link budget, as
//! Semtech's LoRa formulas (and Reticulum's, for the rate) work them out.
//! Shown beside an RNode interface's options in both UIs.

/// Reticulum's largest packet, in bytes.
const MTU: usize = 500;
/// What LoRa radios add to the thermal noise floor, in dB (SX127x and
/// SX126x, as their datasheets give it).
const NOISE_FIGURE: f64 = 6.0;
/// The preamble LoRa sends by default, in symbols.
const PREAMBLE: f64 = 8.0;

/// An RNode's radio settings, as in its interface section.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radio {
    pub frequency_hz: Option<f64>,
    pub bandwidth_hz: f64,
    pub spreading_factor: u32,
    /// 5 to 8, for 4/5 to 4/8.
    pub coding_rate: u32,
    pub txpower_dbm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Estimate {
    pub bits_per_second: f64,
    /// A full packet's time on air, in seconds.
    pub packet_secs: f64,
    /// The faintest signal it can hear, in dBm.
    pub sensitivity_dbm: f64,
    /// TX power less the sensitivity, in dB: the loss a link can take.
    pub link_budget_db: Option<f64>,
    /// How far that budget reaches in free space (in line of sight, with no
    /// antenna gain), in km.
    pub free_space_km: Option<f64>,
}

impl Radio {
    /// From an interface's values (`frequency`, `bandwidth`,
    /// `spreadingfactor`, `codingrate`, `txpower`), when the radio's are
    /// there and make sense.
    pub fn from_values<'a>(value: impl Fn(&str) -> Option<&'a str>) -> Option<Radio> {
        let number = |key: &str| value(key).and_then(|v| v.trim().parse::<f64>().ok()).filter(|n| n.is_finite());
        let radio = Radio {
            // LoRa's bands, and what any radio can send.
            frequency_hz: number("frequency").filter(|f| (100e6..=3e9).contains(f)),
            bandwidth_hz: number("bandwidth")?,
            spreading_factor: number("spreadingfactor")? as u32,
            coding_rate: number("codingrate")? as u32,
            txpower_dbm: number("txpower").filter(|p| (-30.0..=50.0).contains(p)),
        };
        let fits = (7_800.0..=1_625_000.0).contains(&radio.bandwidth_hz)
            && (5..=12).contains(&radio.spreading_factor)
            && (5..=8).contains(&radio.coding_rate);
        fits.then_some(radio)
    }

    pub fn estimate(&self) -> Estimate {
        let sf = self.spreading_factor as f64;
        let cr = self.coding_rate as f64;
        let symbol_secs = 2f64.powf(sf) / self.bandwidth_hz;
        // As Reticulum's RNodeInterface reckons it.
        let bits_per_second = sf * (4.0 / cr) / symbol_secs;
        // Semtech's time on air, with an explicit header and a CRC, and
        // low data rate optimisation where symbols are over 16 ms.
        let low_rate = if symbol_secs > 0.016 { 2.0 } else { 0.0 };
        let bits = (8 * MTU) as f64 - 4.0 * sf + 28.0 + 16.0;
        let payload_symbols = 8.0 + ((bits / (4.0 * (sf - low_rate))).ceil() * cr).max(0.0);
        let packet_secs = (PREAMBLE + 4.25 + payload_symbols) * symbol_secs;
        // The noise in the bandwidth, and the SNR each spreading factor can
        // still decode at.
        let snr_limit = -2.5 * (sf - 4.0);
        let sensitivity_dbm = -174.0 + 10.0 * self.bandwidth_hz.log10() + NOISE_FIGURE + snr_limit;
        let link_budget_db = self.txpower_dbm.map(|tx| tx - sensitivity_dbm);
        let free_space_km = match (link_budget_db, self.frequency_hz) {
            (Some(budget), Some(hz)) => Some(10f64.powf((budget - 32.44 - 20.0 * (hz / 1e6).log10()) / 20.0)),
            _ => None,
        };
        Estimate { bits_per_second, packet_secs, sensitivity_dbm, link_budget_db, free_space_km }
    }
}

/// What an RNode interface's options come to, if it is one.
pub fn for_options(options: &[super::OptionView]) -> Option<Estimate> {
    let value = |key: &str| options.iter().find(|o| o.key == key).and_then(|o| o.value.as_deref());
    if value("type")?.trim() != "RNodeInterface" {
        return None;
    }
    Some(Radio::from_values(value)?.estimate())
}

impl Estimate {
    /// In words, a line each.
    pub fn lines(&self) -> Vec<String> {
        let rate = self.bits_per_second;
        let rate = if rate >= 1000.0 { format!("{:.2} kbps", rate / 1000.0) } else { format!("{rate:.0} bps") };
        let mut lines = vec![format!("About {rate}; a full packet ({MTU} bytes) takes {:.2} s on air", self.packet_secs)];
        let mut hears = format!("Hears signals down to {:.1} dBm", self.sensitivity_dbm);
        if let Some(budget) = self.link_budget_db {
            hears.push_str(&format!(", a link budget of {budget:.1} dB"));
        }
        lines.push(hears);
        if let Some(km) = self.free_space_km {
            let km = if km >= 10.0 { format!("{km:.0}") } else { format!("{km:.1}") };
            lines.push(format!("In free space, up to about {km} km (less with hills, buildings or trees)"));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radio(values: &[(&str, &str)]) -> Option<Radio> {
        Radio::from_values(|key| values.iter().find(|(k, _)| *k == key).map(|(_, v)| *v))
    }

    #[test]
    fn the_numbers_are_semtechs() {
        let lora =
            radio(&[("frequency", "867200000"), ("bandwidth", "125000"), ("spreadingfactor", "8"), ("codingrate", "5"), ("txpower", "7")])
                .unwrap()
                .estimate();
        // 8 × 4/5 × 125000 / 256.
        assert!((lora.bits_per_second - 3125.0).abs() < 0.01, "{lora:?}");
        // -174 + 51 + 6 - 10.
        assert!((lora.sensitivity_dbm - -127.03).abs() < 0.01, "{lora:?}");
        assert!((lora.link_budget_db.unwrap() - 134.03).abs() < 0.01);
        // 12.25 symbols of preamble and 638 of payload, of 2.048 ms each.
        assert!((lora.packet_secs - 1.33171).abs() < 0.0001, "{lora:?}");
        let km = lora.free_space_km.unwrap();
        assert!((km - 138.5).abs() < 0.5, "{km}");
        assert_eq!(
            lora.lines(),
            [
                "About 3.13 kbps; a full packet (500 bytes) takes 1.33 s on air",
                "Hears signals down to -127.0 dBm, a link budget of 134.0 dB",
                "In free space, up to about 138 km (less with hills, buildings or trees)",
            ]
        );
        // Long range, slow: symbols over 16 ms are optimised.
        let slow = radio(&[("bandwidth", "125000"), ("spreadingfactor", "12"), ("codingrate", "8")]).unwrap().estimate();
        assert!(slow.bits_per_second < 200.0 && (slow.packet_secs - 26.878).abs() < 0.001, "{slow:?}");
        assert_eq!(slow.lines().len(), 2);
        assert!(slow.lines()[0].starts_with("About 183 bps"), "{:?}", slow.lines());
    }

    #[test]
    fn settings_that_cant_be_have_none() {
        assert!(radio(&[("bandwidth", "125000"), ("spreadingfactor", "8")]).is_none());
        assert!(radio(&[("bandwidth", "125000"), ("spreadingfactor", "13"), ("codingrate", "5")]).is_none());
        assert!(radio(&[("bandwidth", "nope"), ("spreadingfactor", "8"), ("codingrate", "5")]).is_none());
        assert!(radio(&[("bandwidth", "NaN"), ("spreadingfactor", "8"), ("codingrate", "5")]).is_none());
        assert!(radio(&[("bandwidth", "125000"), ("spreadingfactor", "8"), ("codingrate", "4")]).is_none());
        let no_power = radio(&[("frequency", "x"), ("bandwidth", "125000"), ("spreadingfactor", "8"), ("codingrate", "5")]).unwrap();
        assert_eq!((no_power.frequency_hz, no_power.estimate().link_budget_db), (None, None));
        let too_much =
            radio(&[("frequency", "1"), ("bandwidth", "125000"), ("spreadingfactor", "8"), ("codingrate", "5"), ("txpower", "717")]);
        assert_eq!(too_much.map(|r| (r.frequency_hz, r.txpower_dbm)), Some((None, None)));
    }
}
