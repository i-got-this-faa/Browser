//! The only channel Lua sees. The UI pushes a context snapshot before each
//! hook/command call; scripts return [`Request`]s. No GPUI or engine types:
//! this boundary is the contract.

use serde::{Deserialize, Serialize};

/// Snapshot of open pages, pushed to Lua as `browser.tabs` before each call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TabInfo {
    pub id: u64,
    pub url: String,
    pub title: String,
    pub workspace: u64,
    pub active: bool,
}

/// What the snapshot contains; also the payload for tab-related events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserSnapshot {
    pub tabs: Vec<TabInfo>,
    pub active_workspace: u64,
}

/// Events the UI may deliver to `events.<name>` hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "args", rename_all = "snake_case")]
