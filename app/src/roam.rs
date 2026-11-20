//! Idle Wi-Fi roam check.
//!
//! wpa_supplicant changes the access point only when the link breaks. After
//! an outage the panel can stay on a farther access point for hours, although
//! the nearer one is back. So about every 30 minutes, and only while the panel
//! is idle (no voice run, no sound, no ringing timer), the panel scans and
//! moves to an access point of the same network that is at least 10 dB
//! stronger.
//!
//! The move is a one-shot `roam`: the network is never tied to one access
//! point, so a panel that is moved to another room keeps working.
//!
//! The thresholds are the same as in ESPHome's `wifi.roam`: no scan at -49 dBm
//! or better, and a move needs at least 10 dB more.
//!
//! A link can say "connected" and still carry nothing (seen after a move between
//! two access points: associated, but no traffic). So after every move, and
//! whenever the link to Home Assistant is down for a while, the panel checks that
//! the router answers; when it does not, it reconnects (reassociate, then a full
//! disconnect and connect).
//!
//! A strong signal is not always a good link: an access point (or its channel near
//! the panel) can lose most frames while its signal stays strong. So every few
//! minutes while idle the panel pings the router 20 times; when two such probes in a
//! row lose 15 % or more or take 150 ms or more on average, it moves to the best
//! other access point of the network and avoids the poor one for two hours. A move
//! is probed the same way, and a poor new access point means a move back. Nothing is
//! tied to an access point: after two hours every one counts again.
//!
//! Music counts as busy (a move drops the stream for a moment), so a panel that fell to a
//! farther access point while music played would stay there as long as the music lasts.
//! When the music stalls (its buffer runs dry, see `sendspin`) and nothing else is going on,
//! the check runs at once during the music, at most every 10 minutes: the stream breaks up
//! anyway, and the move still needs an access point at least 10 dB stronger.

use std::process::Command;
use std::time::Duration;

const IFACE: &str = "wlan0";
/// First check after the app starts.
const FIRST_CHECK: Duration = Duration::from_secs(5 * 60);
/// Between two checks.
const INTERVAL: Duration = Duration::from_secs(30 * 60);
/// When the panel is busy, not connected, or wpa_supplicant refuses the
/// scan: the next try.
const RETRY: Duration = Duration::from_secs(60);
/// Scan results are read this long after the scan request (2.4 and 5 GHz
/// take about 3–5 s).
const SCAN_WAIT: Duration = Duration::from_secs(8);
/// After a move: when the new link is checked.
const MOVE_WAIT: Duration = Duration::from_secs(15);
/// The link watch: how often it looks, how long Home Assistant must be away before it does, and the
/// least time between two reconnects.
const WATCH_EVERY: Duration = Duration::from_secs(10);
const WATCH_AWAY: Duration = Duration::from_secs(30);
const WATCH_GAP: Duration = Duration::from_secs(120);
/// No scan at this signal or better (dBm).
const GOOD_SIGNAL: i32 = -49;
/// A move needs an access point at least this much stronger (dB).
const MIN_IMPROVEMENT: i32 = 10;
/// The link probe: how often (while idle), and when a link is poor (loss %, average ms).
const PROBE_EVERY: Duration = Duration::from_secs(5 * 60);
const POOR_LOSS: u32 = 15;
const POOR_MS: f32 = 150.0;
/// A poor access point is left out of the moves for this long.
const AVOID_FOR: Duration = Duration::from_secs(2 * 60 * 60);
/// Leaving a poor access point: the other one must have at least this signal (dBm).
const LEAVE_MIN_SIGNAL: i32 = -75;
/// Poor access points (bssid, unix seconds until), kept over app restarts until the next boot.
const POOR_FILE: &str = "/tmp/panelos-poor-aps";
/// A stalling stream: how often the roam thread looks, and the least time between two checks it starts.
const STALL_POLL: Duration = Duration::from_secs(5);
const STALL_GAP: Duration = Duration::from_secs(10 * 60);

/// The current connection.
#[derive(Debug, Clone, PartialEq)]
struct Link {
    bssid: String,
    ssid: String,
}

/// One access point from the scan results.
#[derive(Debug, Clone, PartialEq)]
struct Ap {
    bssid: String,
    freq: u32,
    signal: i32,
    ssid: String,
}

#[derive(Debug, PartialEq)]
enum Decision {
    /// `current`: the current access point's signal; `best`: the strongest
    /// other access point of the same network, if any.
    Stay { current: i32, best: Option<i32> },
    Move { current: i32, to: Ap },
}

fn wpa_cli(args: &[&str]) -> Option<String> {
    let out = Command::new("wpa_cli").arg("-i").arg(IFACE).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn ok(reply: Option<String>) -> bool {
    reply.is_some_and(|r| r.trim() == "OK")
}

/// `wpa_cli status`: the current link, only when fully connected.
fn parse_status(text: &str) -> Option<Link> {
    let get = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('='));
    if get("wpa_state")? != "COMPLETED" {
        return None;
    }
    Some(Link { bssid: get("bssid")?.to_ascii_lowercase(), ssid: get("ssid")?.to_string() })
}

/// `wpa_cli signal_poll`: the link's signal in dBm.
fn parse_signal(text: &str) -> Option<i32> {
    text.lines().find_map(|l| l.strip_prefix("RSSI="))?.trim().parse().ok()
}

/// `wpa_cli scan_results`: a header line, then bssid, frequency, signal
/// level, flags and SSID, separated by tabs.
fn parse_scan(text: &str) -> Vec<Ap> {
    text.lines()
        .skip_while(|l| !l.starts_with("bssid"))
        .skip(1)
        .filter_map(|l| {
            let mut f = l.split('\t');
            let bssid = f.next()?.to_ascii_lowercase();
            let freq = f.next()?.parse().ok()?;
            let signal = f.next()?.parse().ok()?;
            let _flags = f.next()?;
            let ssid = f.next().unwrap_or_default().to_string();
            Some(Ap { bssid, freq, signal, ssid })
        })
        .collect()
}

/// The current access point is compared as the same scan saw it (the same
/// measurement as the others); `link_signal` is used when the scan missed it.
/// `poor`: access points measured poor lately (never chosen); `leaving`: the current link is poor, so
/// any other access point of the network with a usable signal will do.
fn decide(link: &Link, link_signal: i32, aps: &[Ap], poor: &[String], leaving: bool) -> Decision {
    let current = aps.iter().find(|a| a.bssid == link.bssid).map_or(link_signal, |a| a.signal);
    let best = aps
        .iter()
        .filter(|a| a.ssid == link.ssid && a.bssid != link.bssid && !poor.contains(&a.bssid))
        .max_by_key(|a| a.signal);
    match best {
        Some(b) if leaving && b.signal >= LEAVE_MIN_SIGNAL => Decision::Move { current, to: b.clone() },
        Some(b) if !leaving && b.signal >= current + MIN_IMPROVEMENT => Decision::Move { current, to: b.clone() },
        _ => Decision::Stay { current, best: best.map(|b| b.signal) },
    }
}

/// `ping` output: the loss in % and the average time in ms.
fn parse_ping(text: &str) -> Option<(u32, f32)> {
    let loss = text.lines().find(|l| l.contains("packet loss"))?;
    let pct: u32 = loss.split(',').find(|p| p.contains("packet loss"))?.trim().split('%').next()?.trim().parse().ok()?;
    let avg = text
        .lines()
        .find(|l| l.contains("min/avg/max"))
        .and_then(|l| l.split('=').nth(1)?.trim().split('/').nth(1)?.trim().parse().ok())
        .unwrap_or(f32::INFINITY);
    Some((pct, avg))
}

/// No answer at all is not "poor" (a router may not answer pings; a dead link is the link watch's job).
fn is_poor(loss: u32, avg_ms: f32) -> bool {
    loss < 100 && (loss >= POOR_LOSS || avg_ms >= POOR_MS)
}

/// 20 pings to the router, 0.2 s apart: (loss %, average ms); None without a router.
fn probe() -> Option<(u32, f32)> {
    let gw = router()?;
    let out = Command::new("ping").args(["-c", "20", "-i", "0.2", "-W", "1", &gw]).output().ok()?;
    parse_ping(&String::from_utf8_lossy(&out.stdout))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The access points measured poor that are still avoided.
fn poor_aps() -> Vec<String> {
    let now = now_secs();
    std::fs::read_to_string(POOR_FILE)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (b, until) = l.split_once(' ')?;
            (until.trim().parse::<u64>().ok()? > now).then(|| b.to_string())
        })
        .collect()
}

fn mark_poor(bssid: &str) {
    let now = now_secs();
    let mut lines: Vec<String> = std::fs::read_to_string(POOR_FILE)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.split_once(' ').is_some_and(|(b, u)| b != bssid && u.trim().parse::<u64>().is_ok_and(|u| u > now)))
        .map(str::to_string)
        .collect();
    lines.push(format!("{bssid} {}", now + AVOID_FOR.as_secs()));
    let _ = std::fs::write(POOR_FILE, lines.join("\n") + "\n");
}

/// One check. False when it could not run (not connected, scan refused, the
/// panel got busy): the caller tries again soon.
fn check(idle: &dyn Fn() -> bool, leaving: bool) -> bool {
    let Some(link) = wpa_cli(&["status"]).as_deref().and_then(parse_status) else { return false };
    let Some(signal) = wpa_cli(&["signal_poll"]).as_deref().and_then(parse_signal) else { return false };
    if signal >= GOOD_SIGNAL && !leaving {
        crate::connlog::note(&format!("roam check: {signal} dBm on {}, strong enough, no scan", link.bssid));
        return true;
    }
    // A scan for the network's own name: an access point that does not broadcast it (a hidden SSID) answers only a
    // probe that names it, and a plain scan lists it without a name. FAIL-BUSY while wpa_supplicant scans or connects
    // by itself.
    let hex: String = link.ssid.bytes().map(|b| format!("{b:02x}")).collect();
    if !ok(wpa_cli(&["scan", "ssid", &hex])) && !ok(wpa_cli(&["scan"])) {
        return false;
    }
    std::thread::sleep(SCAN_WAIT);
    let aps = wpa_cli(&["scan_results"]).map(|t| parse_scan(&t)).unwrap_or_default();
    let same: Vec<String> = aps.iter().filter(|a| a.ssid == link.ssid).map(|a| format!("{} {} MHz {} dBm", a.bssid, a.freq, a.signal)).collect();
    crate::connlog::note(&format!("roam check: access points of the network: {}", same.join(", ")));
    // wpa_supplicant may move by itself on the scan results (a weak link and a few dB more elsewhere)
    if let Some(now) = wpa_cli(&["status"]).as_deref().and_then(parse_status).filter(|now| now.bssid != link.bssid) {
        crate::connlog::note(&format!("roam check: wpa_supplicant moved from {} to {} after the scan", link.bssid, now.bssid));
        return true;
    }
    let poor = poor_aps();
    match decide(&link, signal, &aps, &poor, leaving) {
        Decision::Stay { current, best: Some(best) } => {
            crate::connlog::note(&format!("roam check: {current} dBm on {}, strongest other access point {best} dBm, staying", link.bssid))
        }
        Decision::Stay { current, best: None } => {
            crate::connlog::note(&format!("roam check: {current} dBm on {}, no other access point of the network, staying", link.bssid))
        }
        Decision::Move { current, to } => {
            // A voice run may have started during the scan.
            if !idle() {
                return false;
            }
            crate::connlog::note(&format!(
                "roam check: moving from {} ({current} dBm{}) to {} ({} dBm, {} MHz)",
                link.bssid,
                if leaving { ", poor link" } else { "" },
                to.bssid,
                to.signal,
                to.freq
            ));
            if !ok(wpa_cli(&["roam", &to.bssid])) {
                crate::connlog::note(&format!("roam check: wpa_supplicant refused the move"));
                return true;
            }
            std::thread::sleep(MOVE_WAIT);
            match wpa_cli(&["status"]).as_deref().and_then(parse_status) {
                Some(now) if now.bssid == to.bssid => crate::connlog::note(&format!("roam check: now on {}", now.bssid)),
                Some(now) => crate::connlog::note(&format!("roam check: the move did not happen, on {}", now.bssid)),
                None => crate::connlog::note(&format!("roam check: not connected after the move, wpa_supplicant reconnects by itself")),
            }
            // connected is not enough: the router must answer through the new link
            if !router_answers() {
                crate::connlog::note(&format!("roam check: no traffic after the move, reconnecting"));
                reconnect();
            } else if let Some((loss, avg)) = probe().filter(|&(l, a)| is_poor(l, a)) {
                // a strong but poor access point: back to the one before (unless that one was poor)
                crate::connlog::note(&format!("roam check: {} is poor ({loss} % lost, {avg:.0} ms), avoided for 2 h", to.bssid));
                mark_poor(&to.bssid);
                if !leaving && ok(wpa_cli(&["roam", &link.bssid])) {
                    crate::connlog::note(&format!("roam check: moving back to {}", link.bssid));
                    std::thread::sleep(MOVE_WAIT);
                    if !router_answers() {
                        reconnect();
                    }
                }
            }
        }
    }
    true
}

/// The default router of the Wi-Fi link (from /proc/net/route).
fn router() -> Option<String> {
    parse_router(&std::fs::read_to_string("/proc/net/route").ok()?)
}

fn parse_router(table: &str) -> Option<String> {
    table.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 || f[0] != IFACE || f[1] != "00000000" {
            return None;
        }
        let g = u32::from_str_radix(f[2], 16).ok()?.to_le_bytes();
        Some(format!("{}.{}.{}.{}", g[0], g[1], g[2], g[3]))
    })
}

/// The router answers a ping (three tries, 2 s each).
fn router_answers() -> bool {
    let Some(gw) = router() else { return false };
    (0..3).any(|_| Command::new("ping").args(["-c", "1", "-W", "2", &gw]).output().map(|o| o.status.success()).unwrap_or(false))
}

/// A link that carries nothing: reassociate; still nothing, a full disconnect and connect
/// (wpa_supplicant chooses the access point again, never a fixed one).
fn reconnect() {
    let _ = wpa_cli(&["reassociate"]);
    std::thread::sleep(MOVE_WAIT);
    if router_answers() {
        crate::connlog::note(&format!("link watch: traffic again after reassociating"));
        return;
    }
    crate::connlog::note(&format!("link watch: still no traffic, disconnecting and connecting again"));
    let _ = wpa_cli(&["disconnect"]);
    std::thread::sleep(Duration::from_secs(2));
    let _ = wpa_cli(&["reconnect"]);
    std::thread::sleep(MOVE_WAIT);
    crate::connlog::note(&format!("link watch: {}", if router_answers() { "traffic again" } else { "still no traffic" }));
}

/// Switches the Wi-Fi's power save off when it is on (also after a reconnect turned it on again).
fn power_save_off() {
    let on = Command::new("iw").args(["dev", IFACE, "get", "power_save"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(": on")).unwrap_or(false);
    if on {
        let done = Command::new("iw").args(["dev", IFACE, "set", "power_save", "off"]).status().map(|s| s.success()).unwrap_or(false);
        crate::connlog::note(&format!("link watch: Wi-Fi power save {}", if done { "switched off" } else { "could not be switched off" }));
    }
}

/// The link watch: while Home Assistant (`linked`) is away for a while and the Wi-Fi says connected,
/// the router must answer; when it does not, the link is made again.
fn watch(linked: impl Fn() -> bool) {
    let mut away_since: Option<std::time::Instant> = None;
    let mut last_fix: Option<std::time::Instant> = None;
    loop {
        // the Wi-Fi's power save off (the panel runs on mains): with it on, the RTL8723DS (rtw88)
        // missed beacons and answered pings in 0.25-3 s; off, in a few ms
        power_save_off();
        std::thread::sleep(WATCH_EVERY);
        if linked() {
            away_since = None;
            continue;
        }
        let since = *away_since.get_or_insert_with(std::time::Instant::now);
        let associated = wpa_cli(&["status"]).as_deref().and_then(parse_status).is_some();
        if since.elapsed() < WATCH_AWAY || !associated || last_fix.is_some_and(|t| t.elapsed() < WATCH_GAP) {
            continue;
        }
        if router_answers() {
            continue;
        }
        crate::connlog::note(&format!("link watch: Wi-Fi connected but the router does not answer, reconnecting"));
        last_fix = Some(std::time::Instant::now());
        reconnect();
    }
}

/// Runs the roam check thread (`idle`: whether the panel is idle; `stalled`: the music stalls and
/// nothing else is going on) and the link watch (`linked`: whether the link to Home Assistant is up).
pub fn spawn(
    idle: impl Fn() -> bool + Send + 'static,
    stalled: impl Fn() -> bool + Send + 'static,
    linked: impl Fn() -> bool + Send + 'static,
) {
    if let Err(e) = std::thread::Builder::new().name("link-watch".into()).spawn(move || watch(linked)) {
        crate::connlog::note(&format!("link watch not started: {e}"));
    }
    let started = std::thread::Builder::new().name("roam".into()).spawn(move || {
        // the first check waits a while after the start; a stalling stream does not
        let mut next_check = std::time::Instant::now() + FIRST_CHECK;
        let mut poor_probes = 0;
        let mut stall_check: Option<std::time::Instant> = None;
        loop {
            if std::time::Instant::now() >= next_check {
                let done = idle() && check(&idle, false);
                next_check = std::time::Instant::now() + if done { INTERVAL } else { RETRY };
            }
            // until the next probe or check; a stalling stream wakes it
            let wake = std::time::Instant::now()
                + PROBE_EVERY.min(next_check.saturating_duration_since(std::time::Instant::now())).max(RETRY);
            let mut stall = false;
            while std::time::Instant::now() < wake {
                std::thread::sleep(STALL_POLL.min(wake.saturating_duration_since(std::time::Instant::now())));
                if stalled() && stall_check.is_none_or(|t| t.elapsed() >= STALL_GAP) {
                    stall = true;
                    break;
                }
            }
            if stall {
                stall_check = Some(std::time::Instant::now());
                crate::connlog::note("roam check: the music stalls, checking during the music");
                check(&|| idle() || stalled(), false);
                continue;
            }
            if !idle() {
                continue;
            }
            // the link's quality: two poor probes in a row (30 s apart) and the panel leaves this access point
            match probe() {
                Some((loss, avg)) if is_poor(loss, avg) => {
                    poor_probes += 1;
                    crate::connlog::note(&format!("link probe: {loss} % lost, {avg:.0} ms average (poor, {poor_probes} in a row)"));
                    if poor_probes < 2 {
                        std::thread::sleep(Duration::from_secs(30));
                        if let Some((l, a)) = probe().filter(|&(l, a)| is_poor(l, a)) {
                            poor_probes += 1;
                            crate::connlog::note(&format!("link probe: {l} % lost, {a:.0} ms average (poor, {poor_probes} in a row)"));
                        } else {
                            poor_probes = 0;
                        }
                    }
                    if poor_probes >= 2 {
                        if let Some(link) = wpa_cli(&["status"]).as_deref().and_then(parse_status) {
                            mark_poor(&link.bssid);
                            crate::connlog::note(&format!("link probe: {} avoided for 2 h, looking for another access point", link.bssid));
                        }
                        poor_probes = 0;
                        check(&idle, true);
                        next_check = std::time::Instant::now() + INTERVAL;
                    }
                }
                _ => poor_probes = 0,
            }
        }
    });
    if let Err(e) = started {
        crate::connlog::note(&format!("roam check not started: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_from_route_table() {
        let table = concat!(
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\n",
            "wlan0\t000200C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\n",
            "wlan0\t00000000\t010200C0\t0003\t0\t0\t0\t00000000\n",
        );
        assert_eq!(parse_router(table).as_deref(), Some("192.0.2.1"));
        assert_eq!(parse_router("Iface\tDestination\tGateway\n"), None);
    }

    const STATUS: &str = "bssid=AA:00:00:00:00:01\nfreq=2462\nssid=Home\nid=0\nmode=station\nkey_mgmt=WPA2-PSK\nwpa_state=COMPLETED\nip_address=10.0.0.5\n";

    fn ap(bssid: &str, signal: i32, ssid: &str) -> Ap {
        Ap { bssid: bssid.into(), freq: 2437, signal, ssid: ssid.into() }
    }

    #[test]
    fn status_only_when_connected() {
        let link = parse_status(STATUS).unwrap();
        assert_eq!(link, Link { bssid: "aa:00:00:00:00:01".into(), ssid: "Home".into() });
        assert_eq!(parse_status(&STATUS.replace("COMPLETED", "SCANNING")), None);
    }

    #[test]
    fn signal_poll() {
        assert_eq!(parse_signal("RSSI=-67\nLINKSPEED=58\nNOISE=9999\nFREQUENCY=2462\n"), Some(-67));
        assert_eq!(parse_signal("FAIL\n"), None);
    }

    #[test]
    fn scan_results() {
        let text = "bssid / frequency / signal level / flags / ssid\n\
                    aa:00:00:00:00:01\t2462\t-67\t[WPA2-PSK-CCMP][ESS]\tHome\n\
                    AA:00:00:00:00:02\t2437\t-45\t[WPA2-PSK-CCMP][ESS]\tHome\n\
                    aa:00:00:00:00:03\t5180\t-80\t[ESS]\t\n";
        let aps = parse_scan(text);
        assert_eq!(aps.len(), 3);
        assert_eq!(aps[1], Ap { bssid: "aa:00:00:00:00:02".into(), freq: 2437, signal: -45, ssid: "Home".into() });
        assert_eq!(aps[2].ssid, "");
    }

    #[test]
    fn moves_only_for_ten_db_on_the_same_network() {
        let link = parse_status(STATUS).unwrap();
        let current = ap("aa:00:00:00:00:01", -67, "Home");
        // 9 dB more: stays.
        let aps = [current.clone(), ap("aa:00:00:00:00:02", -58, "Home")];
        assert_eq!(decide(&link, -66, &aps, &[], false), Decision::Stay { current: -67, best: Some(-58) });
        // 10 dB more: moves.
        let aps = [current.clone(), ap("aa:00:00:00:00:02", -57, "Home")];
        assert!(matches!(decide(&link, -66, &aps, &[], false), Decision::Move { current: -67, to } if to.bssid == "aa:00:00:00:00:02"));
        // A stronger access point of another network does not count.
        let aps = [current.clone(), ap("aa:00:00:00:00:03", -30, "Other")];
        assert_eq!(decide(&link, -66, &aps, &[], false), Decision::Stay { current: -67, best: None });
        // The scan missed the current access point: the link's signal counts.
        let aps = [ap("aa:00:00:00:00:02", -57, "Home")];
        assert_eq!(decide(&link, -66, &aps, &[], false), Decision::Stay { current: -66, best: Some(-57) });
    }

    #[test]
    fn poor_access_points() {
        let link = parse_status(STATUS).unwrap();
        let current = ap("aa:00:00:00:00:01", -44, "Home");
        let aps = [current.clone(), ap("aa:00:00:00:00:02", -61, "Home"), ap("aa:00:00:00:00:03", -30, "Home")];
        // a poor stronger one is never chosen
        let poor = vec!["aa:00:00:00:00:03".to_string()];
        assert_eq!(decide(&link, -44, &aps, &poor, false), Decision::Stay { current: -44, best: Some(-61) });
        // leaving a poor link: a weaker one will do, when usable
        assert!(matches!(decide(&link, -44, &aps, &poor, true), Decision::Move { to, .. } if to.bssid == "aa:00:00:00:00:02"));
        let aps = [current, ap("aa:00:00:00:00:02", -80, "Home")];
        assert_eq!(decide(&link, -44, &aps, &[], true), Decision::Stay { current: -44, best: Some(-80) });
    }

    #[test]
    fn ping_summary() {
        let busybox = "PING 192.0.2.1 (192.0.2.1): 56 data bytes\n\n--- 192.0.2.1 ping statistics ---\n20 packets transmitted, 14 packets received, 30% packet loss\nround-trip min/avg/max = 2.691/120.438/679.152 ms\n";
        assert_eq!(parse_ping(busybox), Some((30, 120.438)));
        assert!(is_poor(30, 12.0) && is_poor(0, 160.0) && !is_poor(5, 20.0));
        let none = "20 packets transmitted, 0 packets received, 100% packet loss\n";
        assert_eq!(parse_ping(none).map(|(l, _)| l), Some(100));
        assert!(!parse_ping(none).is_some_and(|(l, a)| is_poor(l, a)));
    }
}
