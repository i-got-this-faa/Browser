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
