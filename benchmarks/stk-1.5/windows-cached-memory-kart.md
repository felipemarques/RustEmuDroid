# Cached Vulkan memory in SuperTuxKart on Windows

The experimental SDK engine with cached Vulkan allocations averaged **90.19 game FPS**, up from **85.56 FPS (+5.41%)** with the option disabled in the same binary. The release RustEmuDroid 0.21.1 window ran the four cold boots on the RTX 5070 Ti. The separate clock synchronization option stayed off. The stock three-player test on this PC measured **85.62 FPS** on RustEmuDroid, **90.02** on Kemu and **89.99** on BlueStacks. This brings the experimental Android 17 setup to the same 90 Hz target in this replay. [Stock runs and conditions](windows-current-three-players.md)

| Run | Cache | Game frames / time | Game FPS | Android presentations/s | Android p99 | Rust panel frames/s |
|---|---|---:|---:|---:|---:|---:|
| A1 | Off | 3,257 / 38.072 s | 85.55 | 84.57 | 23.10 ms | 82.70 |
| B1 | On | 3,432 / 38.055 s | 90.19 | 90.01 | 14.63 ms | 87.51 |
| B2 | On | 3,433 / 38.065 s | 90.19 | 89.96 | 14.53 ms | 86.78 |
| A2 | Off | 3,258 / 38.074 s | 85.57 | 85.16 | 22.92 ms | 83.49 |

Each boot used the same Windows QEMU, verified gfxstream DLL, Android 17 Google Play r7 image, signed x86-64 game APK, 1600 × 900 High Vulkan preset, FIFO presentation, four vCPUs and 800 × 450 Rust viewport. The game completed a warm-up before each measured Black Forest replay. The game rate pools frames over the full replay durations. The Android and panel counters sample separate stages for about 20 seconds. [Measurements, frame counts and timestamps](windows-cached-memory-kart.json) · [Original game reports and per-frame traces](windows-cached-memory-kart-traces.zip)

Across the qualified Android samples, **205 of 3,396** presentation intervals exceeded 1.5 refresh periods with the cache disabled, versus **0 of 3,601** enabled. The reference is 16.667 ms at 90 Hz before rounding. The Android p99 in the earlier BlueStacks runs was 11.11 ms, and Kemu's was 13.34–13.56 ms. The three platforms also differ in Android version and presentation path. [Three-player frame pacing](windows-current-three-players.md)

An independent allocation trace found that the game stored its object data in the memory type affected by this option. Median elapsed time for 4 MiB of 32-bit writes to that type dropped from 99.596 to 0.196 ms, while the uniform/indirect buffer type remained slow. This explains why the experiment targeted host-visible Vulkan memory. The game result above measures the complete replay and includes graphics work, scheduling and presentation waits. Testing other games and GPUs requires separate measurements. [Memory measurements and game allocations](windows-vulkan-mapped-writes.md) · [Experimental renderer option](gfxstream-cached-host-qa.patch)
