"""Run the ABI fixtures on one already-started, idle x86-64 Android guest."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import secrets
import socket
from statistics import median
import subprocess
import time

from build import ABIS, ACTIVITY, GOLDEN, digest

ORDER = ("x86_64", "arm64-v8a", "arm64-v8a", "x86_64")
COLUMNS = ["wall_ns", "thread_cpu_ns", "minor_faults", "major_faults"]
PROPERTIES = ("ro.build.fingerprint", "ro.build.version.sdk", "ro.build.version.release",
              "ro.product.cpu.abi", "ro.product.cpu.abilist", "ro.dalvik.vm.native.bridge")


def collect(args, report):
    # Require an existing local ADB server and address only the requested target.
    with socket.create_connection(("127.0.0.1", args.port), timeout=3):
        pass
    adb = [str(args.adb), "-H", "127.0.0.1", "-P", str(args.port), "-s", args.serial]

    def call(*command):
        result = subprocess.run([*adb, *map(str, command)], capture_output=True, text=True,
                                encoding="utf-8", errors="replace", timeout=90)
        if result.returncode:
            raise RuntimeError(f"ADB {command} failed: {result.stdout}{result.stderr}")
        return result.stdout.strip()

    def shell(*command):
        return call("shell", *command)

    def installed(user, package):
        listing = shell("pm", "list", "packages", "-u", "--user", user, package).splitlines()
        if any(not line.startswith("package:") for line in listing):
            raise RuntimeError(f"Could not check package state for user {user}: {listing}")
        return f"package:{package}" in listing

    if call("get-state") != "device":
        raise RuntimeError("The selected guest is unavailable")
    properties = {key: shell("getprop", key) for key in PROPERTIES}
    kernel = shell("uname", "-m")
    user = int(shell("am", "get-current-user"))
    users = [int(value) for value in re.findall(r"UserInfo\{(\d+):", shell("pm", "list", "users"))]
    report["identity"] = {"properties": properties, "kernel_machine": kernel, "user": user, "users": users}
    if (properties["ro.build.version.sdk"] != str(args.expected_sdk) or user not in users
            or kernel != "x86_64" or properties["ro.product.cpu.abi"] != "x86_64"):
        raise RuntimeError("The selected guest does not match the requested SDK, user or x86-64 baseline")
    if args.fingerprint and properties["ro.build.fingerprint"] != args.fingerprint:
        raise RuntimeError("The selected guest fingerprint does not match")
    if not args.execute:
        return

    receipt = args.build_dir / "build-receipt.json"
    build = json.loads(receipt.read_text(encoding="utf-8"))
    report.update(build_receipt_sha256=digest(receipt), golden=GOLDEN)
    if build["golden"] != GOLDEN:
        raise RuntimeError("The build receipt does not match this fixture's workloads")
    for abi, (_, package, _) in ABIS.items():
        apk, record = args.build_dir / f"rust-abicost-{abi}.apk", build["packages"][abi]
        if record["package"] != package or record["activity"] != ACTIVITY or digest(apk) != record["sha256"]:
            raise RuntimeError(f"Unexpected build for {abi}")
        if any(installed(guest_user, package) for guest_user in users):
            raise RuntimeError(f"The fixture package already exists: {package}")

    attempted, pids = [], set()
    try:
        for abi, (_, package, _) in ABIS.items():
            attempted.append(package)
            apk = args.build_dir / f"rust-abicost-{abi}.apk"
            if not call("install", "--user", user, apk).endswith("Success") or not installed(user, package):
                raise RuntimeError(f"Could not install {package}")
            primary = re.search(r"^\s*primaryCpuAbi=(\S+)\s*$", shell("dumpsys", "package", package), re.M)
            if not primary or primary[1] != abi:
                raise RuntimeError(f"Package Manager did not confirm {abi} for {package}")
            report["installed"].append({"package": package, "abi": primary[1],
                                        "apk_sha256": build["packages"][abi]["sha256"]})

        for lap, abi in enumerate(ORDER, 1):
            package = ABIS[abi][1]
            nonce = secrets.token_hex(16)
            launch = shell("am", "start", "-S", "-W", "--user", user, "-n", f"{package}/{ACTIVITY}",
                           "--es", "nonce", nonce, "--ei", "samples", args.samples)
            if "Status: ok" not in launch or "Error:" in launch:
                raise RuntimeError(f"Could not launch {package}: {launch}")
            record = {"lap": lap, "abi": abi, "nonce": nonce, "quiet_seconds": args.quiet_seconds}
            report["runs"].append(record)
            time.sleep(args.quiet_seconds)
            messages = shell("logcat", "-d", "-v", "raw", "-s", "RustAbiCost:V", "'*:S'")
            lines = [line[len(nonce) + 1:] for line in messages.splitlines() if line.startswith(nonce + " ")]
            if len(lines) != 1 or lines[0].startswith("ERROR "):
                raise RuntimeError(f"Expected one fixture result for {package}: {lines!r}")
            record["result"] = result = json.loads(lines[0])
            expected = {**GOLDEN, "abi": abi, "package": package, "process_64bit": True,
                        "pointer_bits": 64, "sample_columns": COLUMNS}
            if any(result.get(key) != value for key, value in expected.items()):
                raise RuntimeError(f"ABI, workload or digest mismatch for {package}")
            if result["pid"] in pids or result["pid"] <= 0 or result["tid"] <= 0 or result["pid"] == result["tid"]:
                raise RuntimeError("Expected a worker thread in a fresh fixture process")
            pids.add(result["pid"])
            record["statistics"] = dict(validate_samples(result, task, args.samples) for task in ("arithmetic", "memory"))
            print(f"Lap {lap}: {abi}", flush=True)
    finally:
        for package in reversed(attempted):
            try:
                if installed(user, package):
                    status = shell("pm", "uninstall", "--user", user, package)
                    if not status.endswith("Success") or installed(user, package):
                        raise RuntimeError(status)
                report["cleanup"][package] = "absent"
            except Exception as error:
                report["cleanup"][package] = str(error)

    if any(status != "absent" for status in report["cleanup"].values()):
        raise RuntimeError("Fixture package cleanup did not complete")


def validate_samples(result, task, count):
    rows = result[task]
    if len(rows) != count:
        raise RuntimeError(f"Wrong sample count for {task}")
    for wall, cpu, minor, major in rows:
        if not (wall > 0 and cpu > 0 and minor >= -1 and major >= -1):
            raise RuntimeError(f"Invalid clocks or page-fault counters for {task}")
    cpu_minus_wall_ns = [cpu - wall for wall, cpu, _, _ in rows]
    return task, {
        "median_wall_ns": median(row[0] for row in rows),
        "median_thread_cpu_ns": median(row[1] for row in rows),
        "major_faults": [row[3] for row in rows],
        "samples_cpu_exceeds_wall": sum(duration > 0 for duration in cpu_minus_wall_ns),
        "max_cpu_excess_ns": max(0, max(cpu_minus_wall_ns)),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adb", type=Path, required=True)
    parser.add_argument("--port", type=int, default=5037)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--expected-sdk", type=int, required=True)
    parser.add_argument("--fingerprint", help="Require this guest build fingerprint")
    parser.add_argument("--build-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=7, choices=range(1, 10), metavar="1..9")
    parser.add_argument("--quiet-seconds", type=int, default=15, choices=range(5, 121), metavar="5..120")
    parser.add_argument("--execute", action="store_true", help="Install, run and remove the two fixture APKs")
    args = parser.parse_args()
    if not args.adb.is_file() or not re.fullmatch(r"[a-zA-Z0-9._:-]+", args.serial) or args.output.exists():
        parser.error("Provide an existing ADB executable, the exact guest serial and a new output file")
    args.adb = args.adb.resolve()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    report = {"utc": datetime.now(timezone.utc).isoformat(), "serial": args.serial, "server_port": args.port,
              "execute": args.execute, "order": ORDER, "installed": [], "runs": [], "cleanup": {}}
    try:
        collect(args, report)
    except Exception as error:
        report["error"] = str(error)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if "error" in report:
        raise SystemExit(report["error"])
    print(args.output)


if __name__ == "__main__":
    main()
