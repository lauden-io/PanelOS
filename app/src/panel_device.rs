//! The panel's own device in Home Assistant (PanelOS integration): display,
//! settings and sensors. The Home Assistant client (`ha.rs`) reports them
//! with `panelos/report` (changes only) and passes the commands it gets on
//! its `panelos/subscribe` subscription to `command()`. The voice satellite
//! stays on the ESPHome API (`esphome.rs`), as its own device in Home
//! Assistant, connected via this one.

use serde_json::{json, Map, Value};

use crate::esphome::{Control, DeviceState};

/// Device name, maker and model as Home Assistant shows them (Gen1: the
/// first generation of the panel; a Gen2 follows).
pub const NAME: &str = "NSPanel Pro";
pub const MANUFACTURER: &str = "PanelOS";
pub const MODEL: &str = "NSPanel Pro 86 Gen1";

pub const THEMES: [(&str, &str); 3] = [("dark", "Dark"), ("light", "Light"), ("auto", "Auto")];
pub const STYLES: [(&str, &str); 2] = [("clock", "Clock"), ("black", "Screen off")];
pub const DISPLAY_MODES: [(&str, &str); 2] = [("dashboard", "Dashboard"), ("assistant", "Assistant")];

fn label(list: &[(&str, &str)], key: &str) -> Value {
    list.iter().find(|(k, _)| *k == key).map(|(_, l)| Value::from(*l)).unwrap_or(Value::Null)
}

fn labels(list: &[(&str, &str)]) -> Value {
    list.iter().map(|(_, l)| *l).collect::<Vec<_>>().into()
}

fn key_of<'a>(list: &[(&'a str, &str)], label: &str) -> Option<&'a str> {
    list.iter().find(|(_, l)| *l == label).map(|(k, _)| *k)
}

/// Titles of the dashboards the panel's user may open ("Overview" first).
fn dashboard_titles(s: &DeviceState) -> Vec<String> {
    let mut titles: Vec<String> = s.dashboards.iter().map(|(_, t)| t.clone()).collect();
    if titles.is_empty() {
        titles.push(current_dashboard(s));
    }
    titles
}

fn current_dashboard(s: &DeviceState) -> String {
    s.dashboards
        .iter()
        .find(|(u, _)| *u == s.dashboard)
        .map(|(_, t)| t.clone())
        .unwrap_or_else(|| if s.dashboard.is_empty() { "Overview".into() } else { s.dashboard.clone() })
}

/// Who the panel is, for the device in Home Assistant.
pub fn device_info(mac: &str) -> Value {
    json!({
        "name": NAME,
        "manufacturer": MANUFACTURER,
        "model": MODEL,
        "sw_version": env!("CARGO_PKG_VERSION"),
        "mac": mac,
        // what this panel can do (the integration offers only these)
        "caps": CAPS,
    })
}

/// Touch screen, pictures and animation, web view, live video, microphone, speaker, proximity and
/// light sensors.
pub const CAPS: [&str; 8] = ["touch", "images", "web", "video", "mic", "speaker", "proximity", "light"];

/// Options of the select entities.
pub fn options(s: &DeviceState) -> Map<String, Value> {
    let screensaver: Vec<&str> = crate::settings::SCREENSAVER_CHOICES.iter().map(|(_, l)| *l).collect();
    let mut m = Map::new();
    m.insert("dashboard".into(), dashboard_titles(s).into());
    m.insert("theme".into(), labels(&THEMES));
    m.insert("screensaver".into(), screensaver.into());
    m.insert("screensaver_style".into(), labels(&STYLES));
    m.insert("display_mode".into(), labels(&DISPLAY_MODES));
    m
}

/// Entity states, keyed as in the integration.
pub fn states(s: &DeviceState) -> Map<String, Value> {
    let screensaver = crate::settings::SCREENSAVER_CHOICES
        .iter()
        .find(|(secs, _)| *secs == s.screensaver_seconds)
        .map(|(_, l)| Value::from(*l))
        .unwrap_or(Value::Null);
    let text = |v: &str| if v.is_empty() { Value::Null } else { Value::from(v) };
    let mut m = Map::new();
    m.insert("screen".into(), s.screen_on.into());
    m.insert("dashboard".into(), current_dashboard(s).into());
    m.insert("theme".into(), label(&THEMES, &s.theme));
    m.insert("brightness".into(), s.brightness.into());
    m.insert("auto_brightness".into(), s.auto_brightness.into());
    m.insert("screensaver".into(), screensaver);
    m.insert("screensaver_style".into(), label(&STYLES, &s.screensaver_style));
    m.insert("proximity_wake".into(), s.proximity_wake.into());
    m.insert("rain_on_glass".into(), s.rain_on_glass.into());
    m.insert("lightning".into(), s.lightning.into());
    m.insert("display_mode".into(), label(&DISPLAY_MODES, &s.display_mode));
    m.insert("illuminance".into(), s.lux.into());
    m.insert("presence".into(), s.presence.into());
    m.insert("soc_temperature".into(), s.soc_temp.into());
    m.insert("wifi_signal".into(), s.wifi_dbm.into());
    m.insert("ip_address".into(), text(&s.ip));
    m.insert("app_version".into(), env!("CARGO_PKG_VERSION").into());
    m
}

/// A picture of the screen as JPEG in base64, for the PanelOS page's live preview (small enough to send
/// several a second).
pub fn jpeg_base64(width: u32, height: u32, rgb: &[u8]) -> Option<String> {
    let mut out = Vec::with_capacity(64 * 1024);
    // (quality 65: about a third smaller than 78, still sharp at the page's preview size)
    let encoder = jpeg_encoder::Encoder::new(&mut out, 65);
    encoder.encode(rgb, width as u16, height as u16, jpeg_encoder::ColorType::Rgb).ok()?;
    Some(base64(&out))
}

/// A picture of the screen as PNG in base64, for the PanelOS page.
pub fn png_base64(width: u32, height: u32, rgb: &[u8]) -> Option<String> {
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgb).ok()?;
    }
    Some(base64(&png))
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A command from an entity of the panel's device; same settings as the
/// settings screen (`on_setting`).
pub fn command(key: &str, value: &Value, s: &DeviceState) -> Option<Control> {
    let setting = |k: &str, v: String| Some(Control::Setting(k.to_string(), v));
    let text = value.as_str().unwrap_or_default();
    match key {
        "screen" => value.as_bool().map(Control::Screen),
        "theme" => key_of(&THEMES, text).and_then(|k| setting("theme", k.into())),
        "screensaver_style" => key_of(&STYLES, text).and_then(|k| setting("screensaver_style", k.into())),
        "display_mode" => key_of(&DISPLAY_MODES, text).and_then(|k| setting("display_mode", k.into())),
        "screensaver" => crate::settings::SCREENSAVER_CHOICES
            .iter()
            .position(|(_, l)| *l == text)
            .and_then(|i| setting("screensaver_index", i.to_string())),
        "dashboard" => s.dashboards.iter().position(|(_, t)| *t == text).and_then(|i| setting("dashboard_index", i.to_string())),
        "brightness" => value.as_f64().and_then(|v| setting("brightness", format!("{}", v.round().clamp(1.0, 100.0) as i32))),
        "auto_brightness" | "proximity_wake" | "rain_on_glass" | "lightning" => value.as_bool().and_then(|v| setting(key, v.to_string())),
        "restart_app" | "reboot" => setting(key, String::new()),
        // (the place of a page among the pages shown)
        "page" => value.as_u64().and_then(|i| setting("show_page", i.to_string())),
        _ => None,
    }
}
