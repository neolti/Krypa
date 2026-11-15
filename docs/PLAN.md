# Krypa — Implementation Plan

Status legend: **V** = verified on real toolchain/device, **U** = untested assumption.
Nothing in this file is **V** yet. Each work package (WP) flips its own items when proven.

## 0. Constraints that shape the plan

- Dev machine is Windows. iOS builds need macOS + Xcode, so **Phase 1 (iOS rendering PoC)
  cannot be built or verified here**. It needs a Mac or a macOS CI runner (simulator only;
  touch/memory/perf need a physical iPhone).
- Therefore the order is: Phase 0 research → platform-independent Rust core (testable
  anywhere) → desktop dev harness for the egui UI → iOS shell on macOS.
- Planning is done by the lead model; implementation is delegated to cheaper models
  (Sonnet-class). Each WP owns a disjoint set of files so WPs can run in parallel.
- `C:\Projects\ENGINEERING_GUIDELINES.md` applies: fail loud, no dead/speculative code,
  tests encode intent, versioned migrations, no secrets/PII in logs, lockfile committed.

## 1. Provisional architecture (all U until WP-R confirms)

1. **UIKit owns the process and run loop.** Rust is a static library. No eframe, no winit
   on iOS; egui is driven directly (`egui` + `egui-wgpu`) from a Swift `CADisplayLink`.
2. **Rust GPU view** = `UIView` backed by `CAMetalLayer`; wgpu surface created from the
   layer. Redraw on demand (egui repaint requests), not a free-running loop.
3. **WKWebView is a sibling native view**, composited by the OS. No screenshots, no
   re-implemented rendering.
4. **Touch routing is explicit.** The Rust view overrides hit-testing and asks Rust
   "is this point on browser-owned UI?" (list of opaque rects published each frame).
   Points outside go to WKWebView untouched.
5. **FFI:** two surfaces. (a) narrow C ABI for the hot path: surface create/resize/
   destroy, frame, touch, keyboard, safe-area; (b) command/event API for the engine
   (UniFFI vs. hand C ABI decided by WP-R). All calls main-thread-affine unless stated.
6. **Engine abstraction is command/event**, never sync getters. Rust core emits
   `EngineCommand`s; the platform adapter reports `EngineEvent`s. Capabilities are
   declared per platform, not assumed universal.
7. **Storage:** SQLite for Krypa metadata only. WebKit owns cookies/caches/site data
   (`WKWebsiteDataStore`); Krypa never touches those files.
8. **Android:** WPE WebKit is a research item, not a commitment (WP-R reports).

### 1.1 Amendments after WP-R (2026-11-13, doc-verified only; see ARCHITECTURE.md §9)

These override the items above where they differ.
- Item 4: size the Rust view to the chrome region where possible instead of a permanent
  full-screen transparent layer; the opaque-rect list is for popups and overlays.
- Item 5: one UniFFI (0.32.x, proc-macro) surface by default; a C ABI for `frame` and
  `push_touch` only if device measurement shows a need. swift-bridge rejected.
- Item 6: navigation / new-window policy is a pure synchronous Rust function, because
  the WKWebView delegate must answer inside the callback.
- Minimum iOS 17.0 (named profiles need `WKWebsiteDataStore.init(forIdentifier:)`).
- `EngineCapabilities` is reported at runtime. No public WKWebView suspend call was
  found, so `Suspended` never implies JavaScript was paused.
- A `krypa-ios-ffi` crate (wave 3) is the one documented exception to "no unsafe":
  wgpu's `create_surface_unsafe` requires it.
- Android: WPE WebKit verdict is "needs spike", not a commitment.
- Toolchain: Rust 1.95+ (egui 0.37). Dev machine uses the `stable-x86_64-pc-windows-gnu`
  toolchain; crates needing a C compiler (bundled SQLite in WP-4) need MSVC build tools
  or MinGW gcc installed first.
- Non-code tasks for the owner: apply early for the default-browser entitlement
  (service workers, Add to Home Screen and passkeys in WKWebView depend on it); the
  launcher must show a URL/search field on launch to meet Apple's browser criteria;
  ship no curated app catalog in v1 (guidelines 4.7 / 2.5.8 exposure).
- WP-6 exit criteria additionally: VoiceOver on the egui chrome, IME composition (CJK),
  UniFFI panic behaviour and per-call cost.

### 1.2 Install flow, icons and external open (2026-11-14, tested headless only)

Add to Krypa now asks the engine for the page's manifest (`FetchManifest` / `ManifestFetched`), shows name and origin in a dialog and saves through `krypa-storage` on confirm, falling back to the manual install when the manifest is missing or unusable; the harness loads installed apps at startup. App cards show manifest icons (host fetch, PNG only, size and pixel caps, placeholder on any failure). Pages that navigate to `tel:`, `mailto:` and similar now need a user gesture, the foreground session, and a Cancel-by-default confirmation; `EngineCommand::OpenExternal` hands the link to the OS. Details in ARCHITECTURE.md section 4. Not done: manifest shortcuts, share targets, service workers, update checks, JPEG/WebP/SVG icons, persisted icon bytes, real WKWebView adapter code (all still U on device).

## 2. Workspace layout (created only as WPs need it)

```
Cargo.toml                 workspace, shared dependency versions
crates/krypa-navigation    WP-1
crates/krypa-apps          WP-2
crates/krypa-engine-api    WP-3
crates/krypa-core          WP-3
crates/krypa-storage       WP-4 (wave 2)
crates/krypa-ui            WP-5 (wave 2)
apps/desktop-harness       WP-5 (wave 2) dev-only, mock engine
platforms/ios              WP-6 (wave 3, macOS)
tests/web-fixtures         WP-7 (wave 2)
docs/                      WP-R
```

Rules for every Rust WP: no `unsafe`; no `unwrap`/`expect` outside tests; `thiserror`
error enums; public items documented; all web-derived strings (titles, manifest fields,
URLs) treated as hostile; never log full URLs with query strings, titles, or page
content at info level. Done = `cargo fmt --check`, `cargo clippy -- -D warnings`,
`cargo test -p <crate>` all green, nothing skipped. Do not `git commit`.

## 3. Wave 1 (parallel, no shared files)

### WP-R — Phase 0 research and docs (web research, no code)
Writes `docs/ARCHITECTURE.md`, `docs/RISKS.md`, `docs/IOS_INTEGRATION.md`,
`docs/ANDROID_INTEGRATION.md`, `docs/APP_STORE_COMPLIANCE.md`, `docs/adr/0001-integration-model.md`.
Must answer, with source links and a V/U mark per claim:
- Current egui / egui-wgpu / wgpu versions; iOS Metal backend status; how to create a
  wgpu surface from a `CAMetalLayer` owned by UIKit (exact API); known examples.
- egui without winit: required input events, IME/text input story on iOS, AccessKit
  status on iOS (VoiceOver) — is browser-owned egui UI accessible at all today?
- UniFFI vs. C ABI (+ swift-bridge): ownership, callbacks, threading, async; pick one.
- WKWebView: `WKWebsiteDataStore` default / nonPersistent / `init(forIdentifier:)` and
  minimum iOS version; process-termination callback; state restoration
  (`interactionState`); manifest discovery without a privileged JS bridge; service
  worker availability in WKWebView (App-Bound Domains / browser entitlement).
- App Review Guidelines 2.5.1, 2.5.2, 2.5.4, 2.5.6, 2.5.8, 4.7, 4.7.2 — which apply to
  Krypa as a browser vs. as an app catalog; default-browser entitlement requirements.
  One table row per sensitive feature in the format the master prompt specifies.
- WPE WebKit Android: maturity, release cadence, binary size, feature gaps, who ships
  security updates. Recommendation: viable / not viable / needs spike.
- Risk register: risk, likelihood, impact, mitigation, owner WP, how it gets verified.
Confirm or overturn each item in §1 explicitly.

### WP-1 — `krypa-navigation`
- `classify_input(&str) -> Input::{Url, Search}`: address-bar text to URL or search
  query (`example.com`, `localhost:3000`, `foo bar`, `what is rust?`, IPs, IDN).
- `NavigationUrl`: validated, normalized top-level URL. Scheme allowlist `https`,
  `http` (flagged insecure). Reject `javascript:`, `data:`, `file:`, `blob:`, empty
  host, embedded credentials. Distinct `ExternalScheme` result for `mailto:`, `tel:`,
  app links, so the caller can ask the user.
- `Origin` + `display_origin()`: what the security indicator shows. Punycode for
  mixed-script / confusable hosts; strip nothing that identifies the site.
- `SecurityState::{Secure, Insecure, CertificateError}` (data only; the engine reports it as `ConnectionSecurity`, see ARCHITECTURE.md section 4).
- `SearchConfig`: template URL with query encoding; default provider configurable.
- Tests encode intent: "a javascript: URL can never become a navigation", "userinfo
  spoof `https://bank.com@evil.test` shows evil.test", "search text is percent-encoded".

### WP-2 — `krypa-apps`
- Conservative Web App Manifest parsing (serde; unknown fields ignored; wrong types
  degrade to "absent", never panic). Size cap on input.
- Validation per W3C manifest processing: `start_url` must be same-origin with the
  document URL else fall back to document URL; `scope` default and same-origin check;
  `id` resolution; `display` fallback chain limited to what Krypa supports
  (`standalone`, `minimal-ui`, `browser`; `fullscreen` degrades to `standalone`).
- Name sanitization: trim, length cap, strip control and bidi-override characters.
- Icon selection: same-scheme https only, pick best size for a target px, `purpose`.
- `theme_color` / `background_color` parsed to RGBA or dropped.
- `InstalledApp` model + `from_manifest(...)` and `manual(document_url, title)` for
  sites without a manifest. `is_in_scope(&Url)` for navigation leaving the app.
- Tests: "a manifest cannot install an app pointing at another origin", "hostile
  names cannot spoof via bidi overrides", "garbage JSON yields an error, not a panic".

### WP-3 — `krypa-engine-api` + `krypa-core`
- `krypa-engine-api`: `SessionId`, `ProfileId`, `ProfileKind::{Default, Persistent(id),
  Private}`, `EngineCommand` (create/destroy session, load, reload, stop, back, forward,
  suspend, resume, respond to permission), `EngineEvent` (navigation state, progress,
  title, connection security, icon, new-window request, external-scheme request, permission request, load
  error, content-process terminated, session destroyed), `EngineCapabilities` (what this
  platform really supports). Plain data, serde, no platform code, no async runtime.
- `krypa-core`: lifecycle state machine `NotRunning, Loading, Foreground, Background,
  Suspended, Terminated, Failed` with an explicit legal-transition table (illegal
  transition = typed error). `SessionManager`: owns sessions, one foreground at a time,
  consumes `EngineEvent`s, emits `EngineCommand`s, LRU retention with a configurable
  live-view budget, memory-pressure handler that evicts background views first and
  never the foreground one, recovery path after content-process termination.
  Suspended (view alive, paused) and Terminated (view destroyed, URL kept for restore)
  must be distinguishable in the public state.
- Tests with an in-test fake engine: "live views never exceed the budget", "memory
  pressure never destroys the foreground session", "a crashed session reloads on next
  foreground, not silently", "private sessions leave no restorable state".

## 4. Wave 2 (after wave 1 is reviewed)

- **WP-4 `krypa-storage`**: rusqlite, forward-only numbered migrations, repositories for
  installed apps, favorites, settings, history, profiles, permission decisions; indexes
  on every FK and filter column; tests against a real on-disk SQLite file.
- **WP-5 `krypa-ui` + `apps/desktop-harness`**: egui design system (tokens, light/dark,
  app card, toolbar, search field, switcher card, permission dialog, error/loading
  views), launcher / browser chrome / switcher screens driven by `krypa-core` state.
  Harness runs on Windows with a mock engine; publishes the opaque-rect list used for
  touch routing so that logic is testable before iOS exists.
- **WP-7 `tests/web-fixtures`**: static fixture sites (multi-page, SPA routing, forms,
  manifest, service worker/offline, external links, cross-origin, error pages).
- **CI**: fmt, clippy, test on every push; macOS job added with WP-6.

## 5. Wave 3 — Phase 1 iOS rendering PoC (needs macOS)

**WP-6 `platforms/ios`**: UIKit app, `KrypaRootViewController`, Metal-backed Rust view,
WKWebView sibling, FFI per ADR-0001. Exit criteria, each checked on simulator and then
a physical iPhone: egui renders at native scale; WKWebView scrolls at full rate under a
transparent Rust overlay; taps on toolbar reach egui and taps on page reach WebKit;
keyboard works for both address bar and page forms; rotation and safe-area changes;
background/foreground without surface loss or page reload. Results recorded in
`docs/IOS_INTEGRATION.md` as V or as a documented blocker.

## 6. Later phases (not planned in detail yet)

Install flow and dedicated app mode → app switcher and session retention on device →
profiles/isolation per `WKWebsiteDataStore` findings → browser mode features (bookmarks,
history, downloads, private) → performance instrumentation and budgets from measured
hardware → Android spike per WP-R recommendation.
