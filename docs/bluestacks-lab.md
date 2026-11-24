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

The experiments demonstrate storage attachment, loader execution, independent kernel startup and entry into the original initramfs. QMP confirms that QEMU attached the VirtIO devices. Guest disk discovery, Android partition mounts, Android boot completion, graphics, input and games remain unverified. The external Windows integration is the working way to open the Pie64 games and saves. [Linux x86 boot protocol](https://docs.kernel.org/arch/x86/boot.html) · [QEMU invocation](https://www.qemu.org/docs/master/system/invocation.html)
