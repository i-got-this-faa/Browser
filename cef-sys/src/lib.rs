//! Raw C ABI to the CEF embedding shim (cef-sys/shim/shim.cc).
//!
//! Nothing here is safe API — see the `webview-cef` crate for the Rust-side
//! wrapper. Frames cross as raw BGRA + damage rects; input/focus/metrics
//! travel back over the same boundary. No encoded images anywhere.

#![allow(non_camel_case_types, non_snake_case, clippy::missing_safety_doc)]

use std::os::raw::{c_char, c_int, c_void};

pub type cef_sink_fn = unsafe extern "C" fn(ev: *const CefEvent, ud: *mut c_void);

pub const CEF_EV_FRAME: u32 = 1;
pub const CEF_EV_POPUP_FRAME: u32 = 2;
pub const CEF_EV_TITLE: u32 = 3;
pub const CEF_EV_URL: u32 = 4;
pub const CEF_EV_LOADING: u32 = 5;
pub const CEF_EV_CLOSED: u32 = 6;
pub const CEF_EV_DMABUF: u32 = 7;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct CefEvent {
    pub kind: u32,
    pub view_id: u64,
    pub w: i32,
    pub h: i32,
    pub rects: [[i32; 4]; 16],
    pub nrects: c_int,
    pub loading: c_int,
    pub str_: *const c_char,
    pub dmabuf_fd: c_int,
    pub stride: u32,
    pub offset: u64,
    pub modifier: u64,
    pub drm_format: u32,
}

unsafe extern "C" {
    pub fn cef_early_process(argc: c_int, argv: *mut *mut c_char) -> c_int;
    pub fn cef_set_sink(fn_: cef_sink_fn, ud: *mut c_void);
    pub fn cef_engine_start(
        subprocess: *const c_char,
        resources: *const c_char,
        locales: *const c_char,
        cache: *const c_char,
    ) -> c_int;

    pub fn cef_view_create(id: u64, url: *const c_char, w: i32, h: i32) -> *mut c_void;
    pub fn cef_view_destroy(view: *mut c_void);

    pub fn cef_view_lock_frame(view: *mut c_void, w: *mut i32, h: *mut i32) -> *const u8;
    pub fn cef_view_unlock_frame(view: *mut c_void);
    pub fn cef_view_lock_popup(view: *mut c_void, w: *mut i32, h: *mut i32) -> *const u8;
    pub fn cef_view_unlock_popup(view: *mut c_void);
    pub fn cef_view_popup_visible(view: *mut c_void) -> i32;
    pub fn cef_view_popup_rect(view: *mut c_void, out: *mut i32);

    pub fn cef_view_navigate(view: *mut c_void, url: *const c_char);
    pub fn cef_view_back(view: *mut c_void);
    pub fn cef_view_forward(view: *mut c_void);
    pub fn cef_view_reload(view: *mut c_void, hard: c_int);
    pub fn cef_view_resize(view: *mut c_void, w: i32, h: i32);
    pub fn cef_view_focus(view: *mut c_void, focus: c_int);
    pub fn cef_view_hidden(view: *mut c_void, hidden: c_int);

    /// kind: 0=move 1=down 2=up; button: 0=left 1=middle 2=right
    pub fn cef_view_mouse(
        view: *mut c_void,
        kind: c_int,
        button: c_int,
        x: c_int,
        y: c_int,
        click_count: c_int,
        modifiers: c_int,
    );
    pub fn cef_view_wheel(view: *mut c_void, x: c_int, y: c_int, dx: c_int, dy: c_int);
    /// type: cef_key_event_type_t (0=RAWKEYDOWN 1=KEYDOWN 2=KEYUP 3=CHAR);
    /// mods: CEF EVENTFLAG bits; ch16: character for KEYEVENT_CHAR.
    pub fn cef_view_key(
        view: *mut c_void,
        type_: c_int,
        windows_key_code: c_int,
        native_key_code: c_int,
        mods: u32,
        ch16: u16,
    );

    pub fn cef_wayland_init(display: *mut c_void, parent_surface: *mut c_void) -> c_int;
    pub fn cef_view_attach_wayland(view: *mut c_void);
    pub fn cef_view_set_geometry(
        view: *mut c_void,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        visible: c_int,
        has_overlay: c_int,
    );
    pub fn cef_view_get_screenshot(
        view: *mut c_void,
        out_buf: *mut *mut u8,
        out_w: *mut i32,
        out_h: *mut i32,
        out_size: *mut usize,
    ) -> c_int;
}
