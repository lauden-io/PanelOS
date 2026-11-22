// PanelOS: not part of Slint. Which items are moving on the screen, so that only
// those are drawn at their exact sub-pixel position (itemrenderer.rs,
// `render_and_blend_layer` and `draw_text`): a moving layer or text glides
// instead of stepping a whole pixel at a time, and a still one is snapped to
// whole pixels as upstream does, so nothing at rest gets softer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::time::{Duration, Instant};

use i_slint_core::item_tree::ItemTreeRef;
use i_slint_core::items::ItemRc;

/// How long after its last step an item still counts as moving: a motion that
/// steps a few times a second stays smooth, and an item that stopped settles
/// onto whole pixels this long afterwards.
const HOLD: Duration = Duration::from_millis(200);

/// Smaller changes of a device position than this are not a move.
const EPSILON: f32 = 1e-3;

/// `PANELOS_ALIGN_LAYERS=1`: cached layers are always snapped to whole pixels (upstream; measurements).
pub fn align_layers() -> bool {
    static ALIGN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ALIGN.get_or_init(|| std::env::var_os("PANELOS_ALIGN_LAYERS").is_some())
}

/// `PANELOS_ALIGN_TEXT=1`: text is always snapped to whole pixels (upstream; measurements).
pub fn align_text() -> bool {
    static ALIGN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ALIGN.get_or_init(|| std::env::var_os("PANELOS_ALIGN_TEXT").is_some())
}

#[derive(Clone, Copy)]
struct Axis {
    pos: f32,
    last: Option<Instant>,
    prev: Option<Instant>,
}

impl Axis {
    fn new(pos: f32) -> Self {
        Self { pos, last: None, prev: None }
    }

    /// Notes the position drawn now. True while the axis moves: it changed within
    /// `HOLD`, and so did the change before (a single jump, as when a layout
    /// changes, does not count, so that item stays snapped).
    fn step(&mut self, pos: f32, now: Instant) -> bool {
        if (pos - self.pos).abs() > EPSILON {
            self.pos = pos;
            self.prev = self.last;
            self.last = Some(now);
        }
        match (self.last, self.prev) {
            (Some(last), Some(prev)) => {
                now.saturating_duration_since(last) < HOLD
                    && last.saturating_duration_since(prev) < HOLD
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy)]
struct Motion {
    x: Axis,
    y: Axis,
}

/// The last device position of each layer and text item drawn, per component
/// (the same keys as Slint's `ItemCache`). No allocation per frame: an entry is
/// made the first time an item is drawn and goes with its component.
#[derive(Default)]
pub struct MotionCache {
    map: RefCell<HashMap<*const u8, HashMap<u32, Motion>>>,
}

impl MotionCache {
    /// Notes where the item is drawn now (device pixels) and returns whether it
    /// moves along x and along y.
    pub fn moving(&self, item_rc: &ItemRc, x: f32, y: f32, now: Instant) -> (bool, bool) {
        let component = &**item_rc.item_tree() as *const _ as *const u8;
        let mut map = self.map.borrow_mut();
        match map.entry(component).or_default().entry(item_rc.index()) {
            Entry::Occupied(mut entry) => {
                let motion = entry.get_mut();
                (motion.x.step(x, now), motion.y.step(y, now))
            }
            Entry::Vacant(entry) => {
                entry.insert(Motion { x: Axis::new(x), y: Axis::new(y) });
                (false, false)
            }
        }
    }

    /// Must be called when a component is destroyed.
    pub fn component_destroyed(&self, component: ItemTreeRef) {
        let component = ItemTreeRef::as_ptr(component).as_ptr() as *const u8;
        self.map.borrow_mut().remove(&component);
    }

    pub fn clear_all(&self) {
        self.map.borrow_mut().clear();
    }
}
