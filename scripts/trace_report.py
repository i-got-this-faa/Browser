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


