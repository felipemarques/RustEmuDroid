# Stock Vulkan host-memory allocation in SuperTuxKart on RustEmuDroid 0.22.0

Enabling the stock Android Emulator's `VulkanAllocateHostMemory` option reduced the pooled SuperTuxKart replay rate from **85.74 to 71.76 FPS** on the RTX 5070 Ti, a **16.3% decrease**. Each enabled replay also had fewer Android presentations per second and a longer P99 presentation interval than either disabled replay.

The November 25, 2026 test used four cold boots of the same Android 17 Google Play r7 x86-64 QA profile, in order OFF, ON, ON, OFF. Each boot used the official RustEmuDroid 0.22.0 Windows executable, stock Android Emulator 37.3.10, WHPX, four vCPUs and a 1600 x 900 guest at 90 Hz. It ran one warmup and one measured Black Forest replay with the same x86-64 SuperTuxKart 1.5 QA APK, High graphics at 100% render resolution and Vulkan FIFO verified in the game log. Descriptor batching, the experimental graphics cache and experimental TSC synchronization were off throughout. The stock logs confirmed the requested setting in every boot. The official client version and SHA-256 were verified for all four launches. [Run data and binary hashes](windows-stock-vulkan-host-memory-replay-v0220-rtx.json)

| Run | Host allocation | Game frames | Game FPS | Android presentations/s | Android P99, ms | Rust panel frames/s |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| A1 | OFF | 3,261 | 85.67 | 85.37 | 23.41 | 83.42 |
| B1 | ON | 2,739 | 71.91 | 70.43 | 42.05 | 67.48 |
| B2 | ON | 2,726 | 71.60 | 70.18 | 41.91 | 67.25 |
| A2 | OFF | 3,266 | 85.80 | 85.36 | 22.91 | 82.30 |

Game FPS uses the full profiler report. Each Android measurement uses about 20 seconds of unique presentations for the foreground game SurfaceView. The Windows sampler verified the foreground preview and moving game at 800 x 450 in all four runs. The panel counter measures frames processed for drawing. Physical display scanout was outside the test. The game and Android clocks cover different windows.

The guest game thread spent less elapsed time in `drawAll` and more in Vulkan `endScene` as the option changed from OFF to ON. Weighting by the measured game frames, `drawAll` went from **6.294 to 3.396 ms/frame** and `endScene` went from **3.817 to 8.967 ms/frame**. Within `endScene`, mean fence wait rose from **0.141 to 4.686 ms/frame** and image acquisition from 2.667 to 3.081 ms/frame. Queue submit stayed near 0.07 ms/frame. The fence P95 was 0.264–0.287 ms with the flag off and 14.654–15.014 ms with it on. These nested scopes include time spent waiting on API calls. They do not identify GPU execution time or the memory backing used by any specific allocation.

During the qualified Android sample, the measured QEMU process consumed 2,181–2,280 ms of Windows CPU time per elapsed second with the option off and 2,394–2,465 ms/s with it on. The Rust presenter consumed 246–267 ms/s off and 203–245 ms/s on. These are user plus kernel time across each process's threads anchored to the Android sample. The counters cover the QEMU and Rust processes without attributing those totals to individual render calls.

The earlier **0.21.1** ABBA recorded 85.42 FPS off and 74.08 FPS on. Its four game profiles qualified, while only one Windows preview sample per condition qualified. Each series measures one client version and is reported separately. The 0.22.0 run closed its four QA guests and restored the single habitual 60 Hz Android Home. [Earlier 0.21.1 replay data](windows-stock-vulkan-host-memory-replay-rtx.json)

The trace archive contains each run's `report.csv`, `quadros.csv` and Vulkan renderer lines. Compute replay FPS as `1000 * frame_count / profiling_duration_ms` and pool conditions by summing frame counts and durations. For elapsed time per frame, retain rows with a positive `Main loop` and include zeros from the child scopes. The JSON gives Android timestamps relative to each sample's first presentation. Divide their count by `duration_s` for presentations per second, and take the zero-based index `floor(0.99 * (n - 1))` over the `n` sorted gaps for P99. The counter durations and process CPU deltas are in the same JSON. [Trace archive](windows-stock-vulkan-host-memory-replay-v0220-rtx-traces.zip) · [Scope parser](summarize-vulkan-end-scene.py)
