//! How times and dates read everywhere (the `clock` and `date_style`
//! settings): set once at start and whenever they change.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};

pub const CLOCKS: &[&str] = &["24-hour", "12-hour"];
pub const DATE_STYLES: &[&str] = &["month-day", "day-month", "year-month-day"];

static TWELVE_HOUR: AtomicBool = AtomicBool::new(false);
/// An index into [`DATE_STYLES`].
static DATE_STYLE: AtomicU8 = AtomicU8::new(0);

/// Use these from now on (as the settings say).
pub fn set(clock: &str, date_style: &str) {
    TWELVE_HOUR.store(clock == "12-hour", Ordering::Relaxed);
    let style = DATE_STYLES.iter().position(|s| *s == date_style).unwrap_or(0);
    DATE_STYLE.store(style as u8, Ordering::Relaxed);
}

/// A time of day: "14:05" or "2:05 PM", with seconds if asked.
pub fn time(t: &Zoned, seconds: bool) -> String {
    time_in(t, seconds, TWELVE_HOUR.load(Ordering::Relaxed))
}

fn time_in(t: &Zoned, seconds: bool, twelve_hour: bool) -> String {
    let format = match (twelve_hour, seconds) {
        (false, false) => "%H:%M",
        (false, true) => "%H:%M:%S",
        (true, false) => "%-I:%M %p",
        (true, true) => "%-I:%M:%S %p",
    };
    t.strftime(format).to_string()
}

/// A day this year: "Oct 07", "07 Oct" or "10-07".
pub fn day(t: &Zoned) -> String {
    day_in(t, DATE_STYLE.load(Ordering::Relaxed))
}

fn day_in(t: &Zoned, style: u8) -> String {
    let format = match style {
        1 => "%d %b",
        2 => "%m-%d",
        _ => "%b %d",
    };
    t.strftime(format).to_string()
}

/// A day of any year: "Oct 07 2025", "07 Oct 2025" or "2025-10-07".
pub fn date(t: &Zoned) -> String {
    date_in(t, DATE_STYLE.load(Ordering::Relaxed))
}

fn date_in(t: &Zoned, style: u8) -> String {
    let format = match style {
        1 => "%d %b %Y",
        2 => "%Y-%m-%d",
        _ => "%b %d %Y",
    };
    t.strftime(format).to_string()
}

/// A time with its day before it: "Oct 07 14:05" (the year too, for
/// another year's), with seconds if asked.
pub fn dated(t: &Zoned, seconds: bool) -> String {
    let day = if t.year() == Zoned::now().year() { day(t) } else { date(t) };
    format!("{day} {}", time(t, seconds))
}

/// A time as it reads here, if it can be one: see [`in_zone`].
pub fn local(timestamp: f64) -> Option<Zoned> {
    in_zone(&TimeZone::system(), timestamp)
}

/// A time in `zone`, if it can be one there. A time is held up to the year
/// 9999 (and back to 9999 BC), and a message can say any time.
fn in_zone(zone: &TimeZone, timestamp: f64) -> Option<Zoned> {
    Some(Timestamp::from_second(timestamp as i64).ok()?.to_zoned(zone.clone()))
}

/// When a message was, as lists show it: the time today, else the day (or
/// the date, another year) and the time.
pub fn when(timestamp: f64) -> String {
    let Some(t) = local(timestamp) else { return String::new() };
    let now = Zoned::now();
    if t.date() == now.date() {
        time(&t, false)
    } else if t.year() == now.year() {
        format!("{} {}", day(&t), time(&t, false))
    } else {
        format!("{} {}", date(&t), time(&t, false))
    }
}

/// When, briefly: the time today, the day this year, else the date.
pub fn short(timestamp: f64) -> String {
    let Some(t) = local(timestamp) else { return String::new() };
    let now = Zoned::now();
    if t.date() == now.date() {
        time(&t, false)
    } else if t.year() == now.year() {
        day(&t)
    } else {
        date(&t)
    }
}

/// When, in full: the date and the time.
pub fn full(timestamp: f64) -> String {
    local(timestamp).map(|t| format!("{} {}", date(&t), time(&t, false))).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_or_twenty_four_hours_and_three_date_orders() {
        // The settings themselves are global (other tests draw times): each
        // way is tried directly.
        let t = local_time(2025, 10, 7, 14, 5, 9);
        assert_eq!((time_in(&t, false, false), time_in(&t, true, false)), ("14:05".into(), "14:05:09".into()));
        assert_eq!((time_in(&t, false, true), time_in(&t, true, true)), ("2:05 PM".into(), "2:05:09 PM".into()));
        let styles = |style| (day_in(&t, style), date_in(&t, style));
        assert_eq!(styles(0), ("Oct 07".into(), "Oct 07 2025".into()));
        assert_eq!(styles(1), ("07 Oct".into(), "07 Oct 2025".into()));
        assert_eq!(styles(2), ("10-07".into(), "2025-10-07".into()));
        assert_eq!(DATE_STYLES.iter().position(|s| *s == "year-month-day"), Some(2));
    }

    fn local_time(year: i16, month: i8, day: i8, hour: i8, minute: i8, second: i8) -> Zoned {
        jiff::civil::date(year, month, day).at(hour, minute, second, 0).to_zoned(TimeZone::system()).unwrap()
    }

    #[test]
    fn times_past_what_jiff_holds_are_none() {
        use jiff::tz::Offset;
        // The last second held, in UTC, is the last one in a zone behind UTC
        // too; one more, or one before the first, is none, in any zone.
        let (last, first) = (Timestamp::MAX.as_second() as f64, Timestamp::MIN.as_second() as f64);
        let ahead = TimeZone::fixed(Offset::constant(9));
        let behind = TimeZone::fixed(Offset::constant(-5));
        assert!(in_zone(&ahead, last + 1.0).is_none());
        assert!(in_zone(&behind, first - 1.0).is_none());
        assert_eq!(in_zone(&behind, last).map(|t| t.year()), Some(9999));
        assert_eq!(in_zone(&ahead, first).map(|t| t.year()), Some(-9999));
        assert_eq!(
            in_zone(&ahead, 1_800_000_000.0).map(|t| t.strftime("%Y-%m-%d %H:%M:%S").to_string()),
            Some("2027-01-15 17:00:00".into())
        );
        // Here, whatever the zone: past the end shows no time.
        assert_eq!((when(f64::MAX), short(f64::INFINITY), full(-f64::MAX)), (String::new(), String::new(), String::new()));
        assert!(local(last + 1.0).is_none());
    }

    #[test]
    fn times_with_their_day() {
        // Another year's has its year; this year's doesn't.
        let then = local_time(2001, 2, 3, 4, 5, 6);
        let stamp = dated(&then, true);
        assert!(stamp.contains("2001") && stamp.ends_with(&time(&then, true)), "{stamp}");
        let now = Zoned::now();
        let stamp = dated(&now, false);
        assert_eq!(stamp, format!("{} {}", day(&now), time(&now, false)));
    }
}
