"""Keep exact stock compile and link recipes for both runtime ABIs."""
import argparse
import re
import shlex
import subprocess
from pathlib import Path


BASE = "out/soong/.intermediates/frameworks/base/core/jni/libandroid_runtime/"
VARIANT = re.compile(r"android_(x86_64|x86)(?:_[a-z0-9_]+)?_core_shared")


def ninja_targets(ninja):
    with subprocess.Popen([*ninja, "targets", "all"], stdout=subprocess.PIPE, text=True) as proc:
        records = [line.rstrip().rsplit(": ", 1) for line in proc.stdout if line.startswith(BASE)]
        if proc.wait():
            raise RuntimeError("Ninja failed to enumerate the stock graph")
    return records


def target_candidates(records, arch, suffix, rule):
    candidates = []
    for path, edge in records:
        variant = VARIANT.fullmatch(path[len(BASE):].split("/", 1)[0])
        if variant and variant[1] == arch and path.endswith(suffix) and edge == rule:
            candidates.append(path)
    return candidates


def single_target(records, arch, suffix, rule):
    matches = target_candidates(records, arch, suffix, rule)
    if len(matches) != 1:
        raise ValueError(f"Expected exactly one {arch} {rule} {suffix} target, found {matches}")
    return matches[0]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("report", type=Path)
    parser.add_argument("--partial", action="store_true", help="Allow response files missing after a failed build")
    args = parser.parse_args()
    graphs = sorted(Path("out").glob("combined*.ninja"))
    if args.partial and not graphs:
        print("No combined Ninja graph was generated")
        return
    if len(graphs) != 1:
        raise ValueError(f"Expected one combined product Ninja graph, found {graphs}")
    ninja = ["/usr/bin/ninja", "-f", str(graphs[0]), "-w", "dupbuild=err", "-t"]
    records = ninja_targets(ninja)

    def run_ninja(*tool_args):
        return subprocess.check_output([*ninja, *tool_args], text=True)

    for abi, arch in (("lib", "x86"), ("lib64", "x86_64")):
        out = args.report / abi
        out.mkdir(exist_ok=True)
        compilation = sorted(target_candidates(records, arch, ".o", "g.cc.cc"))
        if not compilation:
            raise ValueError(f"No stock C++ compile targets for {abi}")
        (out / "android-runtime-compile-all.cmd.txt").write_text(run_ninja("commands", "-s", *compilation))
        for name, suffix, rule in (("android-runtime-compile", "/AndroidRuntime.o", "g.cc.cc"),
                                   ("android-runtime-link", "/libandroid_runtime.so", "g.cc.ld")):
            target = single_target(records, arch, suffix, rule)
            command = run_ninja("commands", "-s", target)
            (out / (name + ".cmd.txt")).write_text(command)
            (out / (name + ".inputs.txt")).write_text(run_ninja("query", target))
            if name == "android-runtime-link":
                for index, token in enumerate(shlex.split(command)):
                    match = re.fullmatch(r"(?:-Wl,)?@(.+)", token)
                    if match:
                        rsp = Path(match[1])
                        if not rsp.is_file():
                            if args.partial:
                                print(f"Missing response file after build failure: {rsp}")
                                continue
                            raise FileNotFoundError(f"Missing retained link response file {rsp}")
                        (out / f"android-runtime-link-{index}.rsp.txt").write_bytes(rsp.read_bytes())


if __name__ == "__main__":
    main()
