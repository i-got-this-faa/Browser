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
and only when content actually changed.

## Round 2: the event-driven pump (2026-09-24)

The round-1 "Rejected" entry said an event-driven pump would need fd-backed
wakeups for every event source. That undercounted the sources: the frame
sources are two threads (CEF sink, CDP reader) plus the watcher and control
acceptor threads — none of which own an fd the UI executor can poll. A
one-slot waker channel makes all of them wake the pump with one line each.

**Implementation** (`browser-core/src/wakeslot.rs`):

- `frame_wake()` — an `async_channel::bounded(1)` receiver the pump awaits.
  Capacity 1 coalesces storms: an animated page painting 400x/s yields one
  pump iteration per rendered frame.
- `kick()` — non-blocking, callable from any thread, before or after the
  pump first awaits. Producers: CEF frame sink, CDP screencast reader,
  config-watcher callback (browser-config), control-socket acceptor, and
  toast start (its countdown advances on pump ticks).
- The pump races `timer(16ms)` against `frame_wake().recv()` — but the timer
  is **60Hz only while `animation_deadline()` says animation is in flight**
  (smooth scroll, toast countdown); otherwise it is a 1-hour watchdog that
  exists so a lost kick can never wedge the pump.
- Toasts became wall-clock (`toast_deadline`) instead of frame-counted —
  frame counting would freeze the countdown on an idle shell.
- Control socket: the acceptor thread now reads the request line (500ms
  timeout) and hands `(stream, line)` to the UI thread over a channel, then
  kicks. The UI thread does zero network reads and zero accepts.

**Measured** (same methodology, real CEF engine):

| metric | 16ms timer pump | event-driven pump |
|---|---|---|
| idle CPU (main proc, 12s window, /proc schedstat) | wake loop ~60Hz + renders | **0.405% of one core** |
| timer-driven frames while idle | 60/s (pre-round-1), 3.2/s (round 1) | **0** — all remaining frames are CEF OnPaint for the visible page (~1/s damage + ~1/s empty-damage), the content engine's own floor |
| control-socket reply latency during silent client | 213ms (waited on a 500ms read) | **3ms** |
| hot reload / nofreeze / agent API suites | pass | pass |

The remaining 2 fps worth of `frame` spans in a steady-state trace are CEF
OnPaint events with (often empty) damage — the engine's repaint cadence for
a visible page, not shell work. Suppressing those would mean suppressing
page redraws. Idle texture upload still happens for those paints
(~1.8MB/10s for one 958x1050 page); a version-diff on the composited buffer
could skip the empty-damage uploads if that ever matters.

## Rejected / not-worth-it

- Layout math is not a bottleneck: geometry+visible is 50ns (1 page) to
  1.5us (256 pages); snapshot build 36ns-7us. No change needed.
- The 16ms timer is gone (round 2 replaced it); the round-1 note is kept
  for the record: an event-driven pump "would need an fd-backed wakeup for
  every engine event source" — it needed a waker in each *source thread*,
  not an fd per source, which is exactly what `wakeslot::kick` provides.
- CDP PNG path (`STRIP_ENGINE=cdp`) is the frozen test harness (decisions.tsv
  frame/frozen): encode/decode stays, do not invest.

## Reproduce

```sh
bash scripts/trace_session.sh /tmp/t.jsonl idle25   # or navigate / burst / verify
python3 scripts/trace_report.py /tmp/t.jsonl
bash scripts/verify_hotreload.sh
bash scripts/verify_nofreeze.sh
bash scripts/verify_agent_api.sh
cargo bench -p browser-ui --bench layout_bench
cargo test --workspace
```

See also `docs/AGENT_API.md`: the agent control surface that rides on the
same event-driven socket (every command kicks the pump, so replies are
computed immediately on an idle shell).
