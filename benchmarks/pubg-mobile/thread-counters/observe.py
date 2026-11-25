"""Collect a bounded task-accounting trace from a selected Android x86-64 PUBG process."""

import argparse
from pathlib import Path
import secrets
import subprocess
import time

from analyze import load


def collect(args):
    interval_ms = args.interval_ms if args.interval_ms is not None else 200
    if (
        not 25 <= interval_ms <= 1000
        or not 2 <= args.samples <= 3600
        or args.samples * interval_ms > 90000
    ):
        raise ValueError(
            "Use 25–1000 ms per sample and at most 90 seconds of requested delays."
        )
    if args.interval_ms is None and args.samples > 400:
        raise ValueError("Select --interval-ms explicitly when using over 400 samples.")
    selector = (
        ["-t", str(args.transport)]
        if args.transport is not None
        else ["-s", args.serial]
    )
    prefix = [str(args.adb), "-H", "127.0.0.1", "-P", str(args.port), *selector]

    def text(*cmd):
        return subprocess.check_output(
            [*prefix, *cmd], encoding="utf8", timeout=15
        ).strip()

    if (
        text("get-state") != "device"
        or text("get-serialno") != args.serial
        or text("shell", "getprop", "ro.build.version.sdk") != str(args.api)
        or text("shell", "uname", "-m") != "x86_64"
    ):
        raise RuntimeError("Unexpected device identity")
    pids = text("shell", "pidof", "com.tencent.ig").split()
    if (
        len(pids) != 1
        or not pids[0].isdigit()
        or (args.expected_pid is not None and int(pids[0]) != args.expected_pid)
    ):
        raise RuntimeError("Expected one running PUBG process")
    remote = "/data/local/tmp/rustemu-task-counts-" + secrets.token_hex(12)
    if (
        text("shell", "ls", "-d", "/data/local/tmp") != "/data/local/tmp"
        or args.out.exists()
    ):
        raise RuntimeError("Missing temporary directory or output already exists")
    begin = time.monotonic_ns()
    with args.out.open("xb") as dest:
        try:
            text("push", str(args.bin), remote)
            text("shell", "chmod", "700", remote)
            delay = [] if args.interval_ms is None else [str(args.interval_ms)]
            timeout = args.samples * (interval_ms / 1000 + 0.025) + 15
            subprocess.run(
                [*prefix, "exec-out", remote, pids[0], str(args.samples), *delay],
                stdout=dest,
                timeout=timeout,
                check=True,
            )
        finally:
            text("shell", "rm", "-f", remote)
    if len(load(args.out)) != args.samples:
        raise RuntimeError("The observer did not return every requested sample")
    print(
        f"observed {pids[0]} host monotonic {begin}..{time.monotonic_ns()}", flush=True
    )


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--adb", type=Path, required=True)
    p.add_argument("--bin", type=Path, default=Path(__file__).with_name("observe-stat"))
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--api", type=int, choices=(28, 37), required=True)
    p.add_argument("--serial", required=True)
    p.add_argument("--transport", type=int)
    p.add_argument("--expected-pid", type=int)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--samples", type=int, default=270, metavar="2..3600")
    p.add_argument(
        "--interval-ms",
        type=int,
        metavar="25..1000",
        help="delay between samples in milliseconds (default: 200)",
    )
    collect(p.parse_args())
