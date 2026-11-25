# SuperTuxKart CPU scope overhead on Windows

The same signed SuperTuxKart 1.5 x86-64 QA APK ran Black Forest High Vulkan FIFO in the released RustEmuDroid 0.21.1 window. Android 17 Google Play used Android Emulator 37.3.10 on the RTX 5070 Ti with four guest vCPUs, 1600 × 900 at active 90 Hz, and a fitted 800 × 450 RGBA panel. Both modes matched all 21 reported graphics fields. The build matched 5,577 asset files and the same signer as the earlier scoped APK.

A uses the detailed CPU profiler. B uses `--qa-frame-totals-only` to omit detailed timing scopes while retaining the two outer timer reads per measured frame and the same frame-count and duration report. The run checked the mode marker, the single B CSV column and exact agreement between its positive frame rows and report totals. Each engine ran A–B–B–A in its own single boot after one excluded warm-up for each mode. Other builds were paused during the timed windows.

| Engine | Pass | Mode | Game FPS | Android SurfaceView/s | Surface p99 | Rust panel/s | CPU CSV columns |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| SDK Android 17 | 1 | Detailed scopes | 85.74 | 85.12 | 23.38 ms | 83.61 | 26 |
| SDK Android 17 | 2 | Frame totals only | 86.01 | 85.95 | 22.96 ms | 83.96 | 1 |
| SDK Android 17 | 3 | Frame totals only | 85.86 | 85.75 | 22.73 ms | 82.77 | 1 |
| SDK Android 17 | 4 | Detailed scopes | 85.03 | 84.66 | 23.46 ms | 82.63 | 26 |
| Kemu Android 16 | 1 | Detailed scopes | 90.02 | 90.02 | 13.41 ms | 86.60 | 26 |
| Kemu Android 16 | 2 | Frame totals only | 90.01 | 90.01 | 13.65 ms | 86.44 | 1 |
| Kemu Android 16 | 3 | Frame totals only | 90.01 | 90.02 | 13.42 ms | 86.87 | 1 |
| Kemu Android 16 | 4 | Detailed scopes | 90.02 | 90.02 | 13.55 ms | 87.85 | 26 |

Game FPS pooled to **85.38 with detailed scopes** and **85.93 with frame totals only** (+0.55 FPS). Android cadence was **84.89 → 85.85 presentations/s**. The selected Windows QEMU process used 2248 → 2333 CPU ms/s during those Android samples. The planned materiality check requires at least 2 FPS pooled gain and both neighboring B–A differences larger than the bigger within-mode spread.

The SDK difference did not meet the planned materiality check.

The matched Rust window using Kemu Android 16 without the detailed profiler delivered **90.01 game FPS** and **90.02 Android presentations/s** across two B replays. Kemu A pooled to 90.02 game FPS. The totals-only game gap was **+4.08 FPS for Kemu** and includes the runtime and image differences.

A separate pre-replay probe compared guest `CLOCK_MONOTONIC_RAW` with Windows QPC across the four vCPUs. The largest observed spread between time-adjusted TSC offsets was 1.877 seconds on the SDK guest. Kemu's observed spread, 45 ns, stayed below the probe's guest-clock bracket. This probe measured clock alignment outside the game runs.

Game FPS covers the full replay. SurfaceView samples cover a separate 20-second foreground interval. The Rust counter measures panel processing during its complete reporting intervals. This is a controlled measurement of the QA profiler cost in this replay. Stock games and physical scanout require their own measurements. The fixture was removed and the single usual Android 17 Home was restored.

[Sanitized measurements and hashes](windows-profiler-overhead.json) · [Build and run both modes](cpu-scope-ab.md) · [Clock alignment against Windows](windows-guest-clock-alignment.md)
