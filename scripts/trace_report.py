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
