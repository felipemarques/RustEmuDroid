# SuperTuxKart 1.5 Vulkan frame profiling

[The earlier comparison](windows-vulkan-90hz-profiler.csv) measured all of `IrrDriver update` and the narrower `drawAll` marker. The difference includes several Vulkan operations. The [optional STK patch](vulkan-end-scene.patch) splits the Vulkan `endScene` path into CPU profiler columns using STK's existing marker stack and benchmark export. It also logs the `VkPresentModeKHR` value after each successful swapchain creation. The patch applies to upstream STK 1.5 commit [`1fb491f507216c5d181ccd85f29ff08eca003827`](https://github.com/supertuxkart/stk-code/tree/1fb491f507216c5d181ccd85f29ff08eca003827).

## Build and record

1. In a separate STK checkout at that commit, check and apply the patch.

   ```sh
   git apply --check /path/to/RustEmuDroid/benchmarks/stk-1.5/vulkan-end-scene.patch
   git apply /path/to/RustEmuDroid/benchmarks/stk-1.5/vulkan-end-scene.patch
   ```

2. Use the [STK 1.5 Android build instructions](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/android/README.ANDROID) and [1.5 dependency bundle](https://github.com/supertuxkart/dependencies/releases/tag/1.5). Follow the pinned [build script's SDK/NDK defaults](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/android/make.sh) for the actual checkout. Prepare the same Black Forest assets for both builds and use a local QA signing key. Use optimized builds for timing. For the existing Android replay runner, keep the release package ID and set `PROJECT_VERSION=1.5`. For example, with the dependencies, assets, and signing environment already prepared, build from `android/` with the following command. Choose `COMPILE_ARCH=armv7` instead if that is the ABI verified on the target.

   ```sh
   PROJECT_VERSION=1.5 PROJECT_CODE=1 BUILD_TYPE=release COMPILE_ARCH=x86_64 \
       STK_QA_VULKAN_END_SCENE=1 ./make.sh
   ```

   Use the resulting locally signed release APK on a disposable QA guest without the store package. For a desktop build, enable `-DSTK_QA_VULKAN_END_SCENE=ON` when configuring with CMake, select Vulkan in the game, then pass `--benchmark` on launch. The desktop and Android build switches default to off.

3. On Android, verify the serial and expected renderer for the one QA guest. Launch the QA app once to create its configuration. Run the existing replay script with High/Vulkan and use the same scene, guest refresh, display size, graphics options and ABI across the comparison. This command is an example for an Android 17 AVD named `RustEmuDroid` already running at 90 Hz. `--fps-limit 90` sets the game's preference, while the guest controls the actual mode.

   ```sh
   python benchmarks/stk-1.5/compare.py --rust emulator-5554 --rounds 2 \
       --presets high-vulkan --fps-limit 90 --output end-scene-replay
   ```

   Keep the fresh `report.csv`, `quadros.csv`, and `renderer.txt` together. The replay script copies the `STK QA Vulkan VkPresentModeKHR` lines from the game's `stdout.log` to `renderer.txt`. If swapchain creation appears again, keep every selected mode and note the recreation. Common values are 0 (immediate), 1 (mailbox), 2 (FIFO), and 3 (FIFO relaxed). An unchanged selected mode does not support a FIFO versus non-FIFO comparison. [Vulkan mode definitions](https://docs.vulkan.org/refpages/latest/refpages/source/VkPresentModeKHR.html)

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

Offline validation covered patch application, compilation of the patched Vulkan driver object for Android x86_64 and ARMv7, and CSV export with STK's marker stack and writer in a three-frame fixture with a controlled clock. Full APK linking and a live Vulkan replay with the extra markers remain to be measured.
