use std::fmt;

use url::{Host, Url};

use crate::error::{ExternalLink, NavigationError, MAX_INPUT_BYTES};
use crate::origin::Origin;
use crate::scheme::{self, SchemeClass};

/// Trims surrounding whitespace/control characters, as the URL standard does.
pub(crate) fn trim_input(input: &str) -> &str {
    input.trim_matches(|c: char| c <= ' ' || c.is_whitespace())
}

/// Removes the tab and newline characters the URL standard silently drops, so that
/// scheme checks see exactly what the URL parser will see (`java<TAB>script:`).
pub(crate) fn strip_tab_newline(input: &str) -> String {
    input
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect()
}

/// A validated, normalized top-level navigation target.
///
/// Invariants: scheme is `https` or `http`; host is present and has no empty
/// labels; no username or password; port is not 0; a single trailing dot on a
/// domain is removed so one site has one canonical form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NavigationUrl {
    url: Url,
}

impl NavigationUrl {
    /// Validates `input` as a web URL.
    ///
    /// Surrounding whitespace is trimmed and embedded tabs/newlines are removed
    /// (as every browser does) before checking. Non-web schemes yield
    /// [`NavigationError::BlockedScheme`] or [`NavigationError::ExternalScheme`].
    pub fn parse(input: &str) -> Result<Self, NavigationError> {
        Self::parse_limited(input, MAX_INPUT_BYTES)
    }

    pub(crate) fn parse_limited(input: &str, max_bytes: usize) -> Result<Self, NavigationError> {
        if input.len() > max_bytes {
            return Err(NavigationError::TooLong);
        }
        let trimmed = trim_input(input);
        if trimmed.is_empty() {
            return Err(NavigationError::Empty);
        }
        let cleaned = strip_tab_newline(trimmed);
        let mut url = Url::parse(&cleaned).map_err(|_| NavigationError::InvalidUrl)?;

        match scheme::classify(url.scheme(), url.has_authority()) {
            SchemeClass::Web => {}
            SchemeClass::External => {
                return Err(NavigationError::ExternalScheme(ExternalLink::new(
                    url.scheme().to_owned(),
                    url.into(),
                )))
            }
            SchemeClass::Blocked | SchemeClass::Unknown => {
                return Err(NavigationError::BlockedScheme(url.scheme().to_owned()))
            }
        }

        if !url.username().is_empty() || url.password().is_some() {
            return Err(NavigationError::EmbeddedCredentials);
        }
        if url.port() == Some(0) {
            return Err(NavigationError::InvalidUrl);
        }
        let trailing_dot_free = match url.host() {
            None => return Err(NavigationError::InvalidHost),
            Some(Host::Domain(domain)) => {
                let bare = domain.strip_suffix('.').unwrap_or(domain);
                if bare.is_empty() || bare.split('.').any(str::is_empty) {
                    return Err(NavigationError::InvalidHost);
                }
                (bare.len() != domain.len()).then(|| bare.to_owned())
            }
            Some(_) => None,
        };
        if let Some(bare) = trailing_dot_free {
            url.set_host(Some(&bare))
                .map_err(|_| NavigationError::InvalidHost)?;
        }
        Ok(Self { url })
    }

    /// The normalized URL text.
    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    /// The underlying parsed URL, read-only.
    pub fn as_url(&self) -> &Url {
        &self.url
    }

    /// The origin (scheme, host, port) of this URL.
    pub fn origin(&self) -> Origin {
        Origin::from_validated(&self.url)
    }

    /// True for plain `http`: the UI must flag it as not secure.
    pub fn is_insecure(&self) -> bool {
        self.url.scheme() == "http"
    }
}

impl fmt::Display for NavigationUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.url.as_str())
    }
}
