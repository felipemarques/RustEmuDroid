"""Export relative PUBG procfs and SurfaceFlinger summaries without guest access."""

import argparse
import json
from pathlib import Path
import re
from statistics import median

import analyze


NS = 1_000_000_000
INVALID = 2**63 - 1


def read(path):
    return json.loads(path.read_bytes())


def presentation_times(polls, layer):
    frames = {}
    for poll in polls:
        if poll["layer"] != layer:
            raise ValueError("Polls changed surfaces.")
        for line in poll["dump"].splitlines()[1:]:
            cells = line.split()
            if len(cells) != 3 or not all(cell.isdigit() for cell in cells):
                continue
            _, present, ready = map(int, cells)
            if not 0 < present < INVALID:
                continue
            candidates = frames.setdefault(present, set())
            if 0 < ready < INVALID:
                candidates.add(ready)
    return frames


def frame_metrics(frames, start, end):
    presents = sorted(frames)
    count = sum(start <= present < end for present in presents)
    gaps = sorted(
        min(right, end) - max(left, start)
        for left, right in zip(presents, presents[1:])
        if min(right, end) > max(left, start)
    )
    if not gaps:
        raise ValueError("No presentation intervals in this sample.")
    return {
        "seconds": (end - start) / NS,
        "presentations": count,
        "hz": count * NS / (end - start),
        "p50_ms": gaps[int((len(gaps) - 1) * 0.50)] / 1_000_000,
        "p95_ms": gaps[int((len(gaps) - 1) * 0.95)] / 1_000_000,
        "p99_ms": gaps[int((len(gaps) - 1) * 0.99)] / 1_000_000,
        "max_ms": gaps[-1] / 1_000_000,
        "over_100_ms": sum(gap > 100_000_000 for gap in gaps),
        "over_250_ms": sum(gap > 250_000_000 for gap in gaps),
    }


def stable_roles(samples):
    roles = set(samples[0]["tasks"])
    if not all(set(sample["tasks"]) == roles for sample in samples):
        raise ValueError("The tracked task set changed during this capture.")
    if not all(
        len({sample["tasks"][role][0] for sample in samples}) == 1 for role in roles
    ):
        raise ValueError("The process or a tracked task was replaced.")
    threads = [role for role in roles if role.startswith("UEGameThread (")]
    if not threads:
        raise ValueError("No UEGameThread accounting was captured.")
    first, last = samples[0]["tasks"], samples[-1]["tasks"]
    active = max(threads, key=lambda role: sum(last[role][3:]) - sum(first[role][3:]))
    labels = {role: re.sub(r" \(\d+\)$", "", role) for role in roles}
    for thread in threads:
        labels[thread] = "UEGameThread (other tasks combined)"
    labels[active] = "UEGameThread (highest CPU)"
    return labels


def counters(interval, samples, labels, origin, ticks):
    start, end = interval
    sampled = analyze.summarize([("observed", start / NS, end / NS)], samples)[0]
    report = {
        "window_relative_ns": [start - origin, end - origin],
        "window_duration_ns": end - start,
        "samples": sampled["samples"],
        "coverage_relative_ns": None,
        "coverage_ns": None,
        "coverage_fraction": None,
        "counters": None,
    }
    if "covered_s" not in sampled:
        return report
    coverage = [round(value * NS) for value in sampled["covered_s"]]
    covered_ns = coverage[1] - coverage[0]
    totals = {}
    for role, values in sampled["stats"].items():
        target = totals.setdefault(labels[role], {key: 0 for key in values})
        for key, value in values.items():
            target[key] += value
    for target in totals.values():
        target["cpu_ms"] = (
            (target["user_ticks"] + target["kernel_ticks"]) * 1000 / ticks
        )
    report.update(
        coverage_relative_ns=[value - origin for value in coverage],
        coverage_ns=covered_ns,
        coverage_fraction=covered_ns / (end - start),
        counters=dict(sorted(totals.items())),
    )
    return report


def export(directory):
    trace = directory / "proc-stat.txt"
    route = read(directory / "inventory.json")
    raw = read(directory / "inventory-sf-latency.json")
    lobby = read(directory / "lobby-sf-latency.json")
    layer = route["layer"]
    if lobby["layer"] != layer:
        raise ValueError(
            "The stationary sample and route did not use the same surface."
        )
    frames = presentation_times(raw["polls"], layer)
    if sorted(frames) != route["guest_presentation_ns"]:
        raise ValueError(
            "Distinct raw presentation timestamps differ from the route receipt."
        )
    origin = route["origin_ns"] - route["approximate_host_to_guest_offset_ns"]
    end = origin + 48 * NS
    metrics = frame_metrics(frames, origin, end)
    if metrics != route["total"]:
        raise ValueError("Raw calculations do not match the route totals.")
    schedule = [
        (12 * i + delay, action)
        for i in range(4)
        for delay, action in ((0, "open"), (7, "close"))
    ]
    reported = [
        ((tap["scheduled_ns"] - route["origin_ns"]) / NS, tap["action"])
        for tap in route["route"]
    ]
    if schedule != reported:
        raise ValueError("Unexpected Inventory schedule.")
    header = trace.read_text(encoding="utf-8").splitlines()[0].split()
    if header[:2] != ["H", "sample_interval_ms"] or header[3:6] != [
        "clock",
        "monotonic",
        "tick_hz",
    ]:
        raise ValueError("Unexpected observer clock header.")
    sample_ms, ticks = int(header[2]), int(header[6])
    if sample_ms <= 0 or ticks <= 0:
        raise ValueError("Invalid clock tick rate or sample interval.")
    samples = analyze.load(trace)
    labels = stable_roles(samples)
    stationary = lobby["guest_window_ns"]
    baseline = presentation_times(lobby["polls"], layer)
    lobby_metrics = frame_metrics(baseline, *stationary)
    result = {
        "schema": 1,
        "sampling": {
            "interval_ms": sample_ms,
            "clock_ticks_per_second": ticks,
            "complete_samples": len(samples),
            "median_interval_ms": median(
                (b["start"] - a["start"]) * 1000 for a, b in zip(samples, samples[1:])
            ),
        },
        "matching": {
            "raw_distinct_timestamps": len(frames),
            "route_timestamp_list_matches_raw": True,
            "complete_route_metrics_match_raw": True,
            "same_surface_for_lobby_and_route": True,
        },
        "method": {
            "time_zero": "First scheduled open, mapped approximately to guest monotonic time.",
            "counter_coverage": "First and last complete stat samples wholly inside each guest window.",
            "dominant_named_thread": "UEGameThread with highest CPU delta in the whole observation.",
            "close_gap": "Longest full observed presentation gap inside each nominal five-second close window. "
            "The paired ready gap uses those same two SurfaceFlinger records.",
        },
        "tap_schedule": [
            {"seconds": seconds, "action": action} for seconds, action in schedule
        ],
        "stationary_lobby": {
            "presentations": lobby_metrics,
            "procfs": counters(stationary, samples, labels, origin, ticks),
        },
        "route": {
            "presentations": metrics,
            "procfs": counters((origin, end), samples, labels, origin, ticks),
        },
        "closes": [],
    }
    for close in range(4):
        window = origin + (12 * close + 7) * NS, origin + (12 * close + 12) * NS
        sequence = sorted(time for time in frames if window[0] <= time < window[1])
        present = max(zip(sequence, sequence[1:]), key=lambda pair: pair[1] - pair[0])
        entry = {
            "visit": close + 1,
            "nominal_window_relative_ns": [value - origin for value in window],
            "present_gap": counters(present, samples, labels, origin, ticks),
            "ready_gap": None,
        }
        candidates = [sorted(frames[timestamp]) for timestamp in present]
        if (
            all(len(values) == 1 for values in candidates)
            and candidates[0][0] < candidates[1][0]
        ):
            ready = (candidates[0][0], candidates[1][0])
            entry["ready_gap"] = counters(ready, samples, labels, origin, ticks)
        else:
            entry["ready_candidates_relative_ns"] = [
                [value - origin for value in values] for values in candidates
            ]
        result["closes"].append(entry)
    conflicts = sum(len(values) > 1 for values in frames.values())
    if conflicts:
        result["matching"]["presentations_with_multiple_ready_times"] = conflicts
        result["matching"]["ambiguous_ready_times"] = [
            {
                "presentation_relative_ns": timestamp - origin,
                "ready_relative_ns": sorted(value - origin for value in values),
            }
            for timestamp, values in sorted(frames.items())
            if len(values) > 1
        ]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("raw_directory", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = export(args.raw_directory)
    with args.output.open("x", encoding="utf-8") as stream:
        json.dump(report, stream, indent=2)
        stream.write("\n")


if __name__ == "__main__":
    main()
