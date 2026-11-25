#!/usr/bin/env python3
"""Read one STK QA log and summarize complete Vulkan timestamp batches."""

import argparse
import json
import math
from pathlib import Path
import re
from statistics import median


PREFIX = re.compile(r"STK_VK_GPU_V1:? ")


def fields(line):
    return dict(item.split("=", 1) for item in line.split())


def summarize(lines):
    queue = None
    active = False
    stopped = False
    ticks = []
    for line in lines:
        match = PREFIX.search(line)
        if match is None:
            continue
        kind, _, payload = line[match.end():].strip().partition(" ")
        if stopped:
            raise ValueError("Expected one driver lifetime beginning with queue metadata")
        if kind == "disabled" and payload == "allocation" and queue is None:
            stopped = {"status": "disabled", "reason": payload}
        elif kind == "queue" and queue is None:
            values = fields(payload)
            compute_graphics = int(values["compute_and_graphics"])
            if compute_graphics not in (0, 1):
                raise ValueError("Invalid timestampComputeAndGraphics")
            queue = {
                "graphics_queue_family": int(values["family"]),
                "timestamp_valid_bits": int(values["valid_bits"]),
                "timestamp_period_ns": float(values["period_ns"]),
                "timestamp_compute_and_graphics": bool(compute_graphics),
            }
        elif kind == "disabled" and queue is not None and not active:
            stopped = {"status": "disabled", "reason": payload}
        elif kind == "active" and queue is not None and not active and not payload:
            bits = queue["timestamp_valid_bits"]
            period = queue["timestamp_period_ns"]
            if not 1 <= bits <= 64 or not math.isfinite(period) or period <= 0:
                raise ValueError("Invalid properties for active Vulkan timestamps")
            active = True
        elif kind == "ticks" and active:
            batch = [int(value) for value in payload.split()]
            if not 1 <= len(batch) <= 64 or any(value < 0 or value >= (1 << bits) for value in batch):
                raise ValueError("Invalid timestamp batch")
            ticks.extend(batch)
        elif kind == "end" and active:
            counts = {key: int(value) for key, value in fields(payload).items()}
            if counts.keys() != {"submitted", "samples", "unavailable", "discarded"}:
                raise ValueError("Invalid counters in final record")
            if (min(counts.values()) < 0 or counts["samples"] != len(ticks) or
                    counts["submitted"] != len(ticks) + counts["unavailable"] + counts["discarded"]):
                raise ValueError("Incomplete batches or inconsistent driver counters")
            stopped = {"status": "complete", **counts}
        else:
            raise ValueError(f"Unexpected timestamp record {kind!r}")

    if not stopped:
        raise ValueError("Missing terminal record. Keep the log through normal benchmark exit")
    if queue and not math.isfinite(queue["timestamp_period_ns"]):
        queue["timestamp_period_ns"] = None
    result = {**(queue or {}), **stopped, "durations_ms": None}
    if ticks:
        ordered = sorted(ticks)
        ms_per_tick = period / 1_000_000
        result["durations_ms"] = {
            "mean": sum(ordered) / len(ordered) * ms_per_tick,
            "p50": median(ordered) * ms_per_tick,
            "p95": ordered[math.ceil(0.95 * len(ordered)) - 1] * ms_per_tick,
            "max": ordered[-1] * ms_per_tick,
        }
        result["zero_intervals"] = ordered.count(0)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path)
    args = parser.parse_args()
    try:
        with args.log.open(encoding="utf-8", errors="replace") as stream:
            result = summarize(stream)
    except (KeyError, OSError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps(result, indent=2, allow_nan=False))


if __name__ == "__main__":
    main()
