//! Home Assistant WebSocket client (own thread): login, dashboard config,
//! live states of the dashboard's entities (`subscribe_entities`, compressed
//! updates), weather forecasts, service calls from the UI, and the PanelOS
//! integration: its settings, show requests and entity commands come in, the
//! panel's device states go out (`panelos/report`, see `panel_device.rs`).

use std::collections::{HashMap, HashSet};
use std::net::TcpStream;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::auth::{Auth, TokenError};

#[derive(Debug, Clone, Default)]
pub struct EntityState {
    pub state: String,
    pub attrs: Map<String, Value>,
}

pub enum Command {
    CallService { domain: String, service: String, entity_id: String, data: Value },
    /// Start a new session (dashboard or extra entities changed).
    Reload,
    /// Ask for the list of dashboards (answered with Event::Dashboards).
    ListDashboards,
    /// Picture of the screen (PNG, base64) asked for by the PanelOS page.
    Screen(String),
    /// A page's look or options set on the panel (its page sheet): the integration keeps them.
    PageSet { index: usize, variant: Option<i32>, options: Value },
    /// A timer started or changed on the panel: Home Assistant's own timer of the panel's voice device
    /// (start / pause / resume / add / cancel); a start Home Assistant refuses runs on the panel instead.
    Timer { action: String, timer_id: String, seconds: u32, name: String },
    /// An answer to one of PanelOS's notifications (ok, read, dismiss, button with its index).
    Notification { id: String, answer: String, button: Option<i32> },
    /// Any other websocket command of Home Assistant (its type and data; the id is added).
    Raw(Value),
}

pub enum Event {
    Status { text: String, connected: bool },
    NeedPairing,
    Dashboard(Value),
    States(Vec<(String, Option<EntityState>)>),
    /// (url_path, title); "" = the default dashboard.
    Dashboards(Vec<(String, String)>),
    /// Assistant mode settings from the PanelOS integration; None = the
    /// integration is not installed (or has no entry for this panel).
    Panel(Option<Value>),
    /// show_view / show_message request from the PanelOS integration.
    Show(Value),
    /// Command from an entity of the panel's device (PanelOS integration).
    Command { key: String, value: Value },
    /// The timers running on the panel in Home Assistant (sent when the panel subscribes).
    Timers(Vec<Value>),
    /// Home Assistant could not start a timer for the panel (an older integration, no timer device): the
    /// panel runs it itself.
    TimerRefused { seconds: u32, name: String },
    /// Weather forecast (hourly or daily) of the forecast entity.
    Forecast { hourly: bool, items: Vec<Value> },
    /// Persistent notifications: "current" (all of them), "added",
    /// "updated" or "removed", with the notifications by id.
    Notes { kind: String, notes: Value },
}

/// The panel's name for the PanelOS integration (its ESPHome name).
pub const PANEL_ID: &str = "nspanel-pro";

/// Settings the UI thread can change while the client runs.
#[derive(Default)]
pub struct Shared {
    pub dashboard: std::sync::Mutex<String>,
    pub extra_entities: std::sync::Mutex<Vec<String>>,
    /// The entities the running session subscribed to (the UI thread asks for a new session when a page needs one
    /// that is missing).
    pub subscribed: std::sync::Mutex<Vec<String>>,
    /// The UI thread asked for a new session for the pages' entities; the new session clears it.
    pub reload_asked: std::sync::atomic::AtomicBool,
    /// Weather entity whose forecasts are followed ("" = none).
    pub forecast_entity: std::sync::Mutex<String>,
    /// What the panel's device in Home Assistant shows; filled by the UI
    /// thread (the ESPHome API reads the voice part of it).
    pub device: Arc<std::sync::Mutex<crate::esphome::DeviceState>>,
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// Every entity id mentioned in a dashboard config ("entity" keys, "entities" lists).
pub fn entities_in(config: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut HashSet<String>) {
        match v {
            Value::Object(map) => {
                for (k, val) in map {
                    match (k.as_str(), val) {
                        ("entity", Value::String(s)) => {
                            out.insert(s.clone());
                        }
                        ("entities", Value::Array(items)) => {
                            for item in items {
                                match item {
                                    Value::String(s) => {
                                        out.insert(s.clone());
                                    }
                                    other => walk(other, out),
                                }
                            }
                        }
                        _ => walk(val, out),
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            _ => {}
        }
    }
    let mut set = HashSet::new();
    walk(config, &mut set);
    let mut v: Vec<String> = set.into_iter().filter(|e| e.contains('.')).collect();
    v.sort();
    v
}

fn read_json(ws: &mut Socket) -> Result<Option<Value>, String> {
    match ws.read() {
        Ok(Message::Text(t)) => serde_json::from_str(t.as_str()).map(Some).map_err(|e| e.to_string()),
        Ok(Message::Close(_)) => Err("connection closed".into()),
        Ok(_) => Ok(None),
        Err(tungstenite::Error::Io(e))
            if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
        {
            Ok(None)
        }
        Err(e) => Err(e.to_string()),
    }
}

fn send_json(ws: &mut Socket, v: &Value) -> Result<(), String> {
    ws.send(Message::text(v.to_string())).map_err(|e| e.to_string())
}

fn set_timeout(ws: &mut Socket, t: Option<Duration>) {
    if let MaybeTlsStream::Plain(s) = ws.get_mut() {
        let _ = s.set_read_timeout(t);
        // a send that cannot go out (the network gone, its buffer full) fails instead of waiting for
        // ever: the link is noticed as lost and made again
        let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
    }
}

/// Applies one `subscribe_entities` event to the state map; returns the changed entities.
fn apply_compressed(states: &mut HashMap<String, EntityState>, ev: &Value) -> Vec<(String, Option<EntityState>)> {
    let mut changed = Vec::new();
    if let Some(added) = ev.get("a").and_then(Value::as_object) {
        for (id, v) in added {
            let st = EntityState {
                state: v.get("s").and_then(Value::as_str).unwrap_or_default().to_string(),
                attrs: v.get("a").and_then(Value::as_object).cloned().unwrap_or_default(),
            };
            states.insert(id.clone(), st.clone());
            changed.push((id.clone(), Some(st)));
        }
    }
    if let Some(diffs) = ev.get("c").and_then(Value::as_object) {
        for (id, d) in diffs {
            let st = states.entry(id.clone()).or_default();
            if let Some(plus) = d.get("+") {
                if let Some(s) = plus.get("s").and_then(Value::as_str) {
                    st.state = s.to_string();
                }
                if let Some(a) = plus.get("a").and_then(Value::as_object) {
                    for (k, v) in a {
                        st.attrs.insert(k.clone(), v.clone());
                    }
                }
            }
            if let Some(minus) = d.get("-").and_then(|m| m.get("a")).and_then(Value::as_array) {
                for k in minus.iter().filter_map(Value::as_str) {
                    st.attrs.remove(k);
                }
            }
            changed.push((id.clone(), Some(st.clone())));
        }
    }
    if let Some(removed) = ev.get("r").and_then(Value::as_array) {
        for id in removed.iter().filter_map(Value::as_str) {
            states.remove(id);
            changed.push((id.to_string(), None));
        }
    }
    changed
}

fn session(
    ws_url: &str,
    shared: &Shared,
    auth: &Auth,
    commands: &Receiver<Command>,
    emit: &dyn Fn(Event),
) -> Result<(), String> {
    let dashboard_name = shared.dashboard.lock().unwrap().clone();
    let dashboard = dashboard_name.as_str();
    let url_path = if dashboard.is_empty() { Value::Null } else { Value::String(dashboard.to_string()) };
    let token = match auth.access_token() {
        Ok(t) => t,
        Err(TokenError::NotPaired) => {
            emit(Event::NeedPairing);
            return Err("not paired".into());
        }
        Err(TokenError::Retry(e)) => return Err(e),
    };

    let (mut ws, response) = tungstenite::connect(ws_url).map_err(|e| format!("connect: {e}"))?;
    // No battery-backed clock: until NTP has run, take Home Assistant's time.
    if let Some(date) = response.headers().get("date").and_then(|v| v.to_str().ok()) {
        if crate::clock::set_from_http_date(date) {
            eprintln!("nspanel-app: clock set from Home Assistant ({date})");
        }
    }
    set_timeout(&mut ws, Some(Duration::from_secs(15)));
    let hello = read_json(&mut ws)?.ok_or("no greeting")?;
    if hello["type"] != "auth_required" {
        return Err(format!("unexpected greeting: {hello}"));
    }
    send_json(&mut ws, &json!({"type": "auth", "access_token": token}))?;
    let answer = read_json(&mut ws)?.ok_or("no auth answer")?;
    if answer["type"] != "auth_ok" {
        auth.invalidate_access();
        return Err(format!("login rejected: {}", answer["message"]));
    }

    let mut next_id: u64 = 1;
    // Who the panel is logged in as: the user's id goes to Home Assistant as the Panel user sensor of the panel's
    // ESPHome device, and the PanelOS integration accepts commands as this panel only from that user (and
    // administrators). Asked first, so the sensor is usually there before the panel subscribes.
    let user_id = next_id;
    send_json(&mut ws, &json!({"id": user_id, "type": "auth/current_user"}))?;
    next_id += 1;
    let dash_id = next_id;
    send_json(&mut ws, &json!({"id": dash_id, "type": "lovelace/config", "url_path": url_path}))?;
    let config = loop {
        let msg = read_json(&mut ws)?.ok_or("no dashboard answer")?;
        if msg["id"] == user_id {
            take_user(&msg, &shared.device);
        }
        if msg["id"] == dash_id {
            if msg["success"] != true {
                return Err(format!("dashboard '{dashboard}': {}", msg["error"]["message"]));
            }
            break msg["result"].clone();
        }
    };
    let mut entities = entities_in(&config);
    for extra in shared.extra_entities.lock().unwrap().iter() {
        if !extra.is_empty() && !entities.contains(extra) {
            entities.push(extra.clone());
        }
    }
    *shared.subscribed.lock().unwrap() = entities.clone();
    shared.reload_asked.store(false, std::sync::atomic::Ordering::SeqCst);
    emit(Event::Dashboard(config));

    next_id += 1;
    let sub_id = next_id;
    send_json(&mut ws, &json!({"id": sub_id, "type": "subscribe_entities", "entity_ids": entities}))?;
    // Dashboard edits in Home Assistant: reload the config (and the entity list).
    next_id += 1;
    let lovelace_id = next_id;
    send_json(&mut ws, &json!({"id": lovelace_id, "type": "subscribe_events", "event_type": "lovelace_updated"}))?;
    // Assistant mode settings and show requests (own integration; an
    // "unknown command" error means it is not installed).
    next_id += 1;
    let mut panel_id = next_id;
    send_json(&mut ws, &json!({"id": panel_id, "type": "panelos/subscribe", "panel_id": PANEL_ID}))?;
    // Home Assistant's notifications (the info page).
    next_id += 1;
    let notes_id = next_id;
    send_json(&mut ws, &json!({"id": notes_id, "type": "persistent_notification/subscribe"}))?;
    // Retried every minute while the integration is missing, so installing
    // it needs no panel restart.
    let mut panel_retry_at: Option<Instant> = None;
    // Weather forecasts (no longer a state attribute in Home Assistant).
    let forecast_entity = shared.forecast_entity.lock().unwrap().clone();
    let mut forecast_ids: Vec<(u64, bool)> = Vec::new();
    if !forecast_entity.is_empty() {
        for (kind, hourly) in [("hourly", true), ("daily", false)] {
            next_id += 1;
            forecast_ids.push((next_id, hourly));
            send_json(
                &mut ws,
                &json!({"id": next_id, "type": "weather/subscribe_forecast", "entity_id": forecast_entity, "forecast_type": kind}),
            )?;
        }
    }
    emit(Event::Status { text: "Connected".into(), connected: true });

    set_timeout(&mut ws, Some(Duration::from_millis(150)));
    let mut states: HashMap<String, EntityState> = HashMap::new();
    let mut last_rx = Instant::now();
    let mut alive_at = Instant::now();
    let mut alive_id = 0u64;
    let mut log_at = Instant::now();
    let mut log_id = 0u64;
    let mut dashboards_request: Option<u64> = None;
    // The panel's device (PanelOS integration): reported once the
    // subscription is accepted, then only what changed.
    let mut report = Report::default();
    // timer starts asked of Home Assistant: request id -> (seconds, name), so a refused one runs on the panel
    let mut timer_starts: std::collections::HashMap<u64, (u32, String)> = std::collections::HashMap::new();
    // page settings sent and not answered yet (request ids): a new session asked for meanwhile waits for their answers
    // (at most 2 s), so a setting made on the panel just before is not lost with this connection
    let mut page_sets: Vec<u64> = Vec::new();
    let mut reload_at: Option<Instant> = None;
    loop {
        if reload_at.is_some_and(|t| page_sets.is_empty() || Instant::now() >= t) {
            return Ok(());
        }
        while let Ok(cmd) = commands.try_recv() {
            match cmd {
                Command::CallService { domain, service, entity_id, data } => {
                    next_id += 1;
                    let mut msg = json!({"id": next_id, "type": "call_service", "domain": domain, "service": service, "service_data": data});
                    // Services without a target (persistent_notification.dismiss …) get none.
                    if !entity_id.is_empty() {
                        msg["target"] = json!({ "entity_id": entity_id });
                    }
                    send_json(&mut ws, &msg)?;
                }
                Command::Raw(mut msg) => {
                    next_id += 1;
                    msg["id"] = json!(next_id);
                    send_json(&mut ws, &msg)?;
                }
                Command::Reload if page_sets.is_empty() => return Ok(()),
                Command::Reload => {
                    reload_at.get_or_insert(Instant::now() + Duration::from_secs(2));
                }
                Command::ListDashboards => {
                    next_id += 1;
                    dashboards_request = Some(next_id);
                    send_json(&mut ws, &json!({"id": next_id, "type": "lovelace/dashboards/list"}))?;
                }
                Command::Screen(png) if report.linked => {
                    next_id += 1;
                    let t = Instant::now();
                    let sent = send_json(&mut ws, &json!({"id": next_id, "type": "panelos/screen", "panel_id": PANEL_ID, "jpeg": png}));
                    crate::SCREEN_SEND_MS.store(t.elapsed().as_millis() as u64, std::sync::atomic::Ordering::SeqCst);
                    crate::SCREENS_SENT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    sent?;
                }
                Command::Screen(_) => {
                    crate::SCREENS_SENT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                Command::Timer { action, timer_id, seconds, name } => {
                    next_id += 1;
                    let mut msg = json!({"id": next_id, "type": "panelos/timer", "panel_id": PANEL_ID, "action": action});
                    if !timer_id.is_empty() {
                        msg["timer_id"] = json!(timer_id);
                    }
                    if seconds > 0 {
                        msg["seconds"] = json!(seconds);
                    }
                    if !name.is_empty() {
                        msg["name"] = json!(name);
                    }
                    if action == "start" {
                        timer_starts.insert(next_id, (seconds, name));
                    }
                    if report.linked {
                        send_json(&mut ws, &msg)?;
                    } else if action == "start" {
                        // no integration link: the panel runs it
                        timer_starts.remove(&next_id);
                        emit(Event::TimerRefused { seconds, name: msg["name"].as_str().unwrap_or("").to_string() });
                    }
                }
                Command::Notification { id, answer, button } => {
                    next_id += 1;
                    let mut msg = json!({"id": next_id, "type": "panelos/notification", "panel_id": PANEL_ID, "notification_id": id, "answer": answer});
                    if let Some(b) = button {
                        msg["button"] = json!(b);
                    }
                    if report.linked {
                        send_json(&mut ws, &msg)?;
                    }
                }
                Command::PageSet { index, variant, options } => {
                    next_id += 1;
                    let mut msg = json!({"id": next_id, "type": "panelos/page_set", "panel_id": PANEL_ID, "index": index});
                    if let Some(v) = variant {
                        msg["variant"] = json!(v);
                    }
                    if let Some(o) = options.as_object() {
                        // the page's entities go beside its options (the music page's player picked on the panel)
                        let mut o = o.clone();
                        if let Some(e) = o.remove("entities") {
                            msg["entities"] = e;
                        }
                        if !o.is_empty() {
                            msg["options"] = Value::Object(o);
                        }
                    }
                    send_json(&mut ws, &msg)?;
                    page_sets.push(next_id);
                }
            }
        }

        match read_json(&mut ws)? {
            Some(msg) => {
                last_rx = Instant::now();
                let report_answer = msg["type"] == "result" && report.answered(msg["id"].as_u64().unwrap_or(0));
                match msg["type"].as_str() {
                    Some("result") if report_answer => {
                        if msg["success"] == false {
                            let why = msg["error"]["message"].as_str().unwrap_or("").to_string();
                            eprintln!("nspanel-app: device report not accepted: {why}");
                            if why.contains("alarms") && !report.no_alarms {
                                // An integration from before the alarms: everything again, without them.
                                report = Report { linked: true, no_alarms: true, no_music_player: report.no_music_player, ..Default::default() };
                            } else if why.contains("music_player") && !report.no_music_player {
                                // An integration from before the music player: everything again, without it.
                                report = Report { linked: true, no_alarms: report.no_alarms, no_music_player: true, ..Default::default() };
                            } else {
                                // An older integration without panelos/report.
                                report.linked = false;
                            }
                        }
                    }
                    Some("event") if msg["id"] == sub_id => {
                        let changed = apply_compressed(&mut states, &msg["event"]);
                        if !changed.is_empty() {
                            emit(Event::States(changed));
                        }
                    }
                    Some("event") if msg["id"] == lovelace_id => {
                        let changed = msg["event"]["data"]["url_path"].as_str().unwrap_or("");
                        if changed == dashboard {
                            // Start a fresh session: new config, new subscriptions.
                            return Ok(());
                        }
                    }
                    Some("event") if msg["id"] == panel_id => {
                        let ev = &msg["event"];
                        if let Some(config) = ev.get("config") {
                            let found = config.as_object().is_some_and(|m| !m.is_empty());
                            emit(Event::Panel(found.then(|| config.clone())));
                        }
                        if let Some(show) = ev.get("show") {
                            emit(Event::Show(show.clone()));
                        }
                        if let Some(list) = ev.get("timers").and_then(Value::as_array) {
                            emit(Event::Timers(list.clone()));
                        }
                        if let Some(key) = ev["command"]["key"].as_str() {
                            emit(Event::Command { key: key.to_string(), value: ev["command"]["value"].clone() });
                        }
                    }
                    Some("result") if msg["id"] == panel_id && msg["success"] == false => {
                        emit(Event::Panel(None));
                        // Home Assistant may still be loading the integration: asked again soon
                        panel_retry_at = Some(Instant::now() + Duration::from_secs(10));
                    }
                    Some("result") if msg["id"] == panel_id => report = Report { linked: true, ..Default::default() },
                    Some("result") if msg["id"] == user_id => take_user(&msg, &shared.device),
                    Some("result") if msg["id"].as_u64().is_some_and(|id| timer_starts.contains_key(&id)) => {
                        if let Some((seconds, name)) = msg["id"].as_u64().and_then(|id| timer_starts.remove(&id)) {
                            if msg["success"] != true {
                                eprintln!("nspanel-app: Home Assistant did not start the timer ({}): the panel runs it", msg["error"]["message"]);
                                emit(Event::TimerRefused { seconds, name });
                            }
                        }
                    }
                    Some("result") if msg["id"].as_u64().is_some_and(|id| page_sets.contains(&id)) => {
                        page_sets.retain(|id| msg["id"].as_u64() != Some(*id));
                        if msg["success"] != true {
                            eprintln!("nspanel-app: page setting not accepted: {}", msg["error"]["message"]);
                        }
                    }
                    Some("event") if msg["id"] == notes_id => {
                        let ev = &msg["event"];
                        emit(Event::Notes {
                            kind: ev["type"].as_str().unwrap_or("").to_string(),
                            notes: ev["notifications"].clone(),
                        });
                    }
                    Some("event") if forecast_ids.iter().any(|(id, _)| msg["id"] == *id) => {
                        let hourly = forecast_ids.iter().any(|(id, h)| msg["id"] == *id && *h);
                        let items = msg["event"]["forecast"].as_array().cloned().unwrap_or_default();
                        emit(Event::Forecast { hourly, items });
                    }
                    Some("result") if msg["success"] == false && forecast_ids.iter().any(|(id, _)| msg["id"] == *id) => {
                        // This weather entity does not offer this forecast type.
                    }
                    Some("result") if Some(msg["id"].as_u64().unwrap_or(0)) == dashboards_request => {
                        let mut list = vec![(String::new(), "Overview".to_string())];
                        for d in msg["result"].as_array().into_iter().flatten() {
                            if let Some(url) = d["url_path"].as_str() {
                                let title = d["title"].as_str().unwrap_or(url).trim().to_string();
                                list.push((url.to_string(), title));
                            }
                        }
                        emit(Event::Dashboards(list));
                    }
                    // the keep-alive's answer (an older integration without it answers with an error)
                    Some("result") if msg["id"].as_u64() == Some(alive_id) || msg["id"].as_u64() == Some(log_id) => {}
                    Some("result") if msg["success"] == false => {
                        eprintln!("nspanel-app: request {} failed: {}", msg["id"], msg["error"]);
                    }
                    _ => {}
                }
            }
            None => {
                let _ = ws.flush();
            }
        }

        if let Some(msg) = report.next(&shared.device) {
            next_id += 1;
            let mut msg = msg;
            msg["id"] = next_id.into();
            send_json(&mut ws, &msg)?;
            report.sent(next_id);
        }

        if panel_retry_at.is_some_and(|t| Instant::now() >= t) {
            panel_retry_at = None;
            next_id += 1;
            panel_id = next_id;
            send_json(&mut ws, &json!({"id": panel_id, "type": "panelos/subscribe", "panel_id": PANEL_ID}))?;
        }

        // Keep-alive, both ways, every second: "panelos/alive" to the integration (which notes it and
        // answers; an older one answers with an error, which counts the same), or Home Assistant's own
        // ping before the integration took the subscription. Nothing heard for 8 s: the link is gone
        // (the status line shows it at once, the offline screen after its usual wait). Not less: Home
        // Assistant itself stalls now and then (up to 2.3 s measured on a wired client), plus the Wi-Fi.
        if alive_at.elapsed() >= Duration::from_secs(1) {
            next_id += 1;
            let msg = if report.linked {
                json!({"id": next_id, "type": "panelos/alive", "panel_id": PANEL_ID})
            } else {
                json!({"id": next_id, "type": "ping"})
            };
            send_json(&mut ws, &msg)?;
            alive_at = Instant::now();
            alive_id = next_id;
        }
        if last_rx.elapsed() > Duration::from_secs(8) {
            return Err("nothing from Home Assistant for 8 s".into());
        }
        // development install: the new system log lines go along with the keep-alive (the integration
        // keeps them in a file; lines from a time without a link wait on the panel)
        if report.linked && log_at.elapsed() >= Duration::from_secs(1) {
            log_at = Instant::now();
            let lines = crate::devlog::take(400);
            if !lines.is_empty() {
                next_id += 1;
                send_json(&mut ws, &json!({"id": next_id, "type": "panelos/log", "panel_id": PANEL_ID, "lines": lines}))?;
                log_id = next_id;
            }
        }
    }
}

/// The id and name of the user in Home Assistant's answer to `auth/current_user`; None for an error or an odd answer
/// (Home Assistant's user ids are 32 hex digits).
fn current_user(msg: &Value) -> Option<(String, String)> {
    if msg["success"] != true {
        return None;
    }
    let id = msg["result"]["id"].as_str()?;
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some((id.to_string(), msg["result"]["name"].as_str().unwrap_or("").to_string()))
}

/// Home Assistant said who the panel is logged in as: the Panel user sensor of the ESPHome device shows it.
fn take_user(msg: &Value, device: &std::sync::Mutex<crate::esphome::DeviceState>) {
    match current_user(msg) {
        Some((id, name)) => {
            crate::esphome::set_panel_user(device, &id, &name);
        }
        None => eprintln!("nspanel-app: Home Assistant did not say which user the panel is logged in as: {}", msg["error"]),
    }
}

/// What the panel's device in Home Assistant was last told, so a report
/// carries only changes. Checked at most every 200 ms.
#[derive(Default)]
struct Report {
    /// The integration accepted the panel's subscription.
    linked: bool,
    device_sent: bool,
    dashboard: Option<String>,
    /// The music player last reported.
    music_player: Option<String>,
    /// The integration does not take the music player (older than it).
    no_music_player: bool,
    states: Map<String, Value>,
    options: Map<String, Value>,
    /// The alarms last sent.
    alarms: Option<Value>,
    /// The integration does not take alarms (older than the alarms on its page).
    no_alarms: bool,
    pending: HashSet<u64>,
    checked: Option<Instant>,
}

impl Report {
    fn next(&mut self, device: &std::sync::Mutex<crate::esphome::DeviceState>) -> Option<Value> {
        if !self.linked || self.checked.is_some_and(|t| t.elapsed() < Duration::from_millis(200)) {
            return None;
        }
        self.checked = Some(Instant::now());
        let snapshot = device.lock().unwrap().clone();
        if !snapshot.ready {
            return None;
        }
        let changed = |now: Map<String, Value>, sent: &mut Map<String, Value>| {
            let diff: Map<String, Value> = now.into_iter().filter(|(k, v)| sent.get(k) != Some(v)).collect();
            sent.extend(diff.clone());
            diff
        };
        let states = changed(crate::panel_device::states(&snapshot), &mut self.states);
        let options = changed(crate::panel_device::options(&snapshot), &mut self.options);
        let dashboard = (self.dashboard.as_deref() != Some(snapshot.web_dashboard.as_str())).then(|| snapshot.web_dashboard.clone());
        let music_player =
            (!self.no_music_player && self.music_player.as_deref() != Some(snapshot.music_player.as_str())).then(|| snapshot.music_player.clone());
        let alarms = (!self.no_alarms && !snapshot.alarms.is_null() && self.alarms.as_ref() != Some(&snapshot.alarms)).then(|| snapshot.alarms.clone());
        if states.is_empty() && options.is_empty() && dashboard.is_none() && music_player.is_none() && alarms.is_none() && self.device_sent {
            return None;
        }
        let mut msg = json!({"type": "panelos/report", "panel_id": PANEL_ID});
        if let Some(d) = dashboard {
            msg["dashboard"] = d.clone().into();
            self.dashboard = Some(d);
        }
        if let Some(p) = music_player {
            msg["music_player"] = p.clone().into();
            self.music_player = Some(p);
        }
        if !self.device_sent {
            msg["device"] = crate::panel_device::device_info(&crate::esphome::mac_address().to_lowercase());
            self.device_sent = true;
        }
        if !options.is_empty() {
            msg["options"] = options.into();
        }
        if let Some(a) = alarms {
            msg["alarms"] = a.clone();
            self.alarms = Some(a);
        }
        if !states.is_empty() {
            msg["states"] = states.into();
        }
        Some(msg)
    }

    fn sent(&mut self, id: u64) {
        self.pending.insert(id);
    }

    /// True (once) for the answer to one of the reports.
    fn answered(&mut self, id: u64) -> bool {
        self.pending.remove(&id)
    }
}

/// Why a session failed, for the status line and the retry delay.
#[derive(Clone, Copy, PartialEq)]
enum Failure {
    /// No network (Wi-Fi not up yet, or gone).
    Offline,
    /// The network is there, Home Assistant does not answer.
    Unreachable,
    /// Home Assistant answered with an error (login, dashboard, …).
    Other,
}

fn classify(e: &str) -> Failure {
    if e.contains("os error 101") || e.contains("Network is unreachable") {
        Failure::Offline
    } else if e.starts_with("connect:")
        || e.contains("Connection Failed")
        || e.contains("os error 111")
        || e.contains("os error 113")
        || e.contains("timed out")
        || e.contains("Connection reset")
    {
        Failure::Unreachable
    } else {
        Failure::Other
    }
}

/// Runs the client for good: Home Assistant may be away for hours (power cut,
/// server down), and the panel must come back by itself.
///
/// Retry delays: without network every 0.5 s during the first minute after
/// the start (Wi-Fi comes up a few seconds after the app, and the first screen
/// waits for this connection), then every 5 s; while Home Assistant does not
/// answer 2, 4, 8, then every 10 s. A failure is logged when its cause
/// changes, not on every try, and the recovery with the length of the outage.
pub fn spawn(
    ws_url: String,
    shared: Arc<Shared>,
    auth: Arc<Auth>,
    commands: Receiver<Command>,
    emit: impl Fn(Event) + Send + 'static,
) {
    std::thread::spawn(move || {
        let client_started = Instant::now();
        let mut delay = 2;
        let mut reloading = false;
        // Current outage: since when, and the last logged cause. Cells: the
        // session reports its connection through the emit callback.
        let failing_since: std::cell::Cell<Option<Instant>> = std::cell::Cell::new(None);
        // the causes already logged in this outage (two causes taking turns are logged once each)
        let last_logged: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        loop {
            if !reloading && failing_since.get().is_none() {
                emit(Event::Status { text: "Connecting to Home Assistant…".into(), connected: false });
            }
            let started = Instant::now();
            let result = session(&ws_url, &shared, &auth, &commands, &|event| {
                if let Event::Status { connected: true, .. } = event {
                    if let Some(since) = failing_since.take() {
                        crate::connlog::note(&format!("Home Assistant connected again after {} s", since.elapsed().as_secs()));
                    }
                    last_logged.borrow_mut().clear();
                }
                emit(event)
            });
            // Ok means the dashboard or the settings changed: reconnect at once, quietly.
            reloading = result.is_ok();
            if reloading {
                eprintln!("nspanel-app: reloading the dashboard");
                continue;
            }
            let Err(e) = &result else { continue };
            if !auth.is_paired() {
                eprintln!("nspanel-app: Home Assistant session ended: {e}");
                emit(Event::NeedPairing);
                // Wait until a new login is stored.
                while !auth.is_paired() {
                    std::thread::sleep(Duration::from_secs(1));
                }
                continue;
            }
            let failure = classify(e);
            let cause = match failure {
                Failure::Offline => "no network".to_string(),
                _ => e.clone(),
            };
            if !last_logged.borrow().contains(&cause) {
                crate::connlog::note(&format!("Home Assistant session ended: {e}"));
                last_logged.borrow_mut().push(cause);
            }
            if failing_since.get().is_none() {
                failing_since.set(Some(Instant::now()));
            }
            let text = match failure {
                Failure::Offline => "Waiting for the network…".to_string(),
                Failure::Unreachable => "Home Assistant not reachable, retrying…".to_string(),
                Failure::Other => format!("Home Assistant: {e}"),
            };
            emit(Event::Status { text, connected: false });
            if failure == Failure::Offline {
                let wait = if client_started.elapsed() < Duration::from_secs(60) { 500 } else { 5000 };
                std::thread::sleep(Duration::from_millis(wait));
                continue;
            }
            if started.elapsed() > Duration::from_secs(60) {
                delay = 2;
            }
            std::thread::sleep(Duration::from_secs(delay));
            delay = (delay * 2).min(10);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_user_from_the_answer() {
        let ok = json!({"id": 1, "type": "result", "success": true,
            "result": {"id": "0123456789abcdef0123456789abcdef", "name": "Panel", "is_owner": false, "is_admin": false}});
        assert_eq!(current_user(&ok), Some(("0123456789abcdef0123456789abcdef".into(), "Panel".into())));
        // a user without a name
        let unnamed = json!({"id": 1, "type": "result", "success": true, "result": {"id": "abc123", "name": null}});
        assert_eq!(current_user(&unnamed), Some(("abc123".into(), String::new())));
    }

    #[test]
    fn current_user_refuses_errors_and_odd_ids() {
        let error = json!({"id": 1, "type": "result", "success": false, "error": {"code": "unknown_command", "message": "Unknown command."}});
        assert_eq!(current_user(&error), None);
        let no_id = json!({"id": 1, "type": "result", "success": true, "result": {"name": "Panel"}});
        assert_eq!(current_user(&no_id), None);
        let empty = json!({"id": 1, "type": "result", "success": true, "result": {"id": ""}});
        assert_eq!(current_user(&empty), None);
        let odd = json!({"id": 1, "type": "result", "success": true, "result": {"id": "abc 123"}});
        assert_eq!(current_user(&odd), None);
        let long = json!({"id": 1, "type": "result", "success": true, "result": {"id": "a".repeat(65)}});
        assert_eq!(current_user(&long), None);
        let number = json!({"id": 1, "type": "result", "success": true, "result": {"id": 42}});
        assert_eq!(current_user(&number), None);
    }
}
