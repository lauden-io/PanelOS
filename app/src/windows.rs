//! City windows that light up one by one (drawn city and drawn big city backgrounds). The art comes
//! in two kinds: the usual layers with their windows painted lit, and "unlit" versions with every
//! window dark plus a list of the windows (place, size, lit colour, how many of each kind are lit).
//! This draws the lit ones into a transparent picture laid over the unlit art (only as big as the
//! area the windows take, placed by the UI), changes one window at a time (more in the evening,
//! fewer late at night, a few lamps on dull days) and hands over the next picture: the UI fades
//! between the two, so a window comes on or goes off softly.

use serde_json::Value;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::{AppWindow, WinRect};

struct Win {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    c: [u8; 3],
    o: f32,
    p: f32,
}

#[derive(Default)]
struct State {
    /// "vcity" / "vmetro" and the light variant: the list in use.
    key: String,
    wins: Vec<Win>,
    /// which of them are lit now, and each window's own place in the lighting order (so the same
    /// windows tend to be lit, like a real street)
    lit: Vec<bool>,
    rank: Vec<f32>,
    /// day lamps per condition (cloudy 0.3, rain 0.45 …)
    conds: Vec<(String, f32)>,
    next_change: f64,
    /// a window fading in or out now: until when (then the change goes into the lit layer)
    fading: Option<f64>,
    /// windows a tap turned on or off (spec 25.H2): left so until then
    held: Vec<(usize, f64)>,
}

thread_local!(static STATE: std::cell::RefCell<State> = std::cell::RefCell::new(State::default()));

fn random() -> u64 {
    use std::cell::Cell;
    thread_local!(static SEED: Cell<u64> = Cell::new(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(11) | 1
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

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn load(key: &str) -> Option<(Vec<Win>, Vec<(String, f32)>)> {
    let (dir, file) = if let Some(base) = key.strip_suffix("-light") { ("wx-light", format!("windows-{base}-light.json")) } else { ("wx", format!("windows-{key}.json")) };
    let path = std::path::Path::new(env!("PANELOS_UI_PICTURES")).join(dir).join(file);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let hex = |s: &str| -> [u8; 3] {
        let h = s.trim_start_matches('#');
        let n = u32::from_str_radix(h, 16).unwrap_or(0xffd27a);
        [(n >> 16) as u8, (n >> 8) as u8, n as u8]
    };
    let wins = v["windows"]
        .as_array()?
        .iter()
        .filter_map(|w| {
            Some(Win {
                x: w["x"].as_f64()? as f32,
                y: w["y"].as_f64()? as f32,
                w: w["w"].as_f64()? as f32,
                h: w["h"].as_f64()? as f32,
                c: hex(w["c"].as_str().unwrap_or("#ffd27a")),
                o: w["o"].as_f64().unwrap_or(1.0) as f32,
                p: w["p"].as_f64().unwrap_or(0.5) as f32,
            })
        })
        .collect();
    let conds = v["conds"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0) as f32)).collect()).unwrap_or_default();
    Some((wins, conds))
}

/// The share of a kind's windows lit now: as the art at night (more in the evening, fewer late at
/// night and towards the morning), its day lamps on dull days, none on bright days.
fn share(night: f32, hour: u32, day_lamps: f32) -> f32 {
    let at_night = match hour {
        17..=22 => 1.0,
        23 | 0 => 0.75,
        1..=4 => 0.4,
        5..=6 => 0.6,
        _ => 0.85,
    };
    at_night * night + day_lamps * (1.0 - night)
}

/// The condition name the art's day lamps list uses, for a mood.
fn cond_name(mood: i32) -> &'static str {
    match mood {
        2 | 5 => "cloudy",
        3 => "rain",
        4 => "snow",
        8 => "thunder",
        9 => "exceptional",
        _ => "",
    }
}

/// The area a picture of these windows takes in the layer (x, y, width, height): from the whole pixel at their top left
/// to the whole pixel past their bottom right, at least one pixel.
fn area(wins: &[Win]) -> (f32, f32, f32, f32) {
    if wins.is_empty() {
        return (0.0, 0.0, 1.0, 1.0);
    }
    let x0 = wins.iter().map(|w| w.x).fold(f32::MAX, f32::min).floor().max(0.0);
    let y0 = wins.iter().map(|w| w.y).fold(f32::MAX, f32::min).floor().max(0.0);
    let x1 = wins.iter().map(|w| w.x + w.w).fold(f32::MIN, f32::max).ceil();
    let y1 = wins.iter().map(|w| w.y + w.h).fold(f32::MIN, f32::max).ceil();
    (x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0))
}

fn render(st: &State, only: Option<usize>) -> (Image, f32, f32, f32, f32) {
    // the lit layer over the area of all the windows, or one window cut out (its own small picture); the UI places each
    let (ox, oy, w, h) = match only {
        Some(i) => area(&st.wins[i..=i]),
        None => area(&st.wins),
    };
    let (bw, bh) = (w as u32, h as u32);
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(bw, bh);
    let px = buf.make_mut_slice();
    for (i, (win, on)) in st.wins.iter().zip(&st.lit).enumerate() {
        let draw = match only {
            Some(k) => k == i,
            None => *on,
        };
        if !draw {
            continue;
        }
        // area coverage per pixel (windows sit at sub-pixel places)
        let (x0, y0, x1, y1) = (win.x.max(ox), win.y.max(oy), (win.x + win.w).min(ox + w), (win.y + win.h).min(oy + h));
        let mut py = y0.floor();
        while py < y1 {
            let cy = ((py + 1.0).min(y1) - py.max(y0)).max(0.0);
            let mut pxx = x0.floor();
            while pxx < x1 {
                let cx = ((pxx + 1.0).min(x1) - pxx.max(x0)).max(0.0);
                let a = (cx * cy * win.o).clamp(0.0, 1.0);
                let (lx, ly) = ((pxx - ox) as u32, (py - oy) as u32);
                if a > 0.0 && lx < bw && ly < bh {
                    let d = &mut px[(ly * bw + lx) as usize];
                    let da = d.a as f32 / 255.0;
                    let oa = a + da * (1.0 - a);
                    let mix = |c: u8, dc: u8| ((c as f32 * a + dc as f32 * da * (1.0 - a)) / oa.max(1e-6)).round() as u8;
                    *d = Rgba8Pixel { r: mix(win.c[0], d.r), g: mix(win.c[1], d.g), b: mix(win.c[2], d.b), a: (oa * 255.0).round() as u8 };
                }
                pxx += 1.0;
            }
            py += 1.0;
        }
    }
    (Image::from_rgba8(buf), ox, oy, w, h)
}

/// The lit layer to the UI: its picture and its place.
fn set_base(ui: &AppWindow, st: &State) {
    let (img, x, y, w, h) = render(st, None);
    ui.set_win_base(img);
    ui.set_win_base_rect(WinRect { x, y, w, h });
}

/// Once a second: the city theme in use (4 drawn city, 6 big city; light or dark), the weather and
/// the sun. A window that comes on fades in over the lit layer; one that goes off is taken out of
/// the layer and fades out on top of it; a second later the layer is redrawn with the change.
pub fn tick(ui: &AppWindow, theme: i32, light: bool, mood: i32, sun_el: f32, hour: u32) {
    let base = match theme {
        4 => "vcity",
        6 => "vmetro",
        _ => {
            if ui.get_win_on() {
                ui.set_win_on(false);
            }
            return;
        }
    };
    let key = if light { format!("{base}-light") } else { base.to_string() };
    STATE.with(|cell| {
        let mut st = cell.borrow_mut();
        let fresh = st.key != key;
        if fresh {
            st.key = key.clone();
            st.fading = None;
            match load(&key) {
                Some((wins, conds)) => {
                    st.rank = (0..wins.len()).map(|_| (random() % 10_000) as f32 / 10_000.0).collect();
                    st.lit = vec![false; wins.len()];
                    st.wins = wins;
                    st.conds = conds;
                }
                None => st.wins.clear(),
            }
        }
        // fog: its mist lies over the windows (no unlit art): the painted layers stay
        if st.wins.is_empty() || matches!(mood, 5 | 13 | 15) {
            if ui.get_win_on() {
                ui.set_win_on(false);
            }
            return;
        }
        let night = ((-sun_el - 1.0) / 6.0).clamp(0.0, 1.0);
        let lamps = st.conds.iter().find(|(k, _)| k == cond_name(mood)).map(|(_, v)| *v).unwrap_or(0.0);
        let s = share(night, hour, lamps);
        // a window is wanted lit when its own place in the order is under its kind's share
        let t = now();
        st.held.retain(|(_, until)| *until > t);
        let held: Vec<usize> = st.held.iter().map(|(i, _)| *i).collect();
        let mut want: Vec<bool> = st.wins.iter().zip(&st.rank).map(|(w, r)| *r < w.p * s).collect();
        for &i in &held {
            if i < want.len() {
                want[i] = st.lit[i];
            }
        }
        if fresh || !ui.get_win_on() {
            // the first picture: as it should be now, no fades (the page shows it as a whole)
            st.lit = want;
            set_base(ui, &st);
            ui.set_win_in_show(false);
            ui.set_win_out_show(true);
            ui.set_win_on(true);
            st.fading = None;
            st.next_change = t + 3.0;
            return;
        }
        // a fade that ran its course: the change goes into the layer, the fading pictures empty
        if let Some(until) = st.fading {
            if t < until {
                return;
            }
            st.fading = None;
            set_base(ui, &st);
            ui.set_win_in(Image::default());
            ui.set_win_in_show(false);
            ui.set_win_out(Image::default());
            ui.set_win_out_show(true);
            return;
        }
        if t < st.next_change {
            return;
        }
        // one window towards what is wanted; at the target, now and then one goes off and another
        // of its kind comes on, so the city lives
        let diff: Vec<usize> = (0..st.lit.len()).filter(|&i| st.lit[i] != want[i]).collect();
        let (mut on, mut off) = (None, None);
        if !diff.is_empty() {
            let i = diff[(random() as usize) % diff.len()];
            if want[i] { on = Some(i) } else { off = Some(i) }
        } else {
            let lit: Vec<usize> = (0..st.lit.len()).filter(|&i| st.lit[i]).collect();
            if !lit.is_empty() {
                let i = lit[(random() as usize) % lit.len()];
                let pool: Vec<usize> = (0..st.lit.len()).filter(|&j| !st.lit[j] && (st.wins[j].p - st.wins[i].p).abs() < 1e-3).collect();
                if !pool.is_empty() {
                    let j = pool[(random() as usize) % pool.len()];
                    st.rank.swap(i, j);
                    off = Some(i);
                    on = Some(j);
                }
            }
        }
        if let Some(i) = off {
            // out of the layer, on top of it at full, then fading out
            st.lit[i] = false;
            let (img, x, y, w, h) = render(&st, Some(i));
            ui.set_win_out(img);
            ui.set_win_out_rect(WinRect { x, y, w, h });
            set_base(ui, &st);
            ui.set_win_out_show(false);
        }
        if let Some(i) = on {
            let (img, x, y, w, h) = render(&st, Some(i));
            ui.set_win_in(img);
            ui.set_win_in_rect(WinRect { x, y, w, h });
            ui.set_win_in_show(true);
            st.lit[i] = true;
        }
        if on.is_some() || off.is_some() {
            st.fading = Some(t + 1.0);
        }
        st.next_change = t + if diff.is_empty() { 9.0 } else { 2.5 } + (random() % 3000) as f64 / 1000.0;
    });
}

/// A tap on the city at (x, y) of the window layer (528 x 528): the window there lights or goes dark, fading like the
/// others, and the street leaves it so for a minute (spec 25.H2).
pub fn tap(ui: &AppWindow, x: f32, y: f32) {
    STATE.with(|cell| {
        let mut st = cell.borrow_mut();
        if st.wins.is_empty() || st.fading.is_some() || !ui.get_win_on() {
            return;
        }
        let near = |w: &Win| x >= w.x - 4.0 && x <= w.x + w.w + 4.0 && y >= w.y - 4.0 && y <= w.y + w.h + 4.0;
        let Some(i) = st.wins.iter().position(near) else {
            return;
        };
        let t = now();
        if st.lit[i] {
            st.lit[i] = false;
            let (img, x, y, w, h) = render(&st, Some(i));
            ui.set_win_out(img);
            ui.set_win_out_rect(WinRect { x, y, w, h });
            set_base(ui, &st);
            ui.set_win_out_show(false);
        } else {
            let (img, x, y, w, h) = render(&st, Some(i));
            ui.set_win_in(img);
            ui.set_win_in_rect(WinRect { x, y, w, h });
            ui.set_win_in_show(true);
            st.lit[i] = true;
        }
        st.fading = Some(t + 1.0);
        st.held.retain(|(j, _)| *j != i);
        st.held.push((i, t + 60.0));
        eprintln!("nspanel-app: city window tapped");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(x: f32, y: f32, w: f32, h: f32) -> Win {
        Win { x, y, w, h, c: [255, 210, 122], o: 1.0, p: 0.5 }
    }

    #[test]
    fn area_of_one_window_covers_its_pixels() {
        assert_eq!(area(&[win(10.4, 20.6, 3.2, 4.0)]), (10.0, 20.0, 4.0, 5.0));
        // never less than a pixel, never left of or above the layer
        assert_eq!(area(&[win(-1.5, 7.0, 0.0, 0.0)]), (0.0, 7.0, 1.0, 1.0));
    }

    #[test]
    fn area_of_all_windows_is_their_union() {
        let wins = [win(12.0, 247.0, 6.0, 9.0), win(300.5, 251.2, 4.0, 6.0), win(520.0, 340.0, 8.8, 12.0)];
        assert_eq!(area(&wins), (12.0, 247.0, 517.0, 105.0));
        assert_eq!(area(&[]), (0.0, 0.0, 1.0, 1.0));
    }
}
