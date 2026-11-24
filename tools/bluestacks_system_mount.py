"""Prepare the verified Pie64 system-mount diagnostic without mounting host disks in QEMU."""

import gzip
import hashlib
import os
import pwd
import shutil
import stat
import struct
import subprocess
import tempfile
from pathlib import Path

from bluestacks_qemu import INITRAMFS_SHA256, cpio_header, verify_copy

ROOT_SHA256 = "b3fa7ddc10067349b2b8a441688253e7d94537093909ead2723869ddf7c7c074"
BOOT_RAW_SHA256 = "74f12bf5cb69c6a2dd15dc2ffe0b68698826c2ec5dc01d258cead24f3ac4ba35"
ROOT_RAW_SHA256 = "aa615fd8f4bb114a6297fc0ac101c8810b671f4fdf774468d53497103689eb02"
ANDROID_RAMDISK_SHA256 = "ffadff1d63e1e398645dd5e18cdead7d99ef45e038596399b4377615fe4524b9"
SYSTEM_MARKER = "Pie64 lab: original system directory attached"
CONFIG_MARKER = "Pie64 lab: invoking original BlueStacks configuration"
CONFIG_BLOCKER = "ERROR: android_id is empty"
ROOT_MOUNT = b"boot/lab-root.squashfs"
HOST_ENV = {"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LC_ALL": "C"}


def system_mount_user(run_as):
    if os.geteuid() != 0:
        raise ValueError("--system-mount requires root for temporary read-only filesystem mounts; use sudo")
    name = run_as or os.environ.get("SUDO_USER")
    if not name:
        raise ValueError("Pass --run-as with the non-root user that should run QEMU")
    try:
        user = pwd.getpwnam(name)
    except KeyError as error:
        raise ValueError(f"Unknown QEMU user: {name}") from error
    if user.pw_uid == 0:
        raise ValueError("The system-mount diagnostic runs QEMU as a non-root user")
    for executable in ("mount", "umount", "mksquashfs", "unsquashfs", "rsync"):
        if shutil.which(executable, path=HOST_ENV["PATH"]) is None:
            raise ValueError(f"--system-mount requires {executable} on the Linux host")
    return user


def drop_privileges(user, scratch):
    for path in scratch.iterdir():
        os.chown(path, user.pw_uid, user.pw_gid, follow_symlinks=False)
    os.chown(scratch, user.pw_uid, user.pw_gid, follow_symlinks=False)
    os.initgroups(user.pw_name, user.pw_gid)
    os.setgid(user.pw_gid)
    os.setuid(user.pw_uid)


def qemu_environment():
    env = os.environ.copy()
    for name in ("LD_LIBRARY_PATH", "QEMU_MODULE_DIR"):
        bundled = env.pop(f"RUSTEMUDROID_QEMU_{name}", None)
        if bundled:
            env[name] = bundled
    return env


def copy_verified_raw(source, target, size, expected):
    # Freeze a verified copy in the root-owned directory before mounting it.
    digest = hashlib.sha256()
    with os.fdopen(os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as data:
        info = os.fstat(data.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size != size:
            raise ValueError(f"Unexpected converted {source.name} size or type")
        with target.open("xb") as snapshot:
            remaining = size
            while remaining:
                chunk = data.read(min(remaining, 1024 * 1024))
                if not chunk:
                    raise ValueError(f"Unexpected end of converted {source.name}")
                digest.update(chunk)
                if chunk.count(0) == len(chunk):
                    snapshot.seek(len(chunk), os.SEEK_CUR)
                else:
                    snapshot.write(chunk)
                remaining -= len(chunk)
            snapshot.truncate(size)
    if digest.hexdigest() != expected:
        raise ValueError(f"Converted {source.name} failed SHA-256 verification")


def convert_system_inputs(qemu_img, boot, root, user, scratch):
    if shutil.disk_usage(scratch).free < 8 * 1024**3:
        raise ValueError("--system-mount requires at least 8 GiB free in the Linux temporary filesystem")
    inputs = (
        (boot, "vdi", "fastboot.raw", 8_429_568, BOOT_RAW_SHA256),
        (root, "vpc", "root.raw", 8_589_901_824, ROOT_RAW_SHA256),
    )
    with tempfile.TemporaryDirectory(prefix="rustemudroid-pie64-convert-") as temporary:
        staging = Path(temporary)
        os.chown(staging, user.pw_uid, user.pw_gid)
        groups = os.getgrouplist(user.pw_name, user.pw_gid)
        for disk, fmt, name, size, digest in inputs:
            output = staging / name
            subprocess.run(
                [qemu_img, "convert", "-f", fmt, "-O", "raw", "-S", "4k", str(disk), str(output)],
                check=True, user=user.pw_uid, group=user.pw_gid, extra_groups=groups, env=qemu_environment(),
            )
            copy_verified_raw(output, scratch / name, size, digest)
    return scratch / "fastboot.raw", scratch / "root.raw"


def make_squashfs(raw, scratch):
    with raw.open("r+b") as image:
        mbr = image.read(512)
        if len(mbr) != 512 or mbr[510:] != b"\x55\xaa" or mbr[450] != 0x83:
            raise ValueError("Expected the verified Pie64 MBR Linux root partition")
        start, count = struct.unpack_from("<II", mbr, 454)
        expected = (start + count) * 512
        missing = expected - raw.stat().st_size
        if start != 2048 or not 0 <= missing <= 64 * 1024:
            raise ValueError("Unexpected Pie64 root partition geometry")
        # QEMU's VPC geometry can omit the final sectors of this known disk.
        image.truncate(expected)
        image.seek(start * 512 + 1024 + 56)
        if image.read(2) != b"\x53\xef":
            raise ValueError("Expected an ext4 root filesystem")

    source = scratch / "source"
    target = scratch / "target"
    source.mkdir()
    target.mkdir()
    squashfs = scratch / "root.squashfs"
    mount_flags = "ro,loop,nodev,nosuid,noexec"
    subprocess.run(["mount", "-t", "ext4", "-o", f"{mount_flags},noload,offset={start * 512}",
                    str(raw), str(source)], check=True, env=HOST_ENV)
    try:
        verify_copy(source / "android/ramdisk.img", ANDROID_RAMDISK_SHA256)
        if not (source / "android/system").is_dir():
            raise ValueError("The copied root does not contain the Android system directory")
        subprocess.run(["mksquashfs", str(source), str(squashfs), "-comp", "gzip",
                        "-noappend", "-no-progress", "-processors", "2", "-mem", "512M",
                        "-xattrs", "-exit-on-error"], check=True, stdout=subprocess.DEVNULL, env=HOST_ENV)
        subprocess.run(["unsquashfs", "-s", str(squashfs)], check=True, stdout=subprocess.DEVNULL, env=HOST_ENV)
        subprocess.run(["mount", "-t", "squashfs", "-o", mount_flags, str(squashfs), str(target)],
                       check=True, env=HOST_ENV)
        try:
            changes = subprocess.check_output([
                "rsync", "-aHAX", "--dry-run", "--delete", "--itemize-changes", "--checksum",
                "--numeric-ids", f"{source}/", f"{target}/",
            ], env=HOST_ENV)
            if changes:
                raise ValueError("The read-only SquashFS did not match the copied ext4 tree")
        finally:
            subprocess.run(["umount", str(target)], check=True, env=HOST_ENV)
    finally:
        subprocess.run(["umount", str(source)], check=True, env=HOST_ENV)
    raw.unlink()
    return squashfs


def system_mount_init(init):
    replacements = (
        (b"load_module /boot/bstmods/bstinput.ko",
         b'log_echo "Pie64 lab: using standard Linux input devices"'),
        (b'log_echo "Mounting Android root file system /dev/sda1"\n\n'
         b"mount -o ro /dev/sda1 /boot/android",
         b'log_echo "Mounting Android root file system from read-only SquashFS"\n\n'
         b"mount -t squashfs -o loop,ro /boot/lab-root.squashfs /boot/android"),
        (b'log_echo "Reading cmdline"',
         b'echo "<2>Pie64 lab: mounted RAM SquashFS read-only" > /dev/kmsg\n'
         b'log_echo "Reading cmdline"'),
        (b"if [ -e /boot/android/android/system.sfs ]; then",
         b'echo "<2>Pie64 lab: original Android ramdisk extracted" > /dev/kmsg\n'
         b"if [ -e /boot/android/android/system.sfs ]; then"),
        (b"#mkdir cache", f'echo "<2>{SYSTEM_MARKER}" > /dev/kmsg\n#mkdir cache'.encode()),
        (b"populate_bst_conf\ngenerate_android_ids",
         f'echo "<2>{CONFIG_MARKER}" > /dev/kmsg\npopulate_bst_conf\ngenerate_android_ids'.encode()),
    )
    for old, new in replacements:
        if init.count(old) != 1:
            raise ValueError("Unexpected Pie64 boot init script")
        init = init.replace(old, new)
    return init


def build_system_carrier(initramfs, squashfs, scratch):
    verify_copy(initramfs, INITRAMFS_SHA256)
    original = gzip.decompress(initramfs.read_bytes())
    size = squashfs.stat().st_size
    if size + len(original) + 4096 >= 2 * 1024**3:
        raise ValueError("Uncompressed system-mount CPIO would exceed 2 GiB")
    carrier = scratch / "system-mount-initramfs.gz"
    offset = 0
    updated = False
    with carrier.open("wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", compresslevel=4, mtime=0) as output:
        while offset < len(original):
            header = original[offset:offset + 110]
            if header[:6] != b"070701":
                raise ValueError("Expected a newc Pie64 initramfs")
            fields = [int(header[6 + index * 8:14 + index * 8], 16) for index in range(13)]
            content_size, name_size = fields[6], fields[11]
            name_end = offset + 110 + name_size
            name = original[offset + 110:name_end - 1]
            content_start = (name_end + 3) & ~3
            next_offset = (content_start + content_size + 3) & ~3
            if name == b"boot/init":
                init = system_mount_init(original[content_start:content_start + content_size])
                output.write(cpio_header(name, fields, len(init)))
                output.write(init + bytes(-len(init) % 4))
                updated = True
            elif name == b"TRAILER!!!" and updated:
                file_fields = [0xA001, 0o100644, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]
                output.write(cpio_header(ROOT_MOUNT, file_fields, size))
                with squashfs.open("rb") as payload:
                    shutil.copyfileobj(payload, output, 1024 * 1024)
                output.write(bytes(-size % 4))
                output.write(original[offset:])
                break
            else:
                output.write(original[offset:next_offset])
            offset = next_offset
        else:
            raise ValueError("Missing boot/init or trailer in the Pie64 initramfs")
    if carrier.stat().st_size > 1_850_000_000:
        raise ValueError("Compressed system-mount carrier exceeds the Pie64 boot budget")
    return carrier
