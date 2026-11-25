#!/usr/bin/env python3
"""Summarize STK 1.5 race and renderer scopes from matched replay CSVs."""

import argparse
import csv
import hashlib
import json
from pathlib import Path
import re


COLUMNS = {
    "main_loop": "Main loop",
    "irr_driver_update": "IrrDriver update",
    "update_race": "Update race",
    "world_update": "World::update()",
    "world_graphics_weather": "World::update (weather)",
    "world_graphics_camera": "World::updateGraphics (camera)",
    "input_gui": "Input/GUI",
    "music": "Music",
    "database_polling": "Database polling update",
    "protocol_manager": "Protocol manager update",
}
OTHER_SIBLINGS = ("input_gui", "music", "database_polling", "protocol_manager")


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def normalized(total_us, frames, duration_ms):
    return {
        scope: {
            "total_us": elapsed,
            "ms_per_frame": elapsed / frames / 1_000,
            "ms_per_profiled_second": elapsed / duration_ms,
        }
        for scope, elapsed in total_us.items()
    }


def analyze(report, profile):
    with report.open(encoding="utf-8-sig", newline="") as stream:
        rows = list(csv.reader(stream, skipinitialspace=True))
    expected_frames, duration_ms = map(int, rows[1][:2])
    graphics = {name.strip(): value.strip() for name, value in zip(rows[6][1:], rows[7][1:]) if name.strip()}
    if not (expected_frames > 0 and duration_ms > 0 and len(graphics) == 21):
        raise ValueError(f"{report.name}: invalid replay report")

    totals = dict.fromkeys(COLUMNS, 0)
    totals.update(
        update_race_unassigned=0,
        outside_renderer_and_race=0,
        main_loop_unassigned=0,
    )
    frames = zero_rows = 0
    with profile.open(encoding="utf-8-sig", newline="") as stream:
        heading = stream.readline()
        if not heading.startswith("#"):
            raise ValueError(f"{profile.name}: expected the STK profile header")
        header = next(csv.reader([heading[1:].strip()], skipinitialspace=True))
        header = [re.sub(r"\(\d+\)$", "", name.strip()) for name in header]
        if any(header.count(label) != 1 for label in COLUMNS.values()):
            raise ValueError(f"{profile.name}: expected exactly one column per scope")
        indices = {key: header.index(label) for key, label in COLUMNS.items()}
        for row_number, row in enumerate(csv.reader(stream), 2):
            values = {key: int(row[index]) for key, index in indices.items()}
            if values["main_loop"] == 0 and all(value == 0 for value in values.values()):
                zero_rows += 1
                continue

            values["outside_renderer_and_race"] = (
                values["main_loop"] - values["irr_driver_update"] - values["update_race"]
            )
            values["main_loop_unassigned"] = (
                values["outside_renderer_and_race"] - sum(values[key] for key in OTHER_SIBLINGS)
            )
            values["update_race_unassigned"] = (
                values["update_race"] - values["world_update"] -
                values["world_graphics_weather"] - values["world_graphics_camera"]
            )
            if values["main_loop"] <= 0 or min(values.values()) < 0:
                raise ValueError(f"{profile.name} row {row_number}: invalid scope nesting")
            frames += 1
            for key, value in values.items():
                totals[key] += value

    if frames != expected_frames or totals["main_loop"] // 1_000 != duration_ms:
        raise ValueError(f"{profile.name}: Main loop count/duration differs from report")

    return {
        "game_frames": frames,
        "profiled_duration_ms": duration_ms,
        "game_fps": frames * 1_000 / duration_ms,
        "zero_profile_rows_excluded": zero_rows,
        "scopes": normalized(totals, frames, duration_ms),
        "game_report_sha256": sha256(report),
        "frame_profile_sha256": sha256(profile),
    }, graphics


def pooled(runs):
    frames = sum(run["game_frames"] for run in runs)
    duration_ms = sum(run["profiled_duration_ms"] for run in runs)
    totals = {scope: sum(run["scopes"][scope]["total_us"] for run in runs) for scope in runs[0]["scopes"]}
    return {
        "game_frames": frames,
        "profiled_duration_ms": duration_ms,
        "game_fps": frames * 1_000 / duration_ms,
        "scopes": normalized(totals, frames, duration_ms),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lap", nargs=2, metavar=("REPORT", "PROFILE"), action="append", required=True)
    args = parser.parse_args()
    runs = []
    common_graphics = None
    try:
        for report, profile in args.lap:
            run, graphics = analyze(Path(report), Path(profile))
            if common_graphics is not None and graphics != common_graphics:
                raise ValueError("Replays did not use matching graphics settings")
            common_graphics = graphics
            runs.append(run)
    except (IndexError, OSError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps({"graphics": common_graphics, "runs": runs, "pooled": pooled(runs)}, indent=2))


if __name__ == "__main__":
    main()
