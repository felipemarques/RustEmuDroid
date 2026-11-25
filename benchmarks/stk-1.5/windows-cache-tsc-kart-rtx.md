# WHPX clock synchronization with Vulkan caching held on

With Vulkan caching enabled throughout, the clock fix reduced QEMU CPU consumption from **2,188 to 1,778 ms per second** (-18.7%) on this Windows PC. Black Forest stayed around **90 game FPS and 90 Android presentations/s**. In this run, enabling the clock fix did not increase game or panel frame rates. [Per-replay data and Android presentation timestamps](windows-cache-tsc-kart-rtx.json)

## Four cold boots on the RTX 5070 Ti

| Replay | TSC synchronization | Game frames / time | Game FPS | Android presentations/s | Android p99, ms | Panel processed frames/s |
|---|---|---:|---:|---:|---:|---:|
| A1 | Off | 3,432 / 38.055 s | 90.19 | 90.00 | 14.42 | 88.37 |
| B1 | On | 3,432 / 38.126 s | 90.02 | 89.96 | 14.33 | 87.66 |
| B2 | On | 3,433 / 38.136 s | 90.02 | 90.00 | 14.21 | 86.95 |
| A2 | Off | 3,431 / 38.060 s | 90.15 | 90.02 | 14.16 | 87.23 |

| Pooled result | TSC off | TSC on |
|---|---:|---:|
| Game FPS | 90.17 | 90.02 |
| Android presentations/s | 90.01 | 89.98 |
| Panel processed frames/s | 87.80 | 87.30 |
| Android intervals over 1.5 display periods | 1 / 3,602 | 1 / 3,601 |
| Windows QEMU CPU, ms/s | 2,187.65 | 1,778.04 |
| Windows Rust presenter CPU, ms/s | 250.72 | 258.32 |

The game rate divides the total frame count by the total replay duration. Android's SurfaceView samples cover about 20 seconds per replay. P99 uses the nearest-rank interval in each sample. The panel counts distinct frames processed for drawing, at its own sampling interval. Windows CPU time sums threads per process, measured over the Android observation window. GPU time, desktop compositor work and physical display cadence are outside those measurements. [Game CSVs, per-frame CPU traces and writes](windows-cache-tsc-kart-rtx-traces.zip)

All four launches used the same experimental SDK 37.3.10 QEMU and stock gfxstream backend, the published RustEmuDroid 0.21.1 window, Android 17 Google Play r7 x86-64 and WHPX with four vCPUs. The guest was 1600 x 900 at 90 Hz and the panel was fitted to 800 x 450 physical pixels. The same x86-64 SuperTuxKart 1.5 APK replayed Black Forest with 21 matching High settings and Vulkan FIFO. Each cold boot had one excluded warm-up. The full APK and engine digests are in the JSON.

## Independent checks

The same cached-allocation marker was present exactly once in every closed engine log. The guest advertised type 2, heap 1 and flags 0x6 in all four runs. The separate mapped-write probe completed outside each timed replay. Its median elapsed type 2 writes took 0.058-0.074 ms for a 4 MiB batch of copies and 0.198-0.277 ms for a batch of 32-bit stores. [Earlier matched cache-only replays](windows-cached-memory-kart.md) and [Vulkan write tests](windows-vulkan-mapped-writes.md) establish what changed when cache was the variable.

Only the B boots reported TSC synchronization at init and reset. The independent ordered vCPU probe passed on both. Its observed offset bound relative to vCPU 0 was 1.89-1.94 s with the option off and 0.334-0.403 ms with it on. Thread CPU for the libc gettimeofday call was 9.32-9.68 microseconds off and 18.1-19.8 ns on. The normal Rust launch did not expose the guest's selected clocksource through the unprivileged shell. These runs therefore report the marker and measured clock behavior. The separate clock boot traces document HPET and TSC selection directly. [Clock patch and cold-boot qualification](whpx-tsc-sync.md#android-17-cold-boots-on-the-same-executable)

This is a local two-replay-per-setting comparison at a 90 Hz display with FIFO presentation. It does not estimate the benefit at an untested refresh rate, uninstrumented gameplay, or another game. The QA APK and clock/write probes were removed and the normal 60 Hz Android Home was restored after the series.
