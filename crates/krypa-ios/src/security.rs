//! What the connection of a committed page deserves, decided here and not in Swift.
//!
//! Swift reports three facts about the document `WKWebView` just committed: whether the
//! server trust of the main frame is present and evaluates as valid, and whether the page
//! loaded only secure content (`hasOnlySecureContent`). The URL scheme comes from the URL
//! itself. This pure function turns them into the contract's [`ConnectionSecurity`], so the
//! rule "Secure only when everything is positively confirmed" is one unit-tested place.

use krypa_engine_api::ConnectionSecurity;
use serde::Deserialize;
use url::Url;

/// What the adapter learned about the server trust of the main frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Trust {
    /// No trust object (plain HTTP, or the platform has none for this document).
    Absent,
    /// A trust object that evaluated successfully (`SecTrustEvaluateWithError`).
    Valid,
    /// A trust object that failed evaluation.
    Invalid,
}

/// `Secure` only for an `https` document with valid trust and no insecure subresources.
/// Everything else is less trusting; a certificate that failed is reported as such.
#[must_use]
pub fn connection_security(
    url: &Url,
    trust: Trust,
    only_secure_content: bool,
) -> ConnectionSecurity {
    if url.scheme() != "https" {
        return ConnectionSecurity::Insecure;
    }
    match trust {
        Trust::Invalid => ConnectionSecurity::CertificateError,
        // https without anything confirming the certificate is not "secure".
        Trust::Absent => ConnectionSecurity::Insecure,
        Trust::Valid if only_secure_content => ConnectionSecurity::Secure,
        // Mixed content: the contract has no variant of its own, report Insecure.
        Trust::Valid => ConnectionSecurity::Insecure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ConnectionSecurity::{CertificateError, Insecure, Secure};

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    /// Why: the lock icon is the one thing users rely on. Only the single fully confirmed
    /// combination may produce `Secure`.
    #[test]
    fn only_https_with_valid_trust_and_only_secure_content_is_secure() {
        let mut secure = 0;
        for scheme in ["https", "http", "about", "data"] {
            for trust in [Trust::Absent, Trust::Valid, Trust::Invalid] {
                for only_secure in [false, true] {
                    let url = if scheme == "about" {
                        u("about:blank")
                    } else if scheme == "data" {
                        u("data:text/plain,hi")
                    } else {
                        u(&format!("{scheme}://example.com/"))
                    };
                    if connection_security(&url, trust, only_secure) == Secure {
                        secure += 1;
                        assert_eq!((scheme, trust, only_secure), ("https", Trust::Valid, true));
                    }
                }
            }
        }
        assert_eq!(secure, 1);
    }

    #[test]
    fn plain_http_is_insecure_even_if_the_adapter_claims_trust() {
        // A confused or hostile caller must not be able to lift http to Secure.
        assert_eq!(
            connection_security(&u("http://example.com/"), Trust::Valid, true),
            Insecure
        );
    }

    #[test]
    fn mixed_content_downgrades_an_https_page_to_insecure() {
        assert_eq!(
            connection_security(&u("https://example.com/"), Trust::Valid, false),
            Insecure
        );
    }

    #[test]
    fn a_failed_certificate_is_reported_as_a_certificate_error() {
        for only_secure in [true, false] {
            assert_eq!(
                connection_security(&u("https://example.com/"), Trust::Invalid, only_secure),
                CertificateError
            );
        }
    }

    #[test]
    fn https_without_any_trust_information_is_not_secure() {
        assert_eq!(
            connection_security(&u("https://example.com/"), Trust::Absent, true),
            Insecure
        );
    }
}
