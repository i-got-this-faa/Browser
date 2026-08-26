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

fn bench(name: &str, pages: usize, iters: u32) {
    let strip = strip_with(pages);
    let vp = Viewport { width: 1280.0, height: 720.0 };
    let start = std::time::Instant::now();
    let mut sink = 0.0f32;
    for _ in 0..iters {
        let geos = frame_geometries_scaled(&strip, &vp, 500.0, 0.78, 1.0);
        // Consume a value so the work is not optimized away.
        if let Some((_, g)) = geos.last() {
            sink += g.rel_x + g.width;
        }
        // visible() is called by strip_right/strip_left/render per frame too.
        let vis = strip.visible();
        sink += vis.len() as f32;
    }
    let total = start.elapsed();
    println!(
        "{name:<28} {pages:>5} pages  {iters:>7} iters  {:>10.1} ns/iter  (sink {sink:.0})",
        total.as_nanos() as f64 / iters as f64
    );
}

fn bench_snapshot(pages: usize, iters: u32) {
    let strip = strip_with(pages);
    let start = std::time::Instant::now();
    let mut sink = 0usize;
    for _ in 0..iters {
        // What Shell::snapshot builds for every Lua hook/command call.
        let tabs: Vec<(u64, String, String)> = strip
            .visible()
            .iter()
            .map(|p| (p.id, p.url.clone(), p.title.clone()))
            .collect();
        sink += tabs.len();
    }
    let total = start.elapsed();
    println!(
        "snapshot_build {pages:>20} pages  {iters:>7} iters  {:>10.1} ns/iter  (sink {sink})",
        total.as_nanos() as f64 / iters as f64
    );
}

