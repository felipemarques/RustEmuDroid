# CPU writes to Vulkan storage buffers on Windows

The Black Forest replay spent about 2.9 ms more render-thread CPU time per frame preparing draw commands on SDK Android 17 than on Kemu Android 16. In this part of STK, the game writes object transforms, skinning data and materials into a mapped storage buffer. This separate Android probe measures how quickly one thread can write to each compatible Vulkan memory type. [Game replays](windows-draw-all-android17.md) · [STK draw buffer allocation](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/lib/graphics_engine/src/ge_vulkan_draw_call.cpp#L1604-L1626)

## Installed SDK 37.3.10, Android 17 Google Play

The probe ran on the habitual RustEmuDroid 0.21.1 Android Home, with one VM and the RTX 5070 Ti. Android reported `Goldfish GFXStream (NVIDIA GeForce RTX 5070 Ti)`. The table gives median **thread CPU milliseconds per 4 MiB written**, across six batches per cell.

| CPU write destination | Heap | Vulkan flags | `memcpy` | 32-bit stores |
|---|---:|---|---:|---:|
| Aligned ordinary memory | — | — | 0.061 ms | 0.242 ms |
| Type 2 | 1 | Host-visible, coherent | 9.614 ms | 82.345 ms |
| Type 3 | 1 | Host-visible, coherent, cached | 0.058 ms | 0.196 ms |
| Type 5 | 0 | Device-local, host-visible, coherent | 16.796 ms | 133.622 ms |

For both write patterns the cacheable mapping performed similarly to ordinary memory on this device. The uncached mappings were sensitive to the write pattern. The experiment measures synthetic writes with no GPU submission. It does not establish which allocation STK uses, how many such writes STK issues, or how much an emulator change can improve a game. A host build was running during this diagnostic. [All 48 elapsed and thread CPU measurements](windows-mapped-writes-sdk-stock.tsv) · [Per-device Vulkan properties](windows-kart-vulkan-capabilities.json)

## Which memory SuperTuxKart used

During a separate Black Forest replay on the experimental SDK engine with the cached-allocation switch disabled, all 18 mapped storage-buffer allocations observed in the instrumented draw-call code used **type 2, heap 1**. All 12 mapped uniform/indirect allocations used **type 5, heap 0**. The collector matched the 30 consecutive allocation records to one game process and checked their flags against the same guest's Vulkan inventory. This identifies the allocation used by the game for object data. The trace counts allocations, while the bytes written per frame remain unmeasured. [All 30 allocations](windows-draw-buffer-allocations-sdk17.json) · [Game instrumentation patch](vulkan-draw-buffer-memory.patch)

## Effect of enabling cached allocations

Two cold boots used the same experimental QEMU, stock gfxstream DLL, RTX 5070 Ti, 90 Hz Android 17 profile and probe executable. The virtual-clock fix stayed off. Each boot ran six batches per cell. The values below are **median elapsed milliseconds per 4 MiB**. Use elapsed time for these short samples because the guest thread CPU counter returned zero in three of the 48 enabled measurements. All 96 elapsed measurements were positive. [Option disabled](windows-mapped-writes-cache-off.tsv) · [Option enabled](windows-mapped-writes-cache-on.tsv)

| CPU write destination | Bulk copy, off → on | 32-bit writes, off → on |
|---|---:|---:|
| Ordinary memory | 0.068 → 0.070 ms | 0.214 → 0.228 ms |
| Type 2, used for game object storage | **14.540 → 0.069 ms** | **99.596 → 0.196 ms** |
| Type 3, already cached | 0.090 → 0.077 ms | 0.290 → 0.212 ms |
| Type 5, used for uniform/indirect buffers | 20.751 → 21.595 ms | 159.401 → 179.743 ms |

The option brought type 2 writes into the range of ordinary memory. Type 5 remained slow. Both boots advertised the same guest memory types. The original thread-CPU qualification rule could not classify the enabled arm because it required positive readings throughout. The elapsed measurements and the game's actual use of type 2 motivated a separate four-replay game test. The table measures memory access only. [Probe source](mapped-write.c) · [Validation and medians](summarize-mapped-writes.py)

A separate A/B on Google's stock Windows SDK used `VulkanAllocateHostMemory` with the patched cache and clock changes absent. Type 2 fell from **10.108 to 0.086 ms** for bulk copy and **91.843 to 0.290 ms** for 32-bit stores. The storage buffer's compatibility mask excluded type 5 in the enabled boot. [Stock SDK test and raw samples](windows-stock-vulkan-host-memory.md)

## How Kemu handles the game's buffer

Kemu 0.7.1 exposed an Adreno 830 to its Android 16 guest while the Windows gfxstream log selected the RTX 5070 Ti. The RustEmuDroid 0.21.1 window ran Kemu’s engine and system image. In one 90 Hz boot we ran the same x86-64 probe, then played the instrumented Black Forest replay. All 18 mapped object-storage allocations used type 2, heap 1, flags `0x6`, just as on the Android 17 SDK engine. All 12 uniform/indirect allocations used Kemu type 4, heap 0, flags `0x7`. [Full allocation trace](windows-draw-buffer-allocations-kemu16.json)

| Median elapsed time per 4 MiB written to the game's storage memory | Bulk copy | 32-bit writes |
|---|---:|---:|
| Android 17, SDK-based engine, cache off | 14.540 ms | 99.596 ms |
| Android 17, same engine, cache on | 0.069 ms | 0.196 ms |
| Android 16, Kemu engine | 0.084 ms | 0.207 ms |

Kemu already reached roughly the same write speeds that the cache option enabled on the Android 17 engine. The advertised flags alone therefore cannot explain the difference in write time. These observations identify the memory used by the game and measure the CPU cost on both engines. The mechanism that backs Kemu's mapping remains unverified. Kemu’s separate device-local type 4 showed both fast and slow batches. All 48 raw samples are available. [Kemu probe](windows-mapped-writes-kemu.tsv) · [Four scored Android 17 game replays](windows-cached-memory-kart.md)

## BlueStacks Pie64's guest Vulkan driver

REA inspected two x86-64 Android libraries extracted from the private copy of the BlueStacks 5.22.166.1003 system image. The default Vulkan driver declares a dependency on `libvulkan_enc.so`. Its imported allocation, mapping and binding functions match symbols exported by that library, including `gfxstream::vk::ResourceTracker::on_vkAllocateMemory`, `VkEncoder::vkMapMemory` and `VkEncoder::vkMapMemoryIntoAddressSpaceGOOGLE`. This places a gfxstream encoder in the guest driver interface. AOSP's implementation documents the roles of the resource tracker and encoder. The exact BlueStacks source revision and host caching policy are unknown. [ELF matches, artifact hashes and REA evidence IDs](windows-bluestacks-vulkan-encoder.json) · [AOSP resource tracker](https://android.googlesource.com/platform/external/mesa3d/+/cba72e4fb9e0f3c9a10f446355a83a62e8fa8b9d/src/gfxstream/guest/vulkan_enc/ResourceTracker.cpp)

The same Android driver imports the encoder's descriptor-update tracker and its queue command for submitting descriptor sets in batches. The Windows renderer from this installation contains both the corresponding feature name and queue command name. These findings give us a reason to test the corresponding switch in the official SDK. Whether BlueStacks negotiated batching or invoked it in this replay remains open. [Matched guest symbols, Windows renderer hash and string offsets](windows-bluestacks-vulkan-encoder.json)

### Measured writes and game allocations on Pie64

BlueStacks 5.22.166.1003 ran the isolated Pie64 clone with Android 9 (API 28), four vCPUs, a 1600 x 900 display at 90 Hz and the RTX 5070 Ti. The x86-64 STK replay used High Vulkan, FIFO presentation and the same 21 graphics settings as the SDK and Kemu comparisons. The original x86-64 API 28 probe ran from the idle Android Home in a separate boot of the same clone. The table gives median **elapsed / guest thread CPU milliseconds per 4 MiB written**, with six batches per cell.

| CPU write destination | Heap | Vulkan flags | Bulk copy | 32-bit stores |
|---|---:|---:|---:|---:|
| Aligned ordinary memory | - | - | 0.062 / 0.061 | 0.210 / 0.210 |
| Type 2, object storage | 1 | 0x6 | 0.062 / 0.061 | 0.204 / 0.204 |
| Type 3 | 1 | 0xe | 0.065 / 0.065 | 0.225 / 0.224 |
| Type 4, uniform/indirect | 0 | 0x7 | 0.060 / 0.060 | 0.200 / 0.200 |

The game logged **21 mapped object-storage allocations on type 2** and **12 mapped uniform/indirect allocations on type 4**. Both types matched the index, heap and flags measured by the ELF in the separate boot. Type 2 write costs were close to ordinary memory in both timing measures, as on Kemu and on the SDK engine with its cache option enabled. All 48 elapsed and CPU samples were positive. [Raw Pie64 write samples](windows-mapped-writes-pie64.tsv) · [All 33 Pie64 allocation records and provenance](windows-draw-buffer-allocations-pie64.json)

In both boots, the shell Vulkan inventory returned a single empty device record. The ELF and game each reported the RTX through Vulkan. The Vulkan property was empty and the optional process-map check did not establish live library paths. The static encoder link and these timings leave the host mapping policy unverified. The logged replay was excluded from FPS comparison. Both QA fixtures were removed, the clone's scoped FPS settings were restored, and the stock STK package and configuration were preserved. [Paired game measurements](windows-current-three-players.md)

## Reproduction

An independent cold boot of the source-built SDK core reproduced the ordering. After both host builds had finished, the same probe on Android 17 measured type 2 at **10.105 / 87.753 ms** (memcpy / scalar), type 3 at **0.057 / 0.189 ms**, and type 5 at **16.917 / 133.391 ms**. The ordinary allocation took **0.069 / 0.191 ms**. The TSC option was disabled, and this executable contained no cached-allocation switch. The buffer size, access masks, GPU and six-batch workload matched the stock test. [Raw output from the second run](windows-mapped-writes-rebuilt-off.tsv) · [Source-built core and Android identity](whpx-tsc-sync.md#android-17-cold-boots-on-the-same-executable)

The probe creates a 64 KiB storage/transfer-source buffer for each compatible, host-visible memory type, checks the resource requirements, binds and maps each allocation once, and compares it with an aligned ordinary allocation. The type mask was `0x2f`, the allocation requirement was 65,536 bytes and alignment was 16 bytes. Each batch writes the same 64 KiB region 64 times. Target order reverses on alternate rounds and the two write methods exchange order. A final CPU fence is included in the timing. Reading back and verifying the bytes happens after timing. The probe then releases all resources. [Probe source](mapped-write.c)

Build with the Android NDK's Clang. The measured x86-64 Android API 28 PIE built with NDK `30.0.14904198` on Windows and SHA-256 `e5dc2a7c66e3eca750b6b06ff18a39b713fe6884f3927df2447c2788bd38436e`. The following assumes the current directory contains `mapped-write.c` and the NDK's `clang` is on `PATH`.

```sh
clang --target=x86_64-linux-android28 -std=gnu11 -O2 -Wall -Wextra -Werror -fPIE -pie mapped-write.c -lvulkan -o mapped-write-x86_64
```

Use the ADB transport of the selected x86-64 QA guest. Copy the ELF to a fresh filename under `/data/local/tmp`, give it owner execute permission, run it with a 50-second timeout, and capture stdout verbatim. A successful run ends with `complete` and exits zero. Remove the temporary ELF. The following command verifies the header, resource mask, complete sample count and timing matrix before calculating medians.

```sh
python summarize-mapped-writes.py windows-mapped-writes-sdk-stock.tsv
```

The upstream gfxstream source contains `VulkanAllocateUncachedHostVisibleAsCached`. When enabled on a supported path, it selects a cached coherent memory type on the same heap for eligible host-visible allocations and checks the resource's compatible memory types. Device-local and external-memory paths have additional exclusions. The SDK baseline above used its regular configuration. [Upstream gfxstream implementation](https://android.googlesource.com/platform/hardware/google/gfxstream/+/501867e6a3b1a3be7b935cc977d291150846edab/host/vulkan/vk_emulated_physical_device_memory.cpp)

The [experimental QEMU patch](gfxstream-cached-host-qa.patch) exposes this upstream feature to the private test process through `RUSTEMUDROID_QA_VULKAN_CACHED_HOST=1`. It applies to the same pinned Windows source checkout used for the clock experiment. The resulting x86-64 executable has SHA-256 `0698696ebcc59745f57ffc3c975382a399a83ebbe80e3f30ee0d982a9ea481cc`. A same-executable comparison must leave the TSC option unset in both boots. The guest can continue to advertise the same type and flags when the renderer chooses a different host allocation. The observable outcomes are the measured write cost and game performance. [Build manifest and Windows commands](whpx-tsc-sync.md#reproduce-the-windows-source-checkout)
