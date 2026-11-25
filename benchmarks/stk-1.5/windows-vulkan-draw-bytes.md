# Vulkan draw-buffer write volume in SuperTuxKart 1.5

Object records accounted for **96.295%** of the CPU-side Vulkan payload counted in one Black Forest replay. Uniform data and indirect commands together accounted for **2.688%**, averaging **5,768.8 bytes per game frame** and peaking at **10,272 bytes**. This experiment measures the payload written by selected STK functions. Its bytes do not determine the time spent writing or waiting on those buffers. [Summary and SHA-256 manifest](windows-vulkan-draw-bytes.json) · [Compressed source traces](windows-vulkan-draw-bytes-traces.zip)

## Conditions and frame selection

One Android 17 Google Play r7 x86-64 QA guest ran both replays in order, first with the byte counters off and then with them on. Both used the same signed STK 1.5 QA APK, Android Emulator 37.3.10 candidate, four vCPUs, RTX 5070 Ti host Vulkan, Android 1600 × 900 at 90 Hz, High settings, Vulkan FIFO and matching values for all 21 graphics fields in the game reports. The candidate's Vulkan cached-host option was enabled and its experimental WHPX TSC synchronization was disabled throughout. A media-toolchain build was running concurrently on the host. The original game reports and CPU CSVs preserve that context for auditing. The FPS and elapsed timings from these instrumented laps are excluded from the player ranking and cannot establish the cost of enabling the counters.

The original profiler and byte CSV are aligned by row index. The ON capture closed with 3,420 rows. Row 0 is the prelude and has both zero main-loop duration and zero byte counters. Every other row has a positive `Main loop(1)`, matching the report's 3,419 game frames. Across those rows, each frame had one storage generation, one storage attempt and one dynamic upload call. The OFF control had 3,429 positive profiler rows and emitted neither a byte CSV nor byte-counter markers. The separate QA APK was removed after the run.

## Bytes per eligible game frame

Shares use the sum of the five byte categories. Percentiles are nearest-rank over the 3,419 eligible frames. Combined rows sum the categories per frame before computing percentiles.

| Payload | Total bytes | Mean | P50 | P95 | P99 | Peak | Share |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Storage: skinning matrices | 4,676,992 | 1,367.9 | 1,216 | 1,792 | 1,792 | 3,200 | 0.637% |
| Storage: object records | 706,486,208 | 206,635.3 | 158,720 | 495,040 | 551,232 | 590,336 | 96.295% |
| Storage: material IDs | 2,781,344 | 813.5 | 784 | 1,556 | 1,636 | 1,648 | 0.379% |
| Uniform data | 5,816,768 | 1,701.3 | 2,032 | 2,032 | 2,032 | 2,032 | 0.793% |
| Indirect commands | 13,906,720 | 4,067.5 | 3,920 | 7,780 | 8,180 | 8,240 | 1.896% |
| **All storage** | **713,944,544** | **208,816.8** | **161,044** | **498,336** | **554,656** | **593,760** | **97.312%** |
| **Uniform + indirect** | **19,723,488** | **5,768.8** | **5,872** | **9,812** | **10,212** | **10,272** | **2.688%** |
| **All counted writes** | **733,668,032** | **214,585.6** | **167,088** | **507,888** | **564,848** | **603,952** | **100%** |

The counters follow the storage writes in `GEVulkanDrawCall::generate` (identity and joint matrices, 64-byte object records and material IDs) and the dynamic-buffer copies (uniform blocks and 20-byte indexed indirect commands). They count payload bytes at the write sites and exclude untouched alignment gaps. They cover only the instrumented render paths. Total guest memory traffic, cache-line traffic, transfers to the GPU and the latency of those operations remain unmeasured. [Counter patch](vulkan-draw-bytes.patch)

## What the earlier type-5 allocation trace establishes

A separate SDK17 VMA capture saw the storage allocation on guest type 2, heap 1, with flags `0x6`. It saw the uniform/indirect allocation on guest type 5, heap 0, with flags `0x7` (device-local, host-visible, host-coherent). The byte-counter replay identifies the same buffer roles and does not independently record its chosen memory types. Memory-type indices describe a device's own table. The equivalent guest flags and heap were reported on type 4 in the Kemu and BlueStacks captures. [SDK17 allocation trace](windows-draw-buffer-allocations-sdk17.json) · [Cross-guest mapped writes](windows-vulkan-mapped-writes.md)

At gfxstream revision `501867e6a3b1a3be7b935cc977d291150846edab`, the host selects the actual memory type from the guest index. With exact **host** flags `0x7`, the allocation helper selects the `MAP_CACHE_WC` protocol hint. The cached-host replacement option excludes original host types flagged device-local or host-cached. On the direct mapping route, gfxstream passes the guest physical address, host pointer and size to the VMM. The blob route carries cache metadata. The WHPX path in QEMU revision `538bcdea5479b3857cd8885cbb9306bfec9c7247` maps the range with access flags. These source paths leave the actual host type and route for this STK allocation, and the effective guest CPU cache policy, open. The byte share also leaves open whether the small uniform/indirect writes have a disproportionately high cost. [gfxstream host type selection](https://android.googlesource.com/platform/hardware/google/gfxstream/+/501867e6a3b1a3be7b935cc977d291150846edab/host/vulkan/vk_memory_allocation_helper.cpp#630) · [Cache hint](https://android.googlesource.com/platform/hardware/google/gfxstream/+/501867e6a3b1a3be7b935cc977d291150846edab/host/vulkan/vk_memory_allocation_helper.cpp#812) · [Cached-host exclusions](https://android.googlesource.com/platform/hardware/google/gfxstream/+/501867e6a3b1a3be7b935cc977d291150846edab/host/vulkan/vk_emulated_physical_device_memory.cpp#53) · [Direct route](https://android.googlesource.com/platform/hardware/google/gfxstream/+/501867e6a3b1a3be7b935cc977d291150846edab/host/vulkan/vk_decoder_global_state.cpp#7134) · [Blob route](https://android.googlesource.com/platform/hardware/google/gfxstream/+/501867e6a3b1a3be7b935cc977d291150846edab/host/vulkan/vk_decoder_global_state.cpp#7908) · [QEMU WHPX mapping](https://android.googlesource.com/platform/external/qemu/+/538bcdea5479b3857cd8885cbb9306bfec9c7247/target/i386/whpx-all.c#1681)

## Reproduce

1. Build the separate Android x86-64 QA package at the pinned STK 1.5 revision. Follow the [Android build and replay recipe](vulkan-end-scene.md) and apply [`vulkan-end-scene.patch`](vulkan-end-scene.patch) followed by [`vulkan-draw-bytes.patch`](vulkan-draw-bytes.patch). Use the same assets for both arms and a separate QA application ID, JNI namespace and storage directory. The manifest records the APK, certificate, replay asset, library and patch digests for this run.
2. Replay Black Forest in High/Vulkan with game VSync enabled on that QA install. Use `--benchmark` for the OFF arm and `--benchmark --qa-draw-bytes` for the ON arm, keeping the same guest and settings. Collect each game's `report.csv`, profiler `quadros.csv` and the renderer lines. For ON, also collect the companion `*.qa-draw-bytes-*.csv` as `on/draw-bytes.csv` and preserve the active and terminal markers in `on/renderer.txt`. The patch accumulates counters in memory and writes the companion when the profiler exports the report.
3. To audit the supplied archive from the root of this repository, run the following in Python 3.10 or newer. The script checks the reports against the profiler and reproduces the statistics and member hashes.

   ```sh
   python -B benchmarks/stk-1.5/summarize-draw-bytes.py benchmarks/stk-1.5/windows-vulkan-draw-bytes-traces.zip
   ```
