"""Test the data probe's evidence and exit status without starting QEMU."""

import argparse
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import bluestacks_data_probe as probe


class DataProbeTest(unittest.TestCase):
    def simulate_probe(self, markers, qemu_exit=0):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for name in ("fastboot.vdi", "Data_orig.vhdx"):
                (directory / name).touch()
            args = argparse.Namespace(
                images=directory, output=directory / "evidence", seconds=1,
                qemu="fake-qemu", qemu_img="fake-qemu-img", firmware=None, bios=None,
            )

            def observe(_command, _scratch, output, _seconds, inspect_disks):
                inspect_disks([
                    {"device": "anchor", "inserted": {"ro": True, "image": {"format": "raw"}}},
                    {"device": "factory", "inserted": {"ro": True, "image": {
                        "format": "qcow2", "backing-image": {"format": "vhdx"},
                    }}},
                ])
                (output / "serial.log").write_text("\n".join(markers))
                return {"qemu_exit": qemu_exit, "observation_window_completed": True}

            with (
                patch.object(probe, "os", SimpleNamespace(name="posix")),
                patch.object(probe, "verify_copy"),
                patch.object(probe.subprocess, "run"),
                patch.object(probe, "extract_pie64_linux", return_value=("kernel", "initramfs")),
                patch.object(probe, "build_data_carrier", return_value="carrier"),
                patch.object(probe, "observe_qemu", side_effect=observe),
                patch("builtins.print"),
            ):
                exit_code = probe.run_probe(args)
            result = json.loads((args.output / "result.json").read_text())
            return exit_code, result

    def test_success_requires_all_milestones(self):
        complete = (probe.PARTITION_MARKER, probe.SUPERBLOCK_MARKER, probe.MOUNT_MARKER)
        exit_code, result = self.simulate_probe(complete)
        self.assertEqual(exit_code, 0)
        self.assertTrue(result["factory_partition_detected"])
        self.assertTrue(result["factory_ext4_superblock_verified"])
        self.assertTrue(result["factory_ext4_mounted_read_only"])
        self.assertEqual(self.simulate_probe(complete, qemu_exit=1)[0], 1)

    def test_incomplete_observation_writes_evidence_and_fails(self):
        exit_code, result = self.simulate_probe((probe.PARTITION_MARKER,))
        self.assertEqual(exit_code, 1)
        self.assertTrue(result["factory_partition_detected"])
        self.assertFalse(result["factory_ext4_superblock_verified"])
        self.assertFalse(result["factory_ext4_mounted_read_only"])


if __name__ == "__main__":
    unittest.main()
