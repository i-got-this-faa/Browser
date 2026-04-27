//! Micro-benchmarks for the strip layout hot path (cargo bench -p browser-ui).
//!
//! Measures the per-frame geometry math the shell runs every render, the
//! repeated visible() sort, and snapshot building — the O(n) per-frame costs
//! that show up in the trace as render.geos / frame.render time.

use browser_core::{Page, Strip};
use browser_layout::{frame_geometries_scaled, Viewport};

fn strip_with(n: usize) -> Strip {
    let mut s = Strip::new(12.0, 0.78);
    let w = 1000.0;
    for i in 0..n {
        let id = s.alloc_id();
        let page = Page::new(id, 1, "https://example.com", i as f32 * (w + 12.0), w);
        s.pages.push(page);
    }
    if n > 0 {
        s.active_page = Some(1);
    }
    s
}

