//! CST9220 touch reports -> pointer events for Slint.
//!
//! `cst92xx::CST92xx::touches()` reports the currently held contacts only; it
//! does not say whether a contact went down or up. `TouchTracker` keeps the
//! previous state so the run loop can emit press/move/release edges.

use cst92xx::Point;

/// A pointer edge, in panel (physical) pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchEvent {
    Press { x: u16, y: u16 },
    Move { x: u16, y: u16 },
    Release { x: u16, y: u16 },
}

/// Tracks whether a finger is currently down.
#[derive(Debug, Default)]
pub struct TouchTracker {
    active: bool,
    last: (u16, u16),
}

impl TouchTracker {
    pub const fn new() -> Self {
        Self {
            active: false,
            last: (0, 0),
        }
    }

    /// True while a finger is down, used to poll faster during a gesture.
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Compare a fresh report against the previous one and return the edge, if any.
    pub fn update(&mut self, points: &[Option<Point>]) -> Option<TouchEvent> {
        let point = points.iter().flatten().next();

        match (point, self.active) {
            (Some(p), false) => {
                self.active = true;
                self.last = (p.x, p.y);
                Some(TouchEvent::Press { x: p.x, y: p.y })
            }
            (Some(p), true) => {
                if (p.x, p.y) != self.last {
                    self.last = (p.x, p.y);
                    Some(TouchEvent::Move { x: p.x, y: p.y })
                } else {
                    None
                }
            }
            (None, true) => {
                self.active = false;
                Some(TouchEvent::Release {
                    x: self.last.0,
                    y: self.last.1,
                })
            }
            (None, false) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: u16, y: u16) -> Point {
        Point {
            track_id: 0,
            x,
            y,
            area: 0,
        }
    }

    #[test]
    fn emits_press_move_release() {
        let mut tracker = TouchTracker::new();
        let idle = [None, None];

        assert_eq!(
            tracker.update(&[Some(point(10, 20))]),
            Some(TouchEvent::Press { x: 10, y: 20 })
        );
        assert!(tracker.is_active());
        assert_eq!(tracker.update(&[Some(point(10, 20))]), None);
        assert_eq!(
            tracker.update(&[Some(point(30, 40))]),
            Some(TouchEvent::Move { x: 30, y: 40 })
        );
        assert_eq!(
            tracker.update(&idle),
            Some(TouchEvent::Release { x: 30, y: 40 })
        );
        assert!(!tracker.is_active());
        assert_eq!(tracker.update(&idle), None);
    }

    #[test]
    fn uses_first_active_contact() {
        let mut tracker = TouchTracker::new();
        let report = [None, Some(point(5, 6))];
        assert_eq!(
            tracker.update(&report),
            Some(TouchEvent::Press { x: 5, y: 6 })
        );
    }
}
