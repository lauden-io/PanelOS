//! Minimal mDNS (RFC 6762 / 6763) inside the app, no system daemon:
//! - announces the ESPHome API (`_esphomelib._tcp`), so Home Assistant
//!   discovers the panel and keeps its address up to date;
//! - browses for Home Assistant (`_home-assistant._tcp`) for the first-start
//!   setup screen.
//!
//! One UDP socket on port 5353 joined to 224.0.0.251. Answers queries for its
//! own names, sends two announcements after start and after an address change.
//! mDNS does not cross routed networks (VLANs) unless the router repeats it.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const PORT: u16 = 5353;
const T_A: u16 = 1;
const T_PTR: u16 = 12;
const T_TXT: u16 = 16;
const T_SRV: u16 = 33;
const T_ANY: u16 = 255;
const CLASS_IN: u16 = 1;
/// Class bit: unique record (cache flush) in answers, unicast response wanted in questions.
const TOP_BIT: u16 = 0x8000;
const HA_SERVICE: &str = "_home-assistant._tcp.local";
const ENUM_SERVICES: &str = "_services._dns-sd._udp.local";

/// A service this panel announces.
#[derive(Clone)]
pub struct Service {
    /// Instance and host name, e.g. "nspanel-pro".
    pub name: String,
    /// Service type, e.g. "_esphomelib._tcp".
    pub kind: String,
    pub port: u16,
    pub txt: Vec<String>,
}

/// A Home Assistant found on the network.
#[derive(Clone, Debug, PartialEq)]
pub struct HomeAssistant {
    /// The instance name (Home Assistant's location name).
    pub name: String,
    /// Base URL, e.g. "http://192.168.1.10:8123".
    pub url: String,
}

/// Shared with the app: switch browsing on, read what was found.
#[derive(Default)]
pub struct Browser {
    pub active: AtomicBool,
    pub found: Mutex<Vec<HomeAssistant>>,
}

pub fn spawn(services: Vec<Service>, browser: Arc<Browser>) {
    std::thread::spawn(move || loop {
        // Waits for an address, runs until the address changes or the socket fails.
        if let Some(ip) = own_ipv4() {
            match run(&services, &browser, ip) {
                Ok(()) => continue,
                Err(e) => eprintln!("nspanel-app: mdns: {e}"),
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    });
}

/// The panel's IPv4 address on Wi-Fi (the source address towards the group).
fn own_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(SocketAddrV4::new(GROUP, PORT)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

/// Networks directly attached to the panel: routes without a gateway in
/// /proc/net/route (addresses in network byte order, printed as numbers).
fn attached_networks() -> Vec<(u32, u32)> {
    let text = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let addr = |i: usize| {
                f.get(i).and_then(|v| u32::from_str_radix(v, 16).ok()).map(|v| u32::from_be_bytes(v.to_ne_bytes()))
            };
            let (dest, gateway, mask) = (addr(1)?, addr(2)?, addr(7)?);
            (gateway == 0 && mask != 0).then_some((dest, mask))
        })
        .collect()
}

fn on_link(src: Ipv4Addr) -> bool {
    let src = u32::from(src);
    attached_networks().iter().any(|&(net, mask)| src & mask == net & mask)
}

fn run(services: &[Service], browser: &Browser, ip: Ipv4Addr) -> Result<(), String> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT)).map_err(|e| format!("bind: {e}"))?;
    socket.join_multicast_v4(&GROUP, &ip).map_err(|e| format!("join: {e}"))?;
    let _ = socket.set_multicast_loop_v4(false);
    let _ = socket.set_multicast_ttl_v4(255);
    socket.set_read_timeout(Some(Duration::from_millis(500))).map_err(|e| e.to_string())?;
    let group = SocketAddr::V4(SocketAddrV4::new(GROUP, PORT));
    for service in services {
        eprintln!("nspanel-app: mdns: announcing {}.{}.local on {ip}", service.name, service.kind);
    }

    let all_names: Vec<Names> = services.iter().map(Names::new).collect();
    let mut announce_at = vec![Instant::now(), Instant::now() + Duration::from_secs(1)];
    let mut next_query: Option<Instant> = None;
    let mut queries_sent = 0u32;
    let mut address_check = Instant::now();
    // Home Assistant instances: instance name → partial record data.
    let mut ha: HashMap<String, Partial> = HashMap::new();
    let mut buf = [0u8; 9000];
    loop {
        let now = Instant::now();
        announce_at.retain(|t| {
            if *t <= now {
                for names in &all_names {
                    let _ = socket.send_to(&names.response(0, None, &[], ip, true), group);
                }
                false
            } else {
                true
            }
        });
        if browser.active.load(Ordering::Relaxed) {
            if next_query.is_none_or(|t| t <= now) {
                let _ = socket.send_to(&query(HA_SERVICE, T_PTR), group);
                queries_sent += 1;
                // Quick first tries, then every 10 s.
                next_query = Some(now + Duration::from_secs(if queries_sent < 4 { 1 } else { 10 }));
            }
        } else {
            next_query = None;
            queries_sent = 0;
        }
        if address_check.elapsed() > Duration::from_secs(30) {
            address_check = Instant::now();
            if own_ipv4() != Some(ip) {
                return Ok(());
            }
        }

        let (len, from) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(e) => return Err(format!("recv: {e}")),
        };
        let Some(msg) = Message::parse(&buf[..len]) else { continue };
        if msg.flags & 0x8000 == 0 {
            // A query: answer what is ours, only to the panel's own network
            // (mDNS is link-local; no answers to routed or spoofed senders,
            // so the panel cannot be used to reflect traffic).
            let SocketAddr::V4(src) = from else { continue };
            if !on_link(*src.ip()) {
                continue;
            }
            let wanted: Vec<(String, u16)> = msg.questions.iter().map(|q| (q.0.to_lowercase(), q.1)).collect();
            let legacy = from.port() != PORT;
            let unicast = legacy || msg.questions.iter().any(|q| q.2 & TOP_BIT != 0);
            for names in &all_names {
                let reply = names.response(msg.id, legacy.then_some(&msg.questions), &wanted, ip, false);
                if reply.len() > 12 {
                    let to = if unicast { from } else { group };
                    let _ = socket.send_to(&reply, to);
                }
            }
        } else if browser.active.load(Ordering::Relaxed) {
            collect_home_assistant(&msg, &mut ha);
            let mut found: Vec<HomeAssistant> = ha.values().filter_map(Partial::complete).collect();
            found.sort_by(|a, b| a.name.cmp(&b.name));
            found.dedup_by(|a, b| a.url == b.url);
            *browser.found.lock().unwrap() = found;
        }
    }
}

/// The records of the announced service.
struct Names {
    service: Service,
    kind: String,     // "_esphomelib._tcp.local"
    instance: String, // "nspanel-pro._esphomelib._tcp.local"
    host: String,     // "nspanel-pro.local"
}

impl Names {
    fn new(service: &Service) -> Self {
        let kind = format!("{}.local", service.kind);
        Names {
            instance: format!("{}.{kind}", service.name),
            host: format!("{}.local", service.name),
            kind,
            service: service.clone(),
        }
    }

    /// A response with the records the questions ask for (all of them for an
    /// announcement). Empty (header only) when nothing matches.
    fn response(&self, id: u16, echo: Option<&Vec<(String, u16, u16)>>, wanted: &[(String, u16)], ip: Ipv4Addr, all: bool) -> Vec<u8> {
        let asks = |name: &str, t: u16| all || wanted.iter().any(|(n, qt)| n == &name.to_lowercase() && (*qt == t || *qt == T_ANY));
        let mut answers: Vec<Vec<u8>> = Vec::new();
        let mut extra: Vec<Vec<u8>> = Vec::new();
        let ptr = || record(&self.kind, T_PTR, CLASS_IN, 4500, &name_bytes(&self.instance));
        let srv = || {
            let mut d = vec![0, 0, 0, 0];
            d.extend(self.service.port.to_be_bytes());
            d.extend(name_bytes(&self.host));
            record(&self.instance, T_SRV, CLASS_IN | TOP_BIT, 120, &d)
        };
        let txt = || {
            let mut d = Vec::new();
            for t in &self.service.txt {
                d.push(t.len().min(255) as u8);
                d.extend(&t.as_bytes()[..t.len().min(255)]);
            }
            record(&self.instance, T_TXT, CLASS_IN | TOP_BIT, 4500, &d)
        };
        let a = || record(&self.host, T_A, CLASS_IN | TOP_BIT, 120, &ip.octets());

        if asks(&self.kind, T_PTR) {
            answers.push(ptr());
            extra.extend([srv(), txt(), a()]);
        }
        if !all && asks(ENUM_SERVICES, T_PTR) {
            answers.push(record(ENUM_SERVICES, T_PTR, CLASS_IN, 4500, &name_bytes(&self.kind)));
        }
        if !all && asks(&self.instance, T_SRV) {
            answers.push(srv());
            extra.push(a());
        }
        if !all && asks(&self.instance, T_TXT) {
            answers.push(txt());
        }
        if !all && asks(&self.host, T_A) {
            answers.push(a());
        }
        if answers.is_empty() {
            return vec![0; 12];
        }
        extra.retain(|e| !answers.contains(e));
        extra.dedup();

        let mut out = Vec::new();
        out.extend(id.to_be_bytes());
        out.extend(0x8400u16.to_be_bytes()); // response, authoritative
        let questions = echo.map(|q| q.len()).unwrap_or(0) as u16;
        out.extend(questions.to_be_bytes());
        out.extend((answers.len() as u16).to_be_bytes());
        out.extend(0u16.to_be_bytes());
        out.extend((extra.len() as u16).to_be_bytes());
        for (name, t, class) in echo.into_iter().flatten() {
            out.extend(name_bytes(name));
            out.extend(t.to_be_bytes());
            out.extend((class & !TOP_BIT).to_be_bytes());
        }
        for r in answers.iter().chain(extra.iter()) {
            out.extend(r);
        }
        out
    }
}

fn name_bytes(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.trim_end_matches('.').split('.').filter(|l| !l.is_empty()) {
        let l = &label.as_bytes()[..label.len().min(63)];
        out.push(l.len() as u8);
        out.extend(l);
    }
    out.push(0);
    out
}

fn record(name: &str, t: u16, class: u16, ttl: u32, data: &[u8]) -> Vec<u8> {
    let mut out = name_bytes(name);
    out.extend(t.to_be_bytes());
    out.extend(class.to_be_bytes());
    out.extend(ttl.to_be_bytes());
    out.extend((data.len() as u16).to_be_bytes());
    out.extend(data);
    out
}

fn query(name: &str, t: u16) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    out.extend(name_bytes(name));
    out.extend(t.to_be_bytes());
    out.extend(CLASS_IN.to_be_bytes());
    out
}

/// A parsed message: questions (name, type, class) and all resource records.
struct Message<'a> {
    id: u16,
    flags: u16,
    questions: Vec<(String, u16, u16)>,
    records: Vec<Record<'a>>,
}

struct Record<'a> {
    name: String,
    kind: u16,
    /// The whole packet (for compressed names inside the data) and the data range.
    packet: &'a [u8],
    data: std::ops::Range<usize>,
}

impl<'a> Message<'a> {
    fn parse(p: &'a [u8]) -> Option<Self> {
        let u16_at = |i: usize| p.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
        let id = u16_at(0)?;
        let flags = u16_at(2)?;
        let (qd, an, ns, ar) = (u16_at(4)?, u16_at(6)?, u16_at(8)?, u16_at(10)?);
        let mut pos = 12;
        let mut questions = Vec::new();
        for _ in 0..qd {
            let (name, next) = read_name(p, pos)?;
            questions.push((name, u16_at(next)?, u16_at(next + 2)?));
            pos = next + 4;
        }
        let mut records = Vec::new();
        for _ in 0..(an as usize + ns as usize + ar as usize) {
            let (name, next) = read_name(p, pos)?;
            let kind = u16_at(next)?;
            let len = u16_at(next + 8)? as usize;
            let start = next + 10;
            if start + len > p.len() {
                return None;
            }
            records.push(Record { name, kind, packet: p, data: start..start + len });
            pos = start + len;
        }
        Some(Message { id, flags, questions, records })
    }
}

/// A (possibly compressed) name at `pos`; returns it and the position after it.
fn read_name(p: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut end: Option<usize> = None;
    for _ in 0..64 {
        let len = *p.get(pos)? as usize;
        if len == 0 {
            return Some((labels.join("."), end.unwrap_or(pos + 1)));
        }
        if len & 0xC0 == 0xC0 {
            let target = ((len & 0x3F) << 8) | *p.get(pos + 1)? as usize;
            end.get_or_insert(pos + 2);
            pos = target;
            continue;
        }
        labels.push(String::from_utf8_lossy(p.get(pos + 1..pos + 1 + len)?).into_owned());
        pos += 1 + len;
    }
    None
}

/// What is known about one Home Assistant instance so far.
#[derive(Default)]
struct Partial {
    name: String,
    url: Option<String>,
    target: Option<(String, u16)>,
    address: Option<Ipv4Addr>,
}

impl Partial {
    fn complete(&self) -> Option<HomeAssistant> {
        let url = self.url.clone().or_else(|| {
            let (_, port) = self.target.as_ref()?;
            Some(format!("http://{}:{port}", self.address?))
        })?;
        Some(HomeAssistant { name: self.name.clone(), url })
    }
}

fn collect_home_assistant(msg: &Message, ha: &mut HashMap<String, Partial>) {
    let suffix = format!(".{HA_SERVICE}");
    // Host name → address, for SRV targets.
    let mut hosts: HashMap<String, Ipv4Addr> = HashMap::new();
    for r in &msg.records {
        let d = &r.packet[r.data.clone()];
        match r.kind {
            T_PTR if r.name.eq_ignore_ascii_case(HA_SERVICE) => {
                if let Some((instance, _)) = read_name(r.packet, r.data.start) {
                    let name = instance.strip_suffix(&suffix).unwrap_or(&instance).to_string();
                    ha.entry(instance.to_lowercase()).or_default().name = name;
                }
            }
            T_SRV if r.name.to_lowercase().ends_with(&suffix) && d.len() > 6 => {
                if let Some((target, _)) = read_name(r.packet, r.data.start + 6) {
                    let port = u16::from_be_bytes([d[4], d[5]]);
                    ha.entry(r.name.to_lowercase()).or_default().target = Some((target.to_lowercase(), port));
                }
            }
            T_TXT if r.name.to_lowercase().ends_with(&suffix) => {
                let mut i = 0;
                let mut kv: HashMap<String, String> = HashMap::new();
                while i < d.len() {
                    let l = d[i] as usize;
                    let s = String::from_utf8_lossy(d.get(i + 1..(i + 1 + l).min(d.len())).unwrap_or(&[])).into_owned();
                    if let Some((k, v)) = s.split_once('=') {
                        kv.insert(k.to_string(), v.to_string());
                    }
                    i += 1 + l;
                }
                let entry = ha.entry(r.name.to_lowercase()).or_default();
                if let Some(n) = kv.get("location_name").filter(|n| !n.is_empty()) {
                    entry.name = n.clone();
                }
                // internal_url first: base_url may be an external (cloud) address.
                if let Some(u) = ["internal_url", "base_url"].iter().filter_map(|k| kv.get(*k)).find(|u| u.starts_with("http")) {
                    entry.url = Some(u.trim_end_matches('/').to_string());
                }
            }
            T_A if d.len() == 4 => {
                hosts.insert(r.name.to_lowercase(), Ipv4Addr::new(d[0], d[1], d[2], d[3]));
            }
            _ => {}
        }
    }
    for p in ha.values_mut() {
        if let Some((target, _)) = &p.target {
            if let Some(ip) = hosts.get(target) {
                p.address = Some(*ip);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        let mut p = vec![0u8; 12];
        p.extend(name_bytes("nspanel-pro._esphomelib._tcp.local"));
        let (n, end) = read_name(&p, 12).unwrap();
        assert_eq!(n, "nspanel-pro._esphomelib._tcp.local");
        assert_eq!(end, p.len());
    }

    #[test]
    fn finds_home_assistant() {
        // A response as Home Assistant's zeroconf sends it: PTR, SRV, TXT, A.
        let instance = "Home._home-assistant._tcp.local";
        let mut srv = vec![0, 0, 0, 0];
        srv.extend(8123u16.to_be_bytes());
        srv.extend(name_bytes("homeassistant.local"));
        let mut txt = Vec::new();
        for t in ["location_name=Home", "internal_url=http://192.168.1.10:8123", "base_url=http://192.168.1.10:8123/"] {
            txt.push(t.len() as u8);
            txt.extend(t.as_bytes());
        }
        let records = [
            record(HA_SERVICE, T_PTR, CLASS_IN, 4500, &name_bytes(instance)),
            record(instance, T_SRV, CLASS_IN | TOP_BIT, 120, &srv),
            record(instance, T_TXT, CLASS_IN | TOP_BIT, 4500, &txt),
            record("homeassistant.local", T_A, CLASS_IN | TOP_BIT, 120, &[192, 168, 1, 10]),
        ];
        let mut p = vec![0, 0, 0x84, 0, 0, 0, 0, 4, 0, 0, 0, 0];
        for r in &records {
            p.extend(r);
        }
        let msg = Message::parse(&p).unwrap();
        let mut ha = HashMap::new();
        collect_home_assistant(&msg, &mut ha);
        let found: Vec<HomeAssistant> = ha.values().filter_map(Partial::complete).collect();
        assert_eq!(found, vec![HomeAssistant { name: "Home".into(), url: "http://192.168.1.10:8123".into() }]);

        // Without URLs in TXT: from SRV and A.
        let records = [
            record(HA_SERVICE, T_PTR, CLASS_IN, 4500, &name_bytes(instance)),
            record(instance, T_SRV, CLASS_IN | TOP_BIT, 120, &srv),
            record("homeassistant.local", T_A, CLASS_IN | TOP_BIT, 120, &[192, 168, 1, 10]),
        ];
        let mut p = vec![0, 0, 0x84, 0, 0, 0, 0, 3, 0, 0, 0, 0];
        for r in &records {
            p.extend(r);
        }
        let msg = Message::parse(&p).unwrap();
        let mut ha = HashMap::new();
        collect_home_assistant(&msg, &mut ha);
        let found: Vec<HomeAssistant> = ha.values().filter_map(Partial::complete).collect();
        assert_eq!(found, vec![HomeAssistant { name: "Home".into(), url: "http://192.168.1.10:8123".into() }]);
    }

    #[test]
    fn answers_ptr_query() {
        let service = Service { name: "nspanel-pro".into(), kind: "_esphomelib._tcp".into(), port: 6053, txt: vec!["mac=aabbccddeeff".into()] };
        let names = Names::new(&service);
        let q = query("_esphomelib._tcp.local", T_PTR);
        let msg = Message::parse(&q).unwrap();
        let wanted: Vec<(String, u16)> = msg.questions.iter().map(|q| (q.0.clone(), q.1)).collect();
        let reply = names.response(0, None, &wanted, Ipv4Addr::new(192, 168, 5, 100), false);
        let parsed = Message::parse(&reply).unwrap();
        let kinds: Vec<u16> = parsed.records.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec![T_PTR, T_SRV, T_TXT, T_A]);
    }
}
