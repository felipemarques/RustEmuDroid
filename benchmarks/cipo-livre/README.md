# Cipó Livre, Windows Android guest presentation benchmark

On November 25, 2026, RustEmuDroid v0.22.0 ran Cipó Livre at **59.68–59.98 Android presentations/s** on a GeForce RTX 5070 Ti. The Google Play Android 17 guest used a 1600 × 900 display at 60 Hz. Godot reported Vulkan 1.3 through its Forward Mobile renderer. The local build retained the game's sound and used the two Android audio changes described below.

| Scene, RTX 5070 Ti | RustEmuDroid FPS, runs 1 / 2 | Kemu FPS, runs 1 / 2 | Rust P99, runs 1 / 2 | Kemu P99, runs 1 / 2 |
| --- | ---: | ---: | ---: | ---: |
| Standing in the forest at spawn | 59.88 / 59.98 | 60.00 / 60.04 | 19.98 / 19.47 ms | 19.17 / 19.23 ms |
| Walking forward from spawn into the river | 59.68 / 59.93 | 59.85 / 59.95 | 20.83 / 19.65 ms | 18.70 / 18.94 ms |

Each run lasted 20 seconds. The end-of-run game HUD showed **60 FPS and 80% 3D render scale** in all eight captures. The character was standing in the forest or swimming at the end of the walk in every corresponding run. The longest interval on Rust was 47.87 ms and on Kemu 49.28 ms, both during the first walk. These samples describe interactive play at a 60 Hz target with adaptive resolution. [Presentation timestamps as frame intervals](windows-rtx-vulkan.json) · [HUD evidence and capture hashes](windows-hud-check.json)

Both players used the same release APK and the same physical RTX. Kemu 0.7.1 ran Android 16. Its host engine selected the RTX, while its Android Vulkan device identified itself to the game as Qualcomm Adreno 830. RustEmuDroid v0.22.0 ran Android 17 with the experimental Vulkan memory cache and default clock. Its guest exposed the RTX name through GFXStream. The near-60 rates show how the two configurations played at this display setting. Measuring their maximum throughput would require a separate workload.

## RustEmuDroid on the integrated Radeon 780M

Two independent boots ran the same v0.22.0 Windows client, cached QEMU core, Android 17 Google Play image revision and audio-adjusted x86-64 APK used for the Rust RTX measurements. Each used 1600 × 900 at 60 Hz. The Windows adapter, emulator startup log and Godot's Vulkan device line all identified the **Radeon 780M**. Each number below is a 20-second sample whose final game state was verified on screen.

| Scene | Radeon FPS, boots 1 / 2 | Radeon P99, boots 1 / 2 | 3D render scale at the end |
| --- | ---: | ---: | ---: |
| Standing in the forest | 58.79 / 59.43 | 31.54 / 28.61 ms | 80% / 80% |
| Walking into the river, ending in a swim | 55.13 / 57.19 | 38.58 / 35.56 ms | 80% / 70% |

The integrated GPU had longer frame intervals than the RTX in both matched scenes. The longest interval during the matched Radeon walks was 70.86 ms. Adaptive render scale was active, so the GPU comparison describes the default mobile experience with variable internal resolution. Two other samples from the first Radeon boot ended with the character trotting and on a vine. Their raw data is retained with `endpoint_review.matched = false` and excluded from the table. The second boot repeated one pair. [Frame intervals and per-case validation](windows-radeon-vulkan.json) · [HUD readings and image hashes](windows-hud-check.json)

At 60 Hz, 25 ms spans one and a half refresh periods. Across the two matched walks in RustEmuDroid v0.22.0, **279 of 2,247 intervals (12.42%)** exceeded 25 ms on the Radeon, compared with **7 of 2,393 (0.29%)** on the RTX. The counts use consecutive Android presentation timestamps within each walk. [Radeon intervals](windows-radeon-vulkan.json) · [RTX intervals](windows-rtx-vulkan.json)

## Android audio and the original APK

On the RustEmuDroid Android 17 profile, the original release APK installed and selected the RTX. Android recorded a native exit on the audio thread during forest generation. A separate export changing only the audio driver to Dummy also exited before gameplay, this time on the game's sound-generation thread. For the scored run, the game generated its sounds using the main-thread path it already uses on the web, and looping PCM clips used their last valid sample as the loop endpoint. Both changes were tested together. The Godot engine, Android audio driver, shaders and visual assets came from the same build. The score applies to this locally adjusted APK. [Exact game-source changes](android-audio.patch)

On BlueStacks 5.22.166.1003 Pie64, the original APK started the Godot main loop and synthesized the forest sounds. The renderer reported repeated Vulkan queue presentation errors. Both Android's framebuffer and the foreground Windows player stayed black. There is no gameplay FPS result for this combination. [Renderer, observation time and test conditions](windows-bluestacks-vulkan-start.json)

### BlueStacks with OpenGL Compatibility

Using the audio-adjusted build and switching Godot to its OpenGL Compatibility renderer made the game playable on the same Pie64 test instance and physical RTX. At 1600 × 900 and 60 Hz, the stationary view measured **59.90 / 59.99 presentations/s**. The walk into the river measured **58.18 / 59.97**. The first walk contained one **433.33 ms** interval between presentations. Its P99 was still 16.67 ms. The second walk's longest interval was 16.67 ms. This is a separate rendering condition from the Vulkan comparison above.

The APK keeps the Vulkan build's audio adjustment and adds the Godot arguments `--rendering-method gl_compatibility --rendering-driver opengl3`. The game selected OpenGL ES 3.2 Compatibility and reported an Adreno 650 device identity inside Android. All four end frames showed 60 FPS at 80% render scale. [OpenGL frame intervals and build identity](windows-rtx-opengl.json) · [End-frame HUD readings](windows-hud-check.json)

## Build and measurement

The starting build is Cipó Livre 0.1 (version code 1), package `br.com.felipemarques.cipolivre`. The Android emulator preset exports x86-64 from the pinned Cipó Livre revision using the official Godot 4.7.2 editor and release Android template for Windows. A local QA key signs the APK. Use a single recorded APK SHA-256 across a comparison. The export records each QA audio variant separately. [Cipó Livre source and Android preset](https://github.com/felipemarques/cipo-godot/tree/2f4088e8cd82ca5e210552ab9d6c7511e83891a1) · [Official Godot 4.7.2 downloads](https://godotengine.org/download/archive/4.7.2-stable/)

Each session verifies the foreground game and samples its own Android SurfaceView. After warm-up, it alternates a stationary forest view and a repeatable walk, with two samples per scene. The walk uses four timed touchscreen gestures. Presentation rate is the number of distinct actual-presentation timestamps divided by elapsed collection time. Intervals use adjacent timestamps in the sample and exclude time before its first and after its last timestamp. The output stores the exact gaps in nanoseconds so the timeline can be reconstructed from zero. This measures Android surface presentation, independently of the game HUD and Windows composition. [SurfaceFlinger collector](../pubg-mobile/capture-presentations.py)

Presets declared in the source are defaults. The exporter reports the actual game quality preset only when supplied with a separate verified menu or settings receipt. The renderer requires the game-side Godot device line. A HUD value is a single end-of-sample reading only if OCR identifies one unambiguous FPS and render-scale pair. Missing evidence leaves the corresponding fields empty. Dynamic resolution can change during play. [Source defaults and presets](https://github.com/felipemarques/cipo-godot/blob/2f4088e8cd82ca5e210552ab9d6c7511e83891a1/scripts/settings.gd)

## Export existing run receipts

The exporter reads completed case receipts without operating Android. Give each player a run directory and supply the exact APK build receipt used for those runs.

```powershell
python -B benchmarks/cipo-livre/export.py --build-receipt 'qa/build-receipt.json' `
  --run 'rust=qa/rust-run' --run 'kemu=qa/kemu-run' --run 'blue=qa/blue-run' `
  --output benchmarks/cipo-livre/windows-comparison.json
```

If available, pass `--settings RUN_ID=FILE` with an object such as `{"quality_preset":"media","evidence":"game-settings","vsync":true,"fps_limit":0}`. Use `verified-menu-selection` for a positively verified menu selection. `results.json` and individual frame receipts must agree. An interrupted run with completed cases is identified as partial. The public export carries hashes and selected metadata. The APK, captures, OCR text, raw logs, machine paths and profile identifiers stay out of the export.
