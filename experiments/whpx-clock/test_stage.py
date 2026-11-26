"""Exercise the public staging paths with synthetic tools and two tiny Git projects."""
import ctypes
import json
import os
from pathlib import Path
import runpy
import struct
import subprocess
from tempfile import TemporaryDirectory, gettempdir
import unittest
from unittest.mock import patch
from zipfile import ZipFile

HERE = Path(__file__).resolve().parent
LAB = runpy.run_path(str(HERE / "stage.py"))
SCOPE = LAB["stage"].__globals__
CORE = LAB["CORE"]


def put(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return path


def pe(path, marker=True):
    header = bytearray(70)
    header[:2] = b"MZ"
    struct.pack_into("<I", header, 0x3C, 64)
    header[64:70] = b"PE\x00\x00\x64\x86"
    return put(path, bytes(header) + (b"RUSTEMUDROID_QA_WHPX_SYNC_TSC\0WHPX TSC QA: synchronized" if marker else b""))


def dacl(path):
    """Return the folder's DACL in SDDL form, for example D:AI(A;OICIID;FA;;;SY)."""
    advapi = ctypes.WinDLL("advapi32", use_last_error=True)
    kernel = ctypes.WinDLL("kernel32")
    advapi.GetNamedSecurityInfoW.argtypes = [ctypes.c_wchar_p, ctypes.c_int, ctypes.c_uint32, ctypes.c_void_p,
                                             ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p,
                                             ctypes.POINTER(ctypes.c_void_p)]
    advapi.ConvertSecurityDescriptorToStringSecurityDescriptorW.argtypes = [
        ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.POINTER(ctypes.c_void_p), ctypes.c_void_p]
    kernel.LocalFree.argtypes = [ctypes.c_void_p]
    descriptor, text = ctypes.c_void_p(), ctypes.c_void_p()
    status = advapi.GetNamedSecurityInfoW(str(path), 1, 4, None, None, None, None, ctypes.byref(descriptor))
    if status:
        raise OSError(status, "GetNamedSecurityInfoW failed", str(path))
    try:
        if not advapi.ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor, 1, 4, ctypes.byref(text), None):
            raise ctypes.WinError(ctypes.get_last_error())
        return ctypes.wstring_at(text.value)
    finally:
        kernel.LocalFree(text)
        kernel.LocalFree(descriptor)


def commit(root):
    LAB["git"](root, "init", "-q")
    LAB["git"](root, "add", ".")
    LAB["git"](root, "-c", "user.name=QA", "-c", "user.email=qa@example.invalid", "-c", "commit.gpgSign=false",
               "commit", "-qm", "Synthetic source fixture")
    return LAB["git"](root, "rev-parse", "HEAD")


class LabTests(unittest.TestCase):
    def test_prebuilt_zip_uses_only_reference_paths_sizes_and_hashes(self):
        with TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            dll = "lib64/test.dll"
            blob = pe(root / dll).read_bytes()
            row = {"source": dll, "destinations": [dll], "origin": "locally_linked", "bytes": 1, "sha256": ""}
            data = {"overlay": [row], "cache_variant": {"reference_overrides": {
                dll: {"bytes": len(blob), "sha256": LAB["digest"](root / dll)}}}}
            archive = root / "overlay.zip"
            for i, value in enumerate((blob, blob[:-1], blob[:-1] + b"?")):
                dest = root / str(i)
                with ZipFile(archive, "w") as out:
                    out.writestr("artifacts/" + dll, value)
                    out.writestr("../unexpected", b"ignored")
                if len(value) != len(blob):
                    with self.assertRaisesRegex(ValueError, "size"):
                        LAB["unpack_cache_overlay"](archive, dest, data)
                    continue
                LAB["unpack_cache_overlay"](archive, dest, data)
                self.assertFalse((root / "unexpected").exists())
                if value == blob:
                    plan, _, variant = LAB["overlay_plan"](dest, None, data, strict=True, requested="cache")
                    self.assertEqual((plan[0][0], variant), (dest / dll, "cache"))
                else:
                    with self.assertRaisesRegex(ValueError, "digest"):
                        LAB["overlay_plan"](dest, None, data, strict=True, requested="cache")

    def test_source_media_zip_requires_its_digest_and_matched_companion(self):
        with TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            dll = "lib64/libemugl_common.dll"
            for name in (CORE, dll):
                pe(root / name)
            with (root / CORE).open("ab") as core:
                core.write(b"RUSTEMUDROID_QA_VULKAN_CACHED_HOST\0RustEmuDroid QA: requested cached Vulkan host allocations")
            modules = {name: {"sha256": LAB["digest"](root / name), "bytes": (root / name).stat().st_size}
                       for name in (CORE, dll)}
            rows = [{"source": name, "origin": "locally_linked", "destinations": [name]} for name in modules]
            data = {"overlay": rows, "cache_variant": {"reference_overrides": {}}}
            archive = root / "media.zip"
            with ZipFile(archive, "w") as out:
                for name in modules:
                    out.write(root / name, "artifacts/" + name)
            expected_sha = LAB["digest"](archive)
            with patch.dict(SCOPE, MEDIA_CACHE_MODULES=modules, MEDIA_CACHE_ZIP_SHA256=expected_sha):
                extracted = root / "extracted"
                self.assertEqual(LAB["unpack_cache_overlay"](archive, extracted, data), expected_sha)
                plan, _, _ = LAB["overlay_plan"](extracted, None, data, strict=True, requested="cache")
                self.assertEqual([sha for _, _, sha, _ in plan], [modules[name]["sha256"] for name in modules])
                with (extracted / dll).open("r+b") as module:
                    module.seek(-1, 2)
                    module.write(b"?")
                with self.assertRaisesRegex(ValueError, "digest"):
                    LAB["overlay_plan"](extracted, None, data, strict=True, requested="cache")
                with ZipFile(archive, "a") as out:
                    out.comment = b"changed envelope"
                with self.assertRaisesRegex(ValueError, "ZIP checksum"):
                    LAB["unpack_cache_overlay"](archive, root / "modified", data)

    def test_public_manifest(self):
        data = LAB["manifest"]()
        projects, patches = LAB["source_inputs"](data)
        self.assertEqual(len(projects), 60)
        self.assertEqual(len(patches), 1)
        self.assertTrue(patches[0].is_file())
        self.assertEqual(len(LAB["source_inputs"](data, "cache")[1]), 2)
        groups = data["overlay"]
        self.assertEqual(sum(len(row["destinations"]) for row in groups if row["origin"] != "microsoft_dia"), 20)

    def test_local_build_attestation(self):
        with TemporaryDirectory(prefix="whpx-source-test-") as tmp:
            r = Path(tmp).resolve()
            source, build = r / "src", r / "build"
            build.mkdir()
            qemu, dep = source / "external/qemu", source / "external/dep"
            for relative in LAB["PATCHED"]:
                put(qemu / relative, b"original\n")
            put(dep / "dep.txt", b"pinned companion\n")
            q_sh, d_sh = commit(qemu), commit(dep)
            for relative in LAB["PATCHED"]:
                put(qemu / relative, b"patched\n")
            patch_file = put(r / "source.patch", subprocess.check_output(["git", "-C", str(qemu), "diff", "--binary"]))
            lock = put(r / "source.xml", (f'<manifest><project name="external/qemu" revision="{q_sh}"/>'
                                          f'<project name="external/dep" revision="{d_sh}"/></manifest>\n').encode())
            data = {"basis": {"project_count": 2, "qemu_source_commit": q_sh, "public_patch_path": patch_file.name,
                              "public_patch_sha256": LAB["digest"](patch_file), "public_manifest_path": lock.name,
                              "public_manifest_sha256": LAB["digest"](lock), "candidate_build_id": "qa-whpx-16970787",
                              "android_emulator_revision": "37.3.10"},
                    "overlay": [{"source": CORE, "origin": "locally_linked", "destinations": [CORE],
                                 "sha256": "0" * 64, "bytes": 2}]}
            with patch.dict(SCOPE, PROJECT=r):
                proof = LAB["source_check"](source, data)
                canonical = lock.read_bytes()
                put(lock, canonical.replace(b"\n", b"\r\n"))
                self.assertNotEqual(LAB["digest"](lock), LAB["lf_digest"](lock))
                self.assertEqual(LAB["source_inputs"](data)[0], proof["projects"])
                core = pe(build / CORE)
                put(build / "configure.log", b"clang-r596125 Clang 22.0.2 Generating done")
                put(build / "build.log", (f"Building target/i386/whpx-all.c.obj qemu-system-x86_64.dir/cpus.c.obj\n"
                                          f"Linking CXX executable {CORE}\n").encode())
                targets = [CORE, *(f"CMakeFiles/qemu-system-x86_64.dir/{file}.obj" for file in LAB["PATCHED"]
                                   if file.endswith(".c"))]
                put(build / ".ninja_log", ("# ninja log v5\n" + "".join(f"0\t1\t1\t{t}\tdeadbeef\n" for t in targets)).encode())
                put(build / "CMakeCache.txt", ('CMAKE_GENERATOR:INTERNAL=Ninja\nCMAKE_BUILD_TYPE:STRING=Release\n'
                                              'BUILD_TESTING:BOOL=OFF\nOPTION_MINBUILD:BOOL=ON\n'
                                              'OPTION_SDK_TOOLS_BUILD_NUMBER:STRING=qa-whpx-16970787\n'
                                              'OPTION_SDK_TOOLS_REVISION:STRING=37.3.10\n'
                                              f'CMAKE_HOME_DIRECTORY:INTERNAL={qemu.as_posix()}\n').encode())
                receipts = {"version": 1, "variant": "clock", "overlay_manifest_sha256": LAB["lf_digest"](LAB["MANIFEST"]),
                            "source": str(source), "verified_source": proof, "logs": LAB["check_logs"](build, source, data),
                            "files": LAB["linked_inventory"](build, data)}
                LAB["write"](build / "build-receipt.json", receipts)
                plan, seal, variant = LAB["overlay_plan"](build, None, data)
                self.assertEqual(plan[0][2], LAB["digest"](core))
                self.assertTrue(seal)
                self.assertEqual(variant, "clock")
                with self.assertRaisesRegex(ValueError, "different variant"):
                    LAB["overlay_plan"](build, None, data, requested="cache")
                with self.assertRaisesRegex(ValueError, "digest"):
                    LAB["overlay_plan"](build, None, data, strict=True)
                put(core, core.read_bytes() + b"tampered")
                with self.assertRaisesRegex(ValueError, "changed"):
                    LAB["overlay_plan"](build, None, data)
                put(dep / "dep.txt", b"different tracked source\n")
                with self.assertRaisesRegex(ValueError, "Modified pinned source"):
                    LAB["source_check"](source, data)
                put(dep / "dep.txt", b"pinned companion\n")
                put(qemu / LAB["PATCHED"][0], b"a different patch\n")
                with self.assertRaisesRegex(ValueError, "differ"):
                    LAB["source_check"](source, data)
                put(core, core.read_bytes() + b"RUSTEMUDROID_QA_VULKAN_CACHED_HOST")
                with self.assertRaisesRegex(ValueError, "clock experiment markers"):
                    LAB["inspect_pe"](core, core=True)
                put(core, core.read_bytes() + b"RustEmuDroid QA: requested cached Vulkan host allocations")
                LAB["inspect_pe"](core, core=True, variant="cache")

    def test_staging_and_rollback_paths(self):
        with TemporaryDirectory(prefix="whpx-stage-test-") as tmp:
            root = Path(tmp).resolve()
            stock = root / "stock"
            launcher = put(stock / "emulator/emulator.exe", b"launcher")
            put(stock / "platform-tools/adb.exe", b"adb")
            put(stock / "system-images/owned.img", b"private")
            build = pe(root / "built.exe")
            data = {"stock_checks": {"emulator/emulator.exe": LAB["digest"](launcher)}}
            plan = [(build, [CORE], LAB["digest"](build), build.stat().st_size)]
            out = root / "new"
            secret = put(stock / "emulator/adbkey", b"private")
            with self.assertRaisesRegex(ValueError, "credential"):
                LAB["stage"](out, stock, plan, {}, data, False)
            self.assertFalse(out.exists())
            secret.unlink()
            with self.assertRaisesRegex(ValueError, "overlaps"):
                LAB["new_out"](stock / "new", stock)
            deep = LAB["new_out"](root / "private" / "engines" / "cache", stock)
            self.assertTrue(deep.parent.is_dir())
            self.assertFalse(deep.exists())
            with patch("shutil.copytree", side_effect=OSError("interrupted copy")):
                with self.assertRaisesRegex(OSError, "interrupted"):
                    LAB["stage"](out, stock, plan, {}, data, False)
            self.assertFalse(out.exists())
            self.assertFalse(list(root.glob(".whpx-stage-*")))
            with patch.dict(SCOPE, loader_smoke=lambda sdk, _data: self.check_final_sdk(sdk, out)):
                LAB["stage"](out, stock, plan, {}, data, True)
            self.assertEqual(json.loads((out / "staging.json").read_bytes())["qemu_sha256"], LAB["digest"](build))
            self.assertFalse((out / "sdk/system-images").exists())
            self.assertFalse((stock / "emulator" / CORE).exists())
            failed = root / "failed"
            with patch.dict(SCOPE, loader_smoke=lambda *_: self.fail_smoke()):
                with self.assertRaisesRegex(ValueError, "lost loader"):
                    LAB["stage"](failed, stock, plan, {}, data, True)
            self.assertFalse((failed / "staging.json").exists())
            self.assertTrue((failed / "staging-error.json").is_file())

    @unittest.skipUnless(os.name == "nt", "Windows access lists")
    def test_staged_folder_inherits_the_parent_access_list(self):
        # tempfile.mkdtemp() gives Windows folders a protected owner/SYSTEM/Administrators list on
        # Python >= 3.12.4. A folder staged elevated that way could not be read by the normal launcher.
        # The fixture itself must not come from mkdtemp(), or every folder in it would inherit that list.
        root = Path(gettempdir()).resolve() / f"whpx-acl-test-{os.getpid()}-{os.urandom(4).hex()}"
        root.mkdir()
        self.addCleanup(LAB["remove_tree"], root)
        parent = root / "engines"
        parent.mkdir()
        stock = root / "stock"
        launcher = put(stock / "emulator/emulator.exe", b"launcher")
        put(stock / "platform-tools/adb.exe", b"adb")
        build = pe(root / "built.exe")
        data = {"stock_checks": {"emulator/emulator.exe": LAB["digest"](launcher)}}
        plan = [(build, [CORE], LAB["digest"](build), build.stat().st_size)]
        reference = parent / "reference"
        reference.mkdir()
        out = parent / "new"
        LAB["stage"](out, stock, plan, {}, data, False)
        self.assertFalse(dacl(out).startswith("D:P"), dacl(out))
        self.assertEqual(dacl(out), dacl(reference))
        (reference / "sdk").mkdir()
        self.assertEqual(dacl(out / "sdk"), dacl(reference / "sdk"))
        self.assertFalse(list(parent.glob(".whpx-stage-*")))

    def test_performance_variant_patches_qemu_and_gfxstream(self):
        data = LAB["manifest"]()
        projects, patches = LAB["source_inputs"](data, "performance")
        self.assertEqual([p.name for p in patches],
                         ["whpx-tsc-sync.patch", "gfxstream-cached-host-qa.patch", "performance-engine-qemu.patch"])
        _, sets = LAB["patch_sets"](data, "performance")
        self.assertEqual(set(sets), {"external/qemu", "hardware/google/gfxstream"})
        self.assertEqual(sets["hardware/google/gfxstream"][0][0].name, "performance-engine-gfxstream.patch")
        self.assertEqual(projects["hardware/google/gfxstream"], "501867e6a3b1a3be7b935cc977d291150846edab")
        self.assertTrue(set(LAB["PATCHED"]) | {LAB["CACHE_SOURCE"]} <= set(sets["external/qemu"][1]))
        names = LAB["gate_variables"]()
        self.assertIn("RUSTEMUDROID_QA_ASG_SPIN_US", LAB["gate_variables"]("gfxstream"))
        self.assertNotIn("RUSTEMUDROID_QA_ASG_SPIN_US", LAB["gate_variables"]("qemu"))
        self.assertEqual(len(names), len(set(names)))
        with self.assertRaisesRegex(ValueError, "no archived reference"):
            LAB["overlay_plan"](HERE, None, data, strict=True, requested="performance")

    def test_performance_source_check_with_synthetic_projects(self):
        with TemporaryDirectory(prefix="whpx-perf-test-") as tmp:
            r = Path(tmp).resolve()
            source = r / "src"
            qemu, gfx = source / "external/qemu", source / "hardware/google/gfxstream"
            qemu_files = (*LAB["PATCHED"], LAB["CACHE_SOURCE"], "extra/perf.c")
            for relative in qemu_files:
                put(qemu / relative, b"original\n")
            put(gfx / "host/ring_stream.cpp", b"original\n")
            q_sh, g_sh = commit(qemu), commit(gfx)
            layers = (("clock", qemu, LAB["PATCHED"]), ("cache", qemu, (LAB["CACHE_SOURCE"],)),
                      ("perf", qemu, ("extra/perf.c",)), ("gfx", gfx, ("host/ring_stream.cpp",)))
            patches = {}
            for name, root, files in layers:
                for relative in files:
                    put(root / relative, f"{name}\n".encode())
                patches[name] = put(r / f"{name}.patch",
                                    subprocess.check_output(["git", "-C", str(root), "diff", "--binary"]))
                LAB["git"](root, "checkout", "-q", "--", ".")
            blobs = {qemu: {}, gfx: {}}
            for name, root, files in layers:
                for relative in files:
                    blobs[root][relative] = subprocess.run(["git", "-C", str(root), "hash-object", "--stdin"],
                                                           input=f"{name}\n".encode(), capture_output=True,
                                                           check=True).stdout.decode().strip()
            lock = put(r / "source.xml", (f'<manifest><project name="external/qemu" revision="{q_sh}"/>'
                                          f'<project name="hardware/google/gfxstream" revision="{g_sh}"/></manifest>\n').encode())
            sha = LAB["digest"]
            data = {"basis": {"project_count": 2, "qemu_source_commit": q_sh, "public_patch_path": "clock.patch",
                              "public_patch_sha256": sha(patches["clock"]), "public_manifest_path": lock.name,
                              "public_manifest_sha256": sha(lock)},
                    "cache_variant": {"patch_path": "cache.patch", "patch_sha256": sha(patches["cache"])}}
            spec = {"version": 1, "variant": "performance", "gates": [],
                    "projects": {"external/qemu": {"commit": q_sh, "patched_blobs": blobs[qemu], "patches": [
                                     {"path": f"{name}.patch", "sha256": sha(patches[name])}
                                     for name in ("clock", "cache", "perf")]},
                                 "hardware/google/gfxstream": {"commit": g_sh, "patched_blobs": blobs[gfx], "patches": [
                                     {"path": "gfx.patch", "sha256": sha(patches["gfx"])}]}}}
            description = put(r / "performance-engine.json", json.dumps(spec).encode())
            with patch.dict(SCOPE, PROJECT=r, PERFORMANCE=description):
                proof = LAB["source_check"](source, data, "performance", prepare=True)
                self.assertEqual(proof["gfxstream_patched_blobs"], blobs[gfx])
                self.assertEqual(proof["patched_blobs"], blobs[qemu])
                self.assertEqual(LAB["source_check"](source, data, "performance"), proof)
                put(gfx / "host/ring_stream.cpp", b"edited\n")
                with self.assertRaisesRegex(ValueError, "gfxstream contents differ"):
                    LAB["source_check"](source, data, "performance")
                put(gfx / "host/ring_stream.cpp", b"gfx\n")
                spec["projects"]["hardware/google/gfxstream"]["patched_blobs"] = {"host/ring_stream.cpp": "0" * 40}
                put(description, json.dumps(spec).encode())
                with self.assertRaisesRegex(ValueError, "no longer produce the pinned"):
                    LAB["source_check"](source, data, "performance")

    def check_final_sdk(self, sdk, out):
        self.assertEqual(sdk, out / "sdk")
        self.assertTrue((sdk / "emulator" / CORE).exists())
        return [{"flag": "synthetic", "code": 0}]

    @staticmethod
    def fail_smoke():
        raise ValueError("lost loader")


if __name__ == "__main__":
    unittest.main(verbosity=2)
