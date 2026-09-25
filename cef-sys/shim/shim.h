// shim.h — C ABI between the Rust shell and the CEF (Chromium content API)
// embedding. Frames cross as raw BGRA pixels + damage rects; input, focus and
// device metrics travel the other way. No PNG, no base64, no screenshots.
#ifndef STRIP_CEF_SHIM_H
#define STRIP_CEF_SHIM_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// ---- event kinds delivered to the single Rust sink ------------------------
enum {
  CEF_EV_FRAME = 1,       // view frame updated: see rects; nrects==0 => full
  CEF_EV_POPUP_FRAME = 2, // popup (select dropdown etc.) buffer updated
  CEF_EV_TITLE = 3,
  CEF_EV_URL = 4,
  CEF_EV_LOADING = 5,
  CEF_EV_CLOSED = 6,
  CEF_EV_DMABUF = 7,      // native dmabuf frame delivered
  CEF_EV_CURSOR = 8,      // cursor shape changed (cef_cursor_type_t)
};

typedef struct {
  uint32_t kind;
  uint64_t view_id;
  // FRAME / POPUP_FRAME: geometry of the updated buffer (view or popup).
  int32_t w, h;
  // Damage rects for CEF_EV_FRAME (pixel coords, upper-left origin BGRA).
  // nrects == 0 means "full frame". POPUP_FRAME always carries the popup rect.
  int32_t rects[16][4];
  int32_t nrects;
  int32_t loading;          // CEF_EV_LOADING: 1 = loading, 0 = done
  int32_t cursor_type;      // CEF_EV_CURSOR: cef_cursor_type_t enum value
  // CEF_EV_TITLE / CEF_EV_URL: UTF-8, valid only during the sink call.
  const char *str;
  // CEF_EV_DMABUF:
  int32_t dmabuf_fd;
  uint32_t stride;
  uint64_t offset;
  uint64_t modifier;
  uint32_t drm_format;
} cef_event_t;

typedef void (*cef_sink_fn)(const cef_event_t *ev, void *ud);

// Must be called before anything else in the process. If the return value is
// >= 0 this process is a CEF child (renderer/gpu/utility) and the caller must
// exit with that code immediately.
int cef_early_process(int argc, char **argv);

// Install the single event sink (before or after engine start; events only
// flow once a view exists). The pointer/strings are only valid for the call.
void cef_set_sink(cef_sink_fn fn, void *ud);

// Initialize CEF (idempotent, safe to call from any thread). Blocks until the
// context is ready or times out. Returns 0 on success. CEF runs its own UI
// thread (multi_threaded_message_loop); commands posted via CefPostTask.
// subprocess: this executable (CefExecuteProcess early-exit pattern);
// resources/locales/cache: absolute paths.
int cef_engine_start(const char *subprocess, const char *resources,
                     const char *locales, const char *cache);

// ---- views ----------------------------------------------------------------
// All calls are cheap and thread-safe; input is forwarded onto CEF's UI
// thread internally.
void *cef_view_create(uint64_t id, const char *url, int32_t w, int32_t h);
void cef_view_destroy(void *view);

// Lock the view's stable BGRA frame buffer; returns its base pointer and
// current size. Copy only `rects` from the last FRAME event while holding the
// lock, then unlock. Same pattern for the popup buffer.
const uint8_t *cef_view_lock_frame(void *view, int32_t *w, int32_t *h);
void cef_view_unlock_frame(void *view);
const uint8_t *cef_view_lock_popup(void *view, int32_t *w, int32_t *h);
void cef_view_unlock_popup(void *view);
int32_t cef_view_popup_visible(void *view);
void cef_view_popup_rect(void *view, int32_t out[4]);

void cef_view_navigate(void *view, const char *url);
void cef_view_back(void *view);
void cef_view_forward(void *view);
void cef_view_reload(void *view, int hard);
void cef_view_resize(void *view, int32_t w, int32_t h);
void cef_view_focus(void *view, int focus);
// Hidden pages must not composite (DoD); CEF stops producing frames.
void cef_view_hidden(void *view, int hidden);

// Mouse: kind 0=move 1=down 2=up; button 0=left 1=middle 2=right.
void cef_view_mouse(void *view, int kind, int button, int x, int y,
                    int click_count, int modifiers);
// Wheel in pixels.
void cef_view_wheel(void *view, int x, int y, int dx, int dy);

// Keyboard: type = cef_key_event_type_t (0=RAWKEYDOWN 1=KEYDOWN 2=KEYUP
// 3=CHAR). mods = CEF EVENTFLAG bits verbatim (SHIFT=2 CTRL=4 ALT=8 META=128).
void cef_view_key(void *view, int type, int windows_key_code,
                  int native_key_code, uint32_t mods, uint16_t ch16);

// Native Wayland subsurface presentation (zero-copy dmabuf import)
int cef_wayland_init(void *wl_display, void *wl_parent_surface);
void cef_view_attach_wayland(void *view);
void cef_view_set_geometry(void *view, int32_t x, int32_t y, int32_t w, int32_t h,
                           int32_t visible, int32_t has_overlay);
int cef_view_get_screenshot(void *view, uint8_t **out_buf, int32_t *out_w, int32_t *out_h, size_t *out_size);

// Frame rate control (refresh rate matching, e.g. 144Hz)
void cef_set_target_frame_rate(int32_t fps);
int32_t cef_get_target_frame_rate(void);
void cef_view_set_frame_rate(void *view, int32_t fps);

void cef_view_mouse_leave(void *view);
void cef_wayland_dispatch(void);

#ifdef __cplusplus
}
#endif
#endif // STRIP_CEF_SHIM_H
