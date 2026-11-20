//! Media artwork for the assistant mode: fetches the player's
//! `entity_picture` through Home Assistant (JPEG), scaled down on the panel.

use std::io::Read;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::Duration;

use slint::{Rgb8Pixel, SharedPixelBuffer};

use crate::auth::Auth;

/// Box-filter downscale so the longer side is at most `max` pixels.
fn downscale(src: SharedPixelBuffer<Rgb8Pixel>, max: u32) -> SharedPixelBuffer<Rgb8Pixel> {
    let (w, h) = (src.width(), src.height());
    let factor = w.max(h).div_ceil(max).max(1);
    if factor == 1 {
        return src;
    }
    let (nw, nh) = (w / factor, h / factor);
    let mut out = SharedPixelBuffer::<Rgb8Pixel>::new(nw, nh);
    let s = src.as_bytes();
    let o = out.make_mut_bytes();
    let n = (factor * factor) as u32;
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 3];
            for dy in 0..factor {
                let row = ((y * factor + dy) * w + x * factor) as usize * 3;
                for dx in 0..factor as usize {
                    for c in 0..3 {
                        acc[c] += s[row + dx * 3 + c] as u32;
                    }
                }
            }
            let i = (y * nw + x) as usize * 3;
            for c in 0..3 {
                o[i + c] = (acc[c] / n) as u8;
            }
        }
    }
    out
}

/// Starts the fetcher; send a player and its picture path (or None to clear), get the player and its picture back.
pub fn spawn(
    ha_url: String,
    auth: Arc<Auth>,
    deliver: impl Fn(String, Option<SharedPixelBuffer<Rgb8Pixel>>) + Send + 'static,
) -> Sender<(String, Option<String>)> {
    let (tx, rx) = mpsc::channel::<(String, Option<String>)>();
    std::thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            // Only the newest request of each player matters.
            let mut requests = vec![first];
            while let Ok(newer) = rx.try_recv() {
                requests.retain(|(k, _)| *k != newer.0);
                requests.push(newer);
            }
            for (key, request) in requests {
                let Some(path) = request else {
                    deliver(key, None);
                    continue;
                };
                let result = (|| -> Result<_, String> {
                    let url = if path.starts_with("http") { path.clone() } else { format!("{ha_url}{path}") };
                    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(10)).build();
                    let mut req = agent.get(&url);
                    // The login token only goes to Home Assistant itself, never to
                    // an artwork URL elsewhere.
                    if url.starts_with(&format!("{ha_url}/")) {
                        if let Ok(token) = auth.access_token() {
                            req = req.set("Authorization", &format!("Bearer {token}"));
                        }
                    }
                    let resp = req.call().map_err(|e| e.to_string())?;
                    let mut bytes = Vec::new();
                    resp.into_reader().take(8 * 1024 * 1024).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                    crate::camera::decode(&bytes).map(|img| downscale(img, 240))
                })();
                match result {
                    Ok(img) => deliver(key, Some(img)),
                    Err(e) => {
                        eprintln!("nspanel-app: artwork: {e}");
                        deliver(key, None);
                    }
                }
            }
        }
    });
    tx
}
