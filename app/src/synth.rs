//! The synthwave scenes (home background, timers theme,
//! thermostat look): which view (day, night, dawn) and the fade
//! between them by the sun, the floor colour, and a passing car now and then.
//! The clock, the floor lines, the palms and the cars' motion run in the UI.

use serde_json::Value;
use slint::{Color, ComponentHandle, ModelRc, VecModel};

use crate::{AppWindow, SwCar};

const SCENES: [&str; 4] = ["sunset", "city", "beach", "stars"];

/// The cars' neon colours, around the colour wheel: hot pink, neon red, sunset orange, sun yellow,
/// mint, cyan, electric blue, ultraviolet, violet, magenta, and chrome white.
const CAR_TINTS: [u32; 11] = [0xff2a6d, 0xff3b3b, 0xff8b3d, 0xffd319, 0x01ffc3, 0x00e5ff, 0x4d7cff, 0x8a5cff, 0xc86bfa, 0xff3df0, 0xeef0ff];
/// Colours too close to tell apart at a glance (by index into CAR_TINTS).
const CAR_TINT_NEAR: [(usize, usize); 7] = [(0, 1), (1, 2), (4, 5), (6, 7), (7, 8), (8, 9), (9, 0)];
/// How many of the last cars a colour (or one close to it) must not repeat within.
const CAR_TINT_GAP: usize = 5;

/// The next car's colour: never the colour of one of the last five cars or one close to it (so never
/// two alike on the screen either). Of the colours left, only one that still leaves a colour for the car
/// after it (with these colours that is always possible, so the rule always holds; the colour used
/// longest ago is only a safety net).
fn next_tint() -> usize {
    thread_local!(static RECENT: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) });
    let near = |a: usize, b: usize| a == b || CAR_TINT_NEAR.iter().any(|&(x, y)| (x, y) == (a, b) || (y, x) == (a, b));
    RECENT.with(|r| {
        let mut recent = r.borrow_mut();
        // cars ago this colour (or a close one) was last used; usize::MAX = not lately
        let ago = |t: usize, list: &[usize]| list.iter().rev().position(|&u| near(t, u)).unwrap_or(usize::MAX);
        let free = |list: &[usize]| -> Vec<usize> { (0..CAR_TINTS.len()).filter(|&t| ago(t, list) >= CAR_TINT_GAP).collect() };
        let mut options = free(&recent);
        let keeps_room: Vec<usize> = options
            .iter()
            .copied()
            .filter(|&c| {
                let mut then = recent.clone();
                then.push(c);
                !free(&then).is_empty()
            })
            .collect();
        if !keeps_room.is_empty() {
            options = keeps_room;
        }
        let pick = if options.is_empty() {
            (0..CAR_TINTS.len()).max_by_key(|&t| ago(t, &recent)).unwrap_or(0)
        } else {
            options[(random() % options.len() as u64) as usize]
        };
        recent.push(pick);
        let extra = recent.len().saturating_sub(CAR_TINT_GAP * 2);
        recent.drain(..extra);
        pick
    })
}

fn random() -> u64 {
    use std::cell::Cell;
    thread_local!(static SEED: Cell<u64> = Cell::new(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7) | 1
    ));
    SEED.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        x
    })
}

fn colour(hex: &str) -> Color {
    crate::pages::parse_color(hex).unwrap_or(Color::from_rgb_u8(0xff, 0x2b, 0xd6))
}

/// The floor colour of a view (colours.json of the scenes).
fn grid(scene: usize, view: &str) -> Color {
    thread_local!(static TABLE: Value = std::fs::read_to_string(
        std::path::Path::new(env!("PANELOS_UI_PICTURES")).join("home/fl/colours.json")
    ).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null));
    TABLE.with(|t| colour(t[SCENES[scene]][view][0].as_str().unwrap_or("#ff2bd6")))
}

/// A car's trip to the horizon (seconds): a random length in this range, so the speeds vary.
const CAR_TRIP: (f32, f32) = (8.0, 18.0);

/// The floor lines the cars drive along: the middle one and four on each side (the outer ones start partly off
/// screen and come in from a lower corner).
const LANES: [f32; 9] = [-4.0, -3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0];
/// The outermost lane on either side.
const MAX_LANE: f32 = 4.0;

/// The floor line under a tap on the road at (`x`, `y`): the lines are 43.8 / d + 2.2 px apart at the depth
/// d = 150 / (y - 330) of the tap (46 px at the bottom edge, closer towards the horizon).
fn lane_at(x: f32, y: f32) -> f32 {
    let d = 150.0 / (y - 330.0).max(1.0);
    let spacing = 43.8 / d + 2.2;
    ((x - 240.0) / spacing).round().clamp(-MAX_LANE, MAX_LANE)
}

/// A car's depth at a share of its trip (as the scene draws it: 0.46 under the front edge … 60).
fn car_depth(share: f32) -> f32 {
    0.46 * (60.0f32 / 0.46).powf(share.clamp(0.0, 1.0))
}

/// Whether a car on `line`, starting at `born` with a trip of `dur`, never touches `other`: either it
/// never reaches it, or where it passes it their lanes are far enough apart at that depth (the widest
/// car body is 46 of its 48 sprite pixels, 176 px wide at depth 1; lanes are 43.8 / d + 2.2 apart; 10 %
/// to spare).
fn clear_of(line: f32, born: f32, dur: f32, other: &SwCar) -> bool {
    // the same share of the trip = the same depth: when (if ever) the new car comes level
    if dur >= other.dur {
        return born >= other.born;
    }
    let t = (born * other.dur - other.born * dur) / (other.dur - dur);
    let share = (t - other.born) / other.dur;
    if share > 1.0 || t < born {
        return true;
    }
    let d = car_depth(share);
    let apart = (line - other.line).abs() * (43.8 / d + 2.2);
    apart >= 1.1 * (46.0 * 176.0 / 48.0) / d
}

/// The trip for a new car on `line`: the wanted one when it touches no car on the road, else just
/// slower (1 s more) than each car it would touch; None when that is longer than the longest trip.
fn fitting_trip(line: f32, now: f32, want: f32, alive: &[SwCar]) -> Option<f32> {
    let mut dur = want;
    for _ in 0..4 {
        match alive.iter().find(|c| !clear_of(line, now, dur, c)) {
            Some(c) => dur = dur.max(c.dur - (now - c.born) + 1.0),
            None => return (dur <= CAR_TRIP.1).then_some(dur),
        }
    }
    alive.iter().all(|c| clear_of(line, now, dur, c)).then_some(dur).filter(|d| *d <= CAR_TRIP.1)
}

/// How far behind the car it follows the patrol car starts, as a share of their trip (0.3-0.6 s was too
/// close; 0.7-1.1 s still overlapped near the viewer). From 15 % on, the patrol car's
/// roof stays below the car ahead, so it never covers it; with the same trip it keeps this distance.
const PATROL_BEHIND: (f32, f32) = (0.15, 0.18);

/// The patrol car's kind (black and white with a light bar; never an ordinary car).
const PATROL: i32 = 5;

/// A synthwave scene is on the screen now (the page shown, not under the screensaver).
fn on_screen(ui: &AppWindow, s: &crate::UiState) -> bool {
    if s.screensaver {
        return false;
    }
    let pages = ui.get_pages();
    let Some(p) = slint::Model::row_data(&pages, ui.get_page().max(0) as usize) else { return false };
    let theme = if p.theme >= 0 { p.theme } else { ui.get_weather().theme };
    ((p.kind == "home" || p.kind == "weather") && theme == 8) || (p.kind == "timers" && p.variant == 3) || (p.kind == "thermostat" && p.variant == 3)
}

/// The page that chooses the panel's synthwave scene (its place in the integration's list): the first custom or weather
/// details page with the synthwave background of its own, else the first custom page.
fn scene_page(config: Option<&Value>) -> Option<usize> {
    let pages = crate::pages::config_pages(config);
    let kind = |p: &Value| p.get("kind").and_then(Value::as_str).map(str::to_string);
    pages
        .iter()
        .find(|(_, p)| {
            matches!(kind(p).as_deref(), Some("home" | "weather")) && p.get("options").and_then(|o| o.get("theme")).and_then(Value::as_i64) == Some(8)
        })
        .or_else(|| pages.iter().find(|(_, p)| kind(p).as_deref() == Some("home")))
        .map(|(i, _)| *i)
}

/// The synthwave scene of the panel (one for every page with the synthwave background): the scene of the page that
/// chooses it (`scene_page`); else the first custom page's, else the old panel setting.
fn scene_of(config: Option<&Value>) -> usize {
    let option = |i: usize| config?.get("pages")?.get(i)?.get("options")?.get("scene")?.as_u64();
    let first_home = crate::pages::kind_pages(config, "home").first().map(|(i, _)| *i);
    scene_page(config)
        .and_then(option)
        .or_else(|| first_home.and_then(option))
        .or_else(|| config.and_then(|c| c.get("fl_scene")).and_then(Value::as_u64))
        .unwrap_or(0)
        .min(3) as usize
}

fn scene_choice(s: &crate::UiState) -> usize {
    scene_of(s.panel_config.as_ref())
}

/// A scene picked on the panel (the synthwave background's menu, the thermostat's neon look): set on the page that
/// chooses the scene, so it shows at once on every page with the synthwave background; Home Assistant keeps it.
pub fn set_scene(ui: &AppWindow, k: i64) {
    let Some(cfg) = crate::with_state(|s| scene_page(s.panel_config.as_ref())) else { return };
    crate::page_set_at(ui, cfg, None, serde_json::json!({ "scene": k.clamp(0, 3) }));
    crate::refresh_pages(ui);
}

thread_local! {
    /// The patrol car's clock: seconds a synthwave scene was on screen since the last one, and when
    /// the next one is due (a random 20-30 minutes of that).
    static PATROL_CLOCK: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((0.0, -1.0)) };
}

/// Counts a second of synthwave on screen (called once a second); true when the patrol car is due.
fn patrol_clock(shown: bool) -> bool {
    PATROL_CLOCK.with(|c| {
        let (mut on, mut due) = c.get();
        if due < 0.0 {
            due = 1200.0 + (random() % 601) as f32;
        }
        if shown && on < due {
            on += 1.0;
        }
        c.set((on, due));
        on >= due
    })
}

/// Development (load test): `n` cars on the road at once, spread out from near to far, the last one
/// the patrol car just behind the one before it on a neighbouring line. With more cars than lines two
/// may share a line (at different depths).
pub fn test_cars(ui: &AppWindow, n: usize) {
    let n = n.clamp(1, 20);
    let now = ui.get_scene_t();
    let lanes = [-4.0f32, 4.0, -2.0, 2.0, 0.0, -3.0, 3.0, -1.0, 1.0];
    let mut cars: Vec<SwCar> = Vec::new();
    for k in 0..n {
        let dur = 14.0;
        if k == n - 1 && k > 0 {
            let lead = cars[k - 1].clone();
            let line = if lead.line < MAX_LANE { lead.line + 1.0 } else { lead.line - 1.0 };
            cars.push(SwCar { line, kind: PATROL, tint: Color::from_rgb_u8(255, 255, 255), born: lead.born + lead.dur * PATROL_BEHIND.0, dur: lead.dur, ..Default::default() });
            break;
        }
        let t = CAR_TINTS[next_tint()];
        cars.push(SwCar {
            line: lanes[k % lanes.len()],
            kind: (random() % PATROL as u64) as i32,
            tint: Color::from_rgb_u8((t >> 16) as u8, (t >> 8) as u8, t as u8),
            // the first is the farthest: born earliest
            born: now - (n - 1 - k) as f32 * 1.1,
            dur,
            ..Default::default()
        });
    }
    cars.sort_by(|a, b| a.born.total_cmp(&b.born));
    set_cars(ui, cars);
}

/// Development: the patrol car is due now (it joins the next ordinary car).
pub fn patrol_now() {
    PATROL_CLOCK.with(|c| c.set((1.0, 0.0)));
}

/// The patrol car set off: the next one comes after another 20-30 minutes of synthwave on screen.
fn patrol_went() {
    PATROL_CLOCK.with(|c| c.set((0.0, 1200.0 + (random() % 601) as f32)));
}

/// The scene clock's step (seconds): a whole number of the scenes' 40 s loops.
const SCENE_WRAP_S: f32 = 1200.0;

thread_local! {
    /// Each timers page's scene while its option says "random": the scene, whether the page has been shown with it,
    /// and the scene clock's ticks since the page was last shown or a swipe moved the pages.
    static TIMERS_RANDOM: std::cell::RefCell<Vec<(usize, bool, u32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

thread_local! {
    /// The timers pages' scene pictures (loaded once each).
    static TIMERS_SCENES: std::cell::RefCell<std::collections::HashMap<String, slint::Image>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The next random scene: any of the other three (never the same twice in a row).
fn next_random_scene(current: usize, roll: u64) -> usize {
    (current + 1 + (roll % 3) as usize) % 4
}

/// The pages moved (a swipe settled on another page): the timers page's next scene waits until it is out of sight.
pub fn page_changed() {
    TIMERS_RANDOM.with(|r| {
        for (_, _, away) in r.borrow_mut().iter_mut() {
            *away = 0;
        }
    });
}

/// The timers page's random scene (called once a second). A new one is picked while the page is out of sight, once it
/// has been shown with the last one: two ticks after it went (by then a swipe's slide has settled) and with no finger
/// moving the pages, so however the page is opened (a swipe, a timer on the island, the status bar's mark, a ring) it
/// comes with its new scene from the first frame, and the scene never changes while the page is in view.
fn timers_random_scene(ui: &AppWindow, n: usize) -> usize {
    let pages = ui.get_pages();
    let showing = slint::Model::row_data(&pages, ui.get_page().max(0) as usize).is_some_and(|p| p.kind == "timers" && p.n == n as i32);
    let dragging = ui.get_dragging();
    TIMERS_RANDOM.with(|r| {
        let mut r = r.borrow_mut();
        while r.len() <= n {
            r.push((random() as usize % 4, false, 0));
        }
        let (mut scene, mut seen, mut away) = r[n];
        if showing || dragging {
            seen |= showing;
            away = 0;
        } else {
            away = away.saturating_add(1);
            if seen && away >= 2 {
                scene = next_random_scene(scene, random());
                seen = false;
            }
        }
        r[n] = (scene, seen, away);
        scene
    })
}

/// The views' blend by the sun: `mix` puts the night view over the day view (day above +3°, night below -6°, a fade
/// between); `dawn` puts the dawn view over the night view while the sun rises from -12° to -6° (only while it rises,
/// never in the evening). From -6° on, the dawn view fades into the day together with the night view.
fn views(el: f64, rising: bool) -> (f32, f32) {
    let mix = ((3.0 - el) / 9.0).clamp(0.0, 1.0) as f32;
    let dawn = if rising { ((el + 12.0) / 6.0).clamp(0.0, 1.0) as f32 } else { 0.0 };
    (mix, dawn)
}

/// The view's colour (the floor lines, the widgets' accent and their glow): day, night and dawn, mixed like the
/// pictures.
fn view_colour(scene: usize, mix: f32, dawn: f32) -> Color {
    let lerp_c = |a: Color, b: Color, k: f32| {
        let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
        Color::from_rgb_u8(l(a.red(), b.red()), l(a.green(), b.green()), l(a.blue(), b.blue()))
    };
    let day = grid(scene, "day");
    if mix <= 0.0 {
        return day;
    }
    let night = grid(scene, "night");
    let night = if dawn > 0.0 { lerp_c(night, grid(scene, "dawn"), dawn) } else { night };
    lerp_c(day, night, mix)
}

/// Whether a shown blend value is already `want`: within `tol`, and exactly 0 or 1 when that is wanted (so a view
/// that is gone is not drawn at a trace of opacity).
fn blend_is(shown: f32, want: f32, tol: f32) -> bool {
    if want == 0.0 || want == 1.0 {
        return shown == want;
    }
    (shown - want).abs() <= tol
}

/// Sets the views' blend, the view's colour and the shooting stars (at night, and at any time in space). Each only
/// when it changed: a set redraws every page with a scene (the scenes live on pages out of sight too).
fn show_views(ui: &AppWindow, scene: usize, mix: f32, dawn: f32, tol: f32) {
    if !blend_is(ui.get_home_fl_mix(), mix, tol) {
        ui.set_home_fl_mix(mix);
    }
    if !blend_is(ui.get_home_fl_mix_c(), dawn, tol) {
        ui.set_home_fl_mix_c(dawn);
    }
    let g = view_colour(scene, mix, dawn);
    if ui.get_home_fl_grid() != g {
        ui.set_home_fl_grid(g);
    }
    let star = if scene == 3 { 1.0 } else { mix };
    if !blend_is(ui.get_home_fl_star(), star, tol) {
        ui.set_home_fl_star(star);
    }
}

/// How long a new sun value takes to come in on screen: 75 whole 40 ms frames (3 s), straight, without easing.
const GLIDE_FRAMES: u32 = 75;
const GLIDE_FRAME_MS: u64 = 40;

/// The views' glide: from which blend (mix, dawn) to which, for which scene, and the frames done.
#[derive(Clone, Copy, Default)]
struct Glide {
    from: (f32, f32),
    to: (f32, f32),
    scene: usize,
    step: u32,
}

thread_local! {
    static GLIDE: slint::Timer = slint::Timer::default();
    static GLIDE_PATH: std::cell::Cell<Glide> = std::cell::Cell::new(Glide::default());
}

/// The blend `step` frames into a glide of GLIDE_FRAMES frames from `from` to `to`.
fn glide_at(from: (f32, f32), to: (f32, f32), step: u32) -> (f32, f32) {
    let k = (step as f32 / GLIDE_FRAMES as f32).min(1.0);
    (from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k)
}

/// A scene on screen takes a new blend in a glide from the one shown (the sun's values come minutes apart, each a
/// step). A glide on its way keeps going to the same value, or starts again from where it is to a new one; the
/// timer runs only while it glides.
fn glide_to(ui: &AppWindow, scene: usize, mix: f32, dawn: f32) {
    let to = (mix, dawn);
    let running = GLIDE.with(|t| t.running());
    let path = GLIDE_PATH.with(|p| p.get());
    if running && path.to == to && path.scene == scene {
        return;
    }
    let from = (ui.get_home_fl_mix(), ui.get_home_fl_mix_c());
    if !running && blend_is(from.0, mix, 0.004) && blend_is(from.1, dawn, 0.004) {
        show_views(ui, scene, mix, dawn, 0.004);
        return;
    }
    GLIDE_PATH.with(|p| p.set(Glide { from, to, scene, step: 0 }));
    let weak = ui.as_weak();
    GLIDE.with(|t| {
        t.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(GLIDE_FRAME_MS), move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut g = GLIDE_PATH.with(|p| p.get());
            g.step += 1;
            GLIDE_PATH.with(|p| p.set(g));
            let (m, d) = glide_at(g.from, g.to, g.step);
            show_views(&ui, g.scene, m, d, 0.0);
            if g.step >= GLIDE_FRAMES {
                GLIDE.with(|t| t.stop());
            }
        })
    });
}

/// Updates the scenes that are in use (once a second).
pub fn tick(ui: &AppWindow, s: &crate::UiState) {
    // any page on the list with a synthwave look (a kind may be there more than once)
    let pages = ui.get_pages();
    let any = |f: &dyn Fn(&crate::PageSlot) -> bool| (0..slint::Model::row_count(&pages)).filter_map(|i| slint::Model::row_data(&pages, i)).any(|p| f(&p));
    let home = ui.get_weather().theme == 8 || any(&|p| (p.kind == "home" || p.kind == "weather") && p.theme == 8);
    let timers = any(&|p| p.kind == "timers" && p.variant == 3);
    let thermo = any(&|p| p.kind == "thermostat" && p.variant == 3);
    if !home && !timers && !thermo {
        return;
    }
    let scene = scene_choice(s);
    // The view by the sun (see `views`); the development input's weather values may set the sun
    // ({"wx": {"sun_el": -8, "rising": true}}).
    let sun = s.states.get("sun.sun");
    let dev = |k: &str| s.dev_wx.as_ref().and_then(|o| o.get(k));
    let el = dev("sun_el").and_then(Value::as_f64).or_else(|| sun.and_then(|e| e.attrs.get("elevation")).and_then(Value::as_f64)).unwrap_or(30.0);
    let rising = dev("rising").and_then(Value::as_bool).or_else(|| sun.and_then(|e| e.attrs.get("rising")).and_then(Value::as_bool)).unwrap_or(false);
    let (mix, dawn) = views(el, rising);
    let pic = |view: &str, floor: bool| {
        crate::quick::picture(&format!("home/fl/fl-{scene}-{view}{}.png", if floor { "-floor" } else { "" }))
    };
    let new_scene = ui.get_home_fl_scene() != scene as i32 || ui.get_home_fl_back_a().size().width == 0;
    if new_scene {
        ui.set_home_fl_scene(scene as i32);
        ui.set_home_fl_back_a(pic("day", false));
        ui.set_home_fl_floor_a(pic("day", true));
        ui.set_home_fl_back_b(pic("night", false));
        ui.set_home_fl_floor_b(pic("night", true));
        // every scene's dawn view (drawn only while the dawn shows)
        ui.set_home_fl_back_c(pic("dawn", false));
        ui.set_home_fl_floor_c(pic("dawn", true));
    }
    // each synthwave timers page's own scene (its option; -1 = a new random one every time the page is opened; none =
    // the home page's)
    let tpages: Vec<serde_json::Value> = crate::pages::kind_pages(s.panel_config.as_ref(), "timers").into_iter().map(|(_, p)| p.clone()).collect();
    for (n, page) in tpages.iter().enumerate() {
        if crate::pages::variant_of(page) != 3 {
            continue;
        }
        let tscene = match crate::pages::option_of(page, "scene").and_then(|v| v.as_i64()) {
            Some(-1) => timers_random_scene(ui, n),
            Some(k) => k.clamp(0, 3) as usize,
            None => scene,
        };
        let pic = |part: &str| TIMERS_SCENES.with(|m| {
            m.borrow_mut().entry(format!("{tscene}-{part}")).or_insert_with(|| crate::quick::picture(&format!("timers/sw/scene-{tscene}-{part}.png"))).clone()
        });
        let (back, floor) = (pic("back"), pic("floor"));
        crate::pages::with_inst(ui.get_timers_inst(), |m| ui.set_timers_inst(m), n, |d| {
            d.sw_scene = tscene as i32;
            d.sw_back = back;
            d.sw_floor = floor;
        });
    }
    // The views' blend: a new scene, or one out of sight, takes it at once; on screen a new sun value glides there
    // (`glide_to`).
    let shown = on_screen(ui, s);
    if new_scene || !shown {
        GLIDE.with(|g| g.stop());
        show_views(ui, scene, mix, dawn, 0.004);
    } else {
        glide_to(ui, scene, mix, dawn);
    }
    // The scene clock, the palms and the cars only while a scene is on screen: the scene time comes from the
    // animation clock, and every read of it asks for a frame (a read every second redrew every page once a
    // second). Cars whose trip ended meanwhile are out of sight; the next tick on screen clears them.
    if !shown {
        return;
    }
    // The scene clock moves its base on by 20 minutes (30 times the 40 s loop of the floor, the
    // palms and the star) and the car on the road with it, in the same step: nothing jumps.
    let cars = ui.get_home_fl_cars();
    let st = ui.get_scene_t();
    if st > SCENE_WRAP_S {
        // (several steps at once when no scene ran for a long time)
        let shift = (st / SCENE_WRAP_S).floor() * SCENE_WRAP_S;
        ui.set_scene_base(ui.get_scene_base() + shift as i64 * 1000);
        for i in 0..slint::Model::row_count(&cars) {
            if let Some(mut c) = slint::Model::row_data(&cars, i) {
                c.born -= shift;
                slint::Model::set_row_data(&cars, i, c);
            }
        }
        ui.set_palm_gust_at(ui.get_palm_gust_at() - shift);
    }
    // The palms' gusts: the next one 6-14 s after the last started, 1.6-2.0x the calm sway, held 1-3 s;
    // between gusts the calm is 0.6-0.8x.
    let st = ui.get_scene_t();
    let at = ui.get_palm_gust_at();
    // (scheduled once the right palm, 0.22 s behind, has settled too)
    if st > at + 3.8 + ui.get_palm_gust_hold() {
        let next = st + 6.0 + (random() % 800) as f32 / 100.0;
        ui.set_palm_gust_at(next);
        ui.set_palm_gust_peak(1.6 + (random() % 40) as f32 / 100.0);
        ui.set_palm_gust_hold(1.0 + (random() % 200) as f32 / 100.0);
        ui.set_palm_calm_before(ui.get_palm_calm());
        ui.set_palm_calm(0.6 + (random() % 20) as f32 / 100.0);
    }
    // The cars: each on its own line (never two on one), random kind and vivid colour, a random 8–18 s
    // to the horizon, so their speeds differ. A faster one may overtake a slower one only where the two
    // never touch (see clear_of); else it is made a little slower. The traffic setting (the
    // integration's synthwave page) only sets how often a new one starts: light = one at a time (a new
    // one when the last is gone), more = up to 2, busy = up to 3, a new one once the newest is a third /
    // a fifth of its way.
    let traffic = crate::first_page_option(s, "home", "traffic").and_then(|v| v.as_u64()).unwrap_or(0).min(2) as usize;
    let now = ui.get_scene_t();
    let all: Vec<SwCar> = (0..slint::Model::row_count(&cars)).filter_map(|i| slint::Model::row_data(&cars, i)).collect();
    // a shoved car lost up to 0.64 s of its trip (spec 25.H5)
    let alive: Vec<SwCar> = all.iter().filter(|c| now - c.born <= c.dur + 0.5 + if c.pushed { 0.7 } else { 0.0 }).cloned().collect();
    // the patrol car rides along with another one: the traffic rules count the ordinary cars
    let ordinary: Vec<&SwCar> = alive.iter().filter(|c| c.kind != PATROL).collect();
    let most = [1usize, 2, 3][traffic];
    let gap = [1.0f32, 0.34, 0.2][traffic];
    let newest = ordinary.iter().max_by(|a, b| a.born.total_cmp(&b.born));
    let room = ordinary.len() < most && newest.is_none_or(|c| traffic > 0 && (now - c.born) / c.dur >= gap);
    let patrol_due = patrol_clock(shown);
    let mut changed = alive.len() != all.len();
    let mut next = alive.clone();
    if room {
        let last_line = all.iter().max_by(|a, b| a.born.total_cmp(&b.born)).map(|c| c.line).unwrap_or(9.0);
        let choices: Vec<f32> = LANES.iter().copied().filter(|l| *l != last_line && !alive.iter().any(|c| c.line == *l)).collect();
        // a random trip; a faster car may overtake a slower one only where the two would not touch
        // (lanes far enough apart at the depth where it passes); otherwise it is made just slower than
        // that one, and when that would need a trip over the longest, it waits
        let line = if choices.is_empty() { 9.0 } else { choices[(random() % choices.len() as u64) as usize] };
        let want = CAR_TRIP.0 + (random() % 1001) as f32 / 1000.0 * (CAR_TRIP.1 - CAR_TRIP.0);
        let trip = fitting_trip(line, now, want, &alive);
        if let (false, Some(dur)) = (choices.is_empty(), trip) {
            let t = CAR_TINTS[next_tint()];
            // at most two cars of one shape at a time (the patrol car never comes as an ordinary one)
            let kinds: Vec<i32> = (0..PATROL).filter(|k| alive.iter().filter(|c| c.kind == *k).count() < 2).collect();
            next.push(SwCar {
                line,
                kind: kinds[(random() % kinds.len() as u64) as usize],
                tint: Color::from_rgb_u8((t >> 16) as u8, (t >> 8) as u8, t as u8),
                born: now,
                dur,
                ..Default::default()
            });
            // the patrol car, when due: just behind this one (15-18 % of their trip later, the same trip, so
            // the same pace) on a neighbouring free line; never two at once
            if patrol_due && !alive.iter().any(|c| c.kind == PATROL) {
                let born = now + dur * (PATROL_BEHIND.0 + (random() % 101) as f32 / 100.0 * (PATROL_BEHIND.1 - PATROL_BEHIND.0));
                // a free neighbouring line where, at the leader's pace, it touches no other car
                let side: Vec<f32> = [line - 1.0, line + 1.0]
                    .into_iter()
                    .filter(|l| l.abs() <= MAX_LANE && !alive.iter().any(|c| c.line == *l))
                    .filter(|l| alive.iter().all(|c| clear_of(*l, born, dur, c)))
                    .collect();
                if !side.is_empty() {
                    next.push(SwCar {
                        line: side[(random() % side.len() as u64) as usize],
                        kind: PATROL,
                        tint: Color::from_rgb_u8(255, 255, 255),
                        born,
                        dur,
                        ..Default::default()
                    });
                    patrol_went();
                }
            }
            changed = true;
        }
    }
    if changed {
        // far ones first (the newest is the nearest: drawn last, on top)
        next.sort_by(|a, b| a.born.total_cmp(&b.born));
        set_cars(ui, next);
    }
}

/// The cars on the road, far ones first. Row by row in the one shared model: a car that leaves or
/// comes only takes or adds its own row, so the scene does not build every car anew (a whole new
/// model cost a 30-80 ms frame each time one car left).
fn set_cars(ui: &AppWindow, next: Vec<SwCar>) {
    use slint::Model;
    let cars = ui.get_home_fl_cars();
    let Some(vec) = cars.as_any().downcast_ref::<VecModel<SwCar>>() else {
        let model = ModelRc::new(VecModel::from(next));
        ui.set_home_fl_cars(model.clone());
        ui.set_timers_sw_cars(model);
        return;
    };
    // the cars that went (by their birth: each car has its own), then the new ones in their place
    let mut i = 0;
    while i < vec.row_count() {
        let keep = vec.row_data(i).is_some_and(|c| next.iter().any(|n| n.born == c.born && n.line == c.line));
        if keep {
            i += 1;
        } else {
            vec.remove(i);
        }
    }
    for (k, c) in next.into_iter().enumerate() {
        let here = vec.row_data(k).is_some_and(|r| r.born == c.born && r.line == c.line);
        if !here {
            vec.insert(k.min(vec.row_count()), c);
        }
    }
}

thread_local! {
    /// When a tap last asked for a car (spec 25.H4: at most one every 3 s).
    static ASKED: std::cell::Cell<f32> = const { std::cell::Cell::new(-100.0) };
}

/// Taps on the synthwave home background (spec 25.H4 / H5): `synth.push` "born:side" shoves that car (a fresh shove while
/// it skids), `synth.lane` "x:y" asks for a car on the floor line under the tap.
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    use slint::Model;
    let cars = ui.get_home_fl_cars();
    let Some(vec) = cars.as_any().downcast_ref::<VecModel<SwCar>>() else {
        return;
    };
    let now = ui.get_scene_t();
    match what {
        "synth.push" => {
            let mut it = value.split(':');
            let born: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(-1.0);
            let dir: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(1.0);
            for i in 0..vec.row_count() {
                if let Some(mut c) = vec.row_data(i) {
                    if (c.born - born).abs() < 0.001 {
                        c.pushed = true;
                        c.push_t = now;
                        c.push_dir = if dir < 0.0 { -1.0 } else { 1.0 };
                        vec.set_row_data(i, c);
                        eprintln!("nspanel-app: synthwave car shoved");
                    }
                }
            }
        }
        "synth.lane" => {
            if now - ASKED.with(|a| a.get()) < 3.0 {
                return;
            }
            let mut at = value.split(':').map(|v| v.parse::<f32>().ok());
            let x = at.next().flatten().unwrap_or(240.0);
            let y = at.next().flatten().unwrap_or(480.0);
            let line = lane_at(x, y);
            let all: Vec<SwCar> = (0..vec.row_count()).filter_map(|i| vec.row_data(i)).collect();
            let alive: Vec<SwCar> = all.iter().filter(|c| now - c.born <= c.dur + 0.5).cloned().collect();
            if alive.iter().any(|c| c.line == line) {
                return;
            }
            let want = CAR_TRIP.0 + (random() % 1001) as f32 / 1000.0 * (CAR_TRIP.1 - CAR_TRIP.0);
            let Some(dur) = fitting_trip(line, now, want, &alive) else {
                return;
            };
            ASKED.with(|a| a.set(now));
            let t = CAR_TINTS[next_tint()];
            let mut next = alive;
            next.push(SwCar {
                line,
                kind: (random() % PATROL as u64) as i32,
                tint: Color::from_rgb_u8((t >> 16) as u8, (t >> 8) as u8, t as u8),
                born: now,
                dur,
                ..Default::default()
            });
            next.sort_by(|a, b| a.born.total_cmp(&b.born));
            set_cars(ui, next);
            eprintln!("nspanel-app: synthwave car asked for (lane {line})");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_lanes_from_minus_four_to_four() {
        assert_eq!(LANES.len(), 9);
        assert_eq!(LANES.first().copied(), Some(-MAX_LANE));
        assert_eq!(LANES.last().copied(), Some(MAX_LANE));
        assert!(LANES.windows(2).all(|w| w[1] - w[0] == 1.0));
    }

    #[test]
    fn a_tap_at_the_bottom_picks_the_line_46_px_apart() {
        assert_eq!(lane_at(240.0, 480.0), 0.0);
        assert_eq!(lane_at(240.0 + 46.0, 480.0), 1.0);
        assert_eq!(lane_at(240.0 - 2.0 * 46.0, 479.0), -2.0);
        // the outer lines and beyond: the outermost one
        assert_eq!(lane_at(240.0 + 4.0 * 46.0, 480.0), 4.0);
        assert_eq!(lane_at(470.0, 480.0), 4.0);
        assert_eq!(lane_at(5.0, 480.0), -4.0);
    }

    #[test]
    fn a_tap_further_up_uses_the_closer_lines_there() {
        // y 380: d = 3, lines 16.8 px apart
        assert_eq!(lane_at(240.0 + 3.0 * 16.8, 380.0), 3.0);
        assert_eq!(lane_at(240.0 - 16.8, 380.0), -1.0);
        // at the horizon the tap still gives a line
        assert_eq!(lane_at(250.0, 330.0), 4.0);
    }

    #[test]
    fn a_random_scene_is_never_the_same_twice() {
        for current in 0..4 {
            let next: Vec<usize> = (0..3).map(|roll| next_random_scene(current, roll)).collect();
            assert!(next.iter().all(|&n| n != current && n < 4));
            // each of the other three can come
            let mut sorted = next.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), 3);
        }
    }

    #[test]
    fn the_views_follow_the_sun() {
        // full day above +3 degrees, full night below -6, a fade between
        assert_eq!(views(10.0, true), (0.0, 1.0));
        assert_eq!(views(3.0, false), (0.0, 0.0));
        assert_eq!(views(-6.0, false), (1.0, 0.0));
        assert!((views(-1.5, false).0 - 0.5).abs() < 1e-6);
        // the dawn view comes in over the night view while the sun rises from -12 to -6 degrees …
        assert_eq!(views(-20.0, true), (1.0, 0.0));
        assert_eq!(views(-12.0, true), (1.0, 0.0));
        assert!((views(-9.0, true).1 - 0.5).abs() < 1e-6);
        assert_eq!(views(-6.0, true), (1.0, 1.0));
        // … and fades into the day with the night view (drawn at mix * dawn)
        let (mix, dawn) = views(-1.5, true);
        assert!((mix * dawn - 0.5).abs() < 1e-6);
        // never in the evening
        assert_eq!(views(-9.0, false).1, 0.0);
        assert_eq!(views(-6.0, false).1, 0.0);
    }

    #[test]
    fn a_glide_is_straight_and_ends_on_the_value() {
        assert_eq!(GLIDE_FRAMES as u64 * GLIDE_FRAME_MS, 3000);
        let (from, to) = ((1.0, 0.0), (1.0, 0.6));
        assert_eq!(glide_at(from, to, 0), from);
        assert_eq!(glide_at(from, to, GLIDE_FRAMES), to);
        assert_eq!(glide_at(from, to, GLIDE_FRAMES + 5), to);
        // the same step every frame (no easing)
        let a = glide_at(from, to, 10).1 - glide_at(from, to, 9).1;
        let b = glide_at(from, to, 60).1 - glide_at(from, to, 59).1;
        assert!((a - b).abs() < 1e-6 && a > 0.0);
    }

    #[test]
    fn a_gone_view_is_exactly_gone() {
        assert!(!blend_is(0.003, 0.0, 0.004));
        assert!(blend_is(0.0, 0.0, 0.004));
        assert!(!blend_is(0.998, 1.0, 0.004));
        assert!(blend_is(0.502, 0.5, 0.004));
        assert!(!blend_is(0.502, 0.5, 0.0));
    }

    #[test]
    fn the_first_page_with_the_synthwave_background_chooses_the_scene() {
        let config = serde_json::json!({ "pages": [
            { "kind": "music" },
            { "kind": "home", "options": { "theme": 2, "scene": 1 } },
            { "kind": "weather", "options": { "theme": 8, "scene": 3 } },
            { "kind": "home", "options": { "theme": 8, "scene": 2 } },
        ] });
        assert_eq!(scene_page(Some(&config)), Some(2));
        assert_eq!(scene_of(Some(&config)), 3);
        // the custom page measured on the panel: the beach
        let config = serde_json::json!({ "pages": [
            { "kind": "home", "options": { "theme": 8, "scene": 2 } },
            { "kind": "weather", "options": {} },
            { "kind": "timers", "variant": 3 },
        ] });
        assert_eq!(scene_page(Some(&config)), Some(0));
        assert_eq!(scene_of(Some(&config)), 2);
    }

    #[test]
    fn without_a_scene_of_its_own_the_first_custom_page_chooses() {
        // the synthwave page has no scene: the first custom page's, where the panel's menus then set it
        let config = serde_json::json!({ "pages": [
            { "kind": "home", "options": { "scene": 1 } },
            { "kind": "weather", "options": { "theme": 8 } },
        ] });
        assert_eq!(scene_page(Some(&config)), Some(1));
        assert_eq!(scene_of(Some(&config)), 1);
        // no synthwave background at all (the thermostat's neon look): the first custom page
        let config = serde_json::json!({ "pages": [
            { "kind": "thermostat", "variant": 3 },
            { "kind": "home", "options": { "theme": 0, "scene": 3 } },
        ] });
        assert_eq!(scene_page(Some(&config)), Some(1));
        assert_eq!(scene_of(Some(&config)), 3);
        // the old panel setting, else the mountains
        assert_eq!(scene_of(Some(&serde_json::json!({ "fl_scene": 2 }))), 2);
        assert_eq!(scene_of(None), 0);
        assert_eq!(scene_page(None), None);
    }

    #[test]
    fn cars_on_far_apart_lines_pass_each_other() {
        // a slow car on -4 just set off; a fast one on +4 overtakes it without touching
        let slow = SwCar { line: -4.0, born: 0.0, dur: 18.0, ..Default::default() };
        assert!(clear_of(4.0, 0.5, 8.0, &slow));
        // on the same line it would run into it
        assert!(!clear_of(-4.0, 0.5, 8.0, &slow));
    }
}
