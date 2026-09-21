//! Core types shared across the browser crates.

use serde::{Deserialize, Serialize};

pub mod perf;

pub type PageId = u64;
pub type WorkspaceId = u64;

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
}

impl Page {
    pub fn new(id: PageId, workspace: WorkspaceId, url: impl Into<String>, x: f32, width: f32) -> Self {
        Self { id, url: url.into(), title: String::new(), workspace, x, width }
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
            workspaces: vec![Workspace { id: 1, name: "main".into() }],
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

    /// Rightmost edge across all pages in the active workspace.
    pub fn strip_right(&self) -> f32 {
        self.visible().last().map(|p| p.right()).unwrap_or(0.0)
    }

    /// Leftmost edge across all pages in the active workspace.
    pub fn strip_left(&self) -> f32 {
        self.visible().first().map(|p| p.x).unwrap_or(0.0)
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
        self.workspaces.push(Workspace { id, name: name.into() });
        id
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
        assert_eq!(s.strip_right(), 100.0);
        assert!(s.remove_workspace(ws));
        assert_eq!(s.pages.len(), 2);
    }
}
