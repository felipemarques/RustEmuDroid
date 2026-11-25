"""Align task counters to Android presentation-clock windows, using only complete sample intervals."""

import argparse
import json
import re
from pathlib import Path

STAT = re.compile(r"^(\d+) \((.*)\) (.*)$")
MEMORY_FIELDS = ("VmRSS", "RssAnon", "RssFile", "RssShmem", "VmSwap")


def load(path):
    snapshots = []
    current = None
    for line in path.read_text(encoding="utf8").splitlines():
        if line.startswith("B "):
            _, i, start = line.split()
            if current is not None or int(i) != len(snapshots):
                raise ValueError("Non-consecutive sample boundary")
            current = {"start": float(start), "tasks": {}}
        elif line.startswith("P "):
            _, path, raw = line.split(" ", 2)
            match = STAT.fullmatch(raw)
            if not match or current is None:
                raise ValueError("Invalid stat row")
            tid, name, rest = match.groups()
            fields = rest.split()  # first is Linux stat field 3 (state)
            role = "process" if "/task/" not in path else f"{name} ({tid})"
            current["tasks"][role] = tuple(
                int(fields[i - 3]) for i in (22, 10, 12, 14, 15)
            )
        elif line.startswith("M "):
            _, field, value, unit = line.split()
            if current is None or field not in MEMORY_FIELDS or unit != "kB" or not value.isdecimal():
                raise ValueError("Invalid process memory row")
            memory = current.setdefault("memory_kib", {})
            if field in memory:
                raise ValueError("Duplicate process memory row")
            memory[field] = int(value)
        elif line.startswith("E "):
            _, i, end = line.split()
            if int(i) != len(snapshots) or current is None:
                raise ValueError("Missing sample")
            current["end"] = float(end)
            if current["end"] < current["start"] or "process" not in current["tasks"]:
                raise ValueError("Unreadable process or bad bounds")
            if "memory_kib" in current and set(current["memory_kib"]) != set(MEMORY_FIELDS):
                raise ValueError("Incomplete process memory counters")
            if snapshots and ("memory_kib" in current) != ("memory_kib" in snapshots[0]):
                raise ValueError("Process memory coverage changed during sampling")
            snapshots.append(current)
            current = None
    if current is not None or len(snapshots) < 2:
        raise ValueError("Incomplete run")
    return snapshots


def summarize(pairs, samples):
    result = []
    for label, start, end in pairs:
        inside = [s for s in samples if s["start"] >= start and s["end"] <= end]
        if len(inside) < 2:
            result.append(
                dict(window=label, windows_s=[start, end], samples=len(inside))
            )
            continue
        a, b = inside[0], inside[-1]
        stats = {}
        for role in sorted(a["tasks"].keys() & b["tasks"].keys()):
            first = a["tasks"][role]
            last = b["tasks"][role]
            if first[0] != last[0]:
                continue  # task replaced
            delta = tuple(v - u for u, v in zip(first[1:], last[1:]))
            if min(delta) < 0:
                raise ValueError("Counter reset")
            stats[role] = dict(
                zip(
                    ("minor_faults", "major_faults", "user_ticks", "kernel_ticks"),
                    delta,
                )
            )
        row = dict(
            window=label,
            window_s=[start, end],
            covered_s=[a["start"], b["end"]],
            samples=len(inside),
            stats=stats,
        )
        if "memory_kib" in a and "process" in stats:
            row["process_memory_kib"] = {
                field: {
                    "first": a["memory_kib"][field],
                    "last": b["memory_kib"][field],
                    "peak": max(s["memory_kib"][field] for s in inside),
                }
                for field in MEMORY_FIELDS
            }
        result.append(row)
    return result


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("snapshot", type=Path)
    p.add_argument(
        "windows",
        type=Path,
        help="JSON list of [name, start_monotonic_seconds, end_monotonic_seconds]",
    )
    args = p.parse_args()
    print(
        json.dumps(
            summarize(json.loads(args.windows.read_text()), load(args.snapshot)),
            indent=2,
        )
    )
