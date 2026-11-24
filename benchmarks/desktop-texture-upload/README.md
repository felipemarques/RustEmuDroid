# Direct OpenGL texture upload on Windows

Direct upload reduced mean desktop frame preparation from **1.22 to 0.21 ms per frame** (83%) and launcher CPU use from **40.48% to 36.60% of one logical CPU** across three short samples per path. The tested panel submitted about 44–47 frames per second. These samples establish a reduction in host preparation cost. Guest cadence and long intervals varied between runs.

## Test conditions

The private RustEmuDroid 0.5.0 build toggled between the standard egui RGBA copy and direct upload from the same decoded bytes while one Windows launcher and one Android Emulator 37.3.10 VM stayed open. The launcher window was 1080 × 680. Its Glow context reported AMD Radeon 780M and OpenGL 3.3 Core. The Android 17 Google Play x86-64 guest rendered Vulkan on the NVIDIA GeForce RTX 5070 Ti at 1600 × 900.

The Unity *FPS WW2 URP* demo stood at the Level 1 trench spawn with a fixed camera. Quality was Max, antialiasing Very High, terrain High, grass, shadows and real-time lights on, depth of field off, motion blur 0.3 and the frame limit 60. Six 18-second samples followed the order below. Each began three seconds after writing the mode selector. Consecutive identical modes were distinct timed samples in the same backend block.

| Run | Upload | Panel preparation (ms/frame) | Panel submissions/s | Guest presentations/s | Guest P95 / max interval (ms) | Launcher CPU (%) | QEMU CPU (%) |
| ---: | :--- | ---: | ---: | ---: | :--- | ---: | ---: |
| 1 | Legacy | 1.234 | 44.15 | 44.40 | 34.98 / 51.32 | 38.57 | 307.67 |
| 2 | Direct | 0.214 | 45.83 | 45.97 | 35.40 / 66.72 | 37.96 | 306.72 |
| 3 | Direct | 0.213 | 47.30 | 47.76 | 35.22 / 66.27 | 36.59 | 303.44 |
| 4 | Legacy | 1.212 | 46.30 | 46.44 | 34.84 / 64.79 | 41.81 | 300.91 |
| 5 | Legacy | 1.220 | 44.99 | 45.17 | 35.15 / 49.23 | 41.06 | 298.89 |
| 6 | Direct | 0.212 | 46.01 | 46.17 | 35.27 / 66.03 | 35.26 | 301.37 |

The direct path saves a full CPU copy of the decoded image. QEMU averaged 302.49% on legacy and 303.84% on direct. Guest P95 intervals stayed near 35 ms, with longer maximum intervals in the direct samples. The six runs support a launcher CPU saving. The small difference in submission rates does not establish a repeatable gaming FPS gain.

## Reproduce the counters

- **Panel.** Take the first and last samples within each CPU window from the same texture_ab block. Divide the change in painted by the panel sample span. Divide prep_ns_delta by prepared_delta and by 1,000,000 for milliseconds per prepared frame. The callback counts submissions to the Glow renderer. Preparation ends before the OpenGL upload call.
- **CPU.** Take cumulative user plus system CPU seconds before and after the window for RustEmuDroid and its QEMU process. Divide each process delta by elapsed wall-clock seconds and multiply by 100. A fully occupied logical CPU equals 100%.
- **Guest.** Poll the game's SurfaceView with SurfaceFlinger latency every 0.5 seconds. Keep distinct, positive presentation timestamps from the second column newer than the pre-sample baseline, excluding pending timestamps. Divide the interval count by the span between the first and last retained timestamps. Use consecutive timestamp differences for P95 and maximum. These are Android surface presentations.

[Per-run counters](display-upload-runs.csv)
