"""Export comparable Cipó Livre Android presentation receipts without private logs or images."""

import argparse
from datetime import UTC, datetime
import hashlib
import json
import math
from pathlib import Path
import re


PACKAGE = "br.com.felipemarques.cipolivre"
SCENES = ("forest", "walk")
PLAYERS = {"rust": "RustEmuDroid", "kemu": "Kemu", "blue": "BlueStacks", "bluestacks": "BlueStacks"}
RENDER = re.compile(
    r"\b(Vulkan|OpenGL(?: API)?(?: OpenGL ES| ES)?)\s+([0-9]+(?:\.[0-9]+){1,3})(?:[A-Za-z0-9._ ()-]*?)"
    r"\s+-\s+(Forward Mobile|Mobile|Forward\+|Compatibility)\s+-\s+Using Device(?: #\d+)?:\s*"
    r"([A-Za-z0-9._+/()®™ -]{1,160})\s*$", re.I | re.M
)
HUD = re.compile(r"\b(\d{1,4})\s*fps\s*[.·|:\- ]*\s*resolu[cç][aã]o\s*(?:3D\s*)?(\d{1,3})\s*%", re.I)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(path):
    return json.loads(path.read_bytes())


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def digest(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value), "Expected a SHA-256 digest")
    return value


def renderer(path):
    if not path.is_file():
        return None
    matches = set(RENDER.findall(path.read_text(encoding="utf-8", errors="replace")))
    if len(matches) != 1:
        return None
    api, version, mode, device = matches.pop()
    return {"api": api, "api_version": version, "godot_rendering_method": mode,
            "reported_device": device.strip(), "log_sha256": sha(path)}


def graphics(path):
    if path is None:
        return None
    proof = read(path)
    preset = proof["quality_preset"]
    evidence = proof["evidence"]
    require(preset in ("baixa", "media", "média", "alta", "ultra", "personalizada")
            and evidence in ("game-settings", "verified-menu-selection"),
            "Unrecognized preset proof")
    result = {"quality_preset": preset, "evidence": evidence, "settings_receipt_sha256": sha(path)}
    if "vsync" in proof:
        require(type(proof["vsync"]) is bool, "Unexpected VSync value")
        result["vsync"] = proof["vsync"]
    if "fps_limit" in proof:
        require(type(proof["fps_limit"]) is int and proof["fps_limit"] in (0, 30, 60, 120),
                "Unexpected game FPS limit")
        result["fps_limit"] = proof["fps_limit"]
    return result


def measured_frame_set(root, recorded, period):
    scene, repeat = recorded["scene"], recorded["repeat"]
    require(scene in SCENES and type(repeat) is int and repeat in (1, 2), "Unknown replay case")
    file = root / f"{scene}-{repeat}/frames.json"
    frame_set = read(file)
    timestamps = frame_set.pop("relative_stamps_ns")
    require(frame_set == recorded, f"Summary and raw frames disagree: {scene}-{repeat}")
    require(len(timestamps) >= 2 and timestamps[0] == 0
            and all(type(v) is int for v in timestamps)
            and all(a < b for a, b in zip(timestamps, timestamps[1:])), "Invalid presentation timeline")
    gaps = [b - a for a, b in zip(timestamps, timestamps[1:])]
    ordered = sorted(gaps)
    count = len(timestamps)
    require(type(frame_set["layer"]) is str and PACKAGE in frame_set["layer"]
            and "SurfaceView" in frame_set["layer"], "Unexpected game layer")
    seconds = frame_set["duration_s"]
    require(math.isfinite(seconds) and seconds > 0, "Invalid collection duration")
    rates = {
        "presentations_per_second": count / seconds,
        **{f"p{q}_ms": ordered[(len(ordered) - 1) * q // 100] / 1e6 for q in (50, 95)},
        "p99_interval_ms": ordered[(len(ordered) - 1) * 99 // 100] / 1e6,
        "max_interval_ms": ordered[-1] / 1e6,
        "intervals_over_250_ms": sum(gap > 250_000_000 for gap in gaps),
        "gaps_above_1_5_refresh": sum(2 * gap > 3 * period for gap in gaps),
    }
    require(count == frame_set["presentations"]
            and all(math.isclose(value, frame_set[key], rel_tol=1e-9) for key, value in rates.items()),
            f"Raw cadence disagrees with the summary: {scene}-{repeat}")
    hud = {(int(fps), int(scale)) for fps, scale in HUD.findall(frame_set.get("hud_ocr", ""))
           if 1 <= int(scale) <= 200}
    hud_reading = None
    if len(hud) == 1:
        fps, scale = hud.pop()
        hud_reading = {"fps": fps, "render_scale_percent": scale, "evidence": "paired labels in end-frame OCR"}
    return {"scene": scene, "repeat": repeat, "duration_s": seconds, "presentations": count,
            **rates, "presentation_gap_ns": gaps, "gestures_at_s": frame_set["gestures_at_s"],
            "hud_at_end": hud_reading, "raw_frame_receipt_sha256": sha(file)}


def run(name, folder, expected, settings):
    role = name.split("-")[0]
    require(role in PLAYERS and re.fullmatch(r"[a-z0-9-]+", name), "Unknown run label")
    root = folder / "game" if (folder / "game/results.json").is_file() else folder
    summary = read(root / "results.json")
    period = summary["display_period_ns"]
    px = summary["guest_screenshot_px"]
    require(type(period) is int and period > 0 and len(px) == 2 and
            all(type(value) is int and value > 0 for value in px) and px[0] > px[1], "Invalid display evidence")
    if role == "rust":
        install = read(folder / "install.json")
        require(install["package"] == PACKAGE and digest(install["sha256"]) == expected
                and "primaryCpuAbi=x86_64" in install["package_fields"], "Unverified installed Rust APK or ABI")
        profile = read(folder / "profile.json")
        version, player_hash = profile.get("release_version"), digest(profile["binary_sha256"])
    else:
        install = read(folder / "apk-cleanup.json")
        identity = read(folder / "identity.json")
        require(install["package"] == identity["package"] == PACKAGE
                and digest(install["installed_sha256"]) == digest(identity["apk_sha256"]) == expected
                and install["primary_cpu_abi"] == identity["primary_cpu_abi"] == "x86_64"
                and install["removed"], "Unverified peer APK, ABI or cleanup")
        version = identity["engine_version"]
        player_hash = None
    require(version is None or re.fullmatch(r"v?\d+(?:\.\d+){1,4}", version),
            "Unexpected player version label")
    intervals = [measured_frame_set(root, case, period) for case in summary["cases"]]
    cases = [(item["scene"], item["repeat"]) for item in intervals]
    require(cases and len(cases) == len(set(cases)), "Duplicate or missing replay cases")
    date_utc = datetime.fromtimestamp((root / "results.json").stat().st_mtime, UTC).date().isoformat()
    release = summary["android"]
    require(isinstance(release, str) and re.fullmatch(r"\d+(?:\.\d+){0,2}", release), "Unexpected Android release")
    return {"id": name, "player": PLAYERS[role], "player_version": version, "player_sha256": player_hash,
            "date_utc_from_summary_file": date_utc, "android_release": release, "apk_sha256": expected,
            "display_px": px, "surfaceflinger_nominal_period_ns": period, "warmup_s": summary["warmup_s"],
            "renderer": renderer(root / "renderer.txt"), "game_settings": graphics(settings),
            "complete_protocol": len(cases) == 4, "cases": intervals}


def pairs(values):
    result = {}
    for value in values:
        name, separator, path = value.partition("=")
        require(separator and name and path and name not in result, "Use unique RUN_ID=PATH arguments")
        result[name] = Path(path)
    return result


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--build-receipt", type=Path, required=True)
    cli.add_argument("--run", action="append", required=True, metavar="RUN_ID=FOLDER")
    cli.add_argument("--settings", action="append", default=[], metavar="RUN_ID=JSON")
    cli.add_argument("--output", type=Path, required=True)
    args = cli.parse_args()
    build = read(args.build_receipt)
    require(build["apk_package"] == PACKAGE and build["abi"] == "x86_64" and build["mode"] == "release"
            and re.fullmatch(r"[0-9a-f]{40}", build["source_commit"])
            and re.fullmatch(r"\d+\.\d+\.\d+\.stable\.official\.[a-f0-9]+", build["godot"]),
            "Unexpected QA build receipt")
    expected = digest(build["sha256"])
    runs, settings = pairs(args.run), pairs(args.settings)
    require(settings.keys() <= runs.keys(), "Preset evidence names a run that was not supplied")
    artifact = {key: build[key] for key in ("source_commit", "godot", "preset", "mode", "abi")}
    variant = build.get("qa_variant")
    require(variant in (None, "dummy-audio", "audio-main-thread", "audio-compatibility", "graphics-only"),
            "Unrecognized QA variant")
    artifact.update(package=PACKAGE, sha256=expected, signing_certificate_sha256=digest(build["signing_certificate_sha256"]),
                    audio_driver="Dummy" if build.get("qa_dummy_audio") else "default", qa_variant=variant)
    if variant == "audio-compatibility":
        require((build["renderer_requested"], build["rendering_driver_requested"]) ==
                ("gl_compatibility", "opengl3") and not build["qa_dummy_audio"],
                "Unexpected Compatibility build options")
        artifact.update(renderer_requested="gl_compatibility", rendering_driver_requested="opengl3")
    result = {"schema": 1, "game_build": artifact, "timestamp_encoding": "first presentation at zero; "
              "frame-to-frame nanosecond deltas in sample order",
              "runs": [run(name, folder, expected, settings.get(name)) for name, folder in runs.items()]}
    with args.output.open("x", encoding="utf-8", newline="\n") as output:
        output.write(json.dumps(result, ensure_ascii=False, separators=(",", ":")) + "\n")


if __name__ == "__main__":
    main()
