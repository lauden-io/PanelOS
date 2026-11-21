//! Web view: the Home Assistant frontend in WPE WebKit.
//!
//! The browser is `panelos-web` from the web bundle (a SquashFS image on the
//! persistent partition, mounted at `/opt/panelos-web` by S50panelos-web),
//! started chrooted in it. It renders off screen on the GPU and puts every
//! frame into shared memory; this side copies it into a Slint image and
//! sends touches, the page and login tokens back (line protocol, see
//! `buildroot/package/panelos-web/src/panelos-web.c`). If the helper ends,
//! it is started again and gets the last page.

use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::auth::{Auth, TokenError};

pub const BUNDLE_ROOT: &str = "/opt/panelos-web";
const HELPER: &str = "/usr/bin/panelos-web";
pub const WIDTH: u32 = 480;
pub const HEIGHT: u32 = 480;

/// True when the web bundle is mounted.
pub fn available() -> bool {
    std::path::Path::new(&format!("{BUNDLE_ROOT}{HELPER}")).exists()
}

#[derive(Clone, Default, PartialEq)]
struct Page {
    url: String,
    hide_header: bool,
    hide_sidebar: bool,
}

#[derive(Default)]
struct Shared {
    stream: Option<UnixStream>,
    page: Page,
    visible: bool,
    /// Frames straight into the display buffers (the web view is all there is on the screen).
    direct: bool,
    /// Resource paths the page does not load (card resources the dashboard does not use).
    skip: Vec<String>,
    /// Page size, percent.
    zoom: u32,
    /// The time zone rule the running helper started with.
    tz: String,
}

pub struct WebView {
    shared: Mutex<Shared>,
    on_loading: Mutex<Option<Arc<LoadingSink>>>,
    /// Answer to the last `check_top`: -1 not known yet, 0 no, 1 yes.
    top: AtomicI8,
}

type FrameSink = dyn Fn(SharedPixelBuffer<Rgba8Pixel>, crate::webstats::FrameTimes) + Send + Sync;
/// true: a page started loading; false: it is ready to be shown.
type LoadingSink = dyn Fn(bool) + Send + Sync;

/// Display buffers the helper renders into while the web view is all there
/// is on the screen (dma-buf, pitch in bytes); set once at start.
static SCANOUT: std::sync::OnceLock<Vec<(OwnedFd, u32)>> = std::sync::OnceLock::new();

pub fn set_scanout_buffers(buffers: Vec<(OwnedFd, u32)>) {
    let _ = SCANOUT.set(buffers);
}

impl WebView {
    /// Starts the helper (and keeps it running). `on_frame` gets every new
    /// frame on the helper's reader thread.
    pub fn start(auth: Arc<Auth>, on_frame: Box<FrameSink>) -> Arc<Self> {
        let view = Arc::new(Self { shared: Mutex::new(Shared::default()), on_loading: Mutex::new(None), top: AtomicI8::new(-1) });
        let v = view.clone();
        let on_frame: Arc<FrameSink> = on_frame.into();
        std::thread::Builder::new()
            .name("web".into())
            .spawn(move || {
                let mut backoff = Duration::from_secs(1);
                loop {
                    let started = Instant::now();
                    if let Err(e) = v.run_once(&auth, on_frame.clone()) {
                        eprintln!("nspanel-app: web view: {e}");
                    }
                    v.shared.lock().unwrap().stream = None;
                    if started.elapsed() > Duration::from_secs(60) {
                        backoff = Duration::from_secs(1);
                    }
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            })
            .expect("web thread");
        view
    }

    /// Told when a page starts loading (true) and when it is ready (false).
    pub fn on_loading(&self, sink: impl Fn(bool) + Send + Sync + 'static) {
        *self.on_loading.lock().unwrap() = Some(Arc::new(sink));
    }

    fn loading(&self, on: bool) {
        let sink = self.on_loading.lock().unwrap().clone();
        if let Some(sink) = sink {
            sink(on);
        }
    }

    /// The page to show; reloads only when something changed.
    pub fn load(&self, url: &str, hide_header: bool, hide_sidebar: bool) {
        let page = Page { url: url.to_string(), hide_header, hide_sidebar };
        let mut s = self.shared.lock().unwrap();
        if s.page == page {
            return;
        }
        s.page = page.clone();
        if let Some(stream) = s.stream.as_mut() {
            let _ = send_page(stream, &page);
        }
    }

    /// Resources (URL paths) the page must not load; reloads the page when the list changes.
    pub fn set_skip(&self, paths: Vec<String>) {
        let mut s = self.shared.lock().unwrap();
        if s.skip == paths {
            return;
        }
        s.skip = paths;
        let line = skip_line(&s.skip);
        if let Some(stream) = s.stream.as_mut() {
            let _ = writeln!(stream, "{line}");
        }
    }

    /// Size of the page, percent (100 = as in a browser at 480 × 480).
    pub fn set_zoom(&self, percent: u32) {
        let mut s = self.shared.lock().unwrap();
        if s.zoom == percent {
            return;
        }
        s.zoom = percent;
        if let Some(stream) = s.stream.as_mut() {
            let _ = writeln!(stream, "ZOOM {percent}");
        }
    }

    /// Frames straight into the display buffers, or as images for the UI.
    pub fn set_direct(&self, on: bool) {
        let mut s = self.shared.lock().unwrap();
        s.direct = on;
        if let Some(stream) = s.stream.as_mut() {
            let _ = writeln!(stream, "DIRECT {}", on as i32);
        }
    }

    /// Touch in screen coordinates: kind 'd' (down), 'm' (move), 'u' (up).
    pub fn touch(&self, kind: char, x: f32, y: f32) {
        self.send(&format!("TOUCH {kind} {} {}", x.round() as i32, y.round() as i32));
    }

    /// Asks whether what lies under a screen point is scrolled to its top
    /// (for pull to refresh); the answer comes as `at_top`.
    pub fn check_top(&self, x: f32, y: f32) {
        self.top.store(-1, Ordering::Relaxed);
        self.send(&format!("ATTOP {} {}", x.round() as i32, y.round() as i32));
    }

    /// The answer to the last `check_top`, once it is there.
    pub fn at_top(&self) -> Option<bool> {
        match self.top.load(Ordering::Relaxed) {
            -1 => None,
            v => Some(v == 1),
        }
    }

    /// Loads the page again (pull to refresh).
    pub fn reload(&self) {
        self.send("RELOAD");
    }

    /// Hidden pages do not render (screensaver, assistant mode).
    pub fn set_visible(&self, on: bool) {
        let mut s = self.shared.lock().unwrap();
        if s.visible == on {
            return;
        }
        s.visible = on;
        if let Some(stream) = s.stream.as_mut() {
            let _ = writeln!(stream, "VISIBLE {}", on as i32);
        }
    }

    /// The helper takes the time zone when it starts: after a change it is started again (the page loads again).
    pub fn follow_time_zone(&self) {
        let s = self.shared.lock().unwrap();
        if s.tz != crate::timezone::rule() {
            if let Some(stream) = s.stream.as_ref() {
                eprintln!("nspanel-app: web view: starting again in the new time zone");
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    /// Development: a raw line for the helper (LOAD, SCROLLFIND, SCROLLTEST).
    pub fn dev(&self, line: &str) {
        self.send(line);
    }

    fn send(&self, line: &str) {
        if let Some(stream) = self.shared.lock().unwrap().stream.as_mut() {
            let _ = writeln!(stream, "{line}");
        }
    }

    fn run_once(&self, auth: &Arc<Auth>, on_frame: Arc<FrameSink>) -> std::io::Result<()> {
        // The bundle's /etc/resolv.conf points to /tmp/resolv.conf.
        let _ = std::fs::copy("/etc/resolv.conf", format!("{BUNDLE_ROOT}/tmp/resolv.conf"));
        let (ours, theirs) = UnixStream::pair()?;
        let child_fd = theirs.as_raw_fd();
        let tz = crate::timezone::rule();
        self.shared.lock().unwrap().tz = tz.clone();
        let mut cmd = Command::new("/usr/sbin/chroot");
        cmd.args([BUNDLE_ROOT, HELPER, "--fd", "3", "--size", &format!("{WIDTH}x{HEIGHT}"), "--data", "/data"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", "/data")
            .env("XDG_RUNTIME_DIR", "/tmp")
            .env("XDG_CACHE_HOME", "/data/cache")
            .env("TZ", tz)
            .stdin(Stdio::null());
        // Development aid: WebKit's own switches (WEBKIT_*) from the app's environment.
        for (key, value) in std::env::vars().filter(|(k, _)| k.starts_with("WEBKIT_")) {
            cmd.env(key, value);
        }
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                if libc::dup2(child_fd, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        drop(theirs);

        let result = self.talk(ours, auth, on_frame);
        let _ = child.kill();
        let _ = child.wait();
        result
    }

    fn talk(&self, stream: UnixStream, auth: &Arc<Auth>, on_frame: Arc<FrameSink>) -> std::io::Result<()> {
        // First message: "SHM w h" with the shared memory.
        let (first, fd) = recv_with_fd(stream.as_raw_fd())?;
        let mut parts = first.split_whitespace();
        let (Some("SHM"), Some(w), Some(h)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(std::io::Error::other(format!("unexpected start: {first}")));
        };
        let (w, h): (usize, usize) = (w.parse().unwrap_or(0), h.parse().unwrap_or(0));
        let fd = fd.ok_or_else(|| std::io::Error::other("no shared memory"))?;
        if w != WIDTH as usize || h != HEIGHT as usize {
            return Err(std::io::Error::other(format!("frame size {w}x{h}")));
        }
        let frame_bytes = w * h * 4;
        let shm = Mapping::new(&fd, 2 * frame_bytes)?;
        eprintln!("nspanel-app: web view started");

        // The display buffers for direct frames (the helper falls back to
        // shared memory without them).
        if let Some(buffers) = SCANOUT.get() {
            let fds: Vec<RawFd> = buffers.iter().map(|(fd, _)| fd.as_raw_fd()).collect();
            let pitch = buffers.first().map(|(_, p)| *p).unwrap_or(0);
            send_with_fds(stream.as_raw_fd(), &format!("BUFS {} {WIDTH} {HEIGHT} {pitch}\n", fds.len()), &fds)?;
        }

        {
            let mut s = self.shared.lock().unwrap();
            let mut writer = stream.try_clone()?;
            if !s.skip.is_empty() {
                writeln!(writer, "{}", skip_line(&s.skip))?;
            }
            if s.zoom != 0 && s.zoom != 100 {
                writeln!(writer, "ZOOM {}", s.zoom)?;
            }
            if !s.page.url.is_empty() {
                send_page(&mut writer, &s.page)?;
            }
            writeln!(writer, "VISIBLE {}", s.visible as i32)?;
            writeln!(writer, "DIRECT {}", s.direct as i32)?;
            s.stream = Some(writer);
            // the time zone changed while the helper started: it starts again in the new one
            if s.tz != crate::timezone::rule() {
                return Err(std::io::Error::other("the time zone changed"));
            }
        }

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                return Err(std::io::Error::other("helper ended"));
            }
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("FRAME") => {
                    let mut num = || parts.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
                    let (n, produced, readback) = (num(), num(), num());
                    let slot = (n % 2) as usize * frame_bytes;
                    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(WIDTH, HEIGHT);
                    buffer.make_mut_bytes().copy_from_slice(&shm.bytes()[slot..slot + frame_bytes]);
                    on_frame(buffer, crate::webstats::frame_copied(produced, readback));
                }
                Some("AUTH") => {
                    let auth = auth.clone();
                    let mut writer = self.shared.lock().unwrap().stream.as_ref().and_then(|s| s.try_clone().ok());
                    // Token refresh may wait for Home Assistant: not on the reader.
                    std::thread::spawn(move || match auth.access_token_with_lifetime() {
                        Ok((token, left)) => {
                            if let Some(w) = writer.as_mut() {
                                let _ = writeln!(w, "TOKEN {token} {left}");
                            }
                        }
                        Err(TokenError::NotPaired) => eprintln!("nspanel-app: web view: no login"),
                        Err(TokenError::Retry(e)) => eprintln!("nspanel-app: web view: token: {e}"),
                    });
                }
                Some("LOADED") => eprintln!("nspanel-app: web view: page loaded"),
                Some("LOADING") => self.loading(true),
                Some("READY") => {
                    eprintln!("nspanel-app: web view: page ready ({})", parts.next().unwrap_or("?"));
                    self.loading(false);
                }
                Some("FAILED") => eprintln!("nspanel-app: web view: {}", line.trim()),
                Some("SCROLL") => eprintln!("nspanel-app: web view: {}", line.trim()),
                Some("ATTOP") => self.top.store((parts.next() == Some("1")) as i8, Ordering::Relaxed),
                Some("FRAMEBUF") => {
                    let mut num = || parts.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
                    let (i, produced, render) = (num() as usize, num(), num());
                    crate::platform::web_frame_ready(i, crate::webstats::frame_copied(produced, render));
                    // Wakes the event loop.
                    let _ = slint::invoke_from_event_loop(|| {});
                }
                _ => {}
            }
        }
    }
}

/// "SKIP <path> ...": paths without query strings, no spaces.
fn skip_line(paths: &[String]) -> String {
    let clean: Vec<&str> = paths
        .iter()
        .map(|p| p.split('?').next().unwrap_or_default())
        .filter(|p| !p.is_empty() && !p.contains(char::is_whitespace))
        .collect();
    format!("SKIP {}", clean.join(" ")).trim_end().to_string()
}

fn send_page(stream: &mut UnixStream, page: &Page) -> std::io::Result<()> {
    writeln!(stream, "KIOSK {} {}", page.hide_header as i32, page.hide_sidebar as i32)?;
    writeln!(stream, "LOAD {}", page.url)
}

/// One message with file descriptors (SCM_RIGHTS).
fn send_with_fds(sock: RawFd, text: &str, fds: &[RawFd]) -> std::io::Result<()> {
    let space = unsafe { libc::CMSG_SPACE((fds.len() * std::mem::size_of::<RawFd>()) as u32) } as usize;
    let mut ctl = vec![0u8; space];
    let mut iov = libc::iovec { iov_base: text.as_ptr() as *mut _, iov_len: text.len() };
    // SAFETY: zeroed msghdr pointing to the buffers above, which outlive the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctl.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: filling the one control message the buffer was sized for.
    unsafe {
        let c = libc::CMSG_FIRSTHDR(&msg);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN((fds.len() * std::mem::size_of::<RawFd>()) as u32) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(c).cast::<RawFd>(), fds.len());
    }
    // SAFETY: sendmsg on our socket with the message above.
    if unsafe { libc::sendmsg(sock, &msg, 0) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// One message with an optional file descriptor (SCM_RIGHTS).
fn recv_with_fd(sock: RawFd) -> std::io::Result<(String, Option<OwnedFd>)> {
    let mut buf = [0u8; 128];
    let mut ctl = [0u8; 64];
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
    // SAFETY: zeroed msghdr with pointers to local buffers that outlive the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctl.as_mut_ptr().cast();
    msg.msg_controllen = ctl.len() as _;
    // SAFETY: recvmsg on our socket into the buffers above.
    let n = unsafe { libc::recvmsg(sock, &mut msg, libc::MSG_CMSG_CLOEXEC) };
    if n <= 0 {
        return Err(std::io::Error::other("no start message"));
    }
    let mut fd = None;
    // SAFETY: walking the control messages the kernel filled in.
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let raw = std::ptr::read_unaligned(libc::CMSG_DATA(c) as *const RawFd);
                fd = Some(OwnedFd::from_raw_fd(raw));
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    let text = String::from_utf8_lossy(&buf[..n as usize]).trim().to_string();
    Ok((text, fd))
}

struct Mapping {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is only read; the helper writes the other slot.
unsafe impl Send for Mapping {}

impl Mapping {
    fn new(fd: &OwnedFd, len: usize) -> std::io::Result<Self> {
        // SAFETY: read-only shared mapping of the helper's memfd.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { ptr: ptr.cast(), len })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: ptr/len describe a live mapping.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmapping what new() mapped.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}
