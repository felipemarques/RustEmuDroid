"""Plot two Inventory routes per player on the Radeon 780M or the RTX 5070 Ti."""

import argparse
import csv
from bisect import bisect_left
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.ticker import MultipleLocator


HERE = Path(__file__).resolve().parent
SECOND = 1_000_000_000
SECONDS = range(48)


def counts_for_runs(source, players):
    times = {run: set() for _, _, runs in players for run in runs}
    with source.open(encoding="utf-8", newline="") as src:
        for row in csv.DictReader(src):
            if row["run"] in times:
                times[row["run"]].add(int(row["present_from_route_start_ns"]))
    counts = {}
    for name, frames in times.items():
        frames = sorted(frames)
        if not frames or frames[0] >= 0 or frames[-1] < len(SECONDS) * SECOND:
            raise ValueError(f"Missing measurement boundary in {name}")
        edges = [bisect_left(frames, t * SECOND) for t in range(len(SECONDS) + 1)]
        counts[name] = [b - a for a, b in zip(edges, edges[1:])]
    return counts


def plot(source, output, gpu, players):
    counts = counts_for_runs(source, players)
    fig, axes = plt.subplots(2, 1, sharex=True, figsize=(10, 4.7))
    fig.subplots_adjust(left=.083, right=.98, bottom=.14, top=.80, hspace=.42)
    fig.text(.083, .94, f"PUBG MOBILE  ·  Inventory on the {gpu}",
             weight="bold", size=16, color="#162931")
    fig.text(.083, .885, "Android frames presented per second   |   "
             "Shaded intervals show Inventory open", size=11, color="#415762")
    for ax, (player, color, runs) in zip(axes, players):
        for start in range(0, 48, 12):
            ax.axvspan(start, start + 7, color="#e8edf0", linewidth=0, zorder=0)
        for run, linestyle, label in zip(runs, ("-", "--"), ("First run", "Repeat")):
            ax.stairs(counts[run], range(49), baseline=None, color=color,
                      linewidth=1.7, linestyle=linestyle, label=label)
        ax.set_title(player, loc="left", fontsize=12, color=color, fontweight="bold", pad=5)
        ax.set(xlim=(0, 48), ylim=(0, 36), yticks=(0, 10, 20, 30))
        ax.grid(axis="y", color="#a6b8c1", linewidth=.5, alpha=.7)
        ax.spines[["top", "right", "left"]].set_visible(False)
        ax.spines["bottom"].set_color("#a6b8c1")
        ax.tick_params(length=0, pad=5, colors="#394e59")
        ax.legend(loc="lower right", ncol=2, frameon=True, framealpha=.95, fontsize=8)
    axes[-1].xaxis.set_major_locator(MultipleLocator(6))
    axes[-1].set_xlabel("Seconds from the first Inventory tap", labelpad=6)
    fig.savefig(output, dpi=160, facecolor="white")
    plt.close(fig)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rtx", action="store_true", help="Plot the v0.18.0 RTX comparison")
    if parser.parse_args().rtx:
        plot(HERE / "v0.18.0-rtx-inventory.csv", HERE / "v0.18.0-rtx-inventory.png",
             "RTX 5070 Ti", (("BlueStacks Pie64", "#007887", ("blue-1", "blue-2")),
                             ("RustEmuDroid v0.18.0", "#b44c16", ("native-1", "native-2"))))
    else:
        plot(HERE / "inventory-ready-timing.csv", HERE / "radeon-inventory.png", "Radeon 780M",
             (("BlueStacks Pie64", "#007887", ("bluestacks-radeon-first", "bluestacks-radeon-repeat")),
              ("RustEmuDroid", "#b44c16", ("rust-radeon-first", "rust-radeon-repeat"))))
