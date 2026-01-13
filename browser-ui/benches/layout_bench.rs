//! Micro-benchmarks for the strip layout hot path (cargo bench -p browser-ui).
//!
//! Measures the per-frame geometry math the shell runs every render, the
//! repeated visible() sort, and snapshot building — the O(n) per-frame costs
//! that show up in the trace as render.geos / frame.render time.

use browser_core::{Page, Strip};
use browser_layout::{frame_geometries_scaled, Viewport};

