# SuperTuxKart replay with and without CPU scopes

[Eight measured Windows replays on SDK Android 17 and Kemu Android 16](windows-profiler-overhead.md)

Use one isolated signed APK for both modes. Keep the Black Forest replay, Vulkan renderer and original frame report. In the second mode, the benchmark times the outer main-loop iteration and bypasses the other CPU profiler scopes. The A/B changes the clock reads, event tables and locks used by profiling together. [QA patch](cpu-scope-ab.patch)

`--profile-laps` and `--profile-time` select an automatically driven `ProfileWorld` race. `--benchmark` loads the fixed Black Forest replay with ghost karts. Disabling the profiler by its ordinary switch also removes the samples used to calculate `Total frame count` and `Total profiling time`. The QA patch keeps the existing replay and report paths. [STK 1.5 argument handling](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/main.cpp#L1730-L1773) · [Benchmark implementation](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/utils/profiler.cpp#L580-L685)

## Build one Android APK

Use a separate STK checkout, the Linux Android toolchain and the isolated QA identity described in the existing build recipe. It specifies JDK 17, SDK API 35, NDK r28b, STK 1.5 dependencies and the original Black Forest assets. The checked application ID and private save directory are `org.rustemudroid.qa.stkvulkan` and `stk-vulkan-qa`. Use a QA signing key. Both modes must use the resulting exact same APK. [Android QA signing, package names and assets](vulkan-end-scene.md#build-and-record)

```bash
repo=/absolute/path/to/RustEmuDroid
git checkout --detach 1fb491f507216c5d181ccd85f29ff08eca003827
git apply --check "$repo/benchmarks/stk-1.5/vulkan-end-scene.patch"
git apply "$repo/benchmarks/stk-1.5/vulkan-end-scene.patch"
git apply --check --whitespace=error-all "$repo/benchmarks/stk-1.5/cpu-scope-ab.patch"
git apply "$repo/benchmarks/stk-1.5/cpu-scope-ab.patch"

# Finish the isolated package and asset preparation from the recipe above.
cd android
COMPILE_ARCH=x86_64 bash ./make_deps.sh
PROJECT_VERSION=1.5 PROJECT_CODE=328 BUILD_TYPE=beta COMPILE_ARCH=x86_64 \
    STK_QA_VULKAN_END_SCENE=1 bash ./make.sh
```

The published patch passed `git apply --check --whitespace=error-all` against the clean upstream files and again with the Vulkan scope patch applied. The cached Windows-lab build produced an x86-64 1.5/328 APK with SHA-256 `af154c0079f8a822303231076c0e6edb71235005c7839b8109378895b9f5e002`. It matched all 5,600 entries outside `libmain.so` and the signature against the earlier scoped APK and used the same QA signer. The recorded APK digest identifies that particular build. Builds with other signatures or toolchains can have different digests. The separate [Vulkan GPU timestamp patch](vulkan-gpu-timestamps.md) was outside this A/B build.

## Run and preserve the outputs

Install the signed APK in an owned QA guest. Verify the AVD and ADB transport, JNI package, ABI and installed APK hash. Keep the existing STK settings and use a fresh game process per replay. An A/B/B/A order provides two samples per mode while keeping the guest, preset, renderer and presenter fixed. For the High/Vulkan comparison at 90 Hz, also verify the 21 graphics fields, `VkPresentModeKHR=2` (FIFO), guest and panel sizes and the active display cadence. Keep the game VSync flag the same in both modes. The benchmark ignores the software `max_fps` sleep.

| Mode | Temporary `config.xml` `commandline` value | Required result |
| --- | --- | --- |
| A | `--benchmark` | Existing CPU CSV with `Main loop`, original scope columns and the added Vulkan phases |
| B | `--benchmark --qa-frame-totals-only` | `STK_QA_FRAME_TOTALS_ONLY active` in the game's log and exactly one timing column, `Main loop(1)` |

The [replay runner](compare.py) uses A by default. Add `--qa-frame-totals-only` together with `--standalone-vulkan-qa` for B. The runner verifies the uploaded temporary XML, the mode marker, the CSV heading and agreement between its rows and the report totals. Use a fresh output directory for each pass.

```bash
python "$repo/benchmarks/stk-1.5/compare.py" --adb "$ADB" --rust "$SERIAL" \
    --standalone-vulkan-qa --qa-frame-totals-only --rounds 1 \
    --presets high-vulkan --fps-limit 90 --game-vsync on --output b1
```

Keep each pass's original `report.csv`, `quadros.csv`, and `renderer.txt`. For B, `renderer.txt` also contains the verified mode marker. The runner restores the original game XML. Positive `Main loop` rows must equal the report's frame count, and `sum(positive Main loop microseconds) // 1000` must equal report duration in milliseconds. All four historical profiles below satisfied both checks. Preserve SurfaceFlinger presented timestamps and their collection interval separately from full-replay game FPS and any Windows panel counts. [Android presentation collector](../pubg-mobile/capture-presentations.py)

## Boundaries and unavailable fields

A opens the `Main loop` scope at the outer iteration's entry and closes it immediately before the profiler frame synchronization. The normal benchmark enables profiling during the race update, and the next frame synchronization arms the first complete timed iteration. When the race ends, the benchmark turns profiling off during the update. The finished report contains only frames already completed and synchronized before that transition. B times the same outer boundaries with two direct calls to the same `gettimeofday`-based profiler clock. It clears its ordered durations on activation, waits for synchronization to arm them, and observes the disabled profiler flag on the ending iteration before retaining it. The B branch skips all other CPU marker clock reads and the per-frame synchronization clock read. The simulation clock and graphics calls are unchanged. [Outer boundaries](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/main_loop.cpp#L457-L459) · [Race activation and exit phases](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/modes/world_status.cpp)

The original CSV can include a zero-duration initial ring slot when profiling activates mid-frame. B writes its positive finished intervals directly, so it omits that slot. An earlier, separate CPU-profiled GPU timestamp pilot had these distributions. The counts describe exported rows. They cannot count work outside the recorded benchmark interval. [Historical GPU pilot receipts](windows-vulkan-gpu-timestamps.json)

| Historical CPU profile | Positive `Main loop` rows | Nonpositive rows |
| --- | ---: | --- |
| SDK Android 17, GPU-off | 3,268 | One leading 0 |
| SDK Android 17, GPU-on | 3,264 | One leading 0 |
| Kemu Android 16, GPU-off | 3,432 | One leading 0 |
| Kemu Android 16, GPU-on | 3,432 | One leading 0 |

In B, retain the original summary report, the measured `Main loop` column, and the stable-FPS fields derived from those durations. Other CPU scopes are **unavailable**. Leave their table or JSON cells blank or null. A zero in an existing A column can mean the scope did not run in that frame or its duration rounded below one microsecond. The routine GPU CSV has no comparable timings for this B mode. A blank or zero GPU entry carries no measurement of GPU execution. The [phase analyzer](summarize-vulkan-end-scene.py) applies to A. For B, analyze its outer-frame samples and report totals only.

Counting distinct non-root headings with positive duration gives a lower bound of two removed marker reads per heading plus one synchronization read. On the historical GPU-off profiles above, the median lower bound was 51 reads per Android 17 frame and 41 on Kemu. Repeated same-name scopes, other threads and durations that round to zero can increase the real call count. The separate Android 17 clock measurement of 8.44 µs per libc `gettimeofday` implies roughly 0.43 ms for 51 reads. An A/B replay is required to measure any change to FPS or cadence. [Clock probe and limits](windows-guest-clock-cost.md)
