# PUBG Inventory thread counters

This fixture reads ordinary Linux accounting counters on an x86-64 Android guest running PUBG MOBILE. Each record includes the Android monotonic time before and after the reads, the process totals and available tasks named `UEGameThread`, `RHIThread`, `RenderThread 1` or `FAsyncLoading`. The observer uses the permissions of the Android shell.

## Record

1. Put the game in its lobby. Record the original settings, camera view, display mode and physical GPU. Use `adb devices -l` and `adb -s <serial> shell pidof com.tencent.ig` to identify the running instance and process.
2. Build [observe.c](observe.c) with the x86-64 compiler from the Android NDK for API 28 or later. Set `NDK_CLANG` below to your NDK's target-prefixed compiler, for example `x86_64-linux-android28-clang` on Linux or `x86_64-linux-android28-clang.cmd` on Windows.
3. In the commands below, run from this directory and substitute the executable, serial, API level and output path. The Python wrapper uploads a uniquely named temporary observer and removes it after the run. At the 200 ms default, 315 samples cover roughly 63 seconds.

```text
<NDK_CLANG> -O2 -Wall -Wextra -Werror -fPIE -pie observe.c -o observe-stat
python -B observe.py --adb <adb> --bin observe-stat --serial <serial> --port 5037 --api 37 --expected-pid <pid> --samples 315 --out proc-stat.txt
```

While the observer runs, poll `dumpsys SurfaceFlinger --latency <game-layer>` about every 440 ms on that same device. Keep the exact foreground SurfaceView, all three timestamp columns and the host monotonic times just before and after each poll. Record six stationary seconds first. Over the next 48 seconds, open Inventory at 0, 12, 24 and 36 seconds and close it at 7, 19, 31 and 43 seconds. Verify the screens and restore the original settings afterwards. Use a new game process for each measured route. [Frame collection and Android timestamp handling](../capture-presentations.py)

Add `--memory` to record process memory in the same samples. The observer reads `VmRSS`, `RssAnon`, `RssFile`, `RssShmem` and `VmSwap` from the game's ordinary `/proc/<pid>/status`. Each window with at least two complete samples reports its first, last and peak values in KiB alongside the fault and CPU counters. Changes between polls or mappings responsible for individual faults require separate evidence. An older guest kernel may lack one of these fields. The optional collection reports an error if a field is unavailable.

## Analyze

On RustEmuDroid v0.18.1, the optional memory mode collected all five fields in each of 315 samples of the same PUBG 4.7.0 process on Android 17. The median interval from the beginning to the end of an individual read was 0.64 ms and the 95th percentile was 1.90 ms. The third visit showed the backpack finish screen, so the run was excluded from the Inventory comparison. Earlier Android 17 and Pie64 traces collected without the option still produce their original counter reports.

Each distinct presentation counts once. Where SurfaceFlinger reports multiple possible ready times for an endpoint, the exporter leaves that ready interval unmeasured and records all the candidates. It can still measure the presentation gap. [Pie64 measurements](../v0.18.0-pie64-thread-counters.json)

For a specific pause, take the pair of Android buffer-ready timestamps from the records on either side of the gap. Divide the nanoseconds by one billion and save the interval in a JSON list, for example `[["first-close", 237.803, 239.477]]`. Run `python -B analyze.py proc-stat.txt windows.json`. The analyzer keeps only pairs of complete samples within the interval and reports the covered start and end. CPU counters advance in ticks, with the tick rate recorded in the observer's header. The process row already includes its threads.

For a complete route from the project harness, run `python -B export-counts.py <raw-run-directory> --output <new-relative-report.json>` with the following inputs.

| File | Required fields |
|---|---|
| `proc-stat.txt` | Raw observer output |
| `inventory-sf-latency.json` | `polls[]` with the layer name and the raw three-column latency dump |
| `lobby-sf-latency.json` | Same poll format, the selected `layer` and `guest_window_ns` start and end |
| `inventory.json` | `layer`, `origin_ns`, `approximate_host_to_guest_offset_ns`, sorted `guest_presentation_ns`, 48-second `total` metrics, and scheduled taps in `route[]` |

The exporter checks the frame totals and tap schedule, selects the most CPU-active task with the game-thread name and writes relative timestamps and role totals. [Measured Windows results](../../../docs/game-testing.md#what-the-game-thread-did-during-the-first-inventory-close) · [Android 17 samples](../v0.18.0-android17-thread-counters.json)

## One-close retest on RustEmuDroid v0.19.0

PUBG MOBILE 4.7.0 used its original global Balanced preference, Classic style and disabled Lobby Graphics Enhancement on Android 17 and an RTX 5070 Ti. The stationary game SurfaceView presented 155 distinct frames in the 5.263075947-second window, **29.45 presentations/s**. This counts timestamps in the half-open interval from −13.343075947 s to −8.080 s relative to the lower bound of the close tap. The route used one fresh Inventory visit.

The first close produced a **1,617.664 ms presentation gap** and a **1,616.447 ms ready gap** between the same two frames. The last presentation and ready time before the gap fell inside the estimated 30 ms tap bracket. The game returned to Lobby in the same process. An earlier attempt showed an exit confirmation after the attempted open, so the visual guard ended it before a close tap.

| Window | Complete samples | Process RSS first / last / peak, KiB | Busiest game thread minor faults |
| --- | ---: | ---: | ---: |
| Stationary Lobby | 52 | 2,751,176 / 2,751,792 / 2,751,792 | 624 |
| First-close ready gap | 15 | 2,767,580 / 2,772,096 / 2,772,096 | 61,824 |

During 1.431 sampled seconds inside the gap, the busiest game thread accumulated 1,120 ms of CPU time. The process recorded 62,135 minor faults and zero major faults. Its last sampled RSS was 4,516 KiB above the first value. Shared RSS rose by 2,980 KiB, and anonymous RSS peaked 1,824 KiB above its first value before ending 1,488 KiB higher. Process swap stayed at zero in all 270 samples. The fault totals count events, while RSS describes sampled process-wide residency. [Relative frame timestamps, counters, and first/last/peak memory values](v0.19.0-android17-first-close.json)

The earlier accepted v0.18.0 run used Customize Lobby Balanced/High on a four-visit route. It recorded a 1,583.816 ms first-close ready gap and 62,293 minor faults on the busiest game thread. The different presets and routes limit a launcher-version comparison. The v0.18.1 recording above also used Customize Lobby Balanced/High. It verified the memory observer and was excluded from the route comparison. [Earlier accepted v0.18.0 counters](../v0.18.0-android17-thread-counters.json)
