# iOS integration (Phase 0 research)

Date of research: 2026-11-13. Author: WP-R. Nothing here was built or run on a device or simulator.

Marking: **V-doc** = stated by the primary source linked next to it. **U** = assumption or inference, not confirmed. No claim is device-verified (V-device); the Phase 1 exit criteria in PLAN.md section 5 are what will flip items to V-device.

Where a source is a crates.io metadata page, the version and date come from the crates.io API response on 2026-11-13.

## 1. Versions in play

| Component | Version | Date | Mark |
|---|---|---|---|
| egui, egui-wgpu | 0.37.0, `rust_version` 1.95 | 2026-10-15 | V-doc [egui](https://crates.io/crates/egui), [egui-wgpu](https://crates.io/crates/egui-wgpu) |
| wgpu | 30.0.2 | 2026-11-12 | V-doc [wgpu](https://crates.io/crates/wgpu) |
| egui-wgpu dependency | `wgpu ^30.0`; `winit ^0.30.13` is optional | | V-doc [deps](https://crates.io/api/v1/crates/egui-wgpu/0.37.0/dependencies) |
| accesskit | 0.25.2 | 2026-10-25 | V-doc [accesskit](https://crates.io/crates/accesskit) |
| accesskit_ios | 0.2.2 (crate created 2026-05-11) | updated 2026-10-25 | V-doc [accesskit_ios](https://crates.io/crates/accesskit_ios) |
| uniffi | 0.32.2 | 2026-09-23 | V-doc [uniffi](https://crates.io/crates/uniffi) |
| swift-bridge | 0.1.59 | 2026-01-06 | V-doc [swift-bridge](https://crates.io/crates/swift-bridge) |
| App Review Guidelines | last update 2026-10-16 | | V-doc [Apple news](https://developer.apple.com/news/?id=guidelines) |

Consequence: the Rust toolchain must be at least 1.95 (V-doc, from egui 0.37.0 `rust_version`). Whether egui 0.37 and accesskit_ios 0.2.2 depend on the same `accesskit` major/minor is **U**; `cargo tree -d` must be run once the workspace exists.

## 2. Recommended minimum iOS version: 17.0

| API | Minimum iOS | Mark |
|---|---|---|
| `WKWebsiteDataStore.init(forIdentifier:)` | 17.0 | V-doc [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkwebsitedatastore/init(foridentifier:).json) |
| `WKWebView.isInspectable` | 16.4 | V-doc [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkwebview/isinspectable.json) |
| `WKWebView.interactionState` | 15.0 | V-doc [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkwebview/interactionstate.json) |
| `evaluateJavaScript(_:in:contentWorld:)` | 15.0 | V-doc [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkwebview/evaluatejavascript(_:in:contentworld:).json) |
| `WKContentWorld` | 14.0 | V-doc [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkcontentworld.json) |
| `CADisplayLink.preferredFrameRateRange` | 15.0 | V-doc [Apple](https://developer.apple.com/tutorials/data/documentation/quartzcore/cadisplaylink/preferredframeraterange.json) |
| `com.apple.developer.web-browser` | 14.0 | V-doc [Apple](https://developer.apple.com/tutorials/data/documentation/bundleresources/entitlements/com.apple.developer.web-browser.json) |

iOS 17.0 is the floor because per-profile data isolation needs `init(forIdentifier:)`. Below 17 Krypa could only offer the default store and `nonPersistent()`, so there would be no profiles. Install-base share of iOS 17+ was not researched (U).

## 3. egui and wgpu on iOS without winit

### 3.1 Metal backend status (plan section 1 items 1, 2)

- wgpu's Metal backend is actively maintained for iOS. The unreleased section of the trunk changelog lists iOS-specific fixes: simulator downlevel flags (`target_abi = "sim"`), a `Surface::configure` crash on iOS below 16, and sRGB colour-space handling. V-doc [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md). These are on trunk; whether each is in 30.0.2 is **U**.
- The wgpu `examples` directory shows no iOS example. V-doc [examples](https://github.com/gfx-rs/wgpu/tree/trunk/examples) (the subfolders `bug-repro`, `features`, `standalone` were not expanded, so a hidden iOS example is not ruled out; U).
- Known third-party example of the pattern "UIKit host owns the view, Rust gets the layer pointer, no winit": the `app-surface` crate. It creates the instance with `Backends::METAL` and `InstanceDescriptor::new_without_display_handle()` and calls `create_surface_unsafe(SurfaceTargetUnsafe::CoreAnimationLayer(obj.metal_layer))`. V-doc [app-surface 1.11.0 ios.rs](https://docs.rs/crate/app-surface/1.11.0/source/src/ios.rs). It does not use egui and its wgpu version was not shown (U). No complete egui + wgpu + Swift host + static library example was found (searched 2026-11-13).

### 3.2 Creating the wgpu surface from a UIKit-owned `CAMetalLayer` (item 2)

Exact API, quoted from wgpu source:

```rust
// wgpu/src/api/surface.rs, tag v30.0.2 (V-doc)
#[non_exhaustive]
pub enum SurfaceTargetUnsafe {
    RawHandle { raw_display_handle: Option<RawDisplayHandle>, raw_window_handle: RawWindowHandle },
    #[cfg(metal)] CoreAnimationLayer(*mut core::ffi::c_void), // doc: "Surface from `CoreAnimationLayer`."
    // plus Drm, dx12 variants
}
// wgpu instance API (trunk, V-doc)
pub unsafe fn create_surface_unsafe<'window>(
    &self, target: SurfaceTargetUnsafe,
) -> Result<Surface<'window>, CreateSurfaceError>
```

Sources: [surface.rs @ v30.0.2](https://raw.githubusercontent.com/gfx-rs/wgpu/v30.0.2/wgpu/src/api/surface.rs), [instance.rs @ trunk](https://raw.githubusercontent.com/gfx-rs/wgpu/trunk/wgpu/src/api/instance.rs). The safety contract is that the pointer must be a valid layer object (V-doc, same file). `create_surface_unsafe` is `unsafe`, so the iOS FFI crate cannot be `#![forbid(unsafe_code)]`; see ARCHITECTURE.md section 6.

Alternative path: `RawHandle` with a raw-window-handle UIKit window handle (U; not researched).

Facts that shape the Swift side (V-doc from [wgpu-hal metal surface.rs](https://raw.githubusercontent.com/gfx-rs/wgpu/trunk/wgpu-hal/src/metal/surface.rs)):
- `Surface::from_layer(&CAMetalLayer)` asserts via `isKindOfClass` that the object is a `CAMetalLayer`, then retains it. So the pointer must be a `CAMetalLayer`, not an arbitrary `CALayer`.
- `configure` calls `setDrawableSize` with the configured extent and does **not** set `contentsScale`. The Swift view must set `contentsScale` and pass the pixel size (bounds times scale) to Rust on every layout change.
- Apple: a UIView backed by `CAMetalLayer` overrides `class var layerClass: AnyClass { get }`; UIKit calls it once at view creation. V-doc [layerClass](https://developer.apple.com/tutorials/data/documentation/uikit/uiview/layerclass.json), [CAMetalLayer](https://developer.apple.com/tutorials/data/documentation/quartzcore/cametallayer.json).
- `CAMetalLayer.nextDrawable()` waits for a free drawable; request late, release early, wrap in an autorelease pool. V-doc, same CAMetalLayer page.

wgpu 30 surface-frame API changes the Rust frame loop must follow (V-doc [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md)): `Surface::get_current_texture` returns a `CurrentSurfaceTexture` enum (v29); presenting is `Queue::present(surface_texture)` (v30, replaces `SurfaceTexture::present()`); instance creation is `new_with_display_handle` / `new_without_display_handle` (v29).

Transparency: to composite the egui view over WKWebView the layer must be non-opaque. On trunk (unreleased) `CompositeAlphaMode::PostMultiplied` is replaced by `PreMultiplied` for non-opaque `CAMetalLayer`s and `PostMultiplied` now returns `UnsupportedAlphaMode`. V-doc [CHANGELOG](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md). Whether this is in 30.0.2 is **U**; a 30.0.2 smoke test must pick the supported mode from `SurfaceCapabilities`.

### 3.3 Driving egui directly (item 1)

egui-wgpu: the winit integration is an optional feature (V-doc [egui-wgpu deps](https://crates.io/api/v1/crates/egui-wgpu/0.37.0/dependencies)); the crate has `WgpuSetup::{CreateNew, Existing}` and `WgpuSetupExisting` for an existing wgpu setup (V-doc [WgpuSetup](https://docs.rs/egui-wgpu/0.37.0/egui_wgpu/enum.WgpuSetup.html); fields of `WgpuSetupExisting` not confirmed, U). Whether `default-features = false` is required to avoid pulling winit is **U**; check `cargo tree -i winit`.

Frame pipeline (all signatures V-doc from [egui Context](https://docs.rs/egui/0.37.0/egui/struct.Context.html) and [egui_wgpu Renderer](https://docs.rs/egui-wgpu/0.37.0/egui_wgpu/struct.Renderer.html)):

```rust
pub fn run_ui(&self, new_input: RawInput, run_ui: impl FnMut(&mut Ui)) -> FullOutput
pub fn tessellate(&self, shapes: Vec<ClippedShape>, pixels_per_point: f32) -> Vec<ClippedPrimitive>
pub fn new(device: &Device, output_color_format: TextureFormat, options: RendererOptions) -> Self
pub fn update_texture(&mut self, device: &Device, queue: &Queue, id: TextureId, image_delta: &ImageDelta)
pub fn update_buffers(&mut self, device: &Device, queue: &Queue, encoder: &mut CommandEncoder,
    paint_jobs: &[ClippedPrimitive], screen_descriptor: &ScreenDescriptor) -> Vec<CommandBuffer>
pub fn render(&self, render_pass: &mut RenderPass<'static>,
    paint_jobs: &[ClippedPrimitive], screen_descriptor: &ScreenDescriptor)
```

- `render` requires `RenderPass<'static>`, obtained with `wgpu::RenderPass::forget_lifetime`; `update_buffers` must be called first or `render` panics. V-doc, Renderer page.
- `ScreenDescriptor { size_in_pixels: [u32; 2], pixels_per_point: f32 }`. V-doc [ScreenDescriptor](https://docs.rs/egui-wgpu/0.37.0/egui_wgpu/struct.ScreenDescriptor.html).
- `FullOutput { platform_output, textures_delta, shapes, pixels_per_point, viewport_output }`: textures_delta must be applied before painting and removed textures freed after. V-doc [FullOutput](https://docs.rs/egui/0.37.0/egui/struct.FullOutput.html).
- `RendererOptions { msaa_samples, depth_stencil_format, dithering, predictable_texture_filtering }`. V-doc [RendererOptions](https://docs.rs/egui-wgpu/0.37.0/egui_wgpu/struct.RendererOptions.html). `msaa_samples` 0/1 = off (default).

### 3.4 Input events Krypa must synthesise (replacing egui-winit)

`RawInput` fields (V-doc [RawInput](https://docs.rs/egui/0.37.0/egui/struct.RawInput.html)): `viewport_id`, `viewports`, `safe_area_insets: Option<SafeAreaInsets>`, `screen_rect: Option<Rect>` (points), `max_texture_side`, `time`, `predicted_dt`, `events: Vec<Event>`, `hovered_files`, `dropped_files`, `focused`, `system_theme`.

`Event` variants (V-doc [Event](https://docs.rs/egui/0.37.0/egui/enum.Event.html)): `Copy`, `Cut`, `Paste(String)`, `Text(String)`, `Key{key, physical_key, pressed, repeat, modifiers}`, `PointerMoved(Pos2)`, `PointerButton{pos, button, pressed, modifiers}`, `PointerGone`, `Touch{device_id, id, phase, pos, force}`, `MouseWheel{..}`, `Zoom(f32)`, `Rotate(f32)`, `Ime(ImeEvent)`, `WindowFocused(bool)`, `ModifiersChanged`, `AccessKitActionRequest(ActionRequest)` and others.

Required minimum for Krypa chrome:
1. Touch to pointer emulation. egui-winit does this itself: one touch drives the pointer; Started = move + primary press; Moved = move; Ended = release then `PointerGone`; Cancelled = `PointerGone` without release; it also always pushes `Event::Touch`; `pos` is divided by `pixels_per_point`. V-doc [egui-winit lib.rs](https://raw.githubusercontent.com/emilk/egui/main/crates/egui-winit/src/lib.rs). Without winit, Krypa must reimplement these rules. Scrolling a list in egui needs drag handling from `PointerMoved` while pressed (U on exact behaviour).
2. `screen_rect` in points and `safe_area_insets`. egui-winit refreshes `safe_area_insets` on resize/scale-change/focus on iOS (V-doc, same file). Swift supplies `UIView.safeAreaInsets` (U for the exact property; not fetched).
3. `Event::Text`, `Event::Key` (backspace, enter, arrows from hardware keyboard), `Event::Ime(ImeEvent::Preedit{text, active_range_chars} | Commit(String) | DeleteSurrounding{before_chars, after_chars})`. `ImeEvent::Enabled`/`Disabled` are deprecated and unused. V-doc [ImeEvent](https://docs.rs/egui/0.37.0/egui/enum.ImeEvent.html).
4. Clipboard: `Paste`/`Copy`/`Cut` in; output via `PlatformOutput.commands: Vec<OutputCommand>` (the docs page does not list which commands exist; copy and open-url are U).
5. `WindowFocused(bool)`, `system_theme` (dark mode), `max_texture_side` (from wgpu limits), `time`.

Output Swift must act on (V-doc [PlatformOutput](https://docs.rs/egui/0.37.0/egui/output/struct.PlatformOutput.html)): `commands`, `ime: Option<IMEOutput>` ("set if, and only if, the user is currently editing text"), `mutable_text_under_cursor`, `accesskit_update: Option<TreeUpdate>` ("needs to be per-viewport"), `events` (screen-reader events), `cursor_icon` (ignore on iOS).

### 3.5 Text input and IME on iOS (item 2)

- egui-winit's IME mapping: winit `Preedit` to `ImeEvent::Preedit` (byte range converted to char range), `Commit` to `ImeEvent::Commit`; on output it calls `set_ime_allowed(ime.is_some())`, `set_ime_purpose`, and `set_ime_cursor_area` with the caret rect. V-doc [egui-winit](https://raw.githubusercontent.com/emilk/egui/main/crates/egui-winit/src/lib.rs). Krypa must do the equivalent in Swift against UIKit.
- UIKit side: `UIKeyInput` requires `hasText`, `insertText(_:)`, `deleteBackward()`, and gives "only a small subset of the available keyboards and languages". V-doc [UIKeyInput](https://developer.apple.com/tutorials/data/documentation/uikit/uikeyinput.json). Marked text (needed for CJK and autocorrect composition) requires the full `UITextInput` protocol: `markedTextRange`, `setMarkedText(_:selectedRange:)`, `unmarkText()`, `selectedTextRange`, `firstRect(for:)`, `caretRect(for:)`, `inputDelegate`, plus custom `UITextPosition`/`UITextRange` subclasses. V-doc [UITextInput](https://developer.apple.com/tutorials/data/documentation/uikit/uitextinput.json).
- Implication: the Rust view must be a `UIResponder` that conforms to `UITextInput`, backed by a text mirror (string, selection, marked range) that Swift keeps in sync with egui's focused `TextEdit`. egui's `IMEOutput` documents the caret rect but egui does not expose the full text to the host (U); the mirror has to be fed by Krypa's own address-bar model. Recommendation: the address bar state lives in `krypa-core`/`krypa-ui` as a plain `String` + selection that egui edits through `Event::Text`/`Ime`, and Swift mirrors it. This is a design inference (U).
- Status: **unresolved**. There is no documented egui-on-iOS soft-keyboard story without winit. The Phase 1 keyboard exit criterion is the verification. Fallback if composition proves too costly: a native `UITextField` hosted above the egui view for the address bar only (U; a degradation of the "all chrome in egui" premise, see ADR-0001).

### 3.6 Redraw on demand (item 2)

- `Context::request_repaint(&self)`, `request_repaint_after(&self, duration: Duration)`, and `set_request_repaint_callback(&self, callback: impl Fn(RequestRepaintInfo) + Send + Sync + 'static)`. V-doc [Context](https://docs.rs/egui/0.37.0/egui/struct.Context.html). The callback is `Send + Sync`, so it can fire on any thread; it must only set an atomic flag and hop to the main thread (U for the actual threads used).
- `CADisplayLink.preferredFrameRateRange: CAFrameRateRange` (iOS 15+) is a best-effort range; Low Power Mode, thermal state and accessibility settings can change it. V-doc [Apple](https://developer.apple.com/tutorials/data/documentation/quartzcore/cadisplaylink/preferredframeraterange.json). Pausing the link when idle (`isPaused`) and resuming it from the repaint callback is the standard pattern (U; `isPaused` not fetched). The `CADisableMinimumFrameDurationOnPhone` Info.plist key for 120 Hz is **U** (not confirmed in the fetched page).
- Rule: while a WKWebView scroll/animation is running, egui does not need to redraw; the link stays paused unless egui requested a repaint. This means the overlay must not drive web frame pacing (U).

### 3.7 Background / foreground

V-doc [Apple Metal background guide](https://developer.apple.com/tutorials/data/documentation/metal/preparing-your-metal-app-to-run-in-the-background.json): when deactivated, stop committing command buffers; before returning from the background-transition callback call `waitUntilScheduled()` on any unscheduled last command buffer; committed work in background fails with `MTLCommandBufferError.Code.notPermitted`. The page cites `applicationDidEnterBackground(_:)` as deprecated; the scene-lifecycle equivalent is **U**. Rust side requirement: `frame()` must be a no-op after `suspend()`, and must handle `notPermitted` and surface errors by recreating the surface (U; this is the Phase 1 "no surface loss" criterion).

## 4. Touch routing (plan section 1 item 4)

- UIKit `hitTest(_:with:)` ignores hidden views, views with `isUserInteractionEnabled == false`, and views with alpha below 0.01; it does not consider drawn content, so a transparent Metal view is still hit. Returning `nil` from an override makes the touch fall through to views behind. V-doc [hitTest](https://developer.apple.com/tutorials/data/documentation/uikit/uiview/hittest(_:with:).json), which includes the "PassthroughView" example.
- Therefore the mechanism in item 4 is supported by documented behaviour: the Rust view overrides `hitTest` to return itself only inside published rects, else `nil`; the sibling WKWebView underneath then receives the touch.
- Caveats (all U): (a) rects come from the previous frame because egui is immediate mode, so a popup opening this frame can miss one frame of touches; (b) a gesture that starts on the page and continues over a chrome rect stays with the page (UIKit assigns touches to the view hit at touch-began); (c) edge-swipe back/forward gestures of WKWebView (`allowsBackForwardNavigationGestures`, U) interact with the overlay; (d) a full-screen non-opaque Metal layer costs GPU blending and battery, measure it. Mitigation in ADR-0001: shrink the Rust view frame to the chrome region while no sheet is open.

## 5. WKWebView

### 5.1 Data stores (item "WKWebsiteDataStore")

V-doc [WKWebsiteDataStore](https://developer.apple.com/tutorials/data/documentation/webkit/wkwebsitedatastore.json) (class `@MainActor`, iOS 9+):

| Need | API | Minimum iOS | Notes |
|---|---|---|---|
| Default profile | `default()` | 9.0 | "persists to disk" |
| Private | `nonPersistent()` | 9.0 | "keeps website data in memory and doesn't write it to disk" |
| Named profile | `init(forIdentifier: UUID)` | 17.0 | creates the store if missing; throws an exception if the identifier is not valid; "Use this method to get a data store for a profile" ([Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkwebsitedatastore/init(foridentifier:).json)) |
| List profiles | `fetchAllDataStoreIdentifiers(_:)` | | fetches identifiers of existing stores that have identifiers |
| Delete profile | `remove(forIdentifier:completionHandler:)` | | |
| Inspect | `identifier: UUID?`, `isPersistent: Bool` | | |
| Cookies | `httpCookieStore: WKHTTPCookieStore` | | |

Assignment goes through `WKWebViewConfiguration.websiteDataStore` (shown in Apple's example on the same page). Design inferences (U): Krypa stores only the profile UUID and a display name in SQLite; it reconciles with `fetchAllDataStoreIdentifiers`; deleting a profile requires destroying its web views first (not documented, test); the default store and an identified store are distinct and the default profile maps to `default()`, so `ProfileKind::Default` must not be an identified store; whether `nonPersistent()` called twice gives isolated stores is **U** (create one per private session explicitly and hold the reference).

### 5.2 Content-process termination (item "process-termination callback")

`optional func webViewWebContentProcessDidTerminate(_ webView: WKWebView)` on `WKNavigationDelegate`, iOS 9+. "WebKit calls this method when the process for the specified web view terminates for any reason." V-doc [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wknavigationdelegate/webviewwebcontentprocessdidterminate(_:).json). It maps to `EngineEvent::ContentProcessTerminated`. What the view shows afterwards (blank) and that `reload()` recovers it are **U**.

### 5.3 State restoration (item "interactionState")

`var interactionState: Any? { get set }`, iOS 15+, "capture the current state of interaction in a web view so that you can restore that state later to another web view". V-doc [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkwebview/interactionstate.json). The page has no discussion section. Unverified (U): the object's serialisability to disk across launches, size, whether it contains page history URLs (privacy: must never be persisted for private sessions, which is the plan's WP-3 test), and whether it restores scroll position and form state on a different `WKWebViewConfiguration`. Fallback: keep URL + title only and reload.

### 5.4 Manifest discovery without a privileged JS bridge (item "manifest discovery")

- Search result: no public `WKWebView` API that returns the Web App Manifest was found (negative result from the WKWebView reference and a web search on 2026-11-13; **U** because absence is not proof).
- Documented building blocks: `WKContentWorld` (iOS 14+) is "a namespace to separate your app's web environment from individual webpages"; it "doesn't isolate the DOM", so DOM reads work from any world. V-doc [WKContentWorld](https://developer.apple.com/tutorials/data/documentation/webkit/wkcontentworld.json). `evaluateJavaScript(_ javaScript: String, in frame: WKFrameInfo? = nil, contentWorld: WKContentWorld) async throws -> Any?` (iOS 15+) V-doc [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkwebview/evaluatejavascript(_:in:contentworld:).json).
- Proposed approach (U): after `didFinish`, call `evaluateJavaScript` in a private named world (`WKContentWorld.world(name:)`) with a constant script that returns the `href` of `link[rel~="manifest"]` and the document URL as a string. The result comes back as the call's return value, so no `WKScriptMessageHandler` is registered and nothing is exposed as `window.webkit.messageHandlers` to the page. Then fetch the manifest: either (a) from the same world with `callAsyncJavaScript`, so the request carries the page origin and cookies, or (b) natively with `URLSession`, which does not use the profile's cookie store. Which one matches the manifest spec's credentials mode is **U**; start with (b) restricted to `https`, size-capped (matches WP-2's input cap) and no cookies.
- The script is app-authored and constant; it never interpolates page data (the plan's "hostile strings" rule). The returned value is untrusted and goes to `krypa-apps` as a string.
- This also fits App Review 4.7.2 "may not extend or expose native platform APIs ... to the software without prior permission" (V-doc [Guidelines](https://developer.apple.com/app-store/review/guidelines/)); see APP_STORE_COMPLIANCE.md.

### 5.5 Service workers (item "service worker availability")

- V-doc: apps holding `com.apple.developer.web-browser` can "Use Service Workers in `WKWebView` instances" and "Load pages from all domains with full script access". [Apple default-browser page](https://developer.apple.com/tutorials/data/documentation/xcode/preparing-your-app-to-be-the-default-browser.json).
- V-doc (WebKit blog, 2020): non-browser apps can declare up to 10 domains in `WKAppBoundDomains`, and set `WKWebViewConfiguration.limitsNavigationsToAppBoundDomains`; with the flag a web view on an app-bound domain can use `evaluateJavaScript`, `addUserScript`, `window.webkit.messageHandlers` and two `WKHTTPCookieStore` methods; other web views are denied those APIs. [WebKit blog](https://webkit.org/blog/10882/app-bound-domains/). The article does not mention service workers; that App-Bound Domains enable service workers is from secondary sources only (developer forums, [InAppWebView docs](https://www.inappwebview.dev/docs/5.x.x/service-worker/)) and is **U**.
- Consequence: a general-purpose browser cannot use App-Bound Domains (10-domain cap, navigation limited). Service workers for arbitrary sites need the default-browser entitlement. If the entitlement is refused, offline PWAs and anything service-worker-gated will not work, and `EngineCapabilities` must report `service_workers: false`.
- Krypa must not add `WKAppBoundDomains` to Info.plist: the 2020 blog implies that once an app declares it, web views without the flag lose `evaluateJavaScript` (the wording is ambiguous; **U**), and Apple forum reports describe views failing to load ([thread 745775](https://developer.apple.com/forums/thread/745775), forum reports, U).
- Do not confuse with `com.apple.developer.web-browser-engine.*` entitlements: a forum report shows a WKWebView app crashing on device asking for them when using app-bound domains ([thread 784292](https://developer.apple.com/forums/thread/784292), unanswered, U). Treat as a warning sign to test early on a physical device.

### 5.6 Default browser entitlement (item "default-browser entitlement requirements")

All V-doc from [Preparing your app to be the default web browser](https://developer.apple.com/tutorials/data/documentation/xcode/preparing-your-app-to-be-the-default-browser.json):
- Info.plist must specify the HTTP and HTTPS schemes. On launch the app must provide a text field for entering a URL, search tools, or curated bookmark lists. Opening an http(s) URL must navigate directly to that destination and render the expected content.
- Request via Apple's managed entitlement form (`com.apple.developer.web-browser`; optional `com.apple.developer.browser.app-installation` in the same form).
- Granted capabilities: selectable as default browser, full script access on all domains, service workers in WKWebView, Add to Home Screen via `UIActivityViewController` with the `WKWebView` in `activityItems`.
- Restrictions: cannot claim Universal Links for specific domains (the system ignores such claims); apps are rejected if Info.plist contains `NSPhotoLibraryUsageDescription` (use `NSPhotoLibraryAddUsageDescription`, and `PHPickerViewController`), `NSLocationAlwaysUsageDescription`/`NSLocationAlwaysAndWhenInUseUsageDescription`, `NSHomeKitUsageDescription`, `NSBluetoothAlwaysUsageDescription`, `NSHealthShareUsageDescription`/`NSHealthUpdateUsageDescription`.
- `UIApplication.shared.isDefault(.webBrowser)` is rate-limited and throws with `retryAvailableDateErrorKey`.
- Apple engineer statement (Jul 2026): an iOS browser needs only `com.apple.developer.web-browser` for passkeys, WKWebView then provides the full support. [forum thread 837755](https://developer.apple.com/forums/thread/837755) (single answer; the asker had been declined the entitlement; U beyond what the thread says).
- Approval is not guaranteed: a developer in that thread reports an earlier declined request (U, anecdotal). The approval criteria that matter to Krypa's design: the launch screen must expose a URL field, search or curated bookmarks, so a pure "app launcher" home screen may not qualify. This should change WP-5's launcher design (see ARCHITECTURE.md section 9).
- Alternative browser engines (BrowserEngineKit) are not applicable: Japan-only from iOS 26.2 with engine-steward and WPT/Test262 thresholds ([Apple Japan page](https://developer.apple.com/support/alternative-browser-engines-jp)); WKWebView is the engine.

## 6. Other WKWebView facts used (each V-doc, availability as stated)

- `WKPreferences.isFraudulentWebsiteWarningEnabled`, default `true`, iOS 13+. [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkpreferences/isfraudulentwebsitewarningenabled.json). Keep it on (the default-browser page permits Safe Browsing style warnings).
- `WKContentRuleListStore` (iOS 11+): compile JSON rules; a rule list must be added to the web view's `WKUserContentController`; limits are not documented. [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkcontentruleliststore.json).
- `WKUIDelegate`: `webView(_:requestMediaCapturePermissionFor:initiatedByFrame:type:decisionHandler:)`, `webView(_:requestDeviceOrientationAndMotionPermissionFor:initiatedByFrame:decisionHandler:)`, `webView(_:createWebViewWith:for:windowFeatures:) -> WKWebView?`, `webView(_:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:)`. [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkuidelegate.json). The page also lists `webView(_:requestGeolocationPermissionFor:initiatedByFrame:decisionHandler:)` but no availability; geolocation permission flow is **U** and needs a device test.
- `WKWebView.isInspectable` (iOS 16.4+, default false): enable for debug builds only (U policy choice; see APP_STORE_COMPLIANCE.md).
- `WKWebView.createPDF`, `takeSnapshot(with:completionHandler:)`, `find(_:configuration:)`, `cameraCaptureState`/`microphoneCaptureState`, `requestMediaPlaybackState(completionHandler:)` exist. [Apple](https://developer.apple.com/tutorials/data/documentation/webkit/wkwebview.json) (per-member availability not shown, U).
- `takeSnapshot` is allowed for the app switcher thumbnails only; it is not a rendering path (plan item 3 rejects screenshots as the display path).

## 7. Accessibility (item "AccessKit status on iOS")

- An iOS adapter exists: AccessKit lists iOS among released adapters ("Implements the UIAccessibility protocols in UIKit"); "all at rough feature parity" but "don't yet support all types of UI elements or all of the properties in the schema", no rich text or hypertext yet. V-doc [AccessKit README](https://github.com/AccessKit/accesskit). Crate `accesskit_ios` 0.2.2, version 0.x. V-doc [crates.io](https://crates.io/crates/accesskit_ios).
- API: `Adapter` (you subclass the `UIView` and forward `isAccessibilityElement`, `accessibilityElements`, `accessibilityHitTest:`) and `SubclassingAdapter` (dynamic Objective-C subclassing). `pub unsafe fn new(view: *mut c_void, activation_handler: impl 'static + ActivationHandler, ...) -> Self`; `pub fn update_if_active(&self, update_factory: impl FnOnce() -> TreeUpdate) -> Option<QueuedEvents>`; handlers are always called on the main thread; construct before the view is first shown or focused. V-doc [accesskit_ios docs](https://docs.rs/accesskit_ios/0.2.2/accesskit_ios/) and [SubclassingAdapter](https://docs.rs/accesskit_ios/0.2.2/accesskit_ios/struct.SubclassingAdapter.html).
- egui side: `Context::enable_accesskit(&self)` ("Enable generation of AccessKit tree updates in all future frames"), `PlatformOutput::accesskit_update: Option<TreeUpdate>`, and `Event::AccessKitActionRequest(ActionRequest)` as input. V-doc [Context](https://docs.rs/egui/0.37.0/egui/struct.Context.html), [PlatformOutput](https://docs.rs/egui/0.37.0/egui/output/struct.PlatformOutput.html), [Event](https://docs.rs/egui/0.37.0/egui/enum.Event.html). eframe's `accesskit` feature only forwards to `egui-winit/accesskit` (V-doc [eframe Cargo.toml](https://raw.githubusercontent.com/emilk/egui/main/crates/eframe/Cargo.toml)), so wiring on a non-winit host is custom work. No existing example of egui + accesskit_ios without winit was found (U).
- Answer to the plan's question "is browser-owned egui UI accessible at all today": **possibly, unverified**. All pieces exist (egui produces a tree, an iOS adapter exists, actions come back as events). Nobody has verified the combination here, the adapter is 0.2.x and incomplete by its own README, and the glue is Krypa's. Treat VoiceOver on chrome as a Phase 1 spike with an explicit pass/fail, not as given. Fallback: native `UIAccessibilityElement` proxies for the few chrome controls (U).
- Prefer `Adapter` over `SubclassingAdapter` for Krypa: it avoids runtime class swizzling on a view we already own, which also reduces App Review ambiguity under 2.5.1 (U; runtime-API use was not researched).
- The page's own accessibility comes from WKWebView natively (U). The Rust overlay must not hide it: set the overlay's non-chrome area as not an accessibility element (U).

## 8. Items not answered

- Web Push / notifications in WKWebView for third-party browsers: no Apple or WebKit statement found. Marked unresolved in APP_STORE_COMPLIANCE.md.
- Third-party credential-provider (password manager) autofill into WKWebView: only forum anecdotes (U).
- Exact behaviour of `nonPersistent()` instance sharing, `interactionState` serialisation, content-process-termination recovery, `WKDownload` availability: need device tests.
