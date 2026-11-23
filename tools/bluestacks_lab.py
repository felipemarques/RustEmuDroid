"""Run a bounded QEMU probe against an offline copy of a BlueStacks Pie64 VM."""

import argparse
import json
import shutil
import socket
import struct
import subprocess
import tempfile
import time
from pathlib import Path

DISKS = {"boot": ("fastboot.vdi", "vdi"), "root": ("Root.vhd", "vpc"), "data": ("Data.vhdx", "vhdx")}


class Qmp:
    def __init__(self, address, process):
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.socket.settimeout(5)
        deadline = time.monotonic() + 8
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


def extract_pie64_linux(qemu_img, boot, scratch):
    info = subprocess.check_output([qemu_img, "info", "-f", "vdi", "--output=json", str(boot)])
    if not 1024 <= json.loads(info)["virtual-size"] <= 64 * 1024 * 1024:
        raise ValueError("Unexpected Pie64 boot disk size")

    raw = scratch / "fastboot.raw"
    subprocess.run([qemu_img, "convert", "-f", "vdi", "-O", "raw", str(boot), str(raw)], check=True)
    image = raw.read_bytes()
    payload = image[512:]
    if image[510:512] != b"\x55\xaa" or payload[0x202:0x206] != b"HdrS":
        raise ValueError("Expected a Pie64 boot sector followed by a Linux bzImage")

    # x86 Linux boot protocol describes the setup and protected-mode payload lengths.
    setup_sectors = payload[0x1f1] or 4
    protected_bytes = 16 * struct.unpack_from("<I", payload, 0x1f4)[0]
    kernel_end = 512 * (1 + setup_sectors) + protected_bytes
    initrd_start = ((kernel_end + 511) // 512) * 512
    # This Pie64 disk stores the initramfs size in the final boot-sector word before the signature.
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


def run_probe(args):
    output = args.output.resolve()
    images = args.images.resolve(strict=True)
    sources = {}
    for label, (filename, fmt) in DISKS.items():
        source = (images / filename).resolve(strict=True)
        if source.parent != images or not source.is_file():
            raise ValueError(f"Expected a regular disk file under the lab directory: {filename}")
        sources[label] = (source, fmt)

    output.mkdir(parents=True)
    with tempfile.TemporaryDirectory(prefix="rustemudroid-pie64-") as temporary:
        scratch = Path(temporary)
        for label, (source, fmt) in sources.items():
            subprocess.run(
                [args.qemu_img, "create", "-q", "-f", "qcow2", "-F", fmt, "-b",
                 str(source), str(scratch / f"{label}.qcow2")],
                check=True,
            )

        command = [
            args.qemu, "-machine", "q35,sata=off,dump-guest-core=off",
            "-accel", args.accel, "-cpu", "host" if args.accel == "kvm" else "max",
            "-nodefaults", "-nic", "none", "-m", "4096", "-smp", "4",
            "-display", "none", "-vga", "std", "-monitor", "none",
            "-serial", f"file:{scratch / 'serial.log'}",
            "-debugcon", f"file:{scratch / 'debug.log'}",
            "-qmp", f"unix:{scratch / 'qmp.sock'},server=on,wait=off",
            "-no-reboot", "-S",
            "-device", "piix3-ide,id=legacy,addr=02.0", "-device", "ich9-ahci,id=sata",
        ]
        if args.firmware:
            command += ["-L", str(args.firmware.resolve())]
        if args.bios:
            command += ["-bios", str(args.bios.resolve())]
        if args.remap_ide_dma:
            command += ["-option-rom", str(build_dma_rom(scratch))]
        if args.direct_linux:
            kernel, initrd = extract_pie64_linux(args.qemu_img, sources["boot"][0], scratch)
            console = "console=ttyS0,115200n8 earlyprintk=serial,ttyS0,115200 keep_bootcon ignore_loglevel"
            command += [
                "-kernel", str(kernel), "-initrd", str(initrd),
                "-append", f"{console} {args.kernel_args}".strip(),
            ]
        for label in DISKS:
            command += ["-drive", f"file={scratch / (label + '.qcow2')},if=none,format=qcow2,id={label}"]
        command += [
            "-device", "ide-hd,drive=boot,bus=legacy.0,bootindex=1",
            "-device", "ide-hd,drive=root,bus=sata.0",
            "-device", "ide-hd,drive=data,bus=sata.1",
        ]
        with (output / "qemu.log").open("wb") as host_log:
            process = subprocess.Popen(command, stdout=host_log, stderr=subprocess.STDOUT)
            monitor = None
            try:
                monitor = Qmp(scratch / "qmp.sock", process)
                block = monitor.execute("query-block")
                (output / "disks.json").write_text(json.dumps(block, indent=2) + "\n")
                for label, (_, fmt) in DISKS.items():
                    image = next(item["inserted"]["image"] for item in block if item["device"] == label)
                    expected = {"format": "qcow2", "backing": fmt}
                    actual = {"format": image["format"], "backing": image["backing-image"]["format"]}
                    if actual != expected:
                        raise RuntimeError(f"Unexpected storage mapping for {label}")
                devices = monitor.execute("human-monitor-command", **{"command-line": "info qtree"})
                (output / "devices.txt").write_text(devices)
                monitor.execute("cont")
                deadline = time.monotonic() + args.seconds
                while process.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.2)
                if process.poll() is None:
                    monitor.execute("stop")
                    status = monitor.execute("query-status")
                    (output / "status.json").write_text(json.dumps(status, indent=2) + "\n")
                    registers = monitor.execute("human-monitor-command", **{"command-line": "info registers"})
                    (output / "registers.txt").write_text(registers)
                    pci = monitor.execute("human-monitor-command", **{"command-line": "info pci"})
                    (output / "pci.txt").write_text(pci)
                    monitor.execute("screendump", filename=str(output / "screen.ppm"))
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
    print(f"Probe finished with QEMU exit {process.returncode}. Evidence: {output}")
    return int(process.returncode != 0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--images", required=True, type=Path, help="Directory with the three offline disk copies")
    parser.add_argument("--output", required=True, type=Path, help="New directory for device and boot evidence")
    parser.add_argument("--qemu", default="qemu-system-x86_64")
    parser.add_argument("--qemu-img", default="qemu-img")
    parser.add_argument("--firmware", type=Path, help="Alternative QEMU firmware search directory")
    parser.add_argument("--bios", type=Path, help="Alternative BIOS ROM")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--remap-ide-dma", action="store_true", help="Set the PIIX3 bus-master I/O base to 0xd040")
    mode.add_argument("--direct-linux", action="store_true", help="Start the original kernel and initramfs with a serial console")
    parser.add_argument("--kernel-args", default="", help="Extra Linux parameters for the direct boot diagnostic")
    parser.add_argument("--accel", choices=("tcg", "kvm"), default="tcg")
    parser.add_argument("--seconds", type=int, default=40, help="Guest observation window in seconds (1–120)")
    args = parser.parse_args()
    if not 1 <= args.seconds <= 120:
        parser.error("--seconds must be between 1 and 120")
    if args.kernel_args and not args.direct_linux:
        parser.error("--kernel-args requires --direct-linux")
    return run_probe(args)


if __name__ == "__main__":
    raise SystemExit(main())
