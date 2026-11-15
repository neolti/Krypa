//! The state the UI draws from, and what it hands back to its caller.

use egui::Rect;
use krypa_apps::InstalledApp;
use krypa_core::{
    FailureReason, InstallRefusal, InstallSource, Notice, PendingExternalOpen, SessionInfo,
    SessionManager,
};
use krypa_engine_api::{
    ConnectionSecurity, EngineCapabilities, LoadErrorKind, PermissionDecision, PermissionKind,
    PermissionRequestId, SessionId,
};
use krypa_navigation::{DisplayOrigin, ExternalLink, NavigationUrl, SecurityState};
use url::Url;

use crate::app_icons::IconStore;
use crate::theme::ThemeChoice;

/// Which screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    /// Branding, address field, apps, recents.
    #[default]
    Launcher,
    /// The foreground session with browser chrome around the web view.
    Browser,
    /// Cards for every session.
    Switcher,
}

/// An entry of the recents list. The URL is already validated; the title is hostile text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentEntry {
    /// Page title as stored (sanitized again at render time).
    pub title: String,
    /// Where it leads.
    pub url: NavigationUrl,
}

/// A permission prompt waiting for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPrompt {
    /// Session whose page asked.
    pub session: SessionId,
    /// Prompt id to answer with.
    pub request: PermissionRequestId,
    /// What is requested.
    pub kind: PermissionKind,
    /// Requesting origin.
    pub origin: Url,
}

/// An app waiting for the user to confirm its installation: built by the core from the
/// page's manifest (or from the page itself) and never saved before the user agrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPrompt {
    /// The page it was made for.
    pub session: SessionId,
    /// What would be installed.
    pub app: InstalledApp,
    /// Whether it comes from a manifest or is a plain shortcut, and why.
    pub source: InstallSource,
}

/// Something the user asked for. The UI never performs these itself: the caller turns them
/// into `SessionManager` calls and engine commands.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// Open this address. Produced only from `krypa_navigation::classify_input`; the UI
    /// never assembles a URL. From the launcher the caller opens a new session; the
    /// foreground session already shows its own page.
    Navigate(NavigationUrl),
    /// The text is a search query; the caller applies the configured search provider.
    Search(String),
    /// The text is a link for another app (mail, phone). The caller must ask first.
    OpenExternal(ExternalLink),
    /// Open the installed app with this id (`InstalledApp::id`).
    OpenApp(Url),
    /// Back in the session.
    GoBack(SessionId),
    /// Forward in the session.
    GoForward(SessionId),
    /// Reload the session (also the retry action of the error view).
    Reload(SessionId),
    /// Stop loading.
    Stop(SessionId),
    /// Start installing the page of this session as an app: the caller asks the engine
    /// for the page's manifest and the result comes back as an [`InstallPrompt`].
    AddToKrypa(SessionId),
    /// The user confirmed the app shown in the install dialog; save it.
    ConfirmInstall(Box<InstalledApp>),
    /// The user answered the "open another app" dialog. The caller passes this to
    /// `SessionManager::respond_external_open`; the core, not the UI, knows the URL.
    RespondExternalOpen {
        /// Session whose page asked.
        session: SessionId,
        /// `true` to hand the link to the system, `false` (Cancel) to do nothing.
        allow: bool,
    },
    /// An app icon is wanted and was never fetched: the host fetches this `https` URL (at
    /// most `app_icons::MAX_ICON_BYTES`), decodes it with `app_icons::decode_icon`, and
    /// answers with `AppState::icons.finish`.
    FetchIcon(Url),
    /// Make the session the foreground one.
    SwitchTo(SessionId),
    /// Close the session for good.
    CloseSession(SessionId),
    /// The user answered a permission prompt.
    RespondPermission {
        /// Session whose page asked.
        session: SessionId,
        /// Prompt answered.
        request: PermissionRequestId,
        /// The answer.
        decision: PermissionDecision,
    },
    /// A setting in [`AppState`] (theme, reduced motion) changed; persist it.
    SettingsChanged,
}

/// Everything one UI frame produces for the platform shell.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FrameOutput {
    /// What the user asked for this frame.
    pub intents: Vec<Intent>,
    /// Screen rectangles (points) covered by browser-owned UI. Touches inside go to egui,
    /// all others to the web view. Includes toolbars, error and loading views drawn over
    /// the page, and the whole screen while a dialog or settings panel is open.
    pub covered_rects: Vec<Rect>,
    /// Where the web view belongs (points). `None` when no web view is showing.
    pub web_view_rect: Option<Rect>,
    /// The frame changes with time (spinner); the caller should keep drawing. When
    /// `false`, drawing again is only needed on input or when egui asks for a repaint.
    pub animating: bool,
}

impl FrameOutput {
    /// Whether a touch at `point` belongs to browser-owned UI.
    #[must_use]
    pub fn is_covered(&self, point: egui::Pos2) -> bool {
        self.covered_rects.iter().any(|r| r.contains(point))
    }
}

/// All state of the browser interface. The caller refreshes the session data from the
/// `SessionManager` with [`AppState::sync_sessions`] and feeds its notices to
/// [`AppState::apply_notice`]; the rest is owned by the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    /// Current screen.
    pub screen: Screen,
    /// Text in the launcher's address field.
    pub address_input: String,
    /// Why the last submitted text was rejected, as an index into the strings; cleared on edit.
    pub address_error: Option<&'static str>,
    /// All sessions, in manager order.
    pub sessions: Vec<SessionInfo>,
    /// The foreground session.
    pub foreground: Option<SessionId>,
    /// What the engine supports.
    pub capabilities: EngineCapabilities,
    /// Installed apps.
    pub installed_apps: Vec<InstalledApp>,
    /// Recent pages.
    pub recents: Vec<RecentEntry>,
    /// Permission prompts waiting, oldest first; the first one is shown.
    pub permission_prompts: Vec<PermissionPrompt>,
    /// The app waiting for the user's yes or no.
    pub install_prompt: Option<InstallPrompt>,
    /// Why "Add to Krypa" was refused for the page, until the user dismisses the message.
    pub install_refusal: Option<InstallRefusal>,
    /// The "open another app" prompt, copied from the core by [`AppState::sync_sessions`];
    /// the core owns it (one per session, dropped when the page goes away).
    pub external_prompt: Option<PendingExternalOpen>,
    /// Icons of the installed apps.
    pub icons: IconStore,
    /// The settings panel is open.
    pub settings_open: bool,
    /// Theme choice.
    pub theme: ThemeChoice,
    /// Reduced motion: no spinner, no animated transitions, no continuous repaint.
    pub reduced_motion: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    /// An empty state showing the launcher.
    #[must_use]
    pub fn new() -> Self {
        Self {
            screen: Screen::Launcher,
            address_input: String::new(),
            address_error: None,
            sessions: Vec::new(),
            foreground: None,
            capabilities: EngineCapabilities::default(),
            installed_apps: Vec::new(),
            recents: Vec::new(),
            permission_prompts: Vec::new(),
            install_prompt: None,
            install_refusal: None,
            external_prompt: None,
            icons: IconStore::new(),
            settings_open: false,
            theme: ThemeChoice::System,
            reduced_motion: false,
        }
    }

    /// Copies sessions, foreground and capabilities from the manager and drops permission
    /// prompts whose session no longer has a live view.
    pub fn sync_sessions(&mut self, manager: &SessionManager) {
        self.sessions = manager.sessions();
        self.foreground = manager.foreground_session();
        self.capabilities = *manager.capabilities();
        let live: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|s| s.state.has_live_view())
            .map(|s| s.id)
            .collect();
        self.permission_prompts
            .retain(|p| live.contains(&p.session));
        if self
            .install_prompt
            .as_ref()
            .is_some_and(|p| !live.contains(&p.session))
        {
            self.install_prompt = None;
        }
        self.external_prompt = manager.pending_external_open();
        if self.screen == Screen::Browser && self.foreground.is_none() {
            self.screen = Screen::Launcher;
        }
    }

    /// Reacts to a notice from the manager. Failures need no action here: the failed
    /// session's info drives the error view.
    pub fn apply_notice(&mut self, notice: &Notice) {
        match notice {
            Notice::PermissionRequested {
                session,
                request,
                kind,
                origin,
            } => self.permission_prompts.push(PermissionPrompt {
                session: *session,
                request: *request,
                kind: *kind,
                origin: origin.clone(),
            }),
            Notice::InstallCandidate {
                session,
                app,
                source,
            } => {
                self.install_prompt = Some(InstallPrompt {
                    session: *session,
                    app: (**app).clone(),
                    source: *source,
                });
            }
            Notice::InstallRefused { reason, .. } => self.install_refusal = Some(*reason),
            Notice::SessionOpened { .. } => self.screen = Screen::Browser,
            Notice::ViewEvicted { .. }
            | Notice::ViewLost { .. }
            | Notice::PrivateSessionDiscarded { .. }
            | Notice::SessionFailed { .. }
            | Notice::ExternalOpenRequested { .. } => {}
        }
    }

    /// Info of the foreground session.
    #[must_use]
    pub fn foreground_info(&self) -> Option<&SessionInfo> {
        let id = self.foreground?;
        self.sessions.iter().find(|s| s.id == id)
    }
}

/// The URL to show for a session: where it is, or where it will restore to.
#[must_use]
pub fn session_url(info: &SessionInfo) -> Option<&Url> {
    info.current_url.as_ref().or(info.restorable_url.as_ref())
}

/// The origin text for a URL, produced by `krypa-navigation`. `None` when the URL is not a
/// valid web URL, in which case the UI shows "unknown address", never the raw URL.
#[must_use]
pub fn display_origin_of(url: &Url) -> Option<DisplayOrigin> {
    NavigationUrl::parse(url.as_str())
        .ok()
        .map(|u| u.origin().display_origin())
}

/// Connection security to show for a session, `None` when the engine has confirmed
/// nothing for the current page (shown as "not verified", never as secure). Rules, in
/// order: a failed TLS load is a certificate error; no report from the engine (see
/// `SessionInfo::security`, owned by `krypa-core`) means `None`, so an `https` URL alone
/// earns no trust; the report is combined with the URL by `SecurityState::resolve`, so
/// plain HTTP is never secure; no usable URL is insecure.
#[must_use]
pub fn security_for(info: &SessionInfo) -> Option<SecurityState> {
    if info.failure == Some(FailureReason::LoadFailed(LoadErrorKind::Tls)) {
        return Some(SecurityState::CertificateError);
    }
    let reported = match info.security? {
        ConnectionSecurity::Secure => SecurityState::Secure,
        ConnectionSecurity::Insecure => SecurityState::Insecure,
        ConnectionSecurity::CertificateError => SecurityState::CertificateError,
    };
    let Some(url) = session_url(info).and_then(|u| NavigationUrl::parse(u.as_str()).ok()) else {
        return Some(SecurityState::Insecure);
    };
    Some(SecurityState::resolve(&url, reported))
}
