# Windows memory at Android Home

This benchmark samples the Windows processes hosting the Android Home screen. Use **Working set** to see how many bytes of pageable memory are resident for a process at that instant, including shared pages. **Private commit** counts private memory committed to the process, including pages that may be outside its working set. Guest RAM in the emulator settings is a third quantity. [Windows working set](https://learn.microsoft.com/en-us/windows/win32/memory/working-set) · [Windows process memory counters](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex)

## Android Home samples

On one Windows 11 Pro PC with a Ryzen 9 8945HS and RTX 5070 Ti, we ran one VM at a time. Each guest had **8 GiB (8,192 MiB)** configured. We waited two seconds after verifying Home and took three process readings one second apart. The table reports the median for each counter. [All 15 process readings in bytes](home-samples.csv)

| Player and Android version | Windows process | Working set | Private commit |
| --- | --- | ---: | ---: |
| BlueStacks 5.22.166.1003 Pie64 · Android 9 | `HD-Player.exe` | 1,709.94 MiB (1.670 GiB) | 10,247.56 MiB (10.007 GiB) |
| Kemu 0.7.1 · Android 16 | `qemu-system-x86_64.exe` | 3,113.11 MiB (3.040 GiB) | 9,508.62 MiB (9.286 GiB) |
| RustEmuDroid 0.21.1 · Android 17 | `qemu-system-x86_64.exe` | 5,864.61 MiB (5.727 GiB) | 9,766.55 MiB (9.538 GiB) |
| RustEmuDroid 0.21.1 · Android 17 | `rustemudroid.exe` | 118.31 MiB (0.116 GiB) | 126.88 MiB (0.124 GiB) |
| RustEmuDroid 0.21.1 · Android 17 | `emulator.exe` | 11.85 MiB (0.012 GiB) | 10.78 MiB (0.011 GiB) |

| Player | Android Home | Guest uptime around the samples | Display observed |
| --- | --- | ---: | --- |
| BlueStacks Pie64 | BlueStacks Home (`com.uncube.launcher3`) | 10.65–12.89 s | Unrecorded for this run |
| Kemu | Quickstep (`com.android.launcher3`) | 15.79–17.91 s | 1600 × 900 at 90 Hz. Android reported a 45 Hz render cadence. |
| RustEmuDroid | RustEmuDroid Home (`org.rustemudroid.home`) | 59.41–61.69 s | 1600 × 900 at 60 Hz |

The BlueStacks row covers its official player without the Rust preview. The Kemu launcher exited after starting the QEMU process that hosts its window. RustEmuDroid ran the published Windows build with its integrated display. The median sum of private commit across its three selected processes was 9,904.21 MiB (9.672 GiB). Working sets can count the same shared pages in multiple processes.

These snapshots cover different Android versions, Home apps and times since boot. The first BlueStacks working-set sample was 1,614.46 MiB and the last was 1,716.37 MiB. Machine-wide comparisons also need memory attributed to other services, the hypervisor, drivers and the GPU. [RustEmuDroid v0.21.1](https://github.com/felipemarques/RustEmuDroid/releases/tag/v0.21.1)

## Repeat the measurement

Start one player, let Android finish booting and leave its Home visible. In Windows Task Manager, open **Details** and note the PID of the VM process. BlueStacks Pie64 uses `HD-Player.exe`. In the tested Kemu installation the boot launcher exits, leaving `qemu-system-x86_64.exe` to host both Android and the window. With the native RustEmuDroid Windows player, record the PIDs of `rustemudroid.exe`, its `emulator.exe` child and the QEMU process under that child. You can inspect their parent PIDs in PowerShell.

```powershell
Get-CimInstance Win32_Process |
    Where-Object Name -In 'HD-Player.exe', 'qemu-system-x86_64.exe', 'emulator.exe', 'rustemudroid.exe' |
    Select-Object Name, ProcessId, ParentProcessId
python -m pip install psutil
$vmProcessId = [int](Read-Host 'VM process ID')
python -B benchmarks/windows-home-memory/sample.py --pid $vmProcessId
```

Pass additional PIDs after `--pid` to sample several processes in the same interval. The default takes three readings one second apart. Divide the byte values by 1,048,576 for MiB or 1,073,741,824 for GiB. The sampler checks that each selected PID still belongs to its original process. For comparisons over time, also record the Android Home package and guest uptime from `/proc/uptime` through the selected guest's ADB transport.
