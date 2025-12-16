//! Shell state shared by the UI and the runtime: strip + per-page webviews.
//!
//! The UI renders from this state; ops mutate it. Webviews live beside the
//! logical strip entries and are keyed by page id.

use browser_core::{Page, PageId, Strip, WorkspaceId, DEFAULT_GAP, DEFAULT_PAGE_FRACTION};
use browser_layout::{scroll_to_active, scroll_to_page, ScrollOffset, Viewport};
use std::collections::HashMap;

/// Everything known about one page: strip entry plus its webview id and the
/// latest rendered frame (PNG bytes straight from the engine).
#[derive(Debug, Clone)]
pub struct PageSlot {
    pub page: Page,
    /// None until the engine has spawned a target for this page.
    pub webview_id: Option<u64>,
    /// Last completed frame, PNG-encoded.
    pub frame_png: Option<Vec<u8>>,
    /// Engine has acknowledged the navigate for the initial URL.
    pub loading: bool,
}

/// Shell state: strip geometry + per-page payloads + scroll + UI flags.
#[derive(Debug, Clone)]
