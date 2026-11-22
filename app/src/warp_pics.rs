//! The pictures the weather scene's picture functions in `ui/photo_sky.slint` pick (`fg`, `gfg`, `trees`, `gtrees`),
//! worked out at build time (build.rs) into tables for src/warp.rs, so the app loads exactly the pictures the scene
//! draws, for every theme, condition, light variant and lit state, without a second list to keep in step.
//!
//! Each function's body is nested `cond ? a : b` over `t == N`, `m == N`, `k == N`, `root.light-variant` and
//! `root.win-on`, down to `@image-url("...")`. Anything else in a body stops the build with its place.
//!
//! The same for the night stars of src/stars.rs: the sky photos the `sky` function picks for the photo themes, with the
//! star lists and cloud cover maps tools/split_night_stars.py wrote next to them.

use std::fmt::Write as _;

/// Themes 0..=7, conditions 0..=16 and one more for any other condition (the functions' last branch).
pub const THEMES: usize = 8;
pub const MOODS: usize = 18;
/// The photo themes (0 nature, 1 countryside, 2 city): their stars are part of the sky photos.
pub const PHOTO_THEMES: usize = 3;

#[derive(Debug)]
pub enum Node {
    Pic(String),
    Is(String, i32),
    Flag(String),
    If(Box<Node>, Box<Node>, Box<Node>),
}

/// What a function is asked for: theme, condition, dusk (0) or dawn (1), the light variant, the unlit city art.
#[derive(Clone, Copy)]
pub struct Args {
    pub t: i32,
    pub m: i32,
    pub k: i32,
    pub light: bool,
    pub unlit: bool,
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s[self.i..].starts_with(|c: char| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn eat(&mut self, tok: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(tok) {
            self.i += tok.len();
            true
        } else {
            false
        }
    }

    fn err(&self, what: &str) -> String {
        let near: String = self.s[self.i..].chars().take(40).collect();
        format!("expected {what} at {}: {near:?}", self.i)
    }

    fn ternary(&mut self) -> Result<Node, String> {
        let c = self.atom()?;
        if !self.eat("?") {
            return Ok(c);
        }
        let a = self.ternary()?;
        if !self.eat(":") {
            return Err(self.err("':'"));
        }
        let b = self.ternary()?;
        Ok(Node::If(Box::new(c), Box::new(a), Box::new(b)))
    }

    fn atom(&mut self) -> Result<Node, String> {
        if self.eat("(") {
            let n = self.ternary()?;
            if !self.eat(")") {
                return Err(self.err("')'"));
            }
            return Ok(n);
        }
        if self.eat("@image-url(\"") {
            let end = self.s[self.i..].find("\")").ok_or_else(|| self.err("'\")'"))?;
            let path = self.s[self.i..self.i + end].to_string();
            self.i += end + 2;
            return Ok(Node::Pic(path));
        }
        self.ws();
        let start = self.i;
        while self.s[self.i..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_') {
            self.i += 1;
        }
        let id = self.s[start..self.i].to_string();
        if id.is_empty() {
            return Err(self.err("a name, '(' or a picture"));
        }
        if self.eat("==") {
            self.ws();
            let n0 = self.i;
            while self.s[self.i..].starts_with(|c: char| c.is_ascii_digit()) {
                self.i += 1;
            }
            let n = self.s[n0..self.i].parse().map_err(|_| self.err("a number"))?;
            return Ok(Node::Is(id, n));
        }
        Ok(Node::Flag(id))
    }
}

/// The body of `pure function <name>(…) -> image { … }` in the source, parsed.
pub fn function(src: &str, name: &str) -> Result<Node, String> {
    let head = format!("pure function {name}(");
    let at = src.find(&head).ok_or_else(|| format!("no function {name}"))?;
    let open = at + src[at..].find('{').ok_or_else(|| format!("{name}: no body"))?;
    let close = open + src[open..].find('}').ok_or_else(|| format!("{name}: body not closed"))?;
    let mut p = Parser { s: &src[open + 1..close], i: 0 };
    let n = p.ternary().map_err(|e| format!("{name}: {e}"))?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("{name}: {}", p.err("the end of the body")));
    }
    Ok(n)
}

fn test(n: &Node, a: &Args) -> Result<bool, String> {
    match n {
        Node::Is(v, x) => {
            let have = match v.as_str() {
                "t" => a.t,
                "m" => a.m,
                "k" => a.k,
                _ => return Err(format!("unknown value {v}")),
            };
            Ok(have == *x)
        }
        Node::Flag(f) => match f.as_str() {
            "root.light-variant" => Ok(a.light),
            "root.win-on" => Ok(a.unlit),
            _ => Err(format!("unknown flag {f}")),
        },
        _ => Err("a picture or a choice where a condition belongs".into()),
    }
}

/// The picture the function picks for these arguments.
pub fn pick<'a>(n: &'a Node, a: &Args) -> Result<&'a str, String> {
    match n {
        Node::Pic(p) => Ok(p),
        Node::If(c, x, y) => {
            if test(c, a)? {
                pick(x, a)
            } else {
                pick(y, a)
            }
        }
        _ => Err("a condition where a picture belongs".into()),
    }
}

/// The index of the light variant and the unlit art in a table row.
pub fn variant(light: bool, unlit: bool) -> usize {
    light as usize * 2 + unlit as usize
}

/// Rust source of the tables: FG and TREES [theme][condition][variant], GFG and GTREES [theme][condition 0 or 1][dusk 0,
/// dawn 1][variant] (only the sunny and partly cloudy conditions have dusk and dawn pictures).
pub fn generate(src: &str) -> Result<String, String> {
    let mut out = String::from("// Generated by build.rs from ui/photo_sky.slint (src/warp_pics.rs).\n");
    for (table, func, moods, ks) in [("FG", "fg", MOODS, 1), ("TREES", "trees", MOODS, 1), ("GFG", "gfg", 2, 2), ("GTREES", "gtrees", 2, 2)] {
        let f = function(src, func)?;
        let ty = if ks == 1 { format!("[[[&str; 4]; {moods}]; {THEMES}]") } else { format!("[[[[&str; 4]; 2]; {moods}]; {THEMES}]") };
        let _ = write!(out, "#[allow(dead_code)]\npub static {table}: {ty} = [");
        for t in 0..THEMES as i32 {
            out.push('[');
            for m in 0..moods as i32 {
                if ks == 2 {
                    out.push('[');
                }
                for k in 0..ks {
                    out.push('[');
                    for v in 0..4 {
                        let a = Args { t, m, k, light: v >= 2, unlit: v % 2 == 1 };
                        let _ = write!(out, "{:?},", pick(&f, &a).map_err(|e| format!("{func}({t}, {m}, {k}): {e}"))?);
                    }
                    out.push_str("],");
                }
                if ks == 2 {
                    out.push_str("],");
                }
            }
            out.push_str("],");
        }
        out.push_str("];\n");
    }
    Ok(out)
}

/// A star list's lines: `x y b k` (tile pixels, brightness 0..1, 1 twinkles / 0 steady), `#` lines are comments.
pub fn parse_stars(text: &str) -> Result<Vec<(f32, f32, f32, bool)>, String> {
    let mut stars = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let v: Vec<f32> = line.split_whitespace().map(str::parse).collect::<Result<_, _>>().map_err(|e| format!("line {}: {e}", n + 1))?;
        match v[..] {
            [x, y, b, k] if (0.0..792.0).contains(&x) && (0.0..528.0).contains(&y) && (0.0..=1.0).contains(&b) && (k == 0.0 || k == 1.0) => {
                stars.push((x, y, b, k == 1.0))
            }
            _ => return Err(format!("line {}: expected x y b k, got {line:?}", n + 1)),
        }
    }
    Ok(stars)
}

/// Rust source of the night stars' tables for src/stars.rs: SETS, one per sky photo with a star list next to it
/// (wx/<name>-sky.jpg: wx/<name>-stars.txt, and wx/<name>-cover.png if the photo has clouds), and SET_OF [photo theme]
/// [condition], the set of the photo the `sky` function picks there. `read` gives a file of the UI folder as text,
/// `exists` tells whether one is there.
pub fn night_stars(src: &str, read: &dyn Fn(&str) -> Option<String>, exists: &dyn Fn(&str) -> bool) -> Result<String, String> {
    let sky = function(src, "sky")?;
    let mut photos: Vec<&str> = Vec::new();
    let mut sets = String::from("pub static SETS: &[StarSet] = &[\n");
    let mut index = format!("pub static SET_OF: [[Option<usize>; {MOODS}]; {PHOTO_THEMES}] = [");
    for t in 0..PHOTO_THEMES as i32 {
        index.push('[');
        for m in 0..MOODS as i32 {
            let photo = pick(&sky, &Args { t, m, k: 0, light: false, unlit: false }).map_err(|e| format!("sky({t}, {m}): {e}"))?;
            let set = match photos.iter().position(|p| *p == photo) {
                Some(i) => Some(i),
                None => match photo.strip_suffix("-sky.jpg").and_then(|base| Some((base, read(&format!("{base}-stars.txt"))?))) {
                    Some((base, text)) => {
                        let stars = parse_stars(&text).map_err(|e| format!("{base}-stars.txt: {e}"))?;
                        if stars.is_empty() {
                            return Err(format!("{base}-stars.txt: no stars"));
                        }
                        let cover = format!("{base}-cover.png");
                        let cover = if exists(&cover) { format!("Some({cover:?})") } else { "None".into() };
                        let (lo, hi) = stars.iter().fold((f32::MAX, f32::MIN), |(lo, hi), s| (lo.min(s.1), hi.max(s.1)));
                        let _ = write!(sets, "    StarSet {{ sky: {photo:?}, cover: {cover}, y_min: {lo:?}, y_max: {hi:?}, stars: &[");
                        for (x, y, b, twinkle) in stars {
                            let _ = write!(sets, "Star {{ x: {x:?}, y: {y:?}, b: {b:?}, twinkle: {twinkle} }},");
                        }
                        sets.push_str("] },\n");
                        photos.push(photo);
                        Some(photos.len() - 1)
                    }
                    None => None,
                },
            };
            let _ = write!(index, "{set:?},");
        }
        index.push_str("],");
    }
    Ok(format!("// Generated by build.rs from ui/photo_sky.slint and the star lists of its night photos (src/warp_pics.rs).\n{sets}];\n{index}];\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        pure function fg(t: int, m: int) -> image { t == 0 ? (m == 0 ? @image-url("wx/a.png") : @image-url("wx/b.png"))
            : (root.light-variant ? @image-url("wx-light/c-light.png") : root.win-on ? @image-url("wx/c-unlit.png") : @image-url("wx/c.png")) }
    "#;

    fn args(t: i32, m: i32, light: bool, unlit: bool) -> Args {
        Args { t, m, k: 0, light, unlit }
    }

    #[test]
    fn picks_follow_the_conditions() {
        let f = function(SAMPLE, "fg").unwrap();
        assert_eq!(pick(&f, &args(0, 0, false, false)).unwrap(), "wx/a.png");
        assert_eq!(pick(&f, &args(0, 5, true, true)).unwrap(), "wx/b.png");
        assert_eq!(pick(&f, &args(4, 2, true, true)).unwrap(), "wx-light/c-light.png");
        assert_eq!(pick(&f, &args(4, 2, false, true)).unwrap(), "wx/c-unlit.png");
        assert_eq!(pick(&f, &args(4, 2, false, false)).unwrap(), "wx/c.png");
    }

    #[test]
    fn anything_unknown_is_an_error() {
        assert!(function(SAMPLE, "trees").is_err());
        let odd = "pure function fg(t: int) -> image { root.night ? @image-url(\"a\") : @image-url(\"b\") }";
        let f = function(odd, "fg").unwrap();
        assert!(pick(&f, &args(0, 0, false, false)).is_err());
        let broken = "pure function fg(t: int) -> image { t == 0 ? @image-url(\"a\") }";
        assert!(function(broken, "fg").is_err());
    }

    #[test]
    fn the_scene_file_gives_the_known_pictures() {
        let src = include_str!("../ui/photo_sky.slint");
        let fg = function(src, "fg").unwrap();
        let trees = function(src, "trees").unwrap();
        let gtrees = function(src, "gtrees").unwrap();
        let gfg = function(src, "gfg").unwrap();
        assert_eq!(pick(&fg, &args(3, 6, false, false)).unwrap(), "wx/vcountryside-wind-fg.png");
        assert_eq!(pick(&fg, &args(4, 2, true, true)).unwrap(), "wx-light/vcity-cloudy-fg-unlit-light.png");
        assert_eq!(pick(&fg, &args(0, 16, false, false)).unwrap(), "wx/nature-snowy-night-fg.png");
        assert_eq!(pick(&fg, &args(5, 16, false, false)).unwrap(), "wx/vneutral-winter-night-fg.png");
        assert_eq!(pick(&trees, &args(5, 0, true, false)).unwrap(), "wx-light/vneutral-sunny-trees-light.png");
        let dawn = Args { t: 3, m: 1, k: 1, light: false, unlit: false };
        assert_eq!(pick(&gtrees, &dawn).unwrap(), "wx/vcountryside-partly-cloudy-dawn-trees.png");
        let dusk = Args { t: 4, m: 0, k: 0, light: false, unlit: true };
        assert_eq!(pick(&gfg, &dusk).unwrap(), "wx/vcity-sunny-dusk-fg.png");
        assert!(generate(src).unwrap().contains("pub static GTREES"));
    }

    #[test]
    fn star_lists_are_read_strictly() {
        let ok = parse_stars("# x y b k\n331.2 91.0 0.349 1\r\n\n12.5 200.0 0.031 0\n").unwrap();
        assert_eq!(ok, vec![(331.2, 91.0, 0.349, true), (12.5, 200.0, 0.031, false)]);
        assert!(parse_stars("331 91 0.3").is_err());
        assert!(parse_stars("800 91 0.3 1").is_err());
        assert!(parse_stars("331 91 0.3 2").is_err());
        assert!(parse_stars("331 91 x 1").is_err());
    }

    #[test]
    fn night_stars_follow_the_sky_photos() {
        let src = r#"pure function sky(t: int, m: int) -> image { t == 0 ? (m == 7 ? @image-url("wx/a-night-sky.jpg")
            : m == 11 ? @image-url("wx/b-night-sky.jpg") : m == 16 ? @image-url("wx/a-night-sky.jpg") : @image-url("wx/day-sky.jpg"))
            : (root.light-variant ? @image-url("wx-light/v-light.png") : @image-url("wx/v.png")) }"#;
        let read = |p: &str| (p == "wx/a-night-stars.txt").then(|| "10 20 0.5 1\n30.5 40 0.02 0\n".to_string());
        let exists = |p: &str| p == "wx/a-night-cover.png";
        let out = night_stars(src, &read, &exists).unwrap();
        assert!(out.contains(r#"StarSet { sky: "wx/a-night-sky.jpg", cover: Some("wx/a-night-cover.png"), y_min: 20.0, y_max: 40.0, stars: &[Star { x: 10.0, y: 20.0, b: 0.5, twinkle: true },Star { x: 30.5, y: 40.0, b: 0.02, twinkle: false },] }"#), "{out}");
        // one set for the photo, both conditions that show it point at it; the photo without a list has none
        assert_eq!(out.matches("StarSet {").count(), 1);
        assert!(out.contains("[None,None,None,None,None,None,None,Some(0),None,None,None,None,None,None,None,None,Some(0),None,],[None,"), "{out}");
        // a broken list stops the build
        let bad = |p: &str| (p == "wx/a-night-stars.txt").then(|| "10 20\n".to_string());
        assert!(night_stars(src, &bad, &exists).is_err());
    }
}
