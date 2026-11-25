# Android native-bridge CPU benchmark

The fixture compares two small JNI workloads built for x86-64 and ARM64 from the same C source with the same Clang flags. Each APK contains the same DEX and one ABI-specific ELF library. Neither APK requests Android permissions.

| Workload | What is timed | Reference digest |
| --- | --- | --- |
| Arithmetic | 6,000,000 dependent unsigned 64-bit XOR, shift and multiply steps | `b6fae11725cce32b` |
| Indexed memory | 1,048,576 dependent reads through a deterministic permutation in a 1 MiB table | `00fffe953f9f42b0` |

Each sample reports elapsed monotonic time and current-thread CPU time in nanoseconds. The runner also records the change in minor and major page faults reported by `getrusage(RUSAGE_THREAD)`; a value of `-1` means the counter was unavailable. Allocation, permutation construction, fault-counter reads and result serialization occur outside the timed loops.

## Build

From the repository root, use Python 3, a JDK and a local Android SDK with Platform Tools, an SDK platform, Build Tools and a side-by-side NDK. The defaults match the fixture build: SDK platform `android-37.0`, Build Tools `37.0.0` and NDK `30.0.14904198` (r30 beta 1). The host tools were tested on Windows. Set `--host-tag` if your NDK places the host Clang under a different prebuilt directory. Both Android library targets use API 26.

```sh
python -B benchmarks/native-bridge/build.py --sdk "/path/to/android/sdk" --jdk "/path/to/jdk" --out "/path/to/local-output"
```

The command creates the signed APKs, a fixture-only signing key and a build receipt with file hashes in the output directory. Use a local output directory outside the checkout.

## Collect

Start one x86-64 Android guest with a local ADB server and verify its serial. For the ARM64 run, the guest must accept and load ARM64 app libraries through its native bridge. Keep the guest idle and run only one emulator at a time. Hold the host power mode and guest CPU settings fixed during the comparison. Close games and pause other measurements, builds and downloads.

```sh
python -B benchmarks/native-bridge/runner.py --adb "/path/to/android/sdk/platform-tools/adb" --port 5037 --serial emulator-5554 --expected-sdk 37 --build-dir "/path/to/local-output" --output "/path/to/local-output/guest-run.json" --execute
```

On Windows use `adb.exe`. Substitute the guest API level, serial and ADB server port. Use a new output file each time. Omit `--execute` to check the guest identity without installing anything, and pass `--fingerprint` if you need to require a particular guest build.

The runner checks that both fixture package IDs are absent, verifies the installed ABI with Package Manager, and checks the ABI again in each JNI result. It runs x86-64, ARM64, ARM64, x86-64 in that order. A fresh process uses one worker thread, two warmups per workload and seven recorded samples per workload by default. The runner waits 15 seconds after each launch before reading the result by its unique logcat nonce. Increase `--quiet-seconds` and start a fresh run if the guest fails to finish in that interval. The runner removes the two fixture packages and records cleanup in the JSON.

## Interpret

Report the median of each workload per process. For a within-guest ARM64-to-x86-64 ratio, divide the medians from laps 2 and 3 by those from their adjacent native laps 1 and 4, respectively. Keep elapsed time and thread CPU time separate. Review page faults and disagreement between the neighboring pairs before summarizing.

The ratio includes instruction-set and compiler code-generation differences, the guest's translation and execution paths, and their interaction with CPU caches and memory. Elapsed time can also reflect scheduling delays. A thread CPU time above elapsed time is recorded as a clock discrepancy. The result retains both times and reports how many samples disagreed and the largest excess. Compare within the same guest and describe emulator, Android build, CPU configuration and native bridge when comparing across guests. These synthetic loops do not measure game FPS, rendering, frame pacing or the fraction of a game frame spent translating ARM64 code.

## Windows results

On November 24, 2026, we ran the fixture on a Windows 11 PC with a Ryzen 9 8945HS and an RTX 5070 Ti. The two x86-64 guests ran sequentially, with four online vCPUs and 8 GiB configured RAM each. BlueStacks Pie64 5.22.166.1003 used Android 9 (API 28) with `libnb.so`. RustEmuDroid 0.13.5 used the Android 17 Google Play image (API 37) with `libndk_translation.so`. Both guests started idle and loaded the same signed APKs. Package Manager and each loaded JNI library confirmed the expected ABI.

| Guest | Workload | x86-64 thread CPU, two medians (ms) | ARM64 thread CPU, two medians (ms) | ARM64 / x86-64 thread CPU | ARM64 / x86-64 wall time |
| --- | --- | ---: | ---: | ---: | ---: |
| BlueStacks Pie64 | Arithmetic | 13.772, 12.462 | 12.606, 12.449 | 0.956 | 0.956 |
| BlueStacks Pie64 | Indexed memory | 7.997, 6.792 | 6.477, 7.920 | 0.972 | 0.972 |
| RustEmuDroid Android 17 | Arithmetic | 13.363, 13.706 | 13.632, 13.396 | 0.999 | 1.005 |
| RustEmuDroid Android 17 | Indexed memory | 8.068, 7.624 | 6.518, 9.431 | 1.000 | 0.999 |

The last two columns give the geometric mean of the adjacent ARM64/x86-64 ratios (laps 2/1 and 3/4). The arithmetic thread CPU ratio was 4.4% higher on Android 17. The memory thread CPU ratios spanned 0.810 to 1.166 in Pie64 and 0.808 to 1.237 in Android 17 across the two pairs. The paired memory results varied much more than the difference between guest averages.

The CSV contains 112 samples from eight fresh processes. All samples in the completed sweeps matched their workload digest and recorded zero page faults. A preliminary Android 17 sweep stopped during the second lap when thread CPU exceeded elapsed time by more than the previous 0.100 ms tolerance. Three samples in that sweep had CPU times above elapsed time, with a largest excess of 0.221 ms. We restarted the full sweep using a collector that records the discrepancy. Every sample in the completed sweep reported thread CPU time at or below elapsed time.

The timed loops follow two warmups per task. This setup measures sustained execution of two small JNI workloads. It leaves first-use translation, game loading, rendering and frame pacing outside the measurement. [Per-sample timings](results.csv)

## First use of ARM64 code on Windows

On November 25, 2026, we measured the first visit to each of four groups of 1,024 distinct functions, then revisited each group three times in the same order. The two guests ran sequentially on the same Ryzen 9 8945HS PC. RustEmuDroid 0.18.0 used the Android 17 Google Play x86-64 image. BlueStacks 5.22.166.1003 used a restored Pie64 Android 9 instance. Each guest had four vCPUs and 8 GiB of RAM. The two guests ran the same pair of signed APKs in four fresh processes each (x86-64, ARM64, ARM64, x86-64).

| Guest | App code | First visit (µs) | Repeat (µs) | First thread CPU (µs) | Repeat thread CPU (µs) |
|---|---|---:|---:|---:|---:|
| RustEmuDroid, Android 17 | x86-64 | 25.55 | 14.75 | 17.82 | 7.49 |
| BlueStacks, Android 9 | x86-64 | 32.40 | 9.97 | 31.39 | 9.72 |
| RustEmuDroid, Android 17 | ARM64 | 1,463.85 | 33.35 | 1,441.93 | 24.88 |
| BlueStacks, Android 9 | ARM64 | 345.00 | 112.63 | 344.44 | 111.79 |

Each entry covers 1,024 function calls. For a first visit we report the median across eight groups (four per process, two processes per ABI). For a repeat we take the median of the three revisits within each group, then the median across those eight values. The first two timing columns use elapsed wall time. The Android 17 ARM64 first visit took 4.24 times as long as on Pie64. On subsequent visits, the order reversed. Across the eight first ARM64 visits, the worker incurred 1,648 minor page faults on Android 17 and 15 on Pie64. Neither guest recorded a major fault. The timings include translation, dispatch, page faults and the generated code's own execution. This exercise does not identify which of those costs accounts for the difference or how much time PUBG spends on any of them. [Individual visits and fault counts](first-use-results.csv) · [Measured PUBG first Inventory close](../../docs/game-testing.md#pubg-inventory-retest-with-rustemudroid-v0180)

### Reproduce the first-use measurement

Run the build and collection commands above with `--first-use` on **both** commands. Give the build a fresh output directory. Add `--samples 4` to the collector for one first traversal and three repeats, and run it once per guest with a separate JSON destination. Export the results with the command below.

```sh
python -B benchmarks/native-bridge/export_first_use.py native.json pie64.json --csv first-use-results.csv
```

The build checks with `llvm-nm` that the 4,096 generated functions have distinct addresses in each library. Every visit follows the same permutation and must return the reference digest for its group. The exporter verifies both guests used the same build receipt and both fixture packages were removed after each run. First use means the first traversal of that group in the process. Persistent caches and guest memory retain their normal behavior across launches. [Generator and reference results](first_use.py) · [Timed traversal](first_use.c)

## Cost of touching discarded anonymous pages

During the first PUBG Inventory close on Android 17, the game thread recorded 61,824 minor faults in 1.431 seconds of sampled time. The fixture measures writes to 61,440 distinct anonymous pages in a private mapping, 240 MiB with a 4 KiB page size. It writes one byte per page, reads it back and accumulates a checksum. The resident control uses previously touched pages. The other condition calls `madvise(MADV_DONTNEED)` once on the mapping before timing, which lets Linux supply fresh zero-filled pages on demand. [PUBG trace](../../docs/game-testing.md#first-inventory-close-on-the-published-v0190-windows-build) · [Linux madvise](https://man7.org/linux/man-pages/man2/madvise.2.html)

Both conditions measure wall time, thread CPU time, minor faults and major faults in one uninterrupted traversal on the same thread. Allocation, prefaulting and discard happen before timing. Conditions alternate order between samples. The fixture reports the guest page size and checks every sample against the expected sum of 1,873,920. In an earlier Android 17 pilot that reused a small mapping, the fixture rejected the first timing run and emitted no valid samples. The results below use the larger contiguous mapping. [Timed code](page_touch.c) · [Linux per-thread counters](https://man7.org/linux/man-pages/man2/getrusage.2.html)

On the Ryzen 9 8945HS, the same two signed APKs ran on Android 17 Google Play in the RustEmuDroid lab and in BlueStacks 5.22.166.1003 Pie64 on Android 9. Both guests reported 4 KiB pages and had four vCPUs and 8 GiB configured. Each row averages the medians of two fresh processes, with three samples per condition in each process. The table uses wall time for the resident baseline. In two short resident trials on Android 17 the thread clock exceeded elapsed time by 0.456 and 0.175 ms. All samples are available for comparison. [48 individual measurements](page-touch-results.csv)

| Guest | App code | Resident wall time (ms) | After discard, wall time (ms) | After discard, thread CPU (ms) |
|---|---|---:|---:|---:|
| Android 17 Google Play | x86-64 | 1.73 | 95.95 | 95.74 |
| Android 17 Google Play | ARM64 | 1.36 | 89.42 | 88.93 |
| BlueStacks Pie64 | x86-64 | 1.01 | 44.80 | 44.79 |
| BlueStacks Pie64 | ARM64 | 1.02 | 39.97 | 39.97 |

Every traversal after discard produced exactly 61,440 minor faults. Resident traversals produced zero to two. All 48 trials had zero major faults. The Android 17 ARM64 test took about 89 ms of thread CPU to handle these writes, compared with the 1,120 ms of game-thread CPU previously observed around a PUBG Inventory close with a similar fault count. The game trace leaves the faulting mappings and the intervening game work unidentified. These results describe whole guest configurations and one pattern of memory access. The two processes per ABI also showed timing variation within a guest. [PUBG first-close measurement](../../docs/game-testing.md#first-inventory-close-on-the-published-v0190-windows-build)

To reproduce, add `--page-touch` to both the build and the runner commands above. Use a fresh build directory for the pair of APKs and the same directory on both guests. Run the collector against each verified serial with `--samples 3`, a matching SDK and a separate output file. Export all recorded samples with the command below.

```sh
python -B benchmarks/native-bridge/export_page_touch.py android17.json pie64.json --csv page-touch-results.csv
```
