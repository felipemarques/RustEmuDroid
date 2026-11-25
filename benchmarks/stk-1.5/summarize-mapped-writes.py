"""Validate the complete output of mapped-write.c and summarize CPU and elapsed time."""

import argparse
import hashlib
import json
from pathlib import Path
from statistics import median


def summarize(raw):
    rows = [r.split("\t") for r in raw.decode().splitlines()]
    if not rows or len(rows[0]) != 10 or rows[0][0] != "meta" or rows[-1] != ["complete"]:
        raise ValueError("Missing the device header or completion marker")
    renderer = rows[0][1]
    vendor, family, size, passes, rounds, mask, allocation_size, alignment = map(int, rows[0][2:])
    if not renderer or (size, passes, rounds) != (65536, 64, 6) or allocation_size < size or alignment <= 0:
        raise ValueError("Unexpected workload or memory requirements")
    types = {-1: {"kind": "normal allocation"}}
    samples = {}
    for fields in rows[1:-1]:
        if fields[0] == "type" and len(fields) == 4:
            index, heap, flags = map(int, fields[1:])
            if samples or index in types or not 0 <= index < 32 or not mask & (1 << index) or not flags & 2 or heap < 0:
                raise ValueError("Unexpected host-visible memory type")
            types[index] = {"kind": "Vulkan", "heap": heap, "property_flags": flags}
        elif fields[0] == "sample" and len(fields) == 6:
            _, r, t, method, elapsed, cpu = fields
            number, index, wall_ns, cpu_ns = map(int, (r, t, elapsed, cpu))
            key = (index, method, number)
            if (key in samples or index not in types or method not in ("memcpy", "scalar32") or
                    not 0 <= number < rounds or min(wall_ns, cpu_ns) <= 0):
                raise ValueError("Duplicate or invalid timing sample")
            samples[key] = (wall_ns, cpu_ns)
        else:
            raise ValueError("Unexpected row in probe output")
    if len(types) < 2 or len(samples) != len(types) * 2 * rounds:
        raise ValueError("Incomplete sample matrix")
    targets = []
    for index, properties in types.items():
        target = {"memory_type": index, **properties, "methods": {}}
        for method in ("memcpy", "scalar32"):
            times = [samples[index, method, i] for i in range(rounds)]
            elapsed, cpu = zip(*times, strict=True)
            target["methods"][method] = {
                "elapsed_ns": elapsed, "thread_cpu_ns": cpu,
                "median_elapsed_ms": median(elapsed) / 1e6, "median_thread_cpu_ms": median(cpu) / 1e6,
            }
        targets.append(target)
    return {"source_sha256": hashlib.sha256(raw).hexdigest(), "guest_renderer": renderer,
            "vendor_id": vendor, "graphics_queue_family": family, "buffer_bytes": size,
            "passes_per_batch": passes, "batches_per_method_target": rounds,
            "storage_transfer_src_memory_type_bits": mask, "required_size": allocation_size,
            "required_alignment": alignment, "targets": targets}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tsv", type=Path, help="verbatim stdout from one successful probe run")
    args = parser.parse_args()
    print(json.dumps(summarize(args.tsv.read_bytes()), indent=2, allow_nan=False))
