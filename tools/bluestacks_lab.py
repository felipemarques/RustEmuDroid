"""Run a bounded QEMU probe against an offline copy of a BlueStacks Pie64 VM."""

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path

from bluestacks_qemu import (
    BOOT_SHA256, add_qemu_args, extract_pie64_linux, linux_boot, observe_qemu,
    qemu_command, unpack_pie64_linux, verify_copy,
)

DISKS = {"boot": ("fastboot.vdi", "vdi"), "root": ("Root.vhd", "vpc"), "data": ("Data.vhdx", "vhdx")}


def build_dma_rom(scratch):
    source = Path(__file__).with_name("bluestacks_piix3_dma.S")
    obj = scratch / "dma.o"
    rom = scratch / "dma.rom"
    subprocess.run(["as", "--32", str(source), "-o", str(obj)], check=True)
    subprocess.run(["objcopy", "-O", "binary", "-j", ".text", str(obj), str(rom)], check=True)
    image = bytearray(rom.read_bytes())
    if len(image) != 512 or image[:3] != b"\x55\xaa\x01":
        raise RuntimeError("Expected a single-sector option ROM")
    image[-1] = (-sum(image[:-1])) & 0xff
    rom.write_bytes(image)
    return rom


def collect_legacy_snapshots(monitor, output):
    registers = monitor.execute("human-monitor-command", **{"command-line": "info registers"})
    (output / "registers.txt").write_text(registers)
    pci = monitor.execute("human-monitor-command", **{"command-line": "info pci"})
    (output / "pci.txt").write_text(pci)
    monitor.execute("screendump", filename=str(output / "screen.ppm"))


def run_probe(args):
    if args.data_probe:
        from bluestacks_data_probe import run_probe as run_data_probe

        return run_data_probe(args)
    output = args.output.resolve()
    images = args.images.resolve(strict=True)
    guest_disks = required_disks = DISKS
    if args.system_mount:
        guest_disks = {}
        required_disks = {label: DISKS[label] for label in ("boot", "root")}
    sources = {}
    for label, (filename, fmt) in required_disks.items():
        source = (images / filename).resolve(strict=True)
        if source.parent != images or not source.is_file():
            raise ValueError(f"Expected a regular disk file under the lab directory: {filename}")
        sources[label] = (source, fmt)

    if args.system_mount:
        if os.name != "posix":
            raise ValueError("--system-mount needs Linux or WSL. On Windows, use Test-BlueStacksImage.ps1 -SystemMount")
        from bluestacks_system_mount import (
            CONFIG_BLOCKER, CONFIG_MARKER, ROOT_SHA256, SYSTEM_MARKER,
            build_system_carrier, convert_system_inputs, drop_privileges, make_squashfs,
            qemu_environment, system_mount_user,
        )
        user = system_mount_user(args.run_as)
        verify_copy(sources["boot"][0], BOOT_SHA256)
        verify_copy(sources["root"][0], ROOT_SHA256)
    if not args.system_mount:
        output.mkdir(parents=True)
    with tempfile.TemporaryDirectory(prefix="rustemudroid-pie64-") as temporary:
        scratch = Path(temporary)
        launch_env = None
        for label in guest_disks:
            source, fmt = sources[label]
            subprocess.run(
                [args.qemu_img, "create", "-q", "-f", "qcow2", "-F", fmt, "-b",
                 str(source), str(scratch / f"{label}.qcow2")],
                check=True,
            )

        command = qemu_command(args, scratch, args.accel)
        command += ["-debugcon", f"file:{scratch / 'debug.log'}"]
        if guest_disks and not args.virtio_blk:
            command += ["-device", "piix3-ide,id=legacy,addr=02.0", "-device", "ich9-ahci,id=sata"]
        if args.remap_ide_dma:
            command += ["-option-rom", str(build_dma_rom(scratch))]
        if args.direct_linux or args.system_mount:
            kernel_args = args.kernel_args
            if args.system_mount:
                print("Preparing a verified read-only SquashFS copy of the Pie64 root.", flush=True)
                boot, root = convert_system_inputs(args.qemu_img, sources["boot"][0], sources["root"][0], user, scratch)
                kernel, initrd = unpack_pie64_linux(boot, scratch)
                squashfs = make_squashfs(root, scratch)
                initrd = build_system_carrier(initrd, squashfs, scratch)
                build = {"root_squashfs_bytes": squashfs.stat().st_size, "carrier_bytes": initrd.stat().st_size,
                         "boot_sha256": BOOT_SHA256, "root_sha256": ROOT_SHA256}
                squashfs.unlink()
                drop_privileges(user, scratch)
                output.mkdir(parents=True)
                (output / "system-mount.json").write_text(json.dumps(build, indent=2) + "\n")
                kernel_args = "rdinit=/boot/init"
                launch_env = qemu_environment()
            else:
                kernel, initrd = extract_pie64_linux(args.qemu_img, sources["boot"][0], scratch)
            command += linux_boot(kernel, initrd, kernel_args)
        for label in guest_disks:
            command += ["-drive", f"file={scratch / (label + '.qcow2')},if=none,format=qcow2,id={label}"]
        if args.virtio_blk:
            command += [
                "-device", "virtio-blk-pci,drive=boot,bootindex=1",
                "-device", "virtio-blk-pci,drive=root",
                "-device", "virtio-blk-pci,drive=data",
            ]
        elif guest_disks:
            command += [
                "-device", "ide-hd,drive=boot,bus=legacy.0,bootindex=1",
                "-device", "ide-hd,drive=root,bus=sata.0",
                "-device", "ide-hd,drive=data,bus=sata.1",
            ]
        def inspect_disks(block):
            if args.system_mount and block:
                raise RuntimeError("The system-mount VM must have no attached disks")
            for label, (_, fmt) in guest_disks.items():
                image = next(item["inserted"]["image"] for item in block if item["device"] == label)
                if image["format"] != "qcow2" or image["backing-image"]["format"] != fmt:
                    raise RuntimeError(f"Unexpected storage mapping for {label}")

        result = observe_qemu(command, scratch, output, args.seconds, inspect_disks,
                              after_pause=collect_legacy_snapshots, env=launch_env,
                              startup_seconds=30 if args.system_mount else 8)
        if args.system_mount:
            serial_path = output / "serial.log"
            serial = serial_path.read_text(errors="replace") if serial_path.is_file() else ""
            result.update(system_attached=SYSTEM_MARKER in serial,
                          original_configuration_started=CONFIG_MARKER in serial,
                          android_id_missing=CONFIG_BLOCKER in serial)
        (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"Probe finished with QEMU exit {result['qemu_exit']}. Evidence: {output}")
    return int(result["qemu_exit"] != 0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_qemu_args(parser)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--remap-ide-dma", action="store_true", help="Set the PIIX3 bus-master I/O base to 0xd040")
    mode.add_argument("--direct-linux", action="store_true", help="Start the original kernel and initramfs with a serial console")
    mode.add_argument("--system-mount", action="store_true",
                      help="Mount the verified copied system through temporary read-only SquashFS")
    mode.add_argument("--data-probe", action="store_true",
                      help="Check the verified Pie64 data seed via read-only VirtIO SCSI")
    parser.add_argument("--run-as", help="Non-root QEMU user for --system-mount (defaults to SUDO_USER)")
    parser.add_argument("--kernel-args", default="", help="Extra Linux parameters for the direct boot diagnostic")
    parser.add_argument("--virtio-blk", action="store_true", help="Attach copies to VirtIO block devices for direct Linux boot")
    parser.add_argument("--accel", choices=("tcg", "kvm"), default="tcg")
    args = parser.parse_args()
    if not 1 <= args.seconds <= 120:
        parser.error("--seconds must be between 1 and 120")
    if args.kernel_args and not args.direct_linux:
        parser.error("--kernel-args requires --direct-linux")
    if args.virtio_blk and not args.direct_linux:
        parser.error("--virtio-blk requires --direct-linux")
    if args.run_as and not args.system_mount:
        parser.error("--run-as requires --system-mount")
    if args.data_probe and args.accel != "tcg":
        parser.error("--data-probe uses TCG")
    try:
        return run_probe(args)
    except ValueError as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
