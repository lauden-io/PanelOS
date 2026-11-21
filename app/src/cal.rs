//! Calendar page: the calendar entities picked for it, their events from Home
//! Assistant (REST `/api/calendars`, read in a thread every five minutes and
//! when the page's calendars change), laid out as agenda, day, three days,
//! week or month, and the look's menus: the chip's picker (the view, the
//! calendars shown), the page settings, a day's list, an event's details and
//! its edit form (written back with `calendar/event/update` and `/delete`).

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use crate::draw::{self, Ink, Job};
use crate::pages::{local_format, now, parse_iso};
use crate::{texts, with_state, AppWindow, CalBoard, CalCell, CalDay, CalEvent, CalItem, WallCell};

/// One event as read from Home Assistant.
#[derive(Clone, Debug)]
struct Ev {
    title: String,
    /// Epoch seconds; all-day events from local midnight to local midnight.
    start: i64,
    end: i64,
    all_day: bool,
    /// The calendar's place among the page's calendars (its colour, its pen); set for the page the event is shown on.
    cal: usize,
    /// The calendar entity it comes from.
    entity: String,
    note: String,
    place: String,
    /// Home Assistant's event id and the occurrence of a recurring one (empty when none).
    uid: String,
    rid: String,
}

impl Ev {
    /// The id the views carry, to find the event again (an edit, a delete).
    fn id(&self) -> String {
        format!("{}|{}|{}|{}", self.entity, self.uid, self.rid, self.start)
    }
}

/// Calendar colours by order (HA has none; the integration may set them later).
const TINTS: [u32; 6] = [0x3d9bff, 0xff7a3d, 0x2fd27a, 0xc86bfa, 0xffb21e, 0xff4fa0];
/// The wall calendar's pens by calendar order (the same as its written notes).
const INKS: [Ink; 5] = [Ink::Blue, Ink::Purple, Ink::Red, Ink::Green, Ink::Black];
const INK_RGB: [u32; 5] = [0x1b348a, 0x5c3296, 0xc0242a, 0x16703c, 0x1e1e22];

/// Calendar entity features (Home Assistant's CalendarEntityFeature).
const CAN_DELETE: u64 = 2;
const CAN_UPDATE: u64 = 4;

fn rgb(c: u32) -> Color {
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn tint(i: usize) -> Color {
    rgb(TINTS[i % TINTS.len()])
}

thread_local! {
    /// The events of every calendar the calendar pages show.
    static EVENTS: std::cell::RefCell<Vec<Ev>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Each page's days behind the month grid's and the wall calendar's cells and the week's columns (a tap opens that
    /// day's list), by the page's row (`PageSlot.n`).
    static DAYS: std::cell::RefCell<HashMap<(usize, &'static str), Vec<i64>>> = std::cell::RefCell::new(HashMap::new());
    /// Each page's events behind the departure board's rows and the tear-off page's lines, with their "when" line (a
    /// tap opens them).
    static ROWS: std::cell::RefCell<HashMap<(usize, &'static str), Vec<(CalEvent, String)>>> = std::cell::RefCell::new(HashMap::new());
    /// Each page's back / forward from today in its view's steps.
    static SHIFT: std::cell::RefCell<HashMap<usize, i64>> = std::cell::RefCell::new(HashMap::new());
    /// Each page's own start view in the integration as last seen (none until the page first had its settings).
    static START: std::cell::RefCell<HashMap<usize, Option<i64>>> = std::cell::RefCell::new(HashMap::new());
    /// Each page's calendars hidden on the panel, set here and not yet back from the integration.
    static HIDE_PENDING: std::cell::RefCell<HashMap<usize, Vec<String>>> = std::cell::RefCell::new(HashMap::new());
    /// What each page's views were made from last (they are made again only when it changes).
    static LAST: std::cell::RefCell<HashMap<usize, u64>> = std::cell::RefCell::new(HashMap::new());
    /// The event being edited.
    static EDIT: std::cell::RefCell<Option<Edit>> = const { std::cell::RefCell::new(None) };
    /// A delete waiting for its undo strip to run out: the event's id, its calendar entity, uid and occurrence.
    static DELETING: std::cell::RefCell<Option<(String, String, String, String)>> = const { std::cell::RefCell::new(None) };
}

fn shift_of(n: usize) -> i64 {
    SHIFT.with(|o| o.borrow().get(&n).copied().unwrap_or(0))
}

fn set_shift(n: usize, v: i64) -> i64 {
    SHIFT.with(|o| o.borrow_mut().insert(n, v).unwrap_or(0))
}

/// The events of a page's calendars, each with its calendar's place among the page's (its colour).
fn page_events(entities: &[String]) -> Vec<Ev> {
    EVENTS.with(|e| {
        e.borrow()
            .iter()
            .filter_map(|ev| entities.iter().position(|c| *c == ev.entity).map(|cal| Ev { cal, ..ev.clone() }))
            .collect()
    })
}

/// The edit form's event (the times live in the UI: `cal-edit-start-min` / `-end-min`, minutes from `day0`).
#[derive(Clone, Debug)]
struct Edit {
    id: String,
    entity: String,
    uid: String,
    rid: String,
    title: String,
    day0: i64,
    /// An all-day event's length in days (kept when only its day changes).
    days: i64,
    note: String,
    place: String,
}

/// What the fetch thread reads (the calendars change with the integration).
static WANTED: Mutex<Vec<String>> = Mutex::new(Vec::new());
static STARTED: AtomicBool = AtomicBool::new(false);
/// Read again at once (after an edit or a delete).
static REFETCH: AtomicBool = AtomicBool::new(false);

/// A new time zone: the events read again (the six weeks asked for and the all-day events' days are local dates) and
/// every view made again.
pub fn time_zone_changed() {
    REFETCH.store(true, Ordering::SeqCst);
    LAST.with(|l| l.borrow_mut().clear());
}

/// Local midnight of the day of `t` (epoch seconds).
fn midnight(t: i64) -> i64 {
    // SAFETY: localtime_r / mktime on local values.
    unsafe {
        let tt = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&tt, &mut tm);
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm) as i64
    }
}

/// Local midnight `days` days after the one of `t` (DST-proof).
fn day_after(t: i64, days: i64) -> i64 {
    midnight(midnight(t) + days * 86400 + 12 * 3600)
}

/// The local time `min` minutes after the midnight `day0` (whole days first, then the clock time: DST-proof).
fn at(day0: i64, min: i64) -> i64 {
    let base = day_after(day0, min.div_euclid(1440));
    let m = min.rem_euclid(1440);
    // SAFETY: localtime_r / mktime on local values.
    unsafe {
        let tt = base as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&tt, &mut tm);
        tm.tm_hour = (m / 60) as i32;
        tm.tm_min = (m % 60) as i32;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm) as i64
    }
}

/// Minutes of `t` from the midnight `day0` (whole days and the clock time).
fn minutes_from(day0: i64, t: i64) -> i64 {
    let days = (midnight(t) - midnight(day0) + 43200).div_euclid(86400);
    let hm = local_format(t, "%H %M");
    let mut it = hm.split(' ').filter_map(|x| x.parse::<i64>().ok());
    days * 1440 + it.next().unwrap_or(0) * 60 + it.next().unwrap_or(0)
}

/// A local date "YYYY-MM-DD" as the epoch of its midnight.
fn date_epoch(d: &str) -> Option<i64> {
    let y: i32 = d.get(0..4)?.parse().ok()?;
    let m: i32 = d.get(5..7)?.parse().ok()?;
    let day: i32 = d.get(8..10)?.parse().ok()?;
    // SAFETY: mktime on a local struct.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = y - 1900;
        tm.tm_mon = m - 1;
        tm.tm_mday = day;
        tm.tm_isdst = -1;
        Some(libc::mktime(&mut tm) as i64)
    }
}

fn weekday(t: i64) -> i64 {
    // 0 Monday … 6 Sunday
    (local_format(t, "%u").parse::<i64>().unwrap_or(1) - 1).clamp(0, 6)
}

/// Starts the reader (once Home Assistant's address and login are known).
fn ensure_started(ui: &AppWindow, s: &crate::UiState) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let (Some(url), Some(auth)) = (s.config.as_ref().map(|c| c.ha_url.clone()), s.auth.clone()) else {
        STARTED.store(false, Ordering::SeqCst);
        return;
    };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let mut last: Vec<String> = Vec::new();
        let mut next = std::time::Instant::now();
        loop {
            let wanted = WANTED.lock().unwrap().clone();
            if wanted != last || std::time::Instant::now() >= next || REFETCH.swap(false, Ordering::SeqCst) {
                last = wanted.clone();
                next = std::time::Instant::now() + Duration::from_secs(300);
                let events = fetch(&url, &auth, &wanted);
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    EVENTS.with(|e| *e.borrow_mut() = events);
                    if let Some(ui) = weak.upgrade() {
                        with_state(|s| refresh(&ui, s));
                    }
                });
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

/// The events of the calendars from the start of this week for six weeks.
fn fetch(url: &str, auth: &Arc<crate::auth::Auth>, calendars: &[String]) -> Vec<Ev> {
    let from = day_after(now(), -weekday(now()) - 7);
    let to = day_after(from, 49);
    let iso = |t: i64| local_format(t, "%Y-%m-%dT%H:%M:%S%z");
    let mut out = Vec::new();
    let Ok(token) = auth.access_token() else { return out };
    for (i, cal) in calendars.iter().enumerate() {
        let query = format!("{url}/api/calendars/{cal}?start={}&end={}", enc(&iso(from)), enc(&iso(to)));
        let resp = ureq::get(&query).set("Authorization", &format!("Bearer {token}")).timeout(Duration::from_secs(10)).call();
        let Ok(resp) = resp else {
            eprintln!("nspanel-app: calendar {cal}: {}", resp.err().map(|e| e.to_string()).unwrap_or_default());
            continue;
        };
        let mut body = String::new();
        if resp.into_reader().take(4 * 1024 * 1024).read_to_string(&mut body).is_err() {
            continue;
        }
        let Ok(Value::Array(list)) = serde_json::from_str::<Value>(&body) else { continue };
        for e in list {
            let date = |k: &str| e.get(k).and_then(|v| v.get("date")).and_then(Value::as_str).and_then(date_epoch);
            let time = |k: &str| e.get(k).and_then(|v| v.get("dateTime")).and_then(Value::as_str).and_then(parse_iso);
            let (start, end, all_day) = match (time("start"), time("end"), date("start"), date("end")) {
                (Some(a), Some(b), _, _) => (a, b, false),
                (_, _, Some(a), Some(b)) => (a, b, true),
                _ => continue,
            };
            let text = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            out.push(Ev {
                title: text("summary"),
                start,
                end,
                all_day,
                cal: i,
                entity: cal.clone(),
                note: text("description"),
                place: text("location"),
                uid: text("uid"),
                rid: text("recurrence_id"),
            });
        }
    }
    out.sort_by_key(|e| (e.start, !e.all_day));
    out
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b':' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A clock time in the panel's time format ("18:45" / "6:45 PM").
fn hm(t: i64) -> String {
    crate::clock::local_hm(t)
}

/// Two clock times as a range: "09:30–11:00", "9:30–11:00 AM" (one AM / PM when both share it), "11:30 AM–1:00 PM".
fn join_range(a: &str, b: &str) -> String {
    match (a.rsplit_once(' '), b.rsplit_once(' ')) {
        (Some((ta, sa)), Some((_, sb))) if sa == sb && (sa == "AM" || sa == "PM") => format!("{ta}–{b}"),
        _ => format!("{a}–{b}"),
    }
}

fn range(start: i64, end: i64) -> String {
    join_range(&hm(start), &hm(end))
}

/// A clock time on the departure board's flap cells: "18:45", or in 12-hour "6:45P" (as boards print it).
fn board_text(s: &str) -> String {
    s.replace(" AM", "A").replace(" PM", "P")
}

fn board_hm(t: i64) -> String {
    board_text(&hm(t))
}

/// "YYYY-MM-DD HH:MM:SS" (Home Assistant's local time of a calendar's next event) as epoch seconds.
fn local_epoch(s: &str) -> Option<i64> {
    let num = |a: usize, b: usize| s.get(a..b).and_then(|v| v.parse::<i32>().ok());
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13).unwrap_or(0), num(14, 16).unwrap_or(0), num(17, 19).unwrap_or(0));
    // SAFETY: mktime on a local struct.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = y - 1900;
        tm.tm_mon = mo - 1;
        tm.tm_mday = d;
        tm.tm_hour = h;
        tm.tm_min = mi;
        tm.tm_sec = se;
        tm.tm_isdst = -1;
        Some(libc::mktime(&mut tm) as i64)
    }
}

/// The next (or current) event of the given calendars for the home widgets, from the calendars' own states
/// (Home Assistant puts each calendar's next event in its attributes), coloured like on the calendar page.
pub fn next_event(states: &std::collections::HashMap<String, crate::ha::EntityState>, calendars: &[String], t: i64) -> crate::HomeEvent {
    let mut best: Option<(i64, i64, bool, String, usize)> = None;
    for (i, id) in calendars.iter().enumerate() {
        let Some(e) = states.get(id) else { continue };
        let attr = |k: &str| e.attrs.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let title = attr("message");
        let (Some(start), Some(end)) = (local_epoch(&attr("start_time")), local_epoch(&attr("end_time"))) else { continue };
        if title.is_empty() || end <= t {
            continue;
        }
        let all_day = e.attrs.get("all_day").and_then(Value::as_bool).unwrap_or(false);
        if best.as_ref().is_none_or(|b| start < b.0) {
            best = Some((start, end, all_day, title, i));
        }
    }
    let Some((start, end, all_day, title, i)) = best else { return crate::HomeEvent::default() };
    let rel = if start <= t {
        "now".to_string()
    } else if midnight(start) == midnight(t) {
        let d = start - t;
        let (h, m) = (d / 3600, (d % 3600) / 60);
        if h > 0 { format!("in {h} h {m} min") } else { format!("in {} min", m.max(1)) }
    } else if midnight(start) == day_after(t, 1) {
        "Tomorrow".to_string()
    } else if start < day_after(t, 7) {
        local_format(start, "%A")
    } else {
        local_format(start, "%-d %B")
    };
    let time = if all_day { "All day".to_string() } else { format!("{} – {}", hm(start), hm(end)) };
    crate::HomeEvent { title: title.into(), time: time.into(), rel: rel.into(), tint: tint(i) }
}

fn rel(start: i64, end: i64, t: i64) -> String {
    if start <= t && t < end {
        return "now".into();
    }
    let d = start - t;
    if d <= 0 || d > 12 * 3600 {
        return String::new();
    }
    let (h, m) = (d / 3600, (d % 3600) / 60);
    if h > 0 { format!("in {h} h {m} min") } else { format!("in {m} min") }
}

/// The two-letter code of a calendar (the departure board's gate).
fn code(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase()
}

/// What the views need to know of each calendar of the page.
struct Cals {
    names: Vec<String>,
    features: Vec<u64>,
}

fn cals(s: &crate::UiState, entities: &[String]) -> Cals {
    let attr = |id: &String| s.states.get(id);
    Cals {
        names: entities
            .iter()
            .map(|id| attr(id).and_then(|e| e.attrs.get("friendly_name")).and_then(Value::as_str).unwrap_or(id).to_string())
            .collect(),
        features: entities
            .iter()
            .map(|id| attr(id).and_then(|e| e.attrs.get("supported_features")).and_then(Value::as_u64).unwrap_or(0))
            .collect(),
    }
}

/// An event's description or place as the details show it. Calendars hand over what the invitation held: HTML,
/// the targets of links in angle brackets ("Join<https://…>"), links hundreds of characters long, separator lines,
/// runs of blank lines. Tags go (a line break, a paragraph or a list item starts a line), entities are read, a link
/// target in angle brackets goes, a long bare link shows only its host, a line without any letter or digit goes,
/// blank runs become one empty line, and past `cap` characters the text ends with "…".
fn readable(text: &str, cap: usize) -> String {
    let text = text.replace("\r\n", "\n").replace(['\r', '\t'], " ");
    // tags and link targets
    let mut plain = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(i) = rest.find('<') {
        plain.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let tag_like = after.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '/' || c == '!');
        match after.find('>').filter(|_| tag_like) {
            Some(end) => {
                let inner = after[..end].trim();
                let name: String =
                    inner.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
                let lower = inner.to_ascii_lowercase();
                if lower.starts_with("http") || lower.starts_with("mailto:") || lower.starts_with("tel:") {
                    plain.push(' ');
                } else if name == "li" {
                    // a list item starts a line with a bullet; its end adds nothing (the next item starts its own)
                    if !inner.starts_with('/') {
                        plain.push_str("\n• ");
                    }
                } else if matches!(name.as_str(), "br" | "p" | "div" | "tr" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
                    plain.push('\n');
                }
                rest = &after[end + 1..];
            }
            None => {
                plain.push('<');
                rest = after;
            }
        }
    }
    plain.push_str(rest);
    // entities
    let mut decoded = String::with_capacity(plain.len());
    let mut rest = plain.as_str();
    while let Some(i) = rest.find('&') {
        decoded.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let entity = after.find(';').filter(|&e| e <= 8).map(|e| &after[..e]);
        let ch = entity.and_then(|e| match e {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => e
                .strip_prefix("#x")
                .or_else(|| e.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| e.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        });
        match (entity, ch) {
            (Some(e), Some(c)) => {
                decoded.push(c);
                rest = &after[e.len() + 1..];
            }
            _ => {
                decoded.push('&');
                rest = after;
            }
        }
    }
    decoded.push_str(rest);
    // words and lines
    let mut lines: Vec<String> = Vec::new();
    for line in decoded.split('\n') {
        let words: Vec<String> = line
            .split(' ')
            .filter(|w| !w.is_empty())
            .map(|w| {
                let lower = w.to_ascii_lowercase();
                let link = ["https://", "http://"].iter().find_map(|p| lower.starts_with(*p).then_some(p.len()));
                match link {
                    Some(n) if w.chars().count() > 40 => {
                        let host: String = w[n..].chars().take_while(|&c| !matches!(c, '/' | '?' | '#' | '>')).collect();
                        format!("{host}/…")
                    }
                    _ => w.to_string(),
                }
            })
            .collect();
        let line = words.join(" ");
        if !line.is_empty() && !line.chars().any(char::is_alphanumeric) {
            continue;
        }
        if line.is_empty() && lines.last().is_none_or(String::is_empty) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let out = lines.join("\n");
    if out.chars().count() <= cap {
        return out;
    }
    let cut: String = out.chars().take(cap).collect();
    let cut = match cut.rfind(char::is_whitespace) {
        Some(i) if cut.len() - i < 40 => cut[..i].to_string(),
        _ => cut,
    };
    format!("{}…", cut.trim_end())
}

/// An event for the views; `day0` is the midnight of the day it is shown on.
fn event(e: &Ev, c: &Cals, day0: i64, t: i64) -> CalEvent {
    let start_min = if e.all_day { 0 } else { ((e.start - day0) / 60).clamp(0, 24 * 60) as i32 };
    let end_min = if e.all_day { 24 * 60 } else { ((e.end - day0) / 60).clamp(0, 24 * 60) as i32 };
    let now = !e.all_day && e.start <= t && t < e.end;
    let name = c.names.get(e.cal).cloned().unwrap_or_default();
    CalEvent {
        title: e.title.clone().into(),
        time: if e.all_day { "All day".into() } else { range(e.start, e.end).into() },
        t1: if e.all_day { "All day".into() } else { hm(e.start).into() },
        t2: if e.all_day { SharedString::new() } else { hm(e.end).into() },
        code: code(&name).into(),
        cal: name.into(),
        tint: tint(e.cal),
        ink: rgb(INK_RGB[e.cal % INK_RGB.len()]),
        all_day: e.all_day,
        start: start_min,
        end: end_min.max(start_min + 15),
        lane: 0,
        lanes: 1,
        day: weekday(day0) as i32,
        past: e.end <= t,
        now,
        progress: if now { ((t - e.start) as f32 / (e.end - e.start).max(1) as f32).clamp(0.0, 1.0) } else { 0.0 },
        rel: if e.all_day || midnight(day0) != midnight(t) { SharedString::new() } else { rel(e.start, e.end, t).into() },
        note: readable(&e.note, 1500).into(),
        place: readable(&e.place, 300).into(),
        editable: c.features.get(e.cal).is_some_and(|f| f & CAN_UPDATE != 0) && !e.uid.is_empty(),
        id: e.id().into(),
        date: local_format(day0, "%a %-d %b").into(),
    }
}

/// Side by side when timed events overlap: each gets a lane of its group.
fn lanes(list: &mut [CalEvent]) {
    let mut i = 0;
    while i < list.len() {
        let mut end = list[i].end;
        let mut j = i + 1;
        while j < list.len() && list[j].start < end {
            end = end.max(list[j].end);
            j += 1;
        }
        let mut lane_end: Vec<i32> = Vec::new();
        for k in i..j {
            let lane = lane_end.iter().position(|&e| e <= list[k].start).unwrap_or(lane_end.len());
            if lane == lane_end.len() {
                lane_end.push(list[k].end);
            } else {
                lane_end[lane] = list[k].end;
            }
            list[k].lane = lane as i32;
        }
        for k in i..j {
            list[k].lanes = lane_end.len() as i32;
        }
        i = j;
    }
}

fn day_label(day0: i64, today: i64) -> String {
    let d = (midnight(day0 + 12 * 3600) - today + 43200) / 86400;
    match d {
        0 => format!("Today · {}", local_format(day0, "%a %-d %b")),
        1 => "Tomorrow".into(),
        _ => local_format(day0, "%a %-d %b"),
    }
}

/// "Series - S01E02 - Title" (an episode calendar): the series name; other titles as they are.
fn short(title: &str) -> String {
    if let Some(i) = title.find(" - S") {
        let rest: Vec<char> = title[i + 4..].chars().take(3).collect();
        if rest.len() == 3 && rest[0].is_ascii_digit() && rest[1].is_ascii_digit() && rest[2] == 'E' {
            return title[..i].to_string();
        }
    }
    title.to_string()
}

/// The wall calendar: this month's grid, each day's events written in pen by the app (one ink per
/// calendar, today circled in red); the days behind its cells kept for page `n`.
fn wall(n: usize, events: &[Ev], t: i64, written: Option<&[usize]>) -> Vec<WallCell> {
    let today = midnight(t);
    let first = date_epoch(&local_format(t, "%Y-%m-01")).unwrap_or(today);
    let grid0 = day_after(first, -weekday(first));
    let month = local_format(t, "%m");
    DAYS.with(|d| d.borrow_mut().insert((n, "wall"), (0..42).map(|k| day_after(grid0, k)).collect()));
    (0..42)
        .map(|k| {
            let day0 = day_after(grid0, k);
            let day1 = day_after(day0, 1);
            let notes: Vec<(String, Ink)> =
                events
                    .iter()
                    .filter(|e| e.start < day1 && e.end > day0 && written.is_none_or(|w| w.contains(&e.cal)))
                    .map(|e| (short(&e.title), INKS[e.cal % INKS.len()]))
                    .collect();
            let is_today = day0 == today;
            WallCell {
                num: local_format(day0, "%-d").parse().unwrap_or(0),
                other_month: local_format(day0, "%m") != month,
                sunday: weekday(day0) == 6,
                today: is_today,
                // today's circle is drawn on by the page itself (spec 25.K4)
                notes: if notes.is_empty() {
                    Default::default()
                } else {
                    draw::get(Job::Cell { notes, today: false, seed: (day0 / 86400) as u32 })
                },
            }
        })
        .collect()
}

/// Which calendars page `n` shows (by entity order): the integration's choice, or the one just made on the panel
/// until the integration has it. At least one is always shown.
fn shown(page: &Value, entities: &[String], n: usize) -> Vec<bool> {
    let set: Vec<String> = crate::pages::option_of(page, "hidden_calendars")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let hidden = HIDE_PENDING.with(|p| {
        let mut p = p.borrow_mut();
        match p.get(&n).cloned() {
            Some(want) if want == set => {
                p.remove(&n);
                set.clone()
            }
            Some(want) => want,
            None => set.clone(),
        }
    });
    shown_of(entities, &hidden)
}

fn shown_of(entities: &[String], hidden: &[String]) -> Vec<bool> {
    let on: Vec<bool> = entities.iter().map(|id| !hidden.contains(id)).collect();
    if on.iter().any(|&x| x) { on } else { vec![true; entities.len()] }
}

/// The chip: the one calendar shown by name, else how many are shown.
fn chip_text(names: &[String], on: &[bool]) -> String {
    let shown: Vec<&String> = names.iter().zip(on).filter(|(_, &o)| o).map(|(n, _)| n).collect();
    match shown.as_slice() {
        [one] => (*one).clone(),
        many => format!("{} calendars", many.len()),
    }
}

/// A text on `n` flap cells: capitals, one character a cell, padded with blanks (times stand at the right).
fn flap_cells(text: &str, n: usize) -> Vec<SharedString> {
    let chars: Vec<String> = text.to_uppercase().chars().filter(|c| !c.is_control()).take(n).map(String::from).collect();
    let pad: Vec<String> = std::iter::repeat_n(" ".to_string(), n - chars.len()).collect();
    let cells = if text.contains(':') { pad.into_iter().chain(chars).collect::<Vec<_>>() } else { chars.into_iter().chain(pad).collect() };
    cells.into_iter().map(SharedString::from).collect()
}

/// The picker's rows: changed in place (a gate flips, a box is ticked) unless the calendars themselves changed; the
/// new model when they did.
fn set_items(cur: &ModelRc<CalItem>, items: Vec<CalItem>) -> Option<ModelRc<CalItem>> {
    let same = |a: &CalItem, b: &CalItem| a.name == b.name && a.on == b.on && a.last == b.last && a.tint == b.tint;
    if slint::Model::row_count(cur) == items.len() {
        if let Some(vec) = slint::Model::as_any(cur).downcast_ref::<VecModel<CalItem>>() {
            for (i, it) in items.into_iter().enumerate() {
                if !slint::Model::row_data(vec, i).is_some_and(|c| same(&c, &it)) {
                    slint::Model::set_row_data(vec, i, it);
                }
            }
            return None;
        }
    }
    Some(ModelRc::new(VecModel::from(items)))
}

/// Every calendar page, each from its own calendars, the ones it shows, its view and its look.
pub fn refresh(ui: &AppWindow, s: &mut crate::UiState) {
    let pages = crate::pages::kind_pages(s.panel_config.as_ref(), "calendar").into_iter().map(|(_, p)| p.clone()).collect::<Vec<_>>();
    // every calendar of every calendar page is read
    let mut wanted: Vec<String> = Vec::new();
    for p in &pages {
        for e in crate::pages::entities_of(p) {
            if !wanted.contains(&e) {
                wanted.push(e);
            }
        }
    }
    *WANTED.lock().unwrap() = wanted.clone();
    if wanted.is_empty() {
        crate::pages::keep_insts(&ui.get_cal_inst(), pages.len());
        return;
    }
    ensure_started(ui, s);
    let t = now();
    // what every calendar page shows alike: the clocks and dates of the looks, their own lines
    ui.set_cal_board_clock_cells(ModelRc::new(VecModel::from(flap_cells(&board_hm(t), if crate::clock::h24() { 5 } else { 6 }))));
    let year = local_format(t, "%Y");
    ui.set_cal_wall_year(year.clone().into());
    let leap = year.parse::<i64>().is_ok_and(|y| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0);
    ui.set_cal_desk_line(format!("DESK CALENDAR {year} \u{b7} {} PAGES", if leap { 366 } else { 365 }).into());
    ui.set_cal_now_min(((t - midnight(t)) / 60) as i32);
    if crate::page_looks(ui, "calendar").contains(&3) {
        ui.set_cal_wall_month(local_format(t, "%B").to_uppercase().into());
        if ui.get_cal_wall_photo().size().width == 0 {
            ui.set_cal_wall_photo(crate::quick::picture("cal/wall-photo.jpg"));
        }
    }
    // (once a minute, and when the time format changes)
    let minute = t / 60 * 2 + crate::clock::h24() as i64;
    thread_local!(static SHARED: std::cell::Cell<i64> = const { std::cell::Cell::new(-1) });
    if SHARED.with(|m| m.replace(minute)) != minute {
        ui.set_cal_month_names(ModelRc::new(VecModel::from(
            ["M", "T", "W", "T", "F", "S", "S"].iter().map(|d| SharedString::from(*d)).collect::<Vec<_>>(),
        )));
        ui.set_cal_board_clock(board_hm(t).into());
        ui.set_cal_board_date(local_format(t, "%a %-d %b").to_uppercase().into());
        // the board's crawl: four lines (never shorter than its strip)
        let crawl: Vec<String> = (0..4).map(|i| texts::kept("cal.board.tickers", i)).collect();
        ui.set_cal_board_ticker(crawl.join("   \u{2022}   ").into());
        ui.set_cal_board_empty(texts::kept("cal.board.empty", 0).into());
        ui.set_cal_empty_title(texts::kept("cal.empty.titles", 0).into());
        ui.set_cal_empty_sub(texts::kept("cal.empty.subs", 0).into());
        ui.set_cal_tear_month(local_format(t, "%B %Y").to_uppercase().into());
        ui.set_cal_tear_day(local_format(t, "%-d").parse().unwrap_or(1));
        ui.set_cal_tear_weekday(local_format(t, "%A").to_uppercase().into());
        ui.set_cal_tear_sunday(weekday(t) == 6);
        ui.set_cal_tear_info(format!("week {} \u{b7} day {}", local_format(t, "%V").trim_start_matches('0'), local_format(t, "%j").trim_start_matches('0')).into());
        ui.set_cal_tear_line(texts::kept("cal.tear.lines", 0).into());
    }
    for (n, page) in pages.iter().enumerate() {
        refresh_page(ui, s, n, page);
    }
    crate::pages::keep_insts(&ui.get_cal_inst(), pages.len());
}

/// One calendar page (row `n` of the kind's data).
fn refresh_page(ui: &AppWindow, s: &crate::UiState, n: usize, page: &Value) {
    let entities = crate::pages::entities_of(page);
    if entities.is_empty() {
        return;
    }
    let look = crate::pages::variant_of(page);
    let c = cals(s, &entities);
    let on = shown(page, &entities, n);
    // the top-right chip names what the page shows: its calendar, or how many
    let chip = chip_text(&c.names, &on);
    // the chip's picker: every calendar of the page in its colour (the wall calendar's ink too), shown or not
    let n_on = on.iter().filter(|&&o| o).count();
    let items: Vec<CalItem> = c
        .names
        .iter()
        .enumerate()
        .map(|(i, nm)| CalItem {
            name: nm.clone().into(),
            tint: tint(i),
            ink: rgb(INK_RGB[i % INK_RGB.len()]),
            code: code(nm).into(),
            on: on[i],
            last: on[i] && n_on == 1,
            cells: ModelRc::new(VecModel::from(flap_cells(&code(nm), 2).into_iter().chain(flap_cells(nm, 9)).collect::<Vec<_>>())),
        })
        .collect();
    // the start view is the last one used (kept by the integration); an own start view set there wins when it
    // changes; applied once, so a view picked on the panel is never undone by its own echo
    let own = crate::pages::option_of(page, "view").and_then(|v| v.as_i64()).filter(|v| (0..=4).contains(v));
    let last = crate::pages::option_of(page, "last_view").and_then(|v| v.as_i64()).filter(|v| (0..=4).contains(v));
    let mut view = crate::pages::inst(&ui.get_cal_inst(), n).view;
    match START.with(|v| v.borrow_mut().insert(n, own)) {
        None => {
            view = own.or(last).unwrap_or(0) as i32;
            set_shift(n, 0);
        }
        Some(prev) if prev != own => {
            if let Some(v) = own {
                view = v as i32;
                set_shift(n, 0);
            }
        }
        _ => {}
    }
    crate::pages::with_inst(ui.get_cal_inst(), |m| ui.set_cal_inst(m), n, |d| {
        if d.chip.as_str() != chip {
            d.chip = chip.clone().into();
        }
        if let Some(m) = set_items(&d.cals, items) {
            d.cals = m;
        }
        d.view = view;
    });
    let t = now();
    let today = midnight(t);
    let deleting = DELETING.with(|d| d.borrow().as_ref().map(|x| x.0.clone()));
    let events: Vec<Ev> =
        page_events(&entities).into_iter().filter(|e| on.get(e.cal).copied().unwrap_or(false) && deleting.as_deref() != Some(e.id().as_str())).collect();
    let on_day = |day0: i64| -> Vec<&Ev> {
        let day1 = day_after(day0, 1);
        events.iter().filter(|e| e.start < day1 && e.end > day0).collect()
    };
    // back / forward: days in the day view, weeks in the week and 3-day views, months in the month view
    let shift = shift_of(n);
    // the wall calendar's pen notes come in one by one: its cells follow them
    let wall_cells = (look == 3).then(|| {
        // the calendars written on the wall (daily routines would fill every cell): set in the integration, else all
        let written: Option<Vec<usize>> = crate::pages::option_of(page, "wall_calendars")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).filter_map(|id| entities.iter().position(|e| e == id)).collect());
        wall(n, &events, t, written.as_deref())
    });
    if let Some(rows) = wall_cells {
        crate::pages::with_inst(ui.get_cal_inst(), |m| ui.set_cal_inst(m), n, |d| {
            if let Some(m) = crate::pages::update_rows(&d.wall_cells, rows) {
                d.wall_cells = m;
            }
        });
    }
    // New models rebuild the page: only when the events, the minute, the view, the shown calendars or the time
    // format changed.
    let key = events.iter().fold((t / 60) as u64 ^ ((view as u64) << 56) ^ ((shift as u64) << 40) ^ ((crate::clock::h24() as u64) << 63), |k, e| {
        k.wrapping_mul(31).wrapping_add(e.start as u64 ^ (e.end as u64).rotate_left(17) ^ e.title.len() as u64)
    }) ^ c.names.iter().fold(0u64, |k, nm| k.wrapping_mul(131).wrapping_add(nm.len() as u64 ^ nm.bytes().map(u64::from).sum::<u64>()))
        ^ (crate::clock::h24() as u64) << 62
        ^ on.iter().fold(7u64, |k, &o| k.wrapping_mul(3).wrapping_add(o as u64)) << 20
        ^ (look as u64) << 8;
    if LAST.with(|l| l.borrow_mut().insert(n, key)) == Some(key) {
        return;
    }
    // agenda: the next 14 days that have events
    let mut agenda = Vec::new();
    for dd in 0..14 {
        let day0 = day_after(today, dd);
        let list: Vec<CalEvent> = on_day(day0).iter().filter(|e| dd > 0 || e.end > t).map(|e| event(e, &c, day0, t)).collect();
        if list.is_empty() && dd > 0 {
            continue;
        }
        agenda.push(CalDay {
            label: day_label(day0, today).into(),
            free: if list.is_empty() { texts::kept("cal.free", 0).into() } else { SharedString::new() },
            events: ModelRc::new(VecModel::from(list)),
        });
    }
    // day: today on a timeline
    let dayv = day_after(today, if view == 1 { shift } else { 0 });
    let mut timed: Vec<CalEvent> = on_day(dayv).iter().filter(|e| !e.all_day).map(|e| event(e, &c, dayv, t)).collect();
    lanes(&mut timed);
    let allday: Vec<CalEvent> = on_day(dayv).iter().filter(|e| e.all_day).map(|e| event(e, &c, dayv, t)).collect();
    let day_free: SharedString = if timed.is_empty() && allday.is_empty() { texts::kept("cal.free", 0).into() } else { SharedString::new() };
    // week: Monday to Sunday of this week; 3 days: today and the next two (back / forward three days at a time)
    let (col0, columns) = grid_days(today, weekday(t), view, shift);
    let monday = day_after(today, -weekday(t) + if view == 2 { 7 * shift } else { 0 });
    let mut week_events = Vec::new();
    let mut week_allday = Vec::new();
    let mut nums = Vec::new();
    let mut wnames = Vec::new();
    for dd in 0..columns {
        let day0 = day_after(col0, dd);
        nums.push(local_format(day0, "%-d").parse::<i32>().unwrap_or(0));
        wnames.push(SharedString::from(local_format(day0, "%a").to_uppercase()));
        let column = |mut ev: CalEvent| {
            ev.day = dd as i32;
            ev
        };
        let mut timed: Vec<CalEvent> = on_day(day0).iter().filter(|e| !e.all_day).map(|e| column(event(e, &c, day0, t))).collect();
        lanes(&mut timed);
        week_events.extend(timed);
        week_allday.extend(on_day(day0).iter().filter(|e| e.all_day).map(|e| column(event(e, &c, day0, t))));
    }
    DAYS.with(|d| d.borrow_mut().insert((n, "week"), (0..columns).map(|k| day_after(col0, k)).collect()));
    let today_column = (today - col0 + 43200).div_euclid(86400);
    // month: six weeks from the Monday on or before the first
    let months = local_format(t, "%Y").parse::<i64>().unwrap_or(2026) * 12 + local_format(t, "%m").parse::<i64>().unwrap_or(1) - 1
        + if view == 3 { shift } else { 0 };
    let first = date_epoch(&format!("{:04}-{:02}-01", months.div_euclid(12), months.rem_euclid(12) + 1)).unwrap_or(today);
    let grid0 = day_after(first, -weekday(first));
    let month = local_format(first, "%m");
    DAYS.with(|d| d.borrow_mut().insert((n, "month"), (0..42).map(|k| day_after(grid0, k)).collect()));
    let cells: Vec<CalCell> = (0..42)
        .map(|k| {
            let day0 = day_after(grid0, k);
            let evs = on_day(day0);
            CalCell {
                num: local_format(day0, "%-d").parse().unwrap_or(0),
                other_month: local_format(day0, "%m") != month,
                today: day0 == today,
                labels: ModelRc::new(VecModel::from(evs.iter().take(2).map(|e| SharedString::from(e.title.as_str())).collect::<Vec<_>>())),
                tints: ModelRc::new(VecModel::from(evs.iter().take(2).map(|e| tint(e.cal)).collect::<Vec<_>>())),
                more: evs.len().saturating_sub(2) as i32,
            }
        })
        .collect();
    // departure board: today's and the coming events, one row each (fields cut to their cells; the event field
    // gives a cell to the 12-hour time)
    let what_cells = if crate::clock::h24() { 16 } else { 15 };
    let cell = |s: &str, k: usize| -> String { s.to_uppercase().chars().filter(|c| !c.is_control()).take(k).collect() };
    let mut board: Vec<CalBoard> = Vec::new();
    let mut board_ev: Vec<(CalEvent, String)> = Vec::new();
    for dd in 0..7 {
        let day0 = day_after(today, dd);
        for e in on_day(day0) {
            if board.len() >= 6 {
                break;
            }
            if e.start < day0 && dd > 0 {
                continue;
            }
            let (status, state) = if e.end <= t {
                ("DEPARTED".to_string(), 2)
            } else if e.start <= t {
                ("BOARDING".to_string(), 1)
            } else if dd == 0 {
                ("ON TIME".to_string(), 0)
            } else {
                (local_format(day0, "%a %-d").to_uppercase(), 3)
            };
            let ev = event(e, &c, day0, t);
            board_ev.push((ev.clone(), format!("{} \u{b7} {}", day_label(day0, today), ev.time)));
            board.push(CalBoard {
                time: if e.all_day { "--:--".into() } else { board_hm(e.start).into() },
                what: cell(&e.title, what_cells).into(),
                gate: cell(&code(c.names.get(e.cal).map(String::as_str).unwrap_or("")), 2).into(),
                status: cell(&status, 8).into(),
                state,
            });
        }
    }
    ROWS.with(|r| r.borrow_mut().insert((n, "board"), board_ev));
    // tear-off: today's page
    let todays: Vec<CalEvent> = on_day(today).iter().map(|e| event(e, &c, today, t)).collect();
    let shown_n = todays.len().min(3);
    ROWS.with(|r| r.borrow_mut().insert((n, "tear"), todays.iter().take(shown_n).map(|e| (e.clone(), format!("Today \u{b7} {}", e.time))).collect()));
    let tear_more: SharedString = if todays.len() > shown_n { format!("+{} more", todays.len() - shown_n).into() } else { SharedString::new() };
    let (title, sub) = match view {
        1 => (
            match shift {
                0 => "Today".to_string(),
                1 => "Tomorrow".into(),
                -1 => "Yesterday".into(),
                _ => local_format(dayv, "%A"),
            },
            local_format(dayv, "%A, %-d %B"),
        ),
        2 => (
            match shift {
                0 => "This week".to_string(),
                1 => "Next week".into(),
                -1 => "Last week".into(),
                _ => format!("Week {}", local_format(monday, "%V").trim_start_matches('0')),
            },
            format!("{}\u{2013}{} \u{b7} week {}", local_format(monday, "%-d"), local_format(day_after(monday, 6), "%-d %B"), local_format(monday, "%V").trim_start_matches('0')),
        ),
        4 => {
            let last = day_after(col0, columns - 1);
            let span = if local_format(col0, "%m") == local_format(last, "%m") {
                format!("{}\u{2013}{}", local_format(col0, "%-d"), local_format(last, "%-d %B"))
            } else {
                format!("{} \u{2013} {}", local_format(col0, "%-d %B"), local_format(last, "%-d %B"))
            };
            (span, format!("{}\u{2013}{} \u{b7} week {}", local_format(col0, "%a"), local_format(last, "%a"), local_format(col0, "%V").trim_start_matches('0')))
        }
        3 => (local_format(first, "%B"), local_format(first, "%Y")),
        _ => ("Coming up".into(), local_format(t, "%A, %-d %B")),
    };
    crate::pages::with_inst(ui.get_cal_inst(), |m| ui.set_cal_inst(m), n, |d| {
        d.agenda = ModelRc::new(VecModel::from(agenda));
        d.day_free = day_free;
        d.day_events = ModelRc::new(VecModel::from(timed));
        d.day_allday = ModelRc::new(VecModel::from(allday));
        d.day_today = dayv == today;
        d.week_names = ModelRc::new(VecModel::from(wnames));
        d.week_nums = ModelRc::new(VecModel::from(nums));
        d.week_today = if (0..columns).contains(&today_column) { today_column as i32 } else { -1 };
        d.week_events = ModelRc::new(VecModel::from(week_events));
        d.week_allday = ModelRc::new(VecModel::from(week_allday));
        d.month_cells = ModelRc::new(VecModel::from(cells));
        d.board_rows = ModelRc::new(VecModel::from(board));
        d.tear_more = tear_more;
        d.tear_events = ModelRc::new(VecModel::from(todays.into_iter().take(shown_n).collect::<Vec<_>>()));
        d.title = title.into();
        d.sub = sub.into();
    });
}

/// The days of the plain look's time grid: the first day's midnight and how many columns. The week view shows
/// Monday to Sunday (`shift` weeks from this one), the 3-day view today and the next two (`shift` × 3 days on).
fn grid_days(today: i64, today_weekday: i64, view: i32, shift: i64) -> (i64, i64) {
    if view == 4 {
        (day_after(today, 3 * shift), 3)
    } else {
        (day_after(today, -today_weekday + if view == 2 { 7 * shift } else { 0 }), 7)
    }
}

/// The events shown on the page (the calendars left on, nothing being deleted) of the days from `day0`, `days` long.
fn days_list(s: &crate::UiState, day0: i64, days: i64, caps: bool, skip_empty: bool) -> Vec<CalDay> {
    let entities = crate::page_entities(s, "calendar");
    let c = cals(s, &entities);
    let n = crate::acting_n("calendar");
    let page = crate::pages::kind_pages(s.panel_config.as_ref(), "calendar").get(n).map(|(_, p)| (*p).clone()).unwrap_or(Value::Null);
    let on = shown(&page, &entities, n);
    let all = page_events(&entities);
    let deleting = DELETING.with(|d| d.borrow().as_ref().map(|x| x.0.clone()));
    let t = now();
    let today = midnight(t);
    let mut out = Vec::new();
    for k in 0..days {
        let d0 = day_after(day0, k);
        let d1 = day_after(d0, 1);
        let list: Vec<CalEvent> = all
            .iter()
            .filter(|e| e.start < d1 && e.end > d0 && on.get(e.cal).copied().unwrap_or(false) && deleting.as_deref() != Some(e.id().as_str()))
            .map(|e| event(e, &c, d0, t))
            .collect();
        if list.is_empty() && skip_empty {
            continue;
        }
        let label = match (d0 - today + 43200).div_euclid(86400) {
            0 => format!("Today · {}", local_format(d0, "%a %-d %b")),
            1 => format!("Tomorrow · {}", local_format(d0, "%a %-d %b")),
            _ => local_format(d0, "%A · %a %-d %b"),
        };
        out.push(CalDay {
            label: if caps { label.to_uppercase().into() } else { label.into() },
            free: texts::kept("cal.free", 0).into(),
            events: ModelRc::new(VecModel::from(list)),
        });
    }
    out
}

/// A day's events as a list in the look's menu (a tap on one opens its details over it).
fn open_day(ui: &AppWindow, s: &crate::UiState, day0: i64) {
    let today = midnight(now());
    let head = if day0 == today { format!("Today · {}", local_format(day0, "%a %-d %b")) } else { local_format(day0, "%A, %-d %B") };
    let caps = if day0 == today { format!("Today · {}", local_format(day0, "%A %-d %B")) } else { local_format(day0, "%A %-d %B") };
    ui.set_cal_day_sheet_title(head.into());
    ui.set_cal_day_sheet_caps(caps.to_uppercase().into());
    ui.set_cal_day_sheet_short(local_format(day0, "%a %-d %b").into());
    ui.set_cal_day_sheet_week(false);
    ui.set_cal_day_sheet_days(ModelRc::new(VecModel::from(days_list(s, day0, 1, false, false))));
    ui.set_cal_day_sheet_free(texts::kept("cal.free", 0).into());
    ui.set_cal_picker(false);
    ui.set_cal_day_sheet(true);
}

/// The departure board's head: the week's departures (the coming seven days that have any) on the station's
/// printed timetable.
fn open_week(ui: &AppWindow, s: &crate::UiState) {
    let today = midnight(now());
    ui.set_cal_day_sheet_title("This week".into());
    ui.set_cal_day_sheet_caps("TIMETABLE · THIS WEEK".into());
    ui.set_cal_day_sheet_short(SharedString::new());
    ui.set_cal_day_sheet_week(true);
    ui.set_cal_day_sheet_days(ModelRc::new(VecModel::from(days_list(s, today, 7, true, true))));
    ui.set_cal_day_sheet_free(texts::kept("cal.board.empty", 0).into());
    ui.set_cal_picker(false);
    ui.set_cal_day_sheet(true);
}

/// The edit form's day line ("Thu 19 Nov").
fn edit_day_text(day0: i64) -> String {
    local_format(day0, "%a %-d %b")
}

/// Edit: the details turn into the form (the same rows in every look; the times are the UI's drums).
fn open_edit(ui: &AppWindow, s: &crate::UiState, id: &str) {
    let entities = crate::page_entities(s, "calendar");
    let Some(e) = page_events(&entities).into_iter().find(|e| e.id() == id) else { return };
    let c = cals(s, &entities);
    let features = c.features.get(e.cal).copied().unwrap_or(0);
    if features & CAN_UPDATE == 0 || e.uid.is_empty() {
        return;
    }
    let day0 = midnight(e.start);
    let (start, end) = if e.all_day { (9 * 60, 10 * 60) } else { (minutes_from(day0, e.start), minutes_from(day0, e.end).max(minutes_from(day0, e.start) + 5)) };
    let name = c.names.get(e.cal).cloned().unwrap_or_default();
    ui.set_cal_edit_title(e.title.clone().into());
    ui.set_cal_edit_cal(name.clone().into());
    ui.set_cal_edit_code(code(&name).into());
    ui.set_cal_edit_tint(tint(e.cal));
    ui.set_cal_edit_day(edit_day_text(day0).into());
    ui.set_cal_edit_allday(e.all_day);
    ui.set_cal_edit_start_min(start as i32);
    ui.set_cal_edit_end_min(end as i32);
    ui.set_cal_edit_open(0);
    ui.set_cal_edit_typing(false);
    ui.set_cal_edit_can_delete(features & CAN_DELETE != 0);
    EDIT.with(|x| {
        *x.borrow_mut() = Some(Edit {
            id: e.id(),
            entity: entities.get(e.cal).cloned().unwrap_or_default(),
            uid: e.uid.clone(),
            rid: e.rid.clone(),
            title: e.title.clone(),
            day0,
            days: ((midnight(e.end) - day0 + 43200) / 86400).max(1),
            note: e.note.clone(),
            place: e.place.clone(),
        })
    });
    ui.set_cal_edit(true);
}

/// The event as the form leaves it, for `calendar/event/update`: (start, end) as Home Assistant reads them.
fn edit_times(day0: i64, days: i64, all_day: bool, start_min: i64, end_min: i64) -> (Value, Value, i64, i64) {
    if all_day {
        let end = day_after(day0, days.max(1));
        (json!(local_format(day0, "%Y-%m-%d")), json!(local_format(end, "%Y-%m-%d")), day0, end)
    } else {
        let a = at(day0, start_min);
        let b = at(day0, end_min.max(start_min + 5));
        let iso = |t: i64| local_format(t, "%Y-%m-%dT%H:%M:%S%z");
        (json!(iso(a)), json!(iso(b)), a, b)
    }
}

fn close_edit(ui: &AppWindow) {
    ui.set_cal_edit(false);
    ui.set_cal_edit_typing(false);
    ui.set_cal_edit_open(0);
}

/// Every calendar menu closed (the page left the screen, a ring, the screensaver).
pub fn close_all(ui: &AppWindow) {
    ui.set_cal_picker(false);
    ui.set_cal_settings(false);
    ui.set_cal_day_sheet(false);
    ui.set_cal_detail(false);
    close_edit(ui);
}

/// The delete runs when its undo strip has gone (5 s) and nobody took it back.
fn delete_later(ui: &AppWindow, id: String) {
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_secs(5), move || {
        let Some(ui) = weak.upgrade() else { return };
        let Some((pid, entity, uid, rid)) = DELETING.with(|d| d.borrow().clone()) else { return };
        if pid != id {
            return;
        }
        let mut msg = json!({ "type": "calendar/event/delete", "entity_id": entity, "uid": uid });
        if !rid.is_empty() {
            msg["recurrence_id"] = json!(rid);
        }
        crate::send_raw(msg);
        EVENTS.with(|e| e.borrow_mut().retain(|e| e.id() != pid));
        DELETING.with(|d| *d.borrow_mut() = None);
        REFETCH.store(true, Ordering::SeqCst);
        ui.set_cal_undo(false);
        with_state(|s| refresh(&ui, s));
    });
}

/// Taps on the page and its menus (`cal.*`).
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    match what {
        // the view (the plain look's picker): kept as the last used one, the start view after a restart
        "cal.view" => {
            let view: i32 = value.parse().unwrap_or(0).clamp(0, 4);
            let n = crate::acting_n("calendar");
            crate::pages::with_inst(ui.get_cal_inst(), |m| ui.set_cal_inst(m), n, |d| d.view = view);
            set_shift(n, 0);
            ui.set_cal_picker(false);
            ui.set_cal_settings(false);
            crate::page_set(ui, "calendar", None, json!({ "last_view": view }));
            with_state(|s| refresh(ui, s));
        }
        // a new look closes the settings and opens them again as the new look's menu once the old one has left
        "cal.look" => {
            let look: i32 = value.parse().unwrap_or(0);
            if crate::page_look("calendar") == look {
                return;
            }
            ui.set_cal_settings(false);
            crate::page_set(ui, "calendar", Some(look), Value::Null);
            with_state(|s| refresh(ui, s));
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(320), move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_cal_settings(true);
                }
            });
        }
        // the chip: its picker, and the chip again closes it
        "cal.picker" => {
            let open = !ui.get_cal_picker();
            close_all(ui);
            ui.set_cal_picker(open);
        }
        "cal.picker-close" => ui.set_cal_picker(false),
        // a calendar shown or hidden on the page (at least one stays on); the integration keeps the choice
        "cal.toggle" => {
            let k: usize = value.parse().unwrap_or(usize::MAX);
            let at = crate::acting_n("calendar");
            let hidden = with_state(|s| {
                let entities = crate::page_entities(s, "calendar");
                let page = crate::pages::kind_pages(s.panel_config.as_ref(), "calendar").get(at).map(|(_, p)| (*p).clone()).unwrap_or(Value::Null);
                let mut on = shown(&page, &entities, at);
                if k >= on.len() || (on[k] && on.iter().filter(|&&x| x).count() <= 1) {
                    return None;
                }
                on[k] = !on[k];
                let hidden: Vec<String> = entities.iter().zip(&on).filter(|(_, &o)| !o).map(|(id, _)| id.clone()).collect();
                HIDE_PENDING.with(|p| p.borrow_mut().insert(at, hidden.clone()));
                Some(hidden)
            });
            if let Some(hidden) = hidden {
                crate::page_set(ui, "calendar", None, json!({ "hidden_calendars": hidden }));
                with_state(|s| refresh(ui, s));
            }
        }
        // a tap on a day (month grid, wall calendar, a week column or a narrow event there): that day's events in a list
        "cal.month-day" | "cal.wall-day" | "cal.week-day" => {
            let key = match what {
                "cal.month-day" => "month",
                "cal.wall-day" => "wall",
                _ => "week",
            };
            let k: usize = value.parse().unwrap_or(0);
            let at = crate::acting_n("calendar");
            if let Some(day0) = DAYS.with(|d| d.borrow().get(&(at, key)).and_then(|v| v.get(k).copied())) {
                with_state(|s| open_day(ui, s, day0));
            }
        }
        // the tear-off's "+n more": today's list
        "cal.tear-more" => with_state(|s| open_day(ui, s, midnight(now()))),
        // the departure board's head: the week's departures
        "cal.board-week" => with_state(|s| open_week(ui, s)),
        "cal.day-sheet-close" => {
            ui.set_cal_day_sheet(false);
            ui.set_cal_detail(false);
            close_edit(ui);
        }
        // a departure board row or a tear-off line: its details
        "cal.board" | "cal.tear" => {
            let key = if what == "cal.board" { "board" } else { "tear" };
            let k: usize = value.parse().unwrap_or(0);
            let at = crate::acting_n("calendar");
            if let Some((ev, when)) = ROWS.with(|r| r.borrow().get(&(at, key)).and_then(|v| v.get(k).cloned())) {
                ui.set_cal_picker(false);
                ui.set_cal_detail_ev(ev);
                ui.set_cal_detail_when(when.into());
                ui.set_cal_detail(true);
            }
        }
        // the page settings (the quick shade's page row)
        "cal.settings" => {
            close_all(ui);
            ui.set_cal_settings(true);
        }
        "cal.settings-close" => ui.set_cal_settings(false),
        "cal.close-all" => close_all(ui),
        "cal.prev" | "cal.next" => {
            let at = crate::acting_n("calendar");
            set_shift(at, shift_of(at) + if what == "cal.next" { 1 } else { -1 });
            with_state(|s| refresh(ui, s));
        }
        // the page came on screen: the time grids show the period with today in it again
        "cal.shown" => {
            if set_shift(crate::acting_n("calendar"), 0) != 0 {
                with_state(|s| refresh(ui, s));
            }
        }
        "cal.detail-close" => {
            ui.set_cal_detail(false);
            close_edit(ui);
        }
        // the edit form
        "cal.edit" => with_state(|s| open_edit(ui, s, value)),
        "cal.edit-close" => {
            close_edit(ui);
            ui.set_cal_detail(false);
        }
        "cal.edit-day" => {
            let step: i64 = value.parse().unwrap_or(0);
            EDIT.with(|x| {
                if let Some(e) = x.borrow_mut().as_mut() {
                    e.day0 = day_after(e.day0, step);
                    ui.set_cal_edit_day(edit_day_text(e.day0).into());
                }
            });
        }
        "cal.title" => {
            ui.set_cal_edit_open(0);
            ui.set_cal_kb_layer(if ui.get_cal_edit_title().is_empty() { 1 } else { 0 });
            ui.set_cal_edit_typing(true);
        }
        "cal.key" => {
            let mut draft = ui.get_cal_edit_title().to_string();
            let layer = ui.get_cal_kb_layer();
            match value {
                "enter" => {
                    ui.set_cal_edit_typing(false);
                    return;
                }
                "back" => {
                    draft.pop();
                    if draft.is_empty() && layer == 0 {
                        ui.set_cal_kb_layer(1);
                    }
                }
                "clear" => draft.clear(),
                "shift" => ui.set_cal_kb_layer(match layer {
                    0 => 1,
                    1 => 0,
                    2 => 3,
                    _ => 2,
                }),
                "symbols" => ui.set_cal_kb_layer(if layer >= 2 { 0 } else { 2 }),
                "layout" => ui.set_kb_layout(1 - ui.get_kb_layout().clamp(0, 1)),
                k => {
                    if draft.chars().count() < 120 {
                        draft.push_str(k);
                    }
                    if layer == 1 {
                        ui.set_cal_kb_layer(0);
                    }
                }
            }
            ui.set_cal_edit_title(draft.into());
        }
        // Save: written to the calendar in Home Assistant, shown at once, read again right after
        "cal.save" => {
            let Some(e) = EDIT.with(|x| x.borrow_mut().take()) else { return };
            let title = ui.get_cal_edit_title().trim().to_string();
            let title = if title.is_empty() { e.title.clone() } else { title };
            let all_day = ui.get_cal_edit_allday();
            let (dtstart, dtend, a, b) =
                edit_times(e.day0, e.days, all_day, ui.get_cal_edit_start_min() as i64, ui.get_cal_edit_end_min() as i64);
            let mut event = json!({ "summary": title, "dtstart": dtstart, "dtend": dtend });
            if !e.note.is_empty() {
                event["description"] = json!(e.note);
            }
            if !e.place.is_empty() {
                event["location"] = json!(e.place);
            }
            let mut msg = json!({ "type": "calendar/event/update", "entity_id": e.entity, "uid": e.uid, "event": event });
            if !e.rid.is_empty() {
                msg["recurrence_id"] = json!(e.rid);
            }
            crate::send_raw(msg);
            EVENTS.with(|ev| {
                let mut ev = ev.borrow_mut();
                if let Some(x) = ev.iter_mut().find(|x| x.id() == e.id) {
                    x.title = title.clone();
                    x.start = a;
                    x.end = b;
                    x.all_day = all_day;
                }
                ev.sort_by_key(|e| (e.start, !e.all_day));
            });
            REFETCH.store(true, Ordering::SeqCst);
            close_edit(ui);
            ui.set_cal_detail(false);
            with_state(|s| refresh(ui, s));
            if ui.get_cal_day_sheet() {
                ui.set_cal_day_sheet(false);
            }
        }
        // Delete: the event leaves the page at once; a short undo strip follows, the delete is sent when it goes
        "cal.delete" => {
            let Some(e) = EDIT.with(|x| x.borrow_mut().take()) else { return };
            // an earlier delete still waiting goes now
            if let Some((_, entity, uid, rid)) = DELETING.with(|d| d.borrow_mut().take()) {
                let mut msg = json!({ "type": "calendar/event/delete", "entity_id": entity, "uid": uid });
                if !rid.is_empty() {
                    msg["recurrence_id"] = json!(rid);
                }
                crate::send_raw(msg);
                REFETCH.store(true, Ordering::SeqCst);
            }
            DELETING.with(|d| *d.borrow_mut() = Some((e.id.clone(), e.entity.clone(), e.uid.clone(), e.rid.clone())));
            close_edit(ui);
            ui.set_cal_detail(false);
            ui.set_cal_day_sheet(false);
            ui.set_cal_undo_text(format!("“{}” deleted", short(&e.title)).into());
            ui.set_cal_undo(true);
            with_state(|s| refresh(ui, s));
            delete_later(ui, e.id);
        }
        "cal.undo" => {
            DELETING.with(|d| *d.borrow_mut() = None);
            ui.set_cal_undo(false);
            with_state(|s| refresh(ui, s));
        }
        _ => eprintln!("nspanel-app: act {what} {value} (not handled)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(entity: &str, title: &str, start: i64) -> Ev {
        Ev { title: title.into(), start, end: start + 3600, all_day: false, cal: 99, entity: entity.into(), note: String::new(), place: String::new(),
            uid: format!("{title}-uid"), rid: String::new() }
    }

    #[test]
    fn each_calendar_page_shows_its_own_calendars_in_its_own_colours() {
        EVENTS.with(|e| *e.borrow_mut() = vec![ev("calendar.work", "Standup", 100), ev("calendar.home", "Dinner", 200), ev("calendar.club", "Match", 300)]);
        // a page with the club and the home calendar: their events, coloured by their place on that page
        let page = page_events(&["calendar.club".to_string(), "calendar.home".to_string()]);
        let seen: Vec<(&str, usize)> = page.iter().map(|e| (e.title.as_str(), e.cal)).collect();
        assert_eq!(seen, [("Dinner", 1), ("Match", 0)]);
        // another page with the work calendar only
        let other = page_events(&["calendar.work".to_string()]);
        assert_eq!(other.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Standup"]);
        // an event keeps its id on every page (an edit or a delete finds it from any of them)
        let both = page_events(&["calendar.home".to_string()]);
        assert_eq!(both[0].id(), page[0].id());
        EVENTS.with(|e| e.borrow_mut().clear());
    }

    #[test]
    fn each_calendar_page_keeps_its_own_period() {
        set_shift(0, 0);
        set_shift(1, 2);
        assert_eq!(shift_of(0), 0);
        assert_eq!(shift_of(1), 2);
        assert_eq!(set_shift(1, 0), 2);
        assert_eq!(shift_of(7), 0);
    }

    #[test]
    fn ranges_share_one_am_pm() {
        assert_eq!(join_range("09:30", "11:00"), "09:30–11:00");
        assert_eq!(join_range("9:30 AM", "11:00 AM"), "9:30–11:00 AM");
        assert_eq!(join_range("11:30 AM", "1:00 PM"), "11:30 AM–1:00 PM");
        assert_eq!(join_range("6:00 PM", "8:15 PM"), "6:00–8:15 PM");
    }

    #[test]
    fn board_times_fit_the_cells() {
        assert_eq!(board_text("18:45"), "18:45");
        assert_eq!(board_text("6:45 PM"), "6:45P");
        assert_eq!(board_text("10:05 AM"), "10:05A");
    }

    #[test]
    fn gate_codes() {
        assert_eq!(code("Work"), "WO");
        assert_eq!(code("a"), "A");
        assert_eq!(code("Ünnepek"), "ÜN");
        assert_eq!(code("-- x"), "X");
    }

    #[test]
    fn at_least_one_calendar_stays() {
        let ids: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(shown_of(&ids, &["b".into()]), vec![true, false, true]);
        assert_eq!(shown_of(&ids, &ids.clone()), vec![true, true, true]);
        assert_eq!(shown_of(&ids, &[]), vec![true, true, true]);
    }

    #[test]
    fn flap_cells_pad() {
        let v: Vec<String> = flap_cells("Work", 6).iter().map(|x| x.to_string()).collect();
        assert_eq!(v, ["W", "O", "R", "K", " ", " "]);
        let t: Vec<String> = flap_cells("6:45P", 6).iter().map(|x| x.to_string()).collect();
        assert_eq!(t, [" ", "6", ":", "4", "5", "P"]);
    }

    #[test]
    fn grid_periods() {
        // the 3-day view starts today and steps three days; the week view starts on Monday and steps weeks
        let today = midnight(now());
        let wd = weekday(today);
        assert_eq!(grid_days(today, wd, 4, 0), (today, 3));
        assert_eq!(grid_days(today, wd, 4, 1), (day_after(today, 3), 3));
        assert_eq!(grid_days(today, wd, 4, -2), (day_after(today, -6), 3));
        assert_eq!(grid_days(today, wd, 2, 0), (day_after(today, -wd), 7));
        assert_eq!(grid_days(today, wd, 2, 1), (day_after(today, 7 - wd), 7));
        assert_eq!(weekday(grid_days(today, wd, 2, -1).0), 0);
    }

    #[test]
    fn descriptions_read_plainly() {
        assert_eq!(readable("Hello<br>world", 100), "Hello\nworld");
        assert_eq!(readable("<p>One</p><p>Two</p>", 100), "One\n\nTwo");
        assert_eq!(readable("<ul><li>a</li><li>b</li></ul>", 100), "• a\n• b");
        assert_eq!(readable("<b>bold</b> &amp; <i>more</i> &lt;3 &#233;&#x41;", 100), "bold & more <3 éA");
        assert_eq!(readable("x < y and a&b", 100), "x < y and a&b");
        // a link target in angle brackets goes, the words round it stay apart
        assert_eq!(readable("Join the meeting<https://example.org/l/meetup-join/19%3ameeting_abc>now", 100), "Join the meeting now");
        assert_eq!(readable("Write <mailto:someone@example.org>", 100), "Write");
        // a long bare link shows its host; a short one stays
        let long = format!("https://eu.safelinks.example.com/?url={}", "x%2F".repeat(150));
        assert_eq!(readable(&format!("see {long} there"), 100), "see eu.safelinks.example.com/… there");
        assert_eq!(readable("https://meet.example.com/abc-def", 100), "https://meet.example.com/abc-def");
        // separator lines and blank runs
        assert_eq!(readable("Top\n-::~:~::~:~:~::-\n\n\n\nBottom\n\n", 100), "Top\n\nBottom");
        assert_eq!(readable("a\r\n\tb", 100), "a\nb");
        // past the cap: cut at a space, "…"
        assert_eq!(readable("one two three four", 9), "one two…");
        assert_eq!(readable("", 9), "");
    }

    #[test]
    fn chip_names_the_shown() {
        let names: Vec<String> = ["Family", "Work", "School", "Films"].iter().map(|s| s.to_string()).collect();
        assert_eq!(chip_text(&names, &[true, true, true, true]), "4 calendars");
        assert_eq!(chip_text(&names, &[true, false, true, true]), "3 calendars");
        assert_eq!(chip_text(&names, &[false, true, false, false]), "Work");
    }
}
