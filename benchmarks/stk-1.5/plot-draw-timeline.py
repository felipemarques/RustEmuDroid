"""Plot render-thread CPU in the game's generate() function over the Black Forest replay."""

import argparse
import csv
import hashlib
import io
import json
from pathlib import Path
import re
import statistics
import zipfile


HERE = Path(__file__).resolve().parent
ENGINES = {"android17": "SDK · Android 17", "kemu16": "Kemu · Android 16"}


def intervals(profile, phases):
    rows = csv.reader(io.StringIO(profile[1:].decode()), skipinitialspace=True)
    if profile[:1] != b"#":
        raise ValueError("Expected a profiler header")
    columns = [re.sub(r"\(\d+\)$", "", name.strip()) for name in next(rows)]
    main_loop = columns.index("Main loop")
    draw = columns.index("drawAll() for kart 0")
    bins = {}
    elapsed_us = 0
    probes = csv.DictReader(io.StringIO(phases.decode()))
    for ordinal, (row, probe) in enumerate(zip(rows, probes, strict=True)):
        if int(probe["profiler_row"]) != ordinal:
            raise ValueError("Profiler and scene records have different row order")
        frame_us = int(row[main_loop])
        if frame_us <= 0:
            continue
        second = elapsed_us // 1_000_000
        elapsed_us += frame_us
        if int(row[draw]) <= 0 or probe["status"] != "complete":
            continue
        bucket = bins.setdefault(second, {"n": 0, "long": 0, "cpu_ns": 0})
        bucket["n"] += 1
        bucket["long"] += frame_us * 90 > 1_500_000
        bucket["cpu_ns"] += int(probe["generate_cpu_ns"])
    # Compare complete one-second intervals only. The last interval is partial.
    return [bins[s] for s in range(elapsed_us // 1_000_000)]


def collect():
    series = {}
    for engine in ENGINES:
        stem = f"windows-draw-all-{engine}"
        receipt = json.loads((HERE / f"{stem}.json").read_text(encoding="utf-8"))
        if receipt["display_hz"] != 90:
            raise ValueError("Expected the matched 90 Hz series")
        detailed = [run for run in receipt["runs"] if run["mode"] == "B"]
        if len(detailed) != 2:
            raise ValueError("Expected two instrumented runs")
        with zipfile.ZipFile(HERE / f"{stem}-b-replays.zip") as archive:
            for i, run in enumerate(detailed, 1):
                files = {}
                for name in ("quadros.csv", "draw-all.csv"):
                    data = archive.read(f"b{i}/{name}")
                    if hashlib.sha256(data).hexdigest() != run["sha256"][name]:
                        raise ValueError(f"Trace digest mismatch for {engine} B{i} {name}")
                    files[name] = data
                series[engine, i] = intervals(files["quadros.csv"], files["draw-all.csv"])
    if len({len(v) for v in series.values()}) != 1:
        raise ValueError("The runs cover different numbers of complete seconds")
    return series


def cpu_ms(bucket):
    return bucket["cpu_ns"] / bucket["n"] / 1_000_000


def chart(series, output):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    fig, ax = plt.subplots(figsize=(9.8, 4.4), layout="constrained")
    seconds = len(next(iter(series.values())))
    times = [s + 0.5 for s in range(seconds)]
    for start, stop in ((7, 9), (29, 31)):
        ax.axvspan(start, stop, color="#eeeeeb", zorder=0)
    for (engine, label), color in zip(ENGINES.items(), ("#24549a", "#a54e1a"), strict=True):
        pairs = [[cpu_ms(series[engine, i][s]) for i in (1, 2)] for s in range(seconds)]
        ax.fill_between(times, [min(p) for p in pairs], [max(p) for p in pairs], color=color, alpha=.23)
        ax.plot(times, [statistics.mean(p) for p in pairs], color=color, linewidth=2, label=label)
    ax.set(xlim=(0, seconds), ylim=(0, 10), xlabel="Elapsed replay time (s)", ylabel="Mean render-thread CPU per frame (ms)")
    ax.set_title("Generating Vulkan draw commands in Black Forest", loc="left", fontweight="bold", pad=12)
    ax.spines[["top", "right"]].set_visible(False)
    ax.grid(axis="y", color="#d7dce1", linewidth=.7)
    ax.legend(frameon=False, loc="upper right")
    fig.savefig(output, dpi=155, facecolor="white")
    plt.close(fig)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--figure", type=Path, help="write a PNG using matplotlib")
    args = parser.parse_args()
    series = collect()
    for (engine, run), buckets in series.items():
        marked = [buckets[s] for s in (7, 8, 29, 30)]
        print(f"{engine} B{run}: {sum(b['long'] for b in marked)} of {sum(b['long'] for b in buckets)} long frames in the two highlighted windows; "
              f"{len(buckets)} complete seconds")
    if args.figure:
        chart(series, args.figure)
