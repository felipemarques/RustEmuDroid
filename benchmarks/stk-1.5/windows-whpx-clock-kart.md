# What the virtual clock fix changed in SuperTuxKart

With the test engine's Windows Qt window, enabling synchronized virtual CPU counters increased the pooled game rate from **85.45 to 87.38 FPS** (+1.93 FPS, +2.26%). This comparison uses the same Android 17 Google Play image and the same rebuilt QEMU in both arms. The three-player benchmark measured the [released Rust client separately](windows-current-three-players.md).

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
