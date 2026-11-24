"""Measure new Android presentations while an operator plays on one selected device."""

import argparse
import json
import re
import shlex
import subprocess
import sys
import time
from pathlib import Path


def run(args):
    return subprocess.check_output(args, text=True, encoding="utf-8", errors="replace", timeout=8)


def select_device(adb, port, serial):
    server = [adb, "-H", "127.0.0.1", "-P", str(port)]
    for line in run([*server, "devices", "-l"]).splitlines():
        fields = line.split()
        if fields[:2] == [serial, "device"]:
            for field in fields[2:]:
                if re.fullmatch(r"transport_id:[1-9]\d*", field):
                    return [*server, "-t", field.split(":")[1]]
    raise ValueError(f"No authorized transport ID for {serial}. Check adb devices -l.")


def shell(device, *args):
    return run([*device, "shell", shlex.join(args)])


def require_foreground(device, package):
    activities = shell(device, "dumpsys", "activity", "activities")
    # Android 9 and newer releases label the resumed activity differently.
    resumed = re.compile(r"\b(topResumedActivity|mResumedActivity|ResumedActivity)\s*[:=]")
    component = re.compile(r"\bu\d+\s+([\w.]+)/")
    focused = {}
    for line in activities.splitlines():
        marker, activity = resumed.search(line), component.search(line)
        if marker:
            focused.setdefault(marker[1], set()).add(activity[1] if activity else None)
    for marker in ("topResumedActivity", "mResumedActivity", "ResumedActivity"):
        if marker in focused:
            if focused[marker] == {package}:
                return
            break
    raise ValueError(f"Bring {package} to the foreground before measuring.")


def layers(device, package):
    listing = shell(device, "dumpsys", "SurfaceFlinger", "--list")
    names = []
    for line in listing.splitlines():
        if "{" in line and " parentId=" in line:
            line = line.split("{", 1)[1].split(" parentId=", 1)[0]
        name = line.strip()
        surface = re.match(r"^(?:[0-9a-fA-F]+\s+)?SurfaceView(?=[\s(\[-])", name)
        if surface and re.search(r"(?<![\w.])" + re.escape(package) + "/", name):
            names.append(name)
    return list(dict.fromkeys(names))


def presentation_times(dump):
    # FrameTracker writes desired, actual presentation and buffer-ready time.
    stamps = set()
    for line in dump.splitlines():
        cols = line.split()
        if len(cols) == 3 and all(col.isdecimal() for col in cols):
            value = int(cols[1])
            if 0 < value < 2**63 - 1:
                stamps.add(value)
    return stamps


def probe(device, name):
    started = time.monotonic_ns()
    dump = shell(device, "dumpsys", "SurfaceFlinger", "--latency", name)
    finished = time.monotonic_ns()
    return (started + finished) // 2, presentation_times(dump), finished - started


def select_layer(device, package, requested):
    names = layers(device, package)
    if requested and requested in names:
        return requested
    if not requested and names:
        first = {name: max(probe(device, name)[1], default=0) for name in names}
        time.sleep(0.3)
        moving = [name for name in names if max(probe(device, name)[1], default=0) > first[name]]
        if len(moving) == 1:
            return moving[0]
    choices = "\n".join(names) or "(none)"
    raise ValueError(f"Could not identify one advancing game surface. SurfaceViews for {package}:\n"
                     f"{choices}\nKeep the game visible or pass its exact --layer name.")


def collect(device, name, seconds):
    first_poll, previous, first_rtt = probe(device, name)
    if not previous:
        raise ValueError("This surface has no usable presentation timestamps.")
    baseline = max(previous)
    observed = set()
    end = first_poll + seconds * 1_000_000_000
    last_poll = first_poll
    while last_poll < end:
        wake = min(end, last_poll + 400_000_000)
        time.sleep(max(0, (wake - time.monotonic_ns()) / 1e9))
        last_poll, current, last_rtt = probe(device, name)
        if previous.isdisjoint(current):
            raise ValueError("Frame history lost continuity. The layer changed or polling missed "
                             "a full history buffer. Repeat with the same game surface.")
        observed.update(value for value in current if value > baseline)
        previous = current
    return last_poll - first_poll, sorted(observed), [first_rtt, last_rtt]


def summarize(elapsed_ns, stamps):
    gaps = sorted(b - a for a, b in zip(stamps, stamps[1:]))
    return {
        "duration_s": elapsed_ns / 1e9,
        "presentations": len(stamps),
        "presentations_per_second": len(stamps) * 1e9 / elapsed_ns,
        "p99_interval_ms": gaps[(len(gaps) - 1) * 99 // 100] / 1e6 if gaps else None,
        "max_interval_ms": gaps[-1] / 1e6 if gaps else None,
        "intervals_over_250_ms": sum(gap > 250_000_000 for gap in gaps),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adb", default="adb", help="path to the adb executable")
    parser.add_argument("--port", type=int, default=5037, help="port of the local ADB server")
    parser.add_argument("--serial", required=True, help="device name shown by adb devices -l")
    parser.add_argument("--package", default="com.tencent.ig")
    parser.add_argument("--layer", help="exact game SurfaceView name if automatic selection is ambiguous")
    parser.add_argument("--seconds", type=int, default=48)
    parser.add_argument("--output", type=Path, required=True, help="new JSON file for this run")
    args = parser.parse_args()
    if args.seconds < 1 or not 1 <= args.port <= 65535:
        parser.error("Use a positive duration and a valid ADB server port.")
    if args.output.exists() or not args.output.parent.is_dir():
        parser.error("--output must be a new file in an existing directory.")
    device = select_device(args.adb, args.port, args.serial)
    require_foreground(device, args.package)
    name = select_layer(device, args.package, args.layer)
    android = shell(device, "getprop", "ro.build.version.release").strip()
    print(f"Measuring {name} for {args.seconds} seconds. Play the chosen scene now.", flush=True)
    elapsed, stamps, boundary_rtts = collect(device, name, args.seconds)
    require_foreground(device, args.package)
    if name not in layers(device, args.package):
        raise ValueError("Game surface disappeared during the sample.")
    metrics = summarize(elapsed, stamps)
    report = {
        "android": android, "package": args.package, "layer": name, **metrics,
        "boundary_poll_round_trip_ms": [duration / 1e6 for duration in boundary_rtts],
        "presentation_ns_from_first_new_frame": [value - stamps[0] for value in stamps],
    }
    with args.output.open("x", encoding="utf-8") as stream:
        json.dump(report, stream, indent=2)
        stream.write("\n")
    print(json.dumps(metrics, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
