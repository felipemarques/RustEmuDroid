# Three Windows Android players at 90 Hz on the Radeon 780M

Original Kemu Qt and the official BlueStacks player each completed two SuperTuxKart 1.5 Black Forest replays on the integrated Radeon 780M. Both used the same signed x86-64 QA APK, four guest vCPUs, all 21 High graphics fields, 1600 × 900 at 100% render scale, and Vulkan FIFO with VSync on. The Android displays were verified at 90 Hz. The harness set `max_fps=90` in the game configuration. The benchmark bypasses STK's software frame limiter.

| Player and Android image | Replay | Game frames / time | Game FPS | Android SurfaceView/s | Surface P99 | Game loops >16.667 ms | Android gaps >16.667 ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Kemu Qt 0.7.1, Android 16 | 1 | 3,433 / 38.137 s | 90.02 | 90.06 | 12.78 ms | 1 / 3,433 | 0 / 1,802 |
| Kemu Qt 0.7.1, Android 16 | 2 | 3,433 / 38.136 s | 90.02 | 90.11 | 13.02 ms | 1 / 3,433 | 0 / 1,804 |
| BlueStacks 5.22.166.1003, Android 9 Pie64 | 1 | 3,421 / 38.136 s | 89.71 | 89.56 | 11.11 ms | 2 / 3,421 | 9 / 1,792 |
| BlueStacks 5.22.166.1003, Android 9 Pie64 | 2 | 3,422 / 38.137 s | 89.73 | 90.02 | 11.11 ms | 1 / 3,422 | 0 / 1,801 |

The pooled rates divide total counts by total durations within each condition. The Rust rows come from the separate four-boot cache experiment on the same Radeon with the same APK and preset. Each Rust condition has two valid replays.

| Player and setting | Pooled game FPS | Pooled Android SurfaceView/s | Visible game area on Windows |
| --- | ---: | ---: | --- |
| RustEmuDroid 0.21.1, Android 17, prepared SDK with cache off | 84.36 | 83.76 | Rust/Glow fitted RGBA panel, 800 × 450 |
| RustEmuDroid 0.21.1, Android 17, same SDK with cache on | 90.17 | 89.95 | Rust/Glow fitted RGBA panel, 800 × 450 |
| Kemu Qt 0.7.1 | 90.02 | 90.09 | Native Qt game surface, 1280 × 720 |
| BlueStacks 5.22.166.1003 | 89.72 | 89.79 | Native player game surface, 1600 × 900 |

Both Rust conditions used the same prepared Android Emulator 37.3.10 binary with **Shared Vulkan memory** enabled and WHPX TSC synchronization off. With caching enabled, the measured rates of all three players were close to the 90 Hz Android target for this replay. Guest Android builds, Vulkan interfaces, display densities and Windows game areas still differed. Each rate comes from two runs. [Four Rust cache replays and traces](windows-cached-memory-radeon780m.md) · [RTX 5070 Ti three-player comparison](windows-current-three-players.md)

## Adapter, sampling and cleanup

Kemu's gfxstream log selected the physical Radeon 780M with the AMD proprietary Vulkan driver. STK inside Kemu reported **Adreno (TM) 830**, driver 512.849.1. The BlueStacks host log and the STK Vulkan report both named the Radeon 780M. The game recorded Vulkan driver 2.0.416 there. Windows placed both game windows on the Radeon-driven 1920 × 1080, 100 Hz output. Process-scoped WDDM counters increased only on the Radeon over the warm-up and replay stages for both original players. These deltas identify the adapter over the session. Game FPS, Android SurfaceView presentations and physical scanout measure different stages. The 100 Hz figure is the monitor mode.

The game profiler measures the whole replay. Each Android sample spans about 20 seconds late enough to see the foreground game surface. Kemu and BlueStacks used their existing player-specific sampling starts, 12 seconds and at least 26 seconds after launch, respectively. Kemu used an excluded full replay as warm-up. BlueStacks used the activity and asset warm-up from its earlier protocol. Long game loops and Android intervals use the same strict threshold of 1.5 Android refresh periods, applied to their independent timelines.

Each player used its existing disposable QA profile. The runs restored the original Windows GPU preferences, Android rate policy and game settings, removed the QA APKs, and ended with one habitual Android 17 Home at 60 Hz. BlueStacks also restored the clone's original refresh preferences and its dedicated-GPU preference. One earlier Kemu setup exited before starting the emulator because its GPU preference preflight failed. The two completed Kemu replays came from the retry.

The public archive contains the four report CSVs, renderer logs, six game-profiler columns, Android presentation timestamps relative to the first sample, and WDDM deltas by adapter role. The export recomputed the frame counts, durations and SurfaceView summaries from those recordings and verified the 21 graphics fields. [Exact replay results and checksums](windows-current-three-players-radeon780m.json) · [Four replays and WDDM data](windows-current-three-players-radeon780m-traces.zip)

```sh
python summarize-long-frames.py windows-current-three-players-radeon780m-traces.zip --hz 90
```
