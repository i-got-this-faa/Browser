// shim.cc — the embedding boundary. A thin C++ layer over CEF's content API
// exposing a small C ABI to Rust. The shell owns a genuine CefBrowser per
// page; frames cross as raw BGRA pixels + damage rects. No PNG, no JPEG, no
// base64, no Page.captureScreenshot anywhere.
//
// Threading: CEF runs its own UI thread (multi_threaded_message_loop). Every
// browser touch from Rust is wrapped in a CefPostTask(TID_UI). Events flow to
// the single Rust sink from arbitrary CEF threads; the sink only enqueues.
//
// Frame path (the whole point): OnPaint copies CEF's BGRA buffer into THIS
// view's stable heap buffer (dirty-row bounded memcpy, buffer reused across
// frames), coalescing damage into the union rect. Rust locks the buffer,
// patches only the damage rects into its render image, unlocks. One texture
// in gpui, zero allocations per frame.

#include "shim.h"

#include <algorithm>
#include <atomic>
#include <cstring>
#include <map>
#include <mutex>
#include <string>
#include <vector>

#include "include/cef_app.h"
#include "include/cef_browser.h"
#include "include/cef_client.h"
#include "include/cef_command_line.h"
#include "include/cef_render_handler.h"
#include "include/cef_display_handler.h"
#include "include/cef_load_handler.h"
#include "include/cef_life_span_handler.h"
#include "include/cef_focus_handler.h"
#include "include/base/cef_callback.h"
#include "include/cef_task.h"
#include "include/wrapper/cef_closure_task.h"
#include "include/wrapper/cef_helpers.h"

#ifndef CEF_SUBPROCESS_PATH
#define CEF_SUBPROCESS_PATH "cef-helper"
#endif
#ifndef CEF_RESOURCES_PATH
#define CEF_RESOURCES_PATH "Resources"
#endif
#ifndef CEF_LOCALES_PATH
#define CEF_LOCALES_PATH "Resources/locales"
#endif
#ifndef CEF_CACHE_PATH
#define CEF_CACHE_PATH "cef-cache"
#endif
#ifndef CEF_UA
#define CEF_UA "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36"
#endif

namespace {

struct Rect {
  int32_t x = 0, y = 0, w = 0, h = 0;
  bool empty() const { return w == 0 || h == 0; }
  void unite(const CefRect& r) {
    if (r.width <= 0 || r.height <= 0) return;
    if (empty()) {
      x = r.x; y = r.y; w = r.width; h = r.height;
      return;
    }
    int32_t x2 = std::max(x + w, r.x + r.width);
    int32_t y2 = std::max(y + h, r.y + r.height);
    x = std::min(x, r.x);
    y = std::min(y, r.y);
    w = x2 - x;
    h = y2 - y;
  }
  void unite(const Rect& r) {
    unite(CefRect(r.x, r.y, r.w, r.h));
  }
};

// Stable BGRA buffer. Reallocated ONLY when the engine changes size; damage
// is the union of everything painted since Rust last drained.
struct FrameBuffer {
  std::vector<uint8_t> px;
  int32_t w = 0, h = 0;
  Rect damage;
  std::mutex mu;

  void store(const uint8_t* src, int32_t sw, int32_t sh, const Rect& dirty) {
    std::lock_guard<std::mutex> lk(mu);
    if (sw != w || sh != h) {
      // Size change: reallocation (the ONLY one), then full damage.
      w = sw; h = sh;
      px.assign(size_t(sw) * size_t(sh) * 4, 0);
      damage = Rect();
      full_pending = true;
    }
    if (!dirty.empty()) {
      int32_t x0 = std::max(0, dirty.x);
      int32_t y0 = std::max(0, dirty.y);
      int32_t x1 = std::min(w, dirty.x + dirty.w);
      int32_t y1 = std::min(h, dirty.y + dirty.h);
      if (x1 > x0 && y1 > y0) {
        size_t rowbytes = size_t(x1 - x0) * 4;
        for (int32_t row = y0; row < y1; ++row) {
          const uint8_t* s = src + (size_t(row) * sw + x0) * 4;
          uint8_t* d = px.data() + (size_t(row) * w + x0) * 4;
          memcpy(d, s, rowbytes);
        }
      }
      // Union must reflect this frame's damage even across drains: keep
      // growing until Rust takes it (under the same lock, so no race).
      damage.unite(dirty);
    } else {
      memcpy(px.data(), src, px.size());
      damage = Rect();
      full_pending = true;
    }
  }

  bool full_pending = false;
};

// ---------------------------------------------------------------------------
// Sink (C++ -> Rust)
// ---------------------------------------------------------------------------

std::atomic<cef_sink_fn> g_sink{nullptr};
std::atomic<void*> g_sink_ud{nullptr};

void emit(uint32_t kind, uint64_t view_id, const char* str) {
  cef_sink_fn fn = g_sink.load(std::memory_order_acquire);
  if (!fn) return;
  cef_event_t ev{};
  ev.kind = kind;
  ev.view_id = view_id;
  ev.str = str;
  fn(&ev, g_sink_ud.load(std::memory_order_relaxed));
}

void emit_frame(uint64_t id, bool popup, int32_t w, int32_t h,
                const Rect& dmg) {
  cef_sink_fn fn = g_sink.load(std::memory_order_acquire);
  if (!fn) return;
  cef_event_t ev{};
  ev.kind = popup ? CEF_EV_POPUP_FRAME : CEF_EV_FRAME;
  ev.view_id = id;
  ev.w = w;
  ev.h = h;
  ev.nrects = 1;
  ev.rects[0][0] = dmg.x; ev.rects[0][1] = dmg.y;
  ev.rects[0][2] = dmg.w; ev.rects[0][3] = dmg.h;
  fn(&ev, g_sink_ud.load(std::memory_order_relaxed));
}

// ---------------------------------------------------------------------------
// View. CEF-refcounted: CEF threads can outlive Rust's destroy() call by the
// time it takes posted tasks to drain, so lifetime is owned by CefRefPtr
// chains (global map + handlers + in-flight lambdas), never raw delete.
// ---------------------------------------------------------------------------

struct View : public CefBaseRefCounted {
  uint64_t id = 0;
  CefRefPtr<CefBrowser> browser;  // UI thread only

  FrameBuffer frame;
  FrameBuffer popup;
  CefRect popup_geom;             // in view coords; UI thread writes
  std::mutex popup_geom_mu;

  int32_t w = 800, h = 600;
  float dsf = 1.0f;
  std::mutex geom_mu;

  IMPLEMENT_REFCOUNTING(View);
};

using ViewRef = CefRefPtr<View>;

// ---------------------------------------------------------------------------
// Handlers. Created per view; CEF holds refs via the client. Handlers hold
// ViewRef so callbacks are always safe.
// ---------------------------------------------------------------------------

