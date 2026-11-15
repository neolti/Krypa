use crate::navigation_url::NavigationUrl;

/// Connection security as reported by the engine. Data only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecurityState {
    /// Valid certificate over HTTPS.
    Secure,
    /// Plain HTTP or otherwise not protected.
    Insecure,
    /// HTTPS whose certificate failed validation.
    CertificateError,
}

impl SecurityState {
    /// Combines the engine's report with what the URL itself allows: a plain
    /// `http` page can never be shown as [`SecurityState::Secure`], even if the
    /// engine (wrongly) says so.
    pub fn resolve(url: &NavigationUrl, reported: SecurityState) -> SecurityState {
        if url.is_insecure() && reported == SecurityState::Secure {
            SecurityState::Insecure
        } else {
            reported
        }
    }
}
