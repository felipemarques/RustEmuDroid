"""Check the engine manifest writer with tiny fake binaries instead of a real engine."""
import json
from pathlib import Path
import runpy
from tempfile import TemporaryDirectory
import unittest

HERE = Path(__file__).resolve().parent
TOOL = runpy.run_path(str(HERE / "engine_manifest.py"))
SPEC = json.loads((HERE / "performance-engine.json").read_bytes())


def put(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return path


def markers(module, *, drop=()):
    found = [marker for name, modules in SPEC["capabilities"].items() for marker in modules.get(module, [])
             if marker not in drop]
    return b"\0".join(marker.encode() for marker in found)


class ManifestTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(TemporaryDirectory(prefix="engine-manifest-"))).resolve()
        self.engine = self.root / "engine"
        sdk = self.engine / "sdk/emulator"
        put(sdk / "source.properties", b"Pkg.Revision=37.3.10\nPkg.BuildId=16970787\n")
        put(sdk / "emulator.exe", b"MZ launcher")
        self.qemu = put(sdk / "qemu/windows-x86_64/qemu-system-x86_64.exe", b"MZ qa-whpx-16970787\0" + markers("qemu"))
        self.gfx = put(sdk / "lib64/libgfxstream_backend.dll", b"MZ " + markers("gfxstream"))
        put(sdk / "lib64/libemugl_common.dll", b"MZ helper")

    def test_writes_the_client_schema_with_hashes_and_verified_capabilities(self):
        wanted = ["tsc_sync", "vulkan_cached_host", "asg_spin", "tsc_sync", "vcpu_placement_high"]
        folder, manifest = TOOL["build_manifest"](self.engine, wanted, extra=["emulator/lib64/libemugl_common.dll"])
        target = TOOL["write_manifest"](folder, manifest)
        written = json.loads(target.read_bytes())
        self.assertEqual(target, self.engine / "rustemudroid-engine.json")
        self.assertEqual(list(written), ["schema", "kind", "engine_version", "build_id", "emulator_sha256", "qemu_sha256",
                                         "gfxstream_sha256", "extra_sha256", "capabilities"])
        self.assertEqual((written["schema"], written["kind"], written["engine_version"], written["build_id"]),
                         (1, "rustemudroid-performance-engine", "37.3.10", "qa-whpx-16970787"))
        self.assertEqual(written["qemu_sha256"], TOOL["digest"](self.qemu))
        self.assertEqual(written["gfxstream_sha256"], TOOL["digest"](self.gfx))
        self.assertEqual(list(written["extra_sha256"]), ["emulator/lib64/libemugl_common.dll"])
        self.assertEqual(written["capabilities"], ["tsc_sync", "vulkan_cached_host", "asg_spin", "vcpu_placement_high"])
        with self.assertRaisesRegex(ValueError, "already exists"):
            TOOL["write_manifest"](folder, manifest)
        TOOL["write_manifest"](folder, manifest | {"capabilities": []}, replace=True)
        self.assertEqual(json.loads(target.read_bytes())["capabilities"], [])
        self.assertFalse(list(self.engine.glob(".engine-manifest-*")))

    def test_every_published_capability_has_markers_in_matching_fake_modules(self):
        _, _, files = TOOL["engine_files"](self.engine)
        self.assertTrue(all(TOOL["evidence"](files, TOOL["capability_markers"]()).values()))
        self.assertEqual(set(SPEC["capabilities"]), {
            "tsc_sync", "lapic_fastpath", "kvm_pv_eoi", "vulkan_cached_host", "vulkan_cached_device_local_v2",
            "asg_spin", "vulkan_native_swapchain", "vcpu_placement_high", "guest_kernel_parameters"})

    def test_rejects_capabilities_without_evidence(self):
        put(self.qemu, b"MZ qa-whpx-16970787\0" + markers("qemu", drop={"sets-high", "RustEmuDroid QA: KVM PV EOI active"}))
        with self.assertRaisesRegex(ValueError, "vcpu_placement_high.*sets-high"):
            TOOL["build_manifest"](self.engine, ["tsc_sync", "vcpu_placement_high"])
        with self.assertRaisesRegex(ValueError, "kvm_pv_eoi"):
            TOOL["build_manifest"](self.engine, ["kvm_pv_eoi"])
        _, _, files = TOOL["engine_files"](self.engine)
        found = TOOL["evidence"](files, TOOL["capability_markers"]())
        self.assertEqual([name for name, ok in found.items() if not ok], ["kvm_pv_eoi", "vcpu_placement_high"])
        with self.assertRaisesRegex(ValueError, "Unknown capability: vcpu_placement"):
            TOOL["build_manifest"](self.engine, ["vcpu_placement"])
        put(self.gfx, b"MZ stock renderer")
        with self.assertRaisesRegex(ValueError, "libgfxstream_backend.dll lacks"):
            TOOL["build_manifest"](self.engine, ["asg_spin"])

    def test_rejects_wrong_build_version_and_unsafe_extra_paths(self):
        with self.assertRaisesRegex(ValueError, "identify itself as build other-build"):
            TOOL["build_manifest"](self.engine, ["tsc_sync"], build_id="other-build local")
        _, manifest = TOOL["build_manifest"](self.engine, ["tsc_sync"], build_id="qa-whpx-16970787 local A+B")
        self.assertEqual(manifest["build_id"], "qa-whpx-16970787 local A+B")
        self.assertNotIn("extra_sha256", manifest)
        for bad in ("lib64/libemugl_common.dll", "emulator\\lib64\\libemugl_common.dll", "emulator/../adb.exe",
                    "C:/emulator/x.dll", "emulator/emulator.exe", "emulator/lib64/missing.dll"):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                TOOL["build_manifest"](self.engine, ["tsc_sync"], extra=[bad])
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            TOOL["build_manifest"](self.engine, ["tsc_sync"], extra=["emulator/lib64/libemugl_common.dll"] * 2)
        put(self.engine / "sdk/emulator/source.properties", b"Pkg.Revision=37.4.2\n")
        with self.assertRaisesRegex(ValueError, "37.3.10"):
            TOOL["build_manifest"](self.engine, ["tsc_sync"])
        with self.assertRaisesRegex(ValueError, "sdk"):
            TOOL["build_manifest"](self.engine / "sdk", ["tsc_sync"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
