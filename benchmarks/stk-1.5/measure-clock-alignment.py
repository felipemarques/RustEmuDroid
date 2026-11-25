"""Compare elapsed guest time on each allowed vCPU with the host's perf_counter."""
import argparse
import hashlib
import json
import os
import queue
import secrets
import statistics
import subprocess
import threading
import time
from pathlib import Path


def collect(adb, binary, remote, rounds, interval):
    subprocess.run([*adb, "push", str(binary), remote], check=True, timeout=30,
                   stdout=subprocess.DEVNULL)
    subprocess.run([*adb, "shell", "chmod", "700", remote], check=True, timeout=10)
    with subprocess.Popen([*adb, "shell", "-T", remote], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as proc:
        events = queue.Queue()

        def read_lines():
            for line in proc.stdout:
                events.put((time.perf_counter_ns(), line.decode("ascii", errors="replace").strip()))
            events.put((time.perf_counter_ns(), None))

        reader = threading.Thread(target=read_lines, daemon=True)
        reader.start()
        try:
            _, ready = events.get(timeout=10)
            if not ready or not ready.startswith("READY "):
                raise ValueError(f"Guest clock probe did not start: {ready!r}")
            count = int(ready.split()[1])
            batches = []
            for number in range(rounds):
                start = time.perf_counter_ns()
                proc.stdin.write(f"{number}\n".encode("ascii"))
                proc.stdin.flush()
                rows = {}
                for _ in range(count):
                    _, line = events.get(timeout=10)
                    if not line:
                        raise ValueError("Guest probe exited during a batch")
                    fields = line.split()
                    if len(fields) != 6 or fields[0] != "CPU" or int(fields[1]) != number:
                        raise ValueError(f"Unexpected guest sample: {line}")
                    cpu, before, ticks, after = map(int, fields[2:])
                    if cpu in rows:
                        raise ValueError(f"Duplicate sample from CPU {cpu}: {line}")
                    rows[cpu] = {"before_ns": before, "ticks": ticks, "after_ns": after}
                end, trailer = events.get(timeout=10)
                if trailer != f"END {number}":
                    raise ValueError(f"Incomplete guest batch: {trailer!r}")
                batches.append({"host_send_ns": start, "host_end_ns": end, "cpus": rows})
                if number + 1 < rounds:
                    time.sleep(interval)
            proc.stdin.close()
            if proc.wait(timeout=10):
                raise ValueError("Guest clock probe failed")
            return batches
        finally:
            if proc.poll() is None:
                proc.kill()


def summarize(batches):
    first, last = batches[0], batches[-1]
    cpus = sorted(first["cpus"])
    if any(sorted(row["cpus"]) != cpus for row in batches):
        raise ValueError("Guest CPU affinity changed during measurement")
    wall_min = last["host_send_ns"] - first["host_end_ns"]
    wall_max = last["host_end_ns"] - first["host_send_ns"]
    if wall_min <= 0:
        raise ValueError("Host measurement windows overlapped")
    rates = {}
    tick_ratios = []
    regressions = []
    for cpu in cpus:
        samples = [b["cpus"][cpu] for b in batches]
        for index, sample in enumerate(samples):
            if sample["after_ns"] < sample["before_ns"]:
                regressions.append({"cpu": cpu, "batch": index, "clock": "raw_inside_bracket"})
            if index:
                previous = samples[index - 1]
                if sample["before_ns"] < previous["after_ns"]:
                    regressions.append({"cpu": cpu, "batch": index, "clock": "raw_between_batches"})
                if sample["ticks"] <= previous["ticks"]:
                    regressions.append({"cpu": cpu, "batch": index, "clock": "tsc_between_batches"})
        a, b = first["cpus"][cpu], last["cpus"][cpu]
        guest_min = b["before_ns"] - a["after_ns"]
        guest_max = b["after_ns"] - a["before_ns"]
        rates[cpu] = [1e6 * (guest_min / wall_max - 1), 1e6 * (guest_max / wall_min - 1)]
        if guest_min > 0 and b["ticks"] > a["ticks"]:
            tick_ratios.append((b["ticks"] - a["ticks"]) / ((guest_min + guest_max) / 2))
    summary = {"allowed_cpus": cpus, "guest_raw_minus_host_rate_bounds_ppm_by_cpu": rates,
               "sampled_regressions": regressions,
               "max_host_batch_round_trip_ms": max((b["host_end_ns"] - b["host_send_ns"]) / 1e6
                                                    for b in batches)}
    # A later read includes nonnegative elapsed ticks in addition to the offset.
    # Reversing CPU order gives the opposite bound without using either RAW epoch.
    ref = cpus[0]
    bounds = {ref: [0, 0]}
    for cpu in cpus[1:]:
        lower, upper = [], []
        for batch in batches:
            samples = batch["cpus"]
            order = list(samples)
            delta = samples[cpu]["ticks"] - samples[ref]["ticks"]
            if order.index(cpu) < order.index(ref):
                lower.append(delta)
            else:
                upper.append(delta)
        bounds[cpu] = [max(lower) if lower else None, min(upper) if upper else None]
    summary["ordered_tsc_offset_bounds_cycles_relative_to_first_cpu"] = bounds
    if regressions or len(tick_ratios) != len(cpus):
        return summary
    ticks_per_ns = statistics.median(tick_ratios)
    spreads, bracket = [], []
    for batch in batches:
        samples = batch["cpus"]
        a = samples[cpus[0]]
        offsets = []
        for row in samples.values():
            tick_ns = (row["ticks"] - a["ticks"]) / ticks_per_ns
            raw_ns = ((row["before_ns"] - a["before_ns"]) + (row["after_ns"] - a["after_ns"])) / 2
            offsets.append(tick_ns - raw_ns)
            bracket.append(row["after_ns"] - row["before_ns"])
        spreads.append(max(offsets) - min(offsets))
    summary.update(estimated_tsc_ticks_per_ns=ticks_per_ns,
                   per_round_spread_of_time_adjusted_tsc_offsets_ns=spreads,
                   largest_guest_clock_bracket_ns=max(bracket))
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adb", required=True)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--expected-avd", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=9)
    parser.add_argument("--interval", type=float, default=1.0)
    args = parser.parse_args()
    if args.rounds < 2 or not args.interval > 0:
        parser.error("At least two rounds separated by a positive interval are required.")
    if args.output.exists():
        parser.error("Choose a fresh output file.")
    adb = [args.adb, "-s", args.serial]
    identity = subprocess.check_output([*adb, "emu", "avd", "path"], text=True, timeout=10)
    lines = [line.strip() for line in identity.splitlines() if line.strip() and line.strip() != "OK"]
    if len(lines) != 1 or not os.path.samefile(lines[0], args.expected_avd):
        parser.error("The active AVD differs from --expected-avd.")
    remote = "/data/local/tmp/rustemudroid-clock-" + secrets.token_hex(8)
    try:
        batches = collect(adb, args.binary, remote, args.rounds, args.interval)
    finally:
        subprocess.run([*adb, "shell", "rm", "-f", remote], check=True, timeout=10)
    result = {"version": 1, "host_clock": time.get_clock_info("perf_counter").implementation,
              "guest_clock": "CLOCK_MONOTONIC_RAW", "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
              "interval_seconds": args.interval, "summary": summarize(batches), "batches": batches}
    with args.output.open("x", encoding="utf-8") as out:
        json.dump(result, out, indent=2)
        out.write("\n")
    print(json.dumps(result["summary"], indent=2))


if __name__ == "__main__":
    main()
