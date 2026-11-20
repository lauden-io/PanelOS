//! To-do page: the to-do lists picked for it, the chosen list's items (read
//! with `todo.get_items` over Home Assistant's REST API in a thread: every
//! minute, when its state changes and right after a tick), ticked here with
//! `todo.update_item`.

use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use slint::{ComponentHandle, SharedString};

use crate::draw::{self, Job, Tint};
use crate::pages::{local_format, now, parse_iso};
use crate::{icons, texts, with_state, AppWindow, TodoItem, TodoList};

#[derive(Clone, Debug, PartialEq)]
struct Item {
    uid: String,
    summary: String,
    done: bool,
    due: String,
    note: String,
}

thread_local! {
    static ITEMS: std::cell::RefCell<Vec<Item>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The third due date chip: days from today.
    static DUE_DAYS: std::cell::Cell<u32> = const { std::cell::Cell::new(2) };
    /// Items just ticked on the panel: they stay in the open list (ticked, struck through, folding: spec M24) for
    /// LEAVE_MS, then move down.
    static LEAVING: std::cell::RefCell<Vec<(String, Instant)>> = const { std::cell::RefCell::new(Vec::new()) };
}
const LEAVE_MS: u64 = 1500;

thread_local! {
    /// An item added on the panel, to be moved to the top of its list once Home Assistant knows its id: (list, text).
    static TO_TOP: std::cell::RefCell<Option<(String, String)>> = const { std::cell::RefCell::new(None) };
}

thread_local! {
    /// The keys of the last update of a list, and the items first seen in a later update with when (spec 25.D3-D5: the
    /// looks that print or write a new item in; not on the first read of a list).
    static SEEN: std::cell::RefCell<(String, Vec<String>, Vec<(String, Instant)>)> = const { std::cell::RefCell::new((String::new(), Vec::new(), Vec::new())) };
}
const FRESH_MS: u64 = 900;

fn item_key(i: &Item) -> String {
    if i.uid.is_empty() { i.summary.clone() } else { i.uid.clone() }
}
/// The list to read, and a request to read it now.
static WANTED: Mutex<(String, bool)> = Mutex::new((String::new(), false));
static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
        let mut last = String::new();
        let mut next = Instant::now();
        loop {
            let (list, now_please) = {
                let mut w = WANTED.lock().unwrap();
                let r = w.clone();
                w.1 = false;
                r
            };
            if !list.is_empty() && (list != last || now_please || Instant::now() >= next) {
                last = list.clone();
                next = Instant::now() + Duration::from_secs(60);
                if let Some(items) = fetch(&url, &auth, &list) {
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        // a new item from the panel goes to the top of the list (C-550)
                        let top = TO_TOP.with(|t| t.borrow().clone());
                        if let Some((l, text)) = top.filter(|(l, _)| *l == list) {
                            if let Some(pos) = items.iter().position(|i| !i.done && i.summary == text && !i.uid.is_empty()) {
                                TO_TOP.with(|t| *t.borrow_mut() = None);
                                if pos > 0 {
                                    crate::send_raw(json!({ "type": "todo/item/move", "entity_id": l, "uid": items[pos].uid, "previous_uid": null }));
                                    WANTED.lock().unwrap().1 = true;
                                }
                            }
                        }
                        ITEMS.with(|i| *i.borrow_mut() = items);
                        if let Some(ui) = weak.upgrade() {
                            with_state(|s| refresh(&ui, s));
                        }
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

fn fetch(url: &str, auth: &std::sync::Arc<crate::auth::Auth>, list: &str) -> Option<Vec<Item>> {
    let token = auth.access_token().ok()?;
    let resp = ureq::post(&format!("{url}/api/services/todo/get_items?return_response"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(10))
        .send_string(&json!({ "entity_id": list }).to_string());
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            eprintln!("nspanel-app: to-do {list}: {e}");
            return None;
        }
    };
    let mut body = String::new();
    resp.into_reader().take(2 * 1024 * 1024).read_to_string(&mut body).ok()?;
    let v: Value = serde_json::from_str(&body).ok()?;
    let items = v.get("service_response")?.get(list)?.get("items")?.as_array()?;
    Some(
        items
            .iter()
            .map(|i| {
                let text = |k: &str| i.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                Item {
                    uid: text("uid"),
                    summary: text("summary"),
                    done: text("status") == "completed",
                    due: text("due"),
                    note: text("description").lines().next().unwrap_or("").to_string(),
                }
            })
            .collect(),
    )
}

/// The due date as the list shows it, and how urgent it is (0 later, 1 today, 2 overdue).
fn due(d: &str) -> (String, i32) {
    if d.is_empty() {
        return (String::new(), 0);
    }
    let t = now();
    let today = local_format(t, "%Y-%m-%d");
    let tomorrow = local_format(t + 86400, "%Y-%m-%d");
    let (date, time) = match parse_iso(d) {
        Some(e) if d.len() > 10 => (local_format(e, "%Y-%m-%d"), Some(crate::clock::local_hm(e))),
        _ => (d.get(..10).unwrap_or(d).to_string(), None),
    };
    let state = if date < today { 2 } else if date == today { 1 } else { 0 };
    let day = if date == today {
        "Today".to_string()
    } else if date == tomorrow {
        "Tomorrow".into()
    } else {
        // "Nov 20" for other days (the local date of the due day).
        let e = parse_iso(&format!("{date}T12:00:00")).unwrap_or(t);
        local_format(e, "%b %-d")
    };
    (time.map(|t| format!("{day} {t}")).unwrap_or(day), state)
}

fn row(i: &Item) -> TodoItem {
    let (due, due_state) = due(&i.due);
    TodoItem {
        text: i.summary.clone().into(),
        note: i.note.clone().into(),
        due: due.into(),
        due_state,
        done: i.done,
        tape: Default::default(),
        hand: Default::default(),
        fresh: false,
    }
}

/// The to-do page (only when the panel has one).
pub fn refresh(ui: &AppWindow, s: &mut crate::UiState) {
    let pages = ui.get_pages();
    let Some(slot) = (0..slint::Model::row_count(&pages)).filter_map(|i| slint::Model::row_data(&pages, i)).find(|p| p.kind == "todo")
    else {
        return;
    };
    let entities = crate::page_entities(s, "todo");
    if entities.is_empty() {
        return;
    }
    // the label maker and the notepad need pictures drawn by the app (GPU build); otherwise the plain look
    let look = if !draw::AVAILABLE && slot.variant >= 3 { 0 } else { slot.variant };
    ui.set_todo_look(look);
    let sel = (ui.get_todo_sel().max(0) as usize).min(entities.len() - 1);
    {
        let mut w = WANTED.lock().unwrap();
        // A changed count of open items in HA: read the list again.
        let open: String = s.states.get(&entities[sel]).map(|e| e.state.clone()).unwrap_or_default();
        thread_local!(static OPEN: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) });
        if OPEN.with(|o| o.replace(open.clone())) != open {
            w.1 = true;
        }
        w.0 = entities[sel].clone();
    }
    // the list takes due dates (Home Assistant's "set due date" feature, 16, or date and time, 32)
    let features = s.states.get(&entities[sel]).and_then(|e| e.attrs.get("supported_features")).and_then(Value::as_u64).unwrap_or(0);
    ui.set_todo_can_due(features & (16 | 32) != 0);
    // descriptions on items (SET_DESCRIPTION_ON_ITEM)
    ui.set_todo_can_note(features & 64 != 0);
    ensure_started(ui, s);
    let lists: Vec<TodoList> = entities
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let st = s.states.get(id);
            TodoList {
                name: st.and_then(|e| e.attrs.get("friendly_name")).and_then(Value::as_str).unwrap_or(id).into(),
                icon: icons::glyph(if id.contains("shopping") { "cart-outline" } else { "format-list-checks" })
                    .map(String::from)
                    .unwrap_or_default()
                    .into(),
                open: st.and_then(|e| e.state.parse().ok()).unwrap_or(0),
                tint: (i % 4) as i32,
            }
        })
        .collect();
    let name = lists.get(sel).map(|l| l.name.to_string()).unwrap_or_default();
    if let Some(m) = crate::pages::update_rows(&ui.get_todo_lists(), lists) {
        ui.set_todo_lists(m);
    }
    ui.set_todo_sel(sel as i32);
    let items = ITEMS.with(|i| i.borrow().clone());
    let show_done = ui.get_todo_show_done();
    // the label maker's tapes and the notepad's lines (completed ones only while they are unfolded)
    let tint = [Tint::Red, Tint::Blue, Tint::Green, Tint::Black][sel % 4];
    let looks = crate::page_looks(ui, "todo");
    let (tapes, hands) = (draw::AVAILABLE && looks.contains(&3), draw::AVAILABLE && looks.contains(&4));
    let drawn = |i: &Item, done_part: bool| -> TodoItem {
        let mut r = row(i);
        if done_part && !show_done {
            return r;
        }
        let (due_text, due_state) = due(&i.due);
        let struck = done_part || i.done;
        let seed = draw::seed(&i.summary);
        if tapes {
            r.tape = draw::get(Job::TapeItem { text: i.summary.clone(), tint, seed, due: due_text.clone(), due_state, note: i.note.clone(), struck });
        }
        if hands {
            r.hand = draw::get(Job::HandItem { text: i.summary.clone(), seed, due: due_text, due_state, note: i.note.clone(), ticked: struck, faded: done_part });
        }
        r
    };
    let leaving: Vec<String> = LEAVING.with(|l| {
        let mut l = l.borrow_mut();
        l.retain(|(_, t)| t.elapsed() < Duration::from_millis(LEAVE_MS));
        l.iter().map(|(k, _)| k.clone()).collect()
    });
    let stays = |i: &&Item| i.done && leaving.contains(&item_key(i));
    let list_id = entities[sel].clone();
    let fresh: Vec<String> = SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        let keys: Vec<String> = items.iter().map(item_key).collect();
        if seen.0 == list_id && !seen.1.is_empty() {
            for k in &keys {
                if !seen.1.contains(k) && !seen.2.iter().any(|(f, _)| f == k) {
                    seen.2.push((k.clone(), Instant::now()));
                }
            }
        } else {
            seen.2.clear();
        }
        seen.0 = list_id.clone();
        seen.1 = keys;
        seen.2.retain(|(_, t)| t.elapsed() < Duration::from_millis(FRESH_MS));
        seen.2.iter().map(|(k, _)| k.clone()).collect()
    });
    let open: Vec<TodoItem> = items
        .iter()
        .filter(|i| !i.done || stays(i))
        .map(|i| {
            let mut r = drawn(i, false);
            r.fresh = fresh.contains(&item_key(i));
            r
        })
        .collect();
    let done: Vec<TodoItem> = items.iter().filter(|i| i.done && !stays(i)).map(|i| drawn(i, true)).collect();
    ui.set_todo_leaving(items.iter().filter(|i| stays(i)).count() as i32);
    // the receipt and terminal looks' own lines and the receipt's stamp
    ui.set_todo_receipt_footer(texts::kept("todo.receipt.footers", 0).into());
    ui.set_todo_receipt_empty(texts::kept("todo.receipt.empty", 0).into());
    ui.set_todo_term_empty(texts::kept("todo.term.empty", 0).into());
    ui.set_todo_stamp(format!("{} {}", local_format(now(), "%d.%m.%Y"), crate::clock::local_hm(now())).into());
    if open.is_empty() {
        ui.set_todo_empty_title(texts::kept("todo.empty.titles", 0).into());
        ui.set_todo_empty_sub(texts::kept("todo.empty.subs", 0).into());
    }
    let n_open = open.iter().filter(|i| !i.done).count();
    if tapes {
        let count = if n_open == 0 { "ALL CLEAR".to_string() } else { format!("{n_open} TO DO") };
        ui.set_todo_label_head(draw::get(Job::TapeHead { name: name.clone(), count, seed: draw::seed(&name) }));
        ui.set_todo_label_done_head(draw::get(Job::Tape { text: format!("DONE · {}", done.len()), tint: Tint::Black, small: true, seed: 77 + done.len() as u32 }));
        if n_open == 0 {
            let text = texts::kept("todo.label.empty", 0);
            ui.set_todo_label_empty_tape(draw::get(Job::Tape { text, tint, small: false, seed: 4242 }));
        }
    }
    if hands {
        let count = if n_open == 0 { "all done".to_string() } else { format!("{n_open} left") };
        ui.set_todo_note_head(draw::get(Job::HandHead { title: name.clone(), count, seed: draw::seed(&name) }));
        ui.set_todo_note_done(draw::get(Job::HandDone { n: done.len(), open: show_done }));
        if n_open == 0 {
            let (title, sub) = (ui.get_todo_empty_title().to_string(), ui.get_todo_empty_sub().to_string());
            ui.set_todo_note_empty(draw::get(Job::HandEmpty { title, sub }));
        }
    }
    if let Some(m) = crate::pages::update_rows(&ui.get_todo_open(), open) {
        ui.set_todo_open(m);
    }
    if let Some(m) = crate::pages::update_rows(&ui.get_todo_done(), done) {
        ui.set_todo_done(m);
    }
}

/// Taps on the page (`todo.*`).
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    let n: usize = value.parse().unwrap_or(0);
    match what {
        "todo.check" | "todo.uncheck" => {
            let list = WANTED.lock().unwrap().0.clone();
            let items = ITEMS.with(|i| i.borrow().clone());
            let pick: Vec<&Item> = items.iter().filter(|i| i.done == (what == "todo.uncheck")).collect();
            let Some(item) = pick.get(n) else { return };
            let status = if what == "todo.check" { "completed" } else { "needs_action" };
            let key = item_key(item);
            crate::call("todo", "update_item", &list, json!({ "item": key, "status": status }));
            // ticked here: it stays in the open list while its tick plays, then moves down
            if what == "todo.check" {
                LEAVING.with(|l| l.borrow_mut().push((key.clone(), Instant::now())));
                let weak = ui.as_weak();
                slint::Timer::single_shot(Duration::from_millis(LEAVE_MS + 20), move || {
                    if let Some(ui) = weak.upgrade() {
                        with_state(|s| refresh(&ui, s));
                    }
                });
            }
            // Shown ticked at once; the list is read again right after.
            let uid = item.uid.clone();
            ITEMS.with(|i| {
                for it in i.borrow_mut().iter_mut() {
                    if it.uid == uid {
                        it.done = what == "todo.check";
                    }
                }
            });
            WANTED.lock().unwrap().1 = true;
            with_state(|s| refresh(ui, s));
        }
        "todo.done-toggle" => {
            ui.set_todo_show_done(!ui.get_todo_show_done());
            with_state(|s| refresh(ui, s));
        }
        // adding an item: the field with the keyboard (a capital first), due date chips where the list takes one
        "todo.add" => {
            ui.set_todo_draft(SharedString::new());
            ui.set_todo_note_draft(SharedString::new());
            ui.set_todo_note_on(false);
            ui.set_todo_due_pick(-1);
            ui.set_todo_kb_layer(1);
            ui.set_todo_adding(true);
        }
        "todo.add-cancel" => ui.set_todo_adding(false),
        // the list chip (and the list name): the sheet with the lists and the look
        "todo.picker" => ui.set_todo_picker(true),
        "todo.picker-close" => ui.set_todo_picker(false),
        "todo.pick" => {
            ui.set_todo_picker(false);
            if n < slint::Model::row_count(&ui.get_todo_lists()) && n as i32 != ui.get_todo_sel() {
                ui.set_todo_sel(n as i32);
                ui.set_todo_scroll(0.0);
                WANTED.lock().unwrap().1 = true;
                with_state(|s| refresh(ui, s));
            }
        }
        "todo.look" => {
            ui.set_todo_picker(false);
            crate::page_set(ui, "todo", Some(n as i32), Value::Null);
            with_state(|s| refresh(ui, s));
        }
        // which field the keyboard writes: 0 the item, 1 its description
        "todo.field" => {
            let note = n == 1 && ui.get_todo_can_note();
            ui.set_todo_note_on(note);
            let empty = if note { ui.get_todo_note_draft().is_empty() } else { ui.get_todo_draft().is_empty() };
            if empty {
                ui.set_todo_kb_layer(1);
            }
        }
        "todo.due" => {
            let pick = ui.get_todo_due_pick();
            match n {
                // a second tap takes the date off
                0 | 1 if pick == n as i32 => ui.set_todo_due_pick(-1),
                0 | 1 => ui.set_todo_due_pick(n as i32),
                // the third chip steps a day further on each tap (from the day after tomorrow, two weeks on)
                _ => {
                    let days = if pick == 2 { DUE_DAYS.with(|d| d.get()) % 15 + 1 } else { 2 };
                    let days = days.max(2);
                    DUE_DAYS.with(|d| d.set(days));
                    ui.set_todo_due_picked(local_format(now() + days as i64 * 86_400, "%a %-d %b").into());
                    ui.set_todo_due_pick(2);
                }
            }
        }
        "todo.key" => {
            let note_on = ui.get_todo_note_on();
            let mut draft = if note_on { ui.get_todo_note_draft().to_string() } else { ui.get_todo_draft().to_string() };
            let layer = ui.get_todo_kb_layer();
            match value {
                "enter" => {
                    let text = if note_on { ui.get_todo_draft().trim().to_string() } else { draft.trim().to_string() };
                    let note = if note_on { draft.trim().to_string() } else { ui.get_todo_note_draft().trim().to_string() };
                    if !text.is_empty() {
                        let list = WANTED.lock().unwrap().0.clone();
                        let days = match ui.get_todo_due_pick() {
                            0 => Some(0),
                            1 => Some(1),
                            2 => Some(DUE_DAYS.with(|d| d.get())),
                            _ => None,
                        };
                        let mut data = json!({ "item": text });
                        if let Some(d) = days {
                            data["due_date"] = json!(local_format(now() + d as i64 * 86_400, "%Y-%m-%d"));
                        }
                        if !note.is_empty() && ui.get_todo_can_note() {
                            data["description"] = json!(note);
                        }
                        crate::call("todo", "add_item", &list, data);
                        TO_TOP.with(|t| *t.borrow_mut() = Some((list.clone(), text.clone())));
                        // shown at once, at the top; the list is read again right after
                        ITEMS.with(|i| {
                            i.borrow_mut().insert(0, Item { uid: String::new(), summary: text, done: false, due: String::new(), note })
                        });
                        WANTED.lock().unwrap().1 = true;
                    }
                    ui.set_todo_adding(false);
                    with_state(|s| refresh(ui, s));
                    return;
                }
                "back" | "clear" => {
                    if value == "back" {
                        draft.pop();
                    } else {
                        draft.clear();
                    }
                    // back at the start of the line: a capital again
                    if draft.is_empty() && layer == 0 {
                        ui.set_todo_kb_layer(1);
                    }
                }
                "shift" => ui.set_todo_kb_layer(match layer {
                    0 => 1,
                    1 => 0,
                    2 => 3,
                    _ => 2,
                }),
                "symbols" => ui.set_todo_kb_layer(if layer >= 2 { 0 } else { 2 }),
                "layout" => ui.set_kb_layout(1 - ui.get_kb_layout().clamp(0, 1)),
                k => {
                    if draft.chars().count() < 120 {
                        draft.push_str(k);
                    }
                    // one capital, then small letters again
                    if layer == 1 {
                        ui.set_todo_kb_layer(0);
                    }
                }
            }
            if note_on {
                ui.set_todo_note_draft(SharedString::from(draft));
            } else {
                ui.set_todo_draft(SharedString::from(draft));
            }
        }
        _ => eprintln!("nspanel-app: act {what} {value} (not handled)"),
    }
}

