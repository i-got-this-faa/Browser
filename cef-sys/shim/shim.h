// shim.h — C ABI between the Rust shell and the CEF (Chromium content API)
// embedding. Frames cross as raw BGRA pixels + damage rects; input, focus and
// device metrics travel the other way. No PNG, no base64, no screenshots.
#ifndef STRIP_CEF_SHIM_H
#define STRIP_CEF_SHIM_H

#include <stdint.h>

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
  // CEF_EV_TITLE / CEF_EV_URL: UTF-8, valid only during the sink call.
  const char *str;
} cef_event_t;

typedef void (*cef_sink_fn)(const cef_event_t *ev, void *ud);

// Must be called before anything else in the process. If the return value is
// >= 0 this process is a CEF child (renderer/gpu/utility) and the caller must
// exit with that code immediately.
int cef_early_process(int argc, char **argv);

// Install the single event sink (before or after engine start; events only
// flow once a view exists). The pointer/strings are only valid for the call.
void cef_set_sink(cef_sink_fn fn, void *ud);

