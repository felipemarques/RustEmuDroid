# BlueStacks integration and boot experiment

## Launch an installed game on Windows

The RustEmuDroid panel can open an existing BlueStacks 5 instance. Pick **Open BlueStacks**, choose an instance and optionally enter an Android package such as `org.supertuxkart.stk`. BlueStacks opens its own window and uses the apps and saves in that instance. The integration reads the Windows installation entry and the instance names from `bluestacks.conf`. It dispatches the same player arguments as BlueStacks game shortcuts. [BlueStacks desktop shortcuts](https://support.bluestacks.com/hc/en-us/articles/4402959142541-How-to-create-desktop-shortcut-for-a-specific-instance-on-BlueStacks-5)

To run this from Windows PowerShell, first select the executable for your release ZIP or source checkout and list the instance IDs. The [Windows walkthrough](../README.md#open-an-installed-bluestacks-instance-on-windows) shows both paths and the expected output. After setting `$emu` in that walkthrough, run the following command in the same window to open an installed SuperTuxKart game in the example instance.

```powershell
& $emu bluestacks open --instance Pie64 --package org.supertuxkart.stk
```

The test on BlueStacks 5.22.166.1003 discovered `Pie64` and confirmed that SuperTuxKart connected to the graphics engine after the last command. The launch command reports dispatch. BlueStacks handles game availability and launch errors in its own window.

## Boot the copied disks in an isolated QEMU experiment

The Pie64 instance uses BIOS boot, an ICH9 chipset, a PIIX3 IDE controller and an AHCI controller. Its instance definition assigns the disks as follows.

| Disk | QEMU image format | Attachment |
|---|---|---|
| `fastboot.vdi` | VDI | Primary master IDE. Contains the x86 boot sector, Linux kernel and initramfs. |
| `Root.vhd` | VPC | AHCI port 0. Contains the Android system. |
| `Data.vhdx` | VHDX | AHCI port 1. Contains the writable app data. |

Shut down the instance and copy all three disks to a separate private directory. Keep the copies and outputs under the ignored `.rustemudroid/` tree. The runner attaches temporary QCOW2 overlays backed by those copies. It uses a local monitor socket, disables all guest networking and deletes the overlays after the run. [BlueStacks backup documentation](https://support.bluestacks.com/hc/en-us/articles/360059480692-How-to-backup-and-restore-data-on-BlueStacks-5) · [QEMU disk images](https://www.qemu.org/docs/master/system/images.html)

On Windows, run `powershell -ExecutionPolicy Bypass -File .\tools\Test-BlueStacksImage.ps1` from the repository root. The helper uses WSL Ubuntu 24.04 and checks the three offline copies before running the original kernel and initramfs with VirtIO block devices. Use `-LegacyStorage` for the direct kernel test with IDE and AHCI. If `Pie64/manifest.json` is present, the helper checks file sizes against the manifest. Use `-VerifyHashes` to also verify the SHA-256 hashes. The helper prints the evidence paths for each run.

With Python 3 and QEMU for x86-64 installed on Linux or WSL, run these commands from the repository root. The IDE remapping experiment also needs GNU Binutils for `as` and `objcopy`.

```sh
lab=.rustemudroid/bluestacks-lab
python3 tools/bluestacks_lab.py --images "$lab/Pie64" --output "$lab/control"
python3 tools/bluestacks_lab.py --images "$lab/Pie64" --output "$lab/remapped" --remap-ide-dma
python3 tools/bluestacks_lab.py --images "$lab/Pie64" --output "$lab/direct-linux" --direct-linux
python3 tools/bluestacks_lab.py --images "$lab/Pie64" --output "$lab/direct-virtio" --direct-linux --virtio-blk
```

Each invocation needs a new output directory. QMP records the device tree and block mappings before boot. Check `serial.log` for the last guest message and `result.json` for the QEMU exit code and whether the full observation window elapsed. If QEMU is still running at the end of the window, the probe also collects final PCI mappings, CPU registers and a screen dump. A successful probe exit means evidence was collected.

| Experiment | Observed on QEMU 8.2.2 with SeaBIOS |
|---|---|
| Control | QMP identified all three overlays and their original formats. The BIOS ran the fastboot sector. The CPU remained in a loop polling IDE DMA status at I/O port `0xd042`. |
| IDE DMA relocation | A small SeaBIOS option ROM assigned the PIIX3 bus-master registers to `0xd040`. QMP verified the resource address. The loader passed the DMA loop and reached Linux x86 real-mode setup. |
| Direct kernel diagnostic | QEMU loaded the kernel and initramfs from the copied boot disk through its Linux boot protocol. Linux 4.19.195 logged early output on the serial console and reached 64-bit execution. TCG and KVM both reported a watchdog soft lockup in SCSI error handling. TCG with `libata.dma=0 libata.force=noncq pci=nomsi` reached the same point. |
| Q35 built-in AHCI | Attaching all three copies to the built-in AHCI controller also produced a SCSI error-handler soft lockup at about 29 seconds. |
| Direct kernel with VirtIO block | With SATA and PIIX3 absent, the original initramfs reached userspace and attempted to load the BlueStacks input module. The guest logged a null pointer fault in `bstvmsg` during `bstinput` initialization, then panicked when PID 1 exited. QEMU stopped when the guest requested a reboot. |

These modes demonstrate storage attachment, loader execution, independent kernel startup and entry into the original initramfs. QMP confirms that QEMU attached the VirtIO devices. Guest disk discovery, disk-backed Android partition mounts, Android boot completion, graphics, input and games remain unverified. The external Windows integration is the working way to open the Pie64 games and saves. [Linux x86 boot protocol](https://docs.kernel.org/arch/x86/boot.html) · [QEMU invocation](https://www.qemu.org/docs/master/system/invocation.html)

## Opt-in system mount

This diagnostic uses the verified Pie64 `fastboot.vdi` and `Root.vhd` copies. The tool matches both SHA-256 hashes against the pair used in the lab. The data disk is optional for this mode and remains outside the VM.

The Linux host or WSL distribution needs Python 3.11 or newer, QEMU for x86-64, `qemu-img`, `squashfs-tools`, `rsync`, loop-mounted ext4 and SquashFS support, and 8 GiB of free temporary filesystem space. The filesystem preparation needs root. The disk converter runs as the non-root user, and the tool hash-checks its output in a separate private directory before mounting it. The ext4 mount is read-only with journal replay disabled. The tool compares the SquashFS to the copied ext4 tree by checksum and metadata, unmounts both, deletes the intermediate raw copy, and drops privileges before starting QEMU.

On Windows, the PowerShell helper performs preparation as WSL root and runs QEMU as the distribution's non-root default user.

```powershell
powershell -ExecutionPolicy Bypass -File .\tools\Test-BlueStacksImage.ps1 -SystemMount
```

On Linux, run the following from the repository root. QEMU runs as the caller of `sudo`. If invoking from a root shell, supply the intended non-root user with `--run-as`.

```sh
lab=.rustemudroid/bluestacks-lab
sudo python3 -B tools/bluestacks_lab.py --images "$lab/Pie64" --output "$lab/system-mount" --system-mount
```

The guest has no network or attached disks. The temporary boot script omits the proprietary `bstinput` load, leaves the standard Linux input devices active, and loop-mounts the complete copied root as read-only SquashFS. The original Android ramdisk and later BlueStacks initialization and configuration checks run as shipped. QEMU mounted the copied root, extracted the original Android ramdisk, and attached the original Android system directory. The original configuration then reported `ERROR: android_id is empty` and the guest stopped before Android completed startup. `system-mount.json` describes the staged image and `result.json` records the observed milestones. The temporary carrier is removed when the diagnostic exits.

### What the image still needs from its host

In the tested Pie64 ramdisk, the configuration reads run before its explicit VirtualBox guest and shared-folder driver loads. The boot scripts also reference BlueStacks-specific message, input, audio, graphics and camera modules. A later data check expects a partition at `/dev/block/sdb1`. That check was identified in the scripts and was never reached in this run.

The stock `boot/bstsetconf.sh` invokes `/boot/bin/bstconf` and writes the properties it receives to `/.bstconf.prop`. That file is an output of this step. REA identified the helper as an ELF executable. It is byte-for-byte identical to the copy under `android/system/out_bstconf/bstconf` in the mounted root (40,600 bytes, SHA-256 `1c0190542063939a60f74534ec0a464fe516981a8e7cee64541674b4a54bcb56`). Its strings reference the BlueStacks message device `/dev/bstvmsg` and failures to initialize the host-call library and fetch instance properties. These static observations point to a dependency on the vendor's host service. A successful exchange with that service in an independent VM remains unverified.

A matching host configuration interface and working guest devices are still needed to demonstrate a full boot and play a game. The public VirtualBox guest-properties documentation describes its own Guest Additions interface. The BlueStacks recovery procedure restores an offline `Data.vhdx` into the same version of its App Player. Neither document specifies the early Pie64 host interface used here. [VirtualBox Guest Additions](https://docs.oracle.com/en/virtualization/virtualbox/7.0/user/guestadditions.html) · [BlueStacks backup and restore](https://support.bluestacks.com/hc/en-us/articles/360059480692-How-to-backup-and-restore-data-on-BlueStacks-5)

For the tested Windows build 5.22.166.1003, the installed `LICENSE.txt` beside `HD-Player.exe` describes source availability for modified open-source components and includes a separate notice for the VirtualBox base package. The notice does not identify a standalone host implementation for the Pie64 configuration service. The experiment still requires a matching service and graphics devices to reach Android Home.

### Display hardware available to the copied kernel

Two diskless QEMU 8.2.2 runs started the original Pie64 Linux 4.19.195 kernel with a temporary BusyBox initramfs and no networking. The guest checked the PCI driver, VirtIO function driver and Linux device registrations in sysfs. Both runs reached the final diagnostic marker.

| Display device at PCI 00:02.0 | Observed inside the guest |
|---|---|
| VirtIO GPU (`1af4:1050`), 1024 × 768 requested | The PCI transport bound to `virtio-pci`. VirtIO function `0x0010` had no graphics driver. The kernel registered neither a DRM card nor a framebuffer. |
| Standard VGA (`1234:1111`), VESA mode requested | The kernel registered `fb0` as VESA VGA at 1024 × 768, 16 bits per pixel. It registered no DRM card or KMS connector. |

The copied Android userspace includes a DRM framebuffer composer that references `/dev/dri/card0` and links to `libdrm`. Its service is marked `disabled` in the supplied init file. The factory gralloc libraries, including the variants named `default` and `android_x86_64`, depend on the graphics encoders supplied with the image. The VGA result proves that this kernel can expose a basic framebuffer. Accelerated Android composition and game rendering still require a working path through those userspace components.

The stock system’s `lib/modules/4.19.195/modules.builtin` lists 480 entries, including the generic framebuffer and its drawing helpers. Neither this list, the eight entries in `modules.dep`, nor the modules found in the system and boot ramdisks identify a driver for VMware SVGA, Bochs DRM, VBoxVideo, QXL, Cirrus DRM, VirtIO GPU or i915. The ramdisk’s `bstpgaipc` module declares PCI ID `1af4:10f0`, while the tested QEMU VirtIO GPU advertised `1af4:1050`. The original kernel contains no extractable `IKCONFIG` record to confirm individual build settings. On this evidence, changing the QEMU display model alone has no verified driver path to the supplied Android compositor.

### Could the Android 9 SDK supply the missing drivers?

Pie64 and the AOSP Pie emulator both target API 28 on x86-64. Their graphics components expect different interfaces. The copied Pie64 system declares `ro.treble.enabled=false`, stores its vendor files in `/system/vendor`, and uses a non-isolated linker namespace. Its device manifest declares the graphics allocator 2.0 as `passthrough` (loaded in the caller), and its framework manifest declares the composer 2.1 service. AOSP’s Pie Goldfish device manifest declares the allocator as `hwbinder` (a separate service) and puts the composer in the device manifest. The AOSP x86-64 board also specifies a separate vendor image and VNDK. [AOSP Pie Goldfish manifest](https://android.googlesource.com/device/generic/goldfish/+/refs/tags/android-9.0.0_r61/manifest.xml) · [AOSP Pie x86-64 board configuration](https://android.googlesource.com/platform/build/+/refs/tags/android-9.0.0_r61/target/board/generic_x86_64/BoardConfig.mk)

Keeping the copied Android framework while changing boards would require integrating the graphics services, their native library dependencies and device policy. This comparison inspected manifests, properties and ELF dependencies offline. The comparison did not run `checkvintf` or boot a mixed image. Installing the complete SDK image would test a different Android system.

## Opt-in data storage probe

Use an offline copy of the Pie64 `Data_orig.vhdx` alongside `fastboot.vdi`. The probe checks both files against the SHA-256 hashes of the verified fixtures. The regular `Data.vhdx` and Android root are outside this probe.

```powershell
powershell -ExecutionPolicy Bypass -File .\tools\Test-BlueStacksData.ps1 -Images 'C:\Lab\Pie64'
```

On Windows, the script uses WSL Ubuntu 24.04 by default. Install the QEMU system and image tools in that distribution, or point `-Images` at the existing lab folder whose parent contains `qemu-tools`. Set `-Output` to choose a new evidence directory. With `-VerifyHashes`, the script also verifies both copies against the adjacent `manifest.json`. The checkout's existing `Test-BlueStacksImage.ps1 -DataProbe` uses this same entry point.

On Linux or WSL with Python 3.11 or later and QEMU for x86-64 and `qemu-img` installed, run from the repository root:

```sh
lab=.rustemudroid/bluestacks-lab
python3 -B tools/bluestacks_data_probe.py --images "$lab/Pie64" --output "$lab/data-probe"
```

The original Pie64 kernel starts a small initramfs with BusyBox and a storage probe. QEMU attaches the seed through a read-only QCOW2 overlay and uses an empty first disk so that the seed can appear as Linux `/dev/sdb1`. The probe disables networking and checks that both QEMU disks are read-only.

On QEMU 8.2.2, the original kernel recognized the VirtIO SCSI controller and the seed's MBR partition. The guest confirmed the partition was read-only, read the ext4 superblock signature, and mounted ext4 at `/data` with `ro,noload`. It verified the read-only mount through `/proc/mounts` and unmounted it. The probe reads filesystem metadata and never browses the mounted tree. The command exits successfully when QEMU exits successfully and the partition, ext4 signature, and read-only mount are all observed. See `serial.log` and `result.json` for the observed milestones, including incomplete runs. The separate system-mount test still stops at the original BlueStacks host configuration.
