//! The People widgets' profile pictures: each person's `entity_picture`, fetched from Home Assistant with the panel's
//! login in a thread of its own, cut to its middle square, shrunk to the largest head the widgets draw and made round
//! (the outside clear, so the widget draws it as it is: no clip, no layer). Kept per person until the picture's address
//! changes; one that could not be fetched is asked for again after a while.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::Read;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::auth::Auth;

/// The largest side kept: the biggest head (64 px) at the panel's largest zoom, with room to spare.
const SIDE: u32 = 128;
/// The largest picture decoded (wider or higher ones are left out: the initials stay).
const MAX_PIXELS: u64 = 2048 * 2048;
/// A picture that could not be fetched is asked for again after this long.
const RETRY: Duration = Duration::from_secs(300);
/// A request without an answer is sent again after this long.
const LOST: Duration = Duration::from_secs(120);

enum Pic {
    Asked(Instant),
    Ready(Image),
    Failed(Instant),
}

thread_local! {
    /// Per person id: the picture's address, and the picture or where its fetching stands.
    static CACHE: RefCell<HashMap<String, (String, Pic)>> = RefCell::new(HashMap::new());
    static FETCHER: RefCell<Option<Sender<(String, String)>>> = const { RefCell::new(None) };
    /// Counts the changes of the pictures that are here (one came, one went).
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// The pictures wanted now (person id, picture address): new and changed ones are fetched, the others forgotten.
pub fn want(list: &[(String, String)]) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let here = |c: &HashMap<String, (String, Pic)>| c.values().filter(|(_, p)| matches!(p, Pic::Ready(_))).count();
        let before = here(&*c);
        c.retain(|id, (url, _)| list.iter().any(|(i, u)| i == id && u == url));
        if here(&*c) != before {
            VERSION.set(VERSION.get() + 1);
        }
        for (id, url) in list {
            let ask = match c.get(id) {
                None => true,
                Some((_, Pic::Asked(t))) => t.elapsed() > LOST,
                Some((_, Pic::Failed(t))) => t.elapsed() > RETRY,
                Some((_, Pic::Ready(_))) => false,
            };
            if ask && send(id, url) {
                c.insert(id.clone(), (url.clone(), Pic::Asked(Instant::now())));
            }
        }
    });
}

/// The pictures that are here, by person id (in id order), and the count of their changes so far.
pub fn ready() -> (u64, Vec<(String, Image)>) {
    let mut list: Vec<(String, Image)> = CACHE.with(|c| {
        c.borrow()
            .iter()
            .filter_map(|(id, (_, p))| match p {
                Pic::Ready(img) => Some((id.clone(), img.clone())),
                _ => None,
            })
            .collect()
    });
    list.sort_by(|a, b| a.0.cmp(&b.0));
    (VERSION.get(), list)
}

/// The count of the pictures' changes so far (a new one, one gone).
pub fn version() -> u64 {
    VERSION.get()
}

/// Hands a picture to the fetcher (started with the first one); false without Home Assistant yet.
fn send(id: &str, url: &str) -> bool {
    FETCHER.with(|f| {
        let mut f = f.borrow_mut();
        if f.is_none() {
            let Some((ha_url, auth)) = crate::sounds::ha() else { return false };
            *f = Some(spawn(ha_url, auth));
        }
        f.as_ref().is_some_and(|tx| tx.send((id.to_string(), url.to_string())).is_ok())
    })
}

fn spawn(ha_url: String, auth: Arc<Auth>) -> Sender<(String, String)> {
    let (tx, rx) = mpsc::channel::<(String, String)>();
    std::thread::spawn(move || {
        for (id, url) in rx {
            let px = match fetch(&ha_url, &auth, &url) {
                Ok(px) => Some(px),
                Err(e) => {
                    eprintln!("nspanel-app: profile picture: {e}");
                    None
                }
            };
            let _ = slint::invoke_from_event_loop(move || arrived(id, url, px));
        }
    });
    tx
}

/// A fetched picture (None: it could not be had) is kept if it is still the one wanted; the widgets then show it.
fn arrived(id: String, url: String, px: Option<SharedPixelBuffer<Rgba8Pixel>>) {
    let shown = CACHE.with(|c| match c.borrow_mut().get_mut(&id) {
        Some((u, pic)) if *u == url => {
            let ok = px.is_some();
            if ok {
                VERSION.set(VERSION.get() + 1);
            }
            *pic = match px {
                Some(b) => Pic::Ready(Image::from_rgba8(b)),
                None => Pic::Failed(Instant::now()),
            };
            ok
        }
        _ => false,
    });
    if shown {
        crate::home_edit::people_pictures_arrived();
    }
}

/// Where a picture is fetched from, and whether the panel's login goes with it: a path on Home Assistant, or a full
/// address (the login only when it is Home Assistant's own). None for anything else.
fn address(ha_url: &str, path: &str) -> Option<(String, bool)> {
    let ha = ha_url.trim_end_matches('/');
    if path.starts_with('/') && !path.starts_with("//") {
        Some((format!("{ha}{path}"), true))
    } else if path.starts_with("http://") || path.starts_with("https://") {
        Some((path.to_string(), path.starts_with(&format!("{ha}/"))))
    } else {
        None
    }
}

fn fetch(ha_url: &str, auth: &Auth, path: &str) -> Result<SharedPixelBuffer<Rgba8Pixel>, String> {
    let (url, login) = address(ha_url, path).ok_or("not a picture address")?;
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(10)).build();
    let mut req = agent.get(&url);
    if login {
        if let Ok(token) = auth.access_token() {
            req = req.set("Authorization", &format!("Bearer {token}"));
        }
    }
    let resp = req.call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    resp.into_reader().take(8 * 1024 * 1024).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let (w, h, channels, px) = decode(&bytes)?;
    let (side, round) = round_head(&px, w, h, channels, SIDE).ok_or("empty picture")?;
    Ok(SharedPixelBuffer::clone_from_slice(&round, side, side))
}

/// A JPEG or PNG picture's pixels: width, height, channels (3 RGB or 4 RGBA) and the pixels.
fn decode(bytes: &[u8]) -> Result<(u32, u32, usize, Vec<u8>), String> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
        dec.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = dec.read_info().map_err(|e| e.to_string())?;
        let (w, h) = reader.info().size();
        if w as u64 * h as u64 > MAX_PIXELS {
            return Err(format!("picture too large ({w}x{h})"));
        }
        let mut buf = vec![0; reader.output_buffer_size().ok_or("picture too large")?];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        buf.truncate(info.buffer_size());
        let (w, h) = (info.width, info.height);
        match info.color_type {
            png::ColorType::Rgba => Ok((w, h, 4, buf)),
            png::ColorType::Rgb => Ok((w, h, 3, buf)),
            png::ColorType::GrayscaleAlpha => Ok((w, h, 4, buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect())),
            png::ColorType::Grayscale => Ok((w, h, 3, buf.iter().flat_map(|&g| [g, g, g]).collect())),
            other => Err(format!("PNG colours {other:?}")),
        }
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        let img = crate::camera::decode(bytes)?;
        if img.width() as u64 * img.height() as u64 > MAX_PIXELS {
            return Err(format!("picture too large ({}x{})", img.width(), img.height()));
        }
        Ok((img.width(), img.height(), 3, img.as_bytes().to_vec()))
    } else {
        Err("not a JPEG or PNG picture".into())
    }
}

/// A picture made round: its middle square, shrunk by a whole factor (each pixel the mean of the ones it covers,
/// weighted by how opaque they are) to a side of at most `max`, clear outside the circle with a soft edge. `channels`
/// is 3 (RGB) or 4 (RGBA). The side and RGBA pixels; None for an empty or short picture.
pub fn round_head(px: &[u8], w: u32, h: u32, channels: usize, max: u32) -> Option<(u32, Vec<u8>)> {
    let s0 = w.min(h);
    if s0 == 0 || max == 0 || !(3..=4).contains(&channels) || px.len() < (w as usize) * (h as usize) * channels {
        return None;
    }
    let f = s0.div_ceil(max).max(1);
    let side = (s0 / f).max(1);
    let (x0, y0) = ((w - side * f) / 2, (h - side * f) / 2);
    let r = side as f32 / 2.0;
    let mut out = vec![0u8; (side * side * 4) as usize];
    for y in 0..side {
        for x in 0..side {
            let mut acc = [0u32; 4];
            for dy in 0..f {
                let row = ((y0 + y * f + dy) * w + x0 + x * f) as usize * channels;
                for dx in 0..f as usize {
                    let p = &px[row + dx * channels..];
                    let a = if channels == 4 { p[3] as u32 } else { 255 };
                    for c in 0..3 {
                        acc[c] += p[c] as u32 * a;
                    }
                    acc[3] += a;
                }
            }
            // the circle's edge: a pixel half inside it is half clear
            let d = ((x as f32 + 0.5 - r).powi(2) + (y as f32 + 0.5 - r).powi(2)).sqrt();
            let cover = (r - d + 0.5).clamp(0.0, 1.0);
            let o = ((y * side + x) * 4) as usize;
            if acc[3] > 0 {
                for c in 0..3 {
                    out[o + c] = (acc[c] / acc[3]) as u8;
                }
            }
            out[o + 3] = (acc[3] as f32 / (f * f) as f32 * cover).round() as u8;
        }
    }
    Some((side, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_kept_per_person_until_their_address_changes() {
        CACHE.with(|c| c.borrow_mut().insert("person.anna".into(), ("/a.png".into(), Pic::Asked(Instant::now()))));
        let v0 = version();
        // a late answer for an older address is left out, a failed one shows nothing
        arrived("person.anna".into(), "/old.png".into(), Some(SharedPixelBuffer::new(2, 2)));
        assert!(ready().1.is_empty());
        arrived("person.anna".into(), "/a.png".into(), None);
        assert!(ready().1.is_empty());
        assert_eq!(version(), v0);
        CACHE.with(|c| c.borrow_mut().insert("person.anna".into(), ("/a.png".into(), Pic::Asked(Instant::now()))));
        arrived("person.anna".into(), "/a.png".into(), Some(SharedPixelBuffer::new(2, 2)));
        let (v1, list) = ready();
        assert_eq!(list.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["person.anna"]);
        assert!(v1 > v0);
        // still wanted at the same address: kept, nothing changes
        want(&[("person.anna".into(), "/a.png".into())]);
        assert_eq!(version(), v1);
        assert_eq!(ready().1.len(), 1);
        // a new address: the old picture goes (the new one is asked for once Home Assistant is known)
        want(&[("person.anna".into(), "/b.png".into())]);
        assert!(ready().1.is_empty());
        assert!(version() > v1);
    }

    #[test]
    fn pictures_come_from_home_assistant_with_the_login_only_there() {
        let ha = "http://ha.local:8123";
        assert_eq!(address(ha, "/api/image/serve/abc/512x512"), Some(("http://ha.local:8123/api/image/serve/abc/512x512".into(), true)));
        assert_eq!(address("http://ha.local:8123/", "/local/me.png?v=2"), Some(("http://ha.local:8123/local/me.png?v=2".into(), true)));
        assert_eq!(address(ha, "http://ha.local:8123/local/me.png"), Some(("http://ha.local:8123/local/me.png".into(), true)));
        assert_eq!(address(ha, "https://pictures.example/me.jpg"), Some(("https://pictures.example/me.jpg".into(), false)));
        assert_eq!(address(ha, "http://ha.local:81234/x.png").map(|a| a.1), Some(false));
        assert_eq!(address(ha, "//elsewhere/x.png"), None);
        assert_eq!(address(ha, "me.png"), None);
    }

    #[test]
    fn a_head_is_the_middle_square_shrunk_and_round() {
        // 300 x 200, blue on the left 50 columns and the right 50, red in the middle square
        let (w, h) = (300u32, 200u32);
        let mut px = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                px.extend_from_slice(if (50..250).contains(&x) { &[255, 0, 0] } else { &[0, 0, 255] });
            }
        }
        let (side, out) = round_head(&px, w, h, 3, 128).unwrap();
        // 200 / 2: at most 128 by a whole factor
        assert_eq!(side, 100);
        let at = |x: u32, y: u32| &out[((y * side + x) * 4) as usize..((y * side + x) * 4 + 4) as usize];
        // the middle: red, opaque; the corners: clear; no blue anywhere
        assert_eq!(at(50, 50), [255, 0, 0, 255]);
        assert_eq!(at(0, 0)[3], 0);
        assert_eq!(at(99, 99)[3], 0);
        assert!(out.chunks_exact(4).all(|p| p[2] == 0));
        // the edge is soft: the top middle pixel lies almost inside, one on the diagonal mostly outside
        assert!(at(50, 0)[3] > 200);
        assert!(at(14, 14)[3] > 0 && at(14, 14)[3] < 128);
        // a small picture stays at its size
        assert_eq!(round_head(&[9; 3 * 40 * 40], 40, 40, 3, 128).map(|r| r.0), Some(40));
        // nothing to make round
        assert!(round_head(&[], 0, 0, 3, 128).is_none());
        assert!(round_head(&[0; 10], 4, 4, 3, 128).is_none());
    }

    #[test]
    fn clear_pixels_do_not_darken_a_shrunk_head() {
        // 4 x 4 RGBA: white opaque on the left half, black and clear on the right; shrunk to 2 x 2 by 2
        let mut px = Vec::new();
        for _ in 0..4 {
            for x in 0..4 {
                px.extend_from_slice(if x < 2 { &[255, 255, 255, 255] } else { &[0, 0, 0, 0] });
            }
        }
        let (side, out) = round_head(&px, 4, 4, 4, 2).unwrap();
        assert_eq!(side, 2);
        // the left column keeps its white, the right one stays clear (not grey)
        assert_eq!(&out[0..3], [255, 255, 255]);
        assert_eq!(out[7], 0);
    }

    #[test]
    fn png_and_jpeg_pictures_decode() {
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, 2, 2);
            enc.set_color(png::ColorType::GrayscaleAlpha);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().unwrap();
            wr.write_image_data(&[10, 255, 20, 255, 30, 0, 40, 128]).unwrap();
        }
        let (w, h, ch, px) = decode(&png_bytes).unwrap();
        assert_eq!((w, h, ch), (2, 2, 4));
        assert_eq!(&px[..8], [10, 10, 10, 255, 20, 20, 20, 255]);
        let mut jpeg_bytes = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpeg_bytes, 90).encode(&[200; 8 * 8 * 3], 8, 8, jpeg_encoder::ColorType::Rgb).unwrap();
        let (w, h, ch, px) = decode(&jpeg_bytes).unwrap();
        assert_eq!((w, h, ch, px.len()), (8, 8, 3, 8 * 8 * 3));
        assert!(decode(b"GIF89a....").is_err());
    }
}
