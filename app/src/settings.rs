//! Panel settings changed on the panel (and later from Home Assistant):
//! `/mnt/persist/nspanel/settings.json`.

use serde::{Deserialize, Serialize};

use crate::config::PERSIST_DIR;

/// Screensaver timeouts offered in the settings, in seconds (0 = never).
pub const SCREENSAVER_CHOICES: &[(u32, &str)] =
    &[(0, "Never"), (30, "30 s"), (60, "1 min"), (120, "2 min"), (300, "5 min"), (600, "10 min"), (1800, "30 min")];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "dark", "light" or "auto" (sun below the horizon / dark room).
    pub theme: String,
    /// Manual backlight level, percent.
    pub brightness: u8,
    pub auto_brightness: bool,
    pub screensaver_seconds: u32,
    /// "clock" or "black".
    pub screensaver_style: String,
    pub proximity_wake: bool,
    /// Drops on the glass over the photo backgrounds when it rains.
    pub rain_on_glass: bool,
    /// Lightning (flashes and bolts) in the storm scenes.
    pub lightning: bool,
    /// Dashboard url_path; empty = the one in app.json.
    pub dashboard: String,
    /// Weather entity shown on the screensaver; empty = none.
    pub weather_entity: String,
    /// Active wake word model ids (first → Home Assistant's "Assistant",
    /// second → "Assistant 2"), as on ESPHome voice satellites.
    pub wake_words: Vec<String>,
    pub mic_muted: bool,
    /// Speaker volume, percent (codec digital volume: 100 % = 0 dB, 0 % = off).
    pub volume: u8,
    /// "dashboard" (a Home Assistant dashboard in the web view; the panel's pages without the web bundle) or
    /// "assistant" (the panel's own pages, full-screen conversation).
    pub display_mode: String,
    /// Last assistant mode configuration received from the PanelOS
    /// integration (kept so a restart starts with the right entities).
    pub assistant: AssistantConfig,
    /// How much the reported sensors must change before Home Assistant gets
    /// a new value (PanelOS integration options).
    pub reporting: Reporting,
    /// Dashboard mode in the web view (PanelOS integration options).
    pub web: WebConfig,
    /// The music page's player picked on the panel ("" = the integration's).
    pub music_player: String,
    /// The calendar page's view picked on the panel (agenda, day, week …).
    pub calendar_view: i32,
    /// Alarms set on the panel's timers page.
    pub alarms: Vec<crate::alarms::Alarm>,
    /// The touch calibration's correction (pixels), from the touch test.
    pub touch_offset: (f32, f32),
    /// Child lock: touches do nothing until the lock is held (see `device_options`).
    pub child_lock: bool,
    /// The time format of every clock time the panel shows: true 24-hour, false 12-hour.
    pub time_24h: bool,
}

/// What the web view shows in dashboard mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// Dashboard url_path from the integration; empty = the panel's own
    /// choice (`Settings::dashboard`).
    pub dashboard: String,
    pub show_header: bool,
    pub show_sidebar: bool,
    /// Card resources (URL paths) the web view does not load.
    pub skip_resources: Vec<String>,
    /// Size of the page, percent.
    pub zoom: u32,
    /// The loading screen covers the page until it is complete.
    pub loading_screen: bool,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            dashboard: String::new(),
            show_header: true,
            show_sidebar: false,
            skip_resources: Vec::new(),
            zoom: 100,
            loading_screen: true,
        }
    }
}

impl WebConfig {
    pub fn from_integration(v: &serde_json::Value) -> Self {
        let d = Self::default();
        let dashboard = v.get("dashboard").and_then(serde_json::Value::as_str).unwrap_or("");
        Self {
            // url_path characters only (it goes into the page URL)
            dashboard: if dashboard.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                dashboard.to_string()
            } else {
                String::new()
            },
            show_header: v.get("show_header").and_then(serde_json::Value::as_bool).unwrap_or(d.show_header),
            show_sidebar: v.get("show_sidebar").and_then(serde_json::Value::as_bool).unwrap_or(d.show_sidebar),
            skip_resources: v
                .get("skip_resources")
                .and_then(serde_json::Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(serde_json::Value::as_str)
                        .filter(|p| p.starts_with('/') && p.len() <= 300)
                        .take(100)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            zoom: v.get("zoom").and_then(serde_json::Value::as_u64).map(|z| z.clamp(50, 150) as u32).unwrap_or(100),
            loading_screen: v.get("loading_screen").and_then(serde_json::Value::as_bool).unwrap_or(true),
        }
    }
}

/// The smallest light change sent (lux), whatever the percentage.
pub const LUX_MIN_STEP: f64 = 5.0;

/// Change thresholds of the sensors Home Assistant records. Every value sent
/// is a row in its database, so small wobbles are not sent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reporting {
    /// Light: percent of the last value sent, and at least 5 lx (the noise of a dark room).
    pub illuminance_percent: f64,
    /// SoC temperature, °C, on a five-minute average.
    pub soc_temperature_step: f64,
    /// Wi-Fi signal, dB, on a five-minute average.
    pub wifi_signal_step: f64,
}

impl Default for Reporting {
    fn default() -> Self {
        Self { illuminance_percent: 15.0, soc_temperature_step: 1.0, wifi_signal_step: 3.0 }
    }
}

impl Reporting {
    /// From the integration's config message; missing or odd values keep the
    /// defaults, the rest is held to the ranges the options offer.
    pub fn from_integration(v: &serde_json::Value) -> Self {
        let d = Self::default();
        let get = |key: &str, default: f64, min: f64, max: f64| {
            v.get(key).and_then(serde_json::Value::as_f64).filter(|x| x.is_finite()).map_or(default, |x| x.clamp(min, max))
        };
        Self {
            illuminance_percent: get("illuminance_change", d.illuminance_percent, 1.0, 100.0),
            soc_temperature_step: get("soc_temperature_change", d.soc_temperature_step, 0.1, 10.0),
            wifi_signal_step: get("wifi_signal_change", d.wifi_signal_step, 1.0, 20.0),
        }
    }

    /// Light change worth sending, from the last value sent (lux).
    pub fn lux_step(&self, before: f64) -> f64 {
        (before * self.illuminance_percent / 100.0).max(LUX_MIN_STEP)
    }
}

/// What the assistant mode shows; set in Home Assistant (PanelOS
/// integration options). Without the integration: the weather entity above,
/// no media player.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantConfig {
    /// True when the integration answered (otherwise the defaults apply).
    pub from_integration: bool,
    pub weather_entity: String,
    pub media_player_entity: String,
    pub show_conversation: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // By the light sensor from the first start (setup and pairing screens too).
            theme: "auto".into(),
            brightness: 70,
            auto_brightness: true,
            screensaver_seconds: 120,
            screensaver_style: "clock".into(),
            proximity_wake: true,
            rain_on_glass: true,
            lightning: true,
            dashboard: String::new(),
            weather_entity: String::new(),
            wake_words: vec!["okay_nabu".into(), "hey_jarvis".into()],
            mic_muted: false,
            volume: 80,
            display_mode: "dashboard".into(),
            assistant: AssistantConfig::default(),
            reporting: Reporting::default(),
            web: WebConfig::default(),
            music_player: String::new(),
            calendar_view: 0,
            alarms: Vec::new(),
            touch_offset: (0.0, 0.0),
            child_lock: false,
            time_24h: true,
        }
    }
}

fn path() -> String {
    std::env::var("NSPANEL_SETTINGS").unwrap_or_else(|_| format!("{PERSIST_DIR}/settings.json"))
}

/// A file next to the settings (the same folder, so a second instance with its own settings keeps its own).
pub fn beside(name: &str) -> std::path::PathBuf {
    std::path::Path::new(&path()).with_file_name(name)
}

/// The last settings the PanelOS integration sent: the pages come back exactly as they were after a restart, a
/// reboot or while Home Assistant starts (it answers without them until the integration has loaded).
pub fn load_panel_config() -> Option<serde_json::Value> {
    std::fs::read_to_string(beside("panel-config.json")).ok().and_then(|t| serde_json::from_str(&t).ok())
}

pub fn save_panel_config(config: &serde_json::Value) {
    let p = beside("panel-config.json");
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, config.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// When the panel first saw each notification (by its title and text): Home Assistant makes some again with a
/// new time at every start (an invalid config), so the panel shows when it really came.
pub fn load_notes_seen() -> serde_json::Value {
    std::fs::read_to_string(beside("notes-seen.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_notes_seen(seen: &serde_json::Value) {
    let p = beside("notes-seen.json");
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, seen.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let p = path();
        let tmp = format!("{p}.tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }

    pub fn screensaver_index(&self) -> i32 {
        SCREENSAVER_CHOICES.iter().position(|(s, _)| *s == self.screensaver_seconds).unwrap_or(0) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_of_an_earlier_version_still_load() {
        // the assistant mode's quick tiles of earlier versions are left out
        let stored = r#"{"theme": "dark", "volume": 40,
            "assistant": {"from_integration": true, "weather_entity": "weather.example", "tile_entities": ["light.a"]}}"#;
        let s: Settings = serde_json::from_str(stored).expect("loads");
        assert_eq!((s.theme.as_str(), s.volume), ("dark", 40));
        assert_eq!(s.assistant.weather_entity, "weather.example");
        assert!(!serde_json::to_string(&s).unwrap().contains("tile_entities"));
    }
}
