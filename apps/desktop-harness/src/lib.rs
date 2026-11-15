//! Wiring of `krypa-ui` to a real `SessionManager` and a fake engine, without any window
//! code. The `desktop-harness` binary adds eframe on top; keeping this part a library
//! lets it be tested headless.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod engine;
pub mod wiring;
