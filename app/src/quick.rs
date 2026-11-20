//! Quick controls page: the entities picked for it in the integration, as
//! tiles, a mixing desk (faders) or a flight deck (levers). A tap toggles; a
//! long press on a tile, a desk tape or a deck plate opens the control's
//! details in the look's own menu, and the quick shade's page row opens the
//! page's settings (spec 41).

use std::cell::RefCell;
use std::collections::HashMap;

use serde_json::{json, Value};
use slint::{Color, Model, ModelRc, SharedString, VecModel};

use crate::ha::EntityState;
use crate::{icons, texts, with_state, AppWindow, QuickDetails, QuickGroup, QuickTile};

/// The deck lamp of everything that is not a colour light.
fn green() -> Color {
    Color::from_rgb_u8(0x69, 0xdb, 0x7c)
}

/// Whether a light can show colours or colour temperatures (more than on / off and brightness).
fn colour_capable(modes: Option<&Value>) -> bool {
    modes.and_then(Value::as_array).is_some_and(|m| m.iter().filter_map(Value::as_str).any(|m| !matches!(m, "onoff" | "brightness")))
}

/// The tiles for the page's entities, in their order (the grid of the 4 x 4
/// look: wide cards for lights and covers, icon cells for the rest).
pub fn tiles(states: &HashMap<String, EntityState>, entities: &[String], accent: Color) -> Vec<QuickTile> {
    let (mut col, mut row) = (0i32, 0i32);
    entities
        .iter()
        .map(|id| {
            let st = states.get(id);
            let domain = id.split('.').next().unwrap_or("");
            let state = st.map(|s| s.state.as_str()).unwrap_or("unavailable");
            let attr = |k: &str| st.and_then(|s| s.attrs.get(k));
            let num = |k: &str| attr(k).and_then(Value::as_f64);
            let name = attr("friendly_name").and_then(Value::as_str).unwrap_or(id).to_string();
            let gone = matches!(state, "unavailable" | "unknown");
            let kind = match domain {
                "light" => 0,
                "cover" => 2,
                "climate" => 3,
                "vacuum" => 4,
                _ => 1,
            };
            let on = match domain {
                "cover" => matches!(state, "open" | "opening"),
                "climate" => state != "off" && !gone,
                "vacuum" => matches!(state, "cleaning" | "returning"),
                _ => matches!(state, "on" | "playing" | "home"),
            };
            let (dim, value, short) = match domain {
                "light" if num("brightness").is_some() || on => {
                    let b = num("brightness").unwrap_or(0.0) / 255.0;
                    (true, b as f32, format!("{:.0} %", b * 100.0))
                }
                "cover" if num("current_position").is_some() => {
                    let p = num("current_position").unwrap_or(0.0);
                    (true, (p / 100.0) as f32, format!("{p:.0} %"))
                }
                "climate" if num("temperature").is_some() => {
                    let t = num("temperature").unwrap_or(0.0);
                    let (lo, hi) = (num("min_temp").unwrap_or(7.0), num("max_temp").unwrap_or(35.0));
                    (true, ((t - lo) / (hi - lo).max(1.0)).clamp(0.0, 1.0) as f32, format!("{t:.1} °C").replace(".0 °C", " °C"))
                }
                _ => (false, 0.0, String::new()),
            };
            let words = match (domain, state) {
                (_, "unavailable") | (_, "unknown") => texts::fresh("quick.gone"),
                ("cover", "open") => "Open".into(),
                ("cover", "closed") => "Closed".into(),
                ("cover", "opening") => "Opening".into(),
                ("cover", "closing") => "Closing".into(),
                ("climate", s) => {
                    let mode = s.replace('_', " ");
                    let mode = mode[..1].to_uppercase() + &mode[1..];
                    if dim && on { format!("{mode} · {short}") } else { mode }
                }
                (_, "on") if dim => format!("On · {short}"),
                (_, "on") => "On".into(),
                (_, "off") => "Off".into(),
                (_, s) => {
                    let s = s.replace('_', " ");
                    s[..1].to_uppercase() + &s[1..]
                }
            };
            // A colour light glows in its own colour; everything else in the accent.
            let rgb = attr("rgb_color").and_then(Value::as_array).filter(|_| on).and_then(|c| {
                let v: Vec<u8> = c.iter().filter_map(Value::as_u64).map(|x| x.min(255) as u8).collect();
                (v.len() == 3).then(|| Color::from_rgb_u8(v[0], v[1], v[2]))
            });
            let tint = rgb.unwrap_or(accent);
            let colour_light = domain == "light" && colour_capable(attr("supported_color_modes"));
            let (led, led_known) = lamp(id, on, rgb, colour_light);
            let icon = attr("icon")
                .and_then(Value::as_str)
                .and_then(icons::glyph)
                .or_else(|| icons::glyph(icons::default_icon(domain, state, attr("device_class").and_then(Value::as_str))))
                .map(String::from)
                .unwrap_or_default();
            let w = if matches!(kind, 0 | 2 | 3) { 2 } else { 1 };
            if col + w > 4 {
                col = 0;
                row += 1;
            }
            let tile = QuickTile {
                name: name.into(),
                label: SharedString::new(),
                icon: icon.into(),
                kind,
                on,
                dim,
                value,
                state: words.into(),
                short: short.into(),
                gone,
                tint,
                led,
                led_known,
                col,
                row,
                w,
                tape: Default::default(),
                has_tape: false,
            };
            col += w;
            tile
        })
        .collect()
}

/// The colour lights' last colours: seen while on, and kept on the panel (`quick-colours.json` beside the settings,
/// written when such a light is seen off with a colour that is not kept yet), so a lever throw lights the lamp in the
/// light's own colour at once, also after an app restart.
#[derive(Default)]
struct Lamps {
    loaded: bool,
    seen: HashMap<String, Color>,
    kept: HashMap<String, [u8; 3]>,
}

impl Lamps {
    fn path() -> std::path::PathBuf {
        crate::settings::beside("quick-colours.json")
    }

    fn load(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let Some(map) = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str::<HashMap<String, [u8; 3]>>(&t).ok()) else {
            return;
        };
        for (id, c) in &map {
            self.seen.entry(id.clone()).or_insert(Color::from_rgb_u8(c[0], c[1], c[2]));
        }
        self.kept = map;
    }

    fn keep(&mut self, id: &str, c: Color) {
        let rgb = [c.red(), c.green(), c.blue()];
        if self.kept.get(id) == Some(&rgb) {
            return;
        }
        self.kept.insert(id.to_string(), rgb);
        let p = Self::path();
        let tmp = p.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string(&self.kept) {
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
}

/// The deck lamp's colour and whether it is known: a colour light's own colour (its last one while it is off), green
/// for everything else; a colour light the panel has never seen on has none yet (its lamp waits for the light).
fn lamp(id: &str, on: bool, rgb: Option<Color>, colour_light: bool) -> (Color, bool) {
    LAMPS.with(|l| {
        let mut l = l.borrow_mut();
        l.load();
        if let Some(c) = rgb {
            l.seen.insert(id.to_string(), c);
        }
        let last = l.seen.get(id).copied();
        if let (false, true, Some(c)) = (on, colour_light, last) {
            l.keep(id, c);
        }
        match last {
            Some(c) => (c, true),
            None => (green(), !colour_light),
        }
    })
}

/// A picture of the UI bundle (page assets the app sets as properties).
pub fn picture(rel: &str) -> slint::Image {
    let path = std::path::Path::new(env!("PANELOS_UI_PICTURES")).join(rel);
    slint::Image::load_from_path(&path).unwrap_or_default()
}

/// The tiles a quick controls page shows at most (the integration takes no more; more go on a page of their own).
pub const MAX_TILES: usize = 8;

/// Every quick controls page from the states (only when the panel has one), each from its own entities and options.
pub fn refresh(ui: &AppWindow, s: &mut crate::UiState) {
    let pages = crate::pages::kind_pages(s.panel_config.as_ref(), "quick").into_iter().map(|(_, p)| p.clone()).collect::<Vec<Value>>();
    if pages.is_empty() {
        // (a quick controls page that is not on the integration's list has no tiles)
        crate::pages::keep_insts(&ui.get_quick_inst(), 0);
        return;
    }
    // the deck's placard may name the panel's room (lines that need it wait until the panel knows its room)
    let room = crate::config_str(s, "room").to_uppercase();
    ui.set_quick_placard(texts::kept_if("quick.placard", 0, |l| !l.contains("{ROOM}") || !room.is_empty()).replace("{ROOM}", &room).into());
    if ui.get_quick_screw().size().width == 0 {
        // The desk's and the deck's panels and parts (loaded once).
        ui.set_quick_mixer_panel(picture("quick/mixer-panel.png"));
        ui.set_quick_fader(picture("quick/fader.png"));
        ui.set_quick_toggle_up(picture("quick/toggle3d-up.png"));
        ui.set_quick_toggle_down(picture("quick/toggle3d-down.png"));
        // the lever's throw: 9 frames, down … up
        let frames: Vec<slint::Image> = (0..9).map(|k| picture(&format!("quick/toggle3d-{k}.png"))).collect();
        if frames.iter().all(|f| f.size().width > 0) {
            ui.set_quick_lever_frames(ModelRc::new(VecModel::from(frames)));
        }
        ui.set_quick_lamp_off(picture("quick/lamp-off.png"));
        ui.set_quick_lamp_glow(picture("quick/lamp-glow.png"));
        ui.set_quick_lamp_spec(picture("quick/lamp-spec.png"));
        ui.set_quick_lamp_amber(picture("quick/lamp-amber.png"));
        ui.set_quick_screw(picture("quick/screw.png"));
    }
    // the desk's settings menu: "SETTINGS" on the same tape
    if crate::page_looks(ui, "quick").contains(&1) && crate::draw::AVAILABLE && ui.get_quick_settings_tape().size().width == 0 {
        let job = crate::draw::Job::DeskTape { text: "SETTINGS".into(), tint: crate::draw::Tint::Blue, seed: crate::draw::seed("SETTINGS"), width: 140 };
        ui.set_quick_settings_tape(crate::draw::get(job));
    }
    for (n, page) in pages.iter().enumerate() {
        let data = page_data(s, page, &room);
        crate::pages::with_inst(ui.get_quick_inst(), |m| ui.set_quick_inst(m), n, |d| {
            let QuickData { list, title, mood, groups, deck } = data;
            if d.title.as_str() != title {
                d.title = title.into();
            }
            if d.mood.as_str() != mood {
                d.mood = mood.into();
            }
            if let Some(m) = crate::pages::update_rows(&d.tiles, list) {
                d.tiles = m;
            }
            if let Some(groups) = groups {
                if !same_rows(&d.groups, &groups) {
                    d.groups = ModelRc::new(VecModel::from(groups));
                }
            }
            if let Some(deck) = deck {
                d.deck_panel = deck;
            }
        });
    }
    crate::pages::keep_insts(&ui.get_quick_inst(), pages.len());
    if ui.get_quick_sheet() {
        details(ui, s, ui.get_quick_sheet_index().max(0) as usize);
    }
}

/// What one quick controls page shows: its tiles, title, line under the tiles, the deck's group frames (None: not the
/// flight deck) and the deck's plate when it needs another one.
struct QuickData {
    list: Vec<QuickTile>,
    title: String,
    mood: String,
    groups: Option<Vec<QuickGroup>>,
    deck: Option<slint::Image>,
}

/// One quick controls page from its own entities, look and options.
fn page_data(s: &crate::UiState, page: &Value, room: &str) -> QuickData {
    let look = crate::pages::variant_of(page);
    let option = |k: &str| crate::pages::option_of(page, k);
    let entities: Vec<String> = crate::pages::entities_of(page).into_iter().take(MAX_TILES).collect();
    let mut list = tiles(&s.states, &entities, crate::accent(s));
    // the flight deck's plate: its finger wear under the levers' places (they are centred by their number)
    let levers = list.len().clamp(1, 8);
    let deck = (look == 2).then(|| DECKS.with(|d| d.borrow_mut().entry(levers).or_insert_with(|| picture(&format!("quick/deck-panel-{levers}.png"))).clone()));
    // the mixing desk: a tiny label tape under each fader, as wide as its strip (the integration's label, else the
    // name without the room), coloured by kind; the flight deck's plates show the same short text
    if look == 1 || look == 2 {
        let own = option("tapes").and_then(Value::as_array).cloned().unwrap_or_default();
        let width = tape_width(list.len());
        for (i, (t, id)) in list.iter_mut().zip(&entities).enumerate() {
            let text = own
                .get(i)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| short_name(&t.name, room));
            let tint = match id.split('.').next().unwrap_or("") {
                "climate" | "fan" | "water_heater" => crate::draw::Tint::Green,
                "media_player" => crate::draw::Tint::Red,
                _ => crate::draw::Tint::Blue,
            };
            t.label = text.clone().into();
            if look == 1 && crate::draw::AVAILABLE {
                let seed = crate::draw::seed(id);
                t.tape = crate::draw::get(crate::draw::Job::DeskTape { text, tint, seed, width });
                t.has_tape = t.tape.size().width > 0;
            }
        }
    }
    // the flight deck's painted group frames
    let groups = (look == 2).then(|| {
        let mode = option("groups").and_then(Value::as_str).unwrap_or_default().to_string();
        let starts: Vec<bool> =
            option("group_starts").and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_bool().unwrap_or(false)).collect()).unwrap_or_default();
        let names: Vec<String> =
            option("group_names").and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
        let domains: Vec<&str> = entities.iter().map(|id| id.split('.').next().unwrap_or("")).collect();
        // groups off: one frame with the panel's room, or a line for a panel that does not know where it is
        let one = if room.is_empty() { texts::kept("quick.lost.group", 0) } else { room.to_string() };
        frames(&groupings(&domains, &mode, &starts, &names, &one))
            .into_iter()
            .map(|f| QuickGroup { row: f.row as i32, start: f.start as i32, count: f.count as i32, name: f.name.into() })
            .collect()
    });
    QuickData {
        title: texts::kept(if look == 2 { "quick.titles.deck" } else { "quick.titles" }, 0),
        mood: mood(&list),
        list,
        groups,
        deck,
    }
}

/// Whether a model holds exactly these rows.
fn same_rows<T: Clone + PartialEq + 'static>(m: &ModelRc<T>, rows: &[T]) -> bool {
    m.row_count() == rows.len() && rows.iter().enumerate().all(|(i, r)| m.row_data(i).as_ref() == Some(r))
}

/// The desk tape's widest visible strip for `n` channels: the strip's step (8 fill the desk, few get up to 84 px),
/// less a little room between neighbours.
fn tape_width(n: usize) -> u32 {
    ((448.0 / n.max(1) as f32).min(84.0) - 4.0).round().max(36.0) as u32
}

/// An effect's name as words: underscores to spaces, the first letter capital.
fn effect_name(e: &str) -> String {
    let words = e.replace('_', " ");
    let mut c = words.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

thread_local! {
    /// The colour lights' last colours (seen and kept).
    static LAMPS: RefCell<Lamps> = RefCell::new(Lamps::default());
    /// The effects of the light whose details are open (Home Assistant's own names).
    static EFFECTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// The deck's plates, by the number of levers (loaded once each).
    static DECKS: RefCell<HashMap<usize, slint::Image>> = RefCell::new(HashMap::new());
}

/// A control's label when none is set in the integration: its whole name, without the panel's room in front
/// ("Living room shutter left" -> "SHUTTER LEFT"); the desk's tape and the deck's plate take it in smaller letters or
/// in two lines when it is long.
fn short_name(name: &str, room: &str) -> String {
    let n = name.trim();
    let r = room.trim().to_lowercase();
    let rest: String = if !r.is_empty() && n.to_lowercase().starts_with(&format!("{r} ")) {
        n.chars().skip(r.chars().count() + 1).collect()
    } else {
        n.to_string()
    };
    rest.trim().to_uppercase()
}

/// The line under the tiles: all off, some on, all on.
pub fn mood(tiles: &[QuickTile]) -> String {
    let live: Vec<&QuickTile> = tiles.iter().filter(|t| !t.gone).collect();
    if live.is_empty() {
        return String::new();
    }
    let on = live.iter().filter(|t| t.on).count();
    texts::kept(
        if on == 0 {
            "quick.off"
        } else if on == live.len() {
            "quick.allon"
        } else {
            "quick.some"
        },
        0,
    )
}

// ---------------------------------------------------------------------------- the flight deck's groups

/// A painted frame over neighbouring levers of one row of the flight deck: the row, its first cell in the row, how
/// many cells, its name.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub row: usize,
    pub start: usize,
    pub count: usize,
    pub name: String,
}

/// The deck's two rows for `n` levers (at most 8): up to four in a row (5 = 3 + 2, 6 = 3 + 3, 7 = 4 + 3).
pub fn deck_rows(n: usize) -> (usize, usize) {
    let n = n.min(8);
    let r0 = if n <= 4 {
        n
    } else if n <= 6 {
        3
    } else {
        4
    };
    (r0, n - r0)
}

/// The name of an automatic group: what its controls are.
fn group_name(domain: &str) -> &'static str {
    match domain {
        "light" => "LIGHTS",
        "cover" => "SHUTTERS",
        "climate" | "fan" | "water_heater" | "humidifier" => "CLIMATE",
        "switch" | "input_boolean" => "SWITCHES",
        "media_player" => "MEDIA",
        "scene" => "SCENES",
        "script" => "SCRIPTS",
        "vacuum" => "CLEANING",
        "sensor" => "SENSORS",
        _ => "DEVICES",
    }
}

/// Each lever's group (an id and its name; none: no frame), in the user's order:
/// - automatic ("auto", the default): neighbours of one kind (LIGHTS, SHUTTERS, CLIMATE, SWITCHES, MEDIA …);
/// - "off": one group over the top row, named `one` (the panel's room);
/// - "custom": a group starts at the first lever and wherever `starts` says so, named by `names` at its first lever
///   (an empty name: those levers have no frame).
pub fn groupings(domains: &[&str], mode: &str, starts: &[bool], names: &[String], one: &str) -> Vec<Option<(usize, String)>> {
    let n = domains.len().min(8);
    let (r0, _) = deck_rows(n);
    match mode {
        "off" => (0..n).map(|i| (i < r0 && !one.is_empty()).then(|| (0, one.to_string()))).collect(),
        "custom" => {
            let (mut id, mut name) = (0usize, String::new());
            (0..n)
                .map(|i| {
                    if i == 0 || starts.get(i).copied().unwrap_or(false) {
                        id += 1;
                        name = names.get(i).map(|s| s.trim().to_uppercase()).unwrap_or_default();
                    }
                    (!name.is_empty()).then(|| (id, name.clone()))
                })
                .collect()
        }
        _ => {
            let (mut id, mut last) = (0usize, "");
            domains[..n]
                .iter()
                .map(|d| {
                    let g = group_name(d);
                    if id == 0 || g != last {
                        id += 1;
                        last = g;
                    }
                    Some((id, g.to_string()))
                })
                .collect()
        }
    }
}

/// The frames of the levers' groups: a group's run of neighbours in a row; at the end of the top row the frame stops
/// and the bottom row starts a new one with the same name.
pub fn frames(levers: &[Option<(usize, String)>]) -> Vec<Frame> {
    let n = levers.len().min(8);
    let (r0, _) = deck_rows(n);
    let mut out = Vec::new();
    for (row, from, to) in [(0, 0, r0), (1, r0, n)] {
        let mut k = from;
        while k < to {
            let Some((id, name)) = &levers[k] else {
                k += 1;
                continue;
            };
            let mut e = k + 1;
            while e < to && levers[e].as_ref().map(|(i, _)| i) == Some(id) {
                e += 1;
            }
            out.push(Frame { row, start: k - from, count: e - k, name: name.clone() });
            k = e;
        }
    }
    out
}

// ---------------------------------------------------------------------------- taps

/// Taps on the page (`quick.*`): the entity at the tile's index.
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    let (entities, states) = with_state(|s| (crate::page_entities(s, "quick"), s.states.clone()));
    let index = |v: &str| v.split(':').next().and_then(|i| i.parse::<usize>().ok());
    let arg = |v: &str| v.split(':').nth(1).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
    let id_of = |v: &str| index(v).and_then(|i| entities.get(i)).cloned();
    match what {
        "quick.toggle" => {
            if let Some(id) = id_of(value) {
                toggle(&id, &states);
            }
        }
        "quick.alloff" => {
            for id in &entities {
                let domain = id.split('.').next().unwrap_or("");
                match domain {
                    "light" | "switch" | "fan" | "input_boolean" | "media_player" | "climate" => {
                        crate::call(domain, "turn_off", id, json!({}))
                    }
                    _ => {}
                }
            }
        }
        // a long press: the control's details in the look's menu
        "quick.details" => {
            let Some(i) = index(value) else { return };
            with_state(|s| details(ui, s, i));
            ui.set_quick_settings(false);
            ui.set_quick_sheet(true);
        }
        // the page's settings (the quick shade's page row opens them through PageMenu; this is the same from outside)
        "quick.settings" => {
            ui.set_quick_sheet(false);
            ui.set_quick_settings(true);
        }
        "quick.close" => {
            ui.set_quick_sheet(false);
            ui.set_quick_settings(false);
        }
        // the look chosen in the page's settings (0 tiles, 1 mixing desk, 2 flight deck), kept in the integration
        "quick.look" => {
            let look = value.parse::<i32>().unwrap_or(0).clamp(0, 2);
            crate::page_set(ui, "quick", Some(look), Value::Null);
            with_state(|s| refresh(ui, s));
        }
        // value "index:level" with the level 0 … 1
        "quick.level" => {
            let Some(id) = id_of(value) else { return };
            let level = arg(value).clamp(0.0, 1.0);
            let domain = id.split('.').next().unwrap_or("");
            match domain {
                "light" if level <= 0.005 => crate::call("light", "turn_off", &id, json!({})),
                "light" => crate::call("light", "turn_on", &id, json!({"brightness_pct": (level * 100.0).round().max(1.0)})),
                "cover" => crate::call("cover", "set_cover_position", &id, json!({"position": (level * 100.0).round()})),
                "climate" => {
                    let st = states.get(&id);
                    let num = |k: &str| st.and_then(|s| s.attrs.get(k)).and_then(Value::as_f64);
                    let (lo, hi) = (num("min_temp").unwrap_or(7.0), num("max_temp").unwrap_or(35.0));
                    let step = num("target_temp_step").unwrap_or(0.5);
                    let t = ((lo + level * (hi - lo)) / step).round() * step;
                    crate::call("climate", "set_temperature", &id, json!({"temperature": t}));
                }
                _ => toggle(&id, &states),
            }
        }
        // colour temperature 0 (warm) … 1 (cool)
        "quick.temp" => {
            let Some(id) = id_of(value) else { return };
            let st = states.get(&id);
            let num = |k: &str| st.and_then(|s| s.attrs.get(k)).and_then(Value::as_f64);
            let (warm, cool) = (num("min_color_temp_kelvin").unwrap_or(2000.0), num("max_color_temp_kelvin").unwrap_or(6500.0));
            let k = warm + arg(value).clamp(0.0, 1.0) * (cool - warm);
            crate::call("light", "turn_on", &id, json!({"color_temp_kelvin": k.round()}));
        }
        "quick.colour" => {
            let Some(id) = id_of(value) else { return };
            let c = PALETTE[(arg(value) as usize).min(PALETTE.len() - 1)];
            crate::call("light", "turn_on", &id, json!({"rgb_color": [c >> 16, (c >> 8) & 255, c & 255]}));
        }
        "quick.effect" => {
            let Some(id) = id_of(value) else { return };
            let k = arg(value) as usize;
            if let Some(effect) = EFFECTS.with(|e| e.borrow().get(k).cloned()) {
                crate::call("light", "turn_on", &id, json!({"effect": effect}));
            }
        }
        // 0 open, 1 stop, 2 close
        "quick.cover" => {
            let Some(id) = id_of(value) else { return };
            let service = match arg(value) as i32 {
                0 => "open_cover",
                1 => "stop_cover",
                _ => "close_cover",
            };
            crate::call("cover", service, &id, json!({}));
        }
        _ => eprintln!("nspanel-app: act {what} {value} (not handled)"),
    }
}

fn toggle(id: &str, states: &HashMap<String, EntityState>) {
    let domain = id.split('.').next().unwrap_or("");
    match domain {
        "scene" => crate::call("scene", "turn_on", id, json!({})),
        "script" => crate::call("script", "turn_on", id, json!({})),
        "cover" => crate::call("cover", "toggle", id, json!({})),
        "vacuum" => {
            let cleaning = states.get(id).is_some_and(|s| s.state == "cleaning");
            crate::call("vacuum", if cleaning { "return_to_base" } else { "start" }, id, json!({}));
        }
        "climate" => {
            let off = states.get(id).is_none_or(|s| s.state == "off");
            crate::call("climate", if off { "turn_on" } else { "turn_off" }, id, json!({}));
        }
        _ => crate::call("homeassistant", "toggle", id, json!({})),
    }
}

/// Colours offered for colour lights in the details.
const PALETTE: [u32; 7] = [0xffd8a8, 0xff6b6b, 0xffa94d, 0xffe066, 0x69db7c, 0x4dabf7, 0xb197fc];

/// The palette colour nearest to a light's colour, if it is near enough to count as that one.
fn nearest_colour(rgb: [u8; 3]) -> Option<usize> {
    let d = |c: u32| {
        let p = [(c >> 16) as i32, ((c >> 8) & 255) as i32, (c & 255) as i32];
        (0..3).map(|k| (p[k] - rgb[k] as i32).pow(2)).sum::<i32>()
    };
    let (k, best) = PALETTE.iter().enumerate().map(|(k, c)| (k, d(*c))).min_by_key(|(_, v)| *v)?;
    (best <= 60 * 60).then_some(k)
}

/// The details of control `i`: what it can do (its tile from the page, with its label and tape).
pub fn details(ui: &AppWindow, s: &crate::UiState, i: usize) {
    let Some(tile) = crate::pages::inst(&ui.get_quick_inst(), crate::acting_n("quick")).tiles.row_data(i) else { return };
    let entities = crate::page_entities(s, "quick");
    let Some(id) = entities.get(i) else { return };
    let domain = id.split('.').next().unwrap_or("");
    let st = s.states.get(id);
    let state = st.map(|e| e.state.as_str()).unwrap_or("unavailable");
    let attr = |k: &str| st.and_then(|e| e.attrs.get(k));
    let num = |k: &str| attr(k).and_then(Value::as_f64);
    let modes: Vec<String> = attr("supported_color_modes")
        .and_then(Value::as_array)
        .map(|m| m.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    // the level: brightness (not for an on / off light), a cover's position, a thermostat's set temperature
    let level = match domain {
        "light" => (modes.is_empty() && num("brightness").is_some()) || modes.iter().any(|m| m != "onoff"),
        "cover" => num("current_position").is_some(),
        "climate" => num("temperature").is_some(),
        _ => false,
    };
    let has_temp = modes.iter().any(|m| m == "color_temp");
    let (k_lo, k_hi) = (num("min_color_temp_kelvin").unwrap_or(2000.0), num("max_color_temp_kelvin").unwrap_or(6500.0));
    let temp = num("color_temp_kelvin").map(|k| ((k - k_lo) / (k_hi - k_lo).max(1.0)).clamp(0.0, 1.0)).unwrap_or(0.5);
    let colourful = modes.iter().any(|m| matches!(m.as_str(), "hs" | "rgb" | "rgbw" | "rgbww" | "xy"));
    // the palette colour it shows (only while it shows a colour, not a white)
    let colour = attr("color_mode")
        .and_then(Value::as_str)
        .filter(|m| matches!(*m, "hs" | "rgb" | "rgbw" | "rgbww" | "xy") && state == "on")
        .and_then(|_| attr("rgb_color").and_then(Value::as_array))
        .and_then(|c| {
            let v: Vec<u8> = c.iter().filter_map(Value::as_u64).map(|x| x.min(255) as u8).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        })
        .and_then(nearest_colour);
    // the light's effects: Home Assistant's names as words ("channel_change" -> "Channel change"; the deck writes
    // them in capitals); the raw names are what the light takes back
    let raw: Vec<String> = attr("effect_list")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let caps = crate::page_look("quick") == 2;
    let effects: Vec<SharedString> = raw
        .iter()
        .map(|e| {
            let words = effect_name(e);
            SharedString::from(if caps { words.to_uppercase() } else { words })
        })
        .collect();
    let effect = attr("effect").and_then(Value::as_str).and_then(|e| raw.iter().position(|r| r == e));
    let d = QuickDetails {
        level,
        unit: if domain == "climate" { 1 } else { 0 },
        lo: num("min_temp").unwrap_or(7.0) as f32,
        hi: num("max_temp").unwrap_or(35.0) as f32,
        step: num("target_temp_step").unwrap_or(0.5) as f32,
        has_temp,
        temp: temp as f32,
        k_lo: k_lo as f32,
        k_hi: k_hi as f32,
        colour: colour.map_or(-1, |k| k as i32),
        effect: effect.map_or(-1, |k| k as i32),
        moving: match (domain, state) {
            ("cover", "opening") => 0,
            ("cover", "closing") => 2,
            _ => -1,
        },
    };
    EFFECTS.with(|e| *e.borrow_mut() = raw);
    // only what changed (a state change elsewhere refreshes the page while the details are open)
    let colours: Vec<Color> = if colourful { PALETTE.iter().map(|c| Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, *c as u8)).collect() } else { Vec::new() };
    if !same_rows(&ui.get_quick_sheet_colours(), &colours) {
        ui.set_quick_sheet_colours(ModelRc::new(VecModel::from(colours)));
    }
    if !same_rows(&ui.get_quick_sheet_effects(), &effects) {
        ui.set_quick_sheet_effects(ModelRc::new(VecModel::from(effects)));
    }
    if ui.get_quick_details() != d {
        ui.set_quick_details(d);
    }
    if ui.get_quick_sheet_index() != i as i32 {
        ui.set_quick_sheet_index(i as i32);
    }
    if ui.get_quick_sheet_tile() != tile {
        ui.set_quick_sheet_tile(tile);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn frame(row: usize, start: usize, count: usize, name: &str) -> Frame {
        Frame { row, start, count, name: name.into() }
    }

    #[test]
    fn rows_of_the_deck() {
        assert_eq!(deck_rows(1), (1, 0));
        assert_eq!(deck_rows(4), (4, 0));
        assert_eq!(deck_rows(5), (3, 2));
        assert_eq!(deck_rows(6), (3, 3));
        assert_eq!(deck_rows(7), (4, 3));
        assert_eq!(deck_rows(12), (4, 4));
    }

    #[test]
    fn automatic_groups_follow_the_kinds_and_stop_at_the_row_end() {
        let d = ["light", "light", "light", "cover", "cover", "climate", "fan", "switch"];
        let f = frames(&groupings(&d, "auto", &[], &[], "HALL"));
        assert_eq!(
            f,
            vec![
                frame(0, 0, 3, "LIGHTS"),
                frame(0, 3, 1, "SHUTTERS"),
                frame(1, 0, 1, "SHUTTERS"),
                frame(1, 1, 2, "CLIMATE"),
                frame(1, 3, 1, "SWITCHES"),
            ]
        );
        // the same kind again later is a group of its own
        let f = frames(&groupings(&["light", "switch", "light"], "", &[], &[], ""));
        assert_eq!(f, vec![frame(0, 0, 1, "LIGHTS"), frame(0, 1, 1, "SWITCHES"), frame(0, 2, 1, "LIGHTS")]);
    }

    #[test]
    fn groups_off_is_one_frame_over_the_top_row() {
        let d = ["light", "cover", "climate", "switch", "media_player"];
        assert_eq!(frames(&groupings(&d, "off", &[], &[], "KITCHEN")), vec![frame(0, 0, 3, "KITCHEN")]);
        assert!(frames(&groupings(&d, "off", &[], &[], "")).is_empty());
    }

    #[test]
    fn own_groups_start_where_the_user_says() {
        let d = ["light", "light", "cover", "cover", "switch", "switch", "light"];
        let starts = [false, false, true, false, true, false, true];
        let n = names(&["Couch", "", "Blinds", "", "", "", "lights"]);
        // 4 + 3: COUCH over 0-1, BLINDS over 2-3 (row 0); the unnamed group (4-5) has no frame; LIGHTS over 6
        assert_eq!(
            frames(&groupings(&d, "custom", &starts, &n, "")),
            vec![frame(0, 0, 2, "COUCH"), frame(0, 2, 2, "BLINDS"), frame(1, 2, 1, "LIGHTS")]
        );
        // two neighbouring groups with the same name keep their own frames
        let f = frames(&groupings(&["light", "light"], "custom", &[false, true], &names(&["A", "a"]), ""));
        assert_eq!(f, vec![frame(0, 0, 1, "A"), frame(0, 1, 1, "A")]);
    }

    #[test]
    fn palette_colour_of_a_light() {
        assert_eq!(nearest_colour([0x4d, 0xab, 0xf7]), Some(5));
        assert_eq!(nearest_colour([0x50, 0xb0, 0xf0]), Some(5));
        assert_eq!(nearest_colour([0x00, 0x00, 0x00]), None);
    }

    #[test]
    fn tape_width_follows_the_strips() {
        assert_eq!(tape_width(8), 52);
        assert_eq!(tape_width(3), 80);
        assert_eq!(tape_width(0), 80);
    }

    #[test]
    fn short_names_drop_the_room() {
        assert_eq!(short_name("Living room shutter left", "living room"), "SHUTTER LEFT");
        assert_eq!(short_name("Desk lamp", "living room"), "DESK LAMP");
    }
}
