//! Thermostat page: the climate entities picked for it, one on the dial at a
//! time (the device chip opens them all), its modes, target and options.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::ha::EntityState;
use crate::{icons, with_state, AppWindow, ClimateDev, ClimateRoom};

/// HA's hvac mode → the page's mode code.
fn mode_code(mode: &str) -> Option<i32> {
    Some(match mode {
        "off" => 0,
        "heat" => 1,
        "cool" => 2,
        "heat_cool" | "auto" => 3,
        "dry" => 4,
        "fan_only" => 5,
        _ => return None,
    })
}

fn words(s: &str) -> String {
    let s = s.replace('_', " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn strings(a: Option<&Value>) -> Vec<String> {
    a.and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

fn model(list: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(list.iter().map(|s| SharedString::from(words(s))).collect::<Vec<_>>()))
}

fn caps(list: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(list.iter().map(|s| SharedString::from(words(s).to_uppercase())).collect::<Vec<_>>()))
}

/// One device for the dial (and the other looks).
pub fn device(id: &str, st: Option<&EntityState>) -> ClimateDev {
    let attr = |k: &str| st.and_then(|s| s.attrs.get(k));
    let num = |k: &str| attr(k).and_then(Value::as_f64);
    let state = st.map(|s| s.state.as_str()).unwrap_or("unavailable");
    let hvac = strings(attr("hvac_modes"));
    let modes: Vec<i32> = hvac.iter().filter_map(|m| mode_code(m)).collect();
    let current_code = mode_code(state).unwrap_or(0);
    let mode = modes.iter().position(|&m| m == current_code).unwrap_or(0) as i32;
    let action = match attr("hvac_action").and_then(Value::as_str).unwrap_or("") {
        "heating" | "preheating" => 1,
        "cooling" => 2,
        "drying" => 4,
        "fan" => 5,
        _ => 0,
    };
    let target = num("temperature");
    let show_target = target.is_some() && matches!(current_code, 1 | 2 | 3);
    let label = match (current_code, action) {
        (0, _) => "Off".to_string(),
        (_, 1) => "Heating to".into(),
        (_, 2) => "Cooling to".into(),
        (_, 4) => "Drying".into(),
        (_, 5) => "Fan".into(),
        (c, _) => format!("{} · Idle", ["Off", "Heat", "Cool", "Auto", "Dry", "Fan"][c.clamp(0, 5) as usize]),
    };
    let fans = strings(attr("fan_modes"));
    let swings = strings(attr("swing_modes"));
    let presets = strings(attr("preset_modes"));
    let fan = attr("fan_mode").and_then(Value::as_str).unwrap_or("").to_string();
    let swing = attr("swing_mode").and_then(Value::as_str).unwrap_or("").to_string();
    let preset = attr("preset_mode").and_then(Value::as_str).filter(|p| *p != "none").unwrap_or("").to_string();
    let fan_level = match fan.as_str() {
        "low" | "1" => 1,
        "medium_low" | "2" => 2,
        "medium" | "mid" | "3" => 3,
        "medium_high" | "4" => 4,
        "high" | "5" => 5,
        _ => 0,
    };
    let cools = hvac.iter().any(|m| m == "cool");
    let icon = icons::glyph(if cools { "air-conditioner" } else { "radiator" }).map(String::from).unwrap_or_default();
    let step = num("target_temp_step").unwrap_or(if cools { 1.0 } else { 0.5 });
    ClimateDev {
        name: attr("friendly_name").and_then(Value::as_str).unwrap_or(id).into(),
        icon: icon.into(),
        mode,
        modes: ModelRc::new(VecModel::from(modes)),
        action,
        label: label.into(),
        target: target.unwrap_or(0.0) as f32,
        show_target,
        current: num("current_temperature").unwrap_or(0.0) as f32,
        humidity: num("current_humidity").unwrap_or(-1.0) as f32,
        tmin: num("min_temp").unwrap_or(7.0) as f32,
        tmax: num("max_temp").unwrap_or(35.0) as f32,
        step: step as f32,
        fan: if fans.is_empty() { SharedString::new() } else { words(&fan).into() },
        swing: if swings.is_empty() { SharedString::new() } else { words(&swing).into() },
        preset: words(&preset).into(),
        has_preset: !presets.is_empty(),
        fans: model(&fans),
        swings: model(&swings),
        presets: model(&presets),
        fan_level,
        fan_i: fans.iter().position(|f| *f == fan).map(|i| i as i32).unwrap_or(-1),
        swing_i: swings.iter().position(|f| *f == swing).map(|i| i as i32).unwrap_or(-1),
        preset_i: presets.iter().position(|f| *f == preset).map(|i| i as i32).unwrap_or(-1),
        fan_caps: caps(&fans),
        swing_caps: caps(&swings),
        preset_caps: caps(&presets),
    }
}

/// A value to the device's step (a step of 0 or less: as it is).
fn to_step(v: f32, step: f32) -> f32 {
    if step > 0.0 {
        (v / step).round() * step
    } else {
        v
    }
}

/// A temperature as the page writes it: whole degrees without a decimal ("23"), else one decimal ("20.5").
fn deg(t: f32) -> String {
    let tenths = (t * 10.0).round() as i64;
    if tenths % 10 == 0 { format!("{}", tenths / 10) } else { format!("{:.1}", tenths as f32 / 10.0) }
}

const MODE_WORDS: [&str; 6] = ["Off", "Heat", "Cool", "Auto", "Dry", "Fan"];
const ACTION_WORDS: [&str; 6] = ["", "heating", "cooling", "", "drying", "fan"];

/// A device's line in the rooms menu: its mode, the target and what it does now ("Heat 23° · heating",
/// "Heat 20° · idle", "Dry", "Off").
fn device_line(code: i32, action: i32, target: Option<f32>) -> String {
    let code = code.clamp(0, 5) as usize;
    if code == 0 {
        return MODE_WORDS[0].to_string();
    }
    let mut line = MODE_WORDS[code].to_string();
    if let Some(t) = target {
        line += &format!(" {}°", deg(t));
    }
    // what it does, unless the mode already says it (dry: drying, fan only: blowing)
    let busy = if code >= 4 && action as usize == code {
        ""
    } else {
        ACTION_WORDS.get(action.clamp(0, 5) as usize).copied().unwrap_or("")
    };
    if !busy.is_empty() {
        line += &format!(" · {busy}");
    } else if target.is_some() {
        line += " · idle";
    }
    line
}

/// The same for the remote's zone LCD, in short capitals: "HEAT 23" while heating, "IDLE 20" while it waits, "DRY",
/// "OFF".
fn lcd_caps(code: i32, action: i32, target: Option<f32>) -> String {
    let code = code.clamp(0, 5) as usize;
    if code == 0 {
        return "OFF".into();
    }
    match target {
        Some(t) if matches!(action, 1 | 2 | 4 | 5) => format!("{} {}", MODE_WORDS[action as usize].to_uppercase(), deg(t)),
        Some(t) => format!("IDLE {}", deg(t)),
        None => MODE_WORDS[code].to_uppercase(),
    }
}

/// The rooms menu: one card per device (its name, the room's temperature and humidity as the device measures them,
/// what it does).
fn rooms(devices: &[ClimateDev], sel: usize) -> Vec<ClimateRoom> {
    devices
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let code = slint::Model::row_data(&d.modes, d.mode as usize).unwrap_or(0);
            let target = d.show_target.then_some(d.target);
            ClimateRoom {
                name: d.name.clone(),
                temp: format!("{:.1}°", d.current).into(),
                hum: if d.humidity >= 0.0 { format!("{:.0} %", d.humidity).into() } else { SharedString::new() },
                temp_v: d.current,
                hum_v: if d.humidity >= 0.0 { d.humidity.round() } else { -1.0 },
                outside: false,
                dev: ModelRc::new(VecModel::from(vec![SharedString::from(device_line(code, d.action, target))])),
                dev_icon: ModelRc::new(VecModel::from(vec![d.icon.clone()])),
                dev_mode: ModelRc::new(VecModel::from(vec![code])),
                dev_busy: ModelRc::new(VecModel::from(vec![code != 0 && d.action > 0])),
                dev_caps: ModelRc::new(VecModel::from(vec![SharedString::from(lcd_caps(code, d.action, target))])),
                dev_index: ModelRc::new(VecModel::from(vec![i as i32])),
                sel: if i == sel { 0 } else { -1 },
            }
        })
        .collect()
}

/// Every thermostat page from the states (only when the panel has one), each from its own climate entities.
pub fn refresh(ui: &AppWindow, s: &mut crate::UiState) {
    let pages = crate::pages::kind_pages(s.panel_config.as_ref(), "thermostat").into_iter().map(|(_, p)| p.clone()).collect::<Vec<_>>();
    for (n, page) in pages.iter().enumerate() {
        let entities = crate::pages::entities_of(page);
        let devices: Vec<ClimateDev> = entities
            .iter()
            .map(|id| {
                let st = s.states.get(id);
                let mut d = device(id, st);
                // a target set on the panel stays shown until Home Assistant reports it back (see `Setpoint`)
                if let Some(t) = on_page(id, reported(st)) {
                    d.target = t;
                }
                d
            })
            .collect();
        crate::pages::with_inst(ui.get_thermo_inst(), |m| ui.set_thermo_inst(m), n, |d| {
            if devices.is_empty() {
                // none picked (or all gone): the chip says so and opens nothing
                if slint::Model::row_count(&d.devices) > 0 {
                    d.devices = ModelRc::default();
                    d.rooms = ModelRc::default();
                }
                return;
            }
            let sel = (d.sel.max(0) as usize).min(devices.len() - 1);
            let rooms = rooms(&devices, sel);
            if let Some(m) = crate::pages::update_rows(&d.devices, devices) {
                d.devices = m;
            }
            if let Some(m) = crate::pages::update_rows(&d.rooms, rooms) {
                d.rooms = m;
            }
            d.sel = sel as i32;
        });
        // the menus of a page without devices close
        if entities.is_empty() && crate::acting_n("thermostat") == n {
            ui.set_thermo_picker(false);
            ui.set_thermo_sheet(false);
        }
    }
    crate::pages::keep_insts(&ui.get_thermo_inst(), pages.len());
    // a value reported back may let the next one go
    arm(ui);
}

/// − / + presses in a quick row are sent together once the presses pause this long.
const SETTLE: Duration = Duration::from_millis(700);
/// A value sent must come back from Home Assistant within this time, or the page shows Home Assistant's again (a cloud
/// thermostat can report a new target only 20 s later)
const CONFIRM: Duration = Duration::from_secs(30);

fn same(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

/// A target set on the panel (− / + presses, a turned ring or knob) on its way to Home Assistant, one per device.
/// - Every press is one step from the value shown; while a change is under way the page shows the panel's value, so
///   an echo of an older value from Home Assistant neither moves the page nor becomes the base of the next press.
/// - Presses in a quick row go out as one call with the value shown, once the presses pause (`SETTLE`); a turn goes
///   out when the finger lifts.
/// - One call per device at a time: the next value waits until Home Assistant reports the one sent, or `CONFIRM`
///   passes (a device that gets several calls at once may keep only some of them).
/// - A value not reported back within `CONFIRM`: the page shows Home Assistant's value again.
#[derive(Debug, Clone, Default, PartialEq)]
struct Setpoint {
    /// The value on the page while a change is under way.
    shown: f32,
    /// A value not sent yet: when it is due.
    due: Option<Instant>,
    /// The value sent, not reported back yet, and when it went.
    sent: Option<(f32, Instant)>,
    /// The presses gathered into the value not sent yet (for the log).
    presses: u32,
}

/// What a `Setpoint` has to do now.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    /// Nothing yet.
    Wait,
    /// This value goes to Home Assistant.
    Send(f32),
    /// The value sent did not come back: the page shows Home Assistant's value again.
    Back,
    /// Nothing under way any more.
    Done,
}

impl Setpoint {
    fn busy(&self) -> bool {
        self.due.is_some() || self.sent.is_some()
    }

    /// The value the page shows: the panel's own while a change is under way, else Home Assistant's.
    fn shown(&self, reported: Option<f32>) -> Option<f32> {
        if self.busy() {
            Some(self.shown)
        } else {
            reported
        }
    }

    /// One − / + press: `step` from the value shown (Home Assistant's `reported` when nothing is under way), kept in
    /// `lo` … `hi`.
    fn press(&mut self, reported: f32, step: f32, lo: f32, hi: f32, now: Instant) -> f32 {
        let base = if self.busy() { self.shown } else { reported };
        self.shown = (base + step).min(hi).max(lo);
        self.due = Some(now + SETTLE);
        self.presses += 1;
        self.shown
    }

    /// The value of a turned ring or knob: due at once.
    fn set(&mut self, value: f32, now: Instant) {
        self.shown = value;
        self.due = Some(now);
        self.presses = 0;
    }

    /// Home Assistant reports its target: the value sent is through when they match (an older value is not).
    fn report(&mut self, reported: Option<f32>) {
        if let (Some((v, _)), Some(r)) = (self.sent, reported) {
            if same(v, r) {
                self.sent = None;
            }
        }
    }

    /// What is due at `now`, with Home Assistant reporting `reported`.
    fn tick(&mut self, reported: Option<f32>, now: Instant) -> Step {
        self.report(reported);
        let mut lost = false;
        if let Some((_, at)) = self.sent {
            if now < at + CONFIRM {
                return Step::Wait;
            }
            self.sent = None;
            lost = true;
        }
        match self.due {
            Some(due) if now >= due => {
                self.due = None;
                self.presses = 0;
                // back where Home Assistant is (a press and its opposite): nothing to send
                if reported.is_some_and(|r| same(r, self.shown)) {
                    return Step::Done;
                }
                self.sent = Some((self.shown, now));
                Step::Send(self.shown)
            }
            Some(_) => Step::Wait,
            None if lost => Step::Back,
            None => Step::Done,
        }
    }

    /// When `tick` has something to do next.
    fn next(&self) -> Option<Instant> {
        match (self.sent, self.due) {
            (Some((_, at)), _) => Some(at + CONFIRM),
            (None, due) => due,
        }
    }
}

thread_local! {
    /// The targets set on the panel that are under way, by entity.
    static SETPOINTS: RefCell<HashMap<String, Setpoint>> = RefCell::new(HashMap::new());
    /// Wakes the setpoints when one has something to do.
    static TIMER: slint::Timer = slint::Timer::default();
}

/// Home Assistant's target of a climate entity.
fn reported(st: Option<&EntityState>) -> Option<f32> {
    st.and_then(|s| s.attrs.get("temperature")).and_then(Value::as_f64).map(|t| t as f32)
}

/// The target a page shows for `id` when a change was set on it on the panel (None: no such change, Home Assistant's).
fn on_page(id: &str, reported: Option<f32>) -> Option<f32> {
    SETPOINTS.with(|m| {
        let mut m = m.borrow_mut();
        let sp = m.get_mut(id)?;
        sp.report(reported);
        let shown = sp.shown(reported);
        if !sp.busy() {
            m.remove(id);
        }
        shown
    })
}

/// The timer for the setpoint that has something to do first (none: stopped).
fn arm(ui: &AppWindow) {
    let next = SETPOINTS.with(|m| m.borrow().values().filter_map(Setpoint::next).min());
    TIMER.with(|t| match next {
        None => t.stop(),
        Some(at) => {
            let weak = ui.as_weak();
            t.start(slint::TimerMode::SingleShot, at.saturating_duration_since(Instant::now()), move || {
                if let Some(ui) = weak.upgrade() {
                    tick_all(&ui);
                }
            });
        }
    });
}

/// The setpoints' due work: values sent, values that did not come back shown as Home Assistant has them. Not to be
/// called inside `with_state`.
fn tick_all(ui: &AppWindow) {
    let now = Instant::now();
    let ids: Vec<String> = SETPOINTS.with(|m| m.borrow().keys().cloned().collect());
    let now_reported: Vec<Option<f32>> = with_state(|s| ids.iter().map(|id| reported(s.states.get(id))).collect());
    let mut sends = Vec::new();
    let mut back = false;
    SETPOINTS.with(|m| {
        let mut m = m.borrow_mut();
        for (id, r) in ids.iter().zip(now_reported) {
            let Some(sp) = m.get_mut(id) else { continue };
            let (presses, shown) = (sp.presses, sp.shown);
            match sp.tick(r, now) {
                Step::Send(v) => sends.push((id.clone(), v, presses)),
                Step::Back => {
                    let r = r.map(deg).unwrap_or_else(|| "none".into());
                    eprintln!("nspanel-app: thermostat {id}: {} not reported back in {} s; showing {r} again", deg(shown), CONFIRM.as_secs());
                    back = true;
                }
                Step::Wait | Step::Done => {}
            }
        }
        m.retain(|_, sp| sp.busy());
    });
    for (id, v, presses) in sends {
        let from = match presses {
            0 => "turn".to_string(),
            1 => "1 press".into(),
            p => format!("{p} presses"),
        };
        eprintln!("nspanel-app: thermostat {id}: target {} ({from})", deg(v));
        // two decimals at most, so a 0.1 step goes out as 20.1, not as the nearest f32
        call("set_temperature", &id, json!({ "temperature": (v as f64 * 100.0).round() / 100.0 }));
    }
    if back {
        with_state(|s| refresh(ui, s));
    }
    arm(ui);
}

/// Taps on the page (`thermo.*`).
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    // the page's menus: the rooms (the device chip), the options (the pills); all of them close when the page leaves
    // the screen
    match what {
        "thermo.sheet" => return ui.set_thermo_sheet(true),
        "thermo.sheet-close" => return ui.set_thermo_sheet(false),
        "thermo.picker" => return ui.set_thermo_picker(true),
        "thermo.picker-close" => return ui.set_thermo_picker(false),
        "thermo.menus-close" => {
            ui.set_thermo_sheet(false);
            ui.set_thermo_picker(false);
            ui.set_thermo_settings(false);
            return;
        }
        _ => {}
    }
    let (entities, states) = with_state(|s| (crate::page_entities(s, "thermostat"), s.states.clone()));
    // the page on screen's devices and the one it shows
    let n = crate::acting_n("thermostat");
    let sel = crate::pages::inst(&ui.get_thermo_inst(), n).sel.max(0) as usize;
    let Some(id) = entities.get(sel).cloned() else { return };
    let st: Option<&EntityState> = states.get(&id);
    let attr = |k: &str| st.and_then(|s| s.attrs.get(k));
    let idx: usize = value.parse().unwrap_or(0);
    match what {
        // every press one step from the value shown, shown at once; a quick row of presses goes out as one value when
        // the presses pause (see `Setpoint`)
        "thermo.minus" | "thermo.plus" => {
            let d = device(&id, st);
            let step = if what == "thermo.plus" { d.step } else { -d.step };
            let t = SETPOINTS.with(|m| m.borrow_mut().entry(id.clone()).or_default().press(d.target, step, d.tmin, d.tmax, Instant::now()));
            show_target(ui, n, sel, t);
            arm(ui);
        }
        // a value from a turned knob or dial (wall, dial and neon looks), to the device's step: shown, and sent at once
        // (after the value before it, if that one is still on its way)
        "thermo.target" => {
            let d = device(&id, st);
            let v: f32 = value.parse().unwrap_or(d.target);
            let t = to_step(v, d.step).min(d.tmax).max(d.tmin);
            SETPOINTS.with(|m| m.borrow_mut().entry(id.clone()).or_default().set(t, Instant::now()));
            show_target(ui, n, sel, t);
            tick_all(ui);
        }
        "thermo.mode" => {
            let hvac: Vec<String> = strings(attr("hvac_modes")).into_iter().filter(|m| mode_code(m).is_some()).collect();
            if let Some(mode) = hvac.get(idx) {
                call("set_hvac_mode", &id, json!({"hvac_mode": mode}));
            }
        }
        "thermo.fan" | "thermo.swing" | "thermo.preset" => {
            let (key, service, field) = match what {
                "thermo.fan" => ("fan_modes", "set_fan_mode", "fan_mode"),
                "thermo.swing" => ("swing_modes", "set_swing_mode", "swing_mode"),
                _ => ("preset_modes", "set_preset_mode", "preset_mode"),
            };
            if let Some(v) = strings(attr(key)).get(idx) {
                call(service, &id, json!({ field: v }));
            }
        }
        // The remote's keys: power off / on, the next mode (not off), the next
        // fan or swing step, the next preset (one preset: on / off).
        "thermo.key" => {
            let state = st.map(|s| s.state.as_str()).unwrap_or("off");
            let next = |list: Vec<String>, cur: &str| -> Option<String> {
                if list.is_empty() {
                    return None;
                }
                let i = list.iter().position(|v| v == cur).map(|i| (i + 1) % list.len()).unwrap_or(0);
                Some(list[i].clone())
            };
            match idx {
                0 => call(if state == "off" { "turn_on" } else { "turn_off" }, &id, json!({})),
                1 => {
                    let modes: Vec<String> = strings(attr("hvac_modes")).into_iter().filter(|m| m != "off" && mode_code(m).is_some()).collect();
                    if let Some(m) = next(modes, state) {
                        call("set_hvac_mode", &id, json!({ "hvac_mode": m }));
                    }
                }
                2 => {
                    if let Some(f) = next(strings(attr("fan_modes")), attr("fan_mode").and_then(Value::as_str).unwrap_or("")) {
                        call("set_fan_mode", &id, json!({ "fan_mode": f }));
                    }
                }
                3 => {
                    if let Some(f) = next(strings(attr("swing_modes")), attr("swing_mode").and_then(Value::as_str).unwrap_or("")) {
                        call("set_swing_mode", &id, json!({ "swing_mode": f }));
                    }
                }
                _ => {
                    if let Some(p) = next(strings(attr("preset_modes")), attr("preset_mode").and_then(Value::as_str).unwrap_or("")) {
                        call("set_preset_mode", &id, json!({ "preset_mode": p }));
                    }
                }
            }
        }
        // a device picked in the rooms menu (its index among the page's devices)
        "thermo.room" => {
            if idx < entities.len() {
                crate::pages::with_inst(ui.get_thermo_inst(), |m| ui.set_thermo_inst(m), n, |d| d.sel = idx as i32);
            }
            ui.set_thermo_picker(false);
            with_state(|s| refresh(ui, s));
        }
        _ => eprintln!("nspanel-app: act {what} {value} (not handled)"),
    }
}

/// A target set on the panel, shown on its page at once (the device's row in place).
fn show_target(ui: &AppWindow, n: usize, sel: usize, t: f32) {
    let devs = crate::pages::inst(&ui.get_thermo_inst(), n).devices;
    if let Some(mut cur) = slint::Model::row_data(&devs, sel) {
        cur.target = t;
        slint::Model::set_row_data(&devs, sel, cur);
    }
}

fn call(service: &str, id: &str, data: Value) {
    crate::call("climate", service, id, data);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degrees_drop_a_zero_decimal() {
        assert_eq!(deg(23.0), "23");
        assert_eq!(deg(20.5), "20.5");
        assert_eq!(deg(19.96), "20");
        assert_eq!(deg(-2.5), "-2.5");
    }

    #[test]
    fn device_lines() {
        assert_eq!(device_line(1, 1, Some(23.0)), "Heat 23° · heating");
        assert_eq!(device_line(1, 0, Some(20.0)), "Heat 20° · idle");
        assert_eq!(device_line(2, 2, Some(24.5)), "Cool 24.5° · cooling");
        assert_eq!(device_line(3, 1, Some(22.0)), "Auto 22° · heating");
        assert_eq!(device_line(4, 4, None), "Dry");
        assert_eq!(device_line(5, 0, None), "Fan");
        assert_eq!(device_line(5, 5, None), "Fan");
        assert_eq!(device_line(0, 0, Some(21.0)), "Off");
    }

    #[test]
    fn lcd_caps_lines() {
        assert_eq!(lcd_caps(1, 1, Some(23.0)), "HEAT 23");
        assert_eq!(lcd_caps(1, 0, Some(20.0)), "IDLE 20");
        assert_eq!(lcd_caps(3, 2, Some(24.5)), "COOL 24.5");
        assert_eq!(lcd_caps(4, 4, None), "DRY");
        assert_eq!(lcd_caps(0, 1, Some(20.0)), "OFF");
    }

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn quick_presses_are_one_step_each_and_go_out_once_as_shown() {
        let t0 = Instant::now();
        let mut sp = Setpoint::default();
        // three − presses 0.35 s apart from 20.5
        assert_eq!(sp.press(20.5, -0.5, 10.0, 32.0, at(t0, 0)), 20.0);
        assert_eq!(sp.tick(Some(20.5), at(t0, 300)), Step::Wait);
        assert_eq!(sp.press(20.5, -0.5, 10.0, 32.0, at(t0, 350)), 19.5);
        assert_eq!(sp.press(20.5, -0.5, 10.0, 32.0, at(t0, 700)), 19.0);
        assert_eq!(sp.shown(Some(20.5)), Some(19.0));
        // nothing goes out while the presses come; then one call with the value shown
        assert_eq!(sp.tick(Some(20.5), at(t0, 1000)), Step::Wait);
        assert_eq!(sp.next(), Some(at(t0, 700) + SETTLE));
        assert_eq!(sp.tick(Some(20.5), at(t0, 1400)), Step::Send(19.0));
        assert_eq!(sp.tick(Some(20.5), at(t0, 1500)), Step::Wait);
        // Home Assistant reports it: through, the page follows Home Assistant again
        sp.report(Some(19.0));
        assert!(!sp.busy());
        assert_eq!(sp.tick(Some(19.0), at(t0, 2000)), Step::Done);
        assert_eq!(sp.shown(Some(19.0)), Some(19.0));
    }

    #[test]
    fn an_older_echo_neither_moves_the_page_nor_becomes_the_base() {
        let t0 = Instant::now();
        let mut sp = Setpoint::default();
        for k in 0..3 {
            sp.press(20.5, 0.5, 10.0, 32.0, at(t0, k * 350));
        }
        assert_eq!(sp.tick(Some(20.5), at(t0, 1500)), Step::Send(22.0));
        // the state after an earlier call comes in late
        sp.report(Some(21.0));
        assert!(sp.busy());
        assert_eq!(sp.shown(Some(21.0)), Some(22.0));
        // the next press counts from 22.0, the value shown
        assert_eq!(sp.press(21.0, -0.5, 10.0, 32.0, at(t0, 1800)), 21.5);
    }

    #[test]
    fn the_next_value_waits_for_the_one_sent() {
        let t0 = Instant::now();
        let mut sp = Setpoint::default();
        sp.press(20.5, 0.5, 10.0, 32.0, at(t0, 0));
        assert_eq!(sp.tick(Some(20.5), at(t0, 700)), Step::Send(21.0));
        sp.press(20.5, 0.5, 10.0, 32.0, at(t0, 900));
        // settled, but 21.0 has not come back yet: one call at a time
        assert_eq!(sp.tick(Some(20.5), at(t0, 1700)), Step::Wait);
        assert_eq!(sp.next(), Some(at(t0, 700) + CONFIRM));
        sp.report(Some(21.0));
        assert!(sp.busy());
        assert_eq!(sp.next(), Some(at(t0, 900) + SETTLE));
        assert_eq!(sp.tick(Some(21.0), at(t0, 1800)), Step::Send(21.5));
    }

    #[test]
    fn a_value_not_reported_back_gives_way_to_home_assistants() {
        let t0 = Instant::now();
        let mut sp = Setpoint::default();
        sp.press(20.5, -0.5, 10.0, 32.0, at(t0, 0));
        sp.press(20.5, -0.5, 10.0, 32.0, at(t0, 350));
        sp.press(20.5, -0.5, 10.0, 32.0, at(t0, 700));
        assert_eq!(sp.tick(Some(20.5), at(t0, 1400)), Step::Send(19.0));
        // Home Assistant keeps 20.0
        assert_eq!(sp.tick(Some(20.0), at(t0, 1400 + 29900)), Step::Wait);
        assert_eq!(sp.shown(Some(20.0)), Some(19.0));
        assert_eq!(sp.tick(Some(20.0), at(t0, 1400 + 30000)), Step::Back);
        assert!(!sp.busy());
        assert_eq!(sp.shown(Some(20.0)), Some(20.0));
        // the next press counts from the real value
        assert_eq!(sp.press(20.0, 0.5, 10.0, 32.0, at(t0, 32000)), 20.5);
    }

    #[test]
    fn presses_after_a_lost_value_still_go_out() {
        let t0 = Instant::now();
        let mut sp = Setpoint::default();
        sp.press(20.5, 0.5, 10.0, 32.0, at(t0, 0));
        assert_eq!(sp.tick(Some(20.5), at(t0, 700)), Step::Send(21.0));
        sp.press(20.5, 0.5, 10.0, 32.0, at(t0, 2000));
        // 21.0 never came back; the newer value goes out when its wait is over
        assert_eq!(sp.tick(Some(20.5), at(t0, 30700)), Step::Send(21.5));
    }

    #[test]
    fn a_press_and_its_opposite_send_nothing() {
        let t0 = Instant::now();
        let mut sp = Setpoint::default();
        sp.press(20.5, 0.5, 10.0, 32.0, at(t0, 0));
        sp.press(20.5, -0.5, 10.0, 32.0, at(t0, 300));
        assert_eq!(sp.tick(Some(20.5), at(t0, 1000)), Step::Done);
        assert!(!sp.busy());
    }

    #[test]
    fn a_turn_goes_out_at_once_after_the_value_before_it() {
        let t0 = Instant::now();
        let mut sp = Setpoint::default();
        sp.set(23.5, at(t0, 0));
        assert_eq!(sp.tick(Some(20.5), at(t0, 0)), Step::Send(23.5));
        sp.set(24.0, at(t0, 400));
        assert_eq!(sp.shown(Some(20.5)), Some(24.0));
        assert_eq!(sp.tick(Some(20.5), at(t0, 400)), Step::Wait);
        assert_eq!(sp.tick(Some(23.5), at(t0, 900)), Step::Send(24.0));
        assert_eq!(sp.tick(Some(24.0), at(t0, 1500)), Step::Done);
    }

    #[test]
    fn presses_stay_in_the_range() {
        let t0 = Instant::now();
        let mut sp = Setpoint::default();
        assert_eq!(sp.press(31.5, 0.5, 10.0, 32.0, at(t0, 0)), 32.0);
        assert_eq!(sp.press(31.5, 0.5, 10.0, 32.0, at(t0, 300)), 32.0);
        let mut low = Setpoint::default();
        assert_eq!(low.press(10.0, -0.5, 10.0, 32.0, at(t0, 0)), 10.0);
    }

    #[test]
    fn turned_values_snap_to_the_step() {
        assert_eq!(to_step(23.4, 0.5), 23.5);
        assert_eq!(to_step(23.2, 0.5), 23.0);
        assert_eq!(to_step(23.6, 1.0), 24.0);
        assert_eq!(to_step(21.0, 0.0), 21.0);
    }
}
