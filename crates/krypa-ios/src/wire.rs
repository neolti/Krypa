//! What crosses the boundary, as bytes: UTF-8 JSON in both directions (ADR-0003).
//!
//! - Rust -> Swift: an [`EngineCommand`] serialised as it is (the contract type, nothing
//!   new to keep in step).
//! - Swift -> Rust: a [`HostEvent`], a deliberately narrower type than `EngineEvent`.
//!   Swift can only say what `WKWebView` knows. It cannot send `SecurityChanged`,
//!   `ManifestFetched` or `PermissionRequested`: the security verdict is computed here from
//!   facts ([`crate::security`]), and the other two are not implemented on iOS yet.
//!
//! Everything arriving here is untrusted input: size-capped, UTF-8 checked, parsed with
//! unknown fields refused, enum values checked by serde, numbers range-checked.

use krypa_engine_api::{EngineCommand, EngineEvent, LoadErrorKind, NavigationPhase, SessionId};
use serde::Deserialize;
use url::Url;

use crate::security::{connection_security, Trust};

/// Largest event accepted, in bytes. A URL of 8 KiB is already unusual; 32 KiB leaves room
/// for the title and the JSON around it and still bounds what hostile pages can make Swift
/// send.
pub const MAX_EVENT_BYTES: usize = 32 * 1024;
/// Longest error-domain string accepted.
const MAX_DOMAIN_BYTES: usize = 64;

/// Why a byte string was not accepted as an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    /// Empty or longer than [`MAX_EVENT_BYTES`].
    BadLength,
    /// Not valid UTF-8.
    NotUtf8,
    /// Not valid JSON for a [`HostEvent`] (unknown variant or field, bad enum value, bad
    /// URL, wrong type).
    Malformed,
    /// Valid shape, impossible value (progress above 1000, oversized domain).
    OutOfRange,
}

/// Facts the Swift adapter reports. Session ids are the core's; the core rejects the ones
/// it does not know.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum HostEvent {
    /// A provisional navigation of the main frame started.
    NavigationStarted {
        /// Source session.
        session: SessionId,
        /// URL being loaded.
        url: Url,
        /// Back is possible.
        can_go_back: bool,
        /// Forward is possible.
        can_go_forward: bool,
    },
    /// The main frame committed a new document. One event carries both the commit and the
    /// facts about its connection, so a commit without a security report cannot happen.
    DocumentCommitted {
        /// Source session.
        session: SessionId,
        /// URL of the committed document.
        url: Url,
        /// Back is possible.
        can_go_back: bool,
        /// Forward is possible.
        can_go_forward: bool,
        /// Server trust of the main frame.
        trust: Trust,
        /// `WKWebView.hasOnlySecureContent`.
        only_secure_content: bool,
    },
    /// The load finished.
    NavigationFinished {
        /// Source session.
        session: SessionId,
        /// URL of the document.
        url: Url,
        /// Back is possible.
        can_go_back: bool,
        /// Forward is possible.
        can_go_forward: bool,
    },
    /// The URL or the history flags of the committed document changed without a new
    /// document (`pushState`, fragment navigation). Reported as `Finished`: the contract has
    /// no same-document phase and `Finished` leaves the security report alone.
    LocationChanged {
        /// Source session.
        session: SessionId,
        /// New URL.
        url: Url,
        /// Back is possible.
        can_go_back: bool,
        /// Forward is possible.
        can_go_forward: bool,
    },
    /// The facts about the committed document's connection changed (mixed content loaded
    /// after the commit). The URL is the one the core already knows for the session.
    SecurityFacts {
        /// Source session.
        session: SessionId,
        /// Server trust of the main frame.
        trust: Trust,
        /// `WKWebView.hasOnlySecureContent`.
        only_secure_content: bool,
    },
    /// Load progress in per mille.
    Progress {
        /// Source session.
        session: SessionId,
        /// 0..=1000.
        permille: u16,
    },
    /// Page title (hostile text; the core sanitizes it).
    Title {
        /// Source session.
        session: SessionId,
        /// New title.
        title: String,
    },
    /// A load failed. Raw `NSError` facts; the category is decided here.
    LoadFailed {
        /// Source session.
        session: SessionId,
        /// `NSError.domain`.
        domain: String,
        /// `NSError.code`.
        code: i32,
    },
    /// `webViewWebContentProcessDidTerminate`.
    ContentProcessTerminated {
        /// Source session.
        session: SessionId,
    },
    /// The web view is gone (acknowledges `DestroySession`).
    SessionDestroyed {
        /// Source session.
        session: SessionId,
    },
}

impl HostEvent {
    /// Parses and validates the bytes Swift sent.
    ///
    /// # Errors
    /// See [`WireError`].
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.is_empty() || bytes.len() > MAX_EVENT_BYTES {
            return Err(WireError::BadLength);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| WireError::NotUtf8)?;
        let event: Self = serde_json::from_str(text).map_err(|_| WireError::Malformed)?;
        match &event {
            Self::Progress { permille, .. } if *permille > 1000 => Err(WireError::OutOfRange),
            Self::LoadFailed { domain, .. } if domain.len() > MAX_DOMAIN_BYTES => {
                Err(WireError::OutOfRange)
            }
            _ => Ok(event),
        }
    }

    /// The session the event is about.
    #[must_use]
    pub const fn session(&self) -> SessionId {
        match self {
            Self::NavigationStarted { session, .. }
            | Self::DocumentCommitted { session, .. }
            | Self::NavigationFinished { session, .. }
            | Self::LocationChanged { session, .. }
            | Self::SecurityFacts { session, .. }
            | Self::Progress { session, .. }
            | Self::Title { session, .. }
            | Self::LoadFailed { session, .. }
            | Self::ContentProcessTerminated { session }
            | Self::SessionDestroyed { session } => *session,
        }
    }

    /// The contract events this fact turns into, in order. `known_url` is the URL the core
    /// holds for the session; only [`HostEvent::SecurityFacts`] needs it (no URL: the page
    /// cannot be confirmed, so the report is `Insecure`).
    #[must_use]
    pub fn into_engine_events(self, known_url: Option<&Url>) -> Vec<EngineEvent> {
        match self {
            Self::NavigationStarted {
                session,
                url,
                can_go_back,
                can_go_forward,
            } => vec![nav(
                session,
                url,
                NavigationPhase::Started,
                can_go_back,
                can_go_forward,
            )],
            Self::DocumentCommitted {
                session,
                url,
                can_go_back,
                can_go_forward,
                trust,
                only_secure_content,
            } => {
                let security = connection_security(&url, trust, only_secure_content);
                vec![
                    nav(
                        session,
                        url,
                        NavigationPhase::Committed,
                        can_go_back,
                        can_go_forward,
                    ),
                    EngineEvent::SecurityChanged { session, security },
                ]
            }
            Self::NavigationFinished {
                session,
                url,
                can_go_back,
                can_go_forward,
            }
            | Self::LocationChanged {
                session,
                url,
                can_go_back,
                can_go_forward,
            } => vec![nav(
                session,
                url,
                NavigationPhase::Finished,
                can_go_back,
                can_go_forward,
            )],
            Self::SecurityFacts {
                session,
                trust,
                only_secure_content,
            } => {
                let security = known_url
                    .map_or(krypa_engine_api::ConnectionSecurity::Insecure, |u| {
                        connection_security(u, trust, only_secure_content)
                    });
                vec![EngineEvent::SecurityChanged { session, security }]
            }
            Self::Progress { session, permille } => {
                vec![EngineEvent::ProgressChanged { session, permille }]
            }
            Self::Title { session, title } => vec![EngineEvent::TitleChanged { session, title }],
            Self::LoadFailed {
                session,
                domain,
                code,
            } => vec![EngineEvent::LoadFailed {
                session,
                kind: load_error_kind(&domain, code),
                code,
            }],
            Self::ContentProcessTerminated { session } => {
                vec![EngineEvent::ContentProcessTerminated { session }]
            }
            Self::SessionDestroyed { session } => vec![EngineEvent::SessionDestroyed { session }],
        }
    }
}

fn nav(
    session: SessionId,
    url: Url,
    phase: NavigationPhase,
    can_go_back: bool,
    can_go_forward: bool,
) -> EngineEvent {
    EngineEvent::NavigationChanged {
        session,
        url,
        phase,
        can_go_back,
        can_go_forward,
    }
}

/// Category of a failed load from an `NSError` (domain and code; the message is never used
/// because it can contain page data). Codes are the documented `NSURLError` values.
#[must_use]
pub fn load_error_kind(domain: &str, code: i32) -> LoadErrorKind {
    match (domain, code) {
        // NSURLErrorCancelled; WebKitErrorDomain 102 is "frame load interrupted by policy
        // change", which is what a navigation we cancelled ourselves looks like.
        ("NSURLErrorDomain", -999) | ("WebKitErrorDomain", 102) => LoadErrorKind::Cancelled,
        // NSURLErrorSecureConnectionFailed .. NSURLErrorClientCertificateRequired:
        // -1200 secure connection failed, -1201 bad date, -1202 untrusted, -1203 unknown
        // root, -1204 not yet valid, -1205/-1206 client certificate.
        ("NSURLErrorDomain", -1206..=-1200) => LoadErrorKind::Tls,
        // errSSL* from Security.framework (-9800 ..= -9899).
        ("NSOSStatusErrorDomain", -9899..=-9800) => LoadErrorKind::Tls,
        // -1022: App Transport Security refused a cleartext load (a policy block, not a fault).
        ("NSURLErrorDomain", -1022) => LoadErrorKind::Blocked,
        // Timeout, bad URL, unsupported URL, cannot find host, cannot connect, connection
        // lost, DNS failure, not connected, international roaming off, call active, data not
        // allowed.
        (
            "NSURLErrorDomain",
            -1001 | -1000 | -1002 | -1003 | -1004 | -1005 | -1006 | -1009 | -1018 | -1019 | -1020,
        ) => LoadErrorKind::Network,
        _ => LoadErrorKind::Other,
    }
}

/// An [`EngineCommand`] as the UTF-8 JSON Swift reads.
///
/// # Errors
/// Only if serialisation fails, which plain contract types cannot do; the error is returned
/// instead of panicking because this runs behind the C ABI.
pub fn command_to_json(command: &EngineCommand) -> Result<String, serde_json::Error> {
    serde_json::to_string(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use krypa_engine_api::ConnectionSecurity;

    fn parse(s: &str) -> Result<HostEvent, WireError> {
        HostEvent::parse(s.as_bytes())
    }

    #[test]
    fn a_committed_document_always_yields_a_security_report_after_the_commit() {
        // Why: the core treats a commit without a report as "Not verified"; one event that
        // carries both facts makes the omission impossible for the adapter.
        let ev = parse(
            r#"{"DocumentCommitted":{"session":3,"url":"https://example.com/","can_go_back":false,"can_go_forward":false,"trust":"Valid","only_secure_content":true}}"#,
        )
        .unwrap();
        let out = ev.into_engine_events(None);
        assert_eq!(out.len(), 2);
        assert!(matches!(
            out[0],
            EngineEvent::NavigationChanged {
                phase: NavigationPhase::Committed,
                ..
            }
        ));
        assert_eq!(
            out[1],
            EngineEvent::SecurityChanged {
                session: SessionId::new(3),
                security: ConnectionSecurity::Secure
            }
        );
    }

    #[test]
    fn swift_cannot_assert_security_directly() {
        // The verdict is Rust's; an adapter bug or hostile bridge must not be able to say Secure.
        for raw in [
            r#"{"SecurityChanged":{"session":1,"security":"Secure"}}"#,
            r#"{"ManifestFetched":{"session":1,"request":1,"result":"NotFound"}}"#,
            r#"{"PermissionRequested":{"session":1,"request":1,"kind":"Camera","origin":"https://a.example/"}}"#,
        ] {
            assert_eq!(parse(raw), Err(WireError::Malformed), "{raw}");
        }
    }

    #[test]
    fn plain_http_commits_as_insecure_whatever_the_adapter_claims() {
        let ev = parse(
            r#"{"DocumentCommitted":{"session":1,"url":"http://127.0.0.1:8080/","can_go_back":false,"can_go_forward":false,"trust":"Valid","only_secure_content":true}}"#,
        )
        .unwrap();
        assert_eq!(
            ev.into_engine_events(None)[1],
            EngineEvent::SecurityChanged {
                session: SessionId::new(1),
                security: ConnectionSecurity::Insecure
            }
        );
    }

    #[test]
    fn later_security_facts_use_the_url_the_core_knows_and_fail_closed_without_one() {
        let ev = || HostEvent::SecurityFacts {
            session: SessionId::new(1),
            trust: Trust::Valid,
            only_secure_content: false,
        };
        let known = Url::parse("https://example.com/").unwrap();
        assert_eq!(
            ev().into_engine_events(Some(&known)),
            vec![EngineEvent::SecurityChanged {
                session: SessionId::new(1),
                security: ConnectionSecurity::Insecure
            }]
        );
        assert_eq!(
            ev().into_engine_events(None),
            vec![EngineEvent::SecurityChanged {
                session: SessionId::new(1),
                security: ConnectionSecurity::Insecure
            }]
        );
    }

    #[test]
    fn hostile_bytes_are_rejected_without_panicking() {
        assert_eq!(HostEvent::parse(b""), Err(WireError::BadLength));
        assert_eq!(
            HostEvent::parse(&vec![b'a'; MAX_EVENT_BYTES + 1]),
            Err(WireError::BadLength)
        );
        assert_eq!(
            HostEvent::parse(&[0xFF, 0xFE, b'{']),
            Err(WireError::NotUtf8)
        );
        for raw in [
            "null",
            "[]",
            "{",
            r#"{"Progress":{"session":1}}"#,
            r#"{"Progress":{"session":-1,"permille":5}}"#,
            r#"{"Progress":{"session":1,"permille":70000}}"#,
            r#"{"Progress":{"session":1,"permille":5,"extra":1}}"#,
            r#"{"Nope":{"session":1}}"#,
            r#"{"NavigationStarted":{"session":1,"url":"not a url","can_go_back":false,"can_go_forward":false}}"#,
            r#"{"SecurityFacts":{"session":1,"trust":"Maybe","only_secure_content":true}}"#,
        ] {
            assert_eq!(parse(raw), Err(WireError::Malformed), "{raw}");
        }
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        assert_eq!(
            parse(r#"{"Progress":{"session":1,"permille":1001}}"#),
            Err(WireError::OutOfRange)
        );
        let long = "x".repeat(MAX_DOMAIN_BYTES + 1);
        assert_eq!(
            parse(&format!(
                r#"{{"LoadFailed":{{"session":1,"domain":"{long}","code":1}}}}"#
            )),
            Err(WireError::OutOfRange)
        );
        assert!(parse(r#"{"Progress":{"session":1,"permille":1000}}"#).is_ok());
    }

    /// Why: a certificate failure must reach the UI as `Tls` (error view, "certificate"
    /// indicator), and a navigation we cancelled ourselves must not look like a failure.
    #[test]
    fn ns_errors_map_to_the_right_load_error_kind() {
        use LoadErrorKind::{Blocked, Cancelled, Network, Other, Tls};
        let cases = [
            ("NSURLErrorDomain", -999, Cancelled),
            ("WebKitErrorDomain", 102, Cancelled),
            ("NSURLErrorDomain", -1200, Tls),
            ("NSURLErrorDomain", -1201, Tls),
            ("NSURLErrorDomain", -1202, Tls),
            ("NSURLErrorDomain", -1203, Tls),
            ("NSURLErrorDomain", -1204, Tls),
            ("NSURLErrorDomain", -1206, Tls),
            ("NSOSStatusErrorDomain", -9807, Tls),
            ("NSURLErrorDomain", -1022, Blocked),
            ("NSURLErrorDomain", -1001, Network),
            ("NSURLErrorDomain", -1003, Network),
            ("NSURLErrorDomain", -1004, Network),
            ("NSURLErrorDomain", -1009, Network),
            ("NSURLErrorDomain", -1100, Other),
            ("WebKitErrorDomain", 101, Other),
            // Same number, wrong domain: never trusted as a TLS error.
            ("SomeOtherDomain", -1202, Other),
            ("NSURLErrorDomain", 0, Other),
            ("", i32::MIN, Other),
        ];
        for (domain, code, want) in cases {
            assert_eq!(load_error_kind(domain, code), want, "{domain} {code}");
        }
    }

    #[test]
    fn commands_serialise_to_the_contract_shape_swift_reads() {
        let cmd = EngineCommand::Load {
            session: SessionId::new(5),
            url: Url::parse("https://example.com/a").unwrap(),
        };
        let json = command_to_json(&cmd).unwrap();
        assert_eq!(
            json,
            r#"{"Load":{"session":5,"url":"https://example.com/a"}}"#
        );
        assert_eq!(serde_json::from_str::<EngineCommand>(&json).unwrap(), cmd);

        let create = EngineCommand::CreateSession {
            session: SessionId::new(1),
            profile: krypa_engine_api::ProfileKind::Default,
            initial_url: Some(Url::parse("http://127.0.0.1:8080/").unwrap()),
        };
        let json = command_to_json(&create).unwrap();
        assert_eq!(
            json,
            r#"{"CreateSession":{"session":1,"profile":"Default","initial_url":"http://127.0.0.1:8080/"}}"#
        );
    }

    #[test]
    fn every_event_names_its_session() {
        let ev = parse(r#"{"ContentProcessTerminated":{"session":9}}"#).unwrap();
        assert_eq!(ev.session(), SessionId::new(9));
        assert_eq!(
            ev.into_engine_events(None),
            vec![EngineEvent::ContentProcessTerminated {
                session: SessionId::new(9)
            }]
        );
    }
}
