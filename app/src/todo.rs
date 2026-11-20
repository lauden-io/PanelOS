//! To-do page: the to-do lists picked for it, the chosen list's items (read
//! with `todo.get_items` over Home Assistant's REST API in a thread: every
//! minute, when its state changes and right after a tick), ticked here with
//! `todo.update_item`, added with `todo.add_item` (and moved to the top of
//! the list). The chip's list picker and the page settings are the look's own
//! menus (spec 40).

use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use slint::{ComponentHandle, SharedString};

use crate::clock::local_hm;
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
    /// Each list's items as Home Assistant sent them last (the lists the to-do pages show).
    static ITEMS: std::cell::RefCell<HashMap<String, Vec<Item>>> = std::cell::RefCell::new(HashMap::new());
    /// The third due date chip: days from today.
    static DUE_DAYS: std::cell::Cell<u32> = const { std::cell::Cell::new(2) };
    /// Items just ticked on the panel: they stay in the open list (ticked and struck through for 1.5 s, then folding
    /// in 250 ms: spec M24) for LEAVE_MS, then move down.
    static LEAVING: std::cell::RefCell<Vec<(String, Instant)>> = const { std::cell::RefCell::new(Vec::new()) };
}
const LEAVE_MS: u64 = 1750;

/// An item added on the panel: shown at the top of its list at once and moved there in Home Assistant (which adds at
/// the bottom) once it knows the item's id.
#[derive(Clone, Debug)]
struct Added {
    list: String,
    text: String,
    note: String,
    uid: Option<String>,
    moved: bool,
    since: Instant,
}
/// How long the panel keeps an added item on top by itself while Home Assistant catches up.
const ADDED_MS: u64 = 15_000;

thread_local! {
    static ADDED: std::cell::RefCell<Vec<Added>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The lists that take `todo/item/move` (Home Assistant's MOVE_TODO_ITEM feature).
    static CAN_MOVE: std::cell::RefCell<HashMap<String, bool>> = std::cell::RefCell::new(HashMap::new());
    /// A look change from the settings: the menu closes, the look changes, its menu opens again (counts the changes,
    /// so a later one wins).
    static LOOK_TURN: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The items as Home Assistant sent them, with the items added here on top: an item it already has moves to the top
/// (it adds at the bottom), one it has not got yet keeps its own row there. Returns the moves to send (list, uid), the
/// ids Home Assistant gave the added items in this read, and whether to read the list again soon.
fn place_added(list: &str, items: &mut Vec<Item>, added: &mut Vec<Added>, can_move: bool, now: Instant) -> (Vec<(String, String)>, Vec<String>, bool) {
    added.retain(|a| now.duration_since(a.since) < Duration::from_millis(ADDED_MS));
    let mut moves = Vec::new();
    let mut found = Vec::new();
    let mut again = false;
    let mut on_top: Vec<Item> = Vec::new();
    added.retain_mut(|a| {
        if a.list != list {
            return true;
        }
        let pos = match &a.uid {
            Some(u) => items.iter().position(|i| &i.uid == u),
            // the newest open item with that text (Home Assistant adds at the bottom)
            None => items.iter().rposition(|i| !i.done && i.summary == a.text && !i.uid.is_empty()),
        };
        match pos {
            Some(p) => {
                if a.uid.is_none() {
                    a.uid = Some(items[p].uid.clone());
                    found.push(items[p].uid.clone());
                }
                if p == 0 && on_top.is_empty() {
                    // Home Assistant has it on top: done
                    return false;
                }
                if !can_move {
                    // the list cannot move items: Home Assistant's order rules
                    return false;
                }
                if !a.moved {
                    moves.push((list.to_string(), items[p].uid.clone()));
                    a.moved = true;
                }
                again = true;
                on_top.push(items.remove(p));
                true
            }
            None => {
                // not there yet: the panel's own row stays on top until it is
                again = true;
                on_top.push(Item { uid: String::new(), summary: a.text.clone(), done: false, due: String::new(), note: a.note.clone() });
                true
            }
        }
    });
    // the last one added on top
    for it in on_top {
        items.insert(0, it);
    }
    (moves, found, again)
}

thread_local! {
    /// Each list's keys at its last update, and the items first seen in a later update with when (spec 25.D3-D5: the
    /// looks that print or write a new item in; not on the first read of a list).
    static SEEN: std::cell::RefCell<HashMap<String, (Vec<String>, Vec<(String, Instant)>)>> = std::cell::RefCell::new(HashMap::new());
    /// Each list's count of open items as Home Assistant's state says (a change reads the list again).
    static OPEN: std::cell::RefCell<HashMap<String, String>> = std::cell::RefCell::new(HashMap::new());
}
const FRESH_MS: u64 = 900;

fn item_key(i: &Item) -> String {
    if i.uid.is_empty() { i.summary.clone() } else { i.uid.clone() }
}
/// The lists to read (the one each to-do page shows), and the ones to read now.
static WANTED: Mutex<(Vec<String>, Vec<String>)> = Mutex::new((Vec::new(), Vec::new()));

/// A list read again now.
fn read_now(list: &str) {
    let mut w = WANTED.lock().unwrap();
    if !w.1.iter().any(|l| l == list) {
        w.1.push(list.to_string());
    }
}

fn can_move(list: &str) -> bool {
    CAN_MOVE.with(|c| c.borrow().get(list).copied().unwrap_or(false))
}
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
        // when each list is read next (every minute; at once when it is new or asked for)
        let mut next: HashMap<String, Instant> = HashMap::new();
        loop {
            let (lists, now_please) = {
                let mut w = WANTED.lock().unwrap();
                (w.0.clone(), std::mem::take(&mut w.1))
            };
            next.retain(|l, _| lists.contains(l));
            for list in lists {
                if !now_please.contains(&list) && next.get(&list).is_some_and(|t| Instant::now() < *t) {
                    continue;
                }
                next.insert(list.clone(), Instant::now() + Duration::from_secs(60));
                if let Some(items) = fetch(&url, &auth, &list) {
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        // a new item from the panel goes to the top of the list (C-550): without "previous_uid",
                        // Home Assistant moves it first
                        let mut items = items;
                        let (moves, found, again) = ADDED.with(|a| place_added(&list, &mut items, &mut a.borrow_mut(), can_move(&list), Instant::now()));
                        // an added item with its id from Home Assistant is not a new one again (it played its entrance)
                        SEEN.with(|seen| {
                            if let Some(seen) = seen.borrow_mut().get_mut(&list) {
                                seen.0.extend(found);
                            }
                        });
                        for (l, uid) in moves {
                            crate::send_raw(json!({ "type": "todo/item/move", "entity_id": l, "uid": uid }));
                        }
                        if again {
                            let list = list.clone();
                            slint::Timer::single_shot(Duration::from_millis(1200), move || read_now(&list));
                        }
                        ITEMS.with(|i| i.borrow_mut().insert(list, items));
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
        Some(e) if d.len() > 10 => (local_format(e, "%Y-%m-%d"), Some(local_hm(e))),
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

/// The label tapes' colours, one per list in turn.
const TAPE_TINTS: [Tint; 4] = [Tint::Red, Tint::Blue, Tint::Green, Tint::Black];

/// The list a to-do page shows: its pick among the page's lists (the first one until one is picked).
fn shown_list(entities: &[String], sel: i32) -> Option<(usize, String)> {
    if entities.is_empty() {
        return None;
    }
    let sel = (sel.max(0) as usize).min(entities.len() - 1);
    Some((sel, entities[sel].clone()))
}

/// Every to-do page (only when the panel has one), each with its own lists and the one it shows.
pub fn refresh(ui: &AppWindow, s: &mut crate::UiState) {
    let pages = crate::pages::kind_pages(s.panel_config.as_ref(), "todo").into_iter().map(|(_, p)| p.clone()).collect::<Vec<_>>();
    // the list each page shows: read (and read again when Home Assistant's count of its open items changes)
    let shows: Vec<Option<(usize, String)>> = pages
        .iter()
        .enumerate()
        .map(|(n, p)| shown_list(&crate::pages::entities_of(p), crate::pages::inst(&ui.get_todo_inst(), n).sel))
        .collect();
    let mut lists: Vec<String> = Vec::new();
    for (_, list) in shows.iter().flatten() {
        if !lists.contains(list) {
            lists.push(list.clone());
        }
    }
    {
        let open_changed: Vec<String> = lists
            .iter()
            .filter(|l| {
                let open: String = s.states.get(*l).map(|e| e.state.clone()).unwrap_or_default();
                OPEN.with(|o| o.borrow_mut().insert((*l).clone(), open.clone())).is_some_and(|before| before != open)
            })
            .cloned()
            .collect();
        WANTED.lock().unwrap().0 = lists.clone();
        for l in open_changed {
            read_now(&l);
        }
    }
    if lists.is_empty() {
        crate::pages::keep_insts(&ui.get_todo_inst(), pages.len());
        return;
    }
    let features = |list: &str| s.states.get(list).and_then(|e| e.attrs.get("supported_features")).and_then(Value::as_u64).unwrap_or(0);
    // items can be moved (MOVE_TODO_ITEM): a new one goes to the top
    CAN_MOVE.with(|c| {
        let mut c = c.borrow_mut();
        for l in &lists {
            c.insert(l.clone(), features(l) & 8 != 0);
        }
    });
    // the adding form is the page on screen's: its list takes due dates (Home Assistant's "set due date" feature, 16,
    // or date and time, 32) and descriptions on items (SET_DESCRIPTION_ON_ITEM)
    let acting = crate::acting_n("todo");
    if let Some(Some((_, list))) = shows.get(acting) {
        ui.set_todo_can_due(features(list) & (16 | 32) != 0);
        ui.set_todo_can_note(features(list) & 64 != 0);
    }
    ensure_started(ui, s);
    // the items first seen in this update of each list (once per list, whichever pages show it)
    let fresh: HashMap<String, Vec<String>> = lists
        .iter()
        .map(|list| {
            let keys: Vec<String> = ITEMS.with(|i| i.borrow().get(list).map(|v| v.iter().map(item_key).collect()).unwrap_or_default());
            let fresh = SEEN.with(|seen| {
                let mut seen = seen.borrow_mut();
                let entry = seen.entry(list.clone()).or_default();
                if !entry.0.is_empty() {
                    for k in &keys {
                        if !entry.0.contains(k) && !entry.1.iter().any(|(f, _)| f == k) {
                            entry.1.push((k.clone(), Instant::now()));
                        }
                    }
                }
                entry.0 = keys;
                entry.1.retain(|(_, t)| t.elapsed() < Duration::from_millis(FRESH_MS));
                entry.1.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()
            });
            (list.clone(), fresh)
        })
        .collect();
    let leaving: Vec<String> = LEAVING.with(|l| {
        let mut l = l.borrow_mut();
        l.retain(|(_, t)| t.elapsed() < Duration::from_millis(LEAVE_MS));
        l.iter().map(|(k, _)| k.clone()).collect()
    });
    // the receipt and terminal looks' own lines and the receipt's stamp
    ui.set_todo_receipt_footer(texts::kept("todo.receipt.footers", 0).into());
    ui.set_todo_receipt_empty(texts::kept("todo.receipt.empty", 0).into());
    ui.set_todo_term_empty(texts::kept("todo.term.empty", 0).into());
    ui.set_todo_stamp(format!("{} {}", local_format(now(), "%d.%m.%Y"), local_hm(now())).into());
    // the receipt's menus: "19 NOV 2026  14:28"
    ui.set_todo_menu_stamp(format!("{}  {}", local_format(now(), "%d %b %Y").to_uppercase(), local_hm(now())).into());
    ui.set_todo_empty_title(texts::kept("todo.empty.titles", 0).into());
    ui.set_todo_empty_sub(texts::kept("todo.empty.subs", 0).into());
    if draw::AVAILABLE {
        // the label maker's menus: the heads, labels and Done on the same tapes (drawn ahead, so a change to that look
        // opens its settings with them)
        let menu: Vec<slint::Image> = [
            ("LISTS", Tint::Black, false, 501),
            ("SETTINGS", Tint::Black, false, 502),
            ("LOOK", Tint::White, true, 503),
            ("TIME FORMAT", Tint::White, true, 504),
            ("DONE", Tint::Red, false, 505),
        ]
        .into_iter()
        .map(|(text, tint, small, seed)| draw::get(Job::Tape { text: text.into(), tint, small, seed }))
        .collect();
        if let Some(m) = crate::pages::update_rows(&ui.get_todo_menu_tapes(), menu) {
            ui.set_todo_menu_tapes(m);
        }
        if crate::page_looks(ui, "todo").contains(&4) {
            let (title, sub) = (ui.get_todo_empty_title().to_string(), ui.get_todo_empty_sub().to_string());
            ui.set_todo_note_empty(draw::get(Job::HandEmpty { title, sub }));
        }
    }
    for (n, page) in pages.iter().enumerate() {
        let entities = crate::pages::entities_of(page);
        let Some(Some((sel, list_id))) = shows.get(n).cloned() else { continue };
        // the label maker and the notepad need pictures drawn by the app (GPU build); otherwise the plain look
        let look = crate::pages::variant_of(page);
        let (tapes, hands) = (draw::AVAILABLE && look == 3, draw::AVAILABLE && look == 4);
        let show_done = crate::pages::inst(&ui.get_todo_inst(), n).show_done;
        let lists: Vec<TodoList> = entities
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let st = s.states.get(id);
                let name: String = st.and_then(|e| e.attrs.get("friendly_name")).and_then(Value::as_str).unwrap_or(id).to_string();
                let open: i32 = st.and_then(|e| e.state.parse().ok()).unwrap_or(0);
                let tint = (i % 4) as i32;
                // the label maker's menu: the name on tapes in the list's colour (the shown one marked and rimmed), what is
                // left on a small white tape
                let (tape, count_tape) = if tapes {
                    let shown = i == sel;
                    let text = if shown { format!("\u{25B6} {name}") } else { name.clone() };
                    (
                        draw::get(Job::TapeLabel { text, tint: TAPE_TINTS[i % 4], small: false, seed: draw::seed(&name), max_w: 220, rim: shown }),
                        draw::get(Job::Tape { text: open.to_string(), tint: Tint::White, small: true, seed: draw::seed(&name).wrapping_add(7) }),
                    )
                } else {
                    Default::default()
                };
                TodoList {
                    name: name.into(),
                    icon: icons::glyph(if id.contains("shopping") { "cart-outline" } else { "format-list-checks" })
                        .map(String::from)
                        .unwrap_or_default()
                        .into(),
                    open,
                    tint,
                    tape,
                    count_tape,
                }
            })
            .collect();
        let name = lists.get(sel).map(|l| l.name.to_string()).unwrap_or_default();
        let items = ITEMS.with(|i| i.borrow().get(&list_id).cloned().unwrap_or_default());
        // the label maker's tapes and the notepad's lines (completed ones only while they are unfolded)
        let tint = TAPE_TINTS[sel % 4];
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
        let stays = |i: &&Item| i.done && leaving.contains(&item_key(i));
        let fresh = fresh.get(&list_id).cloned().unwrap_or_default();
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
        let leaving_n = items.iter().filter(|i| stays(i)).count() as i32;
        let n_open = open.iter().filter(|i| !i.done).count();
        let heads = tapes.then(|| {
            let count = if n_open == 0 { "ALL CLEAR".to_string() } else { format!("{n_open} TO DO") };
            (
                draw::get(Job::TapeHead { name: name.clone(), count, seed: draw::seed(&name) }),
                draw::get(Job::Tape { text: format!("DONE \u{b7} {}", done.len()), tint: Tint::Black, small: true, seed: 77 + done.len() as u32 }),
                (n_open == 0).then(|| draw::get(Job::Tape { text: texts::kept("todo.label.empty", 0), tint, small: false, seed: 4242 })),
            )
        });
        let notes = hands.then(|| {
            let count = if n_open == 0 { "all done".to_string() } else { format!("{n_open} left") };
            (draw::get(Job::HandHead { title: name.clone(), count, seed: draw::seed(&name) }), draw::get(Job::HandDone { n: done.len(), open: show_done }))
        });
        crate::pages::with_inst(ui.get_todo_inst(), |m| ui.set_todo_inst(m), n, |d| {
            if let Some(m) = crate::pages::update_rows(&d.lists, lists) {
                d.lists = m;
            }
            d.sel = sel as i32;
            d.leaving = leaving_n;
            if let Some((head, done_head, empty)) = heads {
                d.label_head = head;
                d.label_done_head = done_head;
                if let Some(e) = empty {
                    d.label_empty_tape = e;
                }
            }
            if let Some((head, done_note)) = notes {
                d.note_head = head;
                d.note_done = done_note;
            }
            if let Some(m) = crate::pages::update_rows(&d.open_items, open) {
                d.open_items = m;
            }
            if let Some(m) = crate::pages::update_rows(&d.done_items, done) {
                d.done_items = m;
            }
        });
    }
    crate::pages::keep_insts(&ui.get_todo_inst(), pages.len());
}

/// The list the to-do page on screen shows.
fn acting_list(ui: &AppWindow) -> Option<String> {
    let entities = with_state(|s| crate::page_entities(s, "todo"));
    shown_list(&entities, crate::pages::inst(&ui.get_todo_inst(), crate::acting_n("todo")).sel).map(|(_, l)| l)
}

/// Taps on the page (`todo.*`).
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    let n: usize = value.parse().unwrap_or(0);
    match what {
        "todo.check" | "todo.uncheck" => {
            let Some(list) = acting_list(ui) else { return };
            let items = ITEMS.with(|i| i.borrow().get(&list).cloned().unwrap_or_default());
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
                for it in i.borrow_mut().get_mut(&list).into_iter().flatten() {
                    if it.uid == uid {
                        it.done = what == "todo.check";
                    }
                }
            });
            read_now(&list);
            with_state(|s| refresh(ui, s));
        }
        "todo.done-toggle" => {
            crate::pages::with_inst(ui.get_todo_inst(), |m| ui.set_todo_inst(m), crate::acting_n("todo"), |d| d.show_done = !d.show_done);
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
        // the list chip (and the list name): the list picker in the look's own object; the chip again closes it
        "todo.picker" => {
            let open = !ui.get_todo_picker();
            if open {
                ui.set_todo_settings(false);
                // fresh date and time on the receipt's head
                with_state(|s| refresh(ui, s));
            }
            ui.set_todo_picker(open);
        }
        "todo.picker-close" => ui.set_todo_picker(false),
        "todo.pick" => {
            ui.set_todo_picker(false);
            let at = crate::acting_n("todo");
            let d = crate::pages::inst(&ui.get_todo_inst(), at);
            if n < slint::Model::row_count(&d.lists) && n as i32 != d.sel {
                crate::pages::with_inst(ui.get_todo_inst(), |m| ui.set_todo_inst(m), at, |d| d.sel = n as i32);
                ui.set_todo_scroll(0.0);
                with_state(|s| refresh(ui, s));
                if let Some(list) = acting_list(ui) {
                    read_now(&list);
                }
            }
        }
        // the page settings (the quick shade's page row): look and time format
        "todo.settings" => {
            ui.set_todo_picker(false);
            ui.set_todo_pend_look(-1);
            with_state(|s| refresh(ui, s));
            ui.set_todo_settings(true);
        }
        // Done, a tap beside, [ save & exit ]: a look chosen in the terminal's config applies now
        "todo.settings-close" => {
            let pend = ui.get_todo_pend_look();
            ui.set_todo_pend_look(-1);
            if pend >= 0 && pend != current_look(ui) {
                change_look(ui, pend);
            } else {
                ui.set_todo_settings(false);
            }
        }
        // the page left the screen: its menus and the adding form close
        "todo.menus-close" => {
            ui.set_todo_picker(false);
            ui.set_todo_settings(false);
            ui.set_todo_pend_look(-1);
            ui.set_todo_adding(false);
        }
        // the terminal's config: the look line steps through the looks; it applies when the menu is left
        "todo.pend-look" => ui.set_todo_pend_look(if n as i32 == current_look(ui) { -1 } else { n as i32 }),
        "todo.look" => {
            if n as i32 != current_look(ui) {
                change_look(ui, n as i32);
            }
        }
        // the time format changed: the due times and stamps are written again (after the setting is stored)
        "todo.refresh" => {
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(0), move || {
                if let Some(ui) = weak.upgrade() {
                    with_state(|s| refresh(&ui, s));
                }
            });
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
                    if let (false, Some(list)) = (text.is_empty(), acting_list(ui)) {
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
                        // shown at once, at the top (where the list can move items; else where Home Assistant puts it,
                        // the end of the open ones); the list is read again right after
                        let row = Item { uid: String::new(), summary: text.clone(), done: false, due: String::new(), note: note.clone() };
                        if can_move(&list) {
                            ADDED.with(|a| a.borrow_mut().push(Added { list: list.clone(), text, note, uid: None, moved: false, since: Instant::now() }));
                            ITEMS.with(|i| i.borrow_mut().entry(list.clone()).or_default().insert(0, row));
                        } else {
                            ITEMS.with(|i| {
                                let mut i = i.borrow_mut();
                                let items = i.entry(list.clone()).or_default();
                                let at = items.iter().rposition(|it| !it.done).map(|p| p + 1).unwrap_or(0);
                                items.insert(at, row);
                            });
                        }
                        read_now(&list);
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


/// The look of the to-do page on screen.
fn current_look(_ui: &AppWindow) -> i32 {
    crate::page_look("todo")
}

/// A new look from the settings: the menu closes (its own way out), the page takes the look, and the settings open again
/// as the new look's menu, as long as the page is still on screen.
fn change_look(ui: &AppWindow, look: i32) {
    ui.set_todo_settings(false);
    ui.set_todo_pend_look(-1);
    let turn = LOOK_TURN.with(|t| {
        t.set(t.get() + 1);
        t.get()
    });
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(180), move || {
        let Some(ui) = weak.upgrade() else { return };
        if LOOK_TURN.with(|t| t.get()) != turn {
            return;
        }
        crate::page_set(&ui, "todo", Some(look), Value::Null);
        with_state(|s| refresh(&ui, s));
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(60), move || {
            let Some(ui) = weak.upgrade() else { return };
            let pages = ui.get_pages();
            let on_screen = slint::Model::row_data(&pages, ui.get_page().max(0) as usize).is_some_and(|p| p.kind == "todo");
            if LOOK_TURN.with(|t| t.get()) == turn && on_screen && !ui.get_todo_picker() {
                ui.set_todo_settings(true);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(uid: &str, text: &str, done: bool) -> Item {
        Item { uid: uid.into(), summary: text.into(), done, due: String::new(), note: String::new() }
    }

    fn added(text: &str, since: Instant) -> Added {
        Added { list: "todo.a".into(), text: text.into(), note: String::new(), uid: None, moved: false, since }
    }

    #[test]
    fn an_added_item_goes_to_the_top_and_is_moved_once() {
        let now = Instant::now();
        let mut adds = vec![added("Milk", now)];
        let mut items = vec![item("1", "Eggs", false), item("2", "Bread", true), item("3", "Milk", false)];
        let (moves, _, again) = place_added("todo.a", &mut items, &mut adds, true, now);
        assert_eq!(moves, vec![("todo.a".to_string(), "3".to_string())]);
        assert!(again);
        assert_eq!(items[0].uid, "3");
        assert_eq!(adds[0].uid.as_deref(), Some("3"));
        // the next read before Home Assistant moved it: still on top, not moved again
        let mut items = vec![item("1", "Eggs", false), item("3", "Milk", false)];
        let (moves, _, _) = place_added("todo.a", &mut items, &mut adds, true, now);
        assert!(moves.is_empty());
        assert_eq!(items[0].uid, "3");
        // Home Assistant has it on top: the panel lets go
        let mut items = vec![item("3", "Milk", false), item("1", "Eggs", false)];
        let (moves, _, again) = place_added("todo.a", &mut items, &mut adds, true, now);
        assert!(moves.is_empty() && !again && adds.is_empty());
    }

    #[test]
    fn an_item_home_assistant_has_not_got_yet_keeps_its_row_on_top() {
        let now = Instant::now();
        let mut adds = vec![added("Milk", now)];
        let mut items = vec![item("1", "Eggs", false)];
        let (moves, _, again) = place_added("todo.a", &mut items, &mut adds, true, now);
        assert!(moves.is_empty() && again);
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].uid.as_str(), items[0].summary.as_str()), ("", "Milk"));
    }

    #[test]
    fn the_newest_of_two_same_texts_is_the_added_one() {
        let now = Instant::now();
        let mut adds = vec![added("Milk", now)];
        let mut items = vec![item("1", "Milk", false), item("2", "Eggs", false), item("3", "Milk", false)];
        let (moves, _, _) = place_added("todo.a", &mut items, &mut adds, true, now);
        assert_eq!(moves[0].1, "3");
    }

    #[test]
    fn lists_that_cannot_move_keep_their_order_and_old_adds_are_dropped() {
        let now = Instant::now();
        let mut adds = vec![added("Milk", now)];
        let mut items = vec![item("1", "Eggs", false), item("3", "Milk", false)];
        let (moves, _, again) = place_added("todo.a", &mut items, &mut adds, false, now);
        assert!(moves.is_empty() && !again && adds.is_empty());
        assert_eq!(items[1].uid, "3");
        let old = now.checked_sub(Duration::from_millis(ADDED_MS + 1)).unwrap_or(now);
        let mut adds = vec![added("Milk", old)];
        let mut items = vec![item("1", "Eggs", false)];
        place_added("todo.a", &mut items, &mut adds, true, now);
        if old != now {
            assert!(adds.is_empty());
            assert_eq!(items.len(), 1);
        }
    }

    #[test]
    fn each_page_shows_its_own_pick_among_its_lists() {
        let lists = vec!["todo.shop".to_string(), "todo.work".to_string()];
        assert_eq!(shown_list(&lists, 1), Some((1, "todo.work".to_string())));
        // a pick past the page's lists (a list taken off it): its last one; none picked yet: the first
        assert_eq!(shown_list(&lists, 5), Some((1, "todo.work".to_string())));
        assert_eq!(shown_list(&lists, -1), Some((0, "todo.shop".to_string())));
        assert_eq!(shown_list(&[], 0), None);
    }

    #[test]
    fn another_lists_adds_wait() {
        let now = Instant::now();
        let mut adds = vec![added("Milk", now)];
        let mut items = vec![item("1", "Eggs", false)];
        let (moves, _, again) = place_added("todo.b", &mut items, &mut adds, true, now);
        assert!(moves.is_empty() && !again);
        assert_eq!(adds.len(), 1);
        assert_eq!(items.len(), 1);
    }
}
