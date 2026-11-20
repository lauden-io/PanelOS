//! Voice satellite: microphones (PDM, 16 kHz) → on-device wake word →
//! audio to Home Assistant's Assist pipeline over the ESPHome API → spoken
//! answer (TTS audio over the API) on the speaker. Follow-up listening when
//! Home Assistant asks for it (`continue_conversation`), like ESPHome voice
//! satellites. Echo cancellation (WebRTC AEC3, `native/aec.cc`) with the
//! player's own output as the reference, so the wake word also works while
//! the panel talks or beeps (it interrupts the answer).

use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};

pub const MODEL_DIR: &str = "/usr/share/nspanel/wakewords";
const CAPTURE_DEVICE: &str = "hw:CARD=pdmmics,DEV=0";
const PLAYBACK_DEVICE: &str = "hw:CARD=rk809,DEV=0";
const MIC_CHANNELS: usize = 4; // PDM data lines 0 and 1; the microphones are channels 2 and 3
const MIC_GAIN: i32 = 8;
const CHUNK: usize = 512; // samples per read (32 ms at 16 kHz)
const AEC_FRAME: usize = 160; // 10 ms at 16 kHz

/// What the speaker plays, as 16 kHz mono, for the echo canceller: the player
/// appends while it writes to ALSA, the microphone thread takes 10 ms per
/// microphone frame (zeros when it is empty).
pub type EchoRef = Arc<Mutex<VecDeque<i16>>>;

/// voice thread → ESPHome API thread
pub enum ToApi {
    /// Level (dBFS), SNR (dB, None without enough background) and model score
    /// (0..1) of a wake word, sent right before its `Start`.
    WakeInfo { level_dbfs: f32, score: f32, snr_db: Option<f32> },
    Start { conversation_id: String, wake_word_phrase: String },
    Audio(Vec<u8>),
    Stop,
    AnnounceFinished { success: bool },
}

/// ESPHome API thread → voice thread
pub enum FromApi {
    Started { error: bool },
    Event { kind: i32, data: Vec<(String, String)> },
    TtsAudio(Vec<u8>),
    SetWakeWords(Vec<String>),
    /// Home Assistant announcement: media URL (absolute or relative to Home Assistant) and its text.
    Announce { media_id: String, text: String },
}

/// voice thread → UI
/// The microphone level while the voice view shows (0 … 1 for -60 … -20 dBFS, f32 bits): the view's halo and trace
/// read it.
pub static MIC_LEVEL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Set while the voice view shows: only then are the level and the traces kept up to date.
pub static METER_ON: AtomicBool = AtomicBool::new(false);
/// The last 100 ms of the microphone and of the panel's own sound (16 kHz): the oscilloscope look draws the sound itself.
pub static MIC_TRACE: std::sync::Mutex<std::collections::VecDeque<i16>> = std::sync::Mutex::new(std::collections::VecDeque::new());
pub static OUT_TRACE: std::sync::Mutex<std::collections::VecDeque<i16>> = std::sync::Mutex::new(std::collections::VecDeque::new());
const TRACE_KEEP: usize = 1600;

fn keep_trace(q: &std::sync::Mutex<std::collections::VecDeque<i16>>, samples: impl Iterator<Item = i16>) {
    let mut q = q.lock().unwrap();
    q.extend(samples);
    let extra = q.len().saturating_sub(TRACE_KEEP);
    q.drain(..extra);
}

#[derive(Debug, Clone)]
pub enum VoiceUi {
    Listening,
    /// Listening for more without a wake word (Home Assistant asked back): ends after FOLLOW_UP_SILENCE.
    FollowUp,
    /// Nothing was understood (no text from the speech).
    NotUnderstood,
    Heard(String),
    Thinking,
    Answer(String),
    Idle,
    Error(String),
    /// "Stop" said while the panel spoke or a timer rang.
    Stopped,
}

#[derive(Debug, Clone)]
pub struct WakeWord {
    pub id: String,
    pub phrase: String,
    pub path: String,
    pub cutoff: f32,
    pub window: i32,
}

/// Models that run on the panel but are not offered as wake words: "stop"
/// listens only while the panel speaks or a timer rings, and stops it (as on
/// ESPHome voice satellites).
pub const INTERNAL_WORDS: &[&str] = &["stop"];

/// Wake word models installed on the panel (`<id>.tflite` + `<id>.json`).
pub fn available_wake_words() -> Vec<WakeWord> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(MODEL_DIR) else { return out };
    for entry in dir.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        if id == "vad" {
            continue;
        }
        let model = path.with_extension("tflite");
        if !model.exists() {
            continue;
        }
        let cutoff = v["micro"]["probability_cutoff"].as_f64().unwrap_or(0.97) as f32;
        out.push(WakeWord {
            phrase: v["wake_word"].as_str().unwrap_or(&id).to_string(),
            cutoff,
            window: v["micro"]["sliding_window_size"].as_i64().unwrap_or(5) as i32,
            path: model.to_string_lossy().into_owned(),
            id,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[repr(C)]
struct MwwEngine {
    _private: [u8; 0],
}

extern "C" {
    fn mww_create() -> *mut MwwEngine;
    fn mww_add_model(e: *mut MwwEngine, path: *const c_char, cutoff: f32, window: c_int) -> c_int;
    fn mww_set_enabled(e: *mut MwwEngine, index: c_int, enabled: c_int);
    fn mww_reset(e: *mut MwwEngine);
    fn mww_process(e: *mut MwwEngine, samples: *const i16, count: usize) -> c_int;
    fn mww_last_score(e: *mut MwwEngine) -> f32;
}

struct Engine {
    ptr: *mut MwwEngine,
    words: Vec<WakeWord>,
    last: Option<usize>,
}

#[repr(C)]
struct AecEngine {
    _private: [u8; 0],
}

extern "C" {
    fn aec_create(noise_suppression: c_int) -> *mut AecEngine;
    fn aec_process(e: *mut AecEngine, far: *const i16, near: *const i16, out: *mut i16) -> c_int;
}

/// The echo canceller keeps running (with silence as the far end) while this is set (the file
/// /tmp/nspanel-aec-warm sets it, for measurements: measured 2026-11-17 at 10 % volume, a warm canceller left more of
/// a ring's first second than one started with the ring, so it is not used otherwise).
pub static AEC_WARM: AtomicBool = AtomicBool::new(false);
static FORCE_WARM: AtomicBool = AtomicBool::new(false);
/// /tmp/nspanel-aec-cold: the canceller runs only around playback, as before (for comparing measurements).
static FORCE_COLD: AtomicBool = AtomicBool::new(false);

/// Echo canceller for 10 ms frames; passes the microphones through if the
/// library cannot start, and while the speaker has been silent for 2 s (it
/// costs about a quarter of a core while it runs: 31 % of one core for the
/// app with it).
struct Aec {
    ptr: *mut AecEngine,
    near: Vec<i16>,
    last_far: Option<Instant>,
    /// Microphone and processed energy while the speaker plays (for the log).
    energy: (f64, f64),
}

impl Aec {
    fn new() -> Self {
        // Noise suppression off: the wake word frontend has its own noise
        // reduction and Home Assistant's STT gets the raw (echo-free) voice.
        // SAFETY: plain C API; the engine lives as long as the app.
        let ptr = unsafe { aec_create(-1) };
        if ptr.is_null() {
            eprintln!("nspanel-app: echo cancellation not available");
        }
        Self { ptr, near: Vec::new(), last_far: None, energy: (0.0, 0.0) }
    }

    /// Appends microphone samples and returns the processed ones (whole 10 ms frames).
    fn process(&mut self, mic: &[i16], echo: &EchoRef, out: &mut Vec<i16>) {
        out.clear();
        if self.ptr.is_null() {
            out.extend_from_slice(mic);
            return;
        }
        self.near.extend_from_slice(mic);
        let mut far = [0i16; AEC_FRAME];
        let mut frame = [0i16; AEC_FRAME];
        let whole = self.near.len() / AEC_FRAME * AEC_FRAME;
        for start in (0..whole).step_by(AEC_FRAME) {
            let playing = {
                let mut q = echo.lock().unwrap();
                let playing = !q.is_empty();
                for f in far.iter_mut() {
                    *f = q.pop_front().unwrap_or(0);
                }
                playing
            };
            if playing {
                self.last_far = Some(Instant::now());
            }
            let near = &self.near[start..start + AEC_FRAME];
            let warm = !FORCE_COLD.load(Ordering::Relaxed) && (AEC_WARM.load(Ordering::Relaxed) || FORCE_WARM.load(Ordering::Relaxed));
            if warm || self.last_far.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
                // SAFETY: three buffers of AEC_FRAME samples.
                unsafe { aec_process(self.ptr, far.as_ptr(), near.as_ptr(), frame.as_mut_ptr()) };
                if playing {
                    self.energy.0 += near.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
                    self.energy.1 += frame.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
                }
                out.extend_from_slice(&frame);
            } else {
                out.extend_from_slice(near);
            }
        }
        self.near.drain(..whole);
    }

    /// Microphone energy reduction while the speaker played (dB), since the last call.
    fn take_reduction(&mut self) -> Option<f64> {
        let (raw, clean) = std::mem::take(&mut self.energy);
        (raw > 0.0 && clean > 0.0).then(|| 10.0 * (raw / clean).log10())
    }
}

// SAFETY: the echo canceller is only used from the voice thread.
unsafe impl Send for Aec {}

/// A follow-up window in which nobody starts to speak ends after this time
/// (Home Assistant itself waits 15 s for speech, so the panel used to show
/// listening for 18 s).
const FOLLOW_UP_SILENCE: Duration = Duration::from_secs(5);
/// Speech start on the panel: 10 ms frames this much above the room's level...
const SPEECH_DB: f64 = 12.0;
/// ...for about this many frames (net) in the follow-up window.
const SPEECH_FRAMES: u32 = 15;

/// Speech start detector for the follow-up window. Home Assistant reports
/// its own speech start (event 11); this one is the panel's, so a reply that
/// starts just before the 5 s mark is not cut off while Home Assistant is
/// still processing the audio. The room's level is followed while the panel
/// is idle and silent (falls fast, rises slowly).
struct SpeechStart {
    floor: f64,
    score: u32,
    peak: f64,
}

impl SpeechStart {
    fn new() -> Self {
        Self { floor: 45.0, score: 0, peak: f64::MIN }
    }

    fn level(frame: &[i16]) -> f64 {
        let ms = frame.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / frame.len().max(1) as f64;
        10.0 * (ms + 1.0).log10()
    }

    fn track(&mut self, samples: &[i16]) {
        for f in samples.chunks(AEC_FRAME) {
            let db = Self::level(f);
            let k = if db < self.floor { 0.05 } else { 0.002 };
            self.floor += (db - self.floor) * k;
        }
    }

    fn begin(&mut self) {
        self.score = 0;
        self.peak = f64::MIN;
    }

    /// True once speech has started in the window.
    fn speech(&mut self, samples: &[i16]) -> bool {
        for f in samples.chunks(AEC_FRAME) {
            let over = Self::level(f) - self.floor;
            self.peak = self.peak.max(over);
            if over > SPEECH_DB {
                self.score += 1;
            } else {
                self.score = self.score.saturating_sub(1);
            }
            if self.score >= SPEECH_FRAMES {
                return true;
            }
        }
        false
    }
}

impl Engine {
    fn new(words: &[WakeWord]) -> Option<Self> {
        // SAFETY: plain C API; the engine lives as long as this struct.
        let ptr = unsafe { mww_create() };
        if ptr.is_null() {
            return None;
        }
        let mut loaded = Vec::new();
        for w in words {
            let path = CString::new(w.path.clone()).ok()?;
            // SAFETY: valid engine and C string.
            let idx = unsafe { mww_add_model(ptr, path.as_ptr(), w.cutoff, w.window) };
            if idx >= 0 {
                loaded.push(w.clone());
            } else {
                eprintln!("nspanel-app: cannot load wake word model {}", w.path);
            }
        }
        Some(Self { ptr, words: loaded, last: None })
    }

    fn set_active(&mut self, active: &[String], stop_word: bool) {
        for (i, w) in self.words.iter().enumerate() {
            let on = if INTERNAL_WORDS.contains(&w.id.as_str()) { stop_word } else { active.contains(&w.id) };
            // SAFETY: index within the loaded models.
            unsafe { mww_set_enabled(self.ptr, i as c_int, on as c_int) };
        }
    }

    fn reset(&mut self) {
        // SAFETY: valid engine.
        unsafe { mww_reset(self.ptr) };
    }

    fn process(&mut self, samples: &[i16]) -> Option<&WakeWord> {
        // SAFETY: valid engine, slice pointer and length.
        let idx = unsafe { mww_process(self.ptr, samples.as_ptr(), samples.len()) };
        self.last = (idx >= 0).then_some(idx as usize);
        self.last.and_then(|i| self.words.get(i))
    }

    /// The model found by the last `process` call.
    fn last_detection(&self) -> Option<&WakeWord> {
        self.last.and_then(|i| self.words.get(i))
    }

    /// Window-averaged probability (0..1) of the last detection.
    fn last_score(&self) -> f32 {
        // SAFETY: valid engine.
        unsafe { mww_last_score(self.ptr) }
    }
}

/// What the wake word model heard around a detection (the processed
/// microphone signal), sent with every wake word before the run starts, so
/// that Home Assistant can let the satellite that heard the command best
/// answer when several wake at once:
/// - level: RMS of the last second, dBFS;
/// - SNR: loudest 100 ms of the last second over the RMS of the second
///   before it, dB (the same definition on every satellite).
struct WakeLevel {
    window: VecDeque<i16>,
}

impl WakeLevel {
    const SECOND: usize = 16000; // 1 s at 16 kHz
    const SAMPLES: usize = 2 * Self::SECOND; // the last second and the one before
    const PEAK: usize = 1600; // 100 ms
    const STEP: usize = 160; // the 100 ms window moves in 10 ms steps
    /// Background shorter than this (right after a run): no SNR.
    const MIN_BACKGROUND: usize = 3200;

    fn new() -> Self {
        Self { window: VecDeque::with_capacity(Self::SAMPLES) }
    }

    fn push(&mut self, samples: &[i16]) {
        for &s in samples {
            if self.window.len() == Self::SAMPLES {
                self.window.pop_front();
            }
            self.window.push_back(s);
        }
    }

    fn mean_square<'a>(samples: impl Iterator<Item = &'a i16>) -> f64 {
        let (mut sum, mut n) = (0.0f64, 0usize);
        for &s in samples {
            sum += (s as f64).powi(2);
            n += 1;
        }
        sum / n.max(1) as f64
    }

    /// RMS of the last second, dBFS.
    fn dbfs(&self) -> f32 {
        let skip = self.window.len().saturating_sub(Self::SECOND);
        let ms = Self::mean_square(self.window.iter().skip(skip));
        (10.0 * (ms.max(1.0) / (32768.0f64 * 32768.0)).log10()) as f32
    }

    /// Loudest 100 ms of the last second over the second before it, dB.
    fn snr_db(&self) -> Option<f32> {
        let len = self.window.len();
        let start = len.saturating_sub(Self::SECOND);
        if start < Self::MIN_BACKGROUND {
            return None;
        }
        let background = Self::mean_square(self.window.iter().take(start)).max(1.0);
        // energy per 10 ms step of the last second, then the loudest 100 ms run
        let steps: Vec<f64> = (start..len)
            .step_by(Self::STEP)
            .map(|i| self.window.range(i..(i + Self::STEP).min(len)).map(|&s| (s as f64).powi(2)).sum())
            .collect();
        let per = Self::PEAK / Self::STEP;
        let loudest = steps
            .windows(per.min(steps.len()).max(1))
            .map(|w| w.iter().sum::<f64>())
            .fold(0.0f64, f64::max)
            / Self::PEAK as f64;
        Some((10.0 * (loudest.max(1.0) / background).log10()) as f32)
    }
}

// SAFETY: the engine is only used from the voice thread.
unsafe impl Send for Engine {}

fn open_capture() -> alsa::Result<PCM> {
    let pcm = PCM::new(CAPTURE_DEVICE, Direction::Capture, false)?;
    {
        let hw = HwParams::any(&pcm)?;
        hw.set_channels(MIC_CHANNELS as u32)?;
        hw.set_rate(16000, ValueOr::Nearest)?;
        hw.set_format(Format::s16())?;
        hw.set_access(Access::RWInterleaved)?;
        hw.set_period_size_near(CHUNK as i64, ValueOr::Nearest)?;
        hw.set_buffer_size_near((CHUNK * 8) as i64)?;
        pcm.hw_params(&hw)?;
    }
    pcm.start()?;
    Ok(pcm)
}

/// Speaker output: 48 kHz stereo; 16 kHz mono input is upsampled ×3.
pub enum Sound {
    Pcm16k(Vec<u8>),
    /// Any 16-bit PCM (from a WAV file), resampled to 48 kHz.
    Pcm { rate: u32, channels: u16, data: Vec<u8> },
    Chime,
    Beep,
    /// 48 kHz mono samples, played over and over until `stop()` (ringing timers, alarms, critical alerts).
    Loop(Arc<Vec<i16>>),
    /// 48 kHz mono pieces as they arrive (the media player's stream); it ends with the stream or a stop.
    Stream(Receiver<Vec<i16>>),
    /// No sound: wakes the speaker for the Sendspin music, which the player mixes under every sound.
    Wake,
}

/// Speech over a ringing timer: this many dB over what is left of the ring after the echo canceller...
const RING_SPEECH_DB: f64 = 8.0;
/// ...for this many 10 ms frames (net) starts a piece of speech...
const RING_SPEECH_FRAMES: u32 = 6;
/// ...which ends after this many frames under RING_SPEECH_END dB over the ring, at most RING_SPEECH_MAX frames
/// long, with RING_PREROLL samples from before its start (the first sound of the word).
const RING_SPEECH_END: f64 = 6.0;
const RING_SPEECH_TAIL: u32 = 60;
const RING_SPEECH_MAX: u32 = 250;
/// A piece cut at its longest leaves this many samples for the start of the next one.
const RING_OVERLAP: usize = 16000;
const RING_PREROLL: usize = 8000;
/// At a ring's start the echo canceller is still learning the new sound for about a second, and its leftover
/// sounds like speech to the ear and the "Stop" model (measured: a false piece of speech 1 s after every ring
/// start, once a false "Stop"). Nothing is left unheard because of it: every piece and every "Stop" goes to the
/// speech recognition, and only a stop word stops the ring; this much audio before a "Stop" goes with it.
const RING_STOP_AUDIO: usize = 24000;

/// While a timer rings: cuts out what someone says over the ring (the processed microphone signal; the ring's
/// leftover level is the 40th percentile of the last 2 s), for the speech recognition. The ring itself plays on
/// unchanged.
struct RingEar {
    levels: std::collections::VecDeque<f64>,
    floor: f64,
    score: u32,
    pre: std::collections::VecDeque<i16>,
    // the piece being cut: samples, frames so far, frames since the last loud one
    rec: Option<(Vec<i16>, u32, u32)>,
    pieces: u32,
    peak: f64,
    reported: Instant,
}

impl RingEar {
    fn new() -> Self {
        Self {
            levels: std::collections::VecDeque::new(),
            floor: 0.0,
            score: 0,
            pre: std::collections::VecDeque::new(),
            rec: None,
            pieces: 0,
            peak: f64::MIN,
            reported: Instant::now(),
        }
    }

    /// A finished piece of speech, when one ends in these samples. `room` is the quiet room's level (for the log).
    fn push(&mut self, samples: &[i16], room: f64) -> Option<Vec<i16>> {
        let mut done = None;
        for f in samples.chunks(AEC_FRAME) {
            let db = SpeechStart::level(f);
            if let Some((buf, len, quiet)) = self.rec.as_mut() {
                buf.extend_from_slice(f);
                *len += 1;
                *quiet = if db - self.floor > RING_SPEECH_END { 0 } else { *quiet + 1 };
                if *quiet >= RING_SPEECH_TAIL {
                    done = self.rec.take().map(|(b, _, _)| b);
                } else if *len >= RING_SPEECH_MAX {
                    // still loud: the next piece goes on from the last second of this one
                    let b = self.rec.take().map(|(b, _, _)| b).unwrap_or_default();
                    let keep = b[b.len().saturating_sub(RING_OVERLAP)..].to_vec();
                    let frames = (keep.len() / AEC_FRAME) as u32;
                    self.rec = Some((keep, frames, 0));
                    done = Some(b);
                }
                continue;
            }
            self.pre.extend(f.iter().copied());
            while self.pre.len() > RING_PREROLL {
                self.pre.pop_front();
            }
            self.levels.push_back(db);
            if self.levels.len() > 200 {
                self.levels.pop_front();
            }
            if self.levels.len() % 10 == 0 || self.levels.len() == 200 {
                let mut sorted: Vec<f64> = self.levels.iter().copied().collect();
                sorted.sort_by(|a, b| a.total_cmp(b));
                self.floor = sorted[sorted.len() * 2 / 5];
            }
            // until it knows the ring's leftover, the quiet room's level
            let floor = if self.levels.len() < 50 { self.floor.max(room) } else { self.floor };
            let over = db - floor;
            self.peak = self.peak.max(over);
            if over > RING_SPEECH_DB {
                self.score += 1;
            } else {
                self.score = self.score.saturating_sub(1);
            }
            if self.score >= RING_SPEECH_FRAMES {
                self.score = 0;
                self.pieces += 1;
                self.rec = Some((self.pre.drain(..).collect(), 0, 0));
                eprintln!("nspanel-app: voice: speech over the ring ({over:+.1} dB over its echo, {floor:.0} dB)");
            }
        }
        if self.reported.elapsed() > Duration::from_secs(5) && self.peak > f64::MIN {
            self.reported = Instant::now();
            eprintln!(
                "nspanel-app: voice: ring echo {:.0} dB (room {room:.0} dB), loudest {:+.1} dB over it, {} pieces of speech",
                self.floor, self.peak, self.pieces
            );
            self.peak = f64::MIN;
        }
        done
    }
}

/// A piece of speech heard while a timer rings: to Home Assistant's speech recognition (one at a time), the text
/// back to the voice loop.
fn hear_over_ring(samples: Vec<i16>, tx: &mpsc::Sender<String>) {
    static BUSY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if BUSY.fetch_add(1, Ordering::SeqCst) >= 3 {
        BUSY.fetch_sub(1, Ordering::SeqCst);
        eprintln!("nspanel-app: voice: speech over the ring skipped (three pieces are being recognized)");
        return;
    }
    let tx = tx.clone();
    std::thread::spawn(move || {
        let t0 = Instant::now();
        dump_ring_piece(&samples);
        match crate::stt::recognize(&samples) {
            Ok(text) => {
                eprintln!(
                    "nspanel-app: voice: heard over the ring: '{text}' ({:.1} s of speech, {} ms)",
                    samples.len() as f32 / 16000.0,
                    t0.elapsed().as_millis()
                );
                let _ = tx.send(text);
            }
            Err(e) => eprintln!("nspanel-app: voice: speech over the ring not recognized: {e}"),
        }
        BUSY.fetch_sub(1, Ordering::SeqCst);
    });
}

/// Development: when the folder /tmp/nspanel-ring-dump exists, every piece sent to the speech recognition over a ring
/// is also written there (16 kHz mono WAV, the last 20 kept), to compare recognition settings afterwards.
fn dump_ring_piece(samples: &[i16]) {
    let dir = std::path::Path::new("/tmp/nspanel-ring-dump");
    if !dir.is_dir() {
        return;
    }
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let _ = std::fs::remove_file(dir.join(format!("piece-{:03}.wav", (n + 1000 - 20) % 1000)));
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16000u32.to_le_bytes());
    wav.extend_from_slice(&32000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    let _ = std::fs::write(dir.join(format!("piece-{:03}.wav", n % 1000)), wav);
}

/// Longest sound the panel plays from a file (an answer or announcement).
const MAX_AUDIO_SECS: usize = 180;

/// Sample rate and channels the panel plays, and the most data for them.
fn playable(rate: u32, channels: u16) -> Option<usize> {
    ((8000..=48000).contains(&rate) && (1..=2).contains(&channels))
        .then(|| rate as usize * channels as usize * 2 * MAX_AUDIO_SECS)
}

/// 16-bit PCM WAV → (rate, channels, samples bytes).
pub fn parse_wav(bytes: &[u8]) -> Option<(u32, u16, Vec<u8>)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12;
    let (mut rate, mut channels, mut bits) = (0u32, 0u16, 0u16);
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
            rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().ok()?);
            bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
        } else if id == b"data" {
            let end = if len == 0 || len == u32::MAX as usize { bytes.len() } else { body.saturating_add(len).min(bytes.len()) };
            if bits != 16 {
                return None;
            }
            let max = playable(rate, channels)?;
            let end = end.min(body + max);
            return Some((rate, channels, bytes[body.min(end)..end].to_vec()));
        }
        pos = body.saturating_add(len).saturating_add(len & 1);
    }
    None
}

/// MP3 → (rate, channels, 16-bit samples as bytes).
fn decode_mp3(bytes: &[u8]) -> Option<(u32, u16, Vec<u8>)> {
    let mut dec = minimp3::Decoder::new(bytes);
    let (mut rate, mut channels, mut out, mut max) = (0u32, 0u16, Vec::new(), 0usize);
    loop {
        match dec.next_frame() {
            Ok(f) => {
                if rate == 0 {
                    rate = f.sample_rate as u32;
                    channels = f.channels as u16;
                    max = playable(rate, channels)?;
                }
                if f.sample_rate as u32 == rate && f.channels as u16 == channels {
                    out.extend(f.data.iter().flat_map(|s| s.to_le_bytes()));
                }
                if out.len() >= max {
                    out.truncate(max);
                    break;
                }
            }
            Err(minimp3::Error::Eof) => break,
            Err(minimp3::Error::SkippedData) => continue,
            Err(_) => break,
        }
    }
    (rate > 0 && channels > 0 && !out.is_empty()).then_some((rate, channels, out))
}

pub fn resample_to_48k(rate: u32, channels: u16, data: &[u8]) -> Vec<i16> {
    let frames: Vec<i16> = data
        .chunks_exact(2 * channels as usize)
        .map(|f| {
            let sum: i32 = f.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]) as i32).sum();
            (sum / channels as i32) as i16
        })
        .collect();
    if frames.is_empty() {
        return Vec::new();
    }
    let out_len = (frames.len() as u64 * 48000 / rate as u64) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * rate as f64 / 48000.0;
            let k = pos as usize;
            let frac = pos - k as f64;
            let a = frames[k.min(frames.len() - 1)] as f64;
            let b = frames[(k + 1).min(frames.len() - 1)] as f64;
            (a + (b - a) * frac) as i16
        })
        .collect()
}

pub struct Player {
    tx: Sender<(Sound, u64)>,
    pub busy: Arc<AtomicBool>,
    /// Raised to drop what is playing and queued (wake word during an answer);
    /// sounds queued before the stop carry an older number.
    stop_gen: Arc<std::sync::atomic::AtomicU64>,
    pub echo: EchoRef,
    /// Times the speaker ran dry (ALSA underrun) while playing: audible gaps.
    pub underruns: Arc<AtomicU32>,
}

fn chime() -> Vec<i16> {
    // Two short rising tones, soft attack and release.
    let mut out = Vec::new();
    for (freq, ms) in [(880.0f32, 90u32), (1320.0, 140)] {
        let n = 48 * ms as usize;
        for i in 0..n {
            let t = i as f32 / 48000.0;
            let env = (i as f32 / 480.0).min(1.0) * ((n - i) as f32 / 1440.0).min(1.0);
            out.push((env * 0.25 * (2.0 * std::f32::consts::PI * freq * t).sin() * 32767.0) as i16);
        }
    }
    out
}

pub fn beep() -> Vec<i16> {
    let mut out = Vec::new();
    for i in 0..48 * 400 {
        let on = (i / (48 * 100)) % 2 == 0;
        let t = i as f32 / 48000.0;
        let v = if on { 0.3 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin() } else { 0.0 };
        out.push((v * 32767.0) as i16);
    }
    out
}

fn upsample(pcm16k: &[u8]) -> Vec<i16> {
    let samples: Vec<i16> = pcm16k.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    let mut out = Vec::with_capacity(samples.len() * 3);
    for i in 0..samples.len() {
        let a = samples[i] as i32;
        let b = *samples.get(i + 1).unwrap_or(&samples[i]) as i32;
        out.push(a as i16);
        out.push(((2 * a + b) / 3) as i16);
        out.push(((a + 2 * b) / 3) as i16);
    }
    out
}

/// A sound plays (or is still in the speaker): the Sendspin music stays on under an answer.
static SPEAKING: AtomicBool = AtomicBool::new(false);

pub fn speaking() -> bool {
    SPEAKING.load(Ordering::Relaxed)
}

/// One sound being played (with its place), the media stream's pieces as they come.
struct Playing {
    samples: Arc<Vec<i16>>,
    offset: usize,
    looped: bool,
    stream: Option<Receiver<Vec<i16>>>,
    /// A stream waits for half a second of audio first, so short stalls of the network do not empty the speaker.
    prebuffer: bool,
    gen: u64,
}

fn open_speaker() -> Option<PCM> {
    (|| -> alsa::Result<PCM> {
        let p = PCM::new(PLAYBACK_DEVICE, Direction::Playback, false)?;
        {
            let hw = HwParams::any(&p)?;
            hw.set_channels(2)?;
            hw.set_rate(48000, ValueOr::Nearest)?;
            hw.set_format(Format::s16())?;
            hw.set_access(Access::RWInterleaved)?;
            // 100 ms: the echo reference runs this much ahead of the speaker.
            hw.set_buffer_size_near(48000 / 10)?;
            p.hw_params(&hw)?;
        }
        Ok(p)
    })()
    .map_err(|e| eprintln!("nspanel-app: speaker: {e}"))
    .ok()
}

/// The speaker: the queued sounds one after the other, the Sendspin music mixed under them (and alone while no
/// sound plays), 10 ms at a time. Each written piece also goes to the echo reference (48 kHz → 16 kHz, mean of
/// three samples), so the echo canceller hears the music too.
fn player_thread(
    rx: Receiver<(Sound, u64)>,
    busy: Arc<AtomicBool>,
    stop_gen: Arc<std::sync::atomic::AtomicU64>,
    echo: EchoRef,
    underruns: Arc<AtomicU32>,
) {
    const PERIOD: usize = 480; // 10 ms of frames
    let mut pcm: Option<PCM> = None;
    let mut playing: Option<Playing> = None;
    // the sound has been written but may still be in the speaker's buffer: busy until then
    let mut busy_until: Option<Instant> = None;
    let mut queued: VecDeque<(Sound, u64)> = VecDeque::new();
    loop {
        let music = crate::sendspin::music_active();
        // the next sound: waits for one while nothing plays (and closes the speaker after 400 ms of silence)
        if playing.is_none() {
            let next = if let Some(q) = queued.pop_front() {
                Some(q)
            } else if music || busy_until.is_some() {
                rx.try_recv().ok()
            } else {
                match rx.recv_timeout(Duration::from_millis(400)) {
                    Ok(s) => Some(s),
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(p) = pcm.take() {
                            let _ = p.drain();
                        }
                        busy.store(false, Ordering::SeqCst);
                        SPEAKING.store(false, Ordering::SeqCst);
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };
            if let Some((sound, gen)) = next {
                if gen >= stop_gen.load(Ordering::SeqCst) {
                    let (samples, looped, stream): (Arc<Vec<i16>>, bool, Option<Receiver<Vec<i16>>>) = match sound {
                        Sound::Pcm16k(b) => (Arc::new(upsample(&b)), false, None),
                        Sound::Pcm { rate, channels, data } => (Arc::new(resample_to_48k(rate, channels, &data)), false, None),
                        Sound::Chime => (Arc::new(chime()), false, None),
                        Sound::Beep => (Arc::new(beep()), false, None),
                        Sound::Loop(samples) => (samples, true, None),
                        Sound::Stream(rx) => (Arc::new(Vec::new()), false, Some(rx)),
                        Sound::Wake => (Arc::new(Vec::new()), false, None),
                    };
                    if !samples.is_empty() || stream.is_some() {
                        busy.store(true, Ordering::SeqCst);
                        SPEAKING.store(true, Ordering::SeqCst);
                        busy_until = None;
                        let prebuffer = stream.is_some();
                        playing = Some(Playing { samples, offset: 0, looped, stream, prebuffer, gen });
                    } else if busy_until.is_none() {
                        // nothing to play (the speaker only wakes up): `play` raised busy, it ends now
                        busy_until = Some(Instant::now());
                    }
                } else if busy_until.is_none() {
                    busy_until = Some(Instant::now());
                }
            }
        }
        // the played-out sound's busy time ends, also while the music keeps the speaker running (it stayed
        // busy for good under music: the music was never muted for the listening, the wake word ran as
        // "during playback")
        if playing.is_none() && busy_until.is_some_and(|until| Instant::now() >= until) {
            busy_until = None;
            busy.store(false, Ordering::SeqCst);
            SPEAKING.store(false, Ordering::SeqCst);
        }
        if playing.is_none() && !music {
            if busy_until.is_some() {
                std::thread::sleep(Duration::from_millis(5));
            }
            continue;
        }
        if pcm.is_none() {
            pcm = open_speaker();
            if pcm.is_none() {
                playing = None;
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        }
        let Some(p) = pcm.as_ref() else { continue };
        let Ok(io) = p.io_i16() else {
            pcm = None;
            continue;
        };

        // a stop drops what plays and what the speaker still holds
        if playing.as_ref().is_some_and(|pl| pl.gen < stop_gen.load(Ordering::SeqCst)) {
            playing = None;
            queued.retain(|(_, g)| *g >= stop_gen.load(Ordering::SeqCst));
            let _ = p.drop();
            let _ = p.prepare();
            echo.lock().unwrap().clear();
            busy_until = Some(Instant::now());
            continue;
        }

        // the sound's part of this period
        let mut sound: Vec<i16> = Vec::with_capacity(PERIOD);
        let mut finished = false;
        if let Some(pl) = playing.as_mut() {
            if let Some(srx) = &pl.stream {
                // a stream: take what has arrived; the first piece waits for half a second of audio
                let mut ended = false;
                loop {
                    match srx.try_recv() {
                        Ok(piece) => {
                            let mut v = Arc::try_unwrap(std::mem::take(&mut pl.samples)).unwrap_or_else(|a| (*a).clone());
                            v.drain(..pl.offset.min(v.len()));
                            pl.offset = 0;
                            v.extend(piece);
                            pl.samples = Arc::new(v);
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            ended = true;
                            break;
                        }
                    }
                }
                let have = pl.samples.len() - pl.offset;
                if pl.prebuffer && have >= 24000 {
                    pl.prebuffer = false;
                }
                if ended && have == 0 {
                    finished = true;
                } else if pl.prebuffer && !ended {
                    if !music {
                        // nothing else to play: wait for the stream instead of writing silence
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                } else if have == 0 && !music {
                    // a stall of the network: wait (the speaker drains, as before)
                    match srx.recv_timeout(Duration::from_millis(30)) {
                        Ok(piece) => {
                            pl.samples = Arc::new(piece);
                            pl.offset = 0;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => finished = true,
                    }
                    if !finished {
                        continue;
                    }
                }
                if !finished && !pl.prebuffer {
                    let take = PERIOD.min(pl.samples.len() - pl.offset);
                    sound.extend_from_slice(&pl.samples[pl.offset..pl.offset + take]);
                    pl.offset += take;
                }
            } else {
                while sound.len() < PERIOD {
                    if pl.offset >= pl.samples.len() {
                        if pl.looped {
                            pl.offset = 0;
                        } else {
                            finished = true;
                            break;
                        }
                    }
                    let take = (PERIOD - sound.len()).min(pl.samples.len() - pl.offset);
                    sound.extend_from_slice(&pl.samples[pl.offset..pl.offset + take]);
                    pl.offset += take;
                }
            }
        }
        if finished {
            playing = None;
            // the next queued sound follows at once; the busy flag waits until the speaker has played it out
            match rx.try_recv() {
                Ok(next) => queued.push_back(next),
                Err(_) => {
                    let held = p.delay().unwrap_or(0).max(0) as u64;
                    busy_until = Some(Instant::now() + Duration::from_micros(held * 1_000_000 / 48000 + 20_000));
                }
            }
            if sound.is_empty() {
                continue;
            }
        }

        // the music's part, timed for when this period will be heard
        let frames = if sound.is_empty() { PERIOD } else { sound.len() };
        let held = p.delay().unwrap_or(0).max(0) as i64;
        let heard_at = crate::sendspin::now_us() + held * 1_000_000 / 48000;
        let mut mono = sound;
        mono.resize(frames, 0);
        if music {
            crate::sendspin::mix_music(&mut mono, heard_at);
        }
        let stereo: Vec<i16> = mono.iter().flat_map(|s| [*s, *s]).collect();
        match io.writei(&stereo) {
            Ok(written) => {
                let mut q = echo.lock().unwrap();
                let start = q.len();
                for tri in mono[..written.min(mono.len())].chunks(3) {
                    q.push_back((tri.iter().map(|v| *v as i32).sum::<i32>() / tri.len() as i32) as i16);
                }
                if METER_ON.load(Ordering::Relaxed) {
                    keep_trace(&OUT_TRACE, q.iter().skip(start).copied());
                }
                while q.len() > 16000 {
                    q.pop_front();
                }
            }
            Err(_) => {
                underruns.fetch_add(1, Ordering::SeqCst);
                let _ = p.prepare();
            }
        }
    }
}

impl Player {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<(Sound, u64)>();
        let busy = Arc::new(AtomicBool::new(false));
        let busy2 = busy.clone();
        let stop_gen = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let stop2 = stop_gen.clone();
        let echo: EchoRef = Arc::new(Mutex::new(VecDeque::new()));
        let echo2 = echo.clone();
        let underruns = Arc::new(AtomicU32::new(0));
        let underruns2 = underruns.clone();
        std::thread::spawn(move || player_thread(rx, busy2, stop2, echo2, underruns2));
        Self { tx, busy, stop_gen, echo, underruns }
    }

    pub fn play(&self, s: Sound) {
        self.busy.store(true, Ordering::SeqCst);
        let _ = self.tx.send((s, self.stop_gen.load(Ordering::SeqCst)));
    }

    /// Drops what is playing and what is queued (later sounds play normally).
    pub fn stop(&self) {
        self.stop_gen.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(PartialEq, Debug)]
enum State {
    Idle,
    Starting,
    Streaming,
    Waiting,
}

pub struct VoiceControl {
    pub muted: Arc<AtomicBool>,
    /// A finished timer rings: listen without a wake word (as the ESPHome
    /// satellites do); a stop word or a short stop phrase stops it.
    pub ringing: Arc<AtomicBool>,
    pub active_wake_words: Arc<Mutex<Vec<String>>>,
    /// Home Assistant base URL, for relative announcement media URLs.
    pub ha_url: String,
}

/// Host part of an http(s) URL (without user info and port).
fn url_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    Some(if let Some(v6) = host_port.strip_prefix('[') { v6.split(']').next()? } else { host_port.split(':').next()? })
}

/// Sound is only fetched from Home Assistant's host (any port: e.g. Music
/// Assistant next to it), by name or by the same address.
fn from_home_assistant(url: &str, ha_url: &str) -> bool {
    use std::net::ToSocketAddrs;
    let (Some(host), Some(ha_host)) = (url_host(url), url_host(ha_url)) else { return false };
    if host.eq_ignore_ascii_case(ha_host) {
        return true;
    }
    let ips = |h: &str| (h, 80).to_socket_addrs().map(|a| a.map(|s| s.ip()).collect::<Vec<_>>()).unwrap_or_default();
    let ha_ips = ips(ha_host);
    ips(host).iter().any(|ip| ha_ips.contains(ip))
}

/// Downloads a WAV or MP3 file from Home Assistant and plays it; returns once
/// it has been played.
fn fetch_and_play(url: &str, ha_url: &str, player: &Player) -> Result<(), String> {
    if !from_home_assistant(url, ha_url) {
        return Err("not on Home Assistant's host".into());
    }
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(20)).build();
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(resp.into_reader(), 16 * 1024 * 1024), &mut bytes)
        .map_err(|e| e.to_string())?;
    // Answers come as WAV; announcements from Home Assistant as MP3.
    let (rate, channels, data) = parse_wav(&bytes).or_else(|| decode_mp3(&bytes)).ok_or("neither a 16-bit WAV nor an MP3 file")?;
    player.play(Sound::Pcm { rate, channels, data });
    std::thread::sleep(Duration::from_millis(300));
    while player.busy.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

/// One announcement download at a time.
static ANNOUNCING: AtomicBool = AtomicBool::new(false);

/// A voice run is going on, or an answer is awaited or being fetched.
static RUN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// The voice satellite is busy: a run, an awaited or fetched answer, or an
/// announcement. Sound that is already playing is `Player::busy`.
pub fn busy() -> bool {
    RUN_ACTIVE.load(Ordering::Relaxed) || ANNOUNCING.load(Ordering::Relaxed)
}

/// Downloads and plays an announcement; reports the result back to Home Assistant.
fn announce(media_id: String, ha_url: String, player: Arc<Player>, to_api: Sender<ToApi>) {
    if ANNOUNCING.swap(true, Ordering::SeqCst) {
        eprintln!("nspanel-app: announcement refused (another one is playing)");
        let _ = to_api.send(ToApi::AnnounceFinished { success: false });
        return;
    }
    std::thread::spawn(move || {
        let url = if media_id.starts_with("http") { media_id } else { format!("{ha_url}{media_id}") };
        let result = fetch_and_play(&url, &ha_url, &player);
        if let Err(e) = &result {
            eprintln!("nspanel-app: announcement {url}: {e}");
        }
        ANNOUNCING.store(false, Ordering::SeqCst);
        let _ = to_api.send(ToApi::AnnounceFinished { success: result.is_ok() });
    });
}

/// Longest a run streams the microphones to Home Assistant.
const MAX_STREAM: Duration = Duration::from_secs(30);
/// A streamed answer starts once this much audio is in (16 kHz, 16-bit:
/// 0.8 s), so a short stall in the stream does not cut the speech: Home
/// Assistant sends it about as fast as it is spoken, with waits of up to
/// about 0.7 s between chunks.
const TTS_PREBUFFER_BYTES: usize = 16000 * 2 * 8 / 10;
/// ...or after this long, whatever has arrived.
const TTS_PREBUFFER_WAIT: Duration = Duration::from_millis(1500);

/// Runs the voice satellite thread.
pub fn spawn(
    to_api: Sender<ToApi>,
    from_api: Receiver<FromApi>,
    player: Arc<Player>,
    control: VoiceControl,
    ui: impl Fn(VoiceUi) + Send + 'static,
) {
    std::thread::spawn(move || {
        let words = available_wake_words();
        let Some(mut engine) = Engine::new(&words) else {
            eprintln!("nspanel-app: wake word engine not available");
            return;
        };
        engine.set_active(&control.active_wake_words.lock().unwrap(), false);
        let mut stop_word = false;
        let mut last_speaking: Option<Instant> = None;
        // A timer rings: the panel listens for "Stop" (and the wake words) itself, and holds the ring while
        // someone speaks.
        let mut ring_listen = false;
        let mut ring_audio: std::collections::VecDeque<i16> = std::collections::VecDeque::new();
        // a ring's start: (raw, processed) energy per 100 ms for its first 3 s, for the log
        let mut ring_erle: Vec<(f64, f64)> = Vec::new();
        let mut erle_acc = (0f64, 0f64, 0usize);
        let mut warm_check = Instant::now();
        let mut ring_ear = RingEar::new();
        // texts of speech heard over a ring (from the speech recognition threads)
        let (ring_tx, ring_rx) = mpsc::channel::<String>();
        eprintln!(
            "nspanel-app: wake words loaded: {:?}, active {:?}",
            engine.words.iter().map(|w| &w.phrase).collect::<Vec<_>>(),
            control.active_wake_words.lock().unwrap()
        );
        let mut pcm = loop {
            match open_capture() {
                Ok(p) => break p,
                Err(e) => {
                    eprintln!("nspanel-app: microphones: {e}");
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
        };
        let mut buf = vec![0i16; CHUNK * MIC_CHANNELS];
        let mut mono = vec![0i16; CHUNK];
        let mut aec = Aec::new();
        let mut clean: Vec<i16> = Vec::with_capacity(CHUNK + AEC_FRAME);
        let mut was_speaking = false;
        // Development: NSPANEL_AEC_DUMP=<file> writes microphone and processed
        // samples (interleaved, 16 kHz, s16le) for checking the echo canceller.
        let mut aec_dump = std::env::var("NSPANEL_AEC_DUMP").ok().and_then(|p| std::fs::File::create(p).ok());
        let mut state = State::Idle;
        let mut since = Instant::now();
        let mut conversation_id = String::new();
        let mut continue_conversation = false;
        let mut follow_up_pending = false;
        // Home Assistant streams the spoken answer in the background and may
        // end the run before the audio starts: wait for the stream end.
        let mut expecting_tts: Option<Instant> = None;
        // Development: /tmp/nspanel-voice-test.wav (16 kHz mono 16-bit) is sent
        // as if spoken after the first wake word, instead of the microphones.
        let mut test_audio: Option<(Vec<i16>, usize)> = None;
        // When Home Assistant cannot stream the answer over the API (it only
        // streams 16 kHz WAV), the panel fetches the TTS URL itself.
        let mut tts_url: Option<String> = None;
        let mut tts_chunks = 0usize;
        // Streamed answer: when chunks arrive, how much audio, the longest
        // wait between two chunks (for finding gaps in the playback).
        let mut tts_first: Option<Instant> = None;
        let mut tts_last: Option<Instant> = None;
        let mut tts_bytes = 0usize;
        let mut tts_max_gap = Duration::ZERO;
        let mut underruns_before = 0u32;
        // Streamed answer audio held back until TTS_PREBUFFER_BYTES are in.
        let mut tts_hold: Option<(Vec<u8>, Instant)> = None;
        // The run ended while the answer is still spoken: back to the idle
        // screen when the speech ends, not before.
        let mut idle_after_speech = false;
        let fetching = Arc::new(AtomicBool::new(false));
        // Development: /tmp/nspanel-voice-followup.wav is sent as the speech of
        // the next follow-up run (to reproduce conversations without a person).
        let mut test_follow_up: Option<Vec<i16>> = None;
        // The current run was started by a ringing timer (no wake word).
        // The current run is a follow-up (no wake word): it ends early when
        // nobody starts to speak (FOLLOW_UP_SILENCE).
        let mut follow_up_run = false;
        let mut speech_seen = false;
        let mut stream_start = Instant::now();
        let mut detector = SpeechStart::new();
        let mut wake_level = WakeLevel::new();
        // After the panel ended a silent follow-up: Home Assistant's late
        // error events are not shown.
        let mut quiet_until: Option<Instant> = None;
        loop {
            // Microphones
            let frames = {
                let io = match pcm.io_i16() {
                    Ok(io) => io,
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                };
                match io.readi(&mut buf) {
                    Ok(f) => f,
                    Err(_) => {
                        let _ = pcm.prepare();
                        let _ = pcm.start();
                        0
                    }
                }
            };
            if frames == 0 {
                if pcm.state() == alsa::pcm::State::Disconnected {
                    if let Ok(p) = open_capture() {
                        pcm = p;
                    }
                }
                continue;
            }
            for i in 0..frames {
                let a = buf[i * MIC_CHANNELS + 2] as i32;
                let b = buf[i * MIC_CHANNELS + 3] as i32;
                mono[i] = (((a + b) / 2) * MIC_GAIN).clamp(-32768, 32767) as i16;
            }
            if warm_check.elapsed() > Duration::from_secs(1) {
                warm_check = Instant::now();
                FORCE_WARM.store(std::path::Path::new("/tmp/nspanel-aec-warm").exists(), Ordering::Relaxed);
                FORCE_COLD.store(std::path::Path::new("/tmp/nspanel-aec-cold").exists(), Ordering::Relaxed);
            }
            aec.process(&mono[..frames], &player.echo, &mut clean);
            // the first 3 s of a ring: how much of it the canceller takes out, per 100 ms
            if ring_listen && ring_erle.len() < 30 {
                let raw = &mono[frames - clean.len().min(frames)..frames];
                erle_acc.0 += raw.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
                erle_acc.1 += clean.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
                erle_acc.2 += clean.len();
                if erle_acc.2 >= 1600 {
                    ring_erle.push((erle_acc.0, erle_acc.1));
                    erle_acc = (0.0, 0.0, 0);
                    if ring_erle.len() == 30 {
                        let db = |e: f64| 10.0 * (e / 1600.0 + 1.0).log10();
                        let line: Vec<String> = ring_erle.iter().map(|(r, c)| format!("{:.0}/{:.0}", db(*r), db(*c))).collect();
                        eprintln!("nspanel-app: voice: ring start, microphone/processed dB per 100 ms: {}", line.join(" "));
                    }
                }
            }
            if let Some(f) = aec_dump.as_mut() {
                use std::io::Write;
                // Raw and processed frames line up from the end (the canceller keeps < 10 ms back).
                let raw = &mono[frames - clean.len().min(frames)..frames];
                let mut bytes = Vec::with_capacity(raw.len() * 4);
                for (r, c) in raw.iter().zip(clean.iter()) {
                    bytes.extend_from_slice(&r.to_le_bytes());
                    bytes.extend_from_slice(&c.to_le_bytes());
                }
                let _ = f.write_all(&bytes);
            }
            if clean.is_empty() {
                continue;
            }
            let chunk = &clean[..];
            let muted = control.muted.load(Ordering::SeqCst);

            // Messages from Home Assistant
            while let Ok(msg) = from_api.try_recv() {
                match msg {
                    FromApi::Started { error } => {
                        if state == State::Starting {
                            if error {
                                state = State::Idle;
                                ui(VoiceUi::Error("Home Assistant is not connected".into()));
                            } else {
                                state = State::Streaming;
                                since = Instant::now();
                                stream_start = since;
                                speech_seen = false;
                                detector.begin();
                            }
                        }
                    }
                    FromApi::Event { kind, data } => {
                        let get = |k: &str| data.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
                        since = Instant::now();
                        match kind {
                            // STT VAD start
                            11 => {
                                if !speech_seen {
                                    speech_seen = true;
                                    eprintln!(
                                        "nspanel-app: voice: speech after {} ms (Home Assistant)",
                                        stream_start.elapsed().as_millis()
                                    );
                                }
                            }
                            // STT VAD end, STT end: stop sending audio
                            12 | 4 => {
                                if state == State::Streaming {
                                    state = State::Waiting;
                                }
                                test_audio = None;
                                if kind == 4 {
                                    let text = get("text").unwrap_or_default();
                                    let ringing = control.ringing.load(Ordering::SeqCst);
                                    if ringing {
                                        eprintln!("nspanel-app: voice: heard while the timer rings: '{text}'");
                                    }
                                    if ringing && crate::stt::is_only_stop(&text) {
                                        // A timer rang up during the run and the run's whole sentence is a stop
                                        // phrase: it stops the ring and the run (nothing for the assistant). Any
                                        // other sentence, however short, is a command and goes on to the assistant.
                                        eprintln!("nspanel-app: voice: '{text}' while the timer rings -> stop");
                                        control.ringing.store(false, Ordering::SeqCst);
                                        let _ = to_api.send(ToApi::Stop);
                                        state = State::Idle;
                                        quiet_until = Some(Instant::now() + Duration::from_secs(3));
                                        player.stop();
                                        ui(VoiceUi::Stopped);
                                        follow_up_pending = false;
                                    } else {
                                        if !text.is_empty() {
                                            ui(VoiceUi::Heard(text));
                                        }
                                        ui(VoiceUi::Thinking);
                                    }
                                }
                            }
                            // Intent end; ignored when no run is going on (a follow-up
                            // only ever continues a conversation of the panel's own).
                            6 if state != State::Idle => {
                                if let Some(id) = get("conversation_id") {
                                    conversation_id = id;
                                }
                                continue_conversation = get("continue_conversation").as_deref() == Some("1");
                            }
                            7 => {
                                if let Some(text) = get("text") {
                                    ui(VoiceUi::Answer(text));
                                }
                            }
                            // TTS stream start, TTS end (URL), stream end
                            98 => {
                                expecting_tts = Some(Instant::now());
                                tts_hold = Some((Vec::new(), Instant::now()));
                                tts_chunks = 0;
                                tts_first = None;
                                tts_last = None;
                                tts_bytes = 0;
                                tts_max_gap = Duration::ZERO;
                                underruns_before = player.underruns.load(Ordering::SeqCst);
                            }
                            8 => {
                                expecting_tts = Some(Instant::now());
                                tts_url = get("url");
                            }
                            99 => {
                                expecting_tts = None;
                                if let Some((held, _)) = tts_hold.take() {
                                    if !held.is_empty() {
                                        player.play(Sound::Pcm16k(held));
                                    }
                                }
                                if let (Some(first), Some(last)) = (tts_first, tts_last) {
                                    eprintln!(
                                        "nspanel-app: tts stream: {tts_chunks} chunks, {:.1} s of audio in {:.1} s, longest wait between chunks {} ms",
                                        tts_bytes as f64 / 32000.0,
                                        last.duration_since(first).as_secs_f64(),
                                        tts_max_gap.as_millis()
                                    );
                                }
                                if tts_chunks == 0 {
                                    if let Some(url) = tts_url.take() {
                                        fetching.store(true, Ordering::SeqCst);
                                        let (player, fetching, ha_url) = (player.clone(), fetching.clone(), control.ha_url.clone());
                                        std::thread::spawn(move || {
                                            if let Err(e) = fetch_and_play(&url, &ha_url, &player) {
                                                eprintln!("nspanel-app: answer audio {url}: {e}");
                                            }
                                            fetching.store(false, Ordering::SeqCst);
                                        });
                                    }
                                }
                                tts_url = None;
                            }
                            // Run end (ignored when no run is going on)
                            2 if state != State::Idle => {
                                state = State::Idle;
                                test_audio = None;
                                follow_up_run = false;
                                follow_up_pending = continue_conversation;
                                continue_conversation = false;
                                eprintln!(
                                    "nspanel-app: voice: run end, follow-up {}",
                                    if follow_up_pending { "yes (continue_conversation)" } else { "no" }
                                );
                                if !follow_up_pending {
                                    let speaking_now = player.busy.load(Ordering::SeqCst)
                                        || expecting_tts.is_some()
                                        || fetching.load(Ordering::SeqCst);
                                    if speaking_now {
                                        idle_after_speech = true;
                                    } else {
                                        ui(VoiceUi::Idle);
                                    }
                                }
                            }
                            // Error
                            0 => {
                                state = State::Idle;
                                follow_up_pending = false;
                                idle_after_speech = false;
                                follow_up_run = false;
                                let message = get("message").or_else(|| get("code")).unwrap_or_default();
                                if quiet_until.is_some_and(|t| Instant::now() < t) {
                                    eprintln!("nspanel-app: Assist error after the silent follow-up (not shown): {message}");
                                    ui(VoiceUi::Idle);
                                } else if get("code").as_deref() == Some("stt-no-text-recognized") {
                                    eprintln!("nspanel-app: Assist: nothing understood");
                                    ui(VoiceUi::NotUnderstood);
                                } else {
                                    eprintln!("nspanel-app: Assist error: {message}");
                                    ui(VoiceUi::Error(message));
                                }
                            }
                            _ => {}
                        }
                    }
                    FromApi::TtsAudio(bytes) => {
                        // More may come until 20 s after the last chunk.
                        expecting_tts = Some(Instant::now());
                        tts_chunks += 1;
                        let now = Instant::now();
                        if let Some(last) = tts_last {
                            tts_max_gap = tts_max_gap.max(now - last);
                        }
                        tts_first.get_or_insert(now);
                        tts_last = Some(now);
                        tts_bytes += bytes.len();
                        match tts_hold.as_mut() {
                            Some((held, _)) => {
                                held.extend_from_slice(&bytes);
                                if held.len() >= TTS_PREBUFFER_BYTES {
                                    let (held, _) = tts_hold.take().unwrap();
                                    player.play(Sound::Pcm16k(held));
                                }
                            }
                            None => player.play(Sound::Pcm16k(bytes)),
                        }
                    }
                    FromApi::SetWakeWords(ids) => {
                        engine.set_active(&ids, stop_word);
                        *control.active_wake_words.lock().unwrap() = ids;
                    }
                    FromApi::Announce { media_id, text } => {
                        ui(VoiceUi::Answer(text));
                        announce(media_id, control.ha_url.clone(), player.clone(), to_api.clone());
                    }
                }
            }
            if tts_hold.as_ref().is_some_and(|(held, since)| !held.is_empty() && since.elapsed() > TTS_PREBUFFER_WAIT) {
                let (held, _) = tts_hold.take().unwrap();
                player.play(Sound::Pcm16k(held));
            }
            if expecting_tts.is_some_and(|t| t.elapsed() > Duration::from_secs(20)) {
                expecting_tts = None;
                tts_hold = None;
            }

            if state == State::Idle && test_audio.is_none() {
                const TEST: &str = "/tmp/nspanel-voice-test.wav";
                if let Ok(bytes) = std::fs::read(TEST) {
                    let _ = std::fs::remove_file(TEST);
                    if let Some((16000, 1, data)) = parse_wav(&bytes) {
                        let samples: Vec<i16> = data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                        eprintln!("nspanel-app: voice test with {} samples", samples.len());
                        test_audio = Some((samples, 0));
                        if let Some(word) = engine.words.first() {
                            let phrase = engine
                                .words
                                .iter()
                                .find(|w| control.active_wake_words.lock().unwrap().first() == Some(&w.id))
                                .unwrap_or(word)
                                .phrase
                                .clone();
                            player.play(Sound::Chime);
                            let _ = to_api.send(ToApi::Start { conversation_id: String::new(), wake_word_phrase: phrase });
                            state = State::Starting;
                            since = Instant::now();
                            ui(VoiceUi::Listening);
                        }
                    }
                }
            }

            RUN_ACTIVE.store(
                state != State::Idle || expecting_tts.is_some() || fetching.load(Ordering::SeqCst),
                Ordering::Relaxed,
            );
            // A ringing timer or alarm: a wake word heard by the panel itself stops it at once; what is said over the
            // ring and every "Stop" go to the speech recognition, and a stop word stops it (no answer comes).
            if control.ringing.load(Ordering::SeqCst) && !muted && !aec.ptr.is_null() {
                if !ring_listen {
                    ring_listen = true;
                    ring_erle.clear();
                    erle_acc = (0.0, 0.0, 0);
                    ring_audio.clear();
                    ring_ear = RingEar::new();
                    eprintln!("nspanel-app: voice: listening while the timer rings");
                    stop_word = true;
                    engine.set_active(&control.active_wake_words.lock().unwrap(), true);
                    engine.reset();
                }
                ring_audio.extend(chunk.iter().copied());
                while ring_audio.len() > RING_STOP_AUDIO {
                    ring_audio.pop_front();
                }
                let heard = engine.process(chunk).cloned();
                if let Some(word) = heard.as_ref().filter(|w| INTERNAL_WORDS.contains(&w.id.as_str())) {
                    // "Stop": checked by the speech recognition (the ring's own start can sound like it)
                    eprintln!("nspanel-app: voice: '{}' heard over the ring: checking it", word.phrase);
                    hear_over_ring(ring_audio.iter().copied().collect(), &ring_tx);
                    engine.reset();
                } else if let Some(word) = heard {
                    eprintln!("nspanel-app: voice: '{}' while the timer rings -> stop", word.phrase);
                    control.ringing.store(false, Ordering::SeqCst);
                    if state != State::Idle {
                        let _ = to_api.send(ToApi::Stop);
                        state = State::Idle;
                    }
                    follow_up_pending = false;
                    player.stop();
                    engine.reset();
                    ui(VoiceUi::Stopped);
                }
                if let Some(piece) = ring_ear.push(chunk, detector.floor) {
                    hear_over_ring(piece, &ring_tx);
                }
                // Development: the follow-up test file is also speech over a ringing timer.
                if let Ok(bytes) = std::fs::read("/tmp/nspanel-voice-followup.wav") {
                    let _ = std::fs::remove_file("/tmp/nspanel-voice-followup.wav");
                    if let Some((16000, 1, data)) = parse_wav(&bytes) {
                        eprintln!("nspanel-app: voice test (over the ring) with {} bytes", data.len());
                        hear_over_ring(data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect(), &ring_tx);
                    }
                }
            } else if ring_listen {
                ring_listen = false;
                engine.reset();
            }
            // what the speech recognition heard over the ring: a stop word stops it; nothing goes to the assistant
            while let Ok(text) = ring_rx.try_recv() {
                if crate::stt::stops_ring(&text) && control.ringing.load(Ordering::SeqCst) {
                    eprintln!("nspanel-app: voice: '{text}' while the timer rings -> stop");
                    control.ringing.store(false, Ordering::SeqCst);
                    player.stop();
                    ui(VoiceUi::Stopped);
                } else if !text.is_empty() {
                    eprintln!("nspanel-app: voice: '{text}' is not a stop phrase");
                }
            }
            // the voice view's meter, only while the view shows: the microphone's level and trace
            if METER_ON.load(Ordering::Relaxed) {
                let rms = if muted { 0.0 } else { (chunk.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / chunk.len().max(1) as f64).sqrt() };
                let db = 20.0 * (rms / 32768.0).max(1e-9).log10();
                MIC_LEVEL.store((((db + 60.0) / 40.0).clamp(0.0, 1.0) as f32).to_bits(), Ordering::Relaxed);
                if !muted {
                    keep_trace(&MIC_TRACE, chunk.iter().copied());
                }
            }
            match state {
                State::Idle => {
                    let speaking =
                        player.busy.load(Ordering::SeqCst) || expecting_tts.is_some() || fetching.load(Ordering::SeqCst);
                    if was_speaking && !speaking {
                        if let Some(db) = aec.take_reduction() {
                            eprintln!("nspanel-app: aec: microphone level {db:.1} dB lower while the panel spoke");
                        }
                        let underruns = player.underruns.load(Ordering::SeqCst);
                        if underruns != underruns_before {
                            eprintln!("nspanel-app: speaker: ran dry {} times while the panel spoke", underruns - underruns_before);
                        }
                        underruns_before = underruns;
                        if idle_after_speech {
                            idle_after_speech = false;
                            ui(VoiceUi::Idle);
                        }
                    }
                    was_speaking = speaking;
                    if !speaking {
                        detector.track(chunk);
                    }
                    // "Stop" listens while the panel speaks or beeps (and 2 s after).
                    if speaking {
                        last_speaking = Some(Instant::now());
                    }
                    let want_stop = !aec.ptr.is_null()
                        && (ring_listen || last_speaking.is_some_and(|t| t.elapsed() < Duration::from_secs(2)));
                    if want_stop != stop_word {
                        stop_word = want_stop;
                        engine.set_active(&control.active_wake_words.lock().unwrap(), stop_word);
                    }
                    if control.ringing.load(Ordering::SeqCst) && !muted && !aec.ptr.is_null() {
                        // Ringing timer: the ear above listens (no run, nothing for the assistant).
                        follow_up_pending = false;
                        follow_up_run = false;
                    } else if follow_up_pending && !speaking {
                        // Follow-up: listen again without a wake word.
                        follow_up_pending = false;
                        follow_up_run = true;
                        engine.reset();
                        wake_level = WakeLevel::new();
                        eprintln!("nspanel-app: voice: follow-up listening");
                        if test_follow_up.is_none() {
                            if let Ok(bytes) = std::fs::read("/tmp/nspanel-voice-followup.wav") {
                                let _ = std::fs::remove_file("/tmp/nspanel-voice-followup.wav");
                                if let Some((16000, 1, data)) = parse_wav(&bytes) {
                                    test_follow_up = Some(
                                        data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect(),
                                    );
                                }
                            }
                        }
                        if let Some(samples) = test_follow_up.take() {
                            eprintln!("nspanel-app: voice test (follow-up) with {} samples", samples.len());
                            test_audio = Some((samples, 0));
                        }
                        let _ = to_api.send(ToApi::Start {
                            conversation_id: conversation_id.clone(),
                            wake_word_phrase: String::new(),
                        });
                        state = State::Starting;
                        since = Instant::now();
                        ui(VoiceUi::FollowUp);
                    } else if !muted && (!speaking || !aec.ptr.is_null()) {
                        // With echo cancellation the wake word also works while
                        // the panel speaks: it cuts the answer off.
                        wake_level.push(chunk);
                        if let Some(word) = engine.process(chunk).cloned().filter(|w| INTERNAL_WORDS.contains(&w.id.as_str())) {
                            // "Stop": end the answer or the ringing, no conversation.
                            eprintln!("nspanel-app: stop word '{}'", word.phrase);
                            player.stop();
                            expecting_tts = None;
                            tts_hold = None;
                            idle_after_speech = false;
                            follow_up_pending = false;
                            engine.reset();
                            ui(VoiceUi::Stopped);
                        } else if let Some(word) = engine.last_detection().cloned() {
                            let (level_dbfs, score, snr_db) = (wake_level.dbfs(), engine.last_score(), wake_level.snr_db());
                            eprintln!(
                                "nspanel-app: wake word '{}'{} level {level_dbfs:.1} dBFS snr {} score {score:.2}",
                                word.phrase,
                                if speaking { " (during playback)" } else { "" },
                                snr_db.map(|v| format!("{v:.1} dB")).unwrap_or_else(|| "n/a".into())
                            );
                            // Before the run: Home Assistant has the values when the pipeline starts.
                            let _ = to_api.send(ToApi::WakeInfo { level_dbfs, score, snr_db });
                            // The next wake word measures only audio heard after this run.
                            wake_level = WakeLevel::new();
                            if speaking {
                                player.stop();
                                expecting_tts = None;
                                follow_up_pending = false;
                            }
                            tts_hold = None;
                            idle_after_speech = false;
                            player.play(Sound::Chime);
                            engine.reset();
                            follow_up_run = false;
                            conversation_id.clear();
                            let _ = to_api.send(ToApi::Start {
                                conversation_id: String::new(),
                                wake_word_phrase: word.phrase.clone(),
                            });
                            state = State::Starting;
                            since = Instant::now();
                            ui(VoiceUi::Listening);
                        }
                    }
                }
                State::Starting => {
                    if since.elapsed() > Duration::from_secs(5) {
                        state = State::Idle;
                        test_audio = None;
                        ui(VoiceUi::Error("No answer from Home Assistant".into()));
                    }
                }
                State::Streaming => {
                    // after the wake word too: nobody speaking within FOLLOW_UP_SILENCE ends the listening
                    if !speech_seen && test_audio.is_none() {
                        if detector.speech(chunk) {
                            speech_seen = true;
                            eprintln!(
                                "nspanel-app: voice: speech after {} ms (panel)",
                                stream_start.elapsed().as_millis()
                            );
                        } else if stream_start.elapsed() > FOLLOW_UP_SILENCE {
                            // Nobody spoke: abort the run (Home Assistant drops it, no answer).
                            eprintln!(
                                "nspanel-app: voice: no speech within {} s (room {:.0} dB, loudest {:+.1} dB over it), listening ended",
                                FOLLOW_UP_SILENCE.as_secs(),
                                detector.floor,
                                detector.peak
                            );
                            let _ = to_api.send(ToApi::Stop);
                            state = State::Idle;
                            quiet_until = Some(Instant::now() + Duration::from_secs(3));
                            engine.reset();
                            // an empty follow-up just closes; after a wake word the panel says it heard nothing
                            ui(if follow_up_run { VoiceUi::Idle } else { VoiceUi::NotUnderstood });
                            follow_up_run = false;
                            continue;
                        }
                    }
                    if let Some((samples, pos)) = test_audio.as_mut() {
                        // Test audio, then silence.
                        let mut out = vec![0i16; chunk.len()];
                        for (i, o) in out.iter_mut().enumerate() {
                            *o = *samples.get(*pos + i).unwrap_or(&0);
                        }
                        *pos += chunk.len();
                        let bytes: Vec<u8> = out.iter().flat_map(|s| s.to_le_bytes()).collect();
                        let _ = to_api.send(ToApi::Audio(bytes));
                    } else if !muted {
                        let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
                        let _ = to_api.send(ToApi::Audio(bytes));
                    }
                    // 15 s without news from Home Assistant, or MAX_STREAM in
                    // all (events cannot keep the microphones open longer).
                    if since.elapsed() > Duration::from_secs(15) || stream_start.elapsed() > MAX_STREAM {
                        let _ = to_api.send(ToApi::Stop);
                        state = State::Waiting;
                    }
                }
                State::Waiting => {
                    if since.elapsed() > Duration::from_secs(30) {
                        state = State::Idle;
                        ui(VoiceUi::Idle);
                    }
                }
            }
        }
    });
}
