//! Pure strip-layout math shared by UI, scripting, and tests.
//!
//! Niri-inspired model: pages are first-class surfaces on an infinite
//! horizontal strip. Opening a page never resizes another page.

use browser_core::{Page, PageId, Strip, DEFAULT_GAP, DEFAULT_PAGE_FRACTION};

/// Viewport geometry for one frame of layout.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

