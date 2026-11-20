//! The pages the panel visits by itself (a timer or an alarm rings, Home Assistant shows a view for some seconds) and
//! the way back: where it came from, what holds it away, and when someone paged meanwhile (then it stays).

use std::time::Instant;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
    fn leaving_a_temporary_page_it_came_from() {
        // on a temporary page at 3, a ring goes to the timers page at 6; the temporary page goes
        let v = Visit::base(None, 3).hold(6, Hold::Ring).removed(3);
        assert_eq!(v.from, 2);
        assert_eq!(v.at, 5);
        assert_eq!(v.end(true, 5, Instant::now()), (None, Some(2)));
    }
}
