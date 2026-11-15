//! The session manager: a deterministic state machine. User intents and engine events go
//! in, engine commands and UI notices come out. It never talks to a platform, never
//! blocks and never spawns anything.
//!
//! Invariants: at most one foreground session; sessions with a live web view never exceed
//! the budget after any call returns; the foreground session is never evicted.
//!
//! Logging: only session ids and state names are logged, never URLs or titles.

use std::collections::{BTreeMap, BTreeSet};

use krypa_apps::{InstalledApp, Manifest, MAX_MANIFEST_BYTES};
use krypa_engine_api::{
    ConnectionSecurity, EngineCapabilities, EngineCommand, EngineEvent, LoadErrorKind,
    ManifestFailure, ManifestRequestId, ManifestResult, NavigationPhase, NavigationRequest,
    PermissionDecision, PermissionKind, PermissionRequestId, ProfileKind, SessionId,
};
use krypa_navigation::is_blocked_scheme;
use serde::Serialize;
use url::Url;

use crate::error::CoreError;
use crate::lifecycle::{SessionState, Trigger};
use crate::policy::{
    decide_navigation, is_in_scope, is_loadable, NavigationDecision, PolicyContext,
};

const MAX_TITLE_CHARS: usize = 256;

/// Manager settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagerConfig {
    max_live_views: usize,
}

impl ManagerConfig {
    /// `max_live_views` is the most web views that may exist at once.
    ///
    /// # Errors
    /// [`CoreError::InvalidBudget`] if it is zero.
    pub const fn new(max_live_views: usize) -> Result<Self, CoreError> {
        if max_live_views == 0 {
            return Err(CoreError::InvalidBudget);
        }
        Ok(Self { max_live_views })
    }

    /// The live-view budget.
    #[must_use]
    pub const fn max_live_views(&self) -> usize {
        self.max_live_views
    }
}

/// What to create a session from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    /// Data store kind.
    pub profile: ProfileKind,
    /// First URL to load when the view is created.
    pub url: Option<Url>,
    /// Scope of the installed app this session belongs to, if any.
    pub app_scope: Option<Url>,
}

/// Operating-system memory pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPressure {
    /// Shrink to half the budget by evicting the least recently foregrounded views.
    Warning,
    /// Evict every background view.
    Critical,
}

/// Why a session is in `Failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FailureReason {
    /// The web content process died.
    ContentProcessTerminated,
    /// A load failed.
    LoadFailed(LoadErrorKind),
}

/// Why an install is offered as a plain shortcut to the page instead of from a manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualReason {
    /// The page declares no manifest.
    NoManifest,
    /// The engine could not deliver the manifest.
    FetchFailed,
    /// The manifest was longer than the size cap.
    TooLarge,
    /// The manifest was not usable (not UTF-8 or JSON, not an object, unusable URL).
    Invalid,
}

/// Where the data of an install candidate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallSource {
    /// A manifest the engine delivered and the parser accepted.
    Manifest,
    /// The page itself (its URL and title); see [`ManualReason`].
    Manual(ManualReason),
}

/// A prompt to open another app that waits for the user. Only the foreground session can
/// have one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingExternalOpen {
    /// Session whose page asked.
    pub session: SessionId,
    /// What it wants opened (sensitive; do not log).
    pub url: Url,
    /// The page that asked, as the session shows it now (sensitive; do not log).
    pub origin: Option<Url>,
}

/// Things the UI should know or ask about; not engine commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A new session was opened from a page (new window or out-of-scope link).
    SessionOpened {
        /// The new session (already foreground).
        session: SessionId,
        /// The session that asked for it.
        opener: SessionId,
    },
    /// A background view was destroyed to save memory; the session is `Terminated`.
    ViewEvicted {
        /// Session concerned.
        session: SessionId,
    },
    /// The platform destroyed a view on its own; the session is `Terminated`.
    ViewLost {
        /// Session concerned.
        session: SessionId,
    },
    /// A private session lost its view and was removed: it cannot be restored.
    PrivateSessionDiscarded {
        /// Session concerned.
        session: SessionId,
    },
    /// A session entered `Failed`; show an error view and offer reload.
    SessionFailed {
        /// Session concerned.
        session: SessionId,
        /// Why.
        reason: FailureReason,
    },
    /// The app that "Add to Krypa" would install, built from the page's manifest or, if
    /// that is missing or unusable, from the page itself. Show it and ask; nothing is saved
    /// until the user confirms.
    InstallCandidate {
        /// The page it was made for.
        session: SessionId,
        /// The validated app (name sanitized, URLs bound to the page's origin).
        app: Box<InstalledApp>,
        /// Where it came from.
        source: InstallSource,
    },
    /// A page wants to open another app (mailto, tel, app link); ask the user first. At
    /// most one is pending per session, see [`SessionManager::pending_external_open`].
    ExternalOpenRequested {
        /// Source session.
        session: SessionId,
        /// Target URL (sensitive; do not log).
        url: Url,
    },
    /// A page asks for a permission; answer with `respond_permission`.
    PermissionRequested {
        /// Source session.
        session: SessionId,
        /// Prompt id.
        request: PermissionRequestId,
        /// What is requested.
        kind: PermissionKind,
        /// Requesting origin.
        origin: Url,
    },
}

/// Result of a manager call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Commands for the platform adapter, in order.
    pub commands: Vec<EngineCommand>,
    /// Notices for the UI.
    pub notices: Vec<Notice>,
}

impl Outcome {
    fn merge(&mut self, other: Self) {
        self.commands.extend(other.commands);
        self.notices.extend(other.notices);
    }
}

/// Read-only copy of a session for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionInfo {
    /// Session id.
    pub id: SessionId,
    /// Data store kind.
    pub profile: ProfileKind,
    /// Lifecycle state.
    pub state: SessionState,
    /// Last committed (or initial) URL; sensitive.
    pub current_url: Option<Url>,
    /// URL to restore the session from after its view was destroyed. Always `None` for
    /// private sessions.
    pub restorable_url: Option<Url>,
    /// Page title (hostile text, sanitized and length-capped).
    pub title: Option<String>,
    /// Page icon URL.
    pub icon_url: Option<Url>,
    /// Load progress in per mille.
    pub progress_permille: u16,
    /// Back is possible.
    pub can_go_back: bool,
    /// Forward is possible.
    pub can_go_forward: bool,
    /// Set while the state is `Failed`.
    pub failure: Option<FailureReason>,
    /// What the engine confirmed about the connection of the page shown now. `None` means
    /// unverified (navigation under way, nothing reported, no live view): never show it as
    /// secure. Never more trusting than the last report for the committed page; `Secure` is
    /// also downgraded to `Insecure` if the URL is not `https`.
    pub security: Option<ConnectionSecurity>,
}

#[derive(Debug)]
struct Session {
    profile: ProfileKind,
    app_scope: Option<Url>,
    state: SessionState,
    url: Option<Url>,
    title: Option<String>,
    icon_url: Option<Url>,
    progress_permille: u16,
    can_go_back: bool,
    can_go_forward: bool,
    failure: Option<FailureReason>,
    security: Option<ConnectionSecurity>,
    /// A navigation started and has not committed or failed yet.
    provisional: bool,
    /// Security reported while `provisional`: it is about the page being loaded, and only
    /// becomes `security` when that page commits.
    pending_security: Option<ConnectionSecurity>,
    /// Logical clock value of the last time this became foreground; 0 = never.
    last_foregrounded: u64,
    pending_permissions: BTreeSet<PermissionRequestId>,
    /// "Add to Krypa" waiting for the engine's manifest answer.
    pending_manifest: Option<PendingManifest>,
    /// The one external-open prompt waiting for the user.
    pending_external: Option<Url>,
}

/// A manifest request in flight, bound to the document it was made for.
#[derive(Debug)]
struct PendingManifest {
    request: ManifestRequestId,
    document_url: Url,
}

impl Session {
    fn is_private(&self) -> bool {
        self.profile == ProfileKind::Private
    }

    /// Back to "unverified": nothing is confirmed about whatever is shown next.
    fn reset_security(&mut self) {
        self.security = None;
        self.provisional = false;
        self.pending_security = None;
        self.document_changed();
    }

    /// The document the session shows is going away or gone. Whatever was asked about it
    /// (a manifest, an external open) must not be applied to the next one.
    fn document_changed(&mut self) {
        self.pending_manifest = None;
        self.pending_external = None;
    }
}

/// Owns all sessions and decides which web views stay alive.
#[derive(Debug)]
pub struct SessionManager {
    config: ManagerConfig,
    capabilities: EngineCapabilities,
    sessions: BTreeMap<SessionId, Session>,
    next_id: u64,
    foreground: Option<SessionId>,
    clock: u64,
    /// `DestroySession` commands issued whose `SessionDestroyed` answer is still due.
    pending_destroy_acks: BTreeMap<SessionId, u32>,
    /// Last manifest request id issued; ids are never reused.
    last_manifest_request: u64,
}

impl SessionManager {
    /// New manager. `capabilities` is what the platform reported at runtime.
    #[must_use]
    pub fn new(config: ManagerConfig, capabilities: EngineCapabilities) -> Self {
        Self {
            config,
            capabilities,
            sessions: BTreeMap::new(),
            next_id: 1,
            foreground: None,
            clock: 0,
            pending_destroy_acks: BTreeMap::new(),
            last_manifest_request: 0,
        }
    }

    // ---------------------------------------------------------------- queries

    /// The capabilities the manager was built with.
    #[must_use]
    pub const fn capabilities(&self) -> &EngineCapabilities {
        &self.capabilities
    }

    /// The foreground session, if any.
    #[must_use]
    pub const fn foreground_session(&self) -> Option<SessionId> {
        self.foreground
    }

    /// Number of sessions that currently own a web view.
    #[must_use]
    pub fn live_view_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|s| s.state.has_live_view())
            .count()
    }

    /// Snapshot of one session.
    ///
    /// # Errors
    /// Unknown or destroyed session.
    pub fn info(&self, id: SessionId) -> Result<SessionInfo, CoreError> {
        let s = self.session(id)?;
        Ok(SessionInfo {
            id,
            profile: s.profile.clone(),
            state: s.state,
            current_url: s.url.clone(),
            restorable_url: if s.is_private() { None } else { s.url.clone() },
            title: s.title.clone(),
            icon_url: s.icon_url.clone(),
            progress_permille: s.progress_permille,
            can_go_back: s.can_go_back,
            can_go_forward: s.can_go_forward,
            failure: s.failure,
            security: match (s.security, &s.url) {
                (Some(ConnectionSecurity::Secure), Some(u)) if u.scheme() != "https" => {
                    Some(ConnectionSecurity::Insecure)
                }
                (Some(ConnectionSecurity::Secure), None) => Some(ConnectionSecurity::Insecure),
                (other, _) => other,
            },
        })
    }

    /// Snapshots of all sessions, ordered by id.
    #[must_use]
    pub fn sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .keys()
            .filter_map(|id| self.info(*id).ok())
            .collect()
    }

    /// The external-open prompt that waits for the user, if any. Only the foreground
    /// session can have one, so there is at most one at a time.
    #[must_use]
    pub fn pending_external_open(&self) -> Option<PendingExternalOpen> {
        self.sessions.iter().find_map(|(id, s)| {
            s.pending_external.as_ref().map(|url| PendingExternalOpen {
                session: *id,
                url: url.clone(),
                origin: s.url.clone(),
            })
        })
    }

    /// The synchronous answer for a WKWebView policy callback. Pure: changes nothing.
    ///
    /// # Errors
    /// Unknown or destroyed session.
    pub fn navigation_policy(
        &self,
        id: SessionId,
        request: &NavigationRequest,
    ) -> Result<NavigationDecision, CoreError> {
        let s = self.session(id)?;
        let ctx = PolicyContext {
            app_scope: s.app_scope.as_ref(),
        };
        Ok(decide_navigation(request, &ctx))
    }

    // ---------------------------------------------------------------- intents

    /// Registers a session without creating a web view (`NotRunning`).
    ///
    /// # Errors
    /// Missing platform capability or a URL that is not loadable.
    pub fn create_session(&mut self, spec: SessionSpec) -> Result<SessionId, CoreError> {
        match &spec.profile {
            ProfileKind::Persistent(_) if !self.capabilities.persistent_profiles => {
                return Err(CoreError::CapabilityMissing("persistent_profiles"));
            }
            ProfileKind::Private if !self.capabilities.private_sessions => {
                return Err(CoreError::CapabilityMissing("private_sessions"));
            }
            _ => {}
        }
        if spec
            .url
            .iter()
            .chain(spec.app_scope.iter())
            .any(|u| !is_loadable(u))
        {
            return Err(CoreError::UrlNotLoadable);
        }
        let id = SessionId::new(self.next_id);
        self.next_id += 1;
        self.sessions.insert(
            id,
            Session {
                profile: spec.profile,
                app_scope: spec.app_scope,
                state: SessionState::NotRunning,
                url: spec.url,
                title: None,
                icon_url: None,
                progress_permille: 0,
                can_go_back: false,
                can_go_forward: false,
                failure: None,
                security: None,
                provisional: false,
                pending_security: None,
                last_foregrounded: 0,
                pending_permissions: BTreeSet::new(),
                pending_manifest: None,
                pending_external: None,
            },
        );
        Ok(id)
    }

    /// Opens `url` in a new foreground session next to `opener` (same profile; keeps the
    /// app scope only if `url` is inside it). Callable synchronously from a delegate that
    /// must hand a web view back at once: apply the returned commands in order first.
    ///
    /// # Errors
    /// Unknown opener or a URL that is not loadable.
    pub fn open_new_session(
        &mut self,
        opener: SessionId,
        url: &Url,
    ) -> Result<(SessionId, Outcome), CoreError> {
        let o = self.session(opener)?;
        let spec = SessionSpec {
            profile: o.profile.clone(),
            url: Some(url.clone()),
            app_scope: o.app_scope.clone().filter(|scope| is_in_scope(scope, url)),
        };
        let id = self.create_session(spec)?;
        let mut out = self.foreground(id)?;
        out.notices.push(Notice::SessionOpened {
            session: id,
            opener,
        });
        Ok((id, out))
    }

    /// Makes `id` the foreground session: demotes the previous one, creates the view if it
    /// has none (evicting least recently foregrounded views to respect the budget), and
    /// reloads a `Failed` session. A no-op if it already is foreground.
    ///
    /// # Errors
    /// Unknown or destroyed session.
    pub fn foreground(&mut self, id: SessionId) -> Result<Outcome, CoreError> {
        let state = self.session(id)?.state;
        let mut out = Outcome::default();
        if state == SessionState::Foreground || self.foreground == Some(id) {
            return Ok(out);
        }
        if let Some(prev) = self.foreground.take() {
            if self.session(prev)?.state == SessionState::Foreground {
                self.transition(prev, Trigger::SentToBackground)?;
            }
            // Nobody is looking at that page's prompt any more.
            self.session_mut(prev)?.pending_external = None;
            out.commands.push(EngineCommand::Hide { session: prev });
        }
        self.clock += 1;
        let clock = self.clock;
        self.session_mut(id)?.last_foregrounded = clock;
        self.foreground = Some(id);

        match state {
            SessionState::NotRunning | SessionState::Terminated => {
                self.evict_down_to(self.config.max_live_views - 1, &mut out)?;
                self.transition(id, Trigger::ViewCreated)?;
                let s = self.session_mut(id)?;
                s.failure = None;
                s.progress_permille = 0;
                s.reset_security();
                out.commands.push(EngineCommand::CreateSession {
                    session: id,
                    profile: s.profile.clone(),
                    initial_url: s.url.clone(),
                });
                out.commands.push(EngineCommand::Show { session: id });
            }
            SessionState::Background | SessionState::Suspended => {
                self.transition(id, Trigger::SentToForeground)?;
                if state == SessionState::Suspended && self.capabilities.view_suspend {
                    out.commands.push(EngineCommand::Resume { session: id });
                }
                out.commands.push(EngineCommand::Show { session: id });
            }
            SessionState::Loading => out.commands.push(EngineCommand::Show { session: id }),
            SessionState::Failed => {
                // Coming back to a failed session reloads it, visibly (state Loading).
                out.commands.push(EngineCommand::Show { session: id });
                self.begin_navigation(id, "foreground")?;
                out.commands.push(EngineCommand::Reload { session: id });
            }
            SessionState::Foreground => {}
        }
        Ok(out)
    }

    /// Loads a URL (explicit user action; also recovers a `Failed` session).
    ///
    /// # Errors
    /// Unknown session, URL not loadable, or a state without a live view.
    pub fn navigate(&mut self, id: SessionId, url: Url) -> Result<Outcome, CoreError> {
        if !is_loadable(&url) {
            return Err(CoreError::UrlNotLoadable);
        }
        self.begin_navigation(id, "navigate")?;
        Ok(Self::one(EngineCommand::Load { session: id, url }))
    }

    /// Reloads the page (explicit user action; also recovers a `Failed` session).
    ///
    /// # Errors
    /// Unknown session or a state without a live view.
    pub fn reload(&mut self, id: SessionId) -> Result<Outcome, CoreError> {
        self.begin_navigation(id, "reload")?;
        Ok(Self::one(EngineCommand::Reload { session: id }))
    }

    /// Stops loading.
    ///
    /// # Errors
    /// Unknown session or a state without a live view.
    pub fn stop(&mut self, id: SessionId) -> Result<Outcome, CoreError> {
        self.require_navigable(id, "stop")?;
        Ok(Self::one(EngineCommand::Stop { session: id }))
    }

    /// Goes back in history.
    ///
    /// # Errors
    /// Unknown session or a state without a live view.
    pub fn go_back(&mut self, id: SessionId) -> Result<Outcome, CoreError> {
        self.begin_navigation(id, "go_back")?;
        Ok(Self::one(EngineCommand::GoBack { session: id }))
    }

    /// Goes forward in history.
    ///
    /// # Errors
    /// Unknown session or a state without a live view.
    pub fn go_forward(&mut self, id: SessionId) -> Result<Outcome, CoreError> {
        self.begin_navigation(id, "go_forward")?;
        Ok(Self::one(EngineCommand::GoForward { session: id }))
    }

    /// `Background` -> `Suspended`. The platform is told to pause only if
    /// `EngineCapabilities::view_suspend` is set; the state change happens either way and
    /// means nothing more than "alive, off screen, told to pause if possible".
    ///
    /// # Errors
    /// Unknown session or illegal transition.
    pub fn suspend(&mut self, id: SessionId) -> Result<Outcome, CoreError> {
        self.transition(id, Trigger::Suspend)?;
        Ok(self.suspend_resume_outcome(EngineCommand::Suspend { session: id }))
    }

    /// `Suspended` -> `Background`.
    ///
    /// # Errors
    /// Unknown session or illegal transition.
    pub fn resume(&mut self, id: SessionId) -> Result<Outcome, CoreError> {
        self.transition(id, Trigger::Resume)?;
        Ok(self.suspend_resume_outcome(EngineCommand::Resume { session: id }))
    }

    /// Closes a session for good, in any state. Nothing of it is kept.
    ///
    /// # Errors
    /// Unknown or already destroyed session.
    pub fn close_session(&mut self, id: SessionId) -> Result<Outcome, CoreError> {
        let had_view = self.session(id)?.state.has_live_view();
        self.sessions.remove(&id);
        if self.foreground == Some(id) {
            self.foreground = None;
        }
        let mut out = Outcome::default();
        if had_view {
            self.expect_destroy_ack(id);
            out.commands
                .push(EngineCommand::DestroySession { session: id });
        }
        Ok(out)
    }

    /// Answers a pending permission prompt.
    ///
    /// # Errors
    /// Unknown session or a prompt that is not pending.
    pub fn respond_permission(
        &mut self,
        id: SessionId,
        request: PermissionRequestId,
        decision: PermissionDecision,
    ) -> Result<Outcome, CoreError> {
        if !self.session_mut(id)?.pending_permissions.remove(&request) {
            return Err(CoreError::UnknownPermissionRequest {
                session: id,
                request,
            });
        }
        Ok(Self::one(EngineCommand::RespondToPermission {
            session: id,
            request,
            decision,
        }))
    }

    /// Starts "Add to Krypa" for the page the session shows: asks the engine for that page's
    /// manifest. The answer arrives as `EngineEvent::ManifestFetched` and becomes a
    /// [`Notice::InstallCandidate`]. A second request replaces the first; the answer to the
    /// replaced one is discarded.
    ///
    /// # Errors
    /// Unknown session, a state without a live view, or a page that is not an `http(s)`
    /// page (nothing committed yet, or an error page).
    pub fn request_install(&mut self, id: SessionId) -> Result<Outcome, CoreError> {
        let s = self.session(id)?;
        if !s.state.has_live_view() {
            return Err(CoreError::NotAllowedInState {
                session: id,
                state: s.state,
                what: "install",
            });
        }
        let document_url = s
            .url
            .clone()
            .filter(is_loadable)
            .ok_or(CoreError::UrlNotLoadable)?;
        self.last_manifest_request += 1;
        let request = ManifestRequestId(self.last_manifest_request);
        self.session_mut(id)?.pending_manifest = Some(PendingManifest {
            request,
            document_url,
        });
        Ok(Self::one(EngineCommand::FetchManifest {
            session: id,
            request,
            max_bytes: u32::try_from(MAX_MANIFEST_BYTES).unwrap_or(u32::MAX),
        }))
    }

    /// Answers the external-open prompt. `allow` hands the URL the core itself kept (never
    /// one supplied by the caller) to the platform; otherwise nothing happens.
    ///
    /// # Errors
    /// Unknown session or no prompt pending for it.
    pub fn respond_external_open(
        &mut self,
        id: SessionId,
        allow: bool,
    ) -> Result<Outcome, CoreError> {
        let Some(url) = self.session_mut(id)?.pending_external.take() else {
            return Err(CoreError::NoPendingExternalOpen(id));
        };
        if !allow {
            return Ok(Outcome::default());
        }
        Ok(Self::one(EngineCommand::OpenExternal { session: id, url }))
    }

    /// Reacts to OS memory pressure by evicting background views, least recently
    /// foregrounded first. The foreground session is never touched.
    ///
    /// # Errors
    /// Only on a broken internal invariant.
    pub fn on_memory_pressure(&mut self, level: MemoryPressure) -> Result<Outcome, CoreError> {
        let limit = match level {
            MemoryPressure::Warning => self.config.max_live_views.div_ceil(2),
            MemoryPressure::Critical => 0,
        };
        let mut out = Outcome::default();
        self.evict_down_to(limit, &mut out)?;
        Ok(out)
    }

    // ----------------------------------------------------------------- events

    /// Feeds one platform event in.
    ///
    /// Events for sessions that never existed or are already destroyed are rejected, as
    /// are events that make no sense in the session's state. The only event for a
    /// destroyed session that is accepted is the single `SessionDestroyed` answer to each
    /// `DestroySession` command.
    ///
    /// # Errors
    /// See [`CoreError`].
    pub fn handle_event(&mut self, event: EngineEvent) -> Result<Outcome, CoreError> {
        let id = event.session();
        if matches!(event, EngineEvent::SessionDestroyed { .. }) && self.take_destroy_ack(id) {
            return Ok(Outcome::default());
        }
        let state = self.session(id)?.state;
        if !state.has_live_view() {
            return Err(CoreError::NotAllowedInState {
                session: id,
                state,
                what: event_name(&event),
            });
        }
        match event {
            EngineEvent::NavigationChanged {
                url,
                phase,
                can_go_back,
                can_go_forward,
                ..
            } => self.on_navigation(id, url, phase, can_go_back, can_go_forward),
            EngineEvent::ProgressChanged { permille, .. } => {
                if permille > 1000 {
                    return Err(CoreError::InvalidProgress(permille));
                }
                self.session_mut(id)?.progress_permille = permille;
                Ok(Outcome::default())
            }
            EngineEvent::TitleChanged { title, .. } => {
                self.session_mut(id)?.title = sanitize_title(&title);
                Ok(Outcome::default())
            }
            EngineEvent::SecurityChanged { security, .. } => {
                let s = self.session_mut(id)?;
                if s.state == SessionState::Failed {
                    // The load already failed: a late report must not soften that.
                    return Ok(Outcome::default());
                }
                if s.provisional {
                    s.pending_security = Some(security);
                } else {
                    s.security = Some(security);
                }
                Ok(Outcome::default())
            }
            EngineEvent::IconChanged { icon_url, .. } => {
                self.session_mut(id)?.icon_url = icon_url;
                Ok(Outcome::default())
            }
            EngineEvent::NewWindowRequested { request, .. } => {
                match self.navigation_policy(id, &request)? {
                    NavigationDecision::OpenInNewSession => {
                        Ok(self.open_new_session(id, &request.url)?.1)
                    }
                    NavigationDecision::OpenExternally => {
                        self.request_external_open(id, request.url, request.user_gesture)
                    }
                    // The adapter has no window to refuse or allow here: nothing to do.
                    NavigationDecision::Allow | NavigationDecision::Cancel(_) => {
                        Ok(Outcome::default())
                    }
                }
            }
            EngineEvent::ExternalSchemeRequested {
                url, user_gesture, ..
            } => self.request_external_open(id, url, user_gesture),
            EngineEvent::ManifestFetched {
                request, result, ..
            } => Ok(self.on_manifest(id, request, result)),
            EngineEvent::PermissionRequested {
                request,
                kind,
                origin,
                ..
            } => {
                if !self.session_mut(id)?.pending_permissions.insert(request) {
                    return Err(CoreError::DuplicatePermissionRequest {
                        session: id,
                        request,
                    });
                }
                Ok(Outcome {
                    commands: Vec::new(),
                    notices: vec![Notice::PermissionRequested {
                        session: id,
                        request,
                        kind,
                        origin,
                    }],
                })
            }
            EngineEvent::LoadFailed { kind, .. } => {
                if kind == LoadErrorKind::Cancelled {
                    // A superseded load is not a failure.
                    return Ok(Outcome::default());
                }
                let s = self.session_mut(id)?;
                s.reset_security();
                if kind == LoadErrorKind::Tls {
                    s.security = Some(ConnectionSecurity::CertificateError);
                }
                self.fail(id, Trigger::LoadFailed, FailureReason::LoadFailed(kind))
            }
            EngineEvent::ContentProcessTerminated { .. } => {
                let s = self.session_mut(id)?;
                s.pending_permissions.clear();
                s.reset_security();
                self.fail(
                    id,
                    Trigger::ProcessCrashed,
                    FailureReason::ContentProcessTerminated,
                )
            }
            EngineEvent::SessionDestroyed { .. } => self.on_view_lost(id),
        }
    }

    // ---------------------------------------------------------------- helpers

    /// A page wants something opened by another app. Never opens anything itself: it only
    /// makes the prompt pending. Web schemes and the schemes `krypa-navigation` blocks are
    /// an error (they must not even reach a dialog). Dropped without a prompt: a request
    /// without a user gesture, one from a session that is not the foreground one, and any
    /// request while a prompt is already pending for the session (the first one stays,
    /// repeats are dropped, not coalesced, so a page cannot swap the target under the
    /// user's finger or queue up dialogs).
    fn request_external_open(
        &mut self,
        id: SessionId,
        url: Url,
        user_gesture: bool,
    ) -> Result<Outcome, CoreError> {
        if matches!(url.scheme(), "http" | "https") || is_blocked_scheme(url.scheme()) {
            return Err(CoreError::UnsafeExternalScheme);
        }
        let is_foreground = self.foreground == Some(id);
        let s = self.session_mut(id)?;
        let dropped = if !user_gesture {
            Some("no user gesture")
        } else if !is_foreground {
            Some("session not in the foreground")
        } else if s.pending_external.is_some() {
            Some("a prompt is already pending")
        } else {
            None
        };
        if let Some(why) = dropped {
            tracing::debug!(session = id.get(), why, "external open dropped");
            return Ok(Outcome::default());
        }
        s.pending_external = Some(url.clone());
        Ok(Outcome {
            commands: Vec::new(),
            notices: vec![Notice::ExternalOpenRequested { session: id, url }],
        })
    }

    /// The engine's answer to a manifest request. A result that does not match the
    /// session's pending request (the page changed, the request was replaced, nothing was
    /// asked) is discarded. Otherwise the manifest is parsed defensively; anything wrong
    /// with it falls back to installing the page itself.
    fn on_manifest(
        &mut self,
        id: SessionId,
        request: ManifestRequestId,
        result: ManifestResult,
    ) -> Outcome {
        let Some(s) = self.sessions.get_mut(&id) else {
            return Outcome::default();
        };
        if s.pending_manifest.as_ref().map(|p| p.request) != Some(request) {
            tracing::debug!(session = id.get(), "stale manifest result discarded");
            return Outcome::default();
        }
        let Some(pending) = s.pending_manifest.take() else {
            return Outcome::default();
        };
        let (app, source) = match result {
            ManifestResult::Found {
                manifest_url,
                bytes,
            } => match app_from_manifest(&pending.document_url, &manifest_url, &bytes) {
                Ok(app) => (Some(app), InstallSource::Manifest),
                Err(reason) => (None, InstallSource::Manual(reason)),
            },
            ManifestResult::NotFound => (None, InstallSource::Manual(ManualReason::NoManifest)),
            ManifestResult::Failed(ManifestFailure::TooLarge) => {
                (None, InstallSource::Manual(ManualReason::TooLarge))
            }
            ManifestResult::Failed(_) => (None, InstallSource::Manual(ManualReason::FetchFailed)),
        };
        // The page itself is always a valid candidate: it was checked to be loadable.
        let app =
            app.or_else(|| InstalledApp::manual(&pending.document_url, s.title.as_deref()).ok());
        let Some(app) = app else {
            return Outcome::default();
        };
        Outcome {
            commands: Vec::new(),
            notices: vec![Notice::InstallCandidate {
                session: id,
                app: Box::new(app),
                source,
            }],
        }
    }

    fn one(command: EngineCommand) -> Outcome {
        Outcome {
            commands: vec![command],
            notices: Vec::new(),
        }
    }

    fn suspend_resume_outcome(&self, command: EngineCommand) -> Outcome {
        if self.capabilities.view_suspend {
            Self::one(command)
        } else {
            Outcome::default()
        }
    }

    fn missing(&self, id: SessionId) -> CoreError {
        // Ids are issued in increasing order and never reused.
        if id.get() != 0 && id.get() < self.next_id {
            CoreError::SessionDestroyed(id)
        } else {
            CoreError::UnknownSession(id)
        }
    }

    fn session(&self, id: SessionId) -> Result<&Session, CoreError> {
        self.sessions.get(&id).ok_or_else(|| self.missing(id))
    }

    fn session_mut(&mut self, id: SessionId) -> Result<&mut Session, CoreError> {
        let err = self.missing(id);
        self.sessions.get_mut(&id).ok_or(err)
    }

    fn transition(&mut self, id: SessionId, trigger: Trigger) -> Result<SessionState, CoreError> {
        let s = self.session_mut(id)?;
        let from = s.state;
        let to = from
            .next(trigger)
            .map_err(|e| CoreError::IllegalTransition {
                session: id,
                from: e.from,
                trigger: e.trigger,
            })?;
        s.state = to;
        tracing::debug!(
            session = id.get(),
            ?from,
            ?to,
            ?trigger,
            "lifecycle transition"
        );
        Ok(to)
    }

    fn require_navigable(
        &self,
        id: SessionId,
        what: &'static str,
    ) -> Result<SessionState, CoreError> {
        let state = self.session(id)?.state;
        match state {
            SessionState::Loading
            | SessionState::Foreground
            | SessionState::Background
            | SessionState::Failed => Ok(state),
            _ => Err(CoreError::NotAllowedInState {
                session: id,
                state,
                what,
            }),
        }
    }

    /// A user-driven load: leaves `Failed` for `Loading`.
    fn begin_navigation(&mut self, id: SessionId, what: &'static str) -> Result<(), CoreError> {
        if self.require_navigable(id, what)? == SessionState::Failed {
            self.transition(id, Trigger::RetryStarted)?;
            let s = self.session_mut(id)?;
            s.failure = None;
            s.progress_permille = 0;
            s.reset_security();
        }
        Ok(())
    }

    fn expect_destroy_ack(&mut self, id: SessionId) {
        *self.pending_destroy_acks.entry(id).or_insert(0) += 1;
    }

    fn take_destroy_ack(&mut self, id: SessionId) -> bool {
        match self.pending_destroy_acks.get_mut(&id) {
            Some(n) if *n > 1 => {
                *n -= 1;
                true
            }
            Some(_) => {
                self.pending_destroy_acks.remove(&id);
                true
            }
            None => false,
        }
    }

    /// Least recently foregrounded live view that is not the foreground session.
    fn lru_victim(&self) -> Option<SessionId> {
        self.sessions
            .iter()
            .filter(|(id, s)| s.state.has_live_view() && Some(**id) != self.foreground)
            .min_by_key(|(id, s)| (s.last_foregrounded, **id))
            .map(|(id, _)| *id)
    }

    fn evict_down_to(&mut self, limit: usize, out: &mut Outcome) -> Result<(), CoreError> {
        while self.live_view_count() > limit {
            let Some(victim) = self.lru_victim() else {
                break;
            };
            out.merge(self.evict(victim)?);
        }
        Ok(())
    }

    /// Destroys the view of a non-foreground session. A private session cannot be
    /// restored, so it is removed instead of kept as `Terminated`.
    fn evict(&mut self, id: SessionId) -> Result<Outcome, CoreError> {
        self.transition(id, Trigger::Evict)?;
        self.expect_destroy_ack(id);
        let mut out = Outcome::default();
        out.commands
            .push(EngineCommand::DestroySession { session: id });
        out.merge(self.discard_view(id));
        if self.sessions.contains_key(&id) {
            out.notices.push(Notice::ViewEvicted { session: id });
        }
        Ok(out)
    }

    /// Shared tail of eviction and view loss, after the state is already `Terminated`.
    fn discard_view(&mut self, id: SessionId) -> Outcome {
        let mut out = Outcome::default();
        if let Some(s) = self.sessions.get_mut(&id) {
            s.pending_permissions.clear();
            s.progress_permille = 0;
            s.reset_security();
            if s.is_private() {
                self.sessions.remove(&id);
                out.notices
                    .push(Notice::PrivateSessionDiscarded { session: id });
            }
        }
        out
    }

    fn on_navigation(
        &mut self,
        id: SessionId,
        url: Url,
        phase: NavigationPhase,
        can_go_back: bool,
        can_go_forward: bool,
    ) -> Result<Outcome, CoreError> {
        let is_foreground = self.foreground == Some(id);
        let s = self.session_mut(id)?;
        s.can_go_back = can_go_back;
        s.can_go_forward = can_go_forward;
        if phase == NavigationPhase::Started {
            // Whatever was confirmed for the previous page says nothing about this one.
            s.reset_security();
            s.provisional = true;
            return Ok(Outcome::default());
        }
        if phase == NavigationPhase::Committed {
            // A new document: only a report made for it counts, never the previous one.
            s.security = s.pending_security.take();
            s.provisional = false;
            s.document_changed();
        }
        s.url = Some(url);
        // A failed session stays failed until the user (or the next foreground) retries.
        if s.state == SessionState::Loading {
            let trigger = if is_foreground {
                Trigger::CommittedForeground
            } else {
                Trigger::CommittedBackground
            };
            self.transition(id, trigger)?;
        }
        Ok(Outcome::default())
    }

    fn fail(
        &mut self,
        id: SessionId,
        trigger: Trigger,
        reason: FailureReason,
    ) -> Result<Outcome, CoreError> {
        if self.session(id)?.state == SessionState::Failed {
            self.session_mut(id)?.failure = Some(reason);
            return Ok(Outcome::default());
        }
        self.transition(id, trigger)?;
        self.session_mut(id)?.failure = Some(reason);
        tracing::warn!(session = id.get(), ?reason, "session failed");
        Ok(Outcome {
            commands: Vec::new(),
            notices: vec![Notice::SessionFailed {
                session: id,
                reason,
            }],
        })
    }

    /// The platform destroyed a view we did not ask it to destroy.
    fn on_view_lost(&mut self, id: SessionId) -> Result<Outcome, CoreError> {
        self.transition(id, Trigger::ViewLost)?;
        if self.foreground == Some(id) {
            self.foreground = None;
        }
        tracing::warn!(session = id.get(), "web view lost by the platform");
        let mut out = self.discard_view(id);
        if self.sessions.contains_key(&id) {
            out.notices.push(Notice::ViewLost { session: id });
        }
        Ok(out)
    }
}

/// Builds the app a manifest describes, or says why it cannot be used. `bytes` is hostile:
/// size-capped again here (an adapter may be wrong), UTF-8 and JSON checked, and the result
/// goes through `InstalledApp::from_manifest`, which binds every URL to the document's
/// origin and sanitizes the name. A manifest URL that downgrades an `https` page to
/// `http` is refused.
fn app_from_manifest(
    document_url: &Url,
    manifest_url: &Url,
    bytes: &[u8],
) -> Result<InstalledApp, ManualReason> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ManualReason::TooLarge);
    }
    if document_url.scheme() == "https" && manifest_url.scheme() != "https" {
        return Err(ManualReason::Invalid);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ManualReason::Invalid)?;
    let manifest = Manifest::parse(text).map_err(|_| ManualReason::Invalid)?;
    InstalledApp::from_manifest(document_url, manifest_url, &manifest)
        .map_err(|_| ManualReason::Invalid)
}

/// Strips control and bidi-control characters, trims, caps the length. Titles are
/// hostile page text.
fn sanitize_title(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
        .take(MAX_TITLE_CHARS)
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

const fn event_name(event: &EngineEvent) -> &'static str {
    match event {
        EngineEvent::NavigationChanged { .. } => "NavigationChanged",
        EngineEvent::ProgressChanged { .. } => "ProgressChanged",
        EngineEvent::TitleChanged { .. } => "TitleChanged",
        EngineEvent::SecurityChanged { .. } => "SecurityChanged",
        EngineEvent::IconChanged { .. } => "IconChanged",
        EngineEvent::NewWindowRequested { .. } => "NewWindowRequested",
        EngineEvent::ExternalSchemeRequested { .. } => "ExternalSchemeRequested",
        EngineEvent::ManifestFetched { .. } => "ManifestFetched",
        EngineEvent::PermissionRequested { .. } => "PermissionRequested",
        EngineEvent::LoadFailed { .. } => "LoadFailed",
        EngineEvent::ContentProcessTerminated { .. } => "ContentProcessTerminated",
        EngineEvent::SessionDestroyed { .. } => "SessionDestroyed",
    }
}
