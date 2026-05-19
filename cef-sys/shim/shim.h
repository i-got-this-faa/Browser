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

