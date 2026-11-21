//! NSPanel Pro panel app: the assistant view's pages (living weather home,
//! music, timers, ...) drawn natively, the Home Assistant dashboard in the web
//! view as the second display mode, the voice satellite, and the panel's own
//! device in Home Assistant.

#![allow(clippy::too_many_arguments)]

mod api_link;
mod alarms;
mod alerts;
mod artwork;
mod auth;
mod avatars;
mod cal;
mod cam;
// Camera pictures: JPEG decode, frames through Home Assistant's camera proxy.
#[allow(dead_code)]
mod camera;
mod clock;
mod config;
mod connlog;
mod device_options;
mod devlog;
mod display;
mod draw;
mod esphome;
mod glass;
#[cfg(feature = "gpu")]
mod glass_gl;
#[cfg(feature = "gpu")]
mod gpu;
mod ha;
mod home_edit;
mod hw;
mod icons;
mod info;
mod loading_text;
mod mdns;
mod media;
mod mus;
mod pages;
mod quick;
mod pairing;
mod panel_device;
mod panel_id;
mod platform;
mod roam;
mod sendspin;
mod settings;
mod shade;
mod sounds;
mod stt;
mod synth;
mod texts;
mod thermo;
mod todo;
mod touch;
mod visit;
mod voice;
mod web;
mod webstats;
mod wifi;
mod windows;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
#[cfg(not(feature = "gpu"))]
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use auth::Auth;
use config::{AppConfig, PERSIST_DIR};
use ha::{Command, EntityState, Event};
use settings::Settings;
use visit::{Hold, Visit};

slint::include_modules!();

/// Budapest time as a POSIX TZ rule (works without zoneinfo files).
const DEFAULT_TZ: &str = "CET-1CEST,M3.5.0,M10.5.0/3";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Pull to refresh: a pull counts after this many px down, and reloads the
/// page after this many more.
const PULL_START: f32 = 16.0;
const PULL_DISTANCE: f32 = 110.0;

extern "C" {
    // POSIX; not exported by the libc crate for this target.
    fn tzset();
}

pub struct UiState {
    config: Option<AppConfig>,
    auth: Option<Arc<Auth>>,
    shared: Arc<ha::Shared>,
    commands: Option<Sender<Command>>,
    settings: Settings,
    states: HashMap<String, EntityState>,
    pairing_started: bool,
    dashboards: Vec<(String, String)>,
    connected: bool,
    /// When the link to Home Assistant last came back: for two minutes after that, an entity that turns
    /// "unavailable" / "unknown" (Home Assistant starting, its integrations not loaded yet) keeps its last good
    /// state on the screen; the held ones are applied when it is over or a real state comes.
    linked_at: Option<Instant>,
    held_states: HashMap<String, Option<ha::EntityState>>,
    /// Connected at least once since the start (offline screen vs connecting screen).
    ever_connected: bool,
    /// Home Assistant away since (the offline screen comes after a grace period).
    offline_since: Option<Instant>,
    offline_text: String,
    screensaver: bool,
    /// The screensaver started by itself (idle), not by Home Assistant: it ends when its time window does.
    saver_auto: bool,
    /// Status screens (newest last) and the critical alert sent by the integration; whether the strip is theirs.
    statuses: Vec<alerts::Status>,
    critical: Option<alerts::Critical>,
    alert_strip: bool,
    backlight: hw::Backlight,
    light: hw::LightSensor,
    proximity: hw::ProximityWake,
    /// Proximity test view on: the integration time asked for in
    /// PROXIMITY_TEST_FILE (empty = the default one).
    proximity_test: Option<String>,
    /// Backlight now and where it is heading, per mille of the PWM range;
    /// `fade_backlight()` moves it there smoothly.
    brightness: f64,
    brightness_target: f64,
    lux: Option<f64>,
    dark: Option<bool>,
    /// Auto theme: the other look the light sensor asks for, and since when.
    theme_pending: Option<(bool, Instant)>,
    device: Arc<std::sync::Mutex<esphome::DeviceState>>,
    device_notify: Sender<()>,
    last_reported_lux: Option<f64>,
    last_report: Instant,
    /// Five-minute averages of the once-a-minute readings Home Assistant gets.
    soc_temp_avg: Option<f64>,
    wifi_avg: Option<f64>,
    /// Dashboard mode in the web view (None without the web bundle).
    web: Option<Arc<web::WebView>>,
    /// A swipe down from the top edge of the web view (start y): opens the
    /// panel's quick settings instead of going to the page.
    web_swipe: Option<f32>,
    /// Touch test: on, the point to touch now, where and when the finger came down.
    touch_test: bool,
    touch_test_step: usize,
    /// The touch test started from the settings or the integration (not by its file), and its offsets.
    touch_test_manual: bool,
    touch_offsets: Vec<(f32, f32)>,
    touch_result: Option<String>,
    touch_close_at: Option<Instant>,
    touch_started: Option<Instant>,
    touch_test_down: Option<(Instant, f32, f32, u32)>,
    /// The touch test was started from the settings list: it goes back there when it closes.
    touch_from_settings: bool,
    /// Pull to refresh: where the finger came down, and how far it pulled (0..1, None: not pulling).
    web_pull: Option<(f32, f32)>,
    web_pulled: Option<f32>,
    timers: HashMap<String, pages::Timer>,
    /// Arrival counter of the timers (their order and colour).
    timer_seq: u64,
    /// Home Assistant timers removed on the panel: their later events are ignored.
    dismissed_timers: std::collections::HashSet<String>,
    /// Stopwatch: the lap times (newest first) and the time at the last lap.
    laps: Vec<f32>,
    lap_at: f32,
    ringing: Option<Instant>,
    /// A snoozed alarm: when it rings again, which alarm it is, its label and sound.
    snooze: Option<alarms::Snooze>,
    /// The alarm ringing now (its snooze belongs to it); None while a timer rings.
    ringing_alarm: Option<alarms::AlarmKey>,
    /// The ringing timer's or alarm's sound (`<value>|<default>`, as the integration names sounds).
    ring_tone: String,
    /// The sound looping now (`ring:…` or `crit:…`), to start and stop it once.
    looping: Option<String>,
    /// An alarm's music from Music Assistant: when it was asked for (its default rings if no audio comes).
    music_asked: Option<Instant>,
    /// Same as `ringing.is_some()`, for the voice thread.
    ringing_flag: Arc<std::sync::atomic::AtomicBool>,
    mic_muted: Arc<std::sync::atomic::AtomicBool>,
    wake_words: Arc<std::sync::Mutex<Vec<String>>>,
    player: Arc<voice::Player>,
    voice_hide_at: Option<Instant>,
    /// The integration's configuration of this panel (pages, widgets, entities).
    panel_config: Option<Value>,
    forecast_hourly: Vec<Value>,
    forecast_daily: Vec<Value>,
    wake_phrase: String,
    /// The status bar's wake hint: for which wake phrase, and its text.
    wake_hint: (String, String),
    /// When the wake hint got its line (a new one every 10 minutes).
    wake_hint_at: Option<Instant>,
    /// The status line's line while the microphone is muted (picked at each mute).
    muted_hint: String,
    /// The theme setting at the last check (Auto changing the theme by itself gets the slower change).
    theme_mode_seen: Option<String>,
    /// Each player's cover (the music pages' and the music widgets' players), and the thread that fetches them.
    arts: HashMap<String, Art>,
    art_tx: Option<Sender<(String, Option<String>)>>,
    /// When the dashboard shown by a show request goes by itself (a shown page view's time is its visit's).
    show_until: Option<Instant>,
    /// When a notification's strip goes by itself.
    strip_until: Option<Instant>,
    /// When the light level was last sent to Home Assistant.
    last_lux_report: Instant,
    /// Play / pause pressed on the panel: (the player, playing, when, the position then) until the player reports it
    /// (at most 3 s), so the time neither runs on after a pause nor jumps after a play while the player catches up.
    music_hold: Option<(String, bool, Instant, f32)>,
    /// Next / previous pressed on the panel (see `apply_music_skip`).
    music_skip: Option<MusicSkip>,
    /// Split-flap clock: the digits shown before the last change, and when it changed.
    digits: [String; 4],
    prev_digits: [String; 4],
    flip_at: Option<Instant>,
    /// Weather and forecast labels are refreshed once a minute (and on changes).
    weather_refreshed: Instant,
    /// Development: the system screens were set from the dev input; the
    /// connection state does not change them meanwhile.
    dev_screens: bool,
    /// Do not disturb switched in the quick shade: on or off, and what the integration's setting
    /// (on, off, schedule) said then. It holds until that changes (the schedule's next turn, or
    /// another setting from Home Assistant).
    dnd: Option<(bool, bool)>,
    /// Development: settings from the dev input, kept over Home Assistant's.
    dev_config: Option<Value>,
    /// A temporary page shown (its place in the pager).
    temp_page: Option<usize>,
    /// The alarm open in the editor (None: a new one), the minute last checked for alarms.
    alarm_edit: Option<usize>,
    alarm_minute: i64,
    /// Screen off from the quick shade: dark until the next touch or approach.
    screen_off: bool,
    /// What the weather shown last looked like (see weather_key).
    weather_key: String,
    /// The condition whose living icon frames are loaded (spec M12).
    wx_cond: String,
    /// The timers (and the ones in their last minute) the timer island knows: a new one brings it back (29.3).
    island_seen: Vec<String>,
    island_urgent: Vec<String>,
    /// The volume before the panel was muted (the speaker mark's tap goes back to it; 0 = unknown).
    vol_before_mute: u8,
    /// Development: weather values forced from the dev input.
    dev_wx: Option<Value>,
}

thread_local! {
    static STATE: RefCell<Option<UiState>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut UiState) -> R) -> R {
    STATE.with(|s| f(s.borrow_mut().as_mut().expect("state")))
}

/// One player's cover: the picture's address (None: no cover), the picture, its blurred copy (the plain look, the cover
/// behind the cassette), its own colour (the music page's accent, the record; a grey cover's own tone) and the
/// record's colour (the cover's own, black, white or grey too).
#[derive(Clone, Default)]
struct Art {
    url: Option<String>,
    img: Option<slint::Image>,
    bg: Option<slint::Image>,
    accent: Option<slint::Color>,
    record: Option<slint::Color>,
}

/// The music views' accent for a player: its cover's colour (a grey cover's own tone), the panel's without a cover.
fn cover_accent(s: &UiState, player: &str) -> slint::Color {
    match s.arts.get(player) {
        Some(a) if a.img.is_some() => a.accent.unwrap_or_else(|| accent(s)),
        _ => accent(s),
    }
}

/// The turntable's woods (the integration's list, in its order).
const WOODS: [&str; 7] = ["walnut", "oak", "cherry", "beech", "pine", "pear", "scandi-oak"];

fn accent(s: &UiState) -> slint::Color {
    s.panel_config
        .as_ref()
        .and_then(|c| c.get("accent"))
        .and_then(Value::as_str)
        .and_then(pages::parse_color)
        .unwrap_or_else(|| slint::Color::from_rgb_u8(0xFF, 0x4A, 0x1C))
}

pub fn config_str(s: &UiState, key: &str) -> String {
    s.panel_config.as_ref().and_then(|c| c.get(key)).and_then(Value::as_str).unwrap_or("").to_string()
}

thread_local! {
    /// The window, for the page lookups that need to know the page on screen.
    static UI: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

/// The page of `kind` the panel acts on (`pages::acting_slot`): the page on screen when it is of that kind, else the
/// first of the kind. None when the pager has none.
pub fn acting(kind: &str) -> Option<PageSlot> {
    let ui = UI.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade()))?;
    let pages = ui.get_pages();
    let slots: Vec<PageSlot> = pages.iter().collect();
    pages::acting_slot(&slots, ui.get_page().max(0) as usize, kind).map(|i| slots[i].clone())
}

/// The row of a kind's per-page data the panel acts on (0 without a page of the kind).
pub fn acting_n(kind: &str) -> usize {
    acting(kind).map_or(0, |p| p.n.max(0) as usize)
}

/// The settings of the page of `kind` the panel acts on (see `acting`); without a pager yet, the first of the kind.
fn page_cfg<'a>(s: &'a UiState, kind: &str) -> Option<&'a Value> {
    let list = pages::kind_pages(s.panel_config.as_ref(), kind);
    match acting(kind) {
        Some(slot) if slot.cfg >= 0 => list.into_iter().find(|(i, _)| *i == slot.cfg as usize).map(|(_, p)| p),
        Some(_) => None,
        None => list.into_iter().next().map(|(_, p)| p),
    }
}

/// The entities of the page of `kind` the panel acts on (the integration's page list).
fn page_entities(s: &UiState, kind: &str) -> Vec<String> {
    page_cfg(s, kind).map(pages::entities_of).unwrap_or_default()
}

/// The looks of the pages of a kind on the list (a kind may be there more than once).
pub fn page_looks(ui: &AppWindow, kind: &str) -> Vec<i32> {
    let pages = ui.get_pages();
    (0..pages.row_count()).filter_map(|i| pages.row_data(i)).filter(|p| p.kind == kind).map(|p| p.variant).collect()
}

/// The look of the page of `kind` the panel acts on.
pub fn page_look(kind: &str) -> i32 {
    acting(kind).map_or(0, |p| p.variant)
}

/// Do not disturb: as the integration sets it (on, or on a schedule), unless switched in the quick
/// shade since that last changed.
pub fn dnd_active(s: &UiState) -> bool {
    let set = dnd_setting(s);
    match s.dnd {
        Some((on, then)) if then == set => on,
        _ => set,
    }
}

/// Do not disturb as the integration sets it now (on, off, or the schedule's state now).
pub fn dnd_setting(s: &UiState) -> bool {
    let cfg = s.panel_config.as_ref().and_then(|c| c.get("dnd"));
    let get = |k: &str| cfg.and_then(|c| c.get(k));
    match get("mode").and_then(Value::as_str).unwrap_or("off") {
        "on" => true,
        "schedule" => {
            let mins = |k: &str, d: i64| {
                get(k)
                    .and_then(Value::as_str)
                    .and_then(|t| {
                        let (h, m) = t.split_once(':')?;
                        Some(h.trim().parse::<i64>().ok()? * 60 + m.trim().parse::<i64>().ok()?)
                    })
                    .unwrap_or(d)
            };
            let (from, to) = (mins("from", 22 * 60), mins("to", 7 * 60));
            let t = pages::now();
            let now = pages::local_format(t, "%H").parse::<i64>().unwrap_or(0) * 60 + pages::local_format(t, "%M").parse::<i64>().unwrap_or(0);
            if from <= to {
                now >= from && now < to
            } else {
                now >= from || now < to
            }
        }
        _ => false,
    }
}

/// What comes through do not disturb (alarms, timers, status, ring, voice, calls; all by default).
/// Critical alerts always do.
pub fn dnd_allows(s: &UiState, what: &str) -> bool {
    !dnd_active(s)
        || s.panel_config
            .as_ref()
            .and_then(|c| c.get("dnd"))
            .and_then(|d| d.get("allow"))
            .and_then(|a| a.get(what))
            .and_then(Value::as_bool)
            .unwrap_or(true)
}

/// A new notification from Home Assistant: a strip on top for a few seconds (the integration's
/// "New notifications": strip / card / off, sound, seconds); silent and none in do not disturb.
fn popup(ui: &AppWindow, id: &str, title: &str, message: &str) {
    let (kind, sound, seconds, quiet) = with_state(|s| {
        let p = s.panel_config.as_ref().and_then(|c| c.get("popups"));
        let kind = p.and_then(|p| p.get("kind")).and_then(Value::as_str).unwrap_or("card").to_string();
        let sound = p.and_then(|p| p.get("sound")).and_then(Value::as_bool).unwrap_or(true);
        let seconds = p.and_then(|p| p.get("seconds")).and_then(Value::as_u64).unwrap_or(10).clamp(3, 120);
        (kind, sound, seconds, dnd_active(s) || s.screensaver || s.alert_strip)
    });
    if kind == "off" || quiet {
        return;
    }
    if kind == "card" {
        // the window (30): its time, or until answered
        let until = with_state(|s| {
            s.panel_config.as_ref().and_then(|c| c.get("popups")).and_then(|p| p.get("until")).and_then(Value::as_bool).unwrap_or(false)
        });
        ui.set_pop_until(until);
        ui.set_pop_seconds(seconds as f32);
        info::pop(ui, Some(id));
        wake(ui);
        // the window keeps its own time (a finger holds it, each step starts it again, until answered has none)
        with_state(|s| {
            if sound {
                s.player.play(voice::Sound::Chime);
            }
        });
        return;
    }
    ui.set_strip_title(if title.is_empty() { "Home Assistant".into() } else { title.into() });
    ui.set_strip_sub(message.into());
    ui.set_strip_icon(icons::glyph("bell-outline").map(String::from).unwrap_or_default().into());
    ui.set_strip_has_picto(false);
    ui.set_strip_critical(false);
    ui.set_strip_tint(slint::Color::from_rgb_u8(0x2a, 0x2c, 0x2f));
    ui.set_strip_on(true);
    with_state(|s| {
        s.strip_until = Some(Instant::now() + Duration::from_secs(seconds));
        if sound {
            s.player.play(voice::Sound::Chime);
        }
    });
}

/// Home Assistant's notifications changed (`current`, `added`, `removed`); a new one may pop up.
fn notes_changed(ui: &AppWindow, kind: &str, notes: &Value) {
    let fresh = if kind == "added" {
        notes.as_object().and_then(|m| m.iter().last()).map(|(id, v)| {
            let t = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            (id.clone(), t("title"), t("message"))
        })
    } else {
        None
    };
    info::update(kind, notes);
    with_state(|s| info::refresh(ui, s));
    if let Some((id, title, message)) = fresh {
        popup(ui, &id, &title, &message);
    }
}

/// An option of the page of `kind` the panel acts on (see `acting`).
fn page_option(s: &UiState, kind: &str, key: &str) -> Option<Value> {
    page_cfg(s, kind).and_then(|p| pages::option_of(p, key)).cloned()
}

/// An option of the first page of `kind`: for what one page sets for the whole panel (the synthwave scene).
fn first_page_option(s: &UiState, kind: &str, key: &str) -> Option<Value> {
    pages::kind_pages(s.panel_config.as_ref(), kind).first().and_then(|(_, p)| pages::option_of(p, key)).cloned()
}

fn weather_entity(s: &UiState) -> String {
    // one weather for the panel: the first weather details page that names one
    if let Some(e) = pages::kind_pages(s.panel_config.as_ref(), "weather").iter().find_map(|(_, p)| pages::entities_of(p).into_iter().next()) {
        return e;
    }
    let e = config_str(s, "weather_entity");
    if e.is_empty() { s.settings.weather_entity.clone() } else { e }
}

/// A play / pause pressed on the panel holds until the player reports it (or 3 s): the shown state and, while
/// paused, the position stay where the press left them.
fn apply_music_hold(s: &mut UiState, player: &str, music: &mut HomeMusic) {
    let Some((who, want, at, pos)) = s.music_hold.clone() else { return };
    if who != player {
        return;
    }
    if music.playing == want || at.elapsed() >= Duration::from_secs(3) {
        s.music_hold = None;
        return;
    }
    music.playing = want;
    if !want {
        music.pos = pos;
    }
}

/// Play or pause pressed on the panel for `player` (the play key, the turntable's arm): the glyph turns at once on every
/// page and widget of that player; the player's next state confirms it, or after 3 s the views show its real state
/// again. A pause holds the position of this moment (taken from the player's state now, as every view counts it).
fn hold_play_state(ui: &AppWindow, player: &str, playing: bool) {
    with_state(|s| {
        let pos = pages::home_music(&s.states, player, None, slint::Color::default()).pos;
        s.music_hold = Some((player.to_string(), playing, Instant::now(), pos));
        s.music_skip = None;
        update_music(ui, s, false);
    });
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_secs(3), move || {
        if let Some(ui) = weak.upgrade() {
            refresh_pages(&ui);
        }
    });
}

/// Next / previous pressed on the panel: for up to 3 s the page keeps the play state it had (a player between two
/// tracks says paused or buffering for a moment), and once the new track has shown, the old one does not come back
/// (a player may report the old track again for a moment while it changes).
struct MusicSkip {
    /// The player it was pressed for.
    player: String,
    at: Instant,
    /// The track shown when the key was pressed (`track_key`).
    from: String,
    playing: bool,
    /// The newest other track seen since.
    to: Option<HomeMusic>,
}

fn track_key(m: &HomeMusic) -> String {
    format!("{}\u{1f}{}", m.title, m.artist)
}

/// Applies a running skip hold to the shown music; true when the player reported the old track again and the new
/// one is shown instead (the cover and the other details then stay as they are too).
fn apply_music_skip(s: &mut UiState, player: &str, music: &mut HomeMusic) -> bool {
    let Some(k) = s.music_skip.as_mut().filter(|k| k.player == player) else { return false };
    if k.at.elapsed() >= Duration::from_secs(3) {
        s.music_skip = None;
        return false;
    }
    let mut back = false;
    match skip_step(&k.from, &track_key(music), k.to.is_some()) {
        SkipStep::NewTrack => k.to = Some(music.clone()),
        SkipStep::OldAgain => {
            if let Some(to) = &k.to {
                *music = to.clone();
                back = true;
            }
        }
        SkipStep::Waiting => {}
    }
    if k.playing {
        music.playing = true;
    }
    back
}

#[derive(Debug, PartialEq)]
enum SkipStep {
    /// Another track than the one at the press: the newest one seen.
    NewTrack,
    /// The track of the press again after a new one had shown: the new one stays.
    OldAgain,
    /// Still the track of the press (the player has not changed yet).
    Waiting,
}

fn skip_step(from: &str, now: &str, seen_new: bool) -> SkipStep {
    if now != from {
        SkipStep::NewTrack
    } else if seen_new {
        SkipStep::OldAgain
    } else {
        SkipStep::Waiting
    }
}

thread_local! {
    /// The music page shown last (its row): its player is the panel's.
    static LAST_MUSIC: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The page on screen noted: a music page's player becomes the panel's.
fn note_shown_page(ui: &AppWindow) {
    if let Some(p) = ui.get_pages().row_data(ui.get_page().max(0) as usize).filter(|p| p.kind == "music") {
        LAST_MUSIC.with(|l| l.set(p.n.max(0) as usize));
    }
}

/// The panel's player: the player of the music page shown last (the first one until one is shown; on the first an
/// older pick on the panel goes first), else the integration's. Music widgets without a player of their own, the voice
/// satellite's music commands and its reports use it.
fn media_entity(s: &UiState) -> String {
    let list = pages::kind_pages(s.panel_config.as_ref(), "music");
    let n = LAST_MUSIC.with(|l| l.get());
    let n = if n < list.len() { n } else { 0 };
    music_player_of(s, n, list.get(n).map(|(_, p)| *p))
}

/// The player of music page `n` (its page's settings): the page's own, else the integration's; on the first page an
/// older pick on the panel goes first.
fn music_player_of(s: &UiState, n: usize, page: Option<&Value>) -> String {
    if n == 0 && !s.settings.music_player.is_empty() {
        return s.settings.music_player.clone();
    }
    page.and_then(|p| pages::entities_of(p).into_iter().find(|e| e.starts_with("media_player.")))
        .unwrap_or_else(|| config_str(s, "media_player_entity"))
}

/// The player of a custom page's music widget: the page's own, else the panel's.
fn home_player_of(s: &UiState, page: Option<&Value>) -> String {
    page.and_then(|p| pages::entities_of(p).into_iter().find(|e| e.starts_with("media_player."))).unwrap_or_else(|| media_entity(s))
}

/// Every player a page shows: the music pages' and the custom pages' music widgets' (the panel's among them).
fn shown_players(s: &UiState) -> Vec<String> {
    let cfg = s.panel_config.as_ref();
    let mut out = Vec::new();
    for (n, (_, p)) in pages::kind_pages(cfg, "music").into_iter().enumerate() {
        out.push(music_player_of(s, n, Some(p)));
    }
    for (_, p) in pages::kind_pages(cfg, "home") {
        out.push(home_player_of(s, Some(p)));
    }
    // (last: the list keeps its order whichever music page was shown last)
    out.push(media_entity(s));
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| !p.is_empty() && seen.insert(p.clone()));
    out
}

/// The player a tap on the music controls is for: the music page's or the custom page's on screen, else the panel's.
fn tap_player(ui: &AppWindow, s: &UiState) -> String {
    let slot = ui.get_pages().row_data(ui.get_page().max(0) as usize);
    let cfg = s.panel_config.as_ref();
    match slot {
        Some(p) if p.kind == "music" => {
            let n = p.n.max(0) as usize;
            music_player_of(s, n, pages::kind_pages(cfg, "music").get(n).map(|(_, p)| *p))
        }
        Some(p) if p.kind == "home" => home_player_of(s, pages::kind_pages(cfg, "home").get(p.n.max(0) as usize).map(|(_, p)| *p)),
        _ => media_entity(s),
    }
}

/// What a player shows on the music pages and widgets: its track, position, cover and colours, with a speaker that went
/// quiet and the panel's own presses held (play / pause, next / previous). True with it when a skip hold keeps the new
/// track over the old one the player reported again.
fn player_music(s: &mut UiState, player: &str) -> (HomeMusic, bool) {
    let cover = cover_accent(s, player);
    let mut music = pages::home_music(&s.states, player, s.arts.get(player).and_then(|a| a.img.as_ref()), cover);
    if music.playing && !really_playing(s, player) {
        music.playing = false;
    }
    apply_music_hold(s, player, &mut music);
    let back = apply_music_skip(s, player, &mut music);
    (music, back)
}

/// The cover a player's track asks for now, when it is another one than it has (to fetch); `back`: a skip hold keeps the
/// new track, and its cover with it.
fn wanted_art(s: &mut UiState, player: &str, back: bool) -> Option<(String, Option<String>)> {
    let shows = s.states.get(player).is_some_and(pages::shows_track);
    let url = s.states.get(player).and_then(|p| {
        pages::shows_track(p).then(|| p.attrs.get("entity_picture").and_then(Value::as_str).filter(|u| !u.is_empty()).map(str::to_string)).flatten()
    });
    let art = s.arts.entry(player.to_string()).or_default();
    // a paused track keeps its cover even if the player drops the picture meanwhile; a skip hold that keeps the new
    // track keeps its cover too
    let url = if back || (url.is_none() && shows) { art.url.clone().or(url) } else { url };
    if url == art.url {
        return None;
    }
    art.url = url.clone();
    Some((player.to_string(), url))
}

/// Every music page's and music widget's data from their players: all of it (`full`), or the parts that move every
/// second (the positions, and a widget whose play state changed). Returns the covers to fetch.
fn update_music(ui: &AppWindow, s: &mut UiState, full: bool) -> Vec<(String, Option<String>)> {
    let mut wanted = Vec::new();
    // (a kind without a page keeps one row: a temporary music page shows the panel's player)
    let music_pages = pages::inst_pages(s.panel_config.as_ref(), "music");
    let home_pages: Vec<Value> = pages::kind_pages(s.panel_config.as_ref(), "home").into_iter().map(|(_, p)| p.clone()).collect();
    let panel_accent = accent(s);
    for (n, (_, page)) in music_pages.iter().enumerate() {
        let player = music_player_of(s, n, (!page.is_null()).then_some(page));
        let (music, back) = player_music(s, &player);
        crate::pages::with_inst(ui.get_music_pos(), |m| ui.set_music_pos(m), n, |p| {
            if music.playing || *p != music.pos {
                *p = music.pos;
            }
        });
        if !full {
            // the play state turns at once
            crate::pages::with_inst(ui.get_music_inst(), |m| ui.set_music_inst(m), n, |d| d.playing = music.playing);
            continue;
        }
        if n == 0 {
            // which player shows which track (a mixed-up page can be traced)
            thread_local!(static SHOWN: RefCell<String> = const { RefCell::new(String::new()) });
            let shown = format!("{player}: {} \u{b7} {}", music.title, music.artist);
            if SHOWN.with(|v| v.replace(shown.clone())) != shown {
                eprintln!("nspanel-app: music page shows {shown}");
            }
        }
        let art = s.arts.get(&player).cloned().unwrap_or_default();
        let option = |k: &str| pages::option_of(page, k).cloned();
        let look = pages::variant_of(page);
        let wood = option("wood").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "walnut".into());
        let cover = cover_accent(s, &player);
        let p = s.states.get(&player).cloned();
        crate::pages::with_inst(ui.get_music_inst(), |m| ui.set_music_inst(m), n, |d| {
            d.title = music.title.clone();
            d.artist = music.artist.clone();
            d.playing = music.playing;
            d.dur = music.dur;
            d.player = music.player.clone();
            d.has_art = music.has_art;
            // the cover and its blurred copy are the shown player's: a player without one (or before its cover has
            // come) leaves none of the player shown before (the plain look's and the cassette's backdrops show it
            // without asking has_art)
            d.art = art.img.clone().unwrap_or_default();
            d.art_bg = if art.img.is_some() { art.bg.clone().unwrap_or_default() } else { slint::Image::default() };
            // the music page takes the cover's colour (the record, the accents), else the panel's
            d.accent = if music.has_art { cover } else { panel_accent };
            // the record: the cover's own colour as it is
            let record = if music.has_art { art.record } else { None };
            d.has_record = record.is_some();
            if let Some(c) = record {
                d.record = c;
            }
            // the rest of the page from the same player (nothing of another player stays: a player not known yet
            // shows none of it)
            if !back {
                let attr = |k: &str| p.as_ref().and_then(|p| p.attrs.get(k));
                d.album = attr("media_album_name").and_then(Value::as_str).unwrap_or("").into();
                d.volume = attr("volume_level").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                d.shuffle = attr("shuffle").and_then(Value::as_bool).unwrap_or(false);
                d.repeat = match attr("repeat").and_then(Value::as_str) {
                    Some("all") => 1,
                    Some("one") => 2,
                    _ => 0,
                };
            }
            // the page's own options: the cover behind the cassette (off as designed), the blurred cover, what the
            // record holds, cut off or scroll and its speed, the wood
            d.cs_cover = option("cover_bg").and_then(|v| v.as_bool()).unwrap_or(false);
            d.cover_blur = option("cover_blur").and_then(|v| v.as_bool()).unwrap_or(true);
            d.holds = option("holds").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            d.scroll = option("scroll").and_then(|v| v.as_bool()).unwrap_or(false);
            d.scroll_speed = option("scroll_speed").and_then(|v| v.as_i64()).unwrap_or(3) as f32;
            d.wood = WOODS.iter().position(|w| *w == wood).unwrap_or(0) as i32;
            // the turntable's wooden deck, the cassette's shell and hubs (full or half): the look's own, kept
            if look == 2 {
                d.tt_deck = look_picture(&format!("music/tt/deck-{wood}.png"), "music/tt/deck.png");
            }
            if look == 3 || look == 4 {
                let size = if look == 4 { "half" } else { "full" };
                d.cs_back = look_picture(&format!("music/cs/{size}-back.png"), "");
                d.cs_front = look_picture(&format!("music/cs/{size}-front.png"), "");
                d.cs_hub = look_picture(&format!("music/cs/{size}-hub.png"), "");
            }
        });
        if let Some(w) = wanted_art(s, &player, back) {
            wanted.push(w);
        }
    }
    crate::pages::keep_insts(&ui.get_music_inst(), music_pages.len());
    crate::pages::keep_insts(&ui.get_music_pos(), music_pages.len());
    // the custom pages' music widgets (a weather details page has none), each with its page's player
    for n in 0..home_pages.len().max(1) {
        let player = home_player_of(s, home_pages.get(n));
        let (music, back) = player_music(s, &player);
        crate::pages::with_inst(ui.get_home_music_pos(), |m| ui.set_home_music_pos(m), n, |p| {
            if music.playing || *p != music.pos {
                *p = music.pos;
            }
        });
        crate::pages::with_inst(ui.get_home_inst(), |m| ui.set_home_inst(m), n, |d| {
            // the rest of the widget's data only when it changed (the running time has its own property)
            let mut same = d.music.clone();
            same.pos = music.pos;
            if same != music || full {
                d.music = music;
            }
        });
        if full {
            if let Some(w) = wanted_art(s, &player, back) {
                wanted.push(w);
            }
        }
    }
    crate::pages::keep_insts(&ui.get_home_inst(), home_pages.len().max(1));
    crate::pages::keep_insts(&ui.get_home_music_pos(), home_pages.len().max(1));
    wanted
}

thread_local! {
    /// The looks' pictures loaded from the UI bundle (the turntable's woods, the cassettes' shells), kept.
    static LOOK_PICTURES: RefCell<HashMap<String, slint::Image>> = RefCell::new(HashMap::new());
}

/// A picture of a look (kept once loaded); `fallback` when it is missing.
fn look_picture(rel: &str, fallback: &str) -> slint::Image {
    LOOK_PICTURES.with(|m| {
        let mut m = m.borrow_mut();
        if let Some(img) = m.get(rel) {
            return img.clone();
        }
        let mut img = quick::picture(rel);
        if img.size().width == 0 && !fallback.is_empty() {
            img = quick::picture(fallback);
        }
        m.insert(rel.to_string(), img.clone());
        img
    })
}

/// Sends the covers to fetch to the artwork thread.
fn fetch_art(s: &UiState, wanted: Vec<(String, Option<String>)>) {
    if let Some(tx) = s.art_tx.as_ref() {
        for w in wanted {
            let _ = tx.send(w);
        }
    }
}

/// Entities the panel follows besides the dashboard's.
fn extra_entities(s: &UiState) -> Vec<String> {
    let mut out = vec!["sun.sun".to_string(), "zone.home".to_string(), weather_entity(s)];
    // every player a page shows, and the speaker behind a Music Assistant player (to see when it really plays)
    for player in shown_players(s) {
        if let Some(sp) = speaker_of(s, &player) {
            out.push(sp);
        }
        out.push(player);
    }
    out.extend(s.states.keys().filter(|k| k.starts_with("person.")).cloned());
    // the People widgets' people (everyone Home Assistant has, from the integration, for a widget that shows everyone)
    out.extend(home_edit::people_entities(s.panel_config.as_ref()));
    // the sensor widgets' sensors
    out.extend(home_edit::item_entities(s));
    if let Some(list) = s.panel_config.as_ref().and_then(|c| c.get("entities")).and_then(Value::as_array) {
        out.extend(list.iter().filter_map(Value::as_str).map(str::to_string));
    }
    // Every page's own entities (thermostats, tiles, cameras, calendars, lists).
    if let Some(pages) = s.panel_config.as_ref().and_then(|c| c.get("pages")).and_then(Value::as_array) {
        for p in pages {
            if let Some(list) = p.get("entities").and_then(Value::as_array) {
                out.extend(list.iter().filter_map(Value::as_str).map(str::to_string));
            }
            // entities named in a page's options (the doorbell, the door lock)
            if let Some(o) = p.get("options").and_then(Value::as_object) {
                for key in ["ring", "lock", "player"] {
                    if let Some(e) = o.get(key).and_then(Value::as_str).filter(|e| e.contains('.')) {
                        out.push(e.to_string());
                    }
                }
            }
        }
    }
    // the screensaver's light sensor from Home Assistant (panels without their own)
    if let Some(e) = s.panel_config.as_ref().and_then(|c| c.get("screensaver")).and_then(|v| v.get("light_entity")).and_then(Value::as_str) {
        out.push(e.to_string());
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|e| !e.is_empty() && seen.insert(e.clone()));
    out
}

// ---------------------------------------------------------------- clock and pages

/// Whether a model already holds these rows. A new model makes every element of
/// its `for` loop be created again (with its layers and text), so models are set
/// only when their rows change.
fn same_rows<T: Clone + PartialEq + 'static>(old: &ModelRc<T>, new: &[T]) -> bool {
    old.row_count() == new.len() && new.iter().enumerate().all(|(i, n)| old.row_data(i).as_ref() == Some(n))
}

/// What the weather shows, to tell whether a new WeatherData changes anything
/// (its hour and day lists are models, which compare by identity).
/// Loads the living weather icon's frames for a condition (spec M12): n frames for each of the sizes 34, 44, 50, 64
/// (lightning-rainy: the three rain frames and the bolt layer). Without its frames the icon is the plain glyph.
fn set_wx_frames(ui: &AppWindow, cond: &str) {
    let (name, n, kind) = pages::wx_motion(cond);
    let mut frames = Vec::new();
    for size in [34, 44, 50, 64] {
        for k in 0..n {
            let file = if kind == 7 && k == 3 { format!("home/wxicon/{name}-bolt-{size}.png") } else { format!("home/wxicon/{name}-{k}-{size}.png") };
            frames.push(quick::picture(&file));
        }
    }
    let ok = n > 0 && frames.iter().all(|f| f.size().width > 0);
    let w = ui.global::<WxIcons>();
    if ok {
        w.set_frames(slint::ModelRc::new(slint::VecModel::from(frames)));
        w.set_n(n as i32);
    } else {
        w.set_frames(slint::ModelRc::default());
        w.set_n(0);
    }
    w.set_kind(if ok || n == 0 { kind } else { 0 });
    eprintln!("nspanel-app: weather icon {cond}: {} frames, motion {kind}", if ok { n } else { 0 });
}

fn weather_key(w: &WeatherData) -> String {
    let hours: Vec<String> = w.hours.iter().map(|h| format!("{}{}{}{}", h.label, h.icon, h.temp, h.precip)).collect();
    let days: Vec<String> = w.days.iter().map(|d| format!("{}{}{}{}{}", d.label, d.icon, d.high, d.low, d.precip)).collect();
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{:.1}|{:.1}|{:.1}|{:.1}|{:.1}|{:.1}|{:.2}|{}|{}",
        w.icon, w.temperature, w.condition, w.feels, w.wind, w.humidity, w.precip, w.high_low, w.sunrise, w.sunset,
        w.mood, w.rain, w.theme, w.real_sun, w.wind_kmh.round(), w.gust_kmh.round(), w.sun_az, w.sun_el, w.moon_az, w.moon_el,
        w.dusk, w.dawn, w.moon_phase, hours.join(","), days.join(",")
    )
}

fn update_clock(ui: &AppWindow) {
    // states held after the link came back: applied when the two minutes are over
    let held = with_state(|s| {
        if s.held_states.is_empty() || s.linked_at.is_some_and(|t| t.elapsed() < Duration::from_secs(120)) {
            return Vec::new();
        }
        s.linked_at = None;
        s.held_states.drain().collect::<Vec<_>>()
    });
    if !held.is_empty() {
        handle_event(ui, Event::States(held));
    }
    // the clocks turn at hh:mm:00 (the 1 s tick could be up to a second late): once more right then
    thread_local!(static AT_MINUTE: std::cell::Cell<i64> = const { std::cell::Cell::new(0) });
    let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let to_minute = 60_000 - wall.rem_euclid(60_000);
    let minute = wall / 60_000 + 1;
    if to_minute < 1_100 && AT_MINUTE.with(|m| m.replace(minute)) != minute {
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(to_minute as u64 + 15), move || {
            if let Some(ui) = weak.upgrade() {
                update_clock(&ui);
            }
        });
    }
    let ring = STATE.with(|st| {
        let mut guard = st.borrow_mut();
        let s = guard.as_mut()?;
        let weather = ui.get_weather();
        let (mut data, digits) = pages::clock_data(&weather, &ui.get_home_event().title, &s.prev_digits, 1.0);
        if digits != s.digits {
            s.prev_digits = std::mem::replace(&mut s.digits, digits);
            s.flip_at = Some(Instant::now());
        }
        // The struct changes once a minute (a change re-lays out every text bound
        // to it); the seconds move in the UI from the animation clock, set here
        // when the minute turns (and at the start).
        data.seconds = 0.0;
        if ui.get_clock() != data {
            // the thermostat remote's own clock (hours and minutes as numbers)
            ui.set_thermo_clock_h(data.hh.parse().unwrap_or(0));
            ui.set_thermo_clock_m(data.mm.parse().unwrap_or(0));
            // the notifications page's pager and TV news clock (minutes of the day; -1 before the clock is set)
            ui.set_info_minute(info::minute_of(&data.hh, &data.mm));
            ui.set_clock(data);
            let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            ui.invoke_sync_seconds(wall.rem_euclid(60.0) as f32);
        }
        // Timer chips and the timers page only while there are timers (they count down).
        if !s.timers.is_empty() || ui.get_timers().row_count() > 0 {
            // Rows change in place each second (no element is built again).
            let (colourful, own) = timers_colours(page_cfg(s, "timers"));
            if let Some(m) = pages::update_rows(&ui.get_timers(), pages::timer_chips(&s.timers, accent(s), colourful, &own)) {
                ui.set_timers(m);
            }
            // the timer island comes back by itself when a new timer starts or a timer enters its last minute (29.3)
            let keys: Vec<String> = s.timers.keys().cloned().collect();
            let urgent: Vec<String> = s.timers.iter().filter(|(_, t)| t.active && t.remaining() < 60).map(|(k, _)| k.clone()).collect();
            let fresh = keys.iter().any(|k| !s.island_seen.contains(k));
            let late = urgent.iter().any(|k| !s.island_urgent.contains(k));
            if (fresh || late || keys.is_empty()) && ui.get_island_away() {
                ui.set_island_away(false);
            }
            s.island_seen = keys;
            s.island_urgent = urgent;
            update_timer_lists(ui, s);
            // the plain and synthwave grids: the four that end soonest (34.7)
            let grid = pages::timer_grid(&s.timers);
            if slint::Model::iter(&ui.get_timers_grid()).collect::<Vec<i32>>() != grid {
                ui.set_timers_grid(ModelRc::new(VecModel::from(grid)));
            }
        }
        // Timers the panel runs itself ring here when they reach zero.
        let done: Vec<String> =
            s.timers.iter().filter(|(_, t)| t.own && t.active && t.remaining() == 0).map(|(id, _)| id.clone()).collect();
        for id in done {
            if let Some(t) = s.timers.remove(&id) {
                let (colourful, own) = timers_colours(page_cfg(s, "timers"));
                let tint = pages::timer_tint(t.seq as u64, accent(s), colourful, &own);
                ring_timer(ui, s, t.name, tint);
                RANG.with(|r| r.set(true));
            }
        }
        // The music positions move on while a player plays: their own properties; the rest of a widget's data only
        // when it changed.
        note_shown_page(ui);
        update_music(ui, s, false);
        // The synthwave scenes in use: their view, colour and the next car.
        synth::tick(ui, s);
        // city windows lit one by one (drawn city / big city, the first such custom or weather details page)
        {
            let w = ui.get_weather();
            let pages = ui.get_pages();
            let slot = (0..slint::Model::row_count(&pages)).filter_map(|i| slint::Model::row_data(&pages, i))
                .find(|p| (p.kind == "home" || p.kind == "weather") && matches!(if p.theme >= 0 { p.theme } else { w.theme }, 4 | 6));
            let theme = slot.as_ref().map(|p| if p.theme >= 0 { p.theme } else { w.theme }).unwrap_or(-1);
            let light = slot.as_ref().is_some_and(|p| p.light == 1 || (p.light == 0 && !ui.global::<Theme>().get_dark()));
            let hour = local_hour();
            windows::tick(ui, theme, light, w.mood, w.sun_el, hour);
        }
        // do not disturb: the mark in the status line (the schedule turns by the minute)
        if s.dnd.is_some_and(|(_, then)| then != dnd_setting(s)) {
            s.dnd = None;
        }
        let dnd = dnd_active(s);
        if ui.get_dnd_on() != dnd {
            ui.set_dnd_on(dnd);
        }
        // Alarms: when the minute turns, the ones due ring; the list's "next" line follows.
        let minute = pages::now() / 60;
        if minute != s.alarm_minute {
            let first = s.alarm_minute == 0;
            s.alarm_minute = minute;
            if !first {
                if let Some(r) = alarms::due(s) {
                    s.snooze = None;
                    ring_alarm(ui, s, r.alarm, r.time, r.label, r.tone, r.hour);
                }
            }
            alarms::refresh(ui, s);
        }
        // a snoozed alarm rings again
        if s.snooze.as_ref().is_some_and(|sn| Instant::now() >= sn.at) {
            if let Some(sn) = s.snooze.take() {
                // the big time shows the time now (the alarm's own time has passed)
                let (time, hour) = alarms::now_hm();
                ring_alarm(ui, s, sn.alarm, time, sn.label, sn.tone, hour);
                alarms::refresh(ui, s);
            }
        }
        // Voice view: closes a few seconds after the end.
        if s.voice_hide_at.is_some_and(|t| Instant::now() >= t) {
            s.voice_hide_at = None;
            close_voice_view(ui);
        }
        // Ringing timer or alarm: its sound loops until stopped, at most a minute.
        match s.ringing {
            Some(started) if started.elapsed() > Duration::from_secs(60) => {
                eprintln!("nspanel-app: ringing stopped (a minute passed)");
                s.ringing = None;
                s.ringing_flag.store(false, Ordering::SeqCst);
                ui.set_timers_ringing(false);
                RING_OVER.with(|r| r.set(true));
                None
            }
            Some(started) => {
                ui.set_timers_ring_over(started.elapsed().as_secs_f32());
                // do not disturb may keep alarms or timers silent (they still show)
                let what = if ui.get_timers_ring_alarm() { "alarms" } else { "timers" };
                dnd_allows(s, what).then(|| format!("ring:{}", s.ring_tone))
            }
            None => None,
        }
    });
    // status screens and the critical alert: their times, the strip; the alert's sound loops until held
    let crit = alerts::refresh(ui).map(|tone| format!("crit:{tone}|sos"));
    with_state(|s| loop_sound(s, crit.or(ring)));
    if RANG.with(|r| r.replace(false)) {
        wake(ui);
        // the ringing screen is the timers page's: a temporary one when the list has none
        visit_page(ui, "timers", Hold::Ring);
    }
    if RING_OVER.with(|r| r.replace(false)) {
        end_visit(ui, true);
    }
}

thread_local! {
    /// A timer went away: its sheet (if open) closes.
    static TIMER_SHEET_CLOSE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// A timer the panel runs reached zero during this clock update.
    static RANG: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// A ring ended by itself (its minute passed) during this clock update.
    static RING_OVER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Weather, home widgets, music page and people from the current states.
/// The status bar's hint line: one of the wake word's lines, kept until the wake word changes (a new one every 10
/// min); with the microphone muted one of its own lines instead, a new one each time it gets muted.
fn wake_hint_text(s: &mut UiState) -> String {
    if s.settings.mic_muted {
        if s.muted_hint.is_empty() {
            s.muted_hint = texts::fresh("voice.muted");
        }
        s.muted_hint.clone()
    } else if s.wake_phrase.is_empty() {
        s.muted_hint.clear();
        String::new()
    } else {
        s.muted_hint.clear();
        let stale = s.wake_hint_at.is_none_or(|t| t.elapsed() > Duration::from_secs(600));
        if s.wake_hint.0 != s.wake_phrase || stale {
            s.wake_hint = (s.wake_phrase.clone(), texts::fresh("voice.wake").replace("{wake}", &s.wake_phrase));
            s.wake_hint_at = Some(Instant::now());
        }
        s.wake_hint.1.clone()
    }
}

/// Sets the hint line now (after the microphone was muted or unmuted).
pub fn refresh_hint(ui: &AppWindow) {
    let hint = with_state(|s| wake_hint_text(s));
    if ui.get_wake_hint().as_str() != hint {
        ui.set_wake_hint(hint.into());
    }
}

/// A timers page's colours: colourful timers or the panel's accent, and its own colours for the 1st, 2nd, … timer
/// (the integration's; none: the panel's own colourful ones).
fn timers_colours(page: Option<&Value>) -> (bool, Vec<slint::Color>) {
    let colourful = page.and_then(|p| pages::option_of(p, "colourful")).and_then(Value::as_bool).unwrap_or(true);
    let own = page
        .and_then(|p| pages::option_of(p, "colours"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter_map(pages::parse_color).collect())
        .unwrap_or_default();
    (colourful, own)
}

/// A timers page's functions as its settings have them (alarm, timer, stopwatch; all of them without a choice).
fn timers_functions(page: Option<&Value>) -> [bool; 3] {
    let list = page.and_then(|p| pages::option_of(p, "functions")).and_then(Value::as_array).cloned().unwrap_or_default();
    let on = |k: &str| list.iter().any(|v| v.as_str() == Some(k));
    let f = [on("alarm"), on("timer"), on("stopwatch")];
    if f.iter().any(|x| *x) {
        f
    } else {
        [true, true, true]
    }
}

/// The tab a timers page shows with these functions: `mode` while its function is on, else the timers, the alarms, the
/// stopwatch, the first that is.
fn timers_tab(mode: i32, on: [bool; 3]) -> i32 {
    if on[mode.clamp(0, 2) as usize] {
        mode.clamp(0, 2)
    } else if on[1] {
        1
    } else if on[0] {
        0
    } else {
        2
    }
}

/// Every timers page's own settings: its functions (and the tab it shows among them), its colours, the stopwatch's
/// and the alarms' looks, its synthwave scene choice. A kind without a page keeps one row of defaults (a temporary
/// timers page shows it).
fn refresh_timers(ui: &AppWindow, s: &mut UiState) {
    let list = pages::inst_pages(s.panel_config.as_ref(), "timers");
    for (n, (_, page)) in list.iter().enumerate() {
        let page = (!page.is_null()).then_some(page);
        let on = timers_functions(page);
        let (colourful, _) = timers_colours(page);
        let get = |k: &str| page.and_then(|p| pages::option_of(p, k)).and_then(Value::as_i64);
        let sw_look = get("sw_look").unwrap_or(0).clamp(0, 4) as i32;
        let alarm_look = get("alarm_look").unwrap_or(0).clamp(0, 4) as i32;
        let sw_choice = get("scene").map_or(-2, |k| k.clamp(-1, 3) as i32);
        pages::with_inst(ui.get_timers_inst(), |m| ui.set_timers_inst(m), n, |d| {
            if !d.ready {
                d.ready = true;
                d.mode = 1;
            }
            d.alarm_on = on[0];
            d.timer_on = on[1];
            d.watch_on = on[2];
            d.mode = timers_tab(d.mode, on);
            d.colourful = colourful;
            d.sw_look = sw_look;
            d.alarm_look = alarm_look;
            d.sw_choice = sw_choice;
        });
    }
    pages::keep_insts(&ui.get_timers_inst(), list.len());
    update_timer_lists(ui, s);
}

/// Every timers page's list of the running timers, each in its page's colours; the shade's list in the colours of the
/// timers page the panel acts on.
fn update_timer_lists(ui: &AppWindow, s: &UiState) {
    let list = pages::inst_pages(s.panel_config.as_ref(), "timers");
    for (n, (_, page)) in list.iter().enumerate() {
        let (colourful, own) = timers_colours((!page.is_null()).then_some(page));
        let items = pages::timer_items(&s.timers, accent(s), colourful, &own);
        pages::with_inst(ui.get_timers_inst(), |m| ui.set_timers_inst(m), n, |d| {
            if let Some(m) = pages::update_rows(&d.list, items) {
                d.list = m;
            }
        });
    }
    let (colourful, own) = timers_colours(page_cfg(s, "timers"));
    if let Some(m) = pages::update_rows(&ui.get_timers_list(), pages::timer_items(&s.timers, accent(s), colourful, &own)) {
        ui.set_timers_list(m);
    }
}

/// The timers page the panel acts on: its settings changed by `f` (shown at once; the menu that changed them sends
/// them to the integration).
fn with_timers(ui: &AppWindow, f: impl FnOnce(&mut TimersInst)) {
    pages::with_inst(ui.get_timers_inst(), |m| ui.set_timers_inst(m), acting_n("timers"), f);
}

/// Every clock page's own options (the integration's page options): its parts, letters, width, wandering speed and
/// colour.
fn refresh_clocks(ui: &AppWindow, s: &UiState) {
    let list = pages::inst_pages(s.panel_config.as_ref(), "clock");
    for (n, (_, page)) in list.iter().enumerate() {
        let co = |k: &str| pages::option_of(page, k);
        let flag = |k: &str| co(k).and_then(Value::as_bool).unwrap_or(true);
        let colour = co("colour").and_then(Value::as_str).and_then(pages::parse_color);
        let inst = ClockInst {
            own_accent: colour.is_some(),
            accent: colour.unwrap_or_default(),
            weight: co("weight").and_then(Value::as_i64).unwrap_or(600) as i32,
            width: co("width").and_then(Value::as_i64).unwrap_or(100) as f32 / 100.0,
            date: flag("date"),
            weather: flag("weather"),
            next: flag("next"),
            seconds: flag("seconds"),
            speed: co("speed").and_then(Value::as_i64).unwrap_or(20) as f32,
        };
        pages::with_inst(ui.get_clock_inst(), |m| ui.set_clock_inst(m), n, |d| *d = inst);
    }
    pages::keep_insts(&ui.get_clock_inst(), list.len());
}

fn refresh_pages(ui: &AppWindow) {
    let art = with_state(|s| {
        let theme = first_page_option(s, "home", "theme")
            .and_then(|v| v.as_i64())
            .or_else(|| s.panel_config.as_ref().and_then(|c| c.get("home_theme")).and_then(Value::as_i64))
            .unwrap_or(0);
        // the plain background (9) has no sky: the weather details page keeps the nature photo
        let theme = if theme == 9 { 0 } else { theme.clamp(0, 8) as i32 };
        let entity = weather_entity(s);
        let weather = pages::weather_data(&pages::WeatherInput {
            states: &s.states,
            entity: &entity,
            hourly: &s.forecast_hourly,
            daily: &s.forecast_daily,
            theme,
        });
        let mut weather = weather;
        // the weather page's switches: the real sun and moon (photo and drawn backgrounds), cars and
        // neon letters (synthwave)
        if let Some(b) = first_page_option(s, "home", "real_sun").and_then(|v| v.as_bool()) {
            weather.real_sun = b;
        }
        let cars = first_page_option(s, "home", "cars").and_then(|v| v.as_bool()).unwrap_or(true);
        if ui.get_home_fl_cars_on() != cars {
            ui.set_home_fl_cars_on(cars);
        }
        // neon letters: each widget has its own switch now (HomeItem.neon; the page's old switch is only
        // the default of widgets without one), so the page lets them all glow
        if !ui.get_home_fl_neon() {
            ui.set_home_fl_neon(true);
        }
        // Development: weather values forced from the dev input (measurements).
        if let Some(o) = s.dev_wx.as_ref() {
            let f = |k: &str| o.get(k).and_then(Value::as_f64);
            if let Some(v) = f("mood") { weather.mood = v as i32; }
            if let Some(v) = f("rain") { weather.rain = v as i32; }
            if let Some(v) = f("wind") { weather.wind_kmh = v as f32; }
            if let Some(v) = f("gust") { weather.gust_kmh = v as f32; }
            if let Some(v) = f("theme") { weather.theme = v as i32; }
            if let Some(v) = f("sun_el") { weather.sun_el = v as f32; }
        }
        // the living weather icon: the frames of the condition (spec M12)
        let cond = s.states.get(&entity).map(|st| st.state.clone()).unwrap_or_default();
        if cond != s.wx_cond {
            set_wx_frames(ui, &cond);
            s.wx_cond = cond;
        }
        // rain on the glass over the photo backgrounds: the scene's rain and wind (and the panel's switch)
        glass::set_weather(s.settings.rain_on_glass, glass::rain_level(weather.rain, weather.mood), weather.wind_kmh, weather.gust_kmh);
        let key = weather_key(&weather);
        if key != s.weather_key {
            s.weather_key = key;
            ui.set_weather(weather);
        }
        let a = accent(s);
        if ui.get_clock_accent() != a {
            ui.set_clock_accent(a);
        }
        // the People widgets' groups are collected again as the widgets are read
        home_edit::people_start(s.panel_config.as_ref());
        let items = pages::home_items(s.panel_config.as_ref(), a);
        if !same_rows(&ui.get_home_items(), &items) {
            ui.set_home_items(ModelRc::new(VecModel::from(items)));
        }
        let lays = pages::home_layouts(s.panel_config.as_ref(), a);
        let shown = ui.get_home_layouts();
        let same = shown.row_count() == lays.len() && lays.iter().enumerate().all(|(i, l)| shown.row_data(i).is_some_and(|o| same_rows(&o.items, l)));
        if !same {
            let lays: Vec<HomeLayout> = lays.into_iter().map(|items| HomeLayout { items: ModelRc::new(VecModel::from(items)) }).collect();
            ui.set_home_layouts(ModelRc::new(VecModel::from(lays)));
        }
        // the weather details page's widgets while it has none of its own
        let wx = pages::weather_items(a);
        if !same_rows(&ui.get_weather_items(), &wx) {
            ui.set_weather_items(ModelRc::new(VecModel::from(wx)));
        }
        // each custom page's next event of its calendars (its entities); the first one's also on the clock faces
        let homes: Vec<Value> = pages::kind_pages(s.panel_config.as_ref(), "home").into_iter().map(|(_, p)| p.clone()).collect();
        for n in 0..homes.len().max(1) {
            let calendars: Vec<String> =
                homes.get(n).map(pages::entities_of).unwrap_or_default().into_iter().filter(|e| e.starts_with("calendar.")).collect();
            let event = cal::next_event(&s.states, &calendars, pages::now());
            if n == 0 && ui.get_home_event() != event {
                ui.set_home_event(event.clone());
            }
            crate::pages::with_inst(ui.get_home_inst(), |m| ui.set_home_inst(m), n, |d| d.event = event);
        }
        let t = pages::now();
        if clock::valid() {
            ui.set_home_weekday(pages::local_format(t, "%A").into());
            ui.set_home_day_month(pages::local_format(t, "%e %B").trim().to_string().into());
            ui.set_home_year(pages::local_format(t, "%Y").into());
            ui.set_home_week(format!("Week {}", pages::local_format(t, "%V").trim_start_matches('0')).into());
        }
        let wanted: Vec<String> = s
            .panel_config
            .as_ref()
            .and_then(|c| c.get("people"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let people = pages::people(&s.states, &wanted);
        if !same_rows(&ui.get_people(), &people) {
            ui.set_people(ModelRc::new(VecModel::from(people)));
        }
        // the small heads at the bottom of every page (the settings' show_people): only when asked for
        ui.set_show_people(s.panel_config.as_ref().and_then(|c| c.get("show_people")).and_then(Value::as_bool).unwrap_or(false));
        let hint = wake_hint_text(s);
        if ui.get_wake_hint().as_str() != hint {
            ui.set_wake_hint(hint.into());
        }
        // the music pages and the music widgets
        let art = update_music(ui, s, true);
        // the sensor widgets' values
        home_edit::refresh(ui, s);
        s.weather_refreshed = Instant::now();
        art
    });
    if slint::Model::row_count(&ui.get_music_woods()) == 0 {
        let sw: Vec<slint::Image> = WOODS.iter().map(|w| quick::picture(&format!("music/tt/sw-{w}.png"))).collect();
        ui.set_music_woods(ModelRc::new(VecModel::from(sw)));
    }
    if page_looks(ui, "music").contains(&2) && ui.get_music_tt_grooves().size().width == 0 {
        ui.set_music_tt_grooves(quick::picture("music/tt/grooves.png"));
    }
    with_state(|s| {
        refresh_timers(ui, s);
        refresh_clocks(ui, s);
        quick::refresh(ui, s);
        thermo::refresh(ui, s);
        cam::refresh(ui, s);
        cal::refresh(ui, s);
        todo::refresh(ui, s);
        info::refresh(ui, s);
        fetch_art(s, art);
    });
}

/// Keys of `top` replace those of `base` (a missing base becomes `top`).
fn overlay(base: &mut Option<Value>, top: &Value) {
    let b = base.get_or_insert_with(|| json!({}));
    if let (Some(b), Some(t)) = (b.as_object_mut(), top.as_object()) {
        for (k, v) in t {
            b.insert(k.clone(), v.clone());
        }
    }
}

thread_local! {
    /// Draws the pages once after the page list changed.
    static REWARM: slint::Timer = slint::Timer::default();
}

/// Every page drawn once, one per frame, under the page in view, so the first
/// swipe to a new page costs no more than any other.
fn rewarm(ui: &AppWindow) {
    let weak = ui.as_weak();
    let next = std::cell::Cell::new(0i32);
    REWARM.with(|t| {
        t.start(slint::TimerMode::Repeated, Duration::from_millis(40), move || {
            let Some(ui) = weak.upgrade() else { return };
            let k = next.get();
            if k < ui.get_pages().row_count() as i32 {
                ui.set_warm_page(if k == ui.get_page() { -1 } else { k });
                next.set(k + 1);
            } else {
                ui.set_warm_page(-1);
                REWARM.with(|t| t.stop());
            }
        })
    });
}

/// The page list and what the integration's configuration changes.
fn apply_panel_config(ui: &AppWindow) {
    // copies of the uploaded sounds the settings use: fetched when missing, dropped when unused
    with_state(|s| sounds::keep(&used_sounds(s)));
    // the panel's speech recognition (stop words while a timer rings)
    with_state(|s| {
        let v = s.panel_config.as_ref().and_then(|c| c.get("stt")).cloned().unwrap_or(Value::Null);
        stt::set_engine(v["engine"].as_str().unwrap_or(""), v["language"].as_str().unwrap_or(""));
    });
    with_state(|s| {
        if s.screensaver {
            saver_clock(ui, s);
        }
    });
    // Wi-Fi roaming, the wake distance, the child lock
    with_state(|s| device_options::apply(ui, s));
    let reload = with_state(|s| {
        let slots = pages::page_slots(s.panel_config.as_ref());
        let current = ui.get_page();
        // A new model rebuilds every page: only when the list really changed.
        let shown = ui.get_pages();
        let same = shown.row_count() == slots.len() && slots.iter().enumerate().all(|(i, p)| shown.row_data(i).as_ref() == Some(p));
        if !same && s.temp_page.is_none() {
            ui.set_pages(ModelRc::new(VecModel::from(slots.clone())));
            // a drag a page held for itself (a slider, the turntable's arm) ends with the page it was on: the pages
            // swipe again
            ui.global::<Act>().set_hold_swipe(false);
            rewarm(ui);
        }
        if current as usize >= slots.len() {
            ui.set_page(0);
        }
        set_dashboard(ui, s.settings.display_mode == "dashboard" && s.web.is_some());
        let mut changed = false;
        let extra = extra_entities(s);
        // an entity a page needs that the running session does not follow (e.g. pages from the stored settings
        // before the session knew them): a new session
        {
            let subscribed = s.shared.subscribed.lock().unwrap();
            // asked once until the new session has subscribed (it clears the flag)
            if !subscribed.is_empty()
                && !s.shared.reload_asked.load(Ordering::SeqCst)
                && extra.iter().any(|e| !e.is_empty() && !subscribed.contains(e))
            {
                changed = true;
            }
        }
        {
            let mut e = s.shared.extra_entities.lock().unwrap();
            if *e != extra {
                *e = extra;
                changed = true;
            }
        }
        {
            let entity = weather_entity(s);
            let mut f = s.shared.forecast_entity.lock().unwrap();
            if *f != entity {
                *f = entity;
                s.forecast_hourly.clear();
                s.forecast_daily.clear();
                changed = true;
            }
        }
        // the flag only when the request can reach the session (before the link it is asked again later)
        if changed && s.commands.is_some() {
            s.shared.reload_asked.store(true, Ordering::SeqCst);
        }
        changed
    });
    refresh_pages(ui);
    if reload {
        eprintln!("nspanel-app: the pages follow other entities: new Home Assistant session");
        send(Command::Reload);
    }
}

/// Goes to a page of `kind` (`pages::acting_slot`: the page on screen when it is of that kind, else the first of the
/// kind); returns false if the panel has none.
fn go_to_page(ui: &AppWindow, kind: &str) -> bool {
    let pages = ui.get_pages();
    let slots: Vec<PageSlot> = pages.iter().collect();
    match pages::acting_slot(&slots, ui.get_page().max(0) as usize, kind) {
        Some(i) => {
            if ui.get_page() != i as i32 {
                close_page_sheets(ui);
            }
            ui.set_page(i as i32);
            // a temporary page shown before goes once left
            leave_temporary_page(ui, i as i32);
            true
        }
        None => false,
    }
}

/// Goes to the page at `cfg` in the integration's list (a doorbell's own cameras page); false when the pager lacks it.
fn go_to_cfg(ui: &AppWindow, cfg: usize) -> bool {
    let pages = ui.get_pages();
    let Some(i) = (0..pages.row_count()).find(|&i| pages.row_data(i).is_some_and(|p| p.cfg == cfg as i32)) else { return false };
    if ui.get_page() != i as i32 {
        close_page_sheets(ui);
    }
    ui.set_page(i as i32);
    leave_temporary_page(ui, i as i32);
    true
}

/// Leaving a page closes its sheets, editors and keyboard views (they never wait on a page nobody sees, and never
/// hide the dots of the others).
fn close_page_sheets(ui: &AppWindow) {
    ui.set_quick_sheet(false);
    ui.set_quick_settings(false);
    ui.set_thermo_sheet(false);
    ui.set_thermo_picker(false);
    ui.set_thermo_settings(false);
    ui.set_music_picker(false);
    ui.set_music_settings(false);
    cal::close_all(ui);
    ui.set_timers_quick(false);
    ui.set_timers_sheet(-1);
    ui.set_timers_settings(false);
    ui.set_timers_edit(false);
    ui.set_timers_edit_keyboard(false);
    ui.set_todo_picker(false);
    ui.set_todo_settings(false);
    ui.set_todo_pend_look(-1);
    ui.set_todo_adding(false);
    ui.set_cam_settings(false);
    ui.set_cam_replies(false);
    ui.set_info_settings(false);
    info::leave(ui);
    home_edit::close(ui);
}

thread_local! {
    /// The visit the panel made by itself (a ring, a view shown for some seconds), while it holds.
    static VISIT: std::cell::Cell<Option<Visit>> = const { std::cell::Cell::new(None) };
}

/// Goes to a page by itself (a timer rings, Home Assistant shows a view for some seconds) and remembers where it came
/// from, so `end_visit` can go back there.
fn visit_page(ui: &AppWindow, kind: &str, hold: Hold) {
    // the page's menus close (a ring or a shown view comes over them, also on the same page)
    close_page_sheets(ui);
    let page = ui.get_page();
    let mut v = Visit::base(VISIT.with(|c| c.get()), page);
    let to = if go_to_page(ui, kind) {
        ui.get_page()
    } else {
        let new = with_state(|s| s.temp_page.is_none());
        let Some((at, took_over)) = open_temporary_page(ui, kind) else { return };
        let at = at as i32;
        if new {
            v = v.inserted(at);
        } else if took_over && v.from == at {
            // the temporary page it came from now shows this: back to the page that one was opened next to
            v.from = (at - 1).max(0);
        }
        at
    };
    VISIT.with(|c| c.set(Some(v.hold(to, hold))));
}

/// The ring stopped (`ring`) or the shown view's time is up: back to the page from before (or to the view still
/// shown), unless someone paged meanwhile. The other one's hold stays.
fn end_visit(ui: &AppWindow, ring: bool) {
    let Some(v) = VISIT.with(|c| c.get()) else { return };
    let (left, go) = v.end(ring, ui.get_page(), Instant::now());
    VISIT.with(|c| c.set(left));
    let Some(page) = go else { return };
    if page < 0 || page as usize >= ui.get_pages().row_count() {
        return;
    }
    close_page_sheets(ui);
    ui.set_page(page);
    // a temporary page the visit showed goes once left
    leave_temporary_page(ui, page);
}

/// A view shown without seconds stays: an earlier shown view's way back is dropped (a ring's stays).
fn end_show_visit() {
    VISIT.with(|c| {
        c.set(c.get().and_then(|mut v| {
            v.show = None;
            v.ring.is_some().then_some(v)
        }))
    });
}

/// A page the panel's list lacks (a home widget's view): slides in next to the current one, without a dot; a swipe
/// back to the right returns, and it is gone once left. The page left closes its sheets (they would hold the swipes).
/// A temporary page already there takes the new kind. Returns its place in the pager (a new one shows there 40 ms
/// later) and whether it took over a temporary page of another kind.
fn open_temporary_page(ui: &AppWindow, kind: &str) -> Option<(usize, bool)> {
    let pages = ui.get_pages();
    let vec = pages.as_any().downcast_ref::<VecModel<PageSlot>>()?;
    let slot = pages::plain_slot(kind, 0);
    if let Some(t) = with_state(|s| s.temp_page) {
        if t >= vec.row_count() {
            // still on its way in
            return Some((t, false));
        }
        let other = vec.row_data(t).is_some_and(|p| p.kind != kind);
        if other || ui.get_page() != t as i32 {
            close_page_sheets(ui);
        }
        if other {
            vec.set_row_data(t, slot);
        }
        ui.set_page(t as i32);
        if other {
            refresh_pages(ui);
        }
        return Some((t, other));
    }
    close_page_sheets(ui);
    let at = (ui.get_page().max(0) as usize + 1).min(vec.row_count());
    with_state(|s| s.temp_page = Some(at));
    // the page frame restarts at the page shown (the same picture, drawn once), then the new page slides in
    ui.set_instant(true);
    ui.invoke_reset_pos();
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(40), move || {
        let Some(ui) = weak.upgrade() else { return };
        ui.set_instant(false);
        let pages = ui.get_pages();
        let Some(vec) = pages.as_any().downcast_ref::<VecModel<PageSlot>>() else { return };
        vec.insert(at.min(vec.row_count()), slot);
        ui.set_dots_hidden(true);
        ui.set_page(at as i32);
        refresh_pages(&ui);
    });
    Some((at, false))
}

/// After a swipe: the temporary page goes once it is off screen (the pages
/// after it move up by one, with no visible slide).
fn leave_temporary_page(ui: &AppWindow, now: i32) {
    let Some(t) = with_state(|s| s.temp_page) else { return };
    if now as usize == t {
        return;
    }
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(320), move || {
        let Some(ui) = weak.upgrade() else { return };
        // back on it meanwhile (a visit took it over), or another one in its place: it stays
        if with_state(|s| s.temp_page) != Some(t) || ui.get_page() as usize == t {
            return;
        }
        with_state(|s| s.temp_page = None);
        let pages = ui.get_pages();
        let Some(vec) = pages.as_any().downcast_ref::<VecModel<PageSlot>>() else { return };
        if t < vec.row_count() {
            ui.set_instant(true);
            vec.remove(t);
            VISIT.with(|v| v.set(v.get().map(|v| v.removed(t as i32))));
            let page = ui.get_page();
            if page as usize > t {
                ui.set_page(page - 1);
            }
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(50), move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_instant(false);
                }
            });
        }
        ui.set_dots_hidden(false);
    });
}

// ---------------------------------------------------------------- timers and voice

fn handle_timer(ui: &AppWindow, kind: i32, id: String, name: String, left: u32, total: u32, active: bool) {
    let finished = handle_timer_state(ui, kind, id, name, left, total, active);
    if TIMER_SHEET_CLOSE.with(|c| c.replace(false)) || finished {
        ui.set_timers_sheet(-1);
    }
    if finished {
        wake(ui);
        visit_page(ui, "timers", Hold::Ring);
    }
    update_clock(ui);
}

fn handle_timer_state(ui: &AppWindow, kind: i32, id: String, name: String, left: u32, total: u32, active: bool) -> bool {
    with_state(|s| {
        if s.dismissed_timers.contains(&id) {
            if kind >= 2 {
                s.dismissed_timers.remove(&id);
            }
            return false;
        }
        match kind {
            // started, updated (unless the panel runs it now)
            0 | 1 => {
                if s.timers.get(&id).is_some_and(|t| t.own) {
                    return false;
                }
                let total = if total > 0 { total } else { s.timers.get(&id).map(|t| t.total).unwrap_or(left) };
                let seq = match s.timers.get(&id) {
                    Some(t) => t.seq,
                    None => {
                        s.timer_seq += 1;
                        s.timer_seq
                    }
                };
                s.timers.insert(
                    id,
                    pages::Timer { name, active, end: Instant::now() + Duration::from_secs(left as u64), left, total, seq, own: false },
                );
                false
            }
            // cancelled (by voice too): gone from the panel as well, even when the panel had taken it over after a
            // tap (otherwise the voice says it is cancelled while the panel keeps counting)
            2 => {
                s.timers.remove(&id);
                TIMER_SHEET_CLOSE.with(|c| c.set(true));
                false
            }
            // finished (a timer the panel runs rings by its own clock)
            3 => {
                if s.timers.get(&id).is_some_and(|t| t.own) {
                    return false;
                }
                let gone = s.timers.remove(&id);
                let (colourful, own) = timers_colours(page_cfg(s, "timers"));
                let tint = pages::timer_tint(gone.as_ref().map_or(0, |t| t.seq as u64), accent(s), colourful, &own);
                let name = gone.map(|t| t.name).unwrap_or(name);
                ring_timer(ui, s, name, tint);
                true
            }
            _ => false,
        }
    })
}

/// A timer reached zero: the ringing screen and the beeps.
/// The timers page's sound: chosen in the integration, else its look's own.
fn timer_tone(ui: &AppWindow, s: &UiState) -> String {
    let _ = ui;
    let look = page_look("timers");
    let value = page_option(s, "timers", "sound").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "auto".into());
    format!("{value}|{}", sounds::timer_default(look))
}

/// The uploaded sounds the panel's settings use (a copy of each is kept on the panel).
pub fn used_sounds(s: &UiState) -> Vec<String> {
    let mut out: Vec<String> = s.settings.alarms.iter().map(|a| a.tone.clone()).collect();
    if let Some(pages) = s.panel_config.as_ref().and_then(|c| c.get("pages")).and_then(Value::as_array) {
        out.extend(pages.iter().filter_map(|p| p.get("options")?.get("sound")?.as_str().map(str::to_string)));
    }
    // the shared critical alerts' own sounds, so they ring without Home Assistant too
    if let Some(list) = s.panel_config.as_ref().and_then(|c| c.get("keep_sounds")).and_then(Value::as_array) {
        out.extend(list.iter().filter_map(Value::as_str).map(str::to_string));
    }
    out
}

/// The panel's own media player entity in Home Assistant (the integration tells it).
/// The speaker a Music Assistant player plays on (the integration links them by their device).
fn speaker_of(s: &UiState, player: &str) -> Option<String> {
    s.panel_config.as_ref()?.get("player_links")?.get(player)?.as_str().map(str::to_string)
}

/// Whether the music a player reports as playing is really playing: false once its speaker has been
/// idle (or off) for 30 s while the player still says playing (then the panel shows it paused).
fn really_playing(s: &UiState, player: &str) -> bool {
    // (speaker, quiet since, last asked): the quiet time counts only while the player keeps saying playing; a gap
    // in the asking (the player was paused) starts it again, so a speaker that follows a new start a moment late
    // never makes a fresh start look paused
    // (each speaker its own: several pages may show several players)
    thread_local!(static QUIET_SINCE: std::cell::RefCell<HashMap<String, (Instant, Instant)>> = std::cell::RefCell::new(HashMap::new()));
    let Some(sp) = speaker_of(s, player) else { return true };
    // the panel's own speaker: Music Assistant's music comes over Sendspin, not through that media player
    if sp == media_player_entity(s) && sendspin::music_active() {
        return true;
    }
    let quiet = s.states.get(&sp).is_some_and(|st| !matches!(st.state.as_str(), "playing" | "buffering" | "unavailable" | "unknown"));
    QUIET_SINCE.with(|q| {
        let mut q = q.borrow_mut();
        if !quiet {
            q.remove(&sp);
            return true;
        }
        let now = Instant::now();
        match q.get_mut(&sp) {
            // a player shown on several pages is asked several times a second: that counts as one look
            Some((since, last)) if last.elapsed() < Duration::from_secs(3) => {
                *last = now;
                since.elapsed() < Duration::from_secs(30)
            }
            _ => {
                q.insert(sp, (now, now));
                true
            }
        }
    })
}

/// The live preview's stream: while a page asks (until 3 s after its last request), the next picture
/// is taken as soon as the last one is encoded and on its way, at most 10 a second. One at a time, as
/// JPEG, encoded off the UI thread.
fn screen_stream(ui: &AppWindow) {
    let _ = ui;
    STREAM_UNTIL_MS.store(unix_ms() + 3000, Ordering::SeqCst);
    stream_shot();
}

static STREAM_UNTIL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Preview pictures written to Home Assistant so far, and how long the last one took to write (the
/// Home Assistant thread counts them; the stream paces itself by them).
pub static SCREENS_SENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static SCREEN_SEND_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// When the picture under way was asked for (0: none under way).
static STREAM_BUSY_SINCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn stream_shot() {
    let started = unix_ms();
    let busy = STREAM_BUSY_SINCE.load(Ordering::SeqCst);
    // a picture that never came back (2 s) does not stop the stream
    if busy != 0 && started.saturating_sub(busy) < 2000 {
        return;
    }
    STREAM_BUSY_SINCE.store(started, Ordering::SeqCst);
    let tx = with_state(|s| s.commands.clone());
    platform::request_screen(move |w, h, rgb| {
        std::thread::spawn(move || {
            // below the UI's own threads: the screen stays smooth while the picture is encoded
            // SAFETY: setpriority on this thread's own id.
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 19);
            }
            let before = SCREENS_SENT.load(Ordering::SeqCst);
            if let (Some(jpeg), Some(tx)) = (panel_device::jpeg_base64(w, h, &rgb), tx) {
                let _ = tx.send(Command::Screen(jpeg));
                // the next picture only once this one is out on the network (up to 3 s)
                let t = std::time::Instant::now();
                while SCREENS_SENT.load(Ordering::SeqCst) == before && t.elapsed() < Duration::from_secs(3) {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let spent = unix_ms().saturating_sub(started);
            #[cfg(feature = "gpu")]
            if gpu::FRAME_STATS.load(Ordering::Relaxed) {
                eprintln!("nspanel-app: preview picture in {spent} ms (sent in {} ms)", SCREEN_SEND_MS.load(Ordering::SeqCst));
            }
            // at most 5 a second, and never more than half of the link: a picture that took long to
            // go out (a busy Wi-Fi) leaves as long a pause after it
            let gap = 200u64.max(2 * SCREEN_SEND_MS.load(Ordering::SeqCst)).min(2000);
            if spent < gap {
                std::thread::sleep(Duration::from_millis(gap - spent));
            }
            STREAM_BUSY_SINCE.store(0, Ordering::SeqCst);
            if unix_ms() < STREAM_UNTIL_MS.load(Ordering::SeqCst) {
                let _ = slint::invoke_from_event_loop(stream_shot);
            }
        });
    });
}

/// The link to Home Assistant is up (for the Wi-Fi link watch, which runs in its own thread).
static HA_LINKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The local hour now (0..23).
fn local_hour() -> u32 {
    pages::local_format(pages::now(), "%H").parse().unwrap_or(12)
}

fn media_player_entity(s: &UiState) -> String {
    s.panel_config.as_ref().and_then(|c| c.get("media_player")).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The panel's player as Music Assistant knows it (the integration adds it there for music alarms);
/// the panel's own player when Music Assistant has none yet.
fn ma_player_entity(s: &UiState) -> String {
    let ma = s.panel_config.as_ref().and_then(|c| c.get("ma_player")).and_then(Value::as_str).unwrap_or("").to_string();
    if ma.is_empty() { media_player_entity(s) } else { ma }
}

/// Starts or stops the looping sound: a sounding critical alert first, else a ringing timer or alarm.
/// An alarm with music asks Music Assistant to play it on the panel (quiet at first, louder over
/// 30 s); if no audio comes within 12 s, the alarm's default sound rings instead.
fn loop_sound(s: &mut UiState, want: Option<String>) {
    // the music of an alarm that rang: its default sound when it did not start
    if let Some(asked) = s.music_asked {
        if s.looping == want && asked.elapsed() > Duration::from_secs(12) && !media::flowing() {
            s.music_asked = None;
            eprintln!("nspanel-app: alarm music did not start, its default sound rings");
            let default = want.as_deref().and_then(|k| k.rsplit_once('|')).map(|(_, d)| d.to_string()).unwrap_or_else(|| "birds".into());
            let samples = sounds::resolve("auto", &default).unwrap_or_else(|| Arc::new(voice::beep()));
            s.player.play(voice::Sound::Loop(samples));
            return;
        }
    }
    if s.looping == want {
        return;
    }
    // the music of the alarm before ends with it
    if s.music_asked.take().is_some() || s.looping.as_deref().is_some_and(|k| k.contains(":ma:")) {
        media::halt(media::IDLE);
        let entity = ma_player_entity(s);
        if !entity.is_empty() {
            call("media_player", "media_stop", &entity, json!({}));
        }
    }
    s.player.stop();
    if let Some(key) = &want {
        let tone = key.split_once(':').map(|(_, t)| t).unwrap_or(key);
        let (value, default) = tone.rsplit_once('|').unwrap_or((tone, "marimba"));
        let entity = ma_player_entity(s);
        match value.strip_prefix("ma:") {
            Some(item) if !entity.is_empty() => {
                eprintln!("nspanel-app: alarm music {item} on {entity}");
                media::prepare_ramp(0.15, 30.0);
                call("music_assistant", "play_media", &entity, json!({ "media_id": item, "enqueue": "replace" }));
                s.music_asked = Some(Instant::now());
            }
            _ => {
                let samples = sounds::resolve(value, default).unwrap_or_else(|| Arc::new(voice::beep()));
                s.player.play(voice::Sound::Loop(samples));
            }
        }
    }
    s.looping = want;
}

fn ring_timer(ui: &AppWindow, s: &mut UiState, name: String, tint: slint::Color) {
    // the ringing screen in the timer's own colour (8.8)
    ui.set_timers_ring_tint(tint);
    s.ring_tone = timer_tone(ui, s);
    s.ringing = Some(Instant::now());
    s.ringing_flag.store(true, Ordering::SeqCst);
    ui.set_timers_ring_alarm(false);
    s.ringing_alarm = None;
    ui.set_timers_ring_label(name.into());
    ui.set_timers_ringing(true);
}

/// A timer started on the panel (a quick start or one of one's own): Home Assistant's timer of the panel (voice
/// and every other device see it), or the panel's own while it has no Home Assistant.
fn start_timer(ui: &AppWindow, secs: u32, name: String) {
    if HA_LINKED.load(Ordering::SeqCst) && with_state(|s| s.panel_config.is_some()) {
        send(Command::Timer { action: "start".into(), timer_id: String::new(), seconds: secs, name });
        ui.set_timers_quick(false);
        with_timers(ui, |d| d.mode = 1);
        return;
    }
    start_local_timer(ui, secs, name);
}

/// A timer the panel runs itself (no Home Assistant, or it refused the timer).
fn start_local_timer(ui: &AppWindow, secs: u32, name: String) {
    with_state(|s| {
        s.timer_seq += 1;
        let seq = s.timer_seq;
        s.timers.insert(
            format!("panel-{seq}"),
            pages::Timer {
                name,
                active: true,
                end: Instant::now() + Duration::from_secs(secs as u64),
                left: secs,
                total: secs,
                seq,
                own: true,
            },
        );
    });
    ui.set_timers_quick(false);
    with_timers(ui, |d| d.mode = 1);
}

/// The timers page's functions (alarm, timer, stopwatch) with one of them switched over; None when it would leave none
/// (at least one stays).
fn toggle_function(on: [bool; 3], which: &str) -> Option<[bool; 3]> {
    let i = match which {
        "alarm" => 0,
        "timer" => 1,
        "stopwatch" => 2,
        _ => return None,
    };
    let mut next = on;
    next[i] = !next[i];
    next.iter().any(|v| *v).then_some(next)
}

/// One of the timers page's options set on the panel (the page on screen): kept in the panel's copy of its settings at
/// once (the page shows it before Home Assistant sends the settings back) and sent to the integration.
fn timers_option(ui: &AppWindow, key: &str, value: Value) {
    page_set(ui, "timers", None, json!({ key: value }));
}

/// A page's option in its settings object (made when it has none).
fn set_option(page: &mut Value, key: &str, value: Value) {
    if !page.get("options").is_some_and(Value::is_object) {
        page["options"] = json!({});
    }
    page["options"][key] = value;
}

/// Taps on the timers page (`timers.*`, `watch.*`).
fn timers_act(ui: &AppWindow, what: &str, value: &str) {
    let n: i64 = value.parse().unwrap_or(0);
    // The timer at the page's index `n`.
    let id_at = |s: &UiState| pages::ordered_timers(&s.timers).get(n as usize).map(|(id, _)| (*id).clone());
    match what {
        "timers.mode" => with_timers(ui, |d| d.mode = (n as i32).clamp(0, 2)),
        "timers.pick" => ui.set_timers_focus(n as i32),
        "timers.quick" => ui.set_timers_quick(true),
        // one timer's own sheet (its controls), from a tap on it where several run
        "timers.sheet" => ui.set_timers_sheet(n as i32),
        "timers.sheet-close" => ui.set_timers_sheet(-1),
        "timers.quick-close" => ui.set_timers_quick(false),
        "timers.new" => {
            // a timer of one's own: the editor with hours and minutes (10 min to start from)
            let tint = with_state(|s| accent(s));
            ui.set_timers_edit_timer(true);
            ui.set_timers_edit_new(true);
            ui.set_timers_edit_h(0);
            ui.set_timers_edit_m(10);
            ui.set_timers_edit_label(Default::default());
            ui.set_timers_edit_tint(tint);
            ui.set_timers_edit_keyboard(false);
            ui.set_timers_edit_info(false);
            ui.set_timers_edit(true);
        }
        "timers.create" => {
            let secs = (ui.get_timers_edit_h() as i64 * 3600 + ui.get_timers_edit_m() as i64 * 60).clamp(0, 24 * 3600) as u32;
            if secs == 0 {
                return;
            }
            let name = ui.get_timers_edit_label().trim().to_string();
            ui.set_timers_edit(false);
            ui.set_timers_edit_timer(false);
            start_timer(ui, secs, name);
        }
        "timers.start" => start_timer(ui, n.clamp(1, 24 * 3600) as u32, String::new()),
        // the page's settings in the look's menu (the quick shade's page row), and what they set: shown at once, kept by
        // the integration
        "timers.settings" => ui.set_timers_settings(value == "open"),
        "timers.look" => page_set(ui, "timers", Some(n.clamp(0, 4) as i32), Value::Null),
        "timers.colourful" => {
            with_timers(ui, |d| d.colourful = value == "1");
            timers_option(ui, "colourful", json!(value == "1"));
            with_state(|s| update_timer_lists(ui, s));
        }
        "timers.scene" => {
            with_timers(ui, |d| d.sw_choice = n.clamp(-1, 3) as i32);
            timers_option(ui, "scene", json!(n.clamp(-1, 3)));
        }
        "timers.function" => {
            let d = pages::inst(&ui.get_timers_inst(), acting_n("timers"));
            let on = [d.alarm_on, d.timer_on, d.watch_on];
            let Some(next) = toggle_function(on, value) else { return };
            if !next[d.mode.clamp(0, 2) as usize] {
                // the tab on screen goes: so does its settings menu
                ui.set_timers_settings(false);
            }
            with_timers(ui, |d| {
                d.alarm_on = next[0];
                d.timer_on = next[1];
                d.watch_on = next[2];
                d.mode = timers_tab(d.mode, next);
            });
            let names: Vec<&str> = [(next[1], "timer"), (next[0], "alarm"), (next[2], "stopwatch")].iter().filter(|(on, _)| *on).map(|(_, k)| *k).collect();
            timers_option(ui, "functions", json!(names));
        }
        "timers.toggle" | "timers.add" | "timers.cancel" => with_state(|s| {
            let Some(id) = id_at(s) else { return };
            match what {
                "timers.cancel" => {
                    TIMER_SHEET_CLOSE.with(|c| c.set(true));
                    if let Some(t) = s.timers.remove(&id) {
                        if !t.own {
                            // Home Assistant's timer: cancelled there (voice and other devices see it gone too)
                            let _ = s.commands.as_ref().map(|tx| {
                                tx.send(Command::Timer { action: "cancel".into(), timer_id: id.clone(), seconds: 0, name: String::new() })
                            });
                        }
                    }
                }
                _ => {
                    let Some(t) = s.timers.get_mut(&id) else { return };
                    // Home Assistant's timer: asked of Home Assistant (its answer comes as an updated event); shown
                    // at once
                    if !t.own {
                        let action = if what == "timers.add" { "add" } else if t.active { "pause" } else { "resume" };
                        let _ = s.commands.as_ref().map(|tx| {
                            tx.send(Command::Timer { action: action.into(), timer_id: id.clone(), seconds: if action == "add" { 60 } else { 0 }, name: String::new() })
                        });
                    }
                    if what == "timers.add" {
                        if t.active {
                            t.end += Duration::from_secs(60);
                        } else {
                            t.left += 60;
                        }
                        t.total += 60;
                    } else if t.active {
                        t.left = t.remaining();
                        t.active = false;
                    } else {
                        t.end = Instant::now() + Duration::from_secs(t.left as u64);
                        t.active = true;
                    }
                }
            }
        }),
        "watch.toggle" => {
            if ui.get_timers_watch_running() {
                ui.invoke_watch_stop();
            } else {
                ui.invoke_watch_start();
            }
        }
        "watch.lap" => {
            let now = ui.get_timers_watch();
            let laps = with_state(|s| {
                s.laps.insert(0, now - s.lap_at);
                s.lap_at = now;
                s.laps.clone()
            });
            // the looks' numbers: the time at the last lap, the best lap, the laps' times on the watch (oldest first, 8)
            let mut at = 0.0;
            let mut marks: Vec<f32> = laps.iter().rev().map(|l| { at += l; at }).collect();
            if marks.len() > 8 {
                marks.drain(..marks.len() - 8);
            }
            ui.set_timers_lap_at(now);
            ui.set_timers_lap_best(laps.iter().copied().fold(f32::MAX, f32::min));
            ui.set_timers_lap_marks(ModelRc::new(VecModel::from(marks)));
            ui.set_timers_laps(ModelRc::new(VecModel::from(laps)));
        }
        // the stopwatch tab's look, from its settings menu (a page option the integration keeps)
        "watch.look" => {
            let look = (n as i32).clamp(0, 4);
            with_timers(ui, |d| d.sw_look = look);
            page_set(ui, "timers", None, json!({ "sw_look": look }));
        }
        "watch.reset" => {
            ui.invoke_watch_stop();
            ui.set_timers_watch_base(0.0);
            with_state(|s| {
                s.laps.clear();
                s.lap_at = 0.0;
            });
            ui.set_timers_laps(ModelRc::new(VecModel::from(Vec::<f32>::new())));
            ui.set_timers_lap_at(0.0);
            ui.set_timers_lap_best(-1.0);
            ui.set_timers_lap_marks(ModelRc::new(VecModel::from(Vec::<f32>::new())));
        }
        _ => eprintln!("nspanel-app: act {what} {value} (not handled yet)"),
    }
    if TIMER_SHEET_CLOSE.with(|c| c.replace(false)) {
        ui.set_timers_sheet(-1);
    }
    update_clock(ui);
}

/// An alarm rings: its screen and its sound (looping until stopped, at most a minute).
fn ring_alarm(ui: &AppWindow, s: &mut UiState, alarm: alarms::AlarmKey, time: String, label: String, tone: String, hour: f32) {
    s.ring_tone = format!("{tone}|birds");
    s.ringing_alarm = Some(alarm);
    ui.set_timers_ring_tint(accent(s));
    s.ringing = Some(Instant::now());
    s.ringing_flag.store(true, Ordering::SeqCst);
    ui.set_timers_ring_alarm(true);
    ui.set_timers_ring_time(time.into());
    // the alarm looks draw the ringing time themselves (the radio's LED, the dial's hand)
    ui.set_timers_ring_h(hour);
    ui.set_timers_ring_label(label.into());
    ui.set_timers_ringing(true);
    RANG.with(|r| r.set(true));
}

/// The ringing screen's second button: an alarm sleeps 9 minutes and rings again, a timer gets one more minute.
fn ring_more(ui: &AppWindow) {
    let alarm = ui.get_timers_ring_alarm();
    let label = ui.get_timers_ring_label().to_string();
    let tone = with_state(|s| s.ring_tone.rsplit_once('|').map(|(v, _)| v.to_string()).unwrap_or_default());
    stop_ringing(ui, "more time");
    if alarm {
        with_state(|s| {
            if let Some(key) = s.ringing_alarm.take() {
                s.snooze = Some(alarms::snooze(&s.settings.alarms, key, label, tone));
            }
            // the next-alarm lines show the snoozed ring
            alarms::refresh(ui, s);
        });
    } else {
        start_timer(ui, 60, label);
    }
}

fn stop_ringing(ui: &AppWindow, why: &str) {
    with_state(|s| {
        if s.ringing.is_some() {
            eprintln!("nspanel-app: ringing stopped ({why})");
        }
        s.ringing = None;
        s.ringing_flag.store(false, Ordering::SeqCst);
        // its sound stops at once (a sounding critical alert keeps its own)
        if s.looping.as_deref().is_some_and(|k| k.starts_with("ring:")) {
            loop_sound(s, None);
        }
    });
    ui.set_timers_ringing(false);
    // the ring's visit only: a view Home Assistant shows keeps its own time
    end_visit(ui, true);
}

/// The voice view's status line for a state (listen, think, reply, more,
/// again, fail), in the variant of the look: comic captions in capitals, the
/// terminal and the oscilloscope in their mono wording.
fn voice_line(ui: &AppWindow, state: &str) -> String {
    let place = match ui.get_voice_look() {
        1 | 3 => format!("voice.{state}.mono"),
        2 => format!("voice.{state}.comic"),
        _ => format!("voice.{state}"),
    };
    let line = texts::fresh(&place);
    let line = if line.is_empty() { texts::fresh(&format!("voice.{state}")) } else { line };
    // the terminal draws its own "// " in front of the status line
    if ui.get_voice_look() == 1 {
        line.trim_start_matches('/').trim_start().to_string()
    } else {
        line
    }
}

thread_local! {
    /// Feeds the voice view's halo, peaks and trace while it shows.
    static METER: slint::Timer = slint::Timer::default();
    /// The follow-up's 5 s, the terminal's typing.
    static FOLLOW: slint::Timer = slint::Timer::default();
    static TYPING: slint::Timer = slint::Timer::default();
    static PEAK: std::cell::Cell<(bool, Option<Instant>)> = const { std::cell::Cell::new((true, None)) };
    static THINK_T0: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// The oscilloscope's trace of the last 40 ms of a sound (16 kHz): 100 columns, each its samples' min and max, in a
/// 400 × 160 box (spec 31, F5).
fn scope_path(q: &std::sync::Mutex<std::collections::VecDeque<i16>>) -> String {
    let q = q.lock().unwrap();
    let n = 640.min(q.len());
    if n < 100 {
        return "M 0 80 L 400 80".into();
    }
    let s: Vec<i16> = q.iter().skip(q.len() - n).copied().collect();
    let mut p = String::with_capacity(2400);
    for c in 0..100 {
        let part = &s[c * n / 100..((c + 1) * n / 100).max(c * n / 100 + 1)];
        let (lo, hi) = part.iter().fold((i16::MAX, i16::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        let y = |v: i16| (80.0 - (v as f32 / 32768.0 * 3.0).clamp(-1.0, 1.0) * 74.0).clamp(1.0, 159.0);
        let x = c as f32 * 4.0 + 2.0;
        p.push_str(&format!("{}{x:.0} {:.1} L {x:.0} {:.1} ", if c == 0 { "M " } else { "L " }, y(hi), y(lo)));
    }
    p
}

/// The thinking trace: a flat amber line with a slow ripple (6 px, 0.5 Hz; spec 31, O2).
fn think_path() -> String {
    let t = THINK_T0.with(|t| t.get()).map_or(0.0, |t0| t0.elapsed().as_secs_f32());
    let mut p = String::from("M 0 80 ");
    for c in 1..=50 {
        let x = c as f32 * 8.0;
        let y = 80.0 + 6.0 * (std::f32::consts::TAU * (0.5 * t - x / 200.0)).sin();
        p.push_str(&format!("L {x:.0} {y:.1} "));
    }
    p
}

/// The voice view's look (integration), its pictures and its state colours.
fn voice_view(ui: &AppWindow, state: i32) {
    let look = with_state(|s| s.panel_config.as_ref().and_then(|c| c.get("voice_look")).and_then(Value::as_i64).unwrap_or(0)) as i32;
    ui.set_voice_look(look);
    if look == 1 && ui.get_voice_scan().size().width == 0 {
        ui.set_voice_scan(quick::picture("voice/scan.png"));
    }
    if look == 2 {
        let dots = match state {
            1 | 5 => "voice/dots-blue-500.png",
            3 => "voice/dots-green-500.png",
            4 => "voice/dots-red-500.png",
            6 => "voice/dots-violet-500.png",
            _ => "voice/dots-500.png",
        };
        ui.set_voice_dots(quick::picture(dots));
    }
}

/// Starts or stops the voice view's meter (30 times a second): the microphone level (smoothed), the voice peaks (a
/// ripple, the TRIG lamp: over 0.38, again below 0.25, at most one in 280 ms) and the oscilloscope's trace with its
/// afterglow (the microphone while listening, the ripple while thinking, the panel's own sound while answering).
fn voice_meter(ui: &AppWindow, on: bool) {
    voice::METER_ON.store(on, Ordering::Relaxed);
    METER.with(|t| {
        if !on {
            t.stop();
            ui.set_voice_level(0.0);
            return;
        }
        if t.running() {
            return;
        }
        let weak = ui.as_weak();
        t.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
            let Some(ui) = weak.upgrade() else { return };
            let state = ui.get_voice_state();
            let listening = state == 1 || state == 5;
            let raw = if listening { f32::from_bits(voice::MIC_LEVEL.load(Ordering::Relaxed)) } else { 0.0 };
            let level = ui.get_voice_level() * 0.6 + raw * 0.4;
            ui.set_voice_level(level);
            if listening {
                PEAK.with(|p| {
                    let (armed, last) = p.get();
                    if armed && level > 0.38 && last.is_none_or(|t| t.elapsed() >= Duration::from_millis(280)) {
                        p.set((false, Some(Instant::now())));
                        ui.set_voice_peaks(ui.get_voice_peaks() + 1);
                    } else if !armed && level < 0.25 {
                        p.set((true, last));
                    }
                });
            }
            if ui.get_voice_look() == 3 {
                let path = match state {
                    2 => think_path(),
                    3 => scope_path(&voice::OUT_TRACE),
                    1 | 5 => scope_path(&voice::MIC_TRACE),
                    _ => "M 0 80 L 400 80".into(),
                };
                // the phosphor's afterglow: the last three traces
                ui.set_voice_wave3(ui.get_voice_wave2());
                ui.set_voice_wave2(ui.get_voice_wave1());
                ui.set_voice_wave1(ui.get_voice_wave());
                ui.set_voice_wave(path.into());
            }
        });
    });
}

/// The voice view closes (its time is up, or a tap): its meter, follow-up drain and typing stop with it, so nothing
/// keeps measuring the microphone for a view nobody sees.
fn close_voice_view(ui: &AppWindow) {
    ui.set_voice_on(false);
    voice_meter(ui, false);
    follow_up_drain(ui, false);
    TYPING.with(|t| t.stop());
}

/// Listening for more: the 5 s drain (`voice-left`, 1 … 0) until speech starts (spec 31, F2).
fn follow_up_drain(ui: &AppWindow, on: bool) {
    FOLLOW.with(|t| {
        if !on {
            t.stop();
            ui.set_voice_left(-1.0);
            return;
        }
        let started = Instant::now();
        ui.set_voice_left(1.0);
        let weak = ui.as_weak();
        t.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
            let Some(ui) = weak.upgrade() else { return };
            let left = 1.0 - started.elapsed().as_secs_f32() / 5.0;
            ui.set_voice_left(left.max(0.0));
            if left <= 0.0 {
                FOLLOW.with(|t| t.stop());
            }
        });
    });
}

/// The terminal look's answer types at 40 characters a second (spec 31, T3).
fn type_answer(ui: &AppWindow, text: &str) {
    // the terminal types characters (40 a second), the plain look brings the words one after another (55 ms apart)
    let look = ui.get_voice_look();
    if look != 0 && look != 1 {
        ui.set_voice_typed(text.into());
        return;
    }
    let chars: Vec<char> = text.chars().collect();
    let word_ends: Vec<usize> = chars.iter().enumerate().filter(|(i, c)| c.is_whitespace() || *i + 1 == chars.len()).map(|(i, _)| i + 1).collect();
    ui.set_voice_typed("".into());
    let started = Instant::now();
    let weak = ui.as_weak();
    TYPING.with(|t| {
        t.start(slint::TimerMode::Repeated, Duration::from_millis(25), move || {
            let Some(ui) = weak.upgrade() else { return };
            let s = started.elapsed().as_secs_f32();
            let n = if look == 1 {
                ((s * 40.0) as usize).min(chars.len())
            } else {
                word_ends.get((s / 0.055) as usize).copied().unwrap_or(chars.len())
            };
            ui.set_voice_typed(chars[..n].iter().collect::<String>().into());
            if n >= chars.len() {
                TYPING.with(|t| t.stop());
            }
        });
    });
}

fn handle_voice_ui(ui: &AppWindow, event: voice::VoiceUi) {
    use voice::VoiceUi;
    let hide_after = |secs: u64| with_state(|s| s.voice_hide_at = Some(Instant::now() + Duration::from_secs(secs)));
    let show = with_state(|s| s.panel_config.as_ref().and_then(|c| c.get("show_conversation")).and_then(Value::as_bool).unwrap_or(true));
    match event {
        VoiceUi::Listening | VoiceUi::FollowUp => {
            let more = matches!(event, VoiceUi::FollowUp);
            wake(ui);
            with_state(|s| s.voice_hide_at = None);
            if !more {
                ui.set_voice_heard("".into());
                ui.set_voice_answer("".into());
                ui.set_voice_typed("".into());
            }
            ui.set_voice_detail("".into());
            voice_view(ui, if more { 5 } else { 1 });
            ui.set_voice_line(voice_line(ui, if more { "more" } else { "listen" }).into());
            ui.set_voice_state(if more { 5 } else { 1 });
            follow_up_drain(ui, more);
            // the meter only for a view that shows
            voice_meter(ui, show);
            ui.set_voice_on(show);
        }
        VoiceUi::Heard(text) => {
            follow_up_drain(ui, false);
            ui.set_voice_heard(text.into());
        }
        VoiceUi::Thinking => {
            follow_up_drain(ui, false);
            THINK_T0.with(|t| t.set(Some(Instant::now())));
            voice_view(ui, 2);
            ui.set_voice_line(voice_line(ui, "think").into());
            ui.set_voice_state(2);
        }
        VoiceUi::Answer(text) => {
            type_answer(ui, &text);
            ui.set_voice_answer(text.into());
            voice_view(ui, 3);
            ui.set_voice_line(voice_line(ui, "reply").into());
            ui.set_voice_state(3);
        }
        VoiceUi::Idle => {
            follow_up_drain(ui, false);
            if ui.get_voice_on() {
                hide_after(5);
            } else {
                voice_meter(ui, false);
            }
        }
        VoiceUi::NotUnderstood => {
            follow_up_drain(ui, false);
            voice_view(ui, 6);
            ui.set_voice_line(voice_line(ui, "again").into());
            ui.set_voice_state(6);
            hide_after(5);
        }
        VoiceUi::Error(message) => {
            follow_up_drain(ui, false);
            ui.set_voice_detail(message.into());
            voice_view(ui, 4);
            ui.set_voice_line(voice_line(ui, "fail").into());
            ui.set_voice_state(4);
            hide_after(5);
        }
        VoiceUi::Stopped => {
            stop_ringing(ui, "voice");
            if ui.get_voice_on() {
                hide_after(1);
            }
        }
    }
    platform::note_activity();
}

// ---------------------------------------------------------------- start-up

const SPLASH_PIDFILE: &str = "/var/run/nspanel-splash.pid";

const SPLASH_DONE: &str = "/var/run/nspanel-splash.done";

/// Longest wait for the clock and the first Home Assistant data before the UI
/// takes the display anyway (Home Assistant or the network is down).
const READY_TIMEOUT: Duration = Duration::from_secs(25);

/// Holds the UI back until it has something current to show: the clock is
/// set and the first full state list from Home Assistant is in (a short
/// grace lets the panel settings and forecasts follow). Pairing needed:
/// at once, the QR code is the content. Meanwhile the boot splash keeps its
/// light band running. Returns the events received so far, for the UI.
fn wait_until_ready(events: &mpsc::Receiver<Event>) -> Vec<Event> {
    let started = Instant::now();
    let mut buffered = Vec::new();
    let mut states_at: Option<Instant> = None;
    let mut connected_at: Option<Instant> = None;
    let mut clock_at: Option<Instant> = None;
    loop {
        if clock_at.is_none() && clock::valid() {
            clock_at = Some(Instant::now());
        }
        let data = states_at.is_some_and(|t| t.elapsed() >= Duration::from_millis(300))
            || connected_at.is_some_and(|t| t.elapsed() >= Duration::from_secs(2));
        if (data && clock::valid()) || started.elapsed() >= READY_TIMEOUT {
            break;
        }
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                let pairing = matches!(event, Event::NeedPairing);
                match &event {
                    Event::States(_) => {
                        states_at.get_or_insert_with(Instant::now);
                    }
                    Event::Status { connected: true, .. } => {
                        connected_at.get_or_insert_with(Instant::now);
                    }
                    _ => {}
                }
                buffered.push(event);
                if pairing {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let at = |t: Option<Instant>| t.map(|t| format!("{} ms", (t - started).as_millis())).unwrap_or_else(|| "-".into());
    eprintln!(
        "nspanel-app: ready for the display after {} ms (clock set at {}, connected at {}, states at {}, {} events)",
        started.elapsed().as_millis(),
        at(clock_at),
        at(connected_at),
        at(states_at),
        buffered.len()
    );
    buffered
}

/// Asks the boot splash (nspanel-splash) to hand the display over and waits
/// until it has (at most 40 s). Its pid.
fn splash_handoff() -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(SPLASH_PIDFILE).ok()?.trim().parse().ok()?;
    // SAFETY: plain kill(2).
    if unsafe { libc::kill(pid, libc::SIGUSR1) } != 0 {
        return None;
    }
    let started = Instant::now();
    while !std::path::Path::new(SPLASH_DONE).exists() && started.elapsed() < Duration::from_secs(40) {
        std::thread::sleep(Duration::from_millis(20));
    }
    eprintln!("nspanel-app: display taken over from the boot splash after {} ms", started.elapsed().as_millis());
    Some(pid)
}

/// Home Assistant URLs to try for what was typed: as given with a scheme or a
/// port, otherwise port 8123 (older and Docker installs) and then port 80
/// (Home Assistant OS installed since 2026.8).
fn setup_candidates(input: &str) -> Vec<String> {
    let s = input.trim().trim_end_matches('/');
    if s.is_empty() {
        Vec::new()
    } else if s.starts_with("http://") || s.starts_with("https://") {
        vec![s.to_string()]
    } else if s.contains(':') {
        vec![format!("http://{s}")]
    } else {
        vec![format!("http://{s}:8123"), format!("http://{s}")]
    }
}

/// True if Home Assistant answers at `url`: `/auth/providers` needs no login
/// and only exists there.
fn is_home_assistant(url: &str) -> bool {
    match ureq::get(&format!("{url}/auth/providers")).timeout(Duration::from_secs(4)).call() {
        Ok(r) => r.into_string().is_ok_and(|body| serde_json::from_str::<Value>(&body).is_ok()),
        Err(_) => false,
    }
}

/// The mDNS browser (Home Assistant instances on the network, for the setup screen).
static MDNS_BROWSER: std::sync::OnceLock<Arc<mdns::Browser>> = std::sync::OnceLock::new();

/// First start without settings: the setup screen lists Home Assistant found
/// on the network (mDNS) and takes an address on a keypad. Once Home Assistant
/// answers at the address, it is saved to app.json and the app restarts (the
/// supervisor starts it again), which then shows the pairing QR code.
fn start_setup(ui: &AppWindow, browser: Arc<mdns::Browser>, reason: &str) {
    ui.set_setup(true);
    ui.set_setup_variant(wifi_random());
    if !reason.contains("No such file") {
        ui.set_setup_message(format!("The saved settings could not be read ({reason}); set them again.").into());
        ui.set_setup_message_kind(1);
    }
    if let Some(ip) = pairing::local_ip_towards("8.8.8.8:53") {
        ui.set_net_ip(ip.to_string().into());
    }
    setup_handlers(ui, browser);
}

/// A number for the screens' random texts.
fn wifi_random() -> i32 {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(7);
    (n % 1_000_000_000) as i32
}

/// Settings → Home Assistant: the setup screen with the address the panel uses, a back arrow; a new address that
/// answers is saved and the app starts again with it (a different Home Assistant asks for the pairing again).
fn open_ha_setup(ui: &AppWindow) {
    let Some(browser) = MDNS_BROWSER.get().cloned() else { return };
    let current = with_state(|s| s.config.as_ref().map(|c| c.ha_host_port()).unwrap_or_default());
    // the keypad types digits, dots and colons: the address as host:port
    ui.set_setup_address(current.into());
    ui.set_setup_message("".into());
    ui.set_setup_message_kind(0);
    ui.set_setup_variant(wifi_random());
    ui.set_setup_can_back(true);
    ui.set_setup_wifi_info(false);
    // the Wi-Fi the panel is on (its card on the screen)
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let link = shade::wifi_link();
        let ip = pairing::local_ip_towards("8.8.8.8:53").map(|ip| ip.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            if let Some((name, dbm)) = link {
                ui.set_net_name(name.into());
                ui.set_net_bars(dbm.map_or(4, |d| if d > -55.0 { 4 } else if d > -63.0 { 3 } else if d > -72.0 { 2 } else { 1 }));
            }
            ui.set_net_ip(ip.unwrap_or_default().into());
        });
    });
    setup_handlers(ui, browser);
    ui.set_setup(true);
}

/// The setup screen's callbacks and its list of Home Assistants found (set once).
fn setup_handlers(ui: &AppWindow, browser: Arc<mdns::Browser>) {
    browser.active.store(true, Ordering::Relaxed);
    thread_local!(static READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if READY.with(|r| r.replace(true)) {
        return;
    }
    let back_browser = browser.clone();
    let weak = ui.as_weak();
    ui.on_setup_back(move || {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_setup_busy() {
            return;
        }
        back_browser.active.store(false, Ordering::Relaxed);
        ui.set_setup_pick_open(false);
        ui.set_setup(false);
        ui.set_setup_can_back(false);
    });
    // Found instances, refreshed every second; the timer lives as long as the app.
    let found_timer = slint::Timer::default();
    let weak = ui.as_weak();
    found_timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
        let Some(ui) = weak.upgrade() else { return };
        let found: Vec<FoundHa> = browser
            .found
            .lock()
            .unwrap()
            .iter()
            .map(|h| FoundHa { name: h.name.clone().into(), url: h.url.clone().into() })
            .collect();
        if ui.get_setup_found().row_count() != found.len() {
            ui.set_setup_found(ModelRc::new(VecModel::from(found)));
        }
    });
    std::mem::forget(found_timer);

    let weak = ui.as_weak();
    ui.on_setup_key(move |k| {
        let Some(ui) = weak.upgrade() else { return };
        let mut text = ui.get_setup_address().to_string();
        if k == "back" {
            text.pop();
        } else if text.len() < 40 {
            text.push_str(&k);
        }
        ui.set_setup_address(text.into());
    });
    let weak = ui.as_weak();
    ui.on_setup_pick(move |url| {
        let Some(ui) = weak.upgrade() else { return };
        ui.set_setup_pick_open(false);
        ui.set_setup_address(url);
        ui.invoke_setup_connect();
    });
    let weak = ui.as_weak();
    ui.on_setup_choose_many(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_setup_pick_open(true);
        }
    });
    let weak = ui.as_weak();
    ui.on_setup_pick_back(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_setup_pick_open(false);
        }
    });
    let weak = ui.as_weak();
    ui.on_setup_connect(move || {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_setup_busy() {
            return;
        }
        let candidates = setup_candidates(&ui.get_setup_address());
        if candidates.is_empty() {
            ui.set_setup_message("Type the address first.".into());
            ui.set_setup_message_kind(1);
            return;
        }
        ui.set_setup_busy(true);
        ui.set_setup_message("Looking for Home Assistant…".into());
        ui.set_setup_message_kind(0);
        // opened from the settings: the address the panel uses now
        let now_url = with_state(|s| s.config.as_ref().map(|c| c.ha_url.trim_end_matches('/').to_string()));
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            let found = candidates.iter().find(|u| is_home_assistant(u)).cloned();
            let same = found.is_some() && found.as_deref() == now_url.as_deref();
            let saved = found.as_ref().filter(|_| !same).map(|url| {
                let _ = std::fs::create_dir_all(PERSIST_DIR);
                // the other settings in app.json stay (only the address changes); written whole, then renamed
                let path = format!("{PERSIST_DIR}/app.json");
                let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
                v["ha_url"] = json!(url);
                let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                let tmp = format!("{path}.tmp");
                std::fs::write(&tmp, text + "\n").and_then(|_| std::fs::rename(&tmp, &path))
            });
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                ui.set_setup_busy(false);
                if same {
                    ui.set_setup_message("The panel uses this Home Assistant already.".into());
                    ui.set_setup_message_kind(2);
                    let weak = ui.as_weak();
                    slint::Timer::single_shot(Duration::from_millis(1500), move || {
                        if let Some(ui) = weak.upgrade() {
                            ui.invoke_setup_back();
                        }
                    });
                    return;
                }
                match (found, saved) {
                    (Some(url), Some(Ok(()))) => {
                        eprintln!("nspanel-app: setup: Home Assistant at {url} saved, restarting");
                        ui.set_setup_message(format!("Home Assistant found at {url}. Starting…").into());
                        ui.set_setup_message_kind(2);
                        // The supervisor starts the app again, now with settings.
                        slint::Timer::single_shot(Duration::from_millis(800), || std::process::exit(0));
                    }
                    (Some(_), Some(Err(e))) => {
                        ui.set_setup_message(format!("Could not save the settings: {e}").into());
                        ui.set_setup_message_kind(1);
                    }
                    _ => {
                        ui.set_setup_message(
                            "Home Assistant did not answer at this address. Check the IP address and the port (:8123 or none).".into(),
                        );
                        ui.set_setup_message_kind(1);
                    }
                }
            });
        });
    });
}

fn wake_phrase(ids: &[String]) -> String {
    let Some(id) = ids.first() else { return String::new() };
    voice::available_wake_words().into_iter().find(|w| w.id == *id).map(|w| w.phrase).unwrap_or_default()
}

/// show_view and the other requests of the PanelOS integration (alarms, status screens, critical alerts, notifications).
fn handle_show(ui: &AppWindow, v: &Value) {
    if v["type"].as_str() == Some("touch_test") {
        start_touch_test(ui);
        return;
    }
    if v["type"].as_str() == Some("proximity_calibration") {
        device_options::start_calibration(ui);
        return;
    }
    // the alarms as edited in the integration (the panel keeps and rings them)
    if v["type"].as_str() == Some("alarms") {
        eprintln!("nspanel-app: alarms from Home Assistant ({})", v["list"].as_array().map_or(0, Vec::len));
        alarms::set_all(ui, &v["list"], &v["cancelled"]);
        return;
    }
    // PanelOS's own notifications (the integration's list for this panel): new ones pop up
    if v["type"].as_str() == Some("notes") {
        let fresh = info::update_own(ui, v);
        with_state(|s| info::refresh(ui, s));
        for id in fresh {
            let (title, message) = info::note_text(&id);
            popup(ui, &id, &title, &message);
        }
        return;
    }
    // status screens and critical alerts (the integration's house-wide rules)
    if matches!(v["type"].as_str(), Some("status" | "status_clear" | "critical" | "critical_clear")) {
        eprintln!("nspanel-app: {} {}", v["type"].as_str().unwrap_or(""), v["id"].as_str().unwrap_or(""));
        alerts::show(ui, v);
        return;
    }
    let seconds = v["seconds"].as_u64().unwrap_or(30);
    eprintln!("nspanel-app: show request {v}");
    wake(ui);
    ui.set_set_view(0);
    // with seconds: shown until then, and back to where the panel was; without: it stays
    let until = (seconds > 0).then(|| Instant::now() + Duration::from_secs(seconds));
    let chime = with_state(|s| v["chime"].as_bool().unwrap_or(false).then(|| s.player.clone()));
    match v["view"].as_str().unwrap_or("home") {
        "dashboard" => {
            if ui.get_web_active() {
                set_dashboard(ui, true);
                with_state(|s| s.show_until = until);
            }
        }
        "media" => match until {
            Some(until) => visit_page(ui, "music", Hold::Show(until)),
            None => {
                end_show_visit();
                go_to_page(ui, "music");
            }
        },
        other => {
            set_dashboard(ui, with_state(|s| s.settings.display_mode == "dashboard") && ui.get_web_active());
            let kind = if ui.get_pages().iter().any(|p| p.kind == other) { other } else { "home" };
            match until {
                Some(until) => visit_page(ui, kind, Hold::Show(until)),
                None => {
                    end_show_visit();
                    go_to_page(ui, kind);
                }
            }
        }
    }
    if let Some(player) = chime {
        player.play(voice::Sound::Chime);
    }
}

fn send(cmd: Command) {
    let tx = with_state(|s| s.commands.clone());
    match tx {
        Some(tx) => {
            let _ = tx.send(cmd);
        }
        None => {
            if let Command::CallService { domain, service, entity_id, data } = cmd {
                eprintln!("nspanel-app: (no Home Assistant) {domain}.{service} {entity_id} {data}");
            }
        }
    }
}

/// A page's look (`variant`) or options set on the panel, on the page of `kind` the panel acts on (the one on screen,
/// where the menu that set it is): shown at once, kept by the integration (which sends the panel its new settings back).
/// Not to be called inside `with_state`.
pub fn page_set(ui: &AppWindow, kind: &str, variant: Option<i32>, options: Value) {
    let Some(cfg) = acting(kind).and_then(|p| (p.cfg >= 0).then_some(p.cfg as usize)) else { return };
    page_set_at(ui, cfg, variant, options);
}

/// The same for the page at `cfg` in the integration's list: the pager's slot takes the look at once, the panel's copy
/// of the settings the look and the options (so every view made from them shows the change before the integration's
/// answer), and the integration keeps them. Until the integration's settings carry it, the change holds over older
/// copies that come back meanwhile (`keep_page_sets`); development settings over Home Assistant's take it too.
pub fn page_set_at(ui: &AppWindow, cfg: usize, variant: Option<i32>, options: Value) {
    let pages = ui.get_pages();
    if let Some(v) = variant {
        if let Some(index) = (0..pages.row_count()).find(|&i| pages.row_data(i).is_some_and(|p| p.cfg == cfg as i32)) {
            if let Some(mut p) = pages.row_data(index) {
                p.variant = v;
                pages.set_row_data(index, p);
            }
        }
    }
    with_state(|s| {
        if let Some(page) = config_page_mut(s.panel_config.as_mut(), cfg) {
            apply_page_set(page, variant, &options);
            let kind = page.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
            PAGE_SETS.with(|p| p.borrow_mut().push(HeldPageSet { cfg, kind, variant, options: options.clone(), at: Instant::now() }));
        }
        if let Some(page) = config_page_mut(s.dev_config.as_mut(), cfg) {
            apply_page_set(page, variant, &options);
        }
    });
    send(Command::PageSet { index: cfg, variant, options });
}

/// The page at `cfg` in a copy of the integration's settings.
fn config_page_mut(config: Option<&mut Value>, cfg: usize) -> Option<&mut Value> {
    config?.get_mut("pages")?.as_array_mut()?.get_mut(cfg)
}

/// How long a page setting made on the panel holds over Home Assistant's settings that do not carry it yet.
const PAGE_SET_HOLD: Duration = Duration::from_secs(10);

/// A page setting made on the panel, sent to Home Assistant and not seen back yet.
struct HeldPageSet {
    /// The page's place in the integration's list, and its kind (another page there now does not take it).
    cfg: usize,
    kind: String,
    variant: Option<i32>,
    options: Value,
    at: Instant,
}

thread_local! {
    static PAGE_SETS: RefCell<Vec<HeldPageSet>> = const { RefCell::new(Vec::new()) };
    /// The integration's settings as the panel last kept them (`panel-config.json`).
    static SAVED_PANEL_CONFIG: RefCell<Option<Value>> = const { RefCell::new(None) };
}

/// Settings from Home Assistant: page settings made on the panel a moment ago and not in them yet stay (a copy sent
/// before the integration had them, or one a new connection got before the old one's request arrived, must not take
/// the page back to what it was); once a setting is in, or after `PAGE_SET_HOLD`, it is not held any more.
fn keep_page_sets(config: &mut Option<Value>) {
    PAGE_SETS.with(|held| {
        let mut held = held.borrow_mut();
        held.retain(|h| h.at.elapsed() < PAGE_SET_HOLD);
        let Some(pages) = config.as_mut().and_then(|c| c.get_mut("pages")).and_then(Value::as_array_mut) else { return };
        held.retain(|h| {
            let Some(page) = pages.get_mut(h.cfg).filter(|p| p.get("kind").and_then(Value::as_str) == Some(h.kind.as_str())) else {
                // the list changed meanwhile: that page is not there any more
                return false;
            };
            if page_carries(page, h.variant, &h.options) {
                return false;
            }
            eprintln!("nspanel-app: page {} ({}): the setting made on the panel kept over older settings from Home Assistant", h.cfg, h.kind);
            apply_page_set(page, h.variant, &h.options);
            true
        });
    });
}

/// Whether a page's settings already have a `page_set` (the look, the entities, the options; null: the option gone).
fn page_carries(page: &Value, variant: Option<i32>, options: &Value) -> bool {
    // numbers by value (3 and 3.0 are the same setting)
    let same = |a: &Value, b: &Value| match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    };
    if variant.is_some_and(|v| pages::variant_of(page) != v) {
        return false;
    }
    options.as_object().map_or(true, |o| {
        o.iter().all(|(key, value)| {
            if key == "entities" {
                page.get("entities").is_some_and(|e| same(e, value))
            } else if value.is_null() {
                pages::option_of(page, key).is_none()
            } else {
                pages::option_of(page, key).is_some_and(|v| same(v, value))
            }
        })
    })
}

/// A page's settings with a `page_set` applied: the look, and the options (null removes one).
fn apply_page_set(page: &mut Value, variant: Option<i32>, options: &Value) {
    if let Some(v) = variant {
        page["variant"] = json!(v);
    }
    if let Some(o) = options.as_object() {
        for (key, value) in o {
            if key == "entities" {
                // the page's entities (the music page's player picked on the panel)
                page["entities"] = value.clone();
            } else if value.is_null() {
                if let Some(opts) = page.get_mut("options").and_then(Value::as_object_mut) {
                    opts.remove(key);
                }
            } else {
                set_option(page, key, value.clone());
            }
        }
    }
}

thread_local! {
    /// When the web helper last said a page started loading.
    static WEB_LOADING_AT: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    /// A pull to refresh asked for a reload: the loading screen goes up first.
    static WEB_COVER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The loading screen stays at least until then (the web view's first frames after the dashboard comes on are
    /// empty or half drawn even when the page reports ready).
    static WEB_MIN_UNTIL: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// The loading screen goes up at once (before the web view shows anything: its first frames of a reload are white),
/// for a reload or when the dashboard comes on screen. If no page starts loading within 1.8 s (the dashboard was
/// already loaded), it fades again.
fn cover_web(ui: &AppWindow) {
    // (also called from inside `with_state`: the setting is read only when the state is free, else it counts as on)
    let screen_on = STATE.with(|st| st.try_borrow().ok().and_then(|g| g.as_ref().map(|s| s.settings.web.loading_screen))).unwrap_or(true);
    if !screen_on || ui.get_web_loading() {
        return;
    }
    let asked = Instant::now();
    WEB_MIN_UNTIL.with(|m| m.set(Some(asked + Duration::from_millis(1800))));
    ui.set_web_loading(true);
    loading_text::start(ui);
    ui.set_web_overlay(true);
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(1800), move || {
        let Some(ui) = weak.upgrade() else { return };
        let started = WEB_LOADING_AT.with(|t| t.get()).is_some_and(|t| t >= asked);
        if !started && ui.get_web_loading() {
            // (the page was loaded already: it fades now, its minimum time is over)
            WEB_MIN_UNTIL.with(|m| m.set(None));
            ui.set_web_loading(false);
            loading_text::stop();
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(500), move || {
                if let Some(ui) = weak.upgrade() {
                    if !ui.get_web_loading() {
                        ui.set_web_overlay(false);
                    }
                }
            });
        }
    });
}

/// The dashboard on screen or not; coming on screen, the loading screen covers it first (see `cover_web`).
fn set_dashboard(ui: &AppWindow, on: bool) {
    if on != ui.get_dashboard_mode() {
        eprintln!("nspanel-app: display: {}", if on { "dashboard" } else { "pages" });
    }
    if on && !ui.get_dashboard_mode() && ui.get_web_active() {
        cover_web(ui);
    }
    ui.set_dashboard_mode(on);
}

/// A websocket command of Home Assistant that is not a service call (`todo/item/move` …).
pub fn send_raw(msg: Value) {
    send(Command::Raw(msg));
}

fn call(domain: &str, service: &str, entity_id: &str, data: Value) {
    send(Command::CallService { domain: domain.into(), service: service.into(), entity_id: entity_id.into(), data });
}

fn start_pairing(ui: &AppWindow) {
    let (config, auth) = with_state(|s| (s.config.clone(), s.auth.clone()));
    let (Some(config), Some(auth)) = (config, auth) else { return };
    let weak = ui.as_weak();
    let on_paired = move || {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_pair_state(1);
                ui.set_paired(true);
                ui.set_connecting(true);
            }
        });
    };
    ui.set_pair_ha_url(config.ha_url.clone().into());
    match pairing::start(&config.ha_url, &config.ha_host_port(), config.http_port, auth, on_paired) {
        Ok(p) => {
            if let Some(img) = pairing::qr_image(&p.authorize_url, 254) {
                ui.set_qr(img);
            }
            ui.set_pair_url(p.authorize_url.clone().into());
            ui.set_pair_hint(
                format!("Scan with your phone and log in to Home Assistant ({}). The panel connects by itself.", config.ha_url)
                    .into(),
            );
            eprintln!("nspanel-app: pairing page at {}, login URL {}", p.panel_url, p.authorize_url);
        }
        Err(e) => {
            ui.set_pair_state(2);
            ui.set_pair_hint(format!("Cannot start pairing: {e}").into());
        }
    }
}

fn handle_event(ui: &AppWindow, event: Event) {
    // Home Assistant asking something of the panel stirs the clocks (the live preview's screenshots and the list of
    // notifications sent at every connection do not)
    let asked = match &event {
        Event::Panel(_) | Event::Show(_) => true,
        Event::Command { key, .. } => key != "screenshot",
        Event::Notes { kind, .. } => kind != "current",
        _ => false,
    };
    if asked {
        stir(ui);
    }
    match event {
        Event::Status { text, connected } => {
            HA_LINKED.store(connected, Ordering::SeqCst);
            ui.set_connected(connected);
            with_state(|s| {
                if connected && !s.connected {
                    s.linked_at = Some(Instant::now());
                }
                s.connected = connected;
                if connected {
                    s.ever_connected = true;
                    s.offline_since = None;
                } else if s.offline_since.is_none() {
                    s.offline_since = Some(Instant::now());
                }
                s.offline_text = text;
            });
            update_connection_screens(ui);
            refresh_pages(ui);
        }
        Event::NeedPairing => {
            ui.set_paired(false);
            ui.set_connected(false);
            ui.set_connecting(false);
            let start = with_state(|s| {
                let first = !s.pairing_started;
                s.pairing_started = true;
                first
            });
            if start {
                start_pairing(ui);
            }
        }
        Event::Dashboard(_) => {
            ui.set_paired(true);
            let need_list = with_state(|s| s.dashboards.is_empty());
            if need_list {
                // Gives the "Dashboard" select of the panel's device its options.
                send(Command::ListDashboards);
            }
        }
        Event::States(changes) => {
            #[cfg(feature = "gpu")]
            if gpu::FRAME_STATS.load(Ordering::Relaxed) {
                let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 100_000).unwrap_or(0);
                let ids: Vec<&str> = changes.iter().map(|(id, _)| id.as_str()).take(6).collect();
                eprintln!("nspanel-app: states at {t} ms: {} {:?}", changes.len(), ids);
            }
            let watched = with_state(|s| {
                let mut new_person = false;
                let holding = s.linked_at.is_some_and(|t| t.elapsed() < Duration::from_secs(120));
                for (id, st) in &changes {
                    let bad = st.as_ref().map_or(true, |e| e.state == "unavailable" || e.state == "unknown");
                    let had_good = s.states.get(id).is_some_and(|e| e.state != "unavailable" && e.state != "unknown");
                    if holding && bad && had_good {
                        s.held_states.insert(id.clone(), st.clone());
                        continue;
                    }
                    s.held_states.remove(id);
                    match st {
                        Some(st) => {
                            if id.starts_with("person.") && !s.states.contains_key(id) {
                                new_person = true;
                            }
                            s.states.insert(id.clone(), st.clone());
                        }
                        None => {
                            s.states.remove(id);
                        }
                    }
                }
                // any entity a page shows or follows (weather, player, people, tiles, thermostats, cameras,
                // the doorbell): the pages at once, not at the next minute's refresh (a short ring
                // would be missed in between)
                let used = extra_entities(s);
                let touches = changes.iter().any(|(id, _)| id.starts_with("person.") || used.iter().any(|u| u == id));
                (touches, new_person)
            });
            let (touches, new_person) = watched;
            if touches {
                refresh_pages(ui);
            }
            if new_person {
                apply_panel_config(ui);
            }
        }
        Event::Dashboards(list) => {
            with_state(|s| {
                s.device.lock().unwrap().dashboards = list.clone();
                let _ = s.device_notify.send(());
                s.dashboards = list;
            });
        }
        Event::Panel(config) => {
            with_state(|s| {
                // Home Assistant starting (the integration not loaded yet) or a moment without the entry: the pages
                // and settings stay exactly as they were
                if config.is_none() && s.panel_config.is_some() {
                    eprintln!("nspanel-app: no PanelOS settings from Home Assistant now; keeping the last ones");
                    return;
                }
                let web = config.as_ref().map(settings::WebConfig::from_integration).unwrap_or_default();
                if web != s.settings.web {
                    eprintln!("nspanel-app: dashboard mode settings from Home Assistant: {web:?}");
                    s.settings.web = web;
                    s.settings.save();
                }
                let reporting = config.as_ref().map(settings::Reporting::from_integration).unwrap_or_default();
                if reporting != s.settings.reporting {
                    eprintln!("nspanel-app: sensor reporting from Home Assistant: {reporting:?}");
                    s.settings.reporting = reporting;
                    s.settings.save();
                }
                if let Some(c) = &config {
                    // (compared with the copy kept last: the panel's own copy may already have a change made on it)
                    if SAVED_PANEL_CONFIG.with(|k| k.borrow().as_ref() != Some(c)) {
                        settings::save_panel_config(c);
                        SAVED_PANEL_CONFIG.with(|k| *k.borrow_mut() = Some(c.clone()));
                    }
                }
                s.panel_config = config;
                // widgets and page settings changed on the panel a moment ago stay until Home Assistant has them
                home_edit::keep_pending(&mut s.panel_config);
                keep_page_sets(&mut s.panel_config);
                // Development settings stay on top of what Home Assistant sends.
                if let Some(dev) = s.dev_config.clone() {
                    overlay(&mut s.panel_config, &dev);
                }
            });
            apply_panel_config(ui);
        }
        Event::Show(v) => handle_show(ui, &v),
        Event::Timers(list) => {
            // Home Assistant's timers of the panel now: these replace the ones the panel had from it (its own stay)
            with_state(|s| {
                let ids: Vec<String> = list.iter().filter_map(|t| t["id"].as_str().map(str::to_string)).collect();
                s.timers.retain(|id, t| t.own || ids.contains(id));
                for t in &list {
                    let Some(id) = t["id"].as_str() else { continue };
                    let left = t["left"].as_u64().unwrap_or(0) as u32;
                    let total = (t["total"].as_u64().unwrap_or(left as u64) as u32).max(left);
                    let active = t["active"].as_bool().unwrap_or(true);
                    let seq = match s.timers.get(id) {
                        Some(x) => x.seq,
                        None => {
                            s.timer_seq += 1;
                            s.timer_seq
                        }
                    };
                    let name = t["name"].as_str().unwrap_or("").to_string();
                    s.timers.insert(
                        id.to_string(),
                        pages::Timer { name, active, end: Instant::now() + Duration::from_secs(left as u64), left, total, seq, own: false },
                    );
                }
            });
            // the panel's own timers that ran while Home Assistant was away go to it now (voice and the other
            // devices see them from here on); one it refuses runs on here (TimerRefused)
            let handed: Vec<(u32, String)> = with_state(|s| {
                let now = Instant::now();
                let ids: Vec<String> = s.timers.iter().filter(|(_, t)| t.own && t.active && t.end > now).map(|(id, _)| id.clone()).collect();
                ids.iter()
                    .filter_map(|id| s.timers.remove(id))
                    .map(|t| ((t.end - now).as_secs_f32().round().max(1.0) as u32, t.name))
                    .collect()
            });
            for (seconds, name) in handed {
                eprintln!("nspanel-app: a timer of the panel ({seconds} s left) goes to Home Assistant");
                send(Command::Timer { action: "start".into(), timer_id: String::new(), seconds, name });
            }
            update_clock(ui);
        }
        Event::TimerRefused { seconds, name } => start_local_timer(ui, seconds, name),
        Event::Command { key, .. } if key == "screenshot" => {
            // The live preview: each request keeps a short stream going (a picture about every 120 ms
            // for 3 s), so the page gets the newest picture at once instead of waiting a round trip.
            screen_stream(ui);
        }
        Event::Command { key, value } => {
            let control = with_state(|s| panel_device::command(&key, &value, &s.device.lock().unwrap()));
            match control {
                Some(control) => apply_control(ui, control),
                None => eprintln!("nspanel-app: unknown command from Home Assistant: {key} = {value}"),
            }
        }
        Event::Notes { kind, notes } => notes_changed(ui, &kind, &notes),
        Event::Forecast { hourly, items } => {
            with_state(|s| if hourly { s.forecast_hourly = items } else { s.forecast_daily = items });
            refresh_pages(ui);
        }
    }
}

/// The connecting screen (paired, no data yet) and the offline screen (no
/// network / Home Assistant away for a while after it worked).
fn update_connection_screens(ui: &AppWindow) {
    with_state(|s| {
        if s.dev_screens {
            return;
        }
        let paired = ui.get_paired();
        ui.set_connecting(paired && !s.connected && !s.ever_connected && s.config.is_some());
        let away = !s.connected && s.ever_connected && s.offline_since.is_some_and(|t| t.elapsed() > Duration::from_secs(15));
        let cause = if !away {
            0
        } else if s.offline_text.contains("network") {
            1
        } else {
            2
        };
        if ui.get_offline_cause() != cause {
            ui.set_offline_cause(cause);
        }
    });
}

/// New random lines (empty-list lines, footers, crawls, titles) for the pages out of sight, so each visit shows
/// new ones. It waits until the page has settled and no finger drags a neighbour into view: nothing changes under
/// the eye.
fn reroll_lines(weak: slint::Weak<AppWindow>) {
    slint::Timer::single_shot(Duration::from_millis(700), move || {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_dragging() {
            reroll_lines(ui.as_weak());
            return;
        }
        let pages = ui.get_pages();
        let kind = pages.row_data(ui.get_page().max(0) as usize).map(|p| p.kind.to_string()).unwrap_or_default();
        for (k, prefix) in [("info", "info."), ("todo", "todo."), ("calendar", "cal."), ("quick", "quick.")] {
            if kind != k {
                texts::reroll(prefix);
            }
        }
        // the crawls start afresh on their next visit
        if kind != "info" {
            ui.set_info_news_epoch(ui.get_info_news_epoch() + 1);
        }
        if kind != "calendar" {
            ui.set_cal_board_epoch(ui.get_cal_board_epoch() + 1);
        }
        refresh_pages(&ui);
    });
}

/// Carries out a request from Home Assistant (ESPHome API or the PanelOS
/// integration) on the UI thread.
fn apply_control(ui: &AppWindow, control: esphome::Control) {
    if matches!(control, esphome::Control::Setting(..) | esphome::Control::Screen(_)) {
        stir(ui);
    }
    match control {
        esphome::Control::Setting(k, v) => on_setting(ui, &k, &v),
        esphome::Control::Screen(true) => wake(ui),
        esphome::Control::Screen(false) => with_state(|s| {
            if !s.screensaver {
                start_screensaver(ui, s);
            }
        }),
        esphome::Control::Timer { kind, id, name, left, total, active } => handle_timer(ui, kind, id, name, left, total, active),
    }
}

/// Someone is near or something was asked of the panel: the clocks breathe again at once (see `platform::calm`).
fn stir(ui: &AppWindow) {
    platform::note_stir();
    if ui.global::<Clock>().get_calm() {
        ui.global::<Clock>().set_calm(false);
    }
}

fn wake(ui: &AppWindow) {
    platform::note_activity();
    stir(ui);
    if ui.get_screensaver_on() {
        ui.set_screensaver_on(false);
        with_state(|s| {
            s.screensaver = false;
            s.screen_off = false;
        });
    }
}

/// The darkness below which the screensaver may start by default (lux at the panel). Measured on the
/// panel's own sensor: a lamp-lit evening 5–26 lx, the dark room about 1 lx.
const DARK_LUX: f64 = 3.0;

/// Whether the screensaver may start now (the integration's "When"): only in the dark (the default:
/// darker than `lux`, from the panel's sensor or a Home Assistant one), between two times, from sunset
/// to sunrise, or any time. `leaving`: whether one already on may stay (in the dark it stays until it
/// is clearly brighter, so a reading near the limit does not toggle it).
fn saver_window(s: &UiState, leaving: bool) -> bool {
    let cfg = s.panel_config.as_ref().and_then(|c| c.get("screensaver"));
    let get = |k: &str| cfg.and_then(|c| c.get(k));
    let night = || s.states.get("sun.sun").map(|e| e.state == "below_horizon").unwrap_or(true);
    match get("when").and_then(Value::as_str).unwrap_or("dark") {
        "any" => true,
        "night" => night(),
        "window" => {
            let mins = |k: &str, d: i64| {
                get(k)
                    .and_then(Value::as_str)
                    .and_then(|t| {
                        let (h, m) = t.split_once(':')?;
                        Some(h.trim().parse::<i64>().ok()? * 60 + m.trim().parse::<i64>().ok()?)
                    })
                    .unwrap_or(d)
            };
            let (from, to) = (mins("from", 22 * 60), mins("to", 6 * 60));
            let t = pages::now();
            let now = pages::local_format(t, "%H").parse::<i64>().unwrap_or(0) * 60 + pages::local_format(t, "%M").parse::<i64>().unwrap_or(0);
            if from <= to {
                now >= from && now < to
            } else {
                now >= from || now < to
            }
        }
        _ => {
            let lux = get("light_entity")
                .and_then(Value::as_str)
                .and_then(|e| s.states.get(e))
                .and_then(|e| e.state.parse::<f64>().ok())
                .or(s.lux);
            let limit = get("lux").and_then(Value::as_f64).unwrap_or(DARK_LUX);
            match lux {
                Some(l) if leaving => l <= limit * 2.0 + 2.0,
                Some(l) => l < limit,
                None => night(),
            }
        }
    }
}

fn start_screensaver(ui: &AppWindow, s: &mut UiState) {
    s.screensaver = true;
    s.saver_auto = false;
    // the pages are covered once the screensaver has faded in (0.8 s): their animations' clock starts
    // again unseen then
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(900), move || {
        if let Some(ui) = weak.upgrade() {
            if ui.get_screensaver_on() {
                ui.invoke_reset_clock();
            }
        }
    });
    // the strip goes; a shown view or dashboard keeps its time and goes back under the screensaver
    s.strip_until = None;
    ui.set_set_view(0);
    ui.set_strip_on(false);
    info::pop(ui, None);
    close_page_sheets(ui);
    home_edit::close(ui);
    saver_clock(ui, s);
    ui.set_screensaver_on(true);
}

/// The screensaver's clock as set on the integration's Display page (also while it shows: a change
/// there is seen at once).
fn saver_clock(ui: &AppWindow, s: &UiState) {
    let cfg = s.panel_config.as_ref().and_then(|c| c.get("screensaver")).cloned().unwrap_or(Value::Null);
    let num = |k: &str, d: i64| cfg.get(k).and_then(Value::as_i64).unwrap_or(d);
    let flag = |k: &str| cfg.get(k).and_then(Value::as_bool).unwrap_or(true);
    let style = cfg.get("face").and_then(Value::as_i64)
        .or_else(|| s.panel_config.as_ref().and_then(|c| c.get("saver_style")).and_then(Value::as_i64))
        .unwrap_or(1) as i32;
    ui.set_saver_style(style);
    ui.set_saver_weight(num("weight", 600) as i32);
    ui.set_saver_width(num("width", 100) as f32 / 100.0);
    ui.set_saver_speed(num("speed", 20) as f32);
    ui.set_saver_date(flag("date"));
    ui.set_saver_weather(flag("weather"));
    ui.set_saver_next(flag("next"));
    ui.set_saver_seconds(flag("seconds"));
    ui.set_saver_accent(cfg.get("colour").and_then(Value::as_str).and_then(pages::parse_color).unwrap_or_else(|| accent(s)));
}

/// Weight of a new once-a-minute reading in its average (about five minutes).
const REPORT_SMOOTHING: f64 = 0.2;

fn smooth(avg: Option<f64>, reading: Option<f64>) -> Option<f64> {
    match (avg, reading) {
        (Some(a), Some(r)) => Some(a + REPORT_SMOOTHING * (r - a)),
        (_, r) => r,
    }
}

/// The value to send when the average moved at least `step` from the one
/// sent last (rounded to `1 / per_unit`); None = keep the old one.
fn report_value(sent: Option<f64>, avg: Option<f64>, step: f64, per_unit: f64) -> Option<Option<f64>> {
    match (sent, avg) {
        (Some(s), Some(a)) if (a - s).abs() < step => None,
        (None, None) => None,
        (_, a) => Some(a.map(|v| (v * per_unit).round() / per_unit)),
    }
}

/// Copies what Home Assistant sees (ESPHome API) from the UI state.
fn report_device(s: &mut UiState) {
    let reporting = s.settings.reporting;
    // light: a change of at least the step, at most once in 10 s; and every 10 minutes even without a change
    let since = s.last_lux_report.elapsed();
    let lux_changed = match (s.lux, s.last_reported_lux) {
        (Some(now), Some(before)) => {
            ((now - before).abs() >= reporting.lux_step(before) && since >= Duration::from_secs(10)) || since >= Duration::from_secs(600)
        }
        (a, b) => a.is_some() != b.is_some(),
    };
    let periodic = s.last_report.elapsed() > Duration::from_secs(60);
    let mut d = s.device.lock().unwrap();
    // every value Home Assistant shows from here (a change of any one of them alone is sent at once)
    let before = (
        (d.theme.clone(), d.brightness, d.auto_brightness, d.screensaver_seconds, d.screensaver_style.clone()),
        (d.proximity_wake, d.screen_on, d.dashboard.clone(), d.presence, d.display_mode.clone(), d.volume),
        (d.mic_muted, d.alarms.clone(), d.child_lock, d.rain_on_glass, d.lightning, d.time_24h),
    );
    d.theme = s.settings.theme.clone();
    d.time_24h = s.settings.time_24h;
    d.brightness = s.settings.brightness;
    d.auto_brightness = s.settings.auto_brightness;
    d.screensaver_seconds = s.settings.screensaver_seconds;
    d.screensaver_style = s.settings.screensaver_style.clone();
    d.proximity_wake = s.settings.proximity_wake;
    d.child_lock = s.settings.child_lock;
    d.rain_on_glass = s.settings.rain_on_glass;
    d.lightning = s.settings.lightning;
    d.screen_on = !s.screensaver;
    d.dashboard = s.shared.dashboard.lock().unwrap().clone();
    d.presence = s.proximity.near();
    d.mic_muted = s.settings.mic_muted;
    d.display_mode = s.settings.display_mode.clone();
    d.volume = s.settings.volume;
    // one volume: Music Assistant's volume of the panel's Sendspin player is the panel's own
    crate::sendspin::panel_volume(s.settings.volume);
    d.alarms = serde_json::to_value(crate::alarms::reported(&s.settings.alarms)).unwrap_or_default();
    d.music_player = media_entity(s);
    d.ready = true;
    let after = (
        (d.theme.clone(), d.brightness, d.auto_brightness, d.screensaver_seconds, d.screensaver_style.clone()),
        (d.proximity_wake, d.screen_on, d.dashboard.clone(), d.presence, d.display_mode.clone(), d.volume),
        (d.mic_muted, d.alarms.clone(), d.child_lock, d.rain_on_glass, d.lightning, d.time_24h),
    );
    if lux_changed {
        d.lux = s.lux.map(|l| l.round());
        s.last_reported_lux = s.lux;
        s.last_lux_report = Instant::now();
    }
    if periodic {
        s.soc_temp_avg = smooth(s.soc_temp_avg, hw::soc_temperature());
        s.wifi_avg = smooth(s.wifi_avg, hw::wifi_dbm());
        if let Some(v) = report_value(d.soc_temp, s.soc_temp_avg, reporting.soc_temperature_step, 10.0) {
            d.soc_temp = v;
        }
        if let Some(v) = report_value(d.wifi_dbm, s.wifi_avg, reporting.wifi_signal_step, 1.0) {
            d.wifi_dbm = v;
        }
        d.ip = s
            .config
            .as_ref()
            .and_then(|c| pairing::local_ip_towards(&c.ha_host_port()))
            .map(|ip| ip.to_string())
            .unwrap_or_default();
        s.last_report = Instant::now();
    }
    drop(d);
    if before != after || lux_changed || periodic {
        let _ = s.device_notify.send(());
    }
}

/// Auto theme thresholds: dark below, light above, unchanged in between.
const THEME_DARK_LUX: f64 = 10.0;
const THEME_LIGHT_LUX: f64 = 30.0;
/// How long the light must stay on the other side before the look changes
/// (no flipping at dusk or when a hand passes the sensor).
const THEME_SETTLE: Duration = Duration::from_secs(10);

/// Dark or light for the "auto" theme, by what the integration's `theme_auto` names: the sun (Home Assistant's
/// `sun.sun`), set times (once the clock is set), or the light sensor. The light sensor's first decision is
/// immediate, so the first screen already has the right look. Without what the trigger needs, the light sensor
/// decides; without a sensor reading Home Assistant's sun, otherwise unchanged (dark at the start).
fn auto_dark(s: &mut UiState, by: device_options::AutoBy) -> bool {
    match by {
        device_options::AutoBy::Sun => {
            if let Some(sun) = s.states.get("sun.sun") {
                s.theme_pending = None;
                return sun.state == "below_horizon";
            }
        }
        device_options::AutoBy::Times { from, to } if clock::valid() => {
            s.theme_pending = None;
            return device_options::in_window(device_options::minutes_now(), from, to);
        }
        _ => {}
    }
    let Some(lux) = s.lux else {
        s.theme_pending = None;
        return match s.states.get("sun.sun") {
            Some(sun) => sun.state == "below_horizon",
            None => s.dark.unwrap_or(true),
        };
    };
    let wanted = if lux < THEME_DARK_LUX {
        Some(true)
    } else if lux > THEME_LIGHT_LUX {
        Some(false)
    } else {
        None
    };
    let Some(current) = s.dark else {
        return wanted.unwrap_or(lux < (THEME_DARK_LUX + THEME_LIGHT_LUX) / 2.0);
    };
    match wanted {
        Some(w) if w != current => match s.theme_pending {
            Some((p, since)) if p == w => {
                if since.elapsed() >= THEME_SETTLE {
                    s.theme_pending = None;
                    w
                } else {
                    current
                }
            }
            _ => {
                s.theme_pending = Some((w, Instant::now()));
                current
            }
        },
        _ => {
            s.theme_pending = None;
            current
        }
    }
}

fn update_theme(ui: &AppWindow, s: &mut UiState) {
    let by = device_options::auto_by(s.panel_config.as_ref());
    let dark = match s.settings.theme.as_str() {
        "light" => false,
        "auto" => auto_dark(s, by),
        _ => true,
    };
    // the setting with Auto's trigger: a change of either is someone's switch, the quicker change
    let seen = if s.settings.theme == "auto" { format!("auto {by:?}") } else { s.settings.theme.clone() };
    ui.set_theme_mode(match s.settings.theme.as_str() {
        "light" => 0,
        "auto" => 2,
        _ => 1,
    });
    if s.dark != Some(dark) {
        // Auto changed it by itself (the light, the sun, the time): the slow blend; a switch by someone: the quicker one
        ui.set_theme_slow(s.dark.is_some() && s.settings.theme == "auto" && s.theme_mode_seen.as_deref() == Some(seen.as_str()));
        let first = s.dark.is_none();
        s.dark = Some(dark);
        if first {
            ui.global::<Theme>().set_dark(dark);
        } else {
            // a picture of the screen first, then the switch under it: the picture fades out over the new look
            let weak = ui.as_weak();
            let switched = std::rc::Rc::new(std::cell::Cell::new(false));
            let done = switched.clone();
            platform::request_screen(move |w, h, rgb| {
                let Some(ui) = weak.upgrade() else { return };
                if done.replace(true) {
                    return;
                }
                if rgb.len() >= (w * h * 3) as usize {
                    let pic = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(&rgb[..(w * h * 3) as usize], w, h);
                    ui.set_theme_shot(slint::Image::from_rgb8(pic));
                    ui.invoke_theme_fade();
                }
                ui.global::<Theme>().set_dark(dark);
            });
            // no picture within 1.2 s (the screen is not being drawn): the switch without the crossfade
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(1200), move || {
                if let Some(ui) = weak.upgrade() {
                    if !switched.replace(true) {
                        eprintln!("nspanel-app: theme: no picture of the screen, switched without the crossfade");
                        ui.global::<Theme>().set_dark(dark);
                    }
                }
            });
        }
    }
    s.theme_mode_seen = Some(seen);
}

/// Backlight fade, every 20 ms: towards the target in perceived brightness
/// (gamma 2.2), so a change looks even in the dark too, and with a time
/// constant instead of steps: about 0.8 s down, 0.25 s up, 0.12 s for a big
/// rise (waking from the screensaver).
fn fade_backlight() {
    with_state(|s| {
        const GAMMA: f64 = hw::BACKLIGHT_GAMMA;
        let dt: f64 = 0.02;
        let (now, target) = (s.brightness.clamp(0.0, 1000.0), s.brightness_target.clamp(0.0, 1000.0));
        let tau = if target > now + 300.0 {
            0.12
        } else if target > now {
            0.25
        } else {
            0.8
        };
        let p_now = (now / 1000.0).powf(1.0 / GAMMA);
        let p_target = (target / 1000.0).powf(1.0 / GAMMA);
        let p = p_now + (p_target - p_now) * (1.0 - (-dt / tau).exp());
        s.brightness = if (p - p_target).abs() < 0.002 { target } else { p.powf(GAMMA) * 1000.0 };
        s.backlight.set_permille(s.brightness.round() as u32);
    });
}

/// Web view: the page for the chosen dashboard, rendered only while it is
/// on the screen.
fn update_web(ui: &AppWindow, s: &mut UiState) {
    let Some(view) = s.web.clone() else { return };
    let Some(config) = s.config.as_ref() else { return };
    let dashboard = if !s.settings.web.dashboard.is_empty() {
        s.settings.web.dashboard.clone()
    } else {
        let own = s.shared.dashboard.lock().unwrap().clone();
        if own.is_empty() { "lovelace".to_string() } else { own }
    };
    let url = format!("{}/{}?external_auth=1", config.ha_url.trim_end_matches('/'), dashboard);
    // Before the page: the helper loads it once the resource filter is ready.
    view.set_skip(s.settings.web.skip_resources.clone());
    view.set_zoom(s.settings.web.zoom);
    {
        let mut d = s.device.lock().unwrap();
        if d.web_dashboard != dashboard {
            d.web_dashboard = dashboard.clone();
        }
    }
    view.load(&url, !s.settings.web.show_header, !s.settings.web.show_sidebar);
    let on_screen = ui.get_web_shown() && !s.screensaver && ui.get_set_view() == 0;
    view.set_visible(on_screen);
}

/// Runs twice a second: theme, screensaver, proximity wake, backlight target.
fn tick(ui: &AppWindow) {
    // a view shown for some seconds: its time is up (back to the page before, unless a ring holds the panel)
    if VISIT.with(|v| v.get()).and_then(|v| v.show).is_some_and(|(_, until)| Instant::now() >= until) {
        end_visit(ui, false);
    }
    update_connection_screens(ui);
    with_state(|s| update_web(ui, s));
    let result = with_state(|s| {
        s.lux = s.light.lux();
        update_theme(ui, s);
        // A notification's strip: it goes when its time is up (a status strip keeps its own).
        if s.strip_until.is_some_and(|t| Instant::now() >= t) {
            s.strip_until = None;
            if !s.alert_strip {
                ui.set_strip_on(false);
            }
        }
        // The dashboard shown by a show request: back to the display mode when its time is up.
        if s.show_until.is_some_and(|t| Instant::now() >= t) {
            s.show_until = None;
            set_dashboard(ui, s.settings.display_mode == "dashboard" && s.web.is_some());
        }
        let idle = platform::idle_time();
        // Proximity test view: on while the file exists. Its first line is
        // the integration time to test (a change of the line learns the
        // resting level anew), the rest a prompt shown above the reading.
        let content = std::fs::read_to_string(PROXIMITY_TEST_FILE).ok();
        let test = content.as_deref().map(|c| c.lines().next().unwrap_or("").trim().to_string());
        if let Some(c) = content.as_deref() {
            ui.set_prox_test_prompt(c.lines().skip(1).collect::<Vec<_>>().join("\n").into());
        }
        let mut test_started = false;
        if test != s.proximity_test {
            let integration = test.as_deref().and_then(|t| t.split_whitespace().next()).unwrap_or(hw::PROXIMITY_INTEGRATION);
            s.light.set_proximity_integration(integration);
            s.proximity = hw::ProximityWake::new(integration.parse().unwrap_or(0.01184));
            let cm = device_options::wake_cm(s);
            s.proximity.set_wake_cm(cm);
            test_started = test.is_some() && s.proximity_test.is_none();
            ui.set_prox_test(test.is_some());
            match &test {
                Some(_) => eprintln!("nspanel-app: proximity test on, integration time {integration} s"),
                None => eprintln!("nspanel-app: proximity test off"),
            }
            s.proximity_test = test;
        }
        // Touch test view: on while its file exists.
        // (or started from the settings / the integration; its result stays a few seconds)
        if s.touch_close_at.is_some_and(|t| Instant::now() >= t) {
            s.touch_close_at = None;
        }
        // nobody finishing it: it closes after a minute and a half
        if s.touch_test_manual && s.touch_started.is_some_and(|t| t.elapsed() > Duration::from_secs(90)) {
            s.touch_test_manual = false;
        }
        let touch_test = std::path::Path::new(TOUCH_TEST_FILE).exists() || s.touch_test_manual || s.touch_close_at.is_some();
        if touch_test != s.touch_test {
            s.touch_test = touch_test;
            s.touch_test_step = 0;
            s.touch_test_down = None;
            ui.set_touch_test_step(0);
            ui.set_touch_test_text(touch_test_text(0).into());
            ui.set_touch_test(touch_test);
            eprintln!("nspanel-app: touch test {}", if touch_test { "on" } else { "off" });
            // started from the settings list: back there
            if !touch_test && std::mem::take(&mut s.touch_from_settings) {
                ui.set_set_view(3);
                let weak = ui.as_weak();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(ui) = weak.upgrade() {
                        shade::fill(&ui);
                    }
                });
            }
        }
        // Screensaver (not while Home Assistant shows something)
        let timeout = s.settings.screensaver_seconds;
        // (and not while a timer runs: its chips stay in view)
        let showing = s.show_until.is_some()
            || s.strip_until.is_some()
            || VISIT.with(|v| v.get()).is_some_and(|v| v.show.is_some())
            || ui.get_pop_on()
            || s.proximity_test.is_some()
            || device_options::calibrating()
            || ui.get_voice_on()
            || s.ringing.is_some()
            || s.timers.values().any(|t| t.active)
            || !s.statuses.is_empty()
            || s.critical.is_some()
            // (a Wi-Fi network being tried: its result stays in view)
            || ui.get_wifi_busy();
        if !s.screensaver
            && !showing
            && timeout > 0
            && ui.get_paired()
            && !ui.get_setup()
            && idle > Duration::from_secs(timeout as u64)
            && saver_window(s, false)
        {
            start_screensaver(ui, s);
            s.saver_auto = true;
        }
        // outside its window (a lamp switched on, the morning) the panel shows its pages again
        let saver_ends = s.screensaver && s.saver_auto && !saver_window(s, true);
        // Proximity: presence for Home Assistant, wake from the screensaver
        let mut wake_now = test_started && s.screensaver;
        let reading = s.light.proximity();
        if let Some(p) = reading {
            let approach = s.proximity.update(p);
            wake_now |= approach && s.screensaver && s.settings.proximity_wake;
            if s.proximity_test.is_some() {
                show_proximity_test(ui, s, p);
            }
            // someone near (an approach, or still near) keeps the clocks breathing
            if approach || s.proximity.near() {
                platform::note_stir();
            }
        }
        // the child lock locking again, the wake distance calibration
        device_options::tick(ui, s, idle, reading);
        // the clocks' calm: nobody near and nothing asked of the panel for 10 minutes
        let calm = platform::calm();
        if ui.global::<Clock>().get_calm() != calm {
            ui.global::<Clock>().set_calm(calm);
        }
        // Backlight
        let target = if s.screensaver && (s.settings.screensaver_style == "black" || s.screen_off) && !wake_now {
            0.0
        } else {
            let normal = if s.settings.auto_brightness {
                s.lux.map(hw::auto_brightness_permille).unwrap_or(600) as f64
            } else {
                hw::manual_brightness_permille(s.settings.brightness) as f64
            };
            if s.screensaver && !wake_now { normal.min(150.0) } else { normal }
        };
        s.brightness_target = target;
        report_device(s);
        (wake_now || saver_ends, s.weather_refreshed.elapsed() > Duration::from_secs(60))
    });
    let (wake_now, refresh) = result;
    if wake_now {
        wake(ui);
    }
    if refresh {
        refresh_pages(ui);
    }
}

/// Developer tool: while this file exists the panel shows the proximity
/// reading and an estimated hand distance. First line: the integration time
/// in seconds (one of `in_proximity_integration_time_available`, empty = the
/// default), optionally followed by a word whose change restarts learning;
/// further lines: a prompt for the person at the panel.
const PROXIMITY_TEST_FILE: &str = "/tmp/nspanel-proximity-test";

/// Touch test (development): on while this file exists. Nine points, touched
/// one after the other (corners, edge middles, the middle held for 2 s); each
/// touch is logged with its distance from the point.
const TOUCH_TEST_FILE: &str = "/tmp/nspanel-touch-test";
const TOUCH_TEST_POINTS: [(f32, f32); 9] =
    [(16.0, 16.0), (240.0, 16.0), (464.0, 16.0), (464.0, 240.0), (464.0, 464.0), (240.0, 464.0), (16.0, 464.0), (16.0, 240.0), (240.0, 240.0)];
const TOUCH_TEST_HOLD: Duration = Duration::from_millis(1500);
/// A touch counts for the point only this near to it (pixels); a calibration never needs more.
const TOUCH_TEST_RADIUS: f32 = 40.0;

fn touch_test_text(step: usize) -> String {
    match step {
        0..=7 => format!("Touch the red point ({} of 9)", step + 1),
        8 => "Hold the red point for 2 seconds (9 of 9)".into(),
        _ => with_state(|s| s.touch_result.clone()).unwrap_or_else(|| "Done, thank you".into()),
    }
}

/// The line after a touch too far from the point (it does not count).
fn touch_test_miss_text(step: usize) -> String {
    format!("Touch the red point itself ({} of 9)", step.min(8) + 1)
}

/// Whether a touch at `offset` (dx, dy from the point) counts for it.
fn touch_test_hit(offset: (f32, f32)) -> bool {
    offset.0.hypot(offset.1) <= TOUCH_TEST_RADIUS
}

/// The touch test's way out: nothing is stored, the correction stays as it was.
fn cancel_touch_test(ui: &AppWindow) {
    let back = with_state(|s| {
        s.touch_test_manual = false;
        s.touch_close_at = None;
        s.touch_offsets.clear();
        s.touch_result = None;
        s.touch_test_down = None;
        s.touch_test = false;
        s.touch_test_step = 0;
        std::mem::take(&mut s.touch_from_settings)
    });
    eprintln!("nspanel-app: touch test cancelled");
    ui.set_touch_test(false);
    ui.set_touch_test_step(0);
    if back {
        ui.set_set_view(3);
        shade::fill(ui);
    }
}

/// Starts the touch calibration (the settings' Touch row, or the integration).
fn start_touch_test(ui: &AppWindow) {
    let from_settings = ui.get_set_view() == 3;
    with_state(|s| {
        s.touch_from_settings = from_settings;
        s.touch_test_manual = true;
        s.touch_offsets.clear();
        s.touch_result = None;
        s.touch_started = Some(Instant::now());
    });
    ui.set_set_view(0);
    wake(ui);
}

/// The nine points touched: a correction only when the offsets are systematic (all the same way,
/// more than 3 px on average, spread under 6 px); the test closes a few seconds later.
fn finish_touch_test(s: &mut UiState) {
    let n = s.touch_offsets.len().max(1) as f32;
    let (mx, my) = s.touch_offsets.iter().fold((0.0, 0.0), |a, o| (a.0 + o.0 / n, a.1 + o.1 / n));
    let spread = (s.touch_offsets.iter().map(|o| (o.0 - mx).powi(2) + (o.1 - my).powi(2)).sum::<f32>() / n).sqrt();
    let systematic = (mx * mx + my * my).sqrt() > 3.0 && spread < 6.0;
    let (ox, oy) = s.settings.touch_offset;
    let result = if systematic {
        s.settings.touch_offset = (ox + mx, oy + my);
        *platform::TOUCH_OFFSET.lock().unwrap() = s.settings.touch_offset;
        s.settings.save();
        format!("Corrected by {:+.0} px across, {:+.0} px down", mx, my)
    } else {
        "No correction needed".into()
    };
    eprintln!("nspanel-app: touch calibration: mean ({mx:+.1},{my:+.1}) spread {spread:.1} px → {result}");
    s.touch_result = Some(result);
    s.touch_test_manual = false;
    s.touch_close_at = Some(Instant::now() + Duration::from_secs(4));
}

fn show_proximity_test(ui: &AppWindow, s: &UiState, raw: f64) {
    let rest = s.proximity.rest().unwrap_or(raw);
    let signal = raw - rest;
    let margin = s.proximity.margin();
    let jitter = s.proximity.jitter();
    let integration = s.light.proximity_integration().unwrap_or(0.0);
    let distance = if signal > (3.0 * jitter).max(10.0) {
        hw::proximity_distance_cm(signal, integration).map(|d| format!("{d:.0} cm"))
    } else {
        None
    };
    ui.set_prox_test_distance(distance.unwrap_or_else(|| "–".into()).into());
    ui.set_prox_test_details(
        format!(
            "signal {signal:+.0}, threshold {margin:.0}\nraw {raw:.0}, rest {rest:.0}, jitter {jitter:.1}\nintegration {:.2} ms",
            integration * 1000.0
        )
        .into(),
    );
    ui.set_prox_test_near(s.proximity.near());
}

fn on_setting(ui: &AppWindow, key: &str, value: &str) {
    let mut reload = false;
    let mut lock = false;
    match key {
        // the integration's page settings are open: the panel shows that page (by its place among the
        // pages shown), so the picture there shows what is being set
        "show_page" => {
            let i = value.parse::<i32>().unwrap_or(-1);
            if i >= 0 && (i as usize) < ui.get_pages().row_count() && with_state(|s| s.temp_page.is_none()) {
                wake(ui);
                ui.set_page(i);
            }
            return;
        }
        "restart_app" => {
            eprintln!("nspanel-app: restart requested (settings / Home Assistant button)");
            sendspin::goodbye();
            std::process::exit(0)
        }
        "reboot" => {
            eprintln!("nspanel-app: reboot requested (settings / Home Assistant button)");
            let _ = std::process::Command::new("reboot").status();
            return;
        }
        "logout" => {
            if let Some(auth) = with_state(|s| s.auth.clone()) {
                auth.forget();
            }
            ui.set_set_view(0);
            send(Command::Reload);
            return;
        }
        _ => {}
    }
    with_state(|s| {
        match key {
            "theme" => s.settings.theme = value.to_string(),
            "auto_brightness" => s.settings.auto_brightness = value == "true",
            "brightness" => s.settings.brightness = value.parse::<u8>().unwrap_or(70).clamp(1, 100),
            "screensaver_index" => {
                if let Some((secs, _)) = value.parse::<usize>().ok().and_then(|i| settings::SCREENSAVER_CHOICES.get(i)) {
                    s.settings.screensaver_seconds = *secs;
                }
            }
            "screensaver_style" => s.settings.screensaver_style = value.to_string(),
            "proximity_wake" => s.settings.proximity_wake = value == "true",
            "child_lock" => {
                s.settings.child_lock = value == "true";
                lock = true;
            }
            "rain_on_glass" => s.settings.rain_on_glass = value == "true",
            "lightning" => {
                s.settings.lightning = value == "true";
                ui.set_lightning(s.settings.lightning);
            }
            // "24" or "12"
            "time_format" => s.settings.time_24h = value != "12",
            "display_mode" => {
                if matches!(value, "dashboard" | "assistant") {
                    s.settings.display_mode = value.to_string();
                }
            }
            "wake_words" => {
                s.settings.wake_words = value.split(',').filter(|v| !v.is_empty()).map(str::to_string).collect();
                *s.wake_words.lock().unwrap() = s.settings.wake_words.clone();
                s.wake_phrase = wake_phrase(&s.settings.wake_words);
            }
            "volume" => {
                let new = value.parse::<u8>().unwrap_or(80).min(100);
                // muted from Home Assistant: the speaker mark's tap goes back to the level before
                if new == 0 && s.settings.volume > 0 {
                    s.vol_before_mute = s.settings.volume;
                }
                s.settings.volume = new;
                hw::set_volume(s.settings.volume);
                ui.set_speaker_muted(s.settings.volume == 0);
                // Short sound at the new level (also for changes from Home Assistant).
                s.player.play(voice::Sound::Chime);
            }
            "mic_muted" => {
                s.settings.mic_muted = value == "true";
                s.mic_muted.store(s.settings.mic_muted, Ordering::SeqCst);
                ui.set_mic_muted(s.settings.mic_muted);
                let hint = wake_hint_text(s);
                ui.set_wake_hint(hint.into());
            }
            "dashboard_index" => {
                if let Some((url, _)) = value.parse::<usize>().ok().and_then(|i| s.dashboards.get(i)) {
                    s.settings.dashboard = url.clone();
                    *s.shared.dashboard.lock().unwrap() = url.clone();
                    reload = true;
                }
            }
            _ => eprintln!("nspanel-app: unknown setting {key}"),
        }
        s.settings.save();
    });
    if lock {
        device_options::lock_changed(ui);
    }
    apply_time_format(ui);
    apply_panel_config(ui);
    if reload {
        send(Command::Reload);
    }
}

/// The time format from the settings to every clock: the UI's (`Clock.h24`), the app's texts (`clock::hm`), and the
/// texts already on screen written again.
fn apply_time_format(ui: &AppWindow) {
    let on = with_state(|s| s.settings.time_24h);
    if clock::h24() == on && ui.global::<Clock>().get_h24() == on {
        return;
    }
    clock::set_h24(on);
    ui.global::<Clock>().set_h24(on);
    update_clock(ui);
    with_state(|s| {
        alarms::refresh(ui, s);
        info::refresh(ui, s);
        cal::refresh(ui, s);
        todo::refresh(ui, s);
    });
    refresh_pages(ui);
    if ui.get_set_view() > 0 {
        shade::fill(ui);
    }
}

/// Taps, holds and drags from the pages and layers (`act(what, value)`).
fn handle_act(ui: &AppWindow, what: &str, value: &str) {
    platform::note_activity();
    stir(ui);
    match what {
        "saver.wake" => wake(ui),
        w if w.starts_with("lock.") || w.starts_with("prox.") => device_options::act(ui, w, value),
        "touch.cancel" => cancel_touch_test(ui),
        "timers.stop" => stop_ringing(ui, "tap"),
        "timers.more" => ring_more(ui),
        w if w.starts_with("alarms.") || w.starts_with("alarm.") => alarms::act(ui, w, value),
        w if w.starts_with("synth.") => synth::act(ui, w, value),
        "home.window" => {
            let mut it = value.split(':').map(|v| v.parse::<f32>().unwrap_or(-1.0));
            if let (Some(x), Some(y)) = (it.next(), it.next()) {
                windows::tap(ui, x, y);
            }
        }
        // a finger wiping the drops off the glass (H1): its places along the stroke, then its end
        "glass.wipe" => {
            let mut it = value.split(':').map(|v| v.parse::<f32>().ok());
            if let (Some(Some(x)), Some(Some(y))) = (it.next(), it.next()) {
                glass::wipe(x, y);
            }
        }
        "glass.wipe-end" => glass::wipe_end(),
        w if w.starts_with("timers.") || w.starts_with("watch.") => timers_act(ui, w, value),
        "voice.close" => close_voice_view(ui),
        // the bell: the notifications page, or the list as a temporary page when the panel has none
        "notes.open" => {
            if !go_to_page(ui, "info") {
                open_temporary_page(ui, "info");
            }
        }
        // the status bar's marks (29.2): the microphone and the speaker back on, the timers page
        "mark.mic" => {
            if ui.get_mic_muted() {
                shade::act(ui, "set.mic", "");
            }
        }
        "mark.speaker" => {
            let to = with_state(|s| if s.vol_before_mute > 0 { s.vol_before_mute } else { 30 });
            shade::act(ui, "set.volume", &format!("{}", to as f32 / 100.0));
        }
        "mark.timers" => {
            if !go_to_page(ui, "timers") {
                open_temporary_page(ui, "timers");
            }
        }
        // a timer on the island: the timers page with its sheet
        "island.timer" => {
            if !go_to_page(ui, "timers") {
                open_temporary_page(ui, "timers");
            }
            let i = value.parse::<i32>().unwrap_or(-1);
            if matches!(page_look("timers"), 0 | 3) && slint::Model::row_count(&ui.get_timers_list()) >= 2 {
                ui.set_timers_sheet(i);
            } else if i >= 0 {
                ui.set_timers_focus(i);
            }
        }
        "strip.close" => {
            alerts::act(ui, what);
            with_state(|s| s.strip_until = None);
        }
        "status.fold" | "strip.open" | "info.crit-silence" => alerts::act(ui, what),
        "set.touch" => start_touch_test(ui),
        // the time format from a look's own 12H / 24H key ("true" 24-hour, "false" 12-hour): kept, applied to every
        // clock, reported to the integration
        "set.h24" => on_setting(ui, "time_format", if value == "false" { "12" } else { "24" }),
        "shade.open" => shade::open(ui),
        "shade.peek" => shade::fill(ui),
        // A home widget's view: clock → timers, date / event → calendar,
        // weather now / curve / hours / days / details → weather, music → music.
        "home.tap" => {
            let kind = match value {
                "0" | "9" => "timers",
                "1" | "3" => "calendar",
                "2" | "4" | "5" | "6" | "11" => "weather",
                "7" => "music",
                _ => return,
            };
            if !go_to_page(ui, kind) {
                open_temporary_page(ui, kind);
            }
        }
        // the weather page's edit mode on the panel
        w if w.starts_with("home.") => home_edit::act(ui, w, value),
        // the pages' settings sheets (look and options): open and close
        "music.settings" => {
            ui.set_music_picker(false);
            ui.set_music_settings(true);
        }
        "music.settings-close" => ui.set_music_settings(false),
        "music.look" => page_set(ui, "music", Some(value.parse().unwrap_or(0)), Value::Null),
        // the notification page's look
        "info.look" => {
            page_set(ui, "info", Some(value.parse().unwrap_or(0)), Value::Null);
            // the new look's pictures (the pager, the fridge door and notes) at once, not when the integration answers
            with_state(|s| info::refresh(ui, s));
        }
        "music.holds" => {
            page_set(ui, "music", None, json!({ "holds": value.parse::<i64>().unwrap_or(0) }));
            refresh_pages(ui);
        }
        "music.scroll" => {
            page_set(ui, "music", None, json!({ "scroll": value == "1" }));
            refresh_pages(ui);
        }
        "music.blur" => {
            page_set(ui, "music", None, json!({ "cover_blur": value == "1" }));
            refresh_pages(ui);
        }
        "music.cover-bg" => {
            page_set(ui, "music", None, json!({ "cover_bg": value == "1" }));
            refresh_pages(ui);
        }
        "music.speed" => {
            page_set(ui, "music", None, json!({ "scroll_speed": value.parse::<i64>().unwrap_or(3).clamp(1, 5) }));
            refresh_pages(ui);
        }
        "music.wood" => {
            if let Some(id) = WOODS.get(value.parse::<usize>().unwrap_or(0)) {
                page_set(ui, "music", None, json!({ "wood": id }));
                refresh_pages(ui);
            }
        }
        "thermo.settings" => {
            ui.set_thermo_picker(false);
            ui.set_thermo_settings(true);
        }
        "thermo.settings-close" => ui.set_thermo_settings(false),
        "thermo.look" => page_set(ui, "thermostat", Some(value.parse().unwrap_or(0)), Value::Null),
        // the neon look's scene and cars are the synthwave background's (shared with the home page)
        "thermo.scene" => page_set(ui, "home", None, json!({ "scene": value.parse::<i64>().unwrap_or(0) })),
        "thermo.cars" => {
            let on = with_state(|s| first_page_option(s, "home", "cars").and_then(|v| v.as_bool()).unwrap_or(true));
            page_set(ui, "home", None, json!({ "cars": !on }));
        }
        "cam.settings" => ui.set_cam_settings(true),
        "cam.settings-close" => ui.set_cam_settings(false),
        "info.settings" => ui.set_info_settings(true),
        "info.settings-close" => ui.set_info_settings(false),
        w if w.starts_with("set.") => shade::act(ui, w, value),
        w if w.starts_with("quick.") => quick::act(ui, w, value),
        w if w.starts_with("thermo.") => thermo::act(ui, w, value),
        w if w.starts_with("cam.") => cam::act(ui, w, value),
        w if w.starts_with("cal.") => cal::act(ui, w, value),
        w if w.starts_with("todo.") => todo::act(ui, w, value),
        w if w.starts_with("info.") => info::act(ui, w, value),
        w if w.starts_with("pop.") => info::pop_act(ui, w, value),
        "shade.close" => ui.set_set_view(0),
        "music.picker" | "music.picker-close" | "music.show" | "music.transfer" | "music.group" => mus::act(ui, what, value),
        "music.play" | "music.next" | "music.prev" | "music.shuffle" | "music.repeat" | "music.volume" | "music.seek" | "music.pause" | "music.resume" => {
            // the player of the page the tap came from (a music page, a custom page's music widget)
            let entity = with_state(|s| tap_player(ui, s));
            // what that player shows now
            let (shown, _) = with_state(|s| player_music(s, &entity));
            if entity.is_empty() {
                return;
            }
            // which control sent what to which player (a volume nobody remembered setting can be traced)
            if what != "music.seek" {
                eprintln!("nspanel-app: music page: {} {value} on {entity}", what.trim_start_matches("music."));
            }
            match what {
                "music.play" => {
                    call("media_player", "media_play_pause", &entity, json!({}));
                    hold_play_state(ui, &entity, !shown.playing);
                }
                // the turntable's arm: dropped on its rest pauses, dropped on the record plays
                "music.pause" => {
                    call("media_player", "media_pause", &entity, json!({}));
                    hold_play_state(ui, &entity, false);
                }
                "music.resume" => {
                    call("media_player", "media_play", &entity, json!({}));
                    hold_play_state(ui, &entity, true);
                }
                "music.next" | "music.prev" => {
                    call("media_player", if what == "music.next" { "media_next_track" } else { "media_previous_track" }, &entity, json!({}));
                    let playing = shown.playing;
                    with_state(|s| {
                        let from = track_key(&pages::home_music(&s.states, &entity, None, slint::Color::default()));
                        s.music_skip = Some(MusicSkip { player: entity.clone(), at: Instant::now(), from, playing, to: None });
                    });
                }
                "music.shuffle" => {
                    let on = pages::inst(&ui.get_music_inst(), acting_n("music")).shuffle;
                    call("media_player", "shuffle_set", &entity, json!({"shuffle": !on}))
                }
                "music.repeat" => {
                    let next = match pages::inst(&ui.get_music_inst(), acting_n("music")).repeat {
                        0 => "all",
                        1 => "one",
                        _ => "off",
                    };
                    call("media_player", "repeat_set", &entity, json!({"repeat": next}))
                }
                "music.volume" => {
                    let v: f64 = value.parse().unwrap_or(0.5);
                    call("media_player", "volume_set", &entity, json!({"volume_level": (v.clamp(0.0, 1.0) * 100.0).round() / 100.0}))
                }
                "music.seek" => {
                    let v: f64 = value.parse().unwrap_or(0.0);
                    call("media_player", "media_seek", &entity, json!({"seek_position": v.max(0.0)}))
                }
                _ => {}
            }
        }
        _ => eprintln!("nspanel-app: act {what} {value} (not handled yet)"),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The GPU build runs with the web bundle's libraries (LD_LIBRARY_PATH from
    // the init script; the loader keeps its own copy for later dlopen calls).
    // Programs the app starts (wpa_cli, iw, the web helper) use their own.
    #[cfg(feature = "gpu")]
    std::env::remove_var("LD_LIBRARY_PATH");
    if std::env::var_os("TZ").is_none() {
        std::env::set_var("TZ", DEFAULT_TZ);
    }
    // SAFETY: tzset only re-reads the TZ variable.
    unsafe { tzset() };

    platform::install_screenshot_signal();

    let settings = Settings::load();
    *platform::TOUCH_OFFSET.lock().unwrap() = settings.touch_offset;
    let config = AppConfig::load();
    // mDNS: announces the ESPHome API, so Home Assistant finds the panel;
    // browses for Home Assistant when the setup screen asks for it.
    let mdns_browser = Arc::new(mdns::Browser::default());
    mdns::spawn(vec![esphome::mdns_service(), sendspin::mdns_service()], mdns_browser.clone());
    let _ = MDNS_BROWSER.set(mdns_browser.clone());
    let shared = Arc::new(ha::Shared::default());
    // development install: the system log kept on the data partition and sent to Home Assistant
    devlog::spawn();
    let dashboard_name = if !settings.dashboard.is_empty() {
        settings.dashboard.clone()
    } else {
        config.as_ref().map(|c| c.dashboard.clone()).unwrap_or_default()
    };
    *shared.dashboard.lock().unwrap() = dashboard_name;
    // The same entities and forecast as apply_panel_config() sets later, so the
    // first session needs no reload (the integration's settings come with it).
    *shared.extra_entities.lock().unwrap() = vec!["sun.sun".into(), "zone.home".into(), settings.weather_entity.clone()];
    *shared.forecast_entity.lock().unwrap() = settings.weather_entity.clone();
    // NSPANEL_AUTH: another login file (with NSPANEL_CONFIG and NSPANEL_SETTINGS: a second
    // Home Assistant for tests, the panel's own login untouched).
    let auth_file = std::env::var("NSPANEL_AUTH").unwrap_or_else(|_| format!("{PERSIST_DIR}/ha_auth.json"));
    let auth = config.as_ref().ok().map(|c| Arc::new(Auth::load(&c.ha_url, PathBuf::from(&auth_file))));

    // Home Assistant client first, the display after: the first screen shows
    // the right time and current states instead of 1970 and empty pages.
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let ha_early = match (&config, &auth) {
        (Ok(c), Some(a)) => {
            let (event_tx, event_rx) = mpsc::channel::<Event>();
            ha::spawn(c.ws_url(), shared.clone(), a.clone(), cmd_rx, move |event| {
                let _ = event_tx.send(event);
            });
            let buffered = wait_until_ready(&event_rx);
            Some((event_rx, buffered))
        }
        _ => None,
    };

    // The boot splash owns the display until now: it shows the finished logo,
    // gives up DRM master and keeps the picture until this app has drawn.
    let splash = splash_handoff();
    let display = Rc::new(RefCell::new(display::Display::open("/dev/dri/card0")?));
    // Display buffers the web helper renders into while the web view is all
    // there is on the screen.
    let web_buffers = display.borrow_mut().create_web_buffers(3);
    match web_buffers {
        Ok(buffers) => web::set_scanout_buffers(buffers),
        Err(e) => eprintln!("nspanel-app: no display buffers for the web view: {e}"),
    }
    let (width, height) = (display.borrow().width, display.borrow().height);
    let touch = match touch::Touch::open() {
        Ok(t) => {
            eprintln!("nspanel-app: touch input: {}", t.name);
            Some(t)
        }
        Err(e) => {
            eprintln!("nspanel-app: no touch input: {e}");
            None
        }
    };

    #[cfg(not(feature = "gpu"))]
    let window = {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::SwappedBuffers);
        window.set_size(slint::PhysicalSize::new(width, height));
        window
    };
    #[cfg(feature = "gpu")]
    let window = {
        let _ = (width, height);
        gpu::GpuWindow::new(Rc::clone(&display)).map_err(|e| format!("GPU renderer: {e}"))?
    };
    slint::platform::set_platform(Box::new(platform::PanelPlatform::new(Rc::clone(&window), display, touch)))
        .map_err(|e| format!("set_platform: {e:?}"))?;

    let ui = AppWindow::new()?;
    UI.with(|u| *u.borrow_mut() = Some(ui.as_weak()));
    {
        let weak = ui.as_weak();
        platform::on_press(Box::new(move |pressed| {
            if let Some(ui) = weak.upgrade() {
                ui.set_finger_down(pressed);
            }
        }));
    }
    // the packed word lists the UI files ask for (before anything is drawn)
    ui.global::<Words>().on_list(|place| {
        ModelRc::new(VecModel::from(texts::list(&place).iter().map(|l| slint::SharedString::from(l.as_str())).collect::<Vec<_>>()))
    });
    ui.global::<Words>().on_line(|place| texts::nth(&place, 0).into());
    // Hand-over picture: the splash's last frame (same logo, same pixels),
    // then it fades over the UI.
    {
        // intro.rgba (tools/splash/storyboard.py): u16 x, y, w, h + RGBA.
        let raw: &[u8] = include_bytes!("../assets/intro.rgba");
        let rd = |i: usize| u16::from_le_bytes([raw[i], raw[i + 1]]) as u32;
        let (x, y, w, h) = (rd(0), rd(2), rd(4), rd(6));
        let mut pic = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
        pic.make_mut_bytes().copy_from_slice(&raw[8..8 + (w * h * 4) as usize]);
        ui.set_intro_logo(slint::Image::from_rgba8_premultiplied(pic));
        ui.set_intro_x(x as f32);
        ui.set_intro_y(y as f32);
    }
    // Loading screen background (dashboard mode).
    {
        let mut reader = png::Decoder::new(std::io::Cursor::new(&include_bytes!("../ui/images/loading.png")[..]))
            .read_info()
            .map_err(|e| format!("loading.png: {e}"))?;
        let mut rgb = vec![0; reader.output_buffer_size().unwrap_or(0)];
        let info = reader.next_frame(&mut rgb).map_err(|e| format!("loading.png: {e}"))?;
        if info.color_type == png::ColorType::Rgb {
            let pic = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(&rgb[..info.buffer_size()], info.width, info.height);
            ui.set_loading_image(slint::Image::from_rgb8(pic));
        }
    }
    // The pictures the app draws itself: when new ones are ready, the pages that show them follow.
    {
        let weak = ui.as_weak();
        draw::start(move || {
            if let Some(ui) = weak.upgrade() {
                with_state(|s| {
                    todo::refresh(&ui, s);
                    info::refresh(&ui, s);
                    cal::refresh(&ui, s);
                    quick::refresh(&ui, s);
                });
            }
        });
    }
    // While the start-up picture still covers the screen, every page is drawn
    // once (one per frame), then the picture fades. At most 1.5 s.
    let intro = slint::Timer::default();
    let weak = ui.as_weak();
    let warm_started = Instant::now();
    let warm_next = std::cell::Cell::new(0i32);
    intro.start(slint::TimerMode::Repeated, Duration::from_millis(20), move || {
        let Some(ui) = weak.upgrade() else { return };
        let n = ui.get_pages().row_count() as i32;
        let k = warm_next.get();
        if k < n && warm_started.elapsed() < Duration::from_millis(1500) {
            ui.set_warm_page(if k == ui.get_page() { -1 } else { k });
            warm_next.set(k + 1);
            return;
        }
        if ui.get_intro() {
            ui.set_warm_page(-1);
            if warm_started.elapsed() >= Duration::from_millis(250) {
                ui.set_intro(false);
            }
        }
    });
    let intro_end = slint::Timer::default();
    let weak = ui.as_weak();
    // The picture goes once it has faded (500 ms after the warm-up).
    intro_end.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || {
        let Some(ui) = weak.upgrade() else { return };
        if ui.get_intro() || !ui.get_intro_visible() {
            return;
        }
        thread_local!(static FADE_FROM: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) });
        let from = FADE_FROM.with(|f| match f.get() {
            Some(t) => t,
            None => {
                let t = Instant::now();
                f.set(Some(t));
                t
            }
        });
        if from.elapsed() < Duration::from_millis(650) {
            return;
        }
        ui.set_intro_visible(false);
        if let Some(pid) = splash {
            // SAFETY: plain kill(2).
            unsafe { libc::kill(pid, libc::SIGTERM) };
            let _ = std::fs::remove_file(SPLASH_PIDFILE);
        }
    });

    let phrase = wake_phrase(&settings.wake_words);
    let device = shared.device.clone();
    let (device_notify, device_rx) = mpsc::channel::<()>();
    let mic_muted = Arc::new(std::sync::atomic::AtomicBool::new(settings.mic_muted));
    let wake_words = Arc::new(std::sync::Mutex::new(settings.wake_words.clone()));
    let player = Arc::new(voice::Player::new());
    // the Sendspin player for Music Assistant (mixes into the same speaker); its volume is the panel's volume
    {
        let weak = ui.as_weak();
        sendspin::spawn(player.clone(), settings.volume, move |percent| {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                with_state(|s| {
                    if percent == 0 && s.settings.volume > 0 {
                        s.vol_before_mute = s.settings.volume;
                    }
                    s.settings.volume = percent;
                    hw::set_volume(percent);
                    s.settings.save();
                    let _ = s.device_notify.send(());
                });
                ui.set_speaker_muted(percent == 0);
            });
        });
    }
    hw::set_volume(settings.volume);
    ui.set_speaker_muted(settings.volume == 0);
    let ringing_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    ui.set_mic_muted(settings.mic_muted);
    ui.set_lightning(settings.lightning);
    // the drops on the glass: how much of the screen shows a photo page with nothing over it, and whether a stroke
    // there may wipe
    {
        let (see, wet) = (ui.as_weak(), ui.as_weak());
        glass::connect(
            move || see.upgrade().map_or(0.0, |ui| ui.get_glass_k()),
            move |b| {
                if let Some(ui) = wet.upgrade() {
                    ui.set_glass_wet(b);
                }
            },
        );
    }
    ui.set_voice_assistant("".into());
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    STATE.with(|s| {
        *s.borrow_mut() = Some(UiState {
            config: config.as_ref().ok().cloned(),
            auth: auth.clone(),
            shared: shared.clone(),
            commands: None,
            settings,
            states: HashMap::new(),
            pairing_started: false,
            dashboards: Vec::new(),
            connected: false,
            ever_connected: false,
            offline_since: None,
            offline_text: String::new(),
            screensaver: false,
            saver_auto: false,
            statuses: Vec::new(),
            critical: None,
            alert_strip: false,
            backlight: hw::Backlight::open(),
            light: hw::LightSensor::open(),
            proximity: hw::ProximityWake::default(),
            proximity_test: None,
            brightness: 600.0,
            brightness_target: 600.0,
            lux: None,
            dark: None,
            theme_pending: None,
            device: device.clone(),
            device_notify,
            last_reported_lux: None,
            last_report: Instant::now() - Duration::from_secs(120),
            soc_temp_avg: None,
            wifi_avg: None,
            web: None,
            web_swipe: None,
            touch_test: false,
            touch_test_step: 0,
            touch_test_manual: false,
            touch_offsets: Vec::new(),
            touch_result: None,
            touch_close_at: None,
            touch_started: None,
            touch_from_settings: false,
            touch_test_down: None,
            web_pull: None,
            web_pulled: None,
            timers: HashMap::new(),
            timer_seq: 0,
            dismissed_timers: Default::default(),
            laps: Vec::new(),
            lap_at: 0.0,
            ringing: None,
            snooze: None,
            ringing_alarm: None,
            ringing_flag: ringing_flag.clone(),
            ring_tone: String::new(),
            looping: None,
            music_asked: None,
            mic_muted: mic_muted.clone(),
            wake_words: wake_words.clone(),
            player: player.clone(),
            voice_hide_at: None,
            panel_config: {
                let stored = settings::load_panel_config();
                SAVED_PANEL_CONFIG.with(|k| *k.borrow_mut() = stored.clone());
                stored
            },
            linked_at: None,
            held_states: HashMap::new(),
            forecast_hourly: Vec::new(),
            forecast_daily: Vec::new(),
            wake_phrase: phrase,
            wake_hint: Default::default(),
            wake_hint_at: None,
            muted_hint: String::new(),
            theme_mode_seen: None,
            arts: HashMap::new(),
            art_tx: None,
            show_until: None,
            strip_until: None,
            last_lux_report: Instant::now(),
            music_hold: None,
            music_skip: None,
            digits: Default::default(),
            prev_digits: Default::default(),
            flip_at: None,
            weather_refreshed: Instant::now(),
            dev_screens: false,
            dnd: None,
            dev_config: None,
            temp_page: None,
            alarm_edit: None,
            alarm_minute: 0,
            screen_off: false,
            weather_key: String::new(),
            wx_cond: String::new(),
            island_seen: Vec::new(),
            island_urgent: Vec::new(),
            vol_before_mute: 0,
            dev_wx: None,
        })
    });
    // The look before the first frame (auto theme: the light sensor now), and the time format.
    with_state(|s| {
        s.lux = s.light.lux();
        update_theme(&ui, s);
        clock::set_h24(s.settings.time_24h);
        ui.global::<Clock>().set_h24(s.settings.time_24h);
    });
    apply_panel_config(&ui);
    update_clock(&ui);

    // Voice satellite and ESPHome native API (settings, sensors, Assist).
    let (to_api_tx, to_api_rx) = mpsc::channel::<voice::ToApi>();
    let (from_api_tx, from_api_rx) = mpsc::channel::<voice::FromApi>();
    let link = esphome::VoiceLink { to_api: to_api_rx, from_api: from_api_tx, active_wake_words: wake_words.clone() };
    let weak = ui.as_weak();
    // Only Home Assistant (and app.json api_allow) may use the ESPHome API.
    let api_config = config.as_ref().ok().cloned();
    let api_peers = move || {
        api_config.as_ref().map(|c| api_link::allowed_peers(&c.ha_host_port(), &c.api_allow)).unwrap_or_default()
    };
    esphome::spawn(device, device_rx, link, api_peers, move |control| {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            apply_control(&ui, control);
        });
    });
    // NSPANEL_NO_VOICE: no microphone, wake words or player (measurements).
    if std::env::var_os("NSPANEL_NO_VOICE").is_none() {
        let weak = ui.as_weak();
        voice::spawn(
            to_api_tx,
            from_api_rx,
            player.clone(),
            voice::VoiceControl {
                muted: mic_muted.clone(),
                ringing: with_state(|s| s.ringing_flag.clone()),
                active_wake_words: wake_words.clone(),
                ha_url: with_state(|s| s.config.as_ref().map(|c| c.ha_url.clone()).unwrap_or_default()),
            },
            move |event| {
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        handle_voice_ui(&ui, event);
                    }
                });
            },
        );
        // Wi-Fi roam check only while nothing is heard, said or played; during music when the music stalls.
        let speaking = player.busy.clone();
        let ringing = ringing_flag.clone();
        let quiet = move || !voice::busy() && !speaking.load(Ordering::SeqCst) && !ringing.load(Ordering::SeqCst);
        let quiet_music = quiet.clone();
        roam::spawn(
            // idle: no voice run, no sound, no ringing, no Sendspin music (a roam drops the stream for a moment)
            move || quiet() && !sendspin::music_active(),
            // the music broke up within the last minute, nothing else going on
            move || quiet_music() && sendspin::stalled_within(std::time::Duration::from_secs(60)),
            || HA_LINKED.load(Ordering::SeqCst),
        );
    }

    // UI callbacks
    home_edit::init(&ui);
    let weak = ui.as_weak();
    ui.on_act(move |what, value| {
        if let Some(ui) = weak.upgrade() {
            handle_act(&ui, what.as_str(), value.as_str());
        }
    });
    let weak = ui.as_weak();
    ui.global::<Act>().on_act(move |what, value| {
        if let Some(ui) = weak.upgrade() {
            handle_act(&ui, what.as_str(), value.as_str());
        }
    });
    let weak = ui.as_weak();
    ui.on_page_changed(move |now| {
        platform::note_activity();
        // Pages that fetch while on screen (cameras) start or stop.
        if let Some(ui) = weak.upgrade() {
            close_page_sheets(&ui);
            note_shown_page(&ui);
            synth::page_changed();
            leave_temporary_page(&ui, now);
            with_state(|s| cam::refresh(&ui, s));
            reroll_lines(ui.as_weak());
        }
    });
    let weak = ui.as_weak();
    ui.on_theme_mode_changed(move |m| {
        if let Some(ui) = weak.upgrade() {
            on_setting(&ui, "theme", match m {
                0 => "light",
                1 => "dark",
                _ => "auto",
            });
        }
    });

    // Timers: clock, sensors/backlight/screensaver, dev commands.
    let clock_timer = slint::Timer::default();
    let weak = ui.as_weak();
    clock_timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
        if let Some(ui) = weak.upgrade() {
            update_clock(&ui);
        }
    });
    let ticker = slint::Timer::default();
    let weak = ui.as_weak();
    ticker.start(slint::TimerMode::Repeated, Duration::from_millis(500), move || {
        if let Some(ui) = weak.upgrade() {
            tick(&ui);
        }
    });
    let fader = slint::Timer::default();
    fader.start(slint::TimerMode::Repeated, Duration::from_millis(20), fade_backlight);
    let dev_timer = slint::Timer::default();
    let weak = ui.as_weak();
    dev_timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
        let dev: Vec<String> = std::mem::take(&mut *platform::DEV_COMMANDS.lock().unwrap());
        for line in dev {
            let (Some(ui), Ok(v)) = (weak.upgrade(), serde_json::from_str::<Value>(&line)) else { continue };
            if let Some(show) = v.get("show") {
                handle_show(&ui, show);
            }
            if let Some(a) = v.get("act").and_then(Value::as_array) {
                handle_act(&ui, a.first().and_then(Value::as_str).unwrap_or(""), a.get(1).and_then(Value::as_str).unwrap_or(""));
            }
            if let Some(p) = v.get("page").and_then(Value::as_i64) {
                ui.set_page(p as i32);
            }
            // Development: a notification change as if Home Assistant sent it
            // ({"notes": {"kind": "added", "notes": {"id": {"title": …, "message": …}}}}).
            if let Some(n) = v.get("notes") {
                notes_changed(&ui, n["kind"].as_str().unwrap_or("added"), &n["notes"]);
            }
            // Development: the system screens on demand, frame statistics.
            if let Some(b) = v.get("connecting").and_then(Value::as_bool) {
                with_state(|s| s.dev_screens = true);
                ui.set_connecting(b);
            }
            if let Some(c) = v.get("offline").and_then(Value::as_i64) {
                with_state(|s| s.dev_screens = true);
                ui.set_offline_cause(c as i32);
            }
            if let Some(list) = v.get("pages").and_then(Value::as_array) {
                // "kind" or "kind:look" (e.g. "music:3", the cassette)
                let slots: Vec<PageSlot> = list
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|k| {
                        let (kind, look) = k.split_once(':').unwrap_or((k, "0"));
                        pages::plain_slot(kind, look.parse().unwrap_or(0))
                    })
                    .collect();
                // each page of a kind its own row of the kind's data (the rows past the integration's pages show
                // that kind's defaults)
                let mut seen: HashMap<String, i32> = HashMap::new();
                let slots: Vec<PageSlot> = slots
                    .into_iter()
                    .map(|mut p| {
                        let n = seen.entry(p.kind.to_string()).or_insert(0);
                        p.n = *n;
                        *n += 1;
                        p
                    })
                    .collect();
                ui.set_page(0);
                ui.set_pages(ModelRc::new(VecModel::from(slots)));
                // the looks' pictures (turntable deck, cassette shell) follow the new list
                refresh_pages(&ui);
            }
            if let Some(o) = v.get("wx") {
                with_state(|s| s.dev_wx = o.as_object().map(|_| o.clone()));
                refresh_pages(&ui);
            }
            // Development: integration settings merged over the current ones
            // (e.g. {"config": {"pages": [...]}}), as if Home Assistant sent them.
            if let Some(c) = v.get("config").filter(|c| c.is_object()) {
                with_state(|s| {
                    let mut dev = s.dev_config.clone().unwrap_or_else(|| json!({}));
                    if let (Some(d), Some(n)) = (dev.as_object_mut(), c.as_object()) {
                        for (k, val) in n {
                            d.insert(k.clone(), val.clone());
                        }
                    }
                    s.dev_config = Some(dev.clone());
                    overlay(&mut s.panel_config, &dev);
                });
                apply_panel_config(&ui);
            }
            // Development: the screensaver at once ({"saver": true}; a tap or "saver.wake" ends it)
            if v.get("saver").and_then(Value::as_bool) == Some(true) {
                with_state(|s| start_screensaver(&ui, s));
            }
            if v.get("dev_screens").and_then(Value::as_bool) == Some(false) {
                with_state(|s| s.dev_screens = false);
            }
            if let Some(b) = v.get("pair").and_then(Value::as_bool) {
                ui.set_paired(!b);
            }
            if let Some(b) = v.get("dark").and_then(Value::as_bool) {
                ui.global::<Theme>().set_dark(b);
            }
            if v.get("patrol").and_then(Value::as_bool) == Some(true) {
                synth::patrol_now();
            }
            // Development: the clocks turn calm after this many seconds without activity ({"calm_after": 600})
            if let Some(n) = v.get("calm_after").and_then(Value::as_u64) {
                platform::CALM_AFTER_S.store(n, Ordering::SeqCst);
            }
            // Development: counts as a music stall now ({"music_stall": true}; the roam check reacts within 5 s)
            if v.get("music_stall").and_then(Value::as_bool) == Some(true) {
                sendspin::note_stall();
            }
            if let Some(n) = v.get("cars_test").and_then(Value::as_u64) {
                synth::test_cars(&ui, n as usize);
            }
            #[cfg(feature = "gpu")]
            if let Some(b) = v.get("frame_stats").and_then(Value::as_bool) {
                gpu::FRAME_STATS.store(b, Ordering::SeqCst);
            }
            // Development: the glass drops' GPU pass off / on ({"glass_pass": false}, for measurements)
            if let Some(b) = v.get("glass_pass").and_then(Value::as_bool) {
                glass::set_pass(b);
                ui.window().request_redraw();
            }
            if let Some(a) = v.get("assist").and_then(Value::as_array) {
                ui.set_voice_heard(a.get(1).and_then(Value::as_str).unwrap_or("").into());
                ui.set_voice_answer(a.get(2).and_then(Value::as_str).unwrap_or("").into());
                let st = a.first().and_then(Value::as_i64).unwrap_or(0) as i32;
                // the look from the integration's settings, as a real voice run takes it; the lines, the meter and the
                // follow-up drain as in a real run (nothing left over from an earlier turn)
                voice_view(&ui, st.max(1));
                ui.set_voice_detail("".into());
                let place = match st { 2 => "think", 3 => "reply", 4 => "fail", 5 => "more", 6 => "again", _ => "listen" };
                ui.set_voice_line(voice_line(&ui, place).into());
                if st == 3 {
                    type_answer(&ui, a.get(2).and_then(Value::as_str).unwrap_or(""));
                } else {
                    ui.set_voice_typed("".into());
                }
                if st == 2 {
                    THINK_T0.with(|t| t.set(Some(Instant::now())));
                }
                ui.set_voice_state(st.max(1));
                follow_up_drain(&ui, st == 5);
                voice_meter(&ui, st > 0);
                ui.set_voice_on(st > 0);
            }
            // Measurements: another page in the web view, or a scripted scroll
            // of the page's largest scrolling element (no touch, no click).
            if let Some(url) = v.get("web_load").and_then(Value::as_str) {
                if let Some(view) = with_state(|s| s.web.clone()) {
                    view.dev(&format!("LOAD {url}"));
                }
            }
            if let Some(list) = v.get("web_skip").and_then(Value::as_array) {
                let paths = list.iter().filter_map(Value::as_str).map(str::to_string).collect();
                if let Some(view) = with_state(|s| s.web.clone()) {
                    view.set_skip(paths);
                }
            }
            if v.get("web_scroll_find").is_some() {
                if let Some(view) = with_state(|s| s.web.clone()) {
                    view.dev("SCROLLFIND");
                }
            }
            if let Some(a) = v.get("web_scroll").and_then(Value::as_array) {
                let px = a.first().and_then(Value::as_i64).unwrap_or(300);
                let ms = a.get(1).and_then(Value::as_u64).unwrap_or(400);
                if let Some(view) = with_state(|s| s.web.clone()) {
                    webstats::gesture("scripted scroll", webstats::now_us() + ms * 1000);
                    view.dev(&format!("SCROLLTEST {px} {ms}"));
                }
            }
        }
    });

    // the Wi-Fi screens (settings → Wi-Fi, the setup screen's Wi-Fi card)
    wifi::init(&ui);

    let (config, auth) = match (config, auth) {
        (Ok(c), Some(a)) => (c, a),
        (result, _) => {
            let e = result.err().unwrap_or_else(|| "no login store".into());
            eprintln!("nspanel-app: not configured ({e}): setup screen");
            ui.set_paired(true);
            start_setup(&ui, mdns_browser, &e);
            ui.run()?;
            return Ok(());
        }
    };
    ui.set_paired(auth.is_paired());
    ui.set_connecting(auth.is_paired());
    ui.set_pair_ha_url(config.ha_url.clone().into());

    with_state(|s| s.commands = Some(cmd_tx));
    // the pages as they were (the stored settings), before Home Assistant answers
    if with_state(|s| s.panel_config.is_some()) {
        eprintln!("nspanel-app: pages from the stored settings");
        apply_panel_config(&ui);
    }

    // The Home Assistant client started before the display: hand its events
    // to the UI thread, the ones from the wait first, in order.
    if let Some((events, buffered)) = ha_early {
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            for event in buffered.into_iter().chain(events.iter()) {
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        handle_event(&ui, event);
                    }
                });
            }
        });
    }

    let weak = ui.as_weak();
    // uploaded sounds are fetched from Home Assistant with the panel's login
    sounds::init(config.ha_url.clone(), auth.clone());
    // the panel as a media player in Home Assistant (alarms with music, Music Assistant)
    with_state(|s| media::init(s.device.clone(), s.device_notify.clone(), s.player.clone(), config.ha_url.clone()));
    let art_tx = artwork::spawn(config.ha_url.clone(), auth.clone(), move |player, img| {
        // RGBA in this thread: the renderer takes it without converting. The blurred copy: the cover shrunk
        // to a few pixels and softened (the GPU scales it up smoothly).
        let bg = img.as_ref().map(|b| cam::rgba(&cam::soft(&cam::shrink(b, 18))));
        let tint = img.as_ref().and_then(|b| cam::cover_accent(&cam::shrink(b, 32)));
        let record = img.as_ref().map(|b| cam::cover_record(&cam::shrink(b, 32)));
        if let (Some(t), Some(r)) = (tint, record) {
            eprintln!(
                "nspanel-app: cover colours: accent #{:02x}{:02x}{:02x}, record #{:02x}{:02x}{:02x}",
                t.0, t.1, t.2, r.0, r.1, r.2
            );
        }
        let img = img.map(|b| cam::rgba(&b));
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                with_state(|s| {
                    let art = s.arts.entry(player).or_default();
                    art.img = img.map(slint::Image::from_rgba8);
                    art.bg = bg.map(slint::Image::from_rgba8);
                    art.accent = tint.map(|(r, g, b)| slint::Color::from_rgb_u8(r, g, b));
                    art.record = record.map(|(r, g, b)| slint::Color::from_rgb_u8(r, g, b));
                });
                refresh_pages(&ui);
            }
        });
    });
    with_state(|s| {
        s.art_tx = Some(art_tx);
        s.arts.clear();
    });
    refresh_pages(&ui);

    // NSPANEL_NO_WEB: no web view (measurements).
    if web::available() && std::env::var_os("NSPANEL_NO_WEB").is_none() {
        // Only the newest frame matters: one UI job at a time takes it.
        type Latest = Option<(slint::SharedPixelBuffer<slint::Rgba8Pixel>, webstats::FrameTimes)>;
        let latest: Arc<std::sync::Mutex<Latest>> = Arc::default();
        let weak = ui.as_weak();
        let view = web::WebView::start(
            auth.clone(),
            Box::new(move |frame, times| {
                if latest.lock().unwrap().replace((frame, times)).is_some() {
                    return;
                }
                let latest = latest.clone();
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let (Some(ui), Some((frame, times))) = (weak.upgrade(), latest.lock().unwrap().take()) {
                        webstats::frame_on_ui(times);
                        ui.set_web_frame(slint::Image::from_rgba8(frame));
                        ui.set_web_ready(true);
                    }
                });
            }),
        );
        // Loading screen: shown while a page loads, faded and removed when it is ready.
        let weak = ui.as_weak();
        view.on_loading(move |on| {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                if on {
                    WEB_LOADING_AT.with(|t| t.set(Some(Instant::now())));
                }
                if on {
                    ui.set_web_loading(true);
                }
                if on {
                    // Off in the options: Home Assistant's own start-up shows instead.
                    if with_state(|s| s.settings.web.loading_screen) {
                        loading_text::start(&ui);
                        ui.set_web_overlay(true);
                    }
                } else {
                    // ready: the loading screen fades, but not before its minimum time after the dashboard came on
                    let wait = WEB_MIN_UNTIL.with(|m| m.take()).map_or(Duration::ZERO, |u| u.saturating_duration_since(Instant::now()));
                    let weak = ui.as_weak();
                    slint::Timer::single_shot(wait, move || {
                        let Some(ui) = weak.upgrade() else { return };
                        if WEB_LOADING_AT.with(|t| t.get()).is_some_and(|t| t.elapsed() < wait) {
                            return;
                        }
                        ui.set_web_loading(false);
                        loading_text::stop();
                        let weak = ui.as_weak();
                        slint::Timer::single_shot(Duration::from_millis(500), move || {
                            if let Some(ui) = weak.upgrade() {
                                if !ui.get_web_loading() {
                                    ui.set_web_overlay(false);
                                }
                            }
                        });
                    });
                    return;
                }
            });
        });
        let weak = ui.as_weak();
        let changed_view = view.clone();
        let release_view = view.clone();
        platform::set_web_direct(platform::WebDirect {
            // Straight to the screen only while nothing is drawn over the web view.
            allowed: Box::new(move || {
                let Some(ui) = weak.upgrade() else { return false };
                ui.get_web_shown()
                    && ui.get_web_ready()
                    && !(ui.get_web_overlay()
                        || ui.get_web_pull() > 0.0
                        || ui.get_shade_pos() > 0.002
                        || ui.get_touch_test()
                        || ui.get_set_view() > 0
                        || ui.get_screensaver_on()
                        || ui.get_strip_on()
                        || ui.get_pop_on()
                        || ui.get_voice_on()
                        || ui.get_timers_ringing()
                        || ui.get_setup()
                        || ui.get_prox_test()
                        || ui.get_prox_cal_on()
                        || ui.get_child_lock_open()
                        || ui.get_intro_visible())
            }),
            changed: Box::new(move |on| changed_view.set_direct(on)),
            release: Box::new(move |list| {
                for i in list {
                    release_view.dev(&format!("FREE {i}"));
                }
            }),
        });
        with_state(|s| {
            s.web = Some(view);
            update_web(&ui, s);
        });
        ui.set_web_active(true);
        apply_panel_config(&ui);
    }
    let weak = ui.as_weak();
    ui.on_touch_test_event(move |kind, x, y| {
        let Some(ui) = weak.upgrade() else { return };
        platform::note_activity();
        let step = with_state(|s| {
            match kind {
                0 => s.touch_test_down = Some((Instant::now(), x, y, 0)),
                1 => {
                    if let Some(d) = s.touch_test_down.as_mut() {
                        d.3 += 1;
                    }
                }
                _ => {
                    let Some((at, x0, y0, moves)) = s.touch_test_down.take() else { return None };
                    // done: the result stays until the view closes
                    if s.touch_test_step >= 9 {
                        return None;
                    }
                    let held = at.elapsed();
                    let i = s.touch_test_step.min(8);
                    let (tx, ty) = TOUCH_TEST_POINTS[i];
                    let (dx, dy) = (x0 - tx, y0 - ty);
                    eprintln!(
                        "nspanel-app: touch test: point {} ({tx:.0},{ty:.0}) down ({x0:.0},{y0:.0}) up ({x:.0},{y:.0}) off ({dx:+.0},{dy:+.0}) {:.0} px, held {} ms, {moves} moves",
                        i + 1,
                        (dx * dx + dy * dy).sqrt(),
                        held.as_millis()
                    );
                    // only a touch near the point counts (a tap anywhere else is not a measurement)
                    if !touch_test_hit((dx, dy)) {
                        return Some((s.touch_test_step, false));
                    }
                    if s.touch_test_step < 8 || (s.touch_test_step == 8 && held >= TOUCH_TEST_HOLD) {
                        s.touch_test_step += 1;
                        // measured with the correction in place: the offset left over adds to it
                        s.touch_offsets.push((dx, dy));
                    }
                    if s.touch_test_step == 9 && s.touch_test_manual {
                        finish_touch_test(s);
                    }
                    return Some((s.touch_test_step, true));
                }
            }
            None
        });
        if let Some((step, hit)) = step {
            ui.set_touch_test_step(step as i32);
            ui.set_touch_test_text(if hit { touch_test_text(step) } else { touch_test_miss_text(step) }.into());
        }
    });

    let weak = ui.as_weak();
    ui.on_web_touch(move |kind, x, y| {
        let Some(ui) = weak.upgrade() else { return };
        // Inside with_state only the decision; UI callbacks run after it.
        // The top edge (24 px) belongs to the quick settings swipe and never
        // reaches the page; a pull down from below it, on content scrolled to
        // its top, reloads the page (pull to refresh).
        let (swiped, pulled) = with_state(|s| {
            let Some(view) = s.web.clone() else { return (false, None) };
            match kind {
                0 if y < 24.0 => s.web_swipe = Some(y),
                1 if s.web_swipe.is_some() => {
                    if y - s.web_swipe.unwrap_or(y) > 60.0 {
                        s.web_swipe = None;
                        return (true, None);
                    }
                }
                2 if s.web_swipe.is_some() => s.web_swipe = None,
                0 => {
                    view.touch('d', x, y);
                    view.check_top(x, y);
                    s.web_pull = Some((x, y));
                    s.web_pulled = None;
                }
                1 => {
                    view.touch('m', x, y);
                    if let Some((x0, y0)) = s.web_pull {
                        let (dx, dy) = ((x - x0).abs(), y - y0);
                        if s.web_pulled.is_none() && dy > PULL_START && dy > 2.0 * dx && view.at_top() == Some(true) {
                            s.web_pulled = Some(0.0);
                        } else if s.web_pulled.is_none() && (dy < -PULL_START || dx > PULL_START) {
                            // Scrolling or sideways: not a pull.
                            s.web_pull = None;
                        }
                        if s.web_pulled.is_some() {
                            s.web_pulled = Some(((dy - PULL_START) / PULL_DISTANCE).clamp(0.0, 1.0));
                        }
                    }
                    return (false, s.web_pulled);
                }
                _ => {
                    view.touch('u', x, y);
                    if s.web_pulled.is_some_and(|p| p >= 1.0) {
                        WEB_COVER.with(|c| c.set(true));
                        view.reload();
                    }
                    s.web_pull = None;
                    s.web_pulled = None;
                    return (false, Some(0.0));
                }
            }
            (false, None)
        });
        if WEB_COVER.with(|c| c.replace(false)) {
            cover_web(&ui);
        }
        if let Some(p) = pulled {
            ui.set_web_pull(p);
        }
        if swiped {
            shade::open(&ui);
        }
    });

    let _ = VERSION;
    ui.run()?;
    Ok(())
}

#[cfg(test)]
mod timers_settings_tests {
    use super::{set_option, toggle_function};
    use serde_json::json;

    #[test]
    fn a_function_toggles_but_one_stays() {
        assert_eq!(toggle_function([true, true, true], "alarm"), Some([false, true, true]));
        assert_eq!(toggle_function([false, true, false], "stopwatch"), Some([false, true, true]));
        assert_eq!(toggle_function([false, true, false], "timer"), None);
        assert_eq!(toggle_function([true, true, true], "radio"), None);
    }

    #[test]
    fn an_option_lands_in_the_page() {
        let mut page = json!({ "kind": "timers" });
        set_option(&mut page, "scene", json!(2));
        assert_eq!(page["options"]["scene"], json!(2));
        set_option(&mut page, "colourful", json!(false));
        assert_eq!(page["options"], json!({ "scene": 2, "colourful": false }));
    }
}

#[cfg(test)]
mod music_skip_tests {
    use super::{skip_step, SkipStep};

    #[test]
    fn a_skip_never_flips_back() {
        // Next pressed on "A": the player still says "A", then "B", then "A" again for a moment, then "B"
        assert_eq!(skip_step("A", "A", false), SkipStep::Waiting);
        assert_eq!(skip_step("A", "B", false), SkipStep::NewTrack);
        assert_eq!(skip_step("A", "A", true), SkipStep::OldAgain);
        assert_eq!(skip_step("A", "B", true), SkipStep::NewTrack);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_test_counts_only_near_the_point() {
        assert!(touch_test_hit((0.0, 0.0)));
        // 40 px away: still the point
        assert!(touch_test_hit((24.0, -32.0)));
        assert!(!touch_test_hit((30.0, 30.0)));
        assert!(!touch_test_hit((200.0, 0.0)));
    }

    #[test]
    fn a_page_set_on_the_panel_changes_that_page_only() {
        let mut config = json!({ "pages": [
            { "kind": "quick", "variant": 0, "entities": ["light.a"], "options": { "groups": "off" } },
            { "kind": "quick", "variant": 1, "entities": ["light.b"] },
        ] });
        // the second quick controls page's look and an option (the menu on that page)
        let page = &mut config["pages"][1];
        apply_page_set(page, Some(2), &json!({ "groups": "custom", "tapes": ["DESK"] }));
        assert_eq!(config["pages"][1]["variant"], 2);
        assert_eq!(config["pages"][1]["options"]["groups"], "custom");
        assert_eq!(config["pages"][1]["options"]["tapes"], json!(["DESK"]));
        // the first one keeps its own
        assert_eq!(config["pages"][0]["variant"], 0);
        assert_eq!(config["pages"][0]["options"]["groups"], "off");
        // null takes an option away; a music page's player picked on the panel is its entity
        let page = &mut config["pages"][0];
        apply_page_set(page, None, &json!({ "groups": null, "entities": ["light.c"] }));
        assert!(config["pages"][0]["options"].get("groups").is_none());
        assert_eq!(config["pages"][0]["entities"], json!(["light.c"]));
    }

    fn hold(cfg: usize, kind: &str, variant: Option<i32>, options: Value, ago: Duration) -> Option<HeldPageSet> {
        let at = Instant::now().checked_sub(ago)?;
        Some(HeldPageSet { cfg, kind: kind.into(), variant, options, at })
    }

    #[test]
    fn a_player_picked_on_the_panel_holds_over_older_settings() {
        PAGE_SETS.with(|p| p.borrow_mut().clear());
        // "Play on" on the second music page (the list's third page): b instead of a
        let held = hold(2, "music", None, json!({ "entities": ["media_player.b"] }), Duration::ZERO).unwrap();
        PAGE_SETS.with(|p| p.borrow_mut().push(held));
        let older = || {
            Some(json!({ "pages": [
                { "kind": "home", "entities": ["media_player.a"] },
                { "kind": "music", "entities": ["media_player.a"] },
                { "kind": "music", "entities": ["media_player.a"] },
            ] }))
        };
        // a copy from before the integration had it (or one a new connection got first): the pick stays, on its page only
        let mut config = older();
        keep_page_sets(&mut config);
        assert_eq!(config.as_ref().unwrap()["pages"][2]["entities"], json!(["media_player.b"]));
        assert_eq!(config.as_ref().unwrap()["pages"][1]["entities"], json!(["media_player.a"]));
        assert_eq!(config.as_ref().unwrap()["pages"][0]["entities"], json!(["media_player.a"]));
        assert_eq!(PAGE_SETS.with(|p| p.borrow().len()), 1);
        // the integration's settings with it: nothing is held any more, a later change there counts again
        let mut config = older();
        config.as_mut().unwrap()["pages"][2]["entities"] = json!(["media_player.b"]);
        keep_page_sets(&mut config);
        assert_eq!(PAGE_SETS.with(|p| p.borrow().len()), 0);
        let mut config = older();
        keep_page_sets(&mut config);
        assert_eq!(config.as_ref().unwrap()["pages"][2]["entities"], json!(["media_player.a"]));
    }

    #[test]
    fn a_held_page_setting_goes_with_another_page_or_after_its_time() {
        PAGE_SETS.with(|p| p.borrow_mut().clear());
        let held = hold(1, "music", Some(3), Value::Null, Duration::ZERO).unwrap();
        PAGE_SETS.with(|p| p.borrow_mut().push(held));
        // the list changed in the integration: another kind is at that place now, it keeps its own look
        let mut config = Some(json!({ "pages": [{ "kind": "home" }, { "kind": "timers", "variant": 1 }] }));
        keep_page_sets(&mut config);
        assert_eq!(config.as_ref().unwrap()["pages"][1]["variant"], 1);
        assert_eq!(PAGE_SETS.with(|p| p.borrow().len()), 0);
        // an answer that never comes: Home Assistant's settings count again after the hold
        if let Some(old) = hold(1, "music", Some(3), Value::Null, PAGE_SET_HOLD + Duration::from_secs(1)) {
            PAGE_SETS.with(|p| p.borrow_mut().push(old));
            let mut config = Some(json!({ "pages": [{ "kind": "home" }, { "kind": "music", "variant": 0 }] }));
            keep_page_sets(&mut config);
            assert_eq!(config.as_ref().unwrap()["pages"][1]["variant"], 0);
            assert_eq!(PAGE_SETS.with(|p| p.borrow().len()), 0);
        }
    }

    #[test]
    fn a_page_carries_a_setting_by_value() {
        let page = json!({ "kind": "music", "variant": 3, "entities": ["media_player.b"], "options": { "scroll_speed": 3.0, "scroll": true } });
        assert!(page_carries(&page, Some(3), &json!({ "entities": ["media_player.b"] })));
        assert!(!page_carries(&page, Some(2), &Value::Null));
        assert!(!page_carries(&page, None, &json!({ "entities": ["media_player.a"] })));
        // 3 and 3.0 are the same setting; an option set to null is carried by its absence
        assert!(page_carries(&page, None, &json!({ "scroll_speed": 3, "cover_blur": null })));
        assert!(!page_carries(&page, None, &json!({ "scroll": null })));
        assert!(!page_carries(&page, None, &json!({ "holds": 1 })));
    }

    #[test]
    fn each_timers_page_has_its_functions_and_its_tab_among_them() {
        assert_eq!(timers_functions(None), [true, true, true]);
        let only_alarms = json!({ "options": { "functions": ["alarm"] } });
        assert_eq!(timers_functions(Some(&only_alarms)), [true, false, false]);
        // nothing chosen counts as all of them
        let none = json!({ "options": { "functions": [] } });
        assert_eq!(timers_functions(Some(&none)), [true, true, true]);
        // the tab stays while its function is on, else the timers, the alarms, the stopwatch
        assert_eq!(timers_tab(2, [true, true, true]), 2);
        assert_eq!(timers_tab(2, [true, true, false]), 1);
        assert_eq!(timers_tab(1, [true, false, true]), 0);
        assert_eq!(timers_tab(0, [false, false, true]), 2);
        // colourful by default, the page's own colours
        assert_eq!(timers_colours(None), (true, vec![]));
        let own = json!({ "options": { "colourful": false, "colours": ["#ff0000", "bad"] } });
        assert_eq!(timers_colours(Some(&own)), (false, vec![slint::Color::from_rgb_u8(255, 0, 0)]));
    }
}
