//! Calendar page: the calendar entities picked for it, their events from Home
//! Assistant (REST `/api/calendars`, read in a thread every five minutes and
//! when the page's calendars change), laid out as agenda, day, three days,
//! week or month.

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use crate::pages::{local_format, now, parse_iso};
use crate::draw::{self, Ink, Job};
use crate::{texts, with_state, AppWindow, CalBoard, CalCell, CalDay, CalEvent, WallCell};

/// One event as read from Home Assistant.
#[derive(Clone, Debug)]
struct Ev {
    title: String,
    /// Epoch seconds; all-day events from local midnight to local midnight.
    start: i64,
    end: i64,
    all_day: bool,
    cal: usize,
    note: String,
    place: String,
}

/// Calendar colours by order (HA has none; the integration may set them later).
const TINTS: [u32; 6] = [0x3d9bff, 0xff7a3d, 0x2fd27a, 0xc86bfa, 0xffb21e, 0xff4fa0];

fn tint(i: usize) -> Color {
    let c = TINTS[i % TINTS.len()];
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

thread_local! {
    static EVENTS: std::cell::RefCell<Vec<Ev>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The days behind the month grid's and the wall calendar's cells and the week's columns (a tap opens that day's list).
    static DAYS: std::cell::RefCell<std::collections::HashMap<&'static str, Vec<i64>>> = std::cell::RefCell::new(std::collections::HashMap::new());
    /// The events behind the departure board's rows and the tear-off page's lines, with their "when" line (a tap opens them).
    static ROWS: std::cell::RefCell<std::collections::HashMap<&'static str, Vec<(CalEvent, String)>>> = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Back / forward from today in the view's steps.
    static SHIFT: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
    /// The start view last applied from the integration's settings.
    static START: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// What the fetch thread reads (the calendars change with the integration).
static WANTED: Mutex<Vec<String>> = Mutex::new(Vec::new());
static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let (Some(url), Some(auth)) = (s.config.as_ref().map(|c| c.ha_url.clone()), s.auth.clone()) else {
        STARTED.store(false, std::sync::atomic::Ordering::SeqCst);
        return;
    };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let mut last: Vec<String> = Vec::new();
        let mut next = std::time::Instant::now();
        loop {
            let wanted = WANTED.lock().unwrap().clone();
            if wanted != last || std::time::Instant::now() >= next {
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
            out.push(Ev { title: text("summary"), start, end, all_day, cal: i, note: text("description"), place: text("location") });
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

fn hm(t: i64) -> String {
    crate::clock::local_hm(t)
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

/// An event for the views; `day0` is the midnight of the day it is shown on.
fn event(e: &Ev, names: &[String], day0: i64, t: i64) -> CalEvent {
    let start_min = if e.all_day { 0 } else { ((e.start - day0) / 60).clamp(0, 24 * 60) as i32 };
    let end_min = if e.all_day { 24 * 60 } else { ((e.end - day0) / 60).clamp(0, 24 * 60) as i32 };
    let now = !e.all_day && e.start <= t && t < e.end;
    CalEvent {
        title: e.title.clone().into(),
        time: if e.all_day { "All day".into() } else { format!("{}–{}", hm(e.start), hm(e.end)).into() },
        t1: if e.all_day { "All day".into() } else { hm(e.start).into() },
        t2: if e.all_day { SharedString::new() } else { hm(e.end).into() },
        cal: names.get(e.cal).cloned().unwrap_or_default().into(),
        tint: tint(e.cal),
        all_day: e.all_day,
        start: start_min,
        end: end_min.max(start_min + 15),
        lane: 0,
        lanes: 1,
        day: weekday(day0) as i32,
        past: e.end <= t,
        now,
        progress: if now { ((t - e.start) as f32 / (e.end - e.start).max(1) as f32).clamp(0.0, 1.0) } else { 0.0 },
        rel: if e.all_day { SharedString::new() } else { rel(e.start, e.end, t).into() },
        note: e.note.clone().into(),
        place: e.place.clone().into(),
        editable: false,
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

/// The calendar page from the events read (only when the panel has one).
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
/// calendar, today circled in red).
fn wall(ui: &AppWindow, events: &[Ev], t: i64, written: Option<&[usize]>) {
    let today = midnight(t);
    let first = date_epoch(&local_format(t, "%Y-%m-01")).unwrap_or(today);
    let grid0 = day_after(first, -weekday(first));
    let month = local_format(t, "%m");
    let inks = [Ink::Blue, Ink::Purple, Ink::Red, Ink::Green, Ink::Black];
    DAYS.with(|d| d.borrow_mut().insert("wall", (0..42).map(|k| day_after(grid0, k)).collect()));
    let rows: Vec<WallCell> = (0..42)
        .map(|k| {
            let day0 = day_after(grid0, k);
            let day1 = day_after(day0, 1);
            let notes: Vec<(String, Ink)> =
                events
                    .iter()
                    .filter(|e| e.start < day1 && e.end > day0 && written.map_or(true, |w| w.contains(&e.cal)))
                    .map(|e| (short(&e.title), inks[e.cal % inks.len()]))
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
        .collect();
    if let Some(m) = crate::pages::update_rows(&ui.get_cal_wall_cells(), rows) {
        ui.set_cal_wall_cells(m);
    }
    ui.set_cal_wall_month(local_format(t, "%B").to_uppercase().into());
    ui.set_cal_wall_year(local_format(t, "%Y").into());
    if ui.get_cal_wall_photo().size().width == 0 {
        ui.set_cal_wall_photo(crate::quick::picture("cal/wall-photo.jpg"));
    }
}

pub fn refresh(ui: &AppWindow, s: &mut crate::UiState) {
    let pages = ui.get_pages();
    let Some(slot) =
        (0..slint::Model::row_count(&pages)).filter_map(|i| slint::Model::row_data(&pages, i)).find(|p| p.kind == "calendar")
    else {
        return;
    };
    let entities = crate::page_entities(s, "calendar");
    *WANTED.lock().unwrap() = entities.clone();
    if entities.is_empty() {
        return;
    }
    // the top-right chip names what the page shows: its calendar, or how many
    let chip = match entities.as_slice() {
        [one] => s.states.get(one).and_then(|e| e.attrs.get("friendly_name")).and_then(Value::as_str).unwrap_or(one).to_string(),
        many => format!("{} calendars", many.len()),
    };
    if ui.get_cal_chip().as_str() != chip {
        ui.set_cal_chip(chip.into());
    }
    ensure_started(ui, s);
    ui.set_cal_look(slot.variant);
    let names: Vec<String> = entities
        .iter()
        .map(|id| s.states.get(id).and_then(|e| e.attrs.get("friendly_name")).and_then(Value::as_str).unwrap_or(id).to_string())
        .collect();
    let t = now();
    let today = midnight(t);
    let events = EVENTS.with(|e| e.borrow().clone());
    let on_day = |day0: i64| -> Vec<&Ev> {
        let day1 = day_after(day0, 1);
        events.iter().filter(|e| e.start < day1 && e.end > day0).collect()
    };
    // the start view: set in the integration, else the last one used (kept there too); applied when it changes
    let start = crate::page_option(s, "calendar", "view")
        .and_then(|v| v.as_i64())
        .filter(|v| (0..=4).contains(v))
        .or_else(|| crate::page_option(s, "calendar", "last_view").and_then(|v| v.as_i64()).filter(|v| (0..=4).contains(v)))
        .unwrap_or(0);
    if START.with(|v| v.replace(Some(start))) != Some(start) {
        ui.set_cal_view(start as i32);
        SHIFT.with(|o| o.set(0));
    }
    let view = ui.get_cal_view();
    // back / forward: days in the day view, weeks in the week and 3-day views, months in the month view
    let shift = SHIFT.with(|o| o.get());
    // the wall calendar's pen notes come in one by one: its cells follow them
    if crate::page_looks(ui, "calendar").contains(&3) {
        // the calendars written on the wall (daily routines would fill every cell): set in the integration, else all
        let written: Option<Vec<usize>> = crate::page_option(s, "calendar", "wall_calendars")
            .and_then(|v| v.as_array().cloned())
            .map(|a| a.iter().filter_map(Value::as_str).filter_map(|id| entities.iter().position(|e| e == id)).collect());
        wall(ui, &events, t, written.as_deref());
    }
    // New models rebuild the page: only when the events, the minute or the view changed.
    thread_local!(static LAST: std::cell::Cell<u64> = const { std::cell::Cell::new(0) });
    let key = events.iter().fold((t / 60) as u64 ^ ((view as u64) << 56) ^ ((shift as u64) << 40) ^ ((crate::clock::h24() as u64) << 63), |k, e| {
        k.wrapping_mul(31).wrapping_add(e.start as u64 ^ (e.end as u64).rotate_left(17) ^ e.title.len() as u64)
    }) ^ names.iter().fold(0u64, |k, n| k.wrapping_mul(131).wrapping_add(n.len() as u64 ^ n.bytes().map(u64::from).sum::<u64>()));
    if LAST.with(|l| l.replace(key)) == key {
        return;
    }
    // agenda: the next 14 days that have events
    let mut agenda = Vec::new();
    for d in 0..14 {
        let day0 = day_after(today, d);
        let list: Vec<CalEvent> = on_day(day0).iter().filter(|e| d > 0 || e.end > t).map(|e| event(e, &names, day0, t)).collect();
        if list.is_empty() && d > 0 {
            continue;
        }
        agenda.push(CalDay {
            label: day_label(day0, today).into(),
            free: if list.is_empty() { texts::kept("cal.free", 0).into() } else { SharedString::new() },
            events: ModelRc::new(VecModel::from(list)),
        });
    }
    if agenda.len() <= 1 && agenda.first().is_some_and(|d| slint::Model::row_count(&d.events) == 0) {
        ui.set_cal_empty_title(texts::kept("cal.empty.titles", 0).into());
        ui.set_cal_empty_sub(texts::kept("cal.empty.subs", 0).into());
    }
    ui.set_cal_agenda(ModelRc::new(VecModel::from(agenda)));
    // day: today on a timeline
    let dayv = day_after(today, if view == 1 { shift } else { 0 });
    let mut timed: Vec<CalEvent> = on_day(dayv).iter().filter(|e| !e.all_day).map(|e| event(e, &names, dayv, t)).collect();
    lanes(&mut timed);
    let allday: Vec<CalEvent> = on_day(dayv).iter().filter(|e| e.all_day).map(|e| event(e, &names, dayv, t)).collect();
    ui.set_cal_day_free(if timed.is_empty() && allday.is_empty() { texts::kept("cal.free", 0).into() } else { SharedString::new() });
    ui.set_cal_day_events(ModelRc::new(VecModel::from(timed)));
    ui.set_cal_day_allday(ModelRc::new(VecModel::from(allday)));
    ui.set_cal_day_today(dayv == today);
    ui.set_cal_now_min(((t - today) / 60) as i32);
    // week: Monday to Sunday of this week
    let monday = day_after(today, -weekday(t) + if view == 2 || view == 4 { 7 * shift } else { 0 });
    let mut week_events = Vec::new();
    let mut week_allday = Vec::new();
    let mut nums = Vec::new();
    let mut wnames = Vec::new();
    for d in 0..7 {
        let day0 = day_after(monday, d);
        nums.push(local_format(day0, "%-d").parse::<i32>().unwrap_or(0));
        wnames.push(SharedString::from(local_format(day0, "%a").to_uppercase()));
        let mut timed: Vec<CalEvent> = on_day(day0).iter().filter(|e| !e.all_day).map(|e| event(e, &names, day0, t)).collect();
        lanes(&mut timed);
        week_events.extend(timed);
        week_allday.extend(on_day(day0).iter().filter(|e| e.all_day).map(|e| event(e, &names, day0, t)));
    }
    DAYS.with(|d| d.borrow_mut().insert("week", (0..7).map(|k| day_after(monday, k)).collect()));
    ui.set_cal_week_names(ModelRc::new(VecModel::from(wnames)));
    ui.set_cal_week_nums(ModelRc::new(VecModel::from(nums)));
    ui.set_cal_week_today(if view == 2 || view == 4 { if shift == 0 { weekday(t) as i32 } else { -1 } } else { weekday(t) as i32 });
    ui.set_cal_week_events(ModelRc::new(VecModel::from(week_events)));
    ui.set_cal_week_allday(ModelRc::new(VecModel::from(week_allday)));
    // month: six weeks from the Monday on or before the first
    let months = local_format(t, "%Y").parse::<i64>().unwrap_or(2026) * 12 + local_format(t, "%m").parse::<i64>().unwrap_or(1) - 1
        + if view == 3 { shift } else { 0 };
    let first = date_epoch(&format!("{:04}-{:02}-01", months.div_euclid(12), months.rem_euclid(12) + 1)).unwrap_or(today);
    let grid0 = day_after(first, -weekday(first));
    let month = local_format(first, "%m");
    DAYS.with(|d| d.borrow_mut().insert("month", (0..42).map(|k| day_after(grid0, k)).collect()));
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
    ui.set_cal_month_names(ModelRc::new(VecModel::from(
        ["M", "T", "W", "T", "F", "S", "S"].iter().map(|d| SharedString::from(*d)).collect::<Vec<_>>(),
    )));
    ui.set_cal_month_cells(ModelRc::new(VecModel::from(cells)));
    // departure board: today's and the coming events, one row each (fields cut to their cells)
    let cell = |s: &str, n: usize| -> String {
        let up: String = s.to_uppercase().chars().filter(|c| !c.is_control()).take(n).collect();
        up
    };
    let mut board: Vec<CalBoard> = Vec::new();
    let mut board_ev: Vec<(CalEvent, String)> = Vec::new();
    for d in 0..7 {
        let day0 = day_after(today, d);
        for e in on_day(day0) {
            if board.len() >= 6 {
                break;
            }
            if e.start < day0 && d > 0 {
                continue;
            }
            let (status, state) = if e.end <= t {
                ("DEPARTED".to_string(), 2)
            } else if e.start <= t {
                ("BOARDING".to_string(), 1)
            } else if d == 0 {
                ("ON TIME".to_string(), 0)
            } else {
                (local_format(day0, "%a %-d").to_uppercase(), 3)
            };
            let ev = event(e, &names, day0, t);
            board_ev.push((ev.clone(), format!("{} · {}", day_label(day0, today), ev.time)));
            board.push(CalBoard {
                time: if e.all_day { "--:--".into() } else { hm(e.start).into() },
                what: cell(&e.title, 16).into(),
                gate: cell(names.get(e.cal).map(String::as_str).unwrap_or(""), 2).into(),
                status: cell(&status, 8).into(),
                state,
            });
        }
    }
    ROWS.with(|r| r.borrow_mut().insert("board", board_ev));
    ui.set_cal_board_rows(ModelRc::new(VecModel::from(board)));
    ui.set_cal_board_clock(hm(t).into());
    ui.set_cal_board_date(local_format(t, "%a %-d %b").to_uppercase().into());
    // the board's crawl: four lines (never shorter than its strip)
    let crawl: Vec<String> = (0..4).map(|i| texts::kept("cal.board.tickers", i)).collect();
    ui.set_cal_board_ticker(crawl.join("   \u{2022}   ").into());
    ui.set_cal_board_empty(texts::kept("cal.board.empty", 0).into());
    // tear-off: today's page
    let todays: Vec<CalEvent> = on_day(today).iter().map(|e| event(e, &names, today, t)).collect();
    let shown = todays.len().min(3);
    ROWS.with(|r| r.borrow_mut().insert("tear", todays.iter().take(shown).map(|e| (e.clone(), format!("Today · {}", e.time))).collect()));
    ui.set_cal_tear_month(local_format(t, "%B %Y").to_uppercase().into());
    ui.set_cal_tear_day(local_format(t, "%-d").parse().unwrap_or(1));
    ui.set_cal_tear_weekday(local_format(t, "%A").to_uppercase().into());
    ui.set_cal_tear_sunday(weekday(t) == 6);
    ui.set_cal_tear_info(format!("week {} · day {}", local_format(t, "%V").trim_start_matches('0'), local_format(t, "%j").trim_start_matches('0')).into());
    ui.set_cal_tear_more(if todays.len() > shown { format!("+{} more", todays.len() - shown).into() } else { SharedString::new() });
    ui.set_cal_tear_events(ModelRc::new(VecModel::from(todays.into_iter().take(shown).collect::<Vec<_>>())));
    ui.set_cal_tear_line(texts::kept("cal.tear.lines", 0).into());
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
        2 | 4 => (
            match shift {
                0 => "This week".to_string(),
                1 => "Next week".into(),
                -1 => "Last week".into(),
                _ => format!("Week {}", local_format(monday, "%V").trim_start_matches('0')),
            },
            format!("{}–{} · week {}", local_format(monday, "%-d"), local_format(day_after(monday, 6), "%-d %B"), local_format(monday, "%V").trim_start_matches('0')),
        ),
        3 => (local_format(first, "%B"), local_format(first, "%Y")),
        _ => ("Coming up".into(), local_format(t, "%A, %-d %B")),
    };
    ui.set_cal_title(title.into());
    ui.set_cal_sub(sub.into());
}

/// A day's events as a list over the page (a tap on one opens its details).
fn open_day(ui: &AppWindow, s: &crate::UiState, day0: i64) {
    let entities = crate::page_entities(s, "calendar");
    let names: Vec<String> = entities
        .iter()
        .map(|id| s.states.get(id).and_then(|e| e.attrs.get("friendly_name")).and_then(Value::as_str).unwrap_or(id).to_string())
        .collect();
    let day1 = day_after(day0, 1);
    let t = now();
    let list: Vec<CalEvent> =
        EVENTS.with(|e| e.borrow().iter().filter(|e| e.start < day1 && e.end > day0).map(|e| event(e, &names, day0, t)).collect());
    ui.set_cal_day_sheet_title(local_format(day0, "%A, %-d %B").into());
    ui.set_cal_day_sheet_events(ModelRc::new(VecModel::from(list)));
    ui.set_cal_day_sheet_free(texts::kept("cal.free", 0).into());
    ui.set_cal_day_sheet(true);
}

/// Taps on the page (`cal.*`).
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    match what {
        // the view (sheet chips): kept as the last used one, the start view unless the integration sets one
        "cal.view" => {
            let view: i32 = value.parse().unwrap_or(0).clamp(0, 4);
            ui.set_cal_view(view);
            SHIFT.with(|o| o.set(0));
            ui.set_cal_settings(false);
            crate::page_set(ui, "calendar", None, serde_json::json!({ "last_view": view }));
            with_state(|s| refresh(ui, s));
        }
        "cal.look" => {
            ui.set_cal_settings(false);
            crate::page_set(ui, "calendar", Some(value.parse().unwrap_or(0)), Value::Null);
            with_state(|s| refresh(ui, s));
        }
        // a tap on a day (month grid, wall calendar, a week column or a narrow event there): that day's events in a list
        "cal.month-day" | "cal.wall-day" | "cal.week-day" => {
            let key = match what {
                "cal.month-day" => "month",
                "cal.wall-day" => "wall",
                _ => "week",
            };
            let n: usize = value.parse().unwrap_or(0);
            if let Some(day0) = DAYS.with(|d| d.borrow().get(key).and_then(|v| v.get(n).copied())) {
                with_state(|s| open_day(ui, s, day0));
            }
        }
        "cal.day-sheet-close" => ui.set_cal_day_sheet(false),
        // a departure board row or a tear-off line: its details
        "cal.board" | "cal.tear" => {
            let key = if what == "cal.board" { "board" } else { "tear" };
            let n: usize = value.parse().unwrap_or(0);
            if let Some((ev, when)) = ROWS.with(|r| r.borrow().get(key).and_then(|v| v.get(n).cloned())) {
                ui.set_cal_detail_ev(ev);
                ui.set_cal_detail_when(when.into());
                ui.set_cal_detail(true);
            }
        }
        "cal.settings" => ui.set_cal_settings(true),
        "cal.settings-close" => ui.set_cal_settings(false),
        "cal.prev" | "cal.next" => {
            SHIFT.with(|o| o.set(o.get() + if what == "cal.next" { 1 } else { -1 }));
            with_state(|s| refresh(ui, s));
        }
        "cal.detail-close" => ui.set_cal_detail(false),
        _ => eprintln!("nspanel-app: act {what} {value} (not handled)"),
    }
}
