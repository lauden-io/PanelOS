//! Pictures the app draws itself: the label maker's tapes, the notepad's
//! handwriting, the fridge notes and the wall calendar's pen notes. A
//! renderer can neither tilt a strip of tape nor write every letter a little
//! differently, so Skia draws them on the CPU, in a thread of their own; each
//! is random but stable for its item (seeded by its text). The pages ask with
//! `get`: a picture drawn before comes back at once, a new one is queued and
//! the pages are refreshed when it is ready. (Only in the GPU build, which
//! has Skia; the looks that need them fall back to the plain look otherwise.)

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// Whether this build draws them.
pub const AVAILABLE: bool = cfg!(feature = "gpu");

/// Colours of label tape.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tint {
    Red,
    Blue,
    Green,
    Black,
    Yellow,
    White,
    Grey,
}

/// Pens and pencil.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ink {
    Blue,
    Red,
    Pencil,
    Purple,
    Green,
    Black,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Job {
    /// One label tape (small: the due date's and the note's size).
    Tape { text: String, tint: Tint, small: bool, seed: u32 },
    /// A label in a menu: its text on as many tapes as it needs under each other (wrapped at the words, each line at
    /// most `max_w` px), with a white rim round each when `rim` (the list shown). "▶" marks it, embossed like a letter.
    TapeLabel { text: String, tint: Tint, small: bool, seed: u32, max_w: u32, rim: bool },
    /// A to-do item on tapes: its own, the due date to the right, the note under it.
    TapeItem { text: String, tint: Tint, seed: u32, due: String, due_state: i32, note: String, struck: bool },
    /// The label look's header: the list name and what is left.
    TapeHead { name: String, count: String, seed: u32 },
    /// The mixing desk's tiny label under a fader, its strip at most `width` px wide (smaller letters or two lines
    /// when the text is long).
    DeskTape { text: String, tint: Tint, seed: u32, width: u32 },
    /// A to-do item written on the notepad (rows of 36 px).
    HandItem { text: String, seed: u32, due: String, due_state: i32, note: String, ticked: bool, faded: bool },
    /// The notepad's top margin: the list name and what is left.
    HandHead { title: String, count: String, seed: u32 },
    /// "Done (n)" with its arrow.
    HandDone { n: usize, open: bool },
    /// Nothing left: the title and the line under it.
    HandEmpty { title: String, sub: String },
    /// A sticky note on the fridge (level 1: a warning; big: the opened note).
    Fridge { title: String, message: String, foot: String, level: i32, seed: u32, big: bool },
    /// A wall calendar day: up to two notes in pen, "+n", today circled.
    Cell { notes: Vec<(String, Ink)>, today: bool, seed: u32 },
}

/// The desk label's letter sizes, largest first, and its line height (× the size) when it takes two lines.
#[allow(dead_code)]
const DESK_SIZES: [f32; 4] = [10.0, 9.0, 8.0, 7.5];
#[allow(dead_code)]
const DESK_LINE: f32 = 1.2;

/// How a desk label fits a strip whose text may be `max` px wide: its letter size and its lines. One line in the
/// usual size or a size smaller; else two lines split at the space that keeps the longer line shortest, from a size
/// smaller down to the smallest; else one line in the smallest sizes; only a word too long even then is cut with "…".
/// `width(line, size)` measures a line.
#[allow(dead_code)]
pub fn desk_lines(text: &str, max: f32, width: impl Fn(&str, f32) -> f32) -> (f32, Vec<String>) {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for &px in &DESK_SIZES[..2] {
        if width(&text, px) <= max {
            return (px, vec![text]);
        }
    }
    let words: Vec<&str> = text.split(' ').collect();
    let split = |px: f32| {
        (1..words.len())
            .map(|k| (words[..k].join(" "), words[k..].join(" ")))
            .min_by(|a, b| {
                let wa = width(&a.0, px).max(width(&a.1, px));
                let wb = width(&b.0, px).max(width(&b.1, px));
                wa.total_cmp(&wb)
            })
    };
    for &px in &DESK_SIZES[1..] {
        if let Some((a, b)) = split(px) {
            if width(&a, px).max(width(&b, px)) <= max {
                return (px, vec![a, b]);
            }
        }
    }
    for &px in &DESK_SIZES[2..] {
        if width(&text, px) <= max {
            return (px, vec![text]);
        }
    }
    let px = DESK_SIZES[DESK_SIZES.len() - 1];
    let cut = |line: String| {
        if width(&line, px) <= max {
            return line;
        }
        let mut chars: Vec<char> = line.chars().collect();
        while !chars.is_empty() && width(&(chars.iter().collect::<String>() + "…"), px) > max {
            chars.pop();
        }
        while chars.last() == Some(&' ') {
            chars.pop();
        }
        chars.iter().collect::<String>() + "…"
    };
    match split(px) {
        Some((a, b)) => (px, vec![cut(a), cut(b)]),
        None => (px, vec![cut(text.clone())]),
    }
}

/// A stable seed for a text.
pub fn seed(t: &str) -> u32 {
    t.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// A picture: RGBA, not premultiplied.
pub struct Pic {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

thread_local! {
    static CACHE: RefCell<HashMap<u64, (slint::Image, u64)>> = RefCell::new(HashMap::new());
    static PENDING: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
    static TICK: Cell<u64> = const { Cell::new(0) };
    static WORKER: RefCell<Option<std::sync::mpsc::Sender<(u64, Job)>>> = const { RefCell::new(None) };
    static READY: RefCell<Option<Box<dyn Fn()>>> = RefCell::new(None);
}

fn key(j: &Job) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    j.hash(&mut h);
    h.finish()
}

fn tick() -> u64 {
    TICK.with(|t| {
        t.set(t.get() + 1);
        t.get()
    })
}

/// The picture for a job: drawn before, or empty while it is being drawn.
pub fn get(job: Job) -> slint::Image {
    if !AVAILABLE {
        return Default::default();
    }
    let k = key(&job);
    let t = tick();
    if let Some(img) = CACHE.with(|c| {
        c.borrow_mut().get_mut(&k).map(|e| {
            e.1 = t;
            e.0.clone()
        })
    }) {
        return img;
    }
    // queued once (asked again before the thread started: queued then)
    if !PENDING.with(|p| p.borrow().contains(&k)) {
        let sent = WORKER.with(|w| w.borrow().as_ref().is_some_and(|tx| tx.send((k, job)).is_ok()));
        if sent {
            PENDING.with(|p| p.borrow_mut().insert(k));
        }
    }
    Default::default()
}

/// Starts the drawing thread; `ready` runs on the UI thread when pictures come in.
pub fn start(ready: impl Fn() + 'static) {
    READY.with(|r| *r.borrow_mut() = Some(Box::new(ready)));
    #[cfg(feature = "gpu")]
    {
        let (tx, rx) = std::sync::mpsc::channel::<(u64, Job)>();
        WORKER.with(|w| *w.borrow_mut() = Some(tx));
        let _ = std::thread::Builder::new().name("draw".into()).spawn(move || skia::worker(rx));
    }
}

/// Pictures drawn (on the UI thread): into the cache, the pages refreshed.
#[allow(dead_code)]
fn arrived(done: Vec<(u64, Option<Pic>)>) {
    let t = tick();
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        for (k, pic) in done {
            PENDING.with(|p| p.borrow_mut().remove(&k));
            let img = pic
                .map(|p| {
                    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&p.px, p.w, p.h);
                    slint::Image::from_rgba8(buf)
                })
                .unwrap_or_default();
            c.insert(k, (img, t));
        }
        // the least recently asked go first
        if c.len() > 320 {
            let mut v: Vec<(u64, u64)> = c.iter().map(|(k, e)| (*k, e.1)).collect();
            v.sort_by_key(|e| e.1);
            let n = c.len() - 256;
            for (k, _) in v.into_iter().take(n) {
                c.remove(&k);
            }
        }
    });
    READY.with(|r| {
        if let Some(f) = r.borrow().as_ref() {
            f();
        }
    });
}

#[cfg(feature = "gpu")]
mod skia {
    use super::{Ink, Job, Pic, Tint};
    use skia_safe::font_arguments::{variation_position::Coordinate, VariationPosition};
    use skia_safe::image_filters::{self, CropRect};
    use skia_safe::paint::{Cap, Join, Style};
    use skia_safe::{
        images, surfaces, AlphaType, Color, Color4f, ColorType, CubicResampler, Data, Font, FontArguments, FontMgr, FourByteTag,
        ImageInfo, Paint, PathBuilder, Rect, SamplingOptions, Surface, Typeface,
    };
    use std::collections::HashMap;
    use std::time::Instant;

    const NOTO: &str = "NotoSans[wdth,wght].ttf";
    const CAVEAT: &str = "Caveat[wght].ttf";

    pub fn worker(rx: std::sync::mpsc::Receiver<(u64, Job)>) {
        // below the UI thread: drawing a batch must not cost the screen frames
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 15);
        }
        let mut f = Fonts { mgr: FontMgr::new(), data: HashMap::new(), faces: HashMap::new() };
        while let Ok(first) = rx.recv() {
            let mut next = Some(first);
            let mut done = Vec::new();
            let t0 = Instant::now();
            while let Some((k, job)) = next.take() {
                done.push((k, draw(&mut f, &job)));
                next = rx.try_recv().ok();
                if done.len() >= 8 || next.is_none() {
                    let batch = std::mem::take(&mut done);
                    eprintln!("nspanel-app: drew {} pictures in {} ms", batch.len(), t0.elapsed().as_millis());
                    let _ = slint::invoke_from_event_loop(move || super::arrived(batch));
                }
            }
        }
    }

    fn draw(f: &mut Fonts, job: &Job) -> Option<Pic> {
        match job {
            Job::Tape { text, tint, small, seed } => tape(f, text, *tint, *small as u8, *seed, false, false, 448.0),
            Job::TapeLabel { text, tint, small, seed, max_w, rim } => tape_label(f, text, *tint, *small as u8, *seed, *max_w as f32, *rim),
            Job::DeskTape { text, tint, seed, width } => tape(f, text, *tint, 2, *seed, false, false, *width as f32),
            Job::TapeItem { text, tint, seed, due, due_state, note, struck } => {
                tape_item(f, text, *tint, *seed, due, *due_state, note, *struck)
            }
            Job::TapeHead { name, count, seed } => {
                let a = tape(f, name, Tint::Black, 0, *seed, false, false, 290.0)?;
                let b = tape(f, count, Tint::White, 1, seed.wrapping_add(5), false, false, 448.0)?;
                let mut out = Pic::blank(a.w + 6 + b.w, a.h.max(b.h));
                out.over(&a, 0, 0);
                out.over(&b, a.w as i32 + 6, (a.h as i32 - b.h as i32) / 2);
                Some(out)
            }
            Job::HandItem { text, seed, due, due_state, note, ticked, faded } => {
                hand_item(f, text, *seed, due, *due_state, note, *ticked, *faded)
            }
            Job::HandHead { title, count, seed } => hand_head(f, title, count, *seed),
            Job::HandDone { n, open } => hand_done(f, *n, *open),
            Job::HandEmpty { title, sub } => hand_empty(f, title, sub),
            Job::Fridge { title, message, foot, level, seed, big } => fridge(f, title, message, foot, *level, *seed, *big),
            Job::Cell { notes, today, seed } => cell(f, notes, *today, *seed),
        }
    }

    // ------------------------------------------------------------------ helpers

    struct Fonts {
        mgr: FontMgr,
        data: HashMap<&'static str, Data>,
        faces: HashMap<(&'static str, i32), Typeface>,
    }

    impl Fonts {
        fn face(&mut self, file: &'static str, wght: i32) -> Option<Typeface> {
            if let Some(t) = self.faces.get(&(file, wght)) {
                return Some(t.clone());
            }
            if !self.data.contains_key(file) {
                let path = std::path::Path::new(env!("PANELOS_UI_PICTURES")).join("fonts").join(file);
                let bytes = std::fs::read(&path).map_err(|e| eprintln!("nspanel-app: {}: {e}", path.display())).ok()?;
                self.data.insert(file, Data::new_copy(&bytes));
            }
            let base = self.mgr.new_from_data(self.data[file].clone(), None)?;
            let mut coords = vec![Coordinate { axis: FourByteTag::from_chars('w', 'g', 'h', 't'), value: wght as f32 }];
            if file == NOTO {
                coords.push(Coordinate { axis: FourByteTag::from_chars('w', 'd', 't', 'h'), value: 100.0 });
            }
            let args = FontArguments::new().set_variation_design_position(VariationPosition { coordinates: &coords });
            let tf = base.clone_with_arguments(&args).unwrap_or(base);
            self.faces.insert((file, wght), tf.clone());
            Some(tf)
        }

        fn font(&mut self, file: &'static str, wght: i32, px: f32) -> Option<Font> {
            let mut f = Font::from_typeface(self.face(file, wght)?, px);
            f.set_subpixel(true);
            f.set_linear_metrics(true);
            f.set_edging(skia_safe::font::Edging::AntiAlias);
            Some(f)
        }

        /// The width of a text in the handwriting at its plain size and weight.
        fn measure(&mut self, s: &str, px: f32, wght: i32) -> f32 {
            self.font(CAVEAT, wght, px).map(|f| f.measure_str(s, None).0).unwrap_or(0.0)
        }
    }

    fn adv(f: &Font, c: char) -> f32 {
        let mut b = [0u8; 4];
        f.measure_str(c.encode_utf8(&mut b), None).0
    }

    /// Random but stable (splitmix64).
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Self {
            let mut r = Rng(seed ^ 0x2545_F491_4F6C_DD1D);
            r.next();
            r
        }
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn unit(&mut self) -> f32 {
            (self.next() >> 40) as f32 / 16_777_216.0
        }
        fn uniform(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.unit()
        }
        fn sign(&mut self) -> f32 {
            if self.next() & 1 == 0 {
                -1.0
            } else {
                1.0
            }
        }
        fn pick<T: Copy>(&mut self, v: &[T]) -> T {
            v[(self.next() % v.len() as u64) as usize]
        }
    }

    /// `n` values along an edge: random control points every `k` samples, smoothly joined.
    fn smooth_noise(rng: &mut Rng, n: usize, k: f32, lo: f32, hi: f32) -> Vec<f32> {
        let pts: Vec<f32> = (0..(n as f32 / k) as usize + 3).map(|_| rng.uniform(lo, hi)).collect();
        (0..n)
            .map(|i| {
                let t = i as f32 / k;
                let j = t as usize;
                let f = t - j as f32;
                let f = f * f * (3.0 - 2.0 * f);
                pts[j] * (1.0 - f) + pts[j + 1] * f
            })
            .collect()
    }

    impl Pic {
        fn blank(w: u32, h: u32) -> Pic {
            Pic { w, h, px: vec![0; (w * h * 4) as usize] }
        }

        /// `src` laid over this one at (ox, oy).
        fn over(&mut self, src: &Pic, ox: i32, oy: i32) {
            for y in 0..src.h as i32 {
                let dy = oy + y;
                if dy < 0 || dy >= self.h as i32 {
                    continue;
                }
                for x in 0..src.w as i32 {
                    let dx = ox + x;
                    if dx < 0 || dx >= self.w as i32 {
                        continue;
                    }
                    let si = ((y * src.w as i32 + x) * 4) as usize;
                    let sa = src.px[si + 3] as f32 / 255.0;
                    if sa == 0.0 {
                        continue;
                    }
                    let di = ((dy * self.w as i32 + dx) * 4) as usize;
                    let da = self.px[di + 3] as f32 / 255.0;
                    let oa = sa + da * (1.0 - sa);
                    for c in 0..3 {
                        let v = (src.px[si + c] as f32 * sa + self.px[di + c] as f32 * da * (1.0 - sa)) / oa;
                        self.px[di + c] = v.round().clamp(0.0, 255.0) as u8;
                    }
                    self.px[di + 3] = (oa * 255.0).round() as u8;
                }
            }
        }

        /// Cut to what is drawn.
        fn crop(self) -> Pic {
            let (w, h) = (self.w as usize, self.h as usize);
            let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
            for y in 0..h {
                for x in 0..w {
                    if self.px[(y * w + x) * 4 + 3] > 0 {
                        x0 = x0.min(x);
                        x1 = x1.max(x + 1);
                        y0 = y0.min(y);
                        y1 = y1.max(y + 1);
                    }
                }
            }
            if x1 <= x0 || y1 <= y0 {
                return self;
            }
            let (nw, nh) = (x1 - x0, y1 - y0);
            let mut px = Vec::with_capacity(nw * nh * 4);
            for y in y0..y1 {
                px.extend_from_slice(&self.px[(y * w + x0) * 4..(y * w + x1) * 4]);
            }
            Pic { w: nw as u32, h: nh as u32, px }
        }

        fn image(&self) -> Option<skia_safe::Image> {
            let info = ImageInfo::new((self.w as i32, self.h as i32), ColorType::RGBA8888, AlphaType::Unpremul, None);
            images::raster_from_data(&info, Data::new_copy(&self.px), self.w as usize * 4)
        }
    }

    fn read(surf: &mut Surface) -> Option<Pic> {
        let (w, h) = (surf.width(), surf.height());
        let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let mut px = vec![0u8; (w * h * 4) as usize];
        surf.read_pixels(&info, &mut px, w as usize * 4, (0, 0)).then_some(Pic { w: w as u32, h: h as u32, px })
    }

    /// Tilted by `deg` with a soft shadow falling straight down (`dx`, `dy`), cut to what is drawn.
    fn tilt(pic: &Pic, deg: f32, pad: f32, blur: f32, dx: f32, dy: f32, alpha: f32) -> Option<Pic> {
        let img = pic.image()?;
        let (w, h) = (pic.w as f32, pic.h as f32);
        let r = deg.to_radians();
        let (s, c) = (r.sin(), r.cos());
        let cw = (w * c.abs() + h * s.abs() + 2.0 * pad).ceil() as i32;
        let ch = (w * s.abs() + h * c.abs() + 2.0 * pad).ceil() as i32;
        let mut surf = surfaces::raster_n32_premul((cw, ch))?;
        {
            let canvas = surf.canvas();
            canvas.clear(Color::TRANSPARENT);
            canvas.translate((cw as f32 / 2.0, ch as f32 / 2.0));
            canvas.rotate(deg, None);
            let sampling = SamplingOptions::from(CubicResampler::mitchell());
            let mut sp = Paint::default();
            sp.set_image_filter(image_filters::drop_shadow_only(
                (c * dx + s * dy, -s * dx + c * dy),
                (blur, blur),
                Color4f::new(0.0, 0.0, 0.0, alpha),
                None,
                None,
                None::<CropRect>,
            ));
            canvas.draw_image_with_sampling_options(&img, (-w / 2.0, -h / 2.0), sampling, Some(&sp));
            canvas.draw_image_with_sampling_options(&img, (-w / 2.0, -h / 2.0), sampling, None);
        }
        Some(read(&mut surf)?.crop())
    }

    // ------------------------------------------------------------------ label tape

    fn tint_rgb(t: Tint) -> [f32; 3] {
        let c: [u8; 3] = match t {
            Tint::Red => [198, 42, 47],
            Tint::Blue => [36, 86, 166],
            Tint::Green => [31, 122, 61],
            Tint::Black => [30, 30, 33],
            Tint::Yellow => [242, 194, 48],
            Tint::White => [237, 237, 235],
            Tint::Grey => [112, 112, 117],
        };
        [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
    }

    /// Embossed tape: a tilt of a few degrees, uneven cut ends with stress-whitened, thinned
    /// edges, raised capitals sitting a little unevenly; struck: a raised bar over each letter.
    #[allow(clippy::too_many_arguments)]
    fn tape(f: &mut Fonts, text: &str, tint: Tint, size: u8, seed: u32, struck: bool, faded: bool, max_w: f32) -> Option<Pic> {
        tape_with(f, text, tint, size, seed, struck, faded, max_w, false)
    }

    /// The marker of the list shown ("▶"): not in the tape's font, drawn as a raised triangle this wide (in em).
    const MARK: char = '\u{25B6}';
    const MARK_EM: f32 = 0.62;

    /// The tape's size: height, font size, letter spacing and the padding at the ends (0 an item's tape, 1 the due
    /// date's and the note's, 2 the mixing desk's).
    fn tape_metrics(size: u8) -> (usize, f32, f32, f32) {
        match size {
            2 => (21usize, 10.0, 0.5, 5.0),
            1 => (28usize, 12.0, 1.0, 12.0),
            _ => (44usize, 18.0, 1.6, 17.0),
        }
    }

    /// The width of a tape's capitals (without the ends), as the tape lays them out.
    fn tape_text_w(f: &mut Fonts, text: &str, size: u8) -> Option<f32> {
        let (_, fs, ls, _) = tape_metrics(size);
        let font = f.font(NOTO, 700, fs * 2.0)?;
        let chars: Vec<char> = text.to_uppercase().chars().collect();
        Some(chars.iter().map(|c| if *c == MARK { fs * MARK_EM } else { adv(&font, *c) / 2.0 }).sum::<f32>() + ls * chars.len().saturating_sub(1) as f32)
    }

    /// A white rim `r` px wide round what is drawn (the shown list's tape in a menu).
    fn outline(src: &Pic, r: i32) -> Pic {
        let (w, h) = (src.w as i32 + 2 * r, src.h as i32 + 2 * r);
        let mut out = Pic::blank(w as u32, h as u32);
        let alpha = |x: i32, y: i32| -> u8 {
            if x < 0 || y < 0 || x >= src.w as i32 || y >= src.h as i32 {
                0
            } else {
                src.px[((y * src.w as i32 + x) * 4 + 3) as usize]
            }
        };
        for y in 0..h {
            for x in 0..w {
                let mut a = 0u8;
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dy * dy <= r * r + 1 {
                            a = a.max(alpha(x - r + dx, y - r + dy));
                        }
                    }
                }
                if a > 0 {
                    let i = ((y * w + x) * 4) as usize;
                    out.px[i..i + 4].copy_from_slice(&[255, 255, 255, a]);
                }
            }
        }
        out.over(src, r, r);
        out
    }

    /// A label on tapes under each other: wrapped at the words (a word longer than a tape at its letters), each line
    /// its own tape, so nothing is cut off.
    fn tape_label(f: &mut Fonts, text: &str, tint: Tint, size: u8, seed: u32, max_w: f32, rim: bool) -> Option<Pic> {
        let (_, _, _, pad) = tape_metrics(size);
        let limit = max_w - 2.0 * pad - 8.0;
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        for word in text.split_whitespace() {
            let tryline = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if tape_text_w(f, &tryline, size)? <= limit {
                cur = tryline;
                continue;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            // a word wider than a tape: cut at its letters
            let mut part = String::new();
            for c in word.chars() {
                let mut t = part.clone();
                t.push(c);
                if !part.is_empty() && tape_text_w(f, &t, size)? > limit {
                    lines.push(std::mem::take(&mut part));
                    t = c.to_string();
                }
                part = t;
            }
            cur = part;
        }
        if !cur.is_empty() || lines.is_empty() {
            lines.push(cur);
        }
        let pics: Vec<Pic> = lines
            .iter()
            .enumerate()
            .map(|(k, l)| tape_with(f, l, tint, size, seed.wrapping_add(31 * k as u32), false, false, max_w + 64.0, rim))
            .collect::<Option<Vec<Pic>>>()?;
        if pics.len() == 1 {
            return pics.into_iter().next();
        }
        // each line overlaps the one above by its shadow and tilt margins
        let step = |p: &Pic| p.h as i32 - 10;
        let w = pics.iter().enumerate().map(|(k, p)| p.w as i32 + (k as i32 % 2) * 4).max().unwrap_or(1);
        let h = pics.iter().take(pics.len() - 1).map(step).sum::<i32>() + pics.last().map(|p| p.h as i32).unwrap_or(1);
        let mut out = Pic::blank(w.max(1) as u32, h.max(1) as u32);
        let mut y = 0;
        for (k, p) in pics.iter().enumerate() {
            out.over(p, (k as i32 % 2) * 4, y);
            y += step(p);
        }
        Some(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn tape_with(f: &mut Fonts, text: &str, tint: Tint, size: u8, seed: u32, struck: bool, faded: bool, max_w: f32, rim: bool) -> Option<Pic> {
        const S: usize = 2;
        let sf = S as f32;
        let mut rng = Rng::new(seed as u64);
        let printed = matches!(tint, Tint::Yellow | Tint::White);
        // size 0: an item's tape, 1: the due date's and the note's, 2: the mixing desk's
        let (mut h, mut fs, ls, pad) = tape_metrics(size);
        let small = size > 0;
        // the desk's label is as wide as its strip (`max_w`, the visible tape): smaller letters or two lines first
        let desk: Option<Vec<String>> = if size == 2 {
            let fonts: Vec<(f32, Font)> = super::DESK_SIZES.iter().filter_map(|&px| f.font(NOTO, 700, px * sf).map(|ft| (px, ft))).collect();
            let measure = |t: &str, px: f32| {
                fonts.iter().find(|(s, _)| *s == px).map_or(f32::MAX, |(_, ft)| {
                    t.chars().map(|c| adv(ft, c) / sf).sum::<f32>() + ls * t.chars().count().saturating_sub(1) as f32
                })
            };
            let (px, lines) = super::desk_lines(&text.to_uppercase(), max_w - 2.0 * pad, measure);
            fs = px;
            if lines.len() > 1 {
                h = (2.0 * super::DESK_LINE * px + 5.0).ceil() as usize;
            }
            Some(lines)
        } else {
            None
        };
        let font = f.font(NOTO, 700, fs * sf)?;
        let width_of = |t: &[char]| {
            t.iter().map(|c| if *c == MARK { fs * MARK_EM } else { adv(&font, *c) / sf }).sum::<f32>() + ls * t.len().saturating_sub(1) as f32
        };
        let rows: Vec<Vec<char>> = match desk {
            Some(lines) => lines.iter().map(|l| l.chars().collect()).collect(),
            None => {
                let mut chars: Vec<char> = text.to_uppercase().chars().collect();
                let limit = max_w - 2.0 * pad - 8.0;
                if width_of(&chars) > limit {
                    loop {
                        let mut t = chars.clone();
                        t.push('…');
                        if chars.is_empty() || width_of(&t) <= limit {
                            break;
                        }
                        chars.pop();
                    }
                    while chars.last() == Some(&' ') {
                        chars.pop();
                    }
                    chars.push('…');
                }
                vec![chars]
            }
        };
        let text_w = rows.iter().map(|r| width_of(r)).fold(0.0f32, f32::max);
        let wn = (text_w + 2.0 * pad).ceil() as usize;
        let (w, hh) = (wn * S, h * S);
        // the cut ends: a slight slant and a ragged line, each end on its own
        let slant_l = rng.uniform(-2.2, 2.2) * sf;
        let slant_r = rng.uniform(-2.2, 2.2) * sf;
        let rag_l = smooth_noise(&mut rng, hh, 3.0 * sf, -0.9 * sf, 0.9 * sf);
        let rag_r = smooth_noise(&mut rng, hh, 3.0 * sf, -0.9 * sf, 0.9 * sf);
        let edge = |y: usize, right: bool| {
            let t = y as f32 / hh as f32 - 0.5;
            if right {
                w as f32 - 1.5 * sf + slant_r * t + rag_r[y]
            } else {
                1.5 * sf + slant_l * t + rag_l[y]
            }
        };
        // stress-whitened edges at the cuts: strength varies along the edge, with gaps
        let side = |n: u64| {
            let mut r = Rng::new(seed as u64 * 7 + n);
            let band = r.uniform(1.4, 3.6) * sf;
            let st: Vec<f32> = smooth_noise(&mut r, hh, 5.0 * sf, -0.5, 1.0).into_iter().map(|v| v.clamp(0.0, 1.0)).collect();
            (band, st)
        };
        let (band_l, st_l) = side(1);
        let (band_r, st_r) = side(2);
        // the letters' mask
        let mut mask = vec![0u8; w * hh];
        {
            let info = ImageInfo::new((w as i32, hh as i32), ColorType::Alpha8, AlphaType::Premul, None);
            let mut surf = surfaces::raster(&info, None, None)?;
            let (_, m) = font.metrics();
            let (asc, desc) = (-m.ascent, m.descent);
            // one line in the middle; two lines (the desk's) a line height apart, each centred
            let lh = if rows.len() > 1 { super::DESK_LINE * fs * sf } else { asc + desc };
            let top = (hh as f32 - lh * rows.len() as f32) / 2.0 + (lh - (asc + desc)) / 2.0 + 0.6 * sf;
            {
                let canvas = surf.canvas();
                canvas.clear(Color::TRANSPARENT);
                let mut p = Paint::default();
                p.set_anti_alias(true);
                p.set_color(Color::WHITE);
                for (r, chars) in rows.iter().enumerate() {
                    let y0 = top + r as f32 * lh;
                    let mut x = (pad + (text_w - width_of(chars)) / 2.0) * sf;
                    let mut boxes = Vec::new();
                    for ch in chars {
                        let jy = rng.uniform(-0.45, 0.45) * sf;
                        let base = y0 + jy + asc;
                        if *ch == MARK {
                            // a triangle the height of the capitals
                            let (mw, ch_h) = (fs * MARK_EM * sf, asc * 0.72);
                            let mut pb = PathBuilder::new();
                            pb.move_to((x + 0.06 * mw, base - ch_h));
                            pb.line_to((x + 0.94 * mw, base - ch_h / 2.0));
                            pb.line_to((x + 0.06 * mw, base));
                            pb.close();
                            canvas.draw_path(&pb.detach(), &p);
                            x += mw + ls * sf;
                            continue;
                        }
                        let s = ch.to_string();
                        canvas.draw_str(&s, (x, base), &font, &p);
                        let (a, b) = font.measure_str(&s, Some(&p));
                        if *ch != ' ' {
                            boxes.push((x + b.left, x + b.right));
                        }
                        x += a + ls * sf;
                    }
                    if struck {
                        for (bx0, bx1) in boxes {
                            let yc = y0 + asc * 0.62 + rng.uniform(-0.4, 0.4) * sf;
                            canvas.draw_rect(Rect::from_ltrb(bx0 + 0.3 * sf, yc - sf, bx1 - 0.3 * sf, yc + sf), &p);
                        }
                    }
                }
            }
            surf.read_pixels(&info, &mut mask, w, (0, 0));
        }
        // the plastic, pixel by pixel at 2x
        let base = tint_rgb(tint);
        let mut grain = Rng::new(seed as u64 ^ 0xA5A5_5A5A);
        let mut acc = vec![0f32; w * hh * 4];
        for y in 0..hh {
            let t = y as f32 / hh as f32;
            let shade = 1.0 + 0.10 * (-((t - 0.16) / 0.10).powi(2)).exp() - 0.16 * t.powf(2.2);
            let dtb = (y as f32 + 0.5).min(hh as f32 - y as f32 - 0.5);
            let rim = (1.0 - dtb / (1.4 * sf)).clamp(0.0, 1.0) * if y < hh / 2 { 0.08 } else { -0.10 };
            let (el, er) = (edge(y, false), edge(y, true));
            let ysh = (y + hh - S) % hh;
            let yhi = (y + 1) % hh;
            for x in 0..w {
                let xf = x as f32;
                let (dl, dr) = (xf - el, er - xf);
                let mut inside = (dl.min(dr) / sf + 0.5).clamp(0.0, 1.0) * (dtb / sf).clamp(0.0, 1.0);
                if inside <= 0.0 {
                    continue;
                }
                let g = (grain.unit() + grain.unit() + grain.unit() - 1.5) * 0.02;
                let mut c = [base[0] * shade + g, base[1] * shade + g, base[2] * shade + g];
                for (dist, band, st) in [(dl, band_l, st_l[y]), (dr, band_r, st_r[y])] {
                    let k = (1.0 - dist / band).clamp(0.0, 1.0);
                    let wht = k * k.sqrt() * st;
                    for v in c.iter_mut() {
                        *v = *v * (1.0 - 0.6 * wht) + 0.6 * wht;
                    }
                    if st > 0.6 {
                        inside *= 1.0 - 0.45 * wht;
                    }
                }
                for v in c.iter_mut() {
                    *v = (*v + rim).clamp(0.0, 1.0);
                }
                let l = mask[y * w + x] as f32 / 255.0;
                if !printed {
                    let sh = mask[ysh * w + x] as f32 / 255.0;
                    let hi = mask[yhi * w + x] as f32 / 255.0;
                    let face = 0.03 * grain.unit();
                    let fc = [0.965 - face, 0.965 - face, 0.95 - face];
                    for (i, v) in c.iter_mut().enumerate() {
                        *v *= 1.0 - 0.45 * sh * (1.0 - l);
                        *v += 0.22 * hi * (1.0 - l);
                        *v = *v * (1.0 - l) + fc[i] * l;
                    }
                } else {
                    for v in c.iter_mut() {
                        *v = *v * (1.0 - l) + 0.11 * l;
                    }
                }
                let a = inside * if faded { 0.55 } else { 1.0 };
                let i = (y * w + x) * 4;
                acc[i] = c[0].clamp(0.0, 1.0) * a;
                acc[i + 1] = c[1].clamp(0.0, 1.0) * a;
                acc[i + 2] = c[2].clamp(0.0, 1.0) * a;
                acc[i + 3] = a;
            }
        }
        // down to 1:1 (premultiplied average)
        let mut pic = Pic::blank(wn as u32, h as u32);
        for y in 0..h {
            for x in 0..wn {
                let mut s = [0f32; 4];
                for sy in 0..S {
                    for sx in 0..S {
                        let i = ((y * S + sy) * w + x * S + sx) * 4;
                        for k in 0..4 {
                            s[k] += acc[i + k];
                        }
                    }
                }
                let n = (S * S) as f32;
                let a = s[3] / n;
                let o = (y * wn + x) * 4;
                if a > 0.0 {
                    for k in 0..3 {
                        pic.px[o + k] = ((s[k] / n / a) * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                    pic.px[o + 3] = (a * 255.0).round() as u8;
                }
            }
        }
        let ang = rng.uniform(0.6, 2.6) * rng.sign() * if small { 0.6 } else { 1.0 };
        let pic = if rim { outline(&pic, 2) } else { pic };
        tilt(&pic, ang, 6.0, 2.2, 0.0, 2.0, if faded { 0.22 } else { 0.38 })
    }

    /// One to-do item: its tape, the due date on a small tape to the right, the note on a small white tape under it.
    #[allow(clippy::too_many_arguments)]
    fn tape_item(f: &mut Fonts, text: &str, tint: Tint, seed: u32, due: &str, due_state: i32, note: &str, struck: bool) -> Option<Pic> {
        let max_w = 448.0;
        let x0 = (seed % 12) as i32;
        let dt = if !due.is_empty() && !struck {
            let t = match due_state {
                2 => Tint::Red,
                1 => Tint::Yellow,
                _ => Tint::White,
            };
            tape(f, due, t, 1, seed.wrapping_add(11), false, false, 448.0)
        } else {
            None
        };
        let dw = dt.as_ref().map(|d| d.w as f32 + 8.0).unwrap_or(0.0);
        let mt = tape(f, text, if struck { Tint::Grey } else { tint }, 0, seed, struck, struck, max_w - x0 as f32 - dw)?;
        let nt = if !note.is_empty() && !struck { tape(f, note, Tint::White, 1, seed.wrapping_add(23), false, false, max_w - x0 as f32 - 22.0) } else { None };
        let mut wc = (max_w as i32).min(x0 + mt.w as i32 + dw as i32);
        if let Some(n) = &nt {
            wc = wc.max((max_w as i32).min(x0 + 22 + n.w as i32));
        }
        let hc = mt.h as i32 + nt.as_ref().map(|n| n.h as i32 - 6).unwrap_or(0);
        let mut out = Pic::blank(wc.max(1) as u32, hc.max(1) as u32);
        out.over(&mt, x0, 0);
        if let Some(d) = &dt {
            out.over(d, (wc - d.w as i32).min(x0 + mt.w as i32 + 8), ((mt.h as i32 - d.h as i32) / 2).max(0));
        }
        if let Some(n) = &nt {
            out.over(n, x0 + 22, mt.h as i32 - 6);
        }
        Some(out)
    }

    // ------------------------------------------------------------------ handwriting

    const ROW: f32 = 36.0;
    const BASE: f32 = 7.0;
    const PAD_W: i32 = 388;
    const BOX_X: f32 = 66.0;
    const TEXT_X: f32 = 92.0;
    const RIGHT: f32 = 376.0;

    fn ink_color(ink: Ink, alpha: f32) -> Color4f {
        let c: [u8; 3] = match ink {
            Ink::Blue => [27, 52, 138],
            Ink::Red => [192, 36, 42],
            Ink::Pencil => [86, 86, 92],
            Ink::Purple => [92, 50, 150],
            Ink::Green => [22, 112, 60],
            Ink::Black => [30, 30, 34],
        };
        Color4f::new(c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, alpha)
    }

    /// A canvas for handwriting: every letter with its own size, weight, tilt and baseline, pen strokes with a wobble.
    struct Sheet {
        surf: Surface,
    }

    impl Sheet {
        fn new(w: i32, h: i32) -> Option<Sheet> {
            let mut surf = surfaces::raster_n32_premul((w.max(1), h.max(1)))?;
            surf.canvas().clear(Color::TRANSPARENT);
            Some(Sheet { surf })
        }

        /// Writes `s` from `x` on the baseline; returns the end x (stops with … at `max_w`).
        #[allow(clippy::too_many_arguments)]
        fn text(&mut self, f: &mut Fonts, rng: &mut Rng, s: &str, x: f32, base: f32, px: f32, ink: Ink, wght: i32, alpha: f32, max_w: Option<f32>) -> f32 {
            let limit = max_w.map(|m| x + m);
            let ell = f.measure("…", px, wght);
            let chars: Vec<char> = s.chars().collect();
            let mut xx = x;
            for (i, c0) in chars.iter().enumerate() {
                let sc = rng.uniform(0.95, 1.05);
                let wv = wght + rng.pick(&[-40, 0, 0, 40]);
                let Some(font) = f.font(CAVEAT, wv, px * sc) else { break };
                let mut ch = *c0;
                let mut a = adv(&font, ch);
                let mut last = false;
                if let Some(l) = limit {
                    if xx + a > l - ell && i + 1 < chars.len() {
                        ch = '…';
                        a = adv(&font, ch);
                        last = true;
                    }
                }
                let alpha = alpha * rng.uniform(0.86, 1.0);
                let rot = rng.uniform(-4.0, 4.0);
                let jy = rng.uniform(-0.9, 0.9);
                if ch != ' ' {
                    let mut p = Paint::new(ink_color(ink, alpha), None);
                    p.set_anti_alias(true);
                    let canvas = self.surf.canvas();
                    canvas.save();
                    canvas.translate((xx, base + jy));
                    canvas.rotate(rot, None);
                    canvas.draw_str(ch.to_string(), (0.0, 0.0), &font, &p);
                    canvas.restore();
                }
                xx += a + rng.uniform(-0.25, 0.45);
                if last {
                    break;
                }
            }
            xx
        }

        /// A pen line through `pts`, slightly uneven.
        fn stroke(&mut self, rng: &mut Rng, pts: &[(f32, f32)], width: f32, ink: Ink) {
            if pts.len() < 2 {
                return;
            }
            let mut b = PathBuilder::new();
            b.move_to(pts[0]);
            for p in &pts[1..] {
                b.line_to(*p);
            }
            let path = b.detach();
            let mut p = Paint::new(ink_color(ink, 0.92 * rng.uniform(0.85, 1.0)), None);
            p.set_anti_alias(true);
            p.set_style(Style::Stroke);
            p.set_stroke_width(width);
            p.set_stroke_cap(Cap::Round);
            p.set_stroke_join(Join::Round);
            self.surf.canvas().draw_path(&path, &p);
        }

        fn image(mut self) -> Option<Pic> {
            read(&mut self.surf)
        }
    }

    fn wobble(rng: &mut Rng, x0: f32, y0: f32, x1: f32, y1: f32, n: usize, amp: f32) -> Vec<(f32, f32)> {
        let ph = rng.uniform(0.0, std::f32::consts::TAU);
        let (nx, ny) = (-(y1 - y0), x1 - x0);
        let ln = nx.hypot(ny).max(1e-3);
        (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32;
                let o = (ph + t * std::f32::consts::PI * rng.uniform(0.8, 1.6)).sin() * amp;
                (x0 + (x1 - x0) * t + nx / ln * o, y0 + (y1 - y0) * t + ny / ln * o)
            })
            .collect()
    }

    fn pen_box(sh: &mut Sheet, rng: &mut Rng, x: f32, base: f32, ticked: bool) {
        let s = 16.0;
        let y = base - 15.0;
        let mut j = || rng.uniform(-1.6, 1.6);
        let c = [(x + j(), y + j()), (x + s + j(), y + j()), (x + s + j(), y + s + j()), (x + j(), y + s + j())];
        let mut pts = Vec::new();
        for i in 0..4 {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            pts.extend(wobble(rng, a.0, a.1, b.0, b.1, 4, 0.8));
        }
        pts.push((c[0].0 + 2.5, c[0].1 + rng.uniform(-0.6, 0.6))); // the pen runs on a little past the start
        sh.stroke(rng, &pts, 1.6, Ink::Blue);
        if ticked {
            let p0 = (x + 2.5, y + 8.5);
            let p1 = (x + 7.0, y + 14.5);
            let p2 = (x + 21.0 + rng.uniform(-1.0, 2.0), y - 5.0 + rng.uniform(-1.0, 1.0));
            let mut t = wobble(rng, p0.0, p0.1, p1.0, p1.1, 3, 0.3);
            t.extend(wobble(rng, p1.0, p1.1, p2.0, p2.1, 6, 0.6).into_iter().skip(1));
            sh.stroke(rng, &t, 2.2, Ink::Blue);
        }
    }

    fn strike(sh: &mut Sheet, rng: &mut Rng, x0: f32, x1: f32, base: f32, px: f32) {
        let y = base - px * 0.30;
        let sl = rng.uniform(-1.6, 1.6);
        let pts = wobble(rng, x0 - 3.0, y + sl, x1 + 3.0, y - sl, (((x1 - x0) / 12.0) as usize).max(6), 1.0);
        sh.stroke(rng, &pts, 1.9, Ink::Blue);
    }

    fn circle(sh: &mut Sheet, rng: &mut Rng, x0: f32, y0: f32, x1: f32, y1: f32, ink: Ink) {
        let (cx, cy, rx, ry) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0 + 6.0, (y1 - y0) / 2.0 + 4.0);
        let a0 = rng.uniform(0.0, std::f32::consts::TAU);
        let pts: Vec<(f32, f32)> = (0..=40)
            .map(|i| {
                let a = a0 + (i as f32 / 40.0) * (std::f32::consts::TAU + 0.45);
                let r = 1.0 + 0.04 * (3.0 * a + a0).sin();
                (cx + rx * r * a.cos(), cy + ry * r * a.sin() + i as f32 * 0.05)
            })
            .collect();
        sh.stroke(rng, &pts, 1.7, ink);
    }

    fn underline(sh: &mut Sheet, rng: &mut Rng, x0: f32, x1: f32, y: f32, ink: Ink, width: f32) {
        let a = rng.uniform(-0.5, 0.5);
        let b = rng.uniform(-1.5, 0.5);
        let pts = wobble(rng, x0, y + a, x1, y + b, 8, 1.1);
        sh.stroke(rng, &pts, width, ink);
    }

    fn wrap(f: &mut Fonts, s: &str, px: f32, width: f32, lines: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut cur = String::new();
        for w in s.split(' ') {
            let cand = if cur.is_empty() { w.to_string() } else { format!("{cur} {w}") };
            if f.measure(&cand, px, 520) * 1.06 <= width || cur.is_empty() {
                cur = cand;
            } else {
                out.push(std::mem::replace(&mut cur, w.to_string()));
            }
        }
        out.push(cur);
        if out.len() > lines {
            let rest = out[lines - 1..].join(" ");
            out.truncate(lines - 1);
            out.push(rest);
        }
        out
    }

    /// A to-do item as rows of the pad: the box and the text (up to two lines), the due date on the right
    /// (overdue circled, today underlined in red; later in pencil), the note under it in pencil.
    #[allow(clippy::too_many_arguments)]
    fn hand_item(f: &mut Fonts, text: &str, seed: u32, due: &str, due_state: i32, note: &str, ticked: bool, faded: bool) -> Option<Pic> {
        let mut rng = Rng::new(seed as u64);
        let (px, dpx) = (27.0, 23.0);
        let has_due = !due.is_empty() && !ticked;
        let due_w = if has_due { f.measure(due, dpx, 520) + if due_state == 2 { 18.0 } else { 12.0 } } else { 0.0 };
        let lines = wrap(f, text, px, RIGHT - TEXT_X - due_w - 8.0, 2);
        let has_note = !note.is_empty() && !ticked;
        let rows = lines.len() + has_note as usize;
        let mut sh = Sheet::new(PAD_W, (rows as f32 * ROW) as i32)?;
        let alpha = if faded { 0.55 } else { 1.0 };
        let base0 = ROW - BASE;
        pen_box(&mut sh, &mut rng, BOX_X, base0, ticked);
        for (i, ln) in lines.iter().enumerate() {
            let b = base0 + i as f32 * ROW;
            let room = if i == 0 { RIGHT - TEXT_X - due_w - 8.0 } else { RIGHT - TEXT_X };
            let x1 = sh.text(f, &mut rng, ln, TEXT_X, b, px, Ink::Blue, 520, alpha, Some(room));
            if ticked {
                strike(&mut sh, &mut rng, TEXT_X, x1, b, px);
            }
        }
        if has_due {
            let ink = if due_state > 0 { Ink::Red } else { Ink::Pencil };
            let dw = f.measure(due, dpx, 560);
            let x0 = RIGHT - dw - if due_state == 2 { 7.0 } else { 0.0 };
            sh.text(f, &mut rng, due, x0, base0, dpx, ink, 560, 1.0, None);
            if due_state == 2 {
                circle(&mut sh, &mut rng, x0 - 1.0, base0 - 17.0, x0 + dw + 1.0, base0 + 3.0, Ink::Red);
            } else if due_state == 1 {
                underline(&mut sh, &mut rng, x0 - 2.0, x0 + dw + 2.0, base0 + 4.0, Ink::Red, 1.7);
            }
        }
        if has_note {
            sh.text(f, &mut rng, note, TEXT_X + 4.0, base0 + lines.len() as f32 * ROW, 22.0, Ink::Pencil, 460, 1.0, Some(RIGHT - TEXT_X - 4.0));
        }
        sh.image()
    }

    /// The list name at the top of the pad, underlined; what is left in pencil on the right.
    fn hand_head(f: &mut Fonts, title: &str, count: &str, seed: u32) -> Option<Pic> {
        let mut rng = Rng::new(seed as u64);
        let mut sh = Sheet::new(PAD_W, 80)?;
        let cw = f.measure(count, 22.0, 460);
        let x1 = sh.text(f, &mut rng, title, 22.0, 62.0, 38.0, Ink::Blue, 600, 1.0, Some(RIGHT - 22.0 - cw - 20.0));
        underline(&mut sh, &mut rng, 20.0, (x1 + 6.0).min(RIGHT - cw - 16.0), 69.0, Ink::Blue, 2.0);
        sh.text(f, &mut rng, count, RIGHT - cw, 62.0, 22.0, Ink::Pencil, 460, 1.0, None);
        sh.image()
    }

    fn hand_done(f: &mut Fonts, n: usize, open: bool) -> Option<Pic> {
        let mut rng = Rng::new(99);
        let mut sh = Sheet::new(PAD_W, ROW as i32)?;
        let b = ROW - BASE;
        let x1 = sh.text(f, &mut rng, &format!("Done ({n})"), TEXT_X, b, 23.0, Ink::Pencil, 500, 1.0, None);
        let cx = x1 + 12.0;
        let pts = if open {
            [(cx, b - 4.0), (cx + 5.0, b - 10.0), (cx + 10.0, b - 4.0)]
        } else {
            [(cx, b - 10.0), (cx + 5.0, b - 4.0), (cx + 10.0, b - 10.0)]
        };
        sh.stroke(&mut rng, &pts, 1.6, Ink::Pencil);
        sh.image()
    }

    /// Nothing left: the title (up to two lines) in ink, the line under it in pencil.
    fn hand_empty(f: &mut Fonts, title: &str, sub: &str) -> Option<Pic> {
        let mut rng = Rng::new(4242);
        let tl = wrap(f, title, 30.0, RIGHT - TEXT_X, 2);
        let lines = wrap(f, sub, 23.0, RIGHT - TEXT_X, 2);
        let mut sh = Sheet::new(PAD_W, ((1 + tl.len() + lines.len()) as f32 * ROW) as i32)?;
        let b = 2.0 * ROW - BASE;
        for (i, ln) in tl.iter().enumerate() {
            sh.text(f, &mut rng, ln, TEXT_X, b + i as f32 * ROW, 30.0, Ink::Blue, 600, 1.0, Some(RIGHT - TEXT_X));
        }
        for (i, ln) in lines.iter().enumerate() {
            sh.text(f, &mut rng, ln, TEXT_X, b + (tl.len() + i) as f32 * ROW, 23.0, Ink::Pencil, 460, 1.0, Some(RIGHT - TEXT_X));
        }
        sh.image()
    }

    /// A wall calendar day cell: up to two short notes in pen, "+n" when there are more, today circled in red.
    fn cell(f: &mut Fonts, notes: &[(String, Ink)], today: bool, seed: u32) -> Option<Pic> {
        let (w, h) = (64.0, 46.0);
        let mut rng = Rng::new(seed as u64);
        let mut sh = Sheet::new(w as i32, h as i32)?;
        for (i, (text, ink)) in notes.iter().take(2).enumerate() {
            let room = w - 8.0 - if i == 1 && notes.len() > 2 { 12.0 } else { 0.0 };
            sh.text(f, &mut rng, text, 4.0, 28.0 + i as f32 * 13.0, 14.0, *ink, 560, 1.0, Some(room));
        }
        if notes.len() > 2 {
            sh.text(f, &mut rng, &format!("+{}", notes.len() - 2), w - 15.0, 41.0, 13.0, Ink::Pencil, 520, 1.0, None);
        }
        if today {
            circle(&mut sh, &mut rng, 3.0, 2.0, 17.0, 13.0, Ink::Red);
        }
        sh.image()
    }

    /// A sticky note on the fridge: the title in marker (red for warnings), the message in pen, who / when in
    /// pencil, held by a magnet; tilted a few degrees with a soft shadow.
    #[allow(clippy::too_many_arguments)]
    fn fridge(f: &mut Fonts, title: &str, message: &str, foot: &str, level: i32, seed: u32, big: bool) -> Option<Pic> {
        let mut rng = Rng::new(seed as u64);
        let (w, h) = if big { (360u32, 300u32) } else { (200, 168) };
        let col: [f32; 3] = if level == 1 {
            [250.0, 178.0, 196.0]
        } else {
            rng.pick(&[[252.0, 236.0, 130.0], [252.0, 236.0, 130.0], [178.0, 236.0, 196.0], [170.0, 214.0, 246.0]])
        };
        // paper with a slight curl: lighter at the top, a little darker towards the lower right
        let mut paper = Pic::blank(w, h);
        for y in 0..h {
            for x in 0..w {
                let (fy, fx) = (y as f32 / h as f32, x as f32 / w as f32);
                let shade = 1.04 - 0.10 * fy * fy - 0.05 * fx * fy;
                let i = ((y * w + x) * 4) as usize;
                for k in 0..3 {
                    paper.px[i + k] = (col[k] * shade).clamp(0.0, 255.0) as u8;
                }
                paper.px[i + 3] = 255;
            }
        }
        let mut sh = Sheet::new(w as i32, h as i32)?;
        sh.surf.canvas().draw_image(paper.image()?, (0.0, 0.0), None);
        let (tpx, mpx, fpx) = if big { (40.0, 28.0, 20.0) } else { (28.0, 21.0, 15.0) };
        let pad = if big { 18.0 } else { 12.0 };
        let inner = w as f32 - 2.0 * pad;
        let mut y = if big { 40.0 } else { 30.0 } + tpx * 0.75;
        for ln in wrap(f, title, tpx, inner, 2) {
            sh.text(f, &mut rng, &ln, pad, y, tpx, if level == 1 { Ink::Red } else { Ink::Black }, 700, 1.0, Some(inner));
            y += tpx * 1.02;
        }
        y += mpx * 0.25;
        let mut lines = wrap(f, message, mpx, inner, 12);
        let room = (((h as f32 - fpx * 1.7 - y) / (mpx * 1.08)) as usize + 1).max(1);
        if lines.len() > room {
            // what does not fit ends in "…"
            lines.truncate(room);
            if let Some(l) = lines.last_mut() {
                *l = format!("{}…", l.trim_end_matches([' ', '.', ',', ';']));
            }
        }
        for ln in &lines {
            sh.text(f, &mut rng, ln, pad, y, mpx, Ink::Blue, 520, 1.0, Some(inner));
            y += mpx * 1.08;
        }
        sh.text(f, &mut rng, foot, pad, h as f32 - if big { 12.0 } else { 9.0 }, fpx, Ink::Pencil, 460, 1.0, Some(inner));
        // the magnet
        let r = if big { 16.0 } else { 12.0 };
        let cx = (w / 2) as f32 + rng.uniform(-20.0, 20.0).round();
        let mc: [f32; 3] = rng.pick(&[[206.0, 40.0, 46.0], [40.0, 96.0, 196.0], [36.0, 140.0, 80.0], [240.0, 170.0, 30.0]]);
        {
            let canvas = sh.surf.canvas();
            let mut p = Paint::new(Color4f::new(0.0, 0.0, 0.0, 70.0 / 255.0), None);
            p.set_anti_alias(true);
            canvas.draw_oval(Rect::from_ltrb(cx - r + 1.0, 4.0, cx + r + 1.0, 4.0 + 2.0 * r), &p);
            p.set_color4f(Color4f::new(mc[0] / 255.0, mc[1] / 255.0, mc[2] / 255.0, 1.0), None);
            canvas.draw_oval(Rect::from_ltrb(cx - r, 2.0, cx + r, 2.0 + 2.0 * r), &p);
            p.set_color4f(Color4f::new(1.0, 1.0, 1.0, 90.0 / 255.0), None);
            canvas.draw_oval(Rect::from_ltrb(cx - r * 0.55, 2.0 + r * 0.3, cx - r * 0.05, 2.0 + r * 0.8), &p);
        }
        let note = sh.image()?;
        let ang = rng.uniform(1.0, 4.0) * rng.sign();
        tilt(&note, ang, 10.0, 4.0, 2.0, 4.0, 0.35)
    }
}

#[cfg(test)]
mod tests {
    use super::desk_lines;

    /// Capitals about 0.62 of the size wide, 0.5 px apart (like the desk's Noto Sans Bold).
    fn w(t: &str, px: f32) -> f32 {
        let n = t.chars().count() as f32;
        n * 0.62 * px + 0.5 * (n - 1.0).max(0.0)
    }

    #[test]
    fn desk_labels_fit_their_strip() {
        // short: one line in the usual size
        assert_eq!(desk_lines("DESK", 42.0, w), (10.0, vec!["DESK".to_string()]));
        // a little long: one size smaller
        assert_eq!(desk_lines("KITCHEN", 43.0, w), (9.0, vec!["KITCHEN".to_string()]));
        // two words that do not fit in one line: two lines, never one cut word
        let (px, lines) = desk_lines("SHUTTER LEFT", 42.0, w);
        assert_eq!(lines, vec!["SHUTTER".to_string(), "LEFT".to_string()]);
        assert!(lines.iter().all(|l| w(l, px) <= 42.0));
        // three words: the split that keeps the longer line shortest
        assert_eq!(desk_lines("CEILING LIGHT LEFT", 70.0, w), (9.0, vec!["CEILING".to_string(), "LIGHT LEFT".to_string()]));
        // one long word: the smallest size, cut only when even that does not fit
        let (px, lines) = desk_lines("DEHUMIDIFIER", 42.0, w);
        assert_eq!(px, 7.5);
        assert!(lines[0].ends_with('…') && w(&lines[0], px) <= 42.0);
    }
}
