//! Krypa core: the session lifecycle state machine and the session manager that decides
//! which web views stay alive. Platform-free, async-free and deterministic: events and
//! user intents go in, engine commands and UI notices come out.
//!
//! `policy` is deliberately separate: a pure synchronous function for the web view's
//! navigation / new-window delegate callbacks, which must be answered immediately.
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod error;
mod lifecycle;
mod manager;
mod policy;

#[cfg(test)]
mod manager_tests;

pub use error::CoreError;
pub use lifecycle::{IllegalTransition, SessionState, Trigger};
pub use manager::{
    FailureReason, InstallSource, ManagerConfig, ManualReason, MemoryPressure, Notice, Outcome,
    PendingExternalOpen, SessionInfo, SessionManager, SessionSpec,
};
pub use policy::{
    decide_navigation, is_in_scope, is_loadable, CancelReason, NavigationDecision, PolicyContext,
};
