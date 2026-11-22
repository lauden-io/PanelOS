//! Cameras page: the camera entities picked for it, one live (frames through
//! Home Assistant's camera proxy while the page is on screen) or all as a
//! grid. The blurred background is the frame shrunk to a few pixels: the GPU
//! scales it up smoothly, which looks like a strong blur and costs nothing.

use std::sync::atomic::Ordering;

use slint::{Color, ComponentHandle, ModelRc, Rgb8Pixel, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::camera::{CameraControl, FULL, LIST, OFF};
use crate::{with_state, AppWindow, CamItem};

thread_local! {
    static CONTROL: std::cell::RefCell<Option<CameraControl>> = const { std::cell::RefCell::new(None) };
    /// All cameras: they take turns in the fetcher, one frame each.
    static GRID: slint::Timer = slint::Timer::default();
    /// The cameras taking turns (another cameras page's start the round again).
    static GRID_SET: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    /// Each cameras page's fit and start view as the integration had them last (a change there is taken over).
    static SEEN: std::cell::RefCell<Vec<(Option<String>, String)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// A camera Home Assistant showed that no cameras page has (its entity and its name there): the temporary cameras
    /// page shows it alone, in a row of its own after the cameras pages' rows. It goes with the temporary page.
    static GUEST: std::cell::RefCell<Option<(String, String)>> = const { std::cell::RefCell::new(None) };
}

/// Where Home Assistant's camera view shows `entity` among the cameras pages (`pages`: their places in the integration's
/// list and their settings, in order): the cameras page on screen (`on_screen`, its place in the list) when it has the
/// camera, else the first that has it, as (its place in the list, its row among the cameras pages, the camera's place
/// on the page). None when no cameras page has it.
pub fn page_with(pages: &[(usize, &serde_json::Value)], on_screen: Option<usize>, entity: &str) -> Option<(usize, usize, usize)> {
    let has = |n: usize| crate::pages::entities_of(pages[n].1).iter().position(|e| e == entity).map(|i| (pages[n].0, n, i));
    let on = (0..pages.len()).find(|&n| Some(pages[n].0) == on_screen);
    on.and_then(has).or_else(|| (0..pages.len()).find_map(has))
}

/// Home Assistant's camera view of `entity` (`name`: its name there): on the cameras page that has it (`page_with`) the
/// camera shows big, and that page's place in the integration's list comes back; a camera no cameras page has becomes
/// the temporary cameras page's camera (None).
pub fn show_camera(ui: &AppWindow, entity: &str, name: &str) -> Option<usize> {
    let (found, count) = with_state(|s| {
        let pages = crate::pages::kind_pages(s.panel_config.as_ref(), "camera");
        let slots = ui.get_pages();
        let on = slint::Model::row_data(&slots, ui.get_page().max(0) as usize)
            .filter(|p| p.kind == "camera" && p.cfg >= 0)
            .map(|p| p.cfg as usize);
        (page_with(&pages, on, entity), pages.len())
    });
    match found {
        Some((cfg, n, i)) => {
            crate::pages::with_inst(ui.get_cam_inst(), |m| ui.set_cam_inst(m), n, |d| {
                d.sel = i as i32;
                d.view = 0;
            });
            Some(cfg)
        }
        None => {
            let old = GUEST.with(|g| g.replace(Some((entity.to_string(), name.to_string()))));
            if old.is_none_or(|(e, _)| e != entity) {
                // a fresh row: no picture of another camera, the screen filled (a cameras page's default)
                crate::pages::with_inst(ui.get_cam_inst(), |m| ui.set_cam_inst(m), count, |d| {
                    *d = Default::default();
                    d.fit = fit_index("");
                });
            }
            None
        }
    }
}

/// The row of the cameras pages' data a temporary cameras page shows: the guest camera's (after the cameras pages'
/// rows) while there is one, else the first page's.
pub fn guest_row(s: &crate::UiState) -> i32 {
    if GUEST.with(|g| g.borrow().is_some()) {
        crate::pages::kind_pages(s.panel_config.as_ref(), "camera").len() as i32
    } else {
        0
    }
}

/// Starts the frame fetcher (once Home Assistant's address and login are known).
fn ensure_started(ui: &AppWindow, s: &crate::UiState) {
    if CONTROL.with(|c| c.borrow().is_some()) {
        return;
    }
    let (Some(url), Some(auth)) = (s.config.as_ref().map(|c| c.ha_url.clone()), s.auth.clone()) else { return };
    let control = CameraControl::default();
    let weak = ui.as_weak();
    let fetching = control.entity.clone();
    crate::camera::spawn(url, auth, control.clone(), move |frame| {
        // RGBA here, off the UI thread: the renderer takes it as it is.
        let small = shrink(&frame, 12);
        let frame = rgba(&frame);
        let entity = fetching.lock().unwrap().clone().unwrap_or_default();
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = weak.upgrade() {
                show_frame(&ui, &entity, frame, small);
            }
        });
    });
    CONTROL.with(|c| *c.borrow_mut() = Some(control));
}

/// A frame for the camera on screen: into its tile on the cameras page on screen (the only one the fetcher feeds), the
/// blur behind it and the corner colours.
fn show_frame(ui: &AppWindow, entity: &str, frame: SharedPixelBuffer<Rgba8Pixel>, small: SharedPixelBuffer<Rgb8Pixel>) {
    let n = crate::acting_n("camera");
    let entities = with_state(|s| crate::page_entities(s, "camera"));
    let d = crate::pages::inst(&ui.get_cam_inst(), n);
    let cams = d.items.clone();
    let sel = d.sel.max(0) as usize;
    let index = entities.iter().position(|e| e == entity).unwrap_or(sel);
    if let Some(mut c) = slint::Model::row_data(&cams, index) {
        c.frame = slint::Image::from_rgba8(frame);
        c.state = 0;
        slint::Model::set_row_data(&cams, index, c);
    }
    // the blurred copy belongs to the camera shown big (and the visitor alert's picture)
    if index != sel {
        return;
    }
    let ring = ui.get_cam_ring() && ui.get_cam_ring_n() == n as i32;
    if ring {
        if let Some(c) = slint::Model::row_data(&cams, index) {
            ui.set_cam_ring_frame(c.frame);
        }
    }
    let (w, h) = (small.width() as usize, small.height() as usize);
    let px = small.as_slice();
    let at = |x: usize, y: usize| {
        let p = px[y.min(h - 1) * w + x.min(w - 1)];
        Color::from_rgb_u8(p.r, p.g, p.b)
    };
    let corners = vec![at(0, 0), at(w - 1, 0), at(0, h - 1), at(w - 1, h - 1)];
    let bg = slint::Image::from_rgba8(rgba(&small));
    if ring {
        ui.set_cam_ring_bg(bg.clone());
    }
    crate::pages::with_inst(ui.get_cam_inst(), |m| ui.set_cam_inst(m), n, |d| {
        d.bg_corners = ModelRc::new(VecModel::from(corners));
        d.bg = bg;
    });
}

pub fn rgba(src: &SharedPixelBuffer<Rgb8Pixel>) -> SharedPixelBuffer<Rgba8Pixel> {
    let mut out = SharedPixelBuffer::<Rgba8Pixel>::new(src.width(), src.height());
    for (o, p) in out.make_mut_slice().iter_mut().zip(src.as_slice()) {
        *o = Rgba8Pixel { r: p.r, g: p.g, b: p.b, a: 255 };
    }
    out
}

/// A small picture softened (two 3x3 box passes, edges held): with the GPU's smooth upscaling it
/// reads as an even blur, without the blocky cross of a raw few-pixel picture.
pub fn soft(src: &SharedPixelBuffer<Rgb8Pixel>) -> SharedPixelBuffer<Rgb8Pixel> {
    let (w, h) = (src.width() as i32, src.height() as i32);
    let mut cur: Vec<[u32; 3]> = src.as_slice().iter().map(|p| [p.r as u32, p.g as u32, p.b as u32]).collect();
    for _ in 0..2 {
        let mut next = cur.clone();
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 3];
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let p = cur[((y + dy).clamp(0, h - 1) * w + (x + dx).clamp(0, w - 1)) as usize];
                        for c in 0..3 {
                            acc[c] += p[c];
                        }
                    }
                }
                next[(y * w + x) as usize] = [acc[0] / 9, acc[1] / 9, acc[2] / 9];
            }
        }
        cur = next;
    }
    let mut out = SharedPixelBuffer::<Rgb8Pixel>::new(w as u32, h as u32);
    for (o, p) in out.make_mut_slice().iter_mut().zip(cur) {
        *o = Rgb8Pixel { r: p[0] as u8, g: p[1] as u8, b: p[2] as u8 };
    }
    out
}

/// The frame scaled down to `n` pixels across (box filter).
/// The cover's own colour for the music page (the record, the accents): the strongest hue of a small copy,
/// weighted by how colourful and how common it is, brought to a lightness that reads on the dark deck; for a
/// grey cover its own neutral tone.
pub fn cover_accent(small: &SharedPixelBuffer<Rgb8Pixel>) -> Option<(u8, u8, u8)> {
    const BINS: usize = 24;
    let mut weight = [0f32; BINS];
    let mut sum = [[0f32; 3]; BINS];
    for p in small.as_slice() {
        let (r, g, b) = (p.r as f32 / 255.0, p.g as f32 / 255.0, p.b as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let chroma = max - min;
        if max < 0.12 || chroma < 0.08 {
            continue;
        }
        let hue = if max == r {
            ((g - b) / chroma).rem_euclid(6.0)
        } else if max == g {
            (b - r) / chroma + 2.0
        } else {
            (r - g) / chroma + 4.0
        } / 6.0;
        let bin = ((hue * BINS as f32) as usize).min(BINS - 1);
        let w = chroma * chroma * max;
        weight[bin] += w;
        sum[bin][0] += r * w;
        sum[bin][1] += g * w;
        sum[bin][2] += b * w;
    }
    // the best hue with its neighbours (a hue split over two bins still wins)
    let score = |i: usize| weight[i] + 0.5 * (weight[(i + BINS - 1) % BINS] + weight[(i + 1) % BINS]);
    let best = (0..BINS).max_by(|&a, &b| score(a).total_cmp(&score(b)))?;
    let total: f32 = weight.iter().sum();
    if weight[best] <= 0.0 || total < small.as_slice().len() as f32 * 0.002 {
        // a grey cover: its own neutral tone (the mean colour, as grey as it is), at a brightness that reads
        let n = small.as_slice().len().max(1) as f32;
        let (r, g, b) = small.as_slice().iter().fold((0f32, 0f32, 0f32), |(r, g, b), p| (r + p.r as f32, g + p.g as f32, b + p.b as f32));
        let (r, g, b) = (r / n / 255.0, g / n / 255.0, b / n / 255.0);
        let max = r.max(g).max(b).max(0.01);
        let k = max.clamp(0.55, 0.8) / max;
        let c = |v: f32| (v * k * 255.0).round().clamp(0.0, 255.0) as u8;
        return Some((c(r), c(g), c(b)));
    }
    let (r, g, b) = (sum[best][0] / weight[best], sum[best][1] / weight[best], sum[best][2] / weight[best]);
    // to HSV, then a saturation and brightness that read as a colour on the dark deck and the record
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let s = if max > 0.0 { (max - min) / max } else { 0.0 };
    let (s2, v2) = (s.max(0.5), max.clamp(0.62, 0.92));
    let k = |c: f32| {
        let t = if max > min { (c - min) / (max - min) } else { 0.0 };   // 0 at the smallest channel, 1 at the largest
        let c = v2 * (1.0 - s2 * (1.0 - t));
        (c * 255.0).round().clamp(0.0, 255.0) as u8
    };
    Some((k(r), k(g), k(b)))
}

/// The record's colour on the turntable: the cover's own colour as it is, black, white and grey too (the
/// strongest hue's mean where the cover has colours, else the cover's mean tone).
pub fn cover_record(small: &SharedPixelBuffer<Rgb8Pixel>) -> (u8, u8, u8) {
    const BINS: usize = 24;
    let mut weight = [0f32; BINS];
    let mut sum = [[0f32; 3]; BINS];
    let px = small.as_slice();
    for p in px {
        let (r, g, b) = (p.r as f32 / 255.0, p.g as f32 / 255.0, p.b as f32 / 255.0);
        let max = r.max(g).max(b);
        let chroma = max - r.min(g).min(b);
        if max < 0.12 || chroma < 0.08 {
            continue;
        }
        let hue = if max == r {
            ((g - b) / chroma).rem_euclid(6.0)
        } else if max == g {
            (b - r) / chroma + 2.0
        } else {
            (r - g) / chroma + 4.0
        } / 6.0;
        let bin = ((hue * BINS as f32) as usize).min(BINS - 1);
        let w = chroma * chroma * max;
        weight[bin] += w;
        sum[bin][0] += r * w;
        sum[bin][1] += g * w;
        sum[bin][2] += b * w;
    }
    let total: f32 = weight.iter().sum();
    let best = (0..BINS).max_by(|&a, &b| weight[a].total_cmp(&weight[b])).unwrap_or(0);
    let to8 = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    if weight[best] > 0.0 && total >= px.len() as f32 * 0.002 {
        let w = weight[best];
        return (to8(sum[best][0] / w), to8(sum[best][1] / w), to8(sum[best][2] / w));
    }
    let n = px.len().max(1) as f32;
    let (r, g, b) = px.iter().fold((0f32, 0f32, 0f32), |(r, g, b), p| (r + p.r as f32, g + p.g as f32, b + p.b as f32));
    ((r / n).round() as u8, (g / n).round() as u8, (b / n).round() as u8)
}

pub fn shrink(frame: &SharedPixelBuffer<Rgb8Pixel>, n: u32) -> SharedPixelBuffer<Rgb8Pixel> {
    let (w, h) = (frame.width(), frame.height());
    let (nw, nh) = (n.min(w).max(1), ((n * h) / w.max(1)).clamp(1, h.max(1)));
    let mut out = SharedPixelBuffer::<Rgb8Pixel>::new(nw, nh);
    let src = frame.as_slice();
    let dst = out.make_mut_slice();
    for oy in 0..nh {
        for ox in 0..nw {
            let (x0, x1) = (ox * w / nw, ((ox + 1) * w / nw).max(ox * w / nw + 1));
            let (y0, y1) = (oy * h / nh, ((oy + 1) * h / nh).max(oy * h / nh + 1));
            let (mut r, mut g, mut b, mut k) = (0u32, 0u32, 0u32, 0u32);
            for y in (y0..y1).step_by(2) {
                for x in (x0..x1).step_by(2) {
                    let p = src[(y * w + x) as usize];
                    r += p.r as u32;
                    g += p.g as u32;
                    b += p.b as u32;
                    k += 1;
                }
            }
            let k = k.max(1);
            dst[(oy * nw + ox) as usize] = Rgb8Pixel { r: (r / k) as u8, g: (g / k) as u8, b: (b / k) as u8 };
        }
    }
    out
}

/// Every cameras page from the states, each from its own cameras and options; frames only for the one on screen.
pub fn refresh(ui: &AppWindow, s: &mut crate::UiState) {
    let pages = crate::pages::kind_pages(s.panel_config.as_ref(), "camera").into_iter().map(|(cfg, p)| (cfg, p.clone())).collect::<Vec<_>>();
    // the guest camera goes with the temporary page
    let guest = GUEST.with(|g| {
        if s.temp_page.is_none() {
            g.replace(None);
        }
        g.borrow().clone()
    });
    // the cameras page on screen (only it gets frames)
    let slots = ui.get_pages();
    let on = slint::Model::row_data(&slots, ui.get_page().max(0) as usize).filter(|p| p.kind == "camera" && !ui.get_screensaver_on());
    let shown = on.as_ref().map(|p| p.n.max(0) as usize);
    let entities = match (shown, &guest) {
        (Some(n), Some((entity, _))) if n == pages.len() => vec![entity.clone()],
        _ => shown.and_then(|n| pages.get(n)).map(|(_, p)| crate::pages::entities_of(p)).unwrap_or_default(),
    };
    let d = crate::pages::inst(&ui.get_cam_inst(), shown.unwrap_or(0));
    let sel = (d.sel.max(0) as usize).min(entities.len().saturating_sub(1));
    let grid = shown.is_some() && d.view == 1 && entities.len() > 1;
    CONTROL.with(|c| {
        if let Some(c) = c.borrow().as_ref() {
            if !grid {
                *c.entity.lock().unwrap() = entities.get(sel).cloned();
            }
            c.mode.store(if shown.is_none() || entities.is_empty() { OFF } else if grid { LIST } else { FULL }, Ordering::SeqCst);
        }
    });
    GRID.with(|t| {
        // a new set of cameras (another cameras page) starts the round again
        let key = entities.join(",");
        let same = GRID_SET.with(|g| g.replace(key.clone()) == key);
        if !grid || !same {
            t.stop();
        }
        if grid && !t.running() {
            let list = entities.clone();
            let turn = std::cell::Cell::new(0usize);
            t.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(1100), move || {
                let next = turn.get() % list.len();
                turn.set(next + 1);
                CONTROL.with(|c| {
                    if let Some(c) = c.borrow().as_ref() {
                        *c.entity.lock().unwrap() = Some(list[next].clone());
                    }
                });
            });
        }
    });
    if guest.is_none() && pages.iter().all(|(_, p)| crate::pages::entities_of(p).is_empty()) {
        crate::pages::keep_insts(&ui.get_cam_inst(), pages.len());
        return;
    }
    ensure_started(ui, s);
    for (n, (_, page)) in pages.iter().enumerate() {
        let entities = crate::pages::entities_of(page);
        let option = |k: &str| crate::pages::option_of(page, k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        // the picture: whole (over its blurred copy) or filling the screen (the default), taken when it changes in the
        // integration (the page's settings set it on the panel at once); the start view ("one" / "all") the same
        let (fit, view) = (option("fit"), option("view"));
        let (fit_new, view_new) = SEEN.with(|seen| {
            let mut seen = seen.borrow_mut();
            if seen.len() <= n {
                seen.resize(n + 1, (None, String::new()));
            }
            let fit_new = seen[n].0.as_ref() != Some(&fit);
            let view_new = seen[n].1 != view && !view.is_empty();
            seen[n] = (Some(fit.clone()), view.clone());
            (fit_new, view_new)
        });
        crate::pages::with_inst(ui.get_cam_inst(), |m| ui.set_cam_inst(m), n, |d| {
            let rows: Vec<CamItem> = entities
                .iter()
                .enumerate()
                .map(|(i, id)| {
                    let st = s.states.get(id);
                    let state = match st.map(|e| e.state.as_str()).unwrap_or("unavailable") {
                        "unavailable" | "unknown" => 1,
                        _ => 0,
                    };
                    let old = slint::Model::row_data(&d.items, i);
                    CamItem {
                        name: st.and_then(|e| e.attrs.get("friendly_name")).and_then(serde_json::Value::as_str).unwrap_or(id).into(),
                        frame: old.map(|o| o.frame).unwrap_or_default(),
                        state,
                    }
                })
                .collect();
            if let Some(m) = crate::pages::update_rows(&d.items, rows) {
                d.items = m;
            }
            d.sel = (d.sel.max(0) as usize).min(entities.len().saturating_sub(1)) as i32;
            if fit_new {
                d.fit = fit_index(&fit);
            }
            if view_new {
                d.view = if view == "all" && entities.len() > 1 { 1 } else { 0 };
            }
        });
    }
    // the guest camera's row: its name as Home Assistant sent it (the panel does not follow its state), available
    // unless the panel knows otherwise
    if let Some((entity, name)) = &guest {
        crate::pages::with_inst(ui.get_cam_inst(), |m| ui.set_cam_inst(m), pages.len(), |d| {
            let st = s.states.get(entity);
            let row = CamItem {
                name: st
                    .and_then(|e| e.attrs.get("friendly_name"))
                    .and_then(serde_json::Value::as_str)
                    .or(Some(name.as_str()).filter(|n| !n.is_empty()))
                    .unwrap_or(entity)
                    .into(),
                frame: slint::Model::row_data(&d.items, 0).map(|o| o.frame).unwrap_or_default(),
                state: i32::from(matches!(st.map(|e| e.state.as_str()), Some("unavailable" | "unknown"))),
            };
            if let Some(m) = crate::pages::update_rows(&d.items, vec![row]) {
                d.items = m;
            }
            d.sel = 0;
        });
    }
    crate::pages::keep_insts(&ui.get_cam_inst(), pages.len() + usize::from(guest.is_some()));
    door_replies_state(ui, s);
    doorbell(ui, s, &pages);
}

/// The picture's setting as the page draws it: 0 the whole picture, 1 filling the screen (the default).
fn fit_index(fit: &str) -> i32 {
    if fit == "whole" { 0 } else { 1 }
}

/// The replies at the door when the integration has none set (the page's `replies` option).
const DEFAULT_REPLIES: [&str; 4] =
    ["We're coming, one moment", "Please leave it at the door", "We can't come to the door right now", "Who is it?"];
/// The reply card never scrolls: at most this many lines.
const MAX_REPLIES: usize = 4;

/// The replies offered at the door: the page's own (blank ones left out, at most four), else the defaults.
fn door_replies(option: Option<&serde_json::Value>) -> Vec<String> {
    match option.and_then(serde_json::Value::as_array) {
        Some(list) => list
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .take(MAX_REPLIES)
            .map(str::to_string)
            .collect(),
        None => DEFAULT_REPLIES.iter().map(|t| t.to_string()).collect(),
    }
}

/// The `tts.speak` call that says a reply on the speaker at the door: the text-to-speech engine and the call's data.
/// `tts` is the integration's `tts` (the panel assistant's engine, its language and voice); `language` is the page's
/// own choice ("" = the assistant's language, then also its voice). None without an engine, a speaker or a text.
fn speak_request(tts: Option<&serde_json::Value>, speaker: &str, message: &str, language: &str) -> Option<(String, serde_json::Value)> {
    let get = |key: &str| tts.and_then(|t| t.get(key)).and_then(serde_json::Value::as_str).unwrap_or("");
    let engine = get("engine");
    let message = message.trim();
    if engine.is_empty() || speaker.is_empty() || message.is_empty() {
        return None;
    }
    let mut data = serde_json::json!({ "media_player_entity_id": speaker, "message": message, "cache": true });
    let own = get("language");
    let language = if language.is_empty() { own } else { language };
    if !language.is_empty() {
        data["language"] = serde_json::json!(language);
        // the assistant's voice belongs to its language
        if language == own && !get("voice").is_empty() {
            data["options"] = serde_json::json!({ "voice": get("voice") });
        }
    }
    Some((engine.to_string(), data))
}

fn door_speaker(s: &crate::UiState) -> String {
    crate::page_option(s, "camera", "speaker").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn tts_config(s: &crate::UiState) -> Option<serde_json::Value> {
    s.panel_config.as_ref().and_then(|c| c.get("tts")).cloned()
}

/// The microphone in the visitor window and its replies: shown when the page has a speaker at the door, replies and
/// Home Assistant a text-to-speech engine.
fn door_replies_state(ui: &AppWindow, s: &crate::UiState) {
    let speaker = door_speaker(s);
    let replies = if speaker.is_empty() { Vec::new() } else { door_replies(crate::page_option(s, "camera", "replies").as_ref()) };
    let engine = tts_config(s).is_some_and(|t| t.get("engine").and_then(serde_json::Value::as_str).is_some_and(|e| !e.is_empty()));
    let can_talk = !speaker.is_empty() && !replies.is_empty() && engine;
    if ui.get_cam_can_talk() != can_talk {
        ui.set_cam_can_talk(can_talk);
    }
    if !can_talk && ui.get_cam_replies() {
        ui.set_cam_replies(false);
    }
    let rows: Vec<slint::SharedString> = replies.into_iter().map(Into::into).collect();
    if let Some(m) = crate::pages::update_rows(&ui.get_cam_reply_list(), rows) {
        ui.set_cam_reply_list(m);
    }
}

/// A reply tapped at the door: Home Assistant's `tts.speak` of that line on the speaker at the door, in the page's
/// language. The visitor window stays a minute longer (a conversation may follow).
fn speak_reply(ui: &AppWindow, index: usize) {
    let Some(line) = slint::Model::row_data(&ui.get_cam_reply_list(), index) else { return };
    let (speaker, language, tts) = with_state(|s| {
        let language = crate::page_option(s, "camera", "language").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        (door_speaker(s), language, tts_config(s))
    });
    match speak_request(tts.as_ref(), &speaker, &line, &language) {
        Some((engine, data)) => {
            eprintln!("nspanel-app: door reply {index} on {speaker} ({engine})");
            crate::call("tts", "speak", &engine, data);
            if RING_AT.with(|r| r.get()).is_some() {
                RING_AT.with(|r| r.set(Some(std::time::Instant::now())));
            }
        }
        None => eprintln!("nspanel-app: door reply {index}: no speaker or text-to-speech engine"),
    }
}

thread_local! {
    /// The visitor alert: each doorbell's last state, when one rang.
    static RING_STATE: std::cell::RefCell<std::collections::HashMap<String, String>> = std::cell::RefCell::new(std::collections::HashMap::new());
    static RING_AT: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// How long the visitor alert stays when nobody answers.
const RING_SECONDS: u64 = 60;

/// The doorbells (each cameras page's `ring` option, a binary sensor): when one turns on, the panel wakes on its cameras
/// page with the picture big, a chime (if do not disturb lets it through), and the door lock's "hold to open" when the
/// page has a `lock`. Called with the states.
fn doorbell(ui: &AppWindow, s: &crate::UiState, pages: &[(usize, serde_json::Value)]) {
    // over by itself after a minute
    if RING_AT.with(|r| r.get()).is_some_and(|t| t.elapsed().as_secs() >= RING_SECONDS) {
        RING_AT.with(|r| r.set(None));
        ui.set_cam_ring(false);
    }
    let mut rang = None;
    for (n, (cfg, page)) in pages.iter().enumerate() {
        let Some(entity) = crate::pages::option_of(page, "ring").and_then(|v| v.as_str()).filter(|e| !e.is_empty()) else { continue };
        let state = s.states.get(entity).map(|e| e.state.clone()).unwrap_or_default();
        let before = RING_STATE.with(|r| r.borrow_mut().insert(entity.to_string(), state.clone())).unwrap_or_default();
        if state == "on" && before != "on" && !before.is_empty() && rang.is_none() {
            rang = Some((n, *cfg, page));
        }
    }
    let Some((n, cfg, page)) = rang else { return };
    RING_AT.with(|r| r.set(Some(std::time::Instant::now())));
    let entities = crate::pages::entities_of(page);
    let sel = (crate::pages::inst(&ui.get_cam_inst(), n).sel.max(0) as usize).min(entities.len().saturating_sub(1));
    let name = entities
        .get(sel)
        .and_then(|id| s.states.get(id))
        .and_then(|e| e.attrs.get("friendly_name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Camera")
        .to_string();
    ui.set_cam_ring_title("Someone is at the door".into());
    ui.set_cam_ring_sub(format!("{name} · {}", crate::clock::local_hm(crate::pages::now())).into());
    ui.set_cam_can_open(crate::pages::option_of(page, "lock").and_then(|v| v.as_str()).is_some_and(|l| !l.is_empty()));
    // the page's settings close; the replies start closed
    ui.set_cam_settings(false);
    ui.set_cam_replies(false);
    crate::pages::with_inst(ui.get_cam_inst(), |m| ui.set_cam_inst(m), n, |d| d.view = 0);
    ui.set_cam_ring_n(n as i32);
    ui.set_cam_ring(true);
    if crate::dnd_allows(s, "ring") {
        s.player.play(crate::voice::Sound::Chime);
    }
    // the doorbell's cameras page, after this state update
    let weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let Some(ui) = weak.upgrade() else { return };
        crate::wake(&ui);
        if !crate::go_to_cfg(&ui, cfg) && !crate::go_to_page(&ui, "camera") {
            crate::open_temporary_page(&ui, "camera");
        }
        with_state(|s| refresh(&ui, s));
    });
}

/// Taps on the page (`cam.*`).
pub fn act(ui: &AppWindow, what: &str, value: &str) {
    let n: i32 = value.parse().unwrap_or(0);
    match what {
        "cam.ring-close" => {
            RING_AT.with(|r| r.set(None));
            ui.set_cam_replies(false);
            ui.set_cam_ring(false);
        }
        // the microphone in the visitor window: the replies open or close
        "cam.talk" => ui.set_cam_replies(!ui.get_cam_replies() && ui.get_cam_ring() && ui.get_cam_can_talk()),
        "cam.replies" => ui.set_cam_replies(value == "1" && ui.get_cam_ring() && ui.get_cam_can_talk()),
        "cam.reply" => speak_reply(ui, n.max(0) as usize),
        // the page's settings: the picture changes at once, the integration keeps it
        "cam.fit" => {
            let n = crate::acting_n("camera");
            crate::pages::with_inst(ui.get_cam_inst(), |m| ui.set_cam_inst(m), n, |d| d.fit = fit_index(value));
            crate::page_set(ui, "camera", None, serde_json::json!({ "fit": value }));
        }
        "cam.open" => {
            let lock = with_state(|s| crate::page_option(s, "camera", "lock").and_then(|v| v.as_str().map(str::to_string)));
            if let Some(lock) = lock.filter(|l| !l.is_empty()) {
                let domain = lock.split('.').next().unwrap_or("lock");
                let service = if domain == "lock" { "unlock" } else { "open_cover" };
                crate::call(domain, service, &lock, serde_json::json!({}));
            }
            RING_AT.with(|r| r.set(None));
            ui.set_cam_replies(false);
            ui.set_cam_ring(false);
        }
        "cam.view" => crate::pages::with_inst(ui.get_cam_inst(), |m| ui.set_cam_inst(m), crate::acting_n("camera"), |d| d.view = n),
        "cam.pick" => crate::pages::with_inst(ui.get_cam_inst(), |m| ui.set_cam_inst(m), crate::acting_n("camera"), |d| {
            d.sel = n;
            d.view = 0;
        }),
        _ => eprintln!("nspanel-app: act {what} {value} (not handled)"),
    }
    with_state(|s| refresh(ui, s));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_camera_view_finds_the_page_with_the_camera() {
        let a = json!({"kind": "camera", "entities": ["camera.a", "camera.b"]});
        let b = json!({"kind": "camera", "entities": ["camera.c", "camera.b"]});
        let pages = vec![(1, &a), (4, &b)];
        // the first page that has it: its place in the list, its row, the camera's place on it
        assert_eq!(page_with(&pages, None, "camera.c"), Some((4, 1, 0)));
        assert_eq!(page_with(&pages, None, "camera.b"), Some((1, 0, 1)));
        // the cameras page on screen first
        assert_eq!(page_with(&pages, Some(4), "camera.b"), Some((4, 1, 1)));
        assert_eq!(page_with(&pages, Some(4), "camera.a"), Some((1, 0, 0)));
        // on no cameras page
        assert_eq!(page_with(&pages, Some(1), "camera.d"), None);
        assert_eq!(page_with(&[], None, "camera.a"), None);
    }

    #[test]
    fn fit_defaults_to_filling_the_screen() {
        assert_eq!(fit_index("whole"), 0);
        assert_eq!(fit_index("fill"), 1);
        assert_eq!(fit_index(""), 1);
    }

    #[test]
    fn replies_default_when_none_are_set() {
        assert_eq!(door_replies(None), DEFAULT_REPLIES.to_vec());
        assert_eq!(door_replies(Some(&json!("not a list"))), DEFAULT_REPLIES.to_vec());
    }

    #[test]
    fn replies_skip_blanks_and_keep_four() {
        let set = json!([" One ", "", "  ", "Two", 3, "Three", "Four", "Five"]);
        assert_eq!(door_replies(Some(&set)), vec!["One", "Two", "Three", "Four"]);
        assert!(door_replies(Some(&json!([]))).is_empty());
    }

    #[test]
    fn speak_needs_an_engine_a_speaker_and_a_text() {
        let tts = json!({ "engine": "tts.cloud", "language": "en-US", "voice": "JennyNeural" });
        assert!(speak_request(None, "media_player.door", "Hi", "").is_none());
        assert!(speak_request(Some(&json!({ "engine": "" })), "media_player.door", "Hi", "").is_none());
        assert!(speak_request(Some(&tts), "", "Hi", "").is_none());
        assert!(speak_request(Some(&tts), "media_player.door", "  ", "").is_none());
    }

    #[test]
    fn speak_in_the_assistants_language_and_voice() {
        let tts = json!({ "engine": "tts.cloud", "language": "en-US", "voice": "JennyNeural" });
        let (engine, data) = speak_request(Some(&tts), "media_player.door", " Who is it? ", "").unwrap();
        assert_eq!(engine, "tts.cloud");
        assert_eq!(
            data,
            json!({ "media_player_entity_id": "media_player.door", "message": "Who is it?", "cache": true,
                    "language": "en-US", "options": { "voice": "JennyNeural" } })
        );
    }

    #[test]
    fn speak_in_another_language_without_the_assistants_voice() {
        let tts = json!({ "engine": "tts.cloud", "language": "en-US", "voice": "JennyNeural" });
        let (_, data) = speak_request(Some(&tts), "media_player.door", "Hallo", "de-DE").unwrap();
        assert_eq!(data["language"], "de-DE");
        assert!(data.get("options").is_none());
        // the engine's own language when the assistant has none
        let (_, data) = speak_request(Some(&json!({ "engine": "tts.piper" })), "media_player.door", "Hi", "").unwrap();
        assert!(data.get("language").is_none() && data.get("options").is_none());
    }
}
