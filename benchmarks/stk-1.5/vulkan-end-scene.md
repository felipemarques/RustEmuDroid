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

## Windows Android 17 results

RustEmuDroid 0.18.1 ran the standalone x86-64 QA APK in an Android 17 Google Play AVD on a GeForce RTX 5070 Ti. Both Black Forest replays used High/Vulkan, 1600×900 at render scale 1, a 90 FPS game cap and game VSync. Android reported a 90 Hz active mode during both runs. STK reported Vulkan 1.3 through Goldfish GFXStream and selected FIFO (`VkPresentModeKHR=2`) on both runs. The 21 exported graphics fields matched between runs and matched the earlier stock x86-64 runs A1 and A2 at 90 Hz.

| Replay | STK frames | STK duration | STK FPS | Android SurfaceView presentations | SurfaceView rate | SurfaceView P99 interval |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 3,159 | 38.157 s | 82.790 | 1,626 in 20.024 s | 81.204/s | 23.850 ms |
| 2 | 3,157 | 38.073 s | 82.920 | 1,647 in 20.022 s | 82.258/s | 23.483 ms |

STK FPS divides its frame count by the full replay duration. The Android rate uses a separate, approximately 20-second window of presentation timestamps for the foreground QA game's SurfaceView. That rate describes Android's compositor and does not measure frames shown by the Windows launcher or physical monitor.

| Scope | Replay 1 P50 | Replay 1 P95 | Replay 2 P50 | Replay 2 P95 |
| --- | ---: | ---: | ---: | ---: |
| `Main loop` | 11.392 ms | 17.083 ms | 11.476 ms | 17.061 ms |
| `VK endScene` | 2.428 ms | 6.680 ms | 2.455 ms | 6.744 ms |
| `acquire` | 1.003 ms | 5.588 ms | 0.922 ms | 5.665 ms |
| `present` | 0.302 ms | 0.758 ms | 0.307 ms | 0.848 ms |

`acquire` accounted for the largest measured share of `endScene` time in both runs. These samples cannot attribute that time to a specific GPU or compositor bottleneck. Both replays use the instrumented build, so the cost of the added markers remains unmeasured. An equivalent BlueStacks run with the same x86-64 APK and verified 90 Hz Vulkan mode is still needed for a paired comparison.

The APK has SHA-256 `dbde91eb1451e5f165cbc09b8d1144d13d35283d5f6752cec52704e3e696ff8e`. The data include the game reports' digests, all reported graphics fields and all phase summaries. [Sanitized Windows Android 17 data](windows-vulkan-90hz-scoped-native.json)

To regenerate the aggregate from private QA evidence, place `apk-receipt.json` and `results.json` beside the two `lap-N` directories. Each lap needs its game report, CPU profile, renderer log and active-display receipt. Include the optional `presentations.json` only for a valid SurfaceView capture. The exporter verifies the APK scope, ABI reported by the runner, replay duration, effective 90 Hz, Vulkan evidence and matching graphics fields. It omits timestamps, process IDs, layers and local paths.

```sh
python benchmarks/stk-1.5/export-vulkan-end-scene.py /path/to/qa-run \
    --engine RustEmuDroid --engine-version 0.18.1 --android 17 \
    --output benchmarks/stk-1.5/windows-vulkan-90hz-scoped-native.json
```

A paired BlueStacks export uses `--engine "BlueStacks 5 Pie64" --android 9` with that player's version and `--match-reference benchmarks/stk-1.5/windows-vulkan-90hz-scoped-native.json`. The exporter checks the same APK digest, ABI, graphics fields, active display period and Vulkan present modes before writing that pair. Record host, driver and renderer conditions separately when interpreting the results.
