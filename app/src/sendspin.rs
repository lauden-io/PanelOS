//! The panel as a Sendspin player: Music Assistant's own playback protocol (https://github.com/Sendspin/spec),
//! built into every Music Assistant, so the panel shows up there as a player by itself.
//!
//! - The panel announces `_sendspin._tcp` over mDNS and Music Assistant connects to it (server initiated,
//!   `ws://<panel>:8928/sendspin`).
//! - Every connection is end-to-end encrypted (Noise `KKpsk2_25519_ChaChaPoly_SHA256`; the panel is the responder).
//! - Pairing needs no one: Home Assistant asks the panel's pairing token over the encrypted ESPHome API
//!   (`SendspinPairingTokenRequest`) and Music Assistant pairs with it (Pairing PSK flow). Unpaired access stays off:
//!   a session that has not paired (the pairing PSK, or the sentinel any host gets) can only pair; it never holds the
//!   panel, never starts, clears, ends or feeds the music and never sets its clock.
//! - Audio comes as 48 kHz 16-bit PCM chunks stamped with the server's clock; the time filter maps them to the
//!   panel's clock and the speaker thread (`voice::player_thread`) mixes them in when they are due, under every
//!   other sound. While the voice assistant listens or thinks, the music is silent (as on the XVF speaker).
//! - A stall (the music had played for 5 s, then the buffer ran dry for a second) is noted: the Wi-Fi roam check
//!   then runs during the music (`roam`), as the stream breaks up anyway.
//! - A new server's clock starts the time filter afresh; a session that hears nothing for 30 s (the clock exchanges
//!   answer every 5 s) ends.
//!
//! The identity (Curve25519 key pair), the pairing PSK, the pairing records, volume and mute are kept in
//! `/mnt/persist/nspanel/sendspin.json` (0600).

use std::collections::VecDeque;
use std::io::{ErrorKind, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64U;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Message, WebSocket};

pub const PORT: u16 = 8928;
const PATH: &str = "/sendspin";
const STORE: &str = "/mnt/persist/nspanel/sendspin.json";
const NOISE: &str = "Noise_KKpsk2_25519_ChaChaPoly_SHA256";
const SUITE: &str = "25519_ChaChaPoly_SHA256";
/// The player's name in Music Assistant: "NSPanel Pro", with the panel's name for a panel that has one.
fn player_name() -> String {
    crate::panel_id::label("NSPanel Pro")
}
/// Bytes of outstanding audio the server may send (about 5 s of 48 kHz 16-bit stereo).
const BUFFER_CAPACITY: u32 = 1_000_000;
const SAMPLE_US: f64 = 1_000_000.0 / 48_000.0;

// ---------------------------------------------------------------- clock

/// The panel's monotonic clock in microseconds (the clock the time filter maps the server's to).
pub fn now_us() -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime with a valid pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as i64 * 1_000_000 + ts.tv_nsec as i64 / 1000
}

/// The spec's time filter (messaging.md "Time filter"): a Kalman filter of offset (server − client) and drift.
#[derive(Clone, Copy, Debug)]
struct TimeFilter {
    offset: f64,
    drift: f64,
    p_oo: f64,
    p_od: f64,
    p_dd: f64,
    last_update: i64,
    count: u32,
    use_drift: bool,
}

impl TimeFilter {
    const PROCESS_STD_DEV: f64 = 0.0;
    const DRIFT_PROCESS_STD_DEV: f64 = 1e-11;
    const FORGET_FACTOR: f64 = 2.0;
    const ADAPTIVE_CUTOFF: f64 = 3.0;
    const MIN_SAMPLES: u32 = 100;
    const DRIFT_SIGNIFICANCE: f64 = 2.0;
    const MAX_ERROR_SCALE: f64 = 0.5;

    const fn new() -> Self {
        TimeFilter { offset: 0.0, drift: 0.0, p_oo: f64::INFINITY, p_od: 0.0, p_dd: 0.0, last_update: 0, count: 0, use_drift: false }
    }

    fn update(&mut self, measurement: i64, max_error: i64, time_added: i64) {
        if time_added <= self.last_update || max_error <= 0 {
            return;
        }
        let dt = (time_added - self.last_update) as f64;
        self.last_update = time_added;
        let m = measurement as f64;
        let r = (Self::MAX_ERROR_SCALE * max_error as f64).powi(2);
        if self.count == 0 {
            self.offset = m;
            self.p_oo = r;
            self.drift = 0.0;
            self.count = 1;
            return;
        }
        if self.count == 1 {
            self.drift = (m - self.offset) / dt;
            self.p_dd = (self.p_oo + r) / (dt * dt);
            self.offset = m;
            self.p_oo = r;
            self.count = 2;
            return;
        }
        let offset_p = self.offset + self.drift * dt;
        let mut p_oo_p = self.p_oo + 2.0 * self.p_od * dt + self.p_dd * dt * dt + Self::PROCESS_STD_DEV.powi(2) * dt;
        let mut p_od_p = self.p_od + self.p_dd * dt;
        let mut p_dd_p = self.p_dd + Self::DRIFT_PROCESS_STD_DEV.powi(2) * dt;
        let y = m - offset_p;
        if self.count < Self::MIN_SAMPLES {
            self.count += 1;
        } else if y.abs() > Self::ADAPTIVE_CUTOFF * max_error as f64 {
            let f = Self::FORGET_FACTOR * Self::FORGET_FACTOR;
            p_oo_p *= f;
            p_od_p *= f;
            p_dd_p *= f;
        }
        let s = p_oo_p + r;
        let k_o = p_oo_p / s;
        let k_d = p_od_p / s;
        self.offset = offset_p + k_o * y;
        self.drift += k_d * y;
        self.p_oo = p_oo_p - k_o * p_oo_p;
        self.p_od = p_od_p - k_d * p_oo_p;
        self.p_dd = p_dd_p - k_d * p_od_p;
        self.use_drift = self.drift * self.drift > Self::DRIFT_SIGNIFICANCE * Self::DRIFT_SIGNIFICANCE * self.p_dd;
    }

    /// The server's time at a time of the panel's clock (µs, fractional).
    fn server_time(&self, client: i64) -> f64 {
        let d = if self.use_drift { self.drift } else { 0.0 };
        client as f64 + self.offset + d * (client - self.last_update) as f64
    }
}

// ---------------------------------------------------------------- identity and pairing records

struct Store {
    private: Vec<u8>,
    public: Vec<u8>,
    pairing_psk: Vec<u8>,
    /// (server_id, long-term PSK)
    records: Vec<(String, Vec<u8>)>,
    volume: u8,
    muted: bool,
    /// The server that last held a connection with playback (spec: the "last-playback server").
    last_server: String,
}

static STORE_DATA: OnceLock<Mutex<Store>> = OnceLock::new();

fn random32() -> Vec<u8> {
    let mut b = vec![0u8; 32];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        // never a fixed default: without urandom the key pair generator's randomness stands in
        let params: snow::params::NoiseParams = NOISE.parse().expect("noise params");
        b = snow::Builder::new(params).generate_keypair().map(|k| k.private).unwrap_or_default();
    }
    b
}

fn load_store() -> Store {
    let read = || -> Option<Store> {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(STORE).ok()?).ok()?;
        let key = |k: &str| v.get(k).and_then(Value::as_str).and_then(|s| B64U.decode(s).ok()).filter(|b| b.len() == 32);
        let records = v
            .get("records")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|r| {
                        let id = r.get("server_id")?.as_str()?.to_string();
                        let psk = B64U.decode(r.get("psk")?.as_str()?).ok().filter(|b| b.len() == 32)?;
                        Some((id, psk))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Store {
            private: key("private")?,
            public: key("public")?,
            pairing_psk: key("pairing_psk")?,
            records,
            volume: v.get("volume").and_then(Value::as_u64).unwrap_or(60).min(100) as u8,
            muted: v.get("muted").and_then(Value::as_bool).unwrap_or(false),
            last_server: v.get("last_server").and_then(Value::as_str).unwrap_or_default().to_string(),
        })
    };
    if let Some(s) = read() {
        return s;
    }
    let params: snow::params::NoiseParams = NOISE.parse().expect("noise params");
    let kp = snow::Builder::new(params).generate_keypair().expect("Sendspin key pair");
    let s = Store { private: kp.private, public: kp.public, pairing_psk: random32(), records: Vec::new(), volume: 60, muted: false, last_server: String::new() };
    save_store(&s);
    eprintln!("nspanel-app: sendspin: new identity");
    s
}

fn save_store(s: &Store) {
    let v = json!({
        "private": B64U.encode(&s.private),
        "public": B64U.encode(&s.public),
        "pairing_psk": B64U.encode(&s.pairing_psk),
        "records": s.records.iter().map(|(id, psk)| json!({"server_id": id, "psk": B64U.encode(psk)})).collect::<Vec<_>>(),
        "volume": s.volume,
        "muted": s.muted,
        "last_server": s.last_server,
    });
    let tmp = format!("{STORE}.tmp");
    use std::os::unix::fs::OpenOptionsExt;
    let ok = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| std::io::Write::write_all(&mut f, v.to_string().as_bytes()))
        .is_ok();
    if ok {
        let _ = std::fs::rename(&tmp, STORE);
    }
}

fn store() -> &'static Mutex<Store> {
    STORE_DATA.get_or_init(|| Mutex::new(load_store()))
}

/// The Sendspin client id (base64url of the static public key); None before the start.
pub fn client_id() -> Option<String> {
    STORE_DATA.get().map(|s| B64U.encode(&s.lock().unwrap().public))
}

/// RFC 4648 base32 without padding.
fn base32(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// The version-0 pairing token (pairing.md "Pairing Token"): `SP:0` + base32(client key ‖ pairing PSK), `2` → `9`.
fn token_of(public: &[u8], pairing_psk: &[u8]) -> String {
    let mut payload = public.to_vec();
    payload.extend_from_slice(pairing_psk);
    format!("SP:0{}", base32(&payload).replace('2', "9"))
}

/// The pairing token Home Assistant hands Music Assistant; None before the start.
pub fn pairing_token() -> Option<String> {
    STORE_DATA.get().map(|s| {
        let s = s.lock().unwrap();
        token_of(&s.public, &s.pairing_psk)
    })
}

fn psk_id(psk: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b"sendspin-psk-id-v1");
    h.update(psk);
    B64U.encode(h.finalize())
}

fn sentinel_psk() -> Vec<u8> {
    Sha256::digest(b"sendspin-sentinel-psk-v1").to_vec()
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PskKind {
    LongTerm,
    Pairing,
    Sentinel,
}

/// The PSK message 1 refers to. None: a miss where no fallback applies (a re-handshake, a long-term record of
/// another server).
fn choose_psk(payload: &Value, server_id: &str, initial: bool) -> Option<(PskKind, Vec<u8>)> {
    let id = payload.get("psk_id").and_then(Value::as_str)?;
    let category = payload.get("psk_category").and_then(Value::as_str)?;
    let s = store().lock().unwrap();
    let found = match category {
        "lt" => match s.records.iter().find(|(_, psk)| psk_id(psk) == id) {
            Some((sid, psk)) if sid == server_id => Some((PskKind::LongTerm, psk.clone())),
            Some(_) => return None, // a misbinding, not a miss
            None => None,
        },
        "pr" => (psk_id(&s.pairing_psk) == id).then(|| (PskKind::Pairing, s.pairing_psk.clone())),
        "sn" => (psk_id(&sentinel_psk()) == id).then(|| (PskKind::Sentinel, sentinel_psk())),
        _ => return None,
    };
    match found {
        Some(f) => Some(f),
        None if initial => Some((PskKind::Sentinel, sentinel_psk())),
        None => None,
    }
}

// ---------------------------------------------------------------- the music

struct Music {
    /// A player stream is running (between stream/start and stream/end) and the connection declares playback.
    active: bool,
    channels: u16,
    /// (server timestamp of the first sample, 48 kHz mono samples), in time order
    chunks: VecDeque<(i64, Vec<i16>)>,
    /// The server time of the next sample to play.
    cursor: Option<f64>,
    filter: TimeFilter,
    /// The connection whose clock the filter follows (a new one starts it afresh: another server's clock base).
    filter_conn: u64,
    gain: f32,
    /// For the log line while music plays: sum of squares and count of the mixed music samples, since when.
    stat: (f64, u64, Option<Instant>),
    /// The stall watch: since when the music has played without a gap (reset by a start, a seek, a stall), and since
    /// when the buffer has been dry.
    flowing_since: Option<Instant>,
    dry_since: Option<Instant>,
}

static MUSIC: Mutex<Music> = Mutex::new(Music {
    active: false,
    channels: 2,
    chunks: VecDeque::new(),
    cursor: None,
    filter: TimeFilter::new(),
    filter_conn: 0,
    gain: 0.0,
    stat: (0.0, 0, None),
    flowing_since: None,
    dry_since: None,
});
static MUSIC_ON: AtomicBool = AtomicBool::new(false);
/// Music Assistant's mute of the panel (the store's, without its lock: the speaker thread reads it every 10 ms).
static MUTED: AtomicBool = AtomicBool::new(false);

/// A lock that a panic elsewhere does not poison for good: the speaker thread mixes the music under every sound, so it
/// must never stop on the music's lock.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
/// The last stall: the music had played for a while, then the buffer ran dry for a second (the network brought too
/// little). A start or a seek, whose first chunks are on their way, is no stall.
static LAST_STALL: Mutex<Option<Instant>> = Mutex::new(None);
/// A stall is this long without music after this long with it.
const STALL_DRY: Duration = Duration::from_secs(1);
const STALL_AFTER: Duration = Duration::from_secs(5);
/// A session that heard nothing from its server for this long (the clock exchanges answer every 5 s) ends.
const IDLE_LIMIT: Duration = Duration::from_secs(30);

/// Sendspin music is playing: the speaker keeps running for it.
pub fn music_active() -> bool {
    MUSIC_ON.load(Ordering::Relaxed)
}

/// The music stalled within the last `within` (and still plays).
pub fn stalled_within(within: Duration) -> bool {
    music_active() && lock(&LAST_STALL).is_some_and(|t| t.elapsed() <= within)
}

/// Development: counts as a stall now.
pub fn note_stall() {
    *lock(&LAST_STALL) = Some(Instant::now());
}

fn set_active(on: bool) {
    let mut m = lock(&MUSIC);
    m.active = on;
    m.flowing_since = None;
    m.dry_since = None;
    if !on {
        m.chunks.clear();
        m.cursor = None;
    }
    drop(m);
    let was = MUSIC_ON.swap(on, Ordering::SeqCst);
    if on && !was {
        if let Some(p) = PLAYER.get() {
            p.play(crate::voice::Sound::Wake);
        }
    }
}

static PLAYER: OnceLock<std::sync::Arc<crate::voice::Player>> = OnceLock::new();

/// Mixes the music due at `heard_at` (the panel's clock, µs; when the first frame of `out` will be heard) into
/// `out` (48 kHz mono). Small timing errors are corrected by dropping or repeating one frame per 10 ms (0.2 %,
/// inaudible); large ones (a start, a seek, an underrun) by jumping.
pub fn mix_music(out: &mut [i16], heard_at: i64) {
    let mut m = lock(&MUSIC);
    if !m.active || m.filter.count == 0 || out.is_empty() {
        return;
    }
    // silent while the voice assistant listens or thinks (as the XVF speaker mutes its amplifier)
    // the volume itself is the panel's hardware volume; here only mute
    let target = if MUTED.load(Ordering::Relaxed) || crate::voice::busy() && !crate::voice::speaking() { 0.0 } else { 1.0 };
    mix_into(&mut m, out, heard_at, target);
}

/// How many frames of music to take for `n` frames heard, the cursor `err` µs early (negative: late): beyond 200 µs
/// one fewer (one is repeated) or one more (one is dropped). A block of a single frame is taken as it is.
fn frames_to_take(n: usize, err: f64) -> usize {
    if n < 2 {
        n
    } else if err > 200.0 {
        n - 1
    } else if err < -200.0 {
        n + 1
    } else {
        n
    }
}

/// `take` frames of the chunks from the server time `start` (µs) on; silence where no chunk has them.
fn render(chunks: &VecDeque<(i64, Vec<i16>)>, start: f64, take: usize) -> Vec<i16> {
    let mut samples: Vec<i16> = Vec::with_capacity(take);
    let mut k = 0usize;
    for i in 0..take {
        let t = start + i as f64 * SAMPLE_US;
        let mut v = 0i16;
        while k < chunks.len() {
            let (ts, s) = &chunks[k];
            let pos = ((t - *ts as f64) / SAMPLE_US).round();
            if pos < 0.0 {
                break; // a gap before this chunk
            }
            if (pos as usize) < s.len() {
                v = s[pos as usize];
                break;
            }
            k += 1;
        }
        samples.push(v);
    }
    samples
}

/// The rendered frames brought to exactly `n`: one fewer gets its middle frame repeated, one more loses its middle
/// frame; anything else is padded with silence or cut.
fn fit(mut samples: Vec<i16>, n: usize) -> Vec<i16> {
    if samples.len() + 1 == n && !samples.is_empty() {
        let mid = samples.len() / 2;
        samples.insert(mid, samples[mid]);
    } else if samples.len() == n + 1 {
        samples.remove(samples.len() / 2);
    }
    samples.resize(n, 0);
    samples
}

/// The mixing itself, at the gain `target` (0 silent, 1 full).
fn mix_into(m: &mut Music, out: &mut [i16], heard_at: i64, target: f32) {
    let n = out.len();
    if n == 0 {
        return;
    }
    let due = m.filter.server_time(heard_at);
    let take = match m.cursor {
        Some(c) if (c - due).abs() <= 30_000.0 => frames_to_take(n, c - due),
        _ => {
            m.cursor = Some(due);
            n
        }
    };
    let start = m.cursor.unwrap_or(due);
    // chunks that ended before the cursor are gone
    while let Some((ts, s)) = m.chunks.front() {
        if (*ts as f64) + s.len() as f64 * SAMPLE_US <= start {
            m.chunks.pop_front();
        } else {
            break;
        }
    }
    let samples = render(&m.chunks, start, take);
    let end = start + take as f64 * SAMPLE_US;
    m.cursor = Some(end);
    // the stall watch: the buffer does not reach the end of this block
    let dry = m.chunks.back().is_none_or(|(ts, s)| (*ts as f64) + s.len() as f64 * SAMPLE_US < end);
    let now = Instant::now();
    if !dry {
        m.dry_since = None;
        m.flowing_since.get_or_insert(now);
    } else if m.flowing_since.is_some_and(|f| f.elapsed() >= STALL_AFTER) && m.dry_since.get_or_insert(now).elapsed() >= STALL_DRY {
        // once per stall: the music has to flow for a while again before the next one counts
        m.flowing_since = None;
        m.dry_since = None;
        *lock(&LAST_STALL) = Some(now);
        eprintln!("nspanel-app: sendspin: stall, the buffer ran dry for {} s", STALL_DRY.as_secs());
    } else if m.flowing_since.is_some_and(|f| f.elapsed() < STALL_AFTER) {
        // a gap soon after a start or a seek: no stall, the music starts flowing again
        m.flowing_since = None;
    }
    // back to n frames: one repeated or one dropped in the middle
    let samples = fit(samples, n);
    let mut g = m.gain;
    let step = 1.0 / 480.0; // full scale in 10 ms
    for (o, s) in out.iter_mut().zip(samples.iter()) {
        if g < target {
            g = (g + step).min(target);
        } else if g > target {
            g = (g - step).max(target);
        }
        let v = *s as f32 * g;
        m.stat.0 += (v as f64) * (v as f64);
        *o = (*o as f32 + v).clamp(-32768.0, 32767.0) as i16;
    }
    m.gain = g;
    m.stat.1 += n as u64;
    // every 30 s while music plays: how much is buffered and how loud the music is in the speaker's signal
    let since = *m.stat.2.get_or_insert_with(Instant::now);
    if since.elapsed() >= Duration::from_secs(30) {
        let buffered_ms = m.chunks.back().map_or(0.0, |(ts, s)| (*ts as f64 + s.len() as f64 * SAMPLE_US - due) / 1000.0);
        let rms = (m.stat.0 / m.stat.1.max(1) as f64).sqrt();
        let dbfs = if rms > 0.0 { 20.0 * (rms / 32768.0).log10() } else { -120.0 };
        eprintln!("nspanel-app: sendspin: playing, {buffered_ms:.0} ms buffered, music {dbfs:.1} dBFS (gain {g:.3})");
        m.stat = (0.0, 0, Some(Instant::now()));
    }
}

// ---------------------------------------------------------------- announcing and listening

/// The Sendspin service for the mDNS announcement (the panel waits for Music Assistant to connect).
pub fn mdns_service() -> crate::mdns::Service {
    crate::mdns::Service {
        name: crate::esphome::device_name().into(),
        kind: "_sendspin._tcp".into(),
        port: PORT,
        txt: vec![format!("path={PATH}"), format!("name={}", player_name())],
    }
}

/// The panel's volume (its hardware volume, the one of the shade and Home Assistant): Music Assistant shows and sets
/// this one, so the panel has a single volume. A change from elsewhere is reported to the server (STATE_DIRTY).
static VOLUME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(60);
static STATE_DIRTY: AtomicBool = AtomicBool::new(false);
static SET_VOLUME: OnceLock<Box<dyn Fn(u8) + Send + Sync>> = OnceLock::new();

/// The panel's volume changed (shade, Home Assistant, the speaker mark).
pub fn panel_volume(percent: u8) {
    if VOLUME.swap(percent.min(100), Ordering::SeqCst) != percent.min(100) {
        STATE_DIRTY.store(true, Ordering::SeqCst);
    }
}

/// Each new connection gets a number; the admitted one is in ADMITTED.
static CONN_SEQ: AtomicU64 = AtomicU64::new(0);
/// The app is stopping: the session says goodbye ("restart"), so the server comes back as soon as the app runs again.
static GOODBYE: AtomicBool = AtomicBool::new(false);
static GOODBYE_SENT: AtomicBool = AtomicBool::new(false);

/// Says goodbye to the server before the app stops (waits at most 700 ms; at once without a session).
pub fn goodbye() {
    GOODBYE.store(true, Ordering::SeqCst);
    if lock(&ADMITTED).is_none() {
        return;
    }
    let t = Instant::now();
    while !GOODBYE_SENT.load(Ordering::SeqCst) && t.elapsed() < Duration::from_millis(700) {
        std::thread::sleep(Duration::from_millis(10));
    }
}
static ADMITTED: Mutex<Option<(u64, bool)>> = Mutex::new(None); // (connection, declares playback)

/// The connection `id` holds the panel now.
fn holds(id: u64) -> bool {
    lock(&ADMITTED).is_some_and(|(a, _)| a == id)
}

/// The connection `id` no longer holds the panel (it ended, or its keys no longer allow playback): its music stops.
fn release(id: u64) {
    let mut a = lock(&ADMITTED);
    if a.is_some_and(|(c, _)| c == id) {
        *a = None;
        drop(a);
        set_active(false);
    }
}

pub fn spawn(player: std::sync::Arc<crate::voice::Player>, volume: u8, set_volume: impl Fn(u8) + Send + Sync + 'static) {
    let _ = PLAYER.set(player);
    VOLUME.store(volume.min(100), Ordering::SeqCst);
    let _ = SET_VOLUME.set(Box::new(set_volume));
    // loads or creates the identity (client id and token for Home Assistant)
    MUTED.store(store().lock().unwrap().muted, Ordering::SeqCst);
    std::thread::spawn(|| {
        let listener = loop {
            match TcpListener::bind(("0.0.0.0", PORT)) {
                Ok(l) => break l,
                Err(e) => {
                    eprintln!("nspanel-app: sendspin: listen: {e}");
                    std::thread::sleep(Duration::from_secs(10));
                }
            }
        };
        eprintln!("nspanel-app: sendspin: player waiting on port {PORT}");
        for conn in listener.incoming().flatten() {
            let id = CONN_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
            std::thread::spawn(move || {
                let peer = conn.peer_addr().map(|a| a.to_string()).unwrap_or_default();
                match session(conn, id) {
                    Ok(why) => eprintln!("nspanel-app: sendspin: {peer} ended ({why})"),
                    Err(e) => eprintln!("nspanel-app: sendspin: {peer}: {e}"),
                }
                release(id);
            });
        }
    });
}

// ---------------------------------------------------------------- one connection

struct Conn {
    ws: WebSocket<TcpStream>,
    transport: snow::TransportState,
    kind: PskKind,
    server_id: String,
    /// The handshake hash: the prologue of a re-handshake.
    h: Vec<u8>,
    /// A fragmented message in flight: (type, data).
    frag: Option<(u8, Vec<u8>)>,
}

impl Conn {
    /// A clean WebSocket close (the server sees a normal close, not a dropped connection), at most 300 ms.
    fn close(&mut self) {
        let _ = self.ws.close(None);
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(300) {
            match self.ws.read() {
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    let _ = self.ws.flush();
                }
                Err(_) => break,
            }
        }
    }

    fn send(&mut self, v: &Value) -> Result<(), String> {
        let mut plain = vec![0u8];
        plain.extend(v.to_string().as_bytes());
        let mut out = vec![0u8; plain.len() + 64];
        let n = self.transport.write_message(&plain, &mut out).map_err(|e| format!("encrypt: {e}"))?;
        out.truncate(n);
        self.ws.send(Message::Binary(out.into())).map_err(|e| format!("send: {e}"))
    }

    /// The next whole decrypted message (type byte first), the time its last byte arrived; None after a timeout.
    fn recv(&mut self) -> Result<Option<(Vec<u8>, i64)>, String> {
        loop {
            let msg = match self.ws.read() {
                Ok(m) => m,
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return Ok(None),
                Err(e) => return Err(format!("read: {e}")),
            };
            let at = now_us();
            let data = match msg {
                Message::Binary(d) => d,
                Message::Close(_) => return Err("closed by the server".into()),
                Message::Text(_) => return Err("cleartext after the handshake".into()),
                _ => continue,
            };
            let mut plain = vec![0u8; data.len()];
            let n = self.transport.read_message(&data, &mut plain).map_err(|_| "decrypt failed".to_string())?;
            plain.truncate(n);
            if plain.is_empty() {
                return Err("empty message".into());
            }
            if plain[0] != 1 {
                if self.frag.is_some() {
                    return Err("message inside a fragmented one".into());
                }
                return Ok(Some((plain, at)));
            }
            // a fragment: [1][flags][orig_type][data] first, [1][flags][data] after
            let flags = *plain.get(1).ok_or("fragment without flags")?;
            if flags & 0b1111_1100 != 0 {
                return Err("fragment flags".into());
            }
            if flags & 2 != 0 {
                if self.frag.is_some() {
                    return Err("second first fragment".into());
                }
                let t = *plain.get(2).ok_or("fragment without type")?;
                if t == 1 {
                    return Err("fragment of a fragment".into());
                }
                self.frag = Some((t, plain[3..].to_vec()));
            } else {
                let Some((_, buf)) = self.frag.as_mut() else { return Err("fragment out of sequence".into()) };
                buf.extend_from_slice(&plain[2..]);
                if buf.len() > 4 * 1024 * 1024 {
                    return Err("fragmented message too long".into());
                }
            }
            if flags & 1 != 0 {
                let (t, buf) = self.frag.take().unwrap();
                let mut whole = vec![t];
                whole.extend(buf);
                return Ok(Some((whole, at)));
            }
        }
    }
}

fn text(ws: &mut WebSocket<TcpStream>, deadline: Instant) -> Result<String, String> {
    loop {
        if Instant::now() > deadline {
            return Err("handshake timeout".into());
        }
        match ws.read() {
            Ok(Message::Text(t)) => return Ok(t.to_string()),
            Ok(Message::Close(_)) => return Err("closed during the handshake".into()),
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(e) => return Err(format!("read: {e}")),
        }
    }
}

/// Noise as the responder: reads message 1, picks the PSK it names, answers with message 2.
fn noise_respond(
    msg1: &[u8],
    prologue: &[u8],
    server_id: &str,
    initial: bool,
) -> Result<(snow::HandshakeState, Vec<u8>, PskKind), String> {
    let params: snow::params::NoiseParams = NOISE.parse().map_err(|_| "noise params")?;
    let remote = B64U.decode(server_id).ok().filter(|k| k.len() == 32).ok_or("bad server_id")?;
    let private = store().lock().unwrap().private.clone();
    let mut hs = snow::Builder::new(params)
        .local_private_key(&private)
        .remote_public_key(&remote)
        .prologue(prologue)
        .build_responder()
        .map_err(|e| format!("noise: {e}"))?;
    let mut payload = vec![0u8; msg1.len()];
    let n = hs.read_message(msg1, &mut payload).map_err(|_| "noise message 1 failed".to_string())?;
    let p1: Value = serde_json::from_slice(&payload[..n]).map_err(|_| "noise message 1 payload".to_string())?;
    let (kind, psk) = choose_psk(&p1, server_id, initial).ok_or("no PSK for the server's psk_id")?;
    hs.set_psk(2, &psk).map_err(|e| format!("psk: {e}"))?;
    let mut msg2 = vec![0u8; 256];
    let n = hs.write_message(b"{}", &mut msg2).map_err(|e| format!("noise message 2: {e}"))?;
    msg2.truncate(n);
    Ok((hs, msg2, kind))
}

/// What a `server/activate` may do, by the session's PSK (unpaired access is off).
#[derive(Debug, PartialEq)]
enum Admission {
    /// The session ends with this goodbye reason.
    Refuse(&'static str),
    /// Not paired (the pairing PSK, the sentinel): pairing only; the server that holds the panel and its music stay.
    PairingOnly,
    /// A paired server: it holds the panel, as a player or not.
    Admit { player: bool },
}

/// The activation of `activities` with `roles` (None: as before, `player_now`) for a session with a `kind` PSK.
fn admission(kind: PskKind, activities: &[String], roles: Option<&[String]>, player_now: bool) -> Admission {
    let playback = activities.iter().any(|a| a == "playback");
    let pairing = activities.iter().any(|a| a == "pairing");
    if kind != PskKind::LongTerm {
        if playback || roles.is_some_and(|r| !r.is_empty()) { Admission::Refuse("pairing_required") } else { Admission::PairingOnly }
    } else if pairing {
        Admission::Refuse("unauthorized")
    } else {
        Admission::Admit { player: roles.map_or(player_now, |r| r.iter().any(|x| x == "player@v1")) }
    }
}

fn player_state() -> Value {
    let s = store().lock().unwrap();
    json!({"type": "client/state", "payload": {
        "available": true,
        "player": {
            "volume": VOLUME.load(Ordering::SeqCst),
            "muted": s.muted,
            "output_delay_ms": 0,
            "required_lead_time_ms": 300,
            "min_buffer_ms": 250,
            "supported_commands": ["volume", "mute"],
        }
    }})
}

fn session(stream: TcpStream, id: u64) -> Result<String, String> {
    let _ = stream.set_nodelay(true);
    stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    // the endpoint only, and never from a browser page (no Origin header)
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        if req.uri().path() != PATH || req.headers().contains_key("origin") {
            let mut r = ErrorResponse::new(None);
            *r.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            return Err(r);
        }
        Ok(resp)
    };
    let mut ws = tungstenite::accept_hdr(stream, check).map_err(|e| format!("upgrade: {e}"))?;
    ws.get_ref().set_read_timeout(Some(Duration::from_millis(20))).map_err(|e| e.to_string())?;

    // cleartext init, then the Noise handshake
    let deadline = Instant::now() + Duration::from_secs(30);
    let client_init = json!({"type": "client/init", "payload": {"client_id": client_id().unwrap_or_default(), "version": 1, "suite": SUITE}}).to_string();
    ws.send(Message::Text(client_init.clone().into())).map_err(|e| e.to_string())?;
    let server_init = text(&mut ws, deadline)?;
    let si: Value = serde_json::from_str(&server_init).map_err(|_| "server/init is not JSON")?;
    if si.get("type").and_then(Value::as_str) != Some("server/init") {
        return Err(format!("expected server/init: {}", si.get("type").and_then(Value::as_str).unwrap_or("?")));
    }
    let server_id = si["payload"]["server_id"].as_str().ok_or("server/init without server_id")?.to_string();
    if si["payload"]["version"].as_i64() != Some(1) {
        return Err("server speaks another version".into());
    }
    let hs1: Value = serde_json::from_str(&text(&mut ws, deadline)?).map_err(|_| "noise/handshake is not JSON")?;
    let msg1 = hs1["payload"]["data"].as_str().and_then(|d| B64U.decode(d).ok()).ok_or("noise/handshake without data")?;
    let mut prologue = client_init.into_bytes();
    prologue.extend(server_init.as_bytes());
    let (hs, msg2, kind) = noise_respond(&msg1, &prologue, &server_id, true)?;
    ws.send(Message::Text(json!({"type": "noise/handshake", "payload": {"data": B64U.encode(&msg2)}}).to_string().into()))
        .map_err(|e| e.to_string())?;
    let h = hs.get_handshake_hash().to_vec();
    let transport = hs.into_transport_mode().map_err(|e| format!("transport: {e}"))?;
    eprintln!("nspanel-app: sendspin: connected to {} ({kind:?})", &server_id[..8.min(server_id.len())]);
    let mut c = Conn { ws, transport, kind, server_id, h, frag: None };

    let mut activities: Vec<String> = Vec::new();
    // the player role is only a paired server's (long-term PSK), and only it holds the panel (ADMITTED): a session
    // that has not paired (the pairing PSK, the sentinel) may pair, never touch the music
    let mut player_role = false;
    let mut activated = false;
    let mut admitted = false;
    let mut last_heard = Instant::now();
    let mut pairing_index = 0u32;
    let mut pending_psk: Option<Vec<u8>> = None;
    let mut state_sent = false;
    // clock bursts: 8 exchanges, one after the other, every 5 s; the best of each burst feeds the filter
    let mut burst_at = Instant::now() - Duration::from_secs(60);
    let mut burst_left = 0u32;
    let mut asked_at: Option<(i64, Instant)> = None;
    let mut best: Option<(i64, i64, i64)> = None; // (measurement, max_error, time_added)
    let mut synced = false;
    let started = Instant::now();

    loop {
        if GOODBYE.load(Ordering::SeqCst) {
            let _ = c.send(&json!({"type": "client/goodbye", "payload": {"reason": "restart"}}));
            c.close();
            GOODBYE_SENT.store(true, Ordering::SeqCst);
            return Ok("the app stops".into());
        }
        if admitted && !holds(id) {
            let _ = c.send(&json!({"type": "client/goodbye", "payload": {"reason": "another_server"}}));
            return Ok("another server took over".into());
        }
        if !activated && started.elapsed() > Duration::from_secs(30) {
            return Err("no server/activate within 30 s".into());
        }
        // a live server answers the clock exchanges every 5 s: a silent one is gone (a half-open connection would
        // keep the music "on" and the roam check off until TCP gives up)
        if activated && last_heard.elapsed() > IDLE_LIMIT {
            return Err(format!("nothing from the server for {} s", IDLE_LIMIT.as_secs()));
        }
        // the clock
        if activated {
            let now = Instant::now();
            if burst_left == 0 && now.duration_since(burst_at) >= Duration::from_secs(5) {
                burst_at = now;
                burst_left = 8;
                best = None;
            }
            let reply_overdue = asked_at.is_some_and(|(_, t)| now.duration_since(t) > Duration::from_millis(500));
            if burst_left > 0 && (asked_at.is_none() || reply_overdue) {
                if reply_overdue {
                    burst_left -= 1;
                }
                if burst_left > 0 {
                    let t1 = now_us();
                    c.send(&json!({"type": "client/time", "payload": {"client_transmitted": t1}}))?;
                    asked_at = Some((t1, now));
                } else {
                    asked_at = None;
                }
            }
            if burst_left == 0 {
                if let Some((meas, err, added)) = best.take() {
                    // only the server that holds the panel as a player sets the music's clock (another server's clock
                    // base starts the filter afresh)
                    if player_role && holds(id) {
                        let mut m = lock(&MUSIC);
                        if m.filter_conn != id {
                            m.filter = TimeFilter::new();
                            m.filter_conn = id;
                        }
                        m.filter.update(meas, err, added);
                    }
                    synced = true;
                }
            }
            if synced && player_role && !state_sent {
                c.send(&player_state())?;
                state_sent = true;
                STATE_DIRTY.store(false, Ordering::SeqCst);
            }
            if state_sent && STATE_DIRTY.swap(false, Ordering::SeqCst) {
                c.send(&player_state())?;
            }
        }

        let Some((msg, at)) = c.recv()? else { continue };
        last_heard = Instant::now();
        // this connection plays on the panel: a paired server holding it as a player, with playback
        let plays = player_role && activities.iter().any(|a| a == "playback") && holds(id);
        if msg[0] == 4 {
            // an audio chunk: timestamp, send_ahead, PCM
            if msg.len() < 13 {
                return Err("short audio chunk".into());
            }
            if !plays {
                continue;
            }
            let ts = i64::from_be_bytes(msg[1..9].try_into().unwrap());
            let mut m = lock(&MUSIC);
            if m.active {
                let pcm = &msg[13..];
                let mono: Vec<i16> = if m.channels == 2 {
                    pcm.chunks_exact(4)
                        .map(|f| ((i16::from_le_bytes([f[0], f[1]]) as i32 + i16::from_le_bytes([f[2], f[3]]) as i32) / 2) as i16)
                        .collect()
                } else {
                    pcm.chunks_exact(2).map(|f| i16::from_le_bytes([f[0], f[1]])).collect()
                };
                let pos = m.chunks.iter().rposition(|(t, _)| *t <= ts).map_or(0, |p| p + 1);
                m.chunks.insert(pos, (ts, mono));
                // never more than ~10 s
                while m.chunks.len() > 400 {
                    m.chunks.pop_front();
                }
            }
            continue;
        }
        if msg[0] != 0 {
            continue; // binary messages of roles the panel does not have
        }
        let v: Value = serde_json::from_slice(&msg[1..]).map_err(|_| "message is not JSON")?;
        let kind_s = v.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        let p = v.get("payload").cloned().unwrap_or(Value::Null);
        match kind_s.as_str() {
            "server/hello" => {
                let mac = crate::esphome::mac_address().to_lowercase();
                c.send(&json!({"type": "client/hello", "payload": {
                    "name": player_name(),
                    "device_info": {
                        "product_name": "NSPanel Pro 86",
                        "manufacturer": "PanelOS",
                        "software_version": env!("CARGO_PKG_VERSION"),
                        "mac_address": mac,
                    },
                    "supported_roles": ["player@v1"],
                    "player@v1_support": {
                        "supported_formats": [{"codec": "pcm", "channels": 2, "sample_rate": 48000, "bit_depth": 16}],
                        "buffer_capacity": BUFFER_CAPACITY,
                    },
                    "supported_pair_methods": {"pairing_psk": {"locations": ["device"]}},
                    "unpaired_access": {"enabled": false},
                }}))?;
            }
            "server/activate" => {
                let acts: Vec<String> =
                    p.get("activities").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                let roles: Option<Vec<String>> =
                    p.get("active_roles").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect());
                let playback = acts.iter().any(|a| a == "playback");
                let pairing = acts.iter().any(|a| a == "pairing");
                match admission(c.kind, &acts, roles.as_deref(), activated && player_role) {
                    Admission::Refuse(reason) => {
                        let _ = c.send(&json!({"type": "client/goodbye", "payload": {"reason": reason}}));
                        return Ok(format!("activation refused ({reason})"));
                    }
                    Admission::Admit { player } => {
                        // one admitted connection: playback wins over an idle one
                        {
                            let mut a = lock(&ADMITTED);
                            match *a {
                                Some((other, other_playback)) if other != id && other_playback && !playback => {
                                    drop(a);
                                    let _ = c.send(&json!({"type": "client/goodbye", "payload": {"reason": "concurrent_attempt"}}));
                                    return Ok("another server is playing".into());
                                }
                                _ => *a = Some((id, playback)),
                            }
                        }
                        admitted = true;
                        player_role = player;
                        if player {
                            // a new player: the clock and the chunks of the one before (stamped with its clock) go,
                            // before this one's first chunks come
                            let mut m = lock(&MUSIC);
                            if m.filter_conn != id {
                                m.filter = TimeFilter::new();
                                m.filter_conn = id;
                                m.chunks.clear();
                                m.cursor = None;
                            }
                        }
                        if playback {
                            let mut s = store().lock().unwrap();
                            if s.last_server != c.server_id {
                                s.last_server = c.server_id.clone();
                                save_store(&s);
                            }
                        }
                        if !playback || !player_role {
                            set_active(false);
                        }
                    }
                    Admission::PairingOnly => player_role = false,
                }
                if !player_role {
                    state_sent = false;
                }
                activated = true;
                activities = acts;
                if pairing {
                    pairing_index += 1;
                    let method = p["pairing"]["method"].as_str().unwrap_or("");
                    if method != "pairing_psk" || c.kind != PskKind::Pairing {
                        c.send(&json!({"type": "pair/abort", "payload": {"reason": "method_not_supported"}}))?;
                    } else {
                        // Pairing PSK flow: pair-init and the new long-term PSK at once
                        let psk = random32();
                        c.send(&json!({"type": "client/pair-init", "payload": {"pairing_index": pairing_index}}))?;
                        c.send(&json!({"type": "client/pair-finalize", "payload": {"long_term_psk": B64U.encode(&psk)}}))?;
                        pending_psk = Some(psk);
                    }
                } else {
                    pending_psk = None;
                }
                eprintln!("nspanel-app: sendspin: activate {activities:?} player={player_role}");
            }
            "server/pair-finalize" => {
                if let Some(psk) = pending_psk.take() {
                    let mut s = store().lock().unwrap();
                    s.records.retain(|(sid, _)| sid != &c.server_id);
                    s.records.push((c.server_id.clone(), psk));
                    while s.records.len() > 8 {
                        s.records.remove(0);
                    }
                    save_store(&s);
                    eprintln!("nspanel-app: sendspin: paired with {}", &c.server_id[..8.min(c.server_id.len())]);
                }
            }
            "noise/handshake" => {
                // a re-handshake inside the channel (after pairing, or to rotate the keys)
                let msg1 = p["data"].as_str().and_then(|d| B64U.decode(d).ok()).ok_or("re-handshake without data")?;
                let prologue = c.h.clone();
                let (hs, msg2, kind) = noise_respond(&msg1, &prologue, &c.server_id, false)?;
                c.send(&json!({"type": "noise/handshake", "payload": {"data": B64U.encode(&msg2)}}))?;
                c.h = hs.get_handshake_hash().to_vec();
                c.transport = hs.into_transport_mode().map_err(|e| format!("transport: {e}"))?;
                c.kind = kind;
                pairing_index = 0;
                // what the session may do follows its keys: without a long-term PSK no player, no hold on the panel
                if kind != PskKind::LongTerm {
                    player_role = false;
                    state_sent = false;
                    if admitted {
                        admitted = false;
                        release(id);
                    }
                }
                eprintln!("nspanel-app: sendspin: new session keys ({kind:?})");
            }
            "server/time" => {
                let t1 = p["client_transmitted"].as_i64().unwrap_or(0);
                if asked_at.is_some_and(|(sent, _)| sent == t1) {
                    let t2 = p["server_received"].as_i64().unwrap_or(0);
                    let t3 = p["server_transmitted"].as_i64().unwrap_or(0);
                    let t4 = at;
                    let measurement = ((t2 - t1) + (t3 - t4)) / 2;
                    let max_error = ((t4 - t1) - (t3 - t2)) / 2;
                    if max_error > 0 && best.is_none_or(|(_, e, _)| max_error < e) {
                        best = Some((measurement, max_error, t4));
                    }
                    asked_at = None;
                    burst_left = burst_left.saturating_sub(1);
                }
            }
            "stream/start" => {
                if let Some(pl) = p.get("player").filter(|_| plays) {
                    let ok = pl["codec"] == "pcm" && pl["sample_rate"] == 48000 && pl["bit_depth"] == 16;
                    let channels = pl["channels"].as_u64().unwrap_or(2) as u16;
                    {
                        let mut m = lock(&MUSIC);
                        m.channels = channels;
                        if !m.active {
                            m.chunks.clear();
                            m.cursor = None;
                        }
                    }
                    set_active(ok);
                    eprintln!("nspanel-app: sendspin: stream {} {} Hz {} ch", pl["codec"].as_str().unwrap_or("?"), pl["sample_rate"], channels);
                    if !ok {
                        eprintln!("nspanel-app: sendspin: unsupported format {pl}");
                    }
                }
            }
            "stream/clear" => {
                let roles = p.get("roles").and_then(Value::as_array);
                if plays && roles.is_none_or(|r| r.iter().any(|x| x == "player")) {
                    let mut m = lock(&MUSIC);
                    // a seek: the new chunks are on their way, the gap is no stall
                    m.flowing_since = None;
                    m.dry_since = None;
                    match p.get("from_timestamp").and_then(Value::as_i64) {
                        Some(from) => m.chunks.retain(|(t, _)| *t < from),
                        None => {
                            m.chunks.clear();
                            m.cursor = None;
                        }
                    }
                }
            }
            "stream/end" => {
                let roles = p.get("roles").and_then(Value::as_array);
                if plays && roles.is_none_or(|r| r.iter().any(|x| x == "player")) {
                    set_active(false);
                }
            }
            "server/command" => {
                if let Some(pl) = p.get("player").filter(|_| player_role && holds(id)) {
                    let changed = {
                        let mut s = store().lock().unwrap();
                        let before = (s.volume, s.muted);
                        match pl["command"].as_str() {
                            Some("volume") => {
                                let v = pl["volume"].as_u64().unwrap_or(VOLUME.load(Ordering::SeqCst) as u64).min(100) as u8;
                                if v != VOLUME.swap(v, Ordering::SeqCst) {
                                    if let Some(f) = SET_VOLUME.get() {
                                        f(v);
                                    }
                                }
                                STATE_DIRTY.store(false, Ordering::SeqCst);
                            }
                            Some("mute") => {
                                s.muted = pl["mute"].as_bool().unwrap_or(s.muted);
                                MUTED.store(s.muted, Ordering::SeqCst);
                            }
                            _ => {}
                        }
                        let changed = before != (s.volume, s.muted);
                        if changed {
                            save_store(&s);
                        }
                        changed
                    };
                    if changed || state_sent {
                        c.send(&player_state())?;
                    }
                }
            }
            "server/unpair" => {
                if c.kind == PskKind::LongTerm {
                    let mut s = store().lock().unwrap();
                    s.records.retain(|(sid, _)| sid != &c.server_id);
                    save_store(&s);
                    drop(s);
                    let _ = c.send(&json!({"type": "client/goodbye", "payload": {"reason": "unpaired"}}));
                    return Ok("unpaired".into());
                }
            }
            "group/update" => {
                eprintln!(
                    "nspanel-app: sendspin: group {} {}",
                    p["group_name"].as_str().unwrap_or(""),
                    p["playback_state"].as_str().unwrap_or("")
                );
            }
            "pair/abort" => {
                pending_psk = None;
                eprintln!("nspanel-app: sendspin: pairing aborted ({})", p["reason"].as_str().unwrap_or(""));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_token_matches_the_spec_vector() {
        let key: Vec<u8> = (0u8..32).collect();
        let psk: Vec<u8> = (0xe0u8..=0xff).collect();
        assert_eq!(
            token_of(&key, &psk),
            "SP:0AAAQEAYEAUDAOCAJBIFQYDIOB4IBCEQTCQKRMFYYDENBWHA5DYP6BYPC4PSOLZXH5DU6V97M5XXO74HR6LZ7J5PW674PT6X37T6757Y"
        );
    }

    #[test]
    fn sentinel_psk_id_is_the_published_constant() {
        assert_eq!(psk_id(&sentinel_psk()), "GFsV9tLaSQm9HcFWpKsgYQOr7wFTvNUtkmFwuVz3zoo");
    }

    #[test]
    fn time_filter_follows_an_offset() {
        let mut f = TimeFilter::new();
        for i in 1..50i64 {
            f.update(1_000_000, 500, i * 1_000_000);
        }
        assert!((f.server_time(10_000_000) - 11_000_000.0).abs() < 1.0);
    }

    fn music(cursor_early_us: Option<f64>, chunk: Vec<i16>) -> Music {
        let mut filter = TimeFilter::new();
        filter.update(0, 500, 1); // the server's clock is the panel's
        let mut chunks = VecDeque::new();
        chunks.push_back((1_000_000i64, chunk));
        Music {
            active: true,
            channels: 2,
            chunks,
            cursor: cursor_early_us.map(|e| 1_000_000.0 + e),
            filter,
            filter_conn: 1,
            gain: 1.0,
            stat: (0.0, 0, None),
            flowing_since: None,
            dry_since: None,
        }
    }

    #[test]
    fn frames_to_take_never_takes_none_of_a_block() {
        assert_eq!(frames_to_take(0, 1000.0), 0);
        assert_eq!(frames_to_take(1, 1000.0), 1);
        assert_eq!(frames_to_take(1, -1000.0), 1);
        assert_eq!(frames_to_take(2, 1000.0), 1);
        assert_eq!(frames_to_take(480, 1000.0), 479);
        assert_eq!(frames_to_take(480, -1000.0), 481);
        assert_eq!(frames_to_take(480, 100.0), 480);
    }

    #[test]
    fn fit_gives_exactly_n_frames() {
        assert_eq!(fit(vec![], 0), Vec::<i16>::new());
        assert_eq!(fit(vec![], 1), vec![0]);
        assert_eq!(fit(vec![7], 2), vec![7, 7]);
        assert_eq!(fit(vec![1, 2, 3], 2), vec![1, 3]);
        assert_eq!(fit(vec![1, 2, 3], 4), vec![1, 2, 2, 3]);
        assert_eq!(fit(vec![1, 2], 5), vec![1, 2, 0, 0, 0]);
        assert_eq!(fit(vec![1, 2, 3, 4], 2), vec![1, 2]);
    }

    #[test]
    fn render_fills_gaps_with_silence() {
        let mut chunks = VecDeque::new();
        assert_eq!(render(&chunks, 0.0, 3), vec![0, 0, 0]);
        chunks.push_back((0i64, vec![5i16]));
        assert_eq!(render(&chunks, 0.0, 0), Vec::<i16>::new());
        assert_eq!(render(&chunks, 0.0, 3), vec![5, 0, 0]);
    }

    #[test]
    fn one_frame_blocks_mix_early_late_and_on_time() {
        // the last piece of a sound is one frame long while the music's cursor is early, late or on time
        for early in [Some(1_000.0), Some(-1_000.0), Some(0.0), None] {
            for chunk in [vec![], vec![1000i16], vec![1000i16; 480]] {
                let mut m = music(early, chunk);
                let mut out = [0i16; 1];
                mix_into(&mut m, &mut out, 1_000_000, 1.0);
                let mut empty: [i16; 0] = [];
                mix_into(&mut m, &mut empty, 1_000_000, 1.0);
            }
        }
    }

    #[test]
    fn any_block_length_mixes_to_its_own_length() {
        let mut m = music(Some(500.0), vec![1000i16; 4800]);
        let mut heard = 1_000_000i64;
        for n in [1usize, 2, 3, 480, 1, 0, 1, 481, 2, 1] {
            let mut out = vec![0i16; n];
            mix_into(&mut m, &mut out, heard, 1.0);
            assert_eq!(out.len(), n);
            // the cursor jumps between early and late
            heard += (n as f64 * SAMPLE_US) as i64 + if n % 2 == 0 { 400 } else { -400 };
        }
    }

    #[test]
    fn only_a_paired_server_holds_the_panel() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let player = s(&["player@v1"]);
        for kind in [PskKind::Sentinel, PskKind::Pairing] {
            assert_eq!(admission(kind, &s(&["pairing"]), None, false), Admission::PairingOnly);
            assert_eq!(admission(kind, &[], None, false), Admission::PairingOnly);
            assert_eq!(admission(kind, &s(&["playback"]), None, false), Admission::Refuse("pairing_required"));
            assert_eq!(admission(kind, &s(&["pairing"]), Some(&player), false), Admission::Refuse("pairing_required"));
            assert_eq!(admission(kind, &s(&["pairing"]), Some(&[]), false), Admission::PairingOnly);
        }
        assert_eq!(admission(PskKind::LongTerm, &s(&["pairing"]), None, false), Admission::Refuse("unauthorized"));
        assert_eq!(admission(PskKind::LongTerm, &s(&["playback"]), Some(&player), false), Admission::Admit { player: true });
        assert_eq!(admission(PskKind::LongTerm, &s(&["playback"]), None, true), Admission::Admit { player: true });
        assert_eq!(admission(PskKind::LongTerm, &[], Some(&[]), true), Admission::Admit { player: false });
    }
}
