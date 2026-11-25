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

## Analyze

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
