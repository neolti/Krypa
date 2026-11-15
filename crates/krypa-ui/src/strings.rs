//! Every user-visible string of the interface lives here, so localization later means
//! replacing this one module. Functions are used where a string embeds web-derived text;
//! they receive text that has already been through [`crate::text::display_text`].

use krypa_core::SessionState;
use krypa_engine_api::PermissionKind;

// --- launcher
/// Product name shown as branding.
pub const APP_NAME: &str = "Krypa";
/// Line under the product name.
pub const TAGLINE: &str = "Web apps and browsing, in one place";
/// Accessible name and placeholder of the launcher's address/search field.
pub const ADDRESS_FIELD_LABEL: &str = "Search or enter address";
/// Button that submits the address field.
pub const ADDRESS_GO: &str = "Go";
/// Shown under the field when the typed text can never be opened.
pub const ADDRESS_ERROR_BLOCKED: &str = "This kind of address cannot be opened.";
/// Shown under the field when the typed text carries a user name or password.
pub const ADDRESS_ERROR_CREDENTIALS: &str =
    "Addresses with a user name or password are not opened.";
/// Shown under the field when the typed text is too long.
pub const ADDRESS_ERROR_TOO_LONG: &str = "That text is too long.";
/// Shown under the field for any other rejected input.
pub const ADDRESS_ERROR_INVALID: &str = "That is not a valid address.";
/// Heading of the installed-app grid.
pub const SECTION_APPS: &str = "Apps";
/// Shown when no app is installed yet.
pub const APPS_EMPTY: &str = "No apps yet. Open a site and choose Add to Krypa.";
/// Heading of the recents list.
pub const SECTION_RECENTS: &str = "Recent";
/// Shown when there are no recents.
pub const RECENTS_EMPTY: &str = "Nothing here yet.";

// --- navigation, toolbar
/// Back button.
pub const BACK: &str = "Back";
/// Forward button.
pub const FORWARD: &str = "Forward";
/// Reload button.
pub const RELOAD: &str = "Reload";
/// Stop button.
pub const STOP: &str = "Stop loading";
/// Button that returns to the launcher.
pub const HOME: &str = "Home";
/// Button that opens the app switcher.
pub const SWITCHER: &str = "Show open pages";
/// Settings entry.
pub const SETTINGS: &str = "Settings";
/// Install action in the browser chrome.
pub const ADD_TO_KRYPA: &str = "Add to Krypa";
/// Shown instead of the install action when the page is already inside an installed app.
pub const ALREADY_ADDED: &str = "Added to Krypa";
/// Accessible name of the progress bar.
pub const LOADING: &str = "Loading page";
/// Generic close button.
pub const CLOSE: &str = "Close";
/// Origin text when the page address is unknown.
pub const ORIGIN_UNKNOWN: &str = "Unknown address";
/// Note next to an origin that is shown in its ASCII (punycode) form.
pub const PUNYCODE_NOTE: &str = "Shown in ASCII form";

// --- security indicator
/// Secure connection.
pub const SECURITY_SECURE: &str = "Secure connection";
/// Plain HTTP.
pub const SECURITY_INSECURE: &str = "Not secure";
/// The engine has not confirmed the page's connection (loading, or nothing reported).
pub const SECURITY_UNVERIFIED: &str = "Not verified";
/// Certificate failure.
pub const SECURITY_CERTIFICATE: &str = "Certificate problem";

// --- switcher
/// Switcher screen title.
pub const SWITCHER_TITLE: &str = "Open pages";
/// Button that leaves the switcher for the launcher.
pub const SWITCHER_NEW: &str = "New page";
/// Button that leaves the switcher for the current page.
pub const SWITCHER_DONE: &str = "Done";
/// Shown when no session exists.
pub const SWITCHER_EMPTY: &str = "No open pages.";
/// Tag on private sessions.
pub const PRIVATE_TAG: &str = "Private";
/// Title shown when a page has no usable title.
pub const UNTITLED: &str = "Untitled page";

/// Short state name shown on a switcher card.
#[must_use]
pub const fn state_badge(state: SessionState) -> &'static str {
    match state {
        SessionState::Foreground => "Live, showing",
        SessionState::Loading => "Live, loading",
        SessionState::Background => "Live, in background",
        SessionState::Suspended => "Paused",
        SessionState::Terminated => "Not loaded",
        SessionState::NotRunning => "Not started",
        SessionState::Failed => "Failed",
    }
}

/// One-line hint of what tapping a card does in that state.
#[must_use]
pub const fn state_hint(state: SessionState) -> &'static str {
    match state {
        SessionState::Terminated => "Tap to load it again",
        SessionState::Failed => "Tap to try again",
        SessionState::NotRunning => "Tap to start",
        SessionState::Suspended => "Tap to resume",
        SessionState::Foreground | SessionState::Loading | SessionState::Background => {
            "Tap to open"
        }
    }
}

/// Accessible name of a switcher card.
#[must_use]
pub fn switcher_card_label(
    title: &str,
    origin: &str,
    state: SessionState,
    private: bool,
) -> String {
    let private = if private { ", private" } else { "" };
    format!("{title}, {origin}, {}{private}", state_badge(state))
}

/// Accessible name of a switcher card's close button.
#[must_use]
pub fn close_page_label(title: &str) -> String {
    format!("Close {title}")
}

/// Accessible name of an installed-app card.
#[must_use]
pub fn app_card_label(name: &str, origin: &str) -> String {
    format!("{name}, {origin}")
}

// --- errors, loading
/// Title of the failed-load view.
pub const ERROR_LOAD_TITLE: &str = "This page did not load";
/// Body for network failures.
pub const ERROR_NETWORK: &str = "Check your connection and try again.";
/// Body for certificate failures.
pub const ERROR_TLS: &str = "The connection could not be verified, so Krypa did not open the page.";
/// Body for blocked loads.
pub const ERROR_BLOCKED: &str = "This page was blocked.";
/// Body for other failures.
pub const ERROR_OTHER: &str = "Something went wrong while loading.";
/// Title of the crashed-page view.
pub const ERROR_CRASH_TITLE: &str = "This page stopped working";
/// Body of the crashed-page view.
pub const ERROR_CRASH: &str = "The page ran out of resources or crashed. You can reload it.";
/// Retry button.
pub const RETRY: &str = "Try again";
/// Shown while a page without a view is being brought back.
pub const RESTORING: &str = "Loading page";

// --- permission dialog
/// Allow button.
pub const PERMISSION_ALLOW: &str = "Allow";
/// Deny button.
pub const PERMISSION_DENY: &str = "Don\u{2019}t allow";

/// Dialog title naming the requesting site and what it wants.
#[must_use]
pub fn permission_title(origin: &str, kind: PermissionKind) -> String {
    let what = match kind {
        PermissionKind::Camera => "use your camera",
        PermissionKind::Microphone => "use your microphone",
        PermissionKind::CameraAndMicrophone => "use your camera and microphone",
        PermissionKind::MotionAndOrientation => "use motion and orientation data",
        PermissionKind::Geolocation => "know your location",
    };
    format!("{origin} wants to {what}")
}

// --- settings
/// Settings panel title.
pub const SETTINGS_TITLE: &str = "Settings";
/// Theme row.
pub const SETTINGS_THEME: &str = "Appearance";
/// Theme choice: follow the system.
pub const THEME_SYSTEM: &str = "System";
/// Theme choice: light.
pub const THEME_LIGHT: &str = "Light";
/// Theme choice: dark.
pub const THEME_DARK: &str = "Dark";
/// Reduced-motion row.
pub const SETTINGS_REDUCED_MOTION: &str = "Reduce motion";

// --- external-open dialog
/// Safe default: do nothing.
pub const EXTERNAL_OPEN_CANCEL: &str = "Cancel";
/// Hands the link to the system.
pub const EXTERNAL_OPEN_CONFIRM: &str = "Open";

/// Dialog title naming the page that asked.
#[must_use]
pub fn external_open_title(origin: &str) -> String {
    format!("{origin} wants to open another app")
}

/// The kind and the (shortened) target of the link, one line for the dialog.
#[must_use]
pub fn external_open_target(scheme: &str, target: &str) -> String {
    format!("Type: {scheme}. Link: {target}")
}

// --- install dialog
/// Confirms the install.
pub const INSTALL_CONFIRM: &str = "Add";
/// Safe default: install nothing.
pub const INSTALL_CANCEL: &str = "Cancel";
/// Note when the site has no manifest.
pub const INSTALL_NOTE_NO_MANIFEST: &str =
    "This site has no app information. It is added as a shortcut to this page.";
/// Note when the manifest could not be used.
pub const INSTALL_NOTE_BAD_MANIFEST: &str =
    "This site\u{2019}s app information could not be used. It is added as a shortcut to this page.";

/// Title of the message shown when "Add to Krypa" is refused.
pub const INSTALL_REFUSED_TITLE: &str = "Can\u{2019}t add this site";
/// The page is not https.
pub const INSTALL_REFUSED_NOT_HTTPS: &str = "Only secure (https) sites can be added to Krypa.";
/// An https page whose connection is not confirmed secure.
pub const INSTALL_REFUSED_NOT_SECURE: &str =
    "This site\u{2019}s connection is not verified as secure, so it can\u{2019}t be added to Krypa.";

/// Install dialog title naming the app (already sanitized by `krypa-apps`).
#[must_use]
pub fn install_title(name: &str) -> String {
    format!("Add {name} to Krypa?")
}

/// The address line of the install dialog.
#[must_use]
pub fn install_origin(origin: &str) -> String {
    format!("From {origin}")
}
