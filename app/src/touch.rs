//! Touch input straight from the kernel's evdev device (no libinput): the
//! single-touch emulation the multitouch driver provides (ABS_X, ABS_Y,
//! BTN_TOUCH) is the pointer; the multitouch slots (ABS_MT_*) only tell when a
//! second finger is down, which turns the touch into a two-finger gesture.

use std::fs;
use std::io;
use std::os::fd::RawFd;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const SYN_REPORT: u16 = 0x00;
/// The kernel's event buffer overflowed: everything up to the next SYN_REPORT is incomplete.
const SYN_DROPPED: u16 = 0x03;
const BTN_TOUCH: u16 = 0x14a;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;
/// The controller reports up to five fingers.
const SLOTS: usize = 10;

#[repr(C)]
struct InputEvent {
    time: libc::timeval,
    kind: u16,
    code: u16,
    value: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TouchEvent {
    Down(f32, f32),
    Move(f32, f32),
    Up(f32, f32),
    /// A second finger came down: both fingers' places. The first finger's touch is called off (it taps and drags
    /// nothing any more).
    TwoDown(f32, f32, f32, f32),
    /// The two fingers moved.
    TwoMove(f32, f32, f32, f32),
    /// One of the two fingers lifted; the finger left on the glass does nothing until it lifts too.
    TwoUp,
}

/// One multitouch slot: its contact's tracking id (-1: none) and place.
#[derive(Debug, Clone, Copy)]
struct Slot {
    id: i32,
    x: i32,
    y: i32,
}

/// What a report (one SYN_REPORT) becomes: the pointer of the single-touch emulation, or a two-finger gesture
/// while two contacts are down. Pure, so the rules can be tested without a device.
#[derive(Debug, Default)]
struct Frames {
    reported_down: bool,
    /// The tracking ids of the two fingers of a running gesture.
    pair: Option<(i32, i32)>,
    /// A gesture ended while a finger stays: nothing until every finger is up.
    hold: bool,
}

impl Frames {
    /// One report: `down` and (`x`, `y`) of the single-touch emulation (`moved`: they changed), the contacts down
    /// as (tracking id, x, y) in slot order (`mt_moved`: a contact moved).
    fn report(&mut self, down: bool, x: f32, y: f32, moved: bool, contacts: &[(i32, f32, f32)], mt_moved: bool) -> Vec<TouchEvent> {
        let mut out = Vec::new();
        let find = |id: i32| contacts.iter().find(|c| c.0 == id).map(|c| (c.1, c.2));
        if let Some((a, b)) = self.pair {
            match (find(a), find(b)) {
                (Some(p), Some(q)) => {
                    if mt_moved {
                        out.push(TouchEvent::TwoMove(p.0, p.1, q.0, q.1));
                    }
                }
                _ => {
                    out.push(TouchEvent::TwoUp);
                    self.pair = None;
                    self.hold = true;
                }
            }
        } else if contacts.len() >= 2 {
            // a first finger that came down in this same report goes down first, so its press is the one called off
            if down && !self.reported_down && !self.hold {
                out.push(TouchEvent::Down(x, y));
            }
            let (p, q) = (contacts[0], contacts[1]);
            out.push(TouchEvent::TwoDown(p.1, p.2, q.1, q.2));
            self.pair = Some((p.0, q.0));
            self.hold = false;
        } else if self.hold {
            // the finger left after a gesture lifts: no tap, no release (its press was called off)
        } else {
            match (self.reported_down, down) {
                (false, true) => out.push(TouchEvent::Down(x, y)),
                (true, true) if moved => out.push(TouchEvent::Move(x, y)),
                (true, false) => out.push(TouchEvent::Up(x, y)),
                _ => {}
            }
        }
        if !down {
            self.hold = false;
            self.pair = None;
        }
        self.reported_down = down;
        out
    }
}

pub struct Touch {
    fd: RawFd,
    pub name: String,
    x: i32,
    y: i32,
    down: bool,
    moved: bool,
    slots: [Slot; SLOTS],
    slot: usize,
    mt_moved: bool,
    /// Events were dropped: skip to the next SYN_REPORT, then take the touch state again from the device.
    dropping: bool,
    frames: Frames,
}

/// The contacts down in slot order. With no finger on the glass (BTN_TOUCH 0) every slot is empty, whatever a lost
/// "lifted" left in it: one stale contact would make every later one-finger touch a two-finger gesture.
fn live_contacts(slots: &mut [Slot], down: bool) -> Vec<(i32, f32, f32)> {
    if !down {
        for s in slots.iter_mut() {
            s.id = -1;
        }
    }
    slots.iter().filter(|s| s.id >= 0).map(|s| (s.id, s.x as f32, s.y as f32)).collect()
}

impl Touch {
    /// Opens the first input device whose name looks like a touchscreen.
    pub fn open() -> io::Result<Self> {
        let mut entries: Vec<_> = fs::read_dir("/sys/class/input")?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("event"))
            .collect();
        entries.sort();
        for entry in entries {
            let name = fs::read_to_string(format!("/sys/class/input/{entry}/device/name"))
                .unwrap_or_default()
                .trim()
                .to_string();
            if !name.to_lowercase().contains("touch") {
                continue;
            }
            let path = std::ffi::CString::new(format!("/dev/input/{entry}")).unwrap();
            // SAFETY: plain open(2) of a device node.
            let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            return Ok(Self {
                fd,
                name,
                x: 0,
                y: 0,
                down: false,
                moved: false,
                slots: [Slot { id: -1, x: 0, y: 0 }; SLOTS],
                slot: 0,
                mt_moved: false,
                dropping: false,
                frames: Frames::default(),
            });
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "no touchscreen input device"))
    }

    pub fn fd(&self) -> RawFd {
        self.fd
    }

    /// Whether a finger is on the glass now, asked from the device (EVIOCGKEY); None if the ioctl fails.
    fn touching_now(&self) -> Option<bool> {
        let mut keys = [0u8; 96]; // KEY_MAX 0x2ff
        // _IOC(_IOC_READ, 'E', 0x18, len)
        let req = (2u32 << 30) | ((keys.len() as u32) << 16) | (0x45 << 8) | 0x18;
        // SAFETY: the kernel writes at most `keys.len()` bytes into the buffer.
        let r = unsafe { libc::ioctl(self.fd, req as _, keys.as_mut_ptr()) };
        if r < 0 {
            return None;
        }
        let bit = BTN_TOUCH as usize;
        Some(keys[bit / 8] & (1 << (bit % 8)) != 0)
    }
}

impl Drop for Touch {
    fn drop(&mut self) {
        // SAFETY: the descriptor is ours and closed once.
        unsafe { libc::close(self.fd) };
    }
}

impl Touch {

    /// Reads everything available and returns the resulting pointer events.
    pub fn read_events(&mut self) -> Vec<TouchEvent> {
        let mut out = Vec::new();
        let size = std::mem::size_of::<InputEvent>();
        let mut buf = vec![0u8; size * 64];
        loop {
            // SAFETY: reading into a byte buffer of the given length.
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break;
            }
            for chunk in buf[..n as usize].chunks_exact(size) {
                // SAFETY: the kernel writes whole input_event structs.
                let ev = unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<InputEvent>()) };
                if self.dropping {
                    // the report the overflow cut is incomplete: no contact is trusted after it (a finger still down
                    // stays out of the gesture count until it lifts), the touch state comes from the device
                    if (ev.kind, ev.code) == (EV_SYN, SYN_REPORT) {
                        self.dropping = false;
                        for s in self.slots.iter_mut() {
                            s.id = -1;
                        }
                        if let Some(t) = self.touching_now() {
                            self.down = t;
                        }
                        let contacts = live_contacts(&mut self.slots, self.down);
                        out.extend(self.frames.report(self.down, self.x as f32, self.y as f32, false, &contacts, true));
                        self.moved = false;
                        self.mt_moved = false;
                    }
                    continue;
                }
                match (ev.kind, ev.code) {
                    (EV_ABS, ABS_X) => {
                        self.x = ev.value;
                        self.moved = true;
                    }
                    (EV_ABS, ABS_Y) => {
                        self.y = ev.value;
                        self.moved = true;
                    }
                    (EV_ABS, ABS_MT_SLOT) => self.slot = (ev.value.max(0) as usize).min(SLOTS - 1),
                    (EV_ABS, ABS_MT_TRACKING_ID) => {
                        self.slots[self.slot].id = ev.value;
                        self.mt_moved = true;
                    }
                    (EV_ABS, ABS_MT_POSITION_X) => {
                        self.slots[self.slot].x = ev.value;
                        self.mt_moved = true;
                    }
                    (EV_ABS, ABS_MT_POSITION_Y) => {
                        self.slots[self.slot].y = ev.value;
                        self.mt_moved = true;
                    }
                    (EV_KEY, BTN_TOUCH) => self.down = ev.value != 0,
                    (EV_SYN, SYN_DROPPED) => {
                        eprintln!("nspanel-app: touch: events dropped by the kernel, state read again");
                        self.dropping = true;
                    }
                    (EV_SYN, SYN_REPORT) => {
                        let contacts = live_contacts(&mut self.slots, self.down);
                        out.extend(self.frames.report(self.down, self.x as f32, self.y as f32, self.moved, &contacts, self.mt_moved));
                        self.moved = false;
                        self.mt_moved = false;
                    }
                    _ => {}
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_finger_is_the_pointer() {
        let mut f = Frames::default();
        assert_eq!(f.report(true, 10.0, 20.0, true, &[(5, 10.0, 20.0)], true), vec![TouchEvent::Down(10.0, 20.0)]);
        assert_eq!(f.report(true, 12.0, 22.0, true, &[(5, 12.0, 22.0)], true), vec![TouchEvent::Move(12.0, 22.0)]);
        assert_eq!(f.report(true, 12.0, 22.0, false, &[(5, 12.0, 22.0)], false), vec![]);
        assert_eq!(f.report(false, 12.0, 22.0, false, &[], true), vec![TouchEvent::Up(12.0, 22.0)]);
    }

    #[test]
    fn a_second_finger_turns_it_into_a_gesture() {
        let mut f = Frames::default();
        f.report(true, 100.0, 200.0, true, &[(1, 100.0, 200.0)], true);
        assert_eq!(
            f.report(true, 100.0, 200.0, false, &[(1, 100.0, 200.0), (2, 100.0, 300.0)], true),
            vec![TouchEvent::TwoDown(100.0, 200.0, 100.0, 300.0)]
        );
        // the pointer of the emulation does not move while two are down
        assert_eq!(
            f.report(true, 100.0, 190.0, true, &[(1, 100.0, 190.0), (2, 100.0, 320.0)], true),
            vec![TouchEvent::TwoMove(100.0, 190.0, 100.0, 320.0)]
        );
        // one lifts: the gesture ends, the other finger does nothing until it lifts
        assert_eq!(f.report(true, 100.0, 320.0, true, &[(2, 100.0, 320.0)], true), vec![TouchEvent::TwoUp]);
        assert_eq!(f.report(true, 100.0, 330.0, true, &[(2, 100.0, 330.0)], true), vec![]);
        assert_eq!(f.report(false, 100.0, 330.0, false, &[], true), vec![]);
        // the next touch is a plain one again
        assert_eq!(f.report(true, 50.0, 60.0, true, &[(3, 50.0, 60.0)], true), vec![TouchEvent::Down(50.0, 60.0)]);
    }

    #[test]
    fn two_fingers_landing_together() {
        let mut f = Frames::default();
        assert_eq!(
            f.report(true, 10.0, 10.0, true, &[(1, 10.0, 10.0), (2, 10.0, 90.0)], true),
            vec![TouchEvent::Down(10.0, 10.0), TouchEvent::TwoDown(10.0, 10.0, 10.0, 90.0)]
        );
        // both lift in one report
        assert_eq!(f.report(false, 10.0, 10.0, false, &[], true), vec![TouchEvent::TwoUp]);
        assert_eq!(f.report(true, 30.0, 30.0, true, &[(3, 30.0, 30.0)], true), vec![TouchEvent::Down(30.0, 30.0)]);
    }

    #[test]
    fn a_lost_lift_does_not_stick() {
        // slot 1 never got its "lifted" (dropped by the kernel); the finger of slot 0 lifts
        let mut slots = [Slot { id: -1, x: 0, y: 0 }; SLOTS];
        slots[1] = Slot { id: 7, x: 5, y: 5 };
        let mut f = Frames::default();
        assert_eq!(live_contacts(&mut slots, false), vec![]);
        f.report(false, 0.0, 0.0, false, &[], true);
        // the next touch has one contact again: a plain tap, not a gesture
        slots[0] = Slot { id: 8, x: 40, y: 50 };
        let c = live_contacts(&mut slots, true);
        assert_eq!(c, vec![(8, 40.0, 50.0)]);
        assert_eq!(f.report(true, 40.0, 50.0, true, &c, true), vec![TouchEvent::Down(40.0, 50.0)]);
    }

    #[test]
    fn a_third_finger_changes_nothing() {
        let mut f = Frames::default();
        f.report(true, 10.0, 10.0, true, &[(1, 10.0, 10.0)], true);
        f.report(true, 10.0, 10.0, false, &[(1, 10.0, 10.0), (2, 10.0, 90.0)], true);
        assert_eq!(
            f.report(true, 10.0, 10.0, false, &[(1, 10.0, 10.0), (2, 10.0, 95.0), (3, 200.0, 200.0)], true),
            vec![TouchEvent::TwoMove(10.0, 10.0, 10.0, 95.0)]
        );
        // one of the pair lifts while the third stays: the gesture ends
        assert_eq!(f.report(true, 10.0, 10.0, false, &[(1, 10.0, 10.0), (3, 200.0, 200.0)], true), vec![TouchEvent::TwoUp]);
    }
}
