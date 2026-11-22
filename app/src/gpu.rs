//! GPU output (feature `gpu`): Skia draws the UI with OpenGL ES (Mesa Panfrost
//! on the Mali-G31, libraries from the web bundle) into a GBM surface, and the
//! surface's buffers go to the screen with page flips through [`Display`].
//!
//! The GBM / EGL glue follows Slint's KMS backend (i-slint-backend-linuxkms
//! 1.18: display/gbmdisplay.rs and renderer/skia.rs, GPL-3.0, © SixtyFPS GmbH),
//! adapted to this app's own event loop, display and web view buffers.

use std::cell::{Cell, RefCell};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use drm014::control::{Device as _, FbCmd2Flags};
use gbm::AsRaw;
use i_slint_renderer_skia::{opengl_surface::OpenGLSurface, SkiaRenderer, SkiaSharedContext};
use slint::platform::{Renderer, WindowAdapter, WindowEvent};
use slint::{PhysicalSize, PlatformError, Window};

use crate::display::Display;

/// The DRM device again (a duplicated descriptor), for the GBM device: GBM's
/// crate speaks drm 0.14, the rest of the app drm 0.15. Both only issue ioctls
/// on the same kernel device.
struct GbmCard(std::fs::File);

impl AsFd for GbmCard {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl drm014::Device for GbmCard {}
impl drm014::control::Device for GbmCard {}

/// Raw GBM pointers for EGL (glutin) through raw-window-handle.
struct GbmHandles {
    device: std::ptr::NonNull<std::ffi::c_void>,
    surface: std::ptr::NonNull<std::ffi::c_void>,
}

impl raw_window_handle::HasWindowHandle for GbmHandles {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: the surface lives as long as the program (it is leaked in `Gpu::new`).
        Ok(unsafe {
            raw_window_handle::WindowHandle::borrow_raw(raw_window_handle::RawWindowHandle::Gbm(
                raw_window_handle::GbmWindowHandle::new(self.surface),
            ))
        })
    }
}

impl raw_window_handle::HasDisplayHandle for GbmHandles {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: the device lives as long as the program (it is leaked in `Gpu::new`).
        Ok(unsafe {
            raw_window_handle::DisplayHandle::borrow_raw(raw_window_handle::RawDisplayHandle::Gbm(
                raw_window_handle::GbmDisplayHandle::new(self.device),
            ))
        })
    }
}

/// GBM buffers of the UI on their way to the screen.
struct Buffers {
    surface: &'static gbm::Surface<u32>,
    device: &'static gbm::Device<GbmCard>,
    /// On screen.
    shown: RefCell<Option<gbm::BufferObject<u32>>>,
    /// Waiting for its page flip.
    pending: RefCell<Option<gbm::BufferObject<u32>>>,
    /// A picture of the shown buffer is being read (set when the read ends).
    shown_read: RefCell<Option<Arc<AtomicBool>>>,
    /// Buffers that left the screen while a picture of them was being read:
    /// kept from GBM (not drawn into) until the read ends.
    held: RefCell<Vec<(gbm::BufferObject<u32>, Arc<AtomicBool>)>>,
}

impl Buffers {
    /// A buffer leaves the screen: back to GBM, or held while it is read.
    fn retire(&self, bo: gbm::BufferObject<u32>) {
        if let Some(done) = self.shown_read.borrow_mut().take() {
            if !done.load(Ordering::Acquire) {
                self.held.borrow_mut().push((bo, done));
            }
        }
        self.held.borrow_mut().retain(|(_, done)| !done.load(Ordering::Acquire));
    }
}

/// The window: Slint's window plus the Skia renderer on the GBM surface.
pub struct GpuWindow {
    window: Window,
    renderer: SkiaRenderer,
    size: PhysicalSize,
    needs_redraw: Cell<bool>,
    buffers: Rc<Buffers>,
    display: Rc<RefCell<Display>>,
}

impl std::ops::Deref for GpuWindow {
    type Target = Window;
    fn deref(&self) -> &Window {
        &self.window
    }
}

impl WindowAdapter for GpuWindow {
    fn window(&self) -> &Window {
        &self.window
    }

    fn renderer(&self) -> &dyn Renderer {
        &self.renderer
    }

    fn size(&self) -> PhysicalSize {
        self.size
    }

    fn request_redraw(&self) {
        self.needs_redraw.set(true);
    }
}

/// Development: log frame times every 5 s (NSPANEL_FRAME_STATS or the dev input).
pub static FRAME_STATS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Stats {
    since: std::time::Instant,
    last: Option<std::time::Instant>,
    render: Vec<f32>,
    interval: Vec<f32>,
}

fn pct(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f32 - 1.0) * p).round() as usize]
}

thread_local! {
    /// Development: frames drawn because a property changed / an animation runs / a picture was asked for.
    static WHY: std::cell::Cell<(u32, u32, u32)> = const { std::cell::Cell::new((0, 0, 0)) };
}

fn note_frame(render_ms: f32, start: std::time::Instant) {
    use std::sync::atomic::Ordering;
    if !FRAME_STATS.load(Ordering::Relaxed) {
        return;
    }
    let wait = FLIP_WAIT.with(|w| w.replace(0.0));
    let recorded = T_RECORDED.with(|c| c.take());
    let present = T_PRESENT.with(|c| c.take());
    if render_ms > 40.0 {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 100_000).unwrap_or(0);
        let ms = |a: std::time::Instant, b: std::time::Instant| b.saturating_duration_since(a).as_secs_f32() * 1000.0;
        let (rec, flush) = match (recorded, present) {
            (Some(r), Some(p)) => (ms(start, r), ms(r, p)),
            _ => (-1.0, -1.0),
        };
        let swap = present.map(|p| render_ms - ms(start, p) - wait).unwrap_or(-1.0);
        eprintln!("nspanel-app: slow frame at {t} ms: render {render_ms:.1} ms = items {rec:.1} + flush {flush:.1} + flip wait {wait:.1} + swap {swap:.1}");
    }
    STATS.with(|st| {
        let mut st = st.borrow_mut();
        let now = std::time::Instant::now();
        let s = st.get_or_insert_with(|| Stats { since: now, last: None, render: Vec::new(), interval: Vec::new() });
        if let Some(l) = s.last.replace(now) {
            s.interval.push(now.duration_since(l).as_secs_f32() * 1000.0);
        }
        s.render.push(render_ms);
        let secs = s.since.elapsed().as_secs_f32();
        if secs >= 5.0 {
            let n = s.render.len();
            let why = WHY.with(|w| w.replace((0, 0, 0)));
            eprintln!(
                "nspanel-app: frames {:.1}/s (asked {}, animating {}, picture {}) | interval p50 {:.1} p99 {:.1} max {:.1} ms | render p50 {:.1} p99 {:.1} max {:.1} ms",
                n as f32 / secs,
                why.0,
                why.1,
                why.2,
                pct(&mut s.interval, 0.5),
                pct(&mut s.interval, 0.99),
                pct(&mut s.interval, 1.0),
                pct(&mut s.render, 0.5),
                pct(&mut s.render, 0.99),
                pct(&mut s.render, 1.0)
            );
            *st = None;
        }
    });
}

thread_local! {
    /// Time spent waiting for the previous page flip inside the last render (ms).
    static FLIP_WAIT: Cell<f32> = const { Cell::new(0.0) };
    /// Phases of the frame being drawn: items recorded, pre-present reached.
    static T_RECORDED: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
    static T_PRESENT: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
    static STATS: RefCell<Option<Stats>> = const { RefCell::new(None) };
    /// Called with what is on the screen (width, height, RGB rows top down)
    /// after the next frame is drawn.
    static READBACK: RefCell<Vec<Box<dyn FnOnce(u32, u32, Vec<u8>)>>> = const { RefCell::new(Vec::new()) };
    /// GL functions for reading the frame back and for the passes drawn after Skia.
    static GL: RefCell<Option<Rc<glow::Context>>> = const { RefCell::new(None) };
}

/// Asks for a picture of the next frame (development screenshot, the screen
/// pictures for Home Assistant). The frame is drawn at once if nothing else is.
pub fn read_back_next_frame(sink: impl FnOnce(u32, u32, Vec<u8>) + 'static) {
    READBACK.with(|r| r.borrow_mut().push(Box::new(sink)));
}

/// Whether a picture of the next frame is waiting.
pub fn readback_pending() -> bool {
    READBACK.with(|r| !r.borrow().is_empty())
}

/// The GL context's functions, loaded through EGL (libEGL.so.1 from the bundle).
fn load_gl() -> Option<Rc<glow::Context>> {
    GL.with(|g| {
        if g.borrow().is_none() {
            // SAFETY: dlopen / dlsym on constant C strings; the library stays loaded.
            let get = unsafe {
                let lib = libc::dlopen(c"libEGL.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
                if lib.is_null() {
                    return None;
                }
                let sym = libc::dlsym(lib, c"eglGetProcAddress".as_ptr());
                if sym.is_null() {
                    return None;
                }
                std::mem::transmute::<*mut libc::c_void, extern "C" fn(*const libc::c_char) -> *const libc::c_void>(sym)
            };
            // SAFETY: eglGetProcAddress returns the current context's functions.
            let gl = unsafe { glow::Context::from_loader_function_cstr(|name| get(name.as_ptr())) };
            *g.borrow_mut() = Some(Rc::new(gl));
        }
        g.borrow().clone()
    })
}

/// Reads the back buffer (after Skia's flush, before the swap) for the waiting sinks.
fn serve_readbacks(width: u32, height: u32) {
    let sinks: Vec<_> = READBACK.with(|r| std::mem::take(&mut *r.borrow_mut()));
    if sinks.is_empty() {
        return;
    }
    let Some(gl) = load_gl() else { return };
    use glow::HasContext;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    // SAFETY: reads the bound draw framebuffer (the window surface) into a buffer of the right size.
    unsafe {
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(0, 0, width as i32, height as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut rgba)));
    }
    let (w, h) = (width as usize, height as usize);
    let mut rgb = Vec::with_capacity(w * h * 3);
    // GL rows run bottom up.
    for y in (0..h).rev() {
        for px in rgba[y * w * 4..(y + 1) * w * 4].chunks_exact(4) {
            rgb.extend_from_slice(&px[..3]);
        }
    }
    for sink in sinks {
        sink(width, height, rgb.clone());
    }
}

impl GpuWindow {
    /// Creates the GBM surface on the display's device and the Skia renderer on it.
    pub fn new(display: Rc<RefCell<Display>>) -> Result<Rc<Self>, PlatformError> {
        let (width, height, fd) = {
            let d = display.borrow();
            (d.width, d.height, d.dup_fd().map_err(|e| PlatformError::Other(format!("dup DRM fd: {e}")))?)
        };
        let card = GbmCard(std::fs::File::from(OwnedFd::from(fd)));
        let device: &'static gbm::Device<GbmCard> = Box::leak(Box::new(
            gbm::Device::new(card).map_err(|e| PlatformError::Other(format!("GBM device: {e}")))?,
        ));
        let surface: &'static gbm::Surface<u32> = Box::leak(Box::new(
            device
                .create_surface::<u32>(
                    width,
                    height,
                    gbm::Format::Xrgb8888,
                    gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING,
                )
                .map_err(|e| PlatformError::Other(format!("GBM surface: {e}")))?,
        ));
        let handles = Arc::new(GbmHandles {
            device: std::ptr::NonNull::new(device.as_raw() as *mut std::ffi::c_void).expect("GBM device"),
            surface: std::ptr::NonNull::new(surface.as_raw() as *mut std::ffi::c_void).expect("GBM surface"),
        });
        let size = PhysicalSize::new(width, height);
        // Mali reports BAD_MATCH for an XRGB GBM surface with an 8-bit alpha config.
        let config = glutin::config::ConfigTemplateBuilder::new().with_transparency(false).with_alpha_size(0);
        let filter = |c: &glutin::config::Config| match c {
            glutin::config::Config::Egl(egl) => egl.native_visual() == gbm::Format::Xrgb8888 as u32,
            #[allow(unreachable_patterns)]
            _ => false,
        };
        if std::env::var_os("NSPANEL_FRAME_STATS").is_some() {
            FRAME_STATS.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let gl_surface = OpenGLSurface::new_with_config(handles.clone(), handles, size, None, config, Some(&filter))?;
        let renderer = SkiaRenderer::new_with_surface(&SkiaSharedContext::default(), Box::new(gl_surface));
        let buffers = Rc::new(Buffers {
            surface,
            device,
            shown: RefCell::new(None),
            pending: RefCell::new(None),
            shown_read: RefCell::new(None),
            held: RefCell::new(Vec::new()),
        });

        // Before each swap: the previous frame must have reached the screen
        // (at most one frame queued), then anything that reads the frame.
        {
            let display = Rc::downgrade(&display);
            let buffers = Rc::downgrade(&buffers);
            renderer.set_pre_present_callback(Some(Box::new(move || {
                // rain on the glass over the finished frame (before the pictures read it)
                if let Some(gl) = load_gl() {
                    crate::glass_gl::draw(&gl, width as i32, height as i32);
                }
                T_PRESENT.with(|c| c.set(Some(std::time::Instant::now())));
                if let (Some(display), Some(buffers)) = (display.upgrade(), buffers.upgrade()) {
                    wait_for_flip(&display, &buffers);
                }
                serve_readbacks(width, height);
            })));
        }

        let win = Rc::new_cyclic(|weak: &Weak<GpuWindow>| GpuWindow {
            window: Window::new(weak.clone() as Weak<dyn WindowAdapter>),
            renderer,
            size,
            needs_redraw: Cell::new(true),
            buffers,
            display,
        });
        // Development frame statistics: when the items are drawn into the canvas (before Skia's flush)
        let _ = win.window.set_rendering_notifier(|state, _| {
            if matches!(state, slint::RenderingState::AfterRendering) {
                T_RECORDED.with(|c| c.set(Some(std::time::Instant::now())));
            }
        });
        win.window.dispatch_event(WindowEvent::Resized { size: size.to_logical(1.0) });
        Ok(win)
    }

    /// Draws and queues the frame for the screen if something changed.
    /// Returns whether a frame was drawn.
    pub fn draw_if_needed(&self) -> Result<bool, PlatformError> {
        let readback = READBACK.with(|r| !r.borrow().is_empty());
        let asked = self.needs_redraw.take();
        let animating = self.window.has_active_animations();
        let glass = crate::glass::wants_frame();
        if !asked && !animating && !readback && !glass {
            return Ok(false);
        }
        // Development: why frames are drawn (a property changed, an animation runs, a picture was asked for)
        if FRAME_STATS.load(Ordering::Relaxed) {
            WHY.with(|w| {
                let mut v = w.get();
                v.0 += asked as u32;
                v.1 += animating as u32;
                v.2 += readback as u32;
                w.set(v);
            });
        }
        let started = std::time::Instant::now();
        // the drops move on to this frame
        crate::glass::prepare();
        {
            use i_slint_renderer_skia::SkiaRendererExt;
            let _ = self.renderer.render_transformed(0.0, (0.0, 0.0), self.size)?;
        }
        note_frame(started.elapsed().as_secs_f32() * 1000.0, started);
        // Development: when the window stops / starts asking for animation frames.
        if FRAME_STATS.load(Ordering::Relaxed) {
            thread_local!(static ACTIVE: Cell<bool> = const { Cell::new(false) });
            let active = self.window.has_active_animations();
            if ACTIVE.with(|a| a.replace(active)) != active {
                eprintln!("nspanel-app: animations {}", if active { "running" } else { "stopped" });
            }
        }
        self.present().map_err(|e| PlatformError::Other(format!("present: {e}")))?;
        Ok(true)
    }

    /// The buffer Skia just swapped in goes to the screen at the next vertical blank.
    fn present(&self) -> std::io::Result<()> {
        // SAFETY: called right after eglSwapBuffers (inside render), as GBM requires.
        let mut bo = unsafe { self.buffers.surface.lock_front_buffer() }.map_err(std::io::Error::other)?;
        let fb = match bo.userdata() {
            Some(&fb) => fb,
            None => {
                let flags = match drm014::buffer::PlanarBuffer::modifier(&bo) {
                    Some(m) if !matches!(m, drm014::buffer::DrmModifier::Invalid) => FbCmd2Flags::MODIFIERS,
                    _ => FbCmd2Flags::empty(),
                };
                let fb: u32 = self.buffers.device.add_planar_framebuffer(&bo, flags)?.into();
                bo.set_userdata(fb);
                fb
            }
        };
        self.display.borrow_mut().flip_raw(fb)?;
        *self.buffers.pending.borrow_mut() = Some(bo);
        Ok(())
    }

    /// A page flip completed (called by the event loop after `Display::handle_events`).
    pub fn flipped(&self) {
        flip_done(&self.display, &self.buffers);
    }

    /// A picture of the UI frame on screen, read off the UI thread from the
    /// buffer itself: no extra frame, no GPU stall (reading back from GL waits
    /// for the GPU and then reads uncached memory, ~65 ms on the panel, which
    /// was a visible hitch every time Home Assistant's page asked for the
    /// screen). Gives the sink back when no UI buffer is on screen.
    pub fn read_screen(&self, sink: ScreenSink) -> Result<(), ScreenSink> {
        let shown = self.buffers.shown.borrow();
        let Some(bo) = shown.as_ref() else { return Err(sink) };
        if !self.display.borrow().gbm_on_screen() {
            return Err(sink);
        }
        // Only plain linear buffers can be read as pixels.
        let linear = matches!(bo.modifier(), gbm::Modifier::Linear | gbm::Modifier::Invalid);
        if !linear {
            return Err(sink);
        }
        SCREEN_SINKS.with(|s| s.borrow_mut().push(sink));
        // A read of this buffer is under way: its picture serves this sink too.
        if self.buffers.shown_read.borrow().as_ref().is_some_and(|d| !d.load(Ordering::Acquire)) {
            return Ok(());
        }
        let (width, height, stride) = (self.size.width, self.size.height, bo.stride());
        let fd = match bo.fd() {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("nspanel-app: screen buffer export: {e}");
                let sinks = SCREEN_SINKS.with(|s| std::mem::take(&mut *s.borrow_mut()));
                drop(shown);
                for sink in sinks {
                    read_back_next_frame(sink);
                }
                return Ok(());
            }
        };
        let done = Arc::new(AtomicBool::new(false));
        *self.buffers.shown_read.borrow_mut() = Some(done.clone());
        std::thread::spawn(move || {
            // below the UI's own threads: the screen stays smooth while a picture is taken
            // SAFETY: setpriority on this thread's own id.
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 19);
            }
            let started = std::time::Instant::now();
            let rgb = read_dmabuf(&fd, width, height, stride);
            drop(fd);
            done.store(true, Ordering::Release);
            if FRAME_STATS.load(Ordering::Relaxed) {
                eprintln!("nspanel-app: screen picture read in {:.1} ms (off the UI thread)", started.elapsed().as_secs_f32() * 1000.0);
            }
            let _ = slint::invoke_from_event_loop(move || {
                let sinks = SCREEN_SINKS.with(|s| std::mem::take(&mut *s.borrow_mut()));
                match rgb {
                    Some(rgb) => {
                        for sink in sinks {
                            sink(width, height, rgb.clone());
                        }
                    }
                    None => {
                        eprintln!("nspanel-app: screen buffer could not be mapped, reading the next frame back");
                        for sink in sinks {
                            read_back_next_frame(sink);
                        }
                    }
                }
            });
        });
        Ok(())
    }
}

/// What on-screen pictures are waiting for (see [`GpuWindow::read_screen`]).
pub type ScreenSink = Box<dyn FnOnce(u32, u32, Vec<u8>)>;

thread_local! {
    static SCREEN_SINKS: RefCell<Vec<ScreenSink>> = const { RefCell::new(Vec::new()) };
}

/// After a flip: the buffer that was pending is on screen now (if the UI's
/// own buffer was flipped), the one before goes back to GBM.
fn flip_done(display: &Rc<RefCell<Display>>, buffers: &Buffers) {
    let d = display.borrow();
    if d.flip_pending() {
        return;
    }
    if d.gbm_on_screen() {
        let next = buffers.pending.borrow_mut().take();
        if let Some(bo) = next {
            let old = buffers.shown.borrow_mut().replace(bo);
            if let Some(old) = old {
                buffers.retire(old);
            }
        }
    } else {
        // A web buffer is on screen: the UI's buffers are free again.
        let old = buffers.shown.borrow_mut().take();
        if let Some(old) = old {
            buffers.retire(old);
        }
        buffers.pending.borrow_mut().take();
    }
}

/// What a scanout buffer shows, read through its dma-buf (XRGB8888, linear):
/// RGB rows top down. Runs off the UI thread; the buffer is not drawn into
/// meanwhile (it is on screen, or held by [`Buffers::retire`]).
fn read_dmabuf(fd: &OwnedFd, width: u32, height: u32, stride: u32) -> Option<Vec<u8>> {
    use std::os::fd::AsRawFd;
    const DMA_BUF_IOCTL_SYNC: libc::c_ulong = 0x4008_6200;
    const SYNC_READ: u64 = 1;
    const SYNC_END: u64 = 4;
    let len = (stride * height) as usize;
    // SAFETY: a read-only shared mapping of the whole buffer, unmapped below.
    let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
    if ptr == libc::MAP_FAILED {
        return None;
    }
    let sync = |flags: u64| {
        // SAFETY: DMA_BUF_IOCTL_SYNC with a struct dma_buf_sync { u64 flags }.
        unsafe { libc::ioctl(fd.as_raw_fd(), DMA_BUF_IOCTL_SYNC as _, &flags as *const u64) };
    };
    sync(SYNC_READ);
    // One copy out of the (uncached) mapping, then the conversion in cached memory.
    let mut raw = vec![0u8; len];
    // SAFETY: the mapping is `len` bytes long and readable.
    raw.copy_from_slice(unsafe { std::slice::from_raw_parts(ptr as *const u8, len) });
    sync(SYNC_READ | SYNC_END);
    // SAFETY: unmaps the mapping made above.
    unsafe { libc::munmap(ptr, len) };
    let (w, h, s) = (width as usize, height as usize, stride as usize);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        // XRGB8888 little endian: B, G, R, X.
        for px in raw[y * s..y * s + w * 4].chunks_exact(4) {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    Some(rgb)
}

/// Blocks until no page flip is pending (inside rendering, before the swap).
fn wait_for_flip(display: &Rc<RefCell<Display>>, buffers: &Buffers) {
    let started = std::time::Instant::now();
    wait_for_flip_inner(display, buffers);
    FLIP_WAIT.with(|w| w.set(w.get() + started.elapsed().as_secs_f32() * 1000.0));
}

fn wait_for_flip_inner(display: &Rc<RefCell<Display>>, buffers: &Buffers) {
    loop {
        let (pending, fd) = {
            let d = display.borrow();
            (d.flip_pending(), d.fd())
        };
        if !pending {
            break;
        }
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: poll(2) on one local pollfd.
        let n = unsafe { libc::poll(&mut pfd, 1, 100) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if n == 0 {
            continue;
        }
        let released = display.borrow_mut().handle_events();
        match released {
            Ok(Some(i)) => crate::platform::web_buffer_freed(i),
            Ok(None) => {}
            Err(e) => {
                eprintln!("nspanel-app: DRM events: {e}");
                break;
            }
        }
        flip_done(display, buffers);
    }
}
