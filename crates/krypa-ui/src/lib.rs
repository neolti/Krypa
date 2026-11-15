//! Krypa's own interface: design tokens, components and the three screens (launcher,
//! browser chrome, app switcher), drawn with egui only.
//!
//! The crate depends on `egui` and nothing else of the egui family (no eframe, winit or
//! wgpu), because on iOS it is driven by a native shell. The shell calls
//! [`KrypaUi::frame`] inside an egui pass with an [`AppState`] it keeps in sync with the
//! `SessionManager`, then acts on the returned [`FrameOutput`]: intents to execute, the
//! rectangles that belong to this UI (touch routing), and where the web view goes.
//!
//! Text that comes from the web is hostile; see [`text::display_text`].
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod app_icons;
pub mod components;
pub mod icons;
mod screens;
mod state;
pub mod strings;
pub mod text;
pub mod theme;

#[cfg(test)]
mod tests;

pub use screens::KrypaUi;
pub use state::{
    display_origin_of, security_for, session_url, AppState, FrameOutput, InstallPrompt, Intent,
    PermissionPrompt, RecentEntry, Screen,
};
pub use theme::ThemeChoice;
