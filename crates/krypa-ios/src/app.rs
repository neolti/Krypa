//! The platform-neutral heart of the iOS library: the real `SessionManager`, the UI state and
//! one egui pass per frame. No GPU and no unsafe here, so it is tested on every host.
//!
//! This is also where the iOS wiring lives (the counterpart of the desktop harness's
//! `wiring.rs`): UI intents go to `SessionManager`, the commands it returns wait in a queue
//! the Swift adapter pulls, and facts from the adapter come back as [`HostEvent`]s.

use std::collections::VecDeque;
use std::time::Duration;

use egui::{Event, FullOutput, RawInput, ViewportId, ViewportInfo};
use krypa_core::{
    CoreError, ManagerConfig, NavigationDecision, Outcome, SessionManager, SessionSpec,
};
use krypa_engine_api::{
    EngineCapabilities, EngineCommand, EngineEvent, NavigationRequest, NavigationTarget,
    ProfileKind, SessionId,
};
use krypa_navigation::SearchConfig;
use krypa_ui::app_icons::IconDecodeError;
use krypa_ui::{AppState, FrameOutput, Intent, KrypaUi, Screen};
use url::Url;

use crate::geometry::Viewport;
use crate::input::{Phase, TouchTracker};
use crate::placement::{placement, Placement};
use crate::wire::HostEvent;

/// Live web views kept at once; matches the desktop harness so behaviour stays comparable.
const LIVE_VIEW_BUDGET: usize = 3;

/// What the web view may do with one navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDecision {
    /// Let the web view navigate.
    Allow,
    /// Refuse (blocked, handled in Rust, or not understood: fail closed).
    Cancel,
}

/// What one frame produced.
pub struct Frame {
    /// Shapes and texture changes for the renderer.
    pub full: FullOutput,
    /// What the UI reported (covered rectangles, intents already executed).
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
    manager: SessionManager,
    state: AppState,
    events: Vec<Event>,
    touches: TouchTracker,
    viewport: Viewport,
    intents_dropped: u64,
    /// Commands for the WKWebView adapter that Swift has not pulled yet, in order.
    commands: VecDeque<EngineCommand>,
    placement: Placement,
    search: SearchConfig,
}

impl App {
    /// A launcher showing no sessions.
    ///
    /// # Errors
    /// Only if the view budget constant were zero.
    pub fn new(viewport: Viewport) -> Result<Self, CoreError> {
        // Truthful capabilities: every flag is false because none is implemented on iOS
        // yet (no browser entitlement, no profiles, no private mode, no manifest fetch).
        let manager = SessionManager::new(
            ManagerConfig::new(LIVE_VIEW_BUDGET)?,
            EngineCapabilities::default(),
        );
        let mut state = AppState::new();
        state.sync_sessions(&manager);
        Ok(Self {
            ctx: egui::Context::default(),
            manager,
            state,
            events: Vec::new(),
            touches: TouchTracker::default(),
            viewport,
            intents_dropped: 0,
            commands: VecDeque::new(),
            placement: Placement::NONE,
            search: SearchConfig::default(),
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

    /// Intents the iOS app cannot execute yet (Add to Krypa, installed apps, a link typed in
    /// the launcher). Counted and logged instead of silently lost.
    #[must_use]
    pub fn intents_dropped(&self) -> u64 {
        self.intents_dropped
    }

    /// What the UI draws from (read-only).
    #[must_use]
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// The session manager (read-only), for tests and diagnostics.
    #[must_use]
    pub fn manager(&self) -> &SessionManager {
        &self.manager
    }

    /// Where the web view goes after the last frame.
    #[must_use]
    pub fn placement(&self) -> Placement {
        self.placement
    }

    /// The next command without removing it.
    #[must_use]
    pub fn peek_command(&self) -> Option<&EngineCommand> {
        self.commands.front()
    }

    /// The next command for the web-view adapter, oldest first.
    pub fn next_command(&mut self) -> Option<EngineCommand> {
        self.commands.pop_front()
    }

    /// Feeds one fact from the adapter to the core.
    ///
    /// # Errors
    /// The core refused it: unknown or destroyed session (a stale event), or an event that
    /// makes no sense in the session's state. The refused event changed nothing.
    pub fn push_host_event(&mut self, event: HostEvent) -> Result<(), CoreError> {
        let known_url = self
            .manager
            .info(event.session())
            .ok()
            .and_then(|i| i.current_url);
        for engine_event in event.into_engine_events(known_url.as_ref()) {
            match self.manager.handle_event(engine_event) {
                Ok(outcome) => self.apply(outcome),
                Err(e) => {
                    self.state.sync_sessions(&self.manager);
                    return Err(e);
                }
            }
        }
        self.state.sync_sessions(&self.manager);
        Ok(())
    }

    /// The synchronous answer for a navigation the web view asks about. Anything the core
    /// does not accept (unknown session, bad state) is `Cancel`: fail closed.
    ///
    /// Side effects happen here, in Rust, so Swift only ever acts on Allow/Cancel: an
    /// external scheme becomes an `ExternalSchemeRequested` event for the core (which shows
    /// its own prompt; nothing is opened directly), and a user-activated link that leaves an
    /// installed app, or a new-window request, opens a new session.
    pub fn decide_navigation(
        &mut self,
        session: SessionId,
        request: &NavigationRequest,
    ) -> NavDecision {
        let Ok(decision) = self.manager.navigation_policy(session, request) else {
            return NavDecision::Cancel;
        };
        let result = match decision {
            NavigationDecision::Allow if request.target != NavigationTarget::NewWindow => {
                return NavDecision::Allow;
            }
            // A new window is never handed a web view by Swift, so it is never "allowed".
            NavigationDecision::Allow | NavigationDecision::Cancel(_) => {
                return NavDecision::Cancel;
            }
            NavigationDecision::OpenExternally => {
                self.manager
                    .handle_event(EngineEvent::ExternalSchemeRequested {
                        session,
                        url: request.url.clone(),
                        user_gesture: request.user_gesture,
                    })
            }
            NavigationDecision::OpenInNewSession => self
                .manager
                .open_new_session(session, &request.url)
                .map(|(_, outcome)| outcome),
        };
        match result {
            Ok(outcome) => self.apply(outcome),
            Err(e) => eprintln!("krypa: navigation side effect refused: {e}"),
        }
        self.state.sync_sessions(&self.manager);
        NavDecision::Cancel
    }

    /// Executes what the UI asked for, the way the desktop harness wiring does.
    pub fn execute_intents(&mut self, intents: Vec<Intent>) {
        for intent in intents {
            if let Err(e) = self.execute(intent) {
                // Core errors never contain URLs or titles, so they are safe to log.
                eprintln!("krypa: core refused an intent: {e}");
            }
        }
        self.state.sync_sessions(&self.manager);
    }

    fn execute(&mut self, intent: Intent) -> Result<(), CoreError> {
        match intent {
            Intent::Navigate(url) => self.open(url.as_url().clone()),
            Intent::Search(query) => match self.search.search_url(&query) {
                Ok(url) => self.open(url.as_url().clone()),
                Err(_) => {
                    self.intents_dropped += 1;
                    Ok(())
                }
            },
            Intent::GoBack(id) => self.call(id, SessionManager::go_back),
            Intent::GoForward(id) => self.call(id, SessionManager::go_forward),
            Intent::Reload(id) => self.call(id, SessionManager::reload),
            Intent::Stop(id) => self.call(id, SessionManager::stop),
            Intent::SwitchTo(id) => self.call(id, SessionManager::foreground),
            Intent::CloseSession(id) => self.call(id, SessionManager::close_session),
            Intent::RespondExternalOpen { session, allow } => {
                let outcome = self.manager.respond_external_open(session, allow)?;
                self.apply(outcome);
                Ok(())
            }
            Intent::RespondPermission {
                session,
                request,
                decision,
            } => {
                let outcome = self
                    .manager
                    .respond_permission(session, request, decision)?;
                self.apply(outcome);
                Ok(())
            }
            // Nothing to persist yet; the setting already changed in the UI state.
            Intent::SettingsChanged => Ok(()),
            Intent::FetchIcon(url) => {
                // No host-side icon fetching on iOS yet: say so instead of leaving it pending.
                self.state
                    .icons
                    .finish(&url, Err(IconDecodeError::Unavailable));
                Ok(())
            }
            // Not implemented on iOS: no manifest fetch, no storage, no installed apps, and a
            // link typed in the launcher has no session to ask from.
            Intent::AddToKrypa(_)
            | Intent::ConfirmInstall(_)
            | Intent::OpenApp(_)
            | Intent::OpenExternal(_) => {
                self.intents_dropped += 1;
                eprintln!("krypa: an intent is not supported on iOS yet");
                Ok(())
            }
        }
    }

    fn call(
        &mut self,
        id: SessionId,
        f: fn(&mut SessionManager, SessionId) -> Result<Outcome, CoreError>,
    ) -> Result<(), CoreError> {
        let outcome = f(&mut self.manager, id)?;
        self.apply(outcome);
        Ok(())
    }

    /// Opens `url` in a new foreground session.
    fn open(&mut self, url: Url) -> Result<(), CoreError> {
        let id = self.manager.create_session(SessionSpec {
            profile: ProfileKind::Default,
            url: Some(url),
            app_scope: None,
        })?;
        let outcome = self.manager.foreground(id)?;
        self.apply(outcome);
        self.state.screen = Screen::Browser;
        Ok(())
    }

    fn apply(&mut self, outcome: Outcome) {
        self.commands.extend(outcome.commands);
        for notice in &outcome.notices {
            self.state.apply_notice(notice);
        }
    }

    /// Test-only entry: open a start URL as if the user had typed it. Exists only when the
    /// `debug-start-url` feature is enabled; release builds do not contain it.
    ///
    /// # Errors
    /// The text is not an `http(s)` URL the core accepts.
    #[cfg(feature = "debug-start-url")]
    pub fn debug_open_url(&mut self, raw: &str) -> Result<(), String> {
        let url = krypa_navigation::NavigationUrl::parse(raw).map_err(|e| e.to_string())?;
        self.open(url.as_url().clone()).map_err(|e| e.to_string())?;
        self.state.sync_sessions(&self.manager);
        Ok(())
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
        let intents = std::mem::take(&mut ui.intents);
        let ran_intents = !intents.is_empty();
        self.execute_intents(intents);
        self.placement = if self.state.screen == Screen::Browser {
            placement(self.state.foreground, ui.web_view_rect, &ui.covered_rects)
        } else {
            Placement::NONE
        };
        let delay = full
            .viewport_output
            .get(&ViewportId::ROOT)
            .map(|v| v.repaint_delay);
        let repaint_after = if ui.animating || ran_intents {
            // An intent changed what the next frame shows (a new session, a new screen).
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
    use krypa_engine_api::{ConnectionSecurity, NavigationKind};
    use krypa_navigation::NavigationUrl;

    const TOP: f32 = 59.0;
    const BOTTOM: f32 = 34.0;

    fn app() -> App {
        let insets = Insets {
            top: TOP,
            bottom: BOTTOM,
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

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn nav_intent(s: &str) -> Intent {
        Intent::Navigate(NavigationUrl::parse(s).unwrap())
    }

    fn drain(app: &mut App) -> Vec<EngineCommand> {
        std::iter::from_fn(|| app.next_command()).collect()
    }

    fn host(json: &str) -> HostEvent {
        HostEvent::parse(json.as_bytes()).unwrap()
    }

    /// Opens `https://example.com/` and commits it as a securely loaded document.
    fn open_secure_page(app: &mut App) -> SessionId {
        app.execute_intents(vec![nav_intent("https://example.com/")]);
        let id = app.state().foreground.unwrap();
        drain(app);
        app.push_host_event(host(&format!(
            r#"{{"NavigationStarted":{{"session":{0},"url":"https://example.com/","can_go_back":false,"can_go_forward":false}}}}"#,
            id.get()
        )))
        .unwrap();
        app.push_host_event(host(&format!(
            r#"{{"DocumentCommitted":{{"session":{0},"url":"https://example.com/","can_go_back":false,"can_go_forward":false,"trust":"Valid","only_secure_content":true}}}}"#,
            id.get()
        )))
        .unwrap();
        id
    }

    fn request(u: &str, target: NavigationTarget, gesture: bool) -> NavigationRequest {
        NavigationRequest {
            url: url(u),
            target,
            kind: NavigationKind::LinkActivated,
            user_gesture: gesture,
        }
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
        let (top, bottom) = (TOP, 852.0 - BOTTOM);
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

    /// Why: the UI promises that "open this address" starts the session lifecycle the core
    /// defines; the adapter only ever sees the commands the core issued, in order.
    #[test]
    fn opening_an_address_from_the_launcher_creates_a_foreground_session_and_commands() {
        let mut app = app();
        app.execute_intents(vec![nav_intent("https://example.com/")]);
        assert_eq!(app.state().screen, Screen::Browser);
        let id = app.state().foreground.unwrap();
        assert_eq!(
            drain(&mut app),
            vec![
                EngineCommand::CreateSession {
                    session: id,
                    profile: ProfileKind::Default,
                    initial_url: Some(url("https://example.com/")),
                },
                EngineCommand::Show { session: id },
            ]
        );
        assert!(app.next_command().is_none());
    }

    #[test]
    fn toolbar_intents_become_engine_commands_for_their_session() {
        let mut app = app();
        let id = open_secure_page(&mut app);
        app.execute_intents(vec![
            Intent::Reload(id),
            Intent::GoBack(id),
            Intent::GoForward(id),
            Intent::Stop(id),
        ]);
        assert_eq!(
            drain(&mut app),
            vec![
                EngineCommand::Reload { session: id },
                EngineCommand::GoBack { session: id },
                EngineCommand::GoForward { session: id },
                EngineCommand::Stop { session: id },
            ]
        );
    }

    #[test]
    fn closing_a_session_destroys_its_view_and_late_events_are_rejected() {
        let mut app = app();
        let id = open_secure_page(&mut app);
        app.execute_intents(vec![Intent::CloseSession(id)]);
        assert_eq!(
            drain(&mut app),
            vec![EngineCommand::DestroySession { session: id }]
        );
        // The adapter's acknowledgement is accepted exactly once ...
        let ack = format!(r#"{{"SessionDestroyed":{{"session":{}}}}}"#, id.get());
        assert!(app.push_host_event(host(&ack)).is_ok());
        // ... and anything else a dying web view still sends is stale.
        let late_title = format!(r#"{{"Title":{{"session":{},"title":"x"}}}}"#, id.get());
        assert_eq!(
            app.push_host_event(host(&late_title)),
            Err(CoreError::SessionDestroyed(id))
        );
        assert_eq!(
            app.push_host_event(host(&ack)),
            Err(CoreError::SessionDestroyed(id))
        );
    }

    #[test]
    fn events_for_sessions_the_core_never_issued_are_rejected_and_change_nothing() {
        let mut app = app();
        let id = open_secure_page(&mut app);
        let before = app.state().sessions.clone();
        let stranger = id.get() + 41;
        let ev = format!(
            r#"{{"DocumentCommitted":{{"session":{stranger},"url":"https://evil.example/","can_go_back":false,"can_go_forward":false,"trust":"Valid","only_secure_content":true}}}}"#
        );
        assert_eq!(
            app.push_host_event(host(&ev)),
            Err(CoreError::UnknownSession(SessionId::new(stranger)))
        );
        assert_eq!(app.state().sessions, before);
    }

    /// Why: the lock must only appear for a document the adapter confirmed, and a later
    /// downgrade (mixed content) or a certificate failure must take it away.
    #[test]
    fn security_follows_the_facts_the_adapter_reports() {
        let mut app = app();
        let id = open_secure_page(&mut app);
        let sec = |app: &App| app.state().foreground_info().unwrap().security;
        assert_eq!(sec(&app), Some(ConnectionSecurity::Secure));

        app.push_host_event(host(&format!(
            r#"{{"SecurityFacts":{{"session":{},"trust":"Valid","only_secure_content":false}}}}"#,
            id.get()
        )))
        .unwrap();
        assert_eq!(sec(&app), Some(ConnectionSecurity::Insecure));
    }

    #[test]
    fn a_page_nothing_has_confirmed_is_not_verified() {
        // A navigation start wipes the previous document's report.
        let mut app = app();
        let id = open_secure_page(&mut app);
        app.push_host_event(host(&format!(
            r#"{{"NavigationStarted":{{"session":{},"url":"https://other.example/","can_go_back":true,"can_go_forward":false}}}}"#,
            id.get()
        )))
        .unwrap();
        assert_eq!(app.state().foreground_info().unwrap().security, None);
    }

    #[test]
    fn a_tls_failure_shows_the_certificate_error_and_removes_the_web_view() {
        let mut app = app();
        app.execute_intents(vec![nav_intent("https://expired.example/")]);
        let id = app.state().foreground.unwrap();
        drain(&mut app);
        app.push_host_event(host(&format!(
            r#"{{"LoadFailed":{{"session":{},"domain":"NSURLErrorDomain","code":-1202}}}}"#,
            id.get()
        )))
        .unwrap();
        let info = app.state().foreground_info().unwrap();
        assert_eq!(
            info.failure,
            Some(krypa_core::FailureReason::LoadFailed(
                krypa_engine_api::LoadErrorKind::Tls
            ))
        );
        assert_eq!(info.security, Some(ConnectionSecurity::CertificateError));
        // The error view is Rust's; a web view over it would hide it.
        settle(&mut app);
        assert_eq!(app.placement(), Placement::NONE);
    }

    /// Why: this is the milestone's point: the web view sits below the status bar and the
    /// chrome's top bar, above the bottom bar and the home indicator, nowhere else.
    #[test]
    fn the_web_view_is_placed_between_the_toolbars_inside_the_safe_area() {
        let mut app = app();
        let id = open_secure_page(&mut app);
        settle(&mut app);
        let p = app.placement();
        assert_eq!(p.session, Some(id));
        assert!(
            p.rect.min.y > TOP,
            "web view {:?} starts under the status bar",
            p.rect
        );
        assert!(
            p.rect.max.y < 852.0 - BOTTOM,
            "web view {:?} reaches the home indicator area",
            p.rect
        );
        assert!(p.rect.min.x.abs() < 0.5 && (p.rect.width() - 393.0).abs() < 0.5);
    }

    #[test]
    fn returning_to_the_launcher_takes_the_web_view_off_screen_but_keeps_the_session() {
        let mut app = app();
        let id = open_secure_page(&mut app);
        settle(&mut app);
        assert_eq!(app.placement().session, Some(id));
        app.state.screen = Screen::Launcher;
        settle(&mut app);
        assert_eq!(app.placement(), Placement::NONE);
        assert_eq!(app.manager().live_view_count(), 1);
    }

    /// Why: the manager defines which URLs may load and what happens to the rest; the
    /// adapter must follow it, with the user-gesture flag it measured, never a default.
    #[test]
    fn navigation_policy_decisions_follow_the_core() {
        let mut app = app();
        let id = open_secure_page(&mut app);
        let main = NavigationTarget::MainFrame;
        assert_eq!(
            app.decide_navigation(id, &request("https://example.com/next", main, false)),
            NavDecision::Allow
        );
        for blocked in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,x",
        ] {
            assert_eq!(
                app.decide_navigation(id, &request(blocked, main, true)),
                NavDecision::Cancel,
                "{blocked}"
            );
        }
        // Subframes may use data: and about: (embedded content), never file:.
        assert_eq!(
            app.decide_navigation(
                id,
                &request("about:blank", NavigationTarget::SubFrame, false)
            ),
            NavDecision::Allow
        );
    }

    #[test]
    fn an_external_scheme_with_a_user_gesture_asks_the_user_and_is_never_opened_by_the_web_view() {
        let mut app = app();
        let id = open_secure_page(&mut app);
        let r = request(
            "mailto:someone@example.com",
            NavigationTarget::MainFrame,
            true,
        );
        assert_eq!(app.decide_navigation(id, &r), NavDecision::Cancel);
        assert_eq!(
            app.state().external_prompt.as_ref().map(|p| p.session),
            Some(id)
        );
        // Confirming hands the URL to the adapter as a command; refusing does nothing.
        app.execute_intents(vec![Intent::RespondExternalOpen {
            session: id,
            allow: true,
        }]);
        assert_eq!(
            drain(&mut app),
            vec![EngineCommand::OpenExternal {
                session: id,
                url: url("mailto:someone@example.com")
            }]
        );
    }

    #[test]
    fn an_external_scheme_without_a_user_gesture_is_dropped_silently_by_policy() {
        let mut app = app();
        let id = open_secure_page(&mut app);
        let r = request("tel:+4312345", NavigationTarget::MainFrame, false);
        assert_eq!(app.decide_navigation(id, &r), NavDecision::Cancel);
        assert!(app.state().external_prompt.is_none());
        assert!(drain(&mut app).is_empty());
    }

    /// Why: `target=_blank` and `window.open` are handled on purpose: a user-activated one
    /// opens a new session in the foreground (a "tab"), a script-only one is blocked as a
    /// popup. Swift never creates the window itself.
    #[test]
    fn new_windows_open_a_session_only_with_a_user_gesture() {
        let mut app = app();
        let id = open_secure_page(&mut app);

        let popup = request("https://ads.example/", NavigationTarget::NewWindow, false);
        assert_eq!(app.decide_navigation(id, &popup), NavDecision::Cancel);
        assert_eq!(app.manager().sessions().len(), 1);

        let link = request("https://example.com/b", NavigationTarget::NewWindow, true);
        assert_eq!(app.decide_navigation(id, &link), NavDecision::Cancel);
        assert_eq!(app.manager().sessions().len(), 2);
        let new = app.state().foreground.unwrap();
        assert_ne!(new, id);
        let cmds = drain(&mut app);
        assert!(cmds.contains(&EngineCommand::Hide { session: id }));
        assert!(cmds.contains(&EngineCommand::CreateSession {
            session: new,
            profile: ProfileKind::Default,
            initial_url: Some(url("https://example.com/b")),
        }));
    }

    #[test]
    fn a_navigation_for_an_unknown_session_is_cancelled() {
        let mut app = app();
        let r = request("https://example.com/", NavigationTarget::MainFrame, true);
        assert_eq!(
            app.decide_navigation(SessionId::new(99), &r),
            NavDecision::Cancel
        );
    }

    /// Why: the iOS adapter cannot fetch manifests; the capabilities must say so, or the UI
    /// would offer "Add to Krypa" and wait forever.
    #[test]
    fn capabilities_do_not_promise_what_the_adapter_cannot_do() {
        let app = app();
        assert_eq!(*app.manager().capabilities(), EngineCapabilities::default());
        assert!(!app.state().capabilities.manifest_fetch);
    }

    #[test]
    fn unsupported_intents_are_counted_not_lost() {
        let mut app = app();
        let id = open_secure_page(&mut app);
        app.execute_intents(vec![Intent::AddToKrypa(id)]);
        assert_eq!(app.intents_dropped(), 1);
        assert!(drain(&mut app).is_empty());
    }

    #[cfg(feature = "debug-start-url")]
    #[test]
    fn the_debug_start_url_opens_a_session_and_refuses_non_web_urls() {
        let mut app = app();
        assert!(app.debug_open_url("javascript:alert(1)").is_err());
        assert!(app.debug_open_url("file:///etc/passwd").is_err());
        assert!(app.manager().sessions().is_empty());
        app.debug_open_url("https://example.com/").unwrap();
        assert_eq!(app.manager().sessions().len(), 1);
    }
}
