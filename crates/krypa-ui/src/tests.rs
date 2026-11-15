//! Headless tests: egui runs for real (`Context::run_pass`) with scripted input and no
//! window. They assert intent (what is shown, what the caller is told, where touches go),
//! not pixels. The accessibility tree is the readout, which also proves that the text a
//! screen reader would announce is the right one.

use egui::accesskit::{Action, ActionRequest, Node, NodeId, Role, TreeId};
use egui::{pos2, vec2, Context, Event, Key, Modifiers, RawInput, Rect};
use krypa_apps::InstalledApp;
use krypa_core::{ManagerConfig, SessionInfo, SessionManager, SessionSpec, SessionState};
use krypa_engine_api::{
    ConnectionSecurity, EngineCapabilities, EngineEvent, LoadErrorKind, NavigationPhase,
    PermissionDecision, PermissionKind, PermissionRequestId, ProfileKind, SessionId,
};
use krypa_navigation::{NavigationUrl, SecurityState};
use url::Url;

use crate::state::{security_for, PermissionPrompt};
use crate::{strings, AppState, FrameOutput, Intent, KrypaUi, Screen, ThemeChoice};

const SCREEN: egui::Vec2 = vec2(390.0, 844.0);

struct Harness {
    ctx: Context,
    state: AppState,
    nodes: Vec<(NodeId, Node)>,
    events: Vec<Event>,
    time: f64,
    /// Intents of every pass since the last call to `take_intents`.
    intents: Vec<Intent>,
    last: FrameOutput,
    /// Texture of every mesh painted in the last pass (the font atlas is the default id).
    textures: Vec<egui::TextureId>,
}

fn mesh_textures(shapes: &[egui::epaint::ClippedShape], out: &mut Vec<egui::TextureId>) {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::TextureId>) {
        match shape {
            egui::Shape::Mesh(mesh) => out.push(mesh.texture_id),
            egui::Shape::Vec(inner) => inner.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    shapes.iter().for_each(|c| walk(&c.shape, out));
}

impl Harness {
    /// Runs the first frame only, exactly what a user sees on launch.
    fn first_frame(state: AppState) -> Self {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let mut h = Self {
            ctx,
            state,
            nodes: Vec::new(),
            events: Vec::new(),
            time: 0.0,
            intents: Vec::new(),
            last: FrameOutput::default(),
            textures: Vec::new(),
        };
        h.frame();
        h
    }

    /// Runs a few frames so first-frame sizing passes (dialogs) are done.
    fn new(state: AppState) -> Self {
        let mut h = Self::first_frame(state);
        h.settle();
        h
    }

    fn frame(&mut self) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), SCREEN)),
            time: Some(self.time),
            events: std::mem::take(&mut self.events),
            focused: true,
            ..RawInput::default()
        };
        self.time += 0.016;
        let state = &mut self.state;
        let mut collected = Vec::new();
        let mut last = FrameOutput::default();
        let mut full = self.ctx.run_pass(input, |ctx| {
            last = KrypaUi::frame(ctx, state);
            collected.extend(last.intents.iter().cloned());
        });
        if let Some(update) = full.platform_output.accesskit_update.take() {
            self.nodes = update.nodes;
        }
        self.textures.clear();
        mesh_textures(&full.shapes, &mut self.textures);
        full.drop_without_applying_deltas();
        self.intents.extend(collected);
        self.last = last;
    }

    fn settle(&mut self) {
        for _ in 0..3 {
            self.frame();
        }
    }

    fn take_intents(&mut self) -> Vec<Intent> {
        std::mem::take(&mut self.intents)
    }

    fn node_of(&self, text: &str) -> Option<(NodeId, &Node)> {
        let mut hits: Vec<_> = self
            .nodes
            .iter()
            .filter(|(_, n)| {
                n.role() != Role::TextRun && (n.label() == Some(text) || n.value() == Some(text))
            })
            .collect();
        // A visible caption next to its control shares the control's name; the control wins.
        if hits.len() > 1 {
            hits.retain(|(_, n)| is_interactive(n.role()));
        }
        assert!(
            hits.len() <= 1,
            "label {text:?} is not unique: {} nodes",
            hits.len()
        );
        hits.first().map(|(id, n)| (*id, n))
    }

    fn has_text(&self, text: &str) -> bool {
        self.node_of(text).is_some()
    }

    fn has_text_containing(&self, part: &str) -> bool {
        self.nodes.iter().any(|(_, n)| {
            n.label().is_some_and(|l| l.contains(part))
                || n.value().is_some_and(|v| v.contains(part))
        })
    }

    fn bounds(&self, text: &str) -> Rect {
        let (_, node) = self
            .node_of(text)
            .unwrap_or_else(|| panic!("no node {text:?}"));
        let b = node.bounds().unwrap();
        Rect::from_min_max(
            pos2(b.x0 as f32, b.y0 as f32),
            pos2(b.x1 as f32, b.y1 as f32),
        )
    }

    fn request(&mut self, text: &str, action: Action) {
        let (id, _) = self
            .node_of(text)
            .unwrap_or_else(|| panic!("no node {text:?}"));
        self.events
            .push(Event::AccessKitActionRequest(ActionRequest {
                action,
                target_tree: TreeId::ROOT,
                target_node: id,
                data: None,
            }));
        self.settle();
    }

    fn click(&mut self, text: &str) {
        self.request(text, Action::Click);
    }

    fn type_into_address_field_and_submit(&mut self, text: &str) {
        self.request(strings::ADDRESS_FIELD_LABEL, Action::Focus);
        self.events.push(Event::Text(text.to_owned()));
        self.settle();
        for pressed in [true, false] {
            self.events.push(Event::Key {
                key: Key::Enter,
                physical_key: Some(Key::Enter),
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            });
        }
        self.settle();
    }
}

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

fn session(id: u64, state: SessionState, page: &str, title: Option<&str>) -> SessionInfo {
    SessionInfo {
        id: SessionId::new(id),
        profile: ProfileKind::Default,
        state,
        current_url: Some(url(page)),
        restorable_url: None,
        title: title.map(str::to_owned),
        icon_url: None,
        progress_permille: 1000,
        can_go_back: false,
        can_go_forward: false,
        failure: None,
        security: None,
    }
}

fn browser_state(info: SessionInfo) -> AppState {
    let mut s = AppState::new();
    s.foreground = Some(info.id);
    s.sessions = vec![info];
    s.screen = Screen::Browser;
    s
}

fn launcher_state() -> AppState {
    AppState::new()
}

// ---------------------------------------------------------------- launcher

/// Why: Apple only accepts a browser whose launch screen offers a URL or search field
/// (PLAN.md 1.1). It must be there on the very first frame, with an accessible name.
#[test]
fn launcher_has_address_field_on_first_frame() {
    let mut h = Harness::first_frame(launcher_state());
    let (_, node) = h
        .node_of(strings::ADDRESS_FIELD_LABEL)
        .expect("address field");
    assert_eq!(node.role(), Role::TextInput);
    assert!(h.has_text(strings::APP_NAME));
    h.frame();
    assert!(
        h.last.intents.is_empty(),
        "drawing alone must not produce intents"
    );
}

/// Why: URL validation lives in krypa-navigation. The UI must hand over exactly what
/// `classify_input` returns and never invent a URL of its own.
#[test]
fn submitting_a_host_yields_the_navigation_crate_url() {
    let mut h = Harness::new(launcher_state());
    h.type_into_address_field_and_submit("example.com");
    let expected = NavigationUrl::parse("https://example.com").unwrap();
    assert_eq!(h.take_intents(), vec![Intent::Navigate(expected)]);
    assert!(
        h.state.address_input.is_empty(),
        "field is cleared after submit"
    );
}

#[test]
fn submitting_free_text_is_a_search_not_a_url() {
    let mut h = Harness::new(launcher_state());
    h.type_into_address_field_and_submit("what is rust?");
    assert_eq!(
        h.take_intents(),
        vec![Intent::Search("what is rust?".to_owned())]
    );
}

/// Why: a `javascript:` address must never become any intent, and the user must be told.
#[test]
fn blocked_scheme_yields_no_intent_and_an_error() {
    let mut h = Harness::new(launcher_state());
    h.type_into_address_field_and_submit("javascript:alert(1)");
    assert!(h.take_intents().is_empty());
    assert!(h.has_text(strings::ADDRESS_ERROR_BLOCKED));
    assert_eq!(h.state.screen, Screen::Launcher);
}

#[test]
fn credentials_in_the_address_are_refused() {
    let mut h = Harness::new(launcher_state());
    h.type_into_address_field_and_submit("https://bank.com@evil.test/");
    assert!(h.take_intents().is_empty());
    assert!(h.has_text(strings::ADDRESS_ERROR_CREDENTIALS));
}

#[test]
fn mailto_is_offered_as_external_link_not_navigated() {
    let mut h = Harness::new(launcher_state());
    h.type_into_address_field_and_submit("mailto:a@example.com");
    let intents = h.take_intents();
    assert!(matches!(intents.as_slice(), [Intent::OpenExternal(l)] if l.scheme() == "mailto"));
}

#[test]
fn tapping_an_installed_app_opens_it_by_id() {
    let app = InstalledApp::manual(&url("https://app.example/home"), Some("Mail App")).unwrap();
    let mut state = launcher_state();
    state.installed_apps = vec![app.clone()];
    let mut h = Harness::new(state);
    h.click(&strings::app_card_label("Mail App", "app.example"));
    assert_eq!(h.take_intents(), vec![Intent::OpenApp(app.id().clone())]);
}

// ---------------------------------------------------------------- chrome

fn chrome_text_for(page: &str, reported: Option<ConnectionSecurity>) -> Harness {
    let mut info = session(1, SessionState::Foreground, page, Some("Title"));
    info.security = reported;
    Harness::new(browser_state(info))
}

/// Why: the origin is the one thing the user can trust. It must be exactly the
/// `DisplayOrigin` of krypa-navigation, and a homograph must stay punycode.
#[test]
fn chrome_shows_the_display_origin_in_punycode_when_flagged() {
    // Cyrillic small a in "paypal": looks like paypal.com, is not.
    let spoof = "https://p\u{0430}ypal.com/login";
    let origin = crate::display_origin_of(&url(spoof)).unwrap();
    assert!(
        origin.shown_as_punycode,
        "precondition: navigation flags this host"
    );
    let h = chrome_text_for(spoof, None);
    assert!(h.has_text(&origin.to_string()));
    assert!(origin.to_string().starts_with("xn--"));
    assert!(h.has_text(strings::PUNYCODE_NOTE));
    assert!(!h.has_text_containing('\u{0430}'.to_string().as_str()));
}

#[test]
fn chrome_shows_safe_unicode_hosts_and_non_default_ports() {
    let h = chrome_text_for("https://b\u{00FC}cher.example:8443/a?q=1#x", None);
    assert!(h.has_text("b\u{00FC}cher.example:8443"));
    assert!(!h.has_text(strings::PUNYCODE_NOTE));
    // Path, query and fragment are never part of the origin line.
    assert!(!h.has_text_containing("q=1"));
}

/// Why: the security indicator is a trust claim. Plain HTTP or a certificate failure must
/// never show the secure wording, even if the engine reports "secure".
#[test]
fn insecure_and_certificate_error_never_show_the_secure_indicator() {
    let http = chrome_text_for("http://plain.example/", Some(ConnectionSecurity::Secure));
    assert!(http.has_text(strings::SECURITY_INSECURE));
    assert!(!http.has_text(strings::SECURITY_SECURE));

    let mut failed = session(1, SessionState::Failed, "https://bad.example/", None);
    failed.failure = Some(krypa_core::FailureReason::LoadFailed(LoadErrorKind::Tls));
    failed.security = Some(ConnectionSecurity::Secure);
    let cert = Harness::new(browser_state(failed));
    assert!(cert.has_text(strings::SECURITY_CERTIFICATE));
    assert!(!cert.has_text(strings::SECURITY_SECURE));
    assert!(
        cert.has_text("bad.example"),
        "the real origin stays visible on an error"
    );

    let good = chrome_text_for("https://good.example/", Some(ConnectionSecurity::Secure));
    assert!(good.has_text(strings::SECURITY_SECURE));
    assert!(!good.has_text(strings::SECURITY_INSECURE));
}

/// Why: an `https` URL alone is no proof. Until the engine confirms the page (loading, or
/// nothing reported) the chrome must say "not verified" and never the secure wording.
#[test]
fn an_https_page_the_engine_has_not_confirmed_never_shows_as_secure() {
    let h = chrome_text_for("https://unconfirmed.example/", None);
    assert!(h.has_text(strings::SECURITY_UNVERIFIED));
    assert!(!h.has_text(strings::SECURITY_SECURE));
    assert!(h.has_text("unconfirmed.example"));
}

#[test]
fn the_engine_reported_insecure_and_certificate_states_show_their_own_wording() {
    let h = chrome_text_for("https://mixed.example/", Some(ConnectionSecurity::Insecure));
    assert!(h.has_text(strings::SECURITY_INSECURE));
    let h = chrome_text_for(
        "https://bad.example/",
        Some(ConnectionSecurity::CertificateError),
    );
    assert!(h.has_text(strings::SECURITY_CERTIFICATE));
    assert!(!h.has_text(strings::SECURITY_SECURE));
}

#[test]
fn security_for_is_conservative() {
    let mut i = session(1, SessionState::Foreground, "http://a.example/", None);
    i.security = Some(ConnectionSecurity::Secure);
    assert_eq!(security_for(&i), Some(SecurityState::Insecure));
    i.current_url = Some(url("https://a.example/"));
    i.security = Some(ConnectionSecurity::CertificateError);
    assert_eq!(security_for(&i), Some(SecurityState::CertificateError));
    i.security = None;
    assert_eq!(security_for(&i), None, "https alone earns no trust");
    i.security = Some(ConnectionSecurity::Secure);
    i.current_url = None;
    assert_eq!(
        security_for(&i),
        Some(SecurityState::Insecure),
        "no URL, no trust"
    );
}

/// Why: touch routing. Browser-owned UI must receive touches, the page must receive the
/// rest. A rectangle wrongly left over the page would make it untouchable.
#[test]
fn covered_rects_are_the_toolbars_and_never_the_page() {
    let h = chrome_text_for("https://example.com/", None);
    let web = h.last.web_view_rect.expect("web view area");
    assert!(web.width() > 300.0 && web.height() > 300.0);
    for b in [strings::BACK, strings::SWITCHER, strings::RELOAD] {
        let r = h.bounds(b);
        assert!(h.last.is_covered(r.center()), "{b} must be covered");
        assert!(!web.intersects(r), "{b} must be outside the web view area");
    }
    assert!(!h.last.is_covered(web.center()));
    for r in &h.last.covered_rects {
        assert!(
            !r.intersects(web.shrink(1.0)),
            "{r:?} overlaps the page area {web:?}"
        );
    }
    assert!(!h.last.animating);
}

#[test]
fn an_open_dialog_covers_everything_including_the_page() {
    let mut state = browser_state(session(
        1,
        SessionState::Foreground,
        "https://example.com/",
        None,
    ));
    state.permission_prompts.push(PermissionPrompt {
        session: SessionId::new(1),
        request: PermissionRequestId(9),
        kind: PermissionKind::Camera,
        origin: url("https://example.com/"),
    });
    let h = Harness::new(state);
    let allow = h.bounds(strings::PERMISSION_ALLOW);
    assert!(h.last.covered_rects.iter().any(|r| r.contains_rect(allow)));
    assert!(h.last.is_covered(h.last.web_view_rect.unwrap().center()));
}

/// Why: error and loading views are drawn over the page area, so touches there are ours.
#[test]
fn error_view_over_the_page_area_is_covered_and_retry_reloads() {
    let mut failed = session(1, SessionState::Failed, "https://down.example/", None);
    failed.failure = Some(krypa_core::FailureReason::LoadFailed(
        LoadErrorKind::Network,
    ));
    let mut h = Harness::new(browser_state(failed));
    let web = h.last.web_view_rect.unwrap();
    assert!(h.last.is_covered(web.center()));
    assert!(h.has_text(strings::RETRY));
    h.click(strings::RETRY);
    assert_eq!(h.take_intents(), vec![Intent::Reload(SessionId::new(1))]);
}

#[test]
fn crash_shows_a_distinct_error() {
    let mut crashed = session(1, SessionState::Failed, "https://down.example/", None);
    crashed.failure = Some(krypa_core::FailureReason::ContentProcessTerminated);
    let h = Harness::new(browser_state(crashed));
    assert!(h.has_text(strings::ERROR_CRASH_TITLE));
    assert!(!h.has_text(strings::ERROR_LOAD_TITLE));
}

#[test]
fn toolbar_buttons_emit_intents_for_their_session_and_respect_history_flags() {
    let mut info = session(4, SessionState::Foreground, "https://example.com/", None);
    info.can_go_back = true;
    let mut h = Harness::new(browser_state(info));
    let id = SessionId::new(4);
    h.click(strings::BACK);
    h.click(strings::RELOAD);
    // Forward is disabled: a click on it must do nothing.
    h.click(strings::FORWARD);
    assert_eq!(
        h.take_intents(),
        vec![Intent::GoBack(id), Intent::Reload(id)]
    );
}

#[test]
fn a_loading_page_offers_stop_not_reload_and_shows_progress() {
    let mut info = session(2, SessionState::Loading, "https://example.com/", None);
    info.progress_permille = 400;
    let mut h = Harness::new(browser_state(info));
    assert!(h.has_text(strings::STOP));
    assert!(!h.has_text(strings::RELOAD));
    let (_, bar) = h.node_of(strings::LOADING).unwrap();
    assert_eq!(bar.role(), Role::ProgressIndicator);
    h.click(strings::STOP);
    assert_eq!(h.take_intents(), vec![Intent::Stop(SessionId::new(2))]);
}

#[test]
fn add_to_krypa_is_offered_until_the_page_is_inside_an_installed_app() {
    let info = session(
        3,
        SessionState::Foreground,
        "https://shop.example/cart",
        None,
    );
    let mut h = Harness::new(browser_state(info.clone()));
    h.click(strings::ADD_TO_KRYPA);
    assert_eq!(
        h.take_intents(),
        vec![Intent::AddToKrypa(SessionId::new(3))]
    );

    let mut state = browser_state(info);
    state.installed_apps =
        vec![InstalledApp::manual(&url("https://shop.example/start"), Some("Shop")).unwrap()];
    let mut h = Harness::new(state);
    assert!(!h.has_text(strings::ADD_TO_KRYPA));
    h.click(strings::ALREADY_ADDED);
    assert!(h.take_intents().is_empty());
}

// ---------------------------------------------------------------- switcher

/// Why: the user must be able to tell a running page from a paused one and from one whose
/// view is gone (and will reload). Driven through the real `SessionManager`.
#[test]
fn switcher_distinguishes_live_suspended_and_terminated_cards() {
    let mut m = SessionManager::new(
        ManagerConfig::new(3).unwrap(),
        EngineCapabilities::default(),
    );
    let open = |m: &mut SessionManager, page: &str| {
        let id = m
            .create_session(SessionSpec {
                profile: ProfileKind::Default,
                url: Some(url(page)),
                app_scope: None,
            })
            .unwrap();
        m.foreground(id).unwrap();
        m.handle_event(EngineEvent::NavigationChanged {
            session: id,
            url: url(page),
            phase: NavigationPhase::Committed,
            can_go_back: false,
            can_go_forward: false,
        })
        .unwrap();
        id
    };
    open(&mut m, "https://evicted.example/");
    let paused = open(&mut m, "https://paused.example/");
    open(&mut m, "https://background.example/");
    m.suspend(paused).unwrap();
    // Budget 3: opening a fourth page evicts the least recently used view.
    open(&mut m, "https://showing.example/");
    let infos = m.sessions();
    let mut states: Vec<_> = infos.iter().map(|i| i.state).collect();
    states.sort_by_key(|s| format!("{s:?}"));
    assert_eq!(
        states,
        [
            SessionState::Background,
            SessionState::Foreground,
            SessionState::Suspended,
            SessionState::Terminated
        ],
        "precondition: the manager produced one of each"
    );

    let mut state = AppState::new();
    state.sync_sessions(&m);
    state.screen = Screen::Switcher;
    let h = Harness::new(state);
    let mut labels = Vec::new();
    for i in &infos {
        let host = i
            .current_url
            .as_ref()
            .unwrap()
            .host_str()
            .unwrap()
            .to_owned();
        let label = strings::switcher_card_label("Untitled page", &host, i.state, false);
        assert!(h.has_text(&label), "missing card {label:?}");
        labels.push(label);
    }
    let unique: std::collections::HashSet<_> = labels.iter().collect();
    assert_eq!(unique.len(), labels.len());
    assert!(labels
        .iter()
        .any(|l| l.ends_with(strings::state_badge(SessionState::Terminated))));
    assert!(labels
        .iter()
        .any(|l| l.ends_with(strings::state_badge(SessionState::Foreground))));
}

#[test]
fn terminated_card_is_labelled_differently_from_a_live_one() {
    let mut state = AppState::new();
    state.sessions = vec![
        session(
            1,
            SessionState::Background,
            "https://same.example/",
            Some("Same"),
        ),
        session(
            2,
            SessionState::Terminated,
            "https://same.example/",
            Some("Same"),
        ),
        session(
            3,
            SessionState::Suspended,
            "https://same.example/",
            Some("Same"),
        ),
    ];
    state.screen = Screen::Switcher;
    let h = Harness::new(state);
    let live =
        strings::switcher_card_label("Same", "same.example", SessionState::Background, false);
    let dead =
        strings::switcher_card_label("Same", "same.example", SessionState::Terminated, false);
    let paused =
        strings::switcher_card_label("Same", "same.example", SessionState::Suspended, false);
    assert!(h.has_text(&live) && h.has_text(&dead) && h.has_text(&paused));
    assert!(live.contains("Live") && dead.contains("Not loaded") && paused.contains("Paused"));
    assert!(!dead.contains("Live"));
}

#[test]
fn switcher_card_taps_open_or_close_the_right_session() {
    let mut state = AppState::new();
    state.sessions = vec![
        session(
            1,
            SessionState::Foreground,
            "https://one.example/",
            Some("One"),
        ),
        session(
            2,
            SessionState::Terminated,
            "https://two.example/",
            Some("Two"),
        ),
    ];
    state.foreground = Some(SessionId::new(1));
    state.screen = Screen::Switcher;
    let mut h = Harness::new(state);
    h.click(&strings::close_page_label("One"));
    assert_eq!(
        h.take_intents(),
        vec![Intent::CloseSession(SessionId::new(1))]
    );
    h.click(&strings::switcher_card_label(
        "Two",
        "two.example",
        SessionState::Terminated,
        false,
    ));
    assert_eq!(h.take_intents(), vec![Intent::SwitchTo(SessionId::new(2))]);
    assert_eq!(h.state.screen, Screen::Browser);
}

#[test]
fn private_sessions_are_marked() {
    let mut s = session(
        1,
        SessionState::Foreground,
        "https://one.example/",
        Some("One"),
    );
    s.profile = ProfileKind::Private;
    let mut state = AppState::new();
    state.sessions = vec![s];
    state.screen = Screen::Switcher;
    let h = Harness::new(state);
    assert!(h.has_text(&strings::switcher_card_label(
        "One",
        "one.example",
        SessionState::Foreground,
        true
    )));
}

/// Why: page titles are hostile. A long title with bidi controls and markup must come out
/// short, flat, and literal.
#[test]
fn hostile_page_title_is_truncated_and_literal_in_the_switcher() {
    let title = format!("\u{202E}**bold** <script>{}", "A".repeat(1000));
    let mut state = AppState::new();
    state.sessions = vec![session(
        1,
        SessionState::Foreground,
        "https://one.example/",
        Some(&title),
    )];
    state.screen = Screen::Switcher;
    let h = Harness::new(state);
    let (_, card) = h
        .nodes
        .iter()
        .find(|(_, n)| {
            n.label()
                .is_some_and(|l| l.contains("one.example") && l.contains("**bold**"))
        })
        .expect("card present with literal markup");
    let label = card.label().unwrap();
    assert!(!label.contains('\u{202E}'));
    assert!(
        label.chars().count() < 120,
        "label length {}",
        label.chars().count()
    );
}

// ---------------------------------------------------------------- dialogs, settings

#[test]
fn permission_dialog_names_the_origin_and_answers_the_right_prompt() {
    let mut state = browser_state(session(
        5,
        SessionState::Foreground,
        "https://example.com/",
        None,
    ));
    state.permission_prompts.push(PermissionPrompt {
        session: SessionId::new(5),
        request: PermissionRequestId(77),
        kind: PermissionKind::Microphone,
        origin: url("https://talk.example/room?id=secret"),
    });
    let mut h = Harness::new(state);
    assert!(h.has_text(&strings::permission_title(
        "talk.example",
        PermissionKind::Microphone
    )));
    assert!(
        !h.has_text_containing("secret"),
        "query strings never reach the dialog"
    );
    h.click(strings::PERMISSION_DENY);
    assert_eq!(
        h.take_intents(),
        vec![Intent::RespondPermission {
            session: SessionId::new(5),
            request: PermissionRequestId(77),
            decision: PermissionDecision::Deny,
        }]
    );
    assert!(h.state.permission_prompts.is_empty());
    assert!(
        !h.has_text(strings::PERMISSION_ALLOW),
        "dialog closes after the answer"
    );
}

#[test]
fn permission_allow_grants() {
    let mut state = launcher_state();
    state.permission_prompts.push(PermissionPrompt {
        session: SessionId::new(1),
        request: PermissionRequestId(1),
        kind: PermissionKind::Camera,
        origin: url("https://a.example/"),
    });
    let mut h = Harness::new(state);
    h.click(strings::PERMISSION_ALLOW);
    assert!(matches!(
        h.take_intents().as_slice(),
        [Intent::RespondPermission {
            decision: PermissionDecision::Grant,
            ..
        }]
    ));
}

#[test]
fn settings_changes_are_reported_and_applied() {
    let mut h = Harness::new(launcher_state());
    h.click(strings::SETTINGS);
    assert!(h.state.settings_open);
    h.click(strings::SETTINGS_REDUCED_MOTION);
    h.click(strings::THEME_DARK);
    assert!(h.state.reduced_motion);
    assert_eq!(h.state.theme, ThemeChoice::Dark);
    assert_eq!(
        h.take_intents()
            .iter()
            .filter(|i| **i == Intent::SettingsChanged)
            .count(),
        2
    );
    h.click(strings::CLOSE);
    assert!(!h.state.settings_open);
}

// ---------------------------------------------------------------- reduced motion

/// Why: users who turn off motion must not get a spinner or a repaint-forever loop.
#[test]
fn reduced_motion_removes_the_spinner_and_continuous_animation() {
    let restoring = session(1, SessionState::Terminated, "https://example.com/", None);
    let mut state = browser_state(restoring);
    state.reduced_motion = false;
    let animated = Harness::new(state.clone());
    assert!(animated.last.animating);
    assert!(animated.has_text(strings::RESTORING));

    state.reduced_motion = true;
    let still = Harness::new(state);
    assert!(!still.last.animating);
    assert!(
        still.has_text(strings::RESTORING),
        "the state is still announced in words"
    );
}

// ---------------------------------------------------------------- accessibility

fn all_screens() -> Vec<(&'static str, AppState)> {
    let app = InstalledApp::manual(&url("https://app.example/"), Some("App")).unwrap();
    let mut launcher = launcher_state();
    launcher.installed_apps = vec![app];
    launcher.recents = vec![crate::RecentEntry {
        title: "Recent".into(),
        url: NavigationUrl::parse("https://recent.example/").unwrap(),
    }];
    launcher.sessions = vec![session(
        1,
        SessionState::Foreground,
        "https://a.example/",
        None,
    )];

    let mut chrome = browser_state(session(
        1,
        SessionState::Foreground,
        "https://a.example/",
        None,
    ));
    chrome.sessions[0].can_go_back = true;
    chrome.sessions[0].can_go_forward = true;

    let mut loading = browser_state(session(
        1,
        SessionState::Loading,
        "https://a.example/",
        None,
    ));
    loading.sessions[0].progress_permille = 300;

    let mut failed_info = session(1, SessionState::Failed, "https://a.example/", None);
    failed_info.failure = Some(krypa_core::FailureReason::LoadFailed(
        LoadErrorKind::Network,
    ));
    let failed = browser_state(failed_info);

    let mut switcher = AppState::new();
    switcher.sessions = vec![
        session(1, SessionState::Foreground, "https://a.example/", Some("A")),
        session(2, SessionState::Terminated, "https://b.example/", Some("B")),
    ];
    switcher.foreground = Some(SessionId::new(1));
    switcher.screen = Screen::Switcher;

    let mut settings = launcher_state();
    settings.settings_open = true;

    let mut permission = browser_state(session(
        1,
        SessionState::Foreground,
        "https://a.example/",
        None,
    ));
    permission.permission_prompts.push(PermissionPrompt {
        session: SessionId::new(1),
        request: PermissionRequestId(1),
        kind: PermissionKind::CameraAndMicrophone,
        origin: url("https://a.example/"),
    });
    let mut external = browser_state(session(
        1,
        SessionState::Foreground,
        "https://a.example/",
        None,
    ));
    external.external_prompt = Some(krypa_core::PendingExternalOpen {
        session: SessionId::new(1),
        url: url("tel:+4312345"),
        origin: Some(url("https://a.example/")),
    });
    let mut install = browser_state(session(
        1,
        SessionState::Foreground,
        "https://a.example/",
        None,
    ));
    install.install_prompt = Some(crate::InstallPrompt {
        session: SessionId::new(1),
        app: InstalledApp::manual(&url("https://a.example/"), Some("A")).unwrap(),
        source: krypa_core::InstallSource::Manifest,
    });
    vec![
        ("external-open", external),
        ("install", install),
        ("launcher", launcher),
        ("chrome", chrome),
        ("loading", loading),
        ("failed", failed),
        ("switcher", switcher),
        ("settings", settings),
        ("permission", permission),
    ]
}

fn is_interactive(role: Role) -> bool {
    matches!(
        role,
        Role::Button | Role::TextInput | Role::Switch | Role::CheckBox | Role::RadioButton
    )
}

/// Why: WCAG 2.5.5 / Apple HIG ask for 44 pt targets; checked on every screen in both
/// themes, because a screen added later is exactly where the rule gets forgotten.
#[test]
fn every_interactive_widget_is_at_least_44_points_on_every_screen_and_theme() {
    for theme in [ThemeChoice::Light, ThemeChoice::Dark] {
        for (name, mut state) in all_screens() {
            state.theme = theme;
            let h = Harness::new(state);
            let mut seen = 0;
            for (_, n) in &h.nodes {
                if !is_interactive(n.role()) {
                    continue;
                }
                seen += 1;
                let b = n.bounds().unwrap();
                let (w, hgt) = (b.x1 - b.x0, b.y1 - b.y0);
                assert!(
                    w >= 43.99 && hgt >= 43.99,
                    "{name}/{theme:?}: {:?} is {w:.1}x{hgt:.1}",
                    n.label()
                );
            }
            assert!(
                seen > 0,
                "{name}: found no interactive widgets, the check is blind"
            );
        }
    }
}

#[test]
fn every_interactive_widget_has_an_accessible_name() {
    for (name, state) in all_screens() {
        let h = Harness::new(state);
        for (_, n) in &h.nodes {
            if is_interactive(n.role()) {
                assert!(
                    n.label().is_some_and(|l| !l.trim().is_empty()),
                    "{name}: unnamed {:?} at {:?}",
                    n.role(),
                    n.bounds()
                );
            }
        }
    }
}

#[test]
fn hit_map_matches_what_is_drawn_in_every_screen() {
    // Sanity for the shell: every screen without a web view covers the full screen.
    for (name, state) in all_screens() {
        let h = Harness::new(state);
        if h.last.web_view_rect.is_none() {
            let covered: f32 = h.last.covered_rects.iter().map(|r| r.area()).sum();
            assert!(
                covered >= SCREEN.x * SCREEN.y * 0.99,
                "{name}: covered {covered}"
            );
        }
    }
}

// ---------------------------------------------------------------- dialogs: external open, install

fn external_state(target: &str) -> AppState {
    let mut s = browser_state(session(
        5,
        SessionState::Foreground,
        "https://talk.example/room?id=secret",
        None,
    ));
    s.external_prompt = Some(krypa_core::PendingExternalOpen {
        session: SessionId::new(5),
        url: url(target),
        origin: Some(url("https://talk.example/room?id=secret")),
    });
    s
}

fn press(h: &mut Harness, key: Key) {
    for pressed in [true, false] {
        h.events.push(Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    }
    h.settle();
}

/// Why: the user must see who asks and what for, in text they can trust, before any other
/// app is opened; the page controls the link, so it is sanitized and cut.
#[test]
fn the_external_dialog_names_the_asking_origin_the_scheme_and_a_shortened_target() {
    let long = format!("mailto:a@example.com?subject={}", "x".repeat(400));
    let h = Harness::new(external_state(&long));
    assert!(h.has_text(&strings::external_open_title("talk.example")));
    assert!(
        !h.has_text_containing("secret"),
        "the asking page's query never reaches the dialog"
    );
    assert!(h.has_text_containing("Type: mailto."));
    assert!(h.has_text_containing("mailto:a@example.com?subject=xxx"));
    assert!(
        !h.has_text_containing(&"x".repeat(100)),
        "the target is cut, not shown whole"
    );
    assert!(h.has_text(strings::EXTERNAL_OPEN_CANCEL));
    assert!(h.has_text(strings::EXTERNAL_OPEN_CONFIRM));
}

/// Why: Cancel is the safe default. Enter on a freshly opened dialog, Escape, and a tap
/// on Cancel must all refuse; only an explicit tap on Open allows.
#[test]
fn the_external_dialog_defaults_to_cancel() {
    let s = SessionId::new(5);
    let cancelled = vec![Intent::RespondExternalOpen {
        session: s,
        allow: false,
    }];

    let mut h = Harness::new(external_state("tel:+4312345"));
    press(&mut h, Key::Enter);
    assert_eq!(h.take_intents(), cancelled, "Enter on the open dialog");
    assert!(h.state.external_prompt.is_none());

    let mut h = Harness::new(external_state("tel:+4312345"));
    press(&mut h, Key::Escape);
    assert_eq!(h.take_intents(), cancelled, "Escape");

    let mut h = Harness::new(external_state("tel:+4312345"));
    h.click(strings::EXTERNAL_OPEN_CANCEL);
    assert_eq!(h.take_intents(), cancelled, "Cancel");

    let mut h = Harness::new(external_state("tel:+4312345"));
    h.click(strings::EXTERNAL_OPEN_CONFIRM);
    assert_eq!(
        h.take_intents(),
        vec![Intent::RespondExternalOpen {
            session: s,
            allow: true
        }]
    );
}

#[test]
fn the_external_dialog_does_not_answer_by_itself() {
    let mut h = Harness::new(external_state("tel:+4312345"));
    h.settle();
    assert!(h.take_intents().is_empty());
    assert!(h.state.external_prompt.is_some(), "still waiting");
}

fn install_state(source: krypa_core::InstallSource) -> AppState {
    let mut s = browser_state(session(
        3,
        SessionState::Foreground,
        "https://shop.example/cart",
        None,
    ));
    s.install_prompt = Some(crate::InstallPrompt {
        session: SessionId::new(3),
        app: InstalledApp::manual(&url("https://shop.example/cart"), Some("Shop")).unwrap(),
        source,
    });
    s
}

/// Why: installing persists an app on disk; the user confirms the real name and origin
/// first, and Cancel installs nothing.
#[test]
fn the_install_dialog_shows_name_and_origin_and_confirms_only_on_add() {
    use krypa_core::{InstallSource, ManualReason};
    let mut h = Harness::new(install_state(InstallSource::Manifest));
    assert!(h.has_text(&strings::install_title("Shop")));
    assert!(h.has_text(&strings::install_origin("shop.example")));
    assert!(!h.has_text(strings::INSTALL_NOTE_NO_MANIFEST));
    h.click(strings::INSTALL_CANCEL);
    assert!(h.take_intents().is_empty(), "Cancel installs nothing");
    assert!(h.state.install_prompt.is_none());

    let mut h = Harness::new(install_state(InstallSource::Manifest));
    press(&mut h, Key::Enter);
    assert!(h.take_intents().is_empty(), "Enter is Cancel");

    let mut h = Harness::new(install_state(InstallSource::Manual(
        ManualReason::NoManifest,
    )));
    assert!(h.has_text(strings::INSTALL_NOTE_NO_MANIFEST));
    h.click(strings::INSTALL_CONFIRM);
    let intents = h.take_intents();
    let expected = InstalledApp::manual(&url("https://shop.example/cart"), Some("Shop")).unwrap();
    assert_eq!(intents, vec![Intent::ConfirmInstall(Box::new(expected))]);

    let h = Harness::new(install_state(InstallSource::Manual(ManualReason::Invalid)));
    assert!(h.has_text(strings::INSTALL_NOTE_BAD_MANIFEST));
}

/// Why: when "Add to Krypa" is refused the user must be told why (a silent no-op looks
/// like a broken button), and the message must offer nothing to install.
#[test]
fn a_refused_install_explains_why_and_offers_nothing_to_install() {
    use krypa_core::{InstallRefusal, Notice};
    for (reason, text) in [
        (InstallRefusal::NotHttps, strings::INSTALL_REFUSED_NOT_HTTPS),
        (
            InstallRefusal::ConnectionNotSecure,
            strings::INSTALL_REFUSED_NOT_SECURE,
        ),
    ] {
        let mut state = browser_state(session(
            3,
            SessionState::Foreground,
            "http://plain.example/",
            None,
        ));
        state.apply_notice(&Notice::InstallRefused {
            session: SessionId::new(3),
            reason,
        });
        assert!(state.install_prompt.is_none());
        let mut h = Harness::new(state);
        assert!(h.has_text(strings::INSTALL_REFUSED_TITLE));
        assert!(h.has_text(text));
        assert!(!h.has_text(strings::INSTALL_CONFIRM), "no Add button");
        h.click(strings::CLOSE);
        assert!(h.state.install_refusal.is_none());
        assert!(!h.has_text(text));
        assert!(h.take_intents().is_empty());
    }
}

// ---------------------------------------------------------------- app icons

fn png_bytes(side: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(side, side, image::Rgba([200, 30, 30, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn icon_app(purpose: &str) -> (InstalledApp, Url) {
    let doc = url("https://app.example/");
    let manifest_url = url("https://app.example/manifest.json");
    let m = krypa_apps::Manifest::parse(&format!(
        r#"{{"name":"Icon App","icons":[{{"src":"/i.png","sizes":"192x192","purpose":"{purpose}"}}]}}"#
    ))
    .unwrap();
    (
        InstalledApp::from_manifest(&doc, &manifest_url, &m).unwrap(),
        url("https://app.example/i.png"),
    )
}

fn user_textures(h: &Harness) -> Vec<egui::TextureId> {
    h.textures
        .iter()
        .copied()
        .filter(|t| *t != egui::TextureId::default())
        .collect()
}

/// Why: the card shows the real icon once it is decoded, asks the host for it exactly
/// once, and shows the placeholder (with the same accessible name) before that.
#[test]
fn an_app_card_asks_for_its_icon_once_then_shows_it() {
    let (app, icon_url) = icon_app("any");
    let mut state = launcher_state();
    state.installed_apps = vec![app];
    let mut h = Harness::first_frame(state);
    h.settle();
    assert_eq!(h.take_intents(), vec![Intent::FetchIcon(icon_url.clone())]);
    assert!(user_textures(&h).is_empty(), "placeholder while loading");
    let label = strings::app_card_label("Icon App", "app.example");
    assert!(h.has_text(&label));

    h.state
        .icons
        .finish(&icon_url, crate::app_icons::decode_icon(&png_bytes(64)));
    h.settle();
    assert!(h.take_intents().is_empty(), "no second request");
    let shown = user_textures(&h);
    assert_eq!(shown.len(), 1, "the icon is drawn");
    assert_eq!(h.state.icons.texture(&icon_url), Some(shown[0]));
    assert!(
        h.has_text(&label),
        "the icon is decoration: the accessible name is unchanged"
    );
}

#[test]
fn a_maskable_icon_is_drawn_too() {
    let (app, icon_url) = icon_app("maskable");
    let mut state = launcher_state();
    state.installed_apps = vec![app];
    let mut h = Harness::new(state);
    h.state
        .icons
        .finish(&icon_url, crate::app_icons::decode_icon(&png_bytes(64)));
    h.settle();
    assert_eq!(user_textures(&h).len(), 1);
}

/// Why: a hostile or broken icon must leave the card usable: placeholder, no panic, and
/// no endless re-fetching.
#[test]
fn a_bad_or_oversize_icon_leaves_the_placeholder() {
    for bytes in [
        b"<svg/>".to_vec(),
        png_bytes(crate::app_icons::MAX_ICON_SIDE + 1),
    ] {
        let (app, icon_url) = icon_app("any");
        let mut state = launcher_state();
        state.installed_apps = vec![app];
        let mut h = Harness::new(state);
        assert_eq!(h.take_intents(), vec![Intent::FetchIcon(icon_url.clone())]);
        h.state
            .icons
            .finish(&icon_url, crate::app_icons::decode_icon(&bytes));
        h.settle();
        assert!(user_textures(&h).is_empty());
        assert!(h.take_intents().is_empty(), "failure is not retried");
        assert!(h.has_text(&strings::app_card_label("Icon App", "app.example")));
    }
}

/// Why: an app whose manifest lists no usable icon (the manual install) never asks for
/// one.
#[test]
fn an_app_without_icons_requests_nothing() {
    let mut state = launcher_state();
    state.installed_apps =
        vec![InstalledApp::manual(&url("https://app.example/"), Some("Plain")).unwrap()];
    let mut h = Harness::new(state);
    assert!(h.take_intents().is_empty());
}
