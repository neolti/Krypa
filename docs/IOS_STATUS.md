# iOS status (2026-11-24)

First code on Apple platforms. Evidence: GitHub Actions, macOS 15 runner (Xcode 16.4 default), iOS 18.x Simulator, no physical device. Nothing here is verified on a device.

![Krypa launcher in the iOS Simulator](ios-simulator-launcher.png)

## What works (proven by CI run 43741085386)

- The whole workspace formats, lints (clippy `-D warnings`) and passes its tests on macOS, and the platform-neutral crates build for `aarch64-apple-ios-sim` and `aarch64-apple-ios` (SQLite bundled builds fine for iOS).
- `crates/krypa-ios` (C ABI static library) and `apps/ios` (Swift/UIKit shell, XcodeGen) build, boot in the Simulator and draw the real `krypa-ui` launcher through wgpu/Metal. The CI job fails if the app is not alive 10 s after launch or the Rust side logs a failure.
- Design: [ADR-0002](adr/0002-ios-shell-c-abi.md).

## Web content in a WKWebView (proven by CI run 45286821657, branch `ios-webview`)

A `WKWebView` per session sits above the Metal view in the content rectangle Rust reports; commands and events cross the C ABI as JSON ([ADR-0003](adr/0003-webview-adapter-boundary.md)). Simulator screenshots from the run:

| Case | Screenshot | What it shows |
|---|---|---|
| `https://example.com/` | ![https](ios-webview-https.png) | Page content between the top bar and the bottom bar, "Secure connection" in green, origin `example.com`. |
| self-signed https on 127.0.0.1 | ![cert](ios-webview-cert-error.png) | "Certificate problem" in red, Krypa's own "This page did not load" view, no page content. |

A plain http page shows "Not secure" with the fixture page below the bar (screenshot only in the CI artifact: 308 KB, over the size limit for this folder).

Not verified or not done: rotation (no way to rotate in `simctl`; the web view follows the rect every frame by construction), text input, downloads, permissions (camera, microphone and motion are denied by default), find in page, private mode, manifest fetch ("Add to Krypa" is not offered: `manifest_fetch` is false), Web Inspector only in Debug. Known cosmetic problem in `krypa-ui` (not changed here): the error view's body sentence touches both screen edges at 402 pt width.

## Corrections to the Phase 0 documents (CI showed otherwise)

- wgpu 30.0.2 `request_device` with default limits **fails on the Simulator** (`max_inter_stage_shader_variables` 15 < 16). Krypa requests the adapter's own limits.
- `RequestAdapterOptions` has a new field `apply_limit_buckets`; `TexturesDelta` must be consumed or egui panics on drop. `CurrentSurfaceTexture`, `Queue::present` and `InstanceDescriptor::new_without_display_handle` are as the docs said.
- A Rust staticlib does not carry its framework dependencies into Xcode: the app links `Metal`, `QuartzCore`, `CoreGraphics`, `Foundation`, `UIKit` and `libc++` by hand (`apps/ios/project.yml`). That list linked successfully.
- `CAMetalLayer` pointer to `create_surface_unsafe(CoreAnimationLayer)` works; no raw-window-handle or winit.
- Panic policy changed from "undecided" to catch-and-poison (ADR-0002).

## Not done (next steps, in order)

1. Software keyboard / text input (`UITextInput`) so the address field is usable; taps are forwarded but untested end to end (no tap tool on the runner and XCUITest cannot see egui widgets without the accessibility bridge).
2. Storage location on device, navigation, accessibility (AccessKit), dark/light system theme, scene lifecycle and background/foreground surface handling, physical-device run (needs signing: owner decision).

Known issue: the display link sleeps when idle but the full-screen layer is opaque and redraws on every touch move; battery has not been measured.
