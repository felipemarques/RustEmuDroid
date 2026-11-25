# Host-memory allocation in the stock Windows Android Emulator

On November 25, 2026, two cold boots used the same Google Android Emulator 37.3.10 binaries, Android 17 Google Play r7 x86-64 image, RTX 5070 Ti, four vCPUs, and 1600 × 900 guest at 90 Hz. RustEmuDroid 0.21.1 hosted the isolated guest. The stock emulator log confirmed `VulkanAllocateHostMemory` disabled in A and enabled in B. `VulkanBatchedDescriptorSetUpdate` was disabled in both, and neither boot used the experimental cache or TSC switches.

The same x86-64 probe wrote 4 MiB per batch, with six batches per cell and no GPU submission. Elapsed medians are in milliseconds. The guest thread CPU counter reported zero for one sample in each boot, so the table uses elapsed time. [Probe source](mapped-write.c) · [OFF samples](windows-stock-host-memory-off.tsv) · [ON samples](windows-stock-host-memory-on.tsv)

| Guest memory | Heap | Flags | Bulk copy OFF → ON | 32-bit stores OFF → ON |
|---|---:|---:|---:|---:|
| Ordinary allocation | — | — | 0.066 → 0.069 | 0.208 → 0.219 |
| Vulkan type 2 | 1 | `0x6` | **10.108 → 0.086** | **91.843 → 0.290** |
| Vulkan type 3 | 1 | `0xe` | 0.064 → 0.069 | 0.232 → 0.205 |
| Vulkan type 5 | 0 | `0x7` | 18.771 → unavailable | 146.861 → unavailable |

The storage buffer's compatible type mask changed from `0x2f` to `0x0f`, which excluded type 5 from the enabled probe. Type 2 retained the same guest index, heap and flags and was about 117× faster for bulk copy and 316× faster for 32-bit writes. The engine acknowledged the requested feature and reported none of the tested host-import errors. Per-allocation host backing remains unverified. The sample covers one boot per condition and synthetic CPU writes. Game throughput, compatibility and descriptor batching require their own measurements. [Separate source-built cache comparison](windows-vulkan-mapped-writes.md#effect-of-enabling-cached-allocations)

The stock QEMU SHA-256 was `fae5ffdf64cf8cbd350af1aa726e407c5dce7f335821787cd2b15a5c1671dcf8`. The mapped gfxstream DLL SHA-256 was `4769b6be73db471ccee75bfad2a5590d86991d836258eaf383ea2ffe99757238`. The probe executable SHA-256 was `e5dc2a7c66e3eca750b6b06ff18a39b713fe6884f3927df2447c2788bd38436e`. The original RustEmuDroid 0.22.0 Android Home returned at 60 Hz after both QA VMs closed.
