//! Synchronous navigation policy.
//!
//! WKWebView asks for navigation and new-window policy inside a delegate callback that
//! must be answered immediately. This module is therefore a pure function with no state,
//! no events and no commands, deliberately separate from the async command/event flow.

use krypa_engine_api::{NavigationKind, NavigationRequest, NavigationTarget};
use url::Url;

/// Why a navigation was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// Scheme that must never navigate (`javascript:`, `file:`, top-level `data:`...).
    DisallowedScheme,
    /// `http(s)` URL without a host.
    NoHost,
    /// `http(s)` URL with embedded credentials (`https://bank.com@evil.test`).
    EmbeddedCredentials,
    /// Popup or external-app launch without a user gesture.
    NoUserGesture,
}

/// The answer to a policy question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDecision {
    /// Let the web view navigate.
    Allow,
    /// Refuse.
    Cancel(CancelReason),
    /// Hand the URL to the system (mailto, tel, app links); the UI should ask the user.
    OpenExternally,
    /// Open the URL in a new session (same profile).
    OpenInNewSession,
}

/// Session and app context for a policy question.
#[derive(Debug, Clone, Copy, Default)]
pub struct PolicyContext<'a> {
    /// Scope URL if the session is an installed app: user-activated links that leave it
    /// open in a new session instead of replacing the app.
    pub app_scope: Option<&'a Url>,
}

/// Whether `url` lies inside `scope` (same origin and path prefix).
#[must_use]
pub fn is_in_scope(scope: &Url, url: &Url) -> bool {
    scope.origin() == url.origin() && url.path().starts_with(scope.path())
}

/// Whether `url` may be loaded as a top-level page by Krypa itself: `http(s)` with a
/// host and no credentials.
#[must_use]
pub fn is_loadable(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some_and(|h| !h.is_empty())
        && url.username().is_empty()
        && url.password().is_none()
}

/// Decides a navigation or new-window request right now.
#[must_use]
pub fn decide_navigation(req: &NavigationRequest, ctx: &PolicyContext<'_>) -> NavigationDecision {
    use NavigationDecision::{Allow, Cancel, OpenExternally, OpenInNewSession};
    let top_level = req.target != NavigationTarget::SubFrame;
    match req.url.scheme() {
        "javascript" | "file" => Cancel(CancelReason::DisallowedScheme),
        // Embedded frames legitimately use these; a top-level navigation never may.
        "data" | "blob" | "about" if top_level => Cancel(CancelReason::DisallowedScheme),
        "data" | "blob" | "about" => Allow,
        "http" | "https" => {
            if req.url.host_str().is_none_or(str::is_empty) {
                return Cancel(CancelReason::NoHost);
            }
            if !req.url.username().is_empty() || req.url.password().is_some() {
                return Cancel(CancelReason::EmbeddedCredentials);
            }
            match req.target {
                NavigationTarget::SubFrame => Allow,
                NavigationTarget::NewWindow if req.user_gesture => OpenInNewSession,
                NavigationTarget::NewWindow => Cancel(CancelReason::NoUserGesture),
                NavigationTarget::MainFrame => {
                    // Only a user-activated link may be diverted: server redirects (OAuth
                    // logins) leave the scope legitimately and must keep working.
                    let leaves_app = ctx.app_scope.is_some_and(|s| !is_in_scope(s, &req.url));
                    if leaves_app && req.user_gesture && req.kind == NavigationKind::LinkActivated {
                        OpenInNewSession
                    } else {
                        Allow
                    }
                }
            }
        }
        // mailto:, tel:, app links, custom schemes.
        _ if !top_level => Cancel(CancelReason::DisallowedScheme),
        _ if req.user_gesture => OpenExternally,
        _ => Cancel(CancelReason::NoUserGesture),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use NavigationDecision::{Allow, Cancel, OpenExternally, OpenInNewSession};

    fn req(url: &str, target: NavigationTarget, gesture: bool) -> NavigationRequest {
        NavigationRequest {
            url: Url::parse(url).unwrap(),
            target,
            kind: NavigationKind::LinkActivated,
            user_gesture: gesture,
        }
    }

    fn decide(url: &str, target: NavigationTarget, gesture: bool) -> NavigationDecision {
        decide_navigation(&req(url, target, gesture), &PolicyContext::default())
    }

    #[test]
    fn a_javascript_or_file_url_can_never_become_a_navigation_in_any_frame() {
        for target in [
            NavigationTarget::MainFrame,
            NavigationTarget::SubFrame,
            NavigationTarget::NewWindow,
        ] {
            for url in ["javascript:alert(1)", "file:///etc/passwd"] {
                assert_eq!(
                    decide(url, target, true),
                    Cancel(CancelReason::DisallowedScheme)
                );
            }
        }
    }

    #[test]
    fn top_level_data_and_blob_are_refused_but_embedded_frames_may_use_them() {
        assert_eq!(
            decide("data:text/html,hi", NavigationTarget::MainFrame, true),
            Cancel(CancelReason::DisallowedScheme)
        );
        assert_eq!(
            decide("blob:https://a.test/x", NavigationTarget::NewWindow, true),
            Cancel(CancelReason::DisallowedScheme)
        );
        assert_eq!(
            decide("about:blank", NavigationTarget::SubFrame, false),
            Allow
        );
    }

    #[test]
    fn credentials_in_the_url_cannot_disguise_the_real_host() {
        assert_eq!(
            decide(
                "https://bank.com@evil.test/",
                NavigationTarget::MainFrame,
                true
            ),
            Cancel(CancelReason::EmbeddedCredentials)
        );
    }

    #[test]
    fn plain_web_navigation_is_allowed_including_insecure_http() {
        assert_eq!(
            decide(
                "https://example.com/a?q=1",
                NavigationTarget::MainFrame,
                false
            ),
            Allow
        );
        assert_eq!(
            decide("http://example.com/", NavigationTarget::MainFrame, false),
            Allow
        );
        assert_eq!(
            decide("https://example.com/", NavigationTarget::SubFrame, false),
            Allow
        );
    }

    #[test]
    fn a_popup_without_a_user_gesture_is_blocked_and_with_one_opens_a_new_session() {
        assert_eq!(
            decide("https://ads.test/", NavigationTarget::NewWindow, false),
            Cancel(CancelReason::NoUserGesture)
        );
        assert_eq!(
            decide("https://example.com/", NavigationTarget::NewWindow, true),
            OpenInNewSession
        );
    }

    #[test]
    fn external_schemes_open_the_system_only_after_a_user_gesture() {
        assert_eq!(
            decide("mailto:a@b.test", NavigationTarget::MainFrame, true),
            OpenExternally
        );
        assert_eq!(
            decide("tel:+4312345", NavigationTarget::NewWindow, true),
            OpenExternally
        );
        assert_eq!(
            decide("itms-apps://x", NavigationTarget::MainFrame, false),
            Cancel(CancelReason::NoUserGesture)
        );
        assert_eq!(
            decide("mailto:a@b.test", NavigationTarget::SubFrame, true),
            Cancel(CancelReason::DisallowedScheme)
        );
    }

    #[test]
    fn an_installed_app_does_not_get_replaced_by_a_link_that_leaves_its_scope() {
        let scope = Url::parse("https://app.example/").unwrap();
        let ctx = PolicyContext {
            app_scope: Some(&scope),
        };
        let leaving = req("https://other.test/", NavigationTarget::MainFrame, true);
        assert_eq!(decide_navigation(&leaving, &ctx), OpenInNewSession);
        let inside = req(
            "https://app.example/page",
            NavigationTarget::MainFrame,
            true,
        );
        assert_eq!(decide_navigation(&inside, &ctx), Allow);
    }

    #[test]
    fn an_app_redirect_out_of_scope_is_allowed_so_login_flows_keep_working() {
        let scope = Url::parse("https://app.example/").unwrap();
        let ctx = PolicyContext {
            app_scope: Some(&scope),
        };
        let mut redirect = req("https://login.test/", NavigationTarget::MainFrame, false);
        redirect.kind = NavigationKind::Other;
        assert_eq!(decide_navigation(&redirect, &ctx), Allow);
    }

    #[test]
    fn scope_matching_compares_origin_and_path_prefix() {
        let scope = Url::parse("https://a.test/app/").unwrap();
        assert!(is_in_scope(
            &scope,
            &Url::parse("https://a.test/app/x").unwrap()
        ));
        assert!(!is_in_scope(
            &scope,
            &Url::parse("https://a.test/other").unwrap()
        ));
        assert!(!is_in_scope(
            &scope,
            &Url::parse("http://a.test/app/x").unwrap()
        ));
        assert!(!is_in_scope(
            &scope,
            &Url::parse("https://a.test.evil/app/x").unwrap()
        ));
    }

    #[test]
    fn loadable_means_http_or_https_with_host_and_no_credentials() {
        assert!(is_loadable(&Url::parse("https://example.com").unwrap()));
        assert!(!is_loadable(&Url::parse("javascript:1").unwrap()));
        assert!(!is_loadable(
            &Url::parse("https://u:p@example.com").unwrap()
        ));
        assert!(!is_loadable(&Url::parse("mailto:a@b.test").unwrap()));
    }
}
