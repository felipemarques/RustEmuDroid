# SuperTuxKart 1.5 Vulkan frame profiling

[The earlier comparison](windows-vulkan-90hz-profiler.csv) measured all of `IrrDriver update` and the narrower `drawAll` marker. The difference includes several Vulkan operations. The [optional STK patch](vulkan-end-scene.patch) splits the Vulkan `endScene` path into CPU profiler columns using STK's existing marker stack and benchmark export. It also logs the `VkPresentModeKHR` value after each successful swapchain creation. The patch applies to upstream STK 1.5 commit [`1fb491f507216c5d181ccd85f29ff08eca003827`](https://github.com/supertuxkart/stk-code/tree/1fb491f507216c5d181ccd85f29ff08eca003827).

## Build and record

1. In a separate STK checkout at that commit, check and apply the patch.

   ```sh
   git apply --check /path/to/RustEmuDroid/benchmarks/stk-1.5/vulkan-end-scene.patch
   git apply /path/to/RustEmuDroid/benchmarks/stk-1.5/vulkan-end-scene.patch
   ```

2. Follow the [STK 1.5 Android build instructions](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/android/README.ANDROID) on Linux with the [1.5 dependency bundle](https://github.com/supertuxkart/dependencies/releases/tag/1.5), JDK 17, Android SDK API 35 and NDK r28b. Set `SDK_PATH`, `NDK_PATH`, `STK_KEYSTORE`, `STK_STOREPASS` and `STK_ALIAS` for the private toolchain and QA key. Use the upstream optimized beta build and give it its own application ID, JNI class namespace and storage directory in the separate checkout.

   ```sh
   sed -i \
     -e 's|^export APP_NAME_BETA=.*|export APP_NAME_BETA="SuperTuxKart Vulkan QA"|' \
     -e 's|^export PACKAGE_NAME_BETA=.*|export PACKAGE_NAME_BETA="org.rustemudroid.qa.stkvulkan"|' \
     -e 's|^export PACKAGE_CLASS_NAME_BETA=.*|export PACKAGE_CLASS_NAME_BETA="org/rustemudroid/qa/stkvulkan"|' \
     -e 's|^export APP_DIR_NAME_BETA=.*|export APP_DIR_NAME_BETA="stk-vulkan-qa"|' android/make.sh
   ```

   Prepare the same Black Forest assets for both builds. To use the assets from the verified official 1.5 test APK with SHA-256 `e1f2f49152abfb8f3ed0ced47981e84096758bff9758563aa3b917111a2a3cbd`, extract just `assets/*` from that APK into `android/`. Keep `assets/files.txt` and `assets/data/supertuxkart.1.5`. Use the ABI the target actually runs. Here is the x86-64 build.

   ```sh
   cd android
   COMPILE_ARCH=x86_64 bash ./make_deps.sh
   PROJECT_VERSION=1.5 PROJECT_CODE=328 BUILD_TYPE=beta COMPILE_ARCH=x86_64 \
       STK_QA_VULKAN_END_SCENE=1 bash ./make.sh
   ```

   Before use, check the signed APK with `apksigner verify` and `aapt dump badging`. Its package should be `org.rustemudroid.qa.stkvulkan` and its ABI should match the selected build. Keep a copy before building the baseline with the same settings and local key and `STK_QA_VULKAN_END_SCENE=0`. On desktop, enable `-DSTK_QA_VULKAN_END_SCENE=ON` when configuring with CMake, select Vulkan in the game, then pass `--benchmark` on launch. Both build switches default to off.

3. On Android, verify the serial and expected renderer for the one QA guest. Launch the QA app once to create its configuration. Select the separate package in the replay runner with `--standalone-vulkan-qa`. Use the same scene, guest refresh, display size, graphics options, and ABI across the comparison. This command uses an Android 17 AVD named `RustEmuDroid` already running at 90 Hz. `--fps-limit 90` sets the game's preference, while the guest controls the actual mode.

   ```sh
   python benchmarks/stk-1.5/compare.py --rust emulator-5554 --rounds 2 \
       --standalone-vulkan-qa --presets high-vulkan --fps-limit 90 --output end-scene-replay
   ```

   Keep the fresh `report.csv`, `quadros.csv`, and `renderer.txt` together. The replay script copies the `STK QA Vulkan VkPresentModeKHR` lines from the game's `stdout.log` to `renderer.txt`. If swapchain creation appears again, keep every selected mode and note the recreation. Common values are 0 (immediate), 1 (mailbox), 2 (FIFO), and 3 (FIFO relaxed). Verify that the selected mode changes before interpreting a FIFO comparison. [Vulkan mode definitions](https://docs.vulkan.org/refpages/latest/refpages/source/VkPresentModeKHR.html)

4. Summarize each `quadros.csv` separately. The script requires all six normal path markers, filters to frames with a positive `Main loop`, keeps zeros in child scopes, verifies nesting, and calculates mean, median, nearest-rank P95, and sum in milliseconds.

   ```sh
   python benchmarks/stk-1.5/summarize-vulkan-end-scene.py path/to/quadros.csv > scope-summary.csv
   ```

## Interpret the columns

| CPU column | Work inside the scope |
| --- | --- |
| `VK endScene` | The whole Vulkan driver call, including early returns and cleanup |
| `fence` | The host call to wait on the prior in-flight fence for this slot |
| `reset` | Fence and command-pool reset |
| `acquire` | `vkAcquireNextImageKHR` |
| `record` | STK's main `buildCommandBuffers` call |
| `submit` | Graphics queue lock, `vkQueueSubmit`, and unlock |
| `present` | `vkQueuePresentKHR`, including the graphics queue lock and unlock when STK uses the shared queue |
| `recovery` | Early swapchain recovery on an empty fence list or timeout. The exporter emits the column only if the path ran while profiling |
| `other` | The difference per frame between `VK endScene` and the sum of its instrumented child scopes. It includes bookkeeping, skipped path cleanup, and instrumentation overhead |

These are elapsed durations on the calling CPU thread. A queue API call can spend time waiting, and submitted work can continue after the call returns. Concurrent CPU work, total GPU execution time and physical display presentation require separate evidence. Scope costs include the marker overhead. Compare the game FPS and CPU totals with an equivalent uninstrumented build before using the new markers for microsecond differences. A repeat with VSync changed is useful only when the selected swapchain mode changes, with game FPS and Android SurfaceFlinger cadence recorded separately. [Upstream Vulkan frame path](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/lib/graphics_engine/src/ge_vulkan_driver.cpp#L1773-L1882) · [Upstream CPU export](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/utils/profiler.cpp#L759-L788)

A signed, optimized x86-64 QA APK was built from the pinned source. Offline checks verified its separate package, matching JNI classes, SDL2 dependency, stock Black Forest inputs and eight compiled marker labels. The patch also compiled as an Android ARMv7 driver object. A three-frame fixture used STK's own marker stack and CSV writer and verified the profiler summary with a controlled clock.

A separate opt-in patch records graphics-queue timestamps around the render pass. The QA package keeps the CPU scopes available in the same build. [GPU timing procedure and coverage checks](vulkan-gpu-timestamps.md)

## Windows results at 90 Hz

RustEmuDroid 0.18.1 on Android 17 Google Play, Kemu 0.7.1 on Android 16, and BlueStacks 5.22.166.1003 Pie64 on Android 9 ran the same signed x86-64 QA APK on an RTX 5070 Ti. Each player completed two measured Black Forest replays in a separate batch with High/Vulkan, 1600 × 900, 100% render scale, the 90 FPS game preference and game VSync. All six runs matched the 21 reported graphics fields, verified active 90 Hz in Android and logged Vulkan 1.3 and FIFO (VkPresentModeKHR=2). The desktop frontend, Android build, display density and graphics implementation differ.

The original Kemu launcher and Qt frontend ran from a verified copy of the v0.7.1 distribution with a fresh AVD. The standard boot stopped before Android completed startup. The successful retry used user-mode WHPX APIC and the Kemu XSAVE compatibility switch together. The 90 Hz setting selected the physical guest mode, and Android initially rendered at 45 Hz. The same minimum-refresh and default-game policy settings used in the Rust reference brought Kemu's Android render and SurfaceFlinger timing to 90 Hz. The test restored those settings and removed the QA APK.

STK reported Goldfish GFXStream driver 26.0.65.35 on RustEmuDroid, the virtual Adreno (TM) 830 driver 512.849.1 on Kemu, and NVIDIA driver 610.88.0.0 on Pie64. Kemu's host gfxstream log selected the RTX 5070 Ti with NVIDIA driver 610.88. NVIDIA's WDDM process listing confirmed that QEMU used that GPU during warm-up.

| Engine and replay | STK frames | STK duration | STK FPS | Android SurfaceView presentations | SurfaceView rate | SurfaceView P99 interval |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| RustEmuDroid 1 | 3,159 | 38.157 s | 82.790 | 1,626 in 20.024 s | 81.204/s | 23.850 ms |
| RustEmuDroid 2 | 3,157 | 38.073 s | 82.920 | 1,647 in 20.022 s | 82.258/s | 23.483 ms |
| Kemu 1 | 3,432 | 38.126 s | 90.017 | 1,803 in 20.018 s | 90.069/s | 12.810 ms |
| Kemu 2 | 3,433 | 38.137 s | 90.018 | 1,803 in 20.019 s | 90.064/s | 12.749 ms |
| BlueStacks 1 | 3,432 | 38.127 s | 90.015 | 1,801 in 20.018 s | 89.967/s | 11.111 ms |
| BlueStacks 2 | 3,430 | 38.125 s | 89.967 | 1,801 in 20.026 s | 89.931/s | 11.111 ms |

Pooling frames and durations gives **82.85 game FPS and 81.73 Android presentations/s** for RustEmuDroid, **90.02 and 90.07/s** for Kemu, and **89.99 and 89.95/s** for BlueStacks. STK FPS covers the full replay. The Android rate covers a separate window of about 20 seconds on the foreground game's SurfaceView. Differences of a few hundredths around 90/s in these finite windows do not establish a rate above 90 Hz. Android presentations, Windows panel throughput and physical scanout measure different stages.

A follow-up on the same Kemu engine and Android 16 image measured **90.02 game FPS and 89.98 Android presentations/s** with the integrated RustEmuDroid v0.20.0 frontend. The original Kemu window remained near 90 FPS under the same workload. [Matched Kemu engine with both frontends](#how-the-kemu-and-rust-windows-receive-game-frames)

In a separate v0.21.1 test, changing the Android 17 host composition and guest EGL implementation together reduced QEMU CPU time per presentation by 14.03%. The medians of two kart replays in each mode were 85.61 and 85.77 FPS. [Android 17 compositor and ANGLE experiment](windows-android17-vulkan-composition.md)

Keeping the desktop client and SDK engine fixed, the Android 16 Google APIs image delivered 83.67 FPS and the Android 17 Google Play image delivered 85.61 FPS. The Android 16 image used by Kemu comes from a different build. [Official image comparison on the same runtime](windows-sdk-android16-vs17.md)

The table pools both passes per player. Each mean divides total scope duration by all positive `Main loop` samples, including frames where a child scope recorded zero duration.

| CPU-thread elapsed time per frame | RustEmuDroid | Kemu | BlueStacks |
| --- | ---: | ---: | ---: |
| `Main loop` | 12.069 ms | 11.109 ms | 11.112 ms |
| `IrrDriver update` | 10.636 ms | 10.523 ms | 10.644 ms |
| `drawAll()` | 7.083 ms | 3.044 ms | 2.178 ms |
| `VK endScene` | 3.171 ms | 7.298 ms | 8.301 ms |
| `acquire` | 1.881 ms | 6.736 ms | 7.506 ms |
| `record` | 0.403 ms | 0.279 ms | 0.144 ms |
| `submit` | 0.082 ms | 0.066 ms | 0.074 ms |
| `present` | 0.361 ms | 0.077 ms | 0.452 ms |
| `Main loop` minus `IrrDriver update` | 1.433 ms | 0.586 ms | 0.469 ms |

Kemu and BlueStacks spent 6.74–7.51 ms in `vkAcquireNextImageKHR` and 2.18–3.04 ms in `drawAll()`. RustEmuDroid spent 1.88 ms and 7.08 ms in those scopes, respectively. Full graphics-update times were within 0.13 ms across the three players. The main loop outside the graphics update averaged 1.43 ms on RustEmuDroid, 0.59 ms on Kemu and 0.47 ms on BlueStacks. The acquire scope includes waiting for a swapchain image. These CPU-thread intervals and separate replay batches leave GPU execution time and the cause of the FPS gap unresolved.

The APK has SHA-256 `dbde91eb1451e5f165cbc09b8d1144d13d35283d5f6752cec52704e3e696ff8e`. The data include game-report digests, all reported graphics fields and phase summaries. [RustEmuDroid data](windows-vulkan-90hz-scoped-native.json) · [Kemu data](windows-vulkan-90hz-scoped-kemu.json) · [BlueStacks Pie64 data](windows-vulkan-90hz-scoped-pie64.json)

The Kemu export includes relative SurfaceView timestamps. Divide the number of retained presentations by the recorded polling duration for the rate. Sort consecutive timestamp differences and take the entry at floor(0.99 × (N − 1)) in the zero-indexed list of N gaps for P99.

To regenerate the RustEmuDroid and Pie64 aggregates from private QA evidence, place the operator's receipt (`apk-receipt.json` for RustEmuDroid or `receipt.json` for BlueStacks) and `results.json` beside the two `lap-N` directories. Each lap needs its game report, CPU profile, renderer log and active-display receipt. Include the optional `presentations.json` only for a valid SurfaceView capture. The exporter verifies the APK scope, ABI reported by the runner, replay duration, effective 90 Hz, Vulkan evidence and matching graphics fields. It omits timestamps, process IDs, layers and local paths.

```sh
python benchmarks/stk-1.5/export-vulkan-end-scene.py /path/to/qa-run \
    --engine RustEmuDroid --engine-version 0.18.1 --android 17 \
    --output benchmarks/stk-1.5/windows-vulkan-90hz-scoped-native.json
```

A paired BlueStacks export uses `--engine "BlueStacks 5 Pie64" --android 9` with that player's version and `--match-reference benchmarks/stk-1.5/windows-vulkan-90hz-scoped-native.json`. The exporter checks the same APK digest, ABI, graphics fields, active display period and Vulkan present modes before writing that pair. Record host, driver and renderer conditions separately when interpreting the results.

For an instrumentation baseline, use `--mode baseline --compare-scoped path/to/scoped.json` on two completed runs from the same engine and ABI. The baseline must have the same package and game settings, signed build assets matching the scoped APK, and neither optional phase columns nor the present-mode log. The exporter records scoped-minus-baseline differences in game FPS, SurfaceView rate where measured in both, and P95 elapsed times for `Main loop` and `IrrDriver update`. It pairs runs by ordinal across the two batches. This design cannot separate instrumentation cost from run-to-run or batch differences. The baseline's actual Vulkan present mode is unobserved with the diagnostic log disabled.

### Android 17 instrumentation baseline

The x86-64 baseline APK (`def3b0fd211c91eee4d91cfabae41283ed339c3fef203f3f9b0432f48e5807cd`) used the same signing certificate and all 5,577 assets matched by SHA-256. The APK's ELF, renderer log and profiler exports contained no optional Vulkan instrumentation. On RustEmuDroid 0.18.1 the game's own reports recorded the same 21 graphics fields and the RTX Vulkan driver as the scoped runs.

| Replay | Baseline STK FPS | Baseline Android SurfaceView rate | Scoped minus baseline FPS | Scoped minus baseline SurfaceView rate | Scoped minus baseline P95 `Main loop` |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 83.666 | 81.502/s | −0.877 | −0.298/s | +0.531 ms |
| 2 | 83.478 | 82.388/s | −0.558 | −0.130/s | +0.398 ms |

The scoped batch was modestly slower in these two comparisons. The measured difference includes any change between batches and cannot isolate the overhead of the markers. [Sanitized Android 17 baseline data](windows-vulkan-90hz-baseline-native.json)

### Android 17 game VSync control

Use `--game-vsync off` with the same `compare.py` replay command and a new output directory to repeat the game-side change. Changing only the game's `swap-interval-vsync` setting from `1` to `0` selected mailbox (`VkPresentModeKHR=1`) in two additional runs of the scoped x86-64 APK. The guest stayed at 90 Hz, with the same RTX renderer and the same 21 reported graphics fields. STK's `--benchmark` condition bypasses its software frame limiter. The XML still held the 90 FPS preference, and the game loop could exceed it. [STK 1.5 main loop](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/main_loop.cpp#L711-L735)

| Scoped mode | STK FPS by replay | Pooled STK FPS | Android SurfaceView rate by replay | Pooled SurfaceView rate | SurfaceView P99 by replay |
| --- | ---: | ---: | ---: | ---: | ---: |
| FIFO, VSync on | 82.790 / 82.920 | 82.855 | 81.204 / 82.258 per s | 81.731/s | 23.850 / 23.483 ms |
| Mailbox, VSync off | 94.860 / 99.275 | 97.067 | 78.044 / 81.711 per s | 79.876/s | 24.349 / 23.602 ms |

The integrated Windows panel processed **74.88 frames/s with FIFO** and **76.52 with Mailbox** across the complete panel reporting intervals inside the Android sampling windows. The per-replay rates were 74.26 and 75.50/s with FIFO and 73.40 and 79.65/s with Mailbox. A processed frame is one accepted by the Rust display code. These rates describe a different stage from Android presentation, and neither counter verifies physical monitor scanout. The BlueStacks runs did not include an equivalent panel measurement.

Pooling divides the sum of frames or presentations by the sum of the respective observation durations. Mean `acquire` time dropped from 1.881 to 0.552 ms per frame and mean `endScene` time from 3.171 to 1.836 ms. The game loop produced more frames while Android delivered fewer presentations in these runs. These separate two-run batches do not establish whether the compositor rate difference would repeat. The game VSync preference was restored after the test. [Sanitized Android 17 mailbox data](windows-vulkan-90hz-scoped-native-mailbox.json)

### Pausing the Windows video stream

A private Windows client based on v0.18.1 could cancel and resume the `streamScreenshot` subscription while the same VM, game controls and Android compositor kept running. We used the same signed x86-64 APK, RTX 5070 Ti, 1600 × 900, High/Vulkan, 90 Hz and FIFO. After one warm-up, we measured four fresh replays in the order shown below. On each replay the game frame count covers the full lap, while SurfaceFlinger and host CPU counters cover a 20-second segment. The panel counter covers complete reporting intervals within that segment.

| Replay | Video subscription | Game FPS | Android presentations/s | Android P99 interval | Rust panel frames/s |
| --- | --- | ---: | ---: | ---: | ---: |
| A1 | On | 84.19 | 82.51 | 23.76 ms | 74.69 |
| B1 | Off | 84.08 | 82.86 | 23.61 ms | 0 |
| B2 | Off | 85.41 | 83.67 | 23.82 ms | 0 |
| A2 | On | 83.52 | 81.88 | 23.31 ms | 75.39 |

Across both runs in each state, pausing the video stream raised game throughput from **83.86 to 84.75 FPS** and Android presentations from **82.20 to 83.27/s**. Windows CPU time fell from **514 to 24 ms per second** for the Rust launcher and from **2,675 to 1,941 ms per second** for QEMU. The counters sum all threads for each process. No new video frames reached the panel in either paused run. In this scene, removing video delivery saved about 1.22 CPU seconds per elapsed second across both processes, while the Android game surface remained below 90 presentations/s. Frame delivery to the panel resumed for the final replay. [Per-replay measurements, hashes and controls](windows-90hz-stream-subscription.json)

### Matching the screenshot stream to the Windows panel

A separate private v0.18.1 candidate switched the inline `streamScreenshot` request between the full 1600 × 900 image and a resolution sized to the panel. The four measured Black Forest replays followed a full, scaled, scaled, full sequence after one excluded full-size warm-up. The same Android 17 Google Play AVD stayed up throughout on the RTX 5070 Ti. The signed, instrumented x86-64 APK, High preset, all 21 reported graphics fields, Vulkan FIFO, 1600 × 900 Android display and 90 Hz active refresh matched across the runs. The request changed the delivered screenshot dimensions. The game and Android kept their original render and display settings.

The non-maximized window had a 1080 × 680 physical client area at 96 DPI. The game filled a fixed 1034 × 582 pixel rectangle inside it. The candidate rounded the request up to the smallest 16:9 size that covered that rectangle, capped at the native size. The scaled request and actual RGBA response were both 1040 × 585, six pixels wider and three pixels taller than the fitted rectangle. The full response contained 5,760,000 image bytes per frame and the scaled response contained 2,433,600.

| Replay | Screenshot size | STK FPS | Android presentations/s | Android P99 interval | Received frames/s | Panel frames/s | Received RGBA MiB/s |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| A1 | 1600 × 900 | 82.990 | 80.607 | 23.822 ms | 79.690 | 74.283 | 437.753 |
| B1 | 1040 × 585 | 84.807 | 83.268 | 24.098 ms | 83.094 | 80.422 | 192.849 |
| B2 | 1040 × 585 | 83.966 | 82.162 | 23.630 ms | 81.833 | 79.270 | 189.924 |
| A2 | 1600 × 900 | 82.948 | 80.460 | 23.704 ms | 79.764 | 73.754 | 438.156 |

| Pooled metric | Full | Scaled |
| --- | ---: | ---: |
| STK FPS | 82.969 | 84.386 |
| Android presentations/s | 80.534 | 82.715 |
| Received frames/s | 79.727 | 82.464 |
| Panel frames/s | 74.018 | 79.846 |
| Received RGBA MiB/s | 437.955 | 191.388 |
| QEMU CPU ms/s | 2,664.94 | 2,319.10 |
| Rust launcher CPU ms/s | 493.38 | 337.93 |

The panel processed 5.83 more frames/s while the stream carried 56.3% fewer image bytes per second. QEMU CPU time fell 13.0% and launcher CPU time fell 31.5%. Android presentations rose by 2.18/s and remained below 90/s, with similar P99 intervals. Counts are weighted by their own observation durations. STK covers the full replay. SurfaceFlinger and GetProcessTimes share an approximately 20-second window, and the stream and panel counters cover their own complete intervals inside that window. Received bytes are decoded RGBA payload, excluding gRPC framing and metadata. Panel frames count frames accepted by the launcher, and Windows scan-out and image quality were outside this test. These four passes cover one window size at one DPI and do not establish the result for larger windows or other workloads. The candidate was private and the packaged v0.19.0 Home was restored after the experiment. [Per-replay measurements, geometry and hashes](windows-90hz-stream-resolution.json)

The panel-sized transport experiment covers the Rust launcher. The original Kemu and Pie64 replays used their native frontends. [Three-player replay comparison](#windows-results-at-90-hz)

A separate Windows check exercised the v0.20.0 implementation on a live 1600 × 900 Android 17 display. In a 1040 × 585 stream, a click at window coordinates (193.9, 370.3) selected the Settings row at Android coordinates (300, 466). The PNG was 1600 × 900, and the MP4 contained 57 frames at 1600 × 900. GIF, animated JPEG XL and the retrospective JPEG XL each contained 30 frames at 720 × 405. Fullscreen restored the native stream. The player resumed the smaller stream after each capture, after disarming replay and after leaving fullscreen. This check verified input and image sizes independently of the replay timing experiment. [Live fixture](../../src/ui/adaptive_video_test.rs)

### How the Kemu and Rust windows receive game frames

In Kemu v0.7.1 on Windows, Qt gives the Android display its own native window handle. With its host Vulkan path active, gfxstream creates a child window, uses the posted guest color buffer as a GPU image and presents to a Win32 Vulkan surface. The Windows GPU log from the Black Forest run confirmed host Vulkan on the RTX. [Qt display](https://github.com/kuloPo/Kemu-emulator_external_qemu/blob/1622371aee57f26f5446ca42ec32bd4faddfc440/kemu-qt/src/WindowChrome.cpp#L89-L97) · [Native child window](https://github.com/kuloPo/Kemu-emulator_hardware_google_gfxstream/blob/df6c26c82b04220426243fc56a0cc1fb459827f1/host/native_window/native_sub_window_win32.cpp#L52-L70) · [Vulkan image handling](https://github.com/kuloPo/Kemu-emulator_hardware_google_gfxstream/blob/df6c26c82b04220426243fc56a0cc1fb459827f1/host/vulkan/post_worker_vk.cpp#L50-L82) · [Presentation](https://github.com/kuloPo/Kemu-emulator_hardware_google_gfxstream/blob/df6c26c82b04220426243fc56a0cc1fb459827f1/host/vulkan/display_vk.cpp#L816-L826)

RustEmuDroid's native window runs in a separate process. It subscribes to the emulator's screenshot stream, receives RGBA pixels and uploads them to the desktop texture. Requesting a smaller image reduced CPU and raised the panel's processing rate. Pausing the subscription entirely raised the Android presentation rate from 82.20 to 83.27/s on the same Android 17 VM. That experiment leaves most of the distance to Kemu's 90.07/s unexplained. The release of Android, the bundled emulator and the graphics driver exposed to the guest also differ in the three-player comparison. [Rust texture upload](../../src/display/native_texture.rs) · [Same-VM transport experiment](#pausing-the-windows-video-stream)

An earlier control used the same Kemu Android 16 image and Windows engine with the two frontends. At the Original Vulkan preset and a 60 Hz ceiling, both averaged 60.01 game FPS. [Earlier matched-engine measurements](README.md#painel-integrado-com-aceleração-vulkan)

The 90 Hz follow-up paired the Kemu 0.7.1 Qt runs above with two replays in the optimized RustEmuDroid v0.20.0 Windows client. Rust launched the same Kemu 0.7.1 QEMU/gfxstream binaries and Android 16 image on the RTX 5070 Ti, with the Qt window hidden and the Rust video panel active. The replay used a fresh Rust AVD with 4 vCPUs and 8 GiB. The signed x86-64 APK and its 21 reported graphics fields, Vulkan FIFO and 1600 × 900 at effective 90 Hz matched the original Kemu runs. Adaptive video was off. Each replay started a fresh game process after a warm-up.

Both launchers used user-mode WHPX APIC and passed `noxsaves` to the guest kernel. Kemu Qt set `hw.lcd.vsync=90` in its test profile, while Rust requested `-vsync-rate 90` and applied the temporary Android game and minimum-refresh policies. Both games reported Vulkan 1.3 and the virtual Adreno 830 driver 512.849.1. gfxstream selected the host RTX 5070 Ti with driver 610.88, and NVIDIA's WDDM process listing confirmed the adapter during warm-up on both frontends.

| Frontend and replay | Game frames / duration | Game FPS | Android presentations / duration | Android presentations/s | Android P99 interval | Rust panel frames/s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| RustEmuDroid 0.20.0, Kemu Android 16 — 1 | 3,433 / 38.139 s | 90.013 | 1,802 / 20.019 s | 90.013 | 13.278 ms | 84.026 |
| RustEmuDroid 0.20.0, Kemu Android 16 — 2 | 3,433 / 38.136 s | 90.020 | 1,801 / 20.021 s | 89.957 | 13.406 ms | 84.109 |
| RustEmuDroid 0.20.0, Kemu Android 16 — pooled | 6,866 / 76.275 s | 90.016 | 3,603 / 40.040 s | 89.985 | Per replay above | 84.067 |
| Kemu 0.7.1 Qt, Android 16 — pooled | 6,865 / 76.263 s | 90.017 | 3,606 / 40.037 s | 90.067 | 12.810 / 12.749 ms | — |

The game ran at about 90 FPS in both frontends, and Android's SurfaceView presented about 90 frames/s. The Rust panel processed 84.07 frames/s in its complete reporting intervals within the SurfaceView windows. Game FPS covers the whole replay, and the SurfaceView samples cover approximately 20 seconds per run. The Rust counter measures frames processed for display. Windows scanout and equivalent Kemu Qt panel throughput were outside this test. The comparison uses separate batches and AVDs and describes this Vulkan replay. The later DWM experiment kept the Qt render window active during all four passes and compared two ways of delivering its picture to a second window. [Windows DWM and inline RGBA on a live game](../../experiments/results/windows-kemu-dwm-inline-stk.md)

A subsequent Windows test used one Android 17 Google Play AVD, one RTX and the same 800 × 450 preview size for the released Rust client and the DWM viewer. The game continued to render at 1600 × 900. [Four replays, Android frame intervals and host CPU](../../experiments/results/windows-android17-fit-dwm-stk.md)

[RustEmuDroid with Kemu engine data](windows-vulkan-90hz-scoped-rust-kemu.json) · [Original Kemu data](windows-vulkan-90hz-scoped-kemu.json)
