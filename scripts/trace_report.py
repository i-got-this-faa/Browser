#!/usr/bin/env python3
"""Aggregate a STRIP_TRACE JSONL file into a performance audit report.

Usage: python3 scripts/trace_report.py /path/to/trace.jsonl [top_n]

Prints, per event/span name: count, total ms, mean/max, p95, and the top
fields observed (e.g. which pages got resized most, how many events per
drain). This is the ranking that names the actual hot spots.
"""
import json
import sys
from collections import defaultdict


def load(path):
    rows = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    rows.sort(key=lambda r: r.get("t", 0))
    return rows


def pct(sorted_vals, p):
    if not sorted_vals:
        return 0
    idx = min(len(sorted_vals) - 1, int(len(sorted_vals) * p))
    return sorted_vals[idx]


def main():
    path = sys.argv[1]
    top_n = int(sys.argv[2]) if len(sys.argv) > 2 else 14
    rows = load(path)
    if not rows:
        print("no rows in", path)
        return

    span_dur = defaultdict(list)          # name -> [dur_us]
    field_total = defaultdict(lambda: defaultdict(int))   # name -> field -> sum
    field_max = defaultdict(lambda: defaultdict(int))
    field_top = defaultdict(lambda: defaultdict(int))     # name -> "field=value" -> count
    first_t = rows[0]["t"]
    last_t = rows[-1]["t"]
    wall_s = max(1e-6, (last_t - first_t) / 1e6)

    for r in rows:
        name = r.get("name", "?")
        if r.get("ph") == "X":
            span_dur[name].append(r.get("dur_us", 0))
        else:
            for k, v in (r.get("fields") or {}).items():
                try:
                    n = int(v)
                except (TypeError, ValueError):
                    n = None
                if n is not None:
                    field_total[name][k] += n
                    field_max[name][k] = max(field_max[name][k], n)
                    if k in ("page", "pages", "events", "views"):
                        field_top[name][f"{k}={v}"] += 1

    # Rank by total time (spans) then by count (events) weighted per second.
    ranked = []
    for name, durs in span_dur.items():
        durs.sort()
        total_us = sum(durs)
        ranked.append((total_us / 1000.0, name, len(durs), {
            "mean_us": total_us / len(durs),
            "p95_us": pct(durs, 0.95),
            "max_us": durs[-1],
            "per_s": len(durs) / wall_s,
        }))
    ranked.sort(reverse=True)

    print(f"trace: {path}")
    print(f"wall: {wall_s:.1f}s  rows: {len(rows)}  "
          f"spans: {sum(len(d) for d in span_dur.values())}  "
          f"events: {len(rows) - sum(len(d) for d in span_dur.values())}")
    print()
    print(f"{'name':<26}{'count':>8}{'per_s':>9}{'total_ms':>10}"
          f"{'mean_us':>10}{'p95_us':>9}{'max_ms':>9}")
    for total_ms, name, count, st in ranked[:top_n]:
        print(f"{name:<26}{count:>8}{st['per_s']:>9.1f}{total_ms:>10.1f}"
              f"{st['mean_us']:>10.1f}{st['p95_us']:>9.0f}{st['max_us'] / 1000.0:>9.2f}")

    print("\n-- event field breakdown --")
    ev_names = sorted({r["name"] for r in rows if r.get("ph") != "X"})
    for name in ev_names:
        tot = field_total.get(name) or {}
        if not tot:
            continue
        parts = ", ".join(
            f"{k}: sum={v}" + (f" max={field_max[name][k]}" if k in field_max else "")
            for k, v in sorted(tot.items())
        )
        count_line = ""
        tops = field_top.get(name) or {}
        if tops:
            best = sorted(tops.items(), key=lambda kv: -kv[1])[:4]
            count_line = "  |  " + ", ".join(f"{a} x{b}" for a, b in best)
        print(f"{name:<26} {parts}{count_line}")


if __name__ == "__main__":
    main()
