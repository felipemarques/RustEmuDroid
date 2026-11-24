"""Shared disk verification and QEMU lifecycle for offline Pie64 diagnostics."""

import hashlib
import json
import shutil
import socket
import struct
import subprocess
import time
from pathlib import Path


BOOT_SHA256 = "eb8c58a274b81261f64b405173abdd26dcc4e09d6847d2ca19a265ce77eb7c87"
INITRAMFS_SHA256 = "00e89256b42578548a51abf8be8622132d71dd7e8bbad0ff3b347e92533589be"


class Qmp:
    def __init__(self, address, process, startup_seconds):
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.socket.settimeout(5)
        deadline = time.monotonic() + startup_seconds
        while True:
            try:
                self.socket.connect(str(address))
                break
            except (FileNotFoundError, ConnectionRefusedError):
                if process.poll() is not None or time.monotonic() >= deadline:
                    self.socket.close()
                    raise RuntimeError("QEMU did not open the local monitor. See qemu.log.")
                time.sleep(0.1)
        self.stream = self.socket.makefile("rwb")
        if "QMP" not in self.read():
            self.close()
            raise RuntimeError("Unexpected QEMU monitor greeting")
        self.execute("qmp_capabilities")

    def read(self):
        message = self.stream.readline()
        if not message:
            raise RuntimeError("QEMU monitor disconnected")
        return json.loads(message)

    def execute(self, command, **arguments):
        data = {"execute": command, "arguments": arguments}
        self.stream.write(json.dumps(data).encode() + b"\n")
        self.stream.flush()
        while True:
            reply = self.read()
            if "error" in reply:
                raise RuntimeError(reply["error"]["desc"])
            if "return" in reply:
                return reply["return"]

    def close(self):
        self.stream.close()
        self.socket.close()


def verify_copy(path, expected):
    with path.open("rb") as source:
        actual = hashlib.file_digest(source, "sha256").hexdigest()
    if actual != expected:
        raise ValueError(f"{path.name} does not match the verified Pie64 fixture (SHA-256 mismatch)")


def cpio_header(name, fields, size):
    name += b"\0"
    fields = fields.copy()
    fields[6], fields[11] = size, len(name)
    prefix = b"070701" + b"".join(f"{field:08x}".encode() for field in fields) + name
    return prefix + bytes(-len(prefix) % 4)


def extract_pie64_linux(qemu_img, boot, scratch):
    info = subprocess.check_output([qemu_img, "info", "-f", "vdi", "--output=json", str(boot)])
    if not 1024 <= json.loads(info)["virtual-size"] <= 64 * 1024 * 1024:
        raise ValueError("Unexpected Pie64 boot disk size")

    raw = scratch / "fastboot.raw"
    subprocess.run([qemu_img, "convert", "-f", "vdi", "-O", "raw", str(boot), str(raw)], check=True)
    return unpack_pie64_linux(raw, scratch)


def unpack_pie64_linux(raw, scratch):
    image = raw.read_bytes()
    payload = image[512:]
    if image[510:512] != b"\x55\xaa" or payload[0x202:0x206] != b"HdrS":
        raise ValueError("Expected a Pie64 boot sector followed by a Linux bzImage")

    # x86 Linux boot protocol describes the setup and protected-mode payload lengths.
    setup_sectors = payload[0x1f1] or 4
    protected_bytes = 16 * struct.unpack_from("<I", payload, 0x1f4)[0]
    kernel_end = 512 * (1 + setup_sectors) + protected_bytes
    initrd_start = ((kernel_end + 511) // 512) * 512
    # This Pie64 disk stores the initramfs size in the final boot-sector word.
    initrd_end = initrd_start + struct.unpack_from("<I", image, 0x1fa)[0]
    if not (0 < kernel_end <= initrd_start < initrd_end <= len(payload)):
        raise ValueError("Invalid kernel or initramfs extent")
    if payload[initrd_start:initrd_start + 2] != b"\x1f\x8b":
        raise ValueError("Expected a gzip initramfs after the Linux payload")

    kernel = scratch / "kernel.bz"
    initrd = scratch / "initramfs.gz"
    kernel.write_bytes(payload[:kernel_end])
    initrd.write_bytes(payload[initrd_start:initrd_end])
    return kernel, initrd


def add_qemu_args(parser):
    parser.add_argument("--images", required=True, type=Path, help="Directory with the offline disk copies")
    parser.add_argument("--output", required=True, type=Path, help="New directory for device and boot evidence")
    parser.add_argument("--qemu", default="qemu-system-x86_64")
    parser.add_argument("--qemu-img", default="qemu-img")
    parser.add_argument("--firmware", type=Path, help="Alternative QEMU firmware search directory")
    parser.add_argument("--bios", type=Path, help="Alternative system BIOS image")
    parser.add_argument("--seconds", type=int, default=40, help="Guest observation window in seconds (1–120)")


def qemu_command(args, scratch, accel="tcg"):
    command = [
        args.qemu, "-machine", "q35,sata=off,dump-guest-core=off",
        "-accel", accel, "-cpu", "host" if accel == "kvm" else "max",
        "-nodefaults", "-nic", "none", "-m", "4096", "-smp", "4",
        "-display", "none", "-vga", "std", "-monitor", "none",
        "-serial", f"file:{scratch / 'serial.log'}",
        "-qmp", f"unix:{scratch / 'qmp.sock'},server=on,wait=off",
        "-no-reboot", "-S",
    ]
    if args.firmware:
        command += ["-L", str(args.firmware.resolve())]
    if args.bios:
        command += ["-bios", str(args.bios.resolve())]
    return command


def linux_boot(kernel, initrd, kernel_args):
    console = "console=ttyS0,115200n8 earlyprintk=serial,ttyS0,115200 keep_bootcon ignore_loglevel"
    return ["-kernel", str(kernel), "-initrd", str(initrd), "-append", f"{console} {kernel_args}".strip()]


def observe_qemu(command, scratch, output, seconds, inspect_disks, *, after_pause=None, env=None, startup_seconds=8):
    with (output / "qemu.log").open("wb") as host_log:
        process = subprocess.Popen(command, stdout=host_log, stderr=subprocess.STDOUT, env=env)
        monitor = None
        try:
            monitor = Qmp(scratch / "qmp.sock", process, startup_seconds)
            block = monitor.execute("query-block")
            (output / "disks.json").write_text(json.dumps(block, indent=2) + "\n")
            inspect_disks(block)
            devices = monitor.execute("human-monitor-command", **{"command-line": "info qtree"})
            (output / "devices.txt").write_text(devices)
            monitor.execute("cont")
            deadline = time.monotonic() + seconds
            while process.poll() is None and time.monotonic() < deadline:
                time.sleep(0.2)
            if process.poll() is None:
                monitor.execute("stop")
                status = monitor.execute("query-status")
                (output / "status.json").write_text(json.dumps(status, indent=2) + "\n")
                if after_pause:
                    after_pause(monitor, output)
                monitor.execute("quit")
                process.wait(timeout=5)
        finally:
            if monitor is not None:
                monitor.close()
            if process.poll() is None:
                process.kill()
                process.wait()
            for filename in ("serial.log", "debug.log"):
                recorded = scratch / filename
                if recorded.is_file():
                    shutil.copyfile(recorded, output / filename)
    return {"qemu_exit": process.returncode,
            "observation_window_completed": (output / "status.json").is_file()}
