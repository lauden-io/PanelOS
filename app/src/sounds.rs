//! The sounds the panel rings with: its own designed sounds (in the UI bundle, `sounds/<id>.wav`) and
//! sounds uploaded in the integration, of which the panel keeps a copy (`/mnt/data/panelos-sounds`),
//! so they ring even when Home Assistant is away. The integration names a sound as `auto` (the place's
//! default), `lib:<id>` (a designed one), `own:<id>` (an uploaded one) or `ma:<item>` (Music Assistant,
//! alarms only: the default rings on the panel itself).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::auth::Auth;

/// The designed sounds (ids as the integration uses them).
pub const LIB: [&str; 8] = ["chimes", "marimba", "birds", "bell", "digital", "synth", "sos", "warning"];

/// Where the copies of uploaded sounds are kept.
const OWN_DIR: &str = "/mnt/data/panelos-sounds";
/// The most samples kept in memory (48 kHz mono): the sounds in use, about 4 MB each at most.
const CACHE_MOST: usize = 4;

/// Home Assistant's address and the login, to fetch uploaded sounds.
static HA: OnceLock<(String, Arc<Auth>)> = OnceLock::new();
/// Loaded sounds by file path (48 kHz mono samples), the most recent last.
static CACHE: Mutex<Vec<(PathBuf, Arc<Vec<i16>>)>> = Mutex::new(Vec::new());
/// Uploaded sounds being fetched now.
static FETCHING: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn init(ha_url: String, auth: Arc<Auth>) {
    let _ = HA.set((ha_url, auth));
}

/// Home Assistant's address and the panel's login (for other short requests, e.g. speech recognition).
pub fn ha() -> Option<(String, Arc<Auth>)> {
    HA.get().cloned()
}

fn lib_path(id: &str) -> PathBuf {
    PathBuf::from(env!("PANELOS_UI_PICTURES")).join("sounds").join(format!("{id}.wav"))
}

fn own_path(id: &str) -> PathBuf {
    PathBuf::from(OWN_DIR).join(format!("{id}.wav"))
}

/// An uploaded sound's id is the integration's (letters, digits, `_`, `-`): never a path.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn load(path: &PathBuf) -> Option<Arc<Vec<i16>>> {
    let mut cache = CACHE.lock().unwrap();
    if let Some(i) = cache.iter().position(|(p, _)| p == path) {
        let hit = cache.remove(i);
        let samples = hit.1.clone();
        cache.push(hit);
        return Some(samples);
    }
    drop(cache);
    let bytes = std::fs::read(path).ok()?;
    let (rate, channels, data) = crate::voice::parse_wav(&bytes)?;
    let samples = Arc::new(crate::voice::resample_to_48k(rate, channels, &data));
    if samples.is_empty() {
        return None;
    }
    let mut cache = CACHE.lock().unwrap();
    cache.push((path.clone(), samples.clone()));
    while cache.len() > CACHE_MOST {
        cache.remove(0);
    }
    Some(samples)
}

/// The samples for a sound setting, else the place's default (a designed sound), else none (the
/// caller beeps). An uploaded sound not on the panel yet is fetched meanwhile.
pub fn resolve(value: &str, default: &str) -> Option<Arc<Vec<i16>>> {
    let chosen = match value.split_once(':') {
        Some(("lib", id)) if LIB.contains(&id) => load(&lib_path(id)),
        Some(("own", id)) if safe_id(id) => {
            let path = own_path(id);
            if !path.exists() {
                fetch(id);
            }
            load(&path)
        }
        _ => None,
    };
    chosen.or_else(|| load(&lib_path(default)))
}

/// Fetches an uploaded sound into the panel's copies (in a thread; the default rings meanwhile).
fn fetch(id: &str) {
    let Some((ha_url, auth)) = HA.get().cloned() else { return };
    {
        let mut busy = FETCHING.lock().unwrap();
        if busy.iter().any(|x| x == id) {
            return;
        }
        busy.push(id.to_string());
    }
    let id = id.to_string();
    std::thread::spawn(move || {
        let result = (|| -> Result<u64, String> {
            let url = format!("{ha_url}/api/panelos/sound/{id}");
            let token = auth.access_token().map_err(|_| "no login".to_string())?;
            let resp = ureq::AgentBuilder::new()
                .redirects(0)
                .timeout(Duration::from_secs(60))
                .build()
                .get(&url)
                .set("Authorization", &format!("Bearer {token}"))
                .call()
                .map_err(|e| e.to_string())?;
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(resp.into_reader(), 24 * 1024 * 1024), &mut bytes).map_err(|e| e.to_string())?;
            crate::voice::parse_wav(&bytes).ok_or("not a 16-bit WAV")?;
            std::fs::create_dir_all(OWN_DIR).map_err(|e| e.to_string())?;
            // written beside and renamed: a half-written copy never counts
            let tmp = PathBuf::from(OWN_DIR).join(format!("{id}.part"));
            std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, own_path(&id)).map_err(|e| e.to_string())?;
            Ok(bytes.len() as u64)
        })();
        match result {
            Ok(n) => eprintln!("nspanel-app: sound {id}: kept a copy ({n} bytes)"),
            Err(e) => eprintln!("nspanel-app: sound {id}: {e}"),
        }
        FETCHING.lock().unwrap().retain(|x| *x != id);
    });
}

/// Keeps copies of the uploaded sounds the settings use (fetching missing ones) and removes the
/// copies no setting uses any more.
pub fn keep(used: &[String]) {
    let ids: Vec<&str> = used.iter().filter_map(|v| v.strip_prefix("own:")).filter(|id| safe_id(id)).collect();
    for id in &ids {
        if !own_path(id).exists() {
            fetch(id);
        }
    }
    if let Ok(dir) = std::fs::read_dir(OWN_DIR) {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let id = name.trim_end_matches(".wav");
            if name.ends_with(".wav") && !ids.contains(&id) {
                let _ = std::fs::remove_file(entry.path());
                CACHE.lock().unwrap().retain(|(p, _)| *p != entry.path());
            }
        }
    }
}

/// A designed sound's name on the panel (its own names; the integration uses plain ones).
pub fn panel_name(id: &str) -> String {
    match id {
        "sos" => Some(crate::texts::nth("sounds.sos", 0)).filter(|n| !n.is_empty()).unwrap_or_else(|| "SOS".into()),
        "warning" => "Warning tone".into(),
        _ => String::new(),
    }
}

/// The timers page's default sound: its look's own (Plain, Kitchen timer, Hourglass, Synthwave, Flip clock).
pub fn timer_default(look: i32) -> &'static str {
    match look {
        1 => "bell",
        2 => "chimes",
        3 => "synth",
        4 => "digital",
        _ => "marimba",
    }
}

/// The panel's own alarm sounds in its editor's order (index → designed sound): Rise and Shine (chimes), Up and At 'Em
/// (marimba), Early Bird (birds), Saved by the Bell, Old School (digital), Neon Sunrise (synth), as `LIB` lists them.
pub fn alarm_lib(index: i32) -> &'static str {
    match index {
        0 => "chimes",
        1 => "marimba",
        3 => "bell",
        4 => "digital",
        5 => "synth",
        _ => "birds",
    }
}
