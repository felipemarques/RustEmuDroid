"""Build or stage a source-checked Windows WHPX clock lab SDK without booting Android."""
import argparse
from hashlib import file_digest, sha256
import json
import mmap
import os
from pathlib import Path
import re
import shutil
import stat
import struct
import subprocess
import sys
from tempfile import NamedTemporaryFile, TemporaryDirectory
from xml.etree import ElementTree

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parents[1]
MANIFEST = HERE / "overlay-manifest.json"
CORE = "qemu/windows-x86_64/qemu-system-x86_64.exe"
WORKTREE = "external/qemu"
PATCHED = ("cpus.c", "include/sysemu/whpx.h", "target/i386/whpx-all.c")
CACHE_SOURCE = "android/android-emu/android/opengles.cpp"
CACHE_OBJECT = "android/android-emu/CMakeFiles/android-emu.dir/android/opengles.cpp.obj"
VARIANTS = ("clock", "cache")
COMPILE_LOGS = ("configure.log", "build.log", "CMakeCache.txt", ".ninja_log")
COMPONENTS = ("emulator", "platform-tools")
FORBIDDEN = {".android", "avd", "system-images", "licenses", "adbkey", "adbkey.pub", "accounts.db", "credentials"}
TSC_GATE = "RUSTEMUDROID_QA_WHPX_SYNC_TSC"
CACHE_GATE = "RUSTEMUDROID_QA_VULKAN_CACHED_HOST"


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(path):
    with path.open("rb") as file:
        return file_digest(file, "sha256").hexdigest()


def lf_digest(path):
    return sha256(path.read_bytes().replace(b"\r\n", b"\n")).hexdigest()


def read(path):
    return json.loads(path.read_bytes())


def write(path, data):
    with NamedTemporaryFile(dir=path.parent, prefix=".whpx-receipt-", delete=False) as file:
        temp = Path(file.name)
        file.write((json.dumps(data, indent=2) + "\n").encode("utf-8"))
    try:
        temp.replace(path)
    finally:
        temp.unlink(missing_ok=True)


def checked(path, sha, size=None):
    require(path.is_file() and (size is None or path.stat().st_size == size) and digest(path) == sha,
            f"Artifact digest or length mismatch: {path}")
    return path


def inside(root, relative):
    path = root / relative
    require(path.resolve(strict=True) == path and path.is_file(), f"Expected a regular file in {root}: {relative}")
    return path


def manifest():
    data = read(MANIFEST)
    paths = [d for row in data["overlay"] for d in row["destinations"]]
    require(data["version"] == 1 and data["target"] == "windows-x86_64" and len(data["overlay"]) == 13 and
            len(paths) == len(set(paths)) == 21 and CORE in paths, "Unexpected overlay manifest")
    require(set(data["cache_variant"]["reference_overrides"]) == {CORE, "lib64/libemugl_common.dll"},
            "Unexpected archived cache variant differences")
    for name in (*paths, *data["stock_checks"], *(item["source"] for item in data["overlay"])):
        require(re.fullmatch(r"[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*", name) and
                not {".", ".."}.intersection(name.split("/")), f"Unexpected manifest path {name}")
    return data


def git(root, *args, env=None):
    proc = subprocess.run(["git", "-c", "core.autocrlf=false", "-c", "core.longpaths=true",
                           "-C", str(root), *args], env=env, capture_output=True, check=False)
    require(proc.returncode == 0, proc.stderr.decode(errors="replace").strip() or f"Git failed in {root}")
    return proc.stdout.decode("utf-8").strip()


def source_inputs(data, variant="clock"):
    basis = data["basis"]
    lock = PROJECT / basis["public_manifest_path"]
    require(lf_digest(lock) == basis["public_manifest_sha256"], "The published AOSP lock contents changed")
    patch = checked(PROJECT / basis["public_patch_path"], basis["public_patch_sha256"])
    projects = {entry.get("path", entry.attrib["name"]): entry.attrib["revision"]
                for entry in ElementTree.parse(lock).getroot().findall("project")}
    require(len(projects) == basis["project_count"] and projects[WORKTREE] == basis["qemu_source_commit"],
            "Unexpected pinned source projects")
    patches = (patch,)
    if variant == "cache":
        cached = data["cache_variant"]
        patches += (checked(PROJECT / cached["patch_path"], cached["patch_sha256"]),)
    return projects, patches


def expected_patch_tree(root, patches, changed):
    with TemporaryDirectory(prefix="whpx-index-") as tmp:
        env = dict(os.environ, GIT_INDEX_FILE=str(Path(tmp) / "index"), GIT_OPTIONAL_LOCKS="0")
        git(root, "read-tree", "HEAD", env=env)
        for patch in patches:
            git(root, "apply", "--cached", "--whitespace=error-all", str(patch), env=env)
        return {path: git(root, "rev-parse", f":{path}", env=env) for path in changed}


def source_check(source, data, variant="clock", *, prepare=False):
    projects, patches = source_inputs(data, variant)
    changed_files = PATCHED + ((CACHE_SOURCE,) if variant == "cache" else ())
    qemu = source / WORKTREE
    env = dict(os.environ, GIT_OPTIONAL_LOCKS="0")
    for name, revision in projects.items():
        root = source / name
        require(root.resolve(strict=True) == root and Path(git(root, "rev-parse", "--show-toplevel", env=env)).resolve() == root,
                f"Unexpected project worktree: {name}")
        require(git(root, "rev-parse", "HEAD", env=env) == revision, f"Wrong source revision: {name}")
        if name != WORKTREE:
            require(not git(root, "status", "--porcelain", "--untracked-files=no", env=env),
                    f"Modified pinned source: {name}")
    changed = git(qemu, "diff", "--name-only", "HEAD", "--", env=env).splitlines()
    if prepare and not changed:
        patch_args = tuple(map(str, patches))
        git(qemu, "apply", "--check", "--whitespace=error-all", *patch_args)
        git(qemu, "apply", "--whitespace=error-all", *patch_args)
        changed = git(qemu, "diff", "--name-only", "HEAD", "--", env=env).splitlines()
    require(sorted(changed) == sorted(changed_files), f"QEMU must contain exactly the published {variant} patches")
    require(not git(qemu, "ls-files", "--others", "--exclude-standard", env=env), "Unexpected untracked QEMU sources")
    expected = expected_patch_tree(qemu, patches, changed_files)
    require(all(git(qemu, "hash-object", f"--path={path}", path, env=env) == blob for path, blob in expected.items()),
            "QEMU contents differ from the published patch")
    tree = git(qemu, "rev-parse", "HEAD^{tree}", env=env)
    return {"projects": projects, "qemu_base_tree": tree, "patched_blobs": expected}


def inspect_pe(path, *, core=False, variant="clock"):
    with path.open("rb") as stream:
        header = stream.read(64)
        require(len(header) == 64 and header[:2] == b"MZ", f"Expected PE image: {path}")
        stream.seek(struct.unpack_from("<I", header, 0x3C)[0])
        require(stream.read(6) == b"PE\0\0\x64\x86", f"Expected AMD64 PE image: {path}")
        if core:
            with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as image:
                tsc = image.find(TSC_GATE.encode()) >= 0 and image.find(b"WHPX TSC QA: synchronized") >= 0
                cache_gate = image.find(CACHE_GATE.encode()) >= 0
                cache_message = image.find(b"RustEmuDroid QA: requested cached Vulkan host allocations") >= 0
                require(tsc and cache_gate == cache_message == (variant == "cache"),
                        f"The core does not contain the exact {variant} experiment markers")


def open_log(log):
    raw = log.read_bytes()
    return raw.decode("utf-16" if raw.startswith((b"\xFF\xFE", b"\xFE\xFF")) else "utf-8", errors="replace")


def check_logs(build, source, data, variant="clock"):
    counts = {name: digest(inside(build, name)) for name in COMPILE_LOGS}
    config = open_log(build / "configure.log")
    log = open_log(build / "build.log").replace("\\", "/")
    require("clang-r596125" in config and "Clang 22.0.2" in config and "Generating done" in config,
            "Configure did not attest the pinned host compiler and generator")
    require(f"Linking CXX executable {CORE}" in log and "target/i386/whpx-all.c.obj" in log and
            "qemu-system-x86_64.dir/cpus.c.obj" in log, "Build log lacks the real WHPX objects or linked core")
    ninja = open_log(build / ".ninja_log").replace("\\", "/").splitlines()
    require(ninja and ninja[0].startswith("# ninja log v"), "Expected the actual Ninja result log")
    targets = {cells[3] for line in ninja[1:] if len(cells := line.split("\t")) == 5}
    linked = {item["source"] for item in data["overlay"] if item["origin"] != "microsoft_dia"}
    objects = {f"CMakeFiles/qemu-system-x86_64.dir/{file}.obj" for file in PATCHED if file.endswith(".c")}
    if variant == "cache":
        require(CACHE_OBJECT in log, "Build log lacks the compiled Vulkan-option bridge")
        objects.add(CACHE_OBJECT)
    require((linked | objects).issubset(targets), "Ninja did not emit each expected object and module")
    cache = dict((line.split(":", 1)[0], line.split("=", 1)[1]) for line in open_log(build / "CMakeCache.txt").splitlines()
                 if re.match(r"^[A-Za-z_][^:= ]*:[^=]+=", line))
    expect = {"CMAKE_GENERATOR": "Ninja", "CMAKE_BUILD_TYPE": "Release", "BUILD_TESTING": "OFF",
              "OPTION_MINBUILD": "ON", "OPTION_SDK_TOOLS_BUILD_NUMBER": data["basis"]["candidate_build_id"],
              "OPTION_SDK_TOOLS_REVISION": data["basis"]["android_emulator_revision"]}
    require(all(cache.get(key) == val for key, val in expect.items()) and
            Path(cache["CMAKE_HOME_DIRECTORY"]).resolve() == source / WORKTREE, "CMake recipe or source path differs")
    return counts


def linked_inventory(build, data, variant="clock"):
    files = {}
    for item in data["overlay"]:
        if item["origin"] == "microsoft_dia":
            continue
        path = inside(build, item["source"])
        inspect_pe(path, core=item["source"] == CORE, variant=variant)
        files[item["source"]] = {"sha256": digest(path), "bytes": path.stat().st_size}
    return files


def build_dia(build):
    copied = build / "msdia140.dll"
    if not copied.is_file():
        return None
    cache = open_log(build / "CMakeCache.txt")
    match = re.search(r"(?m)^MSDIA140_DLL_LOCATION_LOCAL:FILEPATH=(.+)$", cache)
    require(match, "DIA copy has no Visual Studio origin in CMakeCache.txt")
    local = Path(match[1].strip()).resolve(strict=True)
    require([part.lower() for part in local.parts[-4:]] == ["dia sdk", "bin", "amd64", "msdia140.dll"],
            "DIA copy did not originate from an x64 Visual Studio DIA SDK")
    inspect_pe(copied)
    verified = {"sha256": digest(local), "bytes": local.stat().st_size}
    checked(copied, verified["sha256"], verified["bytes"])
    return verified


def invoke_logged(command, path, source, *, append=False):
    print(f"Writing {path}", flush=True)
    with path.open("ab" if append else "wb") as out:
        status = subprocess.run(command, cwd=source, stdout=out, stderr=subprocess.STDOUT).returncode
    require(status == 0, f"Build command exited {status}. See {path}")


def new_out(path, *inputs):
    path = path.parent.resolve(strict=True) / path.name
    require(all(not path.is_relative_to(root) and not root.is_relative_to(path) for root in inputs),
            "Output overlaps an input directory")
    require(not path.exists() and not path.is_symlink(), "Output must be a fresh path")
    return path


def build_local(source, output, data, resume, variant):
    require(os.environ.get("VSCMD_ARG_TGT_ARCH") == "x64" and os.environ.get("VCToolsInstallDir"),
            "Run this command from a VS 2022 x64 developer terminal")
    source = source.resolve(strict=True)
    if resume:
        output = output.resolve(strict=True)
        require(not output.is_relative_to(source) and not source.is_relative_to(output) and
                not (output / "build-receipt.json").exists(), "Resume requires the same incomplete build directory")
    else:
        output = new_out(output, source)
    config = inside(source, f"{WORKTREE}/android/build/python/cmake.py")
    cmake = inside(source, "prebuilts/cmake/windows-x86/bin/cmake.exe")
    proof = source_check(source, data, variant, prepare=not resume)
    intent = {"source": str(source), "variant": variant, "overlay_manifest_sha256": lf_digest(MANIFEST),
              "verified_source": proof}
    if resume:
        require(read(inside(output, "build-intent.json")) == intent, "The source tree or recipe changed since build start")
    else:
        output.mkdir(exist_ok=False)
        write(output / "build-intent.json", intent)
    if not (output / "configure-ok.json").exists():
        invoke_logged([sys.executable, "-B", str(config), "--task", "Configure", "--target", "windows",
                       "--out", str(output), "--ccache", "none", "--crash", "none", "--sdk_build_number",
                       data["basis"]["candidate_build_id"], "--cmake_option", "OPTION_MINBUILD=ON",
                       "--cmake_option", "BUILD_TESTING=OFF"], output / "configure.log", source)
        write(output / "configure-ok.json", {"exit_code": 0})
    invoke_logged([str(cmake), "--build", str(output), "--parallel", "2", "--target", "qemu-system-x86_64"],
                  output / "build.log", source, append=True)
    require(source_check(source, data, variant) == proof, "Source changed during the build")
    result = {**intent, "version": 1, "logs": check_logs(output, source, data, variant),
              "files": linked_inventory(output, data, variant), "optional_dia": build_dia(output)}
    write(output / "build-receipt.json", result)
    print(output / "build-receipt.json")


def validate_build(artifacts, data, requested):
    receipt = read(inside(artifacts, "build-receipt.json"))
    require(receipt["version"] == 1 and receipt["overlay_manifest_sha256"] == lf_digest(MANIFEST),
            "Expected a local receipt produced by the current build command")
    variant = receipt["variant"]
    require(variant in VARIANTS and requested in (None, variant), "Build receipt has a different variant")
    source = Path(receipt["source"]).resolve(strict=True)
    require(receipt["verified_source"] == source_check(source, data, variant) and
            receipt["logs"] == check_logs(artifacts, source, data, variant) and
            receipt["files"] == linked_inventory(artifacts, data, variant), "Build evidence or built modules changed")
    return receipt["files"], digest(artifacts / "build-receipt.json"), receipt.get("optional_dia"), variant


def stock_inputs(stock, data):
    revision = re.search(r"(?m)^Pkg\.Revision\s*=\s*(\S+)\s*$",
                         inside(stock, "emulator/source.properties").read_text(encoding="utf-8"))
    require(revision and revision.group(1) == data["basis"]["android_emulator_revision"], "Wrong stock Emulator revision")
    for name, sha in data["stock_checks"].items():
        checked(inside(stock, name), sha)


def overlay_plan(artifacts, dia, data, *, strict=False, requested=None):
    if strict:
        expected, proof_sha, local_dia, variant = {}, None, None, requested or "clock"
    else:
        expected, proof_sha, local_dia, variant = validate_build(artifacts, data, requested)
    overrides = data["cache_variant"]["reference_overrides"] if strict and variant == "cache" else {}
    plan = []
    for row in data["overlay"]:
        name = row["source"]
        item = row | overrides.get(name, {})
        if item["origin"] == "microsoft_dia":
            if dia is None:
                continue
            require(dia.name.lower() == "msdia140.dll", "Supply the local DIA x64 file")
            inspect_pe(dia)
            content = item if strict else local_dia
            require(content, "The local build receipt has no matching DIA input")
            if not strict:
                checked(dia, content["sha256"], content["bytes"])
            path = dia
        else:
            path = inside(artifacts, name)
            content = item if strict else expected[name]
            if strict:
                inspect_pe(path, core=name == CORE, variant=variant)
        if strict:
            content = item
            checked(path, item["sha256"], item["bytes"])
        plan.append((path, item["destinations"], content["sha256"], content["bytes"]))
    return plan, proof_sha, variant


def clean_component(stock, name):
    folder = stock / name
    require(folder.resolve(strict=True) == folder and folder.is_dir(), f"Expected a local SDK {name} component")
    for here, dirs, files in os.walk(folder, followlinks=False):
        for name in dirs + files:
            path = Path(here) / name
            mode = path.lstat()
            require(name.lower() not in FORBIDDEN and not path.is_symlink() and
                    not (getattr(mode, "st_file_attributes", 0) & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0)),
                    f"Unexpected state, credential name or linked item in SDK tools: {path}")
    return folder


def loader_smoke(sdk, data):
    emulator = sdk / "emulator"
    windows = Path(os.environ["SystemRoot"])
    unsafe = {TSC_GATE, CACHE_GATE, "ANDROID_EMULATOR_FEATURES", "ANDROID_SDK_HOME", "ANDROID_PREFS_ROOT",
              "ANDROID_AVD_HOME", "ANDROID_EMULATOR_HOME"}
    env = {key: val for key, val in os.environ.items() if key.upper() not in unsafe}
    env.update(ANDROID_HOME=str(sdk), ANDROID_SDK_ROOT=str(sdk), ANDROID_USER_HOME=str(sdk.parent / "qa-home"))
    env["PATH"] = os.pathsep.join(map(str, (emulator, emulator / "lib64", emulator / "lib64/qt/lib",
                                           emulator / "lib64/gles_angle", windows / "System32", windows)))
    passed = []
    for name, flag in (("emulator.exe", "-version"), ("emulator.exe", "-accel-check"), (CORE, "-version")):
        result = subprocess.run([emulator / name, flag], cwd=emulator, env=env, stdin=subprocess.DEVNULL,
                                capture_output=True, text=True, errors="replace", timeout=25,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        text = result.stdout + result.stderr
        require(result.returncode == 0 and
                (("WHPX" in text and "usable" in text) if flag == "-accel-check" else data["basis"]["candidate_build_id"] in text),
                f"Loader check {name} {flag} failed with {result.returncode}: {text}")
        passed.append({"relative_path": name, "flag": flag, "code": result.returncode})
    return passed


def stage(out, stock, plan, receipt, data, smoke):
    components = [clean_component(stock, name) for name in COMPONENTS]
    with TemporaryDirectory(dir=out.parent, prefix=".whpx-stage-") as temporary:
        temp = Path(temporary)
        require(temp.resolve().parent == out.parent, "Unexpected temporary workspace")
        owned = temp / "owned"
        sdk = owned / "sdk"
        sdk.mkdir(parents=True)
        for component in components:
            shutil.copytree(component, sdk / component.name)
        replaced = {}
        for path, destinations, sha, size in plan:
            for name in destinations:
                dest = sdk / "emulator" / name
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, dest)
                checked(dest, sha, size)
                replaced[name] = sha
        for relative, sha in data["stock_checks"].items():
            if relative != "emulator/" + CORE:
                checked(sdk / relative, sha)
        receipt.update(sdk=str(out / "sdk"), qemu_sha256=replaced[CORE],
                       emulator_sha256=data["stock_checks"]["emulator/emulator.exe"], replaced=replaced)
        require(not out.exists(), "Output appeared before publication")
        owned.rename(out)
    # Run only after the rename. Windows may hold handles in the SDK after emulator.exe exits.
    try:
        receipt["nonboot_tests"] = loader_smoke(out / "sdk", data) if smoke else []
        write(out / "staging.json", receipt)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        write(out / "staging-error.json", {"error": str(error), "android_started": False})
        raise
    return receipt


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    commands = cli.add_subparsers(dest="action", required=True)
    compile_cli = commands.add_parser("build", help="Build and attest a pinned local AOSP checkout")
    compile_cli.add_argument("--source", type=Path, required=True)
    compile_cli.add_argument("--out", type=Path, required=True)
    compile_cli.add_argument("--resume", action="store_true")
    compile_cli.add_argument("--variant", choices=VARIANTS, default="clock")
    for action in ("check", "stage"):
        cmd = commands.add_parser(action)
        cmd.add_argument("--stock-sdk", type=Path, required=True)
        cmd.add_argument("--artifacts", type=Path, required=True)
        cmd.add_argument("--variant", choices=VARIANTS, help="Expected variant; default reads the local build receipt")
        cmd.add_argument("--strict-reference", action="store_true", help="Require the published archived PE checksums")
        cmd.add_argument("--dia", type=Path, help="Optional separately obtained Microsoft DIA runtime DLL")
        if action == "stage":
            cmd.add_argument("--out", type=Path, required=True)
            cmd.add_argument("--smoke", action="store_true", help="Check -version and WHPX without a guest")
    args = cli.parse_args()
    data = manifest()
    try:
        require(os.name == "nt", "This lab targets Windows x64")
        if args.action == "build":
            build_local(args.source, args.out, data, args.resume, args.variant)
            return
        stock = args.stock_sdk.resolve(strict=True)
        artifacts = args.artifacts.resolve(strict=True)
        dia = args.dia.resolve(strict=True) if args.dia else None
        stock_inputs(stock, data)
        plan, proof, variant = overlay_plan(artifacts, dia, data, strict=args.strict_reference, requested=args.variant)
        receipt = {"variant": variant, "reference_artifact_hashes_required": args.strict_reference, "build_receipt_sha256": proof,
                   "overlay_manifest_sha256": lf_digest(MANIFEST), "copied_optional_dia": dia is not None,
                   "android_started": False, "overlay_path_count": sum(len(paths) for _, paths, _, _ in plan)}
        if args.action == "stage":
            out = new_out(args.out, stock, artifacts, *([dia] if dia else []))
            stage(out, stock, plan, receipt, data, args.smoke)
        print(json.dumps(receipt, indent=2))
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        cli.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
