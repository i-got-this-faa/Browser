//! The only channel Lua sees. The UI pushes a context snapshot before each
//! hook/command call; scripts return [`Request`]s. No GPUI or engine types:
//! this boundary is the contract.

use serde::{Deserialize, Serialize};

/// Snapshot of open pages, pushed to Lua as `browser.tabs` before each call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
