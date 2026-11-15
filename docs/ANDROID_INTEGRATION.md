# Android integration (Phase 0 research)

Date: 2026-11-13. Author: WP-R. Research only; nothing was built or run. Android is not committed work (PLAN.md section 1 item 8).

Marking: **V-doc** = confirmed by a linked primary source. **U** = assumption or inference. Where the source is a secondary summary (a search-result summary of an Igalia deck), it is marked **U (secondary)**.

## 1. Recommendation

**WPE WebKit on Android: NEEDS SPIKE. Not viable as a commitment today; viable as a time-boxed experiment after the iOS PoC.**

Why, in order of weight:

1. **Security updates become Krypa's job.** WPE ships inside the APK, unlike Android System WebView, which is a separately updated package (U; Chromium via Play, general knowledge; the `getCurrentWebViewPackage()` API that exposes it is V-doc [android.webkit.WebView](https://developer.android.com/reference/android/webkit/WebView)). WebKit security fixes are published as WebKitGTK and WPE advisories: WSA-2026-0001 to WSA-2026-0006 between 2026-03-18 and 2026-09-29 (V-doc [wpewebkit.org/security](https://wpewebkit.org/security/)), about one every 4 to 7 weeks. The advisory page does not say which branches receive fixes (U). Krypa would have to rebuild the engine and re-ship for each one.
2. **The Android port lags upstream.** Latest WPE WebKit stable is 2.54.2 (2026-10-23) with 2.54.0 on 2026-09-16 (V-doc [wpewebkit.org/release](https://wpewebkit.org/release/)). The newest `wpeview` Maven version is 0.3.3, metadata last updated 2026-03-17 (V-doc [maven-metadata](https://repo1.maven.org/maven2/org/wpewebkit/wpeview/wpeview/maven-metadata.xml)). A search-result summary of Igalia material says 0.3.0 is based on WPE WebKit 2.50.4 (U, secondary). If that holds, the packaged engine is several minor releases behind. Which WPE version 0.3.3 contains was not verified.
3. **Small team, explicitly an experiment.** Igalia's 2026-09-11 post describes the project as "maintained by a small team" and "a valuable experiment" (V-doc [Igalia blog](https://blogs.igalia.com/alex/2026/09/11/wpe-webkit-on-android-now-with-wpeplatform/), paraphrased from the fetched summary). The NLnet grant ran 2023-04 to 2024-10 (V-doc [NLnet](https://nlnet.nl/project/WPE-Android/)); current funding is not stated anywhere I found (U).
4. **Still in flux architecturally.** September 2026: Android moved from libwpe to WPEPlatform, libwpe removed, a `WPEProcessManager` added because Android cannot `fork()`/`exec()`, and the post says WPEPlatform was expected to reach stable "in the next few days" (V-doc, same post). A new Java API mirrors `android.webkit.WebView` (`WebView`, `WebContext`, `WebSettings`, `CookieManager`, `WebViewClient`, `WebChromeClient`). Krypa would build against an API that was just rewritten.
5. **Unknowns that decide the spike:** APK size, memory, JIT/JS performance, media support, feature gaps, minimum Android SDK. None were found (see section 3).

What WPE has going for it: it is actively developed (repo updated 2026-11-02 per a search-result listing, U secondary; WPE WebKit stable releases every 2 to 3 weeks: 2.54.0 09-16, 2.54.1 10-06, 2.54.2 10-23, V-doc release page, cadence is an observation not a policy); it gives WebKit-on-both-platforms consistency; and it is LGPL-2.1 (V-doc [repo README](https://github.com/Igalia/wpe-android)), which has distribution implications (relinking obligations; legal review needed, U).

### Spike definition (if scheduled)

Time-box: 2 weeks, after iOS Phase 1 exit. Questions with pass/fail:
- Build `wpeview` from source with Cerbero on Linux (README says scripts were only tested on Linux, V-doc) and with the prebuilt Maven artifact; record APK size per ABI.
- Render krypa-web-fixtures (WP-7) pages; record failures vs System WebView.
- Measure cold start, memory with 3 views, scroll smoothness on one mid-range device.
- Verify a security-update workflow: how long from a WSA to a shippable APK.
- Gate: if the update path needs more than a documented, repeatable procedure by one engineer, answer is "not viable".

### Fallback engine

If Android is ever committed without WPE: Android System WebView through `androidx.webkit` (U). I believe `androidx.webkit` has a Profile API for isolated profiles, but its minimum version was not confirmed (the Android reference page fetched does not cover it; U). Then the "one engine on both platforms" premise is dropped, and `EngineCapabilities` (ARCHITECTURE.md section 4) carries the differences.

## 2. Maturity and cadence facts

| Fact | Value | Mark |
|---|---|---|
| WPE WebKit stable | 2.54.2, 2026-10-23 | V-doc [release page](https://wpewebkit.org/release/) |
| WPE WebKit dev | 2.53.92, 2026-09-03 | V-doc, same |
| wpe-android repo | `org.wpewebkit.wpeview.WebView`; arm64 and x86_64; MiniBrowser; remote inspector port 5000; WebDriver port 8888; Maven example `org.wpewebkit.wpeview:wpeview:0.3.0`; LGPL-2.1; 595 commits, 125 stars, 28 open issues | V-doc [README](https://github.com/Igalia/wpe-android) |
| Published Maven versions | 0.1.0 ... 0.3.0, 0.3.2, 0.3.3; last updated 2026-03-17 | V-doc [metadata](https://repo1.maven.org/maven2/org/wpewebkit/wpeview/wpeview/maven-metadata.xml) |
| 2024-08 status | WPE WebKit 2.44.1, libsoup3 for HTTP/2, instrumented tests + CI, focus on Android API level 30+ | V-doc [Igalia blog 2024-08-28](https://blogs.igalia.com/jani/bringing-webkit-back-to-android-progress-and-perspectives/) |
| 2026-09 status | WPEPlatform Android backend: `WPEDisplayAndroid` (AHardwareBuffer), `WPEToplevelAndroid` (ANativeWindow via SurfaceHolder callbacks), `WPEViewAndroid` (SurfaceTransaction + sync fences), `WPEScreenSyncObserverAndroid` (AChoreographer); scroll jank on Mali drivers fixed, helped by Skia deferred display lists | V-doc [Igalia blog 2026-09-11](https://blogs.igalia.com/alex/2026/09/11/wpe-webkit-on-android-now-with-wpeplatform/) |
| Android test coverage | 109 WPEView device tests pass on an Android 17 emulator (in one PR) | U (secondary: search-result summary of wpe-android PR 367) |
| Who ships security updates | The app embedding the engine (Krypa); upstream publishes advisories | V-doc advisories; ownership conclusion is inference U |

## 3. Binary size, feature gaps (answered as far as sources allow)

- **Binary size: not answered for the APK.** The only size data: prebuilt bootstrap archives at wpewebkit.org for arm64 at about 285 MB compressed (full) and about 55 MB (runtime, ranging 52 to 94 MB across releases) for releases around 2.48.5; Arch Linux ARM lists 154 MiB installed for the desktop `wpewebkit` 2.54.2 package. These are build-input or Linux figures, not APK sizes. U, from a search-result summary of [wpewebkit.org/android/bootstrap](https://wpewebkit.org/android/bootstrap/2.48.5). The spike must measure the real `libWPEWebKit.so` per ABI and the stripped APK.
- **Feature gaps: not answered.** No published feature matrix was found. Igalia's 2026 material (U secondary) lists WebXR on Android via OpenXR as experimental. The 2026-09 post says "there is still plenty work to do" and gives no gap list (V-doc, paraphrased). Items Krypa needs and cannot assume: WebAuthn/passkeys, service workers (WebKit feature, platform glue U), per-profile data isolation (U), downloads, permission prompts (camera, location), media codecs via GStreamer (the README says WPEWebKit and GStreamer are built by Cerbero, V-doc), Web Push.
- **Minimum SDK:** not stated in the README; the 2024 post says focus on API level 30+ (V-doc).

## 4. Integration model for Krypa on Android (all U; design sketch for a later spike)

- Host: a Java/Kotlin `Activity` that owns the process; Rust as a native library (`cdylib`). The `android-activity` crate is at 0.6.1 (2026-03-24, V-doc [crates.io](https://crates.io/crates/android-activity)) but is for Rust-owned activities; the iOS principle "platform owns the run loop" argues for a Java host plus JNI or UniFFI Kotlin bindings (UniFFI supports Kotlin, V-doc [overview](https://mozilla.github.io/uniffi-rs/latest/)).
- Composition: a `FrameLayout` with the engine view (WPE renders to a `SurfaceView`-style `ANativeWindow`, V-doc via `WPEToplevelAndroid`) and a second `SurfaceView` for egui. wgpu surface from the egui `ANativeWindow` (Vulkan or GLES backend choice U). Z-order, translucency and touch pass-through between two `SurfaceView`s is a known awkward area on Android (U); the spike must test it.
- Touch routing: Java `dispatchTouchEvent`/`onInterceptTouchEvent` on the egui view consulting the same published-rect list as iOS (U).
- Text input: Android `InputConnection` bridge (U), the analogue of the iOS `UITextInput` work.
- Accessibility: `accesskit_android` 0.9.1 exists (V-doc [crates.io](https://crates.io/crates/accesskit_android), created 2025-03-06), described as the Android adapter; its integration with a non-winit host is U.
- Profiles: WPE `WebContext` per profile (U); `CookieManager` exists in the new API (V-doc, Igalia post).
- Distribution: Play Store policy for a browser embedding its own engine was not researched (U).

## 5. Not researched

- Google Play policy for engines other than System WebView, and for alternative browsers.
- GeckoView as an alternative (not researched).
- WPE JIT/sandbox on Android (the 2024 post is silent; the 2026 post says Android cannot `fork()`/`exec()` so auxiliary processes become services, V-doc, but sandbox strength is U).
