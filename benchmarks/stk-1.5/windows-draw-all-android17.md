# SuperTuxKart Vulkan scene preparation on Windows Android 17

The same signed x86-64 SuperTuxKart 1.5 QA APK ran Black Forest High with Vulkan FIFO on the RTX 5070 Ti. RustEmuDroid 0.21.1 used Android Emulator 37.3.10 and Android 17 Google Play, four vCPUs, 1600 × 900 at active 90 Hz and an 800 × 450 fitted Windows panel. All 21 graphics fields matched. A retained the detailed CPU profiler. B added elapsed and CPU timestamps inside the camera 0 Vulkan draw call. Both modes kept the original benchmark report. One warm-up per mode preceded the four measured replays in one QA boot.

| Pass | Probe | Frames / duration | Game FPS | Android presents/s | Android p99 | Rust panel/s | QEMU CPU ms/s | Presenter CPU ms/s |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | Off | 3,227 / 38.057 s | 84.79 | 84.41 | 22.87 ms | 81.33 | 2324 | 225 |
| 2 | On | 3,239 / 38.068 s | 85.08 | 84.71 | 23.47 ms | 82.32 | 2299 | 226 |
| 3 | On | 3,233 / 38.057 s | 84.95 | 84.54 | 23.29 ms | 82.70 | 2328 | 224 |
| 4 | Off | 3,270 / 38.079 s | 85.87 | — | — | — | — | — |

Replay 4 retained its complete game report and profiler trace. The Windows preview lost foreground at phase start before Android, host CPU and panel sampling.

Pooled game FPS changed from **85.33 to 85.02** when the probe was enabled (-0.32 FPS). The adjacent B–A differences were +0.29 and -0.92 FPS. Game FPS uses 2 A and 2 B replays in one boot. Android cadence was 84.41 → 84.63 presentations/s. The panel processed 81.33 → 82.51 frames/s. Android, panel and Windows CPU use only the available samples (1 A, 2 B).

Game FPS covers the entire replay. Android uses a separate foreground SurfaceView window of around 20 seconds. Rust counts frames processed for drawing over its complete reporting intervals. Windows CPU is total scheduled CPU time across each process's threads aligned to the Android sample. Physical display scanout was outside these measurements.

## Within the instrumented frames

- B1 recorded 3,239/3,239 complete camera 0 samples. The other and long groups had 3,091/3,091 and 148/148 complete samples, respectively.
- B2 recorded 3,233/3,233 complete camera 0 samples. The other and long groups had 3,089/3,089 and 144/144 complete samples, respectively.

A long frame exceeded 1.5 display periods, or 16.666… ms at 90 Hz. The table reports the mean difference between complete long frames and other complete frames. Positive values mean more time on the long frames.

| Scene phase | B1 elapsed Δ (ms) | B1 thread CPU Δ (ms) | B2 elapsed Δ (ms) | B2 thread CPU Δ (ms) |
| --- | ---: | ---: | ---: | ---: |
| updateCache | -0.001 | -0.001 | +0.005 | +0.006 |
| Find camera | +0.000 | +0.000 | +0.003 | +0.003 |
| OnAnimate | +0.076 | +0.043 | +0.078 | +0.051 |
| Render camera and find draw call | +0.001 | +0.003 | +0.013 | +0.014 |
| prepare | +0.040 | +0.039 | +0.043 | +0.047 |
| OnRegisterSceneNode | +0.913 | +0.737 | +0.954 | +0.890 |
| generate | +7.293 | +6.855 | +6.710 | +6.477 |
| drawAllInternal total | +8.324 | +7.676 | +7.807 | +7.488 |

Largest elapsed-time increase by phase. B1: generate +7.293 ms elapsed, +6.855 ms thread CPU · B2: generate +6.710 ms elapsed, +6.477 ms thread CPU.

Elapsed time uses `CLOCK_MONOTONIC`. Thread CPU time uses `CLOCK_THREAD_CPUTIME_ID`. The paired reads have their own cost. A difference between elapsed and CPU time can include waiting, scheduling, and measurement overhead. These measurements localize time within the calling thread. Attribution to a kernel, GPU or driver would require a separate test.

## Evidence and reproduction

APK SHA-256 `df06c5e48cd8420bed3ce50ad9e4b36ae4dd4eb4389e23ece08b9f7a094b291f`. Source patch SHA-256 `4ee59b891babf9ec03ba8010079ad2388a3bc6857cad08d8b15528f43135d7cd`. The QA APK was removed and the single usual Android 17 Home returned at 60 Hz with unchanged preferences.

The two B replay exports contain one folder per replay, each with four exact prechecked files. To reproduce the phase summary, extract the ZIP and run `python summarize-draw-all.py b1 --hz 90` and the same command for `b2`.

[Rates, phase means, coverage and hashes](windows-draw-all-android17.json) · [B replay files](windows-draw-all-android17-b-replays.zip) · [QA patch and protocol](vulkan-draw-all-ab.md) · [Separate three-player series](windows-current-three-players.md)

## Matched Rust window with Kemu Android 16

Kemu used the same released Rust frontend, APK, RTX, Black Forest graphics fields, guest and panel dimensions, Vulkan FIFO and 90 Hz guest display. The A/B/B/A ran in a separate boot. The Android image, virtual GPU interface and QEMU build differed. The Kemu QA APK was removed and the single usual Android 17 Home returned at 60 Hz.

| Engine and guest | Probe off game FPS | Probe on game FPS | B − A (FPS) | Android presents/s off → on | Rust panel/s off → on | Android/host samples A / B |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| SDK Android 17 | 85.33 | 85.02 | -0.32 | 84.41 → 84.63 | 81.33 → 82.51 | 1 / 2 |
| Kemu Android 16 | 90.02 | 90.03 | +0.01 | 89.97 → 90.01 | 87.86 → 86.40 | 2 / 2 |

The game rates pool frame counts and replay durations. The following cells are per-replay means from complete samples. The all-frame CPU mean divides the pooled CPU total by the pooled complete frame count. Fewer than ten complete long frames are marked as sparse.

| Engine and pass | Complete other frames | Complete long frames | Long share of eligible frames | `generate` elapsed (other / long, ms) | `generate` thread CPU (other / long, ms) | `generate` thread CPU (all complete, ms) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| SDK Android 17 B1 | 3,091/3,091 | 148/148 | 4.57% | 3.264 / 10.557 | 3.156 / 10.011 | 3.469 |
| SDK Android 17 B2 | 3,089/3,089 | 144/144 | 4.45% | 3.281 / 9.991 | 3.183 / 9.660 | 3.471 |
| Kemu Android 16 B1 | 3,429/3,429 | 3/3 (sparse) | 0.09% | 0.569 / 0.526 | 0.552 / 0.517 | 0.552 |
| Kemu Android 16 B2 | 3,432/3,432 | 1/1 (sparse) | 0.03% | 0.556 / 0.236 | 0.538 / 0.233 | 0.538 |

The A/B series controls the probe within each engine. Cross-engine phase observations still include the guest and runtime differences. Sparse or absent long-frame groups limit what these runs can say about tails.

The actual GPU was the RTX 5070 Ti in both sessions. In a separate paired test, STK logged the same 15 boolean Vulkan feature choices on both guests. Device queries also found the same storage-buffer alignment and six memory type/heap combinations, with the last two types in a different order. The allocation chosen by the game remains an open measurement. [Vulkan decisions and buffer properties](windows-vulkan-gpu-timestamps.md#vulkan-features-reported-by-the-game)

[Kemu rates, phase means and hashes](windows-draw-all-kemu16.json) · [Kemu B replay files](windows-draw-all-kemu16-b-replays.zip)
