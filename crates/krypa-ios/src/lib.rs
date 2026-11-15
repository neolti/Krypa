//! Krypa for iOS: a static library with a tiny C ABI (see `include/krypa_ios.h`).
//!
//! The Swift shell owns the app, the run loop and a `CAMetalLayer`-backed view. It hands the
//! layer to this library, forwards touches and size changes, and calls `render` once per
//! display-link tick. Rust runs `krypa-ui` (egui) and draws it with wgpu.
//!
//! Unsafe code is allowed in exactly two modules: `ffi` (the C ABI) and `gpu` (the raw
//! layer pointer). Every unsafe block states why it is sound. The rest of the crate is
//! plain Rust, compiled and tested on any host.
#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod app;
pub mod geometry;
pub mod input;
pub mod view;

#[allow(unsafe_code)]
pub mod ffi;
#[cfg(target_vendor = "apple")]
#[allow(unsafe_code)]
mod gpu;
