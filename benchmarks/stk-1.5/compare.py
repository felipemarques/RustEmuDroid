"""Run the same SuperTuxKart 1.5 replay on two already-running emulators."""
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
CONFIG = f"{DIRECTORY}/config.xml"

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


def replay(args, name, serial, preset, dest):
    if run(args, serial, "emu", "avd", "name").splitlines()[0].decode().strip().lower() != name.lower():
        raise RuntimeError(f"Unexpected AVD on {serial}")
    package = run(args, serial, "shell", "dumpsys", "package", PACKAGE)
    if b"versionName=1.5" not in package or b"primaryCpuAbi=x86_64" not in package:
        raise ValueError(f"SuperTuxKart 1.5 x86_64 required on {serial}")
    dest.mkdir(parents=True)
    run(args, serial, "shell", "am", "force-stop", PACKAGE)
    original = dest / "original.xml"  # Private recovery copy, removed after restoration.
    run(args, serial, "pull", CONFIG, str(original))
    tree = ET.parse(original)
    video, gfx = tree.find("Video"), tree.find("GFX")
    video.attrib.update(VIDEO)
    gfx.attrib.update(GFX)
    if preset == "high":
        video.attrib.update(enable_bloom="true", enable_glow="true", enable_light_shaft="true")
        gfx.attrib.update({"geometry-level": "4", "ssao": "true", "anisotropic": "8",
                           "shadows_resolution": "1024", "Degraded_IBL": "false"})
    if preset == "vulkan":
        video.set("render_driver", "vulkan")
    tree.find("commandline").set("value", "--benchmark")
    config = dest / "benchmark.xml"
    tree.write(config, encoding="utf-8", xml_declaration=True)
    try:
        run(args, serial, "push", str(config), CONFIG)
        before = run(args, serial, "shell", "stat", "-c", "%Y", REPORT, check=False)
        start = time.time()
        activity = f"{PACKAGE}/.SuperTuxKartActivity"
        run(args, serial, "shell", "am", "start", "-a", "android.intent.action.MAIN", "-c",
            "android.intent.category.LAUNCHER", "-n", activity)
        deadline = time.monotonic() + 150
        while time.monotonic() < deadline:
            time.sleep(2)
            stamp = run(args, serial, "shell", "stat", "-c", "%Y", REPORT, check=False)
            if stamp and stamp != before:
                data = run(args, serial, "exec-out", "cat", REPORT)
                rows = list(csv.reader(io.StringIO(data.decode())))
                if len(rows) < 65:
                    continue
                frames, duration = map(int, rows[1][:2])
                settings = [cell.strip() for cell in rows[7][1:] if cell.strip()]
                if settings[:3] != ["1600", "900", "1"]:
                    raise ValueError(f"Wrong resolution or scale on {name}: {settings[:3]}")
                if preset == "vulkan":
                    log = run(args, serial, "exec-out", "cat", DIRECTORY + "/stdout.log").decode(errors="replace")
                    driver = [line for line in log.splitlines() if "Vulkan" in line and
                              any(x in line.lower() for x in ("driver", "renderer", "device", "vendor"))]
                    if not driver:
                        raise ValueError(f"The game did not confirm its Vulkan renderer on {name}")
                    (dest / "renderer.txt").write_text("\n".join(driver) + "\n", encoding="utf-8")
                (dest / "report.csv").write_bytes(data)
                return dict(engine=name, preset=preset, start=start, end=time.time(),
                            frames=frames, duration_ms=duration, fps=frames*1000/duration,
                            mostly_stable=int(rows[4][1]), settings=settings)
        raise TimeoutError(f"No new complete performance report on {name}")
    finally:
        run(args, serial, "shell", "am", "force-stop", PACKAGE)
        run(args, serial, "push", str(original), CONFIG)
        if run(args, serial, "exec-out", "cat", CONFIG) != original.read_bytes():
            raise RuntimeError(f"Restore {original} to {serial}:{CONFIG}")
        original.unlink()
        config.unlink()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--adb", default="adb")
    p.add_argument("--kemu", required=True, help="ADB serial of the Kemu reference")
    p.add_argument("--rust", required=True, help="ADB serial of the integrated RustEmuDroid")
    p.add_argument("--rounds", type=int, default=3)
    p.add_argument("--presets", nargs="+", choices=["original", "high", "vulkan"], default=["original", "high"])
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    if args.rounds < 1 or args.kemu == args.rust:
        p.error("Use distinct emulators and at least one round")
    args.output.mkdir(parents=True, exist_ok=False)
    engines = [("Kemu", args.kemu), ("RustEmuDroid", args.rust)]
    results = []
    for preset in args.presets:
        settings = None
        for index in range(args.rounds):
            for engine, serial in (engines if index % 2 == 0 else reversed(engines)):
                result = replay(args, engine, serial, preset, args.output / preset / str(index+1) / engine)
                if settings and settings != result["settings"]:
                    raise ValueError(f"Game reported different graphics parameters for {preset}")
                settings = result["settings"]
                results.append(result)
                print(f"{preset} {index+1} {engine}: {result['fps']:.2f} FPS", flush=True)
                (args.output / "results.json").write_text(json.dumps(results, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
