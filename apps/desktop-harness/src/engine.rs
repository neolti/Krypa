//! A fake web engine: answers `EngineCommand`s with plausible `EngineEvent`s on a timer.
//! Development only; nothing here is a model of WebKit beyond the shape of the events.
//!
//! Deterministic by design (no randomness) so a run can be repeated:
//! * every 6th load fails with a network error;
//! * a host containing `fail` fails with a network error, `badcert` with a TLS error;
//! * connection security is reported before the commit: `https` is secure, `http`
//!   insecure, `badcert` a certificate error (then the TLS failure);
//! * a path containing `camera` asks for camera permission after committing;
//! * a path containing `hostile` gets a very long title with bidi controls and markup;
//! * the manifest of a page: a path containing `manifest` has one (with `https` icons), a
//!   path containing `badmanifest` has one that is not JSON, `bigmanifest` one the engine
//!   refuses as too large, and any other page has none; `hostile` in the path also gives
//!   the manifest a spoofing name;
//! * icons (the host fetch, `fetch_icon`): `/icons/bad.png` is not an image, `/icons/huge.png`
//!   fails as too large, a host containing `fail` fails, anything else is a solid disc.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use krypa_engine_api::{
    ConnectionSecurity, EngineCommand, EngineEvent, IconFetchRequest, IconFetchResult,
    LoadErrorKind, ManifestFailure, ManifestResult, NavigationPhase, PermissionKind,
    PermissionRequestId, SessionId,
};
use url::Url;

/// Every Nth load fails, to exercise the error view.
const FAIL_EVERY: u32 = 6;

#[derive(Debug, Default)]
struct View {
    history: Vec<Url>,
    position: usize,
    /// Events not yet due, in time order.
    pending: Vec<(Instant, EngineEvent)>,
}

impl View {
    fn flags(&self) -> (bool, bool) {
        (
            self.position > 0,
            !self.history.is_empty() && self.position + 1 < self.history.len(),
        )
    }
}

/// The fake engine.
#[derive(Debug, Default)]
pub struct FakeEngine {
    views: HashMap<SessionId, View>,
    /// Events that are due immediately (acknowledgements).
    immediate: Vec<EngineEvent>,
    loads: u32,
    next_request: u64,
    /// What the user allowed to be handed to the operating system. The harness has no OS
    /// to hand it to; this is the log (URLs are sensitive: tests read it, nothing prints it).
    pub os_handoffs: Vec<Url>,
}

impl FakeEngine {
    /// A new engine with no views.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of views that exist.
    #[must_use]
    pub fn view_count(&self) -> usize {
        self.views.len()
    }

    /// Whether anything is still scheduled (the caller keeps repainting while true).
    #[must_use]
    pub fn busy(&self) -> bool {
        !self.immediate.is_empty() || self.views.values().any(|v| !v.pending.is_empty())
    }

    /// Executes one command from the core.
    pub fn execute(&mut self, command: EngineCommand, now: Instant) {
        match command {
            EngineCommand::CreateSession {
                session,
                initial_url,
                ..
            } => {
                self.views.insert(session, View::default());
                if let Some(url) = initial_url {
                    self.load(session, url, true, now);
                }
            }
            EngineCommand::DestroySession { session } => {
                self.views.remove(&session);
                self.immediate
                    .push(EngineEvent::SessionDestroyed { session });
            }
            EngineCommand::Load { session, url } => self.load(session, url, true, now),
            EngineCommand::Reload { session } => {
                let current = self
                    .views
                    .get(&session)
                    .and_then(|v| v.history.get(v.position).cloned());
                if let Some(url) = current {
                    self.load(session, url, false, now);
                }
            }
            EngineCommand::GoBack { session } => self.step(session, false, now),
            EngineCommand::GoForward { session } => self.step(session, true, now),
            EngineCommand::Stop { session } => self.stop(session),
            EngineCommand::FetchManifest {
                session, request, ..
            } => {
                let current = self
                    .views
                    .get(&session)
                    .and_then(|v| v.history.get(v.position).cloned());
                if let (Some(url), Some(view)) = (current, self.views.get_mut(&session)) {
                    view.pending.push((
                        now + Duration::from_millis(80),
                        EngineEvent::ManifestFetched {
                            session,
                            request,
                            result: manifest_for(&url),
                        },
                    ));
                    // `due` relies on time order.
                    view.pending.sort_by_key(|(at, _)| *at);
                }
            }
            EngineCommand::OpenExternal { url, .. } => self.os_handoffs.push(url),
            // The fake engine has no real view to show, hide or pause.
            EngineCommand::Show { .. }
            | EngineCommand::Hide { .. }
            | EngineCommand::Suspend { .. }
            | EngineCommand::Resume { .. }
            | EngineCommand::RespondToPermission { .. } => {}
        }
    }

    /// Simulates the web content process dying. `None` if the session has no view.
    pub fn crash(&mut self, session: SessionId) -> Option<EngineEvent> {
        let view = self.views.get_mut(&session)?;
        view.pending.clear();
        Some(EngineEvent::ContentProcessTerminated { session })
    }

    /// A permission prompt for `session`, as a page would raise it.
    pub fn permission_request(
        &mut self,
        session: SessionId,
        kind: PermissionKind,
        origin: Url,
    ) -> EngineEvent {
        self.next_request += 1;
        EngineEvent::PermissionRequested {
            session,
            request: PermissionRequestId(self.next_request),
            kind,
            origin,
        }
    }

    /// Events due at `now`, in order.
    pub fn due(&mut self, now: Instant) -> Vec<EngineEvent> {
        let mut out = std::mem::take(&mut self.immediate);
        for view in self.views.values_mut() {
            let split = view.pending.partition_point(|(at, _)| *at <= now);
            out.extend(view.pending.drain(..split).map(|(_, e)| e));
        }
        out
    }

    fn stop(&mut self, session: SessionId) {
        let Some(view) = self.views.get_mut(&session) else {
            return;
        };
        view.pending.clear();
        if let Some(url) = view.history.get(view.position).cloned() {
            let (can_go_back, can_go_forward) = view.flags();
            // A stopped first load still commits what it had, so the session leaves Loading.
            self.immediate.push(EngineEvent::NavigationChanged {
                session,
                url,
                phase: NavigationPhase::Committed,
                can_go_back,
                can_go_forward,
            });
            self.immediate.push(EngineEvent::ProgressChanged {
                session,
                permille: 1000,
            });
        }
    }

    fn step(&mut self, session: SessionId, forward: bool, now: Instant) {
        let Some(view) = self.views.get_mut(&session) else {
            return;
        };
        let target = if forward {
            view.position + 1
        } else {
            view.position.wrapping_sub(1)
        };
        let Some(url) = view.history.get(target).cloned() else {
            return;
        };
        view.position = target;
        self.load(session, url, false, now);
    }

    /// Schedules the events of one load. `push_history` is false for reload and back/forward.
    fn load(&mut self, session: SessionId, url: Url, push_history: bool, now: Instant) {
        self.loads += 1;
        let fails = self.loads.is_multiple_of(FAIL_EVERY);
        let Some(view) = self.views.get_mut(&session) else {
            return;
        };
        view.pending.clear();
        if push_history {
            view.history
                .truncate(view.position + usize::from(!view.history.is_empty()));
            view.history.push(url.clone());
            view.position = view.history.len() - 1;
        }
        let (can_go_back, can_go_forward) = view.flags();
        let host = url.host_str().unwrap_or_default().to_owned();
        let at = |ms: u64| now + Duration::from_millis(ms);
        let progress =
            |ms: u64, permille: u16| (at(ms), EngineEvent::ProgressChanged { session, permille });
        let nav = |ms: u64, phase: NavigationPhase| {
            (
                at(ms),
                EngineEvent::NavigationChanged {
                    session,
                    url: url.clone(),
                    phase,
                    can_go_back,
                    can_go_forward,
                },
            )
        };
        let mut events = vec![nav(0, NavigationPhase::Started), progress(50, 100)];
        if host.contains("badcert") {
            events.push(security(
                session,
                at(350),
                ConnectionSecurity::CertificateError,
            ));
            events.push((at(400), failure(session, LoadErrorKind::Tls, -1202)));
        } else if host.contains("fail") || fails {
            events.push((at(400), failure(session, LoadErrorKind::Network, -1009)));
        } else {
            events.push(progress(250, 400));
            let trust = if url.scheme() == "https" {
                ConnectionSecurity::Secure
            } else {
                ConnectionSecurity::Insecure
            };
            events.push(security(session, at(400), trust));
            events.push(nav(500, NavigationPhase::Committed));
            events.push((
                at(500),
                EngineEvent::TitleChanged {
                    session,
                    title: title_for(&url),
                },
            ));
            events.push(progress(700, 800));
            events.push(progress(900, 1000));
            events.push(nav(900, NavigationPhase::Finished));
            if url.path().contains("camera") {
                self.next_request += 1;
                events.push((
                    at(1000),
                    EngineEvent::PermissionRequested {
                        session,
                        request: PermissionRequestId(self.next_request),
                        kind: PermissionKind::Camera,
                        origin: url.clone(),
                    },
                ));
            }
        }
        if let Some(view) = self.views.get_mut(&session) {
            view.pending = events;
        }
    }
}

/// What the fake page at `url` answers when asked for its manifest.
fn manifest_for(url: &Url) -> ManifestResult {
    let path = url.path();
    if path.contains("bigmanifest") {
        return ManifestResult::Failed(ManifestFailure::TooLarge);
    }
    let manifest_url = url
        .join("/manifest.webmanifest")
        .unwrap_or_else(|_| url.clone());
    if path.contains("badmanifest") {
        return ManifestResult::Found {
            manifest_url,
            bytes: b"{ this is not json".to_vec(),
        };
    }
    if !path.contains("manifest") {
        return ManifestResult::NotFound;
    }
    let name = if path.contains("hostile") {
        format!("{}Bank <b>Secure</b>", '\u{202E}')
    } else {
        "Fake Shop".to_owned()
    };
    let json = format!(
        r##"{{"name":"{name}","short_name":"Shop","start_url":"/manifest/","scope":"/manifest/",
            "display":"standalone","theme_color":"#0a84ff","background_color":"#ffffff",
            "icons":[{{"src":"/icons/icon-192.png","sizes":"192x192","purpose":"any"}},
                     {{"src":"/icons/icon-512.png","sizes":"512x512","purpose":"maskable"}},
                     {{"src":"http://insecure.test/icon.png","sizes":"48x48"}}]}}"##
    );
    ManifestResult::Found {
        manifest_url,
        bytes: json.into_bytes(),
    }
}

/// The host's icon fetch, faked: no network. A real host fetches `request.url` over
/// `https` without cookies and returns at most `request.max_bytes`.
#[must_use]
pub fn fetch_icon(request: &IconFetchRequest) -> IconFetchResult {
    let host = request.url.host_str().unwrap_or_default();
    let path = request.url.path();
    if request.url.scheme() != "https" || host.contains("fail") || path.ends_with("/huge.png") {
        return IconFetchResult::Failed;
    }
    if path.ends_with("/bad.png") {
        return IconFetchResult::Fetched(b"definitely not a png".to_vec());
    }
    // A different colour per host, so two apps are told apart on the launcher.
    let hash = host
        .bytes()
        .fold(7_u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let [r, g, b, _] = hash.to_be_bytes();
    let side = 192_u32;
    let image = image::RgbaImage::from_fn(side, side, |x, y| {
        let (dx, dy) = (x.abs_diff(side / 2), y.abs_diff(side / 2));
        if dx * dx + dy * dy <= (side / 2 - 12).pow(2) {
            image::Rgba([r, g, b, 255])
        } else {
            image::Rgba([0, 0, 0, 0])
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    match image.write_to(&mut out, image::ImageFormat::Png) {
        Ok(()) if out.get_ref().len() <= request.max_bytes as usize => {
            IconFetchResult::Fetched(out.into_inner())
        }
        _ => IconFetchResult::Failed,
    }
}

fn security(
    session: SessionId,
    at: Instant,
    security: ConnectionSecurity,
) -> (Instant, EngineEvent) {
    (at, EngineEvent::SecurityChanged { session, security })
}

fn failure(session: SessionId, kind: LoadErrorKind, code: i32) -> EngineEvent {
    EngineEvent::LoadFailed {
        session,
        kind,
        code,
    }
}

fn title_for(url: &Url) -> String {
    let host = url.host_str().unwrap_or("unknown");
    if url.path().contains("hostile") {
        return format!(
            "\u{202E}**{host}** <b>not bold</b> {}",
            "very long title ".repeat(30)
        );
    }
    format!("Fake page on {host}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn drain(engine: &mut FakeEngine, start: Instant) -> Vec<EngineEvent> {
        engine.due(start + Duration::from_secs(5))
    }

    #[test]
    fn a_normal_load_commits_with_a_title_and_reaches_full_progress() {
        let mut e = FakeEngine::new();
        let t0 = Instant::now();
        let s = SessionId::new(1);
        e.execute(
            EngineCommand::CreateSession {
                session: s,
                profile: krypa_engine_api::ProfileKind::Default,
                initial_url: Some(url("https://ok.example/")),
            },
            t0,
        );
        let events = drain(&mut e, t0);
        assert!(events.iter().any(|ev| matches!(
            ev,
            EngineEvent::NavigationChanged {
                phase: NavigationPhase::Committed,
                ..
            }
        )));
        assert!(events
            .iter()
            .any(|ev| matches!(ev, EngineEvent::TitleChanged { .. })));
        assert!(events
            .iter()
            .any(|ev| matches!(ev, EngineEvent::ProgressChanged { permille: 1000, .. })));
        assert!(!e.busy());
    }

    #[test]
    fn failing_hosts_fail_with_the_right_kind() {
        let mut e = FakeEngine::new();
        let t0 = Instant::now();
        for (id, host, kind) in [
            (1, "https://badcert.test/", LoadErrorKind::Tls),
            (2, "https://fail.test/", LoadErrorKind::Network),
        ] {
            let s = SessionId::new(id);
            e.execute(
                EngineCommand::CreateSession {
                    session: s,
                    profile: krypa_engine_api::ProfileKind::Default,
                    initial_url: Some(url(host)),
                },
                t0,
            );
            let events = drain(&mut e, t0);
            assert!(
                events
                    .iter()
                    .any(|ev| matches!(ev, EngineEvent::LoadFailed { kind: k, .. } if *k == kind)),
                "{host}"
            );
        }
    }

    /// The sample pages of the harness must report the state their name promises.
    #[test]
    fn sample_pages_report_the_security_their_name_promises() {
        let mut e = FakeEngine::new();
        let t0 = Instant::now();
        for (id, page, expected) in [
            (1, "https://example.com/", ConnectionSecurity::Secure),
            (2, "http://insecure.test/", ConnectionSecurity::Insecure),
            (
                3,
                "https://badcert.test/",
                ConnectionSecurity::CertificateError,
            ),
        ] {
            let s = SessionId::new(id);
            e.execute(
                EngineCommand::CreateSession {
                    session: s,
                    profile: krypa_engine_api::ProfileKind::Default,
                    initial_url: Some(url(page)),
                },
                t0,
            );
            let reported: Vec<_> = drain(&mut e, t0)
                .into_iter()
                .filter_map(|ev| match ev {
                    EngineEvent::SecurityChanged { security, .. } => Some(security),
                    _ => None,
                })
                .collect();
            assert_eq!(reported, vec![expected], "{page}");
        }
    }

    #[test]
    fn destroy_is_acknowledged() {
        let mut e = FakeEngine::new();
        let t0 = Instant::now();
        let s = SessionId::new(1);
        e.execute(
            EngineCommand::CreateSession {
                session: s,
                profile: krypa_engine_api::ProfileKind::Default,
                initial_url: None,
            },
            t0,
        );
        e.execute(EngineCommand::DestroySession { session: s }, t0);
        assert_eq!(
            e.due(t0),
            vec![EngineEvent::SessionDestroyed { session: s }]
        );
        assert_eq!(e.view_count(), 0);
    }
}
