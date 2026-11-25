# SuperTuxKart Vulkan Mailbox on three Windows players

All three players selected Vulkan Mailbox during two SuperTuxKart 1.5 Black Forest replays. The Android SurfaceView presented about 90 times a second on all three, while the game's internal rates ranged from 164 to 372 FPS. Mailbox can replace a pending image before the next refresh, so the internal render loop can run faster than the guest presentation rate. [Vulkan present modes](https://docs.vulkan.org/refpages/latest/refpages/source/VkPresentModeKHR.html)

| Windows player and guest | Run | Game FPS | Android presentations/s | Android P99 interval (ms) | Main loop (ms/frame) | drawAll (ms/frame) | Vulkan acquire (ms/frame) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| RustEmuDroid 0.22.0, Android 17 Google Play | 1 | 163.54 | 89.81 | 14.587 | 6.115 | 3.383 | 0.337 |
| RustEmuDroid 0.22.0, Android 17 Google Play | 2 | 176.38 | 90.05 | 14.482 | 5.670 | 3.107 | 0.331 |
| Kemu 0.7.1 original, Android 16 | 1 | 294.92 | 89.97 | 13.045 | 3.391 | 2.378 | 0.138 |
| Kemu 0.7.1 original, Android 16 | 2 | 289.67 | 90.02 | 12.898 | 3.452 | 2.404 | 0.144 |
| BlueStacks 5.22.166.1003, Android 9 Pie64 | 1 | 372.41 | 89.95 | 11.111 | 2.685 | 1.546 | 0.245 |
| BlueStacks 5.22.166.1003, Android 9 Pie64 | 2 | 350.42 | 89.52 | 11.111 | 2.854 | 1.630 | 0.256 |

The RustEmuDroid panel processed 85.46 and 87.43 frames/s in its verified 800 x 450 Windows viewport. Both passes passed the Windows foreground and moving-image checks. The peer runs use Android foreground and SurfaceView timestamps. Physical display scanout and input latency were outside this measurement.

## Matched workload and remaining differences

The runs used an RTX 5070 Ti, one guest at a time, the same instrumented x86-64 APK, and Black Forest High/Vulkan at 1600 x 900 and 100% render scale. All 21 graphics fields match. Android reported an active 90 Hz mode, and the game logged `VkPresentModeKHR=1` in every replay. The test wrote `max_fps=90` and disabled the game's VSync setting. The reported internal FPS comes from the replay's own profiler.

Rust used the official v0.22.0 Windows launcher, the Emulator 37.3.10 cache engine relinked with rebuilt media libraries, Vulkan mapped-memory cache enabled and the experimental virtual-clock change disabled. Kemu used its original Qt player and its own Android 16 runtime. BlueStacks used the official player with a restored Pie64 clone. The guest OS, Vulkan implementation and Windows presentation paths differ across players. This comparison qualifies the workload and the selected presentation mode, with two samples on each runtime. [Cache pipeline and earlier FIFO results](windows-vulkan-mapped-writes.md)

The Rust traces spent more elapsed time per frame in `drawAll()` than the Kemu and BlueStacks traces. Acquire was a smaller scope in this series. The scopes are nested and include waits and scheduling on the calling thread. Differences in Vulkan encoding and mapped-memory writes are candidates for further testing on a controlled runtime. [STK scope boundaries](vulkan-end-scene.md) [Mapped-memory evidence](windows-vulkan-mapped-writes.md) [BlueStacks Vulkan encoder observations](windows-bluestacks-vulkan-encoder.json)

## Recalculate the results

Game FPS is the STK report's frame count divided by profiled duration. The scope means use the rows where `Main loop` is positive and divide elapsed microseconds by the profiled frame count. The STK CSVs agree with the report's frame count and its millisecond-truncated sum of main-loop time. Each Android row uses the unique SurfaceView presentations during a nominal 20-second sample divided by that sample's elapsed time. P99 uses sorted consecutive presentation gaps at zero-based index `(N - 1) * 99 // 100` for `N` gaps. The JSON includes relative timestamps, counts, durations, scope sums and input SHA-256 values. [Verified data](windows-rtx-mailbox-peers.json) [Replay analyzer](summarize-mailbox-replays.py) [SurfaceView collector](../pubg-mobile/capture-presentations.py)

```text
python -B benchmarks/stk-1.5/summarize-mailbox-replays.py --rust <Rust run>/b1 --kemu <Kemu run>/kemu --blue <Pie64 run> --output <results.json>
```

The Rust input directory contains `mailbox-1` and `mailbox-2` and its parent holds `protocol.json`. The peer directories contain `lap-1`, `lap-2` and `results.json`, plus the Kemu preflight or BlueStacks receipt. Each run includes the STK report, per-frame CPU profile, renderer log and Android SurfaceView sample.
