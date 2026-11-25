# Showing the Kemu game window through Windows DWM

The DWM preview showed the moving SuperTuxKart replay from Kemu's Qt window, including when the preview fully covered the Qt game surface. In two four-replay comparisons, the guest delivered about 90 frames per second with either preview. The selected Windows processes used less CPU with DWM both with Glow's default Radeon 780M renderer and with Glow on the RTX 5070 Ti. [Run-level data and pixel checks](windows-kemu-dwm-inline-stk.json)

## First comparison with Glow on the Radeon 780M

One private Kemu 0.7.1 Android 16 VM with 4 vCPUs and 8 GiB of guest RAM ran throughout the comparison. All four passes used the same x86-64 SuperTuxKart 1.5 build, Black Forest replay, Vulkan High settings, 1600×900 guest output at 90 Hz, and a FIFO swapchain. The XML set `max_fps=90` and `swap-interval-vsync=1`. The game reported a virtual Adreno 830 and the QEMU log identified the RTX 5070 Ti as its physical Vulkan device.

Both previews were foreground at the two pixel checks and displayed on the same 1920×1080, 100 Hz Windows monitor (PnP model ID RTK2555, reported as Generic PnP Monitor) attached to the RTX. The Qt window remained visible beside the preview. DWM copied the top-level Qt window's 842×474 game region, identified by its native `subWin` child, into a fitted 895×504 area. The released Glow v0.1.1 viewer received 1600×900 inline RGBA through a Python gRPC bridge and drew it into an 896×504 client. Glow's OpenGL renderer reported AMD Radeon 780M Graphics. A separate local RGBA playback with a byte-identical copy of Glow and the Windows per-executable high-performance preference confirmed that the RTX can be selected. The four replay passes predate that check.

| Replay | Preview | STK FPS | Android SurfaceView frames/s | P99 Android interval (ms) | QEMU CPU (ms/s) | Viewer CPU (ms/s) |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| A1 | DWM | 90.015 | 90.059 | 13.823 | 1,138.30 | 0.78 |
| B1 | Inline RGBA + Glow | 89.941 | 90.078 | 13.497 | 1,973.49 | 358.11 |
| B2 | Inline RGBA + Glow | 89.712 | 89.964 | 13.426 | 2,040.27 | 334.26 |
| A2 | DWM | 90.018 | 89.913 | 14.149 | 1,144.21 | 1.56 |
| **Pooled A** | **DWM** | **90.016** | **89.986** | | **1,141.25** | **1.17** |
| **Pooled B** | **Inline RGBA + Glow** | **89.827** | **90.021** | | **2,006.88** | **346.18** |

The game FPS uses profiler frame counts and duration over each full replay. The SurfaceView rate uses newly observed Android presentation timestamps in a separate window of about 20 seconds beginning roughly 14 seconds after replay launch. Its first and last poll midpoints also bound Windows user-plus-kernel CPU time. The pooled rates divide total counts by total sampled time, and the pooled CPU columns are means of the per-run rates.

The sum for QEMU and the viewer averaged **1,142.42 ms CPU/s with DWM** and **2,353.07 ms CPU/s with inline + Glow**. These totals omit the Windows desktop compositor, GPU work, and the gRPC bridge's Python worker. In particular, the recorded PID for the bridge was the Windows virtual environment launcher, so its CPU time is omitted from the result. The four runs show a difference in measured CPU load at about the same guest cadence. They provide no DWM destination frame count or physical scanout rate. Glow processed 72.28 and 69.15 preview frames/s over its own 53-second windows, which span more than the race and include its fixed-rate feeder.

## RTX-to-RTX repeat

A separate boot of the same private AVD and a warm-up replay preceded the second A/B/B/A series. The only change to the two preview configurations was a per-executable high-performance GPU preference for a byte-identical copy of Glow. In both inline runs its OpenGL renderer reported NVIDIA GeForce RTX 5070 Ti. The QEMU Vulkan device, RTX-connected monitor, crop, destination dimensions, and 21 reported game settings matched the first series. Windows' per-executable preference was returned to its original absent state after testing.

| Replay | Preview | STK FPS | Android SurfaceView frames/s | P99 Android interval (ms) | QEMU CPU (ms/s) | Viewer CPU (ms/s) | Python bridge CPU (ms/s) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| A1 | DWM | 90.013 | 89.969 | 13.751 | 1,179.02 | 0.78 | — |
| B1 | Inline RGBA + Glow | 89.672 | 89.992 | 15.126 | 2,090.84 | 283.10 | 1,686.87 |
| B2 | Inline RGBA + Glow | 89.498 | 89.908 | 18.433 | 2,197.70 | 281.20 | 1,616.59 |
| A2 | DWM | 90.018 | 89.963 | 13.380 | 1,189.09 | 2.34 | — |
| **Pooled A** | **DWM** | **90.015** | **89.966** | | **1,184.06** | **1.56** | **—** |
| **Pooled B** | **Inline RGBA + Glow** | **89.585** | **89.950** | | **2,144.27** | **282.15** | **1,651.73** |

QEMU plus the viewer averaged **1,185.62 ms CPU/s** with DWM and **2,426.42 ms CPU/s** with inline + Glow. That is **51.1% less CPU** in those two processes on the DWM path. Including the verified Python gRPC worker raises the inline total to **4,078.15 ms CPU/s**. The worker reported its own PID and was confirmed as Glow's parent inside the benchmark's launcher process tree. These CPU totals omit the Windows compositor and GPU work.

The 51.1% figure applies to QEMU plus the standalone viewer in this lab. RustEmuDroid's shipping frontend handles gRPC in Rust. It also offers window-sized frames. Here the inline branch requested 1600×900 RGBA, while the compositor scaled the existing 842×474 Qt picture. Equal on-screen dimensions leave different source resolutions and different amounts of work in the two paths. [Shipping video path and adaptive resolution](../../benchmarks/stk-1.5/vulkan-end-scene.md#matching-the-screenshot-stream-to-the-windows-panel)

All four runs again stayed close to 90 guest frames/s. The two pixel checks in each pass showed different parts of the race and agreed with the Qt game crop (RGB correlations **0.99945–0.99991**). The inline check allowed nearby Qt captures and the best match was within 48 ms. This repeat measures CPU cost and guest cadence on one RTX configuration. Glow processed 71.15 and 62.06 preview frames/s in its 53-second windows. The DWM preview's physical presentation rate was outside this measurement.

## Pixel checks and coverage

A static Home check established that the Qt crop agreed with Android's framebuffer (RGB correlation 0.99922). Both dynamic DWM replays then matched the Qt crop with RGB correlations from **0.99986 to 0.99991** and mean absolute RGB error from **0.27 to 0.36** on a 0–255 scale, using a single desktop capture for both windows. The inline runs matched their nearby Qt captures with correlations from **0.99937 to 0.99955** and mean error from **0.55 to 0.59**. These best-matching captures were 39–47 ms apart. These offsets describe how desktop captures were paired.

In a separate replay, the foreground DWM preview completely covered the Qt render child, which remained restored at the same position. A short independent 320×180 Android stream supplied time-neighbor reference frames for two desktop snapshots. The destination matched with correlations **0.98157** and **0.97303** and mean RGB errors **4.06** and **5.03**. The two destination snapshots differed by **28.51** on the same scale and their sampled interiors contained zero near-black pixels. This demonstrates that the preview can continue displaying the running game while its own window covers the Qt game area. The prototype still depends on a live Qt source window and this check covers one machine and renderer.

The pixel metric is evaluated over the interior of bilinearly normalized 256×144 RGB crops. The data retains the exclusion reasons for the preliminary attempts. [DWM attachment and reproduction steps](../dwm-native-viewer/README.md)

## Observed changes in the Windows preview

A separate two-replay check acquired the same 100 Hz RTX desktop for both side-by-side game regions. It used the official Windows DWM viewer from the v0.21.0 Actions artifact for the DWM run and the RTX-selected Glow build for inline. Each Windows capture covered a fixed 14-second interval within the Black Forest race at 90 Hz in Android. Desktop Duplication exposed the latest composed image and its QPC timestamp. The reader compared 12 scanlines per region against the previous acquisition, counting changes when at least six sampled pixels changed by four or more in any RGB channel. It retained only numeric counts. [DXGI calibration and replay data](windows-kemu-dwm-inline-dxgi.json)

| Preview | Qt source changes/s | Preview changes/s | Desktop updates acquired | Desktop updates superseded | Sampler CPU (ms/s) | Readback processing P95 (ms) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| DWM | 89.64 | 89.64 | 1,285 | 5 (0.39%) | 120.8 | 2.35 |
| Inline RGBA + Glow | 81.86 | 74.71 | 1,249 | 147 (10.53%) | 428.1 | 11.48 |

In the DWM run, 1,255 separate acquisitions showed changes in both regions, with 0 Qt-only and 0 preview-only. The inline run had 952 acquisitions with both changing, 194 Qt-only and 94 preview-only.

The atomic calibration painted both regions in the same window at 50 updates/s during the animated section. It observed **346 changes in each crop across 350 submitted paints** and recorded **9 superseded desktop updates across 477**. Earlier tests with separately composed GDI and DWM windows differed by four and seven changes per ten seconds. Those tests could not distinguish capture error from different composition behavior between the two windows.

Superseded updates are global to the monitor. A missed image can contain zero, one, or both region updates. These rates count sampled visible changes, including the effects of the preview and Windows compositor, and omit repeated or missed imagery. The lower-bound observations cover one run per transport. The inline run's 10.53% unavailable desktop updates and higher sampler cost leave the magnitude of the difference uncertain. The readback P95 was near one 10 ms refresh in the atomic fixture. The game rows report the observed readback cost and capture loss separately. [Microsoft Desktop Duplication frame metadata](https://learn.microsoft.com/windows/win32/api/dxgi1_2/ns-dxgi1_2-dxgi_outdupl_frame_info)
