# What the virtual clock fix changed in SuperTuxKart

With the published **RustEmuDroid 0.21.1 window** and an experimental Windows QEMU, synchronizing the virtual CPU clocks increased the pooled game rate from **85.65 to 87.46 FPS (+2.11%)**. In the engine's separate Qt window the game improved from **85.45 to 87.38 FPS (+2.26%)**. Each frontend had its own four cold boots on the same Android 17 Google Play image and host GPU. The [current three-player comparison](windows-current-three-players.md) records the stock runtime at 85.62 FPS, Kemu at 90.02 and BlueStacks at 89.99.

## RustEmuDroid window, RTX 5070 Ti

| Cold boot, in order | Game frames / time | Game FPS | Android presentations/s | Rust panel frames/s | Android p99 |
|---|---:|---:|---:|---:|---:|
| Disabled A1 | 3,255 / 38.071 s | 85.50 | 85.57 | 82.59 | 23.13 ms |
| Enabled B1 | 3,333 / 38.137 s | 87.40 | 87.16 | 85.87 | 22.93 ms |
| Enabled B2 | 3,337 / 38.124 s | 87.53 | 87.31 | 84.78 | 22.64 ms |
| Disabled A2 | 3,267 / 38.074 s | 85.81 | 85.67 | 83.09 | 23.08 ms |

The game rendered at 1600 × 900 with the High Vulkan preset, FIFO presentation and a nominal 90 Hz display. The release Rust executable fitted the video into an 800 × 450 surface. Across the qualified observation windows, Android presentations rose from **85.62 to 87.24/s** and the Rust panel processed **82.84 to 85.32 frames/s**. Windows CPU time for QEMU and the Rust presenter together fell from **2,558 to 2,249 ms per second (−12.1%)**. GPU and desktop compositor time are outside those CPU totals. [Measurements and presentation timestamps](windows-whpx-clock-kart-rust.json) · [Original game reports and per-frame traces](windows-whpx-clock-kart-rust-traces.zip)

Intervals over 1.5 refresh periods accounted for **174/3,426 (5.08%)** of Android intervals disabled and **111/3,491 (3.18%)** enabled. Over the full game replays the main loop crossed the same threshold **197/6,522** times disabled and **58/6,670** times enabled. In the two [previously identified track sections](windows-draw-all-android17.md#where-the-slow-frames-recur-on-the-track), 7–9 and 29–31 seconds on the replay clock, that count fell from **90/537 to 15/600**. The game and Android counters cover different time windows.

Each boot got a separate warm-up. We checked the running binaries, the loaded gfxstream DLL, Android identity, clock behavior and the signed x86-64 game APK. The Rust window retained its normal launch arguments. The independent Qt boot logs confirmed Linux's clock selection, and the Rust runs checked cross-CPU ordering before gameplay and the QEMU synchronization messages at shutdown. The habitual release returned to Android Home at 60 Hz with one VM after the run.

## Qt window, RTX 5070 Ti

| Cold boot, in order | Game frames / time | Game FPS | Android presentations/s | Android p99 | Android intervals over 16.667 ms¹ |
|---|---:|---:|---:|---:|---:|
| Disabled A1 | 3,262 / 38.065 s | 85.70 | 85.91 | 23.10 ms | 82 / 1,719 |
| Enabled B1 | 3,331 / 38.126 s | 87.37 | 86.82 | 22.74 ms | 67 / 1,737 |
| Enabled B2 | 3,333 / 38.137 s | 87.40 | 87.36 | 22.55 ms | 52 / 1,748 |
| Disabled A2 | 3,243 / 38.062 s | 85.20 | 85.05 | 23.47 ms | 100 / 1,702 |

¹ The comparison uses 1.5 nominal refresh periods before rounding. Android presented the foreground game surface with long gaps in **5.32%** of measured intervals with the option disabled and **3.41%** with it enabled. On the game's full-replay timeline, **264 of 6,505** main-loop intervals exceeded the threshold while disabled, compared with **103 of 6,664** while enabled. Each Android sample covers about 20 seconds inside a roughly 38-second replay. [Per-lap measurements and timestamps](windows-whpx-clock-kart-qt.json) · [Original game reports and frame traces](windows-whpx-clock-kart-qt-traces.zip)

Across those Android observation windows, presentation rate rose from **85.48 to 87.09 frames/s**. Windows measured **2,102 to 1,784 ms of CPU time per second** in the QEMU process that also hosted Qt, a **15.1% decrease**. Windows sums CPU time across that process's threads. GPU time and the desktop compositor are outside this figure.

Each arm booted from cold, verified the virtual CPU clocks, ran one excluded warm-up and then measured one Black Forest replay. All four boots used WHPX, four virtual CPUs, the same x86-64 APK, the same 21 High graphics settings, Vulkan FIFO, Android at 1600 × 900 and 90 Hz, and a visible 800 × 450 Qt game area. We verified the loaded QEMU and gfxstream DLL. The graphics-memory experiment was disabled throughout. The game FPS above divides total frames by total replay duration across the two runs per condition.

The four measured boots all selected the expected Linux clock, HPET when disabled and TSC when enabled. Two runs per setting on this Windows PC support a local comparison. The enabled Qt runs still finished below 90 game FPS. [Clock patch, build and guest validation](whpx-tsc-sync.md#android-17-cold-boots-on-the-same-executable)

The benchmark APK records detailed game timing with clock reads of its own. Both arms used identical instrumentation. Android’s independent presentation timestamps also improved. The exact gain during a normal uninstrumented race, or in another game such as PUBG, needs its own measurement. [Earlier comparison with only frame totals enabled](windows-profiler-overhead.md)
