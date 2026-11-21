//! Alarms on the timers page: kept on the panel (settings.json), set in the
//! page's editor (drums, days, sound, label), ringing on the panel at their
//! minute like a finished timer.

use std::time::{Duration, Instant};

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

/// Which alarm a ring or a snooze belongs to: its time, days and name (alarms have no ids; the integration tells
/// them apart the same way).
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmKey {
    h: i32,
    m: i32,
    days: [bool; 7],
    label: String,
}

impl AlarmKey {
    /// From an alarm as the integration names it (`h`, `m`, `days`, `label`).
    fn from_integration(v: &serde_json::Value) -> Option<Self> {
        Alarm::from_integration(v).map(|a| a.key())
    }
}

/// A snoozed alarm: when it rings again, which alarm it is, its label and sound.
#[derive(Debug, Clone)]
pub struct Snooze {
    pub at: Instant,
    pub alarm: AlarmKey,
    /// The alarm has been on since it rang: switching it off ends the snooze. A once alarm switches itself off as it
    /// rings, which alone does not end it.
    pub armed: bool,
    pub label: String,
    pub tone: String,
}

/// The snooze of a ringing alarm: it rings again in 9 minutes.
pub fn snooze(list: &[Alarm], alarm: AlarmKey, label: String, tone: String) -> Snooze {
    let armed = list.iter().any(|a| a.on && a.key() == alarm);
    Snooze { at: Instant::now() + Duration::from_secs(9 * 60), alarm, armed, label, tone }
}

/// Whether a snooze still holds after the alarm list changed: its alarm is still there (same time, days and name) and
/// has not been switched off since it rang.
fn snooze_holds(sn: &mut Snooze, list: &[Alarm]) -> bool {
    let mut same = list.iter().filter(|a| a.key() == sn.alarm).peekable();
    if same.peek().is_none() {
        return false;
    }
    if same.any(|a| a.on) {
        sn.armed = true;
        return true;
    }
    !sn.armed
}

/// Ends the snooze when its alarm was switched off or deleted.
fn check_snooze(s: &mut crate::UiState) {
    let Some(sn) = s.snooze.as_mut() else { return };
    if !snooze_holds(sn, &s.settings.alarms) {
        eprintln!("nspanel-app: alarm snooze cancelled (its alarm was switched off or deleted)");
        s.snooze = None;
    }
}

fn is_snoozed(s: &crate::UiState, a: &Alarm) -> bool {
    s.snooze.as_ref().is_some_and(|sn| sn.alarm == a.key())
}

impl Alarm {
    pub fn key(&self) -> AlarmKey {
        AlarmKey { h: self.h, m: self.m, days: self.days, label: self.label.clone() }
    }

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

/// The alarms as the panel reports them to the integration: a sound picked in the panel's editor goes as the designed
/// sound it rings with (`lib:<id>`), so the PanelOS page shows it and sends it back unchanged; "auto" and the
/// integration's own sounds go as they were set.
pub fn reported(list: &[Alarm]) -> Vec<Alarm> {
    list.iter()
        .map(|a| {
            let mut a = a.clone();
            if a.tone.is_empty() {
                a.tone = a.ring_tone();
            }
            a
        })
        .collect()
}

/// The alarms as the integration set them (the panel keeps them and rings them itself). `cancelled`: the alarms the
/// change switched off or deleted (a voice assistant's cancel); their snooze ends too, also when the alarm was off
/// already (a once alarm that rang).
pub fn set_all(ui: &AppWindow, list: &serde_json::Value, cancelled: &serde_json::Value) {
    let Some(items) = list.as_array() else { return };
    let mut alarms: Vec<Alarm> = items.iter().take(20).filter_map(Alarm::from_integration).collect();
    alarms.sort_by_key(|a| a.h * 60 + a.m);
    let cancelled: Vec<AlarmKey> = cancelled.as_array().map(|l| l.iter().filter_map(AlarmKey::from_integration).collect()).unwrap_or_default();
    with_state(|s| {
        s.alarm_edit = None;
        s.settings.alarms = alarms;
        s.settings.save();
        if s.snooze.as_ref().is_some_and(|sn| cancelled.contains(&sn.alarm)) {
            eprintln!("nspanel-app: alarm snooze cancelled (its alarm was cancelled in Home Assistant)");
            s.snooze = None;
        }
        check_snooze(s);
        crate::sounds::keep(&crate::used_sounds(s));
        refresh(ui, s);
    });
}

const TINTS: [u32; 6] = [0xffb21e, 0x3d9bff, 0x2fd27a, 0xc86bfa, 0xff5a36, 0xff4fa0];

fn tint(i: usize) -> Color {
    let c = TINTS[i % TINTS.len()];
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// An alarm's time of day in the panel's time format: "6:45" (24-hour, as the alarm looks write it) or "6:45 AM".
pub fn hm(h: i32, m: i32) -> String {
    hm_in(h, m, crate::clock::h24())
}

fn hm_in(h: i32, m: i32, h24: bool) -> String {
    let (h, m) = (h.rem_euclid(24), m.rem_euclid(60));
    if h24 {
        format!("{h}:{m:02}")
    } else {
        format!("{}:{m:02} {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" })
    }
}

/// The days an alarm rings on, short: "Weekdays", "Mon–Thu, Sat", "Once"; three days or more in a row as a range.
fn days_text(d: &[bool; 7]) -> String {
    let names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    match d {
        [true, true, true, true, true, true, true] => return "Every day".into(),
        [true, true, true, true, true, false, false] => return "Weekdays".into(),
        [false, false, false, false, false, true, true] => return "Weekends".into(),
        _ if d.iter().all(|x| !x) => return "Once".into(),
        _ => {}
    }
    let mut parts = Vec::new();
    let mut i = 0;
    while i < 7 {
        if !d[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i + 1 < 7 && d[i + 1] {
            i += 1;
        }
        match i - start {
            0 => parts.push(names[start].to_string()),
            1 => parts.push(format!("{}, {}", names[start], names[i])),
            _ => parts.push(format!("{}–{}", names[start], names[i])),
        }
        i += 1;
    }
    parts.join(", ")
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
        .map(|(i, a)| {
            // a snoozed alarm is on until its snooze rings (a once alarm switched itself off as it rang)
            let snoozed = is_snoozed(s, a);
            AlarmItem {
                time: hm(a.h, a.m).into(),
                days: days_text(&a.days).into(),
                label: if a.label.is_empty() { "Alarm".into() } else { a.label.clone().into() },
                on: a.on || snoozed,
                tint: tint(i),
                hour: a.h as f32 + a.m as f32 / 60.0,
                soon: snoozed || minutes_to(a, t).is_some_and(|m| m <= 1440),
            }
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
            ui.set_timers_next_time(hm(a.h, a.m).into());
            ui.set_timers_next_label(if a.label.is_empty() { "Alarm".into() } else { a.label.clone().into() });
            let from = (a.h as i32 * 60 + a.m as i32 - 30).rem_euclid(1440);
            ui.set_timers_dawn_from(hm(from / 60, from % 60).into());
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
            format!("{} · {when}", hm(a.h, a.m))
        }
        None => String::new(),
    };
    if ui.get_timers_next_alarm().as_str() != text {
        ui.set_timers_next_alarm(text.into());
    }
    // a snoozed alarm: the time of day it rings again (the looks' next-alarm lines show it), -1 none
    let snooze_h = match &s.snooze {
        Some(sn) => {
            let ring = t + sn.at.saturating_duration_since(Instant::now()).as_secs() as i64;
            let (h, m) = (local_format(ring, "%H").parse::<f32>().unwrap_or(0.0), local_format(ring, "%M").parse::<f32>().unwrap_or(0.0));
            h + m / 60.0
        }
        None => -1.0,
    };
    if ui.get_timers_snooze_h() != snooze_h {
        ui.set_timers_snooze_h(snooze_h);
    }
}

/// The time of day now in the panel's time format and as hours of the day (a snoozed alarm rings with it: its own
/// time has passed).
pub fn now_hm() -> (String, f32) {
    let t = now();
    let (h, m) = (local_format(t, "%H").parse().unwrap_or(0), local_format(t, "%M").parse().unwrap_or(0));
    (hm(h, m), h as f32 + m as f32 / 60.0)
}

/// An alarm that rings: which one, its time (as the panel writes it and as hours of the day), label and sound.
pub struct Ring {
    pub alarm: AlarmKey,
    pub time: String,
    pub hour: f32,
    pub label: String,
    pub tone: String,
}

/// Alarms that ring this minute (called when the minute turns).
pub fn due(s: &mut crate::UiState) -> Option<Ring> {
    let t = now();
    let wd = (local_format(t, "%u").parse::<usize>().unwrap_or(1) - 1).min(6);
    let (h, m) = (local_format(t, "%H").parse::<i32>().unwrap_or(-1), local_format(t, "%M").parse::<i32>().unwrap_or(-1));
    let mut rang = None;
    let mut changed = false;
    for a in s.settings.alarms.iter_mut() {
        let once = a.days.iter().all(|x| !x);
        if a.on && a.h == h && a.m == m && (once || a.days[wd]) {
            rang = Some(Ring {
                alarm: a.key(),
                time: hm(a.h, a.m),
                hour: a.h as f32 + a.m as f32 / 60.0,
                label: if a.label.is_empty() { "Alarm".to_string() } else { a.label.clone() },
                tone: a.ring_tone(),
            });
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
            // a snoozed alarm shows on: a tap switches it off and ends its snooze (a once alarm that rang stays off)
            let snoozed = s.settings.alarms.get(n).is_some_and(|a| is_snoozed(s, a));
            if let Some(a) = s.settings.alarms.get_mut(n) {
                a.on = !a.on && !snoozed;
                s.settings.save();
            }
            if snoozed {
                eprintln!("nspanel-app: alarm snooze cancelled (its alarm was switched off)");
                s.snooze = None;
            }
        }),
        "alarm.day" => {
            let days = ui.get_timers_edit_days();
            if let Some(v) = slint::Model::row_data(&days, n) {
                slint::Model::set_row_data(&days, n, !v);
            }
        }
        "alarm.sound" => {
            ui.set_timers_edit_sound(n as i32);
            // a designed sound plays once on the panel's speaker as it is picked (not over a ringing alarm or alert)
            if n < 6 {
                if let Some(samples) = crate::sounds::resolve(&format!("lib:{}", crate::sounds::alarm_lib(n as i32)), "birds") {
                    let data: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
                    with_state(|s| {
                        if s.ringing.is_none() && s.looping.is_none() {
                            s.player.play(crate::voice::Sound::Pcm { rate: 48000, channels: 1, data });
                        }
                    });
                }
            }
        }
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
                    // a timer's label up to 60 characters, an alarm's up to 32 (the keyboard view shows the count and
                    // shakes the field at the limit)
                    if label.chars().count() < crate::pages::label_limit(ui.get_timers_edit_timer()) {
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
        // the page's settings in the alarm and stopwatch looks' menus: the functions (at least one stays on), the
        // colours, the alarm look (the page options the integration keeps)
        "alarms.functions" => {
            let keys = ["alarm", "timer", "stopwatch"];
            let d = crate::pages::inst(&ui.get_timers_inst(), crate::acting_n("timers"));
            let mut on = [d.alarm_on, d.timer_on, d.watch_on];
            let Some(k) = keys.iter().position(|k| *k == value) else { return };
            on[k] = !on[k];
            if !on.iter().any(|x| *x) {
                return;
            }
            crate::with_timers(ui, |d| {
                d.alarm_on = on[0];
                d.timer_on = on[1];
                d.watch_on = on[2];
                d.mode = crate::timers_tab(d.mode, on);
            });
            let list: Vec<&str> = keys.iter().zip(on).filter(|(_, o)| *o).map(|(k, _)| *k).collect();
            crate::page_set(ui, "timers", None, serde_json::json!({ "functions": list }));
        }
        "alarms.colourful" => {
            crate::with_timers(ui, |d| d.colourful = value == "1");
            crate::page_set(ui, "timers", None, serde_json::json!({ "colourful": value == "1" }));
            crate::with_state(|s| crate::update_timer_lists(ui, s));
        }
        "alarms.look" => {
            let look = value.parse::<i32>().unwrap_or(0).clamp(0, 4);
            crate::with_timers(ui, |d| d.alarm_look = look);
            crate::page_set(ui, "timers", None, serde_json::json!({ "alarm_look": look }));
        }
        _ => {}
    }
    with_state(|s| {
        // a deleted alarm (or one saved with another time, days or name) takes its snooze with it
        check_snooze(s);
        refresh(ui, s);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_sounds_ring_as_named_and_go_round_unchanged() {
        // the editor's order: Rise and Shine, Up and At 'Em, Early Bird, Saved by the Bell, Old School, Neon Sunrise
        let named = ["chimes", "marimba", "birds", "bell", "digital", "synth"];
        for (i, id) in named.iter().enumerate() {
            assert_eq!(crate::sounds::alarm_lib(i as i32), *id);
            assert_eq!(crate::sounds::LIB[i], *id);
            // picked on the panel → reported as its designed sound → set back by the integration: the same pick
            let a = Alarm { sound: i as i32, ..Alarm::default() };
            let out = reported(&[a]);
            assert_eq!(out[0].tone, format!("lib:{id}"));
            let back = Alarm::from_integration(&serde_json::to_value(&out[0]).unwrap()).unwrap();
            assert_eq!(back.sound, i as i32);
            assert_eq!(back.ring_tone(), format!("lib:{id}"));
        }
        // the integration's default ("auto") rings Birds and is reported as it was set
        let auto = Alarm::from_integration(&serde_json::json!({"h": 7, "m": 0, "tone": "auto"})).unwrap();
        assert_eq!(auto.ring_tone(), "lib:birds");
        assert_eq!(reported(&[auto])[0].tone, "auto");
    }

    #[test]
    fn alarm_times() {
        assert_eq!(hm_in(6, 45, true), "6:45");
        assert_eq!(hm_in(18, 5, true), "18:05");
        assert_eq!(hm_in(6, 45, false), "6:45 AM");
        assert_eq!(hm_in(0, 0, false), "12:00 AM");
        assert_eq!(hm_in(12, 30, false), "12:30 PM");
        assert_eq!(hm_in(23, 59, false), "11:59 PM");
    }

    #[test]
    fn alarm_days() {
        assert_eq!(days_text(&[true; 7]), "Every day");
        assert_eq!(days_text(&[false; 7]), "Once");
        assert_eq!(days_text(&[true, true, true, true, true, false, false]), "Weekdays");
        assert_eq!(days_text(&[false, false, false, false, false, true, true]), "Weekends");
        assert_eq!(days_text(&[true, true, true, true, false, true, false]), "Mon–Thu, Sat");
        assert_eq!(days_text(&[true, false, true, false, true, false, true]), "Mon, Wed, Fri, Sun");
        assert_eq!(days_text(&[true, true, false, false, false, false, true]), "Mon, Tue, Sun");
        assert_eq!(days_text(&[false, false, true, false, false, false, false]), "Wed");
    }

    const WEEKDAYS: [bool; 7] = [true, true, true, true, true, false, false];

    fn alarm(h: i32, m: i32, days: [bool; 7], label: &str, on: bool) -> Alarm {
        Alarm { h, m, days, label: label.into(), on, ..Alarm::default() }
    }

    #[test]
    fn snooze_of_a_repeating_alarm() {
        let mut list = vec![alarm(6, 45, WEEKDAYS, "Work", true), alarm(8, 0, [false; 7], "", true)];
        let mut sn = snooze(&list, list[0].key(), "Work".into(), "lib:birds".into());
        assert!(sn.armed);
        // another alarm switched off, its own sound changed: it holds
        list[1].on = false;
        assert!(snooze_holds(&mut sn, &list));
        list[0].tone = "lib:marimba".into();
        assert!(snooze_holds(&mut sn, &list));
        // switched off: it ends
        list[0].on = false;
        assert!(!snooze_holds(&mut sn, &list));
    }

    #[test]
    fn snooze_of_a_deleted_or_changed_alarm_ends() {
        let mut list = vec![alarm(11, 18, [true; 7], "", true), alarm(12, 0, [true; 7], "", true)];
        let mut sn = snooze(&list, list[0].key(), "Alarm".into(), String::new());
        list.remove(0);
        assert!(!snooze_holds(&mut sn, &list));
        // saved with another time, days or name: another alarm
        for change in 0..3 {
            let mut list = vec![alarm(11, 18, [true; 7], "", true)];
            let mut sn = snooze(&list, list[0].key(), "Alarm".into(), String::new());
            match change {
                0 => list[0].m = 30,
                1 => list[0].days = WEEKDAYS,
                _ => list[0].label = "Gym".into(),
            }
            assert!(!snooze_holds(&mut sn, &list), "change {change}");
        }
        // the same alarm sent back by Home Assistant: it holds
        let list = vec![alarm(11, 18, [true; 7], "", true)];
        let mut sn = snooze(&list, list[0].key(), "Alarm".into(), String::new());
        assert!(snooze_holds(&mut sn, &[alarm(11, 18, [true; 7], "", true)]));
    }

    #[test]
    fn snooze_of_a_once_alarm() {
        // a once alarm switched itself off as it rang: its snooze holds while it stays off
        let mut list = vec![alarm(7, 0, [false; 7], "", false)];
        let mut sn = snooze(&list, list[0].key(), "Alarm".into(), String::new());
        assert!(!sn.armed);
        assert!(snooze_holds(&mut sn, &list));
        // switched on, then off again: it ends
        list[0].on = true;
        assert!(snooze_holds(&mut sn, &list));
        list[0].on = false;
        assert!(!snooze_holds(&mut sn, &list));
        // deleted: it ends
        let list = vec![alarm(7, 0, [false; 7], "", false)];
        let mut sn = snooze(&list, list[0].key(), "Alarm".into(), String::new());
        assert!(!snooze_holds(&mut sn, &[]));
    }

    #[test]
    fn alarm_keys_from_the_integration() {
        let a = alarm(6, 45, WEEKDAYS, "Work", true);
        let v = serde_json::json!({"h": 6, "m": 45, "days": WEEKDAYS, "label": " Work ", "on": false, "tone": "auto"});
        assert_eq!(AlarmKey::from_integration(&v), Some(a.key()));
        // days cut short count as off; without its time it names no alarm
        let v = serde_json::json!({"h": 6, "m": 45, "days": [true, true, true, true, true], "label": "Work"});
        assert_eq!(AlarmKey::from_integration(&v), Some(a.key()));
        assert_eq!(AlarmKey::from_integration(&serde_json::json!({"m": 45, "label": "Work"})), None);
        assert_ne!(AlarmKey::from_integration(&serde_json::json!({"h": 6, "m": 45, "days": WEEKDAYS})), Some(a.key()));
    }
}
