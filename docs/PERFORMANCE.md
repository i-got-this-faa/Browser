# Performance Audit

Methodology and results of the trace-driven performance audit (2026-09-23).
Every number below was measured with the built-in trace tooling, not guessed.

## Tooling

- `STRIP_TRACE=/path/file.jsonl ./target/debug/browser` enables a JSONL trace
  layer (`browser-ui/src/trace.rs`): one line per span (`"ph":"X"` with
  `dur_us`) and one per instant event (`"ph":"E"`). Only the `perf` target is
  recorded; other crates that also use `tracing` (zbus) are filtered out. With
  the env var unset the layer is a no-op (disabled callsites).
- `perf_event!("name", "field" => value)` and `perf_span!("name")`
  (`browser-core/src/perf.rs`) emit from any crate.
- `scripts/trace_session.sh <trace> <scenario>` runs a live session
  (idle / idle25 / navigate / burst / verify) against a real CEF engine and
  prints the aggregated report.
- `scripts/trace_report.py <trace.jsonl>` ranks spans by total time and
  prints event field sums (who got resized, how many bytes got copied...).
- `cargo bench -p browser-ui --bench layout_bench` covers the pure layout
  math (regression check).

## Findings and fixes

| # | Finding (trace evidence) | Fix | Effect (before -> after) |
|---|---|---|---|
| 1 | `render.resize_calls` fired for **every page on every render** (1594 resize calls in 26s idle; 89 during a 12s burst) — `view_sizes` cache existed but was never used; empty pages retried forever on `no view` | resize only when the rounded frame size changed (`view_sizes` + `has_view` guard) | idle 1594 -> 2, navigate 924 -> 2, burst 89 -> 4 (genuine changes only) |
| 2 | Frame pump called `cx.notify()` unconditionally at 60Hz: GPUI re-rendered the whole tree forever while idle (60.7 renders/s with a single static page) | pump is dirty-flagged: notify only on engine events, scroll steps, or toast hide | idle renders/26s 1594 -> 82 (61 fps -> 3.2 fps) |
| 3 | `Surface::texture` cloned the full ~4MB BGRA buffer per damage event (740MB copied in 26s idle) | take ownership (`mem::take`), hand back a fresh zeroed buffer; bounds hazard on decode failure closed | texture bytes idle 229MB -> 105MB/26s |
| 4 | Texture publish ran **per damage event**, so a burst of N damages cloned N full frames even though only the last is visible | publish in `render()` per visible page (once per displayed frame; hidden pages never upload) | frame.paint mean 1237us -> 295us; upload total 232MB -> 105MB idle, 197MB -> 40MB burst |
| 5 | Every `dispatch` reset `scroll_target` (`scroll_to_active`): a toast or palette open re-centered the strip | ops set `Effects::scroll_recenter` only for focus/workspace/new/close/move | prompt/palette/toast no longer move or redraw the strip |
| 6 | `dispatch` ran `effects()` twice (dup from an edit slip) — two CEF views spawned per spawn effect | dedup | one view per page |
| 7 | Per-frame `listener.try_clone()` = a `dup()` syscall at 60Hz | `Arc<UnixListener>` shared by the pump | syscall gone |
| 8 | Config watcher was a `Shell::new` local: dropped on return, so browser.lua hot reload silently died after startup | watcher handle stored on the shell | `scripts/verify_hotreload.sh` PASS (0.78 -> 0.55 applied live) |
| 9 | Control socket `serve()` did a blocking `read_line` on the UI thread: a client that connects without sending froze the whole browser | 500ms read timeout | `scripts/verify_nofreeze.sh` PASS (state answered in 8ms during a silent client) |

## Before/after summary (measured)

Idle (26s, one page, no interaction):

| metric | before | after |
|---|---|---|
| GPUI renders | 1594 | 82 |
| engine.resize calls | 1594 | 2 |
| texture bytes copied | 229MB | 105MB |
| frame.paint mean | 1237us | 295us |
| frame p95 | 132us | 127us |

Interaction (navigate + new page, 12s):

| metric | before | after |
|---|---|---|
| renders | 924 | 40 |
| resize calls | 924 | 2 |
| frame.paint mean | 1834us | 861us |

Burst (2 new pages, focus flips, overview toggles, 12s):

| metric | before | after |
|---|---|---|
| resize calls | 89 | 4 |
| frame.paint mean | ~513us | 740us (47 paints incl. overview resizes) |
| frame p95 | 255us | 253us |

Remaining costs are legitimate work: paints land at 300-800us per page-frame
and only when content actually changed. The 60Hz pump keeps draining engine
events cheaply (46-90us per tick) so input latency is unaffected.

## Rejected / not-worth-it

- Layout math is not a bottleneck: geometry+visible is 50ns (1 page) to
  1.5us (256 pages); snapshot build 36ns-7us. No change needed.
- The 16ms timer stays: it is the input/event pump. Making it event-driven
  would need an fd-backed wakeup for every engine event source; the dirty
  flag already removed the render cost.
- CDP PNG path (`STRIP_ENGINE=cdp`) is the frozen test harness (decisions.tsv
  frame/frozen): encode/decode stays, do not invest.

## Reproduce

```sh
bash scripts/trace_session.sh /tmp/t.jsonl idle25   # or navigate / burst / verify
python3 scripts/trace_report.py /tmp/t.jsonl
bash scripts/verify_hotreload.sh
bash scripts/verify_nofreeze.sh
cargo bench -p browser-ui --bench layout_bench
cargo test --workspace
```
