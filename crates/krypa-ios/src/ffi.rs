//! The C ABI the Swift shell calls. It is deliberately tiny: create, destroy, resize, touch,
//! render. The matching header is `include/krypa_ios.h`; a test keeps the two in step.
//!
//! Rules every entry point follows:
//! - No Rust type crosses the boundary except an opaque pointer. Enums arrive as plain
//!   integers and are validated, because a C caller can send any value and a Rust enum
//!   holding an invalid value is undefined behaviour.
//! - Results are `KRYPA_*` status codes. Arguments are validated before use; a null pointer
//!   or a non-finite number is an error, never a crash.
//! - A panic never unwinds into Swift (that is undefined behaviour). Each entry point runs
//!   inside `catch_unwind`; after a panic the handle is poisoned and every later call on it
//!   returns `KRYPA_ERR_PANIC`, so the shell can stop drawing and say so instead of limping
//!   on with state a panic left half-updated. We chose this over `panic = "abort"` because
//!   an abort kills the whole app, web sessions included, for a bug in the drawing layer,
//!   and because `abort` would have to be set workspace-wide and breaks `cargo test`.
//! - The handle is not thread-safe: the shell calls everything from the main thread.

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};

use krypa_engine_api::{NavigationKind, NavigationRequest, NavigationTarget, SessionId};
use url::Url;

use crate::app::NavDecision;
use crate::geometry::{Insets, Viewport};
use crate::input::Phase;
use crate::view::{Next, View, ViewError};
use crate::wire::{command_to_json, HostEvent, MAX_EVENT_BYTES};

/// Success.
pub const KRYPA_OK: i32 = 0;
/// A required pointer was null.
pub const KRYPA_ERR_NULL: i32 = 1;
/// An argument was out of range, not finite or not a known value.
pub const KRYPA_ERR_INVALID_ARG: i32 = 2;
/// The GPU could not be set up or drawing failed.
pub const KRYPA_ERR_GPU: i32 = 3;
/// A panic was caught; the handle is dead and must be destroyed.
pub const KRYPA_ERR_PANIC: i32 = 4;
/// This build cannot draw (not an Apple target).
pub const KRYPA_ERR_UNSUPPORTED: i32 = 5;

/// The core refused an event (stale or unknown session, or not valid in the session state).
/// Not a bug in the caller: late events from a dying web view are normal. Ignore and go on.
pub const KRYPA_ERR_REJECTED: i32 = 6;
/// The output buffer is too small; `*out_len` holds the size needed. Nothing was consumed.
pub const KRYPA_ERR_BUFFER_TOO_SMALL: i32 = 7;

/// Largest event or URL the adapter may send, in bytes.
pub const KRYPA_MAX_EVENT_BYTES: usize = MAX_EVENT_BYTES;

/// Navigation answer: let the web view navigate.
pub const KRYPA_NAV_ALLOW: u32 = 0;
/// Navigation answer: refuse. Also what every error leaves behind (fail closed).
pub const KRYPA_NAV_CANCEL: u32 = 1;

/// Navigation target: the main frame.
pub const KRYPA_NAV_TARGET_MAIN: u32 = 0;
/// Navigation target: an embedded frame.
pub const KRYPA_NAV_TARGET_SUB: u32 = 1;
/// Navigation target: a new window (`window.open`, `target=_blank`).
pub const KRYPA_NAV_TARGET_NEW_WINDOW: u32 = 2;

/// Navigation kind: link activated.
pub const KRYPA_NAV_KIND_LINK: u32 = 0;
/// Navigation kind: form submitted.
pub const KRYPA_NAV_KIND_FORM: u32 = 1;
/// Navigation kind: reload.
pub const KRYPA_NAV_KIND_RELOAD: u32 = 2;
/// Navigation kind: back or forward.
pub const KRYPA_NAV_KIND_BACK_FORWARD: u32 = 3;
/// Navigation kind: anything else.
pub const KRYPA_NAV_KIND_OTHER: u32 = 4;

/// `UITouch.Phase.began`.
pub const KRYPA_TOUCH_BEGAN: u32 = 0;
/// `UITouch.Phase.moved`.
pub const KRYPA_TOUCH_MOVED: u32 = 1;
/// `UITouch.Phase.ended`.
pub const KRYPA_TOUCH_ENDED: u32 = 2;
/// `UITouch.Phase.cancelled`.
pub const KRYPA_TOUCH_CANCELLED: u32 = 3;

/// `krypa_view_render` wrote this to `next_frame_ms`: nothing is pending, sleep until input.
pub const KRYPA_IDLE: u32 = u32::MAX;

/// Safe-area insets in points.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KrypaInsets {
    /// Top.
    pub top: f32,
    /// Left.
    pub left: f32,
    /// Bottom.
    pub bottom: f32,
    /// Right.
    pub right: f32,
}

/// Where the web view goes, in points in the Metal view's coordinates.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KrypaPlacement {
    /// Session whose web view is shown; 0 (never issued) means none: remove every web view.
    pub session: u64,
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

/// Opaque handle owned by the shell between `krypa_view_create` and `krypa_view_destroy`.
pub struct KrypaView {
    view: View,
    poisoned: bool,
}

impl KrypaView {
    pub(crate) fn new(view: View) -> Self {
        Self {
            view,
            poisoned: false,
        }
    }
}

fn viewport(width_px: u32, height_px: u32, scale: f32, insets: KrypaInsets) -> Option<Viewport> {
    let insets = Insets {
        top: insets.top,
        left: insets.left,
        bottom: insets.bottom,
        right: insets.right,
    };
    Viewport::new(width_px, height_px, scale, insets).ok()
}

/// Runs `f` so that no panic escapes.
fn guarded(f: impl FnOnce() -> i32) -> i32 {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(KRYPA_ERR_PANIC)
}

/// Runs `f` on the handle behind `view`, with the null, poison and panic handling shared by
/// every call.
///
/// # Safety
/// `view` must be null or a pointer returned by `krypa_view_create` that has not been
/// destroyed, with no other call running on it at the same time.
unsafe fn with_view(view: *mut KrypaView, f: impl FnOnce(&mut View) -> i32) -> i32 {
    // SAFETY: the caller guarantees `view` is null or a live, exclusively used handle.
    let Some(handle) = (unsafe { view.as_mut() }) else {
        return KRYPA_ERR_NULL;
    };
    if handle.poisoned {
        return KRYPA_ERR_PANIC;
    }
    let status = guarded(|| f(&mut handle.view));
    if status == KRYPA_ERR_PANIC {
        handle.poisoned = true;
    }
    status
}

/// Creates a view that draws into `metal_layer` and stores the handle in `*out_view`.
///
/// `metal_layer` is a `CAMetalLayer*`. `width_px`/`height_px` are the drawable size in
/// pixels, `scale` the pixels per point, `insets` the safe area in points. On failure
/// `*out_view` is set to null.
///
/// # Safety
/// - `out_view` must be null or valid for writing one pointer.
/// - `metal_layer` must be null or a live `CAMetalLayer` that the caller keeps alive, and
///   does not draw to itself, until `krypa_view_destroy` has returned.
#[no_mangle]
pub unsafe extern "C" fn krypa_view_create(
    metal_layer: *mut c_void,
    width_px: u32,
    height_px: u32,
    scale: f32,
    insets: KrypaInsets,
    out_view: *mut *mut KrypaView,
) -> i32 {
    if out_view.is_null() {
        return KRYPA_ERR_NULL;
    }
    // SAFETY: `out_view` is non-null and the caller guarantees it is writable.
    unsafe { *out_view = std::ptr::null_mut() };
    guarded(|| {
        if metal_layer.is_null() {
            return KRYPA_ERR_NULL;
        }
        let Some(vp) = viewport(width_px, height_px, scale, insets) else {
            return KRYPA_ERR_INVALID_ARG;
        };
        match create_view(metal_layer, vp) {
            Ok(view) => {
                // SAFETY: `out_view` was checked non-null above and is writable by contract.
                unsafe { *out_view = Box::into_raw(Box::new(KrypaView::new(view))) };
                KRYPA_OK
            }
            Err(ViewError::Unsupported) => KRYPA_ERR_UNSUPPORTED,
            Err(e) => {
                eprintln!("krypa: view creation failed: {e:?}");
                KRYPA_ERR_GPU
            }
        }
    })
}

#[cfg(target_vendor = "apple")]
fn create_view(layer: *mut c_void, vp: Viewport) -> Result<View, ViewError> {
    // SAFETY: `layer` is non-null and the layer's validity and lifetime are the contract of
    // `krypa_view_create`, which is the only caller.
    unsafe { View::with_layer(layer, vp) }
}

#[cfg(not(target_vendor = "apple"))]
fn create_view(_layer: *mut c_void, _vp: Viewport) -> Result<View, ViewError> {
    Err(ViewError::Unsupported)
}

/// Destroys a view. Null is allowed and does nothing. Must be called before the layer is
/// released.
///
/// # Safety
/// `view` must be null or a handle from `krypa_view_create` that was not destroyed yet, and
/// no other call may use it afterwards or at the same time.
#[no_mangle]
pub unsafe extern "C" fn krypa_view_destroy(view: *mut KrypaView) {
    if view.is_null() {
        return;
    }
    // SAFETY: non-null, created by `Box::into_raw` in `krypa_view_create`, and the caller
    // promises this is the only and last use. A panic in a destructor is swallowed: there is
    // nobody left to tell and unwinding into Swift is not allowed.
    let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(view) })));
}

/// The view's size, scale or safe area changed (rotation, split view, keyboard-less layout).
///
/// # Safety
/// `view` must be null or a live handle from `krypa_view_create`; see [`krypa_view_destroy`].
#[no_mangle]
pub unsafe extern "C" fn krypa_view_resize(
    view: *mut KrypaView,
    width_px: u32,
    height_px: u32,
    scale: f32,
    insets: KrypaInsets,
) -> i32 {
    // SAFETY: forwarded from this function's contract.
    unsafe {
        with_view(view, |v| {
            match viewport(width_px, height_px, scale, insets) {
                Some(vp) => {
                    v.resize(vp);
                    KRYPA_OK
                }
                None => KRYPA_ERR_INVALID_ARG,
            }
        })
    }
}

/// Forwards one touch. `touch_id` identifies the finger for its lifetime (the `UITouch`
/// address works), `phase` is a `KRYPA_TOUCH_*` value, `x`/`y` are in points in the view.
///
/// # Safety
/// `view` must be null or a live handle from `krypa_view_create`; see [`krypa_view_destroy`].
#[no_mangle]
pub unsafe extern "C" fn krypa_view_touch(
    view: *mut KrypaView,
    touch_id: u64,
    phase: u32,
    x: f32,
    y: f32,
) -> i32 {
    let phase = match phase {
        KRYPA_TOUCH_BEGAN => Phase::Began,
        KRYPA_TOUCH_MOVED => Phase::Moved,
        KRYPA_TOUCH_ENDED => Phase::Ended,
        KRYPA_TOUCH_CANCELLED => Phase::Cancelled,
        _ => return KRYPA_ERR_INVALID_ARG,
    };
    if !x.is_finite() || !y.is_finite() {
        return KRYPA_ERR_INVALID_ARG;
    }
    // SAFETY: forwarded from this function's contract.
    unsafe {
        with_view(view, |v| {
            v.touch(touch_id, phase, x, y);
            KRYPA_OK
        })
    }
}

/// Runs the UI and draws one frame. `time_seconds` is a monotonic clock
/// (`CACurrentMediaTime()`). `*next_frame_ms` tells the shell when to draw again: 0 means at
/// the next display-link tick, `KRYPA_IDLE` means not until the next touch or resize, any
/// other value is a delay in milliseconds (input still wakes it earlier).
///
/// # Safety
/// `view` must be null or a live handle from `krypa_view_create`; see
/// [`krypa_view_destroy`]. `next_frame_ms` must be null or valid for writing one `u32`.
#[no_mangle]
pub unsafe extern "C" fn krypa_view_render(
    view: *mut KrypaView,
    time_seconds: f64,
    next_frame_ms: *mut u32,
) -> i32 {
    if next_frame_ms.is_null() {
        return KRYPA_ERR_NULL;
    }
    if !time_seconds.is_finite() {
        return KRYPA_ERR_INVALID_ARG;
    }
    // SAFETY: forwarded from this function's contract.
    unsafe {
        with_view(view, |v| match v.render(time_seconds) {
            Ok(next) => {
                let ms = match next {
                    Next::Immediately => 0,
                    Next::After(d) => u32::try_from(d.as_millis().max(1))
                        .unwrap_or(KRYPA_IDLE - 1)
                        .min(KRYPA_IDLE - 1),
                    Next::Idle => KRYPA_IDLE,
                };
                // SAFETY: checked non-null above; writable by contract.
                *next_frame_ms = ms;
                KRYPA_OK
            }
            Err(e) => {
                eprintln!("krypa: render failed: {e:?}");
                KRYPA_ERR_GPU
            }
        })
    }
}

/// Hands one event from the web-view adapter to the core: UTF-8 JSON of a `HostEvent`
/// (`wire.rs`), at most `KRYPA_MAX_EVENT_BYTES` long. Returns `KRYPA_ERR_INVALID_ARG` for
/// anything malformed (nothing changed) and `KRYPA_ERR_REJECTED` when the core refused it.
///
/// # Safety
/// `view` must be null or a live handle (see [`krypa_view_destroy`]); `json` must be null or
/// valid for reading `len` bytes for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn krypa_view_push_event(
    view: *mut KrypaView,
    json: *const u8,
    len: usize,
) -> i32 {
    if json.is_null() {
        return KRYPA_ERR_NULL;
    }
    if len == 0 || len > MAX_EVENT_BYTES {
        return KRYPA_ERR_INVALID_ARG;
    }
    // SAFETY: non-null, and the caller guarantees `len` readable bytes; `len` is bounded.
    let bytes = unsafe { std::slice::from_raw_parts(json, len) };
    // SAFETY: forwarded from this function's contract.
    unsafe {
        with_view(view, |v| match HostEvent::parse(bytes) {
            Err(_) => KRYPA_ERR_INVALID_ARG,
            Ok(event) => match v.app_mut().push_host_event(event) {
                Ok(()) => KRYPA_OK,
                Err(_) => KRYPA_ERR_REJECTED,
            },
        })
    }
}

/// Pulls the next command for the adapter as UTF-8 JSON of an `EngineCommand`. `*out_len` is
/// set to the byte count (written, or needed); 0 with `KRYPA_OK` means the queue is empty.
/// If `buf` is null or `cap` is too small the result is `KRYPA_ERR_BUFFER_TOO_SMALL` and the
/// command stays queued.
///
/// # Safety
/// `view` must be null or a live handle; `out_len` must be null or writable; `buf` must be
/// null or valid for writing `cap` bytes.
#[no_mangle]
pub unsafe extern "C" fn krypa_view_next_command(
    view: *mut KrypaView,
    buf: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> i32 {
    if out_len.is_null() {
        return KRYPA_ERR_NULL;
    }
    // SAFETY: checked non-null; writable by contract.
    unsafe { *out_len = 0 };
    // SAFETY: forwarded from this function's contract.
    unsafe {
        with_view(view, |v| {
            let Some(command) = v.app().peek_command() else {
                return KRYPA_OK;
            };
            let Ok(json) = command_to_json(command) else {
                // Cannot happen for contract types; drop it rather than wedge the queue.
                let _ = v.app_mut().next_command();
                return KRYPA_ERR_INVALID_ARG;
            };
            // SAFETY: `out_len` was checked non-null above and is writable by contract.
            *out_len = json.len();
            if buf.is_null() || cap < json.len() {
                return KRYPA_ERR_BUFFER_TOO_SMALL;
            }
            // SAFETY: `buf` is valid for `cap >= json.len()` bytes by contract and cannot
            // overlap the freshly allocated `json`.
            std::ptr::copy_nonoverlapping(json.as_ptr(), buf, json.len());
            let _ = v.app_mut().next_command();
            KRYPA_OK
        })
    }
}

/// Asks whether the web view may perform a navigation. `*out_decision` is set to
/// `KRYPA_NAV_CANCEL` first and stays so on every error: the web view must cancel unless
/// the call returned `KRYPA_OK` with `KRYPA_NAV_ALLOW`. Side effects (external-link prompt,
/// new session for a new window) happen inside; Swift only acts on the answer.
///
/// # Safety
/// `view` must be null or a live handle; `url` null or valid for `url_len` bytes;
/// `out_decision` null or writable.
#[no_mangle]
pub unsafe extern "C" fn krypa_view_decide_navigation(
    view: *mut KrypaView,
    session: u64,
    url: *const u8,
    url_len: usize,
    target: u32,
    kind: u32,
    user_gesture: u32,
    out_decision: *mut u32,
) -> i32 {
    if out_decision.is_null() {
        return KRYPA_ERR_NULL;
    }
    // SAFETY: checked non-null; writable by contract.
    unsafe { *out_decision = KRYPA_NAV_CANCEL };
    if url.is_null() {
        return KRYPA_ERR_NULL;
    }
    let target = match target {
        KRYPA_NAV_TARGET_MAIN => NavigationTarget::MainFrame,
        KRYPA_NAV_TARGET_SUB => NavigationTarget::SubFrame,
        KRYPA_NAV_TARGET_NEW_WINDOW => NavigationTarget::NewWindow,
        _ => return KRYPA_ERR_INVALID_ARG,
    };
    let kind = match kind {
        KRYPA_NAV_KIND_LINK => NavigationKind::LinkActivated,
        KRYPA_NAV_KIND_FORM => NavigationKind::FormSubmitted,
        KRYPA_NAV_KIND_RELOAD => NavigationKind::Reload,
        KRYPA_NAV_KIND_BACK_FORWARD => NavigationKind::BackForward,
        KRYPA_NAV_KIND_OTHER => NavigationKind::Other,
        _ => return KRYPA_ERR_INVALID_ARG,
    };
    if user_gesture > 1 || url_len == 0 || url_len > MAX_EVENT_BYTES {
        return KRYPA_ERR_INVALID_ARG;
    }
    // SAFETY: non-null, `url_len` readable bytes by contract and bounded above.
    let bytes = unsafe { std::slice::from_raw_parts(url, url_len) };
    let Some(parsed) = std::str::from_utf8(bytes)
        .ok()
        .and_then(|t| Url::parse(t).ok())
    else {
        return KRYPA_ERR_INVALID_ARG;
    };
    let request = NavigationRequest {
        url: parsed,
        target,
        kind,
        user_gesture: user_gesture == 1,
    };
    // SAFETY: forwarded from this function's contract.
    unsafe {
        with_view(view, |v| {
            let answer = v
                .app_mut()
                .decide_navigation(SessionId::new(session), &request);
            if answer == NavDecision::Allow {
                // SAFETY: `out_decision` was checked non-null above and is writable.
                *out_decision = KRYPA_NAV_ALLOW;
            }
            KRYPA_OK
        })
    }
}

/// Writes where the web view goes after the last frame. `session == 0` means no web view
/// is to be on screen.
///
/// # Safety
/// `view` must be null or a live handle; `out` must be null or writable.
#[no_mangle]
pub unsafe extern "C" fn krypa_view_web_placement(
    view: *mut KrypaView,
    out: *mut KrypaPlacement,
) -> i32 {
    if out.is_null() {
        return KRYPA_ERR_NULL;
    }
    let none = KrypaPlacement {
        session: 0,
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    };
    // SAFETY: checked non-null; writable by contract.
    unsafe { *out = none };
    // SAFETY: forwarded from this function's contract.
    unsafe {
        with_view(view, |v| {
            let p = v.app().placement();
            if let Some(session) = p.session {
                // SAFETY: `out` was checked non-null above and is writable.
                *out = KrypaPlacement {
                    session: session.get(),
                    x: p.rect.min.x,
                    y: p.rect.min.y,
                    width: p.rect.width(),
                    height: p.rect.height(),
                };
            }
            KRYPA_OK
        })
    }
}

/// TEST ONLY (feature `debug-start-url`, absent from release builds): opens `url` as if the
/// user had typed it in the launcher.
///
/// # Safety
/// `view` must be null or a live handle; `url` null or valid for `len` bytes.
#[cfg(feature = "debug-start-url")]
#[no_mangle]
pub unsafe extern "C" fn krypa_debug_open_url(
    view: *mut KrypaView,
    url: *const u8,
    len: usize,
) -> i32 {
    if url.is_null() {
        return KRYPA_ERR_NULL;
    }
    if len == 0 || len > MAX_EVENT_BYTES {
        return KRYPA_ERR_INVALID_ARG;
    }
    // SAFETY: non-null, `len` readable bytes by contract and bounded above.
    let bytes = unsafe { std::slice::from_raw_parts(url, len) };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return KRYPA_ERR_INVALID_ARG;
    };
    // SAFETY: forwarded from this function's contract.
    unsafe {
        with_view(view, |v| match v.app_mut().debug_open_url(text) {
            Ok(()) => KRYPA_OK,
            Err(_) => KRYPA_ERR_INVALID_ARG,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::{null, null_mut};

    const INSETS: KrypaInsets = KrypaInsets {
        top: 59.0,
        left: 0.0,
        bottom: 34.0,
        right: 0.0,
    };

    fn headless_handle() -> *mut KrypaView {
        let vp = viewport(1179, 2556, 3.0, INSETS).unwrap();
        let view = View::headless(vp).unwrap();
        Box::into_raw(Box::new(KrypaView::new(view)))
    }

    #[test]
    fn create_without_out_pointer_or_layer_is_an_error_not_a_crash() {
        // SAFETY: null pointers are allowed by the documented contract.
        unsafe {
            assert_eq!(
                krypa_view_create(
                    std::ptr::dangling_mut::<c_void>(),
                    10,
                    10,
                    1.0,
                    INSETS,
                    null_mut()
                ),
                KRYPA_ERR_NULL
            );
            let mut out: *mut KrypaView = std::ptr::dangling_mut::<KrypaView>();
            assert_eq!(
                krypa_view_create(null_mut(), 10, 10, 1.0, INSETS, &mut out),
                KRYPA_ERR_NULL
            );
            assert!(out.is_null(), "a failed create must leave a null handle");
        }
    }

    #[test]
    fn create_validates_geometry_before_touching_the_layer() {
        let mut out: *mut KrypaView = null_mut();
        // SAFETY: the layer pointer is never dereferenced: validation fails first.
        let status = unsafe {
            krypa_view_create(
                std::ptr::dangling_mut::<c_void>(),
                0,
                100,
                1.0,
                INSETS,
                &mut out,
            )
        };
        assert_eq!(status, KRYPA_ERR_INVALID_ARG);
        assert!(out.is_null());
    }

    #[test]
    fn every_call_on_a_null_handle_reports_null() {
        let mut ms = 0;
        // SAFETY: null handles are allowed by the contract.
        unsafe {
            assert_eq!(
                krypa_view_resize(null_mut(), 10, 10, 1.0, INSETS),
                KRYPA_ERR_NULL
            );
            assert_eq!(krypa_view_touch(null_mut(), 1, 0, 0.0, 0.0), KRYPA_ERR_NULL);
            assert_eq!(krypa_view_render(null_mut(), 0.0, &mut ms), KRYPA_ERR_NULL);
            krypa_view_destroy(null_mut());
        }
    }

    #[test]
    fn unknown_touch_phases_and_non_finite_points_are_rejected() {
        let view = headless_handle();
        // SAFETY: `view` is a live handle used from one thread.
        unsafe {
            assert_eq!(
                krypa_view_touch(view, 1, 4, 0.0, 0.0),
                KRYPA_ERR_INVALID_ARG
            );
            assert_eq!(
                krypa_view_touch(view, 1, 0, f32::NAN, 0.0),
                KRYPA_ERR_INVALID_ARG
            );
            assert_eq!(
                krypa_view_touch(view, 1, 0, 0.0, f32::INFINITY),
                KRYPA_ERR_INVALID_ARG
            );
            assert_eq!(
                krypa_view_touch(view, 1, KRYPA_TOUCH_BEGAN, 5.0, 5.0),
                KRYPA_OK
            );
            krypa_view_destroy(view);
        }
    }

    #[test]
    fn render_reports_idle_once_the_launcher_has_settled() {
        // The battery promise: a static launcher must let the display link sleep.
        let view = headless_handle();
        let mut ms = 0;
        let mut t = 0.0;
        // SAFETY: `view` is live and `ms` is a valid out pointer.
        unsafe {
            for _ in 0..10 {
                assert_eq!(krypa_view_render(view, t, &mut ms), KRYPA_OK);
                t += 0.016;
            }
            assert_eq!(ms, KRYPA_IDLE);
            krypa_view_destroy(view);
        }
    }

    #[test]
    fn resize_with_bad_numbers_keeps_the_old_geometry() {
        let view = headless_handle();
        // SAFETY: `view` is a live handle used from one thread.
        unsafe {
            assert_eq!(
                krypa_view_resize(view, 100, 100, 0.0, INSETS),
                KRYPA_ERR_INVALID_ARG
            );
            assert_eq!((*view).view.app().viewport().size_px(), [1179, 2556]);
            assert_eq!(
                krypa_view_resize(
                    view,
                    1000,
                    500,
                    2.0,
                    KrypaInsets {
                        top: 0.0,
                        left: 0.0,
                        bottom: 0.0,
                        right: 0.0
                    }
                ),
                KRYPA_OK
            );
            assert_eq!((*view).view.app().viewport().size_px(), [1000, 500]);
            krypa_view_destroy(view);
        }
    }

    #[test]
    fn a_panic_is_caught_and_poisons_the_handle() {
        // Unwinding into Swift would be undefined behaviour; this is the guard against it.
        let view = headless_handle();
        // SAFETY: `view` is a live handle used from one thread.
        unsafe {
            let status = with_view(view, |_| panic!("boom"));
            assert_eq!(status, KRYPA_ERR_PANIC);
            assert_eq!(krypa_view_touch(view, 1, 0, 1.0, 1.0), KRYPA_ERR_PANIC);
            krypa_view_destroy(view);
        }
    }

    #[test]
    fn the_header_declares_every_entry_point_and_constant_with_the_same_value() {
        let header = include_str!("../include/krypa_ios.h");
        for name in [
            "krypa_view_create",
            "krypa_view_destroy",
            "krypa_view_resize",
            "krypa_view_touch",
            "krypa_view_render",
            "krypa_view_push_event",
            "krypa_view_next_command",
            "krypa_view_decide_navigation",
            "krypa_view_web_placement",
        ] {
            assert!(
                header.contains(&format!("{name}(")),
                "{name} missing from header"
            );
        }
        let constants: [(&str, i64); 24] = [
            ("KRYPA_ERR_REJECTED", KRYPA_ERR_REJECTED.into()),
            (
                "KRYPA_ERR_BUFFER_TOO_SMALL",
                KRYPA_ERR_BUFFER_TOO_SMALL.into(),
            ),
            ("KRYPA_NAV_ALLOW", KRYPA_NAV_ALLOW.into()),
            ("KRYPA_NAV_CANCEL", KRYPA_NAV_CANCEL.into()),
            ("KRYPA_NAV_TARGET_MAIN", KRYPA_NAV_TARGET_MAIN.into()),
            ("KRYPA_NAV_TARGET_SUB", KRYPA_NAV_TARGET_SUB.into()),
            (
                "KRYPA_NAV_TARGET_NEW_WINDOW",
                KRYPA_NAV_TARGET_NEW_WINDOW.into(),
            ),
            ("KRYPA_NAV_KIND_LINK", KRYPA_NAV_KIND_LINK.into()),
            ("KRYPA_NAV_KIND_FORM", KRYPA_NAV_KIND_FORM.into()),
            ("KRYPA_NAV_KIND_RELOAD", KRYPA_NAV_KIND_RELOAD.into()),
            (
                "KRYPA_NAV_KIND_BACK_FORWARD",
                KRYPA_NAV_KIND_BACK_FORWARD.into(),
            ),
            ("KRYPA_NAV_KIND_OTHER", KRYPA_NAV_KIND_OTHER.into()),
            (
                "KRYPA_MAX_EVENT_BYTES",
                i64::try_from(KRYPA_MAX_EVENT_BYTES).unwrap(),
            ),
            ("KRYPA_OK", KRYPA_OK.into()),
            ("KRYPA_ERR_NULL", KRYPA_ERR_NULL.into()),
            ("KRYPA_ERR_INVALID_ARG", KRYPA_ERR_INVALID_ARG.into()),
            ("KRYPA_ERR_GPU", KRYPA_ERR_GPU.into()),
            ("KRYPA_ERR_PANIC", KRYPA_ERR_PANIC.into()),
            ("KRYPA_ERR_UNSUPPORTED", KRYPA_ERR_UNSUPPORTED.into()),
            ("KRYPA_TOUCH_BEGAN", KRYPA_TOUCH_BEGAN.into()),
            ("KRYPA_TOUCH_MOVED", KRYPA_TOUCH_MOVED.into()),
            ("KRYPA_TOUCH_ENDED", KRYPA_TOUCH_ENDED.into()),
            ("KRYPA_TOUCH_CANCELLED", KRYPA_TOUCH_CANCELLED.into()),
            ("KRYPA_IDLE", KRYPA_IDLE.into()),
        ];
        for (name, value) in constants {
            let wanted = format!("{name} = {value}");
            let wanted_hex = format!("{name} = 0x{value:X}");
            assert!(
                header.contains(&wanted) || header.contains(&wanted_hex),
                "header must contain `{wanted}`"
            );
        }
        let _ = null::<u8>();
    }

    /// Why: a release build must not contain the URL-opening hook, so the header may only
    /// mention it inside the opt-in macro block.
    #[test]
    fn the_debug_hook_is_declared_only_inside_its_opt_in_block() {
        let header = include_str!("../include/krypa_ios.h");
        let start = header.find("#ifdef KRYPA_DEBUG_START_URL").unwrap();
        let end = start + header[start..].find("#endif").unwrap();
        assert!(header[start..end].contains("krypa_debug_open_url("));
        assert_eq!(header.matches("krypa_debug_open_url").count(), 1);
    }

    fn push(view: *mut KrypaView, json: &str) -> i32 {
        // SAFETY: `view` is a live handle (or null on purpose); the slice outlives the call.
        unsafe { krypa_view_push_event(view, json.as_ptr(), json.len()) }
    }

    fn pull(view: *mut KrypaView) -> Option<String> {
        let mut buf = vec![0u8; 4096];
        let mut len = 0usize;
        // SAFETY: `view` is live, `buf` has 4096 writable bytes, `len` is writable.
        let status =
            unsafe { krypa_view_next_command(view, buf.as_mut_ptr(), buf.len(), &mut len) };
        assert_eq!(status, KRYPA_OK);
        (len > 0).then(|| String::from_utf8(buf[..len].to_vec()).unwrap())
    }

    fn open(view: *mut KrypaView, url: &str) {
        // The same path the UI takes: an intent executed by the app.
        // SAFETY: `view` is a live handle used from one thread.
        let app = unsafe { (*view).view.app_mut() };
        app.execute_intents(vec![krypa_ui::Intent::Navigate(
            krypa_navigation::NavigationUrl::parse(url).unwrap(),
        )]);
    }

    #[test]
    fn commands_come_out_as_json_in_order_and_an_empty_queue_is_ok_with_zero_length() {
        let view = headless_handle();
        open(view, "https://example.com/");
        let first = pull(view).unwrap();
        assert!(first.starts_with(r#"{"CreateSession":{"session":1,"profile":"Default""#));
        assert_eq!(pull(view).unwrap(), r#"{"Show":{"session":1}}"#);
        assert_eq!(pull(view), None);
        // SAFETY: live handle.
        unsafe { krypa_view_destroy(view) };
    }

    #[test]
    fn a_too_small_buffer_reports_the_size_and_keeps_the_command() {
        let view = headless_handle();
        open(view, "https://example.com/");
        let mut len = 0usize;
        let mut tiny = [0u8; 4];
        // SAFETY: live handle, writable buffer and length.
        unsafe {
            assert_eq!(
                krypa_view_next_command(view, tiny.as_mut_ptr(), tiny.len(), &mut len),
                KRYPA_ERR_BUFFER_TOO_SMALL
            );
            assert!(len > 4);
            assert_eq!(
                krypa_view_next_command(view, null_mut(), 0, &mut len),
                KRYPA_ERR_BUFFER_TOO_SMALL
            );
            krypa_view_destroy(view);
        }
        // Nothing was consumed by the two failed pulls.
    }

    #[test]
    fn a_failed_pull_does_not_consume_the_command() {
        let view = headless_handle();
        open(view, "https://example.com/");
        let mut len = 0usize;
        // SAFETY: live handle, null buffer is allowed.
        let status = unsafe { krypa_view_next_command(view, null_mut(), 0, &mut len) };
        assert_eq!(status, KRYPA_ERR_BUFFER_TOO_SMALL);
        assert!(pull(view).is_some());
        // SAFETY: live handle.
        unsafe { krypa_view_destroy(view) };
    }

    #[test]
    fn hostile_event_bytes_are_refused_without_a_crash_and_change_nothing() {
        let view = headless_handle();
        open(view, "https://example.com/");
        let bad: [&[u8]; 4] = [b"{", &[0xFF, 0xFE], b"null", br#"{"Nope":{}}"#];
        // SAFETY: live handle; pointers and lengths match the slices.
        unsafe {
            for b in bad {
                assert_eq!(
                    krypa_view_push_event(view, b.as_ptr(), b.len()),
                    KRYPA_ERR_INVALID_ARG
                );
            }
            assert_eq!(
                krypa_view_push_event(view, b"x".as_ptr(), 0),
                KRYPA_ERR_INVALID_ARG
            );
            assert_eq!(
                krypa_view_push_event(view, b"x".as_ptr(), KRYPA_MAX_EVENT_BYTES + 1),
                KRYPA_ERR_INVALID_ARG
            );
            assert_eq!(krypa_view_push_event(view, null(), 5), KRYPA_ERR_NULL);
            assert_eq!(
                krypa_view_push_event(null_mut(), b"x".as_ptr(), 1),
                KRYPA_ERR_NULL
            );
            krypa_view_destroy(view);
        }
    }

    #[test]
    fn events_for_an_unknown_session_are_rejected_not_fatal() {
        let view = headless_handle();
        open(view, "https://example.com/");
        let status = push(view, r#"{"Title":{"session":77,"title":"x"}}"#);
        assert_eq!(status, KRYPA_ERR_REJECTED);
        // The handle still works afterwards.
        assert_eq!(
            push(view, r#"{"Title":{"session":1,"title":"ok"}}"#),
            KRYPA_OK
        );
        // SAFETY: live handle.
        unsafe { krypa_view_destroy(view) };
    }

    /// Why: every error path of the navigation question must leave `Cancel` behind, so a
    /// bug or hostile input can never turn into "allowed".
    #[test]
    fn navigation_questions_fail_closed() {
        let view = headless_handle();
        open(view, "https://example.com/");
        let url = b"https://example.com/next";
        let mut d = KRYPA_NAV_ALLOW;
        // SAFETY: live handle; pointers match the slices; `d` is writable.
        unsafe {
            let ask = |view, session, url: &[u8], target, kind, gesture, d: &mut u32| {
                krypa_view_decide_navigation(
                    view,
                    session,
                    url.as_ptr(),
                    url.len(),
                    target,
                    kind,
                    gesture,
                    d,
                )
            };
            assert_eq!(ask(view, 1, url, 0, 0, 0, &mut d), KRYPA_OK);
            assert_eq!(d, KRYPA_NAV_ALLOW);
            for (session, u, target, kind, gesture) in [
                (1u64, &url[..], 9u32, 0u32, 0u32),
                (1, url, 0, 9, 0),
                (1, url, 0, 0, 2),
                (1, b"not a url", 0, 0, 0),
                (1, &[0xFF, 0xFE][..], 0, 0, 0),
                (1, b"javascript:alert(1)", 0, 0, 1),
                (99, url, 0, 0, 0),
            ] {
                d = KRYPA_NAV_ALLOW;
                let status = ask(view, session, u, target, kind, gesture, &mut d);
                assert_eq!(d, KRYPA_NAV_CANCEL, "{status}");
            }
            d = KRYPA_NAV_ALLOW;
            assert_eq!(
                krypa_view_decide_navigation(view, 1, null(), 3, 0, 0, 0, &mut d),
                KRYPA_ERR_NULL
            );
            assert_eq!(d, KRYPA_NAV_CANCEL);
            krypa_view_destroy(view);
        }
    }

    #[test]
    fn the_placement_is_written_once_a_page_is_on_screen() {
        let view = headless_handle();
        let mut p = KrypaPlacement {
            session: 9,
            x: 1.0,
            y: 1.0,
            width: 1.0,
            height: 1.0,
        };
        // SAFETY: live handle; `p` is writable.
        unsafe {
            assert_eq!(krypa_view_web_placement(view, &mut p), KRYPA_OK);
            assert_eq!(p.session, 0, "no page: no web view");
            open(view, "https://example.com/");
            let mut ms = 0;
            for i in 0..6 {
                assert_eq!(
                    krypa_view_render(view, f64::from(i) * 0.016, &mut ms),
                    KRYPA_OK
                );
            }
            assert_eq!(krypa_view_web_placement(view, &mut p), KRYPA_OK);
            assert_eq!(p.session, 1);
            assert!(p.y > 59.0 && p.y + p.height < 852.0 - 34.0);
            assert_eq!(krypa_view_web_placement(view, null_mut()), KRYPA_ERR_NULL);
            krypa_view_destroy(view);
        }
    }
}
