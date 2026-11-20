//! Status screens and critical alerts, sent by the integration (it evaluates
//! the house-wide rules and tells each panel). A status screen covers the
//! panel while its state lasts; a swipe or its button folds it into the strip
//! under the status bar, a tap on the strip opens it again and its × closes
//! the strip (the state goes on without it). A critical alert takes the whole
//! screen on the notifications page, sounds until it is held (when the
//! integration allows holding) and stays in the strip, red and without ×,
//! until its sensor clears.

use std::time::Instant;

use serde_json::Value;
use slint::Color;

use crate::pages::{local_format, now};
use crate::{icons, texts, with_state, AppWindow, StatusScreen};

#[derive(Clone, Debug)]
pub struct Status {
    id: String,
    title: String,
    line: String,
    strip: String,
    icon: String,
    picto: String,
    tint: Color,
    button: String,
    /// Air recovery and the like: ends by itself after this many seconds.
    countdown: Option<u64>,
    since: i64,
    started: Instant,
    folded: bool,
    /// Its strip was closed (×): the state goes on, nothing shows it.
    closed: bool,
}

#[derive(Clone, Debug)]
pub struct Critical {
    id: String,
    title: String,
    message: String,
    icon: String,
    source: String,
    hold: bool,
    silenced: bool,
    since: i64,
    /// Whether it sounds at all, and its sound (as the integration names sounds; default SOS).
    sound: bool,
    tone: String,
}

/// The panel's own text pack per category (the integration's ids).
fn cat_key(cat: &str) -> &str {
    match cat {
        "keepout" => "keep-out",
        c => c,
    }
}

fn cat_icon(cat: &str) -> &'static str {
    match cat {
        "occupied" => "toilet",
        "aftermath" => "fan",
        "bathroom" => "shower",
        "meeting" | "focus" => "headset",
        "sleeping" => "sleep",
        "movie" => "popcorn",
        "gaming" => "controller",
        "vacuum" => "robot-vacuum",
        "cooking" => "chef-hat",
        "workout" => "dumbbell",
        "keepout" | "keep-out" => "hand-back-right",
        _ => "alert-outline",
    }
}

thread_local! {
    /// Own pictograms loaded once (144 px for the screen, 24 px for the strip).
    static PICTOS: std::cell::RefCell<std::collections::HashMap<String, (slint::Image, slint::Image)>> = Default::default();
}

fn picto(name: &str) -> (slint::Image, slint::Image) {
    if name.is_empty() {
        return Default::default();
    }
    PICTOS.with(|p| {
        p.borrow_mut()
            .entry(name.to_string())
            .or_insert_with(|| (crate::quick::picture(&format!("status/{name}-144.png")), crate::quick::picture(&format!("status/{name}-24.png"))))
            .clone()
    })
}

fn glyph(name: &str) -> String {
    icons::glyph(name.trim_start_matches("mdi:")).map(String::from).unwrap_or_default()
}

fn minutes(since: i64) -> String {
    let m = ((now() - since).max(0) / 60) as i64;
    if m < 1 {
        "just now".into()
    } else if m < 60 {
        format!("{m} min")
    } else {
        format!("{} h {} min", m / 60, m % 60)
    }
}

/// A request from the integration: `status`, `status_clear`, `critical`, `critical_clear`.
pub fn show(ui: &AppWindow, v: &Value) {
    let kind = v["type"].as_str().unwrap_or("");
    let id = v["id"].as_str().unwrap_or("").to_string();
    let text = |k: &str| v[k].as_str().unwrap_or("").trim().to_string();
    let mut arrived = false;
    let mut critical_now = false;
    match kind {
        "status" => {
            let cat = v["cat"].as_str().unwrap_or("custom").to_string();
            let key = cat_key(&cat).to_string();
            // the integration's own texts, else the panel's pack for the category (picked once per state)
            let pick = |own: String, place: &str| if own.is_empty() { texts::fresh(&format!("status.{key}.{place}")) } else { own };
            // the screen's own colour, else a deep teal (white text never stands on Signal)
            let tint = v["colour"].as_str().and_then(crate::pages::parse_color).unwrap_or(Color::from_rgb_u8(0x0b, 0x72, 0x85));
            let button = v["button"].as_bool().unwrap_or(false);
            let countdown = v["countdown"].as_f64().filter(|m| *m > 0.0).map(|m| (m * 60.0) as u64);
            arrived = with_state(|s| {
                if let Some(old) = s.statuses.iter_mut().find(|x| x.id == id) {
                    // sent again (a reconnect): the same screen, the integration's changes only
                    old.tint = tint;
                    // the countdown it brings is what is left now: counted from now
                    if countdown.is_some() {
                        old.started = Instant::now();
                    }
                    old.countdown = countdown;
                    if !button {
                        old.button.clear();
                    }
                    return false;
                }
                let folded = !crate::dnd_allows(s, "status");
                let title = pick(text("headline"), "titles");
                let line = pick(text("line"), "lines");
                let strip = texts::fresh(&format!("status.{key}.strip"));
                s.statuses.push(Status {
                    id: id.clone(),
                    strip: if strip.is_empty() { title.clone() } else { strip },
                    title: if title.is_empty() { "Busy".into() } else { title },
                    line,
                    // an MDI icon ("mdi:…") or one of the panel's own pictograms
                    icon: if text("picto").starts_with("mdi:") { text("picto") } else { cat_icon(&cat).into() },
                    picto: if text("picto").starts_with("mdi:") { String::new() } else { text("picto") },
                    tint,
                    // a kind without its own button words still gets its button
                    button: if button { Some(pick(text("button_label"), "buttons")).filter(|b| !b.is_empty()).unwrap_or_else(|| "OK".into()) } else { String::new() },
                    countdown,
                    since: now(),
                    started: Instant::now(),
                    folded,
                    closed: false,
                });
                !folded
            });
        }
        "status_clear" => with_state(|s| s.statuses.retain(|x| x.id != id)),
        "critical" => {
            let c = Critical {
                id: id.clone(),
                title: text("title"),
                message: text("message"),
                icon: if text("icon").is_empty() { "alert".into() } else { text("icon") },
                source: text("source"),
                hold: v["hold"].as_bool().unwrap_or(true),
                silenced: false,
                since: now(),
                sound: v["sound"].as_bool().unwrap_or(true),
                tone: v["tone"].as_str().unwrap_or("auto").chars().take(120).collect(),
            };
            critical_now = with_state(|s| {
                // the same alert sent again keeps its silence
                if s.critical.as_ref().is_some_and(|o| o.id == c.id) {
                    return false;
                }
                s.critical = Some(c);
                true
            });
        }
        "critical_clear" => with_state(|s| {
            if s.critical.as_ref().is_some_and(|c| c.id == id || id.is_empty()) {
                s.critical = None;
            }
        }),
        _ => return,
    }
    if arrived || critical_now {
        crate::wake(ui);
    }
    if critical_now && !crate::go_to_page(ui, "info") {
        crate::open_temporary_page(ui, "info");
    }
    refresh(ui);
}

/// Taps: `status.fold`, `strip.open`, `strip.close`, `info.crit-silence`.
pub fn act(ui: &AppWindow, what: &str) {
    let mut to_info = false;
    let mut note_strip = false;
    with_state(|s| match what {
        "status.fold" => {
            if let Some(t) = s.statuses.last_mut() {
                t.folded = true;
            }
        }
        "strip.open" => {
            if s.critical.is_some() {
                to_info = true;
            } else if let Some(t) = s.statuses.last_mut() {
                t.folded = false;
            }
        }
        // the strip's ×: a folded status screen's strip goes (a critical alert's strip has none); a notification's strip
        // (the integration's "strip" for new notifications) just closes
        "strip.close" => {
            let critical_strip = s.critical.as_ref().is_some_and(|c| c.silenced);
            if !s.alert_strip {
                note_strip = true;
            } else if let Some(t) = s.statuses.last_mut().filter(|t| t.folded && !critical_strip) {
                t.closed = true;
            }
        }
        "info.crit-silence" => {
            if let Some(c) = s.critical.as_mut() {
                c.silenced = true;
            }
        }
        _ => {}
    });
    if note_strip {
        ui.set_strip_on(false);
    }
    if to_info && !crate::go_to_page(ui, "info") {
        crate::open_temporary_page(ui, "info");
    }
    refresh(ui);
}

/// The screens and the strip from the active ones (on every change and once a second); the critical
/// alert's sound while it sounds (not held, not silent).
pub fn refresh(ui: &AppWindow) -> Option<String> {
    let (top, crit, strip_was) = with_state(|s| {
        s.statuses.retain(|x| x.countdown.map_or(true, |c| x.started.elapsed().as_secs() < c));
        (s.statuses.last().cloned(), s.critical.clone(), s.alert_strip)
    });
    // the critical alert on the notifications page, sounding until held
    let crit_on = crit.as_ref().is_some_and(|c| !c.silenced);
    ui.set_info_critical(crit_on);
    if let Some(c) = &crit {
        ui.set_info_crit_title(c.title.as_str().into());
        ui.set_info_crit_message(c.message.as_str().into());
        ui.set_info_crit_icon(glyph(&c.icon).into());
        ui.set_info_crit_source(format!("{} · {}", c.source, minutes(c.since)).trim_start_matches(" · ").into());
        ui.set_info_crit_button(c.hold);
    }
    // a status screen over every page (not over a sounding critical alert)
    let screen = top.as_ref().filter(|t| !t.folded && !crit_on);
    if let Some(t) = screen {
        let (picto, small) = picto(&t.picto);
        let has = picto.size().width > 0;
        let (left, left_text) = match t.countdown {
            Some(c) => {
                let rest = c.saturating_sub(t.started.elapsed().as_secs());
                (rest as f32 / c.max(1) as f32, format!("{}:{:02} left", rest / 60, rest % 60))
            }
            None => (-1.0, String::new()),
        };
        let screen = StatusScreen {
            title: t.title.as_str().into(),
            message: t.line.as_str().into(),
            icon: glyph(&t.icon).into(),
            picto,
            picto_small: small,
            has_picto: has,
            tint: t.tint,
            since: format!("since {} · {}", local_format(t.since, "%H:%M"), minutes(t.since)).into(),
            button: t.button.as_str().into(),
            left,
            left_text: left_text.into(),
        };
        if ui.get_status_screen() != screen {
            ui.set_status_screen(screen);
        }
    }
    ui.set_status_on(screen.is_some());
    // the strip: a silenced critical alert (red), else a folded status screen
    let strip = match (&crit, &top) {
        (Some(c), _) if c.silenced => Some((c.title.clone(), minutes(c.since), glyph(&c.icon), Default::default(), false, Color::from_rgb_u8(0x8e, 0x0d, 0x14), true)),
        (_, Some(t)) if t.folded && !t.closed => {
            let small = picto(&t.picto).1;
            let has = small.size().width > 0;
            let sub = match t.countdown {
                Some(c) => {
                    let rest = c.saturating_sub(t.started.elapsed().as_secs());
                    format!("{}:{:02} left", rest / 60, rest % 60)
                }
                None => minutes(t.since),
            };
            Some((t.strip.clone(), sub, glyph(&t.icon), small, has, t.tint, false))
        }
        _ => None,
    };
    match strip {
        Some((title, sub, icon, picto, has, tint, critical)) => {
            ui.set_strip_title(title.into());
            ui.set_strip_sub(sub.into());
            ui.set_strip_icon(icon.into());
            ui.set_strip_picto(picto);
            ui.set_strip_has_picto(has);
            ui.set_strip_tint(tint);
            ui.set_strip_critical(critical);
            ui.set_strip_on(true);
            with_state(|s| s.alert_strip = true);
        }
        // the strip goes only if it was ours (a message from Home Assistant keeps its own)
        None if strip_was => {
            ui.set_strip_on(false);
            with_state(|s| s.alert_strip = false);
        }
        None => {}
    }
    crit.filter(|c| crit_on && c.sound).map(|c| c.tone)
}
