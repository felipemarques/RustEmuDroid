"""Summarize paired STK draw-byte diagnostic traces using the game profiler's frame eligibility."""

import argparse
import csv
import hashlib
import io
import json
import re
from pathlib import Path
from zipfile import ZipFile


ROLES = (
    "storage_skinning_bytes", "storage_object_bytes", "storage_material_bytes",
    "uniform_bytes", "indirect_bytes",
)
GROUPS = {
    "storage_bytes": ROLES[:3],
    "uniform_indirect_bytes": ROLES[3:],
    "all_counted_bytes": ROLES,
}
COUNTERS = (
    "frame", "storage_generate_calls", "storage_attempts", *ROLES[:3], "upload_calls",
    *ROLES[3:],
)
MARKER = "STK_QA_DRAW_BYTES_V1"


def main_loop(trace):
    rows = csv.reader(io.StringIO(trace.decode("utf-8-sig")), skipinitialspace=True)
    header = next(rows)
    if header[0].replace("#", "", 1).strip().strip('"') != "Main loop(1)":
        raise ValueError("Expected Main loop(1) as the first profiler column")
    return [int(row[0]) for row in rows]


def check_replay(files, mode):
    loops = main_loop(files[f"{mode}/quadros.csv"])
    report = csv.reader(io.StringIO(files[f"{mode}/report.csv"].decode("utf-8-sig")))
    next(report)
    count = int(next(report)[0])
    if count != sum(value > 0 for value in loops):
        raise ValueError(f"Report disagrees with the positive Main loop(1) count in {mode}")
    renderer = files[f"{mode}/renderer.txt"].decode("utf-8-sig")
    if renderer.count("STK QA Vulkan VkPresentModeKHR: 2") != 1:
        raise ValueError(f"Missing Vulkan FIFO selection in {mode}")
    return loops, renderer


def distribution(samples, denominator):
    ordered = sorted(samples)
    count = len(ordered)
    total = sum(ordered)
    return {
        "total_bytes": total,
        "mean_bytes": total / count,
        **{f"p{q}_bytes": ordered[(q * count + 99) // 100 - 1] for q in (50, 95, 99)},
        "peak_bytes": ordered[-1],
        "share_percent": 100 * total / denominator,
    }


def summarize(archive):
    with ZipFile(archive) as trace:
        expected = {f"{mode}/{name}" for mode in ("off", "on")
                    for name in ("quadros.csv", "report.csv", "renderer.txt")} | {"on/draw-bytes.csv"}
        if len(trace.namelist()) != len(expected) or set(trace.namelist()) != expected:
            raise ValueError("Unexpected trace member or missing replay evidence")
        files = {name: trace.read(name) for name in sorted(expected)}

    off_loops, off_renderer = check_replay(files, "off")
    loops, renderer = check_replay(files, "on")
    markers = re.findall(rf"{MARKER} ([^\r\n]+)", renderer)
    if MARKER in off_renderer or markers != ["active", f"end rows={len(loops)}"]:
        raise ValueError("OFF/ON markers do not establish a completed counter run")
    reader = csv.DictReader(io.StringIO(files["on/draw-bytes.csv"].decode("utf-8-sig")))
    if tuple(reader.fieldnames or ()) != COUNTERS:
        raise ValueError("Unexpected draw-byte columns")
    rows = [{key: int(row[key]) for key in COUNTERS} for row in reader]
    if len(rows) != len(loops):
        raise ValueError("Byte and profiler timelines have different lengths")
    selected = []
    excluded = []
    for index, (row, loop) in enumerate(zip(rows, loops, strict=True)):
        if (row["frame"] != index or min(row.values()) < 0
                or row["storage_attempts"] < row["storage_generate_calls"]
                or row["storage_skinning_bytes"] % 64 or row["storage_object_bytes"] % 64
                or row["storage_material_bytes"] % 4 or row["indirect_bytes"] % 20):
            raise ValueError(f"Invalid draw-byte row {index}")
        if loop > 0:
            selected.append(row)
        else:
            excluded.append({"frame": index, "main_loop_us": loop,
                             **{key: row[key] for key in COUNTERS[1:]}})
    total = sum(row[key] for row in selected for key in ROLES)
    if total <= 0:
        raise ValueError("No measured writes in positive profiler rows")
    return {
        "trace_member_sha256": {name: hashlib.sha256(data).hexdigest()
                               for name, data in files.items()},
        "off_game_frames": sum(value > 0 for value in off_loops),
        "on_rows": len(rows),
        "on_game_frames": len(selected),
        "excluded_rows": excluded,
        "call_counts": {key: sum(row[key] for row in selected)
                        for key in ("storage_generate_calls", "storage_attempts", "upload_calls")},
        "bytes": {
            **{key: distribution([row[key] for row in selected], total) for key in ROLES},
            **{key: distribution([sum(row[role] for role in roles) for row in selected], total)
               for key, roles in GROUPS.items()},
        },
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    print(json.dumps(summarize(parser.parse_args().archive), indent=2))
