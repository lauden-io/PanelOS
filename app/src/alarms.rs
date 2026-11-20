//! Alarms on the timers page: kept on the panel (settings.json), set in the
//! page's editor (drums, days, sound, label), ringing on the panel at their
//! minute like a finished timer.

use serde::{Deserialize, Serialize};
use slint::{Color, ModelRc, SharedString, VecModel};

use crate::pages::{local_format, now};
use crate::{with_state, AlarmItem, AppWindow};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Alarm {
    pub h: i32,
    pub m: i32,
    /// Monday … Sunday; none set: once, the next time it comes.
    pub days: [bool; 7],
    pub label: String,
    /// The editor's sound (index into the panel's designed sounds).
    pub sound: i32,
    pub on: bool,
    /// A sound set in the integration (`lib:` / `own:` / `ma:`), empty: the editor's sound.
    pub tone: String,
    /// Its name, shown in the editor.
    pub tone_name: String,
}

impl Default for Alarm {
    fn default() -> Self {
        // Birds (Early Bird), the default for alarms in the integration too
        Self { h: 7, m: 0, days: [true, true, true, true, true, false, false], label: String::new(), sound: 2, on: true, tone: String::new(), tone_name: String::new() }
    }
}

impl Alarm {
    /// The sound it rings with, as the integration names sounds.
    pub fn ring_tone(&self) -> String {
        if self.tone.is_empty() || self.tone == "auto" {
            format!("lib:{}", crate::sounds::alarm_lib(self.sound))
        } else {
            self.tone.clone()
        }
    }

    /// From the integration's alarm (`h`, `m`, `days`, `label`, `tone`, `tone_name`, `on`).
    fn from_integration(v: &serde_json::Value) -> Option<Self> {
        let h = v["h"].as_i64().filter(|h| (0..24).contains(h))? as i32;
        let m = v["m"].as_i64().filter(|m| (0..60).contains(m))? as i32;
        let mut days = [false; 7];
        if let Some(list) = v["days"].as_array() {
            for (i, d) in list.iter().take(7).enumerate() {
                days[i] = d.as_bool().unwrap_or(false);
            }
        }
        let tone = v["tone"].as_str().unwrap_or("auto").chars().take(120).collect::<String>();
        // a designed sound keeps the editor's chip on it
        let sound = tone.strip_prefix("lib:").and_then(|id| crate::sounds::LIB[..6].iter().position(|x| *x == id)).map(|i| i as i32).unwrap_or(2);
        Some(Self {
            h,
            m,
            days,
            label: v["label"].as_str().unwrap_or("").trim().chars().take(32).collect(),
            sound,
            on: v["on"].as_bool().unwrap_or(true),
            tone_name: v["tone_name"].as_str().unwrap_or("").chars().take(60).collect(),
            tone,
        })
    }
}

/// The alarms as the integration set them (the panel keeps them and rings them itself).
pub fn set_all(ui: &AppWindow, list: &serde_json::Value) {
    let Some(items) = list.as_array() else { return };
    let mut alarms: Vec<Alarm> = items.iter().take(20).filter_map(Alarm::from_integration).collect();
    alarms.sort_by_key(|a| a.h * 60 + a.m);
    with_state(|s| {
        s.alarm_edit = None;
        s.settings.alarms = alarms;
        s.settings.save();
        crate::sounds::keep(&crate::used_sounds(s));
        refresh(ui, s);
    });
}

const TINTS: [u32; 6] = [0xffb21e, 0x3d9bff, 0x2fd27a, 0xc86bfa, 0xff5a36, 0xff4fa0];

fn tint(i: usize) -> Color {
    let c = TINTS[i % TINTS.len()];
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn days_text(d: &[bool; 7]) -> String {
    let names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    match d {
        [true, true, true, true, true, true, true] => "Every day".into(),
        [true, true, true, true, true, false, false] => "Weekdays".into(),
        [false, false, false, false, false, true, true] => "Weekends".into(),
        _ if d.iter().all(|x| !x) => "Once".into(),
        _ => names.iter().zip(d).filter(|(_, on)| **on).map(|(n, _)| *n).collect::<Vec<_>>().join(", "),
    }
}

/// Minutes from now to the alarm's next ring (None when it never rings).
fn minutes_to(a: &Alarm, t: i64) -> Option<i64> {
    if !a.on {
        return None;
    }
    let wd = (local_format(t, "%u").parse::<i64>().unwrap_or(1) - 1).clamp(0, 6);
    let mins_now = local_format(t, "%H").parse::<i64>().unwrap_or(0) * 60 + local_format(t, "%M").parse::<i64>().unwrap_or(0);
    let target = a.h as i64 * 60 + a.m as i64;
    (0..8)
        .find(|&d| {
            let day = ((wd + d) % 7) as usize;
            let later = d > 0 || target > mins_now;
            later && (a.days.iter().all(|x| !x) || a.days[day])
        })
        .map(|d| d * 1440 + target - mins_now)
}

/// The alarm list and the "next alarm" line.
pub fn refresh(ui: &AppWindow, s: &crate::UiState) {
    let t = now();
    let rows: Vec<AlarmItem> = s
        .settings
        .alarms
        .iter()
        .enumerate()
        .map(|(i, a)| AlarmItem {
            time: crate::clock::hm(a.h as u32, a.m as u32).into(),
            days: days_text(&a.days).into(),
            label: if a.label.is_empty() { "Alarm".into() } else { a.label.clone().into() },
            on: a.on,
            tint: tint(i),
            hour: a.h as f32 + a.m as f32 / 60.0,
            soon: minutes_to(a, t).is_some_and(|m| m <= 1440),
        })
        .collect();
    if let Some(m) = crate::pages::update_rows(&ui.get_timers_alarms(), rows) {
        ui.set_timers_alarms(m);
    }
    let next = s.settings.alarms.iter().enumerate().filter_map(|(i, a)| minutes_to(a, t).map(|m| (m, i, a))).min_by_key(|(m, _, _)| *m);
    // the looks' numbers: the next alarm's time of day and the minutes until it; the sun from the weather
    match next {
        Some((m, _, a)) => {
            ui.set_timers_next_h(a.h as f32 + a.m as f32 / 60.0);
            ui.set_timers_next_in(m as i32);
            ui.set_timers_next_time(crate::clock::hm(a.h as u32, a.m as u32).into());
            ui.set_timers_next_label(if a.label.is_empty() { "Alarm".into() } else { a.label.clone().into() });
            let from = (a.h as i32 * 60 + a.m as i32 - 30).rem_euclid(1440);
            ui.set_timers_dawn_from(crate::clock::hm(from as u32 / 60, from as u32 % 60).into());
        }
        None => {
            ui.set_timers_next_h(-1.0);
            ui.set_timers_next_in(-1);
        }
    }
    let hours = |t: &str| crate::clock::parse_hm(t).map(|(h, m)| h as f32 + m as f32 / 60.0);
    let w = ui.get_weather();
    if let (Some(r), Some(st)) = (hours(w.sunrise.as_str()), hours(w.sunset.as_str())) {
        ui.set_timers_sunrise_h(r);
        ui.set_timers_sunset_h(st);
    }
    let text = match next {
        Some((m, i, a)) => {
            ui.set_timers_next_tint(tint(i));
            let (h, mm) = (m / 60, m % 60);
            let when = if h >= 24 { format!("in {} d {} h", h / 24, h % 24) } else if h > 0 { format!("in {h} h {mm} min") } else { format!("in {mm} min") };
            format!("{} · {when}", crate::clock::hm(a.h as u32, a.m as u32))
        }
        None => String::new(),
    };
    if ui.get_timers_next_alarm().as_str() != text {
        ui.set_timers_next_alarm(text.into());
    }
}

/// Alarms that ring this minute (called when the minute turns): time, label, sound.
pub fn due(s: &mut crate::UiState) -> Option<(String, String, String)> {
    let t = now();
    let wd = (local_format(t, "%u").parse::<usize>().unwrap_or(1) - 1).min(6);
    let (h, m) = (local_format(t, "%H").parse::<i32>().unwrap_or(-1), local_format(t, "%M").parse::<i32>().unwrap_or(-1));
    let mut rang = None;
    let mut changed = false;
    for a in s.settings.alarms.iter_mut() {
        let once = a.days.iter().all(|x| !x);
        if a.on && a.h == h && a.m == m && (once || a.days[wd]) {
            rang = Some((crate::clock::hm(a.h as u32, a.m as u32), if a.label.is_empty() { "Alarm".to_string() } else { a.label.clone() }, a.ring_tone()));
            if once {
                a.on = false;
                changed = true;
            }
        }
    }
    if changed {
        s.settings.save();
    }
    rang
}

fn open_editor(ui: &AppWindow, a: &Alarm, new: bool, i: usize) {
    ui.set_timers_edit_timer(false);
    ui.set_timers_edit_new(new);
    ui.set_timers_edit_h(a.h);
    ui.set_timers_edit_m(a.m);
    ui.set_timers_edit_days(ModelRc::new(VecModel::from(a.days.to_vec())));
    ui.set_timers_edit_label(a.label.clone().into());
    // a sound from Home Assistant (an uploaded one, music, a designed one the editor has no chip for)
    // is the editor's last chip
    let lib_extra = a.tone.strip_prefix("lib:").filter(|id| !crate::sounds::LIB[..6].contains(id));
    let ha = a.tone.starts_with("own:") || a.tone.starts_with("ma:") || lib_extra.is_some();
    // the ⓘ note on where other sounds come from
    ui.set_timers_ha_sounds(true);
    ui.set_timers_edit_ha_sound(match (ha, a.tone_name.is_empty()) {
        (false, _) => "".into(),
        (true, _) if lib_extra.is_some() => crate::sounds::panel_name(lib_extra.unwrap_or("")).into(),
        (true, true) => "Home Assistant".into(),
        (true, false) => a.tone_name.clone().into(),
    });
    ui.set_timers_edit_sound(if ha { 6 } else { a.sound });
    ui.set_timers_edit_tint(tint(i));
    ui.set_timers_edit_keyboard(false);
    ui.set_timers_edit_info(false);
    ui.set_timers_edit(true);
}

/// Taps on the alarms tab and in the editor (`alarms.*`, `alarm.*`).
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    let n: usize = value.parse().unwrap_or(0);
    match what {
        "alarms.new" => {
            let count = with_state(|s| {
                s.alarm_edit = None;
                s.settings.alarms.len()
            });
            open_editor(ui, &Alarm::default(), true, count);
        }
        "alarms.edit" => {
            let Some(a) = with_state(|s| {
                s.alarm_edit = Some(n);
                s.settings.alarms.get(n).cloned()
            }) else {
                return;
            };
            open_editor(ui, &a, false, n);
        }
        "alarms.toggle" => with_state(|s| {
            if let Some(a) = s.settings.alarms.get_mut(n) {
                a.on = !a.on;
                s.settings.save();
            }
        }),
        "alarm.day" => {
            let days = ui.get_timers_edit_days();
            if let Some(v) = slint::Model::row_data(&days, n) {
                slint::Model::set_row_data(&days, n, !v);
            }
        }
        "alarm.sound" => ui.set_timers_edit_sound(n as i32),
        "alarm.info" => ui.set_timers_edit_info(!ui.get_timers_edit_info()),
        "alarm.label" => {
            // an empty label starts with a capital
            ui.set_timers_kb_layer(if ui.get_timers_edit_label().is_empty() { 1 } else { 0 });
            ui.set_timers_edit_keyboard(true);
        }
        "alarm.key" => {
            let mut label = ui.get_timers_edit_label().to_string();
            let layer = ui.get_timers_kb_layer();
            match value {
                "enter" => ui.set_timers_edit_keyboard(false),
                "back" | "clear" => {
                    if value == "back" {
                        label.pop();
                    } else {
                        label.clear();
                    }
                    // back at the start of the line: a capital again
                    if label.is_empty() && layer == 0 {
                        ui.set_timers_kb_layer(1);
                    }
                }
                "shift" => ui.set_timers_kb_layer(match layer {
                    0 => 1,
                    1 => 0,
                    2 => 3,
                    _ => 2,
                }),
                "symbols" => ui.set_timers_kb_layer(if layer >= 2 { 0 } else { 2 }),
                "layout" => ui.set_kb_layout(1 - ui.get_kb_layout().clamp(0, 1)),
                k => {
                    if label.chars().count() < 32 {
                        label.push_str(k);
                    }
                    // one capital, then small letters again
                    if layer == 1 {
                        ui.set_timers_kb_layer(0);
                    }
                }
            }
            ui.set_timers_edit_label(SharedString::from(label));
        }
        "alarm.save" => {
            let days: Vec<bool> = slint::Model::iter(&ui.get_timers_edit_days()).collect();
            let mut d = [false; 7];
            for (i, v) in days.iter().take(7).enumerate() {
                d[i] = *v;
            }
            let chosen = ui.get_timers_edit_sound();
            let mut a = Alarm {
                h: ui.get_timers_edit_h(),
                m: ui.get_timers_edit_m(),
                days: d,
                label: ui.get_timers_edit_label().trim().to_string(),
                sound: chosen.min(5),
                on: true,
                tone: String::new(),
                tone_name: String::new(),
            };
            with_state(|s| {
                // the Home Assistant chip keeps the sound set there; a designed one replaces it
                if chosen >= 6 {
                    if let Some(old) = s.alarm_edit.and_then(|i| s.settings.alarms.get(i)) {
                        a.sound = old.sound;
                        a.tone = old.tone.clone();
                        a.tone_name = old.tone_name.clone();
                    }
                }
                match s.alarm_edit.take() {
                    Some(i) if i < s.settings.alarms.len() => s.settings.alarms[i] = a,
                    _ => s.settings.alarms.push(a),
                }
                s.settings.alarms.sort_by_key(|a| a.h * 60 + a.m);
                s.settings.save();
            });
            ui.set_timers_edit(false);
        }
        "alarm.delete" => {
            with_state(|s| {
                if let Some(i) = s.alarm_edit.take() {
                    if i < s.settings.alarms.len() {
                        s.settings.alarms.remove(i);
                        s.settings.save();
                    }
                }
            });
            ui.set_timers_edit(false);
        }
        "alarm.close" => {
            ui.set_timers_edit_keyboard(false);
            ui.set_timers_edit(false);
        }
        _ => {}
    }
    with_state(|s| refresh(ui, s));
}
