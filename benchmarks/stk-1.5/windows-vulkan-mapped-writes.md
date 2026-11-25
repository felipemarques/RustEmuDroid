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
