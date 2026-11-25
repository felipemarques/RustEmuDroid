# Comparing the four virtual CPU clocks with Windows time

This experiment measures counters after Android has booted, while the guest is idle. It complements the [Linux boot diagnostics](windows-guest-clocksource.md) and the [cost per clock read](windows-guest-clock-cost.md). CPU affinity is changed only for the test process. The test runs through ordinary ADB shell access. It does not select the guest clocksource or change the Android image.

## Windows results after boot

In a new session of the stock SDK 37.3.10 Android 17 Google Play guest, the estimated TSC offset from vCPU 0 was **−1.875 s on vCPU 1, −1.876 s on vCPU 2 and −1.877 s on vCPU 3** (median of nine reads per CPU). The observed spread was 1.87699–1.87707 s in every round. The longest guest RAW bracket was 86.5 µs. At about 3.993 GHz, the counters advanced at similar rates while retaining different origins. The large skew persisted at the Home screen, after startup had finished. [Per-CPU readings, bounds and binary hash](windows-guest-clock-alignment.json)

All four RAW clocks advanced forward between samples. Each elapsed-rate interval relative to Windows QPC included zero drift, with bounds across the four intervals from −893 to +299 ppm. The largest complete host round trip was 7.32 ms. This is a short check of elapsed time using the clock Linux made available. The separate boot log shows why Linux rejected the raw hardware counter as its time source. [Serial diagnostics](windows-guest-clocksource.md)

On Kemu Android 16, the same binary estimated a spread of at most 45 ns across the four CPUs using RAW. The largest individual RAW bracket was 131 ns, and Kemu's guest RAW clock itself uses TSC in the diagnostic boot. The measurement cannot resolve a 45 ns physical skew. Kemu's QPC rate intervals also all include zero drift, with outer limits −211 to +247 ppm.

An independent check uses only **the order of the TSC reads** to bound a persistent CPU offset. Reading vCPU 0 before vCPU 3 gives an upper bound on the offset of vCPU 3 because time elapsed between reads. Reading them in the reverse order gives a lower bound. Taking the tightest bounds in each direction gives the ranges below. This assumes comparable per-vCPU rates and an offset that stays constant over the observation. The bounds in cycles are independent of the guest RAW clock. Time units use the measured rate of about 3.993 GHz.

| vCPU offset relative to vCPU 0 | SDK Android 17, range (ms) | Kemu Android 16, range (ms) |
| --- | ---: | ---: |
| vCPU 1 | −1874.89 to −1874.77 | −0.107 to +0.084 |
| vCPU 2 | −1876.00 to −1875.52 | −0.206 to +0.159 |
| vCPU 3 | −1877.36 to −1876.63 | −0.259 to +0.239 |

The readings bracket a delay of roughly 1.88 **seconds** on the stock SDK guest and less than 0.26 **milliseconds** in either direction on Kemu. Both sessions used four vCPUs and the same Windows machine. Neither test changed the clocks or measured the effect of repairing synchronization on game FPS. [Relative samples and bounds in cycles](windows-guest-clock-alignment.json)

## Measurement

- The Windows harness sends one request to a persistent guest process and records the time just before sending and on receipt of the complete reply. Each round is bracketed with Python's high-resolution Windows clock, `QueryPerformanceCounter`.
- For each CPU allowed by Android, the native x86-64 probe pins its own thread to that CPU and reads the virtual timestamp counter (`RDTSCP`) between two `CLOCK_MONOTONIC_RAW` readings. It checks the CPU on which the sample ended. Successive rounds alternate CPU order.
- Each pair of host send/receive times bounds the time when that guest batch actually ran. The first and last brackets give conservative bounds on the elapsed guest-to-host clock rate, including ADB latency. A result spanning zero parts per million is consistent with the host rate at that precision.
- To estimate CPU-to-CPU **TSC offset**, fit ticks per nanosecond separately on each vCPU using the first and last rounds and take the median of those slopes. Within a round, compensate each TSC reading for its position in the round using the midpoint of its two guest RAW readings. Report the maximum minus the minimum corrected offset. Host and guest absolute clock epochs are independent. This measures the relative offsets at the observed times. Pinning a thread does not create simultaneous CPU samples. The clock-read brackets and the host round-trip times show the precision limits. A TSC-based guest RAW clock also shares a time source with the TSC being sampled.

## Reproduce

Compile with the Android NDK for x86-64. The measured binary used NDK 30.0.14904198. Run on the same owned AVD, after boot and outside a game benchmark. The harness checks the AVD path, copies the probe into a unique temporary location, collects nine batches about one second apart and removes the binary. The host output contains all sampled values.

```sh
clang --target=x86_64-linux-android28 -O2 -Wall -Wextra -Werror android-clock-alignment.c -o android-clock-alignment
python measure-clock-alignment.py --adb PATH_TO_ADB --serial emulator-PORT --expected-avd PATH_TO_AVD --binary android-clock-alignment --output fresh-clock-alignment.json
```

[Guest probe source](android-clock-alignment.c) · [Host timing and estimator](measure-clock-alignment.py)
