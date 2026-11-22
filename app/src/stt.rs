//! Speech recognition of a short piece of audio through Home Assistant (`POST /api/stt/<engine>`), outside any
//! conversation: what someone says while a timer rings is only checked for a stop word, never answered.
//! The engine and its language are the panel's own assistant's (the integration sends them as `stt`).

use std::sync::Mutex;
use std::time::Duration;

/// The panel's speech recognition: (engine entity, language), from the integration's settings.
static ENGINE: Mutex<Option<(String, String)>> = Mutex::new(None);

pub fn set_engine(engine: &str, language: &str) {
    let want = (!engine.is_empty()).then(|| (engine.to_string(), language.to_string()));
    let mut e = ENGINE.lock().unwrap();
    if *e != want {
        if let Some((engine, language)) = &want {
            eprintln!("nspanel-app: speech recognition for stop words: {engine} ({language})");
        }
        *e = want;
    }
}

pub fn engine() -> Option<(String, String)> {
    ENGINE.lock().unwrap().clone()
}

/// The text of 16 kHz mono samples in the panel's language (blocking; call it off the audio thread).
pub fn recognize(samples: &[i16]) -> Result<String, String> {
    let (engine, language) = engine().ok_or("no speech recognition known for the panel")?;
    recognize_in(&engine, &language, samples)
}

/// What is said over a ringing timer, for its stop words (blocking): made louder first (the echo canceller leaves
/// speech 40 to 50 dB under full scale, and the recognition then often hears nothing), recognized in the panel's
/// language and, when that is not English, in English at the same time (a Hungarian recognition mostly hears nothing
/// in an English "Stop"). The texts heard, the panel's language first.
pub fn recognize_ring(samples: &[i16]) -> Vec<String> {
    let Some((engine, language)) = engine() else { return Vec::new() };
    let loud = louder(samples);
    let english = !language.to_lowercase().starts_with("en");
    std::thread::scope(|s| {
        let other = english.then(|| s.spawn(|| recognize_in(&engine, "en-US", &loud)));
        let mut out = Vec::new();
        match recognize_in(&engine, &language, &loud) {
            Ok(text) => out.push(text),
            Err(e) => eprintln!("nspanel-app: speech recognition ({language}): {e}"),
        }
        if let Some(Ok(result)) = other.map(|h| h.join()) {
            match result {
                Ok(text) => out.push(text),
                Err(e) => {
                    static TOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                    if !TOLD.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        eprintln!("nspanel-app: speech recognition in English: {e}");
                    }
                }
            }
        }
        out
    })
}

/// Quiet speech raised until its loudest sample is about 3 dB under full scale (at most 30 times), never clipped;
/// louder speech stays as it is.
pub fn louder(samples: &[i16]) -> Vec<i16> {
    let peak = samples.iter().map(|&s| (s as i32).abs()).max().unwrap_or(0);
    let gain = if peak == 0 { 1.0 } else { (23000.0 / peak as f32).clamp(1.0, 30.0) };
    samples.iter().map(|&s| (s as f32 * gain).round().clamp(-32768.0, 32767.0) as i16).collect()
}

/// Of the texts heard in a piece of speech, the one that stops a ring, else the first one heard at all.
pub fn ring_text(texts: &[String]) -> String {
    texts.iter().find(|t| stops_ring(t)).or_else(|| texts.iter().find(|t| !t.is_empty())).cloned().unwrap_or_default()
}

fn recognize_in(engine: &str, language: &str, samples: &[i16]) -> Result<String, String> {
    let (ha_url, auth) = crate::sounds::ha().ok_or("no Home Assistant")?;
    let token = auth.access_token().map_err(|_| "no login".to_string())?;
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let resp = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(8))
        .build()
        .post(&format!("{ha_url}/api/stt/{engine}"))
        .set("Authorization", &format!("Bearer {token}"))
        .set("X-Speech-Content", &format!("format=wav; codec=pcm; sample_rate=16000; bit_rate=16; channel=1; language={language}"))
        .send_bytes(&bytes)
        .map_err(|e| e.to_string())?;
    let body = resp.into_string().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if v["result"] != "success" {
        return Ok(String::new());
    }
    Ok(v["text"].as_str().unwrap_or("").to_string())
}

/// What stops a ringing timer: a stop phrase (`is_stop`), or a short sentence whose words include one starting with
/// "állj" or "stop" (through the ring the speech recognition sometimes stretches a lone "állj" into "Álljuk meg …").
/// Any other text never stops it, however short: a one-word fragment of a conversation in the room is not a stop.
pub fn stops_ring(text: &str) -> bool {
    let words: Vec<String> = text.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2).map(str::to_lowercase).collect();
    let stem = words.len() <= 6 && words.iter().any(|w| stop_stem(w));
    is_stop(text) || stem
}

/// Stop words and two-word stop phrases (Hungarian and English).
const STOP_WORDS: &[&str] = &[
    "állj", "álljál", "álljon", "stop", "stopp", "sztop", "sztáp", "sztap", "elég", "csend", "csendet", "hallgass", "kuss", "enough", "quiet", "silence", "cancel",
];
const STOP_PAIRS: &[(&str, &str)] = &[
    ("kapcsold", "ki"),
    ("állítsd", "le"),
    ("állítsd", "meg"),
    ("hagyd", "abba"),
    ("turn", "off"),
    ("shut", "up"),
];
/// Small words that may stand around a stop word in a voice run's stop phrase ("állj már", "állj meg", "elég legyen",
/// "stop the timer", "that's enough").
const STOP_FILLERS: &[&str] = &[
    "már", "most", "kérlek", "légyszi", "meg", "le", "legyen", "oké", "ok", "okay", "a", "az", "időzítő", "időzítőt", "ébresztő", "ébresztőt",
    "please", "now", "it", "that", "s", "the", "timer", "alarm",
];

/// A word starting with "állj", "stop", or "sztop" / "sztáp" / "sztap" (an English "stop" heard by a Hungarian speech
/// recognition).
fn stop_stem(w: &str) -> bool {
    w.starts_with("állj") || w.starts_with("stop") || w.starts_with("sztop") || w.starts_with("sztáp") || w.starts_with("sztap")
}

/// A stop word as `stops_ring` takes it: a listed one, or one of `stop_stem`.
fn stop_word(w: &str) -> bool {
    STOP_WORDS.contains(&w) || stop_stem(w)
}

fn words_of(text: &str) -> Vec<String> {
    let clean: String = text.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    clean.split_whitespace().map(str::to_string).collect()
}

/// A short phrase that means "stop" (Hungarian and English): at most four words, one of them a stop word, or a
/// two-word stop phrase.
pub fn is_stop(text: &str) -> bool {
    let words = words_of(text);
    if words.is_empty() || words.len() > 4 {
        return false;
    }
    words.iter().any(|w| STOP_WORDS.contains(&w.as_str())) || words.windows(2).any(|p| STOP_PAIRS.contains(&(p[0].as_str(), p[1].as_str())))
}

/// A voice run's whole transcript is a stop phrase: the stop words of `stops_ring` (the listed ones and the words
/// starting with "állj", "stop" or "sztop") or a stop pair, with at most a few small words around them, nothing else
/// (at most five words). A command that only contains one ("kapcsold ki a lámpát", "turn off the light") is not: it
/// goes to the assistant even while a timer rings, and so does any other short text ("nem", "köszönöm").
pub fn is_only_stop(text: &str) -> bool {
    let words = words_of(text);
    if words.is_empty() || words.len() > 5 {
        return false;
    }
    let mut stop = vec![false; words.len()];
    for (i, w) in words.iter().enumerate() {
        stop[i] = stop_word(w);
    }
    for i in 1..words.len() {
        if STOP_PAIRS.contains(&(words[i - 1].as_str(), words[i].as_str())) {
            stop[i - 1] = true;
            stop[i] = true;
        }
    }
    stop.iter().any(|s| *s) && words.iter().zip(&stop).all(|(w, s)| *s || STOP_FILLERS.contains(&w.as_str()))
}

#[cfg(test)]
mod tests {
    use super::{is_only_stop, is_stop, louder, ring_text, stops_ring};

    #[test]
    fn an_english_stop_stops_the_ring() {
        // an English speech recognition, and an English "stop" heard by a Hungarian one
        for t in ["Stop.", "Stop!", "Stop, stop!", "Stop it.", "Stop the timer.", "Sztop!", "Sztáp.", "Sztap!", "Sztáp, sztáp!"] {
            assert!(stops_ring(t), "{t}");
            assert!(is_only_stop(t), "{t}");
        }
        // a word that only looks alike is not a stop
        assert!(!stops_ring("Stáb."));
        assert!(!is_only_stop("Stop the music and play the news."));
    }

    #[test]
    fn run_transcripts_that_only_stop() {
        assert!(is_only_stop("Állj!"));
        assert!(is_only_stop("Állj, állj!"));
        assert!(is_only_stop("Stop."));
        assert!(is_only_stop("Elég már."));
        assert!(is_only_stop("Kapcsold ki!"));
        assert!(is_only_stop("Kapcsold ki az ébresztőt."));
        assert!(is_only_stop("Állítsd le az időzítőt!"));
        assert!(is_only_stop("Stop the timer, please."));
        assert!(!is_only_stop(""));
        assert!(!is_only_stop("Kapcsold ki a lámpát."));
        assert!(!is_only_stop("Turn off the kitchen light."));
        assert!(!is_only_stop("Mennyi az idő?"));
        assert!(!is_only_stop("Köszönöm."));
        assert!(!is_only_stop("Elég hangos."));
        assert!(!is_only_stop("Állítólag igen."));
        assert!(!is_only_stop("Stop the music."));
        assert!(!is_only_stop("Indíts egy öt perces időzítőt."));
        assert!(!is_only_stop("A"));
    }

    #[test]
    fn run_end_takes_the_ring_words() {
        // a stop phrase that stops the ring over it stops it at the end of a voice run too
        for t in ["Állj meg!", "Állj le!", "Elég legyen!", "Csend legyen!", "Oké, stop.", "Stopp!", "Álljuk meg.", "Sztop!", "That's enough."] {
            assert!(stops_ring(t), "{t}");
            assert!(is_only_stop(t), "{t}");
        }
        // anything else goes to the assistant: a short answer, a command with a stop pair or a stop word in it
        for t in [
            "Nem.",
            "Igen.",
            "Szia.",
            "Köszönöm.",
            "Elég jó.",
            "Kapcsold ki a lámpát.",
            "Állítsd le a zenét.",
            "Turn off the light.",
            "Indítsd el a stoppert.",
            "Állj meg a kapuban.",
            "Mennyi az idő?",
        ] {
            assert!(!is_only_stop(t), "{t}");
        }
    }

    #[test]
    fn ring_stops() {
        assert!(!stops_ring("Fájl."));
        assert!(!stops_ring("Mobil."));
        assert!(!stops_ring("Szerintem."));
        assert!(!stops_ring("Köszönöm."));
        assert!(!stops_ring("Állítólag igen."));
        assert!(stops_ring("Állj."));
        assert!(stops_ring("Állj, állj!"));
        assert!(!stops_ring("Köszönöm szépen."));
        assert!(!stops_ring(""));
        assert!(!stops_ring("Beállítottam egy egyperces időzítőt."));
        assert!(stops_ring("Álljuk meg az időpontot állítsuk."));
        assert!(!stops_ring("Állítsd be a konyhai időzítőt öt percre, kérlek szépen most."));
    }

    #[test]
    fn stop_words() {
        assert!(is_stop("Állj."));
        assert!(is_stop("Állj, állj!"));
        assert!(is_stop("Stop."));
        assert!(is_stop("Kapcsold ki!"));
        assert!(is_stop("Elég már."));
        assert!(!is_stop("Beállítottam egy egyperces időzítőt."));
        assert!(!is_stop("Köszönöm szépen."));
        assert!(!is_stop(""));
        assert!(!is_stop("Set a one minute timer on the panel and then stop."));
    }

    #[test]
    fn ring_speech_made_louder() {
        let quiet = louder(&[0, 500, -1000, 250]);
        assert_eq!(quiet, vec![0, 11500, -23000, 5750]);
        // at most 30 times
        assert_eq!(louder(&[100, -50]), vec![3000, -1500]);
        // loud speech and silence stay as they are
        assert_eq!(louder(&[30000, -20000]), vec![30000, -20000]);
        assert_eq!(louder(&[0, 0]), vec![0, 0]);
    }

    #[test]
    fn ring_text_picks_the_stop() {
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // a Hungarian recognition hears nothing in an English "Stop", the English one hears it
        assert_eq!(ring_text(&t(&["", "Stop."])), "Stop.");
        assert_eq!(ring_text(&t(&["Nem.", "Stop."])), "Stop.");
        assert_eq!(ring_text(&t(&["Állj!", ""])), "Állj!");
        // what the recognitions made of the ring's leftover never stops it
        for heard in [["Sztálin.", ""], ["Dub.", ""], ["No.", ""], ["", ""]] {
            assert!(!stops_ring(&ring_text(&t(&heard))));
        }
        assert_eq!(ring_text(&t(&["Nem.", ""])), "Nem.");
        assert_eq!(ring_text(&[]), "");
    }
}
