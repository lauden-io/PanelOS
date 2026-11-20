//! ESPHome native API server (TCP 6053): Home Assistant's built-in ESPHome
//! integration adds the panel's voice satellite as a device, with its volume,
//! microphone switch and wake word sensors. The panel's display, settings and
//! sensors are the PanelOS integration's device (`panel_device.rs`), which
//! the voice device is connected via.
//!
//! Only Home Assistant's address (and `api_allow` in app.json) may connect.
//! The connection is encrypted once Home Assistant has handed the panel a key
//! (`api_link.rs`). Each connection gets its own thread for the handshake,
//! hello, device info and key setting; the first other message makes it the
//! main session (entities, states, voice), and an older main session ends.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::api_link::{self, Conn};

use prost::Message;

#[allow(clippy::all, dead_code)]
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/_.rs"));
}
use proto::*;

pub const PORT: u16 = 6053;
/// Device identity, the same in the API and in the mDNS announcement.
pub const DEVICE_NAME: &str = "nspanel-pro";
pub const FRIENDLY_NAME: &str = "NSPanel Pro Voice";
pub const ESPHOME_VERSION: &str = "2026.10.0";
/// "maker.model": Home Assistant shows the device as "Voice satellite by
/// PanelOS".
pub const PROJECT_NAME: &str = "PanelOS.Voice satellite";

/// What the panel reports to Home Assistant; updated by the UI thread.
#[derive(Debug, Clone, Default)]
pub struct DeviceState {
    pub theme: String,
    pub brightness: u8,
    pub auto_brightness: bool,
    pub screensaver_seconds: u32,
    pub screensaver_style: String,
    pub proximity_wake: bool,
    pub child_lock: bool,
    pub screen_on: bool,
    pub dashboards: Vec<(String, String)>,
    pub dashboard: String,
    pub lux: Option<f64>,
    pub soc_temp: Option<f64>,
    pub wifi_dbm: Option<f64>,
    pub presence: bool,
    pub ip: String,
    pub mic_muted: bool,
    pub volume: u8,
    /// "dashboard" or "assistant".
    pub display_mode: String,
    /// Filled from the settings at least once (nothing is reported before).
    pub ready: bool,
    /// url_path of the dashboard the web view shows (for the integration's
    /// choice of card resources).
    pub web_dashboard: String,
    /// The player picked on the music page (the integration sends music commands said to the panel there).
    pub music_player: String,
    /// The panel's alarms (they live on the panel; the integration shows and edits them).
    pub alarms: serde_json::Value,
    /// The media player (media.rs): state (api.proto MediaPlayerState), gain, muted.
    pub media_state: i32,
    pub media_volume: f32,
    pub media_muted: bool,
}

/// Requests from Home Assistant, carried out on the UI thread.
#[derive(Debug, Clone)]
pub enum Control {
    /// Same keys as the settings screen (`on_setting`).
    Setting(String, String),
    /// true = wake the screen, false = screensaver now.
    Screen(bool),
    /// Assist timer event (VoiceAssistantTimerEventResponse).
    Timer { kind: i32, id: String, name: String, total: u32, left: u32, active: bool },
}

/// Voice satellite plumbing for the API thread.
pub struct VoiceLink {
    pub to_api: Receiver<crate::voice::ToApi>,
    pub from_api: std::sync::mpsc::Sender<crate::voice::FromApi>,
    pub active_wake_words: Arc<Mutex<Vec<String>>>,
}

// VoiceAssistantFeature flags (aioesphomeapi): voice assistant, speaker,
// audio over the API, timers, announcements. Home Assistant only asks for
// the wake word configuration when ANNOUNCE is set.
const VOICE_FEATURES: u32 = 1 | 2 | 4 | 8 | 16;

#[derive(Clone, Copy, PartialEq)]
enum Id {
    MicMute,
    Volume,
    WakeLevel,
    WakeScore,
    WakeSnr,
    Media,
}

// Keys and names as since the first version: Home Assistant knows the
// entities by them.
const ENTITIES: &[(Id, u32, &str, &str, &str)] = &[
    (Id::MicMute, 17, "mute_microphone", "Mute microphone", "mdi:microphone-off"),
    (Id::Volume, 19, "volume", "Volume", "mdi:volume-high"),
    // Sent with every wake word, before the run starts (voice::WakeLevel).
    (Id::WakeLevel, 20, "wake_level", "Wake level", "mdi:microphone"),
    (Id::WakeScore, 21, "wake_score", "Wake score", "mdi:target"),
    (Id::WakeSnr, 22, "wake_snr", "Wake SNR", "mdi:waveform"),
    // Home Assistant and Music Assistant play on the panel's speaker (alarms with music).
    (Id::Media, 23, "media_player", "Media player", "mdi:speaker"),
];

/// What the media player can do (ESPHome media player feature flags, the same bits as Home
/// Assistant's): pause, volume set, mute, play media, stop, play, announce.
const MEDIA_FEATURES: u32 = 1 | 4 | 8 | 512 | 4096 | 16384 | 1_048_576;

/// Last wake word level and score, for a (re)connecting Home Assistant.
static LAST_WAKE: Mutex<Option<(f32, f32, Option<f32>)>> = Mutex::new(None);
/// Streamed answer audio as the API receives it: last frame, longest wait.
static TTS_RX: Mutex<(Option<Instant>, Duration)> = Mutex::new((None, Duration::ZERO));

const DIAGNOSTIC: i32 = EntityCategory::Diagnostic as i32;

#[derive(Clone, PartialEq, Debug)]
enum Value {
    Float(Option<f32>),
    Bool(bool),
    Media(i32, u32, bool),
}

fn value_of(id: Id, s: &DeviceState) -> Option<Value> {
    Some(match id {
        Id::Volume => Value::Float(Some(s.volume as f32)),
        Id::MicMute => Value::Bool(s.mic_muted),
        Id::Media => Value::Media(if s.media_state == 0 { crate::media::IDLE } else { s.media_state }, (s.media_volume * 100.0).round() as u32, s.media_muted),
        // Pushed by the voice thread (forward_voice), not diffed here.
        Id::WakeLevel | Id::WakeScore | Id::WakeSnr => return None,
    })
}

fn send<M: Message>(stream: &mut Conn, msg_type: u32, msg: &M) -> io::Result<()> {
    stream.send(msg_type, msg)
}

/// The ESPHome service for the mDNS announcement. Home Assistant matches the
/// `mac` record (lower case, no colons) with the configured device, so a
/// known panel only gets its address updated.
pub fn mdns_service() -> crate::mdns::Service {
    let mac = mac_plain();
    crate::mdns::Service {
        name: DEVICE_NAME.into(),
        kind: "_esphomelib._tcp".into(),
        port: PORT,
        txt: vec![
            format!("friendly_name={FRIENDLY_NAME}"),
            format!("version={ESPHOME_VERSION}"),
            format!("mac={mac}"),
            "network=wifi".into(),
            format!("project_name={PROJECT_NAME}"),
            format!("project_version={}", env!("CARGO_PKG_VERSION")),
        ],
    }
}

/// Wi-Fi MAC address, upper case with colons.
pub fn mac_address() -> String {
    std::fs::read_to_string("/sys/class/net/wlan0/address")
        .map(|s| s.trim().to_uppercase())
        .unwrap_or_else(|_| "00:00:00:00:00:00".into())
}

/// MAC as ESPHome writes it in mDNS and the Noise hello: lower case, no colons.
fn mac_plain() -> String {
    mac_address().replace(':', "").to_lowercase()
}

fn list_entities(stream: &mut Conn) -> io::Result<()> {
    for &(id, key, object_id, name, icon) in ENTITIES {
        let (object_id, name, icon) = (object_id.to_string(), name.to_string(), icon.to_string());
        match id {
            Id::Volume => send(stream, 49, &ListEntitiesNumberResponse {
                object_id, key, name, icon, min_value: 0.0, max_value: 100.0, step: 5.0,
                unit_of_measurement: "%".into(), mode: NumberMode::Slider as i32, ..Default::default()
            })?,
            Id::MicMute => send(stream, 17, &ListEntitiesSwitchResponse { object_id, key, name, icon, ..Default::default() })?,
            Id::Media => {
                // Home Assistant's media proxy turns any source into this: 48 kHz 16-bit stereo WAV
                let format = |purpose: MediaPlayerFormatPurpose| MediaPlayerSupportedFormat {
                    format: "wav".into(),
                    sample_rate: 48000,
                    num_channels: 2,
                    purpose: purpose as i32,
                    sample_bytes: 2,
                    ..Default::default()
                };
                #[allow(deprecated)]
                send(stream, 63, &ListEntitiesMediaPlayerResponse {
                    object_id, key, name, icon, supports_pause: true, feature_flags: MEDIA_FEATURES,
                    supported_formats: vec![format(MediaPlayerFormatPurpose::Default), format(MediaPlayerFormatPurpose::Announcement)],
                    ..Default::default()
                })?
            }
            Id::WakeLevel | Id::WakeScore | Id::WakeSnr => {
                let (unit, decimals) = match id {
                    Id::WakeLevel => ("dBFS", 1),
                    Id::WakeScore => ("", 2),
                    _ => ("dB", 1),
                };
                send(stream, 16, &ListEntitiesSensorResponse {
                    object_id, key, name, icon, unit_of_measurement: unit.into(), accuracy_decimals: decimals,
                    state_class: SensorStateClass::StateClassMeasurement as i32, entity_category: DIAGNOSTIC,
                    ..Default::default()
                })?
            }
        }
    }
    send(stream, 19, &ListEntitiesDoneResponse {})
}

fn send_state(stream: &mut Conn, id: Id, key: u32, value: &Value) -> io::Result<()> {
    match (id, value) {
        (Id::Volume, Value::Float(v)) => send(stream, 50, &NumberStateResponse {
            key, state: v.unwrap_or(0.0), missing_state: v.is_none(), ..Default::default()
        }),
        (Id::MicMute, Value::Bool(v)) => send(stream, 26, &SwitchStateResponse { key, state: *v, ..Default::default() }),
        (Id::Media, Value::Media(state, volume, muted)) => send(stream, 64, &MediaPlayerStateResponse {
            key, state: *state, volume: *volume as f32 / 100.0, muted: *muted, ..Default::default()
        }),
        (Id::WakeLevel | Id::WakeScore | Id::WakeSnr, Value::Float(v)) => send(stream, 25, &SensorStateResponse {
            key, state: v.unwrap_or(0.0), missing_state: v.is_none(), ..Default::default()
        }),
        _ => Ok(()),
    }
}

fn entity_by_key(key: u32) -> Option<Id> {
    ENTITIES.iter().find(|e| e.1 == key).map(|e| e.0)
}

fn handle_command(kind: u32, payload: &[u8], control: &dyn Fn(Control)) {
    let setting = |k: &str, v: String| control(Control::Setting(k.to_string(), v));
    match kind {
        51 => {
            let Ok(m) = NumberCommandRequest::decode(payload) else { return };
            if entity_by_key(m.key) == Some(Id::Volume) {
                setting("volume", format!("{}", m.state.round() as i32));
            }
        }
        33 => {
            let Ok(m) = SwitchCommandRequest::decode(payload) else { return };
            if entity_by_key(m.key) == Some(Id::MicMute) {
                setting("mic_muted", m.state.to_string());
            }
        }
        65 => {
            let Ok(m) = MediaPlayerCommandRequest::decode(payload) else { return };
            if entity_by_key(m.key) == Some(Id::Media) {
                eprintln!("nspanel-app: media command {:?} volume {:?} url {}", m.has_command.then_some(m.command), m.has_volume.then_some(m.volume), m.has_media_url);
                crate::media::command(
                    m.has_command.then_some(m.command),
                    m.has_volume.then_some(m.volume),
                    m.has_media_url.then(|| m.media_url.clone()),
                    m.has_announcement && m.announcement,
                );
            }
        }
        _ => {}
    }
}

fn wake_word_config(voice: &VoiceLink) -> VoiceAssistantConfigurationResponse {
    let available: Vec<VoiceAssistantWakeWord> = crate::voice::available_wake_words()
        .into_iter()
        .filter(|w| !crate::voice::INTERNAL_WORDS.contains(&w.id.as_str()))
        .map(|w| VoiceAssistantWakeWord { id: w.id, wake_word: w.phrase, trained_languages: vec!["en".into()] })
        .collect();
    VoiceAssistantConfigurationResponse {
        available_wake_words: available,
        active_wake_words: voice.active_wake_words.lock().unwrap().clone(),
        max_active_wake_words: 2,
    }
}

fn handle_voice(kind: u32, payload: &[u8], voice: &VoiceLink, stream: &mut Conn, control: &dyn Fn(Control), subscribed: &mut bool) -> io::Result<()> {
    use crate::voice::FromApi;
    match kind {
        89 => {
            if let Ok(m) = SubscribeVoiceAssistantRequest::decode(payload) {
                *subscribed = m.subscribe;
                eprintln!("nspanel-app: Assist subscription {} (flags {})", m.subscribe, m.flags);
            }
        }
        121 => send(stream, 122, &wake_word_config(voice))?,
        123 => {
            if let Ok(m) = VoiceAssistantSetConfiguration::decode(payload) {
                // Only installed models, at most the two the panel offers.
                let available: Vec<String> = wake_word_config(voice).available_wake_words.into_iter().map(|w| w.id).collect();
                let ids: Vec<String> = m.active_wake_words.into_iter().filter(|id| available.contains(id)).take(2).collect();
                let _ = voice.from_api.send(FromApi::SetWakeWords(ids.clone()));
                control(Control::Setting("wake_words".into(), ids.join(",")));
            }
        }
        91 => {
            if let Ok(m) = VoiceAssistantResponse::decode(payload) {
                let _ = voice.from_api.send(FromApi::Started { error: m.error });
            }
        }
        92 => {
            if let Ok(m) = VoiceAssistantEventResponse::decode(payload) {
                let data: Vec<(String, String)> = m.data.into_iter().map(|d| (d.name, d.value)).collect();
                // One line per pipeline event (follow-up decisions are checked from these).
                let short: Vec<String> = data
                    .iter()
                    .filter(|(k, _)| k != "tts_output" && k != "token")
                    .map(|(k, v)| format!("{k}={}", v.chars().take(100).collect::<String>()))
                    .collect();
                eprintln!("nspanel-app: Assist event {} {}", m.event_type, short.join(" "));
                if m.event_type == 99 {
                    let mut rx = TTS_RX.lock().unwrap();
                    if rx.0.is_some() {
                        eprintln!("nspanel-app: API: longest wait between answer audio frames {} ms", rx.1.as_millis());
                    }
                    *rx = (None, Duration::ZERO);
                }
                let _ = voice.from_api.send(FromApi::Event { kind: m.event_type, data });
            }
        }
        106 => {
            if let Ok(m) = VoiceAssistantAudio::decode(payload) {
                if !m.data.is_empty() {
                    let mut rx = TTS_RX.lock().unwrap();
                    let now = Instant::now();
                    if let Some(last) = rx.0 {
                        rx.1 = rx.1.max(now - last);
                    }
                    rx.0 = Some(now);
                    drop(rx);
                    let _ = voice.from_api.send(FromApi::TtsAudio(m.data));
                }
            }
        }
        119 => {
            if let Ok(m) = VoiceAssistantAnnounceRequest::decode(payload) {
                let _ = voice.from_api.send(FromApi::Announce { media_id: m.media_id, text: m.text });
            }
        }
        115 => {
            if let Ok(m) = VoiceAssistantTimerEventResponse::decode(payload) {
                eprintln!(
                    "nspanel-app: timer event {} id={} name='{}' total={} left={} active={}",
                    m.event_type, m.timer_id, m.name, m.total_seconds, m.seconds_left, m.is_active
                );
                control(Control::Timer {
                    kind: m.event_type,
                    id: m.timer_id,
                    name: m.name,
                    total: m.total_seconds,
                    left: m.seconds_left,
                    active: m.is_active,
                });
            }
        }
        _ => {}
    }
    Ok(())
}

/// SNR first (Home Assistant's arbitration reads it), then level and score.
fn send_wake_info(stream: &mut Conn, level_dbfs: f32, score: f32, snr_db: Option<f32>) -> io::Result<()> {
    send_state(stream, Id::WakeSnr, 22, &Value::Float(snr_db.map(|v| (v * 10.0).round() / 10.0)))?;
    send_state(stream, Id::WakeLevel, 20, &Value::Float(Some((level_dbfs * 10.0).round() / 10.0)))?;
    send_state(stream, Id::WakeScore, 21, &Value::Float(Some((score * 100.0).round() / 100.0)))
}

fn forward_voice(
    voice: &VoiceLink,
    stream: &mut Conn,
    subscribed: bool,
    states_subscribed: bool,
    wake_sent: &mut Option<Instant>,
) -> io::Result<()> {
    use crate::voice::{FromApi, ToApi};
    while let Ok(msg) = voice.to_api.try_recv() {
        match msg {
            ToApi::WakeInfo { level_dbfs, score, snr_db } => {
                *LAST_WAKE.lock().unwrap() = Some((level_dbfs, score, snr_db));
                if states_subscribed {
                    send_wake_info(stream, level_dbfs, score, snr_db)?;
                    *wake_sent = Some(Instant::now());
                }
            }
            ToApi::Start { conversation_id, wake_word_phrase } => {
                if !subscribed {
                    let _ = voice.from_api.send(FromApi::Started { error: true });
                    continue;
                }
                if let Some(t) = wake_sent.take() {
                    eprintln!("nspanel-app: wake level sent {} µs before the pipeline request", t.elapsed().as_micros());
                }
                send(stream, 90, &VoiceAssistantRequest {
                    start: true,
                    conversation_id,
                    flags: 1, // USE_VAD: Home Assistant detects the end of speech
                    audio_settings: Some(VoiceAssistantAudioSettings {
                        noise_suppression_level: 0,
                        auto_gain: 0,
                        volume_multiplier: 1.0,
                    }),
                    wake_word_phrase,
                    ..Default::default()
                })?;
            }
            ToApi::Audio(data) => {
                if subscribed {
                    send(stream, 106, &VoiceAssistantAudio { data, end: false, ..Default::default() })?;
                }
            }
            ToApi::Stop => {
                if subscribed {
                    send(stream, 90, &VoiceAssistantRequest { start: false, ..Default::default() })?;
                }
            }
            ToApi::AnnounceFinished { success } => {
                send(stream, 120, &VoiceAssistantAnnounceFinished { success })?;
            }
        }
    }
    Ok(())
}

fn device_info() -> DeviceInfoResponse {
    DeviceInfoResponse {
        name: DEVICE_NAME.into(),
        friendly_name: FRIENDLY_NAME.into(),
        mac_address: mac_address(),
        esphome_version: ESPHOME_VERSION.into(),
        compilation_time: env!("NSPANEL_BUILD_TIME").into(),
        model: "NSPanel Pro 86 (open firmware)".into(),
        manufacturer: "Sonoff".into(),
        project_name: PROJECT_NAME.into(),
        project_version: env!("CARGO_PKG_VERSION").into(),
        voice_assistant_feature_flags: VOICE_FEATURES,
        // Home Assistant hands a keyless panel a key over a Noise session
        // with the all-zero key, then connects encrypted.
        api_encryption_supported: true,
        api_encryption_provisionable: api_link::load_key().is_none(),
        ..Default::default()
    }
}

/// What the panel offers besides its entities: the voice satellite and the Sendspin player, which Music Assistant
/// pairs with the token Home Assistant asks from here (SendspinPairingTokenRequest).
fn capabilities() -> DeviceCapabilitiesResponse {
    DeviceCapabilitiesResponse {
        voice_assistant: Some(VoiceAssistantCapabilities { feature_flags: VOICE_FEATURES }),
        sendspin: Some(SendspinCapabilities { feature_flags: 1, client_id: crate::sendspin::client_id().unwrap_or_default() }),
        ..Default::default()
    }
}

enum Basic {
    Done,
    Close,
    /// Needs the main session.
    Other,
}

/// Messages any connection may send: hello, authentication (always
/// accepted, as on ESPHome since 2026.1), disconnect, ping, device info and
/// the API key.
fn handle_basic(stream: &mut Conn, kind: u32, payload: &[u8]) -> io::Result<Basic> {
    match kind {
        1 => {
            let hello = HelloRequest::decode(payload).unwrap_or_default();
            eprintln!("nspanel-app: ESPHome API client '{}' {}.{}", hello.client_info, hello.api_version_major, hello.api_version_minor);
            send(stream, 2, &HelloResponse {
                api_version_major: 1,
                // 1.15: Home Assistant asks the capabilities (DeviceCapabilitiesRequest), where the panel offers
                // its Sendspin client for pairing
                api_version_minor: 15,
                server_info: format!("nspanel-app {}", env!("CARGO_PKG_VERSION")),
                name: DEVICE_NAME.into(),
            })?;
        }
        3 => send(stream, 4, &AuthenticationResponse { invalid_password: false })?,
        5 => {
            send(stream, 6, &DisconnectResponse {})?;
            return Ok(Basic::Close);
        }
        7 => send(stream, 8, &PingResponse {})?,
        8 => {}
        9 => send(stream, 10, &device_info())?,
        149 => send(stream, 150, &capabilities())?,
        159 => {
            // the Sendspin pairing token only goes over an encrypted session
            let (status, token) = if api_link::load_key().is_none() {
                (SendspinPairingTokenStatus::EncryptionRequired, String::new())
            } else {
                match crate::sendspin::pairing_token() {
                    Some(t) => (SendspinPairingTokenStatus::Ok, t),
                    None => (SendspinPairingTokenStatus::NotReady, String::new()),
                }
            };
            send(stream, 160, &SendspinPairingTokenResponse { status: status as i32, token })?;
        }
        124 => {
            let ok = NoiseEncryptionSetKeyRequest::decode(payload).map(|m| api_link::set_key(stream, &m.key)).unwrap_or(false);
            send(stream, 125, &NoiseEncryptionSetKeyResponse { success: ok })?;
            if ok {
                // Sessions opened before a key change end (as on ESPHome).
                return Ok(Basic::Close);
            }
        }
        _ => return Ok(Basic::Other),
    }
    Ok(Basic::Done)
}

fn serve(
    mut stream: Conn,
    state: &Arc<Mutex<DeviceState>>,
    notify: &Receiver<()>,
    control: &dyn Fn(Control),
    voice: &VoiceLink,
    takeover: &AtomicBool,
) -> io::Result<()> {
    stream.set_read_timeout(Duration::from_millis(15))?;
    let mut subscribed = false;
    let mut voice_subscribed = false;
    let mut sent: HashMap<u32, Value> = HashMap::new();
    let mut last_rx = Instant::now();
    let mut ping_sent: Option<Instant> = None;
    let mut wake_sent: Option<Instant> = None;
    loop {
        if stream.read_some()? {
            last_rx = Instant::now();
            ping_sent = None;
        }
        if takeover.load(Ordering::SeqCst) {
            eprintln!("nspanel-app: ESPHome API: a newer connection takes over");
            return Ok(());
        }
        if stream.outdated() {
            eprintln!("nspanel-app: ESPHome API: the key changed, session ends");
            return Ok(());
        }
        while let Some((kind, payload)) = stream.take_frame()? {
            if std::env::var_os("NSPANEL_API_DEBUG").is_some() && kind != 106 {
                eprintln!("nspanel-app: API rx type {kind} ({} bytes)", payload.len());
            }
            match handle_basic(&mut stream, kind, &payload)? {
                Basic::Close => return Ok(()),
                Basic::Done => continue,
                Basic::Other => {}
            }
            match kind {
                11 => list_entities(&mut stream)?,
                20 => {
                    subscribed = true;
                    sent.clear();
                    let last = *LAST_WAKE.lock().unwrap();
                    if let Some((level_dbfs, score, snr_db)) = last {
                        send_wake_info(&mut stream, level_dbfs, score, snr_db)?;
                    }
                }
                33 | 51 | 65 => handle_command(kind, &payload, control),
                89 | 91 | 92 | 106 | 115 | 119 | 121 | 123 => {
                    handle_voice(kind, &payload, voice, &mut stream, control, &mut voice_subscribed)?
                }
                // Logs, Home Assistant services/states subscriptions, time: not used.
                _ => {}
            }
        }
        forward_voice(voice, &mut stream, voice_subscribed, subscribed, &mut wake_sent)?;
        // Push changed states.
        let changed = notify.try_iter().count() > 0;
        if subscribed && (changed || sent.is_empty()) {
            let snapshot = state.lock().unwrap().clone();
            for &(id, key, ..) in ENTITIES {
                if let Some(v) = value_of(id, &snapshot) {
                    if sent.get(&key) != Some(&v) {
                        send_state(&mut stream, id, key, &v)?;
                        sent.insert(key, v);
                    }
                }
            }
        }
        // Keep-alive towards Home Assistant.
        if ping_sent.is_none() && last_rx.elapsed() > Duration::from_secs(60) {
            send(&mut stream, 7, &PingRequest {})?;
            ping_sent = Some(Instant::now());
        }
        if ping_sent.is_some_and(|t| t.elapsed() > Duration::from_secs(30)) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "no answer to ping"));
        }
    }
}

/// Connections handled at the same time (Home Assistant uses one, plus one
/// briefly when it hands over the API key).
const MAX_CONNECTIONS: usize = 3;
/// A connection that has not become the main session ends after this long without a message.
const SIDE_IDLE: Duration = Duration::from_secs(60);

struct MainParts {
    notify: Receiver<()>,
    voice: VoiceLink,
    control: Box<dyn Fn(Control) + Send>,
}

fn connection(
    stream: TcpStream,
    state: &Arc<Mutex<DeviceState>>,
    main: &Mutex<MainParts>,
    takeover: &AtomicBool,
) -> io::Result<()> {
    let mut conn = Conn::accept(stream, DEVICE_NAME, &mac_plain())?;
    eprintln!(
        "nspanel-app: ESPHome API connection from {} ({})",
        conn.peer(),
        if conn.keyed {
            "encrypted"
        } else if conn.encrypted() {
            "encrypted with the provisioning key"
        } else {
            "plaintext, no key yet"
        }
    );
    conn.set_read_timeout(Duration::from_millis(200))?;
    let mut last = Instant::now();
    loop {
        if conn.read_some()? {
            last = Instant::now();
        } else if last.elapsed() > SIDE_IDLE {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "idle"));
        }
        if conn.outdated() {
            return Ok(());
        }
        while let Some((kind, payload)) = conn.take_frame()? {
            match handle_basic(&mut conn, kind, &payload)? {
                Basic::Close => return Ok(()),
                Basic::Done => {}
                Basic::Other => {
                    // Becomes the main session; an older one ends first.
                    conn.push_front((kind, payload));
                    takeover.store(true, Ordering::SeqCst);
                    let parts = main.lock().unwrap_or_else(|e| e.into_inner());
                    takeover.store(false, Ordering::SeqCst);
                    let result = serve(conn, state, &parts.notify, &*parts.control, &parts.voice, takeover);
                    // A conversation in progress cannot finish without Home Assistant.
                    let _ = parts.voice.from_api.send(crate::voice::FromApi::Event {
                        kind: 0,
                        data: vec![("message".into(), "Home Assistant disconnected".into())],
                    });
                    while parts.voice.to_api.try_recv().is_ok() {}
                    return result;
                }
            }
        }
    }
}

pub fn spawn(
    state: Arc<Mutex<DeviceState>>,
    notify: Receiver<()>,
    voice: VoiceLink,
    allowed: impl Fn() -> Vec<IpAddr> + Send + 'static,
    control: impl Fn(Control) + Send + 'static,
) {
    let main = Arc::new(Mutex::new(MainParts { notify, voice, control: Box::new(control) }));
    let takeover = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(("0.0.0.0", PORT)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("nspanel-app: ESPHome API port {PORT}: {e}");
                return;
            }
        };
        let mut refused: Option<(IpAddr, Instant)> = None;
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let Ok(peer) = stream.peer_addr() else { continue };
            if !allowed().iter().any(|ip| api_link::same_host(*ip, peer.ip())) {
                // Logged once a minute per host.
                if refused.is_none_or(|(ip, t)| ip != peer.ip() || t.elapsed() > Duration::from_secs(60)) {
                    eprintln!("nspanel-app: ESPHome API connection from {peer} refused (not Home Assistant; app.json api_allow adds hosts)");
                    refused = Some((peer.ip(), Instant::now()));
                }
                continue;
            }
            if active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                eprintln!("nspanel-app: ESPHome API connection from {peer} refused ({MAX_CONNECTIONS} open)");
                continue;
            }
            active.fetch_add(1, Ordering::SeqCst);
            let (state, main, takeover, active) = (state.clone(), main.clone(), takeover.clone(), active.clone());
            std::thread::spawn(move || {
                // A malformed message must not take the API down: a panic ends this connection only.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connection(stream, &state, &main, &takeover)));
                match result {
                    Ok(Ok(())) => eprintln!("nspanel-app: ESPHome API connection from {peer} ended"),
                    Ok(Err(e)) => eprintln!("nspanel-app: ESPHome API connection from {peer} ended: {e}"),
                    Err(_) => eprintln!("nspanel-app: ESPHome API connection from {peer} ended by a panic"),
                }
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
}
