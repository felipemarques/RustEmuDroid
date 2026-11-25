"""Run the isolated viewers sequentially on the same input and physical client size."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import threading
import time

import psutil


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def measure(executable, args):
    command = [str(executable.resolve()), "--width", str(args.width), "--height", str(args.height),
               "--window-width", str(args.window_width), "--window-height", str(args.window_height),
               "--fps", str(args.fps), "--seconds", str(args.seconds), "--warmup", str(args.warmup)]
    if args.raw:
        command.extend(["--raw", str(args.raw.resolve())])
    lines = []
    started = []
    samples = []
    deadline = time.monotonic() + args.warmup + args.seconds + 50
    with subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace",
                          creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)) as proc:
        def consume_output():
            for line in proc.stdout:
                if line.strip() == "viewer_source_started":
                    started.append(time.monotonic())
                lines.append(line)

        reader = threading.Thread(target=consume_output, daemon=True)
        reader.start()
        process = psutil.Process(proc.pid)
        try:
            while proc.poll() is None:
                now = time.monotonic()
                if now >= deadline:
                    raise TimeoutError(f"{executable.name} did not finish the measurement")
                try:
                    cpu = process.cpu_times()
                    samples.append((now, cpu.user + cpu.system, process.memory_info().rss))
                except psutil.NoSuchProcess:
                    break
                time.sleep(0.1)
        finally:
            if proc.poll() is None:
                try:
                    proc.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
            reader.join()
    log = "".join(lines)
    if proc.returncode:
        raise RuntimeError(f"{executable.name} exited with {proc.returncode}\n{log[-3500:]}")
    result, = [json.loads(line) for line in lines if line.lstrip().startswith('{"')]
    if (len(started) != 1 or result["source"] != ("raw" if args.raw else "synthetic")
            or result["window_width"] != args.window_width
            or result["window_height"] != args.window_height
            or result.get("client_width_px", result["window_width"]) != args.window_width
            or result.get("client_height_px", result["window_height"]) != args.window_height
            or (result["width"], result["height"]) != (args.width, args.height)
            or result["layers"] != 1 or result["target_fps"] != args.fps
            or not 0 <= result["processed"] <= result["produced"]
            or (result["processed"] == 0 and (result["produced"] != 0 or not args.raw
                or result["duplicates"] == 0 or result["elapsed_seconds"] < args.seconds))):
        raise RuntimeError(f"Viewer returned unexpected geometry, source or counters\n{log[-3500:]}")
    # Stay inside the measured interval; exclude startup, warm-up and shutdown.
    start = started[0] + args.warmup
    stable = [s for s in samples if start + 0.35 <= s[0] <= start + args.seconds - 0.35]
    if len(stable) < 3:
        raise RuntimeError("Insufficient CPU samples inside the source interval")
    duration = stable[-1][0] - stable[0][0]
    result["process_sample"] = {
        "wall_seconds": duration,
        "cpu_seconds": stable[-1][1] - stable[0][1],
        "rss_peak_mib": max(s[2] for s in stable) / (1024 ** 2),
    }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--glow", type=Path, required=True)
    parser.add_argument("--gpui", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="New JSON file")
    parser.add_argument("--raw", type=Path, help="Optional headerless RGBA input file")
    parser.add_argument("--width", type=int, default=1600)
    parser.add_argument("--height", type=int, default=900)
    parser.add_argument("--window-width", type=int, default=1040)
    parser.add_argument("--window-height", type=int, default=585)
    parser.add_argument("--fps", type=int, default=90)
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--seconds", type=int, default=10)
    args = parser.parse_args()
    if args.seconds < 3 or min(args.fps, args.width, args.height) < 1:
        parser.error("Use at least three measured seconds, a positive rate and positive frame dimensions.")
    out = {"os": os.name, "binary_sha256": {"glow": digest(args.glow), "gpui": digest(args.gpui)},
           "raw_input_sha256": None if not args.raw else digest(args.raw), "runs": []}
    with args.output.open("x", encoding="utf-8") as file:
        for expected, exe in (("glow", args.glow), ("gpui", args.gpui),
                              ("gpui", args.gpui), ("glow", args.glow)):
            result = measure(exe, args)
            if result["backend"] != expected:
                raise RuntimeError(f"Expected {expected}, received {result['backend']}")
            out["runs"].append(result)
            file.seek(0)
            json.dump(out, file, indent=2, ensure_ascii=False)
            file.write("\n")
            file.truncate()
            file.flush()
            print(f"{expected}: {result['processed_fps']:.2f} distinct painted frames/s", flush=True)
    # Backends expose different adapter structures. Keep the receipts when a hybrid
    # laptop routes them differently, but do not report that pair as a matched test.
    adapters = {
        (run["adapter"]["device_name"] if isinstance(run["adapter"], dict) else run["adapter"])
        .strip().casefold() for run in out["runs"]
    }
    if len(adapters) != 1:
        raise SystemExit(f"GPU names differ across runs: {', '.join(sorted(adapters))}. "
                         f"Check GPU routing before comparing. Receipts: {args.output}")


if __name__ == "__main__":
    main()
