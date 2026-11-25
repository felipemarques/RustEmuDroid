"""Compare scene traversal elapsed time and render-thread CPU time in one instrumented STK replay."""

import argparse
import csv
import hashlib
import io
import json
from pathlib import Path
import re
from statistics import mean

from compare import DRAW_ALL_MARKER, DRAW_ALL_PHASES, draw_all_coverage


def summarize(directory, hz):
    if hz <= 0:
        raise ValueError("The Android refresh rate must be positive")
    data = {name: (directory / name).read_bytes()
            for name in ("quadros.csv", "draw-all.csv", "report.csv", "renderer.txt")}
    log = data["renderer.txt"].decode().splitlines()
    terminal = [match for line in log if (match := re.search(r"STK_QA_DRAW_ALL_V1 end rows=(\d+)\s*$", line))]
    if len(terminal) != 1 or sum(line.rstrip().endswith(DRAW_ALL_MARKER) for line in log) != 1:
        raise ValueError("Expected the activation and terminal markers from the game")
    report = csv.reader(io.StringIO(data["report.csv"].decode("utf-8-sig")), skipinitialspace=True)
    next(report)
    frames, duration = map(int, next(report)[:2])
    coverage = draw_all_coverage(data["quadros.csv"], data["draw-all.csv"], frames, duration, int(terminal[0][1]))
    profile = csv.reader(io.StringIO(data["quadros.csv"][1:].decode()), skipinitialspace=True)
    names = [re.sub(r"\(\d+\)$", "", field.strip()) for field in next(profile)]
    main, draw = names.index("Main loop"), names.index("drawAll() for kart 0")
    phases = (*DRAW_ALL_PHASES, "total")
    measurements = {"other_frames": [], "long_frames": []}
    eligible = dict.fromkeys(measurements, 0)
    for row, probe in zip(profile, csv.DictReader(io.StringIO(data["draw-all.csv"].decode())), strict=True):
        main_us, draw_us = int(row[main]), int(row[draw])
        if main_us <= 0 or draw_us <= 0:
            continue
        group = "long_frames" if main_us * hz > 1_500_000 else "other_frames"
        eligible[group] += 1
        if probe["status"] != "complete":
            continue
        values = {"game_loop_ms": main_us / 1000, "outer_draw_all_ms": draw_us / 1000}
        for phase in phases:
            elapsed = int(probe[f"{phase}_elapsed_ns"]) / 1e6
            cpu = int(probe[f"{phase}_cpu_ns"]) / 1e6
            values[f"{phase}_elapsed_ms"] = elapsed
            values[f"{phase}_cpu_ms"] = cpu
            values[f"{phase}_elapsed_minus_cpu_ms"] = elapsed - cpu
        measurements[group].append(values)

    groups = {}
    for group, samples in measurements.items():
        values = {key: mean(row[key] for row in samples) for key in samples[0]} if samples else {}
        groups[group] = {"eligible_frames": eligible[group], "complete_frames": len(samples), "means": values}
    other, long = (group["means"] for group in groups.values())
    delta = {key: long[key] - other[key] for key in long.keys() & other.keys()}
    return {"refresh_hz": hz, "game_frames": frames, "duration_ms": duration, "coverage": coverage,
            "groups": groups, "long_minus_other_ms": dict(sorted(delta.items())),
            "sha256": {name: hashlib.sha256(raw).hexdigest() for name, raw in data.items()}}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path, help="directory with compare.py's output from one B replay")
    parser.add_argument("--hz", type=int, required=True, help="verified guest display rate")
    args = parser.parse_args()
    print(json.dumps(summarize(args.directory, args.hz), indent=2, allow_nan=False))
