//! Krypa installed-app model.
//!
//! Parses and validates Web App Manifests. A manifest is attacker-controlled input, so
//! everything here degrades to a safe default instead of failing, and nothing here can
//! produce an app that points at another origin than the page it was installed from.
//! This crate does no network, storage or icon download.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
#![deny(missing_docs)]

mod app;
mod color;
mod error;
mod icon;
mod manifest;

pub use app::{sanitize_name, DisplayMode, InstalledApp, StoredApp, StoredIcon, MAX_NAME_CHARS};
pub use color::Rgba;
pub use error::{ManifestError, StoredAppError};
pub use icon::{Icon, IconPurpose, IconPurposes, IconSize};
pub use manifest::{Manifest, ManifestIcon, MAX_ICONS, MAX_MANIFEST_BYTES};
