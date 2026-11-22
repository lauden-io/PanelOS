//! Home Assistant login: OAuth2 authorization-code flow with the panel as an
//! IndieAuth client. The refresh token is stored on the panel (root only),
//! with the Home Assistant address it was made for, and goes to no other
//! address; short-lived access tokens are fetched from it when needed.

use std::io::Write;
use std::net::IpAddr;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Default, Clone, Serialize, Deserialize)]
struct Stored {
    /// The Home Assistant address the login was made for (empty in a file
    /// written before the address was kept: see `Auth::load`).
    #[serde(default)]
    ha_url: String,
    client_id: String,
    refresh_token: String,
}

/// A stored login's address against the one the panel uses.
#[derive(Debug, PartialEq)]
enum Fit {
    Same,
    /// A file from before the address was kept.
    Unbound,
    /// Made for another address: the token never goes to this one.
    Other,
}

fn fit(stored_url: &str, ha_url: &str) -> Fit {
    if stored_url.is_empty() {
        Fit::Unbound
    } else if stored_url.trim_end_matches('/') == ha_url.trim_end_matches('/') {
        Fit::Same
    } else {
        Fit::Other
    }
}

/// The login file's contents; None without a refresh token.
fn parse(text: &str) -> Option<Stored> {
    serde_json::from_str::<Stored>(text).ok().filter(|s| !s.refresh_token.is_empty())
}

/// A URL's host: an IP address written out, or a name.
#[derive(Debug, Clone, PartialEq)]
enum Host {
    Ip(IpAddr),
    Name(String),
}

/// (scheme, host, port) of an http or https URL, as the url crate (the parser ureq connects by) reads it. None for
/// anything else and for any URL two parsers could read differently: one with `\`, `@`, white space or user info, or
/// whose host and port the url crate reads otherwise than the plain split (a percent sign, an IPv4 address in another
/// notation …).
fn origin(text: &str) -> Option<(String, Host, u16)> {
    if text.chars().any(|c| c == '\\' || c == '@' || c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (scheme, rest) = text.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (host, after) = v6.split_once(']')?;
        match after.strip_prefix(':') {
            Some(p) => (host, p.parse().ok()?),
            None if after.is_empty() => (host, default_port),
            None => return None,
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, p)) => (host, p.parse().ok()?),
            None => (authority, default_port),
        }
    };
    let host = host.to_ascii_lowercase();
    let parsed = url::Url::parse(text).ok()?;
    if parsed.scheme() != scheme || !parsed.username().is_empty() || parsed.password().is_some() || parsed.port_or_known_default() != Some(port) {
        return None;
    }
    let host = match parsed.host()? {
        url::Host::Domain(name) if name == host => Host::Name(host),
        url::Host::Ipv4(ip) if ip.to_string() == host => Host::Ip(IpAddr::V4(ip)),
        url::Host::Ipv6(ip) if host.parse::<std::net::Ipv6Addr>().ok() == Some(ip) => Host::Ip(IpAddr::V6(ip)),
        _ => return None,
    };
    Some((scheme, host, port))
}

/// An address the setup may try and save: one that every parser reads the same way.
pub fn is_plain_address(url: &str) -> bool {
    origin(url).is_some()
}

/// Whether `new_url` is the Home Assistant the panel is logged in to at `old_url`, by what that Home Assistant says
/// of itself (asked on the logged-in connection): its internal and external URL (`urls`) and its IP addresses
/// (`ips`). A host name only as exactly one of the URLs (`old_url` among them: scheme, name and port); a name is never
/// resolved for this, as its DNS record could point elsewhere later. An IP address written out: one in one of the
/// URLs on the same scheme and port, or one of its own IP addresses on a scheme and port one of the URLs has. Never
/// an address of the panel itself (loopback).
pub fn same_home_assistant(new_url: &str, old_url: &str, urls: &[String], ips: &[String]) -> bool {
    let Some((scheme, host, port)) = origin(new_url) else { return false };
    let known: Vec<(String, Host, u16)> = std::iter::once(old_url).chain(urls.iter().map(String::as_str)).filter_map(origin).collect();
    let on_its_port = |h: &Host| known.iter().any(|(s, kh, p)| *s == scheme && *p == port && kh == h);
    match host {
        Host::Name(name) if name == "localhost" || name.ends_with(".localhost") => false,
        Host::Name(name) => on_its_port(&Host::Name(name)),
        Host::Ip(ip) => {
            let ip = ip.to_canonical();
            if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
                return false;
            }
            let canonical = |h: &Host| match h {
                Host::Ip(a) => Host::Ip(a.to_canonical()),
                other => other.clone(),
            };
            let in_urls = known.iter().any(|(s, kh, p)| *s == scheme && *p == port && canonical(kh) == Host::Ip(ip));
            let own = ips.iter().filter_map(|a| a.trim().parse::<IpAddr>().ok()).any(|a| a.to_canonical() == ip);
            in_urls || (own && known.iter().any(|(s, _, p)| *s == scheme && *p == port))
        }
    }
}

/// What becomes of the stored login when the address changes to `new_url`.
#[derive(Debug, PartialEq)]
enum Change {
    /// It was made for `new_url`.
    Keep,
    /// The same Home Assistant at another address: the login goes along.
    Move,
    /// Revoked at this address (the one it was made for), the file deleted.
    Revoke(String),
    /// No address known for it: the file deleted only.
    Delete,
}

/// `old_url`: the address the panel used until now (None on the first setup); `same_ha`: Home Assistant there counts
/// `new_url` among its own addresses.
fn change(stored_url: &str, old_url: Option<&str>, new_url: &str, same_ha: bool) -> Change {
    // a file from before the address was kept belongs to the address the panel used
    let made_for = if stored_url.is_empty() { old_url.unwrap_or_default() } else { stored_url };
    match fit(made_for, new_url) {
        Fit::Same => Change::Keep,
        Fit::Unbound => Change::Delete,
        Fit::Other if same_ha && old_url.is_some_and(|old| fit(made_for, old) == Fit::Same) => Change::Move,
        Fit::Other => Change::Revoke(made_for.trim_end_matches('/').to_string()),
    }
}

/// Revokes a refresh token at `ha_url` (best effort: a failure is only logged).
fn revoke(ha_url: &str, refresh_token: &str, timeout: Duration) {
    let url = format!("{ha_url}/auth/token");
    let form = [("token", refresh_token), ("action", "revoke")];
    if let Err(e) = ureq::post(&url).timeout(timeout).send_form(&form) {
        eprintln!("nspanel-app: login: revoking the token failed: {e}");
    }
}

/// Writes the login file whole (root only), then renames it into place.
fn write_file(path: &Path, stored: &Stored) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(serde_json::to_string(stored).unwrap().as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// The Home Assistant address changes from `old_url` (None on the first setup) to `new_url`; `same_ha`: Home
/// Assistant counts `new_url` among its own addresses (`same_home_assistant`). The login in `path` then goes along to
/// `new_url`; otherwise it ends at the Home Assistant it was made for and the file goes (the new address pairs
/// again), unless it was made for `new_url`. Waits at most `timeout`.
pub fn change_address(path: &Path, old_url: Option<&str>, new_url: &str, same_ha: bool, timeout: Duration) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let Some(stored) = parse(&text) else {
        let _ = std::fs::remove_file(path);
        return;
    };
    match change(&stored.ha_url, old_url, new_url, same_ha) {
        Change::Keep => return,
        Change::Move => {
            let moved = Stored { ha_url: new_url.trim_end_matches('/').to_string(), ..stored.clone() };
            match write_file(path, &moved) {
                Ok(()) => {
                    eprintln!("nspanel-app: login: the new address is the same Home Assistant's, the login goes along");
                    return;
                }
                Err(e) => eprintln!("nspanel-app: login: cannot move the login to the new address: {e}"),
            }
            revoke(old_url.unwrap_or_default().trim_end_matches('/'), &stored.refresh_token, timeout);
        }
        Change::Revoke(at) => revoke(&at, &stored.refresh_token, timeout),
        Change::Delete => eprintln!("nspanel-app: login: no address known for the stored login, deleted without revoking"),
    }
    let _ = std::fs::remove_file(path);
    eprintln!("nspanel-app: login: new Home Assistant address, the old login ended");
}

#[derive(Default)]
struct Inner {
    stored: Option<Stored>,
    access_token: String,
    expires_at: u64,
}

pub struct Auth {
    ha_url: String,
    path: PathBuf,
    inner: Mutex<Inner>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub enum TokenError {
    /// No login, or Home Assistant revoked it: pairing is needed.
    NotPaired,
    /// Temporary problem (network, Home Assistant restarting).
    Retry(String),
}

impl Auth {
    pub fn load(ha_url: &str, path: PathBuf) -> Self {
        let auth = Self { ha_url: ha_url.trim_end_matches('/').to_string(), path, inner: Mutex::new(Inner::default()) };
        let stored = std::fs::read_to_string(&auth.path).ok().and_then(|t| parse(&t)).and_then(|s| match fit(&s.ha_url, &auth.ha_url) {
            Fit::Same => Some(s),
            // A file from before the address was kept: the panel has sent this token to the configured address at
            // every start, so from now on it belongs to that one (no new pairing after the update).
            Fit::Unbound => {
                let s = Stored { ha_url: auth.ha_url.clone(), ..s };
                match auth.save(&s) {
                    Ok(()) => eprintln!("nspanel-app: login: the stored login is bound to the configured address"),
                    Err(e) => eprintln!("nspanel-app: login: cannot bind the stored login to the address: {e}"),
                }
                Some(s)
            }
            // Made for another address (app.json changed by hand): it ends at its own Home Assistant, and the
            // panel pairs again.
            Fit::Other => {
                eprintln!("nspanel-app: login: the stored login is for another address: it ends, pairing again");
                std::thread::spawn(move || revoke(&s.ha_url, &s.refresh_token, Duration::from_secs(10)));
                let _ = std::fs::remove_file(&auth.path);
                None
            }
        });
        auth.inner.lock().unwrap().stored = stored;
        auth
    }

    pub fn is_paired(&self) -> bool {
        self.inner.lock().unwrap().stored.is_some()
    }

    fn save(&self, stored: &Stored) -> std::io::Result<()> {
        write_file(&self.path, stored)
    }

    fn post_token(&self, form: &[(&str, &str)]) -> Result<serde_json::Value, (u16, String)> {
        let url = format!("{}/auth/token", self.ha_url);
        match ureq::post(&url).timeout(std::time::Duration::from_secs(15)).send_form(form) {
            Ok(resp) => serde_json::from_reader(resp.into_reader()).map_err(|e| (0, e.to_string())),
            Err(ureq::Error::Status(code, resp)) => Err((code, resp.into_string().unwrap_or_default())),
            Err(e) => Err((0, e.to_string())),
        }
    }

    /// Exchanges the code from the login redirect for tokens and stores them.
    pub fn exchange_code(&self, client_id: &str, code: &str) -> Result<(), String> {
        let v = self
            .post_token(&[("grant_type", "authorization_code"), ("code", code), ("client_id", client_id)])
            .map_err(|(c, e)| format!("token exchange failed ({c}): {e}"))?;
        let refresh = v["refresh_token"].as_str().ok_or("no refresh_token in answer")?.to_string();
        let access = v["access_token"].as_str().ok_or("no access_token in answer")?.to_string();
        let expires = v["expires_in"].as_u64().unwrap_or(1800);
        let stored = Stored { ha_url: self.ha_url.clone(), client_id: client_id.to_string(), refresh_token: refresh };
        self.save(&stored).map_err(|e| format!("cannot save login: {e}"))?;
        let mut inner = self.inner.lock().unwrap();
        inner.stored = Some(stored);
        inner.access_token = access;
        inner.expires_at = now() + expires;
        Ok(())
    }

    /// A valid access token, refreshed when it is about to expire.
    pub fn access_token(&self) -> Result<String, TokenError> {
        let mut inner = self.inner.lock().unwrap();
        let Some(stored) = inner.stored.clone() else { return Err(TokenError::NotPaired) };
        if !inner.access_token.is_empty() && inner.expires_at > now() + 60 {
            return Ok(inner.access_token.clone());
        }
        // the refresh token goes only to the address it was made for
        if fit(&stored.ha_url, &self.ha_url) != Fit::Same {
            return Err(TokenError::NotPaired);
        }
        match self.post_token(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &stored.refresh_token),
            ("client_id", &stored.client_id),
        ]) {
            Ok(v) => {
                let access = v["access_token"].as_str().unwrap_or_default().to_string();
                if access.is_empty() {
                    return Err(TokenError::Retry("no access_token in answer".into()));
                }
                inner.access_token = access.clone();
                inner.expires_at = now() + v["expires_in"].as_u64().unwrap_or(1800);
                Ok(access)
            }
            Err((400, _)) | Err((401, _)) | Err((403, _)) => {
                // The refresh token was revoked in Home Assistant: log in again.
                inner.stored = None;
                inner.access_token.clear();
                let _ = std::fs::remove_file(&self.path);
                Err(TokenError::NotPaired)
            }
            Err((code, e)) => Err(TokenError::Retry(format!("token refresh failed ({code}): {e}"))),
        }
    }

    /// A valid access token and how many seconds it stays valid (for the
    /// web view, which asks for tokens like Home Assistant's mobile apps).
    pub fn access_token_with_lifetime(&self) -> Result<(String, u64), TokenError> {
        let token = self.access_token()?;
        let left = self.inner.lock().unwrap().expires_at.saturating_sub(now());
        Ok((token, left))
    }

    /// Logs out: revokes the refresh token in Home Assistant (in the
    /// background, best effort) and forgets the stored login.
    pub fn forget(&self) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(stored) = inner.stored.take() {
            std::thread::spawn(move || revoke(&stored.ha_url, &stored.refresh_token, Duration::from_secs(10)));
        }
        inner.access_token.clear();
        let _ = std::fs::remove_file(&self.path);
    }

    /// Forces a refresh on the next access_token() call.
    pub fn invalidate_access(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.access_token.clear();
        inner.expires_at = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // documentation addresses (RFC 5737): nothing in the tests sends to them
    const HA: &str = "http://192.0.2.10:8123";

    #[test]
    fn a_login_fits_only_its_own_address() {
        assert_eq!(fit(HA, HA), Fit::Same);
        assert_eq!(fit("http://192.0.2.10:8123/", HA), Fit::Same);
        assert_eq!(fit("", HA), Fit::Unbound);
        assert_eq!(fit("http://192.0.2.11:8123", HA), Fit::Other);
        assert_eq!(fit("http://192.0.2.10", HA), Fit::Other);
        assert_eq!(fit("https://192.0.2.10:8123", HA), Fit::Other);
    }

    #[test]
    fn old_login_files_still_read_and_new_ones_keep_the_address() {
        let old = parse(r#"{"client_id":"http://192.0.2.20:8080/","refresh_token":"abc"}"#).unwrap();
        assert_eq!(old.ha_url, "");
        assert_eq!(old.refresh_token, "abc");
        let new = Stored { ha_url: HA.into(), ..old };
        let back = parse(&serde_json::to_string(&new).unwrap()).unwrap();
        assert_eq!((back.ha_url.as_str(), back.client_id.as_str(), back.refresh_token.as_str()), (HA, "http://192.0.2.20:8080/", "abc"));
        assert!(parse(r#"{"client_id":"x","refresh_token":""}"#).is_none());
        assert!(parse("not json").is_none());
    }

    #[test]
    fn a_new_address_ends_the_login_at_the_one_it_was_made_for() {
        let new = "http://192.0.2.11:8123";
        assert_eq!(change(HA, Some(HA), new, false), Change::Revoke(HA.into()));
        assert_eq!(change("", Some(HA), new, false), Change::Revoke(HA.into()));
        assert_eq!(change(HA, None, new, false), Change::Revoke(HA.into()));
        assert_eq!(change("", None, new, false), Change::Delete);
        assert_eq!(change(new, Some(HA), new, false), Change::Keep);
        assert_eq!(change("", Some(new), new, false), Change::Keep);
        // the same Home Assistant: the login goes along, but only one made for the address it came from
        assert_eq!(change(HA, Some(HA), new, true), Change::Move);
        assert_eq!(change("", Some(HA), new, true), Change::Move);
        assert_eq!(change(HA, None, new, true), Change::Revoke(HA.into()));
        assert_eq!(change("http://192.0.2.12:8123", Some(HA), new, true), Change::Revoke("http://192.0.2.12:8123".into()));
        assert_eq!(change("", None, new, true), Change::Delete);
    }

    #[test]
    fn urls_split_into_scheme_host_and_port() {
        let ip = |s: &str, h: &str, p: u16| Some((s.to_string(), Host::Ip(h.parse().unwrap()), p));
        let name = |s: &str, h: &str, p: u16| Some((s.to_string(), Host::Name(h.to_string()), p));
        assert_eq!(origin("http://192.0.2.10:8123"), ip("http", "192.0.2.10", 8123));
        assert_eq!(origin("HTTP://HA.Example.org/"), name("http", "ha.example.org", 80));
        assert_eq!(origin("https://ha.example.org/lovelace?x=1"), name("https", "ha.example.org", 443));
        assert_eq!(origin("http://[2001:db8::1]:8123"), ip("http", "2001:db8::1", 8123));
        assert_eq!(origin("https://[2001:db8::1]"), ip("https", "2001:db8::1", 443));
        assert_eq!(origin("http://192.0.2.10:80"), ip("http", "192.0.2.10", 80));
        assert_eq!(origin("ws://192.0.2.10:8123"), None);
        assert_eq!(origin("http://:8123"), None);
        assert_eq!(origin("http://192.0.2.10:port"), None);
        assert_eq!(origin("192.0.2.10:8123"), None);
        // anything two parsers could read differently: ureq would go to the host before the backslash
        assert_eq!(origin("http://evil.example\\@192.0.2.10:8123"), None);
        assert_eq!(origin("http://evil.example\\.192.0.2.10:8123"), None);
        assert_eq!(origin("http://user@192.0.2.10:8123"), None);
        assert_eq!(origin("http://user:pw@192.0.2.10:8123"), None);
        assert_eq!(origin("http://192.0.2.10:8123/@evil.example"), None);
        assert_eq!(origin("http://192.0.2.10 :8123"), None);
        assert_eq!(origin(" http://192.0.2.10:8123"), None);
        assert_eq!(origin("http://192.0.2.10\t:8123"), None);
        assert_eq!(origin("http://evil%2eexample:8123"), None);
        assert_eq!(origin("http://0xc0.0.2.10:8123"), None);
        assert_eq!(origin("http://3221225994:8123"), None);
        assert_eq!(origin("http://192.0.2.010:8123"), None);
        assert!(is_plain_address("http://192.0.2.10:8123"));
        assert!(!is_plain_address("http://evil.example\\@192.0.2.10:8123"));
    }

    #[test]
    fn the_same_home_assistant_by_what_it_says_of_itself() {
        let same = |new: &str, urls: &[&str], ips: &[&str]| {
            let urls: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
            let ips: Vec<String> = ips.iter().map(|u| u.to_string()).collect();
            same_home_assistant(new, HA, &urls, &ips)
        };
        let internal = "http://ha.example.org:8123";
        let external = "https://remote.example.net";
        // a host name: exactly one of its URLs (scheme, name and port)
        assert!(same("http://ha.example.org:8123", &[internal], &[]));
        assert!(same("http://HA.example.org:8123/", &[internal, external], &[]));
        assert!(same("https://remote.example.net/", &[internal, external], &[]));
        assert!(same("https://remote.example.net:443", &[internal, external], &[]));
        assert!(!same("http://ha.example.org", &[internal], &[]));
        assert!(!same("https://ha.example.org:8123", &[internal], &[]));
        assert!(!same("http://remote.example.net", &[internal, external], &[]));
        // a name that is none of its URLs, even when it would resolve to its address: names are never resolved
        assert!(!same("http://ha.example.org:8123", &[], &["192.0.2.10"]));
        assert!(!same("http://other.example.org:8123", &[internal, external], &["192.0.2.10"]));
        assert!(!same("http://ha.example.org.attacker.example:8123", &[internal], &["192.0.2.10"]));
        // a local-only user gets no external URL: the internal one and the old address still count
        assert!(same("http://ha.example.org:8123", &[internal], &["192.0.2.10"]));
        // an IP address written out: one in its URLs, or one of its own on a scheme and port one of its URLs has
        assert!(same("http://[2001:db8::10]:8123", &["http://[2001:db8::10]:8123"], &[]));
        assert!(same("http://192.0.2.20:8123", &[], &["192.0.2.20", "2001:db8::20"]));
        assert!(same("http://[2001:db8::20]:8123", &[internal], &["192.0.2.20", "2001:db8::20"]));
        assert!(same("http://192.0.2.20:8123", &[], &["::ffff:192.0.2.20"]));
        assert!(same("http://[::ffff:192.0.2.20]:8123", &[], &["192.0.2.20"]));
        assert!(same("https://192.0.2.20", &[external], &["192.0.2.20"]));
        assert!(!same("http://192.0.2.20:9000", &[internal], &["192.0.2.20"]));
        assert!(!same("https://192.0.2.20:8123", &[internal], &["192.0.2.20"]));
        // its name's address is not known without a lookup: an IP address only by its own list or its URLs
        assert!(!same("http://192.0.2.20:8123", &[internal], &[]));
        assert!(!same("http://[2001:db8::10]:8123", &[internal], &[]));
        assert!(!same("http://192.0.2.66:8123", &[internal, external], &["192.0.2.10", "192.0.2.20"]));
        // an address the parsers read differently never counts, whatever it ends in
        assert!(!same("http://evil.example\\@192.0.2.10:8123", &[internal], &["192.0.2.10"]));
        assert!(!same("http://evil.example\\@ha.example.org:8123", &[internal], &[]));
        assert!(!same("http://user@192.0.2.20:8123", &[], &["192.0.2.20"]));
        // never the panel itself, even when Home Assistant counts its loopback
        assert!(!same("http://127.0.0.1:8123", &["http://127.0.0.1:8123"], &["127.0.0.1"]));
        assert!(!same("http://[::1]:8123", &["http://[::1]:8123"], &["::1"]));
        assert!(!same("http://localhost:8123", &["http://localhost:8123"], &[]));
        assert!(!same("http://ha.localhost:8123", &["http://ha.localhost:8123"], &[]));
        assert!(!same("not a url", &[internal], &["192.0.2.10"]));
    }

    fn file(name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nspanel-auth-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn loading_binds_an_old_file_and_drops_one_for_another_address() {
        // the logins that get revoked are made for an address where nothing listens (the revocation fails at once)
        const GONE: &str = "http://127.0.0.1:9";
        let old = file("old.json", r#"{"client_id":"c","refresh_token":"abc"}"#);
        let auth = Auth::load(&format!("{GONE}/"), old.clone());
        assert!(auth.is_paired());
        assert_eq!(parse(&std::fs::read_to_string(&old).unwrap()).unwrap().ha_url, GONE);
        assert!(Auth::load(GONE, old.clone()).is_paired());
        let bound = std::fs::read_to_string(&old).unwrap();

        let other = file("other.json", &bound);
        let auth = Auth::load(HA, other.clone());
        assert!(!auth.is_paired());
        assert!(!other.exists());
        assert!(matches!(auth.access_token(), Err(TokenError::NotPaired)));

        let ended = file("ended.json", &bound);
        change_address(&ended, Some(GONE), HA, false, Duration::from_secs(2));
        assert!(!ended.exists());
        let kept = file("kept.json", &bound);
        change_address(&kept, None, GONE, false, Duration::from_secs(2));
        assert!(kept.exists());
        let unbound = file("unbound.json", r#"{"client_id":"c","refresh_token":"abc"}"#);
        change_address(&unbound, None, HA, false, Duration::from_secs(2));
        assert!(!unbound.exists());
        // the same Home Assistant at its new address: the login goes along and is used there (and only there)
        let moved = file("moved.json", &bound);
        change_address(&moved, Some(GONE), HA, true, Duration::from_secs(2));
        let stored = parse(&std::fs::read_to_string(&moved).unwrap()).unwrap();
        assert_eq!((stored.ha_url.as_str(), stored.refresh_token.as_str()), (HA, "abc"));
        let auth = Auth::load(HA, moved.clone());
        assert!(auth.is_paired());
        let _ = std::fs::remove_dir_all(old.parent().unwrap());
    }
}
