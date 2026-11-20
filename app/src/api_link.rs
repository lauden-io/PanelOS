//! Transport of the ESPHome native API: plaintext or Noise
//! (`Noise_NNpsk0_25519_ChaChaPoly_SHA256`, as on ESPHome devices), the
//! panel's API key and the hosts that may connect.
//!
//! Key: `/mnt/persist/nspanel/api_key` (base64 of 32 bytes). Without a key
//! the panel takes plaintext and Noise with an all-zero key, so Home
//! Assistant can hand it a key (ESPHome's dynamic encryption key: the panel
//! reports `api_encryption_provisionable`); with a key, only Noise with that
//! key. Deleting the file (or a key-verified session sending an empty key)
//! goes back to the keyless state.
//!
//! Plaintext frame: 0x00, varint payload length, varint type, payload.
//! Noise frame: 0x01, 16-bit length, encrypted (16-bit type, 16-bit length,
//! payload).

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use prost::Message;

pub const KEY_FILE: &str = "/mnt/persist/nspanel/api_key";
const NOISE_PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_SHA256";
/// Largest plaintext frame taken (the largest real messages are a few KB).
const MAX_PLAIN_FRAME: u64 = 256 * 1024;
/// Noise payload limit: 65535 minus the 16-byte tag and the 4-byte header.
const MAX_NOISE_PAYLOAD: usize = 65535 - 16 - 4;
/// Time for the handshake and for every write.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

pub type Key = [u8; 32];

/// Raised with every key change: sessions opened before it end (as on
/// ESPHome), so Home Assistant reconnects with the new key.
static KEY_GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn key_generation() -> u64 {
    KEY_GENERATION.load(Ordering::SeqCst)
}

pub fn load_key() -> Option<Key> {
    decode_key(std::fs::read_to_string(KEY_FILE).ok()?.trim())
}

/// A base64 key of 32 bytes; an all-zero key is the provisioning key, never a real one.
pub fn decode_key(b64: &str) -> Option<Key> {
    let key: Key = base64_decode(b64)?.try_into().ok()?;
    (key != [0u8; 32]).then_some(key)
}

fn save_key(key: &Key) -> io::Result<()> {
    let tmp = format!("{KEY_FILE}.tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(base64_encode(key).as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, KEY_FILE)
}

fn clear_key() -> io::Result<()> {
    match std::fs::remove_file(KEY_FILE) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// NoiseEncryptionSetKeyRequest: a new key (base64 text) or an empty one to
/// clear it. Allowed while the panel has no key, or in a session that proved
/// the current key. True when stored; the session must end afterwards.
pub fn set_key(conn: &Conn, key_b64: &[u8]) -> bool {
    if !(conn.keyed || load_key().is_none()) {
        eprintln!("nspanel-app: API key change refused (session without the current key)");
        return false;
    }
    let result = if key_b64.is_empty() {
        clear_key().map(|_| "cleared")
    } else {
        let Some(key) = std::str::from_utf8(key_b64).ok().and_then(decode_key) else {
            eprintln!("nspanel-app: API key refused (not 32 bytes of base64, or all zero)");
            return false;
        };
        save_key(&key).map(|_| "stored, encryption on")
    };
    match result {
        Ok(what) => {
            KEY_GENERATION.fetch_add(1, Ordering::SeqCst);
            eprintln!("nspanel-app: API key {what}");
            true
        }
        Err(e) => {
            eprintln!("nspanel-app: API key: {e}");
            false
        }
    }
}

/// Hosts that may use the API: Home Assistant's address (from its URL) and
/// the addresses in `api_allow` of app.json.
pub fn allowed_peers(ha_host_port: &str, extra: &[String]) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = ha_host_port.to_socket_addrs().map(|a| a.map(|s| s.ip()).collect()).unwrap_or_default();
    out.extend(extra.iter().filter_map(|s| s.trim().parse::<IpAddr>().ok()));
    out
}

/// IPv4 written as IPv6 (::ffff:a.b.c.d) compared as IPv4.
pub fn same_host(a: IpAddr, b: IpAddr) -> bool {
    let canon = |ip: IpAddr| match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    canon(a) == canon(b)
}

enum Mode {
    Plain,
    Noise(Box<snow::TransportState>),
}

pub struct Conn {
    stream: TcpStream,
    mode: Mode,
    /// The session proved the panel's key (Noise with the stored key).
    pub keyed: bool,
    /// key_generation() when the session started.
    pub generation: u64,
    rx: Vec<u8>,
    /// Frames read but handed back (see `push_front`).
    pending: VecDeque<(u32, Vec<u8>)>,
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn read_noise_frame(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut header = [0u8; 3];
    stream.read_exact(&mut header)?;
    if header[0] != 0x01 {
        return Err(invalid(format!("frame indicator {:#04x} in a Noise session", header[0])));
    }
    let mut data = vec![0u8; u16::from_be_bytes([header[1], header[2]]) as usize];
    stream.read_exact(&mut data)?;
    Ok(data)
}

fn write_noise_frame(stream: &mut TcpStream, data: &[u8]) -> io::Result<()> {
    let len = u16::try_from(data.len()).map_err(|_| invalid("Noise frame too long"))?;
    let mut frame = Vec::with_capacity(3 + data.len());
    frame.push(0x01);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(data);
    stream.write_all(&frame)
}

/// Chosen protocol (1), device name and MAC (lower case, no colons), each
/// ending with a zero byte: the client checks them before its handshake.
fn server_hello(name: &str, mac: &str) -> Vec<u8> {
    let mut out = vec![0x01];
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.extend_from_slice(mac.as_bytes());
    out.push(0);
    out
}

/// Handshake failure as ESPHome reports it (the client shows "invalid
/// encryption key" for "Handshake MAC failure").
fn reject(stream: &mut TcpStream, reason: &str) {
    let mut data = vec![0x01];
    data.extend_from_slice(reason.as_bytes());
    let _ = write_noise_frame(stream, &data);
}

fn noise_err(e: snow::Error) -> io::Error {
    invalid(format!("Noise: {e}"))
}

impl Conn {
    /// Sets up a new connection: plaintext (only while the panel has no key)
    /// or the Noise handshake with the panel's key (all zeros while it has none).
    pub fn accept(stream: TcpStream, name: &str, mac: &str) -> io::Result<Conn> {
        Self::accept_with(stream, name, mac, load_key())
    }

    /// `accept` with a given key instead of the stored one.
    fn accept_with(mut stream: TcpStream, name: &str, mac: &str, key: Option<Key>) -> io::Result<Conn> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let generation = key_generation();
        let mut first = [0u8; 1];
        if stream.peek(&mut first)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed before the first frame"));
        }
        match first[0] {
            0x00 if key.is_none() => {
                Ok(Conn { stream, mode: Mode::Plain, keyed: false, generation, rx: Vec::new(), pending: VecDeque::new() })
            }
            0x00 => {
                // A frame starting with 0x01 tells the client that encryption is required.
                let _ = write_noise_frame(&mut stream, &server_hello(name, mac));
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "plaintext refused: the API is encrypted"))
            }
            0x01 => {
                let hello = read_noise_frame(&mut stream)?;
                let mut prologue = b"NoiseAPIInit".to_vec();
                prologue.extend_from_slice(&(hello.len() as u16).to_be_bytes());
                prologue.extend_from_slice(&hello);
                write_noise_frame(&mut stream, &server_hello(name, mac))?;
                let psk = key.unwrap_or([0u8; 32]);
                let mut hs = snow::Builder::new(NOISE_PARAMS.parse().map_err(noise_err)?)
                    .psk(0, &psk)
                    .prologue(&prologue)
                    .build_responder()
                    .map_err(noise_err)?;
                let msg = read_noise_frame(&mut stream)?;
                if msg.first() != Some(&0) {
                    reject(&mut stream, "Bad handshake error byte");
                    return Err(invalid("bad handshake frame"));
                }
                let mut scratch = [0u8; 256];
                if hs.read_message(&msg[1..], &mut scratch).is_err() {
                    reject(&mut stream, "Handshake MAC failure");
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        if key.is_some() { "Noise handshake failed (wrong key)" } else { "Noise handshake failed" },
                    ));
                }
                let mut out = [0u8; 256];
                let n = hs.write_message(&[], &mut out[1..]).map_err(noise_err)?;
                out[0] = 0;
                write_noise_frame(&mut stream, &out[..n + 1])?;
                let transport = hs.into_transport_mode().map_err(noise_err)?;
                Ok(Conn {
                    stream,
                    mode: Mode::Noise(Box::new(transport)),
                    keyed: key.is_some(),
                    generation,
                    rx: Vec::new(),
                    pending: VecDeque::new(),
                })
            }
            b => Err(invalid(format!("unknown frame indicator {b:#04x}"))),
        }
    }

    pub fn encrypted(&self) -> bool {
        matches!(self.mode, Mode::Noise(_))
    }

    /// The panel's key changed after this session started.
    pub fn outdated(&self) -> bool {
        key_generation() != self.generation
    }

    pub fn set_read_timeout(&self, t: Duration) -> io::Result<()> {
        self.stream.set_read_timeout(Some(t))
    }

    pub fn peer(&self) -> String {
        self.stream.peer_addr().map(|a| a.to_string()).unwrap_or_default()
    }

    pub fn send<M: Message>(&mut self, msg_type: u32, msg: &M) -> io::Result<()> {
        let payload = msg.encode_to_vec();
        match &mut self.mode {
            Mode::Plain => {
                let mut frame = Vec::with_capacity(payload.len() + 8);
                frame.push(0);
                encode_varint(payload.len() as u64, &mut frame);
                encode_varint(msg_type as u64, &mut frame);
                frame.extend_from_slice(&payload);
                self.stream.write_all(&frame)
            }
            Mode::Noise(t) => {
                if payload.len() > MAX_NOISE_PAYLOAD {
                    return Err(invalid(format!("message type {msg_type} too long for Noise ({} bytes)", payload.len())));
                }
                let mut plain = Vec::with_capacity(payload.len() + 4);
                plain.extend_from_slice(&(msg_type as u16).to_be_bytes());
                plain.extend_from_slice(&(payload.len() as u16).to_be_bytes());
                plain.extend_from_slice(&payload);
                let mut cipher = vec![0u8; plain.len() + 16];
                let n = t.write_message(&plain, &mut cipher).map_err(noise_err)?;
                cipher.truncate(n);
                write_noise_frame(&mut self.stream, &cipher)
            }
        }
    }

    /// Reads what has arrived; Ok(false) when the read timed out, an error
    /// at the end of the connection.
    pub fn read_some(&mut self) -> io::Result<bool> {
        let mut chunk = [0u8; 4096];
        match self.stream.read(&mut chunk) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed by the client")),
            Ok(n) => {
                self.rx.extend_from_slice(&chunk[..n]);
                if self.rx.len() as u64 > 2 * MAX_PLAIN_FRAME {
                    return Err(invalid("receive buffer over the limit"));
                }
                Ok(true)
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Hands a frame back, to be taken again first.
    pub fn push_front(&mut self, frame: (u32, Vec<u8>)) {
        self.pending.push_front(frame);
    }

    /// One complete frame (type, payload) off the front of the buffer.
    pub fn take_frame(&mut self) -> io::Result<Option<(u32, Vec<u8>)>> {
        if let Some(f) = self.pending.pop_front() {
            return Ok(Some(f));
        }
        match &mut self.mode {
            Mode::Plain => take_plain(&mut self.rx),
            Mode::Noise(t) => {
                if self.rx.len() < 3 {
                    return Ok(None);
                }
                if self.rx[0] != 0x01 {
                    return Err(invalid("bad frame indicator in a Noise session"));
                }
                let len = u16::from_be_bytes([self.rx[1], self.rx[2]]) as usize;
                if self.rx.len() < 3 + len {
                    return Ok(None);
                }
                let mut plain = vec![0u8; len];
                let n = t.read_message(&self.rx[3..3 + len], &mut plain).map_err(noise_err)?;
                self.rx.drain(..3 + len);
                if n < 4 {
                    return Err(invalid("Noise frame without a header"));
                }
                let kind = u16::from_be_bytes([plain[0], plain[1]]) as u32;
                let dlen = u16::from_be_bytes([plain[2], plain[3]]) as usize;
                if dlen != n - 4 {
                    return Err(invalid("Noise frame length mismatch"));
                }
                Ok(Some((kind, plain[4..n].to_vec())))
            }
        }
    }
}

fn encode_varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn varint(b: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*pos)?;
        *pos += 1;
        v |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn take_plain(buf: &mut Vec<u8>) -> io::Result<Option<(u32, Vec<u8>)>> {
    if buf.is_empty() {
        return Ok(None);
    }
    if buf[0] != 0 {
        return Err(invalid("bad frame indicator in a plaintext session"));
    }
    let mut pos = 1;
    let Some(len) = varint(buf, &mut pos) else { return Ok(None) };
    if len > MAX_PLAIN_FRAME {
        return Err(invalid(format!("frame of {len} bytes over the limit")));
    }
    let Some(kind) = varint(buf, &mut pos) else { return Ok(None) };
    let kind = u32::try_from(kind).map_err(|_| invalid("message type out of range"))?;
    let end = pos + len as usize; // both bounded: no overflow
    if buf.len() < end {
        return Ok(None);
    }
    let payload = buf[pos..end].to_vec();
    buf.drain(..end);
    Ok(Some((kind, payload)))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let v = B64.iter().position(|&b| b == c)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        for len in 0..40 {
            let data: Vec<u8> = (0..len as u8).map(|i| i.wrapping_mul(37).wrapping_add(11)).collect();
            assert_eq!(base64_decode(&base64_encode(&data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(decode_key(&base64_encode(&[0u8; 32])), None);
        assert!(decode_key(&base64_encode(&[7u8; 32])).is_some());
        assert_eq!(decode_key(&base64_encode(&[7u8; 31])), None);
    }

    #[test]
    fn plain_frames_are_bounded() {
        // A length that would wrap the index must be refused, not panic.
        let mut buf = vec![0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01, 0x01];
        assert!(take_plain(&mut buf).is_err());
        let mut ok = vec![0x00, 0x02, 0x07, 0xaa, 0xbb];
        assert_eq!(take_plain(&mut ok).unwrap(), Some((7, vec![0xaa, 0xbb])));
        let mut partial = vec![0x00, 0x05, 0x07, 0xaa];
        assert_eq!(take_plain(&mut partial).unwrap(), None);
    }

    /// A client as aioesphomeapi does it, with `client_key`, against the panel's side holding `server_key`: `None`
    /// when the panel refused the handshake, otherwise whether the session counts as keyed.
    fn noise_session(server_key: Option<Key>, client_key: Key) -> Option<bool> {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut conn = Conn::accept_with(s, "panel", "aabbccddeeff", server_key).ok()?;
            conn.set_read_timeout(Duration::from_secs(5)).unwrap();
            loop {
                if let Some((kind, payload)) = conn.take_frame().unwrap() {
                    assert_eq!(kind, 7);
                    assert_eq!(payload, Vec::<u8>::new());
                    conn.send(8, &()).unwrap();
                    return Some(conn.keyed);
                }
                conn.read_some().unwrap();
            }
        });
        let mut c = TcpStream::connect(addr).unwrap();
        let mut hs = snow::Builder::new(NOISE_PARAMS.parse().unwrap())
            .psk(0, &client_key)
            .prologue(b"NoiseAPIInit\x00\x00")
            .build_initiator()
            .unwrap();
        let mut msg = [0u8; 256];
        let n = hs.write_message(&[], &mut msg).unwrap();
        let mut out = vec![0x01, 0x00, 0x00, 0x01];
        out.extend_from_slice(&((n + 1) as u16).to_be_bytes());
        out.push(0);
        out.extend_from_slice(&msg[..n]);
        c.write_all(&out).unwrap();
        let hello = read_noise_frame(&mut c).unwrap();
        assert_eq!(hello, b"\x01panel\0aabbccddeeff\0");
        let reply = read_noise_frame(&mut c).unwrap();
        if reply[0] != 0 {
            // refused the way ESPHome does it (the client shows "invalid encryption key")
            assert_eq!(&reply[1..], b"Handshake MAC failure");
            assert_eq!(server.join().unwrap(), None);
            return None;
        }
        hs.read_message(&reply[1..], &mut msg).unwrap();
        let mut t = hs.into_transport_mode().unwrap();
        let mut cipher = [0u8; 64];
        let n = t.write_message(&[0, 7, 0, 0], &mut cipher).unwrap();
        write_noise_frame(&mut c, &cipher[..n]).unwrap();
        let frame = read_noise_frame(&mut c).unwrap();
        let n = t.read_message(&frame, &mut cipher).unwrap();
        assert_eq!(&cipher[..n], &[0, 8, 0, 0]);
        server.join().unwrap()
    }

    #[test]
    fn noise_handshake_matches_esphome() {
        // without a key: the all-zero provisioning key
        assert_eq!(noise_session(None, [0u8; 32]), Some(false));
        // with a key: that key only
        assert_eq!(noise_session(Some([7u8; 32]), [7u8; 32]), Some(true));
        assert_eq!(noise_session(Some([7u8; 32]), [0u8; 32]), None);
        assert_eq!(noise_session(Some([7u8; 32]), [8u8; 32]), None);
    }
}
