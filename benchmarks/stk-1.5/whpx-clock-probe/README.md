# WHPX TSC write synchronization on the real platform

The probe used Windows 10.0.26300, `WinHvPlatform.dll` 10.0.26100.9818 and the Windows SDK 10.0.26100.0. It uses only one private WHP partition at a time. Each process creates four virtual processors and maps one 4 KiB executable page containing `RDTSC; HLT`. The harness checks the HLT exit and gets the guest result from EDX:EAX. No virtual disk, network, interrupt handler, Android image or emulator process is needed.

Build and run from the repository root with Visual Studio C++ tools, the Windows SDK and Python. Choose fresh output paths for each measurement.

```powershell
cmd /d /c benchmarks\stk-1.5\whpx-clock-probe\build.cmd .rustemudroid\qa\whpx-clock-probe-build
python -B benchmarks/stk-1.5/whpx-clock-probe/run.py .rustemudroid/qa/whpx-clock-probe-result --binary .rustemudroid/qa/whpx-clock-probe-build/probe.exe --auto-resume-proof
```

Each process runs and parks the four VPs, waits, and verifies that their clocks are already advancing while parked. It then writes the identical TSC value to VP0 through VP3 with a nominal 20 ms sleep between writes. The actual write span is taken from QPC. A keeps partition time running. B suspends it while all VPs are parked, performs the same writes and resumes it with `WHvResumePartitionTime`. The supplemental B pass lets the next `WHvRunVirtualProcessor` resume partition time, as the QEMU candidate does. It performs no explicit resume call. The harness compares guest reads immediately after resume and again after an 80 ms sleep. [WHP suspend and automatic resume contract](https://learn.microsoft.com/en-us/virtualization/api/hypervisor-platform/funcs/whvsuspendpartitiontime) · [Explicit resume API](https://learn.microsoft.com/en-us/virtualization/api/hypervisor-platform/funcs/whvresumepartitiontime)

| Pass | Condition | First-to-last write span | Four guest TSCs, offset spread on the delayed read |
| --- | --- | ---: | ---: |
| 1 | A, running | 92.026 ms | 92.009–92.067 ms |
| 2 | B, explicit resume | 93.832 ms | 0–49.4 µs |
| 3 | B, explicit resume | 93.833 ms | 0–27.2 µs |
| 4 | A, running | 95.671 ms | 95.653–95.713 ms |
| 5 | B, automatic resume | 92.771 ms | 0–46.1 µs |

In every suspended arm, `WHvGetVirtualProcessorRegisters` returned the exact written value for all four VPs at both post-write parked samples, separated by 30.9–32.0 ms. Each control VP advanced by about 123–129 million ticks during its parked interval. After resumption the guest TSCs advanced with per-VP TSC/QPC ratio bounds spanning 0.999421–1.000484 across the suspended runs, normalized to the WHP-reported 3,992,589,302 Hz. Both early and delayed B observations bounded the spread below 53 µs. The lower bound includes zero. This is resolution-limited alignment, not a claim that the resumed counters have mathematically identical offsets.

In the automatic-resume pass, VP0 read 1,099,511,627,776 ticks in both parked samples, 1,099,511,713,336 in its first guest run and 1,099,880,947,136 in its second. The receipt preserves the equivalent observations for the other three VPs and the QPC brackets relative to the first guest run.

QPC at 10 MHz brackets each register read or guest execution. For each VP, the reported intercept interval subtracts that host bracket from TSC expressed in nanoseconds. The result bounds the spread across the four VPs without assuming that the read occurred at the bracket midpoint. The fitted offset spread in the control tracked the deliberately staggered writes. The process exits only after each of its four VPs and its partition are deleted. The live RustEmuDroid/QEMU process pair remained running.

This isolates the WHP primitive on this host, including the automatic time resumption used by the candidate. The full QEMU reset/locking path, incoming snapshots, Android's clocksource selection and FPS require separate tests. [Measured ticks, clock bounds and SHA-256 receipt](windows-sample.json)
