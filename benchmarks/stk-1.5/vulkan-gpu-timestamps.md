# SuperTuxKart 1.5 Vulkan GPU timestamps

The [timestamp patch](vulkan-gpu-timestamps.patch) samples the graphics queue during the active part of the built-in Black Forest benchmark. Use it alongside the [CPU scopes](vulkan-end-scene.md) to compare elapsed Vulkan calls with elapsed work on the graphics queue.

## Build the QA variant

Start with a separate STK checkout at [1.5 commit `1fb491f5`](https://github.com/supertuxkart/stk-code/tree/1fb491f507216c5d181ccd85f29ff08eca003827). This patch applies after the CPU phase patch. Follow its instructions for an isolated, signed Android QA package, the 1.5 Black Forest assets and the selected ABI.

```sh
git apply --check /path/to/RustEmuDroid/benchmarks/stk-1.5/vulkan-gpu-timestamps.patch
git apply /path/to/RustEmuDroid/benchmarks/stk-1.5/vulkan-gpu-timestamps.patch
cd android
PROJECT_VERSION=1.5 PROJECT_CODE=328 BUILD_TYPE=beta COMPILE_ARCH=x86_64 \
    STK_QA_VULKAN_END_SCENE=1 STK_QA_VULKAN_TIMESTAMPS=1 bash ./make.sh
```

Keep the standard game installed separately. Verify the QA package ID, signature, ABI, asset hashes and actual guest renderer. Save the timestamp APK before building a control with the same source, signer and settings and `STK_QA_VULKAN_TIMESTAMPS=0`. On desktop the equivalent CMake option is `-DSTK_QA_VULKAN_TIMESTAMPS=ON`.

The signed Android x86-64 variant built successfully with NDK r28b. It retained the CPU markers, JNI namespace and SDL dependency. All 5,600 APK entries outside signing metadata and `libmain.so` matched the timestamp-disabled control by CRC and unpacked size. Actual query support remains a per-guest check.

## Capture one replay

1. Select a verified test guest and start a small log collector before launching each fresh benchmark process. Keep a separate log per process. Stop logging after STK exits normally and emits its terminal counter record.

   ```sh
   adb -s "$serial" logcat -T 1 -v brief Irrlicht:V '*:S' \
       | grep --line-buffered 'STK_VK_GPU_V1 ' > vulkan-gpu.log
   ```

2. Run the same High/Vulkan Black Forest benchmark as the [CPU test](vulkan-end-scene.md#build-and-record). Confirm the 21 game settings, resolution, render scale, VSync, actual refresh and swapchain mode. The analyzer expects a single Vulkan device lifetime. Check the PID in the log if a previous process emitted a line just before capture.

3. Run the analyzer and keep the CPU and engine reports from that replay with the output.

   ```sh
   python benchmarks/stk-1.5/summarize-vulkan-gpu.py vulkan-gpu.log > vulkan-gpu.json
   ```

The log reports the selected graphics family, `timestampValidBits`, `timestampComputeAndGraphics` and `timestampPeriod`. A zero-bit graphics family or a failed query pool produces a disabled record and the benchmark continues. Keep the submitted, accepted, unavailable and discarded counts. The analyzer checks that all recorded batches are present and converts tick deltas to milliseconds. A forced stop before the terminal record makes the capture incomplete.

## Interpret the interval

Two queries per frame-in-flight slot mark the first render pass at `COLOR_ATTACHMENT_OUTPUT`, the same stage used for the swapchain semaphore wait, and the end of render recording at `BOTTOM_OF_PIPE`. The patch reads 64-bit results with availability after the slot's existing fence and uses modular subtraction at the reported valid bit width. It batches 64 values per log entry. Swapchain recreation checks the old fences before clearing the slot state. Normal rendering adds no query wait. Final teardown can wait if queried submissions are still pending.

The interval covers graphics queue execution between the selected stages. The timing excludes the CPU's image acquisition and presentation calls and can omit work executed before the start marker. Queue contention can affect the interval. Use a matched timestamp-disabled control to check overhead from timestamp commands, result reads and logging. Keep game FPS, Android SurfaceFlinger cadence and CPU call timings as separate measurements. [Vulkan timestamp queries](https://docs.vulkan.org/spec/latest/chapters/queries.html#queries-timestamps)
