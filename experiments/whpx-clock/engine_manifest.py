"""Write rustemudroid-engine.json for a staged performance engine after checking each capability in its binaries."""
import argparse
from hashlib import file_digest
import json
import mmap
import os
from pathlib import Path, PurePosixPath
import re
from tempfile import NamedTemporaryFile

HERE = Path(__file__).resolve().parent
SPEC = HERE / "performance-engine.json"
MANIFEST_NAME = "rustemudroid-engine.json"
KIND = "rustemudroid-performance-engine"
MODULES = {
    "emulator": "emulator/emulator.exe",
    "qemu": "emulator/qemu/windows-x86_64/qemu-system-x86_64.exe",
    "gfxstream": "emulator/lib64/libgfxstream_backend.dll",
}


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(path):
    with path.open("rb") as file:
        return file_digest(file, "sha256").hexdigest()


def capability_markers(spec=None):
    """{capability: {module: [marker, ...]}} from the published engine description."""
    spec = spec or json.loads(SPEC.read_bytes())
    table = spec["capabilities"]
    require(table and all(set(modules) <= {"qemu", "gfxstream"} and all(modules.values())
                          for modules in table.values()), "Unexpected capability table")
    return table


def engine_files(folder):
    folder = folder.resolve(strict=True)
    sdk = folder / "sdk"
    require(sdk.is_dir() and not sdk.is_symlink(), f"Expected the staged sdk folder in {folder}")
    files = {}
    for name, relative in MODULES.items():
        path = sdk / relative
        require(path.is_file(), f"Missing {relative} in {sdk}")
        files[name] = path
    return folder, sdk, files


def find_all(path, markers):
    with path.open("rb") as stream, mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as image:
        return [marker for marker in markers if image.find(marker.encode("ascii")) < 0]


def verify_capabilities(files, requested, table):
    """Return the requested capabilities in a stable order; raise if any lacks its markers."""
    unknown = sorted(set(requested) - set(table))
    require(not unknown, f"Unknown capability: {', '.join(unknown)}. Known: {', '.join(table)}")
    problems = []
    for name in requested:
        for module, markers in table[name].items():
            missing = find_all(files[module], markers)
            if missing:
                problems.append(f"{name}: {MODULES[module]} lacks {', '.join(repr(m) for m in missing)}")
    require(not problems, "No binary evidence for: " + "; ".join(problems))
    return list(dict.fromkeys(requested))


def evidence(files, table):
    return {name: not any(find_all(files[module], markers) for module, markers in modules.items())
            for name, modules in table.items()}


def engine_version(sdk):
    text = (sdk / "emulator/source.properties").read_text(encoding="utf-8")
    match = re.search(r"(?m)^Pkg\.Revision\s*=\s*(\S+)\s*$", text)
    require(match, "emulator/source.properties has no Pkg.Revision")
    return match.group(1)


def extra_hashes(sdk, names):
    extra = {}
    main = set(MODULES.values())
    for name in names:
        path = PurePosixPath(name)
        require("\\" not in name and ":" not in name and not path.is_absolute() and path.parts[:1] == ("emulator",) and
                len(path.parts) > 1 and not {".", ".."}.intersection(path.parts),
                f"Extra files must be relative paths under emulator/ with '/' separators: {name}")
        require(name not in main and name not in extra, f"Duplicate or main module in --extra: {name}")
        file = sdk / Path(*path.parts)
        require(file.is_file() and not file.is_symlink(), f"Missing extra file: {name}")
        extra[name] = digest(file)
    return extra


def build_manifest(folder, capabilities, *, build_id=None, extra=(), spec=None):
    spec = spec or json.loads(SPEC.read_bytes())
    folder, sdk, files = engine_files(folder)
    table = capability_markers(spec)
    build_id = build_id or spec["build_id"]
    token = build_id.split()[0] if build_id.split() else ""
    require(token and not find_all(files["qemu"], [token]),
            f"The core does not identify itself as build {token or '(empty)'}")
    version = engine_version(sdk)
    require(version == spec["engine_version"], f"Expected Android Emulator {spec['engine_version']}, found {version}")
    manifest = {
        "schema": 1,
        "kind": KIND,
        "engine_version": version,
        "build_id": build_id,
        "emulator_sha256": digest(files["emulator"]),
        "qemu_sha256": digest(files["qemu"]),
        "gfxstream_sha256": digest(files["gfxstream"]),
    }
    hashes = extra_hashes(sdk, extra)
    if hashes:
        manifest["extra_sha256"] = hashes
    manifest["capabilities"] = verify_capabilities(files, capabilities, table)
    return folder, manifest


def write_manifest(folder, manifest, *, replace=False):
    target = folder / MANIFEST_NAME
    require(replace or not target.exists(), f"{target} already exists; pass --replace to rewrite it")
    with NamedTemporaryFile(dir=folder, prefix=".engine-manifest-", delete=False) as file:
        temp = Path(file.name)
        file.write((json.dumps(manifest, indent=2) + "\n").encode("utf-8"))
        file.flush()
        os.fsync(file.fileno())
    try:
        temp.replace(target)
    finally:
        temp.unlink(missing_ok=True)
    return target


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    commands = cli.add_subparsers(dest="action", required=True)
    show = commands.add_parser("evidence", help="Show which known capabilities the staged binaries support")
    show.add_argument("--engine", type=Path, required=True, help="Folder that contains the staged sdk")
    write = commands.add_parser("write", help="Write rustemudroid-engine.json next to sdk")
    write.add_argument("--engine", type=Path, required=True, help="Folder that contains the staged sdk")
    write.add_argument("--capability", action="append", required=True, help="Capability to list; repeat for each")
    write.add_argument("--build-id", help="Build label; its first word must occur in the core")
    write.add_argument("--extra", action="append", default=[], help="Additional file under emulator/ to hash")
    write.add_argument("--replace", action="store_true", help="Rewrite an existing manifest")
    args = cli.parse_args()
    try:
        if args.action == "evidence":
            _, _, files = engine_files(args.engine)
            print(json.dumps(evidence(files, capability_markers()), indent=2))
            return
        folder, manifest = build_manifest(args.engine, args.capability, build_id=args.build_id, extra=args.extra)
        print(write_manifest(folder, manifest, replace=args.replace))
        print(json.dumps(manifest, indent=2))
    except (OSError, ValueError, KeyError) as error:
        cli.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
