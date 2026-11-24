//! Where the web view goes, and whether it may be on screen at all.
//!
//! The web view is a native `UIView` above the Metal view, so it hides whatever Rust draws
//! underneath. Rust therefore tells Swift, after every frame, which session's web view to
//! show and in which rectangle (points, in the Metal view's coordinates), and Swift removes
//! the web view whenever Rust says it must not be there.

use egui::Rect;
use krypa_engine_api::SessionId;

/// Tolerance when asking whether one rectangle lies inside another, in points.
const EPS: f32 = 0.5;

/// The web view's placement for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Session whose web view is shown; `None` means no web view is on screen.
    pub session: Option<SessionId>,
    /// Rectangle in points. Meaningless when `session` is `None`.
    pub rect: Rect,
}

impl Placement {
    /// Nothing is shown.
    pub const NONE: Self = Self {
        session: None,
        rect: Rect::NOTHING,
    };
}

fn contains(outer: Rect, inner: Rect) -> bool {
    outer.min.x <= inner.min.x + EPS
        && outer.min.y <= inner.min.y + EPS
        && outer.max.x >= inner.max.x - EPS
        && outer.max.y >= inner.max.y - EPS
}

/// The placement for a frame. The web view is shown only if the UI reported a rectangle for
/// it, the rectangle is real, and nothing the UI draws covers it entirely: an error view or
/// a modal dialog is drawn into the Metal layer and the native web view would hide it.
/// While a dialog is open the page is therefore removed, which is the price of a native
/// view above the layer (a snapshot placeholder is a later improvement).
#[must_use]
pub fn placement(
    foreground: Option<SessionId>,
    web_view_rect: Option<Rect>,
    covered_rects: &[Rect],
) -> Placement {
    let (Some(session), Some(rect)) = (foreground, web_view_rect) else {
        return Placement::NONE;
    };
    if !rect.is_finite() || rect.width() < 1.0 || rect.height() < 1.0 {
        return Placement::NONE;
    }
    if covered_rects.iter().any(|c| contains(*c, rect)) {
        return Placement::NONE;
    }
    Placement {
        session: Some(session),
        rect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), vec2(w, h))
    }

    const S: SessionId = SessionId::new(1);

    /// Why: the web view must sit exactly between the toolbars the UI reported (below the
    /// status bar and above the home indicator), not under them.
    #[test]
    fn the_web_view_gets_exactly_the_content_rectangle_between_the_toolbars() {
        let content = r(0.0, 110.0, 393.0, 640.0);
        let covered = [r(0.0, 0.0, 393.0, 110.0), r(0.0, 750.0, 393.0, 102.0)];
        let p = placement(Some(S), Some(content), &covered);
        assert_eq!(p.session, Some(S));
        assert_eq!(p.rect, content);
    }

    #[test]
    fn an_error_view_over_the_page_area_removes_the_web_view() {
        // The error view is painted in the Metal layer; a web view on top would hide it.
        let content = r(0.0, 110.0, 393.0, 640.0);
        assert_eq!(
            placement(Some(S), Some(content), &[content]),
            Placement::NONE
        );
    }

    #[test]
    fn a_modal_dialog_covering_the_screen_removes_the_web_view() {
        let content = r(0.0, 110.0, 393.0, 640.0);
        let screen = r(0.0, 0.0, 393.0, 852.0);
        assert_eq!(
            placement(Some(S), Some(content), &[screen]),
            Placement::NONE
        );
    }

    #[test]
    fn no_foreground_session_or_no_rect_means_no_web_view() {
        let content = r(0.0, 110.0, 393.0, 640.0);
        assert_eq!(placement(None, Some(content), &[]), Placement::NONE);
        assert_eq!(placement(Some(S), None, &[]), Placement::NONE);
    }

    #[test]
    fn degenerate_or_non_finite_rectangles_are_never_handed_to_swift() {
        for bad in [
            r(0.0, 0.0, 0.0, 100.0),
            r(0.0, 0.0, 100.0, 0.5),
            r(f32::NAN, 0.0, 10.0, 10.0),
            Rect::NOTHING,
        ] {
            assert_eq!(placement(Some(S), Some(bad), &[]), Placement::NONE);
        }
    }

    #[test]
    fn a_toolbar_that_only_touches_the_content_does_not_hide_it() {
        let content = r(0.0, 110.0, 393.0, 640.0);
        let top = r(0.0, 0.0, 393.0, 110.0);
        assert!(placement(Some(S), Some(content), &[top]).session.is_some());
    }
}
