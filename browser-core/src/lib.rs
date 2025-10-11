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

