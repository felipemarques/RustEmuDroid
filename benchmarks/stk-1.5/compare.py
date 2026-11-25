"""Run the same SuperTuxKart 1.5 replay on running Android emulators."""
import argparse
import csv
import io
import json
from pathlib import Path
import re
import subprocess
import time
import xml.etree.ElementTree as ET

PACKAGE = "org.supertuxkart.stk"
DIRECTORY = f"/sdcard/Android/data/{PACKAGE}/files/supertuxkart/home/supertuxkart/config-0.10"
REPORT = f"{DIRECTORY}/stdout.log.perf-report-black_forest.csv"
TIMINGS = f"{DIRECTORY}/stdout.log.profile-black_forest-cpu-0.csv"
CONFIG = f"{DIRECTORY}/config.xml"
QA_PACKAGE = "org.rustemudroid.qa.stkvulkan"
QA_DIRECTORY = f"/sdcard/Android/data/{QA_PACKAGE}/files/stk-vulkan-qa/home/supertuxkart/config-0.10"
FRAME_TOTALS_OPTION = "--qa-frame-totals-only"
FRAME_TOTALS_MARKER = "STK_QA_FRAME_TOTALS_ONLY active"
DRAW_ALL_OPTION = "--qa-draw-all"
DRAW_ALL_MARKER = "STK_QA_DRAW_ALL_V1 active"
DRAW_ALL_PHASES = ("update_cache", "find_camera", "on_animate", "camera_draw_call",
                   "prepare", "on_register_scene_node", "generate")

VIDEO = dict(width="1600", height="900", real_width="1600", real_height="900",
             fullscreen="true", render_driver="opengl", max_fps="60", scale_rtts_factor="1.000000",
             enable_dynamic_lights="true", enable_texture_compression="true", enable_high_definition_textures="0",
             enable_bloom="false", enable_glow="false", enable_light_shaft="false", enable_dof="false",
             ssr="false", hq_mipmap="false", max_texture_size="256", show_fps="true")
GFX = {"particles-effecs": "2", "animated-characters": "true", "geometry-level": "2",
       "anisotropic": "4", "swap-interval-vsync": "1", "motionblur_enabled": "false",
       "mlaa": "false", "ssao": "false", "light_scatter": "false", "shadows_resolution": "0",
       "pcss": "false", "Degraded_IBL": "true"}


def run(args, serial, *cmd, check=True):
    return subprocess.run([args.adb, "-s", serial, *cmd], capture_output=True, check=check,
                          timeout=30).stdout


def frame_totals_pending(timings, frames, duration):
    """Check the frame-only CSV, returning a reason to retry a possibly partial file."""
    if not timings.startswith(b"#") or not timings.endswith(b"\n"):
        return "waiting for a complete frame-totals CSV"

    def single_column(row):
        fields = [field.strip() for field in row]
        if fields and not fields[-1]:
            fields.pop()  # STK exports one trailing comma even in the single-column mode.
        return fields[0] if len(fields) == 1 else None

    rows = csv.reader(io.StringIO(timings[1:].decode("utf-8")), skipinitialspace=True)
    if single_column(next(rows, [])) != "Main loop(1)":
        raise ValueError("Expected a single Main loop(1) column for QA frame totals")
    count = total_us = 0
    for row in rows:
        value = single_column(row)
        elapsed = int(value) if value and value.isascii() and value.isdecimal() else 0
        if elapsed <= 0:
            raise ValueError("Expected one positive integer duration per QA frame")
        count += 1
        total_us += elapsed
    measured_ms = total_us // 1000
    if count != frames or measured_ms != duration:
        return f"frame totals CSV has {count} rows and {measured_ms} ms, expected {frames} rows and {duration} ms"
    return None


def draw_all_coverage(timings, measured, frames, duration_ms, terminal_rows):
    if not timings.startswith(b"#") or not measured.endswith(b"\n"):
        raise ValueError("Incomplete CPU or QA drawAll CSV")
    profile = csv.reader(io.StringIO(timings[1:].decode("utf-8")), skipinitialspace=True)
    header = [re.sub(r"\(\d+\)$", "", field.strip()) for field in next(profile, [])]
    if header.count("Main loop") != 1 or header.count("drawAll() for kart 0") != 1:
        raise ValueError("Expected Main loop and kart 0 drawAll scopes in the CPU CSV")
    main, draw = header.index("Main loop"), header.index("drawAll() for kart 0")
    probes = csv.DictReader(io.StringIO(measured.decode("utf-8")))
    phases = [(f"{phase}_elapsed_ns", f"{phase}_cpu_ns") for phase in DRAW_ALL_PHASES]
    expected = ["profiler_row", "slot", "status", "mask",
                *(field for fields in phases for field in fields), "total_elapsed_ns", "total_cpu_ns"]
    if probes.fieldnames != expected:
        raise ValueError("Unexpected QA drawAll CSV heading")
    complete_mask = (1 << len(phases)) - 1
    counts = dict(complete=0, not_called=0, partial=0, clock_error=0)
    profiled = scene_frames = rows = total_us = 0
    for index, (cpu, probe) in enumerate(zip(profile, probes, strict=True)):
        if len(cpu) != len(header) or None in probe:
            raise ValueError("CPU or QA drawAll row differs from its heading")
        if int(probe["profiler_row"]) != index or int(probe["slot"]) < 0:
            raise ValueError("QA drawAll rows do not match the profiler order")
        status, mask = probe["status"], int(probe["mask"])
        if status not in counts or not 0 <= mask <= complete_mask:
            raise ValueError("Unexpected QA drawAll sample status or bitmask")
        if (status == "complete") != (mask == complete_mask) or (status == "not_called" and mask):
            raise ValueError("QA drawAll status disagrees with its phase mask")
        total = [0, 0]
        for bit, fields in enumerate(phases):
            values = [probe[field] for field in fields]
            if mask & (1 << bit):
                durations = [int(value) for value in values]
                if min(durations) < 0:
                    raise ValueError("Negative QA drawAll duration")
                total = [a + b for a, b in zip(total, durations)]
            elif values != ["", ""]:
                raise ValueError("Unmeasured QA drawAll phase has a duration")
        exported = [probe["total_elapsed_ns"], probe["total_cpu_ns"]]
        if exported != ([str(value) for value in total] if status == "complete" else ["", ""]):
            raise ValueError("QA drawAll totals disagree with their measured phases")
        rows += 1
        elapsed_us = int(cpu[main])
        if elapsed_us > 0:
            profiled += 1
            total_us += elapsed_us
            if int(cpu[draw]) > 0:
                scene_frames += 1
                counts[status] += 1
    if rows != terminal_rows or profiled != frames or total_us // 1000 != duration_ms:
        raise ValueError(f"QA drawAll rows disagree with terminal/report: "
                         f"{rows}/{terminal_rows} rows, {profiled}/{frames} frames, {total_us // 1000}/{duration_ms} ms")
    if not counts["complete"]:
        raise ValueError("No complete QA drawAll sample overlaps a timed kart 0 draw")
    return dict(rows=rows, positive_main_frames=profiled, positive_draw_frames=scene_frames, **counts)


def replay(args, name, serial, preset, dest, *, fps_limit=None, game_vsync=True):
    standalone_qa = getattr(args, "standalone_vulkan_qa", False)
    frame_totals_only = getattr(args, "qa_frame_totals_only", False)
    draw_all = getattr(args, "qa_draw_all", False)
    if frame_totals_only and not standalone_qa:
        raise ValueError(f"{FRAME_TOTALS_OPTION} requires --standalone-vulkan-qa")
    if draw_all and (not standalone_qa or frame_totals_only or preset not in ("vulkan", "high-vulkan")):
        raise ValueError(f"{DRAW_ALL_OPTION} requires standalone QA, Vulkan and the detailed CPU profiler")
    if name == "BlueStacks":
        if not run(args, serial, "shell", "getprop", "init.svc.bstsvcmgrtest").strip():
            raise RuntimeError(f"Expected BlueStacks on {serial}")
    elif run(args, serial, "emu", "avd", "name").splitlines()[0].decode().strip().lower() != name.lower():
        raise RuntimeError(f"Unexpected AVD on {serial}")
    if standalone_qa:
        package, directory = QA_PACKAGE, QA_DIRECTORY
        report = f"{directory}/stdout.log.perf-report-black_forest.csv"
        timings_path = f"{directory}/stdout.log.profile-black_forest-cpu-0.csv"
        config_path = f"{directory}/config.xml"
    else:
        package, directory = PACKAGE, DIRECTORY
        report, timings_path, config_path = REPORT, TIMINGS, CONFIG
    package_info = run(args, serial, "shell", "dumpsys", "package", package).decode().splitlines()
    props = dict(line.strip().split("=", 1) for line in package_info
                 if line.strip().startswith(("versionName=", "primaryCpuAbi=")))
    abi = props.get("primaryCpuAbi")
    valid_abis = ("armeabi-v7a", "arm64-v8a", "x86", "x86_64") if name == "BlueStacks" else ("x86_64",)
    if props.get("versionName") != "1.5" or abi not in valid_abis:
        raise ValueError(f"Unexpected game version or ABI on {name}: {props.get('versionName')}, {abi}")
    dest.mkdir(parents=True)
    run(args, serial, "shell", "am", "force-stop", package)
    original = dest / "original.xml"  # Private recovery copy, removed after restoration.
    run(args, serial, "pull", config_path, str(original))
    tree = ET.parse(original)
    video, gfx = tree.find("Video"), tree.find("GFX")
    video.attrib.update(VIDEO)
    if fps_limit is not None:
        video.set("max_fps", str(fps_limit))
    gfx.attrib.update(GFX)
    gfx.set("swap-interval-vsync", "1" if game_vsync else "0")
    if preset in ("high", "high-vulkan"):
        video.attrib.update(enable_bloom="true", enable_glow="true", enable_light_shaft="true")
        gfx.attrib.update({"geometry-level": "4", "ssao": "true", "anisotropic": "8",
                           "shadows_resolution": "1024", "Degraded_IBL": "false"})
    if preset in ("vulkan", "high-vulkan"):
        video.set("render_driver", "vulkan")
    benchmark = "--benchmark"
    if frame_totals_only or draw_all:
        benchmark += " " + (FRAME_TOTALS_OPTION if frame_totals_only else DRAW_ALL_OPTION)
    tree.find("commandline").set("value", benchmark)
    config = dest / "benchmark.xml"
    tree.write(config, encoding="utf-8", xml_declaration=True)
    try:
        run(args, serial, "push", str(config), config_path)
        if frame_totals_only or draw_all or not game_vsync:
            if run(args, serial, "exec-out", "cat", config_path) != config.read_bytes():
                mode = "QA frame-totals-only" if frame_totals_only else "QA drawAll" if draw_all else "VSync-off"
                raise RuntimeError(f"Could not verify the {mode} game configuration on {name}")
        def stamp(path):
            return run(args, serial, "shell", "stat", "-c", "%s:%Y", path, check=False).strip()

        before = stamp(report)
        before_timings = stamp(timings_path)
        draw_path = f"{directory}/stdout.log.qa-draw-all-black_forest.csv"
        before_draw = stamp(draw_path) if draw_all else b""
        last_read = "waiting for fresh profiler files"
        start = time.time()
        activity = f"{package}/.SuperTuxKartActivity"
        run(args, serial, "shell", "am", "start", "-a", "android.intent.action.MAIN", "-c",
            "android.intent.category.LAUNCHER", "-n", activity)
        deadline = time.monotonic() + 150
        while time.monotonic() < deadline:
            time.sleep(2)
            summary = stamp(report)
            timing_stamp = stamp(timings_path)
            if summary and summary != before and timing_stamp and timing_stamp != before_timings:
                data = run(args, serial, "exec-out", "cat", report)
                rows = list(csv.reader(io.StringIO(data.decode())))
                if len(rows) < 65:
                    continue
                frames, duration = map(int, rows[1][:2])
                settings = [cell.strip() for cell in rows[7][1:] if cell.strip()]
                if settings[:3] != ["1600", "900", "1"]:
                    raise ValueError(f"Wrong resolution or scale on {name}: {settings[:3]}")
                log = run(args, serial, "exec-out", "cat", directory + "/stdout.log").decode(errors="replace")
                driver = [line for line in log.splitlines() if any(x in line.lower() for x in
                          ("renderer:", "vendor:", "opengl version", "vulkan version", "vulkan driver",
                           "stk qa vulkan vkpresentmodekhr"))]
                if preset in ("vulkan", "high-vulkan") and not any("Vulkan" in line for line in driver):
                    raise ValueError(f"The game did not confirm its Vulkan renderer on {name}")
                if not frame_totals_only:
                    (dest / "renderer.txt").write_text("\n".join(driver) + "\n", encoding="utf-8")
                timings = run(args, serial, "exec-out", "cat", timings_path)
                if not timings.startswith(b"#") or abs(len(timings.splitlines()) - 1 - frames) > 2:
                    # The game may still be flushing per-frame records after writing the summary.
                    last_read = f"{len(timings.splitlines()) - 1} timing rows for {frames} frames"
                    continue
                if frame_totals_only:
                    mode_marker = next((line for line in log.splitlines()
                                        if line.rstrip().endswith(FRAME_TOTALS_MARKER)), None)
                    if mode_marker is None:
                        raise ValueError(f"Game did not report {FRAME_TOTALS_MARKER} on {name}")
                    pending = frame_totals_pending(timings, frames, duration)
                    if pending:
                        last_read = pending
                        continue
                    (dest / "renderer.txt").write_text("\n".join([*driver, mode_marker]) + "\n", encoding="utf-8")
                if draw_all:
                    lines = log.splitlines()
                    activation = [line for line in lines if line.rstrip().endswith(DRAW_ALL_MARKER)]
                    if len(activation) != 1:
                        raise ValueError(f"Expected one {DRAW_ALL_MARKER} on {name}")
                    terminal = [line for line in lines if re.search(r"STK_QA_DRAW_ALL_V1 end rows=\d+\s*$", line)]
                    draw_stamp = stamp(draw_path)
                    if not terminal or not draw_stamp or draw_stamp == before_draw:
                        last_read = "waiting for a fresh drawAll CSV and terminal marker"
                        continue
                    if len(terminal) != 1:
                        raise ValueError("Expected one terminal QA drawAll marker")
                    measured = run(args, serial, "exec-out", "cat", draw_path)
                    (dest / "draw-all.csv").write_bytes(measured)
                    (dest / "renderer.txt").write_text("\n".join([*driver, *activation, *terminal]) + "\n",
                                                       encoding="utf-8")
                (dest / "quadros.csv").write_bytes(timings)
                (dest / "report.csv").write_bytes(data)
                extra = {"draw_all_coverage": draw_all_coverage(timings, measured, frames, duration,
                                                               int(terminal[0].rsplit("=", 1)[1]))} if draw_all else {}
                return dict(engine=name, abi=abi, preset=preset, start=start, end=time.time(),
                            frames=frames, duration_ms=duration, fps=frames*1000/duration,
                            mostly_stable=int(rows[4][1]), settings=settings,
                            configured_max_fps=int(video.get("max_fps")), game_vsync=game_vsync, **extra)
        raise TimeoutError(f"No new complete performance report on {name}: {last_read}")
    finally:
        run(args, serial, "shell", "am", "force-stop", package)
        run(args, serial, "push", str(original), config_path)
        if run(args, serial, "exec-out", "cat", config_path) != original.read_bytes():
            raise RuntimeError(f"Restore {original} to {serial}:{config_path}")
        original.unlink()
        config.unlink()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--adb", default="adb")
    p.add_argument("--kemu", help="ADB serial of an optional Kemu reference")
    p.add_argument("--rust", help="ADB serial of the integrated RustEmuDroid")
    p.add_argument("--bluestacks", help="Local ADB serial of the BlueStacks App Player")
    p.add_argument("--rounds", type=int, default=3)
    p.add_argument("--fps-limit", type=int, default=60,
                   help="Write max_fps to the game's XML (default: 60). Benchmark mode skips the game-loop limiter.")
    p.add_argument("--game-vsync", choices=("on", "off"), default="on",
                   help="Set VSync in the game for each replay and restore its prior value afterwards.")
    p.add_argument("--presets", nargs="+", choices=["original", "high", "vulkan", "high-vulkan"], default=["original", "high"])
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--standalone-vulkan-qa", action="store_true",
                   help="Use the separate STK Vulkan QA package and its own configuration")
    qa_modes = p.add_mutually_exclusive_group()
    qa_modes.add_argument(FRAME_TOTALS_OPTION, action="store_true",
                          help="Measure only main-loop frame totals in the standalone QA package")
    qa_modes.add_argument(DRAW_ALL_OPTION, action="store_true",
                          help="Collect elapsed and thread CPU time for Vulkan drawAll phases in the standalone QA package")
    args = p.parse_args()
    if args.qa_frame_totals_only and not args.standalone_vulkan_qa:
        p.error(f"{FRAME_TOTALS_OPTION} requires --standalone-vulkan-qa")
    if args.qa_draw_all and (not args.standalone_vulkan_qa or not set(args.presets) <= {"vulkan", "high-vulkan"}):
        p.error(f"{DRAW_ALL_OPTION} requires --standalone-vulkan-qa and Vulkan presets")
    engines = [(name, serial) for name, serial in
               [("Kemu", args.kemu), ("RustEmuDroid", args.rust), ("BlueStacks", args.bluestacks)]
               if serial]
    if not engines:
        p.error("Select at least one emulator with --rust, --kemu or --bluestacks")
    if args.fps_limit < 1:
        p.error("--fps-limit must be positive")
    if args.rounds < 1 or len({serial for _, serial in engines}) != len(engines):
        p.error("Use distinct emulators and at least one round")
    args.output.mkdir(parents=True, exist_ok=False)
    results = []
    for preset in args.presets:
        settings = None
        for index in range(args.rounds):
            first = index % len(engines)
            for engine, serial in engines[first:] + engines[:first]:
                result = replay(args, engine, serial, preset, args.output / preset / str(index+1) / engine,
                                fps_limit=args.fps_limit, game_vsync=args.game_vsync == "on")
                if settings and settings != result["settings"]:
                    raise ValueError(f"Game reported different graphics parameters for {preset}")
                settings = result["settings"]
                results.append(result)
                print(f"{preset} {index+1} {engine}: {result['fps']:.2f} FPS", flush=True)
                (args.output / "results.json").write_text(json.dumps(results, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
