"""Sample the current Windows working set and private commit of selected processes."""
import argparse
import json
import sys
import time

import psutil


def sample(pids, count=3, interval=1.0):
    if sys.platform != "win32":
        raise ValueError("This benchmark uses Windows process memory counters")
    if not pids or count < 1 or interval <= 0:
        raise ValueError("Select at least one process, a positive sample count and interval")
    processes = [psutil.Process(pid) for pid in pids]
    if len(set(pids)) != len(processes):
        raise ValueError("Select each process only once")
    rows = []
    start = time.perf_counter()
    for index in range(count):
        time.sleep(max(0, start + index * interval - time.perf_counter()))
        for process in processes:
            memory = process.memory_info()
            if not process.is_running():
                raise RuntimeError("A selected process exited or changed identity")
            rows.append({
                "elapsed_s": round(time.perf_counter() - start, 4),
                "name": process.name(),
                "pid": process.pid,
                "working_set_bytes": memory.wset,
                "private_commit_bytes": memory.private,
            })
    return rows


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True, nargs="+")
    parser.add_argument("--count", type=int, default=3)
    parser.add_argument("--interval", type=float, default=1.0, help="Seconds between samples")
    args = parser.parse_args()
    print(json.dumps(sample(args.pid, args.count, args.interval), indent=2))
