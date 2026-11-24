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
