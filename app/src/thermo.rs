//! Thermostat page: the climate entities picked for it, one on the dial at a
//! time (the device chip opens them all), its modes, target and options.

use serde_json::{json, Value};
use slint::{ModelRc, SharedString, VecModel};

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
                let mut d = device(id, s.states.get(id));
                // a value just set stays shown until Home Assistant reports it (or 5 s pass)
                if let Some(t) = pending(id) {
                    if (d.target - t).abs() < 0.01 {
                        PENDING.with(|p| p.borrow_mut().remove(id));
                    } else {
                        d.target = t;
                    }
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
}

thread_local! {
    /// Targets set on the panel that Home Assistant has not reported back yet: (value, when).
    static PENDING: std::cell::RefCell<std::collections::HashMap<String, (f32, std::time::Instant)>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

fn pending(id: &str) -> Option<f32> {
    PENDING.with(|p| p.borrow().get(id).filter(|(_, at)| at.elapsed() < std::time::Duration::from_secs(5)).map(|(t, _)| *t))
}

fn set_pending(id: &str, t: f32) {
    PENDING.with(|p| p.borrow_mut().insert(id.to_string(), (t, std::time::Instant::now())));
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
        "thermo.minus" | "thermo.plus" => {
            let d = device(&id, st);
            // quick presses count one by one: the next step starts from the value just set, not from Home Assistant's
            // state that has not caught up yet
            let base = pending(&id).unwrap_or(d.target);
            let t = (base + if what == "thermo.plus" { d.step } else { -d.step }).clamp(d.tmin, d.tmax);
            set_pending(&id, t);
            call("set_temperature", &id, json!({"temperature": t}));
            // Shown at once; Home Assistant's new state follows.
            show_target(ui, n, sel, t);
        }
        // a value from a turned knob or dial (wall, dial and neon looks), to the device's step
        "thermo.target" => {
            let d = device(&id, st);
            let v: f32 = value.parse().unwrap_or(d.target);
            let t = ((v / d.step).round() * d.step).clamp(d.tmin, d.tmax);
            set_pending(&id, t);
            call("set_temperature", &id, json!({"temperature": t}));
            show_target(ui, n, sel, t);
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
}
