//! The platform-neutral heart of the iOS library: the real `SessionManager`, the UI state and
//! one egui pass per frame. No GPU and no unsafe here, so it is tested on every host.

use std::time::Duration;

use egui::{Event, FullOutput, RawInput, ViewportId, ViewportInfo};
use krypa_core::{CoreError, ManagerConfig, SessionManager};
use krypa_engine_api::EngineCapabilities;
use krypa_ui::{AppState, FrameOutput, KrypaUi};

use crate::geometry::Viewport;
use crate::input::{Phase, TouchTracker};

/// Live web views kept at once. Nothing opens a web view yet; the value matches the
/// desktop harness so behaviour stays comparable.
const LIVE_VIEW_BUDGET: usize = 3;

/// What one frame produced.
pub struct Frame {
    /// Shapes and texture changes for the renderer.
    pub full: FullOutput,
    /// What the UI reported (covered rectangles, intents).
    pub ui: FrameOutput,
    /// How long the shell may leave the display link idle: zero means draw again at the next
    /// vsync, `None` means nothing is pending until the next input.
    pub repaint_after: Option<Duration>,
}

impl Drop for Frame {
    fn drop(&mut self) {
        // egui panics when a `TexturesDelta` is dropped unapplied. The GPU painter takes the
        // deltas out before this runs; what is left belongs to a frame nobody paints
        // (headless views, tests), where discarding is the intent.
        self.full.textures_delta.clear();
    }
}

/// UI plus core, driven by the shell one frame at a time.
pub struct App {
    ctx: egui::Context,
    /// Held so the core lives as long as the UI. No engine is attached yet (see
    /// docs/IOS_INTEGRATION.md): nothing calls into the manager until the WKWebView adapter
    /// exists, so there is no stub adapter to write.
    _manager: SessionManager,
    state: AppState,
    events: Vec<Event>,
    touches: TouchTracker,
    viewport: Viewport,
    intents_dropped: u64,
}

impl App {
    /// A launcher showing no sessions.
    ///
    /// # Errors
    /// Only if the view budget constant were zero.
    pub fn new(viewport: Viewport) -> Result<Self, CoreError> {
        let manager = SessionManager::new(
            ManagerConfig::new(LIVE_VIEW_BUDGET)?,
            EngineCapabilities::default(),
        );
        let mut state = AppState::new();
        state.sync_sessions(&manager);
        Ok(Self {
            ctx: egui::Context::default(),
            _manager: manager,
            state,
            events: Vec::new(),
            touches: TouchTracker::default(),
            viewport,
            intents_dropped: 0,
        })
    }

    /// The view changed size, scale or insets.
    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    /// The egui context, for tessellating the frame's shapes.
    #[must_use]
    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// Current geometry.
    #[must_use]
    pub fn viewport(&self) -> &Viewport {
        &self.viewport
    }

    /// Queues one touch (position in points).
    pub fn touch(&mut self, id: u64, phase: Phase, x: f32, y: f32) {
        self.touches
            .push(id, phase, egui::pos2(x, y), &mut self.events);
    }

    /// Whether input is waiting for the next frame.
    #[must_use]
    pub fn has_pending_input(&self) -> bool {
        !self.events.is_empty()
    }

    /// Intents the UI produced that nothing executed yet (navigation is not wired). Counted
    /// instead of silently lost, so a test or a log line can show a tap reached the core.
    #[must_use]
    pub fn intents_dropped(&self) -> u64 {
        self.intents_dropped
    }

    /// Runs one egui pass at `time` seconds (any monotonic clock).
    pub fn frame(&mut self, time: f64) -> Frame {
        let mut input = RawInput {
            screen_rect: Some(self.viewport.screen_rect()),
            safe_area_insets: Some(self.viewport.safe_area_insets()),
            time: Some(time),
            events: std::mem::take(&mut self.events),
            focused: true,
            ..RawInput::default()
        };
        // egui derives pixels_per_point from the native value of the root viewport.
        input.viewports.insert(
            ViewportId::ROOT,
            ViewportInfo {
                native_pixels_per_point: Some(self.viewport.scale()),
                ..ViewportInfo::default()
            },
        );
        let state = &mut self.state;
        let mut ui = FrameOutput::default();
        let full = self.ctx.run_pass(input, |ctx| {
            ui = KrypaUi::frame(ctx, state);
        });
        self.intents_dropped += ui.intents.len() as u64;
        let delay = full
            .viewport_output
            .get(&ViewportId::ROOT)
            .map(|v| v.repaint_delay);
        let repaint_after = if ui.animating {
            Some(Duration::ZERO)
        } else {
            // egui reports "no repaint wanted" as a very long delay.
            delay.filter(|d| *d < Duration::from_secs(3600))
        };
        Frame {
            full,
            ui,
            repaint_after,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Insets;

    fn app() -> App {
        let insets = Insets {
            top: 59.0,
            bottom: 34.0,
            ..Insets::default()
        };
        App::new(Viewport::new(1179, 2556, 3.0, insets).unwrap()).unwrap()
    }

    /// The frames egui needs to settle its first layout passes.
    fn settle(app: &mut App) -> Frame {
        let mut t = 0.0;
        let mut f = app.frame(t);
        for _ in 0..4 {
            t += 0.016;
            f = app.frame(t);
        }
        f
    }

    #[test]
    fn first_frame_paints_the_launcher_at_the_requested_scale() {
        let mut app = app();
        let f = settle(&mut app);
        // 3x display: egui must tessellate for 3 pixels per point, or text is blurry/tiny.
        assert_eq!(f.full.pixels_per_point, 3.0);
        assert!(!f.full.shapes.is_empty(), "the launcher drew nothing");
    }

    #[test]
    fn the_launcher_stays_out_of_the_safe_area() {
        // Everything painted must lie below the notch inset and above the home indicator.
        let mut app = app();
        let f = settle(&mut app);
        let (top, bottom) = (59.0, 852.0 - 34.0);
        let text_rects: Vec<egui::Rect> = f
            .full
            .shapes
            .iter()
            .filter(|s| matches!(s.shape, egui::epaint::Shape::Text(_)))
            .map(|s| s.shape.visual_bounding_rect())
            .filter(|r| r.is_finite())
            .collect();
        assert!(!text_rects.is_empty(), "no text was drawn");
        for r in text_rects {
            assert!(r.min.y >= top - 0.5, "text {r:?} is under the notch");
            assert!(r.max.y <= bottom + 0.5, "text {r:?} is under the home bar");
        }
    }

    #[test]
    fn idle_launcher_does_not_ask_for_continuous_redraw() {
        // Battery: with nothing animating the display link may sleep.
        let mut app = app();
        let f = settle(&mut app);
        assert!(f.repaint_after.is_none_or(|d| d > Duration::ZERO));
    }

    #[test]
    fn touches_are_queued_until_the_next_frame_consumes_them() {
        let mut app = app();
        assert!(!app.has_pending_input());
        app.touch(1, Phase::Began, 10.0, 10.0);
        assert!(app.has_pending_input());
        app.frame(0.0);
        assert!(!app.has_pending_input());
    }

    #[test]
    fn a_resize_changes_the_screen_egui_lays_out() {
        let mut app = app();
        settle(&mut app);
        app.set_viewport(Viewport::new(1000, 500, 2.0, Insets::default()).unwrap());
        let f = settle(&mut app);
        assert_eq!(f.full.pixels_per_point, 2.0);
    }
}
