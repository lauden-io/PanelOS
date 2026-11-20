//! Rain on the glass: drops that land on the panel's glass over the photo
//! backgrounds while it rains (Home Assistant's weather), sit, merge, and the
//! heavy ones run down, stretched, leaving small beads and a fading wet streak;
//! the wind bends their path. When the rain stops they dry up. A finger stroke
//! up or down wipes them away along its path (they come back with the rain).
//!
//! This module simulates the drops on the CPU and hands them out as instances;
//! `glass_gl.rs` draws them on the GPU over the frame Skia has just finished,
//! each drop a small lens showing an inverted, wider view of what is under it
//! (text included). The page tells how much of the screen shows a photo
//! background with nothing over it (`glass-k`); hidden, the glass stands still
//! and asks for no frames.

// the software renderer's build simulates nothing (the drops are drawn by the GPU)
#![cfg_attr(not(feature = "gpu"), allow(dead_code))]

use std::cell::RefCell;
use std::time::Instant;

/// Half the width of the strip a finger wipes (px).
const FINGER: f32 = 18.0;
/// Size of the glass (the panel's screen).
const SIZE: (f32, f32) = (480.0, 480.0);
/// Away from the screen longer than this, the glass is set again from the weather (s).
const AWAY_RESET_S: f32 = 20.0;
/// At most this many beads and streak points (the oldest go first): the GPU pass stays small in a downpour.
const MAX_BEADS: usize = 400;
const MAX_TRAIL: usize = 1500;

struct Drop {
    x: f32,
    y: f32,
    r: f32,
    vy: f32,
    phase: f32,
    sliding: bool,
    since_bead: f32,
    next_bead: f32,
    since_trail: f32,
    /// 1 = fully there, falls to 0 while drying.
    fade: f32,
    dry_rate: f32,
}

struct Bead {
    x: f32,
    y: f32,
    r: f32,
    age: f32,
    life: f32,
}

struct TrailPt {
    x: f32,
    y: f32,
    w: f32,
    age: f32,
    life: f32,
}

/// The drops on the glass.
pub struct Glass {
    w: f32,
    h: f32,
    drops: Vec<Drop>,
    beads: Vec<Bead>,
    trail: Vec<TrailPt>,
    rng: u64,
    spawn_acc: f32,
    /// Water the finger gathered during a wipe (volume, r³) and its direction.
    gathered: f32,
    wipe_dir: (f32, f32),
    // reused every step (no allocation per frame)
    new_beads: Vec<Bead>,
    new_trail: Vec<TrailPt>,
    rolls: Vec<f32>,
}

/// Instance floats: x, y, rx (streak: half width), ry, alpha, kind (0 streak, 1 bead, 2 drop).
pub const INSTANCE_FLOATS: usize = 6;

impl Glass {
    pub fn new(w: f32, h: f32, seed: u64) -> Self {
        Glass {
            w,
            h,
            drops: Vec::new(),
            beads: Vec::new(),
            trail: Vec::new(),
            rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            spawn_acc: 0.0,
            gathered: 0.0,
            wipe_dir: (0.0, 1.0),
            new_beads: Vec::new(),
            new_trail: Vec::new(),
            rolls: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.drops.clear();
        self.beads.clear();
        self.trail.clear();
        self.gathered = 0.0;
    }

    pub fn is_empty(&self) -> bool {
        self.drops.is_empty() && self.beads.is_empty() && self.trail.is_empty()
    }

    /// Drops, beads and streak points on the glass.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.drops.len(), self.beads.len(), self.trail.len())
    }

    fn rnd(&mut self) -> f32 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        ((self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / ((1u64 << 24) as f32)
    }

    /// Advances the glass by `dt` seconds. level: 0 no rain (the drops dry up), 1 drizzle, 2 rain, 3 pouring.
    /// wind: 0..1 (more in gusts), pushes running drops to the left (the scenes' wind comes from the right).
    pub fn step(&mut self, dt: f32, level: u8, wind: f32) {
        let level = level.min(3) as usize;
        let rate = [0.0, 2.6, 6.5, 13.0][level];
        let max_drops = [0, 60, 110, 160][level];
        self.spawn_acc += rate * dt;
        while self.spawn_acc >= 1.0 {
            self.spawn_acc -= 1.0;
            if self.drops.len() >= max_drops.max(1) {
                // full: the smallest drop sitting still dries quicker
                if let Some(d) = self.drops.iter_mut().filter(|d| !d.sliding && d.dry_rate == 0.0).min_by(|a, b| a.r.total_cmp(&b.r)) {
                    d.dry_rate = 0.5;
                }
            }
            let u = self.rnd();
            let r = 1.4 + 7.5 * u * u;
            let x = self.rnd() * self.w;
            let y = self.rnd() * self.h * 0.97;
            let phase = self.rnd() * std::f32::consts::TAU;
            let nb = 6.0 + self.rnd() * 9.0;
            self.drops.push(Drop { x, y, r, vy: 0.0, phase, sliding: false, since_bead: 0.0, next_bead: nb, since_trail: 0.0, fade: 0.0, dry_rate: 0.0 });
        }

        // touching drops merge: the bigger one takes the smaller one's water
        let n = self.drops.len();
        for i in 0..n {
            if self.drops[i].r <= 0.0 {
                continue;
            }
            for j in (i + 1)..n {
                if self.drops[j].r <= 0.0 {
                    continue;
                }
                let (a, b) = (&self.drops[i], &self.drops[j]);
                let (dx, dy) = (a.x - b.x, a.y - b.y);
                let lim = (a.r + b.r) * 0.85;
                if dx * dx + dy * dy < lim * lim {
                    let (va, vb) = (a.r.powi(3), b.r.powi(3));
                    let (big, small) = if a.r >= b.r { (i, j) } else { (j, i) };
                    let t = vb / (va + vb);
                    let nx = a.x * (1.0 - t) + b.x * t;
                    let ny = a.y * (1.0 - t) + b.y * t;
                    let nr = (va + vb).cbrt();
                    let fade = a.fade.max(b.fade);
                    let d = &mut self.drops[big];
                    d.x = nx;
                    d.y = ny;
                    d.r = nr;
                    d.fade = fade;
                    d.dry_rate = 0.0;
                    self.drops[small].r = 0.0;
                    if small == i {
                        break;
                    }
                }
            }
        }
        self.drops.retain(|d| d.r > 0.0);

        // move, leave beads and the wet streak, dry
        let rain_on = level > 0;
        let mut rolls = std::mem::take(&mut self.rolls);
        rolls.clear();
        for _ in 0..self.drops.len() * 3 {
            rolls.push(self.rnd());
        }
        let mut new_beads = std::mem::take(&mut self.new_beads);
        let mut new_trail = std::mem::take(&mut self.new_trail);
        for (k, d) in self.drops.iter_mut().enumerate() {
            // a new drop shows quickly; a drying one fades
            if d.dry_rate == 0.0 {
                d.fade = (d.fade + dt * 6.0).min(1.0);
            }
            if !d.sliding && d.r > 6.8 {
                d.sliding = true;
            }
            if d.sliding {
                let acc = 160.0 * ((d.r - 4.2) / 5.0).clamp(0.0, 1.6);
                let vmax = 35.0 + 22.0 * d.r;
                d.vy = (d.vy + acc * dt).min(vmax);
                // stick-slip: now and then the drop is held for a moment
                if rolls[k * 3] < 0.9 * dt {
                    d.vy *= 0.25;
                }
                let dy = d.vy * dt;
                d.y += dy;
                d.phase += dt * (1.5 + rolls[k * 3 + 1]);
                // a wavy path, bent by the wind
                d.x += (d.phase.sin() * 0.35 + (rolls[k * 3 + 2] - 0.5) * 0.3) * dy.min(3.0) * 0.5 - wind * dy * 0.6;
                d.since_bead += dy;
                d.since_trail += dy;
                // one streak point per pixel row (they must not overlap: each one darkens what is under it)
                while d.since_trail > 1.0 {
                    d.since_trail -= 1.0;
                    new_trail.push(TrailPt { x: d.x, y: d.y - d.r * 0.6 - d.since_trail, w: (d.r * 0.42).max(1.0), age: 0.0, life: 1.4 + d.r * 0.08 });
                }
                if d.since_bead > d.next_bead {
                    d.since_bead = 0.0;
                    d.next_bead = 6.0 + rolls[k * 3 + 1] * 10.0;
                    let br = 0.7 + rolls[k * 3 + 2] * (d.r * 0.22).min(1.8);
                    let v = (d.r.powi(3) - br.powi(3)).max(1.0);
                    d.r = v.cbrt();
                    new_beads.push(Bead { x: d.x + (rolls[k * 3] - 0.5) * 1.5, y: d.y - d.r * 1.2, r: br, age: 0.0, life: 4.0 + rolls[k * 3] * 6.0 });
                }
                if d.r < 4.0 {
                    d.sliding = false;
                    d.vy = 0.0;
                }
            } else if !rain_on && d.dry_rate == 0.0 {
                d.dry_rate = 0.06 + rolls[k * 3] * 0.10;
            }
            if d.dry_rate > 0.0 {
                d.fade -= d.dry_rate * dt;
                d.r *= 1.0 - 0.02 * dt;
            }
        }
        let (w, h) = (self.w, self.h);
        self.drops.retain(|d| d.y - d.r * 2.0 < h && d.x > -20.0 && d.x < w + 20.0 && d.r > 0.4 && (d.dry_rate == 0.0 || d.fade > 0.0));
        self.beads.append(&mut new_beads);
        self.trail.append(&mut new_trail);
        self.new_beads = new_beads;
        self.new_trail = new_trail;
        self.rolls = rolls;
        let bead_age = dt * if rain_on { 0.6 } else { 1.0 };
        for b in self.beads.iter_mut() {
            b.age += bead_age;
        }
        self.beads.retain(|b| b.age < b.life);
        for t in self.trail.iter_mut() {
            t.age += dt;
        }
        self.trail.retain(|t| t.age < t.life);
        if self.beads.len() > MAX_BEADS {
            let extra = self.beads.len() - MAX_BEADS;
            self.beads.drain(..extra);
        }
        if self.trail.len() > MAX_TRAIL {
            let extra = self.trail.len() - MAX_TRAIL;
            self.trail.drain(..extra);
        }
    }

    /// A finger moved from `a` to `b` on the glass: the water under it goes. Drops it pushes aside stay as small
    /// beads along the edges of its path; the rest gathers under the fingertip (see [`Glass::wipe_end`]).
    pub fn wipe(&mut self, a: (f32, f32), b: (f32, f32)) {
        let (sx, sy) = (b.0 - a.0, b.1 - a.1);
        let len2 = sx * sx + sy * sy;
        let len = len2.sqrt();
        if len > 0.5 {
            // the direction of the last real move (the water is pushed that way)
            self.wipe_dir = (sx / len, sy / len);
        }
        let dir = self.wipe_dir;
        // distance from the path, on which side of it a point lies, and the nearest point of the path
        let near = |x: f32, y: f32| -> (f32, f32, (f32, f32)) {
            let t = if len2 > 0.0 { (((x - a.0) * sx + (y - a.1) * sy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let (px, py) = (a.0 + sx * t, a.1 + sy * t);
            let (dx, dy) = (x - px, y - py);
            ((dx * dx + dy * dy).sqrt(), dx * dir.1 - dy * dir.0, (px, py))
        };
        let mut pushed: Vec<(f32, f32, f32)> = Vec::new();
        let mut gathered = 0.0;
        self.drops.retain(|d| {
            let (dist, side, (px, py)) = near(d.x, d.y);
            if dist >= FINGER + d.r * 0.5 {
                return true;
            }
            let v = d.r.powi(3) * d.fade.clamp(0.0, 1.0);
            if d.r > 2.0 {
                // a little of it stays at the edge of the stroke, on the drop's side
                let s = if side >= 0.0 { 1.0 } else { -1.0 };
                let br = (0.5 + d.r * 0.16).min(1.7);
                pushed.push((px + dir.1 * s * (FINGER + 1.5), py - dir.0 * s * (FINGER + 1.5), br));
                gathered += (v - br.powi(3)).max(0.0) * 0.6;
            } else {
                gathered += v * 0.6;
            }
            false
        });
        self.beads.retain(|b| near(b.x, b.y).0 >= FINGER);
        self.trail.retain(|t| near(t.x, t.y).0 >= FINGER + t.w);
        for (x, y, r) in pushed {
            let life = 3.0 + self.rnd() * 4.0;
            if x > 0.0 && x < self.w && y > 0.0 && y < self.h {
                self.beads.push(Bead { x, y, r, age: 0.0, life });
            }
        }
        self.gathered += gathered;
    }

    /// The finger left the glass at `at`: the water it gathered stays there as one drop just ahead of it (a big
    /// one runs down); nothing when the stroke ran off the glass.
    pub fn wipe_end(&mut self, at: (f32, f32)) {
        let v = std::mem::take(&mut self.gathered);
        if v < 2.0 {
            return;
        }
        let x = at.0 + self.wipe_dir.0 * FINGER * 0.5;
        let y = at.1 + self.wipe_dir.1 * FINGER * 0.5;
        let edge = 4.0;
        if x < edge || y < edge || x > self.w - edge || y > self.h - edge {
            return;
        }
        let r = v.cbrt().clamp(1.0, 8.0);
        let phase = self.rnd() * std::f32::consts::TAU;
        self.drops.push(Drop { x, y, r, vy: 0.0, phase, sliding: false, since_bead: 0.0, next_bead: 8.0, since_trail: 0.0, fade: 1.0, dry_rate: 0.0 });
    }

    /// The instances to draw, in drawing order (streaks, beads, drops), [`INSTANCE_FLOATS`] floats each.
    pub fn instances(&self, out: &mut Vec<f32>) {
        out.clear();
        for t in &self.trail {
            let a = (1.0 - t.age / t.life).max(0.0).powf(1.5) * 0.55;
            if a > 0.01 {
                out.extend_from_slice(&[t.x, t.y.floor(), t.w, 0.0, a, 0.0]);
            }
        }
        for b in &self.beads {
            let a = (1.0 - b.age / b.life).clamp(0.0, 1.0);
            if a > 0.01 && b.r >= 0.5 {
                out.extend_from_slice(&[b.x, b.y, b.r, b.r, a, 1.0]);
            }
        }
        for d in &self.drops {
            let stretch = if d.sliding { 1.0 + (d.vy / 80.0).min(0.9) } else { 1.0 };
            let rx = d.r * if d.sliding { 0.88 } else { 1.0 };
            let ry = d.r * stretch;
            let a = d.fade.clamp(0.0, 1.0);
            if a > 0.01 && rx >= 0.5 {
                out.extend_from_slice(&[d.x, d.y, rx, ry, a, 2.0]);
            }
        }
    }

    #[cfg(test)]
    fn add_drop(&mut self, x: f32, y: f32, r: f32) {
        self.drops.push(Drop { x, y, r, vy: 0.0, phase: 0.0, sliding: false, since_bead: 0.0, next_bead: 8.0, since_trail: 0.0, fade: 1.0, dry_rate: 0.0 });
    }
}

/// The rain on the glass for a scene: the rain level the weather gives (0 from the mood, as the photo's own rain
/// sheets do: rainy photos rain, thunderstorms pour), 0..3.
pub fn rain_level(rain: i32, mood: i32) -> u8 {
    if rain > 0 {
        rain.min(3) as u8
    } else if mood == 3 || mood == 11 {
        2
    } else if mood == 8 || mood == 9 {
        3
    } else {
        0
    }
}

/// How hard the wind bends the running drops, 0..1, from its speed (km/h): the scenes' rain leans one step
/// further at 15 and at 35 km/h, the drops' path leans with it.
pub fn wind_factor(kmh: f32) -> f32 {
    (kmh / 50.0).clamp(0.0, 1.0)
}

/// The glass as the app runs it: the weather, the setting, what the screen shows.
struct Controller {
    glass: Glass,
    /// The panel's setting (Rain on the glass).
    enabled: bool,
    /// The GPU pass draws (off: no GL, a development switch or a failed setup).
    pass: bool,
    level: u8,
    wind: f32,
    gust: f32,
    /// How much of the screen shows a photo background with nothing over it (0..1), read from the UI.
    visible: Option<Box<dyn Fn() -> f32>>,
    /// Told whether there are drops on the glass to wipe.
    wet_sink: Option<Box<dyn Fn(bool)>>,
    wet: bool,
    /// Something changed that the next frame must show (a wipe, the setting).
    dirty: bool,
    last: Option<Instant>,
    /// Off the screen since (None: on it); `fresh` until first shown.
    hidden: Option<Instant>,
    fresh: bool,
    /// Seconds the glass has run (the gusts' clock).
    clock: f32,
    next_gust: f32,
    inst: Vec<f32>,
    alpha: f32,
    wipe_at: Option<(f32, f32)>,
    // development: frame statistics
    sim_ms: Vec<f32>,
    pass_ms: Vec<f32>,
    frames: u32,
    stats_since: Option<Instant>,
}

impl Controller {
    fn new() -> Self {
        Controller {
            glass: Glass::new(SIZE.0, SIZE.1, 7),
            enabled: true,
            pass: std::env::var("PANELOS_GLASS").map_or(true, |v| v != "0"),
            level: 0,
            wind: 0.0,
            gust: 0.0,
            visible: None,
            wet_sink: None,
            wet: false,
            dirty: false,
            last: None,
            hidden: None,
            fresh: true,
            clock: 0.0,
            next_gust: 9.0,
            inst: Vec::new(),
            alpha: 0.0,
            wipe_at: None,
            sim_ms: Vec::new(),
            pass_ms: Vec::new(),
            frames: 0,
            stats_since: None,
        }
    }

    fn visible(&self) -> f32 {
        self.visible.as_ref().map_or(0.0, |f| f()).clamp(0.0, 1.0)
    }

    fn active(&self) -> bool {
        self.enabled && self.pass
    }

    /// The wind now: its speed, and now and then a gust when the gusts are clearly stronger (a 3 s swell every
    /// 9–15 s, like the scenes' rain).
    fn wind_now(&mut self, dt: f32) -> f32 {
        self.clock += dt;
        if self.clock > self.next_gust + 3.0 {
            let r = self.glass.rnd();
            self.next_gust = self.clock + 9.0 + r * 6.0;
        }
        let t = self.clock - self.next_gust;
        let swell = if (0.0..3.0).contains(&t) { (t / 3.0 * std::f32::consts::PI).sin() } else { 0.0 };
        self.wind + self.gust * swell
    }

    fn set_wet(&mut self) {
        let wet = self.active() && !self.glass.is_empty();
        if wet != self.wet {
            self.wet = wet;
            if let Some(f) = self.wet_sink.as_ref() {
                f(wet);
            }
        }
    }
}

thread_local! {
    static GLASS: RefCell<Controller> = RefCell::new(Controller::new());
}

/// Connects the glass to the UI: how much of the screen it covers, and where to say whether it is wet.
pub fn connect(visible: impl Fn() -> f32 + 'static, wet: impl Fn(bool) + 'static) {
    GLASS.with(|g| {
        let mut g = g.borrow_mut();
        g.visible = Some(Box::new(visible));
        g.wet_sink = Some(Box::new(wet));
    });
}

/// The weather over the photo (rain level 0..3, wind and gusts in km/h) and the panel's setting.
pub fn set_weather(enabled: bool, level: u8, wind_kmh: f32, gust_kmh: f32) {
    GLASS.with(|g| {
        let mut g = g.borrow_mut();
        if g.enabled != enabled {
            g.enabled = enabled;
            g.dirty = true;
            if !enabled {
                g.glass.clear();
                g.fresh = true;
            }
        }
        g.level = level;
        g.wind = wind_factor(wind_kmh);
        g.gust = if gust_kmh > wind_kmh + 8.0 { wind_factor(gust_kmh - wind_kmh).min(0.6) } else { 0.0 };
        g.set_wet();
    });
}

/// Development: the GPU pass on or off ({"glass_pass": false} on the dev input, PANELOS_GLASS=0); also off when
/// its GL setup failed.
pub fn set_pass(on: bool) {
    GLASS.with(|g| {
        let mut g = g.borrow_mut();
        if g.pass != on {
            g.pass = on;
            g.dirty = true;
            if !on {
                g.glass.clear();
                g.fresh = true;
            }
            g.set_wet();
        }
    });
}

/// Whether the glass needs a frame now: it is on screen and has drops (or rain), or a change waits to be shown
/// there (a wipe, drops cleared).
pub fn wants_frame() -> bool {
    GLASS.with(|g| {
        let g = g.borrow();
        (g.dirty || (g.active() && (g.level > 0 || !g.glass.is_empty()))) && g.visible() > 0.001
    })
}

/// Before a frame: the glass moves on to now and makes its instances.
pub fn prepare() {
    GLASS.with(|g| {
        let mut g = g.borrow_mut();
        let g = &mut *g;
        g.dirty = false;
        let now = Instant::now();
        let k = g.visible();
        if !g.active() || k <= 0.001 {
            if g.hidden.is_none() {
                g.hidden = Some(now);
            }
            g.last = None;
            g.inst.clear();
            g.wipe_at = None;
            return;
        }
        let away = g.hidden.take().map(|t| now.duration_since(t).as_secs_f32());
        if g.fresh || away.is_some_and(|s| s > AWAY_RESET_S) {
            // shown (again) after a while: the glass as the weather left it, the drops of the last few seconds
            g.fresh = false;
            g.glass.clear();
            if g.level > 0 {
                let wind = g.wind;
                for _ in 0..80 {
                    g.glass.step(0.1, g.level, wind);
                }
            }
        }
        let dt = g.last.map_or(1.0 / 60.0, |l| now.duration_since(l).as_secs_f32()).min(0.05);
        g.last = Some(now);
        let wind = g.wind_now(dt);
        g.glass.step(dt, g.level, wind);
        let mut inst = std::mem::take(&mut g.inst);
        g.glass.instances(&mut inst);
        g.inst = inst;
        g.alpha = k;
        g.set_wet();
        // (counted with the frames the pass draws, see note_pass)
        if stats_on() && !g.inst.is_empty() {
            g.sim_ms.push(now.elapsed().as_secs_f32() * 1000.0);
        }
    });
}

/// The instances of the frame being drawn and how strongly they show (0..1); nothing when there is nothing.
pub fn with_instances<R>(f: impl FnOnce(&[f32], f32) -> R) -> Option<R> {
    GLASS.with(|g| {
        let g = g.borrow();
        if g.inst.is_empty() || g.alpha <= 0.001 {
            None
        } else {
            Some(f(&g.inst, g.alpha))
        }
    })
}

/// A finger on the glass (a stroke the page took as a wipe): the path from its last place to here is wiped.
pub fn wipe(x: f32, y: f32) {
    GLASS.with(|g| {
        let mut g = g.borrow_mut();
        if !g.active() || g.visible() <= 0.001 {
            return;
        }
        let from = g.wipe_at.unwrap_or((x, y));
        g.glass.wipe(from, (x, y));
        g.wipe_at = Some((x, y));
        g.dirty = true;
    });
}

/// The wiping finger left the glass.
pub fn wipe_end() {
    GLASS.with(|g| {
        let mut g = g.borrow_mut();
        if let Some(at) = g.wipe_at.take() {
            g.glass.wipe_end(at);
            g.dirty = true;
        }
    });
}

/// Development: the time the GPU pass took on the CPU (ms), for the frame statistics.
pub fn note_pass(ms: f32) {
    GLASS.with(|g| {
        let mut g = g.borrow_mut();
        g.pass_ms.push(ms);
        g.frames += 1;
    });
}

/// Development: one line about the glass every 5 s while it draws (with the frame statistics).
pub fn report_if_due() {
    GLASS.with(|g| {
        let mut g = g.borrow_mut();
        let now = Instant::now();
        let since = *g.stats_since.get_or_insert(now);
        let secs = now.duration_since(since).as_secs_f32();
        if secs < 5.0 {
            return;
        }
        g.stats_since = Some(now);
        if g.frames > 0 {
            let (d, b, t) = g.glass.counts();
            let (s50, s99) = (pct(&mut g.sim_ms, 0.5), pct(&mut g.sim_ms, 0.99));
            let (p50, p99) = (pct(&mut g.pass_ms, 0.5), pct(&mut g.pass_ms, 0.99));
            eprintln!(
                "nspanel-app: glass {:.1} frames/s, level {}, {d} drops {b} beads {t} streak points | sim p50 {s50:.2} p99 {s99:.2} ms | pass (CPU) p50 {p50:.2} p99 {p99:.2} ms",
                g.frames as f32 / secs,
                g.level
            );
        }
        g.frames = 0;
        g.sim_ms.clear();
        g.pass_ms.clear();
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

    fn run(g: &mut Glass, secs: f32, level: u8, wind: f32) {
        let steps = (secs * 60.0) as usize;
        for _ in 0..steps {
            g.step(1.0 / 60.0, level, wind);
        }
    }

    #[test]
    fn dry_glass_stays_dry_without_rain() {
        let mut g = Glass::new(480.0, 480.0, 1);
        run(&mut g, 5.0, 0, 0.0);
        assert!(g.is_empty());
    }

    #[test]
    fn more_rain_more_drops_within_the_limit() {
        let mut light = Glass::new(480.0, 480.0, 3);
        let mut heavy = Glass::new(480.0, 480.0, 3);
        run(&mut light, 12.0, 1, 0.0);
        run(&mut heavy, 12.0, 3, 0.0);
        let (l, h) = (light.counts().0, heavy.counts().0);
        assert!(l > 5, "drizzle left {l} drops");
        assert!(h > l, "pouring {h} <= drizzle {l}");
        assert!(h <= 200, "pouring {h} over the limit");
    }

    #[test]
    fn drops_dry_up_when_the_rain_stops() {
        let mut g = Glass::new(480.0, 480.0, 5);
        run(&mut g, 10.0, 2, 0.0);
        assert!(!g.is_empty());
        run(&mut g, 40.0, 0, 0.0);
        assert!(g.is_empty(), "left {:?}", g.counts());
    }

    #[test]
    fn wind_bends_the_path_to_the_left() {
        let mut calm = Glass::new(480.0, 480.0, 9);
        let mut windy = Glass::new(480.0, 480.0, 9);
        calm.add_drop(240.0, 40.0, 7.5);
        windy.add_drop(240.0, 40.0, 7.5);
        run(&mut calm, 1.2, 0, 0.0);
        run(&mut windy, 1.2, 0, 1.0);
        let (c, w) = (&calm.drops[0], &windy.drops[0]);
        assert!(c.y > 70.0 && w.y > 70.0, "the drops did not run: {} {}", c.y, w.y);
        let (dc, dw) = ((c.x - 240.0).abs(), 240.0 - w.x);
        assert!(dw > 0.3 * (w.y - 40.0), "windy drop moved {dw} px left over {} px", w.y - 40.0);
        assert!(dw > dc + 10.0, "calm {dc} windy {dw}");
    }

    #[test]
    fn a_wipe_clears_its_path_only() {
        let mut g = Glass::new(480.0, 480.0, 11);
        for i in 0..12 {
            g.add_drop(240.0 + (i % 3) as f32 * 6.0 - 6.0, 60.0 + i as f32 * 30.0, 3.0);
        }
        g.add_drop(100.0, 200.0, 3.0);
        g.add_drop(380.0, 300.0, 3.0);
        g.wipe((240.0, 40.0), (240.0, 440.0));
        let left: Vec<(f32, f32)> = g.drops.iter().map(|d| (d.x, d.y)).collect();
        assert_eq!(left, vec![(100.0, 200.0), (380.0, 300.0)]);
        // pushed aside: beads just outside the stroke, none inside
        assert!(!g.beads.is_empty());
        assert!(g.beads.iter().all(|b| (b.x - 240.0).abs() >= FINGER));
        // the gathered water stays ahead of the fingertip
        g.wipe_end((240.0, 440.0));
        let last = g.drops.last().unwrap();
        assert!(last.y > 440.0 && (last.x - 240.0).abs() < 1.0, "{} {}", last.x, last.y);
    }

    #[test]
    fn a_stroke_off_the_glass_takes_the_water_with_it() {
        let mut g = Glass::new(480.0, 480.0, 13);
        g.add_drop(240.0, 460.0, 6.0);
        g.wipe((240.0, 400.0), (240.0, 478.0));
        let n = g.drops.len();
        g.wipe_end((240.0, 478.0));
        assert_eq!(g.drops.len(), n);
    }

    #[test]
    fn instances_cover_everything_visible() {
        let mut g = Glass::new(480.0, 480.0, 17);
        run(&mut g, 12.0, 3, 0.4);
        let mut out = Vec::new();
        g.instances(&mut out);
        assert_eq!(out.len() % INSTANCE_FLOATS, 0);
        let drops = out.chunks(INSTANCE_FLOATS).filter(|c| c[5] == 2.0).count();
        assert!(drops > 0 && drops <= g.counts().0);
        assert!(out.chunks(INSTANCE_FLOATS).all(|c| c[4] > 0.0 && c[4] <= 1.0));
    }

    #[test]
    fn a_downpour_stays_small_for_the_gpu() {
        let mut g = Glass::new(480.0, 480.0, 19);
        run(&mut g, 30.0, 3, 1.0);
        let (_, b, t) = g.counts();
        assert!(b <= MAX_BEADS && t <= MAX_TRAIL, "{b} beads {t} streak points");
    }

    #[test]
    fn rain_follows_the_scene() {
        assert_eq!(rain_level(0, 0), 0);
        assert_eq!(rain_level(1, 0), 1);
        assert_eq!(rain_level(3, 3), 3);
        assert_eq!(rain_level(5, 3), 3);
        assert_eq!(rain_level(0, 3), 2);
        assert_eq!(rain_level(0, 11), 2);
        assert_eq!(rain_level(0, 8), 3);
        assert_eq!(rain_level(0, 4), 0);
        assert!(wind_factor(0.0) == 0.0 && wind_factor(25.0) == 0.5 && wind_factor(120.0) == 1.0);
    }
}
