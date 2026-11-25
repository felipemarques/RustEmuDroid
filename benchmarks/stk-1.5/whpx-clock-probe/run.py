"""Run a fresh process and a disposable WHP partition for each trial arm."""
import argparse
import csv
from hashlib import sha256
import io
import json
from pathlib import Path
import re
import subprocess


HERE = Path(__file__).resolve().parent
ARMS = ("running-clock", "suspended-clock", "suspended-clock", "running-clock")
PHASES = ("initial_guest", "baseline_first", "baseline_second", "set",
          "parked_first", "parked_second", "final_guest_first", "final_guest_second")


def require(condition, explanation):
    if not condition:
        raise ValueError(explanation)


def parse(output, arm):
    rows = output.decode("ascii").splitlines()
    header = re.fullmatch(r"# arm=([a-z-]+) qpc_hz=(\d+) whp_tsc_hz=(\d+) vp_count=4", rows[0])
    require(header and header[1] == arm and rows[-1] == "# cleanup_verified=1", "Unverified arm or cleanup")
    qpc_hz, tsc_hz = int(header[2]), int(header[3])
    require(qpc_hz > 0 and tsc_hz > 0, "Clock frequency not available")
    measurements = {}
    for row in csv.DictReader(io.StringIO("\n".join(rows[1:-1]))):
        phase = row.pop("phase")
        vp, before, after, tsc = (int(row[key]) for key in ("vp", "qpc_before", "qpc_after", "tsc"))
        require(before > 0 and after > before and vp not in measurements.get(phase, {}),
                "Invalid QPC bracket or repeated VP")
        measurements.setdefault(phase, {})[vp] = (before, after, tsc)
    expected = set(PHASES)
    if arm != "running-clock":
        expected.add("suspended")
    if arm == "suspended-clock":
        expected.add("resumed")
    require(set(measurements) == expected, "Unexpected trial phases")
    require(all(set(measurements[phase]) == {0, 1, 2, 3} for phase in PHASES), "Four VPs required")
    return qpc_hz, tsc_hz, measurements


def spread(samples, qpc_hz, tsc_hz):
    # A counter sample happened sometime inside the QPC call bracket. Tie each
    # intercept to an arbitrary shared origin to keep the arithmetic precise.
    qpc_origin, _, tsc_origin = samples[0]
    intervals = []
    for _, (before, after, tsc) in sorted(samples.items()):
        tsc_ns = 1e9 * (tsc - tsc_origin) / tsc_hz
        intervals.append([tsc_ns - 1e9 * (after - qpc_origin) / qpc_hz,
                          tsc_ns - 1e9 * (before - qpc_origin) / qpc_hz])
    lo, hi = (list(values) for values in zip(*intervals))
    mid = [(a + b) / 2 for a, b in intervals]
    return {"relative_offset_ns_by_vp": dict(enumerate(mid)),
            "midpoint_spread_ns": max(mid) - min(mid),
            "conservative_spread_bounds_ns": [max(0, max(lo) - min(hi)), max(hi) - min(lo)],
            "widest_qpc_bracket_ns": max(b - a for a, b in intervals)}


def rate(first, second, qpc_hz, tsc_hz):
    start_early, start_late, tsc_start = first
    end_early, end_late, tsc_end = second
    require(end_early > start_late and tsc_end > tsc_start, "A running clock failed to advance")
    tsc_ticks = (tsc_end - tsc_start) * qpc_hz / tsc_hz
    return [tsc_ticks / (end_late - start_early), tsc_ticks / (end_early - start_late)]


def measure(output, arm):
    qpc_hz, tsc_hz, phases = parse(output, arm)
    frozen = arm != "running-clock"
    sets = phases["set"]
    set_span_ms = 1000 * ((sets[3][0] + sets[3][1]) - (sets[0][0] + sets[0][1])) / (2 * qpc_hz)
    values = {sample[2] for sample in sets.values()}
    require(len(values) == 1, "Arms must write one identical counter value to every VP")
    value = values.pop()
    require(set_span_ms >= 50, "The deliberately spaced writes were too close")
    rates = {name: {vp: rate(phases[a][vp], phases[b][vp], qpc_hz, tsc_hz) for vp in range(4)}
             for name, a, b in (("pre_write_parked", "baseline_first", "baseline_second"),
                                ("post_resume_guest", "final_guest_first", "final_guest_second"))}
    require(all(0.98 <= lo <= hi <= 1.02 for samples in rates.values() for lo, hi in samples.values()),
            "Running TSC/QPC ratios are outside the declared sanity bound")
    parked = {vp: phases["parked_second"][vp][2] - phases["parked_first"][vp][2] for vp in range(4)}
    quiet_ms = 1000 * (phases["parked_second"][0][0] - phases["parked_first"][3][1]) / qpc_hz
    require(quiet_ms >= 25, "Post-write parked interval is shorter than expected")
    if frozen:
        require(all(phases[name][vp][2] == value for name in ("parked_first", "parked_second") for vp in range(4)),
                "A suspended counter changed despite parked VPs")
    else:
        require(all(ticks > 0 for ticks in parked.values()), "A VP counter did not advance in the control")
    results = {name: spread(phases[name], qpc_hz, tsc_hz)
               for name in ("final_guest_first", "final_guest_second")}
    for group in results.values():
        lower, upper = group["conservative_spread_bounds_ns"]
        if frozen:
            require(upper < 200_000, "Suspended write did not align the resumed guest TSCs within 0.2 ms")
        else:
            require(lower > 40_000_000 and abs(group["midpoint_spread_ns"] / 1e6 - set_span_ms) < 1,
                    "Control clock skew did not match the intentionally staggered writes")
    return {"arm": arm, "qpc_frequency_hz": qpc_hz, "whp_reported_tsc_frequency_hz": tsc_hz,
            "repeated_tsc_value": value, "first_to_last_write_midpoints_ms": set_span_ms,
            "post_write_parked_interval_ms": quiet_ms, "parked_delta_ticks_by_vp": parked,
            "tsc_samples_by_phase": {name: {vp: sample[2] for vp, sample in phases[name].items()}
                                     for name in ("parked_first", "parked_second",
                                                  "final_guest_first", "final_guest_second")},
            "guest_tsc_offsets": results, "tsc_over_qpc_rate_bounds": rates, "cleanup_verified": True}


def run(output, exe, auto_resume):
    output.mkdir()
    receipt = {"source_sha256": sha256((HERE / "probe.c").read_bytes()).hexdigest(),
               "exe_sha256": sha256(exe.read_bytes()).hexdigest(), "runs": []}
    order = ARMS + (("suspended-auto-resume",) if auto_resume else ())
    for pass_number, arm in enumerate(order, start=1):
        with subprocess.Popen([str(exe), arm], cwd=HERE, stdin=subprocess.DEVNULL,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE) as proc:
            try:
                stdout, stderr = proc.communicate(timeout=8)
            except subprocess.TimeoutExpired:
                proc.kill()
                stdout, stderr = proc.communicate()
                (output / f"{pass_number}-{arm}.csv").write_bytes(stdout)
                (output / f"{pass_number}-{arm}.stderr").write_bytes(stderr)
                raise
            status = proc.returncode
        (output / f"{pass_number}-{arm}.csv").write_bytes(stdout)
        (output / f"{pass_number}-{arm}.stderr").write_bytes(stderr)
        require(status == 0 and not stderr, f"Trial {pass_number} failed; retained its raw stdout/stderr")
        case = measure(stdout, arm)
        case.update(pass_number=pass_number, raw_csv_sha256=sha256(stdout).hexdigest())
        receipt["runs"].append(case)
        print(f"{pass_number} {arm}: {case['guest_tsc_offsets']['final_guest_second']['conservative_spread_bounds_ns']}")
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--auto-resume-proof", action="store_true")
    args = parser.parse_args()
    run(args.output.resolve(), args.binary.resolve(strict=True), args.auto_resume_proof)
