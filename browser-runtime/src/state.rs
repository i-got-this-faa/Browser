//! Shell state shared by the UI and the runtime: strip + per-page webviews.
//!
//! The UI renders from this state; ops mutate it. Webviews live beside the
//! logical strip entries and are keyed by page id.

use browser_core::{Page, PageId, Strip, WorkspaceId, DEFAULT_GAP, DEFAULT_PAGE_FRACTION};
use browser_layout::{scroll_to_active, scroll_to_page, ScrollOffset, Viewport};
use std::collections::HashMap;

/// Everything known about one page: strip entry plus its webview state.
#[derive(Debug, Clone)]
pub struct PageSlot {
    pub page: Page,
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
    /// Page widths `page.width_preset` cycles through (viewport shares).
    pub width_presets: Vec<f32>,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self {
            strip: Strip::new(DEFAULT_GAP, DEFAULT_PAGE_FRACTION),
            slots: HashMap::new(),
            scroll: 0.0,
            overview_open: false,
            quit_requested: false,
            width_presets: Vec::new(),
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
    pub fn add_page(&mut self, url: &str, vp: &Viewport) -> PageId {
        let id = self.strip.alloc_id();
        let width = vp.page_width(self.strip.page_fraction);
        let page = Page::new(id, self.strip.active_workspace, url, 0.0, width);
        if self.strip.active_page.is_some() {
            self.strip.insert_beside(page.clone());
        } else {
            // First page on the strip takes x = 0.
            self.strip.pages.push(page.clone());
        }
        self.strip.active_page = Some(id);
        self.slots.insert(
            id,
            PageSlot { page, loading: true },
        );
        self.scroll = scroll_to_active(&self.strip, vp);
        self.strip.normalize_workspaces();
        id
    }

    /// Remove a page and its slot; choose a sensible successor focus.
    pub fn close_page(&mut self, id: PageId, vp: &Viewport) -> Option<PageId> {
        let removed = self.strip.remove(id)?;
        self.slots.remove(&id);
        let next = self.strip.pick_active_after_remove(&removed);
        self.strip.active_page = next;
        if let Some(n) = next {
            self.scroll = scroll_to_page(&self.strip, vp, n);
        }
        self.strip.normalize_workspaces();
        next
    }

    /// Focus a page, switching to its workspace when it lives elsewhere.
    pub fn focus_page(&mut self, id: PageId, vp: &Viewport) {
        let Some(ws) = self.strip.page(id).map(|p| p.workspace) else { return };
        self.enter_workspace(ws);
        self.strip.active_page = Some(id);
        self.scroll = scroll_to_page(&self.strip, vp, id);
        self.strip.normalize_workspaces();
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

    /// Make `ws` the active workspace, remembering where the one being left
    /// was (focus and scroll) so coming back restores it.
    fn enter_workspace(&mut self, ws: WorkspaceId) {
        let leaving = self.strip.active_workspace;
        if leaving == ws {
            return;
        }
        let (page, scroll) = (self.strip.active_page, self.scroll);
        if let Some(w) = self.strip.workspaces.iter_mut().find(|w| w.id == leaving) {
            w.active_page = page;
            w.scroll = scroll;
        }
        self.strip.active_workspace = ws;
    }

    /// Switch workspaces. Focus returns to the page that was focused there
    /// (the first page if it is gone); leaving an empty workspace removes it.
    pub fn focus_workspace(&mut self, ws: WorkspaceId, vp: &Viewport) {
        let Some(target) = self.strip.workspaces.iter().find(|w| w.id == ws) else { return };
        let remembered = target.active_page;
        self.enter_workspace(ws);
        let pages = self.strip.workspace_pages(ws);
        let focus = remembered
            .filter(|id| pages.iter().any(|p| p.id == *id))
            .or(pages.first().map(|p| p.id));
        self.strip.active_page = focus;
        self.scroll = match focus {
            Some(f) => scroll_to_page(&self.strip, vp, f),
            None => 0.0,
        };
        self.strip.normalize_workspaces();
    }

    /// Send a page to another workspace, ending at the end of its strip. With
    /// `follow` the view and focus go with it; otherwise focus stays on the
    /// workspace it left.
    pub fn send_page_to_workspace(&mut self, id: PageId, ws: WorkspaceId, follow: bool, vp: &Viewport) {
        let Some(before) = self.strip.move_page_to_workspace(id, ws) else { return };
        if follow {
            self.focus_workspace(ws, vp);
            self.focus_page(id, vp);
        } else {
            if self.strip.active_page == Some(id) {
                self.strip.active_page = self.strip.pick_active_after_remove(&before);
            }
            if let Some(a) = self.strip.active_page {
                self.scroll = scroll_to_page(&self.strip, vp, a);
            }
            self.strip.normalize_workspaces();
        }
    }

    /// Record what the engine reports about a page's audio.
    pub fn set_audio_playing(&mut self, id: PageId, playing: bool) {
        if let Some(p) = self.strip.page_mut(id) {
            p.audio.playing = playing;
        }
    }

    /// Flip the user's mute choice for a page; returns the new state.
    pub fn toggle_muted(&mut self, id: PageId) -> Option<bool> {
        let p = self.strip.page_mut(id)?;
        p.audio.muted = !p.audio.muted;
        Some(p.audio.muted)
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

    /// Two pages on workspace 1, one on workspace 2, then the empty bottom one.
    fn two_workspaces() -> (BrowserState, [PageId; 3]) {
        let mut s = BrowserState::default();
        let vp = vp();
        let a = s.add_page("a", &vp);
        let b = s.add_page("b", &vp);
        s.focus_workspace(2, &vp);
        let c = s.add_page("c", &vp);
        s.focus_workspace(1, &vp);
        (s, [a, b, c])
    }

    fn ws_ids(s: &BrowserState) -> Vec<WorkspaceId> {
        s.strip.workspaces.iter().map(|w| w.id).collect()
    }

    #[test]
    fn workspace_switch_restores_the_focus_it_left() {
        let (mut s, [a, b, c]) = two_workspaces();
        let vp = vp();
        assert_eq!(ws_ids(&s), vec![1, 2, 3], "one empty workspace at the bottom");
        assert_eq!(s.active_id(), Some(b), "workspace 1 was left on its newest page");
        s.focus_page(a, &vp);
        s.focus_workspace(2, &vp);
        assert_eq!(s.active_id(), Some(c));
        s.focus_workspace(1, &vp);
        assert_eq!(s.active_id(), Some(a), "remembered page, not the first-or-last one");
    }

    #[test]
    fn leaving_an_empty_workspace_removes_it_and_the_bottom_stays_empty() {
        let (mut s, [_, _, c]) = two_workspaces();
        let vp = vp();
        s.focus_workspace(3, &vp);
        assert_eq!(s.strip.active_workspace, 3);
        s.focus_workspace(1, &vp);
        assert_eq!(ws_ids(&s), vec![1, 2, 3], "still one empty workspace at the bottom");
        // Closing the last page of workspace 2 while on it keeps it until left.
        s.focus_workspace(2, &vp);
        s.close_page(c, &vp);
        assert_eq!(ws_ids(&s), vec![1, 2, 3]);
        s.focus_workspace(1, &vp);
        assert_eq!(ws_ids(&s), vec![1, 3], "the emptied workspace goes away");
    }

    #[test]
    fn focusing_a_page_on_another_workspace_switches_to_it() {
        let (mut s, [_, _, c]) = two_workspaces();
        s.focus_page(c, &vp());
        assert_eq!((s.strip.active_workspace, s.active_id()), (2, Some(c)));
    }

    #[test]
    fn sending_a_page_can_follow_it_or_stay() {
        let (mut s, [a, b, c]) = two_workspaces();
        let vp = vp();
        s.send_page_to_workspace(b, 2, false, &vp);
        assert_eq!((s.strip.active_workspace, s.active_id()), (1, Some(a)), "focus stays behind");
        assert_eq!(s.strip.page(b).unwrap().workspace, 2);
        s.send_page_to_workspace(a, 3, true, &vp);
        assert_eq!((s.strip.active_workspace, s.active_id()), (3, Some(a)), "focus follows the page");
        assert_eq!(ws_ids(&s), vec![2, 3, 4], "workspace 1 emptied and left, a new empty one below");
        let after_c = s.strip.page(c).unwrap().right() + s.strip.gap;
        assert_eq!(s.strip.page(b).unwrap().x, after_c, "moved page lands after the existing ones");
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
    fn mute_toggles_per_page() {
        let mut s = BrowserState::default();
        let a = s.add_page("a", &vp());
        assert_eq!(s.toggle_muted(a), Some(true));
        s.set_audio_playing(a, true);
        assert!(s.strip.page(a).unwrap().audio.playing && s.strip.page(a).unwrap().audio.muted);
        assert_eq!(s.toggle_muted(a), Some(false));
        assert_eq!(s.toggle_muted(999), None);
    }
}
