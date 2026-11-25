# SuperTuxKart on the integrated Radeon 780M

On the Ryzen 9 8945HS test PC, changing how the experimental Windows engine allocates host-visible Vulkan memory raised SuperTuxKart from **84.36 to 90.17 game FPS (+6.89%)**. The game rendered the same High-quality Black Forest replay at 1600 × 900 with a 90 Hz Android display. All four cold boots used the same RustEmuDroid v0.21.1 window, Android 17 Google Play image, four vCPUs and QEMU binary. Each boot ran one warm-up and one measured replay. [Per-frame data and configuration](windows-cached-memory-radeon780m.json)

| Boot | Vulkan cache | Game FPS | Android presentations/s | Android P99 interval | Rust panel frames/s |
|---|---|---:|---:|---:|---:|
| A1 | Off | 84.59 | 84.36 | 23.10 ms | 81.61 |
| B1 | On | 90.17 | 89.95 | 12.88 ms | 87.62 |
| B2 | On | 90.17 | 89.95 | 13.34 ms | 85.83 |
| A2 | Off | 84.13 | 83.17 | 23.22 ms | 80.06 |

Across the two runs per condition, the Android game surface went from **83.76 to 89.95 presentations/s**. Intervals longer than one and a half refresh periods fell from **248 of 3,352 (7.40%) to 0 of 3,600**. The 800 × 450 Rust panel processed **80.84 and 86.72 frames/s** respectively. The game rate covers each full replay. Android timings cover twenty-second samples. The panel counter covers its own sample within those windows. [Game reports, frame traces and probe output for all four boots](windows-cached-memory-radeon780m-traces.zip)

## Which GPU did the work?

Every guest reported the Radeon 780M through gfxstream. Across each boot's probe, warm-up and replay, Windows recorded QEMU 3D engine time on the Radeon and zero on the other adapter. These counters identify the adapter over that whole period. Both conditions used Vulkan FIFO and enabled the engine's shared-memory path with `-feature -VulkanAllocateDeviceMemoryOnly`. The WHPX clock correction was off in all four runs.

In a separate mapped-write probe within each boot, 4 MiB of scalar writes to guest type 1 (heap 1, property flags 0x6) took a median **105.43 and 121.68 ms** with the cache off and **0.258 and 0.281 ms** with it on. The probe exposed three eligible Vulkan memory types with caching off and six with it on. The run records include the complete probe output. Allocation tracing identified the kart's actual storage-buffer type on the RTX. The Radeon data here identify the synthetic probe's type only. [RTX allocation tracing](windows-vulkan-mapped-writes.md)

## How this fits the earlier comparison

At the same 90 Hz target on this Radeon, original Kemu Qt reached **90.02 game FPS** and official BlueStacks Pie64 **89.72** with the same x86-64 APK, resolution, preset and Vulkan FIFO mode. Each completed two timed replays. The Android images, host viewports and sampling windows differed. [All three players on the Radeon 780M](windows-current-three-players-radeon780m.md)

In the earlier **60 FPS, OpenGL ES** test on this Radeon, RustEmuDroid scored **55.49**, Kemu **59.87** and BlueStacks **59.99**. The 90 Hz Rust figures above use a different renderer, guest and target. The v0.22.0 Windows client can select the experimental engine per Android profile. The four measured replays used the v0.21.1 client. [Earlier three-player Radeon measurements](radeon-780m/README.md) · [Windows engine selection](../../docs/releases/v0.22.0.md)
