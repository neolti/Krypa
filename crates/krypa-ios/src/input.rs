//! UIKit touches to egui events, replacing what egui-winit does on other platforms.
//!
//! Rules copied from egui-winit (see docs/IOS_INTEGRATION.md 3.4): the first finger down
//! drives the pointer; began = move + primary press; moved = move; ended = release then
//! pointer gone; cancelled = pointer gone without a release (so a tap that iOS steals, for
//! example by a system gesture, never counts as a click). Every touch also becomes an
//! `Event::Touch`. Positions are points in the view, which is what UIKit reports.

use egui::{Event, Modifiers, PointerButton, Pos2, TouchDeviceId, TouchId};

/// Phase of a UIKit touch (`UITouch.Phase`, without stationary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// `.began`.
    Began,
    /// `.moved`.
    Moved,
    /// `.ended`.
    Ended,
    /// `.cancelled`.
    Cancelled,
}

/// Tracks which finger drives the pointer and turns touches into egui events.
#[derive(Debug, Default)]
pub struct TouchTracker {
    primary: Option<u64>,
}

impl TouchTracker {
    /// Events for one touch. `id` identifies the finger for the touch's lifetime (the shell
    /// derives it from the `UITouch` object address).
    pub fn push(&mut self, id: u64, phase: Phase, pos: Pos2, out: &mut Vec<Event>) {
        let touch_phase = match phase {
            Phase::Began => egui::TouchPhase::Start,
            Phase::Moved => egui::TouchPhase::Move,
            Phase::Ended => egui::TouchPhase::End,
            Phase::Cancelled => egui::TouchPhase::Cancel,
        };
        out.push(Event::Touch {
            device_id: TouchDeviceId(0),
            id: TouchId(id),
            phase: touch_phase,
            pos,
            force: None,
        });

        if phase == Phase::Began && self.primary.is_none() {
            self.primary = Some(id);
        }
        if self.primary != Some(id) {
            return;
        }
        let press = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        match phase {
            Phase::Began => {
                out.push(Event::PointerMoved(pos));
                out.push(press(true));
            }
            Phase::Moved => out.push(Event::PointerMoved(pos)),
            Phase::Ended => {
                out.push(press(false));
                out.push(Event::PointerGone);
                self.primary = None;
            }
            Phase::Cancelled => {
                out.push(Event::PointerGone);
                self.primary = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn run(steps: &[(u64, Phase)]) -> Vec<Event> {
        let mut t = TouchTracker::default();
        let mut out = Vec::new();
        for (id, phase) in steps {
            t.push(*id, *phase, pos2(10.0, 20.0), &mut out);
        }
        out
    }

    fn pointer_events(events: &[Event]) -> Vec<&Event> {
        events
            .iter()
            .filter(|e| !matches!(e, Event::Touch { .. }))
            .collect()
    }

    #[test]
    fn a_tap_is_move_press_release_gone_at_the_touch_position() {
        let events = run(&[(1, Phase::Began), (1, Phase::Ended)]);
        let p = pointer_events(&events);
        assert!(matches!(p[0], Event::PointerMoved(pos) if *pos == pos2(10.0, 20.0)));
        assert!(matches!(p[1], Event::PointerButton { pressed: true, .. }));
        assert!(matches!(p[2], Event::PointerButton { pressed: false, .. }));
        assert!(matches!(p[3], Event::PointerGone));
        assert_eq!(p.len(), 4);
    }

    #[test]
    fn a_cancelled_touch_never_releases_so_it_cannot_click() {
        // iOS cancels a touch when a system gesture takes over; releasing here would fire
        // a button the user did not mean to press.
        let events = run(&[(1, Phase::Began), (1, Phase::Cancelled)]);
        let p = pointer_events(&events);
        assert!(!p
            .iter()
            .any(|e| matches!(e, Event::PointerButton { pressed: false, .. })));
        assert!(matches!(p.last(), Some(Event::PointerGone)));
    }

    #[test]
    fn a_second_finger_does_not_steal_the_pointer() {
        let events = run(&[
            (1, Phase::Began),
            (2, Phase::Began),
            (2, Phase::Moved),
            (2, Phase::Ended),
        ]);
        // Only finger 1 produced pointer events (move + press); finger 2 is Touch-only.
        assert_eq!(pointer_events(&events).len(), 2);
        let touches = events
            .iter()
            .filter(|e| matches!(e, Event::Touch { .. }))
            .count();
        assert_eq!(touches, 4);
    }

    #[test]
    fn the_pointer_is_free_again_after_the_first_finger_lifts() {
        let events = run(&[
            (1, Phase::Began),
            (1, Phase::Ended),
            (2, Phase::Began),
            (2, Phase::Ended),
        ]);
        assert_eq!(pointer_events(&events).len(), 8);
    }

    #[test]
    fn moves_of_a_finger_that_never_began_are_ignored_for_the_pointer() {
        let events = run(&[(9, Phase::Moved)]);
        assert!(pointer_events(&events).is_empty());
    }
}
