//! Slint platform for the panel: the renderer draws the UI (the software
//! renderer into the DRM back buffer, or with the `gpu` feature Skia on the
//! GPU into a GBM surface), touch comes from evdev, and one poll(2) loop waits
//! for page flips, touch input, work sent from other threads and the next
//! Slint timer.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::os::fd::RawFd;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(not(feature = "gpu"))]
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{EventLoopProxy, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{EventLoopError, LogicalPosition, PlatformError};

use crate::display::Display;

/// The window the UI draws into.
#[cfg(not(feature = "gpu"))]
pub type UiWindow = MinimalSoftwareWindow;
#[cfg(feature = "gpu")]
pub type UiWindow = crate::gpu::GpuWindow;
use crate::touch::{Touch, TouchEvent};

/// The touch calibration's correction in screen pixels (subtracted from every finger position).
pub static TOUCH_OFFSET: std::sync::Mutex<(f32, f32)> = std::sync::Mutex::new((0.0, 0.0));

type Job = Box<dyn FnOnce() + Send>;

/// Set by SIGUSR1: write the current screen to /tmp/nspanel-screen.ppm.
static SCREENSHOT: AtomicBool = AtomicBool::new(false);

type ScreenSink = Box<dyn FnOnce(u32, u32, Vec<u8>)>;

/// Web frame rendered straight into a shared display buffer: (buffer, times).
static WEB_NEXT: Mutex<Option<(usize, crate::webstats::FrameTimes)>> = Mutex::new(None);
/// Shared buffers the helper may render into again.
static WEB_FREED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// How the UI thread drives the direct web view (see `set_web_direct`).
pub struct WebDirect {
    /// True while the web view is all there is on the screen.
    pub allowed: Box<dyn Fn() -> bool>,
    /// The answer of `allowed` changed.
    pub changed: Box<dyn Fn(bool)>,
    /// Shared buffers the helper may use again.
    pub release: Box<dyn Fn(&[usize])>,
}

thread_local! {
    static WEB_DIRECT: std::cell::RefCell<Option<(WebDirect, bool)>> = const { std::cell::RefCell::new(None) };
    /// Waiting for a picture of the screen (width, height, RGB).
    static SCREEN_REQUESTS: std::cell::RefCell<Vec<ScreenSink>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Told after every touch whether a one-finger press is under way (see `on_press`).
    static PRESS: std::cell::RefCell<Option<Box<dyn Fn(bool)>>> = const { std::cell::RefCell::new(None) };
    /// Since the web view took the screen over: until when the UI still draws while no web frame came (see `draw`).
    static UI_UNTIL_WEB: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// How long the UI keeps drawing after the web view took the screen over, while the web view sends no frame: long
/// enough for what went off the dashboard (a strip, the voice view, the pull disc) to fade out.
const UI_AFTER_WEB_TAKES_OVER: Duration = Duration::from_millis(1000);

/// UI thread: `f` is told after every touch whether a one-finger press is under way in the UI (false after a lift and
/// once a second finger turns the touch into a gesture), so the UI can settle what a release that never reached it
/// left half done.
pub fn on_press(f: Box<dyn Fn(bool)>) {
    PRESS.with(|p| *p.borrow_mut() = Some(f));
}

/// Web helper (any thread): shared buffer `i` holds a new frame. A frame
/// that was not shown yet is given back.
pub fn web_frame_ready(i: usize, times: crate::webstats::FrameTimes) {
    if let Some((old, _)) = WEB_NEXT.lock().unwrap().replace((i, times)) {
        WEB_FREED.lock().unwrap().push(old);
    }
}

/// UI thread: lets the event loop show web frames straight from the shared
/// buffers while `allowed` says so.
pub fn set_web_direct(direct: WebDirect) {
    WEB_DIRECT.with(|w| *w.borrow_mut() = Some((direct, false)));
}

/// Whether web frames go straight to the screen now; tells `changed`.
fn web_direct_now() -> bool {
    WEB_DIRECT.with(|w| {
        let mut w = w.borrow_mut();
        let Some((direct, was)) = w.as_mut() else { return false };
        let now = (direct.allowed)();
        if now != *was {
            *was = now;
            (direct.changed)(now);
            // the web view sends a frame only when its page changes: a still dashboard would leave the UI's last frame
            // (with what just went off it) on the screen
            UI_UNTIL_WEB.with(|u| u.set(now.then(|| Instant::now() + UI_AFTER_WEB_TAKES_OVER)));
        }
        now
    })
}

/// A web buffer left the screen (seen while the GPU renderer waited for a flip).
#[allow(dead_code)]
pub fn web_buffer_freed(i: usize) {
    WEB_FREED.lock().unwrap().push(i);
}

fn web_release(list: &[usize]) {
    if list.is_empty() {
        return;
    }
    WEB_DIRECT.with(|w| {
        if let Some((direct, _)) = w.borrow().as_ref() {
            (direct.release)(list);
        }
    });
}

/// Calls `sink` on the UI thread with what is on the screen, at the next
/// turn of the event loop.
pub fn request_screen(sink: impl FnOnce(u32, u32, Vec<u8>) + 'static) {
    SCREEN_REQUESTS.with(|r| r.borrow_mut().push(Box::new(sink)));
}

extern "C" fn on_sigusr1(_: libc::c_int) {
    SCREENSHOT.store(true, Ordering::SeqCst);
}

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
/// Milliseconds (since START) of the last touch, for the screensaver.
static LAST_INPUT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Time since the last touch (or since start).
pub fn idle_time() -> Duration {
    Duration::from_millis(now_ms().saturating_sub(LAST_INPUT_MS.load(Ordering::SeqCst)))
}

/// Counts as user activity (touch, approach, wake command).
pub fn note_activity() {
    LAST_INPUT_MS.store(now_ms(), Ordering::SeqCst);
}

/// Fingers that went down on the screen since start (a view Home Assistant shows until someone touches the screen).
static TOUCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many times a finger went down on the screen since start.
pub fn touches() -> u64 {
    TOUCHES.load(Ordering::SeqCst)
}

/// Milliseconds (since START) of the last stir for the clocks' calm: someone near, or something asked of the panel
/// from Home Assistant (these do not hold off the screensaver).
static LAST_STIR_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Someone came near, or Home Assistant asked something of the panel.
pub fn note_stir() {
    LAST_STIR_MS.store(now_ms(), Ordering::SeqCst);
}

/// Seconds without activity or stir before the clocks turn calm (development: `{"calm_after": s}`).
pub static CALM_AFTER_S: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(600);

/// Nobody near and nothing asked of the panel for CALM_AFTER_S: the clocks' colon steps instead of breathing.
pub fn calm() -> bool {
    let last = LAST_INPUT_MS.load(Ordering::SeqCst).max(LAST_STIR_MS.load(Ordering::SeqCst));
    now_ms().saturating_sub(last) >= CALM_AFTER_S.load(Ordering::SeqCst) * 1000
}

/// Set by SIGUSR2; nothing reads it: the handler only keeps the signal from ending the app.
pub static SCROLL_REQUEST: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigusr2(_: libc::c_int) {
    SCROLL_REQUEST.store(true, Ordering::SeqCst);
}

/// Set by SIGTERM (the service stopping): the app says goodbye where it must, then exits.
static STOP_REQUEST: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigterm(_: libc::c_int) {
    STOP_REQUEST.store(true, Ordering::SeqCst);
}

pub fn install_screenshot_signal() {
    // SAFETY: the handlers only store to atomics.
    unsafe {
        libc::signal(libc::SIGUSR1, on_sigusr1 as *const () as libc::sighandler_t);
        libc::signal(libc::SIGUSR2, on_sigusr2 as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_sigterm as *const () as libc::sighandler_t);
    }
    // a stop: Music Assistant hears "restart" (it reconnects as soon as the app is back), then the app ends
    std::thread::spawn(|| loop {
        if STOP_REQUEST.load(Ordering::SeqCst) {
            eprintln!("nspanel-app: stopping");
            crate::sendspin::goodbye();
            std::process::exit(0);
        }
        std::thread::sleep(Duration::from_millis(50));
    });
}

/// Lets other threads run code on the UI thread (slint::invoke_from_event_loop).
#[derive(Clone)]
struct Wakeup {
    fd: RawFd,
    jobs: Arc<Mutex<VecDeque<Job>>>,
    quit: Arc<AtomicBool>,
}

impl Wakeup {
    fn new() -> Self {
        // SAFETY: eventfd(2) with flags only.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        assert!(fd >= 0, "eventfd failed");
        Self { fd, jobs: Arc::default(), quit: Arc::default() }
    }

    fn wake(&self) {
        let one: u64 = 1;
        // SAFETY: writes 8 bytes from a local u64 to the eventfd.
        unsafe { libc::write(self.fd, (&one as *const u64).cast(), 8) };
    }

    fn drain(&self) {
        let mut counter: u64 = 0;
        // SAFETY: reads 8 bytes into a local u64 (non-blocking).
        unsafe { libc::read(self.fd, (&mut counter as *mut u64).cast(), 8) };
        let jobs: Vec<Job> = self.jobs.lock().unwrap().drain(..).collect();
        for job in jobs {
            job();
        }
    }
}

impl EventLoopProxy for Wakeup {
    fn quit_event_loop(&self) -> Result<(), EventLoopError> {
        self.quit.store(true, Ordering::SeqCst);
        self.wake();
        Ok(())
    }

    fn invoke_from_event_loop(&self, event: Box<dyn FnOnce() + Send>) -> Result<(), EventLoopError> {
        self.jobs.lock().unwrap().push_back(event);
        self.wake();
        Ok(())
    }
}

pub struct PanelPlatform {
    window: Rc<UiWindow>,
    start: Instant,
    display: Rc<RefCell<Display>>,
    touch: RefCell<Option<Touch>>,
    /// When to look for the touch device again (ms since START) while there is none.
    touch_retry: std::cell::Cell<u64>,
    wakeup: Wakeup,
    /// Development: FIFO with lines "tap x y" / "down x y" / "move x y" / "up x y"
    /// / "swipe x1 y1 x2 y2 ms" (moves every 16 ms) / "pinch x y d1 d2 ms" (two
    /// fingers above each other round (x, y), d1 px apart, moving to d2 px apart).
    dev_input: Option<RawFd>,
    dev_buf: RefCell<String>,
    /// Touches of a swipe from the development input, with when they are due (µs).
    gesture: RefCell<VecDeque<(u64, TouchEvent)>>,
    /// The two-finger gesture under way.
    pinch: std::cell::Cell<Option<Pinch>>,
}

/// A two-finger gesture: the spread it is measured from, the scale sent last, the fingers' midpoint, and whether
/// the pinch has started (the fingers moved 8 px apart or together).
#[derive(Debug, Clone, Copy)]
struct Pinch {
    from: f32,
    scale: f32,
    mid: (f32, f32),
    started: bool,
}

/// The two fingers' vertical spread (at least 24 px, so fingers side by side do not zoom by leaps).
fn spread(y1: f32, y2: f32) -> f32 {
    (y1 - y2).abs().max(24.0)
}

fn midpoint(x1: f32, y1: f32, x2: f32, y2: f32) -> (f32, f32) {
    ((x1 + x2) / 2.0, (y1 + y2) / 2.0)
}

/// The touches of the development input's "pinch x y d1 d2 ms": the first finger goes down, the second one 40 ms
/// later, they spread from d1 to d2 (moves every 16 ms), then both lift; with when each is due (µs from `start`).
fn pinch_touches(start: u64, x: f32, y: f32, d1: f32, d2: f32, ms: f32) -> Vec<(u64, TouchEvent)> {
    let at = |d: f32| (y - d / 2.0, y + d / 2.0);
    let (a, b) = at(d1);
    let mut out = vec![(start, TouchEvent::Down(x, a)), (start + 40_000, TouchEvent::TwoDown(x, a, x, b))];
    let dur = (ms.max(16.0) * 1000.0) as u64;
    let steps = (dur / 16_000).max(1);
    for i in 1..=steps {
        let (a, b) = at(d1 + (d2 - d1) * i as f32 / steps as f32);
        out.push((start + 40_000 + i * 16_000, TouchEvent::TwoMove(x, a, x, b)));
    }
    out.push((start + 40_000 + steps * 16_000 + 1000, TouchEvent::TwoUp));
    out
}

impl PanelPlatform {
    pub fn new(window: Rc<UiWindow>, display: Rc<RefCell<Display>>, touch: Option<Touch>) -> Self {
        let dev_input = std::env::var("NSPANEL_DEV_INPUT").ok().and_then(|path| {
            let c = std::ffi::CString::new(path).ok()?;
            // SAFETY: mkfifo/open on a C string path.
            unsafe {
                libc::mkfifo(c.as_ptr(), 0o600);
                let fd = libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC);
                (fd >= 0).then_some(fd)
            }
        });
        let _ = now_ms();
        Self {
            window,
            start: Instant::now(),
            display,
            touch: RefCell::new(touch),
            touch_retry: std::cell::Cell::new(now_ms() + 5000),
            wakeup: Wakeup::new(),
            dev_input,
            dev_buf: RefCell::new(String::new()),
            gesture: RefCell::new(VecDeque::new()),
            pinch: std::cell::Cell::new(None),
        }
    }

    /// A finger on the touch screen: the calibration's correction applied (set when the panel's
    /// touch test found a systematic offset).
    fn dispatch_finger(&self, event: TouchEvent) {
        let (dx, dy) = *TOUCH_OFFSET.lock().unwrap();
        let fix = |x: f32, y: f32| ((x - dx).clamp(0.0, 479.0), (y - dy).clamp(0.0, 479.0));
        self.dispatch_touch(match event {
            TouchEvent::Down(x, y) => {
                let (x, y) = fix(x, y);
                TouchEvent::Down(x, y)
            }
            TouchEvent::Move(x, y) => {
                let (x, y) = fix(x, y);
                TouchEvent::Move(x, y)
            }
            TouchEvent::Up(x, y) => {
                let (x, y) = fix(x, y);
                TouchEvent::Up(x, y)
            }
            TouchEvent::TwoDown(x1, y1, x2, y2) => {
                let ((x1, y1), (x2, y2)) = (fix(x1, y1), fix(x2, y2));
                TouchEvent::TwoDown(x1, y1, x2, y2)
            }
            TouchEvent::TwoMove(x1, y1, x2, y2) => {
                let ((x1, y1), (x2, y2)) = (fix(x1, y1), fix(x2, y2));
                TouchEvent::TwoMove(x1, y1, x2, y2)
            }
            TouchEvent::TwoUp => TouchEvent::TwoUp,
        });
    }

    fn dispatch_touch(&self, event: TouchEvent) {
        note_activity();
        if matches!(event, TouchEvent::Down(..)) {
            TOUCHES.fetch_add(1, Ordering::SeqCst);
        }
        // a finger resting on a weather page opens its edit lock (it needs no part of the page under it)
        match event {
            TouchEvent::Down(x, y) => crate::home_edit::finger(0, x, y),
            TouchEvent::Move(x, y) => crate::home_edit::finger(1, x, y),
            TouchEvent::Up(x, y) | TouchEvent::TwoDown(x, y, ..) => crate::home_edit::finger(2, x, y),
            TouchEvent::TwoMove(..) | TouchEvent::TwoUp => {}
        }
        let window_event = match event {
            TouchEvent::Down(x, y) => WindowEvent::PointerPressed {
                position: LogicalPosition::new(x, y),
                button: PointerEventButton::Left,
            },
            TouchEvent::Move(x, y) => WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) },
            TouchEvent::Up(x, y) => WindowEvent::PointerReleased {
                position: LogicalPosition::new(x, y),
                button: PointerEventButton::Left,
            },
            TouchEvent::TwoDown(..) | TouchEvent::TwoMove(..) | TouchEvent::TwoUp => {
                self.dispatch_two(event);
                PRESS.with(|p| p.borrow().as_ref().map(|f| f(false)));
                return;
            }
        };
        self.window.dispatch_event(window_event);
        if let TouchEvent::Up(..) = event {
            self.window.dispatch_event(WindowEvent::PointerExited);
        }
        let pressed = matches!(event, TouchEvent::Down(..) | TouchEvent::Move(..));
        PRESS.with(|p| p.borrow().as_ref().map(|f| f(pressed)));
    }

    /// Two fingers: the first finger's press lets go (it taps and drags nothing), and once the fingers have moved
    /// 8 px apart or together the UI gets a pinch at their midpoint (Slint's `ScaleRotateGestureHandler`). The
    /// scale follows the fingers' vertical spread: the hour grids zoom vertically, as in phone calendars.
    fn dispatch_two(&self, event: TouchEvent) {
        use i_slint_core::input::{BackendMouseEvent, TouchPhase};
        let pinch = |mid: (f32, f32), delta: f32, phase: TouchPhase| {
            WindowEvent::internal(BackendMouseEvent::PinchGesture {
                position: i_slint_core::lengths::LogicalPoint::new(mid.0 as _, mid.1 as _),
                delta,
                phase,
            })
        };
        let mut state = self.pinch.get();
        match event {
            TouchEvent::TwoDown(x1, y1, x2, y2) => {
                self.window.dispatch_event(WindowEvent::PointerExited);
                state = Some(Pinch { from: spread(y1, y2), scale: 1.0, mid: midpoint(x1, y1, x2, y2), started: false });
            }
            TouchEvent::TwoMove(x1, y1, x2, y2) => {
                if let Some(p) = state.as_mut() {
                    let (d, mid) = (spread(y1, y2), midpoint(x1, y1, x2, y2));
                    p.mid = mid;
                    if !p.started {
                        if (d - p.from).abs() > 8.0 {
                            p.started = true;
                            p.from = d;
                            self.window.dispatch_event(pinch(mid, 0.0, TouchPhase::Started));
                        }
                    } else {
                        let scale = d / p.from;
                        let delta = scale / p.scale - 1.0;
                        p.scale = scale;
                        self.window.dispatch_event(pinch(mid, delta, TouchPhase::Moved));
                    }
                }
            }
            _ => {
                if let Some(p) = state.take() {
                    if p.started {
                        self.window.dispatch_event(pinch(p.mid, 0.0, TouchPhase::Ended));
                    }
                }
            }
        }
        self.pinch.set(state);
    }

    /// Pictures of the screen asked for by SIGUSR1 (development) and by `request_screen`.
    fn serve_screen_requests(&self) {
        let shot = SCREENSHOT.swap(false, Ordering::SeqCst);
        let mut sinks = SCREEN_REQUESTS.with(|r| std::mem::take(&mut *r.borrow_mut()));
        if shot {
            sinks.push(Box::new(|w, h, rgb| {
                use std::io::Write;
                let mut out = Vec::with_capacity(rgb.len() + 32);
                let _ = write!(out, "P6\n{w} {h}\n255\n");
                out.extend_from_slice(&rgb);
                if let Err(e) = std::fs::write("/tmp/nspanel-screen.ppm", out) {
                    eprintln!("nspanel-app: screenshot failed: {e}");
                }
            }));
        }
        if sinks.is_empty() {
            return;
        }
        // The software renderer's front buffer, or a web buffer on screen, can be
        // read directly; a GPU frame is read back when the next one is drawn.
        let direct = cfg!(not(feature = "gpu")) || self.display.borrow().web_on_screen().is_some();
        if direct {
            let (w, h, rgb) = self.display.borrow().front_rgb();
            for sink in sinks {
                sink(w, h, rgb.clone());
            }
            return;
        }
        // The UI frame on screen is read from its buffer off the UI thread; only
        // before the first frame is on screen is the next one read back.
        #[cfg(feature = "gpu")]
        for sink in sinks {
            if let Err(sink) = self.window.read_screen(sink) {
                crate::gpu::read_back_next_frame(sink);
            }
        }
    }

    /// Draws the UI, or shows the newest web frame straight from its buffer while
    /// the web view is all there is on the screen.
    fn draw(&self, direct: bool, freed: &mut Vec<usize>) -> Result<(), PlatformError> {
        let started = crate::webstats::now_us();
        let flip_pending = self.display.borrow().flip_pending();
        // A picture of the screen is asked for while the UI's own (GPU) frame is
        // still on screen: draw one UI frame to read it back.
        #[cfg(feature = "gpu")]
        let direct = direct && !(crate::gpu::readback_pending() && self.display.borrow().web_on_screen().is_none());
        if direct {
            if flip_pending {
                return Ok(());
            }
            let next = WEB_NEXT.lock().unwrap().take();
            if let Some((i, times)) = next {
                // The helper's buffer straight to the screen.
                crate::webstats::frame_on_ui(times);
                let shown = self.display.borrow_mut().show_web(i);
                match shown {
                    Ok(()) => {
                        crate::webstats::drawn(started);
                        UI_UNTIL_WEB.with(|u| u.set(None));
                    }
                    Err(e) => {
                        eprintln!("nspanel-app: web frame to the screen: {e}");
                        freed.push(i);
                    }
                }
                return Ok(());
            }
            // no web frame yet since the web view took the screen over: the UI draws what went off it to its end
            if !UI_UNTIL_WEB.with(|u| u.get()).is_some_and(|until| Instant::now() < until) {
                return Ok(());
            }
        }
        if let Some((i, _)) = WEB_NEXT.lock().unwrap().take() {
            freed.push(i);
        }
        #[cfg(not(feature = "gpu"))]
        {
            if flip_pending {
                return Ok(());
            }
            let mut display = self.display.borrow_mut();
            let drawn = self.window.draw_if_needed(|renderer| {
                let (buffer, stride) = display.back_buffer();
                renderer.render(buffer, stride);
            });
            if drawn {
                display.flip().map_err(platform_error)?;
                crate::webstats::drawn(started);
            }
        }
        #[cfg(feature = "gpu")]
        {
            let _ = flip_pending;
            if self.window.draw_if_needed()? {
                crate::webstats::drawn(started);
            }
        }
        Ok(())
    }

    fn read_dev_input(&self, fd: RawFd) -> Vec<TouchEvent> {
        let mut buf = [0u8; 512];
        // SAFETY: read into a local buffer.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            return Vec::new();
        }
        let mut pending = self.dev_buf.borrow_mut();
        pending.push_str(&String::from_utf8_lossy(&buf[..n as usize]));
        let mut out = Vec::new();
        while let Some(pos) = pending.find('\n') {
            let line: String = pending.drain(..=pos).collect();
            // Development: JSON lines are app commands (show requests, assist states).
            if line.trim_start().starts_with('{') {
                DEV_COMMANDS.lock().unwrap().push(line.trim().to_string());
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 6 && parts[0] == "swipe" {
                let n: Vec<f32> = parts[1..].iter().filter_map(|v| v.parse().ok()).collect();
                if n.len() == 5 {
                    let start = crate::webstats::now_us();
                    let dur = (n[4].max(16.0) * 1000.0) as u64;
                    let steps = (dur / 16_000).max(1);
                    let mut g = self.gesture.borrow_mut();
                    g.push_back((start, TouchEvent::Down(n[0], n[1])));
                    for i in 1..=steps {
                        let k = i as f32 / steps as f32;
                        let (x, y) = (n[0] + (n[2] - n[0]) * k, n[1] + (n[3] - n[1]) * k);
                        g.push_back((start + i * 16_000, TouchEvent::Move(x, y)));
                    }
                    g.push_back((start + steps * 16_000 + 1000, TouchEvent::Up(n[2], n[3])));
                    crate::webstats::gesture("swipe", start + steps * 16_000);
                }
                continue;
            }
            if parts.len() == 6 && parts[0] == "pinch" {
                let n: Vec<f32> = parts[1..].iter().filter_map(|v| v.parse().ok()).collect();
                if n.len() == 5 {
                    let touches = pinch_touches(crate::webstats::now_us(), n[0], n[1], n[2], n[3], n[4]);
                    self.gesture.borrow_mut().extend(touches);
                }
                continue;
            }
            if parts.len() != 3 {
                continue;
            }
            let (Ok(x), Ok(y)) = (parts[1].parse::<f32>(), parts[2].parse::<f32>()) else { continue };
            match parts[0] {
                "tap" => {
                    crate::webstats::gesture("tap", crate::webstats::now_us());
                    out.push(TouchEvent::Down(x, y));
                    out.push(TouchEvent::Up(x, y));
                }
                "down" => out.push(TouchEvent::Down(x, y)),
                "move" => out.push(TouchEvent::Move(x, y)),
                "up" => out.push(TouchEvent::Up(x, y)),
                _ => {}
            }
        }
        out
    }
}

/// The drops on the glass move (GPU build; they are drawn only there).
#[cfg(feature = "gpu")]
fn glass_frames() -> bool {
    crate::glass::wants_frame()
}

#[cfg(not(feature = "gpu"))]
fn glass_frames() -> bool {
    false
}

/// JSON commands from the development input FIFO, handled by the UI timer.
pub static DEV_COMMANDS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn platform_error(e: std::io::Error) -> PlatformError {
    PlatformError::Other(e.to_string())
}

impl Platform for PanelPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(self.wakeup.clone()))
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        loop {
            self.wakeup.drain();
            if self.wakeup.quit.load(Ordering::SeqCst) {
                return Ok(());
            }
            self.serve_screen_requests();
            // Swipe from the development input: the touches that are due.
            loop {
                let due = {
                    let mut g = self.gesture.borrow_mut();
                    match g.front() {
                        Some(&(t, _)) if t <= crate::webstats::now_us() => g.pop_front().map(|(_, e)| e),
                        _ => None,
                    }
                };
                match due {
                    Some(event) => self.dispatch_touch(event),
                    None => break,
                }
            }
            crate::webstats::report_if_due();
            slint::platform::update_timers_and_animations();

            let direct = web_direct_now();
            let mut freed = std::mem::take(&mut *WEB_FREED.lock().unwrap());
            self.draw(direct, &mut freed)?;
            web_release(&freed);

            let flip_pending = self.display.borrow().flip_pending();
            let web_waiting = WEB_NEXT.lock().unwrap().is_some() && !flip_pending;
            let mut timeout_ms: i32 = if web_waiting {
                0
            } else if self.window.has_active_animations() || glass_frames() {
                // GPU: the next frame is drawn while this one waits for its flip
                // (the renderer blocks before its swap), so draw again at once.
                if cfg!(feature = "gpu") {
                    0
                } else if flip_pending {
                    -1
                } else {
                    16
                }
            } else {
                match slint::platform::duration_until_next_timer_update() {
                    Some(d) => d.as_millis().min(i32::MAX as u128) as i32,
                    None => -1,
                }
            };
            if let Some(&(t, _)) = self.gesture.borrow().front() {
                let wait = (t.saturating_sub(crate::webstats::now_us()) / 1000) as i32;
                timeout_ms = if timeout_ms < 0 { wait } else { timeout_ms.min(wait) };
            }
            // No touch device (gone when its driver was bound again): look for it every few seconds.
            if self.touch.borrow().is_none() {
                if now_ms() >= self.touch_retry.get() {
                    if let Ok(t) = Touch::open() {
                        eprintln!("nspanel-app: touch input: {}", t.name);
                        *self.touch.borrow_mut() = Some(t);
                    }
                    self.touch_retry.set(now_ms() + 5000);
                }
                if self.touch.borrow().is_none() {
                    let wait = self.touch_retry.get().saturating_sub(now_ms()) as i32;
                    timeout_ms = if timeout_ms < 0 { wait } else { timeout_ms.min(wait) };
                }
            }

            let display_fd = self.display.borrow().fd();
            let touch_fd = self.touch.borrow().as_ref().map(|t| t.fd()).unwrap_or(-1);
            let mut fds = [
                libc::pollfd { fd: display_fd, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: touch_fd, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: self.wakeup.fd, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: self.dev_input.unwrap_or(-1), events: libc::POLLIN, revents: 0 },
            ];
            // SAFETY: poll(2) on a local array; a negative fd is ignored.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(platform_error(err));
            }

            if fds[0].revents & libc::POLLIN != 0 {
                let was_pending = self.display.borrow().flip_pending();
                let released = self.display.borrow_mut().handle_events().map_err(platform_error)?;
                if was_pending && !self.display.borrow().flip_pending() {
                    crate::webstats::flipped();
                    #[cfg(feature = "gpu")]
                    self.window.flipped();
                }
                if let Some(i) = released {
                    web_release(&[i]);
                }
            }

            if fds[1].revents & libc::POLLIN != 0 {
                let events = self.touch.borrow_mut().as_mut().map(|t| t.read_events()).unwrap_or_default();
                for event in events {
                    self.dispatch_finger(event);
                }
            }
            if touch_fd >= 0 && fds[1].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                // the device went away: let it go (no busy loop on a dead descriptor) and look for it again
                eprintln!("nspanel-app: touch input gone");
                *self.touch.borrow_mut() = None;
                self.touch_retry.set(now_ms() + 1000);
            }

            if let Some(fd) = self.dev_input {
                if fds[3].revents & libc::POLLIN != 0 {
                    for event in self.read_dev_input(fd) {
                        self.dispatch_touch(event);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_pinch_spreads_two_fingers() {
        let t = pinch_touches(0, 240.0, 300.0, 100.0, 200.0, 160.0);
        assert_eq!(t[0], (0, TouchEvent::Down(240.0, 250.0)));
        assert_eq!(t[1], (40_000, TouchEvent::TwoDown(240.0, 250.0, 240.0, 350.0)));
        // ten moves 16 ms apart, the last one 200 px apart round the same middle
        assert_eq!(t.len(), 2 + 10 + 1);
        assert_eq!(t[11], (40_000 + 160_000, TouchEvent::TwoMove(240.0, 200.0, 240.0, 400.0)));
        assert_eq!(t[12].1, TouchEvent::TwoUp);
    }

    #[test]
    fn spread_is_vertical() {
        assert_eq!(spread(100.0, 300.0), 200.0);
        assert_eq!(spread(300.0, 100.0), 200.0);
        assert_eq!(spread(200.0, 205.0), 24.0);
        assert_eq!(midpoint(0.0, 100.0, 40.0, 300.0), (20.0, 200.0));
    }
}
