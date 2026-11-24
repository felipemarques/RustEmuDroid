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

The ratio includes instruction-set and compiler code-generation differences, the guest's translation and execution paths, and their interaction with CPU caches and memory. Elapsed time can also reflect scheduling delays. Compare within the same guest and describe emulator, Android build, CPU configuration and native bridge when comparing across guests. These synthetic loops do not measure game FPS, rendering, frame pacing or the fraction of a game frame spent translating ARM64 code.
