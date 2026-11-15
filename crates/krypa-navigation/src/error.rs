use thiserror::Error;

/// Maximum accepted length, in bytes, of any address-bar text or URL.
pub const MAX_INPUT_BYTES: usize = 8192;

/// A link that must be handed to the operating system (mail client, dialer,
/// another app) rather than loaded in the browser. The caller must ask the user
/// before opening it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalLink {
    scheme: String,
    url: String,
}

impl ExternalLink {
    pub(crate) fn new(scheme: String, url: String) -> Self {
        Self { scheme, url }
    }

    /// Lower-case scheme, e.g. `mailto`.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// The normalized URL text. Untrusted: show it escaped, never auto-open it.
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// Why text or a URL cannot become a navigation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NavigationError {
    /// Nothing but whitespace was entered.
    #[error("input is empty")]
    Empty,
    /// Input exceeds [`MAX_INPUT_BYTES`].
    #[error("input is too long")]
    TooLong,
    /// The text is not a parseable URL.
    #[error("not a valid URL")]
    InvalidUrl,
    /// The host is missing, empty, or has an empty label.
    #[error("URL has no valid host")]
    InvalidHost,
    /// The URL carries `user:password@` style credentials (a classic spoofing vector).
    #[error("URL contains embedded credentials")]
    EmbeddedCredentials,
    /// A scheme that may never be navigated to or handed off (`javascript:`, `data:`, ...).
    #[error("scheme `{0}` is not allowed")]
    BlockedScheme(String),
    /// A scheme for the OS to handle; ask the user first.
    #[error("scheme `{}` is handled outside the browser", .0.scheme())]
    ExternalScheme(ExternalLink),
    /// A search template that is not a safe template.
    #[error("invalid search template: {0}")]
    InvalidSearchTemplate(&'static str),
}
