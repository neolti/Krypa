//! Turns untrusted address-bar text and URLs into validated navigations.
//!
//! Everything that reaches this crate is hostile until proven otherwise. The
//! only way to obtain a [`NavigationUrl`] is through validation, so code that
//! accepts one can rely on: scheme is `https` or `http`, the host is non-empty,
//! there are no embedded credentials, and the text was length-capped.
//!
//! * [`classify_input`] decides what address-bar text means.
//! * [`NavigationUrl::parse`] validates a URL that is already known to be a URL.
//! * [`Origin::display_origin`] produces the string a security indicator shows.
//! * [`SearchConfig`] builds search URLs from query text.
//! * [`SecurityState`] is the data the engine reports about a connection.
//!
//! URLs are never included in `Display` output of errors, so error messages are
//! safe to log; `Debug` of [`NavigationError::ExternalScheme`] does carry the URL.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod error;
mod input;
mod navigation_url;
mod origin;
mod scheme;
mod search;
mod security;

pub use error::{ExternalLink, NavigationError, MAX_INPUT_BYTES};
pub use input::{classify_input, Input};
pub use navigation_url::NavigationUrl;
pub use origin::{DisplayOrigin, Origin, Scheme};
pub use scheme::is_blocked_scheme;
pub use search::SearchConfig;
pub use security::SecurityState;
