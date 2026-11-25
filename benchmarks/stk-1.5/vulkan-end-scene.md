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

## Windows results at 90 Hz

RustEmuDroid 0.18.1 ran the standalone x86-64 QA APK in an Android 17 Google Play AVD. BlueStacks 5.22.166.1003 ran the exact same APK as x86-64 on a separate Pie64 lab instance with Android 9. Both used a GeForce RTX 5070 Ti. The four Black Forest replays used High/Vulkan, 1600×900 at render scale 1, the 90 FPS game preference and game VSync. Android reported an active 90 Hz mode during each replay. STK reported Vulkan 1.3 and FIFO (`VkPresentModeKHR=2`) on both engines. RustEmuDroid reported Goldfish GFXStream driver `26.0.65.35`, while BlueStacks reported NVIDIA driver `610.88.0.0`. The 21 exported graphics fields matched across the four runs and matched the earlier stock x86-64 runs A1 and A2 at 90 Hz. The guest Android versions, display density and graphics implementations differ.

| Engine and replay | STK frames | STK duration | STK FPS | Android SurfaceView presentations | SurfaceView rate | SurfaceView P99 interval |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| RustEmuDroid 1 | 3,159 | 38.157 s | 82.790 | 1,626 in 20.024 s | 81.204/s | 23.850 ms |
| RustEmuDroid 2 | 3,157 | 38.073 s | 82.920 | 1,647 in 20.022 s | 82.258/s | 23.483 ms |
| BlueStacks 1 | 3,432 | 38.127 s | 90.015 | 1,801 in 20.018 s | 89.967/s | 11.111 ms |
| BlueStacks 2 | 3,430 | 38.125 s | 89.967 | 1,801 in 20.026 s | 89.931/s | 11.111 ms |

STK FPS divides its frame count by the full replay duration. The Android rate uses a separate, approximately 20-second window of presentation timestamps for the foreground QA game's SurfaceView. That rate describes Android's compositor and does not measure frames shown by the Windows launcher or physical monitor.

The table pools both passes per engine. Each mean divides total scope duration by all positive `Main loop` samples, including frames where a child scope recorded zero duration.

| CPU-thread elapsed time per frame | RustEmuDroid | BlueStacks |
| --- | ---: | ---: |
| `Main loop` | 12.069 ms | 11.112 ms |
| `IrrDriver update` | 10.636 ms | 10.644 ms |
| `drawAll()` | 7.083 ms | 2.178 ms |
| `VK endScene` | 3.171 ms | 8.301 ms |
| `acquire` | 1.881 ms | 7.506 ms |
| `record` | 0.403 ms | 0.144 ms |
| `submit` | 0.082 ms | 0.074 ms |
| `present` | 0.361 ms | 0.452 ms |
| `Main loop` minus `IrrDriver update` | 1.433 ms | 0.469 ms |

BlueStacks spent more elapsed time in `vkAcquireNextImageKHR` and less in `drawAll()`. Total `IrrDriver update` differed by only 0.007 ms per frame, while the rest of `Main loop` averaged 0.964 ms more on RustEmuDroid. The acquire interval includes waiting for a swapchain image and cannot establish GPU execution time or a GPU bottleneck. The tests ran as two samples per engine in separate batches.

The APK has SHA-256 `dbde91eb1451e5f165cbc09b8d1144d13d35283d5f6752cec52704e3e696ff8e`. The data include game-report digests, all reported graphics fields and all phase summaries. [RustEmuDroid data](windows-vulkan-90hz-scoped-native.json) · [BlueStacks Pie64 data](windows-vulkan-90hz-scoped-pie64.json)

To regenerate the aggregate from private QA evidence, place the operator's receipt (`apk-receipt.json` for RustEmuDroid or `receipt.json` for BlueStacks) and `results.json` beside the two `lap-N` directories. Each lap needs its game report, CPU profile, renderer log and active-display receipt. Include the optional `presentations.json` only for a valid SurfaceView capture. The exporter verifies the APK scope, ABI reported by the runner, replay duration, effective 90 Hz, Vulkan evidence and matching graphics fields. It omits timestamps, process IDs, layers and local paths.

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
