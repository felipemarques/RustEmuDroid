# Android 17 Windows preview comparison with fitted gRPC and DWM

The Windows release running its own fitted gRPC panel averaged **85.383 FPS** in the SuperTuxKart 1.5 Black Forest replay. Android 17 with the visible SDK Qt window and the separate DWM viewer averaged **86.132 FPS**. Android SurfaceView rates were **84.565** and **85.629** per second in the sampled race window. [Run data and binary hashes](windows-android17-fit-dwm-stk.json)

## Configuration and results

Both modes used Android Emulator 37.3.10, the same private Android 17 Google Play AVD, 4 vCPUs, 8 GiB, the RTX 5070 Ti, the same x86-64 replay APK and 21 identical recorded High Vulkan graphics settings. The guest rendered 1600 x 900 at 90 Hz with the Vulkan FIFO present mode. Both previews displayed the image at 800 x 450 in the same part of the same RTX-connected 100 Hz monitor. Each boot had one full excluded warm-up replay.

A used the official RustEmuDroid v0.21.0 executable, its Windows Rust/Glow panel, and its native gRPC transport with Fit frames to window turned on. Its sustained video raster was 800 x 450 RGBA. The SDK ran with `-qt-hide-window`. B used the same installed emulator launcher and QEMU, the same AVD and flags with that Qt visibility flag removed, and the official DWM prototype from the v0.21.0 Actions artifact. The Android runtime refresh preferences were verified in both modes. B's visible Qt window and DWM presenter remained on the same monitor. The three boots ran A1, B1/B2, A2. An initial 896 x 504 Rust pass was excluded because the official visible Qt drawable remained at 800 x 450.

| Lap | Windows presenter | STK FPS | Android SurfaceView frames/s | P95 / P99 Android interval (ms) | Intervals >33.33 ms | QEMU CPU (ms/s) | Presenter CPU (ms/s) | Rust panel processed frames/s |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A1 | Rust fitted gRPC | 84.879 | 83.667 | 21.69 / 23.81 | 2 | 2317.23 | 252.49 | 81.93 |
| B1 | Qt + DWM | 86.134 | 85.706 | 14.54 / 23.10 | 1 | 2094.20 | 0.00 | n/a |
| B2 | Qt + DWM | 86.129 | 85.552 | 20.58 / 23.43 | 0 | 2120.22 | 2.34 | n/a |
| A2 | Rust fitted gRPC | 85.885 | 85.462 | 18.86 / 23.03 | 0 | 2231.29 | 276.28 | 82.65 |

FPS uses the complete game profiler report. SurfaceView uses new Android presentations in a separate, approximately 20-second window in the active replay. The CPU columns use Windows user-plus-kernel time for QEMU and the selected presenter over the same interval. QEMU plus the presenter averaged **2,538.65 ms CPU/s** with fitted gRPC and **2,108.38 ms CPU/s** with Qt + DWM. The Rust counter measures processed panel frames when fitted video is active. The JSON also records the Windows working set and private bytes of each selected process at both boundaries. The primary comparison measures Android cadence and process costs. The optional Windows observation below covers changes in the composed image.

Static Home captures compared Android pixels with the destination at the matched viewport. The timed passes verified two distinct race scenes. On the DWM passes, each scene was sampled simultaneously from the visible Qt crop and the DWM preview. A and B compare the two working Windows preview configurations, including the necessary Qt visibility difference and separate boots. The report describes one hardware and Android stack. In this series, the selected process total was 16.95% lower with Qt + DWM.

The primary CPU figures cover the emulator process and the selected Rust presenter. Windows also performs composition in `dwm.exe`; its CPU time and the GPU's work are outside that total. Saving 430 ms of CPU time per second across the two measured processes is about 0.43 of one logical core on this machine.

## Separate Windows image-change samples

One additional 14-second replay sample per configuration captured only the 800 x 450 destination on the RTX desktop. The Windows Desktop Duplication reader copied 12 scanlines and counted material RGB changes from one acquired desktop image to the next. These samples are separate from the four timed game and CPU passes. The Qt sample used its own extra Android boot and warm-up.

| Preview | Observed changes/s | Desktop updates superseded | Sampler CPU (ms/s) | Readback P95 (ms) | Full diagnostic replay FPS |
| --- | ---: | ---: | ---: | ---: | ---: |
| Rust fitted gRPC | 80.43 | 27/1626 (1.66%) | 117.71 | 3.21 | 85.394 |
| Qt + DWM | 82.00 | 6/1386 (0.43%) | 108.33 | 2.41 | 76.044 |

The counts describe changes observed in sampled regions of desktop composition. Superseded updates are global to the display and cannot be assigned to the preview. Repeated images, unobserved changes, and the separate physical scan-out stage are outside these counts. The observed difference is within the acquisition-loss budget. The full diagnostic replays also ran at different game rates. The pair is inconclusive about a Windows preview cadence advantage.
