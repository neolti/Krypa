# ADR-0002: iOS shell with a hand-written C ABI, no UniFFI yet

Status: Accepted for the first iOS milestone (2026-11-15). Supersedes the FFI part of ADR-0001 (Decision 2, "UniFFI for the whole surface") for now; Decision 1 (UIKit owns the app, egui in a `CAMetalLayer` view) is kept and is what this milestone implements. Whether it works on a physical device is still unverified: the proof so far is the iOS Simulator in GitHub Actions (see `docs/IOS_STATUS.md`).

## Context

The owner has no Mac. The only build and run loop is GitHub Actions macOS runners (simulator only, no signing). ADR-0001 recommended UniFFI for the whole Swift/Rust surface and a hand-written C ABI only as a measured fallback.

## Decision

1. **Hand-written C ABI for this milestone.** Five functions (`krypa_view_create`, `_destroy`, `_resize`, `_touch`, `_render`) in `crates/krypa-ios/include/krypa_ios.h`. Why: the milestone's whole Swift-visible surface is five calls with numbers and one opaque pointer. UniFFI would add a code generator, a generated-Swift step and a Swift package to a CI loop where each iteration costs minutes, to wrap types that do not exist yet (`EngineCommand`/`EngineEvent` cross the boundary only when the WKWebView adapter is written). A Rust test keeps header and library in step (names and constant values).
2. **Revisit UniFFI when the engine adapter arrives.** ADR-0001's argument (typed command/event enums for Swift) is still right for that surface. The C ABI and UniFFI can coexist: the C ABI stays for the hot path (touch, render), exactly the split ADR-0001 kept as an escape hatch. This ADR only changes the order: hot path first.
3. **Panics: catch at every entry point, poison the handle; do not set `panic = "abort"`.** Unwinding into Swift is undefined behaviour, so each exported function runs inside `catch_unwind`. After a panic every later call on that handle returns `KRYPA_ERR_PANIC` and the shell stops drawing, so state a panic left half-updated is never used. Rejected: `panic = "abort"`, because it kills the whole app (later: the user's web sessions) for a bug in the drawing layer, and it is a profile setting for the entire workspace that also changes how `cargo test` behaves. UniFFI's own behaviour on panics remained undocumented; the question is moot until it is used.
4. **`unsafe` policy.** The workspace lint `unsafe_code = "forbid"` stays for every existing crate. `krypa-ios` does not inherit workspace lints (a `forbid` cannot be relaxed) and instead sets `unsafe_code = "deny"`, `unsafe_op_in_unsafe_fn = "deny"`, and clippy `undocumented_unsafe_blocks` and `missing_safety_doc` to deny, plus the same `unwrap_used`/`expect_used` denial. `#[allow(unsafe_code)]` appears on exactly two modules: `ffi` (the C ABI) and `gpu` (the raw layer pointer for wgpu). Every unsafe block carries a `SAFETY:` comment; every unsafe function has a `# Safety` section.
5. **Enums from C are integers.** A Rust `enum` parameter holding a value C invented is undefined behaviour, so touch phases and status codes are `u32`/`i32` constants validated on entry.
6. **Redraw on demand.** `krypa_view_render` writes when the next frame is needed (0 = next display-link tick, delay in ms, or `KRYPA_IDLE`); Swift pauses the `CADisplayLink` on idle and wakes it on touch, resize and timers. A static launcher draws nothing.
7. **Project generation: XcodeGen.** `apps/ios/project.yml` is committed; CI (and any Mac) runs `xcodegen generate`. No `.pbxproj` is committed. Minimum iOS 17.0 (IOS_INTEGRATION.md section 2). Bundle identifier `app.krypa.ios` is provisional. Scene-less `UIApplicationDelegate` for now; scene lifecycle comes with the WKWebView work.
8. **GPU surface from a layer pointer.** The Swift view's `layerClass` is `CAMetalLayer`; its pointer goes to `wgpu::Instance::create_surface_unsafe(SurfaceTargetUnsafe::CoreAnimationLayer(ptr))`. No `raw-window-handle` or winit is involved. The wgpu/egui-wgpu dependencies exist only for Apple targets, so Windows builds and tests stay light.
9. **Workspace members.** `apps/*` is replaced by `apps/desktop-harness` in the root `Cargo.toml`, because `apps/ios` is an Xcode project, not a Cargo package.

## Consequences

- Header and Rust constants are duplicated by hand; the sync test is the guard. cbindgen is not used (five functions).
- Rust library name `libkrypa_ios.a`; Xcode links it with `-lkrypa_ios` plus system frameworks (a Rust staticlib does not tell Xcode which frameworks it needs; the list is in `project.yml`).
- No session, navigation, storage or text input crosses the ABI yet. They are the next steps, not half-built seams.
