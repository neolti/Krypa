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

#include <stddef.h>
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
    KRYPA_ERR_UNSUPPORTED = 5,
    KRYPA_ERR_REJECTED = 6,          /* the core refused an event (stale session): ignore */
    KRYPA_ERR_BUFFER_TOO_SMALL = 7   /* *out_len = size needed; nothing consumed */
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

/* ---- Web-view adapter (docs/adr/0003-webview-adapter-boundary.md) ---------------------
 * Everything the adapter sends is untrusted input and is validated; bad input returns an
 * error code and changes nothing. JSON is UTF-8. */

/* Largest event or URL the adapter may send, in bytes. */
enum { KRYPA_MAX_EVENT_BYTES = 32768 };

/* krypa_view_decide_navigation: answer, target and kind. */
enum { KRYPA_NAV_ALLOW = 0, KRYPA_NAV_CANCEL = 1 };
enum {
    KRYPA_NAV_TARGET_MAIN = 0,
    KRYPA_NAV_TARGET_SUB = 1,
    KRYPA_NAV_TARGET_NEW_WINDOW = 2
};
enum {
    KRYPA_NAV_KIND_LINK = 0,
    KRYPA_NAV_KIND_FORM = 1,
    KRYPA_NAV_KIND_RELOAD = 2,
    KRYPA_NAV_KIND_BACK_FORWARD = 3,
    KRYPA_NAV_KIND_OTHER = 4
};

/* Where the web view goes, in points in the Metal view's coordinates. session == 0 (never
 * issued) means no web view is to be on screen. */
typedef struct {
    uint64_t session;
    float x;
    float y;
    float width;
    float height;
} KrypaPlacement;

/* One fact from WKWebView as JSON (a HostEvent, see wire.rs). Returns KRYPA_ERR_INVALID_ARG
 * for malformed input, KRYPA_ERR_REJECTED if the core refused it (late event of a closed
 * session: ignore). */
int32_t krypa_view_push_event(KrypaView *view, const uint8_t *json, size_t len);

/* Next command for the adapter as JSON (an EngineCommand). *out_len = 0 with KRYPA_OK means
 * none are queued. Too small a buffer: KRYPA_ERR_BUFFER_TOO_SMALL, *out_len = size needed,
 * the command stays queued. */
int32_t krypa_view_next_command(KrypaView *view, uint8_t *buf, size_t cap, size_t *out_len);

/* May the web view perform this navigation? *out_decision is set to KRYPA_NAV_CANCEL first
 * and stays so on any error: cancel unless the call returned KRYPA_OK with KRYPA_NAV_ALLOW.
 * user_gesture is 0 or 1 (unknown = 0). Handles external schemes and new windows itself. */
int32_t krypa_view_decide_navigation(KrypaView *view, uint64_t session, const uint8_t *url,
                                     size_t url_len, uint32_t target, uint32_t kind,
                                     uint32_t user_gesture, uint32_t *out_decision);

/* Where the web view goes after the last krypa_view_render. */
int32_t krypa_view_web_placement(KrypaView *view, KrypaPlacement *out);

#ifdef KRYPA_DEBUG_START_URL
/* TEST ONLY. Exists only in libraries built with the `debug-start-url` feature. */
int32_t krypa_debug_open_url(KrypaView *view, const uint8_t *url, size_t len);
#endif

#ifdef __cplusplus
}
#endif

#endif /* KRYPA_IOS_H */
