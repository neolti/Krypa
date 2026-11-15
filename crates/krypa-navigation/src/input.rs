use url::Host;

use crate::error::{ExternalLink, NavigationError, MAX_INPUT_BYTES};
use crate::navigation_url::{strip_tab_newline, trim_input, NavigationUrl};
use crate::scheme::{self, SchemeClass};

/// What address-bar text means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// A validated web navigation.
    Url(NavigationUrl),
    /// Free text for the search provider (trimmed, otherwise unmodified).
    Search(String),
    /// A `mailto:`, `tel:` or app link: ask the user before handing it to the OS.
    External(ExternalLink),
}

/// Classifies untrusted address-bar text.
///
/// * Text with an explicit scheme is validated as a URL. `javascript:`, `data:`,
///   `file:` and similar are an `Err`, never search text, even when padded with
///   whitespace, mixed case, or tabs/newlines inside the scheme.
/// * Text without a scheme is a URL only if it has no whitespace or control
///   characters and looks like a host: `localhost`, a dotted-quad IPv4, a
///   bracketed IPv6, a name with a dot (`example.com`, IDN included), or any host
///   with an explicit port (`localhost:3000`). It becomes `https://...`.
/// * Everything else is search text.
///
/// Credentials (`user@host`) are an `Err`, so an email address typed alone is
/// rejected rather than searched. Input longer than [`MAX_INPUT_BYTES`] is an `Err`.
pub fn classify_input(raw: &str) -> Result<Input, NavigationError> {
    if raw.len() > MAX_INPUT_BYTES {
        return Err(NavigationError::TooLong);
    }
    let trimmed = trim_input(raw);
    if trimmed.is_empty() {
        return Err(NavigationError::Empty);
    }
    let cleaned = strip_tab_newline(trimmed);

    if let Some((scheme, rest)) = split_scheme(&cleaned) {
        let scheme = scheme.to_ascii_lowercase();
        match scheme::classify(&scheme, rest.starts_with("//")) {
            SchemeClass::Blocked => return Err(NavigationError::BlockedScheme(scheme)),
            SchemeClass::Web | SchemeClass::External => return from_parsed(&cleaned),
            // `localhost:3000` looks like a scheme; anything else unknown is just text.
            SchemeClass::Unknown => {}
        }
    }

    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control())
        || trimmed.starts_with(['/', '\\', '@'])
    {
        return Ok(Input::Search(trimmed.to_owned()));
    }
    classify_bare(trimmed)
}

fn from_parsed(text: &str) -> Result<Input, NavigationError> {
    match NavigationUrl::parse(text) {
        Ok(url) => Ok(Input::Url(url)),
        Err(NavigationError::ExternalScheme(link)) => Ok(Input::External(link)),
        Err(e) => Err(e),
    }
}

/// `text` has no scheme, whitespace or control characters.
fn classify_bare(text: &str) -> Result<Input, NavigationError> {
    let candidate = format!("https://{text}");
    let url = match NavigationUrl::parse(&candidate) {
        Ok(url) => url,
        Err(e @ (NavigationError::EmbeddedCredentials | NavigationError::TooLong)) => {
            return Err(e)
        }
        Err(_) => return Ok(Input::Search(text.to_owned())),
    };

    let (host_text, has_port) = bare_host_and_port(text);
    let looks_like_host = match url.as_url().host() {
        Some(Host::Ipv6(_)) => true,
        Some(Host::Ipv4(_)) => is_dotted_quad(host_text),
        Some(Host::Domain(domain)) => {
            has_port || domain == "localhost" || domain.ends_with(".localhost") || {
                let bare = domain.strip_suffix('.').unwrap_or(domain);
                bare.contains('.')
            }
        }
        None => false,
    };
    if looks_like_host {
        Ok(Input::Url(url))
    } else {
        Ok(Input::Search(text.to_owned()))
    }
}

/// Splits `scheme:rest` when the prefix is syntactically a scheme.
fn split_scheme(text: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = text.split_once(':')?;
    let mut chars = scheme.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    (first_ok && rest_ok).then_some((scheme, rest))
}

/// Host text and whether an explicit port was typed, for scheme-less input.
fn bare_host_and_port(text: &str) -> (&str, bool) {
    let authority = text.split(['/', '?', '#']).next().unwrap_or(text);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.starts_with('[') {
        return (authority, authority.contains("]:"));
    }
    match authority.rsplit_once(':') {
        Some((host, _port)) => (host, true),
        None => (authority, false),
    }
}

fn is_dotted_quad(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() == 4
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()))
}
