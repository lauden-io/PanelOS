//! The panel as a media player in Home Assistant (an ESPHome media player entity): Home Assistant,
//! and Music Assistant through it, plays a URL on the panel. Home Assistant's media proxy turns any
//! source into a 48 kHz 16-bit stereo WAV stream (the format the panel announces), which the panel
//! plays as it arrives. Alarms with music use it too: the music starts quietly and gets louder.

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::esphome::DeviceState;
use crate::voice::{Player, Sound};

/// ESPHome media player states (api.proto MediaPlayerState).
pub const IDLE: i32 = 1;
pub const PLAYING: i32 = 2;
pub const PAUSED: i32 = 3;
pub const ANNOUNCING: i32 = 4;

struct Parts {
    device: Arc<Mutex<DeviceState>>,
    notify: Sender<()>,
    player: Arc<Player>,
    ha_url: String,
}

static PARTS: OnceLock<Parts> = OnceLock::new();

struct Media {
    url: String,
    /// The gain on top of the panel's volume (0..1), as Home Assistant sets it.
    volume: f32,
    muted: bool,
    /// A ramp from a quiet start (alarms): (start gain, when, seconds to full).
    ramp: Option<(f32, Instant, f32)>,
    /// Audio has arrived for the current URL.
    flowing: bool,
}

static MEDIA: Mutex<Media> = Mutex::new(Media { url: String::new(), volume: 1.0, muted: false, ramp: None, flowing: false });
/// Each play gets a number; a stream whose number is old stops.
static GEN: AtomicU64 = AtomicU64::new(0);

pub fn init(device: Arc<Mutex<DeviceState>>, notify: Sender<()>, player: Arc<Player>, ha_url: String) {
    let _ = PARTS.set(Parts { device, notify, player, ha_url });
    publish(IDLE);
}

fn publish(state: i32) {
    let Some(p) = PARTS.get() else { return };
    let (volume, muted) = {
        let m = MEDIA.lock().unwrap();
        (m.volume, m.muted)
    };
    {
        let mut d = p.device.lock().unwrap();
        d.media_state = state;
        d.media_volume = volume;
        d.media_muted = muted;
    }
    let _ = p.notify.send(());
}

/// The gain now (the ramp of an alarm included).
fn gain() -> f32 {
    let m = MEDIA.lock().unwrap();
    if m.muted {
        return 0.0;
    }
    // an alarm's music rises to the panel's own volume, whatever gain the player was left at
    match m.ramp {
        Some((from, at, secs)) => (from + (1.0 - from) * (at.elapsed().as_secs_f32() / secs.max(0.1))).min(1.0),
        None => m.volume.clamp(0.0, 1.0),
    }
}

/// The next play starts quietly and gets louder (an alarm's music, asked from Music Assistant).
pub fn prepare_ramp(from: f32, secs: f32) {
    *NEXT_RAMP.lock().unwrap() = Some((from, secs));
}

static NEXT_RAMP: Mutex<Option<(f32, f32)>> = Mutex::new(None);

/// Whether audio is playing now (an alarm's music started).
pub fn flowing() -> bool {
    MEDIA.lock().unwrap().flowing
}

/// A command from Home Assistant (MediaPlayerCommandRequest).
pub fn command(cmd: Option<i32>, volume: Option<f32>, url: Option<String>, announcement: bool) {
    if let Some(v) = volume {
        MEDIA.lock().unwrap().volume = v.clamp(0.0, 1.0);
    }
    if let Some(url) = url.filter(|u| !u.is_empty()) {
        // an alarm asked for this music: it starts quietly (for a minute after the ask)
        let ramp = NEXT_RAMP.lock().unwrap().take();
        play(&url, announcement, if announcement { None } else { ramp });
        return;
    }
    match cmd {
        // play (again after a pause: the stream starts over), pause, stop, mute, unmute, toggle
        Some(0) => {
            let url = MEDIA.lock().unwrap().url.clone();
            if !url.is_empty() {
                play(&url, false, None);
            }
        }
        Some(1) => halt(PAUSED),
        Some(2) => halt(IDLE),
        Some(3) => MEDIA.lock().unwrap().muted = true,
        Some(4) => MEDIA.lock().unwrap().muted = false,
        Some(5) => {
            let playing = PARTS.get().is_some_and(|p| p.device.lock().unwrap().media_state == PLAYING);
            if playing {
                halt(PAUSED);
            } else {
                command(Some(0), None, None, false);
            }
        }
        _ => {}
    }
    let state = PARTS.get().map_or(IDLE, |p| p.device.lock().unwrap().media_state);
    publish(state);
}

/// Stops what plays (the state it leaves: idle or paused).
pub fn halt(state: i32) {
    GEN.fetch_add(1, Ordering::SeqCst);
    if let Some(p) = PARTS.get() {
        p.player.stop();
    }
    MEDIA.lock().unwrap().flowing = false;
    publish(state);
}

/// Plays a URL as a stream; `ramp` = (start gain, seconds to full) for alarms.
pub fn play(url: &str, announcement: bool, ramp: Option<(f32, f32)>) {
    let Some(p) = PARTS.get() else { return };
    let url = if url.starts_with("http") { url.to_string() } else { format!("{}{url}", p.ha_url) };
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    {
        let mut m = MEDIA.lock().unwrap();
        m.url = url.clone();
        m.ramp = ramp.map(|(from, secs)| (from, Instant::now(), secs));
        m.flowing = false;
    }
    p.player.stop();
    publish(if announcement { ANNOUNCING } else { PLAYING });
    let player = p.player.clone();
    std::thread::spawn(move || {
        let result = stream(&url, gen, &player);
        if GEN.load(Ordering::SeqCst) == gen {
            MEDIA.lock().unwrap().flowing = false;
            if let Err(e) = &result {
                eprintln!("nspanel-app: media: {e}");
            }
            publish(IDLE);
        }
    });
}

/// Reads the WAV stream and hands it to the player in 50 ms pieces (48 kHz mono, the gain applied).
fn stream(url: &str, gen: u64, player: &Player) -> Result<(), String> {
    let resp = ureq::AgentBuilder::new()
        .redirects(2)
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .build()
        .get(url)
        .call()
        .map_err(|e| e.to_string())?;
    let mut r = resp.into_reader();
    let (rate, channels) = wav_header(&mut r)?;
    let (tx, rx) = mpsc::sync_channel::<Vec<i16>>(40);
    player.play(Sound::Stream(rx));
    let frame = 2 * channels as usize;
    let mut buf = vec![0u8; frame * (rate as usize / 20)];
    let mut fill = 0;
    loop {
        if GEN.load(Ordering::SeqCst) != gen {
            return Ok(());
        }
        let n = r.read(&mut buf[fill..]).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        fill += n;
        let whole = fill / frame * frame;
        if whole == 0 {
            continue;
        }
        let g = gain();
        let mono = crate::voice::resample_to_48k(rate, channels, &buf[..whole]);
        let piece: Vec<i16> = mono.iter().map(|s| (*s as f32 * g) as i16).collect();
        buf.copy_within(whole..fill, 0);
        fill -= whole;
        MEDIA.lock().unwrap().flowing = true;
        if tx.send(piece).is_err() {
            return Ok(()); // the player dropped it (a stop)
        }
    }
    // let the last pieces play out
    drop(tx);
    while player.busy.load(Ordering::SeqCst) && GEN.load(Ordering::SeqCst) == gen {
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

/// The WAV header of a stream (its data size may be unknown): rate and channels of 16-bit PCM.
fn wav_header(r: &mut impl Read) -> Result<(u32, u16), String> {
    let mut head = [0u8; 12];
    r.read_exact(&mut head).map_err(|e| e.to_string())?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Err("not a WAV stream".into());
    }
    let (mut rate, mut channels, mut bits) = (0u32, 0u16, 0u16);
    loop {
        let mut ch = [0u8; 8];
        r.read_exact(&mut ch).map_err(|e| e.to_string())?;
        let len = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]);
        match &ch[0..4] {
            b"fmt " => {
                // 16 to 40 bytes in any WAV: the length comes from the stream, so a longer one is refused before
                // anything is allocated for it
                if !(16..=64).contains(&len) {
                    return Err(format!("WAV format chunk of {len} bytes"));
                }
                let mut f = vec![0u8; len as usize];
                r.read_exact(&mut f).map_err(|e| e.to_string())?;
                channels = u16::from_le_bytes([f[2], f[3]]);
                rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
                bits = u16::from_le_bytes([f[14], f[15]]);
            }
            b"data" => break,
            _ => {
                // other chunks before the data (a LIST of tags) are small; skipped whole with their pad byte,
                // without keeping them
                let skip = len as u64 + (len & 1) as u64;
                if skip > 1 << 20 {
                    return Err(format!("WAV chunk of {len} bytes before the data"));
                }
                let skipped = std::io::copy(&mut (&mut *r).take(skip), &mut std::io::sink()).map_err(|e| e.to_string())?;
                if skipped < skip {
                    return Err("WAV stream ended in its header".into());
                }
            }
        }
    }
    if bits != 16 || !(8000..=48000).contains(&rate) || !(1..=2).contains(&channels) {
        return Err(format!("unsupported WAV stream ({rate} Hz, {channels} ch, {bits} bit)"));
    }
    Ok((rate, channels))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], body: &[u8], len: u32) -> Vec<u8> {
        let mut c = id.to_vec();
        c.extend_from_slice(&len.to_le_bytes());
        c.extend_from_slice(body);
        c
    }

    fn fmt(rate: u32, channels: u16) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&channels.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        f.extend_from_slice(&(channels * 2).to_le_bytes());
        f.extend_from_slice(&16u16.to_le_bytes());
        f
    }

    fn wav(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut w = b"RIFF\xff\xff\xff\xffWAVE".to_vec();
        for c in chunks {
            w.extend_from_slice(c);
        }
        w
    }

    #[test]
    fn a_stream_header_with_tags_before_the_data() {
        let f = fmt(48000, 2);
        let head = wav(&[chunk(b"LIST", b"INFOabc\0", 7), chunk(b"fmt ", &f, 16), chunk(b"data", &[], u32::MAX)]);
        assert_eq!(wav_header(&mut &head[..]), Ok((48000, 2)));
    }

    #[test]
    fn lengths_from_the_stream_allocate_nothing_large() {
        let f = fmt(48000, 2);
        // a format chunk "4 GiB long" is refused at once
        let head = wav(&[chunk(b"fmt ", &f, u32::MAX)]);
        assert!(wav_header(&mut &head[..]).unwrap_err().contains("format chunk"));
        let head = wav(&[chunk(b"fmt ", &f[..8], 8)]);
        assert!(wav_header(&mut &head[..]).is_err());
        let head = wav(&[chunk(b"LIST", &[], u32::MAX), chunk(b"fmt ", &f, 16)]);
        assert!(wav_header(&mut &head[..]).unwrap_err().contains("before the data"));
        // a chunk longer than the stream
        let head = wav(&[chunk(b"LIST", b"INFO", 100)]);
        assert!(wav_header(&mut &head[..]).is_err());
    }
}
