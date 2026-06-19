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
pub struct BrowserState {
    pub strip: Strip,
    pub slots: HashMap<PageId, PageSlot>,
    pub scroll: ScrollOffset,
    /// Prompt/palette overlay state lives in the UI; the shell only tracks
    /// what affects layout or ops.
    pub overview_open: bool,
    pub quit_requested: bool,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self {
            strip: Strip::new(DEFAULT_GAP, DEFAULT_PAGE_FRACTION),
            slots: HashMap::new(),
            scroll: 0.0,
            overview_open: false,
            quit_requested: false,
        }
    }
}

impl BrowserState {
    pub fn new(gap: f32, fraction: f32) -> Self {
        Self {
            strip: Strip::new(gap, fraction),
            ..Self::default()
        }
    }

    /// Sync gap/fraction after a config reload, resizing page widths so the
    /// strip reflects the new fraction without changing page count.
    pub fn apply_behavior(&mut self, gap: f32, fraction: f32) {
        let old_fraction = self.strip.page_fraction;
        self.strip.gap = gap;
        self.strip.page_fraction = fraction;
        if (old_fraction - fraction).abs() > f32::EPSILON && fraction > 0.0 {
            let scale = fraction / old_fraction;
            for p in self.strip.pages.iter_mut() {
                p.x *= scale;
                p.width *= scale;
            }
        }
    }

    /// Insert a brand-new page beside the active one, with a slot, and give
    /// it focus (niri: new windows take focus). Returns the new page id.
    pub fn focus_page(&mut self, id: PageId, vp: &Viewport) {
        if self.strip.page(id).is_some() {
            self.strip.active_page = Some(id);
            self.scroll = scroll_to_page(&self.strip, vp, id);
        }
    }

    pub fn set_url(&mut self, id: PageId, url: &str) {
        if let Some(p) = self.strip.page_mut(id) {
            p.url = url.to_string();
        }
    }

    pub fn set_title(&mut self, id: PageId, title: &str) {
        if let Some(p) = self.strip.page_mut(id) {
            p.title = title.to_string();
        }
    }

    pub fn active_id(&self) -> Option<PageId> {
        self.strip.active_page
    }

    pub fn slot(&self, id: PageId) -> Option<&PageSlot> {
        self.slots.get(&id)
    }

    pub fn slot_mut(&mut self, id: PageId) -> Option<&mut PageSlot> {
        self.slots.get_mut(&id)
    }

    /// Switch workspaces, focusing the first page there if one exists.
    pub fn focus_workspace(&mut self, ws: WorkspaceId, vp: &Viewport) {
        if !self.strip.workspaces.iter().any(|w| w.id == ws) {
            return;
        }
        self.strip.active_workspace = ws;
        let first = self.strip.workspace_pages(ws).first().map(|p| p.id);
        self.strip.active_page = first;
        if let Some(f) = first {
            self.scroll = scroll_to_page(&self.strip, vp, f);
        } else {
            self.scroll = scroll_to_active(&self.strip, vp);
        }
    }

    /// Next workspace id in creation order after the active one (wraps).
    pub fn next_workspace(&self, forward: bool) -> Option<WorkspaceId> {
        let mut ids: Vec<WorkspaceId> = self.strip.workspaces.iter().map(|w| w.id).collect();
        if ids.len() < 2 {
            return None;
        }
        ids.sort_unstable();
        let cur = self.strip.active_workspace;
        let idx = ids.iter().position(|i| *i == cur)?;
        let next = if forward {
            (idx + 1) % ids.len()
        } else {
            (idx + ids.len() - 1) % ids.len()
        };
        ids.get(next).copied()
    }
}

#[cfg(test)]
