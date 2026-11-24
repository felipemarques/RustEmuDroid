"""Capture the official AOSP link recipes and the objects they actually consume."""
import argparse
from pathlib import Path
import shlex
import subprocess
import tarfile
import json
import hashlib


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    options = parser.parse_args()
    root = Path.cwd()
    report = options.report.resolve(strict=True)
    paths = set()
    architectures = {}

    def relative(value, needed=True):
        path = Path(value)
        if path.is_absolute():
            path = path.relative_to(root)
        if ".." in path.parts or not (root / path).resolve().is_relative_to(root):
            raise ValueError(f"Path outside the stock checkout: {value}")
        if needed and not (root / path).is_file():
            raise FileNotFoundError(f"Missing stock link input: {path}")
        return path

    def collect(tokens, output):
        # File-valued linker switches also occur as -Wl,<switch>,<value>.
        for token in tokens:
            for value in token.removeprefix("-Wl,").split(","):
                value = value.rsplit("=", 1)[-1].removeprefix("@")
                if value.startswith("-") or value == "/dev/null":
                    continue
                if "/" in value and value.endswith((".o", ".a", ".so", ".rsp", ".lds", ".ld",
                                                    ".map", ".ver", ".txt", ".script", ".sym", ".symbols")):
                    path = relative(value)
                    if path != output:
                        paths.add(path)
                elif "/" in value and (root / value).is_file():
                    path = relative(value)
                    if path != output:
                        paths.add(path)

    for install_dir in ("lib", "lib64"):
        metadata = report / install_dir
        driver = shlex.split((metadata / "android-runtime-link.cmd.txt").read_text())
        compiler = relative(driver[0])
        output = relative(driver[driver.index("-o") + 1])
        response_files = {}
        for index, token in enumerate(driver):
            if token.startswith(("@", "-Wl,@")):
                rsp = relative(token.removeprefix("-Wl,").removeprefix("@"))
                recorded = metadata / f"android-runtime-link-{index}.rsp.txt"
                if recorded.read_bytes() != (root / rsp).read_bytes():
                    raise ValueError(f"Retained response file disagrees with its recorded copy: {rsp}")
                response_files[rsp.as_posix()] = shlex.split(recorded.read_text())
        if not response_files:
            raise ValueError(f"No link response file found for {install_dir}")
        flat_response = [token for tokens in response_files.values() for token in tokens]
        android_runtime, = [relative(token) for token in flat_response
                            if token.endswith("/AndroidRuntime.o")]
        dry_run = subprocess.run([*driver, "-###"], check=True, text=True, capture_output=True)
        actual, = [shlex.split(line) for line in dry_run.stderr.splitlines()
                   if line.lstrip().startswith('"') and output.as_posix() in line]
        linker = relative(actual[0])
        collect(driver[1:], output)
        collect(flat_response, output)
        collect(actual[1:], output)
        paths.add(output)
        architectures[install_dir] = {
            "compiler": {"path": compiler.as_posix(), "sha256": sha256(root / compiler),
                         "version_and_target": dry_run.stderr.splitlines()[:3]},
            "linker": {"path": linker.as_posix(), "sha256": sha256(root / linker),
                       "version": subprocess.check_output([actual[0], "--version"], text=True).splitlines()[0]},
            "driver_argv": driver,
            "linker_argv": actual,
            "responses": response_files,
            "android_runtime_object": android_runtime.as_posix(),
            "unstripped_output": output.as_posix(),
        }

    manifest = {"architectures": architectures, "files": {}}
    archive = report / "stock-relink.tar"
    partial = archive.with_suffix(".tar.tmp")
    try:
        with tarfile.open(partial, "w", dereference=True) as bundle:
            for path in sorted(paths):
                manifest["files"][path.as_posix()] = {
                    "sha256": sha256(root / path), "bytes": (root / path).stat().st_size,
                }
                bundle.add(root / path, arcname=path.as_posix(), recursive=False)
        partial.replace(archive)
    finally:
        partial.unlink(missing_ok=True)
    manifest["archive_sha256"] = sha256(archive)
    (report / "stock-relink.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Captured {len(paths)} stock inputs plus pre-strip outputs in {archive}")


if __name__ == "__main__":
    main()

