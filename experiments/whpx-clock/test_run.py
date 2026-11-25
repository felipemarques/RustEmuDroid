"""Offline contract tests for the Windows WHPX experiment launcher."""
from contextlib import nullcontext, redirect_stdout
from hashlib import sha256
import importlib.util
from io import StringIO
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("whpx_clock_run", Path(__file__).with_name("run.py"))
lab = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lab)

WHPX = "WHPX on Windows 10.0.26300 detected.\n"
BOOT = WHPX + "INFO         | Boot completed in 21721 ms\n"
CACHE_MARKER = "RustEmuDroid QA: requested cached Vulkan host allocations\n"
CACHE_IGNORED = "VulkanAllocateUncachedHostVisibleAsCached has no effect\n"
MARKERS = "WHPX TSC QA: synchronized init vcpus=4\nWHPX TSC QA: synchronized reset vcpus=4\n"


class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.base = Path(self.enterContext(TemporaryDirectory(prefix="WHPX fixture "))).resolve()
        self.app = self.make("RustEmuDroid.exe", "fake launcher")
        self.sdk = self.base / "staging/sdk"
        self.profile = self.base / "test profile"
        self.avd = self.profile / "avd/RustEmuDroid.avd"
        self.registration = self.make("test profile/avd/RustEmuDroid.ini", f"path={self.avd}\n")
        self.relative_image = Path("system-images/android-37.0/google_apis_playstore/x86_64")
        self.image = self.sdk / self.relative_image
        self.config = self.make("test profile/avd/RustEmuDroid.avd/config.ini",
                                f"hw.cpu.ncore=4\nimage.sysdir.1={self.relative_image.as_posix()}\n")
        self.props = self.make(self.image / "source.properties",
                               "SystemImage.TagId=google_apis_playstore\nAndroidVersion.ApiLevel=37.0\n"
                               "Pkg.Revision=7\nSystemImage.Abi=x86_64\n")
        self.make(self.image / "system.img", "fake image")
        self.adb = self.make(self.sdk / "platform-tools/adb.exe", "fake adb")
        self.qemu = self.make(self.sdk / lab.QEMU, "fake qemu")
        self.emulator = self.make(self.sdk / "emulator/emulator.exe", "fake emulator")
        self.receipt = self.make("staging/staging.json", json.dumps({
            "qemu_sha256": sha256(self.qemu.read_bytes()).hexdigest(),
            "emulator_sha256": sha256(self.emulator.read_bytes()).hexdigest(),
        }))

    def make(self, path, text):
        file = self.base / path
        file.parent.mkdir(exist_ok=True, parents=True)
        file.write_text(text, encoding="utf-8")
        return file

    def check(self):
        return lab.check_inputs(self.app, self.sdk, self.profile)

    def test_qa_sdk_and_profile_with_relative_image(self):
        self.assertEqual(self.check(), self.image)

    def test_qa_sdk_and_profile_with_absolute_installed_image(self):
        self.config.write_text(f"hw.cpu.ncore=4\nimage.sysdir.1={self.image}\n", encoding="utf-8")
        self.assertEqual(self.check(), self.image)

    def test_profile_registration_must_select_requested_avd(self):
        self.registration.write_text(f"path={self.base / 'other.avd'}\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "registration must point"):
            self.check()

    def test_profile_requires_four_vcpus(self):
        self.config.write_text(self.config.read_text().replace("ncore=4", "ncore=2"), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "four-vCPU"):
            self.check()

    def test_sdk_requires_android_17_google_play_r7_x86_64(self):
        original = self.props.read_text()
        for expected, incompatible in (("37.0", "36.0"), ("google_apis_playstore", "google_apis"),
                                        ("Revision=7", "Revision=6"), ("x86_64", "arm64-v8a")):
            with self.subTest(incompatible=incompatible):
                self.props.write_text(original.replace(expected, incompatible), encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "Android 17 Google Play r7 x86-64"):
                    self.check()

    def test_sdk_requires_adb_and_image(self):
        self.adb.unlink()
        with self.assertRaisesRegex(ValueError, "missing platform-tools"):
            self.check()
        self.adb.write_bytes(b"restore fake adb")
        (self.image / "system.img").unlink()
        with self.assertRaisesRegex(ValueError, "image used in the clock tests"):
            self.check()

    def test_staging_receipt_matches_both_executables(self):
        for binary in (self.qemu, self.emulator):
            with self.subTest(binary=binary.name):
                original = binary.read_bytes()
                binary.write_bytes(b"modified after staging")
                with self.assertRaisesRegex(ValueError, "differs from the staging receipt"):
                    self.check()
                binary.write_bytes(original)

    def test_staging_receipt_is_required(self):
        self.receipt.unlink()
        with self.assertRaises(FileNotFoundError):
            self.check()

    def test_cache_mode_requires_its_staged_qemu(self):
        with self.assertRaisesRegex(ValueError, "staged cache variant"):
            lab.check_inputs(self.app, self.sdk, self.profile, fix="vulkan-cache")
        candidate = b"before\0" + lab.CACHE.encode("ascii") + b"\0after"
        self.qemu.write_bytes(candidate)
        with self.assertRaisesRegex(ValueError, "differs from the staging receipt"):
            lab.check_inputs(self.app, self.sdk, self.profile, fix="vulkan-cache")
        receipt = json.loads(self.receipt.read_bytes())
        receipt["qemu_sha256"] = sha256(candidate).hexdigest()
        self.receipt.write_text(json.dumps(receipt), encoding="utf-8")
        self.assertEqual(lab.check_inputs(self.app, self.sdk, self.profile, fix="vulkan-cache"), self.image)

    def test_check_cli_does_not_launch(self):
        args = ["run.py", "check", "--app", str(self.app), "--sdk", str(self.sdk), "--profile", str(self.profile)]
        with patch.object(lab.sys, "platform", "win32"), patch.object(lab.sys, "argv", args), \
                patch.object(lab, "launch") as launch, redirect_stdout(StringIO()) as out:
            lab.main()
        launch.assert_not_called()
        self.assertIn(str(self.image), out.getvalue())

    def fake_launch(self, logs, *, mode="off", fix="clock", hz=None, status=0, existing=None):
        args = SimpleNamespace(app=self.app, sdk=self.sdk, profile=self.profile,
                               lang="pt-br", mode=mode, fix=fix, hz=hz, gpu_device="Fixture GPU")
        plans = []
        for name, text in (existing or {}).items():
            self.make(self.profile / name, text)

        def popen(cmd, *, env):
            temporary = Path(cmd[cmd.index("--state-dir") + 1])
            config = json.loads((temporary / "launcher.json").read_text("utf-8"))
            plans.append((cmd, env, config, temporary))
            for name, text in logs.items():
                self.make(self.profile / name, text)
            return nullcontext(SimpleNamespace(pid=431, wait=lambda: status, returncode=status))

        with patch.object(lab, "single_test_window", return_value=nullcontext()), \
                patch.object(lab.subprocess, "Popen", side_effect=popen), \
                patch.object(lab.time, "time_ns", side_effect=(1_000_000_000, 1_500_000_000)), \
                patch.dict(lab.os.environ, {lab.TSC.lower(): "inherited", lab.CACHE.lower(): "inherited",
                                           "RUN_TEST_UNRELATED": "keep"}), \
                redirect_stdout(StringIO()):
            lab.launch(args)
        (cmd, env, config, temporary), = plans
        self.assertFalse(temporary.exists())
        self.assertFalse((self.profile / "launcher.json").exists())
        return cmd, env, config

    def test_launch_config_uses_launcher_field_names_and_isolates_toggle(self):
        cmd, env, config = self.fake_launch({"boot-1000-431.log": BOOT + MARKERS}, mode="on", hz=90)
        self.assertEqual(cmd[:6], [self.app, "--sdk", self.sdk, "--lang", "pt-br", "--state-dir"])
        self.assertEqual(cmd[-2:], ["ui", "--play"])
        self.assertEqual(config, {
            "state": str(self.profile), "sdk": str(self.sdk), "image": "", "gpu": "Host",
            "hardware": {"adapter": "Fixture GPU", "camera": "Profile", "webcam_index": 0, "refresh_rate": 90},
            "show_fps": True, "adaptive_video": True,
        })
        self.assertEqual({k: v for k, v in env.items() if k.upper() in (lab.TSC, lab.CACHE)}, {lab.TSC: "1"})
        self.assertEqual(env["RUN_TEST_UNRELATED"], "keep")

    def test_default_refresh_and_disabled_clean_environment(self):
        _, env, config = self.fake_launch({"boot-1000-431.log": BOOT})
        self.assertNotIn("refresh_rate", config["hardware"])
        self.assertFalse({k.upper() for k in env} & {lab.TSC, lab.CACHE})

    def test_vulkan_toggle_only_sets_its_own_flag(self):
        for mode, marker, expected in (("off", "", {}), ("on", CACHE_MARKER, {lab.CACHE: "1"})):
            with self.subTest(mode=mode):
                _, env, _ = self.fake_launch({"boot-1000-431.log": BOOT + marker},
                                             mode=mode, fix="vulkan-cache")
                self.assertEqual({k: v for k, v in env.items() if k.upper() in (lab.TSC, lab.CACHE)}, expected)
                self.assertEqual(env["RUN_TEST_UNRELATED"], "keep")
                (self.profile / "boot-1000-431.log").unlink()

    def test_only_logs_from_this_launch_and_pid_are_used(self):
        self.fake_launch({
            "boot-999-431.log": "stale same PID",
            "boot-1000-432.log": "different PID",
            "boot-invalid-431.log": "malformed timestamp",
            "boot-1501-431.log": "future timestamp",
            "boot-1500-431.log": BOOT,
        }, existing={"boot-1200-431.log": "previous launch within time window"})

    def test_stale_and_other_pid_logs_do_not_qualify(self):
        with self.assertRaisesRegex(ValueError, "No boot log from this launch"):
            self.fake_launch({"boot-999-431.log": WHPX, "boot-1000-432.log": WHPX})

    def test_future_and_preexisting_same_pid_logs_do_not_qualify(self):
        with self.assertRaisesRegex(ValueError, "No boot log from this launch"):
            self.fake_launch({"boot-1501-431.log": BOOT}, existing={"boot-1200-431.log": BOOT})

    def test_failed_child_exit_does_not_qualify(self):
        with self.assertRaisesRegex(ValueError, "exited with code 3"):
            self.fake_launch({"boot-1000-431.log": BOOT}, status=3)

    def test_disabled_rejects_empty_log_and_enabled_markers(self):
        with self.assertRaisesRegex(ValueError, "did not report WHPX"):
            lab.verify_closed_log(self.make("empty.log", ""), False)
        with self.assertRaisesRegex(ValueError, "did not report Android boot completion"):
            lab.verify_closed_log(self.make("partial.log", WHPX), False)
        with self.assertRaisesRegex(ValueError, "markers differed"):
            lab.verify_closed_log(self.make("wrong-arm.log", BOOT + MARKERS), False)

    def test_vulkan_logs_reject_tsc_marker_in_either_mode(self):
        for enabled in (False, True):
            with self.subTest(enabled=enabled):
                cache_marker = CACHE_MARKER if enabled else ""
                log = self.make("mixed-vulkan.log", BOOT + cache_marker + MARKERS)
                with self.assertRaises(ValueError):
                    lab.verify_closed_log(log, enabled, fix="vulkan-cache")

    def test_clock_logs_reject_vulkan_marker_in_either_mode(self):
        for enabled in (False, True):
            with self.subTest(enabled=enabled):
                clock_markers = MARKERS if enabled else ""
                log = self.make("mixed-clock.log", BOOT + clock_markers + CACHE_MARKER)
                with self.assertRaises(ValueError):
                    lab.verify_closed_log(log, enabled, fix="clock")

    def test_vulkan_logs_require_effective_requested_mode(self):
        for enabled, markers in ((True, ""), (True, CACHE_MARKER + CACHE_IGNORED),
                                 (True, CACHE_MARKER * 2), (False, CACHE_MARKER)):
            with self.subTest(enabled=enabled, markers=markers):
                with self.assertRaises(ValueError):
                    lab.verify_closed_log(self.make("wrong-cache.log", BOOT + markers), enabled, fix="vulkan-cache")

    def test_enabled_requires_the_two_successful_four_vcpu_phases(self):
        for marker in ("", MARKERS.splitlines()[0], MARKERS.replace("vcpus=4", "vcpus=2"),
                       MARKERS + "WHPX TSC QA: synchronization failed\n"):
            with self.subTest(marker=marker):
                with self.assertRaisesRegex(ValueError, "markers differed"):
                    lab.verify_closed_log(self.make("failed-toggle.log", BOOT + marker), True)


if __name__ == "__main__":
    unittest.main()
