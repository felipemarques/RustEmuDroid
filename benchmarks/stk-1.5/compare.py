"""Run the same SuperTuxKart 1.5 replay on running Android emulators."""
import argparse
import csv
import io
import json
from pathlib import Path
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


def replay(args, name, serial, preset, dest, *, fps_limit=None):
    if name == "BlueStacks":
        if not run(args, serial, "shell", "getprop", "init.svc.bstsvcmgrtest").strip():
            raise RuntimeError(f"Expected BlueStacks on {serial}")
    elif run(args, serial, "emu", "avd", "name").splitlines()[0].decode().strip().lower() != name.lower():
        raise RuntimeError(f"Unexpected AVD on {serial}")
    if getattr(args, "standalone_vulkan_qa", False):
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
    if preset in ("high", "high-vulkan"):
        video.attrib.update(enable_bloom="true", enable_glow="true", enable_light_shaft="true")
        gfx.attrib.update({"geometry-level": "4", "ssao": "true", "anisotropic": "8",
                           "shadows_resolution": "1024", "Degraded_IBL": "false"})
    if preset in ("vulkan", "high-vulkan"):
        video.set("render_driver", "vulkan")
    tree.find("commandline").set("value", "--benchmark")
    config = dest / "benchmark.xml"
    tree.write(config, encoding="utf-8", xml_declaration=True)
    try:
        run(args, serial, "push", str(config), config_path)
        def stamp(path):
            return run(args, serial, "shell", "stat", "-c", "%s:%Y", path, check=False).strip()

        before = stamp(report)
        before_timings = stamp(timings_path)
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
                (dest / "renderer.txt").write_text("\n".join(driver) + "\n", encoding="utf-8")
                timings = run(args, serial, "exec-out", "cat", timings_path)
                if not timings.startswith(b"#") or abs(len(timings.splitlines()) - 1 - frames) > 2:
                    # The game may still be flushing per-frame records after writing the summary.
                    last_read = f"{len(timings.splitlines()) - 1} timing rows for {frames} frames"
                    continue
                (dest / "quadros.csv").write_bytes(timings)
                (dest / "report.csv").write_bytes(data)
                return dict(engine=name, abi=abi, preset=preset, start=start, end=time.time(),
                            frames=frames, duration_ms=duration, fps=frames*1000/duration,
                            mostly_stable=int(rows[4][1]), settings=settings,
                            configured_max_fps=int(video.get("max_fps")))
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
    p.add_argument("--presets", nargs="+", choices=["original", "high", "vulkan", "high-vulkan"], default=["original", "high"])
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--standalone-vulkan-qa", action="store_true",
                   help="Use the separate STK Vulkan QA package and its own configuration")
    args = p.parse_args()
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
                                fps_limit=args.fps_limit)
                if settings and settings != result["settings"]:
                    raise ValueError(f"Game reported different graphics parameters for {preset}")
                settings = result["settings"]
                results.append(result)
                print(f"{preset} {index+1} {engine}: {result['fps']:.2f} FPS", flush=True)
                (args.output / "results.json").write_text(json.dumps(results, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
