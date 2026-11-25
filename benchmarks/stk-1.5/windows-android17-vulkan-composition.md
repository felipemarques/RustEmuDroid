# Android 17 Vulkan composition and ANGLE on Windows

In four SuperTuxKart 1.5 Black Forest replays on the RTX 5070 Ti, the combined preset reduced median QEMU CPU time per Android presentation from **25.950 to 22.309 ms (−14.03%)**. Median game FPS changed from 85.610 to 85.772 (+0.19%) and Android presentations from 85.334 to 85.415 per second (+0.10%). Both differences in rate were small, and the Rust panel processed about 83 frames per second in each pass. This run supports a CPU-cost improvement for this setup. The roughly 90 FPS of the previously measured Kemu configuration remains unexplained by this switch. [Samples and relative Android timestamps](windows-android17-vulkan-composition.json)

| Pass | Game FPS | Android presents/s | Android p99 gap (ms) | Rust panel frames/s | QEMU CPU ms/present | Rust CPU ms/present |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| A1 · default | 85.518 | 85.450 | 23.291 | 83.198 | 25.940 | 3.256 |
| B1 · Vulkan + ANGLE | 85.759 | 85.463 | 23.089 | 83.076 | 22.273 | 3.018 |
| B2 · Vulkan + ANGLE | 85.785 | 85.368 | 23.196 | 82.525 | 22.345 | 3.099 |
| A2 · default | 85.702 | 85.219 | 23.411 | 83.129 | 25.961 | 3.352 |

## What changed

- A used the emulator defaults. Its log reported `useVulkanComposition=false`, `useVulkanNativeSwapchain=false` and `guestVulkanOnly=false`. Android reported `ro.hardware.egl=emulation`.
- B added only `ANDROID_EMULATOR_FEATURES=VulkanNativeSwapchain` to the QA launcher's environment. Android Emulator 37.3.10.0 (build 16970787) reported all three bits as `true` and Android reported `ro.hardware.egl=angle`. The HWUI property stayed `skiagl` and STK used its own Vulkan 1.3 renderer in all passes. The SDK coupled the changes, so the measurements describe the combined preset. [SDK command-line help](https://developer.android.com/studio/run/emulator-commandline#get-help-about-command-line-options) · [Upstream framebuffer feature selection](https://android.googlesource.com/platform/hardware/google/gfxstream/+/ca9abf8fd1ef9dadedc4de1bdfc1b68beb7850a2/host/FrameBuffer.cpp#378)
- Before timing B, the visible Rust panel matched guest Home pixels. Physical Windows clicks changed the Android Home tab and restored it. Two crops from the warm-up showed different scenes with STK in the foreground. The published launcher was used throughout the experiment. The option was confined to the test session.

## Conditions and measurement

The isolated Android 17 Google Play AVD rendered at 1600 × 900 and 90 Hz on the RTX 5070 Ti with NVIDIA driver 610.88. The client was the shipped RustEmuDroid v0.21.1, with adaptive gRPC video fitted to a physical 800 × 450 viewport. All passes used the same scoped x86-64 SuperTuxKart 1.5 APK, High graphics, the same 21 settings and Vulkan FIFO. The configured `max_fps` was 90 with VSync on. STK's `--benchmark` path skips the software FPS limiter. [RustEmuDroid v0.21.1](https://github.com/felipemarques/RustEmuDroid/releases/tag/v0.21.1) · [Benchmark fixture](compare.py) · [STK 1.5 main loop](https://github.com/supertuxkart/stk-code/blob/1.5/src/main_loop.cpp#L711-L734)

The APK SHA-256 is `dbde91eb1451e5f165cbc09b8d1144d13d35283d5f6752cec52704e3e696ff8e`. The Windows executable SHA-256 is `513fc67688ba6c5e48a57f0db097ffc6c7fcf5358b5133c5f1d2ce2ae51de949`. The JSON includes the runtime QEMU digest, game report hashes and all relative presentation timestamps. [Evidence](windows-android17-vulkan-composition.json)

A1 came from the immediately preceding pass with the same verified configuration and fixture. Its first B boot revealed the ANGLE coupling and was excluded before timing. The packaged client restored the isolated 60 Hz policy before the combined B1/B2/A2 continuation. Each 90 Hz boot ran one warm-up. B1 and B2 shared a boot, while A1 and A2 ran on separate boots. This is one short sequence with two samples per condition.

Game FPS is the replay's original game frame count divided by its report duration. Android counts distinct SurfaceFlinger presentation timestamps for the game's SurfaceView in roughly 20 seconds around the active race. The reported p99 is a gap between those timestamps. The separate panel counter measures frames processed for drawing. QEMU and Rust CPU times include all threads in each named Windows process and use the same SurfaceFlinger polling window. These counters cover game, Android and client stages. Physical monitor scanout was outside the measurement.

The benchmark removed its isolated APK on all three measured boots. The official launcher cleared the temporary 90 Hz journal and restored the QA profile to its original policy. The session ended at the single habitual Android 17 RTX Home at 60 Hz with the same executable and unchanged launcher preferences.

## Historical Kemu comparison

Kemu 0.7.1 on Android 16 measured 90.017 and 90.018 game FPS, with 90.069 and 90.064 Android presentations/s. The same Kemu engine under the Rust frontend previously measured 90.013 and 90.020 game FPS, with 90.013 and 89.957 presentations/s. Those tests used the same APK, scene and RTX. They ran separately with a different Android image and emulator build. This four-pass test leaves the difference in frame rate open. [Kemu Qt evidence](windows-vulkan-90hz-scoped-kemu.json) · [Kemu with Rust frontend evidence](windows-vulkan-90hz-scoped-rust-kemu.json)
