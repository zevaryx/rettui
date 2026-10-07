//! How times and dates read everywhere (the `clock` and `date_style`
//! settings): set once at start and whenever they change.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use chrono::{DateTime, Datelike, Local, TimeZone};

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
pub fn time(t: &DateTime<Local>, seconds: bool) -> String {
    time_in(t, seconds, TWELVE_HOUR.load(Ordering::Relaxed))
}

fn time_in(t: &DateTime<Local>, seconds: bool, twelve_hour: bool) -> String {
    let format = match (twelve_hour, seconds) {
        (false, false) => "%H:%M",
        (false, true) => "%H:%M:%S",
        (true, false) => "%-I:%M %p",
        (true, true) => "%-I:%M:%S %p",
    };
    t.format(format).to_string()
}

/// A day this year: "Oct 07", "07 Oct" or "10-07".
pub fn day(t: &DateTime<Local>) -> String {
    day_in(t, DATE_STYLE.load(Ordering::Relaxed))
}

fn day_in(t: &DateTime<Local>, style: u8) -> String {
    let format = match style {
        1 => "%d %b",
        2 => "%m-%d",
        _ => "%b %d",
    };
    t.format(format).to_string()
}

/// A day of any year: "Oct 07 2025", "07 Oct 2025" or "2025-10-07".
pub fn date(t: &DateTime<Local>) -> String {
    date_in(t, DATE_STYLE.load(Ordering::Relaxed))
}

fn date_in(t: &DateTime<Local>, style: u8) -> String {
    let format = match style {
        1 => "%d %b %Y",
        2 => "%Y-%m-%d",
        _ => "%b %d %Y",
    };
    t.format(format).to_string()
}

fn local(timestamp: f64) -> Option<DateTime<Local>> {
    Local.timestamp_opt(timestamp as i64, 0).single()
}

/// When a message was, as lists show it: the time today, else the day (or
/// the date, another year) and the time.
pub fn when(timestamp: f64) -> String {
    let Some(t) = local(timestamp) else { return String::new() };
    let now = Local::now();
    if t.date_naive() == now.date_naive() {
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
    let now = Local::now();
    if t.date_naive() == now.date_naive() {
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
        let t = Local.with_ymd_and_hms(2025, 10, 7, 14, 5, 9).unwrap();
        assert_eq!((time_in(&t, false, false), time_in(&t, true, false)), ("14:05".into(), "14:05:09".into()));
        assert_eq!((time_in(&t, false, true), time_in(&t, true, true)), ("2:05 PM".into(), "2:05:09 PM".into()));
        let styles = |style| (day_in(&t, style), date_in(&t, style));
        assert_eq!(styles(0), ("Oct 07".into(), "Oct 07 2025".into()));
        assert_eq!(styles(1), ("07 Oct".into(), "07 Oct 2025".into()));
        assert_eq!(styles(2), ("10-07".into(), "2025-10-07".into()));
        assert_eq!(DATE_STYLES.iter().position(|s| *s == "year-month-day"), Some(2));
    }
}
