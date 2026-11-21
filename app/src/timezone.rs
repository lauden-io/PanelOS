//! The panel's time zone: Home Assistant's. The panel has no zone database; the PanelOS integration sends the zone
//! with the panel's settings as its POSIX TZ rule (`{"time_zone": {"name": "Europe/Berlin", "posix":
//! "CET-1CEST,M3.5.0,M10.5.0/3"}}`, the footer line of the zone's TZif file), and the app puts the rule in `TZ`.
//! Every local time in the app goes through the C library (`localtime_r`, `mktime`, `strftime`): the clocks, alarms,
//! timers, calendar, to-do dates, the screensaver and the connection log all follow it. The last zone is kept with
//! the integration's settings (`panel-config.json`), so a panel that starts without Home Assistant still shows its
//! own time; before the first settings ever, Central European time.
//!
//! glibc reads `TZ` once and keeps the zone: `localtime_r` does not look again, so a new rule needs `tzset`.
//! `tzset` and glibc's local time calls share glibc's own lock, so other threads see either zone, never half of one.
//! `TZ` is set once at the start, before any other thread runs; afterwards it is only replaced.

use std::sync::Mutex;

use serde_json::Value;

/// Central European time: the zone until the integration first sends one.
pub const FALLBACK: &str = "CET-1CEST,M3.5.0,M10.5.0/3";

extern "C" {
    // POSIX; not exported by the libc crate for this target.
    fn tzset();
}

/// The zone in effect: its name ("" when not known) and rule.
static CURRENT: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));

/// The zone in the integration's settings, if its rule is one the C library reads: (name, rule).
pub fn from_config(config: &Value) -> Option<(String, String)> {
    let zone = config.get("time_zone")?;
    let rule = zone.get("posix")?.as_str()?;
    if !valid_rule(rule) {
        return None;
    }
    let name = zone.get("name").and_then(Value::as_str).unwrap_or("");
    Some((name.to_string(), rule.to_string()))
}

/// At the start, before any other thread: the zone kept with the last settings, else `TZ` from the environment, else
/// the fallback.
pub fn init(saved: Option<&Value>) {
    let (name, rule) = saved
        .and_then(from_config)
        .or_else(|| std::env::var("TZ").ok().filter(|r| !r.is_empty()).map(|r| (String::new(), r)))
        .unwrap_or_else(|| (String::new(), FALLBACK.to_string()));
    set(&name, &rule);
}

/// The zone from the integration's settings: taken when its rule differs from the one in effect. Some((name, rule))
/// when the rule changed; settings without a zone (an older integration, Home Assistant starting) keep the one in
/// effect.
pub fn apply(config: Option<&Value>) -> Option<(String, String)> {
    let (name, rule) = config.and_then(from_config)?;
    {
        let mut current = CURRENT.lock().unwrap();
        if current.1 == rule {
            // the same rule under another name: nothing to set
            current.0 = name;
            return None;
        }
    }
    set(&name, &rule);
    Some((name, rule))
}

fn set(name: &str, rule: &str) {
    std::env::set_var("TZ", rule);
    // SAFETY: tzset only reads TZ again; glibc serializes it with its own local time calls.
    unsafe { tzset() };
    *CURRENT.lock().unwrap() = (name.to_string(), rule.to_string());
}

/// The rule in effect (the web helper starts with it).
pub fn rule() -> String {
    CURRENT.lock().unwrap().1.clone()
}

/// A POSIX TZ rule as the C library reads it: `std offset [dst [offset] [,start[/time],end[/time]]]`. Names of three
/// or more letters, or quoted in `<…>` (letters, digits, `+`, `-`); offsets `[+|-]hh[:mm[:ss]]` up to 24 hours; the
/// changes as `Jn` (1–365), `n` (0–365) or `Mm.w.d`, their times up to ±167 hours (TZif version 3).
pub fn valid_rule(rule: &str) -> bool {
    let b = rule.as_bytes();
    if b.is_empty() || b.len() > 128 {
        return false;
    }
    let mut i = 0;
    if !name(b, &mut i) || !offset(b, &mut i, 24) {
        return false;
    }
    if i == b.len() {
        return true;
    }
    if !name(b, &mut i) {
        return false;
    }
    if i < b.len() && b[i] != b',' && !offset(b, &mut i, 24) {
        return false;
    }
    if i == b.len() {
        return true;
    }
    for _ in 0..2 {
        if b.get(i) != Some(&b',') {
            return false;
        }
        i += 1;
        if !date(b, &mut i) {
            return false;
        }
        if b.get(i) == Some(&b'/') {
            i += 1;
            if !offset(b, &mut i, 167) {
                return false;
            }
        }
    }
    i == b.len()
}

/// A zone's name: three or more letters, or `<…>` with three or more letters, digits, `+` or `-`.
fn name(b: &[u8], i: &mut usize) -> bool {
    if b.get(*i) == Some(&b'<') {
        let start = *i + 1;
        let mut j = start;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j - start < 3 || b.get(j) != Some(&b'>') {
            return false;
        }
        *i = j + 1;
        true
    } else {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_alphabetic() {
            *i += 1;
        }
        *i - start >= 3
    }
}

/// `[+|-]hh[:mm[:ss]]` with the hours up to `most`.
fn offset(b: &[u8], i: &mut usize, most: u32) -> bool {
    if matches!(b.get(*i), Some(b'+' | b'-')) {
        *i += 1;
    }
    if !number(b, i, 3).is_some_and(|h| h <= most) {
        return false;
    }
    for _ in 0..2 {
        if b.get(*i) != Some(&b':') {
            break;
        }
        *i += 1;
        if !number(b, i, 2).is_some_and(|v| v <= 59) {
            return false;
        }
    }
    true
}

/// The date of a change: `Jn` (1–365, no leap day), `n` (0–365) or `Mm.w.d` (month 1–12, week 1–5, weekday 0–6).
fn date(b: &[u8], i: &mut usize) -> bool {
    match b.get(*i) {
        Some(b'J') => {
            *i += 1;
            number(b, i, 3).is_some_and(|n| (1..=365).contains(&n))
        }
        Some(b'M') => {
            *i += 1;
            let month = number(b, i, 2);
            if b.get(*i) != Some(&b'.') {
                return false;
            }
            *i += 1;
            let week = number(b, i, 1);
            if b.get(*i) != Some(&b'.') {
                return false;
            }
            *i += 1;
            let day = number(b, i, 1);
            month.is_some_and(|m| (1..=12).contains(&m)) && week.is_some_and(|w| (1..=5).contains(&w)) && day.is_some_and(|d| d <= 6)
        }
        Some(c) if c.is_ascii_digit() => number(b, i, 3).is_some_and(|n| n <= 365),
        _ => false,
    }
}

/// Up to `digits` decimal digits at `i` (at least one).
fn number(b: &[u8], i: &mut usize, digits: usize) -> Option<u32> {
    let start = *i;
    while *i < b.len() && *i - start < digits && b[*i].is_ascii_digit() {
        *i += 1;
    }
    if *i == start {
        return None;
    }
    std::str::from_utf8(&b[start..*i]).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rules_of_real_zones_are_valid() {
        for rule in [
            FALLBACK,
            "EST5EDT,M3.2.0,M11.1.0",
            "UTC0",
            "HST10",
            "IST-5:30",
            "<+0530>-5:30",
            "<-03>3",
            "<+01>-1",
            "<-0330>3:30",
            "NST3:30NDT,M3.2.0,M11.1.0",
            "IST-2IDT,M3.4.4/26,M10.5.0",
            "<-02>2<-01>,M3.5.0/-1,M10.5.0/0",
            "<+1030>-10:30<+11>-11,M10.1.0,M4.1.0",
            "<+1245>-12:45<+1345>,M9.5.0/2:45,M4.1.0/3:45",
            "AEST-10AEDT,M10.1.0,M4.1.0/3",
            "EST5EDT4,J60/2,J300/2",
            "EST5EDT4,59/2,299/2",
            "EST5EDT",
        ] {
            assert!(valid_rule(rule), "{rule}");
        }
    }

    #[test]
    fn other_texts_are_not_rules() {
        for rule in [
            "",
            "Europe/Berlin",
            ":Europe/Berlin",
            "CET",
            "CE-1",
            "-1",
            "CET-1CEST,M3.5.0",
            "CET-1CEST,M3.5.0,M10.5.0/3,",
            "CET-1CEST,M13.5.0,M10.5.0/3",
            "CET-1CEST,M3.6.0,M10.5.0/3",
            "CET-1CEST,M3.5.7,M10.5.0/3",
            "CET-1CEST,M3.0.0,M10.5.0/3",
            "CET-1CEST,J0,J300",
            "CET-1CEST,366,300",
            "CET-1CEST,M3.5.0/168,M10.5.0",
            "<+05",
            "<+5>-5",
            "CET-25",
            "CET-1:60",
            "CET-1 CEST",
            "CET-1CEST,M3.5.0,M10.5.0/3\n",
            "CET-1CÉST,M3.5.0,M10.5.0/3",
        ] {
            assert!(!valid_rule(rule), "{rule:?}");
        }
        assert!(!valid_rule(&format!("CET-1{}", "C".repeat(130))));
    }

    #[test]
    fn the_zone_message() {
        let config = json!({"pages": [], "time_zone": {"name": "America/New_York", "posix": "EST5EDT,M3.2.0,M11.1.0"}});
        assert_eq!(from_config(&config), Some(("America/New_York".to_string(), "EST5EDT,M3.2.0,M11.1.0".to_string())));
        // the name is only for the log
        assert_eq!(from_config(&json!({"time_zone": {"posix": "UTC0"}})), Some((String::new(), "UTC0".to_string())));
        // no zone (an older integration), no rule, a rule the C library would not read, the wrong types
        for config in [
            json!({"pages": []}),
            json!({"time_zone": null}),
            json!({"time_zone": {"name": "Europe/Berlin"}}),
            json!({"time_zone": {"name": "Europe/Berlin", "posix": ""}}),
            json!({"time_zone": {"name": "Europe/Berlin", "posix": "Europe/Berlin"}}),
            json!({"time_zone": {"name": "Europe/Berlin", "posix": 1}}),
            json!({"time_zone": "CET-1CEST,M3.5.0,M10.5.0/3"}),
        ] {
            assert_eq!(from_config(&config), None, "{config}");
        }
    }
}
