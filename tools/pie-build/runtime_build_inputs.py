"""Keep exact stock compile and link recipes for both runtime ABIs."""
import argparse
import re
import shlex
import subprocess
from pathlib import Path


def run_ninja(*args):
    return subprocess.check_output(["/usr/bin/ninja", "-f", "out/soong/build.ninja", "-t", *args], text=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("report", type=Path)
    args = parser.parse_args()
    base = Path("out/soong/.intermediates/frameworks/base/core/jni/libandroid_runtime")
    with subprocess.Popen(["/usr/bin/ninja", "-f", "out/soong/build.ninja", "-t", "targets", "all"],
                          stdout=subprocess.PIPE, text=True) as proc:
        records = [line.rstrip().rsplit(": ", 1) for line in proc.stdout if line.startswith(str(base) + "/")]
        if proc.wait():
            raise RuntimeError("Ninja failed to enumerate the stock graph")
    for abi, variant in (("lib", "android_x86_x86_64_core_shared"), ("lib64", "android_x86_64_core_shared")):
        prefix = str(base / variant) + "/"
        out = args.report / abi
        out.mkdir(exist_ok=True)
        for name, suffix, rule in (("android-runtime-compile", "/AndroidRuntime.o", "g.cc.cc"),
                                   ("android-runtime-link", "/libandroid_runtime.so", "g.cc.ld")):
            target, = [path for path, edge in records if path.startswith(prefix) and path.endswith(suffix) and edge == rule]
            command = run_ninja("commands", "-s", target)
            (out / (name + ".cmd.txt")).write_text(command)
            (out / (name + ".inputs.txt")).write_text(run_ninja("query", target))
            if name == "android-runtime-link":
                for index, token in enumerate(shlex.split(command)):
                    match = re.fullmatch(r"(?:-Wl,)?@(.+)", token)
                    if match:
                        rsp = Path(match[1])
                        if not rsp.is_file():
                            raise FileNotFoundError(f"Missing retained link response file {rsp}")
                        (out / f"android-runtime-link-{index}.rsp.txt").write_bytes(rsp.read_bytes())


if __name__ == "__main__":
    main()
