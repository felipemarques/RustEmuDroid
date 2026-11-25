# SuperTuxKart Vulkan scene preparation on the render thread

In the current Windows three-player series, the two SDK Android 17 replays recorded **96 of 3,265** and **105 of 3,254** main-loop intervals above 16,667 µs. The `drawAll() for kart 0` elapsed scope was on average **7.695 and 8.083 ms longer** in those frames than in the other positive-duration frames. `VK endScene` was **1.376 and 1.153 ms shorter**. The optional Android QA probe separates elapsed and render-thread CPU time inside `GEVulkanSceneManager::drawAllInternal` during the Black Forest replay. Both modes keep the detailed CPU profiler and full replay frame report. [Separate official three-player series](windows-current-three-players.md) · [Prior GPU timestamps](windows-vulkan-gpu-timestamps.md) · [QA patch](vulkan-draw-all-ab.patch)

## Build and run

Use STK upstream commit `1fb491f507216c5d181ccd85f29ff08eca003827` and the existing [isolated Android build recipe](vulkan-end-scene.md#build-and-record), including the `vulkan-end-scene.patch` CPU scopes and the original Black Forest assets. Apply `git apply --check --whitespace=error-all /path/to/vulkan-draw-all-ab.patch`, then `git apply /path/to/vulkan-draw-all-ab.patch`. The Android source build includes the probe. The flag is recognized only inside `--benchmark`. Build and sign one x86-64 QA APK `org.rustemudroid.qa.stkvulkan` for both modes. Keep the original game and its save data separate.

The verified lab build used NDK r28b and produced APK SHA-256 `df06c5e48cd8420bed3ce50ad9e4b36ae4dd4eb4389e23ece08b9f7a094b291f`. The package matched the earlier scoped APK's QA certificate and all 5,600 ZIP entries outside the signature and `libmain.so` by size and CRC. The stripped x86-64 game ELF retains both markers and constructs `--qa-draw-all` with overlapping immediate stores. The game log and CSV provide the runtime confirmation.

Use both modes in A/B/B/A order in the same owned guest and renderer configuration. Keep the original 21 graphics settings, 1600 × 900 guest, Vulkan FIFO, active 90 Hz, and the same fitted physical Rust window and VSync choice. Use a new replay process and output directory each time. Verify the confirmed AVD and APK, and collect Android SurfaceView cadence separately over the same in-game interval. Build and other foreground work should finish before timing starts.

```sh
# Select the checked serial and a fresh output directory on each pass.
python benchmarks/stk-1.5/compare.py --adb "$ADB" --rust "$SERIAL" \
    --standalone-vulkan-qa --rounds 1 --presets high-vulkan \
    --fps-limit 90 --game-vsync on --output a1

python benchmarks/stk-1.5/compare.py --adb "$ADB" --rust "$SERIAL" \
    --standalone-vulkan-qa --qa-draw-all --rounds 1 --presets high-vulkan \
    --fps-limit 90 --game-vsync on --output b1
```

Mode A uses `--benchmark`. Mode B adds `--qa-draw-all`. B waits for fresh `stdout.log.qa-draw-all-black_forest.csv` plus the activation and terminal markers `STK_QA_DRAW_ALL_V1 active` and `STK_QA_DRAW_ALL_V1 end rows=N`. The terminal marker follows the closed file. The runner retains `draw-all.csv`, `quadros.csv`, `report.csv` and `renderer.txt` and restores the original XML. It validates sequential row IDs, phase masks, blank fields, phase sums, and equality with the terminal row count and the original frame-count and duration report. It reports coverage among timed frames with a positive kart 0 draw scope. A missing or partial sample remains visible in the coverage counters.

## Boundaries and overhead

- Only camera 0's `drawAll()` in `FixedPipelineRenderer` arms the probe. The directly paired phases are `updateCache`, the camera lookup, `OnAnimate`, camera render and draw-call lookup, `prepare`, `OnRegisterSceneNode` and `generate`. The total spans the entry to `drawAllInternal` through completion of `generate`. It excludes the enclosing renderer's camera activation and `preRenderCallback`, and the later `endScene`.
- The samples use pairs of `CLOCK_MONOTONIC` elapsed time and `CLOCK_THREAD_CPUTIME_ID` for the calling thread. A complete call takes eight boundary pairs (16 clock reads). Adjacent phases share a boundary. The probe writes into an in-memory slot selected by the existing profiler frame index. It allocates its buffer when profiling starts and exports at the end. Mode A performs no extra clock reads, sample allocation or export. Mode B may itself change pacing, especially on HPET. Use the same APK's A/B frame reports and SurfaceView cadence to quantify that effect before interpreting B.
- `profiler_row` is a zero-based data-row ordinal in `quadros.csv`, including any initial nonpositive main-loop row. Use positive main-loop and kart 0 draw durations. Classify a frame as long when `main_loop_us * refresh_hz > 1_500_000`. At 90 Hz a 16,667 µs frame therefore counts as long. The earlier three-player analysis used a strict cutoff of 16,667 µs. Report eligible and complete counts for both groups. Clock failures and incomplete calls have blank missing values. The summed elapsed and thread CPU times include the probes' own boundary costs. Their difference can include scheduling delays, waits and measurement overhead.

In this build, `generate` gathers and orders visible mesh groups and writes per-frame data into mapped buffers. Its timing spans the entire function, including any Vulkan work called from it. Comparing elapsed time with the render thread's CPU time distinguishes time executing on that thread from time spent off CPU. It does not partition individual operations inside the function. [Draw-call generation at the tested revision](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/lib/graphics_engine/src/ge_vulkan_draw_call.cpp#L349-L424)

[Pinned fixed pipeline](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/graphics/fixed_pipeline_renderer.cpp) · [Pinned Vulkan scene manager](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/lib/graphics_engine/src/ge_vulkan_scene_manager.cpp)
