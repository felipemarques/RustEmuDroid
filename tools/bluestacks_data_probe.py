"""Check the verified Pie64 factory data filesystem through read-only VirtIO SCSI."""

import argparse
import gzip
import json
import os
import subprocess
import tempfile
from pathlib import Path

from bluestacks_qemu import (
    BOOT_SHA256, INITRAMFS_SHA256, add_qemu_args, cpio_header, extract_pie64_linux,
    linux_boot, observe_qemu, qemu_command, verify_copy,
)


FACTORY_DATA_SHA256 = "6ee803f4124e2c4a3704726fcacdb6c3fdece107a3d849267e6758408ea52ad5"
PARTITION_MARKER = "Pie64 data probe: sdb1 geometry verified"
SUPERBLOCK_MARKER = "Pie64 data probe: ext4 superblock magic verified"
MOUNT_MARKER = "Pie64 data probe: ext4 mounted read-only at /data and unmounted"
INIT = b"""#!/boot/bin/sh
bb=/boot/bin/busybox
$bb mknod /dev/kmsg c 1 11
log() {
    echo "<2>Pie64 data probe: $*" > /dev/kmsg
}
finish() {
    log "$1"
    $bb reboot -f
    while :; do $bb sleep 1; done
}
$bb mount -t proc proc /proc || finish "cannot mount proc"
$bb mount -t sysfs sysfs /sys || finish "cannot mount sysfs"
count=0
while [ ! -e /sys/class/block/sdb1/start ] && [ "$count" -lt 8 ]; do
    $bb sleep 1
    count=$((count + 1))
done
driver=$($bb cat /sys/class/scsi_host/host*/proc_name 2>/dev/null)
[ "$driver" = "virtio_scsi" ] || finish "VirtIO SCSI host not observed"
log "VirtIO SCSI host observed"
$bb mdev -s
[ -b /dev/sdb1 ] || finish "sdb1 block node not observed"
start=$($bb cat /sys/class/block/sdb1/start)
size=$($bb cat /sys/class/block/sdb1/size)
[ "$start" = "2048" ] && [ "$size" = "268433408" ] ||
    finish "unexpected sdb1 geometry"
log "sdb1 geometry verified"
[ "$($bb cat /sys/class/block/sdb1/ro)" = "1" ] ||
    finish "sdb1 is not read-only"
log "sdb1 is read-only in the guest"
# Read the sector at offset 1024, the start of the ext4 superblock.
$bb dd if=/dev/sdb1 of=/superblock bs=512 skip=2 count=1 >/dev/null 2>&1 ||
    finish "cannot read ext4 superblock sector"
magic=$($bb hexdump -s 56 -n 2 -e '2/1 "%02x"' /superblock 2>/dev/null)
[ "$magic" = "53ef" ] ||
    finish "cannot decode ext4 superblock magic"
log "ext4 superblock magic verified"
$bb mount -t ext4 -o ro,noload /dev/sdb1 /data ||
    finish "cannot mount ext4 read-only"
mounted=false
while read source target filesystem options rest; do
    if [ "$source" = /dev/sdb1 ] && [ "$target" = /data ] && [ "$filesystem" = ext4 ]; then
        case ",$options," in
            *,ro,*) mounted=true ;;
        esac
    fi
done < /proc/mounts
$bb umount /data || finish "cannot unmount /data"
[ "$mounted" = true ] || finish "read-only ext4 mount not observed"
finish "ext4 mounted read-only at /data and unmounted"
"""


def extract_busybox(initramfs):
    verify_copy(initramfs, INITRAMFS_SHA256)
    archive = gzip.decompress(initramfs.read_bytes())
    offset = 0
    while offset + 110 < len(archive):
        header = archive[offset:offset + 110]
        if header[:6] != b"070701":
            break
        size = int(header[54:62], 16)
        namesize = int(header[94:102], 16)
        name_end = offset + 110 + namesize
        name = archive[offset + 110:name_end - 1]
        start = (name_end + 3) & ~3
        if name == b"boot/bin/busybox":
            return archive[start:start + size]
        if name == b"TRAILER!!!":
            break
        offset = (start + size + 3) & ~3
    raise ValueError("Could not find BusyBox in the verified Pie64 initramfs")


def build_data_carrier(initramfs, scratch):
    records = [
        (b".", 0o040755, b""),
        (b"boot", 0o040755, b""),
        (b"boot/bin", 0o040755, b""),
        (b"data", 0o040755, b""),
        (b"dev", 0o040755, b""),
        (b"proc", 0o040755, b""),
        (b"sys", 0o040755, b""),
        (b"boot/bin/busybox", 0o100755, extract_busybox(initramfs)),
        (b"boot/bin/sh", 0o120777, b"busybox"),
        (b"boot/init", 0o100755, INIT),
        (b"TRAILER!!!", 0, b""),
    ]
    archive = bytearray()
    for ino, (name, mode, content) in enumerate(records, 1):
        fields = [ino, mode, 0, 0, 1, 0, len(content), 0, 0, 0, 0, 0, 0]
        archive += cpio_header(name, fields, len(content))
        archive += content + bytes(-len(content) % 4)
    archive += bytes(-len(archive) % 512)
    carrier = scratch / "data-probe-initramfs.gz"
    carrier.write_bytes(gzip.compress(archive, mtime=0))
    return carrier


def run_probe(args):
    if os.name != "posix":
        raise ValueError("Use Test-BlueStacksData.ps1 on Windows or run this command inside Linux/WSL")
    images = args.images.resolve(strict=True)
    boot = images / "fastboot.vdi"
    factory = images / "Data_orig.vhdx"
    for path, expected in ((boot, BOOT_SHA256), (factory, FACTORY_DATA_SHA256)):
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"Expected an ordinary copied file: {path.name}")
        verify_copy(path, expected)

    output = args.output.resolve()
    output.mkdir(parents=True)
    with tempfile.TemporaryDirectory(prefix="rustemudroid-pie64-data-") as temporary:
        scratch = Path(temporary)
        overlay = scratch / "factory.qcow2"
        subprocess.run([args.qemu_img, "create", "-q", "-f", "qcow2", "-F", "vhdx", "-b",
                        str(factory), str(overlay)], check=True)
        anchor = scratch / "anchor.raw"
        with anchor.open("wb") as disk:
            disk.truncate(1024 * 1024)
        kernel, initramfs = extract_pie64_linux(args.qemu_img, boot, scratch)
        carrier = build_data_carrier(initramfs, scratch)
        command = qemu_command(args, scratch) + linux_boot(kernel, carrier, "rdinit=/boot/init")
        command += [
            "-drive", f"file={overlay},if=none,format=qcow2,readonly=on,id=factory",
            "-drive", f"file={anchor},if=none,format=raw,readonly=on,id=anchor",
            "-device", "virtio-scsi-pci,id=storage",
            "-device", "scsi-hd,drive=anchor,bus=storage.0,scsi-id=0",
            "-device", "scsi-hd,drive=factory,bus=storage.0,scsi-id=1",
        ]

        def inspect_disks(block):
            disks = {item["device"]: item["inserted"] for item in block}
            if len(block) != 2 or set(disks) != {"anchor", "factory"} or not all(
                disk["ro"] for disk in disks.values()
            ):
                raise RuntimeError("Expected exactly two read-only SCSI disks")
            if disks["anchor"]["image"]["format"] != "raw":
                raise RuntimeError("Expected a raw placeholder disk")
            image = disks["factory"]["image"]
            if image["format"] != "qcow2" or image["backing-image"]["format"] != "vhdx":
                raise RuntimeError("Expected a QCOW2 overlay of the VHDX seed")

        result = observe_qemu(command, scratch, output, args.seconds, inspect_disks)
    serial_path = output / "serial.log"
    serial = serial_path.read_text(errors="replace") if serial_path.is_file() else ""
    result.update(factory_partition_detected=PARTITION_MARKER in serial,
                  factory_ext4_superblock_verified=SUPERBLOCK_MARKER in serial,
                  factory_ext4_mounted_read_only=MOUNT_MARKER in serial)
    (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    complete = (result["factory_partition_detected"] and result["factory_ext4_superblock_verified"]
                and result["factory_ext4_mounted_read_only"])
    if complete:
        print("The original kernel mounted the factory ext4 read-only at /data and unmounted it.")
    else:
        print("The read-only mount milestone was not observed. Inspect serial.log.")
    print(f"QEMU exit {result['qemu_exit']}. Evidence: {output}")
    return int(result["qemu_exit"] != 0 or not complete)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_qemu_args(parser)
    args = parser.parse_args()
    if not 1 <= args.seconds <= 120:
        parser.error("--seconds must be between 1 and 120")
    try:
        return run_probe(args)
    except (ValueError, FileNotFoundError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
