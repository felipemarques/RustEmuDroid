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


def read_phases(path, frames, scoped):
    samples = PROFILE["read_profile"](path, scoped=scoped)
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
            or str(result.get("android", android)) != android or not 19 <= result["duration_s"] <= 21):
        raise ValueError("Invalid QA game SurfaceView, Android version, or measurement interval")
    count = result["presentations"]
    duration = result["duration_s"]
    if count < 1 or abs(count / duration - result["presentations_per_second"]) > 0.001:
        raise ValueError("Inconsistent SurfaceView counts")
    return {key: result[key] for key in ("duration_s", "presentations", "presentations_per_second",
                                        "p99_interval_ms", "max_interval_ms", "intervals_over_250_ms")}


def run_record(source, index, result, apk, android, game_vsync):
    trial = source / f"lap-{index}"
    report = trial / "report.csv"
    profile = trial / "quadros.csv"
    frames, duration, names, values = read_report(report)
    if (frames, duration, values) != (result["frames"], result["duration_ms"], result["settings"]):
        raise ValueError("The replay receipt does not match its game report")
    if result["abi"] != apk["abi"] or result["preset"] != "high-vulkan" or result["configured_max_fps"] != 90:
        raise ValueError("Expected the approved ABI, High/Vulkan and the 90 FPS game preference")
    if result.get("game_vsync", True) != game_vsync:
        raise ValueError("The game configuration does not match the requested VSync setting")
    display = read_json(trial / "active-display.json")
    period = display.get("period_ns", display.get("sf_reported_period_ns"))
    if abs(period - 11_111_111) > 1:
        raise ValueError("Expected the active 90 Hz SurfaceFlinger period during the replay")
    log = (trial / "renderer.txt").read_text(encoding="utf-8-sig")
    modes = [int(mode) for mode in re.findall(r"STK QA Vulkan VkPresentModeKHR: (\d+)", log)]
    scoped = apk["mode"] == "scoped"
    renderers = set(re.findall(r"Vulkan renderer: ([A-Za-z0-9 .,()_+-]{3,120})$", log, re.M))
    versions = set(re.findall(r"Vulkan version: (\d+\.\d+\.\d+)$", log, re.M))
    drivers = set(re.findall(r"Vulkan driver version: (\d+(?:\.\d+){2,4})$", log, re.M))
    if (bool(modes) != scoped or any(mode not in MODES for mode in modes)
            or any(len(values) != 1 for values in (renderers, versions, drivers))):
        raise ValueError("Cannot verify game renderer, Vulkan version or selected present modes")
    if not scoped and "STK QA Vulkan VkPresentModeKHR" in log:
        raise ValueError("Unexpected present mode log in the baseline")
    return {"run": index, "game_frames": frames, "game_duration_ms": duration,
            "game_fps": frames * 1000 / duration, "game_report_sha256": digest(report),
            "frame_profile_sha256": digest(profile), "graphics": dict(zip(names, values, strict=True)),
            "surface_flinger_period_ns": period, "surface_view": read_surface(trial / "presentations.json", android),
            "vulkan_renderer": renderers.pop(), "vulkan_api_version": versions.pop(),
            "vulkan_driver_version": drivers.pop(),
            "swapchains": [{"present_mode_id": mode, "present_mode": MODES[mode]} for mode in modes] if scoped else None,
            "cpu_scopes": read_phases(profile, frames, scoped)}


def compare_baseline(output, reference, apk):
    fields = ("schema_version", "host", "engine", "engine_version", "android_version", "game_abi",
              "scene", "preset", "game_fps_preference", "game_vsync")
    if (any(reference[key] != output[key] for key in fields) or len(reference["runs"]) != 2
            or reference.get("instrumentation", "scoped") != "scoped"
            or apk["asset_match_reference_sha256"] != reference["apk_sha256"]
            or apk["matched_assets"] != 5577 or not apk["matching_signing_certificate"]):
        raise ValueError("Expected the scoped reference on the same engine, ABI, version and exact assets")
    pairs = []
    for baseline, scoped in zip(output["runs"], reference["runs"], strict=True):
        settings = ("run", "graphics", "vulkan_renderer", "vulkan_api_version", "surface_flinger_period_ns")
        if any(baseline[key] != scoped[key] for key in settings):
            raise ValueError("Scoped and baseline settings, renderer or active display mode differ")
        pair = {"run": baseline["run"], "game_fps": scoped["game_fps"] - baseline["game_fps"],
                "surface_presentations_per_second": None,
                "main_loop_p95_ms": scoped["cpu_scopes"]["Main loop"]["p95_ms"] - baseline["cpu_scopes"]["Main loop"]["p95_ms"],
                "driver_p95_ms": scoped["cpu_scopes"]["IrrDriver update"]["p95_ms"] - baseline["cpu_scopes"]["IrrDriver update"]["p95_ms"]}
        if scoped["surface_view"] and baseline["surface_view"]:
            pair["surface_presentations_per_second"] = (scoped["surface_view"]["presentations_per_second"]
                                                        - baseline["surface_view"]["presentations_per_second"])
        pairs.append(pair)
    return {"scoped_apk_sha256": reference["apk_sha256"], "matched_assets": apk["matched_assets"],
            "basis": "scoped minus baseline by replay ordinal, separate batches", "deltas": pairs}


def pooled_means(runs):
    scopes = runs[0]["cpu_scopes"].keys() & runs[1]["cpu_scopes"].keys()
    means = {scope: sum(run["cpu_scopes"][scope]["sum_ms"] for run in runs)
             / sum(run["cpu_scopes"][scope]["frames_main_gt_0"] for run in runs)
             for scope in runs[0]["cpu_scopes"] if scope in scopes}
    means["Main loop minus IrrDriver update"] = means["Main loop"] - means["IrrDriver update"]
    return means


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="QA results directory containing lap-N/ and the APK receipt")
    parser.add_argument("--engine", choices=("RustEmuDroid", "BlueStacks 5 Pie64"), required=True)
    parser.add_argument("--engine-version", required=True)
    parser.add_argument("--android", choices=("9", "17"), required=True)
    parser.add_argument("--mode", choices=("scoped", "baseline"), default="scoped")
    parser.add_argument("--game-vsync", choices=("on", "off"), default="on")
    parser.add_argument("--match-reference", type=Path, help="A native aggregate for a paired BlueStacks export")
    parser.add_argument("--compare-scoped", type=Path, help="The same engine and ABI with instrumentation enabled")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9]+(?:\.[0-9]+){2,4}", args.engine_version):
        parser.error("specify the public engine version")
    scoped = args.mode == "scoped"
    game_vsync = args.game_vsync == "on"
    if args.engine == "BlueStacks 5 Pie64" and scoped and not args.match_reference:
        parser.error("supply the native reference before exporting a paired BlueStacks trial")
    if not scoped and (not args.compare_scoped or args.match_reference):
        parser.error("a baseline requires --compare-scoped on the same engine and ABI")
    if args.engine == "BlueStacks 5 Pie64":
        trial = read_json(args.source / "receipt.json")
        if (args.android != "9" or trial["guest"]["android_api"] != 28 or not trial["qa_package_removed"]
                or not trial["stock_config_and_package_preserved"] or not trial["wrapper_preferences_restored"]
                or not trial["host_fps_restored"] or not trial["player_shutdown"]["process_exited"]
                or trial["remaining_processes"]):
            parser.error("BlueStacks APK cleanup, verified guest or setting restoration incomplete")
        apk = trial["apk"]["verifier"]
    else:
        apk = read_json(args.source / "apk-receipt.json")
    if (apk["mode"], apk["package"], apk["version_name"], apk["version_code"], apk["compiled_marker_count"]) != (
            args.mode, PACKAGE, "1.5", 328, 8 if scoped else 0):
        parser.error("expected the requested STK 1.5 QA APK receipt")
    if not re.fullmatch(r"[0-9a-f]{64}", apk["sha256"]) or apk["abi"] not in {"x86_64", "armeabi-v7a"}:
        parser.error("expected a valid APK digest and ABI")
    results = read_json(args.source / "results.json")
    if len(results) != 2:
        parser.error("expected two completed replays")
    runs = [run_record(args.source, index, result, apk, args.android, game_vsync)
            for index, result in enumerate(results, 1)]
    if runs[0]["graphics"] != runs[1]["graphics"]:
        parser.error("reported graphics settings changed between replays")
    output = {"schema_version": 1, "host": "Windows x86-64", "engine": args.engine,
              "engine_version": args.engine_version, "android_version": args.android,
              "game": "SuperTuxKart 1.5", "game_package": PACKAGE, "apk_sha256": apk["sha256"],
              "game_abi": apk["abi"], "instrumentation": args.mode, "scene": "Black Forest", "preset": "High/Vulkan",
              "game_fps_preference": 90, "game_vsync": game_vsync, "runs": runs,
              "pooled_mean_cpu_scope_ms": pooled_means(runs)}
    if args.match_reference:
        reference = read_json(args.match_reference)
        matching = ("schema_version", "apk_sha256", "game_abi", "scene", "preset", "game_fps_preference", "game_vsync")
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
    if not scoped:
        output["instrumentation_comparison"] = compare_baseline(output, read_json(args.compare_scoped), apk)
    encoded = json.dumps(output, indent=2)
    if args.output:
        args.output.write_text(encoded + "\n", encoding="utf-8", newline="\n")
    else:
        print(encoded)


if __name__ == "__main__":
    main()
