"""Count Android frame presentations after the scheduled Inventory opens and closes."""

import argparse
import csv
import sys
from pathlib import Path


HERE = Path(__file__).resolve().parent
SECOND = 1_000_000_000
ROUTE = 48
PLAYERS = ("BlueStacks", "RustEmuDroid")
INPUTS = {
    "Radeon 780M": ("inventory-ready-timing.csv", "present_from_route_start_ns",
                    ("bluestacks-radeon-", "rust-radeon-")),
    "RTX 5070 Ti": ("bluestacks-pie64-inventory.csv", "time_from_route_start_ns",
                     ("blue-balanced-", "balanced-")),
}


def phases(gpu, window):
    filename, time_column, prefixes = INPUTS[gpu]
    runs = {prefix + run: set() for prefix in prefixes for run in ("first", "repeat")}
    with (HERE / filename).open(encoding="utf-8", newline="") as source:
        for frame in csv.DictReader(source):
            if frame["run"] in runs:
                runs[frame["run"]].add(int(frame[time_column]))
    for player, prefix in zip(PLAYERS, prefixes):
        for run in ("first", "repeat"):
            times = runs[prefix + run]
            if not times or min(times) >= 0 or max(times) < ROUTE * SECOND:
                raise ValueError(f"Missing route boundary for {gpu}, {player}, {run}")
            counts = dict.fromkeys(("open", "close", "remaining"), 0)
            for t in times:
                if 0 <= t < ROUTE * SECOND:
                    position = t % (12 * SECOND)
                    if position < window * SECOND:
                        counts["open"] += 1
                    elif 7 * SECOND <= position < (7 + window) * SECOND:
                        counts["close"] += 1
                    else:
                        counts["remaining"] += 1
            for phase, count in counts.items():
                seconds = ROUTE - 8 * window if phase == "remaining" else 4 * window
                yield gpu, player, run, window, phase, seconds, count


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=int, default=2, choices=(1, 2, 3),
                        help="width of the window after each scheduled action (default 2)")
    window = parser.parse_args().seconds
    writer = csv.writer(sys.stdout, lineterminator="\n")
    writer.writerow(("gpu", "player", "run", "window_s", "phase", "duration_s", "presentations"))
    for gpu in INPUTS:
        writer.writerows(phases(gpu, window))
