"""Open an existing Android 17 test profile in the Rust window with the staged WHPX engine."""
import argparse
import csv
import ctypes
from ctypes import wintypes
from contextlib import contextmanager
import hashlib
import json
import mmap
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time


QEMU = "emulator/qemu/windows-x86_64/qemu-system-x86_64.exe"
TSC = "RUSTEMUDROID_QA_WHPX_SYNC_TSC"
CACHE = "RUSTEMUDROID_QA_VULKAN_CACHED_HOST"
PROBE_MARK = "WHPX TSC QA:"
CACHE_MARK = "RustEmuDroid QA: requested cached Vulkan host allocations"
CACHE_EXCLUSION = "VulkanAllocateUncachedHostVisibleAsCached has no effect"


def properties(path):
    result = {}
    for row in path.read_text(encoding="utf-8-sig").splitlines():
        name, sep, value = row.partition("=")
        if sep:
            result[name.strip()] = value.strip()
    return result


def check_inputs(app, sdk, profile, fix="clock"):
    if not app.is_file() or app.name.lower() != "rustemudroid.exe":
        raise ValueError("Select the Windows RustEmuDroid executable with --app")
    receipt = json.loads((sdk.parent / "staging.json").read_bytes())
    for label, relative in (("qemu", QEMU), ("emulator", "emulator/emulator.exe")):
        with (sdk / relative).open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        if digest != receipt[f"{label}_sha256"]:
            raise ValueError(f"{relative} differs from the staging receipt")
    if fix == "vulkan-cache":
        with (sdk / QEMU).open("rb") as stream, mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as core:
            if core.find(CACHE.encode()) == -1:
                raise ValueError("Select the staged cache variant for the Vulkan memory experiment")
    if not (sdk / "platform-tools/adb.exe").is_file():
        raise ValueError("The staged runtime is missing platform-tools/adb.exe")
    registered = properties(profile / "avd/RustEmuDroid.ini")
    avd = profile / "avd/RustEmuDroid.avd"
    if Path(registered["path"]) != avd:
        raise ValueError(f"The profile registration must point to {avd}")
    config = properties(avd / "config.ini")
    if config.get("hw.cpu.ncore") != "4":
        raise ValueError("Select an existing four-vCPU test profile for this clock experiment")
    image = (sdk / config["image.sysdir.1"]).resolve(strict=True)
    props = properties(image / "source.properties")
    expected = {"SystemImage.TagId": "google_apis_playstore", "AndroidVersion.ApiLevel": "37.0",
                "Pkg.Revision": "7", "SystemImage.Abi": "x86_64"}
    if not (image / "system.img").is_file() or not all(props.get(k) == v for k, v in expected.items()):
        raise ValueError("Select the Android 17 Google Play r7 x86-64 image used in the clock tests")
    return image


@contextmanager
def single_test_window():
    api = ctypes.WinDLL("kernel32", use_last_error=True)
    api.CreateMutexW.argtypes = [ctypes.c_void_p, wintypes.BOOL, wintypes.LPCWSTR]
    api.CreateMutexW.restype = wintypes.HANDLE
    api.CloseHandle.argtypes = [wintypes.HANDLE]
    ctypes.set_last_error(0)
    handle = api.CreateMutexW(None, False, "Local\\RustEmuDroidWhpxClockLab")
    error = ctypes.get_last_error()
    if not handle:
        raise ctypes.WinError(error)
    try:
        if error == 183:
            raise ValueError("A clock experiment is already open")
        tasklist = Path(os.environ["SystemRoot"]) / "System32/tasklist.exe"
        tasks = subprocess.check_output([tasklist, "/fi", "imagename eq rustemudroid.exe",
                                         "/fo", "csv", "/nh"], text=True, errors="replace", timeout=15)
        if any(row and row[0].lower() == "rustemudroid.exe" for row in csv.reader(tasks.splitlines())):
            raise ValueError("Close the existing RustEmuDroid window, then start the experiment")
        yield
    finally:
        api.CloseHandle(handle)


def verify_closed_log(path, enabled, fix="clock"):
    whpx = False
    completed = False
    markers = []
    cache_requests = 0
    excluded = False
    with path.open(encoding="utf-8", errors="replace") as stream:
        for line in stream:
            whpx |= "WHPX on Windows" in line or "Windows Hypervisor Platform accelerator is operational" in line
            completed |= re.search(r"\bBoot completed in \d+ ms\b", line) is not None
            if PROBE_MARK in line:
                markers.append(line.partition(PROBE_MARK)[2].strip())
            cache_requests += CACHE_MARK in line
            excluded |= CACHE_EXCLUSION in line
    if not whpx:
        raise ValueError(f"The emulator did not report WHPX in {path}")
    if not completed:
        raise ValueError(f"The emulator did not report Android boot completion in {path}")
    phases = {"synchronized init vcpus=4", "synchronized reset vcpus=4"}
    clock_on = enabled and fix == "clock"
    cache_on = enabled and fix == "vulkan-cache"
    if (clock_on and set(markers) != phases) or (not clock_on and markers):
        raise ValueError(f"Clock markers differed from the requested mode in {path}: {markers}")
    if cache_requests != int(cache_on) or (cache_on and excluded):
        raise ValueError(f"Vulkan cache markers differed from the requested mode in {path}")


def launch(args):
    env = {k: v for k, v in os.environ.items() if k.upper() not in (TSC, CACHE)}
    enabled = args.mode == "on"
    if enabled:
        env[CACHE if args.fix == "vulkan-cache" else TSC] = "1"
    hardware = {"adapter": args.gpu_device, "camera": "Profile", "webcam_index": 0}
    if args.hz:
        hardware["refresh_rate"] = args.hz
    prefs = {"state": str(args.profile), "sdk": str(args.sdk), "image": "", "gpu": "Host",
             "hardware": hardware, "show_fps": True, "adaptive_video": True}
    with single_test_window(), tempfile.TemporaryDirectory(prefix="RustEmuDroid-clock-") as temporary:
        (Path(temporary) / "launcher.json").write_text(json.dumps(prefs), encoding="utf-8")
        existing = set(args.profile.glob("boot-*-*.log"))
        start_ms = time.time_ns() // 1_000_000
        cmd = [args.app, "--sdk", args.sdk, "--lang", args.lang, "--state-dir", temporary, "ui", "--play"]
        with subprocess.Popen(cmd, env=env) as child:
            if child.wait() != 0:
                raise ValueError(f"RustEmuDroid exited with code {child.returncode}")
        end_ms = time.time_ns() // 1_000_000
        logs = []
        for path in args.profile.glob(f"boot-*-{child.pid}.log"):
            match = re.fullmatch(r"boot-(\d+)-(\d+)\.log", path.name)
            if path not in existing and match and start_ms <= int(match[1]) <= end_ms:
                logs.append(path)
        if not logs:
            raise ValueError(f"No boot log from this launch was created in {args.profile}")
        for path in sorted(logs):
            verify_closed_log(path, enabled, args.fix)
            print(f"Boot and experiment markers verified ({args.fix} {args.mode}): {path}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("check", "on", "off"))
    parser.add_argument("--fix", choices=("clock", "vulkan-cache"), default="clock")
    for name in ("app", "sdk", "profile"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--hz", type=int, choices=(60, 90), help="Defaults to the selected profile's refresh rate")
    parser.add_argument("--gpu-device", default="", help="Exact host Vulkan device name, or automatic if omitted")
    parser.add_argument("--lang", choices=("pt-br", "en-us", "es-es"), default="pt-br")
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("WHPX clock tests require Windows")
    try:
        args.app, args.sdk, args.profile = (p.resolve(strict=True) for p in (args.app, args.sdk, args.profile))
        image = check_inputs(args.app, args.sdk, args.profile, args.fix)
        print(f"Runtime: {args.sdk}\nAndroid image: {image}\nWritable profile: {args.profile}", flush=True)
        if args.mode != "check":
            launch(args)
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as exc:
        parser.exit(1, f"Clock experiment: {exc}\n")


if __name__ == "__main__":
    main()
