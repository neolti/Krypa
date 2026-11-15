//! Manager tests against an in-test fake engine that behaves like a strict platform
//! adapter: it panics on commands that make no sense (show a view that does not exist,
//! create one twice) and answers every `DestroySession` with `SessionDestroyed`.

use std::collections::BTreeMap;

use krypa_engine_api::{
    ConnectionSecurity, EngineCapabilities, EngineCommand, EngineEvent, LoadErrorKind,
    NavigationKind, NavigationPhase, NavigationRequest, NavigationTarget, PermissionDecision,
    PermissionKind, PermissionRequestId, ProfileId, ProfileKind, SessionId,
};
use url::Url;

use crate::{
    CoreError, FailureReason, ManagerConfig, MemoryPressure, NavigationDecision, Notice, Outcome,
    SessionManager, SessionSpec, SessionState, Trigger,
};

fn u(s: &str) -> Url {
    Url::parse(s).unwrap()
}

#[derive(Default)]
struct FakeEngine {
    /// Live views and whether each was told to suspend.
    views: BTreeMap<SessionId, bool>,
    visible: Option<SessionId>,
    max_live_seen: usize,
}

impl FakeEngine {
    /// Applies commands like a platform would; returns the events it answers with.
    fn apply(&mut self, commands: &[EngineCommand]) -> Vec<EngineEvent> {
        let mut answers = Vec::new();
        for c in commands {
            match c {
                EngineCommand::CreateSession { session, .. } => {
                    assert!(
                        self.views.insert(*session, false).is_none(),
                        "double create"
                    );
                }
                EngineCommand::DestroySession { session } => {
                    assert!(
                        self.views.remove(session).is_some(),
                        "destroy of missing view"
                    );
                    if self.visible == Some(*session) {
                        self.visible = None;
                    }
                    answers.push(EngineEvent::SessionDestroyed { session: *session });
                }
                EngineCommand::Show { session } => {
                    assert!(self.views.contains_key(session), "show of missing view");
                    assert!(self.visible.is_none(), "two visible views");
                    self.visible = Some(*session);
                }
                EngineCommand::Hide { session } => {
                    assert!(self.views.contains_key(session), "hide of missing view");
                    assert_eq!(self.visible, Some(*session), "hide of a view not on screen");
                    self.visible = None;
                }
                EngineCommand::Suspend { session } => {
                    *self
                        .views
                        .get_mut(session)
                        .expect("suspend of missing view") = true;
                }
                EngineCommand::Resume { session } => {
                    *self.views.get_mut(session).expect("resume of missing view") = false;
                }
                EngineCommand::Load { session, .. }
                | EngineCommand::Reload { session }
                | EngineCommand::Stop { session }
                | EngineCommand::GoBack { session }
                | EngineCommand::GoForward { session }
                | EngineCommand::FetchManifest { session, .. }
                | EngineCommand::OpenExternal { session, .. }
                | EngineCommand::RespondToPermission { session, .. } => {
                    assert!(self.views.contains_key(session), "command to missing view");
                }
            }
            self.max_live_seen = self.max_live_seen.max(self.views.len());
        }
        answers
    }
}

struct Rig {
    m: SessionManager,
    e: FakeEngine,
    notices: Vec<Notice>,
    budget: usize,
}

impl Rig {
    fn new(budget: usize) -> Self {
        Self::with_caps(budget, EngineCapabilities::default())
    }

    fn with_caps(budget: usize, caps: EngineCapabilities) -> Self {
        Self {
            m: SessionManager::new(ManagerConfig::new(budget).unwrap(), caps),
            e: FakeEngine::default(),
            notices: Vec::new(),
            budget,
        }
    }

    fn full_caps() -> EngineCapabilities {
        EngineCapabilities {
            service_workers: true,
            passkeys: true,
            add_to_home_screen: true,
            persistent_profiles: true,
            private_sessions: true,
            content_blocking: true,
            view_suspend: true,
        }
    }

    /// Applies an outcome to the fake engine, feeds its answers back, checks invariants.
    fn run(&mut self, out: Outcome) -> Outcome {
        let answers = self.e.apply(&out.commands);
        self.notices.extend(out.notices.clone());
        for a in answers {
            let reply = self.m.handle_event(a).unwrap();
            assert_eq!(reply, Outcome::default());
        }
        self.check_invariants();
        out
    }

    fn check_invariants(&self) {
        assert!(self.m.live_view_count() <= self.budget, "budget exceeded");
        assert!(
            self.e.max_live_seen <= self.budget,
            "platform saw too many views"
        );
        let manager_live: Vec<SessionId> = self
            .m
            .sessions()
            .iter()
            .filter(|s| s.state.has_live_view())
            .map(|s| s.id)
            .collect();
        let engine_live: Vec<SessionId> = self.e.views.keys().copied().collect();
        assert_eq!(
            manager_live, engine_live,
            "manager and platform disagree on live views"
        );
        let fg = self.m.foreground_session();
        if let Some(id) = fg {
            assert!(self.m.info(id).unwrap().state.has_live_view());
            assert_eq!(self.e.visible, Some(id));
        }
        let foreground_states = self
            .m
            .sessions()
            .iter()
            .filter(|s| s.state == SessionState::Foreground)
            .count();
        assert!(foreground_states <= 1);
    }

    fn session(&mut self) -> SessionId {
        self.session_with(ProfileKind::Default)
    }

    fn session_with(&mut self, profile: ProfileKind) -> SessionId {
        self.m
            .create_session(SessionSpec {
                profile,
                url: Some(u("https://start.test/")),
                app_scope: None,
            })
            .unwrap()
    }

    fn fg(&mut self, id: SessionId) -> Outcome {
        let out = self.m.foreground(id).unwrap();
        self.run(out)
    }

    fn event(&mut self, ev: EngineEvent) -> Outcome {
        let out = self.m.handle_event(ev).unwrap();
        self.run(out)
    }

    fn commit(&mut self, id: SessionId, url: &str) {
        self.event(EngineEvent::NavigationChanged {
            session: id,
            url: u(url),
            phase: NavigationPhase::Committed,
            can_go_back: false,
            can_go_forward: false,
        });
    }

    fn state(&self, id: SessionId) -> SessionState {
        self.m.info(id).unwrap().state
    }
}

#[test]
fn live_views_never_exceed_the_budget_however_many_sessions_are_foregrounded() {
    let mut r = Rig::new(3);
    let ids: Vec<_> = (0..10).map(|_| r.session()).collect();
    for round in 0..3 {
        for id in &ids {
            r.fg(*id);
            r.commit(*id, &format!("https://site.test/{round}"));
        }
    }
    assert_eq!(r.e.max_live_seen, 3);
    assert_eq!(r.m.live_view_count(), 3);
}

#[test]
fn eviction_order_is_least_recently_foregrounded_not_creation_order() {
    let mut r = Rig::new(3);
    let [a, b, c, d, e] = [(); 5].map(|()| r.session());
    r.fg(a);
    r.fg(b);
    r.fg(c);
    r.fg(a); // recency now: b (oldest), c, a
    r.fg(d); // must evict b
    assert_eq!(r.state(b), SessionState::Terminated);
    assert!(r.state(c).has_live_view() && r.state(a).has_live_view());
    r.fg(e); // must evict c, not a
    assert_eq!(r.state(c), SessionState::Terminated);
    assert!(r.state(a).has_live_view());
    assert!(r.notices.contains(&Notice::ViewEvicted { session: b }));
}

#[test]
fn memory_pressure_never_destroys_the_foreground_session() {
    let mut r = Rig::new(5);
    let ids: Vec<_> = (0..5).map(|_| r.session()).collect();
    for id in &ids {
        r.fg(*id);
    }
    let out = r.m.on_memory_pressure(MemoryPressure::Warning).unwrap();
    r.run(out);
    assert_eq!(r.m.live_view_count(), 3); // ceil(5/2)
    assert_eq!(r.state(ids[4]), SessionState::Loading);
    // The oldest went first.
    assert_eq!(r.state(ids[0]), SessionState::Terminated);
    assert_eq!(r.state(ids[1]), SessionState::Terminated);
    let out = r.m.on_memory_pressure(MemoryPressure::Critical).unwrap();
    r.run(out);
    assert_eq!(r.m.live_view_count(), 1);
    assert_eq!(r.m.foreground_session(), Some(ids[4]));
    assert!(r.e.views.contains_key(&ids[4]));
}

#[test]
fn memory_pressure_evicts_background_views_even_when_they_are_suspended_or_failed() {
    let mut r = Rig::with_caps(4, Rig::full_caps());
    let [a, b, c] = [(); 3].map(|()| r.session());
    for id in [a, b, c] {
        r.fg(id);
        r.commit(id, "https://x.test/");
    }
    let out = r.m.suspend(a).unwrap();
    r.run(out);
    r.event(EngineEvent::ContentProcessTerminated { session: b });
    assert_eq!(r.state(a), SessionState::Suspended);
    assert_eq!(r.state(b), SessionState::Failed);
    let out = r.m.on_memory_pressure(MemoryPressure::Critical).unwrap();
    r.run(out);
    assert_eq!(r.state(a), SessionState::Terminated);
    assert_eq!(r.state(b), SessionState::Terminated);
    assert_eq!(r.state(c), SessionState::Foreground);
}

#[test]
fn a_terminated_session_keeps_its_url_and_is_recreated_from_it_on_next_foreground() {
    let mut r = Rig::new(1);
    let a = r.session();
    let b = r.session();
    r.fg(a);
    r.commit(a, "https://news.test/story");
    r.fg(b); // budget 1: a is evicted
    let info = r.m.info(a).unwrap();
    assert_eq!(info.state, SessionState::Terminated);
    assert_eq!(info.restorable_url, Some(u("https://news.test/story")));
    let out = r.fg(a);
    assert!(out.commands.contains(&EngineCommand::CreateSession {
        session: a,
        profile: ProfileKind::Default,
        initial_url: Some(u("https://news.test/story")),
    }));
    assert_eq!(r.state(a), SessionState::Loading);
}

#[test]
fn suspended_keeps_a_live_view_while_terminated_does_not() {
    let mut r = Rig::with_caps(2, Rig::full_caps());
    let [a, b, c] = [(); 3].map(|()| r.session());
    r.fg(a);
    r.commit(a, "https://a.test/");
    r.fg(b);
    r.commit(b, "https://b.test/");
    let out = r.m.suspend(a).unwrap();
    r.run(out);
    assert_eq!(r.state(a), SessionState::Suspended);
    assert!(r.e.views.contains_key(&a), "suspended view must stay alive");
    r.fg(c); // evicts a (least recently foregrounded)
    assert_eq!(r.state(a), SessionState::Terminated);
    assert!(!r.e.views.contains_key(&a), "terminated view must be gone");
}

#[test]
fn without_a_real_suspend_the_manager_still_works_and_sends_no_pause_command() {
    let mut r = Rig::new(3); // view_suspend = false
    let [a, b] = [(); 2].map(|()| r.session());
    r.fg(a);
    r.commit(a, "https://a.test/");
    r.fg(b);
    let out = r.m.suspend(a).unwrap();
    assert!(out.commands.is_empty(), "no platform call that cannot work");
    r.run(out);
    assert_eq!(r.state(a), SessionState::Suspended);
    let out = r.fg(a);
    assert_eq!(
        out.commands,
        vec![
            EngineCommand::Hide { session: b },
            EngineCommand::Show { session: a },
        ]
    );
    assert_eq!(r.state(a), SessionState::Foreground);
    // And back to Background -> Suspended via resume.
    r.fg(b);
    let out = r.m.suspend(a).unwrap();
    r.run(out);
    let out = r.m.resume(a).unwrap();
    assert!(out.commands.is_empty());
    assert_eq!(r.state(a), SessionState::Background);
}

#[test]
fn with_a_real_suspend_the_platform_is_told_to_pause_and_to_resume() {
    let mut r = Rig::with_caps(3, Rig::full_caps());
    let [a, b] = [(); 2].map(|()| r.session());
    r.fg(a);
    r.commit(a, "https://a.test/");
    r.fg(b);
    let out = r.m.suspend(a).unwrap();
    assert_eq!(out.commands, vec![EngineCommand::Suspend { session: a }]);
    r.run(out);
    assert!(r.e.views[&a]);
    let out = r.fg(a);
    assert_eq!(
        out.commands,
        vec![
            EngineCommand::Hide { session: b },
            EngineCommand::Resume { session: a },
            EngineCommand::Show { session: a },
        ]
    );
    assert!(!r.e.views[&a]);
}

#[test]
fn a_foreground_crash_is_surfaced_as_failed_and_is_not_silently_reloaded() {
    let mut r = Rig::new(3);
    let a = r.session();
    r.fg(a);
    r.commit(a, "https://a.test/");
    let out = r.event(EngineEvent::ContentProcessTerminated { session: a });
    assert!(out.commands.is_empty(), "no automatic reload");
    assert_eq!(r.state(a), SessionState::Failed);
    assert_eq!(
        r.m.info(a).unwrap().failure,
        Some(FailureReason::ContentProcessTerminated)
    );
    assert!(r.notices.contains(&Notice::SessionFailed {
        session: a,
        reason: FailureReason::ContentProcessTerminated
    }));
    // Stray navigation events do not quietly revive it.
    r.commit(a, "https://a.test/again");
    assert_eq!(r.state(a), SessionState::Failed);
    assert_eq!(r.m.foreground_session(), Some(a));
}

#[test]
fn a_failed_session_reloads_on_explicit_user_reload() {
    let mut r = Rig::new(3);
    let a = r.session();
    r.fg(a);
    r.commit(a, "https://a.test/");
    r.event(EngineEvent::ContentProcessTerminated { session: a });
    let out = r.m.reload(a).unwrap();
    assert_eq!(out.commands, vec![EngineCommand::Reload { session: a }]);
    r.run(out);
    assert_eq!(r.state(a), SessionState::Loading);
    assert_eq!(r.m.info(a).unwrap().failure, None);
    r.commit(a, "https://a.test/");
    assert_eq!(r.state(a), SessionState::Foreground);
}

#[test]
fn a_failed_session_can_also_be_recovered_by_navigating_elsewhere() {
    let mut r = Rig::new(3);
    let a = r.session();
    r.fg(a);
    r.event(EngineEvent::LoadFailed {
        session: a,
        kind: LoadErrorKind::Network,
        code: -1009,
    });
    assert_eq!(r.state(a), SessionState::Failed);
    let out = r.m.navigate(a, u("https://other.test/")).unwrap();
    r.run(out);
    assert_eq!(r.state(a), SessionState::Loading);
}

#[test]
fn a_crashed_session_reloads_on_next_foreground_not_before() {
    let mut r = Rig::new(3);
    let [a, b] = [(); 2].map(|()| r.session());
    r.fg(a);
    r.commit(a, "https://a.test/");
    r.event(EngineEvent::ContentProcessTerminated { session: a });
    let out = r.fg(b);
    assert!(!out.commands.contains(&EngineCommand::Reload { session: a }));
    assert_eq!(
        r.state(a),
        SessionState::Failed,
        "stays failed while in the background"
    );
    let out = r.fg(a);
    assert!(out.commands.contains(&EngineCommand::Reload { session: a }));
    assert_eq!(r.state(a), SessionState::Loading);
}

#[test]
fn a_background_crash_also_marks_failed_and_reloads_when_the_user_returns() {
    let mut r = Rig::with_caps(3, Rig::full_caps());
    let [a, b] = [(); 2].map(|()| r.session());
    r.fg(a);
    r.commit(a, "https://a.test/");
    r.fg(b);
    r.event(EngineEvent::ContentProcessTerminated { session: a });
    assert_eq!(r.state(a), SessionState::Failed);
    let out = r.fg(a);
    assert!(out.commands.contains(&EngineCommand::Reload { session: a }));
}

#[test]
fn a_cancelled_load_is_not_a_failure_but_a_network_error_is() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    r.commit(a, "https://a.test/");
    r.event(EngineEvent::LoadFailed {
        session: a,
        kind: LoadErrorKind::Cancelled,
        code: -999,
    });
    assert_eq!(r.state(a), SessionState::Foreground);
    r.event(EngineEvent::LoadFailed {
        session: a,
        kind: LoadErrorKind::Tls,
        code: -1202,
    });
    assert_eq!(
        r.m.info(a).unwrap().failure,
        Some(FailureReason::LoadFailed(LoadErrorKind::Tls))
    );
}

#[test]
fn private_sessions_leave_no_restorable_state_after_eviction_or_close() {
    let mut r = Rig::with_caps(1, Rig::full_caps());
    let p = r.session_with(ProfileKind::Private);
    let n = r.session();
    r.fg(p);
    r.commit(p, "https://secret.test/page?token=1");
    assert_eq!(
        r.m.info(p).unwrap().restorable_url,
        None,
        "even while alive"
    );
    r.fg(n); // budget 1 evicts the private session
    assert_eq!(r.m.info(p), Err(CoreError::SessionDestroyed(p)));
    assert!(r
        .notices
        .contains(&Notice::PrivateSessionDiscarded { session: p }));
    assert!(r.m.sessions().iter().all(|s| s.id != p));

    let mut r = Rig::with_caps(3, Rig::full_caps());
    let p = r.session_with(ProfileKind::Private);
    r.fg(p);
    r.commit(p, "https://secret.test/");
    let out = r.m.close_session(p).unwrap();
    r.run(out);
    assert_eq!(r.m.info(p), Err(CoreError::SessionDestroyed(p)));
    assert!(r.m.sessions().is_empty());
}

#[test]
fn a_private_view_lost_by_the_platform_is_discarded_not_kept_as_terminated() {
    let mut r = Rig::with_caps(3, Rig::full_caps());
    let p = r.session_with(ProfileKind::Private);
    r.fg(p);
    r.commit(p, "https://secret.test/");
    r.e.views.remove(&p);
    r.e.visible = None;
    let out =
        r.m.handle_event(EngineEvent::SessionDestroyed { session: p })
            .unwrap();
    assert_eq!(
        out.notices,
        vec![Notice::PrivateSessionDiscarded { session: p }]
    );
    assert_eq!(r.m.info(p), Err(CoreError::SessionDestroyed(p)));
    assert_eq!(r.m.foreground_session(), None);
}

#[test]
fn a_persistent_profile_session_keeps_its_restorable_url_after_eviction() {
    let mut r = Rig::with_caps(1, Rig::full_caps());
    let profile = ProfileKind::Persistent(ProfileId("work".into()));
    let w = r.session_with(profile.clone());
    let n = r.session();
    r.fg(w);
    r.commit(w, "https://work.test/doc");
    r.fg(n);
    let info = r.m.info(w).unwrap();
    assert_eq!(info.state, SessionState::Terminated);
    assert_eq!(info.restorable_url, Some(u("https://work.test/doc")));
    assert_eq!(info.profile, profile);
}

#[test]
fn events_for_sessions_that_never_existed_are_rejected_loudly() {
    let mut r = Rig::new(2);
    let ghost = SessionId::new(99);
    let events = [
        EngineEvent::ContentProcessTerminated { session: ghost },
        EngineEvent::TitleChanged {
            session: ghost,
            title: "x".into(),
        },
        EngineEvent::SessionDestroyed { session: ghost },
        EngineEvent::SessionDestroyed {
            session: SessionId::new(0),
        },
    ];
    for ev in events {
        assert!(matches!(
            r.m.handle_event(ev),
            Err(CoreError::UnknownSession(_))
        ));
    }
}

#[test]
fn events_for_closed_sessions_are_rejected_and_only_one_destroy_answer_is_accepted() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    let out = r.m.close_session(a).unwrap();
    assert_eq!(
        out.commands,
        vec![EngineCommand::DestroySession { session: a }]
    );
    // Late event before the platform answered the destroy.
    assert_eq!(
        r.m.handle_event(EngineEvent::TitleChanged {
            session: a,
            title: "late".into()
        }),
        Err(CoreError::SessionDestroyed(a))
    );
    // The platform's answer is accepted exactly once.
    r.run(out); // feeds the first SessionDestroyed
    assert_eq!(
        r.m.handle_event(EngineEvent::SessionDestroyed { session: a }),
        Err(CoreError::SessionDestroyed(a))
    );
    assert_eq!(
        r.m.handle_event(EngineEvent::ContentProcessTerminated { session: a }),
        Err(CoreError::SessionDestroyed(a))
    );
}

#[test]
fn an_event_from_an_evicted_view_is_rejected_because_it_has_no_live_view() {
    let mut r = Rig::new(1);
    let [a, b] = [(); 2].map(|()| r.session());
    r.fg(a);
    r.fg(b);
    assert_eq!(
        r.m.handle_event(EngineEvent::TitleChanged {
            session: a,
            title: "late".into()
        }),
        Err(CoreError::NotAllowedInState {
            session: a,
            state: SessionState::Terminated,
            what: "TitleChanged"
        })
    );
    // A session that was never started cannot send events either.
    let c = r.session();
    assert!(matches!(
        r.m.handle_event(EngineEvent::ContentProcessTerminated { session: c }),
        Err(CoreError::NotAllowedInState { .. })
    ));
}

#[test]
fn illegal_lifecycle_intents_return_typed_errors_and_change_nothing() {
    let mut r = Rig::with_caps(3, Rig::full_caps());
    let [a, b] = [(); 2].map(|()| r.session());
    r.fg(a);
    r.commit(a, "https://a.test/");
    // Foreground cannot be suspended; background cannot be resumed.
    assert_eq!(
        r.m.suspend(a),
        Err(CoreError::IllegalTransition {
            session: a,
            from: SessionState::Foreground,
            trigger: Trigger::Suspend
        })
    );
    r.fg(b);
    assert_eq!(
        r.m.resume(a),
        Err(CoreError::IllegalTransition {
            session: a,
            from: SessionState::Background,
            trigger: Trigger::Resume
        })
    );
    let out = r.m.suspend(a).unwrap();
    r.run(out);
    // Suspended: suspending again is illegal, and navigation needs a resume first.
    assert!(matches!(
        r.m.suspend(a),
        Err(CoreError::IllegalTransition { .. })
    ));
    assert!(matches!(
        r.m.reload(a),
        Err(CoreError::NotAllowedInState { .. })
    ));
    // A session with no view cannot navigate or be suspended.
    let c = r.session();
    assert!(matches!(
        r.m.navigate(c, u("https://x.test/")),
        Err(CoreError::NotAllowedInState {
            state: SessionState::NotRunning,
            ..
        })
    ));
    assert!(matches!(
        r.m.suspend(c),
        Err(CoreError::IllegalTransition {
            from: SessionState::NotRunning,
            ..
        })
    ));
    assert_eq!(r.state(a), SessionState::Suspended);
    r.check_invariants();
}

#[test]
fn a_session_that_never_loaded_cannot_get_a_failure_or_commit_event() {
    let mut r = Rig::new(2);
    let a = r.session();
    assert!(matches!(
        r.m.handle_event(EngineEvent::LoadFailed {
            session: a,
            kind: LoadErrorKind::Network,
            code: 1
        }),
        Err(CoreError::NotAllowedInState { .. })
    ));
}

#[test]
fn a_load_failure_in_a_suspended_view_is_an_illegal_transition_not_ignored() {
    let mut r = Rig::with_caps(3, Rig::full_caps());
    let [a, b] = [(); 2].map(|()| r.session());
    r.fg(a);
    r.commit(a, "https://a.test/");
    r.fg(b);
    let out = r.m.suspend(a).unwrap();
    r.run(out);
    assert!(matches!(
        r.m.handle_event(EngineEvent::LoadFailed {
            session: a,
            kind: LoadErrorKind::Network,
            code: 1
        }),
        Err(CoreError::IllegalTransition {
            from: SessionState::Suspended,
            ..
        })
    ));
    // But a crash of a suspended view is legal: the OS may kill it.
    r.event(EngineEvent::ContentProcessTerminated { session: a });
    assert_eq!(r.state(a), SessionState::Failed);
}

#[test]
fn a_javascript_or_credentialed_url_can_never_be_loaded_or_start_a_session() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    for bad in [
        "javascript:alert(1)",
        "data:text/html,x",
        "file:///x",
        "https://bank.com@evil.test/",
    ] {
        assert_eq!(r.m.navigate(a, u(bad)), Err(CoreError::UrlNotLoadable));
        assert_eq!(
            r.m.create_session(SessionSpec {
                profile: ProfileKind::Default,
                url: Some(u(bad)),
                app_scope: None
            }),
            Err(CoreError::UrlNotLoadable)
        );
    }
}

#[test]
fn losing_the_foreground_view_unprompted_clears_foreground_and_recreates_on_request() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    r.commit(a, "https://a.test/x");
    r.e.views.remove(&a);
    r.e.visible = None;
    let out =
        r.m.handle_event(EngineEvent::SessionDestroyed { session: a })
            .unwrap();
    assert_eq!(out.notices, vec![Notice::ViewLost { session: a }]);
    assert_eq!(r.m.foreground_session(), None);
    assert_eq!(r.state(a), SessionState::Terminated);
    r.check_invariants();
    r.fg(a);
    assert_eq!(r.state(a), SessionState::Loading);
}

#[test]
fn first_commit_leaves_loading_for_foreground_or_background_depending_on_focus() {
    let mut r = Rig::new(3);
    let [a, b] = [(); 2].map(|()| r.session());
    r.fg(a);
    r.fg(b);
    assert_eq!(
        r.state(a),
        SessionState::Loading,
        "switching away mid-load keeps Loading"
    );
    r.commit(a, "https://a.test/");
    assert_eq!(r.state(a), SessionState::Background);
    r.commit(b, "https://b.test/");
    assert_eq!(r.state(b), SessionState::Foreground);
}

#[test]
fn switching_foreground_hides_the_previous_view_and_keeps_one_foreground() {
    let mut r = Rig::new(3);
    let [a, b] = [(); 2].map(|()| r.session());
    r.fg(a);
    r.commit(a, "https://a.test/");
    let out = r.fg(b);
    assert_eq!(out.commands[0], EngineCommand::Hide { session: a });
    assert_eq!(r.state(a), SessionState::Background);
    let again = r.fg(b);
    assert!(
        again.commands.is_empty(),
        "foregrounding the foreground is a no-op"
    );
}

#[test]
fn closing_the_foreground_session_destroys_its_view_and_ids_are_never_reused() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    let out = r.m.close_session(a).unwrap();
    r.run(out);
    assert_eq!(r.m.foreground_session(), None);
    let b = r.session();
    assert_ne!(a, b);
    assert_eq!(r.m.close_session(a), Err(CoreError::SessionDestroyed(a)));
}

#[test]
fn a_window_opened_by_a_user_gesture_becomes_a_foreground_session_in_the_same_profile() {
    let mut r = Rig::with_caps(3, Rig::full_caps());
    let p = r.session_with(ProfileKind::Private);
    r.fg(p);
    r.commit(p, "https://a.test/");
    let request = NavigationRequest {
        url: u("https://popup.test/"),
        target: NavigationTarget::NewWindow,
        kind: NavigationKind::LinkActivated,
        user_gesture: true,
    };
    assert_eq!(
        r.m.navigation_policy(p, &request),
        Ok(NavigationDecision::OpenInNewSession)
    );
    r.event(EngineEvent::NewWindowRequested {
        session: p,
        request,
    });
    let sessions = r.m.sessions();
    assert_eq!(sessions.len(), 2);
    let child = sessions.iter().find(|s| s.id != p).unwrap();
    assert_eq!(
        child.profile,
        ProfileKind::Private,
        "a popup of a private tab is private"
    );
    assert_eq!(r.m.foreground_session(), Some(child.id));
    assert!(r.notices.contains(&Notice::SessionOpened {
        session: child.id,
        opener: p
    }));
}

#[test]
fn a_popup_without_a_user_gesture_creates_nothing() {
    let mut r = Rig::new(3);
    let a = r.session();
    r.fg(a);
    let request = NavigationRequest {
        url: u("https://ads.test/"),
        target: NavigationTarget::NewWindow,
        kind: NavigationKind::Other,
        user_gesture: false,
    };
    let out = r.event(EngineEvent::NewWindowRequested {
        session: a,
        request,
    });
    assert_eq!(out, Outcome::default());
    assert_eq!(r.m.sessions().len(), 1);
}

#[test]
fn the_synchronous_policy_changes_no_state_and_uses_the_sessions_app_scope() {
    let mut r = Rig::new(3);
    let app =
        r.m.create_session(SessionSpec {
            profile: ProfileKind::Default,
            url: Some(u("https://app.test/")),
            app_scope: Some(u("https://app.test/")),
        })
        .unwrap();
    r.fg(app);
    let before = r.m.sessions();
    let leaving = NavigationRequest {
        url: u("https://elsewhere.test/"),
        target: NavigationTarget::MainFrame,
        kind: NavigationKind::LinkActivated,
        user_gesture: true,
    };
    assert_eq!(
        r.m.navigation_policy(app, &leaving),
        Ok(NavigationDecision::OpenInNewSession)
    );
    assert_eq!(r.m.sessions(), before);
    assert_eq!(
        r.m.navigation_policy(SessionId::new(42), &leaving),
        Err(CoreError::UnknownSession(SessionId::new(42)))
    );
}

#[test]
fn a_new_session_from_an_app_keeps_the_app_scope_only_for_in_scope_urls() {
    let mut r = Rig::new(4);
    let app =
        r.m.create_session(SessionSpec {
            profile: ProfileKind::Default,
            url: Some(u("https://app.test/")),
            app_scope: Some(u("https://app.test/")),
        })
        .unwrap();
    r.fg(app);
    let (inside, out) =
        r.m.open_new_session(app, &u("https://app.test/doc"))
            .unwrap();
    r.run(out);
    let (outside, out) =
        r.m.open_new_session(app, &u("https://other.test/"))
            .unwrap();
    r.run(out);
    let leaving = NavigationRequest {
        url: u("https://x.test/"),
        target: NavigationTarget::MainFrame,
        kind: NavigationKind::LinkActivated,
        user_gesture: true,
    };
    assert_eq!(
        r.m.navigation_policy(inside, &leaving),
        Ok(NavigationDecision::OpenInNewSession)
    );
    assert_eq!(
        r.m.navigation_policy(outside, &leaving),
        Ok(NavigationDecision::Allow)
    );
}

#[test]
fn external_scheme_requests_reach_the_ui_as_a_notice_and_unsafe_schemes_are_refused() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    let out = r.event(EngineEvent::ExternalSchemeRequested {
        session: a,
        url: u("mailto:someone@example.com"),
        user_gesture: true,
    });
    assert_eq!(out.commands, vec![]);
    assert!(matches!(
        out.notices.as_slice(),
        [Notice::ExternalOpenRequested { session, .. }] if *session == a
    ));
    for bad in ["javascript:alert(1)", "https://example.com/", "file:///x"] {
        assert_eq!(
            r.m.handle_event(EngineEvent::ExternalSchemeRequested {
                session: a,
                url: u(bad),
                user_gesture: true,
            }),
            Err(CoreError::UnsafeExternalScheme)
        );
    }
}

#[test]
fn a_permission_prompt_is_answered_once_and_dropped_when_the_page_process_dies() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    r.commit(a, "https://cam.test/");
    let req = PermissionRequestId(5);
    let ev = EngineEvent::PermissionRequested {
        session: a,
        request: req,
        kind: PermissionKind::Camera,
        origin: u("https://cam.test"),
    };
    r.event(ev.clone());
    assert_eq!(
        r.m.handle_event(ev),
        Err(CoreError::DuplicatePermissionRequest {
            session: a,
            request: req
        })
    );
    let out =
        r.m.respond_permission(a, req, PermissionDecision::Deny)
            .unwrap();
    assert_eq!(
        out.commands,
        vec![EngineCommand::RespondToPermission {
            session: a,
            request: req,
            decision: PermissionDecision::Deny
        }]
    );
    r.run(out);
    assert_eq!(
        r.m.respond_permission(a, req, PermissionDecision::Grant),
        Err(CoreError::UnknownPermissionRequest {
            session: a,
            request: req
        })
    );
    // A prompt that was pending when the content process died cannot be answered.
    r.event(EngineEvent::PermissionRequested {
        session: a,
        request: PermissionRequestId(6),
        kind: PermissionKind::Microphone,
        origin: u("https://cam.test"),
    });
    r.event(EngineEvent::ContentProcessTerminated { session: a });
    assert!(matches!(
        r.m.respond_permission(a, PermissionRequestId(6), PermissionDecision::Grant),
        Err(CoreError::UnknownPermissionRequest { .. })
    ));
}

#[test]
fn page_supplied_text_and_numbers_are_sanitized_or_rejected() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    r.event(EngineEvent::TitleChanged {
        session: a,
        title: "  Pay\u{202E}pal\u{0000}\n ".into(),
    });
    assert_eq!(r.m.info(a).unwrap().title.as_deref(), Some("Paypal"));
    r.event(EngineEvent::TitleChanged {
        session: a,
        title: "x".repeat(10_000),
    });
    assert_eq!(r.m.info(a).unwrap().title.unwrap().chars().count(), 256);
    assert_eq!(
        r.m.handle_event(EngineEvent::ProgressChanged {
            session: a,
            permille: 1001
        }),
        Err(CoreError::InvalidProgress(1001))
    );
    r.event(EngineEvent::ProgressChanged {
        session: a,
        permille: 1000,
    });
    assert_eq!(r.m.info(a).unwrap().progress_permille, 1000);
}

#[test]
fn history_flags_and_url_follow_navigation_events_but_not_the_started_phase() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    r.event(EngineEvent::NavigationChanged {
        session: a,
        url: u("https://a.test/2"),
        phase: NavigationPhase::Started,
        can_go_back: true,
        can_go_forward: false,
    });
    let info = r.m.info(a).unwrap();
    assert!(info.can_go_back);
    assert_eq!(info.current_url, Some(u("https://start.test/")));
    assert_eq!(info.state, SessionState::Loading);
}

#[test]
fn the_budget_must_be_at_least_one_and_unsupported_profiles_are_refused() {
    assert_eq!(ManagerConfig::new(0), Err(CoreError::InvalidBudget));
    let mut r = Rig::new(2); // default capabilities: nothing supported
    assert_eq!(
        r.m.create_session(SessionSpec {
            profile: ProfileKind::Private,
            url: None,
            app_scope: None
        }),
        Err(CoreError::CapabilityMissing("private_sessions"))
    );
    assert_eq!(
        r.m.create_session(SessionSpec {
            profile: ProfileKind::Persistent(ProfileId("p".into())),
            url: None,
            app_scope: None
        }),
        Err(CoreError::CapabilityMissing("persistent_profiles"))
    );
}

#[test]
fn navigation_commands_reach_the_platform_for_a_live_session() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    r.commit(a, "https://a.test/");
    let url = u("https://b.test/");
    for (out, expected) in [
        (
            r.m.navigate(a, url.clone()).unwrap(),
            EngineCommand::Load { session: a, url },
        ),
        (r.m.stop(a).unwrap(), EngineCommand::Stop { session: a }),
        (
            r.m.go_back(a).unwrap(),
            EngineCommand::GoBack { session: a },
        ),
        (
            r.m.go_forward(a).unwrap(),
            EngineCommand::GoForward { session: a },
        ),
        (r.m.reload(a).unwrap(), EngineCommand::Reload { session: a }),
    ] {
        assert_eq!(out.commands, vec![expected]);
        r.run(out);
    }
    assert_eq!(r.state(a), SessionState::Foreground);
}

// ------------------------------------------------------------- connection security

fn phase(r: &mut Rig, id: SessionId, url: &str, phase: NavigationPhase) {
    r.event(EngineEvent::NavigationChanged {
        session: id,
        url: u(url),
        phase,
        can_go_back: false,
        can_go_forward: false,
    });
}

fn report(r: &mut Rig, id: SessionId, security: ConnectionSecurity) {
    r.event(EngineEvent::SecurityChanged {
        session: id,
        security,
    });
}

fn security(r: &Rig, id: SessionId) -> Option<ConnectionSecurity> {
    r.m.info(id).unwrap().security
}

/// Why: "nothing confirmed" must read as unverified, never as secure, so a page the engine
/// said nothing about cannot borrow trust from its `https` URL.
#[test]
fn a_page_the_engine_never_reported_on_is_unverified_even_over_https() {
    let mut r = Rig::new(2);
    let a = r.session();
    assert_eq!(security(&r, a), None, "no view yet");
    r.fg(a);
    assert_eq!(security(&r, a), None, "view created, nothing reported");
    r.commit(a, "https://a.test/");
    assert_eq!(security(&r, a), None, "committed https, still no report");
}

/// Why: a stale "secure" shown for a new, not yet verified page is a security bug.
#[test]
fn starting_a_new_navigation_drops_the_previous_pages_security() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    phase(&mut r, a, "https://a.test/", NavigationPhase::Started);
    report(&mut r, a, ConnectionSecurity::Secure);
    phase(&mut r, a, "https://a.test/", NavigationPhase::Committed);
    assert_eq!(security(&r, a), Some(ConnectionSecurity::Secure));

    phase(&mut r, a, "https://b.test/", NavigationPhase::Started);
    assert_eq!(security(&r, a), None, "new page is not verified yet");
    phase(&mut r, a, "https://b.test/", NavigationPhase::Committed);
    assert_eq!(
        security(&r, a),
        None,
        "committing without a report for it must not resurrect the old verdict"
    );
}

/// Why: a commit with no preceding `Started` (an adapter that skips it) must still not
/// carry the old page's verdict over to the new document.
#[test]
fn a_commit_without_a_report_clears_the_previous_verdict_even_without_started() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    phase(&mut r, a, "https://a.test/", NavigationPhase::Committed);
    report(&mut r, a, ConnectionSecurity::Secure);
    phase(&mut r, a, "https://b.test/", NavigationPhase::Committed);
    assert_eq!(security(&r, a), None);
}

/// Why: the engine may report before the commit (server trust) or after it
/// (`hasOnlySecureContent`); neither order may leak the verdict onto the wrong page.
#[test]
fn a_report_made_while_loading_applies_only_once_that_page_commits() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    phase(&mut r, a, "https://a.test/", NavigationPhase::Started);
    phase(&mut r, a, "https://a.test/", NavigationPhase::Committed);
    report(&mut r, a, ConnectionSecurity::Secure);
    assert_eq!(security(&r, a), Some(ConnectionSecurity::Secure), "after");

    phase(&mut r, a, "http://b.test/", NavigationPhase::Started);
    report(&mut r, a, ConnectionSecurity::Insecure);
    assert_eq!(
        security(&r, a),
        None,
        "still showing the old page; the report is about the new one"
    );
    phase(&mut r, a, "http://b.test/", NavigationPhase::Committed);
    assert_eq!(
        security(&r, a),
        Some(ConnectionSecurity::Insecure),
        "before"
    );
}

/// Why: a certificate error must never be reported as secure, also not by a late or
/// confused engine report.
#[test]
fn a_tls_failure_is_a_certificate_error_and_a_late_secure_report_cannot_undo_it() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    phase(&mut r, a, "https://a.test/", NavigationPhase::Started);
    report(&mut r, a, ConnectionSecurity::Secure);
    phase(&mut r, a, "https://bad.test/", NavigationPhase::Started);
    r.event(EngineEvent::LoadFailed {
        session: a,
        kind: LoadErrorKind::Tls,
        code: -1202,
    });
    assert_eq!(security(&r, a), Some(ConnectionSecurity::CertificateError));
    report(&mut r, a, ConnectionSecurity::Secure);
    assert_eq!(security(&r, a), Some(ConnectionSecurity::CertificateError));

    // Recovering by reload starts over from unverified.
    let out = r.m.reload(a).unwrap();
    r.run(out);
    assert_eq!(security(&r, a), None);
}

#[test]
fn any_other_load_failure_leaves_the_page_unverified_not_trusted() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    phase(&mut r, a, "https://a.test/", NavigationPhase::Started);
    report(&mut r, a, ConnectionSecurity::Secure);
    phase(&mut r, a, "https://a.test/", NavigationPhase::Committed);
    phase(&mut r, a, "https://down.test/", NavigationPhase::Started);
    r.event(EngineEvent::LoadFailed {
        session: a,
        kind: LoadErrorKind::Network,
        code: -1009,
    });
    assert_eq!(security(&r, a), None);
}

/// Why: plain HTTP can never be secure, even if the engine claims so.
#[test]
fn secure_is_downgraded_when_the_committed_url_is_not_https() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    phase(&mut r, a, "http://plain.test/", NavigationPhase::Started);
    report(&mut r, a, ConnectionSecurity::Secure);
    phase(&mut r, a, "http://plain.test/", NavigationPhase::Committed);
    assert_eq!(security(&r, a), Some(ConnectionSecurity::Insecure));
}

/// Why: sessions are isolated; a report for one tab must not change another, and a report
/// for a session that no longer exists must be rejected, not applied.
#[test]
fn a_report_changes_only_its_own_session_and_stale_sessions_are_rejected() {
    let mut r = Rig::new(3);
    let a = r.session();
    let b = r.session();
    r.fg(a);
    phase(&mut r, a, "https://a.test/", NavigationPhase::Committed);
    report(&mut r, a, ConnectionSecurity::Secure);
    r.fg(b);
    phase(&mut r, b, "https://b.test/", NavigationPhase::Committed);
    report(&mut r, b, ConnectionSecurity::CertificateError);
    assert_eq!(security(&r, a), Some(ConnectionSecurity::Secure));
    assert_eq!(security(&r, b), Some(ConnectionSecurity::CertificateError));

    let out = r.m.close_session(b).unwrap();
    r.run(out);
    let stale = r.m.handle_event(EngineEvent::SecurityChanged {
        session: b,
        security: ConnectionSecurity::Secure,
    });
    assert!(matches!(stale, Err(CoreError::SessionDestroyed(_))));
    assert!(matches!(
        r.m.handle_event(EngineEvent::SecurityChanged {
            session: SessionId::new(999),
            security: ConnectionSecurity::Secure,
        }),
        Err(CoreError::UnknownSession(_))
    ));
    assert_eq!(security(&r, a), Some(ConnectionSecurity::Secure));
}

/// Why: a recreated view (after a crash or eviction) is a new engine state; the old
/// verdict must not survive it.
#[test]
fn losing_the_view_forgets_the_security_state() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    phase(&mut r, a, "https://a.test/", NavigationPhase::Committed);
    report(&mut r, a, ConnectionSecurity::Secure);
    r.event(EngineEvent::ContentProcessTerminated { session: a });
    assert_eq!(security(&r, a), None);

    let out = r.m.reload(a).unwrap();
    r.run(out);
    phase(&mut r, a, "https://a.test/", NavigationPhase::Committed);
    report(&mut r, a, ConnectionSecurity::Secure);
    let b = r.session();
    r.fg(b);
    let out = r.m.on_memory_pressure(MemoryPressure::Critical).unwrap();
    r.run(out);
    assert_eq!(r.state(a), SessionState::Terminated);
    assert_eq!(security(&r, a), None);
}

// ------------------------------------------------------------ install / manifest

use krypa_apps::{InstalledApp, MAX_MANIFEST_BYTES};
use krypa_engine_api::{ManifestFailure, ManifestRequestId, ManifestResult};

use crate::{InstallRefusal, InstallSource, ManualReason};

fn shop(r: &mut Rig) -> SessionId {
    let a = r.session();
    r.fg(a);
    r.commit(a, "https://shop.example/cart?token=secret");
    // Only a page the engine confirmed as secure can be installed.
    report(r, a, ConnectionSecurity::Secure);
    a
}

fn ask_manifest(r: &mut Rig, a: SessionId) -> ManifestRequestId {
    let out = r.m.request_install(a).unwrap();
    let id = match out.commands.as_slice() {
        [EngineCommand::FetchManifest {
            session,
            request,
            max_bytes,
        }] => {
            assert_eq!(*session, a);
            assert_eq!(
                *max_bytes as usize, MAX_MANIFEST_BYTES,
                "the adapter must be told the same cap the parser enforces"
            );
            *request
        }
        other => panic!("expected one FetchManifest, got {other:?}"),
    };
    r.run(out);
    id
}

fn answer(
    r: &mut Rig,
    a: SessionId,
    request: ManifestRequestId,
    result: ManifestResult,
) -> Outcome {
    r.event(EngineEvent::ManifestFetched {
        session: a,
        request,
        result,
    })
}

fn found(json: &str) -> ManifestResult {
    ManifestResult::Found {
        manifest_url: u("https://shop.example/manifest.json"),
        bytes: json.as_bytes().to_vec(),
    }
}

fn candidate(out: &Outcome) -> (&InstalledApp, InstallSource) {
    match out.notices.as_slice() {
        [Notice::InstallCandidate { app, source, .. }] => (app, *source),
        other => panic!("expected one InstallCandidate, got {other:?}"),
    }
}

fn set_title(r: &mut Rig, a: SessionId, title: &str) {
    r.m.handle_event(EngineEvent::TitleChanged {
        session: a,
        title: title.into(),
    })
    .unwrap();
}

/// Why: only the manifest gives the real name, scope and icons; the candidate must carry
/// them, but a hostile manifest must still be unable to leave the page's origin or spoof
/// the name.
#[test]
fn a_delivered_manifest_becomes_a_candidate_bound_to_the_page_origin() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    set_title(&mut r, a, "Cart");
    let req = ask_manifest(&mut r, a);
    let out = answer(
        &mut r,
        a,
        req,
        found(
            &r#"{"name":"Shop@BIDI@evil","start_url":"https://evil.test/","scope":"/app/",
                "display":"standalone",
                "icons":[{"src":"/i/192.png","sizes":"192x192"}]}"#
                .replace("@BIDI@", "\u{202e}"),
        ),
    );
    let (app, source) = candidate(&out);
    assert_eq!(source, InstallSource::Manifest);
    assert_eq!(app.name(), "Shopevil", "bidi override stripped");
    assert_eq!(
        app.start_url().origin(),
        u("https://shop.example/").origin(),
        "a manifest cannot move the app to another origin"
    );
    assert_eq!(app.icons().len(), 1);
    assert_eq!(
        app.icons()[0].src().as_str(),
        "https://shop.example/i/192.png"
    );
}

/// Why: a late answer describes a page that is gone. Applying it would offer to install
/// the old page's identity for the new page.
#[test]
fn a_manifest_result_for_a_page_the_session_left_is_discarded() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    let req = ask_manifest(&mut r, a);
    r.event(EngineEvent::NavigationChanged {
        session: a,
        url: u("https://other.example/"),
        phase: NavigationPhase::Started,
        can_go_back: true,
        can_go_forward: false,
    });
    r.commit(a, "https://other.example/");
    let out = answer(&mut r, a, req, found(r#"{"name":"Old page app"}"#));
    assert_eq!(out, Outcome::default(), "stale result must not be applied");
}

#[test]
fn a_manifest_result_for_a_replaced_request_or_one_never_asked_is_discarded() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    let first = ask_manifest(&mut r, a);
    let second = ask_manifest(&mut r, a);
    assert_ne!(first, second, "request ids are never reused");
    assert_eq!(
        answer(&mut r, a, first, found("{}")),
        Outcome::default(),
        "the replaced request is stale"
    );
    assert_eq!(
        answer(&mut r, a, ManifestRequestId(999), found("{}")),
        Outcome::default()
    );
    // The live request still works, once.
    let out = answer(&mut r, a, second, found(r#"{"name":"Shop"}"#));
    assert_eq!(candidate(&out).1, InstallSource::Manifest);
    assert_eq!(
        answer(&mut r, a, second, found(r#"{"name":"Shop"}"#)),
        Outcome::default(),
        "an answer is applied at most once"
    );
}

/// Why: a manifest over the cap is refused unparsed; its content (here a spoofed name)
/// must not influence the candidate at all.
#[test]
fn an_oversize_manifest_is_refused_and_installs_the_plain_page_instead() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    set_title(&mut r, a, "Cart");
    let req = ask_manifest(&mut r, a);
    let mut big = br#"{"name":"EVIL"}"#.to_vec();
    big.resize(MAX_MANIFEST_BYTES + 1, b' ');
    let out = answer(
        &mut r,
        a,
        req,
        ManifestResult::Found {
            manifest_url: u("https://shop.example/manifest.json"),
            bytes: big,
        },
    );
    let (app, source) = candidate(&out);
    assert_eq!(source, InstallSource::Manual(ManualReason::TooLarge));
    assert_eq!(app.name(), "Cart");

    let mut at_the_cap = br#"{"name":"Fits"}"#.to_vec();
    at_the_cap.resize(MAX_MANIFEST_BYTES, b' ');
    let req = ask_manifest(&mut r, a);
    let out = answer(
        &mut r,
        a,
        req,
        ManifestResult::Found {
            manifest_url: u("https://shop.example/manifest.json"),
            bytes: at_the_cap,
        },
    );
    assert_eq!(
        candidate(&out).1,
        InstallSource::Manifest,
        "the cap is inclusive"
    );
}

#[test]
fn a_page_without_a_manifest_falls_back_to_the_manual_install() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    set_title(&mut r, a, "Cart");
    let req = ask_manifest(&mut r, a);
    let out = answer(&mut r, a, req, ManifestResult::NotFound);
    let (app, source) = candidate(&out);
    assert_eq!(source, InstallSource::Manual(ManualReason::NoManifest));
    let expected =
        InstalledApp::manual(&u("https://shop.example/cart?token=secret"), Some("Cart")).unwrap();
    assert_eq!(app, &expected);

    let req = ask_manifest(&mut r, a);
    let out = answer(
        &mut r,
        a,
        req,
        ManifestResult::Failed(ManifestFailure::Network),
    );
    assert_eq!(
        candidate(&out).1,
        InstallSource::Manual(ManualReason::FetchFailed)
    );
    let req = ask_manifest(&mut r, a);
    let out = answer(
        &mut r,
        a,
        req,
        ManifestResult::Failed(ManifestFailure::TooLarge),
    );
    assert_eq!(
        candidate(&out).1,
        InstallSource::Manual(ManualReason::TooLarge)
    );
}

/// Why: garbage must never become an installed app; the fallback is the plain page and the
/// garbage's content is ignored.
#[test]
fn an_invalid_manifest_installs_nothing_from_it() {
    let cases: Vec<(&str, ManifestResult)> = vec![
        ("not json", found("{\"name\":")),
        ("array", found(r#"["name","EVIL"]"#)),
        (
            "not utf-8",
            ManifestResult::Found {
                manifest_url: u("https://shop.example/manifest.json"),
                bytes: vec![0xff, 0xfe, b'{', b'}'],
            },
        ),
        (
            "plain-http manifest for an https page",
            ManifestResult::Found {
                manifest_url: u("http://shop.example/manifest.json"),
                bytes: br#"{"name":"EVIL"}"#.to_vec(),
            },
        ),
        (
            "non-web manifest url",
            ManifestResult::Found {
                manifest_url: u("data:application/json,{}"),
                bytes: br#"{"name":"EVIL"}"#.to_vec(),
            },
        ),
    ];
    for (what, result) in cases {
        let mut r = Rig::new(2);
        let a = shop(&mut r);
        let req = ask_manifest(&mut r, a);
        let out = answer(&mut r, a, req, result);
        let (app, source) = candidate(&out);
        assert_eq!(
            source,
            InstallSource::Manual(ManualReason::Invalid),
            "{what}"
        );
        assert_ne!(app.name(), "EVIL", "{what}");
    }
}

#[test]
fn install_needs_a_live_page_the_core_can_name() {
    let mut r = Rig::new(2);
    let a = r.session();
    assert!(
        matches!(
            r.m.request_install(a),
            Err(CoreError::NotAllowedInState { .. })
        ),
        "a session without a view has nothing to ask"
    );
    assert_eq!(
        r.m.request_install(SessionId::new(77)),
        Err(CoreError::UnknownSession(SessionId::new(77)))
    );
}

fn refusal(out: &Outcome) -> InstallRefusal {
    assert!(out.commands.is_empty(), "the engine must not be asked");
    match out.notices.as_slice() {
        [Notice::InstallRefused { reason, .. }] => *reason,
        other => panic!("expected one InstallRefused, got {other:?}"),
    }
}

/// Why: owner decision, installed apps are https-only. A plain-http page is refused before
/// the engine is asked for a manifest, so no install dialog can follow, whatever the engine
/// reports about it (even a bogus "secure").
#[test]
fn a_plain_http_page_cannot_be_installed_and_the_engine_is_never_asked() {
    for security in [
        None,
        Some(ConnectionSecurity::Insecure),
        Some(ConnectionSecurity::Secure),
    ] {
        let mut r = Rig::new(2);
        let a = r.session();
        r.fg(a);
        r.commit(a, "http://plain.test/page");
        if let Some(security) = security {
            report(&mut r, a, security);
        }
        let out = r.m.request_install(a).unwrap();
        assert_eq!(refusal(&out), InstallRefusal::NotHttps, "{security:?}");
        // Nothing was left pending that a stray answer could complete.
        let late = answer(&mut r, a, ManifestRequestId(1), ManifestResult::NotFound);
        assert_eq!(late, Outcome::default());
    }
}

/// Why: an https page with a broken certificate is not a place to create an app that is
/// later trusted by its URL alone.
#[test]
fn an_https_page_with_a_certificate_error_cannot_be_installed() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    r.commit(a, "https://shop.example/");
    report(&mut r, a, ConnectionSecurity::CertificateError);
    let out = r.m.request_install(a).unwrap();
    assert_eq!(refusal(&out), InstallRefusal::ConnectionNotSecure);
}

/// Why: "not verified" is no evidence of safety. The safe choice is to refuse until the
/// engine has confirmed the connection.
#[test]
fn an_https_page_whose_connection_is_not_verified_cannot_be_installed() {
    let mut r = Rig::new(2);
    let a = r.session();
    r.fg(a);
    r.commit(a, "https://shop.example/");
    assert_eq!(security(&r, a), None);
    let out = r.m.request_install(a).unwrap();
    assert_eq!(refusal(&out), InstallRefusal::ConnectionNotSecure);
    report(&mut r, a, ConnectionSecurity::Secure);
    assert!(
        !r.m.request_install(a).unwrap().commands.is_empty(),
        "confirmed secure: the manifest is asked for"
    );
}

/// Why: the connection can turn bad between the request and the answer; the candidate
/// must not be offered on the strength of the earlier state.
#[test]
fn a_connection_that_degrades_before_the_manifest_arrives_gets_no_candidate() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    let req = ask_manifest(&mut r, a);
    report(&mut r, a, ConnectionSecurity::CertificateError);
    let out = answer(&mut r, a, req, found(r#"{"name":"Shop"}"#));
    assert_eq!(refusal(&out), InstallRefusal::ConnectionNotSecure);
}

// ------------------------------------------------------------ external open

fn pending(r: &Rig) -> Option<crate::PendingExternalOpen> {
    r.m.pending_external_open()
}

fn external(r: &mut Rig, a: SessionId, url: &str, gesture: bool) -> Outcome {
    r.event(EngineEvent::ExternalSchemeRequested {
        session: a,
        url: u(url),
        user_gesture: gesture,
    })
}

/// Why: the page that asked is named in the prompt, and the URL the OS eventually gets is
/// the one the core kept, not something the UI hands back.
#[test]
fn a_gestured_external_request_waits_for_the_user_and_names_the_asking_page() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    let out = external(&mut r, a, "tel:+4312345", true);
    assert!(
        out.commands.is_empty(),
        "nothing reaches the OS before the user answers"
    );
    let p = pending(&r).unwrap();
    assert_eq!(p.session, a);
    assert_eq!(p.url, u("tel:+4312345"));
    assert_eq!(p.origin, Some(u("https://shop.example/cart?token=secret")));

    let out = r.m.respond_external_open(a, true).unwrap();
    assert_eq!(
        out.commands,
        vec![EngineCommand::OpenExternal {
            session: a,
            url: u("tel:+4312345")
        }]
    );
    assert!(pending(&r).is_none());
    assert_eq!(
        r.m.respond_external_open(a, true),
        Err(CoreError::NoPendingExternalOpen(a)),
        "one confirmation opens at most one thing"
    );
}

/// Why: Cancel is the safe default and must leave no trace that could open later.
#[test]
fn cancel_opens_nothing_and_clears_the_prompt() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    external(&mut r, a, "mailto:a@example.com", true);
    let out = r.m.respond_external_open(a, false).unwrap();
    assert_eq!(out, Outcome::default());
    assert!(pending(&r).is_none());
    assert_eq!(
        r.m.respond_external_open(a, true),
        Err(CoreError::NoPendingExternalOpen(a))
    );
}

/// Why: a page that fires `tel:` on its own (no tap) is a prompt storm or a dialer trap.
#[test]
fn an_external_request_without_a_user_gesture_is_dropped_silently() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    let out = external(&mut r, a, "tel:+4312345", false);
    assert_eq!(out, Outcome::default());
    assert!(pending(&r).is_none());
}

#[test]
fn an_external_request_from_a_background_session_is_dropped() {
    let mut r = Rig::new(3);
    let a = shop(&mut r);
    let b = r.session();
    r.fg(b);
    assert_eq!(r.m.foreground_session(), Some(b));
    let out = external(&mut r, a, "tel:+4312345", true);
    assert_eq!(out, Outcome::default());
    assert!(pending(&r).is_none(), "a hidden page cannot raise a prompt");
}

/// Why: at most one prompt per session. The first stays: replacing it would let a page
/// swap the target while the user reads the dialog.
#[test]
fn a_second_request_while_one_is_pending_is_dropped_and_the_first_stays() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    external(&mut r, a, "tel:+4312345", true);
    let out = external(&mut r, a, "tel:+9999999", true);
    assert_eq!(out, Outcome::default());
    assert_eq!(pending(&r).unwrap().url, u("tel:+4312345"));
    let out = r.m.respond_external_open(a, true).unwrap();
    assert_eq!(
        out.commands,
        vec![EngineCommand::OpenExternal {
            session: a,
            url: u("tel:+4312345")
        }]
    );
}

/// Why: these schemes must never even reach a dialog; `view-source` was not on the core's
/// old private list and is the regression this guards.
#[test]
fn blocked_schemes_never_become_a_prompt() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    for bad in [
        "javascript:alert(1)",
        "data:text/html,hi",
        "file:///etc/passwd",
        "vbscript:x",
        "view-source:https://a.test/",
        "blob:https://a.test/x",
        "ftp://a.test/",
        "https://a.test/",
    ] {
        assert_eq!(
            r.m.handle_event(EngineEvent::ExternalSchemeRequested {
                session: a,
                url: u(bad),
                user_gesture: true,
            }),
            Err(CoreError::UnsafeExternalScheme),
            "{bad}"
        );
        assert!(pending(&r).is_none(), "{bad}");
    }
}

/// Why: the prompt names the page that asked; once that page is gone the prompt is wrong.
#[test]
fn a_prompt_does_not_outlive_its_page_or_its_visibility() {
    let mut r = Rig::new(3);
    let a = shop(&mut r);
    external(&mut r, a, "tel:+4312345", true);
    r.commit(a, "https://other.example/");
    assert!(pending(&r).is_none(), "new document, old prompt gone");

    external(&mut r, a, "tel:+4312345", true);
    let b = r.session();
    r.fg(b);
    assert!(pending(&r).is_none(), "sent to the background, prompt gone");
    assert_eq!(
        r.m.respond_external_open(a, true),
        Err(CoreError::NoPendingExternalOpen(a))
    );
}

#[test]
fn a_popup_to_an_external_scheme_follows_the_same_rules() {
    let mut r = Rig::new(2);
    let a = shop(&mut r);
    let popup = |gesture| NavigationRequest {
        url: u("sms:+4312345"),
        target: NavigationTarget::NewWindow,
        kind: NavigationKind::LinkActivated,
        user_gesture: gesture,
    };
    let out = r.event(EngineEvent::NewWindowRequested {
        session: a,
        request: popup(false),
    });
    assert_eq!(out, Outcome::default());
    assert!(pending(&r).is_none());
    r.event(EngineEvent::NewWindowRequested {
        session: a,
        request: popup(true),
    });
    assert_eq!(pending(&r).unwrap().url, u("sms:+4312345"));
}
