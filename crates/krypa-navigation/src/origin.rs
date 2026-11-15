use std::fmt;

use url::{Host, Url};

use crate::error::NavigationError;
use crate::navigation_url::NavigationUrl;

/// The web schemes Krypa navigates to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scheme {
    /// `https`
    Https,
    /// `http` (not secure)
    Http,
}

impl Scheme {
    fn default_port(self) -> u16 {
        match self {
            Scheme::Https => 443,
            Scheme::Http => 80,
        }
    }
}

/// Scheme, host and port: the unit of web security.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    scheme: Scheme,
    host: Host<String>,
    port: u16,
}

/// What the security indicator shows for an origin. `Display` renders
/// `host` or `host:port` (port only when not the scheme default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayOrigin {
    /// Scheme, so the indicator can show a lock or a warning.
    pub scheme: Scheme,
    /// Host as shown: lower-case, IPv6 in brackets, subdomains and `www.` kept.
    pub host: String,
    /// Port, only when it differs from the scheme default.
    pub port: Option<u16>,
    /// True when the host is an IDN that is shown as Punycode because it is
    /// not provably safe to render as Unicode.
    pub shown_as_punycode: bool,
}

impl Origin {
    /// Caller guarantees `url` passed `NavigationUrl` validation.
    pub(crate) fn from_validated(url: &Url) -> Self {
        let scheme = if url.scheme() == "http" {
            Scheme::Http
        } else {
            Scheme::Https
        };
        let host = url
            .host()
            .map(|h| h.to_owned())
            .unwrap_or_else(|| Host::Domain(String::new()));
        let port = url.port().unwrap_or_else(|| scheme.default_port());
        Self { scheme, host, port }
    }

    /// Rebuilds an origin from its stored parts, re-validating them as strictly as
    /// [`NavigationUrl::parse`](crate::NavigationUrl::parse) does: the parts are
    /// assembled into `scheme://host:port/` and parsed, and the result must give back
    /// exactly the same scheme, port and (already normalized) host text. A host that is
    /// not in canonical form (upper case, trailing dot, userinfo, path, port, empty
    /// labels, ...) is an error, so a stored origin can never be silently rewritten.
    ///
    /// # Errors
    /// [`NavigationError::InvalidHost`] when `host` is not the canonical text of a valid
    /// host; otherwise whatever [`NavigationUrl::parse`](crate::NavigationUrl::parse)
    /// reports for the assembled URL (for example `InvalidUrl` for port 0).
    pub fn from_parts(scheme: Scheme, host: &str, port: u16) -> Result<Self, NavigationError> {
        let name = match scheme {
            Scheme::Https => "https",
            Scheme::Http => "http",
        };
        let parsed = NavigationUrl::parse(&format!("{name}://{host}:{port}/"))?;
        let origin = parsed.origin();
        let canonical = origin.scheme == scheme
            && origin.port == port
            && origin.host.to_string() == host
            && parsed.as_url().path() == "/"
            && parsed.as_url().query().is_none()
            && parsed.as_url().fragment().is_none();
        if canonical {
            Ok(origin)
        } else {
            Err(NavigationError::InvalidHost)
        }
    }

    /// Scheme of this origin.
    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// Host in its normalized (Punycode, lower-case) form.
    pub fn host(&self) -> &Host<String> {
        &self.host
    }

    /// Effective port (default port filled in).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The text for the security indicator.
    ///
    /// The host is the real host of the URL, nothing that identifies the site
    /// is stripped (subdomains and `www.` stay). An IDN host is shown as
    /// Unicode only if every character is ASCII or Latin-with-diacritics and
    /// it re-encodes to the same Punycode; anything else (Cyrillic, Greek,
    /// mixed script, invisible characters) stays Punycode, so a homograph such
    /// as a Cyrillic `a` in `paypal.com` is visible as `xn--...`.
    pub fn display_origin(&self) -> DisplayOrigin {
        let (host, shown_as_punycode) = match &self.host {
            Host::Domain(domain) => display_domain(domain),
            Host::Ipv4(ip) => (ip.to_string(), false),
            Host::Ipv6(ip) => (format!("[{ip}]"), false),
        };
        let port = (self.port != self.scheme.default_port()).then_some(self.port);
        DisplayOrigin {
            scheme: self.scheme,
            host,
            port,
            shown_as_punycode,
        }
    }
}

impl fmt::Display for DisplayOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.host)?;
        if let Some(port) = self.port {
            write!(f, ":{port}")?;
        }
        Ok(())
    }
}

/// Returns the host text and whether it is Punycode that was kept on purpose.
fn display_domain(domain: &str) -> (String, bool) {
    if !domain.split('.').any(|l| l.starts_with("xn--")) {
        return (domain.to_owned(), false);
    }
    let (unicode, result) = idna::domain_to_unicode(domain);
    let safe = result.is_ok()
        && unicode.chars().all(is_safe_char)
        && idna::domain_to_ascii(&unicode).is_ok_and(|ascii| ascii == domain);
    if safe {
        (unicode, false)
    } else {
        (domain.to_owned(), true)
    }
}

/// Conservative allow-list: ASCII host characters and Latin letters with
/// diacritics. Latin look-alikes of ASCII letters (dotless i, long s, kra) are
/// excluded. Other scripts always display as Punycode; this trades readability
/// of legitimate non-Latin domains for spoofing resistance.
fn is_safe_char(c: char) -> bool {
    match c {
        'a'..='z' | '0'..='9' | '-' | '.' => true,
        '\u{00DF}'..='\u{00FF}' => c != '\u{00F7}',
        '\u{0100}'..='\u{017F}' => !matches!(c, '\u{0131}' | '\u{0138}' | '\u{017F}'),
        _ => false,
    }
}
