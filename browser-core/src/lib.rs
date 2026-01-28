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
