//! The integration's display and device options that the panel carries out itself: what switches the Auto theme
//! (the light in the room, the sun or set times), Wi-Fi roaming on or off, the wake distance and its calibration,
//! and the child lock.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use serde_json::Value;
use slint::ComponentHandle;

use crate::{hw, roam, with_state, AppWindow, UiState};

// ---------------------------------------------------------------- theme

/// What switches the Auto theme between dark and light (the integration's `theme_auto`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoBy {
    /// The panel's light sensor (main.rs `auto_dark`; Home Assistant's sun without a reading).
    Light,
    /// Home Assistant's sun: dark while it is below the horizon.
    Sun,
    /// Dark from `from` to `to`, in minutes of the local day (the span may cross midnight).
    Times { from: u32, to: u32 },
}

/// Set times: dark from 20:00 to 7:00 unless the integration says otherwise.
const DARK_FROM: u32 = 20 * 60;
const DARK_TO: u32 = 7 * 60;

/// The Auto theme's trigger in the integration's configuration (the light sensor without one).
pub fn auto_by(config: Option<&Value>) -> AutoBy {
    let c = config.and_then(|c| c.get("theme_auto"));
    let get = |k: &str| c.and_then(|c| c.get(k)).and_then(Value::as_str);
    match get("by").unwrap_or("light") {
        "sun" => AutoBy::Sun,
        "times" => AutoBy::Times {
            from: get("from").and_then(minutes).unwrap_or(DARK_FROM),
            to: get("to").and_then(minutes).unwrap_or(DARK_TO),
        },
        _ => AutoBy::Light,
    }
}

/// "HH:MM" as minutes of the day.
pub fn minutes(text: &str) -> Option<u32> {
    let (h, m) = text.split_once(':')?;
    let (h, m) = (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Whether `now` lies from `from` up to `to` (minutes of the day; the span may cross midnight, equal ends: never).
pub fn in_window(now: u32, from: u32, to: u32) -> bool {
    if from <= to {
        now >= from && now < to
    } else {
        now >= from || now < to
    }
}

/// The local time of day in minutes.
pub fn minutes_now() -> u32 {
    minutes(&crate::pages::local_format(crate::pages::now(), "%H:%M")).unwrap_or(0)
}

// ---------------------------------------------------------------- wake distance, roaming

/// The wake distance the integration sets (`wake_distance`: near, medium, far), cm.
pub fn wake_cm(s: &UiState) -> f64 {
    let key = s.panel_config.as_ref().and_then(|c| c.get("wake_distance")).and_then(Value::as_str).unwrap_or("medium");
    hw::PROXIMITY_WAKE_CHOICES.iter().find(|(k, _)| *k == key).map_or(hw::PROXIMITY_WAKE_CM, |(_, cm)| *cm)
}

/// What a new configuration from the integration changes here (also at the start, from the stored one).
pub fn apply(ui: &AppWindow, s: &mut UiState) {
    roam::set_enabled(s.panel_config.as_ref().and_then(|c| c.get("wifi_roaming")).and_then(Value::as_bool).unwrap_or(true));
    let cm = wake_cm(s);
    s.proximity.set_wake_cm(cm);
    update_lock(ui, s);
}

// ---------------------------------------------------------------- child lock

/// Unlocked on the panel, it locks again after this long without a touch (or when the screensaver starts).
const RELOCK_AFTER: Duration = Duration::from_secs(60);
/// The open lock shows this long before the layer goes.
const UNLOCK_SHOW: Duration = Duration::from_millis(350);

/// The wake distance calibration: a lead-in to step back, a still screen while the panel measures (redraws can
/// disturb the proximity readings), then the reach that results.
enum Cal {
    Off,
    Lead { until: Instant },
    Measure { until: Instant, readings: Vec<f64> },
    Done { until: Instant },
}

const CAL_LEAD: Duration = Duration::from_secs(10);
const CAL_MEASURE: Duration = Duration::from_secs(10);
const CAL_DONE: Duration = Duration::from_secs(6);

struct Local {
    /// The child lock held open on the panel (until RELOCK_AFTER without a touch).
    unlocked: bool,
    cal: Cal,
}

thread_local! {
    static LOCAL: RefCell<Local> = const { RefCell::new(Local { unlocked: false, cal: Cal::Off }) };
}

/// Whether touches are locked now: the child lock is on and not held open.
fn locked(s: &UiState) -> bool {
    s.settings.child_lock && !LOCAL.with(|l| l.borrow().unlocked)
}

/// The child lock layer as the setting and the panel's unlock say.
pub fn update_lock(ui: &AppWindow, s: &UiState) {
    let on = locked(s);
    if ui.get_child_lock() != on {
        ui.set_child_lock(on);
        if !on {
            ui.set_child_lock_open(false);
        }
    }
}

/// The child lock switched on or off (the integration's switch): on, it locks at once and nothing stays open under
/// it. Not inside `with_state`.
pub fn lock_changed(ui: &AppWindow) {
    LOCAL.with(|l| l.borrow_mut().unlocked = false);
    let on = with_state(|s| s.settings.child_lock);
    eprintln!("nspanel-app: child lock {}", if on { "on" } else { "off" });
    if on {
        ui.set_set_view(0);
        crate::close_page_sheets(ui);
    }
    with_state(|s| update_lock(ui, s));
}

/// Twice a second (inside `with_state`): the child lock locks again after a minute without a touch or with the
/// screensaver; the calibration goes on with the proximity reading.
pub fn tick(ui: &AppWindow, s: &mut UiState, idle: Duration, reading: Option<f64>) {
    let relock = LOCAL.with(|l| {
        let mut l = l.borrow_mut();
        let relock = l.unlocked && (idle >= RELOCK_AFTER || s.screensaver || !s.settings.child_lock);
        if relock {
            l.unlocked = false;
        }
        relock
    });
    if relock && s.settings.child_lock {
        eprintln!("nspanel-app: child lock: locked again");
    }
    update_lock(ui, s);
    calibration_tick(ui, s, reading);
}

/// Taps from the child lock and the calibration layers (not inside `with_state`).
pub fn act(ui: &AppWindow, what: &str, _value: &str) {
    match what {
        // the lock held until its ring was full
        "lock.unlock" => {
            if !with_state(|s| s.settings.child_lock) {
                return;
            }
            LOCAL.with(|l| l.borrow_mut().unlocked = true);
            eprintln!("nspanel-app: child lock: unlocked on the panel");
            let weak = ui.as_weak();
            slint::Timer::single_shot(UNLOCK_SHOW, move || {
                if let Some(ui) = weak.upgrade() {
                    with_state(|s| update_lock(&ui, s));
                }
            });
        }
        "prox.calibrate" => start_calibration(ui),
        // a tap ends the calibration, nothing changes
        "prox.cancel" => {
            if calibrating() {
                eprintln!("nspanel-app: proximity: calibration ended by a tap");
                LOCAL.with(|l| l.borrow_mut().cal = Cal::Off);
                ui.set_prox_cal_on(false);
            }
        }
        _ => eprintln!("nspanel-app: unknown act {what}"),
    }
}

// ---------------------------------------------------------------- wake distance calibration

pub fn calibrating() -> bool {
    LOCAL.with(|l| !matches!(l.borrow().cal, Cal::Off))
}

/// Starts the wake distance calibration (the integration's Calibrate). Not inside `with_state`.
pub fn start_calibration(ui: &AppWindow) {
    if with_state(|s| s.light.proximity().is_none()) {
        eprintln!("nspanel-app: proximity: no reading, no calibration");
        return;
    }
    eprintln!("nspanel-app: proximity: calibration started");
    LOCAL.with(|l| l.borrow_mut().cal = Cal::Lead { until: Instant::now() + CAL_LEAD });
    ui.set_set_view(0);
    crate::wake(ui);
    ui.set_prox_cal_step(0);
    ui.set_prox_cal_count(CAL_LEAD.as_secs() as i32);
    ui.set_prox_cal_on(true);
}

fn calibration_tick(ui: &AppWindow, s: &mut UiState, reading: Option<f64>) {
    let now = Instant::now();
    LOCAL.with(|l| {
        let mut l = l.borrow_mut();
        let next = match &mut l.cal {
            Cal::Off => return,
            Cal::Lead { until } if now >= *until => {
                ui.set_prox_cal_step(1);
                Some(Cal::Measure { until: now + CAL_MEASURE, readings: Vec::new() })
            }
            Cal::Lead { until } => {
                let left = until.saturating_duration_since(now).as_secs_f32().ceil() as i32;
                if ui.get_prox_cal_count() != left {
                    ui.set_prox_cal_count(left);
                }
                None
            }
            Cal::Measure { until, readings } => {
                readings.extend(reading);
                if now >= *until {
                    let ok = s.proximity.calibrate(readings);
                    let reach = s.proximity.reach_cm().unwrap_or(hw::PROXIMITY_WAKE_CM);
                    eprintln!(
                        "nspanel-app: proximity: calibration {} ({} readings), wakes within about {reach:.0} cm",
                        if ok { "done" } else { "refused: something moved in front" },
                        readings.len()
                    );
                    ui.set_prox_cal_ok(ok);
                    ui.set_prox_cal_reach(round_to_5(reach));
                    ui.set_prox_cal_step(2);
                    Some(Cal::Done { until: now + CAL_DONE })
                } else {
                    None
                }
            }
            Cal::Done { until } if now >= *until => {
                ui.set_prox_cal_on(false);
                Some(Cal::Off)
            }
            Cal::Done { .. } => None,
        };
        if let Some(next) = next {
            l.cal = next;
        }
    });
}

/// A distance as it is told: to the nearest 5 cm.
fn round_to_5(cm: f64) -> i32 {
    ((cm / 5.0).round() * 5.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn theme_trigger_from_the_configuration() {
        assert_eq!(auto_by(None), AutoBy::Light);
        assert_eq!(auto_by(Some(&json!({"theme_auto": {"by": "sun"}}))), AutoBy::Sun);
        assert_eq!(auto_by(Some(&json!({"theme_auto": {"by": "light"}}))), AutoBy::Light);
        assert_eq!(auto_by(Some(&json!({"theme_auto": {"by": "odd"}}))), AutoBy::Light);
        assert_eq!(
            auto_by(Some(&json!({"theme_auto": {"by": "times", "from": "21:30", "to": "06:15"}}))),
            AutoBy::Times { from: 21 * 60 + 30, to: 6 * 60 + 15 }
        );
        // missing or broken times: the defaults
        assert_eq!(
            auto_by(Some(&json!({"theme_auto": {"by": "times", "from": "25:00"}}))),
            AutoBy::Times { from: DARK_FROM, to: DARK_TO }
        );
    }

    #[test]
    fn time_windows() {
        assert_eq!(minutes("07:05"), Some(425));
        assert_eq!(minutes("24:00"), None);
        assert_eq!(minutes("7"), None);
        // across midnight
        assert!(in_window(23 * 60, 20 * 60, 7 * 60));
        assert!(in_window(0, 20 * 60, 7 * 60));
        assert!(!in_window(7 * 60, 20 * 60, 7 * 60));
        assert!(in_window(20 * 60, 20 * 60, 7 * 60));
        assert!(!in_window(12 * 60, 20 * 60, 7 * 60));
        // within one day
        assert!(in_window(13 * 60, 12 * 60, 14 * 60));
        assert!(!in_window(14 * 60, 12 * 60, 14 * 60));
        // equal ends: never
        assert!(!in_window(8 * 60, 8 * 60, 8 * 60));
    }

    #[test]
    fn distances_told_to_5_cm() {
        assert_eq!(round_to_5(45.0), 45);
        assert_eq!(round_to_5(46.2), 45);
        assert_eq!(round_to_5(53.0), 55);
    }
}
