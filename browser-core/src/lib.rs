//! Core types shared across the browser crates.

use serde::{Deserialize, Serialize};

pub mod perf;
pub mod wakeslot;

pub type PageId = u64;
pub type WorkspaceId = u64;

/// Audio state of one page: what the engine reports (`playing`) and what the
/// user chose (`muted`). A muted page keeps `playing` true while it has a
/// stream, so the indicator can tell "silenced" from "silent".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioState {
    pub playing: bool,
    pub muted: bool,
}

/// A single web surface in the infinite horizontal strip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: PageId,
    pub url: String,
    pub title: String,
    pub workspace: WorkspaceId,
    /// X position in the strip, in virtual pixels.
    pub x: f32,
    /// Natural width in virtual pixels.
    pub width: f32,
    pub audio: AudioState,
    /// Width to return to when un-maximizing; `Some` means maximized.
    pub restore_width: Option<f32>,
}

impl Page {
    pub fn new(id: PageId, workspace: WorkspaceId, url: impl Into<String>, x: f32, width: f32) -> Self {
        Self {
            id,
            url: url.into(),
            title: String::new(),
            workspace,
            x,
            width,
            audio: AudioState::default(),
            restore_width: None,
        }
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }
}

/// One workspace in the vertical stack of dynamic workspaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    /// Focus to restore when the workspace is entered again.
    pub active_page: Option<PageId>,
    /// Horizontal scroll to draw the strip at while the workspace is not the
    /// active one (the active workspace scrolls with `BrowserState::scroll`).
    pub scroll: f32,
}

impl Workspace {
    fn new(id: WorkspaceId, name: impl Into<String>) -> Self {
        Self { id, name: name.into(), active_page: None, scroll: 0.0 }
    }
}

/// The single source of truth for strip geometry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Strip {
    pub pages: Vec<Page>,
    pub workspaces: Vec<Workspace>,
    pub active_workspace: WorkspaceId,
    pub active_page: Option<PageId>,
    pub next_id: PageId,
    /// Gap between pages, virtual px.
    pub gap: f32,
    /// Fraction of the viewport a page occupies.
    pub page_fraction: f32,
}

pub const DEFAULT_PAGE_FRACTION: f32 = 0.78;
pub const DEFAULT_GAP: f32 = 12.0;

impl Strip {
    pub fn new(gap: f32, page_fraction: f32) -> Self {
        Self {
            pages: Vec::new(),
            workspaces: vec![Workspace::new(1, "main")],
            active_workspace: 1,
            active_page: None,
            next_id: 1,
            gap,
            page_fraction,
        }
    }

    pub fn page(&self, id: PageId) -> Option<&Page> {
        self.pages.iter().find(|p| p.id == id)
    }

    pub fn page_mut(&mut self, id: PageId) -> Option<&mut Page> {
        self.pages.iter_mut().find(|p| p.id == id)
    }

    pub fn active(&self) -> Option<&Page> {
        self.active_page.and_then(|id| self.page(id))
    }

    /// Pages in the active workspace, left to right.
    pub fn visible(&self) -> Vec<&Page> {
        let mut pages: Vec<&Page> = self
            .pages
            .iter()
            .filter(|p| p.workspace == self.active_workspace)
            .collect();
        pages.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        pages
    }

    /// Allocate a fresh page id.
    pub fn alloc_id(&mut self) -> PageId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Insert a page to the right of the active page, bumping others.
    /// New pages never resize existing pages.
    pub fn insert_beside(&mut self, page: Page) {
        let Some(active) = self.active() else {
            let id = page.id;
            self.pages.push(page);
            if self.active_page.is_none() {
                self.active_page = Some(id);
            }
            return;
        };
        let insert_x = active.right() + self.gap;
        for p in self.pages.iter_mut() {
            if p.workspace == page.workspace && p.x >= insert_x {
                p.x += page.width + self.gap;
            }
        }
        let mut page = page;
        page.x = insert_x;
        self.pages.push(page);
    }

    /// Remove a page and close the gap left of its slot.
    pub fn remove(&mut self, id: PageId) -> Option<Page> {
        let idx = self.pages.iter().position(|p| p.id == id)?;
        let page = self.pages.remove(idx);
        for p in self.pages.iter_mut() {
            if p.workspace == page.workspace && p.x > page.x {
                p.x -= page.width + self.gap;
            }
        }
        Some(page)
    }

    /// Move a page to `to_x`, shifting pages it crosses (no swap-swap bugs: shift the whole run).
    pub fn move_page(&mut self, id: PageId, to_x: f32) {
        let Some(page) = self.page(id) else { return };
        let (old_x, width) = (page.x, page.width);
        let to_x = to_x.max(0.0);
        if (to_x - old_x).abs() < f32::EPSILON {
            return;
        }
        if to_x > old_x {
            // Pages fully to the left of the new right edge slide left by one slot.
            let new_right = to_x + width;
            for p in self.pages.iter_mut() {
                if p.id != id && p.workspace == self.active_workspace && p.x > old_x && p.x + p.width <= new_right + self.gap {
                    p.x -= width + self.gap;
                }
            }
        } else {
            // Pages fully to the right of the new left edge slide right by one slot.
            for p in self.pages.iter_mut() {
                if p.id != id && p.workspace == self.active_workspace && p.x + p.width < old_x && p.x >= to_x - self.gap {
                    p.x += width + self.gap;
                }
            }
        }
        if let Some(p) = self.page_mut(id) {
            p.x = to_x;
        }
    }

    /// Choose the next active page after removing `removed`. Call after
    /// `remove`: the successor has slid into the removed page's slot, so the
    /// right neighbor is the closest page at or right of `removed.x`.
    pub fn pick_active_after_remove(&self, removed: &Page) -> Option<PageId> {
        let right = self
            .pages
            .iter()
            .filter(|p| p.workspace == removed.workspace && p.x >= removed.x)
            .min_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        let left = self
            .pages
            .iter()
            .filter(|p| p.workspace == removed.workspace && p.x < removed.x)
            .max_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        right.or(left).map(|p| p.id)
    }

    pub fn create_workspace(&mut self, name: impl Into<String>) -> WorkspaceId {
        let id = self.workspaces.iter().map(|w| w.id).max().unwrap_or(0) + 1;
        self.workspaces.push(Workspace::new(id, name));
        id
    }

    /// Position of a workspace in the vertical stack (0 = top).
    pub fn workspace_index(&self, ws: WorkspaceId) -> Option<usize> {
        self.workspaces.iter().position(|w| w.id == ws)
    }

    /// Workspace at a stack position, clamped to the last one.
    pub fn workspace_at(&self, index: usize) -> WorkspaceId {
        let last = self.workspaces.len().saturating_sub(1);
        self.workspaces[index.min(last)].id
    }

    /// The workspace above or below `ws`, if any (the stack does not wrap).
    pub fn workspace_neighbor(&self, ws: WorkspaceId, down: bool) -> Option<WorkspaceId> {
        let i = self.workspace_index(ws)?;
        let j = if down { i + 1 } else { i.checked_sub(1)? };
        self.workspaces.get(j).map(|w| w.id)
    }

    /// Niri-style dynamic workspaces: an empty workspace is dropped as soon
    /// as it is not the active one, and the stack always ends in exactly one
    /// empty workspace. Call after anything that adds, removes or moves pages
    /// or changes the active workspace.
    pub fn normalize_workspaces(&mut self) {
        let active = self.active_workspace;
        let occupied: std::collections::HashSet<WorkspaceId> =
            self.pages.iter().map(|p| p.workspace).collect();
        let last = self.workspaces.last().map(|w| w.id);
        self.workspaces
            .retain(|w| occupied.contains(&w.id) || w.id == active || Some(w.id) == last);
        if self.workspaces.last().is_some_and(|w| occupied.contains(&w.id)) {
            let n = self.workspaces.len() + 1;
            self.create_workspace(format!("workspace {n}"));
        }
    }

    /// Move a page to the end of another workspace's strip, closing the gap it
    /// leaves behind. Returns the page as it was before the move (None when
    /// nothing moved), which is what `pick_active_after_remove` needs.
    pub fn move_page_to_workspace(&mut self, id: PageId, ws: WorkspaceId) -> Option<Page> {
        let from = self.page(id)?.workspace;
        if from == ws || self.workspace_index(ws).is_none() {
            return None;
        }
        let before = self.remove(id)?;
        let mut page = before.clone();
        page.x = self.workspace_pages(ws).last().map_or(0.0, |p| p.right() + self.gap);
        page.workspace = ws;
        self.pages.push(page);
        Some(before)
    }

    /// Set one page's width; pages to its right slide by the difference.
    /// No other page changes size (opening or resizing never resizes others).
    fn set_width(&mut self, id: PageId, width: f32) {
        let Some(page) = self.page(id) else { return };
        let (ws, x, delta) = (page.workspace, page.x, width - page.width);
        for p in self.pages.iter_mut() {
            if p.workspace == ws && p.x > x {
                p.x += delta;
            }
        }
        if let Some(p) = self.page_mut(id) {
            p.width = width;
        }
    }

    /// User resize: also ends maximized state.
    pub fn resize_page(&mut self, id: PageId, width: f32) {
        self.set_width(id, width);
        if let Some(p) = self.page_mut(id) {
            p.restore_width = None;
        }
    }

    /// Keep maximized pages as wide as the viewport after it changed size.
    pub fn refit_maximized(&mut self, full_width: f32) {
        let stale: Vec<PageId> = self
            .pages
            .iter()
            .filter(|p| p.restore_width.is_some() && p.width != full_width)
            .map(|p| p.id)
            .collect();
        for id in stale {
            self.set_width(id, full_width);
        }
    }

    /// Maximize the page to `full_width`, or restore the width it had.
    pub fn toggle_maximize(&mut self, id: PageId, full_width: f32) {
        let Some(page) = self.page(id) else { return };
        match page.restore_width {
            Some(width) => {
                self.set_width(id, width);
                if let Some(p) = self.page_mut(id) {
                    p.restore_width = None;
                }
            }
            None => {
                let width = page.width;
                self.set_width(id, full_width);
                if let Some(p) = self.page_mut(id) {
                    p.restore_width = Some(width);
                }
            }
        }
    }

    pub fn remove_workspace(&mut self, id: WorkspaceId) -> bool {
        if self.workspaces.len() <= 1 || !self.workspaces.iter().any(|w| w.id == id) {
            return false;
        }
        self.pages.retain(|p| p.workspace != id);
        self.workspaces.retain(|w| w.id != id);
        if self.active_workspace == id {
            self.active_workspace = self.workspaces[0].id;
            self.active_page = self
                .pages
                .iter()
                .find(|p| p.workspace == self.active_workspace)
                .map(|p| p.id);
        }
        true
    }

    pub fn workspace_pages(&self, ws: WorkspaceId) -> Vec<&Page> {
        let mut pages: Vec<&Page> = self.pages.iter().filter(|p| p.workspace == ws).collect();
        pages.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        pages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_with(n: usize, width: f32, gap: f32) -> Strip {
        let mut s = Strip::new(gap, DEFAULT_PAGE_FRACTION);
        for i in 0..n {
            let id = s.alloc_id();
            let x = i as f32 * (width + gap);
            s.pages.push(Page::new(id, 1, "", x, width));
        }
        if n > 0 {
            s.active_page = Some(s.pages[0].id);
        }
        s
    }

    #[test]
    fn insert_beside_does_not_resize_existing_pages() {
        let mut s = strip_with(3, 100.0, 10.0);
        let before: Vec<(u64, f32, f32)> = s.pages.iter().map(|p| (p.id, p.x, p.width)).collect();
        let id = s.alloc_id();
        s.insert_beside(Page::new(id, 1, "", 0.0, 100.0));
        let insert_x = 100.0 + 10.0;
        for (pid, x, w) in before {
            let p = s.page(pid).unwrap();
            assert_eq!(p.width, w, "width must never change");
            if x < insert_x {
                assert_eq!(p.x, x, "pages left of insert point must not move");
            } else {
                assert_eq!(p.x, x + 110.0, "pages at/right of insert shift by one slot");
            }
        }
        assert_eq!(s.page(id).unwrap().x, insert_x);
    }

    #[test]
    fn remove_closes_gap() {
        let mut s = strip_with(3, 100.0, 10.0);
        let mid = s.pages[1].id;
        s.remove(mid);
        assert_eq!(s.pages[1].x, 110.0);
        assert_eq!(s.pages.len(), 2);
    }

    #[test]
    fn remove_picks_neighbor_active() {
        let mut s = strip_with(3, 100.0, 10.0);
        s.active_page = Some(s.pages[1].id);
        let mid = s.pages[1].id;
        let removed = s.remove(mid).unwrap();
        assert_eq!(s.pick_active_after_remove(&removed), Some(s.pages[1].id));
    }

    #[test]
    fn move_page_right_shifts_run_left() {
        let mut s = strip_with(4, 100.0, 10.0);
        let ids: Vec<u64> = s.pages.iter().map(|p| p.id).collect();
        s.move_page(ids[0], 220.0); // move first to position of third
        assert_eq!(s.page(ids[0]).unwrap().x, 220.0);
        assert_eq!(s.page(ids[1]).unwrap().x, 0.0);
        assert_eq!(s.page(ids[2]).unwrap().x, 110.0);
        assert_eq!(s.page(ids[3]).unwrap().x, 330.0);
    }

    #[test]
    fn move_page_left_shifts_run_right() {
        let mut s = strip_with(4, 100.0, 10.0);
        let ids: Vec<u64> = s.pages.iter().map(|p| p.id).collect();
        s.move_page(ids[3], 110.0);
        assert_eq!(s.page(ids[3]).unwrap().x, 110.0);
        assert_eq!(s.page(ids[1]).unwrap().x, 220.0);
        assert_eq!(s.page(ids[2]).unwrap().x, 330.0);
        assert_eq!(s.page(ids[0]).unwrap().x, 0.0);
    }

    #[test]
    fn workspaces_are_independent() {
        let mut s = strip_with(2, 100.0, 10.0);
        let ws = s.create_workspace("dev");
        let page = Page::new(s.alloc_id(), ws, "", 0.0, 100.0);
        s.pages.push(page);
        s.active_workspace = ws;
        assert_eq!(s.visible().len(), 1);
        assert!(s.remove_workspace(ws));
        assert_eq!(s.pages.len(), 2);
    }

    fn ws_ids(s: &Strip) -> Vec<WorkspaceId> {
        s.workspaces.iter().map(|w| w.id).collect()
    }

    #[test]
    fn normalize_keeps_one_empty_workspace_at_the_bottom() {
        let mut s = strip_with(1, 100.0, 10.0);
        s.normalize_workspaces();
        assert_eq!(ws_ids(&s), vec![1, 2], "occupied workspace gets an empty one below");
        s.normalize_workspaces();
        assert_eq!(ws_ids(&s), vec![1, 2], "idempotent");
        // Fill the bottom one: another empty workspace appears.
        let id = s.alloc_id();
        s.pages.push(Page::new(id, 2, "", 0.0, 100.0));
        s.normalize_workspaces();
        assert_eq!(ws_ids(&s), vec![1, 2, 3]);
    }

    #[test]
    fn normalize_drops_empty_workspaces_once_left() {
        let mut s = strip_with(1, 100.0, 10.0);
        s.normalize_workspaces();
        s.create_workspace("extra"); // 1, 2 (empty), 3 (empty)
        s.normalize_workspaces();
        assert_eq!(ws_ids(&s), vec![1, 3], "empty middle workspace is removed, last stays");
        // The active workspace survives while empty, even when not last.
        s.active_workspace = 3;
        let id = s.alloc_id();
        s.pages.push(Page::new(id, 3, "", 0.0, 100.0));
        s.normalize_workspaces();
        assert_eq!(ws_ids(&s), vec![1, 3, 4]);
        s.pages.retain(|p| p.id != id);
        s.normalize_workspaces();
        assert_eq!(ws_ids(&s), vec![1, 3, 4], "active empty workspace stays until left");
        s.active_workspace = 1;
        s.normalize_workspaces();
        assert_eq!(ws_ids(&s), vec![1, 4], "left empty workspace is dropped");
    }

    #[test]
    fn workspace_neighbors_do_not_wrap() {
        let mut s = strip_with(1, 100.0, 10.0);
        s.normalize_workspaces();
        assert_eq!(s.workspace_neighbor(1, false), None);
        assert_eq!(s.workspace_neighbor(1, true), Some(2));
        assert_eq!(s.workspace_neighbor(2, true), None);
        assert_eq!(s.workspace_at(0), 1);
        assert_eq!(s.workspace_at(9), 2, "index clamps to the last workspace");
    }

    #[test]
    fn move_to_workspace_appends_and_closes_the_gap() {
        let mut s = strip_with(3, 100.0, 10.0);
        s.normalize_workspaces();
        let ids: Vec<u64> = s.pages.iter().map(|p| p.id).collect();
        assert_eq!(s.move_page_to_workspace(ids[0], 2).map(|p| p.workspace), Some(1));
        let moved = s.page(ids[0]).unwrap();
        assert_eq!((moved.workspace, moved.x, moved.width), (2, 0.0, 100.0));
        assert_eq!(s.page(ids[1]).unwrap().x, 0.0, "source strip closes the gap");
        assert_eq!(s.page(ids[2]).unwrap().x, 110.0);
        // A second page lands after the first, never on top of it.
        assert!(s.move_page_to_workspace(ids[1], 2).is_some());
        assert_eq!(s.page(ids[1]).unwrap().x, 110.0);
        assert!(s.move_page_to_workspace(ids[1], 2).is_none(), "already there");
        assert!(s.move_page_to_workspace(ids[1], 99).is_none(), "unknown workspace");
    }

    #[test]
    fn resize_only_changes_the_target_and_reflows_the_right_side() {
        let mut s = strip_with(4, 100.0, 10.0);
        let ids: Vec<u64> = s.pages.iter().map(|p| p.id).collect();
        s.resize_page(ids[1], 160.0);
        let geo: Vec<(f32, f32)> = ids.iter().map(|i| (s.page(*i).unwrap().x, s.page(*i).unwrap().width)).collect();
        assert_eq!(geo, vec![(0.0, 100.0), (110.0, 160.0), (280.0, 100.0), (390.0, 100.0)]);
        s.resize_page(ids[1], 40.0);
        assert_eq!(s.page(ids[2]).unwrap().x, 160.0, "shrinking pulls the right side in");
        assert_eq!(s.page(ids[0]).unwrap().x, 0.0, "left side never moves");
    }

    #[test]
    fn maximize_toggles_back_to_the_previous_width() {
        let mut s = strip_with(3, 100.0, 10.0);
        let ids: Vec<u64> = s.pages.iter().map(|p| p.id).collect();
        s.toggle_maximize(ids[1], 500.0);
        assert_eq!(s.page(ids[1]).unwrap().width, 500.0);
        assert_eq!(s.page(ids[2]).unwrap().x, 110.0 + 500.0 + 10.0);
        assert_eq!(s.page(ids[0]).unwrap().width, 100.0, "neighbors keep their size");
        s.toggle_maximize(ids[1], 500.0);
        assert_eq!(s.page(ids[1]).unwrap().width, 100.0);
        assert_eq!(s.page(ids[2]).unwrap().x, 220.0, "layout is exactly as before");
        // A resized window keeps maximized pages full width.
        s.toggle_maximize(ids[1], 500.0);
        s.refit_maximized(700.0);
        assert_eq!(s.page(ids[1]).unwrap().width, 700.0);
        assert_eq!(s.page(ids[1]).unwrap().restore_width, Some(100.0));
        s.toggle_maximize(ids[1], 700.0);
        // A manual resize while maximized ends the maximized state.
        s.toggle_maximize(ids[1], 500.0);
        s.resize_page(ids[1], 200.0);
        assert!(s.page(ids[1]).unwrap().restore_width.is_none());
    }
}
