//! Scheme policy shared by input classification and URL validation.

/// Schemes that must never be navigated to, handed to the OS, or treated as search text.
const BLOCKED: &[&str] = &[
    "javascript",
    "data",
    "file",
    "blob",
    "vbscript",
    "about",
    "view-source",
    "filesystem",
    "ftp",
    "ftps",
    "ws",
    "wss",
];

/// Well-known schemes that the OS handles; the user is asked before opening.
const EXTERNAL: &[&str] = &[
    "mailto",
    "tel",
    "sms",
    "facetime",
    "facetime-audio",
    "maps",
    "itms-apps",
    "itms-appss",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemeClass {
    /// `http` / `https`.
    Web,
    Blocked,
    /// Handed to the OS after asking the user.
    External,
    /// Not a scheme we recognise; the caller decides.
    Unknown,
}

/// Classifies a lower-case scheme. `has_authority` is whether `scheme://` follows,
/// which is how custom app links look.
pub(crate) fn classify(scheme: &str, has_authority: bool) -> SchemeClass {
    if scheme == "http" || scheme == "https" {
        SchemeClass::Web
    } else if BLOCKED.contains(&scheme) {
        SchemeClass::Blocked
    } else if EXTERNAL.contains(&scheme) || has_authority {
        SchemeClass::External
    } else {
        SchemeClass::Unknown
    }
}

/// Whether a scheme (any case) must never be navigated to, handed to the OS or shown as
/// something to open: the single list every layer shares.
#[must_use]
pub fn is_blocked_scheme(scheme: &str) -> bool {
    BLOCKED.contains(&scheme.to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_and_local_content_schemes_are_blocked_in_any_case() {
        for s in [
            "javascript",
            "JavaScript",
            "data",
            "file",
            "blob",
            "about",
            "vbscript",
        ] {
            assert!(is_blocked_scheme(s), "{s}");
        }
        for s in ["mailto", "tel", "sms", "itms-apps", "https", "myapp"] {
            assert!(!is_blocked_scheme(s), "{s}");
        }
    }
}
