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

An offline comparison used the vendor partition from Google's API 28 revision 4 x86-64 emulator image. Its device matrix requires Vendor NDK version 28 and lists zero individual Vendor NDK libraries. The copied Pie64 framework manifest declares System SDK 28 and has no Vendor NDK entry. Android 9's compatibility check looks for a matching Vendor NDK version even when the matrix lists zero libraries. The framework includes FCM level 3, which matches Goldfish's target level. The Pie64 framework's composer has instance name `vr`, while the Goldfish device's composer uses `default`. [Google API 28 x86-64 image](https://dl.google.com/android/repository/sys-img/android/x86_64-28_r04.zip) · [Android 9 manifest compatibility checks](https://android.googlesource.com/platform/system/libvintf/+/android-9.0.0_r61/HalManifest.cpp)

We ran the Android 9 r61 host `checkvintf` against both sets of original XML files offline. The staged tree placed the Pie64 framework metadata under `system/etc/vintf`, the Goldfish manifest at `vendor/manifest.xml` and the Goldfish device matrix at `vendor/etc/vintf/compatibility_matrix.xml`. The host build used the Pie compatibility implementation and HIDL sources, GCC 13, Ubuntu Android base/log libraries and TinyXML2. The GCC adaptations supplied missing standard C++ includes and reordered initializers without changing their values.

| Check | Observed result |
|---|---|
| Goldfish device manifest against the assembled Pie64 framework matrix | Passed in the full-tree check with `ro.product.first_api_level=28`. |
| Pie64 framework manifest against the Goldfish device matrix | Failed because the framework declares no supported Vendor NDK version 28. The original tool's direct pairwise command printed `false` and exited with status 1. |

To reproduce the two invocations with that staged tree, run

```sh
checkvintf --check-compat --rootdir="$tree" --property ro.product.first_api_level=28
checkvintf "$tree/system/etc/vintf/manifest.xml" "$tree/vendor/etc/vintf/compatibility_matrix.xml"
```

The r61 full-tree CLI logged the incompatibility on stderr but printed `true` and returned 0. Its loop checks each result yet returns a separate variable initialized to success. A build with an added diagnostic print exposed internal status 1 and the same Vendor NDK reason as the direct pairwise command. The SHA-256 values for that pair were `8e831bf90f12bb101691e06b3db1a89070abd8b69c92844d96e90d767572f0ca` (Pie64 framework manifest) and `8286655f853c49d8483f363a6a577d28a5fe166bfee0f19c2df407c6887f6fd0` (Goldfish device matrix). [Full-tree CLI return value](https://android.googlesource.com/platform/system/libvintf/+/android-9.0.0_r61/check_vintf.cpp#286) · [Order of the two checks](https://android.googlesource.com/platform/system/libvintf/+/android-9.0.0_r61/VintfObject.cpp#517)

### Private Goldfish board overlay

The official reference archive was 564,792,723 bytes and matched the Android SDK repository's SHA-1 `d47a85c8f4e9fd57df97814ad8884eeb0f3a0ef0`. Its SHA-256 was `ff6ce81aa1424951a214da5f392f8e12382de46d33c08aeaa5d21caba6a39b62`. As a control, the unmodified SDK system and vendor metadata passed both the full-tree and direct pairwise checks. [Android SDK image index](https://dl.google.com/android/repository/sys-img/android/sys-img2-3.xml)

On a private copy, we added the SDK's Vendor NDK 28 declaration to the Pie64 framework manifest and staged the matching 439 runtime libraries under `system/lib` and `system/lib64` in the `vndk-28` and `vndk-sp-28` directories. With this single declaration added, the resulting manifest matched the stock SDK framework manifest byte-for-byte (SHA-256 `44c91c8aa54bd2958cec15e05b5e80228b97229d951ee7aa5b57a61a4681587f`). The adapted Pie64 framework metadata and Goldfish vendor metadata then passed both checks, including the full check's per-tree diagnostic. The local overlay includes per-file hashes and the original, control and candidate check logs.

The Goldfish graphics allocator and composer services are x86-64 ELF executables. Fourteen graphics service and implementation binaries led to 74 distinct ELF files through their recursive `DT_NEEDED` dependencies. A static search found a file of the requested name and ABI for each edge in the Pie64 system and Goldfish vendor search directories. This checks names and architectures. Resolving versioned symbols, loading modules at runtime and presenting a frame require further tests.

The original Pie64 product sets `ro.treble.enabled=false`. Its legacy linker configuration does not search the new versioned VNDK directories. Goldfish's Android 9 linker configuration does. The R1 assembly below stages the SDK linker configuration and device policy. The static dependency check leaves native runtime loading and rendering unverified. [Android linker namespaces](https://source.android.com/docs/core/architecture/vndk/linker-namespace)

## Cold-boot the Goldfish system-as-root candidate

The R1 experiment used RustEmuDroid 0.8.0, Android Emulator 37.3.10, WHPX and the SDK's normal x86-64 Goldfish boot path. Its system image retained the 47 factory APKs and the Pie64 framework and libraries from the copied `/android/system` tree. The root outside `/system` came from the official API 28 revision 4 SDK image. The board graft added the verified VNDK payload and framework manifest, the SDK's split SELinux policy, `prop.default`, `ld.config.28.txt`, and the `/system/vendor -> /vendor` link. The original SDK vendor image remained separate and supplied the matching policy, device properties and fstab. The SDK kernel, ramdisk and factory userdata seed were also kept with the board. This test did not attach the populated BlueStacks data disk.

The new GPT disk retained the reference partition layout. Its ext4 system partition begins at 1,048,576 bytes and is 2,684,354,560 bytes long. The host ran a read-only filesystem check, then checked all 4,151 paths against the assembly records for file data or link targets, UID, GID, mode, SELinux label and capabilities. The host metadata check does not start init or validate service transitions. In the compiled SDK policy the allocator and composer have their normal transitions, while the retained `bstsvcmgrtest`, `report_daemon` and `vendor.RTVboxGuestService` entry points match the generic `system_file` type and have no automatic transition from init. None is declared critical.

| Artifact | SHA-256 |
| --- | --- |
| Official SDK archive `x86_64-28_r04.zip` | `ff6ce81aa1424951a214da5f392f8e12382de46d33c08aeaa5d21caba6a39b62` |
| Stock SDK `system.img` | `5dc8e30fadfc905b383acc76981c4a79c98f0e175bf4a20422eb1c0bcd157d9b` |
| Shared stock SDK `vendor.img` | `7e43799e0c0d4f59ac885ba3e1c4955cd3b0c118cfccd8b71f0f24db86914f62` |
| R1 `system.img` | `1073aa89548a65c2575736b69cc1123cc918091d92bb09366bb1da1561b64f6b` |
| R1 framework manifest | `44c91c8aa54bd2958cec15e05b5e80228b97229d951ee7aa5b57a61a4681587f` |

The unmodified SDK control reached `sys.boot_completed=1` in **29.5 seconds** on the RustEmuDroid clock. Guest checks reported SELinux enforcing, Launcher3 as Home, the SurfaceFlinger, input, package and activity services, multitouch axes, and NVIDIA-backed guest GLES on the RTX 5070 Ti. The R1 run ended after **183.83 seconds** of host observation without confirming Android boot. ADB listed a transport and the emulator console returned the expected AVD path. Attempts to open a guest shell repeatedly returned `error: closed`. The first run recorded host graphics initialization but could not query the guest compositor, input or launcher through the closed shell.

A separate R1 run captured guest output through the SDK's kernel console and logcat stream. SurfaceFlinger reported OpenGL ES 2.0 through the Android Emulator OpenGL ES Translator on the RTX 5070 Ti and generated a cache of 24 shaders. The guest reached hardware-backed GLES initialization. The same trace showed the initial `/data` mount, read-only errors during `post-fs-data`, `vold` reporting a missing `/fstab.pie`, and userdata `cryptfs` commands. Those events identify a board configuration discrepancy to test. The log alone does not establish which error prevented Android boot or whether it explains the closed ADB shell.

### R2 fstab compatibility test

R2 uses the validated R1 disk and adds only `/fstab.pie` at the system-as-root filesystem root. Its contents are byte-for-byte the official SDK vendor's `/etc/fstab.ranchu`. The file keeps `forceencrypt=/dev/block/vdd` for `/data` on `/dev/block/vdc`. Its owner is root, mode is `0644`, and SELinux file type is `rootfs`. The read-only ext4 check passed and an offline content and metadata comparison found that new file as the only tree change. The SDK companion files, including the separate vendor image, are the same. R2's `system.img` SHA-256 is `5ba882a7b7c3af4e021b5a14a61d1e003e6cdc17cf39e6148be76905b80b0d4d`.

The first R2 diagnostic ran for 100.0 seconds and used a fresh profile that exposed the 1 MiB SDK userdata seed directly instead of the formatted 6 GiB userdata used by the earlier runs. Android reported an invalid ext4 superblock on `/dev/block/vdc` and could not find `/misc` on its recovery path. That run did not record `cryptfs enablecrypto` or a Zygote start. The verified ADB console continued to work while shell calls returned `error: closed`. The run cannot isolate the fstab change. SurfaceFlinger again logged hardware-backed GLES.

A second R2 diagnostic used separately prepared, filesystem-checked ext4 userdata at the control's 6 GiB size; its companion encryption key image was the original blank SDK seed. With that state and the same R2 system image, `vold` mounted encrypted userdata at `/data` from `/dev/block/dm-0`. The trace then recorded successful `cryptfs init_user0`, `trigger_restart_framework`, and Zygote creating `system_server` about 22 seconds after the kernel started. SELinux audit records showed that process still in `u:r:zygote:s0` and denied access to the secondary Zygote socket and Binder services. The guest later logged a null `IInstalld` reference during system-server dexopt. Those observations locate the next failure during framework startup. They do not establish the cause of the missing domain transition. The ADB console continued to identify the AVD, while the last shell check at 99.0 seconds still returned `error: closed`. The watchdog stopped the run after 100.0 seconds without confirming Launcher or Android boot completion.

Use a dedicated state directory for each test. Keep an untouched, complete copy of the matching SDK image directory for the reference. For R1, point to a separate directory containing the exact validated R1 `system.img` and its unchanged SDK companion files. The following PowerShell commands reproduce the harness's boot arguments using an existing RustEmuDroid 0.8.0 binary. Use `init` once for each new state directory. Select the installed GPU name when running on different hardware.

```powershell
$emu = 'C:\Lab\rustemudroid.exe'
$system = 'C:\Lab\pie28-hybrid-r1'
$state = 'C:\Lab\pie28-hybrid-r1-state'
$gpu = 'NVIDIA GeForce RTX 5070 Ti'

Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $system 'system.img')
& $emu --lang en-us --state-dir $state init --system-dir $system
& $emu --lang en-us --state-dir $state boot --engine sdk --gpu host --gpu-device $gpu --headless --timeout 180
```

Repeat with the stock SDK directory and a second state directory for the control. To inspect a run, list ADB devices and use `adb -s <serial> emu avd path` to match the running transport to the expected `avd/RustEmuDroid.avd` inside that state directory. Then use the same serial with `adb shell getprop sys.boot_completed`. Only query or stop the emulator whose console path matches the test profile. Preserve the profile's `boot-*.log` and the first shell or boot-property result for comparison.

### R3 runtime work

The Windows offline check of the actual `libgui.so` dynamic symbols identified a difference relevant to assembling R3. On both x86 and x86-64 the Pie64 R2 libraries export the `ScreenshotClient::capture` overload used by the r61 SurfaceControl source. Both API 28 revision 4 control libraries export only the earlier overload. Keep the compositor and its provider compatible when assembling the next image. This symbol check does not establish that Android has booted. [AOSP r61 SurfaceControl source](https://android.googlesource.com/platform/frameworks/base/+/refs/tags/android-9.0.0_r61/core/jni/android_view_SurfaceControl.cpp)

The next candidate uses the AOSP Pie Zygote implementation and preserves the copied framework's native method signatures. An offline comparison checked 168 JNI class registration tables per runtime. Tests with Android's x86 and x86-64 linkers exercised the copied native-library helper with small APK fixtures. The fixtures confirmed ABI selection while the default namespace retained the SDK runtime's SELinux binding. These checks ran outside a booted Android system.

The stock AOSP build of `android-9.0.0_r61` produced x86 and x86-64 platform runtimes from manifest commit `3564dc0e96454f300e34543781448fa4185682c5`. A Windows replay of both stock GNU gold link commands reproduced the pre-strip ELFs byte for byte. Two privately linked runtimes retained the copied framework's JNI entry points. Each resolved 2,604 strong imports in the selected library set. [Public AOSP build and artifacts](https://github.com/felipemarques/RustEmuDroid/actions/runs/45449546204)

**Independent Windows boot tests.** Each candidate started from a separate copy of the R2 disk and a fresh 6 GiB userdata fixture. We read back the deployed files, compared their SHA-256 hashes, checked ownership, modes and SELinux labels, validated the GPT and ran a read-only ext4 check before boot. The runs used Windows hardware acceleration and the RTX 5070 Ti. The kernel reported SELinux enforcing.

| System image | Boot evidence | Last confirmed milestone |
|---|---|---|
| R3 with SDK Bionic, 214 files applied | `vold` received SIGSEGV, userdata failed to initialize and the Dalvik cache directory was unavailable. | The emulator console identified the test AVD. The guest ADB shell remained closed during 180 s of observation. |
| R3 with Pie64 Bionic, 212 files applied | Keeping the original 32-bit and 64-bit `libc.so` recovered the encrypted mount on `dm-0` at 20.2 s. Zygote then started and exited with SIGABRT. | Both Zygotes repeatedly restarted. At 150 s the guest shell was still closed. A separate logcat run on the same image traced the abort to AssetManager initialization calling SystemProperties before its native methods were registered. |

The third candidate (R3/C) moved the SystemProperties registration ahead of AssetManager on both architectures. The other 155 registrars kept their relative order. Android then created the system server. The SELinux audit reported its expected domain, `u:r:system_server:s0`, with enforcement active. During bootstrap the process tried to publish the Pie64 service `bstfilterapps`. The board had no service-specific label for this name, so the service manager rejected its registration and SystemServer restarted. Guest SurfaceFlinger identified the RTX 5070 Ti through the Android Emulator OpenGL ES Translator.

The independent image still needs a complete Android boot, working shell, Home, touch and an installed game running inside RustEmuDroid. The Windows BlueStacks integration at the top of this page continues to launch the installed BlueStacks player.

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

### Populated data disk

The private copy of the populated `Data.vhdx` also passed an offline filesystem check. Both it and the factory seed had a single Linux MBR partition starting at sector 2,048 and spanning 268,433,408 sectors. The populated partition's ext4 superblock reported 33,554,176 blocks of 4,096 bytes, which matches that partition's length.

QEMU 8.2.2 exported only the populated partition through a read-only FUSE file in an existing Ubuntu 24.04 session. `e2fsck 1.47.0 -fn` completed all five passes and returned status 0. It reported 17,152 allocated inodes and 3,150,138 allocated blocks (12.02 GiB). The check covered filesystem metadata, including directory connectivity. The ext4 filesystem stayed unmounted. Application launch, Android boot and the content of individual saves require separate verification.
