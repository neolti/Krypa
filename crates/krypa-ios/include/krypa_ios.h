/*
 * Krypa iOS static library: C ABI. Hand-written; a Rust test (ffi.rs) checks that every
 * function and constant below exists in the library with the same value.
 *
 * Threading: call everything from the main thread. A handle is not thread-safe.
 * Panics never cross this boundary: after KRYPA_ERR_PANIC the handle is dead (every call
 * returns KRYPA_ERR_PANIC); stop drawing and call krypa_view_destroy.
 */
#ifndef KRYPA_IOS_H
#define KRYPA_IOS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    KRYPA_OK = 0,
    KRYPA_ERR_NULL = 1,
    KRYPA_ERR_INVALID_ARG = 2,
    KRYPA_ERR_GPU = 3,
    KRYPA_ERR_PANIC = 4,
    KRYPA_ERR_UNSUPPORTED = 5
};

/* UITouch.Phase values for krypa_view_touch. */
enum {
    KRYPA_TOUCH_BEGAN = 0,
    KRYPA_TOUCH_MOVED = 1,
    KRYPA_TOUCH_ENDED = 2,
    KRYPA_TOUCH_CANCELLED = 3
};

/* Written by krypa_view_render when nothing is pending: sleep until the next input. */
enum { KRYPA_IDLE = 0xFFFFFFFF };

/* Opaque handle. */
typedef struct KrypaView KrypaView;

/* Safe-area insets in points (UIView.safeAreaInsets). */
typedef struct {
    float top;
    float left;
    float bottom;
    float right;
} KrypaInsets;

/*
 * Creates a view drawing into a CAMetalLayer. width_px/height_px: drawable size in pixels;
 * scale: pixels per point. On failure *out_view is NULL. The layer must stay alive until
 * krypa_view_destroy has returned.
 */
int32_t krypa_view_create(void *metal_layer, uint32_t width_px, uint32_t height_px,
                          float scale, KrypaInsets insets, KrypaView **out_view);

/* NULL is allowed. Call before releasing the layer. */
void krypa_view_destroy(KrypaView *view);

/* Size, scale or safe area changed. */
int32_t krypa_view_resize(KrypaView *view, uint32_t width_px, uint32_t height_px,
                          float scale, KrypaInsets insets);

/* One touch. touch_id: stable per finger (the UITouch address works); x/y in points. */
int32_t krypa_view_touch(KrypaView *view, uint64_t touch_id, uint32_t phase, float x, float y);

/*
 * Runs the UI and draws a frame. time_seconds: monotonic clock (CACurrentMediaTime()).
 * *next_frame_ms: 0 = draw again at the next display-link tick; KRYPA_IDLE = not until the
 * next touch or resize; otherwise a delay in milliseconds (input wakes it earlier).
 */
int32_t krypa_view_render(KrypaView *view, double time_seconds, uint32_t *next_frame_ms);

#ifdef __cplusplus
}
#endif

#endif /* KRYPA_IOS_H */
