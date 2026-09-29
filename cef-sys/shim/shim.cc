// shim.cc — the embedding boundary. A thin C++ layer over CEF's content API
// exposing a small C ABI to Rust. The shell owns a genuine CefBrowser per
// page.
//
// Threading: CEF runs its own UI thread (multi_threaded_message_loop). Every
// browser touch from Rust is wrapped in a CefPostTask(TID_UI). Events flow to
// the single Rust sink from arbitrary CEF threads; the sink only enqueues.
//
// Frame paths:
// - Wayland (default): OnAcceleratedPaint attaches CEF's dmabuf to a per-page
//   wl_subsurface; niri composites it. wl_buffers are pooled per dmabuf inode.
//   CEF documents that the dmabuf returns to its pool when the callback
//   returns, so the compositor can read a buffer CEF is already reusing; a
//   client-owned copy would close that gap.
// - Fallback: OnPaint copies CEF's BGRA buffer into the view's stable heap
//   buffer (dirty-row bounded memcpy) and Rust patches the damage rects into
//   one gpui texture.

#include "shim.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstring>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>

#ifdef STRIP_WAYLAND_DMABUF
#include <wayland-client.h>
#include "linux-dmabuf-v1-client-protocol.h"
#ifdef STRIP_WAYLAND_VIEWPORTER
#include "viewporter-client-protocol.h"
#endif

extern "C" {
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wattributes"
#include "linux-dmabuf-v1-protocol.c"
#ifdef STRIP_WAYLAND_VIEWPORTER
#include "viewporter-protocol.c"
#endif
#pragma GCC diagnostic pop
}

struct OutputInfo {
  uint32_t name = 0;
  struct wl_output* output = nullptr;
  int32_t refresh_hz = 60;
};

struct WlContext {
  struct wl_display* display = nullptr;
  struct wl_surface* parent_surface = nullptr;
  struct wl_event_queue* queue = nullptr;
  struct wl_compositor* compositor = nullptr;
  struct wl_subcompositor* subcompositor = nullptr;
  struct zwp_linux_dmabuf_v1* dmabuf = nullptr;
  struct wp_viewporter* viewporter = nullptr;
  std::map<uint32_t, std::unique_ptr<OutputInfo>> outputs;
  std::map<struct wl_output*, OutputInfo*> output_by_ptr;
  std::recursive_mutex mu;
};

static WlContext g_wl;
#endif

static std::atomic<int32_t> g_target_fps{0};
int32_t get_target_frame_rate();
void cef_set_target_frame_rate(int32_t fps);
int32_t cef_get_target_frame_rate();
void cef_view_set_frame_rate(void* view, int32_t fps);

#include "include/cef_app.h"
#include "include/cef_audio_handler.h"
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

  // Copy one CEF frame in. Returns what Rust must repaint next time it
  // drains: full=true (nrects==0 on the wire) or the accumulated union rect.
  // A size change (or a frame with no dirty rect) requires a FULL copy of the
  // new buffer — reporting only the incoming frame's rect would leave every
  // other pixel of the freshly allocated buffer zeroed (black-hole paint).
  struct StoreResult {
    bool full;
    Rect rects;
  };
  StoreResult store(const uint8_t* src, int32_t sw, int32_t sh,
                    const Rect& dirty) {
    std::lock_guard<std::mutex> lk(mu);
    bool full = false;
    if (sw != w || sh != h) {
      // Size change: reallocation (the ONLY one), then full damage.
      w = sw; h = sh;
      px.assign(size_t(sw) * size_t(sh) * 4, 0);
      full = true;
    }
    if (full || dirty.empty()) {
      if (!full) full = true;  // empty dirty == CEF repainted everything
      memcpy(px.data(), src, px.size());
      // Full repaint: the accumulated union is meaningless now.
      damage = Rect{0, 0, w, h};
    } else {
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
      // MERGE into the union Rust has not drained yet: overwriting here used
      // to drop the rects of frames that arrived between two drains, so the
      // shell patched only the newest rect and stale pixels stayed on screen.
      damage.unite(dirty);
    }
    return {full, damage};
  }
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
                const Rect& dmg, bool full) {
  cef_sink_fn fn = g_sink.load(std::memory_order_acquire);
  if (!fn) return;
  cef_event_t ev{};
  ev.kind = popup ? CEF_EV_POPUP_FRAME : CEF_EV_FRAME;
  ev.view_id = id;
  ev.w = w;
  ev.h = h;
  if (full) {
    ev.nrects = 0;  // documented convention: nrects == 0 => full frame
  } else {
    ev.nrects = 1;
    ev.rects[0][0] = dmg.x; ev.rects[0][1] = dmg.y;
    ev.rects[0][2] = dmg.w; ev.rects[0][3] = dmg.h;
  }
  fn(&ev, g_sink_ud.load(std::memory_order_relaxed));
}

// ---------------------------------------------------------------------------
// View. CEF-refcounted: CEF threads can outlive Rust's destroy() call by the
// time it takes posted tasks to drain, so lifetime is owned by CefRefPtr
// chains (global map + handlers + in-flight lambdas), never raw delete.
// ---------------------------------------------------------------------------

#ifdef STRIP_WAYLAND_DMABUF
struct BufferKey {
  uint64_t inode = 0;
  uint32_t width = 0;
  uint32_t height = 0;
  uint32_t stride = 0;
  uint64_t modifier = 0;
  uint64_t offset = 0;

  bool operator<(const BufferKey& o) const {
    if (inode != o.inode) return inode < o.inode;
    if (width != o.width) return width < o.width;
    if (height != o.height) return height < o.height;
    if (stride != o.stride) return stride < o.stride;
    if (modifier != o.modifier) return modifier < o.modifier;
    return offset < o.offset;
  }
};

struct PooledBuffer {
  struct wl_buffer* buffer = nullptr;
  BufferKey key{};
  std::atomic<bool> in_use{false};
  uint64_t last_used_frame = 0;

  PooledBuffer() = default;
  ~PooledBuffer() {
    if (buffer) {
      wl_buffer_destroy(buffer);
      buffer = nullptr;
    }
  }
};

struct View;
static void view_frame_done(void* data, struct wl_callback* cb, uint32_t time);
static void pooled_buffer_release(void* data, struct wl_buffer*);
#endif

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

  std::atomic<bool> has_pending_move{false};
  std::atomic<int32_t> pending_move_x{0};
  std::atomic<int32_t> pending_move_y{0};
  std::atomic<uint32_t> pending_move_mods{0};
  std::atomic<uint32_t> mouse_button_modifiers{0};
  // Live audio streams of this page (a page may run several at once).
  std::atomic<int32_t> audio_streams{0};

#ifdef STRIP_WAYLAND_DMABUF
  struct wl_surface* child_surface = nullptr;
  struct wl_subsurface* subsurface = nullptr;
  struct wp_viewport* viewport = nullptr;
  struct wl_buffer* current_buffer = nullptr;
  struct wl_callback* frame_callback = nullptr;
  bool frame_callback_pending = false;
  std::map<BufferKey, std::unique_ptr<PooledBuffer>> buffer_pool;
  uint64_t frame_seq = 0;

  int32_t last_x = 0, last_y = 0, last_w = 0, last_h = 0;
  bool is_visible = true;
  // True while position/size changes are being applied together with the
  // parent's commit (see cef_view_set_geometry).
  bool geometry_synced = false;
  // Visible part of the page as shares of the buffer (see cef_view_set_geometry).
  float crop_u0 = 0, crop_v0 = 0, crop_u1 = 1, crop_v1 = 1;
  bool is_above = false;
  int last_fd = -1;
  uint64_t last_size = 0;
  uint32_t last_stride = 0;
  uint64_t last_offset = 0;
  int32_t last_buf_w = 0;
  int32_t last_buf_h = 0;
  std::recursive_mutex wayland_mu;

  void prune_buffer_pool() {
    if (buffer_pool.size() <= 6) return;
    auto oldest = buffer_pool.end();
    uint64_t oldest_frame = UINT64_MAX;
    for (auto it = buffer_pool.begin(); it != buffer_pool.end(); ++it) {
      if (!it->second->in_use.load(std::memory_order_acquire) &&
          it->second->buffer != current_buffer) {
        if (it->second->last_used_frame < oldest_frame) {
          oldest_frame = it->second->last_used_frame;
          oldest = it;
        }
      }
    }
    if (oldest != buffer_pool.end()) {
      buffer_pool.erase(oldest);
    }
  }

  void request_frame_callback_locked(struct wl_event_queue* queue = nullptr);

#ifdef STRIP_WAYLAND_VIEWPORTER
  // Crop the attached buffer to the visible part. The source rectangle is in
  // buffer pixels and must lie inside the buffer the next commit carries, so
  // this runs whenever the crop changes AND whenever a new buffer is attached.
  void apply_crop_locked(int32_t buf_w, int32_t buf_h) {
    if (!viewport) return;
    const bool full = crop_u0 <= 0 && crop_v0 <= 0 && crop_u1 >= 1 && crop_v1 >= 1;
    if (full || buf_w <= 0 || buf_h <= 0) {
      wl_fixed_t none = wl_fixed_from_int(-1);
      wp_viewport_set_source(viewport, none, none, none, none);
      return;
    }
    // 24.8 fixed point, rounded down so the rectangle never pokes past the
    // buffer edge (that is a protocol error, fatal for the whole client).
    auto fx = [](float share, int32_t size) {
      return static_cast<int32_t>(std::floor(static_cast<double>(share) * size * 256.0));
    };
    int32_t x0 = fx(crop_u0, buf_w), y0 = fx(crop_v0, buf_h);
    int32_t x1 = std::min(fx(crop_u1, buf_w), buf_w * 256);
    int32_t y1 = std::min(fx(crop_v1, buf_h), buf_h * 256);
    x0 = std::min(x0, buf_w * 256 - 1);
    y0 = std::min(y0, buf_h * 256 - 1);
    x1 = std::max(x1, x0 + 1);
    y1 = std::max(y1, y0 + 1);
    wp_viewport_set_source(viewport, x0, y0, x1 - x0, y1 - y0);
  }
#endif

  void on_vblank_done(uint32_t) {
    CefRefPtr<CefBrowser> b;
    {
      std::lock_guard<std::recursive_mutex> lk(wayland_mu);
      frame_callback = nullptr;
      frame_callback_pending = false;
      b = browser;
    }
    if (b) {
      CefPostTask(TID_UI, base::BindOnce([](CefRefPtr<CefBrowser> b) {
        if (b) b->GetHost()->SendExternalBeginFrame();
      }, b));
    }
  }

  void set_frame_rate(int32_t fps) {
    CefRefPtr<CefBrowser> b;
    {
      std::lock_guard<std::recursive_mutex> lk(wayland_mu);
      b = browser;
    }
    if (b) {
      CefPostTask(TID_UI, base::BindOnce([](CefRefPtr<CefBrowser> b, int32_t rate) {
        if (b) b->GetHost()->SetWindowlessFrameRate(rate);
      }, b, fps));
    }
  }

  ~View() override {
    std::lock_guard<std::recursive_mutex> lk(wayland_mu);
    if (frame_callback) {
      wl_callback_destroy(frame_callback);
      frame_callback = nullptr;
    }
    buffer_pool.clear();
    current_buffer = nullptr;
    if (last_fd >= 0) {
      close(last_fd);
      last_fd = -1;
    }
#ifdef STRIP_WAYLAND_VIEWPORTER
    if (viewport) {
      wp_viewport_destroy(viewport);
      viewport = nullptr;
    }
#endif
    if (subsurface) {
      wl_subsurface_destroy(subsurface);
      subsurface = nullptr;
    }
    if (child_surface) {
      wl_surface_destroy(child_surface);
      child_surface = nullptr;
    }
  }
#endif

  IMPLEMENT_REFCOUNTING(View);
};

using ViewRef = CefRefPtr<View>;

#ifdef STRIP_WAYLAND_DMABUF
static void view_frame_done(void* data, struct wl_callback* cb, uint32_t time) {
  wl_callback_destroy(cb);
  View* v = static_cast<View*>(data);
  if (v) {
    v->on_vblank_done(time);
  }
}
static const struct wl_callback_listener view_frame_listener = {
    view_frame_done,
};

static void pooled_buffer_release(void* data, struct wl_buffer*) {
  PooledBuffer* pb = static_cast<PooledBuffer*>(data);
  if (pb) {
    pb->in_use.store(false, std::memory_order_release);
  }
}
static const struct wl_buffer_listener pooled_buffer_listener = {
    pooled_buffer_release,
};

void View::request_frame_callback_locked(struct wl_event_queue* queue) {
  if (!frame_callback_pending && child_surface) {
    frame_callback = wl_surface_frame(child_surface);
    if (frame_callback) {
      if (queue) {
        wl_proxy_set_queue(reinterpret_cast<struct wl_proxy*>(frame_callback), queue);
      } else {
        wl_proxy_set_queue(reinterpret_cast<struct wl_proxy*>(frame_callback), nullptr);
      }
      wl_callback_add_listener(frame_callback, &view_frame_listener, this);
      frame_callback_pending = true;
    }
  }
}
#endif

// ---------------------------------------------------------------------------
// Handlers. Created per view; CEF holds refs via the client. Handlers hold
// ViewRef so callbacks are always safe.
// ---------------------------------------------------------------------------

struct RenderHandler : public CefRenderHandler {
  explicit RenderHandler(ViewRef v) : view(std::move(v)) {}

  CefRefPtr<CefAccessibilityHandler> GetAccessibilityHandler() override {
    return nullptr;
  }

  void GetViewRect(CefRefPtr<CefBrowser>, CefRect& rect) override {
    std::lock_guard<std::mutex> lk(view->geom_mu);
    rect = CefRect(0, 0, view->w > 0 ? view->w : 1, view->h > 0 ? view->h : 1);
  }

  bool GetScreenInfo(CefRefPtr<CefBrowser>, CefScreenInfo& info) override {
    info.device_scale_factor = view->dsf;
    return true;
  }

  bool GetScreenPoint(CefRefPtr<CefBrowser>, int viewX, int viewY,
                      int& screenX, int& screenY) override {
    screenX = viewX;
    screenY = viewY;
    return true;
  }

  void OnPopupShow(CefRefPtr<CefBrowser>, bool show) override {
    if (!show) {
      std::lock_guard<std::mutex> lk(view->popup.mu);
      view->popup.h = 0;  // not visible; buffer contents retained
      view->popup.damage = Rect();
      emit_frame(view->id, true, 0, 0, Rect(), false);  // w==0 => hide popup layer
    }
  }

  void OnPopupSize(CefRefPtr<CefBrowser>, const CefRect& rect) override {
    std::lock_guard<std::mutex> lk(view->popup_geom_mu);
    view->popup_geom = rect;
  }

  void OnPaint(CefRefPtr<CefBrowser>, PaintElementType type,
               const RectList& dirtyRects, const void* buffer, int width,
               int height) override {
    if (!buffer || width <= 0 || height <= 0) return;
    Rect dirty;
    for (const CefRect& r : dirtyRects) dirty.unite(r);

    if (type == PET_VIEW) {
      auto r = view->frame.store(static_cast<const uint8_t*>(buffer), width,
                                 height, dirty);
      emit_frame(view->id, false, width, height, r.rects, r.full);
    } else {
      int32_t px = 0, py = 0;
      {
        std::lock_guard<std::mutex> lk(view->popup_geom_mu);
        px = view->popup_geom.x;
        py = view->popup_geom.y;
      }
      auto r = view->popup.store(static_cast<const uint8_t*>(buffer), width,
                                 height, dirty);
      Rect vd = r.rects;
      if (!r.full) {
        vd.x += px; vd.y += py;  // popup buffer coords -> view coords
      }
      emit_frame(view->id, true, width, height, vd, r.full);
    }
  }

  void OnAcceleratedPaint(CefRefPtr<CefBrowser> browser, PaintElementType type,
                          const RectList& dirtyRects,
                          const CefAcceleratedPaintInfo& info) override {
    if (type != PET_VIEW || info.plane_count < 1) return;

#ifdef STRIP_WAYLAND_DMABUF
    {
      std::lock_guard<std::recursive_mutex> lk_wl(g_wl.mu);
      if (g_wl.dmabuf && view->child_surface) {
        std::lock_guard<std::recursive_mutex> lk(view->wayland_mu);

        uint64_t plane_size = info.planes[0].size;
        if (plane_size == 0 && info.planes[0].fd >= 0) {
          off_t sz = lseek(info.planes[0].fd, 0, SEEK_END);
          if (sz > 0) plane_size = static_cast<uint64_t>(sz);
        }

        uint32_t buf_w = 0;
        uint32_t buf_h = 0;

        if (info.extra.coded_size.width > 0 && info.extra.coded_size.height > 0) {
          buf_w = static_cast<uint32_t>(info.extra.coded_size.width);
          buf_h = static_cast<uint32_t>(info.extra.coded_size.height);
        } else if (info.extra.visible_rect.width > 0 && info.extra.visible_rect.height > 0) {
          buf_w = static_cast<uint32_t>(info.extra.visible_rect.width);
          buf_h = static_cast<uint32_t>(info.extra.visible_rect.height);
        } else {
          buf_w = view->w > 0 ? view->w : 1;
          buf_h = view->h > 0 ? view->h : 1;
        }

        if (info.planes[0].stride > 0 && plane_size > info.planes[0].offset) {
          uint32_t max_lines = static_cast<uint32_t>((plane_size - info.planes[0].offset) / info.planes[0].stride);
          if (buf_h > max_lines && max_lines > 0) {
            buf_h = max_lines;
          }
          uint32_t max_cols = info.planes[0].stride / 4;
          if (buf_w > max_cols && max_cols > 0) {
            buf_w = max_cols;
          }
        }

        // Persistent buffer pool: lookup or import dmabuf once
        BufferKey bkey;
        struct stat st{};
        if (info.planes[0].fd >= 0 && fstat(info.planes[0].fd, &st) == 0) {
          bkey.inode = st.st_ino;
        } else {
          bkey.inode = static_cast<uint64_t>(info.planes[0].fd);
        }
        bkey.width = buf_w;
        bkey.height = buf_h;
        bkey.stride = info.planes[0].stride;
        bkey.modifier = info.modifier;
        bkey.offset = info.planes[0].offset;

        PooledBuffer* pb = nullptr;
        auto it = view->buffer_pool.find(bkey);
        if (it != view->buffer_pool.end()) {
          pb = it->second.get();
        } else {
          struct zwp_linux_buffer_params_v1* params =
              zwp_linux_dmabuf_v1_create_params(g_wl.dmabuf);
          wl_proxy_set_queue(reinterpret_cast<struct wl_proxy*>(params), g_wl.queue);

          uint32_t mod_hi = static_cast<uint32_t>((info.modifier >> 32) & 0xFFFFFFFF);
          uint32_t mod_lo = static_cast<uint32_t>(info.modifier & 0xFFFFFFFF);

          zwp_linux_buffer_params_v1_add(params, info.planes[0].fd, 0,
                                         static_cast<uint32_t>(info.planes[0].offset),
                                         info.planes[0].stride,
                                         mod_hi, mod_lo);

          uint32_t drm_format = 0x34325241; // DRM_FORMAT_ARGB8888
          struct wl_buffer* buf = zwp_linux_buffer_params_v1_create_immed(
              params, buf_w, buf_h, drm_format, 0);
          zwp_linux_buffer_params_v1_destroy(params);

          if (buf) {
            wl_proxy_set_queue(reinterpret_cast<struct wl_proxy*>(buf), nullptr);
            auto new_pb = std::make_unique<PooledBuffer>();
            new_pb->buffer = buf;
            new_pb->key = bkey;
            new_pb->in_use.store(false, std::memory_order_relaxed);
            wl_buffer_add_listener(buf, &pooled_buffer_listener, new_pb.get());
            pb = new_pb.get();
            view->buffer_pool[bkey] = std::move(new_pb);
          }
        }

        if (pb && pb->buffer) {
          pb->last_used_frame = ++view->frame_seq;
          // A page the shell has taken off screen (clipped away, or on another
          // workspace) only records the frame: attaching would put it back on
          // screen at its last place. cef_view_set_geometry re-attaches
          // current_buffer when the page is shown again.
          if (view->is_visible) {
            pb->in_use.store(true, std::memory_order_release);
            wl_surface_attach(view->child_surface, pb->buffer, 0, 0);
            wl_surface_damage(view->child_surface, 0, 0, INT32_MAX, INT32_MAX);
#ifdef STRIP_WAYLAND_VIEWPORTER
            view->apply_crop_locked(static_cast<int32_t>(buf_w), static_cast<int32_t>(buf_h));
            // Until set_geometry has chosen a size, show the buffer at the
            // webview's size. After that the destination is the shell's (the
            // overview shows pages scaled), so frames must not reset it.
            if (view->viewport && view->last_w == 0 && view->w > 0 && view->h > 0) {
              wp_viewport_set_destination(view->viewport, view->w, view->h);
            }
#endif
            // Align next frame deadline with Wayland vblank
            view->request_frame_callback_locked(nullptr);
            wl_surface_commit(view->child_surface);
          }
          view->current_buffer = pb->buffer;
          // Prune only now: a fresh entry has last_used_frame 0 until it is
          // attached, so pruning earlier evicted the buffer we were about to
          // attach (use-after-free in wl_surface_attach).
          view->prune_buffer_pool();

          if (view->last_fd >= 0) close(view->last_fd);
          view->last_fd = dup(info.planes[0].fd);
          view->last_size = plane_size;
          view->last_stride = info.planes[0].stride;
          view->last_offset = info.planes[0].offset;
          view->last_buf_w = static_cast<int32_t>(buf_w);
          view->last_buf_h = static_cast<int32_t>(buf_h);
        }

        wl_display_flush(g_wl.display);
        wl_display_dispatch_queue_pending(g_wl.display, g_wl.queue);

        emit_frame(view->id, false, buf_w, buf_h, Rect{0, 0, static_cast<int32_t>(buf_w), static_cast<int32_t>(buf_h)}, true);
        return;
      }
    }
#endif
  }

  ViewRef view;
 private:
  IMPLEMENT_REFCOUNTING(RenderHandler);
};

struct DisplayHandler : public CefDisplayHandler {
  explicit DisplayHandler(ViewRef v) : view(std::move(v)) {}
  void OnTitleChange(CefRefPtr<CefBrowser>, const CefString& title) override {
    std::string t = title.ToString();
    emit(CEF_EV_TITLE, view->id, t.c_str());
  }
  void OnAddressChange(CefRefPtr<CefBrowser>, CefRefPtr<CefFrame>,
                       const CefString& url) override {
    std::string u = url.ToString();
    emit(CEF_EV_URL, view->id, u.c_str());
  }
  bool OnCursorChange(CefRefPtr<CefBrowser>,
                      CefCursorHandle,
                      cef_cursor_type_t type,
                      const CefCursorInfo&) override {
    cef_sink_fn fn = g_sink.load(std::memory_order_acquire);
    if (!fn) return false;
    cef_event_t ev{};
    ev.kind = CEF_EV_CURSOR;
    ev.view_id = view->id;
    ev.cursor_type = static_cast<int32_t>(type);
    fn(&ev, g_sink_ud.load(std::memory_order_relaxed));
    return true;
  }
  ViewRef view;
 private:
  IMPLEMENT_REFCOUNTING(DisplayHandler);
};

// CEF 154 has no "page is audible" callback; the audio handler's stream
// lifecycle is the only signal. We never read the PCM (OnAudioStreamPacket is
// empty): Chromium keeps playing the page through the system output while
// this handler runs, and hidden pages keep their streams (WasHidden does not
// stop audio), so the indicator stays right for background pages.
struct AudioHandler : public CefAudioHandler {
  explicit AudioHandler(ViewRef v) : view(std::move(v)) {}

  bool GetAudioParameters(CefRefPtr<CefBrowser>, CefAudioParameters&) override {
    return true;
  }
  void OnAudioStreamStarted(CefRefPtr<CefBrowser>, const CefAudioParameters&,
                            int) override {
    if (view->audio_streams.fetch_add(1) == 0) emit_audio(true);
  }
  void OnAudioStreamPacket(CefRefPtr<CefBrowser>, const float**, int,
                           int64_t) override {}
  void OnAudioStreamStopped(CefRefPtr<CefBrowser>) override {
    int32_t n = view->audio_streams.load();
    while (n > 0 && !view->audio_streams.compare_exchange_weak(n, n - 1)) {
    }
    if (n == 1) emit_audio(false);
  }
  void OnAudioStreamError(CefRefPtr<CefBrowser>, const CefString&) override {}

  void emit_audio(bool playing) {
    cef_sink_fn fn = g_sink.load(std::memory_order_acquire);
    if (!fn) return;
    cef_event_t ev{};
    ev.kind = CEF_EV_AUDIO;
    ev.view_id = view->id;
    ev.audio_playing = playing ? 1 : 0;
    fn(&ev, g_sink_ud.load(std::memory_order_relaxed));
  }

  ViewRef view;
 private:
  IMPLEMENT_REFCOUNTING(AudioHandler);
};

struct LoadHandler : public CefLoadHandler {
  explicit LoadHandler(ViewRef v) : view(std::move(v)) {}
  void OnLoadingStateChange(CefRefPtr<CefBrowser>, bool isLoading, bool,
                            bool) override {
    cef_sink_fn fn = g_sink.load(std::memory_order_acquire);
    if (!fn) return;
    cef_event_t ev{};
    ev.kind = CEF_EV_LOADING;
    ev.view_id = view->id;
    ev.loading = isLoading ? 1 : 0;
    fn(&ev, g_sink_ud.load(std::memory_order_relaxed));
  }
  ViewRef view;
 private:
  IMPLEMENT_REFCOUNTING(LoadHandler);
};

struct LifeSpanHandler : public CefLifeSpanHandler {
  explicit LifeSpanHandler(ViewRef v) : view(std::move(v)) {}

  void OnAfterCreated(CefRefPtr<CefBrowser> browser) override {
    view->browser = browser;
    browser->GetHost()->SendExternalBeginFrame();
  }

  void OnBeforeClose(CefRefPtr<CefBrowser>) override {
    emit(CEF_EV_CLOSED, view->id, nullptr);
    view->browser = nullptr;
  }

  bool OnBeforePopup(CefRefPtr<CefBrowser>, CefRefPtr<CefFrame>, int,
                     const CefString& target_url, const CefString&,
                     WindowOpenDisposition, bool, const CefPopupFeatures&,
                     CefWindowInfo&, CefRefPtr<CefClient>&,
                     CefBrowserSettings&, CefRefPtr<CefDictionaryValue>&,
                     bool*) override {
    // No OS popups: navigate the same view instead (the strip model owns
    // page creation). Returning true cancels popup creation.
    CefRefPtr<CefFrame> frame = view->browser
        ? view->browser->GetMainFrame() : nullptr;
    if (frame) {
      CefPostTask(TID_UI, base::BindOnce(
          [](CefRefPtr<CefFrame> f, std::string url) {
            f->LoadURL(url);
          }, frame, target_url.ToString()));
    }
    return true;
  }

  ViewRef view;
 private:
  IMPLEMENT_REFCOUNTING(LifeSpanHandler);
};

struct FocusHandler : public CefFocusHandler {
  explicit FocusHandler(ViewRef v) : view(std::move(v)) {}
  void OnTakeFocus(CefRefPtr<CefBrowser>, bool) override {}
  bool OnSetFocus(CefRefPtr<CefBrowser>, FocusSource) override {
    return false;
  }
  ViewRef view;
 private:
  IMPLEMENT_REFCOUNTING(FocusHandler);
};

struct Client : public CefClient {
  explicit Client(ViewRef v)
      : render(new RenderHandler(v)), display(new DisplayHandler(v)),
        load(new LoadHandler(v)), lifespan(new LifeSpanHandler(v)),
        focus(new FocusHandler(v)), audio(new AudioHandler(v)) {}

  CefRefPtr<CefRenderHandler> GetRenderHandler() override { return render; }
  CefRefPtr<CefDisplayHandler> GetDisplayHandler() override { return display; }
  CefRefPtr<CefLoadHandler> GetLoadHandler() override { return load; }
  CefRefPtr<CefLifeSpanHandler> GetLifeSpanHandler() override { return lifespan; }
  CefRefPtr<CefFocusHandler> GetFocusHandler() override { return focus; }
  CefRefPtr<CefAudioHandler> GetAudioHandler() override { return audio; }

  CefRefPtr<RenderHandler> render;
  CefRefPtr<DisplayHandler> display;
  CefRefPtr<LoadHandler> load;
  CefRefPtr<LifeSpanHandler> lifespan;
  CefRefPtr<FocusHandler> focus;
  CefRefPtr<AudioHandler> audio;
 private:
  IMPLEMENT_REFCOUNTING(Client);
};

std::mutex g_views_mu;
std::map<uint64_t, ViewRef> g_views;

// ---------------------------------------------------------------------------
// App / engine
// ---------------------------------------------------------------------------

class StripApp : public CefApp, public CefBrowserProcessHandler {
 public:
  StripApp() = default;

  void OnBeforeCommandLineProcessing(const CefString&,
                                     CefRefPtr<CefCommandLine> cl) override {
    // Privacy/hardening posture (Helium-family defaults; prebuilt binaries
    // carry no Helium source patches — see decisions.tsv).
    // Smooth scrolling is Chromium's default on Linux; the shell forwards raw
    // wheel deltas and lets Chromium run the only scroll animation.
    cl->AppendSwitchWithValue("force-color-profile", "srgb");
    cl->AppendSwitchWithValue("disable-features", "Translate,BackForwardCache");
    cl->AppendSwitch("disable-background-timer-throttling");
  }

  void OnScheduleMessagePumpWork(int64_t) override {
    // multi_threaded_message_loop: CEF owns its pump; nothing to do.
  }

  CefRefPtr<CefBrowserProcessHandler> GetBrowserProcessHandler() override {
    return this;
  }

 private:
  IMPLEMENT_REFCOUNTING(StripApp);
  DISALLOW_COPY_AND_ASSIGN(StripApp);
};

std::atomic<bool> g_started{false};
CefRefPtr<StripApp> g_app;

}  // namespace

// ---------------------------------------------------------------------------
// C ABI
// ---------------------------------------------------------------------------

int cef_early_process(int argc, char** argv) {
  CefMainArgs args(argc, argv);
  // Heap-allocated: CefExecuteProcess drops the last ref on return, which
  // runs `delete this` (IMPLEMENT_REFCOUNTING). A `static` StripApp would be
  // freed as if it were heap memory -> free(): invalid size.
  CefRefPtr<StripApp> app = new StripApp();
  return CefExecuteProcess(args, app, nullptr);
}

void cef_set_sink(cef_sink_fn fn, void* ud) {
  g_sink_ud.store(ud, std::memory_order_relaxed);
  g_sink.store(fn, std::memory_order_release);
}

int cef_engine_start(const char* subprocess, const char* resources,
                     const char* locales, const char* cache) {
  bool expected = false;
  if (!g_started.compare_exchange_strong(expected, true)) return 0;

  g_app = new StripApp();
  CefMainArgs args(0, nullptr);
  CefSettings settings;
  settings.multi_threaded_message_loop = true;   // CEF owns the UI thread
  settings.windowless_rendering_enabled = true;
  settings.no_sandbox = true;
  settings.log_severity = LOGSEVERITY_WARNING;
  if (subprocess && *subprocess)
    CefString(&settings.browser_subprocess_path).FromString(subprocess);
  if (resources && *resources)
    CefString(&settings.resources_dir_path).FromString(resources);
  if (locales && *locales)
    CefString(&settings.locales_dir_path).FromString(locales);
  if (cache && *cache)
    CefString(&settings.cache_path).FromString(cache);
  CefString(&settings.user_agent).FromASCII(CEF_UA);
  if (!CefInitialize(args, settings, g_app, nullptr)) return -1;
  return 0;
}

int32_t get_target_frame_rate() {
  int32_t fps = g_target_fps.load(std::memory_order_relaxed);
  if (fps >= 24 && fps <= 360) return fps;
  const char* env_fps = getenv("STRIP_FPS");
  if (!env_fps) env_fps = getenv("STRIP_REFRESH_RATE");
  if (env_fps) {
    int f = atoi(env_fps);
    if (f >= 24 && f <= 360) return f;
  }
  return 144; // Default to 144 Hz (matching user's 144.15 Hz display)
}

void cef_set_target_frame_rate(int32_t fps) {
  if (fps < 24 || fps > 360) return;
  int32_t old_fps = g_target_fps.exchange(fps);
  if (old_fps == fps) return;
  fprintf(stderr, "[CEF SHIM] Target frame rate set to %d FPS\n", fps);
  std::lock_guard<std::mutex> lk(g_views_mu);
  for (auto& kv : g_views) {
    ViewRef v = kv.second;
    if (v && v->browser) {
      CefPostTask(TID_UI, base::BindOnce([](ViewRef v, int32_t rate) {
        if (v && v->browser) v->browser->GetHost()->SetWindowlessFrameRate(rate);
      }, v, fps));
    }
  }
}

int32_t cef_get_target_frame_rate() {
  return get_target_frame_rate();
}

void cef_view_set_frame_rate(void* view, int32_t fps) {
  View* raw = static_cast<View*>(view);
  if (!raw || fps < 1 || fps > 360) return;
  ViewRef v(raw);
  CefPostTask(TID_UI, base::BindOnce([](ViewRef v, int32_t rate) {
    if (v && v->browser) v->browser->GetHost()->SetWindowlessFrameRate(rate);
  }, v, fps));
}

void* cef_view_create(uint64_t id, const char* url, int32_t w, int32_t h) {
  ViewRef v = new View();
  v->id = id;
  v->w = w;
  v->h = h;
  CefRefPtr<Client> client = new Client(v);
  {
    std::lock_guard<std::mutex> lk(g_views_mu);
    g_views[id] = v;
  }

  CefWindowInfo info;
  info.SetAsWindowless(cef_window_handle_t());
  info.shared_texture_enabled = 1;
#ifdef STRIP_WAYLAND_DMABUF
  info.external_begin_frame_enabled = 0;
#endif
  CefBrowserSettings bs;
  bs.windowless_frame_rate = get_target_frame_rate();

  CefPostTask(TID_UI, base::BindOnce(
      [](CefRefPtr<Client> client, CefWindowInfo info,
         CefBrowserSettings bs, std::string url) {
        CefBrowserHost::CreateBrowser(info, client, url, bs, nullptr,
                                      nullptr);
      },
      client, info, bs, std::string(url)));

  cef_view_attach_wayland(v.get());
  // Ownership: g_views holds the only view ref for Rust's purposes; the raw
  // pointer stays valid until cef_view_destroy erases it from the map.
  return v.get();
}

void cef_view_destroy(void* view) {
  // Take ownership out of the map; the posted close lambda keeps the view
  // (and its handlers, and therefore the CefBrowser teardown path) alive
  // until CEF has fully closed it on the UI thread.
  ViewRef v;
  {
    std::lock_guard<std::mutex> lk(g_views_mu);
    auto it = g_views.find(reinterpret_cast<uintptr_t>(view) ? 0 : 0);
    (void)it;
    // The raw void* IS the view pointer Rust holds; find by pointer value.
    for (auto& kv : g_views) {
      if (kv.second.get() == view) {
        v = kv.second;
        g_views.erase(kv.first);
        break;
      }
    }
  }
  if (!v) return;
  CefPostTask(TID_UI, base::BindOnce(
      [](ViewRef v) {
        if (v->browser) v->browser->GetHost()->CloseBrowser(true);
      }, v));
}

const uint8_t* cef_view_lock_frame(void* view, int32_t* w, int32_t* h) {
  View* v = static_cast<View*>(view);
  if (!v) return nullptr;
  v->frame.mu.lock();
  *w = v->frame.w;
  *h = v->frame.h;
  return v->frame.px.data();
}
void cef_view_unlock_frame(void* view) {
  static_cast<View*>(view)->frame.mu.unlock();
}
const uint8_t* cef_view_lock_popup(void* view, int32_t* w, int32_t* h) {
  View* v = static_cast<View*>(view);
  if (!v) return nullptr;
  v->popup.mu.lock();
  *w = v->popup.w;
  *h = v->popup.h;
  return v->popup.px.data();
}
void cef_view_unlock_popup(void* view) {
  static_cast<View*>(view)->popup.mu.unlock();
}
int32_t cef_view_popup_visible(void* view) {
  View* v = static_cast<View*>(view);
  return v && v->popup.h > 0 ? 1 : 0;
}
void cef_view_popup_rect(void* view, int32_t out[4]) {
  View* v = static_cast<View*>(view);
  if (!v) return;
  std::lock_guard<std::mutex> lk(v->popup_geom_mu);
  out[0] = v->popup_geom.x;
  out[1] = v->popup_geom.y;
  out[2] = v->popup_geom.width;
  out[3] = v->popup_geom.height;
}

// All commands below re-resolve the view by id on the UI thread, so they are
// safe during async browser creation and after destroy (view simply gone).

void cef_view_navigate(void* view, const char* url) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  ViewRef v(raw);
  CefPostTask(TID_UI, base::BindOnce(
      [](ViewRef v, std::string u) {
        if (v && v->browser) {
          v->browser->GetMainFrame()->LoadURL(u);
          v->browser->GetHost()->SendExternalBeginFrame();
        }
      }, v, std::string(url)));
}

void cef_view_back(void* view) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  ViewRef v(raw);
  CefPostTask(TID_UI, base::BindOnce([](ViewRef v) {
    if (v && v->browser) {
      v->browser->GoBack();
      v->browser->GetHost()->SendExternalBeginFrame();
    }
  }, v));
}

void cef_view_forward(void* view) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  ViewRef v(raw);
  CefPostTask(TID_UI, base::BindOnce([](ViewRef v) {
    if (v && v->browser) {
      v->browser->GoForward();
      v->browser->GetHost()->SendExternalBeginFrame();
    }
  }, v));
}

void cef_view_reload(void* view, int hard) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  ViewRef v(raw);
  CefPostTask(TID_UI, base::BindOnce([](ViewRef v, int hard) {
    if (!v || !v->browser) return;
    if (hard) v->browser->ReloadIgnoreCache();
    else v->browser->Reload();
    v->browser->GetHost()->SendExternalBeginFrame();
  }, v, hard));
}

void cef_view_resize(void* view, int32_t w, int32_t h) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  {
    std::lock_guard<std::mutex> lk(raw->geom_mu);
    if (raw->w == w && raw->h == h) return;
    raw->w = w;
    raw->h = h;
  }
  ViewRef v(raw);
  CefPostTask(TID_UI, base::BindOnce([](ViewRef v) {
    if (v && v->browser) {
      v->browser->GetHost()->WasResized();
      v->browser->GetHost()->SendExternalBeginFrame();
    }
  }, v));
}

void cef_view_focus(void* view, int focus) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  ViewRef v(raw);
  CefPostTask(TID_UI, base::BindOnce([](ViewRef v, int f) {
    if (v && v->browser) v->browser->GetHost()->SetFocus(f != 0);
  }, v, focus));
}

void cef_view_hidden(void* view, int hidden) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  ViewRef v(raw);
  CefPostTask(TID_UI, base::BindOnce([](ViewRef v, int h) {
    if (!v || !v->browser) return;
    v->browser->GetHost()->WasHidden(h != 0);
    // A page shown again (workspace slide, overview) repaints right away.
    if (!h) v->browser->GetHost()->Invalidate(PET_VIEW);
  }, v, hidden));
}

void cef_view_set_muted(void* view, int muted) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  ViewRef v(raw);
  CefPostTask(TID_UI, base::BindOnce([](ViewRef v, int m) {
    if (v && v->browser) v->browser->GetHost()->SetAudioMuted(m != 0);
  }, v, muted));
}

void cef_view_mouse(void* view, int kind, int button, int x, int y,
                    int click_count, int modifiers) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  ViewRef v(raw);

  if (kind == 0) {
    // Coalesce mouse moves to avoid saturating the CEF UI thread task queue.
    v->pending_move_x.store(x, std::memory_order_relaxed);
    v->pending_move_y.store(y, std::memory_order_relaxed);
    v->pending_move_mods.store(static_cast<uint32_t>(modifiers), std::memory_order_relaxed);

    if (!v->has_pending_move.exchange(true, std::memory_order_acq_rel)) {
      CefPostTask(TID_UI, base::BindOnce(
          [](ViewRef v) {
            if (!v || !v->browser) return;

            int px = v->pending_move_x.load(std::memory_order_relaxed);
            int py = v->pending_move_y.load(std::memory_order_relaxed);
            uint32_t kmods = v->pending_move_mods.load(std::memory_order_relaxed);
            uint32_t bmods = v->mouse_button_modifiers.load(std::memory_order_relaxed);
            v->has_pending_move.store(false, std::memory_order_release);

            CefMouseEvent ev;
            ev.x = px;
            ev.y = py;
            ev.modifiers = kmods | bmods;
            v->browser->GetHost()->SendMouseMoveEvent(ev, false);
            v->browser->GetHost()->SendExternalBeginFrame();
          }, v));
    }
    return;
  }

  // Mouse down / up: update button tracking and dispatch immediately.
  uint32_t btn_flag = (button == 0) ? EVENTFLAG_LEFT_MOUSE_BUTTON
                    : (button == 1) ? EVENTFLAG_MIDDLE_MOUSE_BUTTON
                    : EVENTFLAG_RIGHT_MOUSE_BUTTON;
  if (kind == 1) {
    v->mouse_button_modifiers.fetch_or(btn_flag, std::memory_order_relaxed);
  } else if (kind == 2) {
    v->mouse_button_modifiers.fetch_and(~btn_flag, std::memory_order_relaxed);
  }

  uint32_t total_mods = static_cast<uint32_t>(modifiers) |
                        v->mouse_button_modifiers.load(std::memory_order_relaxed);

  CefPostTask(TID_UI, base::BindOnce(
      [](ViewRef v, int kind, int button, int x, int y, int cc, uint32_t mods) {
        if (!v || !v->browser) return;
        v->has_pending_move.store(false, std::memory_order_release);
        CefMouseEvent ev;
        ev.x = x;
        ev.y = y;
        ev.modifiers = mods;
        bool up = (kind == 2);
        v->browser->GetHost()->SendMouseClickEvent(
            ev, button == 0 ? MBT_LEFT : button == 1 ? MBT_MIDDLE : MBT_RIGHT,
            up, cc);
        v->browser->GetHost()->SendExternalBeginFrame();
      }, v, kind, button, x, y, click_count, total_mods));
}

void cef_view_mouse_leave(void* view) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  ViewRef v(raw);
  CefPostTask(TID_UI, base::BindOnce(
      [](ViewRef v) {
        if (!v || !v->browser) return;
        v->has_pending_move.store(false, std::memory_order_release);
        CefMouseEvent ev;
        ev.x = -1;
        ev.y = -1;
        ev.modifiers = 0;
        v->browser->GetHost()->SendMouseMoveEvent(ev, true);
        v->browser->GetHost()->SendExternalBeginFrame();
      }, v));
}

void cef_view_wheel(void* view, int x, int y, int dx, int dy) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  ViewRef v(raw);
  uint32_t bmods = v->mouse_button_modifiers.load(std::memory_order_relaxed);
  CefPostTask(TID_UI, base::BindOnce(
      [](ViewRef v, int x, int y, int dx, int dy, uint32_t mods) {
        if (!v || !v->browser) return;
        CefMouseEvent ev;
        ev.x = x;
        ev.y = y;
        ev.modifiers = mods;
        v->browser->GetHost()->SendMouseWheelEvent(ev, dx, dy);
        v->browser->GetHost()->SendExternalBeginFrame();
      }, v, x, y, dx, dy, bmods));
}

void cef_view_key(void* view, int type, int windows_key_code,
                  int native_key_code, uint32_t mods, uint16_t ch16) {
  View* raw = static_cast<View*>(view);
  if (!raw) return;
  ViewRef v(raw);
  CefPostTask(TID_UI, base::BindOnce(
      [](ViewRef v, int type, int wkc, int nkc, uint32_t mods,
         uint16_t ch16) {
        if (!v || !v->browser) return;
        CefKeyEvent ev;
        ev.type = static_cast<cef_key_event_type_t>(type);
        ev.windows_key_code = wkc;
        ev.native_key_code = nkc;
        ev.modifiers = mods;
        ev.character = ch16;
        ev.unmodified_character = ch16;
        v->browser->GetHost()->SendKeyEvent(ev);
        v->browser->GetHost()->SendExternalBeginFrame();
      }, v, type, windows_key_code, native_key_code, mods, ch16));
}

#ifdef STRIP_WAYLAND_DMABUF
static void output_handle_geometry(void*, struct wl_output*, int32_t, int32_t, int32_t, int32_t,
                                   int32_t, const char*, const char*, int32_t) {}
static void output_handle_mode(void* data, struct wl_output*, uint32_t flags,
                               int32_t, int32_t, int32_t refresh) {
  OutputInfo* info = static_cast<OutputInfo*>(data);
  if (info && (flags & WL_OUTPUT_MODE_CURRENT)) {
    int32_t hz = (refresh + 500) / 1000;
    info->refresh_hz = hz;
    if (hz >= 24 && hz <= 360) {
      if (g_target_fps.load(std::memory_order_relaxed) <= 60) {
        cef_set_target_frame_rate(hz);
      }
    }
  }
}
static void output_handle_done(void*, struct wl_output*) {}
static void output_handle_scale(void*, struct wl_output*, int32_t) {}

static const struct wl_output_listener output_listener = {
    output_handle_geometry,
    output_handle_mode,
    output_handle_done,
    output_handle_scale,
};

static void surface_handle_enter(void* data, struct wl_surface*, struct wl_output* output) {
  View* v = static_cast<View*>(data);
  if (!v || !output) return;
  std::lock_guard<std::recursive_mutex> lk(g_wl.mu);
  auto it = g_wl.output_by_ptr.find(output);
  if (it != g_wl.output_by_ptr.end() && it->second) {
    int32_t hz = it->second->refresh_hz;
    if (hz >= 24 && hz <= 360) {
      cef_set_target_frame_rate(hz);
      cef_view_set_frame_rate(v, hz);
    }
  }
}
static void surface_handle_leave(void*, struct wl_surface*, struct wl_output*) {}

static const struct wl_surface_listener surface_listener = {
    surface_handle_enter,
    surface_handle_leave,
};

static void registry_handle_global(void* data, struct wl_registry* registry,
                                   uint32_t name, const char* interface,
                                   uint32_t version) {
  WlContext* ctx = static_cast<WlContext*>(data);
  if (strcmp(interface, "wl_compositor") == 0) {
    ctx->compositor = static_cast<struct wl_compositor*>(
        wl_registry_bind(registry, name, &wl_compositor_interface, std::min<uint32_t>(version, 4)));
  } else if (strcmp(interface, "wl_subcompositor") == 0) {
    ctx->subcompositor = static_cast<struct wl_subcompositor*>(
        wl_registry_bind(registry, name, &wl_subcompositor_interface, 1));
  } else if (strcmp(interface, "zwp_linux_dmabuf_v1") == 0) {
    ctx->dmabuf = static_cast<struct zwp_linux_dmabuf_v1*>(
        wl_registry_bind(registry, name, &zwp_linux_dmabuf_v1_interface, std::min<uint32_t>(version, 3)));
  } else if (strcmp(interface, "wl_output") == 0) {
    struct wl_output* out = static_cast<struct wl_output*>(
        wl_registry_bind(registry, name, &wl_output_interface, std::min<uint32_t>(version, 2)));
    if (out) {
      auto info = std::make_unique<OutputInfo>();
      info->name = name;
      info->output = out;
      info->refresh_hz = 60;
      OutputInfo* raw = info.get();
      ctx->outputs[name] = std::move(info);
      ctx->output_by_ptr[out] = raw;
      wl_output_add_listener(out, &output_listener, raw);
    }
  }
#ifdef STRIP_WAYLAND_VIEWPORTER
  else if (strcmp(interface, "wp_viewporter") == 0) {
    ctx->viewporter = static_cast<struct wp_viewporter*>(
        wl_registry_bind(registry, name, &wp_viewporter_interface, 1));
  }
#endif
}

static void registry_handle_global_remove(void* data, struct wl_registry*, uint32_t name) {
  WlContext* ctx = static_cast<WlContext*>(data);
  if (!ctx) return;
  auto it = ctx->outputs.find(name);
  if (it != ctx->outputs.end()) {
    ctx->output_by_ptr.erase(it->second->output);
    wl_output_destroy(it->second->output);
    ctx->outputs.erase(it);
  }
}

static const struct wl_registry_listener registry_listener = {
    registry_handle_global,
    registry_handle_global_remove,
};
#endif

int cef_wayland_init(void* display, void* parent_surface) {
#ifdef STRIP_WAYLAND_DMABUF
  std::lock_guard<std::recursive_mutex> lk(g_wl.mu);
  g_wl.display = static_cast<struct wl_display*>(display);
  g_wl.parent_surface = static_cast<struct wl_surface*>(parent_surface);
  if (!g_wl.display || !g_wl.parent_surface) return -1;

  g_wl.queue = wl_display_create_queue(g_wl.display);
  if (!g_wl.queue) return -1;

  struct wl_display* display_wrapper = static_cast<struct wl_display*>(
      wl_proxy_create_wrapper(g_wl.display));
  wl_proxy_set_queue(reinterpret_cast<struct wl_proxy*>(display_wrapper), g_wl.queue);

  struct wl_registry* registry = wl_display_get_registry(display_wrapper);
  wl_proxy_wrapper_destroy(display_wrapper);

  wl_registry_add_listener(registry, &registry_listener, &g_wl);
  wl_display_roundtrip_queue(g_wl.display, g_wl.queue);
  wl_display_roundtrip_queue(g_wl.display, g_wl.queue);

  if (!g_wl.compositor || !g_wl.subcompositor || !g_wl.dmabuf) {
    fprintf(stderr, "[CEF SHIM] Missing Wayland protocols: comp=%p subcomp=%p dmabuf=%p\n",
            g_wl.compositor, g_wl.subcompositor, g_wl.dmabuf);
    return -1;
  }
  fprintf(stderr, "[CEF SHIM] Wayland dmabuf subsurface initialized successfully!\n");

  {
    std::lock_guard<std::mutex> lk_views(g_views_mu);
    for (auto& pair : g_views) {
      cef_view_attach_wayland(pair.second.get());
    }
  }

  return 0;
#else
  return -1;
#endif
}

void cef_view_attach_wayland(void* raw_view) {
#ifdef STRIP_WAYLAND_DMABUF
  View* v = static_cast<View*>(raw_view);
  if (!v) return;

  std::lock_guard<std::recursive_mutex> lk_wl(g_wl.mu);
  if (!g_wl.display || !g_wl.compositor || !g_wl.subcompositor || !g_wl.parent_surface) return;

  std::lock_guard<std::recursive_mutex> lk(v->wayland_mu);
  if (v->child_surface) return;

  v->child_surface = wl_compositor_create_surface(g_wl.compositor);
  wl_surface_add_listener(v->child_surface, &surface_listener, v);

  struct wl_region* empty_region = wl_compositor_create_region(g_wl.compositor);
  wl_surface_set_input_region(v->child_surface, empty_region);
  wl_region_destroy(empty_region);

  v->subsurface = wl_subcompositor_get_subsurface(g_wl.subcompositor, v->child_surface, g_wl.parent_surface);
  wl_subsurface_set_desync(v->subsurface);
  wl_subsurface_place_above(v->subsurface, g_wl.parent_surface);
  v->is_above = true;

#ifdef STRIP_WAYLAND_VIEWPORTER
  if (g_wl.viewporter) {
    v->viewport = wp_viewporter_get_viewport(g_wl.viewporter, v->child_surface);
  }
#endif

  wl_surface_commit(v->child_surface);
  wl_display_flush(g_wl.display);
#endif
}

void cef_view_set_geometry(void* raw_view, int32_t x, int32_t y, int32_t w, int32_t h,
                           int32_t visible, int32_t has_overlay,
                           float u0, float v0, float u1, float v1) {
#ifdef STRIP_WAYLAND_DMABUF
  View* v = static_cast<View*>(raw_view);
  if (!v) return;

  std::lock_guard<std::recursive_mutex> lk_wl(g_wl.mu);
  if (!g_wl.display) return;

  std::lock_guard<std::recursive_mutex> lk(v->wayland_mu);
  if (!v->subsurface || !v->child_surface) return;

  if (!visible) {
    if (v->is_visible) {
      v->is_visible = false;
      wl_surface_attach(v->child_surface, nullptr, 0, 0);
      wl_surface_commit(v->child_surface);
      wl_display_dispatch_queue_pending(g_wl.display, g_wl.queue);
    }
    return;
  }

  const bool was_visible = v->is_visible;
  v->is_visible = true;
  if (!was_visible && v->current_buffer) {
    // Shown again after being detached: put the last frame back at once
    // instead of waiting for CEF to paint a new one.
    wl_surface_attach(v->child_surface, v->current_buffer, 0, 0);
    wl_surface_damage(v->child_surface, 0, 0, INT32_MAX, INT32_MAX);
#ifdef STRIP_WAYLAND_VIEWPORTER
    // The buffer may have changed size while detached; the source rectangle
    // must fit the buffer this commit carries.
    v->apply_crop_locked(v->last_buf_w, v->last_buf_h);
#endif
  }

  if (has_overlay) {
    if (v->is_above) {
      wl_subsurface_place_below(v->subsurface, g_wl.parent_surface);
      v->is_above = false;
    }
  } else {
    if (!v->is_above) {
      wl_subsurface_place_above(v->subsurface, g_wl.parent_surface);
      v->is_above = true;
    }
  }

  // The subsurface position is parent state, applied when the shell commits
  // its window; the viewport size is child state, applied on the child's own
  // commit while the subsurface is desynchronized. Landing in different
  // frames would show a page at its new size in its old place. So while the
  // geometry changes, run the subsurface synchronized: the child's commit is
  // cached and applies together with the parent commit that carries the new
  // position. Once the geometry is stable, go back to desync so CEF's frames
  // present immediately.
  const bool moved = v->last_x != x || v->last_y != y;
#ifdef STRIP_WAYLAND_VIEWPORTER
  const bool resized = v->viewport && (v->last_w != w || v->last_h != h);
#else
  const bool resized = false;
#endif
#ifdef STRIP_WAYLAND_VIEWPORTER
  const bool recropped = v->viewport && (v->crop_u0 != u0 || v->crop_v0 != v0 ||
                                          v->crop_u1 != u1 || v->crop_v1 != v1);
  if (recropped) {
    v->crop_u0 = u0; v->crop_v0 = v0; v->crop_u1 = u1; v->crop_v1 = v1;
    v->apply_crop_locked(v->last_buf_w, v->last_buf_h);
  }
#else
  const bool recropped = false;
#endif
  const bool changing = moved || resized || recropped;
  if (changing && !v->geometry_synced) {
    wl_subsurface_set_sync(v->subsurface);
    v->geometry_synced = true;
  }
  if (moved) {
    wl_subsurface_set_position(v->subsurface, x, y);
    v->last_x = x;
    v->last_y = y;
  }
#ifdef STRIP_WAYLAND_VIEWPORTER
  if (resized) {
    wp_viewport_set_destination(v->viewport, w, h);
    v->last_w = w;
    v->last_h = h;
  }
#endif
  if (!changing && v->geometry_synced) {
    wl_subsurface_set_desync(v->subsurface);
    v->geometry_synced = false;
  }

  wl_surface_commit(v->child_surface);
  wl_display_dispatch_queue_pending(g_wl.display, g_wl.queue);
#endif
}

int cef_view_get_screenshot(void* raw_view, uint8_t** out_buf, int32_t* out_w, int32_t* out_h, size_t* out_size) {
#ifdef STRIP_WAYLAND_DMABUF
  View* v = static_cast<View*>(raw_view);
  if (!v || !out_buf || !out_w || !out_h || !out_size) return -1;

  std::lock_guard<std::recursive_mutex> lk(v->wayland_mu);
  if (v->last_fd < 0 || v->last_size == 0 || v->last_buf_w <= 0 || v->last_buf_h <= 0) return -1;

  void* map = mmap(nullptr, v->last_size, PROT_READ, MAP_SHARED, v->last_fd, 0);
  if (map == MAP_FAILED) return -1;

  size_t bytes = static_cast<size_t>(v->last_buf_w) * v->last_buf_h * 4;
  uint8_t* dst = static_cast<uint8_t*>(malloc(bytes));
  if (!dst) {
    munmap(map, v->last_size);
    return -1;
  }

  const uint8_t* src = static_cast<const uint8_t*>(map) + v->last_offset;
  for (int32_t r = 0; r < v->last_buf_h; ++r) {
    memcpy(dst + r * v->last_buf_w * 4, src + r * v->last_stride, v->last_buf_w * 4);
  }
  munmap(map, v->last_size);

  *out_w = v->last_buf_w;
  *out_h = v->last_buf_h;
  *out_size = bytes;
  *out_buf = dst;
  return 0;
#else
  return -1;
#endif
}

void cef_wayland_dispatch() {
#ifdef STRIP_WAYLAND_DMABUF
  std::lock_guard<std::recursive_mutex> lk(g_wl.mu);
  if (g_wl.display) {
    if (g_wl.queue) {
      wl_display_dispatch_queue_pending(g_wl.display, g_wl.queue);
    }
    wl_display_dispatch_pending(g_wl.display);
  }
#endif
}


