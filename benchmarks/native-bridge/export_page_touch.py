"""Export page-touch timings and fault counts without publishing device identifiers."""
import argparse
import csv
import json
from pathlib import Path

from runner import ABIS, COLUMNS, ORDER


def export(reports, destination):
    rows = []
    builds = set()
    page_sizes = set()
    guests = set()
    for path in reports:
        report = json.loads(path.read_text(encoding="utf-8"))
        properties = report["identity"]["properties"]
        android = properties["ro.build.version.release"]
        if android in guests:
            raise ValueError("Reports with the same Android release would have indistinguishable CSV rows")
        guests.add(android)
        if (report.get("error") or not report["execute"]
                or report["cleanup"] != {package: "absent" for _, package, _ in ABIS.values()}
                or tuple(lap["abi"] for lap in report["runs"]) != ORDER):
            raise ValueError(f"Incomplete fixture run in {path}")
        builds.add(report["build_receipt_sha256"])
        for lap in report["runs"]:
            result = lap["result"]
            if (result["abi"] != lap["abi"] or result["sample_columns"] != COLUMNS
                    or (result["pages"], result["passes"], result["touch_digest"]) != (1024, 60, 1_873_920)
                    or len(result["resident"]) != len(result["discarded"]) or not result["resident"]):
                raise ValueError(f"Invalid page-touch result in {path}")
            page_bytes = result["page_bytes"]
            if page_bytes <= 0 or page_bytes & (page_bytes - 1):
                raise ValueError(f"Invalid native page size in {path}")
            page_sizes.add(page_bytes)
            for mode in ("resident", "discarded"):
                for index, (wall, cpu, minor, major) in enumerate(result[mode], 1):
                    if min(wall, cpu) <= 0 or min(minor, major) < 0:
                        raise ValueError(f"Missing timing or fault counters in {path}")
                    rows.append((android, lap["lap"], lap["abi"], page_bytes, mode, index, wall, cpu, minor, major))
    if len(builds) != 1:
        raise ValueError("Compare guests with the same fixture build")
    if len(page_sizes) != 1:
        raise ValueError("Compare the same number of bytes using matching native page sizes")
    with destination.open("x", encoding="utf-8", newline="") as output:
        writer = csv.writer(output, lineterminator="\n")
        writer.writerow(("android", "process_order", "abi", "page_bytes", "mode", "trial", *COLUMNS))
        writer.writerows(rows)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reports", nargs="+", type=Path)
    parser.add_argument("--csv", required=True, type=Path)
    args = parser.parse_args()
    export(args.reports, args.csv)
