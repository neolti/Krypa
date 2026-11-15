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

use crate::geometry::{Insets, Viewport};
use crate::input::Phase;
use crate::view::{Next, View, ViewError};

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
        ] {
            assert!(
                header.contains(&format!("{name}(")),
                "{name} missing from header"
            );
        }
        let constants: [(&str, i64); 11] = [
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
}
