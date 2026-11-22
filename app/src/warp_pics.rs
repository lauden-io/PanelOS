//! The pictures the weather scene's picture functions in `ui/photo_sky.slint` pick (`fg`, `gfg`, `trees`, `gtrees`),
//! worked out at build time (build.rs) into tables for src/warp.rs, so the app loads exactly the pictures the scene
//! draws, for every theme, condition, light variant and lit state, without a second list to keep in step.
//!
//! Each function's body is nested `cond ? a : b` over `t == N`, `m == N`, `k == N`, `root.light-variant` and
//! `root.win-on`, down to `@image-url("...")`. Anything else in a body stops the build with its place.

use std::fmt::Write as _;

/// Themes 0..=7, conditions 0..=16 and one more for any other condition (the functions' last branch).
pub const THEMES: usize = 8;
pub const MOODS: usize = 18;

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
}
