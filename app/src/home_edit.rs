//! Editing a weather page's widgets on the panel itself (the integration's "Allow editing on the panel"): a finger
//! resting on the page opens the lock, holding the lock opens the edit mode. Widgets are moved, resized, added,
//! removed, set and put in front of or behind each other there; every change goes to the integration as the page's
//! widget list (the same list its own editor changes), so the panel and Home Assistant show the same layout.
//!
//! The page shows a change at once: the panel's copy of the settings takes it first (and keeps it until Home
//! Assistant sends the settings back with it), then the page is drawn from the settings as always.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::ha::{Command, EntityState};
use crate::{AppWindow, HomeItem, HomeSensor, UiState};

/// How long a finger rests on the page before the lock opens.
const HOLD: Duration = Duration::from_millis(700);
/// How far it may wander meanwhile (px).
const SLOP: f32 = 12.0;
/// How long a change sent to Home Assistant wins over older settings coming back from it.
const PENDING_FOR: Duration = Duration::from_secs(6);

/// The lock's titles and the lines under them (one of each, a different one each time, no repeat until all showed).
const TITLES: [&str; 16] = [
    "Redecorating again?",
    "Moving the furniture around?",
    "Feeling like an interior designer today?",
    "The widgets were comfy where they were.",
    "Big plans for this wall?",
    "Rearranging the fridge magnets?",
    "Shall we shuffle things a bit?",
    "Careful, the clock is ticklish.",
    "Time for a little makeover?",
    "Change is good. Usually.",
    "Feng shui for widgets?",
    "Let's tidy up a bit?",
    "One more tweak, promise?",
    "The pixels are ready when you are.",
    "Home makeover, panel edition?",
    "Sure? The widgets just got settled.",
];
const SUBTITLES: [&str; 12] = [
    "Press and hold the lock, like you mean it.",
    "Hold the lock until it gives up.",
    "Keep holding the lock. It's thinking about it.",
    "Give the lock a long hug and it opens.",
    "Hold the lock. No secret handshake needed.",
    "Hold the lock a little longer than feels polite.",
    "The lock likes long goodbyes. Hold it.",
    "Hold the lock and count to three. Slowly.",
    "Hold the lock until the ring says yes.",
    "One long press on the lock and it's all yours.",
    "Hold the lock. Good things come to those who wait.",
    "Hold the lock. It opens for patient people.",
];

thread_local! {
    static UI: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
    /// The finger on the screen: where it came down and its press number (None once it moved away or lifted).
    static FINGER: Cell<Option<(f32, f32, u64)>> = const { Cell::new(None) };
    static PRESSES: Cell<u64> = const { Cell::new(0) };
    /// The lock's lines shown since the start.
    static SHOWN: RefCell<(Vec<usize>, Vec<usize>)> = const { RefCell::new((Vec::new(), Vec::new())) };
    /// Every sensor a sensor widget names, in the order first seen: a sensor widget's slot in the page's sensor list.
    static SENSORS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// The sensors offered in a sensor widget's menu (the ones the panel knows).
    static CHOICES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// The widgets sent to Home Assistant and not yet back in its settings: (page in its list, widgets, when).
    static PENDING: RefCell<Option<(usize, Value, Instant)>> = const { RefCell::new(None) };
    /// The widget picked in "Add widget", waiting for its place.
    static PLACING: RefCell<Option<Value>> = const { RefCell::new(None) };
}

pub fn init(ui: &AppWindow) {
    UI.with(|u| *u.borrow_mut() = Some(ui.as_weak()));
}

// ---------------------------------------------------------------- the long press

/// Every finger event of the touch screen, before the interface gets it (0 down, 1 move, 2 up): a finger that rests
/// on a weather page opens the lock. It needs no part of the page under it, so it works on the widgets and on the sky
/// alike, and the sky's own taps keep working.
pub fn finger(kind: u8, x: f32, y: f32) {
    match kind {
        0 => {
            let n = PRESSES.get() + 1;
            PRESSES.set(n);
            FINGER.set(Some((x, y, n)));
            slint::Timer::single_shot(HOLD, move || held(n));
        }
        1 => {
            if let Some((x0, y0, _)) = FINGER.get() {
                if (x - x0).hypot(y - y0) > SLOP {
                    FINGER.set(None);
                }
            }
        }
        _ => FINGER.set(None),
    }
}

fn held(n: u64) {
    let Some((_, y, m)) = FINGER.get() else { return };
    if m != n {
        return;
    }
    let Some(ui) = UI.with(|u| u.borrow().as_ref().and_then(slint::Weak::upgrade)) else { return };
    // not over the status bar; only with editing on the panel allowed and nothing else on the page
    if y < 40.0 || !allowed() || !ui.get_home_hold_ok() {
        return;
    }
    FINGER.set(None);
    // the press under the finger lets go, so lifting it taps nothing (a widget would open its page)
    ui.window().dispatch_event(slint::platform::WindowEvent::PointerExited);
    open_lock(&ui);
}

fn allowed() -> bool {
    crate::with_state(|s| s.panel_config.as_ref().and_then(|c| c.get("home_edit")).and_then(Value::as_bool).unwrap_or(false))
}

fn open_lock(ui: &AppWindow) {
    let pick = |n: usize, used: &mut Vec<usize>| {
        if used.len() >= n {
            used.clear();
        }
        let free: Vec<usize> = (0..n).filter(|i| !used.contains(i)).collect();
        let i = free[(random() as usize) % free.len()];
        used.push(i);
        i
    };
    let (t, s) = SHOWN.with(|u| {
        let mut u = u.borrow_mut();
        (pick(TITLES.len(), &mut u.0), pick(SUBTITLES.len(), &mut u.1))
    });
    ui.set_home_lock_title(TITLES[t].into());
    ui.set_home_lock_sub(SUBTITLES[s].into());
    ui.set_home_lock_p(0.0);
    ui.set_home_lock(true);
}

fn random() -> u64 {
    thread_local!(static SEED: Cell<u64> = Cell::new(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1
    ));
    SEED.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        x
    })
}

// ---------------------------------------------------------------- the grid

/// A widget's place on the 4 x 4 grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spot {
    pub col: i32,
    pub row: i32,
    pub w: i32,
    pub h: i32,
}

impl Spot {
    fn of(v: &Value) -> Spot {
        let n = |k: &str, d: i64| v.get(k).and_then(Value::as_i64).unwrap_or(d) as i32;
        Spot { col: n("col", 0), row: n("row", 0), w: n("w", 4), h: n("h", 1) }
    }
    fn covers(&self, o: &Spot) -> bool {
        self.col < o.col + o.w && o.col < self.col + self.w && self.row < o.row + o.h && o.row < self.row + self.h
    }
    fn inside(&self) -> bool {
        self.col >= 0 && self.row >= 0 && self.w >= 1 && self.h >= 1 && self.col + self.w <= 4 && self.row + self.h <= 4
    }
}

/// The smallest size of a widget (cells): the nixie clock 3 x 1 (4 x 1 with its seconds), the LCD 2 x 1, the
/// twin-bell 2 x 2, the date, the next event, the weather details and the clock 2 x 1, the forecasts 3 x 1, the rest
/// 1 x 1 (as the integration's editor has them).
pub fn min_size(kind: i32, style: i32, opts: i32) -> (i32, i32) {
    match kind {
        0 => match style {
            8 => (if opts & 1 == 1 { 4 } else { 3 }, 1),
            10 => (2, 2),
            _ => (2, 1),
        },
        1 | 3 | 11 => (2, 1),
        4..=6 => (3, 1),
        _ => (1, 1),
    }
}

fn min_of(v: &Value) -> (i32, i32) {
    let n = |k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0) as i32;
    min_size(n("kind"), n("style"), n("opts"))
}

/// The cells no widget covers (row by row).
pub fn free_cells(spots: &[Spot]) -> [bool; 16] {
    let mut free = [true; 16];
    for (c, f) in free.iter_mut().enumerate() {
        let cell = Spot { col: c as i32 % 4, row: c as i32 / 4, w: 1, h: 1 };
        *f = !spots.iter().any(|s| s.covers(&cell));
    }
    free
}

/// Where a new widget of w x h goes when the free cell `cell` is tapped: the tapped cell as its top-left corner if it
/// fits there, else the nearest place that holds the tapped cell and covers no other widget; at its smallest size if
/// its own size fits nowhere around the cell; else over the others (the editors point that out).
pub fn place(spots: &[Spot], w: i32, h: i32, min: (i32, i32), cell: i32) -> Spot {
    let (cc, cr) = (cell % 4, cell / 4);
    for (ww, hh) in [(w, h), min] {
        for dy in 0..hh {
            for dx in 0..ww {
                let s = Spot { col: cc - dx, row: cr - dy, w: ww, h: hh };
                if s.inside() && !spots.iter().any(|o| o.covers(&s)) {
                    return s;
                }
            }
        }
    }
    Spot { col: cc.min(4 - w), row: cr.min(4 - h), w, h }
}

/// Forward (+1) and Back (-1): the later widget lies on top, so Forward puts a widget just above the next one it lies
/// on, Back just below the one before; None when there is none that way.
pub fn layer_to(spots: &[Spot], k: usize, dir: i32) -> Option<usize> {
    let me = spots.get(k)?;
    let over = spots.iter().enumerate().filter(|(i, s)| *i != k && s.covers(me)).map(|(i, _)| i);
    if dir > 0 { over.filter(|i| *i > k).min() } else { over.filter(|i| *i < k).max() }
}

// ---------------------------------------------------------------- the page's widgets in the settings

/// The pager's page in the integration's page list (the panel leaves out the kinds it does not know; a temporary
/// page is in the pager only).
fn config_index(s: &UiState, page: usize) -> Option<usize> {
    let page = match s.temp_page {
        Some(t) if page == t => return None,
        Some(t) if page > t => page - 1,
        _ => page,
    };
    let pages = s.panel_config.as_ref()?.get("pages")?.as_array()?;
    pages
        .iter()
        .enumerate()
        .filter(|(_, p)| p.get("kind").and_then(Value::as_str).is_some_and(|k| crate::pages::KINDS.contains(&k)))
        .nth(page)
        .map(|(i, _)| i)
}

fn items_of(p: &Value) -> Option<&Vec<Value>> {
    p.get("options")?.get("items")?.as_array()
}

/// The widgets a page shows: its own; a later weather page without its own shows the first one's; the first one
/// without its own shows the panel's layout.
fn shown_items(s: &UiState, ci: usize) -> Vec<Value> {
    let Some(pages) = s.panel_config.as_ref().and_then(|c| c.get("pages")).and_then(Value::as_array) else { return Vec::new() };
    let Some(page) = pages.get(ci) else { return Vec::new() };
    if let Some(a) = items_of(page) {
        return a.clone();
    }
    // the weather details page: its own layout
    if page.get("kind").and_then(Value::as_str) == Some("weather") {
        return crate::pages::weather_items(slint::Color::default()).iter().map(item_json).collect();
    }
    if page.get("kind").and_then(Value::as_str) != Some("home") {
        return Vec::new();
    }
    if let Some(a) = pages.iter().find(|p| p.get("kind").and_then(Value::as_str) == Some("home")).and_then(items_of) {
        return a.clone();
    }
    if let Some(a) = s.panel_config.as_ref().and_then(|c| c.get("home_items")).and_then(Value::as_array) {
        return a.clone();
    }
    crate::pages::home_items(None, slint::Color::default()).iter().map(item_json).collect()
}

fn item_json(it: &HomeItem) -> Value {
    json!({ "kind": it.kind, "col": it.col, "row": it.row, "w": it.w, "h": it.h, "style": it.style, "accent": "",
        "weight": it.weight, "tap": it.tap, "opts": it.opts })
}

fn set_items(s: &mut UiState, ci: usize, items: &[Value]) {
    let Some(p) = s.panel_config.as_mut().and_then(|c| c.get_mut("pages")).and_then(Value::as_array_mut).and_then(|a| a.get_mut(ci)) else {
        return;
    };
    if !p.get("options").is_some_and(Value::is_object) {
        p["options"] = json!({});
    }
    p["options"]["items"] = Value::Array(items.to_vec());
}

/// Settings from Home Assistant: widgets sent from here a moment ago and not in them yet stay (an older copy coming
/// back must not move a widget back for a moment); once they are in, nothing is held any more.
pub fn keep_pending(config: &mut Option<Value>) {
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let Some((ci, items, at)) = p.as_ref() else { return };
        if at.elapsed() > PENDING_FOR {
            *p = None;
            return;
        }
        let Some(page) = config.as_mut().and_then(|c| c.get_mut("pages")).and_then(Value::as_array_mut).and_then(|a| a.get_mut(*ci)) else {
            return;
        };
        if items_of(page) == items.as_array() {
            *p = None;
        } else {
            if !page.get("options").is_some_and(Value::is_object) {
                page["options"] = json!({});
            }
            page["options"]["items"] = items.clone();
        }
    });
}

/// The page on screen changes its widgets with `f` (false: nothing changed); the page shows the change at once and,
/// with `send`, Home Assistant gets the page's new widget list.
fn change(ui: &AppWindow, send: bool, f: impl FnOnce(&mut Vec<Value>) -> bool) {
    let page = ui.get_page().max(0) as usize;
    let done = crate::with_state(|s| {
        let ci = config_index(s, page)?;
        let mut items = shown_items(s, ci);
        if !f(&mut items) {
            return None;
        }
        set_items(s, ci, &items);
        Some((ci, items))
    });
    let Some((ci, items)) = done else { return };
    crate::apply_panel_config(ui);
    if send {
        let items = Value::Array(items);
        PENDING.with(|p| *p.borrow_mut() = Some((ci, items.clone(), Instant::now())));
        crate::send(Command::PageSet { index: ci, variant: None, options: json!({ "items": items }) });
    }
    show_free(ui);
}

fn page_items(ui: &AppWindow) -> Vec<Value> {
    let page = ui.get_page().max(0) as usize;
    crate::with_state(|s| config_index(s, page).map(|ci| shown_items(s, ci)).unwrap_or_default())
}

/// What the edit mode shows besides the widgets: the free cells, the open menu's layer row, the neon letters switch.
fn show_free(ui: &AppWindow) {
    let items = page_items(ui);
    let spots: Vec<Spot> = items.iter().map(Spot::of).collect();
    let free = free_cells(&spots).to_vec();
    if !crate::same_rows(&ui.get_home_free(), &free) {
        ui.set_home_free(ModelRc::new(VecModel::from(free)));
    }
    let open = ui.get_home_settings();
    let (back, fwd) = if open >= 0 {
        (layer_to(&spots, open as usize, -1).is_some(), layer_to(&spots, open as usize, 1).is_some())
    } else {
        (false, false)
    };
    ui.set_home_layer_back(back);
    ui.set_home_layer_fwd(fwd);
    // a widget without its own switch glows as the page's old switch says (as the page draws it)
    let page = ui.get_page().max(0) as usize;
    let page_neon = crate::with_state(|s| {
        let ci = config_index(s, page)?;
        s.panel_config.as_ref()?.get("pages")?.get(ci)?.get("options")?.get("neon")?.as_bool()
    })
    .unwrap_or(true);
    ui.set_home_neon_on(items.iter().any(|i| i.get("neon").and_then(Value::as_bool).unwrap_or(page_neon)));
}

// ---------------------------------------------------------------- the sensor widget

/// The slot of a sensor widget's sensor in the page's sensor list (`style` of the widget on the panel).
pub fn sensor_slot(item: &Value) -> i32 {
    let entity = item.get("entity").and_then(Value::as_str).unwrap_or("").to_string();
    if entity.is_empty() {
        return -1;
    }
    SENSORS.with(|l| {
        let mut l = l.borrow_mut();
        match l.iter().position(|e| *e == entity) {
            Some(i) => i as i32,
            None => {
                l.push(entity);
                l.len() as i32 - 1
            }
        }
    })
}

/// The sensors the weather pages' widgets name (the panel follows them).
pub fn item_entities(s: &UiState) -> Vec<String> {
    let Some(pages) = s.panel_config.as_ref().and_then(|c| c.get("pages")).and_then(Value::as_array) else { return Vec::new() };
    pages
        .iter()
        .filter_map(items_of)
        .flatten()
        .filter_map(|i| i.get("entity").and_then(Value::as_str))
        .filter(|e| e.starts_with("sensor."))
        .map(str::to_string)
        .collect()
}

/// A sensor's value as the widget shows it: whole numbers from 100 up, else one decimal at most.
pub fn sensor_value(state: &str) -> String {
    match state.parse::<f64>() {
        Ok(v) if v.is_finite() => {
            if v.abs() >= 100.0 {
                format!("{}", v.round() as i64)
            } else {
                let t = format!("{:.1}", v);
                t.strip_suffix(".0").map(str::to_string).unwrap_or(t)
            }
        }
        _ => state.to_string(),
    }
}

fn sensor(st: Option<&EntityState>) -> HomeSensor {
    let Some(st) = st else { return HomeSensor::default() };
    let attr = |k: &str| st.attrs.get(k).and_then(Value::as_str).unwrap_or("");
    let ok = !matches!(st.state.as_str(), "unavailable" | "unknown" | "");
    let icon = if attr("icon").is_empty() {
        crate::icons::default_icon("sensor", &st.state, st.attrs.get("device_class").and_then(Value::as_str)).to_string()
    } else {
        attr("icon").to_string()
    };
    HomeSensor {
        name: attr("friendly_name").into(),
        value: if ok { sensor_value(&st.state).into() } else { "–".into() },
        unit: if ok { attr("unit_of_measurement").into() } else { SharedString::new() },
        icon: crate::icons::glyph(&icon).or_else(|| crate::icons::glyph("gauge")).map(|c| c.to_string()).unwrap_or_default().into(),
        ok,
    }
}

/// The page's live parts that come from Home Assistant (the sensor widgets' values), with the pages.
pub fn refresh(ui: &AppWindow, s: &UiState) {
    let list: Vec<HomeSensor> = SENSORS.with(|l| l.borrow().iter().map(|e| sensor(s.states.get(e))).collect());
    if !crate::same_rows(&ui.get_home_sensors(), &list) {
        ui.set_home_sensors(ModelRc::new(VecModel::from(list)));
    }
    // the sensors the panel knows, for a sensor widget's menu and the picker's preview: with a number and a unit
    let mut known: Vec<(&String, &EntityState)> = s
        .states
        .iter()
        .filter(|(k, st)| k.starts_with("sensor.") && st.state.parse::<f64>().is_ok() && st.attrs.get("unit_of_measurement").is_some())
        .collect();
    known.sort_by(|a, b| a.1.attrs.get("friendly_name").and_then(Value::as_str).cmp(&b.1.attrs.get("friendly_name").and_then(Value::as_str)));
    let ids: Vec<String> = known.iter().map(|(k, _)| (*k).clone()).collect();
    if CHOICES.with(|c| *c.borrow() != ids) {
        let names: Vec<SharedString> =
            known.iter().map(|(k, st)| st.attrs.get("friendly_name").and_then(Value::as_str).unwrap_or(k.as_str()).into()).collect();
        ui.set_home_sensor_names(ModelRc::new(VecModel::from(names)));
        ui.set_home_sensor_demo(known.first().map(|(_, st)| sensor(Some(*st))).unwrap_or_default());
        CHOICES.with(|c| *c.borrow_mut() = ids);
    }
}

/// The open menu's sensor widget: its sensor among the choices (-1: none of them).
fn show_sensor_choice(ui: &AppWindow, item: Option<&Value>) {
    let e = item.and_then(|i| i.get("entity")).and_then(Value::as_str).unwrap_or("");
    let at = CHOICES.with(|c| c.borrow().iter().position(|x| x == e)).map_or(-1, |i| i as i32);
    ui.set_home_sensor_sel(at);
}

// ---------------------------------------------------------------- the edit mode

/// Everything of the edit mode closes (leaving the page, the screensaver, Done).
pub fn close(ui: &AppWindow) {
    ui.set_home_lock(false);
    ui.set_home_editing(false);
    ui.set_home_picker(false);
    ui.set_home_settings(-1);
    ui.set_home_bg_sheet(false);
    ui.set_home_place(false);
    ui.set_home_info(false);
    PLACING.with(|p| *p.borrow_mut() = None);
}

fn menus_off(ui: &AppWindow) {
    ui.set_home_picker(false);
    ui.set_home_settings(-1);
    ui.set_home_bg_sheet(false);
    ui.set_home_place(false);
    ui.set_home_info(false);
    PLACING.with(|p| *p.borrow_mut() = None);
}

fn nums(value: &str) -> Vec<i64> {
    value.split(':').map(|v| v.parse::<i64>().unwrap_or(-1)).collect()
}

/// The edit mode's taps and drags (`home.*`).
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    match what {
        "home.lock-close" => ui.set_home_lock(false),
        "home.unlock" => {
            if ui.get_home_lock() {
                ui.set_home_lock(false);
                menus_off(ui);
                ui.set_home_editing(true);
                show_free(ui);
            }
        }
        "home.done" => close(ui),
        "home.info" => ui.set_home_info(!ui.get_home_info()),
        "home.move" => {
            let n = nums(value);
            if n.len() < 3 {
                return;
            }
            change(ui, true, |items| {
                let Some(it) = items.get_mut(n[0] as usize) else { return false };
                let s = Spot::of(it);
                let (col, row) = ((n[1] as i32).clamp(0, 4 - s.w), (n[2] as i32).clamp(0, 4 - s.h));
                if (col, row) == (s.col, s.row) {
                    return false;
                }
                it["col"] = json!(col);
                it["row"] = json!(row);
                true
            });
        }
        "home.size" => {
            let n = nums(value);
            if n.len() < 3 {
                return;
            }
            change(ui, true, |items| {
                let Some(it) = items.get_mut(n[0] as usize) else { return false };
                let s = Spot::of(it);
                let (mw, mh) = min_of(it);
                let (w, h) = ((n[1] as i32).clamp(mw.min(4 - s.col), 4 - s.col), (n[2] as i32).clamp(mh.min(4 - s.row), 4 - s.row));
                if (w, h) == (s.w, s.h) {
                    return false;
                }
                it["w"] = json!(w);
                it["h"] = json!(h);
                true
            });
        }
        "home.remove" => {
            let i = value.parse::<usize>().unwrap_or(usize::MAX);
            ui.set_home_settings(-1);
            change(ui, true, |items| {
                if i >= items.len() {
                    return false;
                }
                items.remove(i);
                true
            });
        }
        "home.settings" => {
            let i = value.parse::<i32>().unwrap_or(-1);
            menus_off(ui);
            show_sensor_choice(ui, page_items(ui).get(i.max(0) as usize));
            ui.set_home_settings(i);
            show_free(ui);
        }
        "home.settings-close" => ui.set_home_settings(-1),
        "home.add" => {
            let open = !ui.get_home_picker();
            menus_off(ui);
            ui.set_home_picker(open);
        }
        "home.add-close" => ui.set_home_picker(false),
        // a preview picked: "kind:style:w:h:opts" (the sensor widget adds its sensor's place among the choices)
        "home.pick" => {
            let n = nums(value);
            if n.len() < 5 {
                return;
            }
            let mut item = json!({ "kind": n[0], "style": n[1], "w": n[2], "h": n[3], "opts": n[4], "col": 0, "row": 0,
                "accent": "", "weight": 600, "tap": !matches!(n[0], 8 | 10) });
            if n[0] == 8 {
                let e = CHOICES.with(|c| c.borrow().get(n.get(5).copied().unwrap_or(0).max(0) as usize).cloned());
                let Some(e) = e else { return };
                item["entity"] = json!(e);
                item["style"] = json!(0);
            }
            menus_off(ui);
            PLACING.with(|p| *p.borrow_mut() = Some(item));
            show_free(ui);
            ui.set_home_place(true);
        }
        "home.place" => {
            let cell = value.parse::<i32>().unwrap_or(-1).clamp(0, 15);
            let Some(mut item) = PLACING.with(|p| p.borrow_mut().take()) else { return };
            ui.set_home_place(false);
            change(ui, true, |items| {
                let spots: Vec<Spot> = items.iter().map(Spot::of).collect();
                let s = Spot::of(&item);
                let at = place(&spots, s.w, s.h, min_of(&item), cell);
                item["col"] = json!(at.col);
                item["row"] = json!(at.row);
                item["w"] = json!(at.w);
                item["h"] = json!(at.h);
                items.push(item);
                true
            });
        }
        "home.place-cancel" => {
            PLACING.with(|p| *p.borrow_mut() = None);
            ui.set_home_place(false);
        }
        "home.bg" => {
            let open = !ui.get_home_bg_sheet();
            menus_off(ui);
            ui.set_home_bg_sheet(open);
        }
        "home.bg-close" => ui.set_home_bg_sheet(false),
        // a widget's menu: "index:key:value:final" (the weight slider sends as it moves and once more when let go)
        "home.set" => {
            let mut parts = value.splitn(4, ':');
            let (Some(i), Some(key), Some(v)) = (parts.next(), parts.next(), parts.next()) else { return };
            let fin = parts.next() != Some("0");
            let i = i.parse::<usize>().unwrap_or(usize::MAX);
            let key = key.to_string();
            let v = v.to_string();
            let mut entity = None;
            if key == "entity" {
                entity = CHOICES.with(|c| c.borrow().get(v.parse::<usize>().unwrap_or(usize::MAX)).cloned());
                if entity.is_none() {
                    return;
                }
            }
            change(ui, fin, |items| {
                let Some(it) = items.get_mut(i) else { return false };
                set_field(it, &key, &v, entity.as_deref())
            });
            if key == "entity" {
                show_sensor_choice(ui, page_items(ui).get(i));
            }
        }
        "home.layer" => {
            let n = nums(value);
            if n.len() < 2 {
                return;
            }
            let k = n[0] as usize;
            let mut to_index = None;
            change(ui, true, |items| {
                let spots: Vec<Spot> = items.iter().map(Spot::of).collect();
                let Some(to) = layer_to(&spots, k, n[1] as i32) else { return false };
                let w = items.remove(k);
                items.insert(to, w);
                to_index = Some(to);
                true
            });
            if let Some(to) = to_index {
                ui.set_home_settings(to as i32);
                show_free(ui);
            }
        }
        // the synthwave background's own settings (shared by every page with that background)
        "home.scene" => {
            let k = value.parse::<i64>().unwrap_or(0).clamp(0, 3);
            first_home_option(ui, "scene", json!(k));
        }
        "home.cars" => first_home_option(ui, "cars", json!(value == "1")),
        // the neon letters: every widget of the page at once
        "home.neon" => {
            let on = value == "1";
            change(ui, true, |items| {
                for it in items.iter_mut() {
                    it["neon"] = json!(on);
                }
                true
            });
        }
        _ => {}
    }
}

/// A setting of the first weather page (the synthwave scene and its cars): the panel shows it at once and Home
/// Assistant keeps it.
fn first_home_option(ui: &AppWindow, key: &str, v: Value) {
    crate::with_state(|s| {
        let page = s
            .panel_config
            .as_mut()
            .and_then(|c| c.get_mut("pages"))
            .and_then(Value::as_array_mut)
            .and_then(|a| a.iter_mut().find(|p| p.get("kind").and_then(Value::as_str) == Some("home")));
        if let Some(p) = page {
            if !p.get("options").is_some_and(Value::is_object) {
                p["options"] = json!({});
            }
            p["options"][key] = v.clone();
        }
    });
    crate::refresh_pages(ui);
    let mut o = serde_json::Map::new();
    o.insert(key.to_string(), v);
    crate::page_set(ui, "home", None, Value::Object(o));
}

/// One setting of a widget from its menu; false when it is already so.
fn set_field(it: &mut Value, key: &str, v: &str, entity: Option<&str>) -> bool {
    let n = || v.parse::<i64>().unwrap_or(0);
    let old = it.clone();
    match key {
        "style" => {
            it["style"] = json!(n().clamp(0, 10));
            grow_to_min(it);
        }
        "accent" => it["accent"] = json!(if v.len() == 7 && v.starts_with('#') { v } else { "" }),
        "weight" => it["weight"] = json!((n() / 100 * 100).clamp(100, 900)),
        // a switch of the widget: its bit flips
        "opt" => {
            let bit = n().clamp(1, 0x8000);
            let opts = it.get("opts").and_then(Value::as_i64).unwrap_or(0);
            it["opts"] = json!(opts ^ bit);
            grow_to_min(it);
        }
        "tap" => it["tap"] = json!(v == "1"),
        // the music widget's long titles: cut off (0) or scrolling (1), and how fast (1..5, its style)
        "scroll" => {
            let opts = it.get("opts").and_then(Value::as_i64).unwrap_or(0);
            it["opts"] = json!(if v == "1" { opts | 1 } else { opts & !1 });
        }
        "speed" => it["style"] = json!(n().clamp(1, 5)),
        "entity" => it["entity"] = json!(entity.unwrap_or("")),
        _ => return false,
    }
    *it != old
}

/// A style or switch that needs more room (the twin-bell, the nixie's seconds): the widget grows to it where the grid
/// allows.
fn grow_to_min(it: &mut Value) {
    let s = Spot::of(it);
    let (mw, mh) = min_of(it);
    if s.w < mw || s.h < mh {
        let (w, h) = (s.w.max(mw).min(4), s.h.max(mh).min(4));
        it["w"] = json!(w);
        it["h"] = json!(h);
        it["col"] = json!(s.col.min(4 - w));
        it["row"] = json!(s.row.min(4 - h));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(col: i32, row: i32, w: i32, h: i32) -> Spot {
        Spot { col, row, w, h }
    }

    #[test]
    fn free_cells_leave_out_the_widgets() {
        let f = free_cells(&[sp(0, 0, 4, 1), sp(2, 2, 2, 2)]);
        assert!(f[..4].iter().all(|c| !c));
        assert!(f[4] && f[8] && f[9]);
        assert!(!f[10] && !f[11] && !f[14] && !f[15]);
    }

    #[test]
    fn a_new_widget_takes_the_tapped_cell_or_its_nearest_fit() {
        let spots = [sp(0, 0, 4, 1)];
        // a 2 x 1 on cell 5 (col 1, row 1): the tapped cell is its corner
        assert_eq!(place(&spots, 2, 1, (1, 1), 5), sp(1, 1, 2, 1));
        // on the last column it moves left to fit
        assert_eq!(place(&spots, 2, 1, (1, 1), 7), sp(2, 1, 2, 1));
        // a 4 x 1 on the bottom row
        assert_eq!(place(&spots, 4, 1, (2, 1), 13), sp(0, 3, 4, 1));
        // no room at its size: its smallest size
        let full = [sp(0, 0, 4, 1), sp(0, 1, 2, 3), sp(3, 1, 1, 3)];
        assert_eq!(place(&full, 2, 1, (1, 1), 6), sp(2, 1, 1, 1));
        // no room at all: over the others, inside the grid
        assert_eq!(place(&[sp(0, 0, 4, 4)], 4, 1, (2, 1), 15), sp(0, 3, 4, 1));
    }

    #[test]
    fn forward_and_back_pass_the_widgets_it_lies_on() {
        let spots = [sp(0, 0, 4, 2), sp(3, 3, 1, 1), sp(0, 0, 1, 1), sp(1, 1, 2, 1)];
        assert_eq!(layer_to(&spots, 0, 1), Some(2));
        assert_eq!(layer_to(&spots, 3, -1), Some(0));
        assert_eq!(layer_to(&spots, 1, 1), None);
        assert_eq!(layer_to(&spots, 1, -1), None);
        assert_eq!(layer_to(&spots, 2, 1), None);
    }

    #[test]
    fn smallest_sizes_follow_the_integration() {
        assert_eq!(min_size(0, 8, 1), (4, 1));
        assert_eq!(min_size(0, 8, 0), (3, 1));
        assert_eq!(min_size(0, 10, 0), (2, 2));
        assert_eq!(min_size(0, 9, 0), (2, 1));
        assert_eq!(min_size(5, 0, 0), (3, 1));
        assert_eq!(min_size(7, 0, 0), (1, 1));
        assert_eq!(min_size(8, 0, 0), (1, 1));
        assert_eq!(min_size(11, 0, 15), (2, 1));
    }

    #[test]
    fn widget_settings_change_and_grow() {
        let mut it = json!({ "kind": 0, "col": 3, "row": 3, "w": 1, "h": 1, "style": 0, "opts": 0 });
        assert!(set_field(&mut it, "style", "10", None));
        assert_eq!(Spot::of(&it), sp(2, 2, 2, 2));
        assert!(!set_field(&mut it, "style", "10", None));
        assert!(set_field(&mut it, "opt", "4", None));
        assert_eq!(it["opts"], json!(4));
        assert!(set_field(&mut it, "opt", "4", None));
        assert_eq!(it["opts"], json!(0));
        assert!(set_field(&mut it, "weight", "540", None));
        assert_eq!(it["weight"], json!(500));
        assert!(set_field(&mut it, "accent", "#ff7a33", None));
        assert!(set_field(&mut it, "accent", "x", None));
        assert_eq!(it["accent"], json!(""));
    }

    #[test]
    fn sensor_values_read_well() {
        assert_eq!(sensor_value("21.53"), "21.5");
        assert_eq!(sensor_value("21.0"), "21");
        assert_eq!(sensor_value("1234.4"), "1234");
        assert_eq!(sensor_value("on"), "on");
    }
}
