# Current Windows comparison across three Android players

Each player ran two newly measured SuperTuxKart 1.5 Black Forest High Vulkan replays at 1600 × 900 on the RTX 5070 Ti. The three used the same signed x86-64 QA APK, four guest vCPUs, all 21 graphics fields, Vulkan FIFO and a verified 90 Hz Android display. The QA fixture uses a separate package and save directory. The 90 FPS game preference is recorded in the XML, and the STK benchmark skips its software frame limiter.

RustEmuDroid used Android 17 Google Play through Android Emulator 37.3.10. Original Qt Kemu used its Android 16 image, and the official BlueStacks player used Android 9 Pie64.

| Player (current build) | Game frames / time | Game FPS | Android SurfaceView/s | Surface p99 |
| --- | ---: | ---: | ---: | ---: |
| RustEmuDroid 0.21.1 / 1 | 3,265 / 38.075 s | 85.75 | 85.42 | 23.51 ms |
| RustEmuDroid 0.21.1 / 2 | 3,254 / 38.062 s | 85.49 | 85.06 | 23.24 ms |
| Kemu 0.7.1 / 1 | 3,432 / 38.128 s | 90.01 | 89.92 | 13.56 ms |
| Kemu 0.7.1 / 2 | 3,432 / 38.125 s | 90.02 | 90.02 | 13.34 ms |
| BlueStacks 5.22.166.1003 / 1 | 3,431 / 38.129 s | 89.98 | 90.05 | 11.11 ms |
| BlueStacks 5.22.166.1003 / 2 | 3,431 / 38.126 s | 89.99 | 89.90 | 11.11 ms |

| Player | Earlier game FPS | Current game FPS | Earlier Android/s | Current Android/s | Current preview |
| --- | ---: | ---: | ---: | ---: | --- |
| RustEmuDroid | 82.85 | 85.62 | 81.73 | 85.24 | integrated Rust/Glow, Fit frames to window, 800 × 450 px |
| Kemu | 90.02 | 90.02 | 90.07 | 89.97 | original Qt native Vulkan subwindow, 1280 × 720 px |
| BlueStacks 5 | 89.99 | 89.99 | 89.95 | 89.98 | official HD-Player, 1600 × 900 px |

Rust gained **2.77 game FPS** against its earlier matched sample. It processed **82.69 frames/s** in its fitted RGBA panel in these captures. The other players' native window sizes describe their observed renderer clients. BlueStacks' outer frame width changed between the geometry checks while its renderer stayed at 1600 × 900. Panel processing, foreground Android SurfaceView timestamps and physical scanout are different stages.

The guest Vulkan interfaces differed. RustEmuDroid reported Goldfish GFXStream (NVIDIA GeForce RTX 5070 Ti) (driver 26.0.65.35), Kemu reported Adreno (TM) 830 (driver 512.849.1), BlueStacks 5 reported NVIDIA GeForce RTX 5070 Ti (driver 610.88.0.0). Kemu used the same user-mode APIC and XSAVE compatibility launch as its earlier control.

Game FPS uses each whole replay. The Android metric is new SurfaceView timestamps in an approximately 20-second in-game window, with the existing player-specific waits recorded in the data. The windows are not frame-aligned across players. Rust and Kemu used an excluded full replay warm-up. BlueStacks used the activity and asset warm-up from its matched earlier protocol.

The earlier matched Rust control was v0.18.1. Kemu and BlueStacks used the same release versions as their earlier controls. The current measurements are a new small batch, with different Android builds, guest drivers, display densities and host window geometry. They cannot assign differences to a single component or predict unrelated games. [Earlier matched measurements](vulkan-end-scene.md#windows-results-at-90-hz)

A first Rust QA launch was discarded before gameplay because its window was not in foreground. The retry used the released binary and the previously verified private AVD. The BlueStacks run used the official player with a separate Pie64 clone. Cleanup restored the player settings, removed the QA APKs and returned to the single habitual Android 17 Home.

[Data, hashes and exact per-replay metrics](windows-current-three-players.json)
