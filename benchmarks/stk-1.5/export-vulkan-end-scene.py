"""Export aggregate replay results without timestamps, process IDs, or local paths."""

import argparse
from contextlib import redirect_stdout
import csv
import hashlib
import io
import json
from pathlib import Path
import re
import runpy


PROFILE = runpy.run_path(str(Path(__file__).with_name("summarize-vulkan-end-scene.py")))
PACKAGE = "org.rustemudroid.qa.stkvulkan"
MODES = {0: "immediate", 1: "mailbox", 2: "FIFO", 3: "FIFO relaxed"}


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def read_report(path):
    with path.open(encoding="utf-8-sig", newline="") as source:
        rows = list(csv.reader(source))
    names, values = ([field.strip() for field in rows[i][1:] if field.strip()] for i in (6, 7))
    if len(names) != 21 or len(values) != 21 or len(set(names)) != 21 or any(not value.isdecimal() for value in values):
        raise ValueError("Expected all 21 reported graphics fields")
    frames, duration_ms = map(int, rows[1][:2])
    if frames <= 0 or not 34_000 <= duration_ms <= 45_000:
        raise ValueError("The Black Forest replay did not complete in the expected interval")
    return frames, duration_ms, names, values


def read_phases(path, frames):
    samples = PROFILE["read_profile"](path)
    if abs(len(samples["Main loop"]) - frames) > 2:
        raise ValueError("STK summary and CPU profile frame counts disagree")
    out = io.StringIO()
    with redirect_stdout(out):
        PROFILE["write_summary"](samples)
    return {row.pop("segment"): {key: float(value) if key.endswith("_ms") else int(value)
                                  for key, value in row.items()}
            for row in csv.DictReader(io.StringIO(out.getvalue()))}


def read_surface(path, android):
    if not path.exists():
        return None
    result = read_json(path)
    layer = rf"^(?:[a-fA-F0-9]+ )?SurfaceView(?:\[| - ){re.escape(PACKAGE)}/"
    if (result["package"] != PACKAGE or not re.match(layer, result["layer"])
            or str(result["android"]) != android or not 19 <= result["duration_s"] <= 21):
        raise ValueError("Invalid QA game SurfaceView, Android version, or measurement interval")
    count = result["presentations"]
    duration = result["duration_s"]
    if count < 1 or abs(count / duration - result["presentations_per_second"]) > 0.001:
        raise ValueError("Inconsistent SurfaceView counts")
    return {key: result[key] for key in ("duration_s", "presentations", "presentations_per_second",
                                        "p99_interval_ms", "max_interval_ms", "intervals_over_250_ms")}


def run_record(source, index, result, apk, android):
    trial = source / f"lap-{index}"
    report = trial / "report.csv"
    profile = trial / "quadros.csv"
    frames, duration, names, values = read_report(report)
    if (frames, duration, values) != (result["frames"], result["duration_ms"], result["settings"]):
        raise ValueError("The replay receipt does not match its game report")
    if result["abi"] != apk["abi"] or result["preset"] != "high-vulkan" or result["configured_max_fps"] != 90:
        raise ValueError("Expected the scoped build's ABI, High/Vulkan and the 90 FPS game cap")
    period = read_json(trial / "active-display.json")["period_ns"]
    if abs(period - 11_111_111) > 1:
        raise ValueError("Expected the active 90 Hz SurfaceFlinger period during the replay")
    log = (trial / "renderer.txt").read_text(encoding="utf-8-sig")
    modes = [int(mode) for mode in re.findall(r"STK QA Vulkan VkPresentModeKHR: (\d+)", log)]
    renderers = set(re.findall(r"Vulkan renderer: ([A-Za-z0-9 .,()_+-]{3,120})$", log, re.M))
    versions = set(re.findall(r"Vulkan version: (\d+\.\d+\.\d+)$", log, re.M))
    if not modes or any(mode not in MODES for mode in modes) or len(renderers) != 1 or len(versions) != 1:
        raise ValueError("Cannot verify game renderer, Vulkan version or selected present modes")
    return {"run": index, "game_frames": frames, "game_duration_ms": duration,
            "game_fps": frames * 1000 / duration, "game_report_sha256": digest(report),
            "frame_profile_sha256": digest(profile), "graphics": dict(zip(names, values, strict=True)),
            "surface_flinger_period_ns": period, "surface_view": read_surface(trial / "presentations.json", android),
            "vulkan_renderer": renderers.pop(), "vulkan_api_version": versions.pop(),
            "swapchains": [{"present_mode_id": mode, "present_mode": MODES[mode]} for mode in modes],
            "cpu_scopes": read_phases(profile, frames)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="QA results directory containing lap-N/ and the APK receipt")
    parser.add_argument("--engine", choices=("RustEmuDroid", "BlueStacks 5 Pie64"), required=True)
    parser.add_argument("--engine-version", required=True)
    parser.add_argument("--android", choices=("9", "17"), required=True)
    parser.add_argument("--match-reference", type=Path, help="A native aggregate for a paired BlueStacks export")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9]+(?:\.[0-9]+){2,4}", args.engine_version):
        parser.error("specify the public engine version")
    if args.engine == "BlueStacks 5 Pie64" and not args.match_reference:
        parser.error("supply the native reference before exporting a paired BlueStacks trial")
    apk = read_json(args.source / "apk-receipt.json")
    if (apk["mode"], apk["package"], apk["version_name"], apk["version_code"], apk["compiled_marker_count"]) != (
            "scoped", PACKAGE, "1.5", 328, 8):
        parser.error("expected the marked STK 1.5 QA APK receipt")
    if not re.fullmatch(r"[0-9a-f]{64}", apk["sha256"]) or apk["abi"] not in {"x86_64", "armeabi-v7a"}:
        parser.error("expected a valid APK digest and ABI")
    results = read_json(args.source / "results.json")
    if len(results) != 2:
        parser.error("expected two completed replays")
    runs = [run_record(args.source, index, result, apk, args.android) for index, result in enumerate(results, 1)]
    if runs[0]["graphics"] != runs[1]["graphics"]:
        parser.error("reported graphics settings changed between replays")
    output = {"schema_version": 1, "host": "Windows x86-64", "engine": args.engine,
              "engine_version": args.engine_version, "android_version": args.android,
              "game": "SuperTuxKart 1.5", "game_package": PACKAGE, "apk_sha256": apk["sha256"],
              "game_abi": apk["abi"], "scene": "Black Forest", "preset": "High/Vulkan",
              "game_fps_cap": 90, "game_vsync": True, "runs": runs}
    if args.match_reference:
        reference = read_json(args.match_reference)
        matching = ("schema_version", "apk_sha256", "game_abi", "scene", "preset", "game_fps_cap", "game_vsync")
        if reference["engine"] != "RustEmuDroid" or any(reference[key] != output[key] for key in matching):
            parser.error("native reference does not use the same APK, ABI and benchmark profile")
        native_runs = reference["runs"]
        if len(native_runs) != 2 or any(run["graphics"] != runs[0]["graphics"] or
                                       abs(run["surface_flinger_period_ns"] - runs[0]["surface_flinger_period_ns"]) > 1
                                       for run in native_runs):
            parser.error("native reference does not match graphics fields and active refresh")
        mode_sets = {frozenset(entry["present_mode_id"] for entry in run["swapchains"]) for run in (*native_runs, *runs)}
        if len(mode_sets) != 1:
            parser.error("native reference and trial used different Vulkan present modes")
    encoded = json.dumps(output, indent=2)
    if args.output:
        args.output.write_text(encoded + "\n", encoding="utf-8", newline="\n")
    else:
        print(encoded)


if __name__ == "__main__":
    main()
