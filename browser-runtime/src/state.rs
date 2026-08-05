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
mod tests {
    use super::*;

    fn vp() -> Viewport {
        Viewport { width: 1000.0, height: 800.0 }
    }

    #[test]
    fn add_close_cycle_keeps_focus_sane() {
        let mut s = BrowserState::default();
        let vp = vp();
        let a = s.add_page("a", &vp);
        let b = s.add_page("b", &vp);
        let c = s.add_page("c", &vp);
        assert_eq!(s.active_id(), Some(c));
        assert_eq!(s.strip.page(b).unwrap().x, s.strip.page(a).unwrap().width + s.strip.gap);
        s.close_page(c, &vp);
        assert_eq!(s.active_id(), Some(b), "right neighbor becomes active");
        s.close_page(b, &vp);
        assert_eq!(s.active_id(), Some(a));
        s.close_page(a, &vp);
        assert_eq!(s.active_id(), None);
        assert!(s.slots.is_empty());
    }

    #[test]
    fn workspace_switch_remembers_first_page() {
        let mut s = BrowserState::default();
        let vp = vp();
        let a = s.add_page("a", &vp);
        s.add_page("b", &vp);
        let ws = s.strip.create_workspace("dev");
        s.strip.active_workspace = ws; // add_page targets the current workspace
        let dev1 = s.add_page("dev1", &vp);
        s.focus_workspace(1, &vp);
        assert_eq!(s.active_id(), Some(a));
        s.focus_workspace(ws, &vp);
        assert_eq!(s.active_id(), Some(dev1), "first page of the target workspace takes focus");
    }

    #[test]
    fn behavior_rescale_preserves_page_count() {
        let mut s = BrowserState::new(12.0, 0.5);
        let vp = vp();
        s.add_page("a", &vp);
        s.add_page("b", &vp);
        s.apply_behavior(20.0, 0.9);
        assert_eq!(s.strip.pages.len(), 2);
        assert!((s.strip.pages[0].width - 900.0).abs() < 1.0);
        assert_eq!(s.strip.gap, 20.0);
    }

    #[test]
    fn next_workspace_wraps() {
        let mut s = BrowserState::default();
        s.strip.create_workspace("w2");
        s.strip.create_workspace("w3");
        assert_eq!(s.next_workspace(true), Some(2));
        s.strip.active_workspace = 3;
        assert_eq!(s.next_workspace(true), Some(1), "wraps to first");
        assert_eq!(s.next_workspace(false), Some(2));
    }
}
