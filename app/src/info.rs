//! Notifications page: Home Assistant's persistent notifications (followed
//! live) and PanelOS's own (from the integration's notify entity and action,
//! with an icon, a level, a picture and buttons), unread and newest first; a
//! tap opens one (and marks it read), its card dismisses it, "Clear all"
//! dismisses them all. The fridge look's notes are drawn by the app; the
//! pager's buttons scroll and open. New ones pop up in the window (spec 30),
//! several in one window ("i of n"). The page's settings menu (39) sets the
//! look, the time format and the pager's finish (the integration's page option).
//! The active status screens and the critical alert stand above the
//! notifications in every look (`alerts.rs` keeps their rows); "Clear all" and
//! Dismiss leave them, a tap on a status screen opens it again.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::draw::{self, Job};
use crate::pages::{now, parse_iso};
use crate::{icons, texts, with_state, AppWindow, InfoNote};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Note {
    title: String,
    message: String,
    created: i64,
    unread: bool,
    /// PanelOS's own (the integration keeps it until it is answered): its id there, after "po:"
    own: bool,
    icon: String,
    level: i32,
    buttons: Vec<String>,
    picture: String,
    until: bool,
}

thread_local! {
    /// The pager picture's finish now shown.
    static PAGER_FINISH: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// The pager's finishes, in the order of its menu and the integration's swatches.
const FINISHES: [&str; 4] = ["graphite", "purple", "clear", "offwhite"];

/// The clock's time as minutes of the day for the pager's and the TV news clock ("14", "28" → 868); -1 before the
/// clock is set ("--").
pub fn minute_of(hh: &str, mm: &str) -> i32 {
    match (hh.parse::<i32>(), mm.parse::<i32>()) {
        (Ok(h), Ok(m)) if (0..24).contains(&h) && (0..60).contains(&m) => h * 60 + m,
        _ => -1,
    }
}

/// A finish's place in the menu (an unknown one counts as graphite).
fn finish_index(finish: &str) -> i32 {
    FINISHES.iter().position(|f| *f == finish).unwrap_or(0) as i32
}

thread_local! {
    static NOTES: std::cell::RefCell<HashMap<String, Note>> = std::cell::RefCell::new(HashMap::new());
    /// The note open (its id), the pager's first list row.
    static OPEN: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    static FIRST: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The new-notification window: the notifications it holds (ids, in order) and the one shown.
    static WIN: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    static WIN_AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The pictures of PanelOS notifications (a camera's snapshot, a picture's address), by note id.
    static PICTURES: std::cell::RefCell<HashMap<String, slint::Image>> = std::cell::RefCell::new(HashMap::new());
}

/// A notification's content key (title and text): the same notification made again keeps its first time.
fn seen_key(title: &str, message: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (title, message).hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The first time each notification was seen: kept while it is there and a day after it went (a restart of Home
/// Assistant makes some of them again with a new time).
fn first_seen(notes: &HashMap<String, Note>) -> HashMap<String, i64> {
    thread_local!(static SEEN: std::cell::RefCell<Option<serde_json::Map<String, Value>>> = const { std::cell::RefCell::new(None) });
    SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        let map = seen.get_or_insert_with(|| crate::settings::load_notes_seen().as_object().cloned().unwrap_or_default());
        let t = now();
        let mut changed = false;
        let mut out = HashMap::new();
        for (id, note) in notes {
            let key = seen_key(&note.title, &note.message);
            let entry = map.entry(key).or_insert_with(|| {
                changed = true;
                serde_json::json!({ "first": note.created, "last": t })
            });
            let first = entry["first"].as_i64().unwrap_or(note.created).min(note.created);
            if entry["last"].as_i64().unwrap_or(0) + 600 < t {
                changed = true;
            }
            *entry = serde_json::json!({ "first": first, "last": t });
            out.insert(id.clone(), first);
        }
        // forgotten a day after the notification went
        let before = map.len();
        map.retain(|_, v| v["last"].as_i64().unwrap_or(0) + 86_400 > t);
        if changed || map.len() != before {
            crate::settings::save_notes_seen(&Value::Object(map.clone()));
        }
        out
    })
}

/// A change of Home Assistant's notifications.
pub fn update(kind: &str, notes: &Value) {
    let Some(map) = notes.as_object() else { return };
    NOTES.with(|n| {
        let mut n = n.borrow_mut();
        if kind == "current" {
            n.retain(|_, v| v.own);
        }
        for (id, v) in map {
            if kind == "removed" {
                n.remove(id);
                continue;
            }
            let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let unread = n.get(id).map(|old| old.unread).unwrap_or(true);
            n.insert(
                id.clone(),
                Note {
                    title: text("title"),
                    message: text("message"),
                    created: parse_iso(&text("created_at")).unwrap_or(now()),
                    unread,
                    ..Default::default()
                },
            );
        }
        // the time each one first came (not Home Assistant's new time after its restart)
        let first = first_seen(&n);
        for (id, note) in n.iter_mut().filter(|(_, v)| !v.own) {
            if let Some(t) = first.get(id) {
                note.created = *t;
            }
        }
    });
}

/// PanelOS's own notifications for this panel (the integration's list, newest first); returns the new ones to pop up.
pub fn update_own(ui: &AppWindow, v: &Value) -> Vec<String> {
    let list = v["notes"].as_array().cloned().unwrap_or_default();
    let mut wanted: Vec<String> = Vec::new();
    NOTES.with(|n| {
        let mut n = n.borrow_mut();
        n.retain(|_, note| !note.own);
        for item in &list {
            let Some(id) = item["id"].as_str() else { continue };
            let text = |k: &str| item[k].as_str().unwrap_or("").to_string();
            let picture = text("picture");
            let key = format!("po:{id}");
            if !picture.is_empty() {
                wanted.push(format!("{key}\n{picture}"));
            }
            n.insert(
                key,
                Note {
                    title: text("title"),
                    message: text("message"),
                    created: item["created"].as_i64().unwrap_or_else(now),
                    unread: !item["read"].as_bool().unwrap_or(false),
                    own: true,
                    icon: text("icon"),
                    level: i32::from(item["level"].as_str() == Some("warning")),
                    buttons: item["buttons"].as_array().map(|b| b.iter().filter_map(|x| x.as_str().map(str::to_string)).take(3).collect()).unwrap_or_default(),
                    picture,
                    until: item["until_answered"].as_bool().unwrap_or(false),
                },
            );
        }
    });
    // their pictures (fetched once)
    for w in wanted {
        let (key, picture) = w.split_once('\n').unwrap_or_default();
        if !PICTURES.with(|p| p.borrow().contains_key(key)) {
            fetch_picture(ui, key.to_string(), picture.to_string());
        }
    }
    v["new"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|id| format!("po:{id}"))).collect()).unwrap_or_default()
}

/// A notification's title and text (the strip of the "strip" setting shows them).
pub fn note_text(id: &str) -> (String, String) {
    NOTES.with(|n| n.borrow().get(id).map(|n| (n.title.clone(), n.message.clone())).unwrap_or_default())
}

/// A notification's picture: a camera's snapshot (through Home Assistant) or a picture's address (JPEG).
fn fetch_picture(ui: &AppWindow, key: String, picture: String) {
    let Some((ha_url, auth)) = with_state(|s| s.config.as_ref().map(|c| c.ha_url.clone()).zip(s.auth.clone())) else { return };
    let weak = slint::ComponentHandle::as_weak(ui);
    std::thread::spawn(move || {
        let ha = picture.starts_with("camera.") || picture.starts_with('/');
        let url = if picture.starts_with("camera.") {
            format!("{ha_url}/api/camera_proxy/{picture}?width=416&height=234")
        } else if picture.starts_with('/') {
            format!("{ha_url}{picture}")
        } else {
            picture.clone()
        };
        let mut req = ureq::get(&url).timeout(std::time::Duration::from_secs(10));
        if ha {
            match auth.access_token() {
                Ok(t) => req = req.set("Authorization", &format!("Bearer {t}")),
                Err(_) => return,
            }
        }
        let bytes = match req.call() {
            Ok(resp) => {
                let mut b = Vec::new();
                use std::io::Read;
                if resp.into_reader().take(8 * 1024 * 1024).read_to_end(&mut b).is_err() {
                    return;
                }
                b
            }
            Err(e) => {
                eprintln!("nspanel-app: notification picture {picture}: {e}");
                return;
            }
        };
        let Ok(frame) = crate::camera::decode(&bytes) else {
            eprintln!("nspanel-app: notification picture {picture}: not a JPEG");
            return;
        };
        let _ = slint::invoke_from_event_loop(move || {
            PICTURES.with(|p| p.borrow_mut().insert(key, slint::Image::from_rgb8(frame)));
            if let Some(ui) = weak.upgrade() {
                with_state(|s| refresh(&ui, s));
                show_window(&ui);
            }
        });
    });
}

/// An answer to one of PanelOS's notifications, for the integration (OK, a button, Dismiss).
fn answer(id: &str, what: &str, button: Option<usize>) {
    let Some(own) = id.strip_prefix("po:") else { return };
    with_state(|s| {
        if let Some(tx) = s.commands.as_ref() {
            let _ = tx.send(crate::ha::Command::Notification { id: own.to_string(), answer: what.to_string(), button: button.map(|b| b as i32) });
        }
    });
}

fn ago(t: i64) -> String {
    let d = (now() - t).max(0);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => crate::pages::local_format(t, "%b %-d"),
    }
}

fn ordered() -> Vec<(String, Note)> {
    let mut list: Vec<(String, Note)> = NOTES.with(|n| n.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    list.sort_by(|a, b| b.1.unread.cmp(&a.1.unread).then(b.1.created.cmp(&a.1.created)));
    list
}

fn note_row(n: &Note, look: i32, big: bool) -> InfoNote {
    note_row_with(n, look, big, None)
}

fn note_row_with(n: &Note, look: i32, big: bool, id: Option<&str>) -> InfoNote {
    let own_icon = n.icon.trim_start_matches("mdi:");
    let bell = icons::glyph(if own_icon.is_empty() { "bell-outline" } else { own_icon })
        .or_else(|| icons::glyph("bell-outline"))
        .map(String::from)
        .unwrap_or_default();
    let title = if n.title.is_empty() { "Home Assistant".to_string() } else { n.title.clone() };
    let when = ago(n.created);
    let picture = id.and_then(|id| PICTURES.with(|p| p.borrow().get(id).cloned()));
    // the fridge look: the note drawn by the app (seeded by its text, so it keeps its colour and tilt)
    let paper = if look == 1 {
        draw::get(Job::Fridge {
            title: title.clone(),
            message: n.message.clone(),
            foot: format!("Home Assistant · {when}"),
            level: n.level,
            seed: draw::seed(&format!("{title}\n{}", n.message)),
            big,
        })
    } else {
        Default::default()
    };
    InfoNote {
        title: title.into(),
        message: n.message.clone().into(),
        icon: bell.as_str().into(),
        level: n.level,
        source: "Home Assistant".into(),
        when: when.into(),
        image: picture.clone().unwrap_or_default(),
        has_image: picture.is_some(),
        actions: slint::ModelRc::new(slint::VecModel::from(actions(n).into_iter().map(slint::SharedString::from).collect::<Vec<_>>())),
        unread: n.unread,
        paper,
    }
}

fn actions(n: &Note) -> Vec<String> {
    let mut a = vec!["Dismiss".to_string()];
    if n.buttons.is_empty() {
        a.push("OK".into());
    } else {
        a.extend(n.buttons.iter().cloned());
    }
    a
}

/// A new notification in the window (the panel's "window" setting): it joins the window if it is up (with
/// "until answered" the newest comes to the front); None closes the window.
pub fn pop(ui: &AppWindow, id: Option<&str>) {
    match id {
        Some(id) => {
            let until = ui.get_pop_until();
            let open = ui.get_pop_on();
            WIN.with(|w| {
                let mut w = w.borrow_mut();
                if !open {
                    w.clear();
                    WIN_AT.with(|a| a.set(0));
                }
                if w.iter().any(|k| k == id) {
                    return;
                }
                if until && !w.is_empty() {
                    w.insert(0, id.to_string());
                    WIN_AT.with(|a| a.set(0));
                } else {
                    w.push(id.to_string());
                }
            });
            show_window(ui);
        }
        None => {
            WIN.with(|w| w.borrow_mut().clear());
            WIN_AT.with(|a| a.set(0));
            ui.set_pop_on(false);
        }
    }
}

/// The window's notifications (the ones gone elsewhere leave it) in the notifications page's look.
fn show_window(ui: &AppWindow) {
    let ids: Vec<String> = WIN.with(|w| {
        let mut w = w.borrow_mut();
        NOTES.with(|n| w.retain(|id| n.borrow().contains_key(id)));
        w.clone()
    });
    if ids.is_empty() {
        if ui.get_pop_on() {
            ui.set_pop_on(false);
        }
        return;
    }
    let at = WIN_AT.with(|a| a.get()).min(ids.len() - 1);
    WIN_AT.with(|a| a.set(at));
    let pages = ui.get_pages();
    let variant = (0..slint::Model::row_count(&pages)).filter_map(|i| slint::Model::row_data(&pages, i)).find(|p| p.kind == "info").map_or(0, |p| p.variant);
    let look = if !draw::AVAILABLE && variant == 1 { 0 } else { variant.clamp(0, 3) };
    let rows: Vec<InfoNote> = NOTES.with(|n| {
        let n = n.borrow();
        ids.iter().filter_map(|id| n.get(id).map(|note| note_row_with(note, look, true, Some(id)))).collect()
    });
    if look == 2 && ui.get_pop_pager().size().width == 0 {
        ui.set_pop_pager(crate::quick::picture("info/pager-window.png"));
        ui.set_info_pager_led(crate::quick::picture("info/pager-led.png"));
    }
    if ui.get_pop_look() != look {
        ui.set_pop_look(look);
    }
    if ui.get_pop_count() != rows.len() as i32 {
        ui.set_pop_count(rows.len() as i32);
    }
    if ui.get_pop_index() != at as i32 {
        ui.set_pop_index(at as i32);
    }
    if ui.get_pop_note() != rows[at] {
        ui.set_pop_note(rows[at].clone());
    }
    if let Some(m) = crate::pages::update_rows(&ui.get_pop_notes(), rows) {
        ui.set_pop_notes(m);
    }
    if !ui.get_pop_on() {
        ui.set_pop_on(true);
    }
}

/// The window's taps (`pop.*`): OK (marks it read, steps to the next or closes), Dismiss (gone, in Home Assistant
/// too), a PanelOS notification's own button, a step sideways, a tap beside it (as OK), the time over (closes,
/// leaves it unread).
pub fn pop_act(ui: &AppWindow, what: &str, value: &str) {
    let ids = WIN.with(|w| w.borrow().clone());
    let at = WIN_AT.with(|a| a.get());
    let Some(id) = ids.get(at).cloned() else {
        pop(ui, None);
        return;
    };
    let note = NOTES.with(|n| n.borrow().get(&id).cloned()).unwrap_or_default();
    let n: usize = value.parse().unwrap_or(0);
    eprintln!("nspanel-app: window {what} {value} ({} of {})", at + 1, ids.len());
    // the card's buttons: Dismiss, then OK or the notification's own
    let what = match what {
        "pop.action" if n == 0 => "pop.dismiss",
        "pop.action" if note.buttons.is_empty() => "pop.ok",
        "pop.action" => "pop.btn",
        "pop.close" => "pop.ok",
        w => w,
    };
    let read = |id: &str| NOTES.with(|m| {
        if let Some(note) = m.borrow_mut().get_mut(id) {
            note.unread = false;
        }
    });
    match what {
        "pop.ok" | "pop.btn" => {
            read(&id);
            if what == "pop.btn" {
                answer(&id, "button", Some(n.saturating_sub(1)));
            } else {
                answer(&id, "ok", None);
            }
            if at + 1 < ids.len() {
                WIN_AT.with(|a| a.set(at + 1));
            } else {
                close_window(ui);
                return;
            }
        }
        "pop.dismiss" => {
            if note.own {
                answer(&id, "dismiss", None);
                NOTES.with(|m| m.borrow_mut().remove(&id));
            } else {
                crate::call("persistent_notification", "dismiss", "", json!({ "notification_id": id }));
            }
            WIN.with(|w| w.borrow_mut().retain(|k| *k != id));
            if ids.len() <= 1 {
                close_window(ui);
                return;
            }
            WIN_AT.with(|a| a.set(at.min(ids.len() - 2)));
        }
        "pop.step" => {
            let next = if value.starts_with('-') { at.checked_sub(1) } else { Some(at + 1).filter(|&k| k < ids.len()) };
            if let Some(k) = next {
                WIN_AT.with(|a| a.set(k));
            }
        }
        "pop.timeout" => {
            close_window(ui);
            return;
        }
        _ => {}
    }
    with_state(|s| refresh(ui, s));
    show_window(ui);
}

fn close_window(ui: &AppWindow) {
    pop(ui, None);
    // (the window keeps its own time: a strip, a shown view or a ring keep theirs)
    with_state(|s| refresh(ui, s));
}

/// The notifications page (only when the panel has one).
pub fn refresh(ui: &AppWindow, _s: &mut crate::UiState) {
    let unread = NOTES.with(|n| n.borrow().values().filter(|n| n.unread).count()) as i32;
    if ui.get_info_unread() != unread {
        ui.set_info_unread(unread);
    }
    // the window follows the list (a notification dismissed elsewhere leaves it)
    // (and a fridge note's picture drawn after the window opened comes in)
    if ui.get_pop_on() {
        show_window(ui);
    }
    let pages = ui.get_pages();
    let Some(slot) = (0..slint::Model::row_count(&pages)).filter_map(|i| slint::Model::row_data(&pages, i)).find(|p| p.kind == "info")
    else {
        return;
    };
    // the fridge needs the notes drawn by the app (GPU build); otherwise the plain look
    let look = if !draw::AVAILABLE && slot.variant == 1 { 0 } else { slot.variant };
    ui.set_info_look(look);
    let list = ordered();
    // the fridge's notes when a page on the list has that look
    let look = if draw::AVAILABLE && crate::page_looks(ui, "info").contains(&1) { 1 } else { look };
    let rows: Vec<InfoNote> = list.iter().map(|(_, n)| note_row(n, look, false)).collect();
    // the open note (gone when Home Assistant dismissed it)
    let open = OPEN.with(|o| o.borrow().clone()).and_then(|id| list.iter().position(|(k, _)| *k == id));
    match open {
        Some(i) => {
            ui.set_info_card_note(note_row(&list[i].1, look, true));
            ui.set_info_card_index(i as i32 + 1);
            ui.set_info_card_count(list.len() as i32);
            ui.set_info_card(true);
        }
        None => {
            OPEN.with(|o| *o.borrow_mut() = None);
            ui.set_info_card(false);
        }
    }
    // the pager's list: the status screens' rows, then the notifications
    let first = FIRST.with(|f| f.get()).min(pager_rows(slint::Model::row_count(&ui.get_info_statuses()), list.len()).saturating_sub(6));
    FIRST.with(|f| f.set(first));
    ui.set_info_pager_first(first as i32);
    if look == 1 {
        ui.set_info_fridge_empty(draw::get(Job::Fridge {
            title: texts::kept("info.fridge.empty", 0),
            message: "Nothing new on the fridge.".into(),
            foot: "the fridge · now".into(),
            level: 0,
            seed: 4242,
            big: false,
        }));
    }
    // the pager's and the news look's lines (their clock follows the panel's, `info-minute`); the pager's body and the
    // fridge door
    if slot.variant == 2 {
        // the pager's finish: loaded again whenever it changes
        let finish = crate::page_option(_s, "info", "finish").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "graphite".into());
        let finish = FINISHES[finish_index(&finish) as usize].to_string();
        ui.set_info_pager_finish(finish_index(&finish));
        if PAGER_FINISH.with(|f| *f.borrow() != finish) || ui.get_info_pager_body().size().width == 0 {
            ui.set_info_pager_body(crate::quick::picture(&format!("info/pager-{finish}.png")));
            ui.set_info_pager_led(crate::quick::picture("info/pager-led.png"));
            PAGER_FINISH.with(|f| *f.borrow_mut() = finish);
        }
    }
    if slot.variant == 1 {
        let door = if slint::ComponentHandle::global::<crate::Theme>(ui).get_dark() { "info/door-dark.png" } else { "info/door-light.png" };
        ui.set_info_fridge_door(crate::quick::picture(door));
    }
    // the news look's crawl: the headlines, a line of the channel's own after every second one and the rest of
    // those lines at the end (three of them: the crawl is never shorter than its strip)
    let mut crawl: Vec<String> = Vec::new();
    let mut own = (0..3).map(|i| texts::kept("info.news.tickers", i));
    for (k, (_, n)) in list.iter().enumerate() {
        crawl.push(if n.title.is_empty() { n.message.to_uppercase() } else { n.title.to_uppercase() });
        if k % 2 == 1 {
            crawl.extend(own.next());
        }
    }
    crawl.extend(own);
    ui.set_info_news_ticker(crawl.join("   \u{2022}   ").into());
    ui.set_info_news_empty(texts::kept("info.news.empty", 0).into());
    ui.set_info_pager_empty(texts::kept("info.pager.empty", 0).into());
    if rows.is_empty() {
        ui.set_info_empty_title(texts::kept("info.empty.titles", 0).into());
        ui.set_info_empty_sub(texts::kept("info.empty.subs", 0).into());
    }
    if let Some(m) = crate::pages::update_rows(&ui.get_info_notes(), rows) {
        ui.set_info_notes(m);
    }
}

/// The pager's rows: the active status screens' first, then the notifications.
fn pager_rows(statuses: usize, notes: usize) -> usize {
    statuses + notes
}

/// What the pager's OK opens from its list (its first row shown): a status screen (its index in the active ones) or a
/// notification (its index in the list).
#[derive(Debug, PartialEq)]
enum PagerRow {
    Status(usize),
    Note(usize),
    None,
}

fn pager_row(first: usize, statuses: usize, notes: usize) -> PagerRow {
    if first < statuses {
        PagerRow::Status(first)
    } else if first - statuses < notes {
        PagerRow::Note(first - statuses)
    } else {
        PagerRow::None
    }
}

/// The active status screens' rows came or went (`alerts.rs`): the pager's first row stays within its list.
pub fn rows_changed(ui: &AppWindow, statuses: usize) {
    let notes = NOTES.with(|n| n.borrow().len());
    let first = FIRST.with(|f| f.get()).min(pager_rows(statuses, notes).saturating_sub(6));
    FIRST.with(|f| f.set(first));
    if ui.get_info_pager_first() != first as i32 {
        ui.set_info_pager_first(first as i32);
    }
}

/// The pager's finish from its menu on the panel (39.3): the same page option the integration sets. It shows at once
/// (the panel keeps it until the integration's settings come back with it).
fn set_finish(ui: &AppWindow, finish: &str) {
    if !FINISHES.contains(&finish) {
        return;
    }
    with_state(|s| {
        let page = s
            .panel_config
            .as_mut()
            .and_then(|c| c.get_mut("pages"))
            .and_then(Value::as_array_mut)
            .and_then(|pages| pages.iter_mut().find(|p| p.get("kind").and_then(Value::as_str) == Some("info")));
        if let Some(page) = page.and_then(Value::as_object_mut) {
            let options = page.entry("options").or_insert_with(|| json!({}));
            if let Some(o) = options.as_object_mut() {
                o.insert("finish".into(), json!(finish));
            }
        }
    });
    crate::page_set(ui, "info", None, json!({ "finish": finish }));
}

/// The page left the screen (#9): its open notification closes (the list shows when it comes back).
pub fn leave(ui: &AppWindow) {
    if OPEN.with(|o| o.borrow_mut().take()).is_some() {
        ui.set_info_card(false);
    }
}

fn open_note(id: Option<String>) {
    if let Some(id) = &id {
        if NOTES.with(|m| m.borrow().get(id).is_some_and(|n| n.own && n.unread)) {
            answer(id, "read", None);
        }
        NOTES.with(|m| {
            if let Some(note) = m.borrow_mut().get_mut(id) {
                note.unread = false;
            }
        });
    }
    OPEN.with(|o| *o.borrow_mut() = id);
}

/// Taps on the page (`info.*`).
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    let n: usize = value.parse().unwrap_or(0);
    let list = ordered();
    let open = OPEN.with(|o| o.borrow().clone()).and_then(|id| list.iter().position(|(k, _)| *k == id));
    match what {
        "info.read" => open_note(list.get(n).map(|(id, _)| id.clone())),
        "info.close" => open_note(None),
        "info.action" => {
            if let Some(i) = open {
                let (id, note) = &list[i];
                match n {
                    0 if note.own => {
                        answer(id, "dismiss", None);
                        NOTES.with(|m| m.borrow_mut().remove(id));
                    }
                    0 => crate::call("persistent_notification", "dismiss", "", json!({ "notification_id": id })),
                    k if !note.buttons.is_empty() => answer(id, "button", Some(k - 1)),
                    _ => answer(id, "ok", None),
                }
            }
            open_note(None);
        }
        // the pager: in the list the arrows scroll and OK opens the top row; in a message they page and OK goes back
        "info.up" | "info.down" => match open {
            Some(i) => {
                let j = if what == "info.up" { i.saturating_sub(1) } else { (i + 1).min(list.len().saturating_sub(1)) };
                open_note(list.get(j).map(|(id, _)| id.clone()));
            }
            None => FIRST.with(|f| f.set(if what == "info.up" { f.get().saturating_sub(1) } else { f.get() + 1 })),
        },
        "info.ok" => match open {
            Some(_) => open_note(None),
            // the top row: a status screen opens again, a notification opens
            None => {
                let active = crate::alerts::listed_ids();
                match pager_row(FIRST.with(|f| f.get()), active.len(), list.len()) {
                    PagerRow::Status(k) => {
                        crate::alerts::open(ui, &active[k]);
                        return;
                    }
                    PagerRow::Note(k) => open_note(list.get(k).map(|(id, _)| id.clone())),
                    PagerRow::None => {}
                }
            }
        },
        // an active status screen on the list: it opens again (a critical alert's row is only shown)
        "info.status" => {
            crate::alerts::open(ui, value);
            return;
        }
        // the shade's rows (29.5): a swipe dismisses one, a tap opens it in its window
        "info.dismiss" => {
            if let Some((id, note)) = list.get(n) {
                if note.own {
                    answer(id, "dismiss", None);
                    NOTES.with(|m| m.borrow_mut().remove(id));
                } else {
                    crate::call("persistent_notification", "dismiss", "", json!({ "notification_id": id }));
                }
            }
        }
        "info.show" => {
            let id = list.get(n).map(|(id, _)| id.clone());
            pop(ui, id.as_deref());
        }
        // the pager's menu: its finish (graphite, purple, clear, offwhite)
        "info.finish" => set_finish(ui, value),
        "info.clear" => {
            for (id, note) in list {
                if note.own {
                    answer(&id, "dismiss", None);
                    NOTES.with(|m| m.borrow_mut().remove(&id));
                } else {
                    crate::call("persistent_notification", "dismiss", "", json!({ "notification_id": id }));
                }
            }
        }
        _ => eprintln!("nspanel-app: act {what} {value} (not handled)"),
    }
    with_state(|s| refresh(ui, s));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finishes() {
        assert_eq!(finish_index("graphite"), 0);
        assert_eq!(finish_index("purple"), 1);
        assert_eq!(finish_index("offwhite"), 3);
        assert_eq!(finish_index("gold"), 0);
    }

    #[test]
    fn pager_rows_start_with_the_status_screens() {
        assert_eq!(pager_rows(2, 3), 5);
        assert_eq!(pager_row(0, 2, 3), PagerRow::Status(0));
        assert_eq!(pager_row(1, 2, 3), PagerRow::Status(1));
        assert_eq!(pager_row(2, 2, 3), PagerRow::Note(0));
        assert_eq!(pager_row(4, 2, 3), PagerRow::Note(2));
        assert_eq!(pager_row(5, 2, 3), PagerRow::None);
        assert_eq!(pager_row(0, 0, 0), PagerRow::None);
        assert_eq!(pager_row(0, 0, 1), PagerRow::Note(0));
    }

    #[test]
    fn minutes_of_the_day() {
        assert_eq!(minute_of("14", "28"), 868);
        assert_eq!(minute_of("00", "00"), 0);
        assert_eq!(minute_of("23", "59"), 1439);
        assert_eq!(minute_of("--", "--"), -1);
        assert_eq!(minute_of("24", "00"), -1);
    }
}
