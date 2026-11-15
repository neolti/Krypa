use thiserror::Error;

/// Why a manifest could not be turned into an installed app.
///
/// Only structural problems are errors. A bad individual field (wrong type, hostile
/// value) is dropped or replaced by its default instead.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ManifestError {
    /// The manifest text is larger than the size cap.
    #[error("manifest is {len} bytes, the limit is {max}")]
    TooLarge {
        /// Size of the rejected input in bytes.
        len: usize,
        /// The cap that was exceeded.
        max: usize,
    },
    /// The manifest is not valid JSON (or is nested too deeply).
    #[error("manifest is not valid JSON: {0}")]
    InvalidJson(String),
    /// The manifest is valid JSON but not a JSON object.
    #[error("manifest is not a JSON object")]
    NotAnObject,
    /// The document URL is not an `http(s)` URL with a host and no credentials.
    #[error("document URL must be http(s) with a host and no credentials")]
    UnsupportedDocumentUrl,
    /// The manifest URL is not an `http(s)` URL with a host and no credentials.
    #[error("manifest URL must be http(s) with a host and no credentials")]
    UnsupportedManifestUrl,
}

/// Why stored app fields could not be turned back into an installed app: the named field
/// violates an invariant of [`InstalledApp`](crate::InstalledApp). Carries no field
/// value, so it is safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("stored app is invalid: {field}")]
pub struct StoredAppError {
    /// The field that failed: `id`, `name`, `start_url`, `scope`, `icons`.
    pub field: &'static str,
}
