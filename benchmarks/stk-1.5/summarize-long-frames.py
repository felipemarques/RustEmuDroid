"""Compare long game loops and Android presentation gaps in the three-player replay archive."""

import argparse
import csv
import io
import zipfile
from collections import defaultdict
from itertools import pairwise
from pathlib import Path
from statistics import mean


SCOPES = ("main_us", "race_us", "renderer_us", "draw_all_us", "end_scene_us", "acquire_us")


def by_lap(archive, name):
    result = defaultdict(list)
    with archive.open(name) as raw, io.TextIOWrapper(raw, encoding="utf-8", newline="") as stream:
        for row in csv.DictReader(stream):
            result[row.pop("player"), int(row.pop("lap"))].append({key: int(value) for key, value in row.items()})
    if not result:
        raise ValueError(f"{name} contains no samples")
    return result


def summarize(archive_path, hz):
    if hz <= 0:
        raise ValueError("The guest refresh rate must be positive")
    with zipfile.ZipFile(archive_path) as archive:
        games = by_lap(archive, "game-frames.csv")
        surfaces = by_lap(archive, "android-presentations.csv")
    if games.keys() != surfaces.keys():
        raise ValueError("The game and Android recordings must cover the same replays")

    for (player, lap), frames in games.items():
        normal, long = [], []
        for frame in frames:
            if not (frame["main_us"] > 0 and min(frame.values()) >= 0
                    and frame["race_us"] + frame["renderer_us"] <= frame["main_us"]
                    and frame["draw_all_us"] + frame["end_scene_us"] <= frame["renderer_us"]
                    and frame["acquire_us"] <= frame["end_scene_us"]):
                raise ValueError(f"Invalid game timing in {player} lap {lap}")
            group = long if frame["main_us"] * hz > 1_500_000 else normal
            group.append(frame)

        stamps = [row["relative_present_ns"] for row in surfaces[player, lap]]
        gaps = [b - a for a, b in pairwise(stamps)]
        if stamps[0] != 0 or not gaps or min(gaps) <= 0:
            raise ValueError(f"Invalid presentation sequence in {player} lap {lap}")
        differences = [
            round((mean(frame[key] for frame in long) - mean(frame[key] for frame in normal)) / 1000, 3)
            if long and normal else None for key in SCOPES
        ]
        yield (player, lap, len(frames), len(long), len(gaps),
               sum(gap * hz > 1_500_000_000 for gap in gaps), *differences)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--hz", type=int, required=True, help="verified Android refresh rate")
    args = parser.parse_args()
    print("player,lap,game_frames,long_game_frames,android_gaps,long_android_gaps,"
          + ",".join(key.removesuffix("_us") + "_long_minus_other_ms" for key in SCOPES))
    for result in summarize(args.archive, args.hz):
        print(",".join("" if value is None else str(value) for value in result))
