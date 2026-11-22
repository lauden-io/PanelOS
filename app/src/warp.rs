//! The weather scenes' moving rows, drawn by the app (ui/photo_sky.slint, 5b–5d): the meadow swaying in the wind with
//! light running over it, the trees bending and the river shimmering. Each is a stack of thin strips of the scene's
//! pictures, every strip shifted sideways by its own small, smoothly changing amount. Drawn by the page, that is one
//! drawing step per strip and picture, a hundred and more a frame in strong wind. Here they become one picture of the
//! rows that move, made at most 30 times a second while the scene is on screen and something in it moves: the same
//! strips and motions, the sub-pixel part interpolated between neighbouring pixels, the pictures of the light (day,
//! dusk, dawn, night) mixed with the opacities the page uses, the sheen over the meadow and the trees over both, in
//! the page's order.
//!
//! The page asks with `Warp.frame(WarpIn)`, a snapshot of what it shows renewed at every 1/30 s step, and draws the
//! answer in place of its strips. The pictures are read from the UI bundle in a thread the first time a scene asks;
//! until they are there the answer is empty and the page draws its strips itself (as it does without the app, in the
//! PC preview).
//!
//! Only the rows that move are worked on, a row at a time with one pair of weights per row (the compiler turns that
//! into vector instructions). A scene's mixed light pictures are kept until the light changes, and two frame buffers
//! per scene take turns, so a frame allocates nothing.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::WarpIn;

mod pics {
    include!(concat!(env!("OUT_DIR"), "/warp_pics.rs"));
}

/// The scene layer's size: its pictures are 528 x 528.
const LAYER: i32 = 528;
/// Clear pixels on each side of every stored row, more than any strip moves, so a shifted row never reads outside it.
const PAD: usize = 32;
const MAX_SHIFT: f32 = PAD as f32 - 2.0;
/// The light running over the meadow: 528 x 200, repeating sideways.
const SHEEN: &str = "wx/grass-sheen.png";
const SHEEN_W: usize = 528;
const SHEEN_H: usize = 200;

/// Rows y .. y + h of a layer picture, its columns x .. x + w, as premultiplied RGBA with PAD clear pixels on each side
/// of every row.
pub struct Band {
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Band {
    fn blank(x: i32, y: i32, w: usize, h: usize) -> Band {
        Band { x, y, w, h, px: vec![0; (w + 2 * PAD) * 4 * h] }
    }

    fn stride(&self) -> usize {
        (self.w + 2 * PAD) * 4
    }

    fn has_row(&self, y: i32) -> bool {
        y >= self.y && y < self.y + self.h as i32
    }

    /// Layer row y (inside the band).
    fn row(&self, y: i32) -> &[u8] {
        let r = (y - self.y) as usize;
        &self.px[r * self.stride()..(r + 1) * self.stride()]
    }

    fn row_mut(&mut self, y: i32) -> &mut [u8] {
        let (r, s) = ((y - self.y) as usize, self.stride());
        &mut self.px[r * s..(r + 1) * s]
    }
}

/// x / 255, rounded (x up to 255 * 255 + 255).
#[inline]
fn div255(x: u32) -> u32 {
    (x + 128 + ((x + 128) >> 8)) >> 8
}

/// Premultiplied `src` over `dst`, pixel by pixel.
fn over(dst: &mut [u8], src: &[u8]) {
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let a = s[3] as u32;
        if a == 0 {
            continue;
        }
        if a == 255 {
            d.copy_from_slice(s);
            continue;
        }
        let k = 255 - a;
        for c in 0..4 {
            d[c] = (s[c] as u32 + div255(d[c] as u32 * k)).min(255) as u8;
        }
    }
}

/// Premultiplied `src` at opacity o (0..=256) over `dst`, pixel by pixel.
fn over_with(dst: &mut [u8], src: &[u8], o: u32) {
    if o >= 256 {
        return over(dst, src);
    }
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let a = (s[3] as u32 * o + 128) >> 8;
        if a == 0 {
            continue;
        }
        let k = 255 - a;
        for c in 0..3 {
            d[c] = (((s[c] as u32 * o + 128) >> 8) + div255(d[c] as u32 * k)).min(255) as u8;
        }
        d[3] = (a + div255(d[3] as u32 * k)).min(255) as u8;
    }
}

/// out = a * (256 - f8) / 256 + b * f8 / 256, byte by byte (one pair of weights for the whole row).
fn lerp(out: &mut [u8], a: &[u8], b: &[u8], f8: u16) {
    if f8 == 0 {
        out.copy_from_slice(a);
        return;
    }
    let (wa, wb) = (256 - f8, f8);
    for ((o, &x), &y) in out.iter_mut().zip(a).zip(b) {
        *o = ((x as u16 * wa + y as u16 * wb + 128) >> 8) as u8;
    }
}

/// A band row moved right by `off` layer pixels into `out`, which covers layer columns x0 .. x0 + out.len() / 4. As
/// the page draws a strip (at the whole part i of `off`, then again at i + 1 with the fraction f as its opacity), but
/// interpolated: out[x] = row[x - i] * (1 - f) + row[x - i - 1] * f. Columns the band does not reach come out clear.
fn shift_row(out: &mut [u8], row: &[u8], band_x: i32, x0: i32, off: f32) {
    let off = if off.is_finite() { off.clamp(-MAX_SHIFT, MAX_SHIFT) } else { 0.0 };
    let whole = off.floor();
    let mut i = whole as i32;
    let mut f8 = ((off - whole) * 256.0).round() as u16;
    if f8 >= 256 {
        f8 = 0;
        i += 1;
    }
    let n = (out.len() / 4) as i32;
    let total = (row.len() / 4) as i32;
    // the row pixel (padding counted) of the first output pixel's main sample
    let s0 = x0 - i - band_x + PAD as i32;
    // output pixels whose both samples (s0 + p and s0 + p - 1) lie in the row
    let lo = (1 - s0).clamp(0, n);
    let hi = (total - s0).clamp(lo, n);
    let (lo_b, hi_b) = (lo as usize * 4, hi as usize * 4);
    out[..lo_b].fill(0);
    if hi > lo {
        let a = (s0 + lo) as usize * 4;
        let len = hi_b - lo_b;
        lerp(&mut out[lo_b..hi_b], &row[a..a + len], &row[a - 4..a - 4 + len], f8);
    }
    out[hi_b..].fill(0);
}

/// The light running over the meadow: row `sy` of the sheen picture, repeating sideways with a copy's left edge at layer
/// column `sx`, over `out` (layer columns x0 ..) at opacity o (0..=256). (The sheen is so soft, at most one level
/// between neighbouring pixels, that its sub-pixel place makes no visible difference: it takes the nearest pixel.)
fn sheen_row(out: &mut [u8], sheen: &Band, sy: i32, x0: i32, sx: f64, o: u32) {
    if o == 0 || !sheen.has_row(sy) || sheen.w != SHEEN_W {
        return;
    }
    let row = sheen.row(sy);
    let n = out.len() / 4;
    let mut col = ((x0 as f64 - sx).round() as i64).rem_euclid(SHEEN_W as i64) as usize;
    let mut i = 0;
    while i < n {
        let run = (n - i).min(SHEEN_W - col);
        let s = (PAD + col) * 4;
        over_with(&mut out[i * 4..(i + run) * 4], &row[s..s + run * 4], o);
        i += run;
        col = 0;
    }
}

fn sin_deg(d: f64) -> f64 {
    d.rem_euclid(360.0).to_radians().sin()
}

/// The gusts' envelope: one 3 s pulse every 11 s, faded in and out with the gusts (`gust_k`).
fn genv(t: f64, gust_k: f64) -> f64 {
    let gt = (t + 3.0).rem_euclid(11.0);
    if gt < 3.0 {
        gust_k * sin_deg(gt / 3.0 * 180.0)
    } else {
        0.0
    }
}

/// A meadow strip's shift: 8 rows from y0 + 8k, more in front; a gust pushes it to the left.
fn grass_shift(k: i32, y0: i32, t: f64, sway: f64, gust: f64) -> f64 {
    let depth = ((k as f64 + 0.5) / ((LAYER - y0) as f64 / 8.0)).powf(1.2);
    (sway + 1.6 * gust) * depth * sin_deg(t * 360.0 / 2.75 + k as f64 * 13.0) - 1.5 * gust * depth
}

/// A river strip's shift: 3 rows from y0 + 3k, each its own way.
fn water_shift(k: i32, t: f64, water: f64) -> f64 {
    water * sin_deg(t * (95.0 + ((k * 37) % 60) as f64) + k as f64 * 67.0)
}

/// A tree's strips: 5 rows each from `y0` while above `base`, of the tree picture's columns x .. x + w; the top bends
/// most, the foot not at all.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tree {
    x: i32,
    w: i32,
    y0: i32,
    base: i32,
    ph: f64,
}

impl Tree {
    fn strips(&self) -> i32 {
        (self.base - self.y0 + 4) / 5
    }
}

const COUNTRYSIDE_TREES: [Tree; 1] = [Tree { x: 0, w: 145, y0: 246, base: 378, ph: 0.0 }];
const NEUTRAL_TREES: [Tree; 2] =
    [Tree { x: 0, w: 139, y0: 245, base: 470, ph: 0.0 }, Tree { x: 408, w: 120, y0: 266, base: 477, ph: 140.0 }];

/// The trees of a theme that bend (the drawn countryside, the neutral scene).
fn trees_of(theme: i32) -> &'static [Tree] {
    match theme {
        3 => &COUNTRYSIDE_TREES,
        5 => &NEUTRAL_TREES,
        _ => &[],
    }
}

fn tree_shift(tree: &Tree, k: i32, t: f64, lean: f64, flut: f64) -> f64 {
    let rel = (((tree.base - tree.y0 - 5 * k) as f64 - 2.5) / (tree.base - tree.y0) as f64).clamp(0.0, 1.0);
    -rel.powf(1.8) * (lean + flut * sin_deg(t * 190.0 + rel * 70.0 + tree.ph))
}

/// A frame's motions, from the page's values (the same formulas as the page's own strips).
#[derive(Debug)]
struct Motion {
    t: f64,
    sway: f64,
    /// how far a gust pushes the meadow
    gust: f64,
    /// the sheen's opacity (0..=256) and the left edge of one of its copies
    sheen: u32,
    sheen_x: f64,
    lean: f64,
    flut: f64,
    water: f64,
}

fn motion(w: &WarpIn) -> Motion {
    let t = w.t as f64;
    let g = genv(t, w.gust_k as f64);
    let (sway, bend) = (w.sway.max(0.0) as f64, w.bend.max(0.0) as f64);
    let sheen_a = (sway / 1.3).min(1.0) * (0.6 + 0.4 * ((sway - 1.3) / 1.3).clamp(0.0, 1.0));
    let gust_bend = (bend / 0.45).min(1.0);
    Motion {
        t,
        sway,
        gust: g * (sway / 1.3).min(1.0),
        sheen: ((sheen_a * (1.0 + 0.5 * g)).min(1.0) * 256.0).round() as u32,
        sheen_x: -(t * 64.0).rem_euclid(528.0),
        lean: bend * (5.0 + 2.0 * sin_deg(t * 41.0)) + 6.0 * g * gust_bend,
        flut: bend * 1.8 + g * gust_bend,
        water: w.water as f64,
    }
}

/// What moves in a snapshot.
#[derive(Debug, PartialEq)]
struct Parts {
    grass: bool,
    water: bool,
    trees: &'static [Tree],
}

fn parts(w: &WarpIn) -> Parts {
    Parts {
        grass: w.sway > 0.0 && matches!(w.theme, 0 | 1 | 3 | 5),
        water: w.water > 0.0 && matches!(w.theme, 4 | 6),
        trees: if w.bend > 0.0 { trees_of(w.theme) } else { &[] },
    }
}

/// The light pictures' opacities (0..=256) in the page's order: day, dusk, dawn, night. The land's rows (meadow, river)
/// leave the day picture out when a picture over it is opaque and dusk and dawn out under a full night; the trees always
/// start from the day picture.
fn weights(w: &WarpIn, trees: bool) -> [u32; 4] {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 256.0).round() as u32;
    let grade = |x: f32| if w.has_grade && x > 0.0 && (trees || w.nmix < 1.0) { q(x) } else { 0 };
    [if trees || !w.day_hidden { 256 } else { 0 }, grade(w.dk), grade(w.dn), if w.nmix > 0.0 { q(w.nmix) } else { 0 }]
}

/// The four light pictures (day, dusk, dawn, night) of the land or of the trees, as the page's picture functions pick
/// them (dusk and dawn only for the sunny and partly cloudy conditions).
fn paths(w: &WarpIn, trees: bool) -> [Option<&'static str>; 4] {
    let t = w.theme.clamp(0, pics::FG.len() as i32 - 1) as usize;
    let m = |m: i32| if (0..17).contains(&m) { m as usize } else { 17 };
    let v = w.light as usize * 2 + w.unlit as usize;
    let (main, grade) = if trees { (&pics::TREES, &pics::GTREES) } else { (&pics::FG, &pics::GFG) };
    let g = |k: usize| (w.has_grade && (0..2).contains(&w.mood)).then(|| grade[t][w.mood as usize][k][v]);
    [Some(main[t][m(w.mood)][v]), g(0), g(1), Some(main[t][m(w.nmood)][v])]
}

/// Where a frame lies in the layer.
#[derive(Clone, Copy, Debug)]
struct Frame {
    x0: i32,
    top: i32,
    w: usize,
    h: usize,
}

/// The land's moving rows in a frame: the meadow (8-row strips, with the sheen over it) or the river (3-row strips).
enum Land<'a> {
    Grass { band: &'a Band, sheen: Option<&'a Band> },
    Water { band: &'a Band },
}

/// Draws one frame into `out` (premultiplied RGBA, fr.w x fr.h): the rows above the land clear, the land's rows shifted
/// strip by strip, the sheen over the meadow, the trees' strips over both. Every pixel is written.
fn compose(out: &mut [u8], fr: &Frame, land: Option<&Land>, trees: &[(Tree, &Band)], m: &Motion, scratch: &mut Vec<u8>) {
    let stride = fr.w * 4;
    let bottom = fr.top + fr.h as i32;
    let land_y = match land {
        Some(Land::Grass { band, .. } | Land::Water { band }) => band.y.clamp(fr.top, bottom),
        None => bottom,
    };
    out[..(land_y - fr.top) as usize * stride].fill(0);
    if let Some(land) = land {
        let (band, step) = match land {
            Land::Grass { band, .. } => (*band, 8),
            Land::Water { band } => (*band, 3),
        };
        let mut k = 0;
        while band.y + step * k < bottom.min(LAYER) {
            let r0 = band.y + step * k;
            let off = (match land {
                Land::Grass { .. } => grass_shift(k, band.y, m.t, m.sway, m.gust),
                Land::Water { .. } => water_shift(k, m.t, m.water),
            }) as f32;
            for y in r0.max(land_y)..(r0 + step).min(bottom) {
                let line = &mut out[(y - fr.top) as usize * stride..][..stride];
                if band.has_row(y) {
                    shift_row(line, band.row(y), band.x, fr.x0, off);
                } else {
                    line.fill(0);
                }
                if let Land::Grass { sheen: Some(sheen), .. } = land {
                    sheen_row(line, sheen, y - band.y, fr.x0, m.sheen_x, m.sheen);
                }
            }
            k += 1;
        }
    }
    for (tree, band) in trees {
        for k in 0..tree.strips() {
            let off = tree_shift(tree, k, m.t, m.lean, m.flut) as f32;
            let i = off.clamp(-MAX_SHIFT, MAX_SHIFT).floor() as i32;
            // the columns the strip reaches (it lies at i and at i + 1), inside the frame
            let c0 = (tree.x + i).max(fr.x0);
            let c1 = (tree.x + tree.w + i + 2).min(fr.x0 + fr.w as i32);
            if c1 <= c0 {
                continue;
            }
            let r0 = tree.y0 + 5 * k;
            for y in r0.max(fr.top)..(r0 + 5).min(bottom) {
                if !band.has_row(y) {
                    continue;
                }
                scratch.resize((c1 - c0) as usize * 4, 0);
                shift_row(scratch, band.row(y), band.x, c0, off);
                let line = &mut out[(y - fr.top) as usize * stride..][..stride];
                over(&mut line[(c0 - fr.x0) as usize * 4..(c1 - fr.x0) as usize * 4], scratch);
            }
        }
    }
}

// --- pictures ---------------------------------------------------------------------------------------------------

/// A band of a picture file: its path in the UI bundle, columns x .. x + w, rows y .. y + h.
type BandKey = (&'static str, i32, i32, usize, usize);

enum Slot {
    Loading,
    Ready(Arc<Band>, Instant),
    Failed,
}

#[derive(Clone)]
enum Got {
    Ready(Arc<Band>),
    Waiting,
    Failed,
}

fn store() -> &'static Mutex<HashMap<BandKey, Slot>> {
    static STORE: OnceLock<Mutex<HashMap<BandKey, Slot>>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}

/// The bands asked for: the loaded ones, the others started loading together in a thread.
fn fetch(keys: &[BandKey]) -> Vec<Got> {
    let now = Instant::now();
    let mut missing = Vec::new();
    let got = {
        let Ok(mut st) = store().lock() else { return vec![Got::Failed; keys.len()] };
        keys.iter()
            .map(|k| match st.get_mut(k) {
                Some(Slot::Ready(b, used)) => {
                    *used = now;
                    Got::Ready(b.clone())
                }
                Some(Slot::Loading) => Got::Waiting,
                Some(Slot::Failed) => Got::Failed,
                None => {
                    st.insert(*k, Slot::Loading);
                    missing.push(*k);
                    Got::Waiting
                }
            })
            .collect()
    };
    if !missing.is_empty() {
        let n = missing.len();
        let started = std::thread::Builder::new().name("warp-pictures".into()).spawn(move || {
            let t0 = Instant::now();
            for k in &missing {
                let slot = match std::panic::catch_unwind(|| read_band(k)).ok().flatten() {
                    Some(b) => Slot::Ready(Arc::new(b), Instant::now()),
                    None => {
                        eprintln!("nspanel-app: warp: {} could not be read", k.0);
                        Slot::Failed
                    }
                };
                if let Ok(mut st) = store().lock() {
                    st.insert(*k, slot);
                }
            }
            eprintln!("nspanel-app: warp: {n} pictures read in {} ms", t0.elapsed().as_millis());
        });
        if started.is_err() {
            if let Ok(mut st) = store().lock() {
                for k in keys {
                    if matches!(st.get(k), Some(Slot::Loading)) {
                        st.insert(*k, Slot::Failed);
                    }
                }
            }
        }
    }
    got
}

/// Reads a band of a PNG picture of the UI bundle: only the rows down to the band's last are decoded.
fn read_band(key: &BandKey) -> Option<Band> {
    let &(path, x, y, w, h) = key;
    let bytes = std::fs::read(std::path::Path::new(env!("PANELOS_UI_PICTURES")).join(path)).ok()?;
    decode_band(&bytes, x, y, w, h)
}

fn decode_band(bytes: &[u8], x: i32, y: i32, w: usize, h: usize) -> Option<Band> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().ok()?;
    let (pw, ph) = reader.info().size();
    let interlaced = reader.info().interlaced;
    let ch = match reader.output_color_type().0 {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        _ => return None,
    };
    let line = reader.output_line_size(pw)?;
    let mut band = Band::blank(x, y, w, h);
    let put = |band: &mut Band, ry: i32, data: &[u8]| {
        let out = band.row_mut(ry);
        for cx in x.max(0)..(x + w as i32).min(pw as i32) {
            let p = &data[cx as usize * ch..cx as usize * ch + ch];
            let (r, g, b, a) = match ch {
                4 => (p[0], p[1], p[2], p[3]),
                3 => (p[0], p[1], p[2], 255),
                2 => (p[0], p[0], p[0], p[1]),
                _ => (p[0], p[0], p[0], 255),
            };
            let o = ((cx - x) as usize + PAD) * 4;
            let a32 = a as u32;
            out[o] = div255(r as u32 * a32) as u8;
            out[o + 1] = div255(g as u32 * a32) as u8;
            out[o + 2] = div255(b as u32 * a32) as u8;
            out[o + 3] = a;
        }
    };
    let last = (y + h as i32).min(ph as i32);
    if interlaced {
        let mut buf = vec![0; reader.output_buffer_size()?];
        reader.next_frame(&mut buf).ok()?;
        for ry in y.max(0)..last {
            put(&mut band, ry, &buf[ry as usize * line..(ry as usize + 1) * line]);
        }
    } else {
        let mut row = vec![0; line];
        for ry in 0..last {
            reader.read_row(&mut row).ok()??;
            if ry >= y {
                put(&mut band, ry, &row);
            }
        }
    }
    Some(band)
}

// --- scenes -----------------------------------------------------------------------------------------------------

/// The light pictures mixed with their opacities: one picture as it is, or several over one another.
enum Mixed {
    One(Arc<Band>),
    Own(Band),
}

impl Mixed {
    fn band(&self) -> &Band {
        match self {
            Mixed::One(b) => b,
            Mixed::Own(b) => b,
        }
    }
}

/// A mix kept until its pictures or opacities change.
struct Mix {
    from: [Option<usize>; 4],
    weights: [u32; 4],
    mixed: Mixed,
}

fn mix(cache: &mut Option<Mix>, srcs: &[Option<Arc<Band>>; 4], weights: [u32; 4]) -> Option<()> {
    let from = std::array::from_fn(|i| if weights[i] > 0 { srcs[i].as_ref().map(|b| Arc::as_ptr(b) as usize) } else { None });
    if cache.as_ref().is_some_and(|c| c.from == from && c.weights == weights) {
        return Some(());
    }
    let used: Vec<usize> = (0..4).filter(|&i| weights[i] > 0).collect();
    let first = srcs[*used.first()?].as_ref()?;
    let mixed = if used.len() == 1 && weights[used[0]] >= 256 {
        Mixed::One(first.clone())
    } else {
        let mut out = Band::blank(first.x, first.y, first.w, first.h);
        for &i in &used {
            let b = srcs[i].as_ref()?;
            if (b.x, b.y, b.w, b.h) != (out.x, out.y, out.w, out.h) {
                return None;
            }
            over_with(&mut out.px, &b.px, weights[i]);
        }
        Mixed::Own(out)
    };
    *cache = Some(Mix { from, weights, mixed });
    Some(())
}

/// Two frame buffers that take turns: the page still shows one while the next is drawn into the other, and once the page
/// lets the older picture go its buffer is drawn into again without a new allocation.
#[derive(Default)]
struct Buffers {
    bufs: [Option<SharedPixelBuffer<Rgba8Pixel>>; 2],
    next: usize,
}

impl Buffers {
    fn take(&mut self, w: u32, h: u32) -> SharedPixelBuffer<Rgba8Pixel> {
        self.bufs[self.next].take().filter(|b| b.width() == w && b.height() == h).unwrap_or_else(|| SharedPixelBuffer::new(w, h))
    }

    fn give_back(&mut self, buf: SharedPixelBuffer<Rgba8Pixel>) -> Image {
        let img = Image::from_rgba8_premultiplied(buf.clone());
        self.bufs[self.next] = Some(buf);
        self.next ^= 1;
        img
    }

    fn bytes(&self) -> usize {
        self.bufs.iter().flatten().map(|b| b.as_bytes().len()).sum()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    theme: i32,
    mood: i32,
    nmood: i32,
    light: bool,
    unlit: bool,
}

struct Scene {
    land: Option<Mix>,
    trees: Vec<Option<Mix>>,
    buffers: Buffers,
    scratch: Vec<u8>,
    last: Option<(WarpIn, Image)>,
    used: Instant,
}

#[derive(Default)]
struct Stats {
    since: Option<Instant>,
    ms: Vec<f32>,
    waiting: u32,
    size: (usize, usize),
}

thread_local! {
    static SCENES: RefCell<HashMap<Key, Scene>> = RefCell::new(HashMap::new());
    static STATS: RefCell<Stats> = RefCell::new(Stats::default());
    static BROKEN: Cell<bool> = const { Cell::new(false) };
}

/// Development: the app's pictures off, so the scenes draw their strips again ({"warp": false} on the development
/// input, to compare the look and the cost).
static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    eprintln!("nspanel-app: warp: the scenes' moving rows drawn by {}", if on { "the app" } else { "the scenes' strips" });
}

/// The page's question (`Warp.frame`): the picture of the moving rows for this snapshot, or an empty picture when the
/// page draws its strips itself (nothing moves, the pictures are still loading or could not be read).
pub fn frame(w: &WarpIn) -> Image {
    if BROKEN.with(Cell::get) || !ENABLED.load(Ordering::Relaxed) {
        return Image::default();
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(w))) {
        Ok(img) => img.unwrap_or_default(),
        Err(_) => {
            BROKEN.with(|b| b.set(true));
            eprintln!("nspanel-app: warp: drawing failed, the scenes draw their strips from now on");
            Image::default()
        }
    }
}

fn render(w: &WarpIn) -> Option<Image> {
    let p = parts(w);
    let has_land = p.grass || p.water;
    if !has_land && p.trees.is_empty() {
        return None;
    }
    let fr = Frame { x0: w.x0, top: w.top, w: w.w.max(0) as usize, h: (w.bottom - w.top).max(0) as usize };
    if fr.w == 0 || fr.h == 0 || w.x0 < 0 || w.x0 + w.w > LAYER || w.top < 0 || w.bottom > LAYER {
        return None;
    }
    let land_y0 = if p.water { w.water_y0 } else { w.grass_y0 };
    if has_land && (land_y0 < w.top || land_y0 >= w.bottom) {
        return None;
    }
    let key = Key { theme: w.theme, mood: w.mood, nmood: w.nmood, light: w.light, unlit: w.unlit };
    SCENES.with(|cell| {
        let mut scenes = cell.borrow_mut();
        let scene = scenes.entry(key).or_insert_with(|| Scene {
            land: None,
            trees: Vec::new(),
            buffers: Buffers::default(),
            scratch: Vec::new(),
            last: None,
            used: Instant::now(),
        });
        scene.used = Instant::now();
        if let Some((last, img)) = &scene.last {
            if last == w {
                return Some(img.clone());
            }
        }

        // the pictures: every light picture the scene may need is asked for (read once, in a thread); the ones with an
        // opacity now must be there
        let land_rows = (w.bottom - land_y0).max(0) as usize;
        let land_keys: [Option<BandKey>; 4] =
            paths(w, false).map(|path| path.filter(|_| has_land).map(|p| (p, 0, land_y0, LAYER as usize, land_rows)));
        let tree_paths = paths(w, true);
        let tree_keys: Vec<[Option<BandKey>; 4]> = p
            .trees
            .iter()
            .map(|tree| tree_paths.map(|path| path.map(|p| (p, tree.x, tree.y0, tree.w as usize, (tree.strips() * 5) as usize))))
            .collect();
        let m = motion(w);
        let sheen_key: Option<BandKey> = (p.grass && m.sheen > 0).then_some((SHEEN, 0, 0, SHEEN_W, SHEEN_H));
        let mut keys: Vec<BandKey> = land_keys.iter().flatten().copied().collect();
        keys.extend(tree_keys.iter().flatten().flatten().copied());
        keys.extend(sheen_key);
        let got = fetch(&keys);
        let find = |k: &Option<BandKey>| k.and_then(|k| keys.iter().position(|x| *x == k)).map(|i| got[i].clone());
        let ready = |k: &Option<BandKey>| match find(k) {
            Some(Got::Ready(b)) => Some(b),
            _ => None,
        };
        // the pictures with an opacity now: all there, or the page draws its strips meanwhile
        let land_w = weights(w, false);
        let tree_w = weights(w, true);
        let mut needed: Vec<Option<Got>> = Vec::new();
        if has_land {
            needed.extend((0..4).filter(|&i| land_w[i] > 0).map(|i| find(&land_keys[i])));
        }
        for tk in &tree_keys {
            needed.extend((0..4).filter(|&i| tree_w[i] > 0).map(|i| find(&tk[i])));
        }
        if sheen_key.is_some() {
            needed.push(find(&sheen_key));
        }
        if needed.iter().any(|g| matches!(g, Some(Got::Waiting))) {
            STATS.with(|s| s.borrow_mut().waiting += 1);
            return None;
        }
        if !needed.iter().all(|g| matches!(g, Some(Got::Ready(_)))) {
            return None;
        }
        if has_land {
            let srcs: [Option<Arc<Band>>; 4] = std::array::from_fn(|i| ready(&land_keys[i]));
            mix(&mut scene.land, &srcs, land_w)?;
        }
        scene.trees.resize_with(p.trees.len(), || None);
        for (n, tk) in tree_keys.iter().enumerate() {
            let srcs: [Option<Arc<Band>>; 4] = std::array::from_fn(|i| ready(&tk[i]));
            mix(&mut scene.trees[n], &srcs, tree_w)?;
        }
        let sheen = ready(&sheen_key);

        let t0 = Instant::now();
        let land_band = if has_land { scene.land.as_ref().map(|m| m.mixed.band()) } else { None };
        let land = land_band.map(|band| if p.grass { Land::Grass { band, sheen: sheen.as_deref() } } else { Land::Water { band } });
        let tree_bands: Vec<(Tree, &Band)> =
            p.trees.iter().zip(&scene.trees).filter_map(|(t, mx)| mx.as_ref().map(|mx| (*t, mx.mixed.band()))).collect();
        if tree_bands.len() != p.trees.len() {
            return None;
        }
        let mut buf = scene.buffers.take(fr.w as u32, fr.h as u32);
        compose(buf.make_mut_bytes(), &fr, land.as_ref(), &tree_bands, &m, &mut scene.scratch);
        drop(tree_bands);
        let img = scene.buffers.give_back(buf);
        scene.last = Some((w.clone(), img.clone()));
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        STATS.with(|s| {
            let mut s = s.borrow_mut();
            s.ms.push(ms);
            s.size = (fr.w, fr.h);
        });
        Some(img)
    })
}

/// Once a second: scenes no longer asked for let their buffers go, pictures unused for a minute are dropped; with the
/// frame statistics on, every 5 s a line with the frames made and their cost.
pub fn tick() {
    let now = Instant::now();
    SCENES.with(|cell| cell.borrow_mut().retain(|_, s| now.duration_since(s.used) < Duration::from_secs(5)));
    if let Ok(mut st) = store().lock() {
        st.retain(|_, slot| !matches!(slot, Slot::Ready(_, used) if now.duration_since(*used) > Duration::from_secs(60)));
    }
    if !stats_on() {
        STATS.with(|s| *s.borrow_mut() = Stats::default());
        return;
    }
    STATS.with(|s| {
        let mut s = s.borrow_mut();
        let since = *s.since.get_or_insert(now);
        let secs = now.duration_since(since).as_secs_f32();
        if secs < 5.0 {
            return;
        }
        if !s.ms.is_empty() || s.waiting > 0 {
            let n = s.ms.len();
            let (p50, p99, max) = (pct(&mut s.ms, 0.5), pct(&mut s.ms, 0.99), pct(&mut s.ms, 1.0));
            let pictures: usize = store()
                .lock()
                .map(|st| st.values().map(|v| if let Slot::Ready(b, _) = v { b.px.len() } else { 0 }).sum())
                .unwrap_or(0);
            let (buffers, mixes) = SCENES.with(|cell| {
                let scenes = cell.borrow();
                let buffers: usize = scenes.values().map(|sc| sc.buffers.bytes()).sum();
                let own = |m: &Option<Mix>| match m {
                    Some(Mix { mixed: Mixed::Own(b), .. }) => b.px.len(),
                    _ => 0,
                };
                let mixes: usize = scenes.values().map(|sc| own(&sc.land) + sc.trees.iter().map(own).sum::<usize>()).sum();
                (buffers, mixes)
            });
            eprintln!(
                "nspanel-app: warp {:.1} frames/s {}x{} | draw p50 {p50:.2} p99 {p99:.2} max {max:.2} ms | waited for pictures {} | memory: pictures {} KB, mixes {} KB, buffers {} KB",
                n as f32 / secs,
                s.size.0,
                s.size.1,
                s.waiting,
                pictures / 1024,
                mixes / 1024,
                buffers / 1024
            );
        }
        *s = Stats { since: Some(now), ..Stats::default() };
    });
}

/// Development: the frame statistics are on.
fn stats_on() -> bool {
    #[cfg(feature = "gpu")]
    {
        crate::gpu::FRAME_STATS.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(feature = "gpu"))]
    {
        false
    }
}

fn pct(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v[((v.len() as f32 - 1.0) * p).round() as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(theme: i32) -> WarpIn {
        WarpIn {
            theme,
            mood: 6,
            nmood: 7,
            light: false,
            unlit: false,
            t: 12.3,
            x0: 24,
            w: 480,
            top: if theme == 5 { 245 } else if theme == 3 { 246 } else { 405 },
            bottom: 504,
            day_hidden: false,
            has_grade: false,
            dk: 0.0,
            dn: 0.0,
            nmix: 0.0,
            grass_y0: if theme == 5 { 440 } else { 428 },
            sway: 2.6,
            bend: 1.0,
            water_y0: 405,
            water: 1.7,
            gust_k: 1.0,
        }
    }

    /// A band filled by f(column, row) (straight RGBA, premultiplied on the way in).
    fn band(x: i32, y: i32, w: usize, h: usize, f: impl Fn(i32, i32) -> [u8; 4]) -> Band {
        let mut b = Band::blank(x, y, w, h);
        for ry in y..y + h as i32 {
            let row = b.row_mut(ry);
            for cx in x..x + w as i32 {
                let [r, g, bl, a] = f(cx, ry);
                let o = ((cx - x) as usize + PAD) * 4;
                row[o..o + 4].copy_from_slice(&[div255(r as u32 * a as u32) as u8, div255(g as u32 * a as u32) as u8, div255(bl as u32 * a as u32) as u8, a]);
            }
        }
        b
    }

    #[test]
    fn the_gust_is_one_pulse_every_eleven_seconds() {
        assert!(genv(0.0, 1.0).abs() < 1e-9);
        assert!((genv(9.5, 1.0) - 1.0).abs() < 1e-9);
        assert!((genv(20.5, 0.5) - 0.5).abs() < 1e-9);
        assert_eq!(genv(3.0, 1.0), 0.0);
        assert_eq!(genv(5.0, 1.0), 0.0);
    }

    #[test]
    fn strips_shift_as_on_the_page() {
        // the meadow: at t 0 the first strip is still; a quarter period later its full sway, deeper strips more
        assert!(grass_shift(0, 428, 0.0, 2.6, 0.0).abs() < 1e-9);
        let d0 = (0.5f64 / 12.5).powf(1.2);
        assert!((grass_shift(0, 428, 2.75 / 4.0, 2.6, 0.0) - 2.6 * d0).abs() < 1e-9);
        assert!(grass_shift(9, 428, 0.3, 2.6, 0.0).abs() <= 2.6 * (9.5f64 / 12.5).powf(1.2) + 1e-9);
        // a gust pushes to the left
        assert!(grass_shift(5, 428, 0.0, 2.6, 1.0) < grass_shift(5, 428, 0.0, 2.6, 0.0));
        // the river stays within its strength
        for k in 0..33 {
            assert!(water_shift(k, 7.7, 1.7).abs() <= 1.7 + 1e-9);
        }
        assert!((water_shift(0, 0.0, 1.7)).abs() < 1e-9);
        assert!((water_shift(1, 0.0, 1.0) - (67f64).to_radians().sin()).abs() < 1e-9);
        // a tree: the foot stays, the top bends left by lean (and its flutter)
        let tree = COUNTRYSIDE_TREES[0];
        assert_eq!(tree.strips(), 27);
        assert_eq!(tree_shift(&tree, 26, 3.0, 7.0, 2.8), 0.0);
        let top = tree_shift(&tree, 0, 0.0, 7.0, 0.0);
        let rel = (132.0f64 - 2.5) / 132.0;
        assert!((top + 7.0 * rel.powf(1.8)).abs() < 1e-9);
        assert_eq!(NEUTRAL_TREES[0].strips(), 45);
        assert_eq!(NEUTRAL_TREES[1].strips(), 43);
    }

    #[test]
    fn motion_follows_the_scene() {
        let mut w = snapshot(3);
        w.t = 9.5; // the gust at its peak
        let m = motion(&w);
        assert!((m.gust - 1.0).abs() < 1e-6);
        assert!((m.flut - (1.8 + 1.0)).abs() < 1e-6);
        assert!(m.lean >= 3.0 + 6.0 - 1e-6 && m.lean <= 7.0 + 6.0 + 1e-6);
        assert_eq!(m.sheen, 256);
        assert!(m.sheen_x <= 0.0 && m.sheen_x > -528.0);
        // a breeze: half the sheen's strength, no gust out of the pulse
        w.sway = 1.3;
        w.t = 5.0;
        let m = motion(&w);
        assert_eq!(m.gust, 0.0);
        assert_eq!(m.sheen, (0.6f64 * 256.0).round() as u32);
    }

    #[test]
    fn what_moves() {
        let mut w = snapshot(3);
        assert_eq!(parts(&w), Parts { grass: true, water: false, trees: &COUNTRYSIDE_TREES });
        w.bend = 0.0;
        assert_eq!(parts(&w), Parts { grass: true, water: false, trees: &[] });
        w.sway = 0.0;
        assert_eq!(parts(&w), Parts { grass: false, water: false, trees: &[] });
        // the city's river moves even in calm weather; the photo city and the line drawing have nothing that moves
        let mut c = snapshot(4);
        c.sway = 0.0;
        c.bend = 0.0;
        c.water = 0.8;
        assert_eq!(parts(&c), Parts { grass: false, water: true, trees: &[] });
        for theme in [2, 7] {
            let mut x = snapshot(theme);
            x.water = 1.0;
            assert_eq!(parts(&x), Parts { grass: false, water: false, trees: &[] });
        }
        assert_eq!(parts(&snapshot(5)).trees.len(), 2);
        // a page that is not asked about draws nothing: an empty picture
        let mut still = snapshot(6);
        still.sway = 0.0;
        still.bend = 0.0;
        still.water = 0.0;
        assert_eq!(frame(&still).size().width, 0);
    }

    #[test]
    fn light_pictures_mix_as_on_the_page() {
        let mut w = snapshot(3);
        assert_eq!(weights(&w, false), [256, 0, 0, 0]);
        // dusk light on a sunny evening
        w.has_grade = true;
        w.dk = 0.5;
        assert_eq!(weights(&w, false), [256, 128, 0, 0]);
        // full night: the land leaves out the day under it and the dusk picture; the trees keep both
        w.nmix = 1.0;
        w.day_hidden = true;
        assert_eq!(weights(&w, false), [0, 0, 0, 256]);
        assert_eq!(weights(&w, true), [256, 128, 0, 256]);
        // without dusk and dawn pictures (a rainy day) only day and night
        w.has_grade = false;
        assert_eq!(weights(&w, true), [256, 0, 0, 256]);
        // the pictures asked for: dusk and dawn only for the sunny and partly cloudy conditions
        let mut p = snapshot(3);
        assert_eq!(paths(&p, false), [Some("wx/vcountryside-wind-fg.png"), None, None, Some("wx/vcountryside-clear-night-fg.png")]);
        p.mood = 0;
        p.has_grade = true;
        p.light = true;
        let tp = paths(&p, true);
        assert_eq!(tp[0], Some("wx-light/vcountryside-sunny-trees-light.png"));
        assert_eq!(tp[2], Some("wx-light/vcountryside-sunny-dawn-trees-light.png"));
    }

    #[test]
    fn rows_shift_by_whole_and_part_pixels() {
        let b = band(0, 0, 528, 1, |x, _| [(x % 256) as u8, 0, 0, 255]);
        let row = b.row(0);
        let mut out = vec![0u8; 480 * 4];
        shift_row(&mut out, row, 0, 24, 2.0);
        assert_eq!(out[0], 22);
        assert_eq!(out[100 * 4], 122);
        shift_row(&mut out, row, 0, 24, -3.0);
        assert_eq!(out[0], 27);
        // half a pixel: the mean of the neighbours; a fraction that rounds up to a whole pixel moves one more
        shift_row(&mut out, row, 0, 24, 0.5);
        assert_eq!(out[0], 24); // (24 + 23) / 2, rounded up
        shift_row(&mut out, row, 0, 24, 0.999);
        assert_eq!(out[0], 23);
        assert_eq!(out[3], 255);
        // a narrow band (a tree): columns it does not reach are clear
        let t = band(100, 0, 10, 1, |_, _| [200, 100, 50, 255]);
        let mut out = vec![9u8; 40 * 4];
        shift_row(&mut out, t.row(0), 100, 90, 0.0);
        assert_eq!(&out[..4], &[0, 0, 0, 0]);
        assert_eq!(&out[10 * 4..10 * 4 + 4], &[200, 100, 50, 255]);
        assert_eq!(&out[20 * 4..20 * 4 + 4], &[0, 0, 0, 0]);
        // with a part pixel the edge is antialiased: half covered
        shift_row(&mut out, t.row(0), 100, 90, 0.5);
        assert_eq!(out[10 * 4 + 3], 128);
        assert_eq!(out[11 * 4 + 3], 255);
        assert_eq!(out[20 * 4 + 3], 128);
    }

    #[test]
    fn interpolation_matches_the_exact_shift() {
        // against a straightforward float shift of an opaque row: never more than one level apart
        let b = band(0, 0, 528, 1, |x, _| [((x * 37) % 251) as u8, ((x * 11) % 256) as u8, (255 - x % 200) as u8, 255]);
        let mut out = vec![0u8; 480 * 4];
        for off in [-5.75f32, -1.2, 0.0, 0.33, 2.5, 4.9] {
            shift_row(&mut out, b.row(0), 0, 24, off);
            let (i, f) = (off.floor() as i32, (off - off.floor()) as f64);
            for p in 0..480 {
                for c in 0..4 {
                    let at = |x: i32| b.row(0)[((x + PAD as i32) * 4) as usize + c] as f64;
                    let x = 24 + p as i32 - i;
                    let want = at(x) * (1.0 - f) + at(x - 1) * f;
                    assert!((out[p * 4 + c] as f64 - want).abs() <= 1.0, "off {off} px {p} channel {c}");
                }
            }
        }
    }

    #[test]
    fn pictures_lie_over_one_another() {
        let mut d = vec![10, 20, 30, 255];
        over(&mut d, &[0, 0, 0, 0]);
        assert_eq!(d, vec![10, 20, 30, 255]);
        over(&mut d, &[200, 100, 50, 255]);
        assert_eq!(d, vec![200, 100, 50, 255]);
        // white at half alpha (premultiplied) over black
        let mut d = vec![0, 0, 0, 255];
        over(&mut d, &[128, 128, 128, 128]);
        assert_eq!(d, vec![128, 128, 128, 255]);
        // an opaque picture at half opacity: halfway
        let mut d = vec![0, 0, 0, 255];
        over_with(&mut d, &[200, 100, 50, 255], 128);
        assert_eq!(d, vec![100, 50, 25, 255]);
        // a light picture mixed over the day one with the page's opacities
        let day = Arc::new(band(0, 400, 4, 2, |_, _| [100, 100, 100, 255]));
        let dusk = Arc::new(band(0, 400, 4, 2, |_, _| [200, 0, 0, 255]));
        let mut cache = None;
        mix(&mut cache, &[Some(day.clone()), Some(dusk.clone()), None, None], [256, 64, 0, 0]).unwrap();
        let px = &cache.as_ref().unwrap().mixed.band().row(400)[PAD * 4..PAD * 4 + 4];
        assert_eq!(px, &[125, 75, 75, 255]);
        // a single picture at full opacity is used as it is
        mix(&mut cache, &[Some(day.clone()), Some(dusk), None, None], [256, 0, 0, 0]).unwrap();
        assert!(matches!(cache.as_ref().unwrap().mixed, Mixed::One(ref b) if Arc::ptr_eq(b, &day)));
    }

    /// A theme 5 frame from made-up pictures: opaque meadow with a sheen, two trees over it.
    fn neutral_frame() -> (Frame, Band, Band, Band, Band, Motion) {
        let w = snapshot(5);
        let fr = Frame { x0: 24, top: 245, w: 480, h: 259 };
        let land = band(0, 440, 528, 64, |x, y| [(x % 200) as u8, (y % 200) as u8, 90, 255]);
        let sheen = band(0, 0, 528, 200, |x, _| [255, 250, 217, (x % 34) as u8]);
        let tree_a = band(0, 245, 139, 225, |x, _| if x > 20 && x < 120 { [20, 80, 30, 255] } else { [0, 0, 0, 0] });
        let tree_b = band(408, 266, 120, 215, |x, _| if x > 430 { [20, 80, 30, 200] } else { [0, 0, 0, 0] });
        (fr, land, sheen, tree_a, tree_b, motion(&w))
    }

    #[test]
    fn a_frame_is_drawn_whole() {
        let (fr, land, sheen, tree_a, tree_b, m) = neutral_frame();
        let mut out = vec![77u8; fr.w * fr.h * 4];
        let mut scratch = Vec::new();
        let lg = Land::Grass { band: &land, sheen: Some(&sheen) };
        compose(&mut out, &fr, Some(&lg), &[(NEUTRAL_TREES[0], &tree_a), (NEUTRAL_TREES[1], &tree_b)], &m, &mut scratch);
        let px = |x: i32, y: i32| {
            let o = ((y - fr.top) as usize * fr.w + (x - fr.x0) as usize) * 4;
            [out[o], out[o + 1], out[o + 2], out[o + 3]]
        };
        // above the meadow, away from the trees: clear (the page's land shows); the meadow opaque; a tree's trunk there
        assert_eq!(px(260, 300), [0, 0, 0, 0]);
        assert_eq!(px(260, 450)[3], 255);
        assert_eq!(px(260, 503)[3], 255);
        assert_eq!(px(70, 460), [20, 80, 30, 255]);
        assert_eq!(px(70, 300), [20, 80, 30, 255]);
        // nothing of the old contents is left
        assert!(!out.chunks_exact(4).any(|p| p == [77, 77, 77, 77]));
        // the meadow's rows are its picture shifted (the foot of a tree has no shift: rel 0)
        let k = (460 - 440) / 8;
        let off = grass_shift(k, 440, m.t, m.sway, m.gust) as f32;
        let mut want = vec![0u8; 480 * 4];
        shift_row(&mut want, land.row(460), 0, 24, off);
        sheen_row(&mut want, &sheen, 460 - 440, 24, m.sheen_x, m.sheen);
        let o = (460 - fr.top) as usize * fr.w * 4;
        assert_eq!(&out[o + 300 * 4..o + 301 * 4], &want[300 * 4..301 * 4]);
    }

    #[test]
    fn buffers_take_turns_without_allocating() {
        let mut b = Buffers::default();
        let first = b.take(480, 100);
        let p1 = first.as_bytes().as_ptr();
        let img1 = b.give_back(first);
        let second = b.take(480, 100);
        let p2 = second.as_bytes().as_ptr();
        assert_ne!(p1, p2);
        let img2 = b.give_back(second);
        // the page let the first picture go: its buffer is drawn into again, no new memory
        drop(img1);
        let mut third = b.take(480, 100);
        assert_eq!(third.make_mut_bytes().as_ptr(), p1);
        let _img3 = b.give_back(third);
        // still shown (img2 held): the other one is copied rather than overwritten
        let mut fourth = b.take(480, 100);
        assert_ne!(fourth.make_mut_bytes().as_ptr(), p2);
        drop(img2);
        // another size: a new buffer
        assert_eq!(b.take(480, 50).height(), 50);
    }

    #[test]
    fn every_picture_the_scenes_ask_for_is_there() {
        let dir = std::path::Path::new(env!("PANELOS_UI_PICTURES"));
        let mut all: Vec<&str> = vec![SHEEN];
        for t in [0, 1, 3, 4, 5, 6] {
            for m in 0..pics::FG[t].len() {
                all.extend(pics::FG[t][m]);
                if t == 3 || t == 5 {
                    all.extend(pics::TREES[t][m]);
                }
            }
            for m in 0..2 {
                for k in 0..2 {
                    all.extend(pics::GFG[t][m][k]);
                    if t == 3 || t == 5 {
                        all.extend(pics::GTREES[t][m][k]);
                    }
                }
            }
        }
        for p in all {
            assert!(dir.join(p).exists(), "{p}");
        }
    }

    #[test]
    fn a_band_is_read_premultiplied() {
        let b = read_band(&(SHEEN, 0, 0, SHEEN_W, SHEEN_H)).expect("sheen");
        assert_eq!((b.w, b.h), (528, 200));
        let row = b.row(100);
        assert_eq!(&row[..PAD * 4], &[0u8; PAD * 4][..]);
        assert!(row.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
        assert!(row.chunks_exact(4).any(|p| p[3] > 0));
        // only a band of the rows: the meadow of the drawn countryside
        let fg = read_band(&("wx/vcountryside-wind-fg.png", 0, 428, 528, 76)).expect("meadow");
        assert!(fg.row(428)[PAD * 4..(PAD + 528) * 4].chunks_exact(4).all(|p| p[3] == 255));
    }

    /// The cost of one strong-wind frame of the neutral scene (480 x 259: meadow, sheen, two trees) on this machine:
    /// `cargo test --release warp::tests::cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn cost() {
        let (fr, land, sheen, tree_a, tree_b, mut m) = neutral_frame();
        let mut out = vec![0u8; fr.w * fr.h * 4];
        let mut scratch = Vec::new();
        let n = 300;
        let t0 = Instant::now();
        for i in 0..n {
            m.t = 12.3 + i as f64 / 30.0;
            let lg = Land::Grass { band: &land, sheen: Some(&sheen) };
            compose(&mut out, &fr, Some(&lg), &[(NEUTRAL_TREES[0], &tree_a), (NEUTRAL_TREES[1], &tree_b)], &m, &mut scratch);
        }
        println!("warp frame 480x259 (meadow, sheen, two trees): {:.3} ms", t0.elapsed().as_secs_f64() * 1000.0 / n as f64);
    }
}
