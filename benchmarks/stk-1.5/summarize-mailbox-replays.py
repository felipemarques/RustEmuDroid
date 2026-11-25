"""Audit matched RTX SuperTuxKart replay profiles and Android presentation samples."""

import argparse
import csv
from hashlib import file_digest
import json
import math
from pathlib import Path
import re
import runpy


HERE = Path(__file__).resolve().parent
READ_PROFILE = runpy.run_path(str(HERE / "summarize-vulkan-end-scene.py"))["read_profile"]
SUMMARIZE_PRESENTATIONS = runpy.run_path(
    str(HERE.parent / "pubg-mobile/capture-presentations.py")
)["summarize"]
MODES = {0: "IMMEDIATE", 1: "MAILBOX", 2: "FIFO", 3: "FIFO_RELAXED"}
SCOPES = {
    "main_loop": "Main loop",
    "draw_all": "drawAll() for kart 0",
    "end_scene_acquire": "VK endScene acquire",
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    with path.open("rb") as stream:
        return file_digest(stream, "sha256").hexdigest()


def read(path):
    return json.loads(path.read_bytes())


def report(path):
    with path.open(encoding="utf-8-sig", newline="") as stream:
        rows = list(csv.reader(stream, skipinitialspace=True))
    frames, duration = map(int, rows[1][:2])
    settings = {name.strip(): value.strip() for name, value in zip(rows[6][1:], rows[7][1:])
                if name.strip()}
    require(frames > 0 and duration > 0 and len(settings) == 21, f"Invalid replay report: {path}")
    return frames, duration, settings


def presentation_sample(path):
    sample = read(path)
    stamps = sample.get("relative_present_ns", sample.get("relative_presentation_ns"))
    require(stamps and len(stamps) >= 2 and stamps[0] == 0
            and all(type(s) is int for s in stamps)
            and all(b > a for a, b in zip(stamps, stamps[1:])), f"Invalid presentation timestamps: {path}")
    elapsed_ns = round(sample["duration_s"] * 1e9)
    require(18e9 <= elapsed_ns <= 24e9, f"Expected a nominal 20-second capture: {path}")
    summary = SUMMARIZE_PRESENTATIONS(elapsed_ns, stamps)
    require(summary["presentations"] == sample["presentations"]
            and math.isclose(summary["presentations_per_second"], sample["presentations_per_second"], abs_tol=1e-6)
            and math.isclose(summary["p99_interval_ms"], sample["p99_interval_ms"], abs_tol=1e-6),
            f"Presentation metadata and timestamps disagree: {path}")
    return summary | {"relative_present_ns": stamps}, sample.get("native_rust_panel")


def run(engine, index, path, sidecar, rust=False):
    raw = path / "game" if rust else path
    profiler, result = raw / "quadros.csv", raw / "report.csv"
    frames, duration, settings = report(result)
    require(sidecar["abi"] == "x86_64" and sidecar["configured_max_fps"] == 90
            and sidecar["game_vsync"] is False and sidecar["frames"] == frames
            and sidecar["duration_ms"] == duration and sidecar["settings"] == list(settings.values())
            and math.isclose(sidecar["fps"], 1000 * frames / duration, abs_tol=1e-6),
            f"Replay settings or recorded FPS differ from the profiler: {path}")
    series = READ_PROFILE(profiler)
    main = series["Main loop"]
    require(len(main) == frames and sum(main) // 1000 == duration,
            f"Main loop count or truncation differs from the frame report: {path}")
    text = (raw / "renderer.txt").read_text(encoding="utf-8")
    modes = {int(mode) for mode in re.findall(r"VkPresentModeKHR:\s*(\d+)\s*$", text, re.M)}
    require(len(modes) == 1 and modes <= MODES.keys() and "Vulkan renderer:" in text,
            f"Vulkan present mode is missing or changed during the replay: {path}")
    mode, = modes
    report_sha, profile_sha = digest(result), digest(profiler)
    panel = None
    if rust:
        evidence = read(path / "verified.json")
        require(evidence["report_sha256"] == report_sha and evidence["trace_sha256"] == profile_sha
                and {int(mode) for mode in evidence["present_modes"]} == modes,
                f"Rust replay verification and raw files differ: {path}")
        panel_verified = evidence["windows_window_verified"]
        require(type(panel_verified) is bool, f"Missing Rust window qualification: {path}")
        sample_path = path / ("sample.json" if panel_verified else "android-cadence-unqualified.json")
        surface, panel = presentation_sample(sample_path)
        require((panel is not None) == panel_verified and
                (panel is None or panel["captured_size_px"] == [800, 450]),
                f"Unverified Rust panel: {path}")
    else:
        sample_path = path / "presentations.json"
        surface, _ = presentation_sample(sample_path)
    scopes = {}
    for name, column in SCOPES.items():
        values = series[column]
        total = sum(values)
        scopes[name] = {"total_us": total, "ms_per_frame": total / frames / 1000,
                        "positive_frames": sum(value > 0 for value in values)}
    return {"engine": engine, "run": index, "actual_vk_present_mode": MODES[mode],
            "actual_vk_present_mode_enum": mode, "game_frames": frames,
            "profiled_duration_ms": duration, "game_fps": frames * 1000 / duration,
            "game_report_sha256": report_sha, "frame_profile_sha256": profile_sha,
            "presentation_sample_sha256": digest(sample_path),
            "cpu_elapsed_scopes": scopes, "android_surface_view": surface,
            "rust_panel_verified": panel is not None, "rust_panel": panel}, settings


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    for engine in ("rust", "kemu", "blue"):
        cli.add_argument(f"--{engine}", type=Path, required=True)
    cli.add_argument("--output", type=Path, required=True)
    args = cli.parse_args()
    rust_evidence = read(args.rust.parent / "protocol.json")
    kemu_evidence = read(args.kemu / "preflight.json")
    blue_evidence = read(args.blue / "receipt.json")
    apk_sha = rust_evidence["game_sha256"]
    require(apk_sha == kemu_evidence["apk"]["verifier"]["sha256"] == blue_evidence["apk"]["verifier"]["sha256"],
            "The signed benchmark APK differs between players")
    require(blue_evidence["status"] == "paired" and len(blue_evidence["laps"]) == 2,
            "The Pie64 replay pair did not qualify")
    collected = []
    settings = None
    for engine, folder in (("RustEmuDroid", args.rust), ("Kemu", args.kemu), ("BlueStacks", args.blue)):
        rust = engine == "RustEmuDroid"
        sidecars = None if rust else read(folder / "results.json")
        for index in (1, 2):
            path = folder / (f"mailbox-{index}" if rust else f"lap-{index}")
            metadata = read(path / "game-result.json") if rust else sidecars[index - 1]
            row, current = run(engine, index, path, metadata, rust)
            if settings is None:
                settings = current
            require(settings == current, "The 21-field graphics settings differ between replays")
            collected.append(row)
    result = {"schema_version": 1, "game": "SuperTuxKart 1.5", "scene": "Black Forest",
              "preset": "High/Vulkan", "host_gpu": "NVIDIA GeForce RTX 5070 Ti", "guest_refresh_hz": 90,
              "game_abi": "x86_64", "configured_max_fps": 90, "game_vsync": False,
              "apk_sha256": apk_sha, "graphics_settings": settings,
              "player_evidence": {
                  "RustEmuDroid": {"version": rust_evidence["edition"], "guest_image": "Android 17 Google Play",
                                  "launcher_sha256": rust_evidence["app_sha256"],
                                  "qemu_sha256": rust_evidence["engine"]["qemu"]},
                  "Kemu": {"guest_image": "Android 16",
                           "launcher_sha256": kemu_evidence["runtime_binary_sha256"]["Kemu.exe"],
                           "qemu_sha256": kemu_evidence["runtime_binary_sha256"][
                               "qemu/windows-x86_64/qemu-system-x86_64.exe"]},
                  "BlueStacks": {"version": blue_evidence["official_player_file_version"],
                                 "guest_image": "Android 9 Pie64"},
              },
              "runs": collected}
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
