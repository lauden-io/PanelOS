//! The pages the panel visits by itself (a timer or an alarm rings, Home Assistant shows a view for some seconds) and
//! the way back: where it came from, what holds it away, and when someone paged meanwhile (then it stays).

use std::time::{Duration, Instant};

/// What holds a visit the panel made by itself on its page.
#[derive(Clone, Copy, Debug)]
pub enum Hold {
    /// A timer or an alarm rings (until it is stopped or its minute passes).
    Ring,
    /// Home Assistant shows a view until then.
    Show(Instant),
}

/// A visit the panel made by itself: the page it came from, the page it went to last (`at`), the ringing screen's
/// page while a timer or an alarm rings, the shown view's page and when its time is up. Pages are places in the pager,
/// kept in step when a temporary page comes or goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visit {
    pub from: i32,
    pub at: i32,
    pub ring: Option<i32>,
    pub show: Option<(i32, Instant)>,
}

impl Visit {
    /// The visit a new one builds on while `page` shows: the one before while the panel is still where that one
    /// took it, else a new one from `page` (someone paged meanwhile: back to where they were).
    pub fn base(prev: Option<Visit>, page: i32) -> Visit {
        match prev {
            Some(v) if v.at == page => v,
            _ => Visit { from: page, at: page, ring: None, show: None },
        }
    }

    pub fn hold(mut self, to: i32, hold: Hold) -> Visit {
        match hold {
            Hold::Ring => self.ring = Some(to),
            Hold::Show(until) => self.show = Some((to, until)),
        }
        self.at = to;
        self
    }

    /// The ring (`ring`) or the shown view ends while `page` shows: the visit left (None: over) and the page to go to
    /// (None: stay). Someone who paged while it held ends the visit where they are. What still holds takes the panel
    /// back (the ringing screen over the shown view); with nothing left it goes back to where it came from.
    pub fn end(mut self, ring: bool, page: i32, now: Instant) -> (Option<Visit>, Option<i32>) {
        let ended = if ring { self.ring.take().is_some() } else { self.show.take().is_some() };
        if !ended {
            return (Some(self), None);
        }
        if page != self.at {
            return (None, None);
        }
        if self.show.is_some_and(|(_, until)| until <= now) {
            self.show = None;
        }
        match self.ring.or(self.show.map(|(p, _)| p)) {
            Some(p) => {
                self.at = p;
                (Some(self), (p != page).then_some(p))
            }
            None => (None, (self.from != page).then_some(self.from)),
        }
    }

    /// The page a visit goes to is a temporary page at `at`: a new one (`new`) moved the pages from there on up by one;
    /// one that took over the temporary page the visit came from (`took_over`) sends the way back to the page that one
    /// was opened next to.
    pub fn temporary(self, at: i32, new: bool, took_over: bool) -> Visit {
        if new {
            self.inserted(at)
        } else if took_over && self.from == at {
            Visit { from: (at - 1).max(0), ..self }
        } else {
            self
        }
    }

    /// A temporary page came in at `t`: the pages from there on moved up by one.
    pub fn inserted(self, t: i32) -> Visit {
        self.map(|i| if i >= t { i + 1 } else { i })
    }

    /// The temporary page at `t` went: the pages after it moved down by one. Coming from it means coming from the page
    /// it was opened next to; being on it means the panel left it (as if someone paged).
    pub fn removed(self, t: i32) -> Visit {
        let from = self.from;
        let mut v = self.map(|i| if i > t { i - 1 } else if i == t { -1 } else { i });
        if from == t {
            v.from = (t - 1).max(0);
        }
        v
    }

    fn map(mut self, f: impl Fn(i32) -> i32) -> Visit {
        self.from = f(self.from);
        self.at = f(self.at);
        self.ring = self.ring.map(&f);
        self.show = self.show.map(|(p, u)| (f(p), u));
        self
    }
}

/// The page kind a view of Home Assistant's `panelos.show_view` shows; None for the start page (`home`, the first page
/// in the pager), also for a view the panel does not know. (`dashboard` is the web view, not a page.)
pub fn view_kind(view: &str) -> Option<&'static str> {
    match view {
        "weather" => Some("weather"),
        "media" => Some("music"),
        "camera" => Some("camera"),
        _ => None,
    }
}

/// A view shown until someone touches the screen (`seconds: 0`) holds while the panel's touch count is still the one
/// it came with (`since`).
pub fn held(since: Option<u64>, touches: u64) -> bool {
    since == Some(touches)
}

/// After a view shown until touched was touched, the display mode it changed (the dashboard over the pages, or the
/// pages over the dashboard) comes back once nobody has used the panel for this long: the screensaver's time, or two
/// minutes when the screensaver never starts.
pub fn back_after(screensaver_seconds: u32) -> Duration {
    Duration::from_secs(if screensaver_seconds > 0 { screensaver_seconds as u64 } else { 120 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn later(s: u64) -> Instant {
        Instant::now() + Duration::from_secs(s)
    }

    #[test]
    fn a_ring_goes_back_where_it_came_from() {
        let v = Visit::base(None, 2).hold(5, Hold::Ring);
        assert_eq!(v.end(true, 5, Instant::now()), (None, Some(2)));
    }

    #[test]
    fn paging_during_a_ring_stays() {
        let v = Visit::base(None, 2).hold(5, Hold::Ring);
        assert_eq!(v.end(true, 3, Instant::now()), (None, None));
    }

    #[test]
    fn a_stop_with_nothing_ringing_keeps_a_shown_view() {
        let v = Visit::base(None, 0).hold(4, Hold::Show(later(30)));
        assert_eq!(v.end(true, 4, Instant::now()), (Some(v), None));
    }

    #[test]
    fn a_view_ending_during_a_ring_waits_for_the_ring() {
        let now = Instant::now();
        let v = Visit::base(None, 0).hold(4, Hold::Show(later(8)));
        let v = Visit::base(Some(v), 4).hold(6, Hold::Ring);
        let (left, go) = v.end(false, 6, now);
        assert_eq!(go, None);
        let left = left.unwrap();
        assert_eq!(left.end(true, 6, now), (None, Some(0)));
    }

    #[test]
    fn a_ring_ending_during_a_view_goes_to_the_view() {
        let v = Visit::base(None, 0).hold(4, Hold::Show(later(30)));
        let v = Visit::base(Some(v), 4).hold(6, Hold::Ring);
        let (left, go) = v.end(true, 6, Instant::now());
        assert_eq!(go, Some(4));
        assert_eq!(left.unwrap().end(false, 4, Instant::now()), (None, Some(0)));
    }

    #[test]
    fn a_view_shown_during_a_ring_returns_to_the_ring() {
        let v = Visit::base(None, 1).hold(6, Hold::Ring);
        let v = Visit::base(Some(v), 6).hold(3, Hold::Show(later(10)));
        let (left, go) = v.end(false, 3, Instant::now());
        assert_eq!(go, Some(6));
        assert_eq!(left.unwrap().end(true, 6, Instant::now()), (None, Some(1)));
    }

    #[test]
    fn a_second_visit_after_paging_starts_from_the_new_page() {
        let v = Visit::base(None, 0).hold(4, Hold::Show(later(30)));
        // someone paged to 2, then a timer rang
        let v = Visit::base(Some(v), 2).hold(6, Hold::Ring);
        assert_eq!(v.from, 2);
        assert_eq!(v.show, None);
        assert_eq!(v.end(true, 6, Instant::now()), (None, Some(2)));
    }

    #[test]
    fn a_temporary_page_keeps_the_numbers_in_step() {
        // from page 5 a view at 1, then a ring on a temporary page opened at 2 (pages from 2 on move up)
        let v = Visit::base(None, 5).hold(1, Hold::Show(later(30)));
        let v = Visit::base(Some(v), 1).inserted(2).hold(2, Hold::Ring);
        assert_eq!(v.from, 6);
        let (left, go) = v.end(true, 2, Instant::now());
        assert_eq!(go, Some(1));
        // the temporary page goes once left: the page it came from is back at 5
        let left = left.unwrap().removed(2);
        assert_eq!(left.end(false, 1, Instant::now()), (None, Some(5)));
    }

    #[test]
    fn a_view_on_a_new_temporary_page_keeps_the_way_back_in_step() {
        // from page 4 a view opens a temporary page at 5: the way back stays 4, a ring's timers page at 6 moves to 7
        let v = Visit::base(None, 6).hold(6, Hold::Ring);
        let moved = v.temporary(5, true, false);
        assert_eq!((moved.from, moved.ring), (7, Some(7)));
        let v = Visit::base(None, 4).temporary(5, true, false).hold(5, Hold::Show(later(30)));
        assert_eq!(v.from, 4);
        assert_eq!(v.end(false, 5, Instant::now()), (None, Some(4)));
    }

    #[test]
    fn a_view_taking_over_the_temporary_page_it_came_from_goes_back_next_to_it() {
        // on a temporary page at 3 (opened next to 2), a view takes it over for another kind
        let v = Visit::base(None, 3).temporary(3, false, true).hold(3, Hold::Show(later(30)));
        assert_eq!(v.from, 2);
        assert_eq!(v.end(false, 3, Instant::now()), (None, Some(2)));
        // a page of the pager (no temporary page): nothing moves
        let v = Visit::base(None, 3);
        assert_eq!(v.temporary(1, false, false), v);
    }

    #[test]
    fn views_go_to_their_page_kinds() {
        assert_eq!(view_kind("weather"), Some("weather"));
        assert_eq!(view_kind("media"), Some("music"));
        assert_eq!(view_kind("camera"), Some("camera"));
        // the start page
        assert_eq!(view_kind("home"), None);
        assert_eq!(view_kind("something new"), None);
    }

    #[test]
    fn a_view_until_touched_holds_until_the_next_touch() {
        assert!(held(Some(7), 7));
        assert!(!held(Some(7), 8));
        assert!(!held(None, 7));
    }

    #[test]
    fn the_display_mode_comes_back_after_the_screensaver_time() {
        assert_eq!(back_after(300), Duration::from_secs(300));
        assert_eq!(back_after(0), Duration::from_secs(120));
    }

    #[test]
    fn leaving_a_temporary_page_it_came_from() {
        // on a temporary page at 3, a ring goes to the timers page at 6; the temporary page goes
        let v = Visit::base(None, 3).hold(6, Hold::Ring).removed(3);
        assert_eq!(v.from, 2);
        assert_eq!(v.at, 5);
        assert_eq!(v.end(true, 5, Instant::now()), (None, Some(2)));
    }
}
