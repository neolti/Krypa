//! Contract between the Krypa core (Rust) and a platform web-engine adapter (e.g. Swift
//! around `WKWebView`). Plain data only: commands flow Rust -> platform, events flow
//! platform -> Rust. No platform code, no async runtime.
//!
//! Strings that come from the web (titles) and URLs are hostile. Consumers must never log
//! URLs with query strings or page titles.
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use serde::{Deserialize, Serialize};
use url::Url;

/// Identifies one browsing session (one web view). Allocated by the core, never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(u64);

impl SessionId {
    /// Wraps a raw id (adapters build ids from the commands they received).
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw id.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Identifies a persistent profile (a platform data store). The adapter interprets the
/// string (on iOS: a UUID for `WKWebsiteDataStore(forIdentifier:)`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileId(pub String);

/// Which website-data store a session uses.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProfileKind {
    /// The platform default data store.
    Default,
    /// A named, persistent data store.
    Persistent(ProfileId),
    /// An in-memory store; nothing survives the session.
    Private,
}

/// Identifies one pending permission prompt raised by a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermissionRequestId(pub u64);

/// Identifies one manifest request. Allocated by the core, never reused; the adapter echoes
/// it in the answer so a result for a page that is gone can be told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ManifestRequestId(pub u64);

/// What a page asks permission for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionKind {
    /// Camera capture.
    Camera,
    /// Microphone capture.
    Microphone,
    /// Camera and microphone together.
    CameraAndMicrophone,
    /// Device motion and orientation.
    MotionAndOrientation,
    /// Geolocation (platform flow still unverified on iOS).
    Geolocation,
}

/// The user's answer to a permission prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionDecision {
    /// Allow.
    Grant,
    /// Refuse.
    Deny,
}

/// What this platform build really supports. Reported by the platform at runtime (an
/// entitlement can be absent in a given build). Every flag defaults to `false`, also when
/// a field is missing from serialized data, so an unknown capability is never assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineCapabilities {
    /// Service workers work in web views.
    pub service_workers: bool,
    /// Passkeys (WebAuthn) work in web views.
    pub passkeys: bool,
    /// Pages can be added to the home screen.
    pub add_to_home_screen: bool,
    /// Named profiles with separate persistent data stores exist.
    pub persistent_profiles: bool,
    /// Private (non-persistent) sessions exist.
    pub private_sessions: bool,
    /// Content blocking rules can be applied.
    pub content_blocking: bool,
    /// The platform offers a real way to suspend a web view's execution. When `false`,
    /// `Suspend`/`Resume` are never sent and nothing may be assumed to be paused.
    pub view_suspend: bool,
    /// The adapter answers `EngineCommand::FetchManifest`. When `false` the UI must not
    /// offer "Add to Krypa" (nothing would ever answer the request).
    pub manifest_fetch: bool,
}

/// Which frame a navigation targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationTarget {
    /// The top-level page of the session.
    MainFrame,
    /// An embedded frame.
    SubFrame,
    /// A new window (`window.open`, `target=_blank`).
    NewWindow,
}

/// What triggered a navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationKind {
    /// The user activated a link.
    LinkActivated,
    /// A form was submitted.
    FormSubmitted,
    /// Reload.
    Reload,
    /// Back/forward.
    BackForward,
    /// Anything else (redirects, script-initiated).
    Other,
}

/// A navigation the web view asks permission for; input of the synchronous policy
/// function in `krypa-core`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationRequest {
    /// Requested URL.
    pub url: Url,
    /// Target frame.
    pub target: NavigationTarget,
    /// What caused it.
    pub kind: NavigationKind,
    /// Best-effort flag from the adapter: a user gesture caused this. Unknown must be
    /// reported as `false`.
    pub user_gesture: bool,
}

/// Why a load failed (no platform message text: it can contain page data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadErrorKind {
    /// The load was cancelled (superseded or stopped). Not a failure.
    Cancelled,
    /// Network error (offline, DNS, refused, timeout).
    Network,
    /// Certificate or TLS error.
    Tls,
    /// Blocked by policy or content rule.
    Blocked,
    /// Anything else.
    Other,
}

/// How far a navigation has progressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NavigationPhase {
    /// Provisional load started.
    Started,
    /// The new document was committed.
    Committed,
    /// The load finished.
    Finished,
}

/// What the engine knows about the connection of the page it shows. There is no
/// "unknown" value: an engine that has nothing confirmed sends no event, and the core then
/// treats the page as unverified. Mixed content (an `https` page that loaded insecure
/// subresources, `hasOnlySecureContent == false` on iOS) has no variant of its own yet:
/// report it as `Insecure`. Never report `Secure` for anything not positively verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConnectionSecurity {
    /// HTTPS with a certificate the platform accepted.
    Secure,
    /// Plain HTTP, or otherwise not protected.
    Insecure,
    /// HTTPS whose certificate was rejected or is invalid.
    CertificateError,
}

/// Why the engine could not deliver a manifest it believes the page has. No platform
/// message text: it can contain page data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManifestFailure {
    /// Network error (offline, DNS, refused, timeout).
    Network,
    /// Blocked by policy (content rule, mixed content).
    Blocked,
    /// The body was longer than the `max_bytes` of the request; the adapter stopped reading.
    TooLarge,
    /// Anything else.
    Other,
}

/// The engine's answer to `EngineCommand::FetchManifest`. Everything in it is hostile
/// page data: the core re-checks the size, parses defensively and never trusts a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManifestResult {
    /// The page declares a manifest and it was fetched.
    Found {
        /// The absolute URL the manifest was fetched from (the resolved
        /// `<link rel="manifest">` `href`, after redirects). Base for relative URLs inside.
        manifest_url: Url,
        /// The raw body, at most `max_bytes` long.
        bytes: Vec<u8>,
    },
    /// The page declares no manifest.
    NotFound,
    /// The page declares one but it could not be delivered.
    Failed(ManifestFailure),
}

/// Rust -> host, not tied to a session: fetch one app icon. The host must use no cookies
/// or credentials, `https` only (also across redirects), a short timeout, and must stop
/// reading after `max_bytes`. Networking is the host's job; the Rust core never opens a
/// socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IconFetchRequest {
    /// Icon URL (`https`, no credentials).
    pub url: Url,
    /// Most bytes to return; a longer body is a failure.
    pub max_bytes: u32,
}

/// What the host fetched for an icon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconFetchResult {
    /// The body, at most `max_bytes` long. Still hostile: nothing is known about the format.
    Fetched(Vec<u8>),
    /// Anything went wrong (network, status, redirect to `http`, too large).
    Failed,
}

/// The host's answer to an [`IconFetchRequest`], matched by `url`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IconFetchResponse {
    /// The URL that was requested.
    pub url: Url,
    /// The outcome.
    pub result: IconFetchResult,
}

/// Rust -> platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineCommand {
    /// Create a web view for `session`, hidden until `Show`.
    CreateSession {
        /// Session to create.
        session: SessionId,
        /// Data store to use.
        profile: ProfileKind,
        /// URL to load right away, if any.
        initial_url: Option<Url>,
    },
    /// Destroy the web view. The adapter answers with `EngineEvent::SessionDestroyed`.
    DestroySession {
        /// Session to destroy.
        session: SessionId,
    },
    /// Load a URL.
    Load {
        /// Target session.
        session: SessionId,
        /// URL to load.
        url: Url,
    },
    /// Reload the current page.
    Reload {
        /// Target session.
        session: SessionId,
    },
    /// Stop loading.
    Stop {
        /// Target session.
        session: SessionId,
    },
    /// Go back in history.
    GoBack {
        /// Target session.
        session: SessionId,
    },
    /// Go forward in history.
    GoForward {
        /// Target session.
        session: SessionId,
    },
    /// Attach the view and make it the visible, interactive one.
    Show {
        /// Target session.
        session: SessionId,
    },
    /// Detach the view from the screen; it stays alive.
    Hide {
        /// Target session.
        session: SessionId,
    },
    /// Pause the view as far as the platform can. Only sent when
    /// `EngineCapabilities::view_suspend` is `true`.
    Suspend {
        /// Target session.
        session: SessionId,
    },
    /// Undo `Suspend`. Only sent when `EngineCapabilities::view_suspend` is `true`.
    Resume {
        /// Target session.
        session: SessionId,
    },
    /// Fetch the manifest of the page the session shows now, with that page's cookies and
    /// credentials mode (only the engine can see its `<link rel="manifest">`). The adapter
    /// answers with exactly one `EngineEvent::ManifestFetched` echoing `request`; it may
    /// answer after the page changed, and the core then discards the result.
    FetchManifest {
        /// Target session.
        session: SessionId,
        /// Echoed in the answer.
        request: ManifestRequestId,
        /// Longest body to read; a longer one is `ManifestFailure::TooLarge`.
        max_bytes: u32,
    },
    /// Hand a URL to the operating system (mail, phone, app link). Only sent after the
    /// user confirmed in Krypa's own dialog; the adapter adds no scheme handling of its own.
    OpenExternal {
        /// Session whose page asked.
        session: SessionId,
        /// What to open.
        url: Url,
    },
    /// Answer a permission prompt.
    RespondToPermission {
        /// Target session.
        session: SessionId,
        /// Prompt being answered.
        request: PermissionRequestId,
        /// The user's decision.
        decision: PermissionDecision,
    },
}

/// Platform -> Rust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineEvent {
    /// Navigation progressed; carries the history flags.
    NavigationChanged {
        /// Source session.
        session: SessionId,
        /// Current URL of the main frame.
        url: Url,
        /// Phase reached.
        phase: NavigationPhase,
        /// Back is possible.
        can_go_back: bool,
        /// Forward is possible.
        can_go_forward: bool,
    },
    /// Load progress, 0..=1000 (per mille).
    ProgressChanged {
        /// Source session.
        session: SessionId,
        /// Progress in per mille.
        permille: u16,
    },
    /// The page title changed (hostile text).
    TitleChanged {
        /// Source session.
        session: SessionId,
        /// New title.
        title: String,
    },
    /// Connection security of the page. For a navigation still in progress (after
    /// `NavigationPhase::Started`, before `Committed`) it describes the page being loaded,
    /// e.g. the server trust result. Every `Committed` document needs its own report, sent
    /// before or after the commit; a commit without one leaves the page unverified.
    SecurityChanged {
        /// Source session.
        session: SessionId,
        /// What the engine confirmed.
        security: ConnectionSecurity,
    },
    /// The page icon changed.
    IconChanged {
        /// Source session.
        session: SessionId,
        /// Icon URL, `None` if cleared.
        icon_url: Option<Url>,
    },
    /// A page asked for a new window.
    NewWindowRequested {
        /// Opener session.
        session: SessionId,
        /// The request.
        request: NavigationRequest,
    },
    /// A navigation to a non-web scheme (mailto, tel, app links).
    ExternalSchemeRequested {
        /// Source session.
        session: SessionId,
        /// Target URL.
        url: Url,
        /// Best-effort flag from the adapter: a user gesture caused this. Unknown (and a
        /// missing field in serialized data) must be `false`; the core then blocks the
        /// request without asking.
        #[serde(default)]
        user_gesture: bool,
    },
    /// The answer to `EngineCommand::FetchManifest`.
    ManifestFetched {
        /// Source session.
        session: SessionId,
        /// The request being answered.
        request: ManifestRequestId,
        /// What the engine found.
        result: ManifestResult,
    },
    /// A page asked for a permission.
    PermissionRequested {
        /// Source session.
        session: SessionId,
        /// Prompt id, unique per session.
        request: PermissionRequestId,
        /// What is requested.
        kind: PermissionKind,
        /// Requesting origin.
        origin: Url,
    },
    /// A load failed.
    LoadFailed {
        /// Source session.
        session: SessionId,
        /// Category of the failure.
        kind: LoadErrorKind,
        /// Platform error code (diagnostics only).
        code: i32,
    },
    /// The web content process died (iOS: `webViewWebContentProcessDidTerminate`).
    ContentProcessTerminated {
        /// Source session.
        session: SessionId,
    },
    /// The web view is gone, either acknowledging `DestroySession` or lost on its own.
    SessionDestroyed {
        /// Source session.
        session: SessionId,
    },
}

impl EngineEvent {
    /// The session this event is about.
    #[must_use]
    pub const fn session(&self) -> SessionId {
        match self {
            Self::NavigationChanged { session, .. }
            | Self::ProgressChanged { session, .. }
            | Self::TitleChanged { session, .. }
            | Self::SecurityChanged { session, .. }
            | Self::IconChanged { session, .. }
            | Self::NewWindowRequested { session, .. }
            | Self::ExternalSchemeRequested { session, .. }
            | Self::ManifestFetched { session, .. }
            | Self::PermissionRequested { session, .. }
            | Self::LoadFailed { session, .. }
            | Self::ContentProcessTerminated { session }
            | Self::SessionDestroyed { session } => *session,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_capability_is_never_assumed_true_when_missing_from_wire_data() {
        let caps: EngineCapabilities = serde_json::from_str(r#"{"passkeys":true}"#).unwrap();
        assert!(caps.passkeys);
        assert!(!caps.service_workers);
        assert!(!caps.view_suspend);
        assert!(!caps.manifest_fetch);
        assert_eq!(
            serde_json::from_str::<EngineCapabilities>("{}").unwrap(),
            EngineCapabilities::default()
        );
    }

    #[test]
    fn session_ids_stay_plain_numbers_on_the_wire() {
        let cmd = EngineCommand::Reload {
            session: SessionId::new(7),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"Reload":{"session":7}}"#);
        assert_eq!(serde_json::from_str::<EngineCommand>(&json).unwrap(), cmd);
    }

    #[test]
    fn events_survive_a_serde_round_trip_so_adapters_can_rely_on_the_shape() {
        let ev = EngineEvent::PermissionRequested {
            session: SessionId::new(1),
            request: PermissionRequestId(3),
            kind: PermissionKind::Camera,
            origin: Url::parse("https://example.com").unwrap(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(serde_json::from_str::<EngineEvent>(&json).unwrap(), ev);
    }

    #[test]
    fn security_events_keep_their_wire_shape() {
        let ev = EngineEvent::SecurityChanged {
            session: SessionId::new(2),
            security: ConnectionSecurity::CertificateError,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(
            json,
            r#"{"SecurityChanged":{"session":2,"security":"CertificateError"}}"#
        );
        assert_eq!(serde_json::from_str::<EngineEvent>(&json).unwrap(), ev);
        assert_eq!(ev.session(), SessionId::new(2));
    }

    #[test]
    fn a_missing_gesture_flag_on_the_wire_means_no_gesture() {
        let ev: EngineEvent = serde_json::from_str(
            r#"{"ExternalSchemeRequested":{"session":1,"url":"tel:+4312345"}}"#,
        )
        .unwrap();
        assert!(matches!(
            ev,
            EngineEvent::ExternalSchemeRequested {
                user_gesture: false,
                ..
            }
        ));
    }

    #[test]
    fn manifest_answers_keep_their_session_and_survive_a_round_trip() {
        let ev = EngineEvent::ManifestFetched {
            session: SessionId::new(4),
            request: ManifestRequestId(9),
            result: ManifestResult::Found {
                manifest_url: Url::parse("https://a.example/m.json").unwrap(),
                bytes: b"{}".to_vec(),
            },
        };
        assert_eq!(ev.session(), SessionId::new(4));
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(serde_json::from_str::<EngineEvent>(&json).unwrap(), ev);
    }

    #[test]
    fn private_and_persistent_profiles_are_distinct_kinds() {
        assert_ne!(
            ProfileKind::Private,
            ProfileKind::Persistent(ProfileId("a".into()))
        );
    }
}
