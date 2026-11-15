# ADR-0001: Integration model and FFI

Status: Proposed (Phase 0, 2026-11-13). Nothing here is device-verified; Phase 1 (WP-6) confirms or reverses it.
Marking: **V-doc** (primary source linked) / **U** (inference). Details and links: [IOS_INTEGRATION.md](../IOS_INTEGRATION.md), [ARCHITECTURE.md](../ARCHITECTURE.md).

## Context

Krypa draws browser UI with egui + wgpu in Rust and renders web content with WKWebView on iOS. App Store distribution and public APIs only are hard requirements. The dev machine is Windows; iOS code cannot be built here.

## Decision 1: UIKit owns the app; egui runs inside a UIKit view, not under winit

- A Swift `UIView` subclass overrides `layerClass` to return `CAMetalLayer` (V-doc [layerClass](https://developer.apple.com/tutorials/data/documentation/uikit/uiview/layerclass.json), [CAMetalLayer](https://developer.apple.com/tutorials/data/documentation/quartzcore/cametallayer.json)).
- Swift passes the layer pointer to Rust once; Rust creates the surface with `Instance::create_surface_unsafe(SurfaceTargetUnsafe::CoreAnimationLayer(ptr))` (V-doc, wgpu 30.0.2, [surface.rs](https://raw.githubusercontent.com/gfx-rs/wgpu/v30.0.2/wgpu/src/api/surface.rs)). Precedent for the pattern: [app-surface](https://docs.rs/crate/app-surface/1.11.0/source/src/ios.rs) (V-doc).
- A `CADisplayLink` drives `frame()`; egui is run directly (`Context::run_ui`, `tessellate`, `egui_wgpu::Renderer`) (V-doc signatures, [Context](https://docs.rs/egui/0.37.0/egui/struct.Context.html), [Renderer](https://docs.rs/egui-wgpu/0.37.0/egui_wgpu/struct.Renderer.html)). Redraw on demand via `request_repaint_after` and the `Send + Sync` repaint callback.
- `WKWebView` is a sibling `UIView`; the Rust view overrides `hitTest(_:with:)` to return `nil` outside browser-owned rects, which UIKit documents as fall-through (V-doc [hitTest](https://developer.apple.com/tutorials/data/documentation/uikit/uiview/hittest(_:with:).json)).
- To limit compositing cost, the Rust view's frame shrinks to the chrome region while no sheet is open (U; measured in Phase 1).
- The Rust view conforms to `UITextInput` for the address bar (V-doc protocol requirements, [UITextInput](https://developer.apple.com/tutorials/data/documentation/uikit/uitextinput.json)); AccessKit's `accesskit_ios::Adapter` is hosted on it (V-doc API, [docs.rs](https://docs.rs/accesskit_ios/0.2.2/accesskit_ios/)).

### Alternatives rejected

| Alternative | Why rejected |
|---|---|
| **eframe / egui-winit / winit owning the app on iOS** | egui-winit does handle iOS safe-area and touch-to-pointer (V-doc [source](https://raw.githubusercontent.com/emilk/egui/main/crates/egui-winit/src/lib.rs)), so this is the fastest PoC path and stays the fallback. Rejected as the target because winit would own the app lifecycle and the view, while Krypa must control scene lifecycle, add a sibling WKWebView, implement full `UITextInput` and host an accessibility adapter on that view. Whether winit on iOS can expose a usable keyboard/IME surface is U (not researched); egui is on winit 0.30.13 (V-doc, egui-wgpu deps) and a winit 0.31 upgrade is a draft (V-doc, [egui #7731](https://github.com/emilk/egui/issues/7731)). |
| **Render web content into an egui/wgpu texture (snapshots or frame capture)** | No public API supplies live WKWebView frames as a GPU texture (U, negative search); `takeSnapshot` is for images, not a display path (V-doc). Would also break input, scrolling, accessibility and video. |
| **Native UIKit/SwiftUI chrome with Rust only for core logic** | Valid and lower risk (text input, VoiceOver and keyboard come free), but gives up the product premise of Rust-native UI. Kept as a tripwire fallback: if the IME or VoiceOver spikes fail (see RISKS.md R3, R4), replace only the failing controls (address bar, buttons) with native views over the egui view. |
| **Permanent full-screen transparent Metal overlay with hit-test passthrough** | Works per documentation but costs a full-screen blended layer and complicates accessibility ordering (U). Used only while a sheet or popup is open. |

## Decision 2: FFI is UniFFI (proc-macro), one binding surface

Chosen: **UniFFI 0.32.x** ([crates.io](https://crates.io/crates/uniffi), 0.32.2 on 2026-09-23) in proc-macro mode, for both the engine command/event API and the view API (`attach_surface`, `resize`, `frame`, `push_touch`, `push_key`, `push_ime`, `set_safe_area`, `detach_surface`, `suspend`, `resume`). The raw `CAMetalLayer` pointer is passed as `u64`.

Why:
- **Typed commands and events.** `EngineCommand` and `EngineEvent` are enums with data; UniFFI generates typed Swift (V-doc [overview](https://mozilla.github.io/uniffi-rs/latest/): Kotlin, Swift and Python have full support). This also gives Kotlin bindings for a later Android host (same page).
- **Swift implements Rust traits.** The platform adapter can be a Swift class implementing a Rust trait. Callback interfaces are "(soft) deprecated" in favour of foreign traits (V-doc [callback interfaces](https://mozilla.github.io/uniffi-rs/latest/types/callback_interfaces.html)). Foreign implementations must be thread-safe themselves (same page). The foreign-traits page returned 404 when fetched, so its threading rules are U.
- **Ownership is clear.** Rust objects live behind `Arc`; Swift holds a reference-counted proxy; foreign destructors drop only their reference (V-doc [interfaces](https://mozilla.github.io/uniffi-rs/latest/types/interfaces.html)). Rust types must be `Send + Sync`, `&mut self` methods are not allowed, interior mutability must be thread-safe (same page). So the Rust handle wraps its state in a mutex; "main-thread only" is enforced by a Swift `@MainActor` facade (U).
- **No async across FFI is needed.** Command/event is callback-based. UniFFI does not support cancellation directly (V-doc [futures](https://mozilla.github.io/uniffi-rs/latest/futures.html)) and async Swift code is not yet `Sendable` (V-doc [Swift overview](https://mozilla.github.io/uniffi-rs/latest/swift/overview.html), issue #2448). Avoiding async avoids both gaps.
- **Smallest hand-written unsafe.** One `unsafe` pointer conversion in `krypa-ios-ffi`.

Costs accepted:
- UniFFI does not package anything: "does not help you ship the library" (V-doc overview). Krypa writes the static-lib and xcframework build script (U).
- Panic behaviour across the boundary is undocumented in the pages read (U). The first spike must test it and record the result here.
- Per-call marshalling overhead for input at 120 Hz is unmeasured (U).

### Alternatives rejected

| Alternative | Why rejected |
|---|---|
| **Hand-written C ABI (+ cbindgen) for everything** | Maximum control and smallest overhead, but `EngineCommand`/`EngineEvent` would need a manual wire format and Swift decoders; more code to keep consistent on each protocol change. Retained as an **escape hatch for the hot path only** (`push_touch`, `frame`) if measurement on a physical iPhone shows UniFFI overhead matters (U; trigger defined in Phase 1). This replaces PLAN.md section 1 item 5's "two surfaces by default". |
| **swift-bridge** | Version 0.1.59 (V-doc [crates.io](https://crates.io/crates/swift-bridge)). README lists `Arc<T>`, `Box<dyn Fn>`, and Swift-to-Rust `FnOnce` closures as not implemented (V-doc [README](https://github.com/chinedufn/swift-bridge)); Krypa needs Swift-implemented callbacks into Rust. Async exists, but there is no Kotlin path. |
| **UniFFI for commands, C ABI for hot path from day one** (PLAN.md item 5) | Two toolchains and two sets of lifetime rules before any evidence the second is needed. Reversible later because the hot-path surface is ten methods. |
| **Other FFI generators (Diplomat, etc.)** | Not evaluated. |

## Consequences

- New crate `krypa-ios-ffi` under `platforms/ios` (PLAN.md section 2 lacks it); only crate allowed `unsafe`, with `#![deny(unsafe_op_in_unsafe_fn)]` (U convention).
- Minimum iOS 17 for profiles (V-doc, `init(forIdentifier:)`).
- Phase 1 must record: UniFFI panic behaviour, per-call cost, IME composition, VoiceOver on chrome, hit-test latency, transparency cost, background/foreground surface survival. If two or more of IME, VoiceOver and overlay performance fail, revisit Decision 1 using the native-chrome fallback.
- wgpu API churns every major (v29 and v30 changed surface and instance creation, V-doc [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md)); pin exact versions.
