# Showing the Kemu game window through Windows DWM

The DWM preview showed the moving SuperTuxKart replay from Kemu's Qt window, including when the preview fully covered the Qt game surface. In four replays, the guest delivered about 90 frames per second with either preview. The selected Windows processes used less CPU with DWM on this machine. The inline viewer rendered on the Radeon 780M, while QEMU and the monitor used the RTX 5070 Ti, so this comparison measures the two working configurations as tested. [Run-level data and pixel checks](windows-kemu-dwm-inline-stk.json)

## Replay and CPU

One private Kemu 0.7.1 Android 16 VM ran throughout the comparison. All four passes used the same x86-64 SuperTuxKart 1.5 build, Black Forest replay, Vulkan High settings, 1600×900 guest output at 90 Hz, and a FIFO swapchain. The XML set `max_fps=90` and `swap-interval-vsync=1`. The game reported a virtual Adreno 830 and the QEMU log identified the RTX 5070 Ti as its physical Vulkan device.

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

## Pixel checks and coverage

A static Home check established that the Qt crop agreed with Android's framebuffer (RGB correlation 0.99922). Both dynamic DWM replays then matched the Qt crop with RGB correlations from **0.99986 to 0.99991** and mean absolute RGB error from **0.27 to 0.36** on a 0–255 scale, using a single desktop capture for both windows. The inline runs matched their nearby Qt captures with correlations from **0.99937 to 0.99955** and mean error from **0.55 to 0.59**. These best-matching captures were 39–47 ms apart. That sampling offset does not measure end-to-end latency.

In a separate replay, the foreground DWM preview completely covered the Qt render child, which remained restored at the same position. A short independent 320×180 Android stream supplied time-neighbor reference frames for two desktop snapshots. The destination matched with correlations **0.98157** and **0.97303** and mean RGB errors **4.06** and **5.03**. The two destination snapshots differed by **28.51** on the same scale and their sampled interiors contained zero near-black pixels. This demonstrates that the preview can continue displaying the running game while its own window covers the Qt game area. The prototype still depends on a live Qt source window and this check covers one machine and renderer.

The pixel metric is evaluated over the interior of bilinearly normalized 256×144 RGB crops. The data retains the exclusion reasons for the preliminary attempts. [DWM attachment and reproduction steps](../dwm-native-viewer/README.md)
