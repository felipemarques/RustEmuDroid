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

## Windows screenshot transport on Android 17

A separate Windows probe subscribed to the existing Android 17 emulator while the RustEmuDroid 0.17.0 panel remained open. The guest used the RTX 5070 Ti at 1600 × 900 and 60 Hz. A fullscreen Android View cycled through red, green, blue and yellow on successive draws. We compared two ways to receive RGBA8888 images from `streamScreenshot`. The inline stream carried 5,760,000 bytes per image. With `MMAP`, the service sent metadata and wrote the pixels to a named Win32 mapping. The probe copied that mapping once per callback before inspecting the pixels. On a static Home screen, both unary and streaming MMAP matched inline captures at every pixel in four samples.

| Pass | Transport | Seconds | Images received | Probe CPU seconds | QEMU CPU seconds | Images with mixed sampled colors |
|---|---|---:|---:|---:|---:|---:|
| A1 | Inline | 5.002 | 298 | 1.297 | 12.406 | 0 |
| B1 | MMAP | 5.013 | 300 | 0.281 | 9.469 | 0 |
| B2 | MMAP | 5.013 | 300 | 0.234 | 9.531 | 0 |
| A2 | Inline | 5.001 | 299 | 1.500 | 10.469 | 0 |

Both transports delivered about 60 images/s. Pooled probe CPU time per received image fell from **4.68 ms to 0.86 ms**. QEMU process CPU time includes its other work, including the running panel. Each image had one expected color at all 2,646 sampled locations in the central 800 × 450 pixels. The single shared buffer allows the producer to start its next write while a delayed reader is copying. This short test covers the observed samples. The released client continues to use inline transport. The [subsequent SuperTuxKart replay](../../docs/game-testing.md#supertuxkart-at-90-hz-with-shared-memory-video-on-windows) measured the experimental client at 90 Hz. [Per-pass data](windows-screenshot-transport.csv) · [Android Emulator streaming implementation](https://android.googlesource.com/platform/external/qemu/+/refs/heads/emu-master-dev/android/android-grpc/services/emulator-controller/server/src/android/emulation/control/EmulatorService.cpp#781)

## Serialized full-frame screenshots with a small change notification

We tested a second shared-memory design on the same Windows PC at 1600 × 900 and 60 Hz. A 160 × 90 inline stream signals framebuffer activity. When the reader is available it requests one full-resolution image with `getScreenshot`, copies it from the mapping, then permits the next request. An idle Home screen supplied one initial small image in 3.005 seconds. In the moving four-color fixture, both full inline streaming and the serialized method delivered 598 complete images over two five-second passes each, about 59.75 images/s. The reader used 0.200 and 0.067 CPU seconds per wall second, respectively. The serialized path took 8.37–8.60 ms at P95 from receipt of the small notification to the copied full frame.

With an artificial 45 ms reader delay, 221 notifications arrived while the reader was busy. The probe compared the full mapping before and after the delay on each of its 78 completed requests. It saw zero changes during those holds. The extra request and copy add latency, and the reader can skip frames under load. The [integrated SuperTuxKart test at 90 Hz](../../docs/game-testing.md#serialized-shared-memory-in-the-windows-game-panel-at-90-hz) subsequently measured the effect on the game and panel. A second fixture changed one source pixel. The small stream delivered 181 distinct sequence numbers and timestamps even while every scaled pixel stayed identical. Full-resolution captures recorded four colors at the changing pixel and one at a stationary control pixel. [Per-pass counters](windows-serialized-screenshot.csv)
