//! Changing the Wi-Fi network on the panel (settings → Wi-Fi, or the setup screen's Wi-Fi card).
//!
//! The panel never locks itself out. The new network is added to the running wpa_supplicant next to the current
//! one and tried; it counts as connected only when the panel is associated, has an address, and Home Assistant
//! answers at its address. Only then is it written to the saved config (`/mnt/persist/nspanel/wpa_supplicant.conf`),
//! with the network it replaces kept as a fallback of lower priority. When it does not connect, the panel goes back
//! to the network it was on and forgets the new one: the saved config is not touched. The saved file is written
//! whole to a temporary file and renamed over the old one, so it is never half written.

use std::cell::RefCell;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AppWindow, WifiNet};

const IFACE: &str = "wlan0";
/// The saved config (read by S40wifi at every start) and the running copy wpa_supplicant was started with.
const SAVED_CONF: &str = "/mnt/persist/nspanel/wpa_supplicant.conf";
const RUNNING_CONF: &str = "/tmp/wpa_supplicant.conf";
const UDHCPC_PID: &str = "/var/run/udhcpc.wlan0.pid";
/// How long each step of a switch may take.
const ASSOCIATE_WAIT: Duration = Duration::from_secs(25);
const ADDRESS_WAIT: Duration = Duration::from_secs(20);
const HA_TRIES: u32 = 3;
/// The scan results are read this long after the scan request (2.4 and 5 GHz take about 3–5 s).
const SCAN_WAIT: Duration = Duration::from_secs(5);

/// A switch runs: the roam check and the link watch keep their hands off the link meanwhile.
static SWITCHING: AtomicBool = AtomicBool::new(false);

pub fn switching() -> bool {
    SWITCHING.load(Ordering::SeqCst)
}

/// How a network is secured, from the scan's flags.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Security {
    Open,
    /// WPA / WPA2 personal (also WPA2 / WPA3 mixed networks).
    Psk,
    /// WPA3 personal only.
    Sae,
}

/// A network from the scan results (strongest access point per name).
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    ssid: String,
    signal: i32,
    security: Security,
}

/// What is being typed: the picked network and its password, or a hidden one by name.
#[derive(Default)]
struct Draft {
    ssid: String,
    password: String,
    security: Option<Security>,
    hidden: bool,
    /// The hidden network screen: 0 the name, 1 the password.
    field: i32,
    show: bool,
}

thread_local! {
    static DRAFT: RefCell<Draft> = RefCell::new(Draft::default());
    /// The last scan, to know a picked network's security.
    static SEEN: RefCell<Vec<Seen>> = const { RefCell::new(Vec::new()) };
}

fn wpa_cli(args: &[&str]) -> Option<String> {
    let out = Command::new("wpa_cli").arg("-i").arg(IFACE).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn ok(reply: Option<String>) -> bool {
    reply.is_some_and(|r| r == "OK")
}

fn random() -> i32 {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(7);
    (n % 1_000_000_000) as i32
}

// ---------------------------------------------------------------- parsing

/// An SSID as wpa_cli prints it (printf-escaped: \xNN, \\, \", \e, \n, \r, \t).
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'x' if i + 3 < b.len() => {
                    if let Some(v) = s.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                        out.push(v);
                        i += 4;
                        continue;
                    }
                    out.push(b[i]);
                    i += 1;
                    continue;
                }
                b'n' => out.push(b'\n'),
                b'r' => out.push(b'\r'),
                b't' => out.push(b'\t'),
                b'e' => out.push(0x1b),
                c => out.push(c),
            }
            i += 2;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn security_of(flags: &str) -> Security {
    if flags.contains("PSK") {
        Security::Psk
    } else if flags.contains("SAE") {
        Security::Sae
    } else if flags.contains("WPA") || flags.contains("RSN") || flags.contains("WEP") || flags.contains("EAP") {
        // enterprise and WEP networks: tried as personal ones (the password screen asks; they will not connect)
        Security::Psk
    } else {
        Security::Open
    }
}

/// `wpa_cli scan_results`: one entry per network name, strongest first; hidden names left out.
fn parse_scan(text: &str) -> Vec<Seen> {
    let mut seen: Vec<Seen> = Vec::new();
    for l in text.lines().skip_while(|l| !l.starts_with("bssid")).skip(1) {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 5 {
            continue;
        }
        let Ok(signal) = f[2].trim().parse::<i32>() else { continue };
        let ssid = unescape(f[4]);
        if ssid.is_empty() || ssid.chars().all(|c| c == '\0') {
            continue;
        }
        let security = security_of(f[3]);
        match seen.iter_mut().find(|s| s.ssid == ssid) {
            Some(s) if signal > s.signal => {
                s.signal = signal;
                s.security = security;
            }
            Some(_) => {}
            None => seen.push(Seen { ssid, signal, security }),
        }
    }
    seen.sort_by(|a, b| b.signal.cmp(&a.signal));
    seen
}

/// Signal bars as the integration's page shows them.
fn bars(dbm: i32) -> i32 {
    if dbm > -55 {
        4
    } else if dbm > -63 {
        3
    } else if dbm > -72 {
        2
    } else {
        1
    }
}

/// `wpa_cli status`: the network id and name while fully connected.
fn parse_status(text: &str) -> Option<(String, String)> {
    let get = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('='));
    if get("wpa_state")? != "COMPLETED" {
        return None;
    }
    Some((get("id")?.to_string(), unescape(get("ssid")?)))
}

/// `wpa_cli list_networks`: id, name, flags of each network wpa_supplicant knows.
fn parse_networks(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .skip_while(|l| !l.starts_with("network id"))
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() >= 2).then(|| (f[0].to_string(), unescape(f[1]), f.get(3).map(|x| x.to_string()).unwrap_or_default()))
        })
        .collect()
}

// ---------------------------------------------------------------- the saved config

/// One `network={…}` block of a wpa_supplicant config, with its name.
#[derive(Debug, Clone, PartialEq)]
struct Block {
    ssid: String,
    lines: Vec<String>,
}

/// The value of an `ssid=` line: "quoted" or hex.
fn ssid_value(v: &str) -> Option<String> {
    let v = v.trim();
    if let Some(q) = v.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        return Some(q.to_string());
    }
    if v.len() % 2 == 0 && v.chars().all(|c| c.is_ascii_hexdigit()) {
        let bytes: Option<Vec<u8>> = (0..v.len()).step_by(2).map(|i| u8::from_str_radix(&v[i..i + 2], 16).ok()).collect();
        return bytes.map(|b| String::from_utf8_lossy(&b).into_owned());
    }
    None
}

/// The lines outside the network blocks, and the blocks.
fn parse_config(text: &str) -> (Vec<String>, Vec<Block>) {
    let mut head = Vec::new();
    let mut blocks = Vec::new();
    let mut cur: Option<Vec<String>> = None;
    for line in text.lines() {
        let t = line.trim();
        match cur.as_mut() {
            None if t.starts_with("network={") => cur = Some(Vec::new()),
            None => {
                if !t.is_empty() {
                    head.push(line.to_string());
                }
            }
            Some(lines) if t == "}" => {
                let lines = std::mem::take(lines);
                let ssid = lines.iter().find_map(|l| l.trim().strip_prefix("ssid=").and_then(ssid_value)).unwrap_or_default();
                blocks.push(Block { ssid, lines });
                cur = None;
            }
            Some(lines) => {
                if !t.is_empty() {
                    lines.push(t.to_string());
                }
            }
        }
    }
    (head, blocks)
}

/// The new saved config: the old one's head lines (the control socket at least), the new network first and the one
/// it replaces as a fallback (a lower priority; left out when it is the same network).
fn build_config(head: &[String], new: &Block, fallback: Option<&Block>) -> String {
    let mut out = String::new();
    if !head.iter().any(|l| l.trim_start().starts_with("ctrl_interface=")) {
        out.push_str("ctrl_interface=/var/run/wpa_supplicant\n");
    }
    for l in head {
        out.push_str(l);
        out.push('\n');
    }
    let mut block = |b: &Block, priority: i32| {
        out.push_str("network={\n");
        for l in b.lines.iter().filter(|l| !l.starts_with("priority=")) {
            out.push('\t');
            out.push_str(l);
            out.push('\n');
        }
        out.push_str(&format!("\tpriority={priority}\n}}\n"));
    };
    block(new, 1);
    if let Some(f) = fallback.filter(|f| f.ssid != new.ssid) {
        block(f, 0);
    }
    out
}

/// An `ssid=` value: quoted when that is safe, else hex.
fn ssid_line(ssid: &str) -> String {
    if ssid.chars().all(|c| c.is_ascii_graphic() || c == ' ') && !ssid.contains('"') && !ssid.contains('\\') {
        format!("ssid=\"{ssid}\"")
    } else {
        format!("ssid={}", hex(ssid))
    }
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// The 256-bit key of a WPA passphrase (`wpa_passphrase`, the passphrase on its standard input, so it never shows
/// on a command line), as hex.
fn psk_hex(ssid: &str, passphrase: &str) -> Option<String> {
    use std::io::Write;
    let mut child = Command::new("wpa_passphrase")
        .arg(ssid)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(format!("{passphrase}\n").as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.trim().strip_prefix("psk=").map(str::to_string))
        .filter(|k| k.len() == 64 && k.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Writes a config file whole: a temporary file beside it (owner only), synced, then renamed over it.
fn write_whole(path: &str, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = format!("{path}.new");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    if let Some(dir) = std::path::Path::new(path).parent() {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- the switch

/// The network to try.
#[derive(Clone)]
struct Target {
    ssid: String,
    password: String,
    security: Security,
    hidden: bool,
}

/// How a switch ended.
#[derive(Debug, PartialEq)]
enum Outcome {
    Connected,
    AlreadyOn,
    TooShort,
    WrongPassword,
    NoAnswer,
    NoAddress,
    NoHomeAssistant,
    Failed,
}

fn current() -> Option<(String, String)> {
    wpa_cli(&["status"]).as_deref().and_then(parse_status)
}

fn ipv4() -> Option<String> {
    let out = Command::new("ip").args(["-4", "-o", "addr", "show", "dev", IFACE]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().skip_while(|w| *w != "inet").nth(1).map(|a| a.split('/').next().unwrap_or(a).to_string())
}

/// A new address for the new link: the DHCP client lets the old lease go and asks again (SIGUSR2, then SIGUSR1).
fn renew_address() {
    let Some(pid) = std::fs::read_to_string(UDHCPC_PID).ok().and_then(|p| p.trim().parse::<i32>().ok()) else { return };
    // SAFETY: kill(2) on the DHCP client's pid from its pid file.
    unsafe {
        libc::kill(pid, libc::SIGUSR2);
    }
    std::thread::sleep(Duration::from_millis(300));
    unsafe {
        libc::kill(pid, libc::SIGUSR1);
    }
}

fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    done()
}

fn home_assistant_answers() -> bool {
    let Some(url) = crate::config::AppConfig::load().ok().map(|c| c.ha_url) else { return true };
    (0..HA_TRIES).any(|i| {
        if i > 0 {
            std::thread::sleep(Duration::from_secs(2));
        }
        crate::is_home_assistant(&url)
    })
}

/// Tries the network; `step` tells the screen where it is. Goes back to the network it was on unless the new one
/// connects.
fn switch(t: &Target, step: &dyn Fn(String)) -> (Outcome, String) {
    let before = current();
    let old_name = before.as_ref().map(|(_, n)| n.clone()).unwrap_or_default();
    let known = wpa_cli(&["list_networks"]).as_deref().map(parse_networks).unwrap_or_default();
    if before.as_ref().is_some_and(|(_, n)| *n == t.ssid) && t.password.is_empty() {
        return (Outcome::AlreadyOn, old_name);
    }
    // a saved network picked without a new password: the one wpa_supplicant knows; otherwise a new one
    let saved_id = (t.password.is_empty() && t.security != Security::Open)
        .then(|| known.iter().find(|(_, n, _)| *n == t.ssid).map(|(id, _, _)| id.clone()))
        .flatten();
    if saved_id.is_none() && t.security == Security::Psk && !(8..=63).contains(&t.password.chars().count()) {
        return (Outcome::TooShort, old_name);
    }
    let added = saved_id.is_none();
    let id = match saved_id {
        Some(id) => id,
        None => {
            let Some(id) = wpa_cli(&["add_network"]).filter(|r| r.chars().all(|c| c.is_ascii_digit()) && !r.is_empty()) else {
                return (Outcome::Failed, old_name);
            };
            let mut good = ok(wpa_cli(&["set_network", &id, "ssid", &hex(&t.ssid)]));
            if t.hidden {
                good &= ok(wpa_cli(&["set_network", &id, "scan_ssid", "1"]));
            }
            good &= match t.security {
                Security::Open => ok(wpa_cli(&["set_network", &id, "key_mgmt", "NONE"])),
                Security::Psk => match psk_hex(&t.ssid, &t.password) {
                    Some(k) => ok(wpa_cli(&["set_network", &id, "key_mgmt", "WPA-PSK"])) && ok(wpa_cli(&["set_network", &id, "psk", &k])),
                    None => false,
                },
                Security::Sae => {
                    ok(wpa_cli(&["set_network", &id, "key_mgmt", "SAE"]))
                        && ok(wpa_cli(&["set_network", &id, "ieee80211w", "2"]))
                        && ok(wpa_cli(&["set_network", &id, "sae_password", &format!("\"{}\"", t.password)]))
                }
            };
            if !good {
                let _ = wpa_cli(&["remove_network", &id]);
                return (Outcome::Failed, old_name);
            }
            id
        }
    };
    crate::connlog::note(&format!("wifi: trying another network (from the panel's settings), {}", if added { "new" } else { "saved" }));
    step(format!("Connecting to {}…", t.ssid));
    let _ = wpa_cli(&["select_network", &id]);
    let mut outcome = Outcome::Connected;
    let mut wrong_key = false;
    let associated = wait_for(ASSOCIATE_WAIT, || {
        if current().is_some_and(|(cid, _)| cid == id) {
            return true;
        }
        // wpa_supplicant leaves a network that refused the key aside for a while
        wrong_key = wpa_cli(&["list_networks"])
            .as_deref()
            .map(parse_networks)
            .unwrap_or_default()
            .iter()
            .any(|(nid, _, flags)| *nid == id && flags.contains("TEMP-DISABLED"));
        wrong_key
    }) && !wrong_key;
    if !associated {
        outcome = if wrong_key { Outcome::WrongPassword } else { Outcome::NoAnswer };
    } else {
        step(format!("Connected to {}, getting an address…", t.ssid));
        renew_address();
        if !wait_for(ADDRESS_WAIT, || ipv4().is_some()) {
            outcome = Outcome::NoAddress;
        } else {
            step("Checking that Home Assistant answers…".into());
            if !home_assistant_answers() {
                outcome = Outcome::NoHomeAssistant;
            }
        }
    }
    if outcome == Outcome::Connected {
        save(t, before.as_ref().map(|(_, n)| n.as_str()), &id);
        // the old network stays known as a fallback
        if let Some((old_id, _)) = &before {
            if *old_id != id {
                let _ = wpa_cli(&["set_network", old_id, "priority", "0"]);
                let _ = wpa_cli(&["set_network", &id, "priority", "1"]);
                let _ = wpa_cli(&["enable_network", old_id]);
            }
        }
        crate::connlog::note(&format!("wifi: the new network connects, saved ({})", ipv4().unwrap_or_default()));
        return (outcome, old_name);
    }
    // back to where it was: the old network again, the new one forgotten
    crate::connlog::note(&format!("wifi: the new network did not connect ({outcome:?}), back to the old one"));
    step(format!("Going back to {}…", if old_name.is_empty() { "the saved network" } else { &old_name }));
    match &before {
        Some((old_id, _)) => {
            let _ = wpa_cli(&["select_network", old_id]);
        }
        None => {}
    }
    if added {
        let _ = wpa_cli(&["remove_network", &id]);
    }
    let _ = wpa_cli(&["enable_network", "all"]);
    renew_address();
    let back = wait_for(ASSOCIATE_WAIT, || current().is_some()) && wait_for(ADDRESS_WAIT, || ipv4().is_some());
    crate::connlog::note(&format!("wifi: {}", if back { "back on the old network" } else { "the old network is not back yet (wpa_supplicant keeps trying)" }));
    (outcome, old_name)
}

/// The network that connected becomes the saved one; the one it replaces stays in the file as a fallback.
fn save(t: &Target, old: Option<&str>, id: &str) {
    let text = std::fs::read_to_string(SAVED_CONF).unwrap_or_default();
    let (head, blocks) = parse_config(&text);
    let new = if t.password.is_empty() && t.security != Security::Open {
        // a saved network picked again: its own block
        blocks.iter().find(|b| b.ssid == t.ssid).cloned()
    } else {
        let mut lines = vec![ssid_line(&t.ssid)];
        if t.hidden {
            lines.push("scan_ssid=1".into());
        }
        match t.security {
            Security::Open => lines.push("key_mgmt=NONE".into()),
            Security::Psk => match psk_hex(&t.ssid, &t.password) {
                Some(k) => lines.push(format!("psk={k}")),
                None => return,
            },
            Security::Sae => {
                lines.push("key_mgmt=SAE".into());
                lines.push("ieee80211w=2".into());
                lines.push(format!("sae_password=\"{}\"", t.password));
            }
        }
        Some(Block { ssid: t.ssid.clone(), lines })
    };
    let Some(new) = new else {
        crate::connlog::note(&format!("wifi: network {id} not found in the saved config, nothing written"));
        return;
    };
    let fallback = old.and_then(|o| blocks.iter().find(|b| b.ssid == o));
    let config = build_config(&head, &new, fallback);
    for path in [SAVED_CONF, RUNNING_CONF] {
        if let Err(e) = write_whole(path, &config) {
            crate::connlog::note(&format!("wifi: {path} not written: {e}"));
        }
    }
}

// ---------------------------------------------------------------- the screens

/// The Wi-Fi list (with a back arrow), scanning at once.
pub fn open(ui: &AppWindow) {
    ui.set_wifi_variant(random());
    ui.set_wifi_can_back(true);
    ui.set_wifi_setup(true);
    scan(ui);
}

fn scan(ui: &AppWindow) {
    if ui.get_wifi_scanning() {
        return;
    }
    ui.set_wifi_scanning(true);
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let _ = wpa_cli(&["scan"]);
        std::thread::sleep(SCAN_WAIT);
        let seen = wpa_cli(&["scan_results"]).as_deref().map(parse_scan).unwrap_or_default();
        let saved: Vec<String> = wpa_cli(&["list_networks"]).as_deref().map(parse_networks).unwrap_or_default().into_iter().map(|(_, n, _)| n).collect();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let rows: Vec<WifiNet> = seen
                .iter()
                .take(30)
                .map(|n| WifiNet {
                    ssid: SharedString::from(n.ssid.as_str()),
                    strength: bars(n.signal),
                    secure: n.security != Security::Open,
                    saved: saved.contains(&n.ssid),
                })
                .collect();
            SEEN.with(|s| *s.borrow_mut() = seen);
            ui.set_wifi_networks(ModelRc::new(VecModel::from(rows)));
            ui.set_wifi_scanning(false);
        });
    });
}

/// Closes every Wi-Fi screen (back to the settings or the setup screen under them).
fn close_all(ui: &AppWindow) {
    ui.set_wifi_password_open(false);
    ui.set_hidden_open(false);
    ui.set_wifi_setup(false);
    ui.set_wifi_busy(false);
    ui.set_wifi_message(SharedString::new());
    if ui.get_set_view() > 0 {
        crate::shade::fill(ui);
    }
}

fn shown_password(d: &Draft) -> String {
    if d.show { d.password.clone() } else { "•".repeat(d.password.chars().count()) }
}

fn refresh_fields(ui: &AppWindow) {
    DRAFT.with(|d| {
        let d = d.borrow();
        ui.set_wifi_shown_text(shown_password(&d).into());
        ui.set_wifi_show_password(d.show);
        ui.set_hidden_name(d.ssid.as_str().into());
        ui.set_hidden_focus(d.field);
    });
}

fn message(ui: &AppWindow, text: &str, kind: i32) {
    ui.set_wifi_message(text.into());
    ui.set_wifi_message_kind(kind);
}

fn pick(ui: &AppWindow, ssid: &str) {
    if ui.get_wifi_busy() {
        return;
    }
    let security = SEEN.with(|s| s.borrow().iter().find(|n| n.ssid == ssid).map(|n| n.security)).unwrap_or(Security::Psk);
    let on_it = current().is_some_and(|(_, n)| n == ssid);
    let saved = wpa_cli(&["list_networks"]).as_deref().map(parse_networks).unwrap_or_default().iter().any(|(_, n, _)| n == ssid);
    DRAFT.with(|d| *d.borrow_mut() = Draft { ssid: ssid.to_string(), security: Some(security), ..Default::default() });
    ui.set_wifi_ssid(ssid.into());
    ui.set_wifi_key_layer(0);
    refresh_fields(ui);
    // an open network, or a saved one the panel is not on: no password to type
    if !on_it && (security == Security::Open || saved) {
        ui.set_wifi_password_open(true);
        connect(ui);
        return;
    }
    if on_it {
        message(ui, "The panel is on this network. A new password replaces the saved one.", 0);
    } else {
        message(ui, "", 0);
    }
    ui.set_wifi_password_open(true);
}

fn connect(ui: &AppWindow) {
    if ui.get_wifi_busy() {
        return;
    }
    let t = DRAFT.with(|d| {
        let d = d.borrow();
        let security = match d.security {
            Some(s) => s,
            None if d.password.is_empty() => Security::Open,
            None => Security::Psk,
        };
        Target { ssid: d.ssid.clone(), password: d.password.clone(), security, hidden: d.hidden }
    });
    if t.ssid.is_empty() {
        message(ui, "Type the network's name first.", 1);
        return;
    }
    ui.set_wifi_busy(true);
    message(ui, &format!("Connecting to {}…", t.ssid), 0);
    SWITCHING.store(true, Ordering::SeqCst);
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let step_weak = weak.clone();
        let step = move |text: String| {
            let w = step_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = w.upgrade() {
                    message(&ui, &text, 0);
                }
            });
        };
        let (outcome, old) = switch(&t, &step);
        SWITCHING.store(false, Ordering::SeqCst);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_wifi_busy(false);
            let back = if old.is_empty() { "The panel keeps its saved network.".to_string() } else { format!("Back on {old}.") };
            match outcome {
                Outcome::Connected | Outcome::AlreadyOn => {
                    message(&ui, &format!("Connected to {}.", t.ssid), 2);
                    let w = ui.as_weak();
                    slint::Timer::single_shot(Duration::from_millis(1500), move || {
                        if let Some(ui) = w.upgrade() {
                            close_all(&ui);
                        }
                    });
                }
                Outcome::TooShort => message(&ui, "A Wi-Fi password has 8 to 63 characters.", 1),
                Outcome::WrongPassword => message(&ui, &format!("The password is not right. {back}"), 1),
                Outcome::NoAnswer => message(&ui, &format!("{} did not answer. {back}", t.ssid), 1),
                Outcome::NoAddress => message(&ui, &format!("{} gave the panel no address. {back}", t.ssid), 1),
                Outcome::NoHomeAssistant => message(&ui, &format!("Home Assistant does not answer on {}. {back}", t.ssid), 1),
                Outcome::Failed => message(&ui, &format!("This network could not be set up. {back}"), 1),
            }
        });
    });
}

/// A key of the password or hidden network keyboard.
fn key(ui: &AppWindow, k: &str) {
    if ui.get_wifi_busy() {
        return;
    }
    let layer = ui.get_wifi_key_layer();
    let hidden = ui.get_hidden_open();
    let mut go = false;
    DRAFT.with(|d| {
        let mut d = d.borrow_mut();
        let name_field = hidden && d.field == 0;
        let text = if name_field { &mut d.ssid } else { &mut d.password };
        match k {
            "back" => {
                text.pop();
            }
            "shift" => ui.set_wifi_key_layer(match layer {
                0 => 1,
                1 => 0,
                2 => 3,
                _ => 2,
            }),
            "symbols" => ui.set_wifi_key_layer(if layer >= 2 { 0 } else { 2 }),
            "layout" => ui.set_kb_layout(1 - ui.get_kb_layout().clamp(0, 1)),
            "enter" => {
                if name_field {
                    d.field = 1;
                } else {
                    go = true;
                }
            }
            k => {
                if text.chars().count() < if name_field { 32 } else { 63 } {
                    text.push_str(k);
                }
                // one capital, then small letters again
                if layer == 1 {
                    ui.set_wifi_key_layer(0);
                }
            }
        }
    });
    refresh_fields(ui);
    if ui.get_wifi_message_kind() == 1 {
        message(ui, "", 0);
    }
    if go {
        connect(ui);
    }
}

/// The Wi-Fi screens' callbacks (set once at the start).
pub fn init(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.on_wifi_pick(move |ssid| {
        if let Some(ui) = weak.upgrade() {
            pick(&ui, &ssid);
        }
    });
    let weak = ui.as_weak();
    ui.on_wifi_rescan(move || {
        if let Some(ui) = weak.upgrade() {
            scan(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_wifi_other(move || {
        if let Some(ui) = weak.upgrade() {
            DRAFT.with(|d| *d.borrow_mut() = Draft { hidden: true, ..Default::default() });
            ui.set_wifi_key_layer(0);
            refresh_fields(&ui);
            message(&ui, "", 0);
            ui.set_hidden_open(true);
        }
    });
    let weak = ui.as_weak();
    ui.on_wifi_back(move || {
        if let Some(ui) = weak.upgrade() {
            if !ui.get_wifi_busy() {
                close_all(&ui);
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_wifi_password_back(move || {
        if let Some(ui) = weak.upgrade() {
            if !ui.get_wifi_busy() {
                ui.set_wifi_password_open(false);
                ui.set_hidden_open(false);
                message(&ui, "", 0);
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_wifi_password_key(move |k| {
        if let Some(ui) = weak.upgrade() {
            key(&ui, &k);
        }
    });
    let weak = ui.as_weak();
    ui.on_wifi_password_toggle_show(move || {
        if let Some(ui) = weak.upgrade() {
            DRAFT.with(|d| {
                let mut d = d.borrow_mut();
                d.show = !d.show;
            });
            refresh_fields(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_hidden_focus_field(move |i| {
        if let Some(ui) = weak.upgrade() {
            DRAFT.with(|d| d.borrow_mut().field = i.clamp(0, 1));
            refresh_fields(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_setup_change_wifi(move || {
        if let Some(ui) = weak.upgrade() {
            open(&ui);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_results() {
        let text = "bssid / frequency / signal level / flags / ssid\n\
                    aa:00:00:00:00:01\t2462\t-67\t[WPA2-PSK-CCMP][ESS]\tHome\n\
                    aa:00:00:00:00:02\t5180\t-45\t[WPA2-PSK-CCMP][ESS]\tHome\n\
                    aa:00:00:00:00:03\t2437\t-80\t[ESS]\tCaf\\xc3\\xa9\n\
                    aa:00:00:00:00:04\t2437\t-50\t[RSN-SAE-CCMP][ESS]\tNew\n\
                    aa:00:00:00:00:05\t2437\t-40\t[WPA2-PSK-CCMP][ESS]\t\n";
        let seen = parse_scan(text);
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0], Seen { ssid: "Home".into(), signal: -45, security: Security::Psk });
        assert_eq!(seen[1], Seen { ssid: "New".into(), signal: -50, security: Security::Sae });
        assert_eq!(seen[2], Seen { ssid: "Café".into(), signal: -80, security: Security::Open });
        assert_eq!(security_of("[WPA2-PSK+SAE-CCMP][ESS]"), Security::Psk);
        assert_eq!((bars(-50), bars(-60), bars(-70), bars(-90)), (4, 3, 2, 1));
    }

    #[test]
    fn status_and_networks() {
        let status = "bssid=aa:00:00:00:00:01\nfreq=2462\nssid=Home \\\"2\\\"\nid=1\nmode=station\nwpa_state=COMPLETED\nip_address=10.0.0.5\n";
        assert_eq!(parse_status(status), Some(("1".into(), "Home \"2\"".into())));
        assert_eq!(parse_status(&status.replace("COMPLETED", "SCANNING")), None);
        let list = "network id / ssid / bssid / flags\n0\tHome\tany\t[CURRENT]\n1\tNew\tany\t[DISABLED][TEMP-DISABLED]\n";
        let n = parse_networks(list);
        assert_eq!(n.len(), 2);
        assert_eq!(n[1], ("1".into(), "New".into(), "[DISABLED][TEMP-DISABLED]".into()));
    }

    #[test]
    fn saved_config_keeps_the_old_network_as_fallback() {
        let old = "ctrl_interface=/var/run/wpa_supplicant\nnetwork={\n\tssid=\"Home\"\n\tpsk=0123\n}\n";
        let (head, blocks) = parse_config(old);
        assert_eq!(head, vec!["ctrl_interface=/var/run/wpa_supplicant".to_string()]);
        assert_eq!(blocks, vec![Block { ssid: "Home".into(), lines: vec!["ssid=\"Home\"".into(), "psk=0123".into()] }]);
        let new = Block { ssid: "New net".into(), lines: vec![ssid_line("New net"), "psk=abcd".into()] };
        let text = build_config(&head, &new, blocks.first());
        assert_eq!(
            text,
            "ctrl_interface=/var/run/wpa_supplicant\nnetwork={\n\tssid=\"New net\"\n\tpsk=abcd\n\tpriority=1\n}\nnetwork={\n\tssid=\"Home\"\n\tpsk=0123\n\tpriority=0\n}\n"
        );
        // read back: both networks, the new one first
        let (_, again) = parse_config(&text);
        assert_eq!(again.iter().map(|b| b.ssid.as_str()).collect::<Vec<_>>(), ["New net", "Home"]);
        // the same network with a new password: no fallback block; an empty old file gets the control socket
        let same = Block { ssid: "Home".into(), lines: vec![ssid_line("Home"), "psk=ffff".into()] };
        let text = build_config(&[], &same, blocks.first());
        assert_eq!(text, "ctrl_interface=/var/run/wpa_supplicant\nnetwork={\n\tssid=\"Home\"\n\tpsk=ffff\n\tpriority=1\n}\n");
    }

    #[test]
    fn odd_names_are_written_in_hex() {
        assert_eq!(ssid_line("Home"), "ssid=\"Home\"");
        assert_eq!(ssid_line("Café"), "ssid=436166c3a9");
        assert_eq!(ssid_line("a\"b"), "ssid=612262");
        assert_eq!(ssid_value("436166c3a9").as_deref(), Some("Café"));
        assert_eq!(ssid_value("\"Home\"").as_deref(), Some("Home"));
    }
}
