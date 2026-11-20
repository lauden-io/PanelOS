//! Music page player picker: which player the page shows (remembered on the
//! panel), moving the music to another one, playing it there too. The list is
//! read when the picker opens (Home Assistant's REST states: Music Assistant
//! players, else every media player), in a thread.

use std::io::Read;
use std::time::Duration;

use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{icons, with_state, AppWindow, MusicPlayer};

thread_local! {
    /// The picker's rows: entity ids in the order shown.
    static IDS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn open(ui: &AppWindow) {
    ui.set_music_picker(true);
    let (url, auth, active) = with_state(|s| (s.config.as_ref().map(|c| c.ha_url.clone()), s.auth.clone(), crate::media_entity(s)));
    let (Some(url), Some(auth)) = (url, auth) else { return };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let Ok(token) = auth.access_token() else { return };
        // Only the media players, from the PanelOS integration (the whole state list is too big for
        // the panel's Wi-Fi; a template would need an administrator login, the panel has an ordinary one)
        let resp = ureq::get(&format!("{url}/api/panelos/players"))
            .set("Authorization", &format!("Bearer {token}"))
            .timeout(Duration::from_secs(10))
            .call();
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                eprintln!("nspanel-app: music players: {e}");
                return;
            }
        };
        let mut body = String::new();
        if resp.into_reader().take(2 * 1024 * 1024).read_to_string(&mut body).is_err() {
            return;
        }
        let all: Vec<Value> = match serde_json::from_str::<Value>(&body) {
            Ok(Value::Array(a)) => a,
            other => {
                eprintln!("nspanel-app: music players: unexpected answer ({:?})", other.err());
                return;
            }
        };
        // A missing attribute comes back as null: treat it as absent.
        let all: Vec<Value> = all
            .into_iter()
            .map(|mut e| {
                if let Some(a) = e["attributes"].as_object_mut() {
                    a.retain(|_, v| !v.is_null());
                }
                e
            })
            .collect();
        let players: Vec<&Value> = all
            .iter()
            .filter(|e| e["entity_id"].as_str().is_some_and(|i| i.starts_with("media_player.")) && e["state"] != "unavailable")
            .collect();
        let mass: Vec<&Value> = players.iter().copied().filter(|e| e["attributes"].get("mass_player_type").is_some()).collect();
        let mut list = if mass.is_empty() { players } else { mass };
        // the panel itself (its Sendspin player in Music Assistant, marked by the integration) comes first
        let own = crate::sendspin::client_id().unwrap_or_default();
        let is_self = |e: &Value| !own.is_empty() && e["attributes"]["sendspin_id"].as_str() == Some(own.as_str());
        list.sort_by_key(|e| !is_self(e));
        eprintln!("nspanel-app: music players: {}, this panel {}", list.len(), if list.first().is_some_and(|e| is_self(e)) { "first" } else { "not listed" });
        let group: Vec<String> = list
            .iter()
            .find(|e| e["entity_id"] == active.as_str())
            .and_then(|e| e["attributes"]["group_members"].as_array())
            .map(|g| g.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        // what a player has on: its track while it plays or is paused (idle with the track kept counts as paused)
        let track = |e: &Value| -> (String, bool) {
            let a = &e["attributes"];
            let state = e["state"].as_str().unwrap_or("");
            let shows = matches!(state, "playing" | "paused" | "buffering") || (state == "idle" && a["media_title"].as_str().is_some_and(|t| !t.is_empty()));
            let now = match (a["media_title"].as_str(), a["media_artist"].as_str()) {
                (Some(t), Some(ar)) if shows && !t.is_empty() => format!("{t} · {ar}"),
                (Some(t), _) if shows && !t.is_empty() => t.to_string(),
                _ => String::new(),
            };
            (now, state == "playing")
        };
        // the group's music (the shown player leads it): its members show it too
        let lead = list.iter().find(|e| e["entity_id"] == active.as_str()).map(|e| track(e)).unwrap_or_default();
        let rows: Vec<(String, String, String, bool, bool, String)> = list
            .iter()
            .map(|e| {
                let id = e["entity_id"].as_str().unwrap_or("").to_string();
                let a = &e["attributes"];
                let name = if is_self(e) { "This panel".to_string() } else { a["friendly_name"].as_str().unwrap_or(&id).to_string() };
                let grouped = group.iter().any(|g| *g == e["entity_id"]);
                let (mut now, mut playing) = track(e);
                if now.is_empty() && grouped && id != active {
                    (now, playing) = lead.clone();
                }
                let icon = if is_self(e) { "tablet" } else if a["device_class"] == "tv" { "television" } else { "speaker" };
                (id, name, icon.to_string(), playing, grouped, now)
            })
            .collect();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let players: Vec<MusicPlayer> = rows
                .iter()
                .map(|(id, name, icon, playing, grouped, now)| MusicPlayer {
                    name: name.as_str().into(),
                    icon: icons::glyph(icon).map(String::from).unwrap_or_default().into(),
                    active: *id == active,
                    grouped: *grouped && *id != active,
                    playing: *playing,
                    now: now.as_str().into(),
                })
                .collect();
            IDS.with(|i| *i.borrow_mut() = rows.into_iter().map(|r| r.0).collect());
            ui.set_music_players(ModelRc::new(VecModel::from(players)));
        });
    });
}

/// Taps on the music page's picker (`music.picker`, `music.show` …).
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    let id = value.parse::<usize>().ok().and_then(|i| IDS.with(|ids| ids.borrow().get(i).cloned()));
    match what {
        "music.picker" => open(ui),
        "music.picker-close" => ui.set_music_picker(false),
        "music.show" => {
            let Some(id) = id else { return };
            with_state(|s| {
                s.settings.music_player = id;
                s.settings.save();
                // the holds of the player shown before do not carry over to this one
                s.music_hold = None;
                s.music_skip = None;
            });
            ui.set_music_picker(false);
            crate::apply_panel_config(ui);
        }
        // Music Assistant moves the queue; the page follows the music.
        "music.transfer" => {
            let Some(id) = id else { return };
            let from = with_state(|s| crate::media_entity(s));
            crate::call("music_assistant", "transfer_queue", &id, json!({ "source_player": from }));
            with_state(|s| {
                s.settings.music_player = id;
                s.settings.save();
                // the holds of the player shown before do not carry over to this one
                s.music_hold = None;
                s.music_skip = None;
            });
            ui.set_music_picker(false);
            crate::apply_panel_config(ui);
        }
        "music.group" => {
            let Some(id) = id else { return };
            let active = with_state(|s| crate::media_entity(s));
            let grouped = ui.get_music_players().iter().enumerate().any(|(i, p)| p.grouped && IDS.with(|ids| ids.borrow().get(i) == Some(&id)));
            if grouped {
                crate::call("media_player", "unjoin", &id, json!({}));
            } else {
                crate::call("media_player", "join", &active, json!({ "group_members": [id] }));
            }
            // The list again once Home Assistant has it.
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(1500), move || {
                if let Some(ui) = weak.upgrade() {
                    if ui.get_music_picker() {
                        open(&ui);
                    }
                }
            });
        }
        _ => {}
    }
}
