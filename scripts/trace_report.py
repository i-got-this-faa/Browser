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


