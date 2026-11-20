//! Wall clock. The panel has no battery-backed clock and starts in 1970. NTP
//! sets the time a few seconds after Wi-Fi is up; until then it comes from
//! Home Assistant (the Date header of the WebSocket handshake), which also
//! covers networks without internet access, where NTP never answers.

/// The panel's time format for every clock time it shows (one panel-wide value, the UI's `Clock.h24`): true
/// 24-hour ("18:45"), false 12-hour ("6:45 PM"). Durations never follow it.
static H24: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_h24(on: bool) {
    H24.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn h24() -> bool {
    H24.load(std::sync::atomic::Ordering::Relaxed)
}

/// A clock time (hour 0-23, minute) in the panel's time format: "18:45" or "6:45 PM".
pub fn hm(h: u32, m: u32) -> String {
    hm_in(h, m, h24())
}

fn hm_in(h: u32, m: u32, h24: bool) -> String {
    if h24 {
        format!("{:02}:{:02}", h % 24, m % 60)
    } else {
        let h12 = match h % 12 { 0 => 12, x => x };
        format!("{h12}:{:02} {}", m % 60, if h % 24 < 12 { "AM" } else { "PM" })
    }
}

/// The local time of day of a unix time in the panel's time format ("18:45" / "6:45 PM").
pub fn local_hm(t: i64) -> String {
    let s = crate::pages::local_format(t, "%H %M");
    let mut it = s.split(' ').filter_map(|x| x.parse::<u32>().ok());
    match (it.next(), it.next()) {
        (Some(h), Some(m)) => hm(h, m),
        _ => crate::pages::local_format(t, "%H:%M"),
    }
}

/// 2026-01-01: anything later means the clock has been set.
const VALID_AFTER: u64 = 1_767_225_600;

pub fn valid() -> bool {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() > VALID_AFTER)
        .unwrap_or(false)
}

/// Sets the clock from an HTTP date ("Sat, 07 Nov 2026 09:10:49 GMT") if it
/// has not been set yet. True if it was set.
pub fn set_from_http_date(date: &str) -> bool {
    if valid() {
        return false;
    }
    let Some(secs) = parse_http_date(date) else { return false };
    if secs <= VALID_AFTER {
        return false;
    }
    let ts = libc::timespec { tv_sec: secs as libc::time_t, tv_nsec: 0 };
    // SAFETY: clock_settime(2) with a valid timespec on the stack.
    unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) == 0 }
}

/// Seconds since 1970 from an IMF-fixdate (RFC 9110), always in GMT.
fn parse_http_date(s: &str) -> Option<u64> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let parts: Vec<&str> = s.split_whitespace().collect();
    let [_weekday, day, month, year, time, zone] = parts.as_slice() else { return None };
    if *zone != "GMT" {
        return None;
    }
    let day: i64 = day.parse().ok()?;
    let month = MONTHS.iter().position(|m| m == month)? as i64 + 1;
    let year: i64 = year.parse().ok()?;
    let hms: Vec<i64> = time.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let [h, m, sec] = hms.as_slice() else { return None };
    if !(1..=31).contains(&day) || *h > 23 || *m > 59 || *sec > 60 {
        return None;
    }
    let secs = days_from_civil(year, month, day) * 86_400 + h * 3600 + m * 60 + sec;
    u64::try_from(secs).ok()
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formats() {
        assert_eq!(hm_in(18, 45, true), "18:45");
        assert_eq!(hm_in(6, 5, true), "06:05");
        assert_eq!(hm_in(18, 45, false), "6:45 PM");
        assert_eq!(hm_in(0, 0, false), "12:00 AM");
        assert_eq!(hm_in(12, 30, false), "12:30 PM");
    }

    #[test]
    fn http_dates() {
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784_111_777));
        assert_eq!(parse_http_date("Thu, 01 Jan 2026 00:00:00 GMT"), Some(VALID_AFTER));
        assert_eq!(parse_http_date("Sun, 29 Feb 2032 12:00:00 GMT"), Some(1_961_668_800));
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 CET"), None);
        assert_eq!(parse_http_date("garbage"), None);
    }
}
