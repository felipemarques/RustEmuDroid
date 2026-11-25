# Stock Vulkan host-memory allocation in the Windows kart replay

On November 25, 2026, enabling `VulkanAllocateHostMemory` reduced SuperTuxKart's Black Forest replay throughput from **85.42 to 74.08 FPS (−13.27%)** across four cold boots. Both enabled replays ran below both controls. This series used the **RustEmuDroid v0.21.1 client** and the **stock Android Emulator 37.3.10.0 engine, build 16970787**. It is a historical result for that client and workload. [Per-arm data and relative presentation timestamps](windows-stock-vulkan-host-memory-replay-rtx.json) · [Original game reports and profiler traces](windows-stock-vulkan-host-memory-replay-rtx-traces.zip)

| Arm | Host-memory option | Game frames / time | Game FPS | Android presentations/s | Android P99 (ms) | Panel processed frames/s | QEMU CPU (ms/s) |
|---|---|---:|---:|---:|---:|---:|---:|
| A1 | Off | 3,246 / 38.072 s | 85.26 | 84.66 | 23.17 | 83.02 | 2,199.5 |
| B1 | On | 2,756 / 38.073 s | 72.39 | 70.98 | 39.92 | 68.36 | 2,377.6 |
| B2 | On | 2,885 / 38.070 s | 75.78 | 74.64* | 37.42* | — | — |
| A2 | Off | 3,257 / 38.062 s | 85.57 | 85.09* | 22.91* | — | — |

\* Android-only samples. The guest's foreground SurfaceView and timestamps passed independent checks in B2 and A2. The Windows preview lost foreground, so its panel and process-CPU measurements were excluded. The two qualified Windows samples give one observation per condition. The four game FPS results use the complete replay reports. The pooled rate divides the total frames by the total duration in each condition.

## Where time changed in the replay

Elapsed milliseconds per game frame from the original profiler are shown below. The `fence` and `acquire` scopes are within `endScene`.

| Arm | `drawAll` | `endScene` | `fence` | `acquire` |
|---|---:|---:|---:|---:|
| A1 | 6.433 | 3.712 | 0.143 | 2.547 |
| B1 | 3.484 | 8.731 | 4.499 | 3.030 |
| B2 | 3.481 | 8.113 | 3.564 | 3.397 |
| A2 | 6.393 | 3.708 | 0.142 | 2.536 |

`drawAll` became shorter with the stock option enabled. Both enabled replays spent more elapsed time inside `endScene`, especially at the fence wait. The scope measures elapsed time in the game's render thread and includes blocking. The source of the longer wait and the game's actual host-memory backing remain unverified. In a separate synthetic stock test, the same feature substantially improved CPU writes to a compatible mapped buffer. [Profiler instrumentation](vulkan-end-scene.patch) · [Independent stock memory probe](windows-stock-vulkan-host-memory.md)

## Reproduction and scope

Use the recorded stock QEMU, emulator launcher, gfxstream DLL, unchanged advanced-feature INIs and signed x86-64 STK 1.5 QA APK. The JSON contains their SHA-256 digests. Use Android 17 Google Play r7 with four vCPUs under WHPX, the RTX 5070 Ti and a 1600 × 900 guest at 90 Hz. Run the same Black Forest High Vulkan replay with FIFO, the same 21 graphics fields, 100% render scale, the game's VSync enabled and a configured `max_fps=90`. Fit the Rust window to 800 × 450 physical pixels. Use one excluded warm-up and one scored replay per cold boot. [Replay harness](compare.py)

Set `ANDROID_EMULATOR_FEATURES` to `-VulkanAllocateHostMemory,-VulkanBatchedDescriptorSetUpdate` for A and `VulkanAllocateHostMemory,-VulkanBatchedDescriptorSetUpdate` for B. Keep the patched cache and TSC environment variables unset. Verify the live Windows process and renderer identities, inheritance in the launcher and QEMU, and explicit stock override states in the closed engine logs. Run in A/B/B/A order with the same binaries. The logs confirmed each requested state and contained none of the tested host-import failures. The ABI, workload, FIFO present mode and report settings were checked against the recorded game output.

Measure the guest SurfaceView for about 20 seconds per replay and recompute P99 from its timestamps. The Rust panel counter uses its own intervals and counts frames processed for drawing. QEMU CPU time sums all process threads over the SurfaceView window. Actual scanout and GPU execution time were outside the measurement. The QA package was removed, the usual v0.22.0 Android Home was restored as the only VM at 60 Hz, and the installed Cipó Livre APK matched its pre-test hash.
