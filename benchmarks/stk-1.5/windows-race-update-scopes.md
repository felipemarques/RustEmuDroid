# Where STK spends time updating the race on Windows

The original Android 17 run spent **0.601 ms more per frame in `Update race`** than Kemu Android 16. That scope accounts for **63% of the 0.960 ms per-frame gap** in the main loop. The more tightly matched SDK comparison found similar race-update costs on stock Android 16 and 17. The next question is what makes this code path cheaper on Kemu, where the runtime and guest image both differ from the official SDK.

All ten replays ran the same signed STK 1.5 x86-64 CPU-scoped APK on an RTX 5070 Ti. Black Forest used High/Vulkan FIFO, a 1600 x 900 guest at 100% render scale and 90 Hz, and the same 21 graphics fields. The first three rows are separate historical runs with different guest builds, emulator frontends and graphics paths. The last two use the same released RustEmuDroid 0.21.1 launcher, Emulator 37.3.10 and host gfxstream binaries. These stock SDK images also differ in their Google APIs versus Google Play variant. [Historical setup](vulkan-end-scene.md#windows-results-at-90-hz) · [Matched SDK setup](windows-sdk-android16-vs17.md)

| Guest and runtime | Frames in two replays | Profiled seconds | Game FPS | Main loop ms/frame | IrrDriver ms/frame | Update race ms/frame | World::update ms/frame | Outside renderer and race ms/frame |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Android 17, Rust 0.18.1 | 6,316 | 76.230 | 82.85 | 12.069 | 10.636 | 1.151 | 0.776 | 0.282 |
| Android 16, Kemu 0.7.1 original | 6,865 | 76.263 | 90.02 | 11.109 | 10.523 | 0.550 | 0.354 | 0.036 |
| Android 9, BlueStacks Pie64 | 6,862 | 76.252 | 89.99 | 11.112 | 10.644 | 0.434 | 0.291 | 0.035 |
| Android 16 Google APIs, SDK 37.3.10 | 6,369 | 76.120 | 83.67 | 11.952 | 10.654 | 1.031 | 0.688 | 0.268 |
| Android 17 Google Play, SDK 37.3.10 | 6,517 | 76.124 | 85.61 | 11.681 | 10.430 | 0.997 | 0.698 | 0.254 |

The original Android 17 comparison places **0.113 ms/frame** of the Kemu gap inside `IrrDriver update` and **0.246 ms/frame** outside both the renderer and race scopes. Of that residual, **0.180 ms/frame** remains unassigned after accounting for the four other named sibling scopes. The nested `World::update()` accounts for **0.422 ms/frame** of the race-update difference. Pie64 spent about the same time per frame in `IrrDriver update` as the original Android 17 run and less in the race scope. These are locations in the call tree where the profiler recorded elapsed time.

| Guest and runtime | Update race per lap, ms/frame | Update race total, s | Update race ms/profiled second | World::update ms/profiled second |
| --- | ---: | ---: | ---: | ---: |
| Android 17, Rust 0.18.1 | 1.161 / 1.141 | 7.269 | 95.36 | 64.33 |
| Android 16, Kemu 0.7.1 original | 0.555 / 0.544 | 3.775 | 49.50 | 31.86 |
| Android 9, BlueStacks Pie64 | 0.443 / 0.426 | 2.979 | 39.07 | 26.18 |
| Android 16 Google APIs, SDK 37.3.10 | 1.055 / 1.007 | 6.565 | 86.24 | 57.58 |
| Android 17 Google Play, SDK 37.3.10 | 0.996 / 0.999 | 6.499 | 85.37 | 59.77 |

Over comparable profiled durations, the historical Android 17 race scope absorbed roughly **46 ms more per second** than Kemu despite rendering fewer frames. In the matched SDK pair, the race-scope difference was **0.87 ms per profiled second**, with stock Android 16 slightly higher. That Android 16 run spent about **37 ms more per profiled second** in the scope than the separate Kemu Android 16 run. The official SDK pair leaves runtime and guest-specific behavior as candidates for the much larger historical gap. The 0.271 ms/frame main-loop difference in the SDK pair is primarily inside `IrrDriver update` (0.224 ms/frame).

## Measurement and scope boundaries

- STK uses the name `Update race` for two disjoint main-loop regions. One calls `World::updateGraphics(frame_duration)` once per graphics iteration. The other calls `updateRace(1, fast_forward)` inside the simulation tick loop. The profiler accumulates both into the same column. The number of calls can vary with frames and tick batching. The CSVs record combined durations and do not expose tick counts. `World::update()`, weather and camera are contained within these regions, so their times stay inside `Update race` in the main-loop partition. [Main loop](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/main_loop.cpp#L563-L649) · [World methods](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/modes/world.cpp#L1110-L1225) · [Profiler accumulation](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/utils/profiler.hpp#L138-L195)
- Each CSV retained the rows with positive `Main loop` and excluded one all-zero row. Counts and summed main-loop durations match STK's own frame report, which truncates the total to integer milliseconds. The per-second denominator is that reported profile duration. The pooled figures divide summed scope times by summed frames or summed duration. The analyzer checks the nesting and residuals per row. [Verified sums and input SHA-256 values](windows-race-update-scopes.json) · [CSV analyzer](summarize-race-update-scopes.py)
- On Android, STK's profiler uses `gettimeofday()`. Scope time is elapsed on the calling thread and can include waits, descheduling, inner markers and clock reads. The marker implementation uses a shared lock and updates a per-thread event table. The same APK gives these guests the same instrumentation code, with platform-dependent timing and scheduling costs. The engine's tick calculation uses a steady clock. `IrrDriver update` measures a caller scope that can include graphics API waits. GPU timestamps and thread CPU time would answer different questions. [Profiler clock and markers](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/utils/profiler.cpp#L75-L227) · [Frame report](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/utils/profiler.cpp#L580-L619)

```sh
python benchmarks/stk-1.5/summarize-race-update-scopes.py \
    --lap lap-1/report.csv lap-1/quadros.csv --lap lap-2/report.csv lap-2/quadros.csv
```

A follow-up QA replay can give the two existing markers distinct names and count graphics updates and simulation ticks. Compare wall time with sampled main-thread CPU time and measure the cost of the clock reads and markers on each guest. Keep a matched control to check whether the new instrumentation changes engine frame rate. This can distinguish more invocations, more on-CPU work per invocation and more elapsed waiting.

A standalone x86-64 probe measured the cost of the two guest clocks used in these code paths. [Clock cost and guest CPU capabilities](windows-guest-clock-cost.md)
