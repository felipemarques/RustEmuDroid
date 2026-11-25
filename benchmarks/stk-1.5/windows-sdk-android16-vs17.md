# Android 16 and Android 17 on the same Windows emulator

RustEmuDroid 0.21.1 used the same Android Emulator 37.3.10 engine and gfxstream runtime on the RTX 5070 Ti with two Google x86-64 images. Android 16 was Google APIs r7 (BE2A.250530.026.F3), and Android 17 was Google Play r7 (CE2A.260420.051.A2). The four Black Forest replays used the same signed SuperTuxKart 1.5 QA APK, all 21 High Vulkan graphics fields and FIFO. Each AVD used 4 vCPUs, 8 GiB, 320 dpi and an active 1600 × 900 display at 90 Hz. Rust's adaptive video delivered 800 × 450 RGBA into an 800 × 450 physical viewport on the same RTX monitor.

The Android 17 rows came from the two ordinary renderer baselines in the prior composition experiment. Each followed an untimed warm-up. The clean Android 16 AVD ran two measured replays after one full untimed warm-up. The APK and launcher hashes, active mode, graphics settings and distinct preview frames were verified in the source receipts.

| Image and replay | Game frames / time | Game FPS | Android SurfaceView presentations/s | Surface p99 | Rust panel frames/s | QEMU CPU ms/s | Rust CPU ms/s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Android 16/1 | 3,179 / 38.046 s | 83.56 | 83.36 | 24.15 ms | 78.87 | 3294 | 264 |
| Android 16/2 | 3,190 / 38.074 s | 83.78 | 81.88 | 23.99 ms | 77.42 | 3175 | 242 |
| Android 16 pooled | | 83.67 | 82.62 | | 78.16 | 3234 | 253 |
| Android 17/1 | 3,255 / 38.062 s | 85.52 | 85.45 | 23.29 ms | 83.20 | 2217 | 278 |
| Android 17/2 | 3,262 / 38.062 s | 85.70 | 85.22 | 23.41 ms | 83.13 | 2212 | 286 |
| Android 17 pooled | | 85.61 | 85.33 | | 83.16 | 2214 | 282 |

The pooled game FPS difference (Android 16 minus Android 17) was **-1.94 FPS**. QEMU used 46% more host CPU time per second in the Android 16 sample. The guest renderer properties were Android 16 `emulation` EGL / `skiagl` HWUI, and Android 17 `emulation` EGL / `skiagl` HWUI. The host reported the stock non-Vulkan composition settings for both images while STK used Vulkan FIFO.

Game FPS covers the replay, Android presentations count new SurfaceView timestamps during a 20-second window, and panel FPS counts Rust's processed images during complete counter intervals within that window. CPU values cover the selected Windows QEMU and Rust processes in the SurfaceView window. Physical scanout was not measured. The 90 FPS game preference is stored in the XML, and STK's benchmark mode skips the software FPS limiter.

This comparison keeps the host engine and presenter fixed. Any measured difference applies to these image packages and benchmark sessions. Google APIs and Google Play include different builds, guest drivers and system components, and the Android 17 runs used an already tested QA AVD. With two replays per image in separate batches, the data cannot assign a difference to Android major version, a specific driver or Kemu's engine.

[Data, per-replay measurements and hashes](windows-sdk-android16-vs17.json)
