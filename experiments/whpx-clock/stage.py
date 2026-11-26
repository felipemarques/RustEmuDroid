"""Build or stage a source-checked Windows WHPX clock lab SDK without booting Android."""
import argparse
from contextlib import ExitStack
from hashlib import file_digest, sha256
import json
import mmap
import os
from pathlib import Path
import re
import secrets
import shutil
import stat
import struct
import subprocess
import sys
from tempfile import NamedTemporaryFile, TemporaryDirectory
from xml.etree import ElementTree
from zipfile import BadZipFile, ZipFile

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parents[1]
MANIFEST = HERE / "overlay-manifest.json"
PERFORMANCE = HERE / "performance-engine.json"
CORE = "qemu/windows-x86_64/qemu-system-x86_64.exe"
WORKTREE = "external/qemu"
GFXSTREAM = "hardware/google/gfxstream"
PATCHED = ("cpus.c", "include/sysemu/whpx.h", "target/i386/whpx-all.c")
CACHE_SOURCE = "android/android-emu/android/opengles.cpp"
CACHE_OBJECT = "android/android-emu/CMakeFiles/android-emu.dir/android/opengles.cpp.obj"
VARIANTS = ("clock", "cache", "performance")
COMPILE_LOGS = ("configure.log", "build.log", "CMakeCache.txt", ".ninja_log")
COMPONENTS = ("emulator", "platform-tools")
FORBIDDEN = {".android", "avd", "system-images", "licenses", "adbkey", "adbkey.pub", "accounts.db", "credentials"}
TSC_GATE = "RUSTEMUDROID_QA_WHPX_SYNC_TSC"
CACHE_GATE = "RUSTEMUDROID_QA_VULKAN_CACHED_HOST"
MEDIA_CACHE_ZIP_SHA256 = "a67d12ec5abbb220cce81ecf623174d278fab08dd5d0a08441be51c642395146"
MEDIA_CACHE_MODULES = {
    CORE: {"bytes": 66279424, "sha256": "b9feb63b82209b4b93ef39131687d25782bc291219bfc59b1a91909076a519c8"},
    "lib64/libemugl_common.dll": {"bytes": 147968, "sha256": "72c8a9c83f991d0d9b57fbaf2be35754418b119aabfa6aa2a97edeae50956565"},
}


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
    if variant in ("cache", "performance"):
        cached = data["cache_variant"]
        patches += (checked(PROJECT / cached["patch_path"], cached["patch_sha256"]),)
    if variant == "performance":
        spec = performance_spec()
        listed = spec["projects"][WORKTREE]["patches"]
        require([item["path"] for item in listed[:2]] == [basis["public_patch_path"], data["cache_variant"]["patch_path"]] and
                spec["projects"][WORKTREE]["commit"] == basis["qemu_source_commit"],
                "The performance engine must build on the published clock and cache patches")
        patches += tuple(checked(PROJECT / item["path"], item["sha256"]) for item in listed[2:])
    return projects, patches


def performance_spec():
    spec = read(PERFORMANCE)
    require(spec["version"] == 1 and spec["variant"] == "performance" and set(spec["projects"]) == {WORKTREE, GFXSTREAM},
            "Unexpected performance engine description")
    for project in spec["projects"].values():
        require(project["patches"] and project["patched_blobs"], "The performance engine lists no patch")
        for name in (*project["patched_blobs"], *(item["path"] for item in project["patches"])):
            require(re.fullmatch(r"[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*", name) and
                    not {".", ".."}.intersection(name.split("/")), f"Unexpected performance engine path {name}")
    return spec


def patch_sets(data, variant):
    """Return {project: (patches, changed files, pinned blobs or None)} for every patched project."""
    projects, patches = source_inputs(data, variant)
    if variant != "performance":
        return projects, {WORKTREE: (patches, PATCHED + ((CACHE_SOURCE,) if variant == "cache" else ()), None)}
    spec = performance_spec()
    sets = {WORKTREE: (patches, tuple(spec["projects"][WORKTREE]["patched_blobs"]), spec["projects"][WORKTREE]["patched_blobs"])}
    gfx = spec["projects"][GFXSTREAM]
    require(projects.get(GFXSTREAM) == gfx["commit"], "The performance engine expects the pinned gfxstream revision")
    sets[GFXSTREAM] = (tuple(checked(PROJECT / item["path"], item["sha256"]) for item in gfx["patches"]),
                       tuple(gfx["patched_blobs"]), gfx["patched_blobs"])
    return projects, sets


def expected_patch_tree(root, patches, changed):
    with TemporaryDirectory(prefix="whpx-index-") as tmp:
        env = dict(os.environ, GIT_INDEX_FILE=str(Path(tmp) / "index"), GIT_OPTIONAL_LOCKS="0")
        git(root, "read-tree", "HEAD", env=env)
        for patch in patches:
            git(root, "apply", "--cached", "--whitespace=error-all", str(patch), env=env)
        return {path: git(root, "rev-parse", f":{path}", env=env) for path in changed}


def source_check(source, data, variant="clock", *, prepare=False):
    projects, sets = patch_sets(data, variant)
    env = dict(os.environ, GIT_OPTIONAL_LOCKS="0")
    for name, revision in projects.items():
        root = source / name
        require(root.resolve(strict=True) == root and Path(git(root, "rev-parse", "--show-toplevel", env=env)).resolve() == root,
                f"Unexpected project worktree: {name}")
        require(git(root, "rev-parse", "HEAD", env=env) == revision, f"Wrong source revision: {name}")
        if name not in sets:
            require(not git(root, "status", "--porcelain", "--untracked-files=no", env=env),
                    f"Modified pinned source: {name}")
    proof = {"projects": projects}
    for name, (patches, changed_files, pinned) in sets.items():
        root = source / name
        label = "QEMU" if name == WORKTREE else name
        changed = git(root, "diff", "--name-only", "HEAD", "--", env=env).splitlines()
        if prepare and not changed:
            # Layered patches change the same files. A single git apply call with all of them can
            # reject a later hunk, so check them in order on a scratch index, then apply them in order.
            expected_patch_tree(root, patches, changed_files)
            for patch in patches:
                git(root, "apply", "--whitespace=error-all", str(patch))
            changed = git(root, "diff", "--name-only", "HEAD", "--", env=env).splitlines()
        require(sorted(changed) == sorted(changed_files), f"{label} must contain exactly the published {variant} patches")
        require(not git(root, "ls-files", "--others", "--exclude-standard", env=env), f"Unexpected untracked {label} sources")
        expected = expected_patch_tree(root, patches, changed_files)
        require(pinned is None or expected == pinned, f"The published {label} patches no longer produce the pinned sources")
        require(all(git(root, "hash-object", f"--path={path}", path, env=env) == blob for path, blob in expected.items()),
                f"{label} contents differ from the published patch")
        tree = git(root, "rev-parse", "HEAD^{tree}", env=env)
        if name == WORKTREE:
            proof |= {"qemu_base_tree": tree, "patched_blobs": expected}
        else:
            proof |= {"gfxstream_base_tree": tree, "gfxstream_patched_blobs": expected}
    return proof


def gate_variables(module=None):
    return [gate["variable"] for gate in performance_spec()["gates"] if module in (None, gate["module"])]


def inspect_pe(path, *, core=False, variant="clock", markers=()):
    with path.open("rb") as stream:
        header = stream.read(64)
        require(len(header) == 64 and header[:2] == b"MZ", f"Expected PE image: {path}")
        stream.seek(struct.unpack_from("<I", header, 0x3C)[0])
        require(stream.read(6) == b"PE\0\0\x64\x86", f"Expected AMD64 PE image: {path}")
        if core or markers:
            with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as image:
                if core:
                    tsc = image.find(TSC_GATE.encode()) >= 0 and image.find(b"WHPX TSC QA: synchronized") >= 0
                    cache_gate = image.find(CACHE_GATE.encode()) >= 0
                    cache_message = image.find(b"RustEmuDroid QA: requested cached Vulkan host allocations") >= 0
                    require(tsc and cache_gate == cache_message == (variant != "clock"),
                            f"The core does not contain the exact {variant} experiment markers")
                missing = [marker for marker in markers if image.find(marker.encode()) < 0]
                require(not missing, f"{path.name} lacks the {variant} markers: {', '.join(missing)}")


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
    if variant in ("cache", "performance"):
        require(CACHE_OBJECT in log, "Build log lacks the compiled Vulkan-option bridge")
        objects.add(CACHE_OBJECT)
    if variant == "performance":
        backend = performance_spec()["backend"]["source"]
        require(f"Linking CXX shared library {backend}" in log, "Build log lacks the linked gfxstream renderer")
        linked.add(backend)
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
        core = item["source"] == CORE
        inspect_pe(path, core=core, variant=variant,
                   markers=gate_variables("qemu") if core and variant == "performance" else ())
        files[item["source"]] = {"sha256": digest(path), "bytes": path.stat().st_size}
    if variant == "performance":
        backend = performance_spec()["backend"]["source"]
        path = inside(build, backend)
        inspect_pe(path, variant=variant, markers=gate_variables("gfxstream"))
        files[backend] = {"sha256": digest(path), "bytes": path.stat().st_size}
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
    path = path.parent.resolve() / path.name
    require(all(not path.is_relative_to(root) and not root.is_relative_to(path) for root in inputs),
            "Output overlaps an input directory")
    require(not path.exists() and not path.is_symlink(), "Output must be a fresh path")
    path.parent.mkdir(parents=True, exist_ok=True)
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
    targets = performance_spec()["build_targets"] if variant == "performance" else ["qemu-system-x86_64"]
    invoke_logged([str(cmake), "--build", str(output), "--parallel", "2", "--target", *targets],
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


def cache_reference(data, core_bytes):
    # Size selects a candidate. overlay_plan verifies the full hash of every module in that set.
    if core_bytes == MEDIA_CACHE_MODULES[CORE]["bytes"]:
        return data | {"cache_variant": data["cache_variant"] | {"reference_overrides": MEDIA_CACHE_MODULES}}
    return data


def overlay_plan(artifacts, dia, data, *, strict=False, requested=None):
    if strict:
        require(requested != "performance", "The performance engine has no archived reference build; stage a local build")
        expected, proof_sha, local_dia, variant = {}, None, None, requested or "clock"
    else:
        expected, proof_sha, local_dia, variant = validate_build(artifacts, data, requested)
    if strict and variant == "cache" and (artifacts / CORE).is_file():
        data = cache_reference(data, inside(artifacts, CORE).stat().st_size)
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
    if variant == "performance":
        backend = performance_spec()["backend"]
        content = expected[backend["source"]]
        plan.append((inside(artifacts, backend["source"]), backend["destinations"], content["sha256"], content["bytes"]))
    return plan, proof_sha, variant


def unpack_cache_overlay(archive, folder, data):
    """Read just the reference cache modules. The build manifest owns their paths and sizes."""
    with archive.open("rb") as stream, ZipFile(stream) as source:
        stream.seek(0)
        archive_sha = file_digest(stream, "sha256").hexdigest()
        names = source.namelist()
        if "artifacts/" + CORE in names:
            core_bytes = source.getinfo("artifacts/" + CORE).file_size
            data = cache_reference(data, core_bytes)
            if core_bytes == MEDIA_CACHE_MODULES[CORE]["bytes"]:
                require(archive_sha == MEDIA_CACHE_ZIP_SHA256, "Source-built media ZIP checksum mismatch")
        overrides = data["cache_variant"]["reference_overrides"]
        for row in data["overlay"]:
            if row["origin"] == "microsoft_dia":
                continue
            relative = row["source"]
            member = "artifacts/" + relative
            expected = row | overrides.get(relative, {})
            require(names.count(member) == 1, f"Expected one ZIP member for {relative}")
            info = source.getinfo(member)
            require(info.file_size == expected["bytes"], f"Unexpected ZIP member size: {relative}")
            destination = folder / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            with source.open(info) as incoming, destination.open("xb") as output:
                shutil.copyfileobj(incoming, output, length=1024 * 1024)
    return archive_sha


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
    unsafe = {TSC_GATE, CACHE_GATE, *gate_variables(), "ANDROID_EMULATOR_FEATURES", "ANDROID_SDK_HOME", "ANDROID_PREFS_ROOT",
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


def workspace_next_to(out):
    """Create the private work directory beside out with the permissions its parent hands down.

    tempfile.mkdtemp() asks for mode 0o700. Since Python 3.12.4 that gives a Windows folder an
    access list of only OWNER RIGHTS, SYSTEM and Administrators, and the rename below keeps it.
    Staged from an elevated prompt, the owner is the Administrators group, so the same user's
    normal (non-elevated) launcher could not read the engine. A plain mkdir inherits instead.
    """
    for _ in range(16):
        path = out.parent / f".whpx-stage-{secrets.token_hex(8)}"
        try:
            path.mkdir()
        except FileExistsError:
            continue
        return path
    raise ValueError(f"Could not create a work directory in {out.parent}")


def remove_tree(path):
    def writable(function, name, _error):
        os.chmod(name, stat.S_IWRITE)
        function(name)
    handler = {"onexc": writable} if sys.version_info >= (3, 12) else {"onerror": writable}
    shutil.rmtree(path, **handler)


def stage(out, stock, plan, receipt, data, smoke):
    components = [clean_component(stock, name) for name in COMPONENTS]
    temp = workspace_next_to(out)
    try:
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
            if relative.removeprefix("emulator/") not in replaced:
                checked(sdk / relative, sha)
        receipt.update(sdk=str(out / "sdk"), qemu_sha256=replaced[CORE],
                       emulator_sha256=data["stock_checks"]["emulator/emulator.exe"], replaced=replaced)
        if "lib64/libgfxstream_backend.dll" in replaced:
            receipt["gfxstream_sha256"] = replaced["lib64/libgfxstream_backend.dll"]
        require(not out.exists(), "Output appeared before publication")
        owned.rename(out)
    finally:
        remove_tree(temp)
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
        inputs = cmd.add_mutually_exclusive_group(required=True)
        inputs.add_argument("--artifacts", type=Path, help="Locally built or extracted artifacts")
        inputs.add_argument("--overlay-zip", type=Path, help="Prebuilt Windows cache module ZIP")
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
        dia = args.dia.resolve(strict=True) if args.dia else None
        stock_inputs(stock, data)
        with ExitStack() as cleanup:
            strict, requested = args.strict_reference, args.variant
            if args.overlay_zip:
                require(requested in (None, "cache"), "The prebuilt module ZIP is for the cache variant")
                strict, requested = True, "cache"
                artifacts = Path(cleanup.enter_context(TemporaryDirectory(prefix="rustemudroid-cache-"))).resolve()
                archive_sha = unpack_cache_overlay(args.overlay_zip, artifacts, data)
            else:
                artifacts = args.artifacts.resolve(strict=True)
            plan, proof, variant = overlay_plan(artifacts, dia, data, strict=strict, requested=requested)
            receipt = {"variant": variant, "reference_artifact_hashes_required": strict, "build_receipt_sha256": proof,
                       "overlay_manifest_sha256": lf_digest(MANIFEST), "copied_optional_dia": dia is not None,
                       "android_started": False, "overlay_path_count": sum(len(paths) for _, paths, _, _ in plan)}
            if args.overlay_zip:
                receipt["binary_archive_sha256"] = archive_sha
            if args.action == "stage":
                out = new_out(args.out, stock, artifacts, *([dia] if dia else []))
                stage(out, stock, plan, receipt, data, args.smoke)
        print(json.dumps(receipt, indent=2))
    except (OSError, ValueError, KeyError, BadZipFile, subprocess.SubprocessError) as error:
        cli.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
