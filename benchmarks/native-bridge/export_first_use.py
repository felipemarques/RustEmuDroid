"""Verify guest reports and export individual code traversal timings."""
import argparse
import csv
import json
from pathlib import Path
from statistics import mean, median

from first_use import BANKS, golden


def export(reports, destination):
    records = []
    per_process = []
    build_hashes = set()
    for path in reports:
        report = json.loads(path.read_text(encoding="utf-8"))
        props = report["identity"]["properties"]
        guest = f"Android {props['ro.build.version.release']}"
        if ("error" in report or report.get("golden") != golden()
                or len(report["cleanup"]) != 2 or set(report["cleanup"].values()) != {"absent"}):
            raise ValueError(f"Incomplete or mismatched run in {path}")
        build_hashes.add(report["build_receipt_sha256"])
        abis = [run["abi"] for run in report["runs"]]
        if abis != ["x86_64", "arm64-v8a", "arm64-v8a", "x86_64"]:
            raise ValueError(f"Unexpected run order in {path}")
        for run in report["runs"]:
            result, abi = run["result"], run["abi"]
            if result["bank_digests"] != golden()["bank_digests"] or result["abi"] != abi:
                raise ValueError(f"Incorrect traversals in {path}")
            cold_cpu, warm_cpu = [], []
            for bank in range(BANKS):
                visits = result[f"bank_{bank}"]
                if len(visits) < 2:
                    raise ValueError("Each bank needs a first traversal and at least one repeat")
                cold_cpu.append(visits[0][1])
                warm_cpu.append(median(row[1] for row in visits[1:]))
                for visit, timing in enumerate(visits, start=1):
                    records.append((guest, props["ro.dalvik.vm.native.bridge"], run["lap"], abi,
                                    bank, visit, *timing))
            per_process.append((guest, run["lap"], abi, mean(cold_cpu) / 1e6, mean(warm_cpu) / 1e6))
    if len(build_hashes) != 1:
        raise ValueError("Use the same fixture build for every guest")
    with destination.open("x", newline="", encoding="utf-8") as output:
        writer = csv.writer(output, lineterminator="\n")
        writer.writerow(("guest", "native_bridge", "lap", "abi", "bank", "visit", "wall_ns",
                         "thread_cpu_ns", "minor_faults", "major_faults"))
        writer.writerows(records)
    print("Guest | lap | ABI | first CPU ms/bank | repeat CPU ms/bank")
    for guest, lap, abi, first, repeat in per_process:
        print(f"{guest} | {lap} | {abi} | {first:.3f} | {repeat:.3f}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reports", type=Path, nargs="+")
    parser.add_argument("--csv", type=Path, required=True, help="New output CSV path")
    args = parser.parse_args()
    export(args.reports, args.csv)
