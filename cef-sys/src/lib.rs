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
