# ADR-0003: WKWebView adapter over the C ABI, JSON on the wire, no UniFFI yet

Status: Accepted for the WKWebView milestone (2026-11-24). Builds on ADR-0002, which said to reconsider UniFFI "when the engine adapter arrives". Evidence is the iOS Simulator in GitHub Actions (see `docs/IOS_STATUS.md`); nothing is verified on a device.

## Context

`EngineCommand` flows Rust to Swift, `EngineEvent` flows Swift to Rust. Both are plain serde types in `krypa-engine-api`. Swift has to run commands against `WKWebView`, report what the web view did, ask a question that must be answered synchronously (may this navigation happen?) and learn where the web view goes.

## Decision

1. **Stay on the hand-written C ABI; add four functions and carry the contract types as UTF-8 JSON.**
   - `krypa_view_push_event(view, json, len)`: Swift to Rust, one `HostEvent`.
   - `krypa_view_next_command(view, buf, cap, out_len)`: Rust to Swift, pull model, one `EngineCommand` per call; `BUFFER_TOO_SMALL` leaves the command queued.
   - `krypa_view_decide_navigation(view, session, url, len, target, kind, user_gesture, out_decision)`: synchronous, fail closed (`out_decision` is Cancel before anything else happens).
   - `krypa_view_web_placement(view, out)`: which session's web view to show and its rectangle in points.

   Why not UniFFI: the surface is four functions plus data that is already `serde`. UniFFI would add a generator, a generated-Swift step and a package to a CI loop where an iteration costs minutes, and would give Swift typed enums for a surface we still validate by hand. JSON from `JSONSerialization` on the Swift side and `serde` with `deny_unknown_fields` on the Rust side give strict validation (unknown variant, bad enum value, bad URL are rejected) with no generated code. Reconsider when the surface grows past what is comfortable to maintain by hand (profiles, permissions, downloads) or when Swift needs callbacks from Rust.
2. **Swift cannot say more than `WKWebView` knows.** The wire type `HostEvent` is narrower than `EngineEvent`: no `SecurityChanged`, `ManifestFetched`, `PermissionRequested`. A committed document is one event (`DocumentCommitted`) carrying the URL, history flags and the facts for the verdict (`trust` Absent, Valid or Invalid from `SecTrustEvaluateWithError` on `serverTrust`, and `hasOnlySecureContent`). Rust computes `ConnectionSecurity` in a pure function (`security.rs`): Secure only for https plus Valid trust plus only-secure content. A commit without a security report is therefore impossible. Raw `NSError` domain and code go to Rust, which maps them to `LoadErrorKind` (TLS codes -1200 to -1206 and errSSL, cancelled, ATS block, network).
3. **Navigation policy lives in Rust.** Every `decidePolicyFor` and `createWebViewWith` asks `krypa_view_decide_navigation`. Swift sends target (main, sub, new window; `targetFrame == nil` is a new window), kind (from `navigationType`) and a best-effort gesture flag: true only for `.linkActivated` and `.formSubmitted`, because `WKNavigationAction` has no user-gesture property. `window.open` reaches `createWebViewWith` only after a gesture (`javaScriptCanOpenWindowsAutomatically = false`), so that call reports a gesture. Side effects happen in Rust: an external scheme becomes `ExternalSchemeRequested` for the core (its own prompt; the web view never opens anything), a new window becomes a new foreground session (a "tab") or is blocked as a popup. Swift only acts on Allow or Cancel and never returns a web view from `createWebViewWith`.
4. **The web view is a sibling view above the Metal view.** After every frame Rust reports the foreground session and the content rectangle (`FrameOutput::web_view_rect`). If something the UI draws covers that rectangle (error view, modal dialog) the placement is none, because a native view would hide Rust's drawing. Consequence: while a dialog is open the page is removed (a snapshot placeholder is a later improvement). Returning to the launcher removes the web view from the hierarchy but keeps the session.
5. **Capabilities are all false** (`EngineCapabilities::default()`): no service workers or passkeys without the browser entitlement, no profiles, no private mode, no content blocking, no suspend. A new capability `manifest_fetch` (additive, default false) says whether the adapter answers `FetchManifest`; the UI offers "Add to Krypa" only if it is set.
6. **Test-only start URL.** Cargo feature `debug-start-url` adds `krypa_debug_open_url` to the library; the Xcode Debug configuration defines `KRYPA_DEBUG_START_URL` (Swift condition and `-Xcc` define for the header block) and reads `-KrypaStartURL <url>`. The Release configuration has none of these. CI builds the library without the feature, asserts the symbol is absent with `nm`, builds the Release app and asserts with `strings` that neither the argument name, the log prefix nor the symbol is in the binary, and only then builds the feature library for the Debug app.

## Input validation at the boundary

Every pointer is null-checked; lengths are bounded (32 KiB) before a slice is made; bytes must be UTF-8; JSON must parse into the exact type; enums arrive as integers or serde enums and are range-checked; progress above 1000 is refused; session ids are the core's, and the core rejects unknown or destroyed ones (`KRYPA_ERR_REJECTED`, not an error for Swift: late events from a dying web view are normal). Nothing in this path panics on bad input; a panic is still caught and poisons the handle (ADR-0002).

## Consequences

- JSON costs an allocation per event and per command; both are rare (navigation rate, not frame rate). Touch and render stay on plain C types.
- Same-document URL changes (`pushState`) are reported as `NavigationPhase::Finished` because the contract has no same-document phase; `Finished` leaves the security report alone.
- The gesture flag is best effort: a script-triggered `a.click()` is also `.linkActivated`. The core still shows its own prompt for external schemes.
