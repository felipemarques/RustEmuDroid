"""Summarize the optional Vulkan endScene scopes in an STK 1.5 CPU profile."""

import argparse
import csv
import math
from pathlib import Path
import re
import statistics
import sys


MAIN = "Main loop"
DRIVER = "IrrDriver update"
DRAW = "drawAll() for kart 0"
END_SCENE = "VK endScene"
PHASES = tuple(f"{END_SCENE} {name}" for name in (
    "fence", "reset", "acquire", "record", "submit", "present",
))
RECOVERY = f"{END_SCENE} recovery"
REMAINDER = f"{END_SCENE} other"
SEGMENTS = (MAIN, DRIVER, END_SCENE, *PHASES, RECOVERY, REMAINDER)


def remove_trailing_column(row):
    return row[:-1] if row and not row[-1].strip() else row


def read_profile(path, *, scoped=True):
    required = (MAIN, DRIVER, END_SCENE, *PHASES) if scoped else (MAIN, DRIVER)
    series = {name: [] for name in (SEGMENTS if scoped else required)}
    with path.open(encoding="utf-8-sig", newline="") as stream:
        heading = stream.readline().removeprefix("#")
        headers = [
            re.sub(r"\(\d+\)$", "", field.strip())
            for field in remove_trailing_column(next(csv.reader([heading], skipinitialspace=True)))
        ]
        if not scoped and any(name.startswith(END_SCENE) for name in headers):
            raise ValueError("baseline profile contains optional Vulkan instrumentation")
        for name in required:
            if headers.count(name) != 1:
                raise ValueError(f"expected one {name!r} column, found {headers.count(name)}")
        if headers.count(RECOVERY) > 1:
            raise ValueError(f"duplicate {RECOVERY!r} column")
        columns = {name: headers.index(name) for name in required}
        if headers.count(DRAW) == 1:
            columns[DRAW] = headers.index(DRAW)
            series[DRAW] = []
        if RECOVERY in headers:
            columns[RECOVERY] = headers.index(RECOVERY)

        for line, row in enumerate(csv.reader(stream), start=2):
            row = remove_trailing_column(row)
            if len(row) != len(headers):
                raise ValueError(f"row {line} has {len(row)} columns, expected {len(headers)}")
            values = {name: int(row[index]) for name, index in columns.items()}
            if any(value < 0 for value in values.values()):
                raise ValueError(f"row {line} contains a negative duration")
            if values[MAIN] == 0:
                continue
            if values[DRIVER] > values[MAIN]:
                raise ValueError(f"row {line} violates profiler scope nesting")
            if scoped:
                values.setdefault(RECOVERY, 0)
                measured = sum(values[name] for name in (*PHASES, RECOVERY))
                if not measured <= values[END_SCENE] <= values[DRIVER]:
                    raise ValueError(f"row {line} violates profiler scope nesting")
                values[REMAINDER] = values[END_SCENE] - measured
            for name, value in values.items():
                series[name].append(value)

    if not series[MAIN]:
        raise ValueError("no positive Main loop duration")
    if scoped and not any(series[END_SCENE]):
        raise ValueError("no positive Vulkan endScene duration inside Main loop")
    return series


def write_summary(series):
    writer = csv.writer(sys.stdout, lineterminator="\n")
    writer.writerow(("segment", "frames_main_gt_0", "frames_segment_gt_0", "sum_ms",
                     "mean_ms", "p50_ms", "p95_ms"))
    for name, values in series.items():
        ordered = sorted(values)
        total = sum(ordered)
        count = len(ordered)
        p95 = ordered[math.ceil(0.95 * count) - 1]
        durations = (total, total / count, statistics.median(ordered), p95)
        writer.writerow((name, count, sum(value > 0 for value in ordered),
                         *(f"{value / 1000:.6f}" for value in durations)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", type=Path, help="STK *.profile-black_forest-cpu-0.csv")
    parser.add_argument("--baseline", action="store_true", help="require absence of optional Vulkan markers")
    args = parser.parse_args()
    try:
        series = read_profile(args.profile, scoped=not args.baseline)
    except (OSError, ValueError) as error:
        parser.exit(1, f"{args.profile}: {error}\n")
    write_summary(series)


if __name__ == "__main__":
    main()
