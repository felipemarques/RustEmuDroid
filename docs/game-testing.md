# Game smoke tests on Windows

The RustEmuDroid runs used Android Emulator 37.3.10 with the Android 17 Google Play x86-64 image, four guest vCPUs and 8 GiB of guest RAM. Unless a run specifies another setting, the guest display was 1600 × 900 and the active GPU was the GeForce RTX 5070 Ti.

## SuperTuxKart release comparison

The Black Forest replay in SuperTuxKart 1.5 averaged **59.94 game FPS on RustEmuDroid 0.2.4** and **60.01 on 0.3.0**. Both used the same Android 17 Google Play profile, native x86-64 game, embedded display, direct Vulkan renderer, RTX 5070 Ti and 60 FPS cap. The replay ran at 1600 × 900 with the High preset and 100% render scale. All 21 game-reported graphics fields matched.

| Windows build | FPS, first replay | FPS, second replay | Main-loop P95, pooled |
|---|---:|---:|---:|
| 0.2.4, published binary | 59.88 | 60.01 | 19.24 ms |
| 0.3.0, local release build | 60.08 | 59.93 | 19.12 ms |

The game recorded 2,283–2,288 frames per run. FPS is the frame count divided by the game's replay duration. P95 uses the positive main-loop timings from its own profiler. The two FPS ranges overlap at the frame cap. This check detected no loss in this replay. It covered the normal in-game mapping path with no control held. Uncapped throughput and active tilt or edge-scroll costs remain unmeasured.

To repeat, verify the AVD path and GPU for each desktop version, then run `python benchmarks/stk-1.5/compare.py --adb <adb> --rust <serial> --rounds 2 --presets high-vulkan --output <new-folder>` with one emulator open. The 0.2.4 runs came first. The harness restored the game's configuration byte-for-byte after each run. [Replay harness](../benchmarks/stk-1.5/compare.py)

## PUBG MOBILE

The stationary test sampled the same Showroom view for about 18 seconds per run. Only guest display resolution changed between the three samples.

| Run | Guest display | Guest presentations per second | Median interval | P95 interval |
|---|---:|---:|---:|---:|
| A1 | 1600 × 900 | 29.30 | 33.37 ms | 49.57 ms |
| B | 1280 × 720 | 29.25 | 33.35 ms | 49.30 ms |
| A2 | 1600 × 900 | 29.34 | 33.33 ms | 48.94 ms |

For the navigation test, each approximately 23-second sample included three passes from Showroom through Settings and Graphics and back to Showroom. Balanced averaged 28.31 guest presentations per second, with intervals up to 284 ms. The tested Custom configuration averaged 27.80. Balanced at 1600 × 900 was restored after testing. **High** was the highest Lobby frame-rate option the game offered. The game controls which Showroom slots can be disabled. These menu samples cover presentation cadence during the named workloads. Match performance and the effect of changing Showroom slots were outside the test.

On this Android 17 image, `adb shell cmd game list-modes com.tencent.ig` reported Standard as active and offered Standard and Custom. `cmd game list-configs` found no package intervention. The image did not offer Android's Performance game mode for PUBG. On the BlueStacks Pie64 installation, PUBG opened and requested age verification before a comparative Showroom run could begin. [Android Game Mode API](https://developer.android.com/games/optimize/adpf/gamemode/gamemode-api)

### Where the inventory stalls occur

Two separate 24-second Perfetto captures compared a stationary lobby with repeated opening and closing of the inventory. The producer measurements count `onFrameAvailable` notifications on PUBG's `RHIThread` for its GameActivity SurfaceView. They measure when a new game buffer became available. SurfaceFlinger FrameTimeline measures the guest display compositor separately. [Perfetto FrameTimeline](https://perfetto.dev/docs/data-sources/frametimeline)

| Lobby workload | PUBG buffer notifications per second | Median interval | P95 interval | P99 interval | Longest interval |
|---|---:|---:|---:|---:|---:|
| Stationary | 29.07 | 34.00 ms | 38.85 ms | 43.21 ms | 141.79 ms |
| Inventory navigation | 23.68 | 34.02 ms | 43.51 ms | 248.21 ms | 2.169 s |

During the 2.169-second gap, PUBG's main game thread ran for 1.318 seconds and spent 59 ms runnable. The SurfaceFlinger main thread ran for about 3 ms. During another gap of 542 ms, the game thread ran for 488 ms and the compositor for 7 ms. The long pauses were already visible in the game's buffer cadence. This locates the stalls before composition without identifying which game operation caused them.

SurfaceFlinger classified 699 display frames in the stationary trace and 570 in the navigation trace. It attributed zero recorded misses to its CPU or GPU deadlines and classified one late navigation frame as a display HAL issue. Among known presentations, the compositor's P99 interval rose from 58.70 to 288.46 ms. Its longest interval rose from 164 ms to 2.175 seconds. The traces had no PUBG app-layer FrameTimeline rows, so they could not count the game's own deadline misses. Perfetto reported 10 and 22 FrameTimeline parse errors, respectively, and one discarded chunk during navigation. The counts refer to observed events.

### Does the Lobby Smooth preset help with Inventory?

On RustEmuDroid 0.3.0, three game launches alternated the Custom Lobby graphics setting between Balanced, Smooth and Balanced. The Lobby frame-rate setting stayed on High. Each 48-second run opened Inventory at 0, 12, 24 and 36 seconds and returned to the lobby seven seconds after each open. The Download Center showed paused packs. The game retained its caches between launches.

| Run and Lobby setting | Guest presentations/s, whole route | First open, presentations/s | Three later opens, pooled presentations/s | P99 interval, whole route | Longest interval |
|---|---:|---:|---:|---:|---:|
| A1 ? Balanced | 25.04 | 22.57 | 26.67 | 136 ms | 1,716 ms |
| B ? Smooth | 25.06 | 20.43 | 27.14 | 227 ms | 1,098 ms |
| A2 ? Balanced | 24.67 | 21.71 | 25.48 | 284 ms | 1,099 ms |

The first Inventory open was slower than the pooled later opens in every run. Smooth still had a pause of about a second and its overall rate was within 0.4 presentations/s of both Balanced runs. This comparison did not establish a repeatable fix. The Showroom warning and the game's combat frame rate were outside this route.

The CSV gives each presentation's estimated time relative to the first scheduled Inventory tap, with one sample outside each boundary. Guest timestamps were aligned to the host clock using the smallest observed difference between a poll's completion and its newest guest presentation. Count timestamps inside each fixed window and divide by its length. For intervals, clip pairs that cross a window boundary. [Presentation timestamps](../benchmarks/pubg-mobile/inventory-presentations.csv)

### Does the desktop frame copy affect the lobby?

A local instrumented desktop build alternated the existing allocation-and-copy path with direct RGBA image construction in the same stationary lobby and guest session. Each kept block was about 18 seconds after trimming a second from both ends. The host timing measures elapsed time on the desktop UI thread while it prepares and submits an image to the texture queue. Its block statistic is the median of the reported one-second P95s.

| Block | Desktop copy path | Guest presentations per second | Guest P95 interval | Desktop texture submission P95 |
|---|---|---:|---:|---:|
| A1 | Existing copy | 29.303 | 50.17 ms | 2.286 ms |
| B1 | Direct RGBA construction | 29.288 | 49.64 ms | 1.411 ms |
| A2 | Existing copy | 29.335 | 49.66 ms | 1.340 ms |
| B2 | Direct RGBA construction | 29.322 | 49.18 ms | 1.387 ms |

Guest cadence stayed within 29.288–29.335 presentations per second. The first baseline had higher desktop submission time. After switching back to the existing path, submission time was comparable to both direct-copy blocks. This run provides no repeatable gain from changing the desktop copy path.

## BombSquad

The publisher’s Android Generic test APK, version 1.8.10 build 23095, matched SHA-256 `247ec801df3b4c569637c51388515e9b8c76ea74f478409034ad53a32ef121a4` and ran as native x86-64 in RustEmuDroid. One sample of the 30-second stress-test round with eight simulated players and the default random playlist recorded 1,092 guest presentations during an 18.21-second window, or 59.95 per second. The P95 interval was 18.57 ms. [Ballistica downloads](https://ballistica.net/downloads)

The RustEmuDroid CLI opened the same APK in BlueStacks 5.22.166.1003, instance Pie64. The player reported the same RTX 5070 Ti. Android reported a 1600 × 900 display, and the instance configuration specified four vCPUs and 8 GiB RAM. With the same stress-test settings, the BlueStacks guest presented 1,084 frames in 18.13 seconds, averaging 59.78 per second with a P95 interval of 16.67 ms. Both runs used the random playlist, so the scene and simulated player actions can differ. Each result represents a single short sample. [Windows CLI instructions](../README.md#open-an-installed-bluestacks-instance-on-windows)

## Minkowski Kart

The official ARM64 APK (`version_1.94-android`, version code 258) ran an offline Nürburgring GP race with three AI opponents in the Android 17 x86-64 profile. The game log identified the Vulkan renderer as Goldfish GFXStream with the GeForce RTX 5070 Ti. The Android release uses CPU mesh subdivision and applies relativistic vertex transformations. The workload includes ARM translation, simulation and rendering. [Developer's Android release](https://github.com/rchristie95/MinkowskiKart/releases/tag/version_1.94-android)

Both samples used the Minkowski kart, Intermediate difficulty, three laps, VSync and the game's Custom graphics preset. The Android display was 1600 × 900 on a local instrumented 0.2.4 desktop build. The game's internal resolution scale controls how many pixels it renders inside that display.

| Internal scale | Sampled scene | Presentations | Span | Presentations per second | Median interval | P95 interval |
|---|---|---:|---:|---:|---:|---:|
| 80% (original setting) | Opening sequence into the race | 73 | 17.65 s | 4.08 | 244.86 ms | 267.60 ms |
| 100% | Active race at the starting position, AI racers moving | 71 | 17.30 s | 4.05 | 247.52 ms | 267.77 ms |

At 100%, the game's own overlay read **4/5/5 FPS, 8,261 KTris** in the active race and **4/4/5 FPS, 8,773 KTris** while accelerating. The on-screen steering control accelerated the kart from rest. Race phases differed between samples, so the table does not isolate the effect of resolution. The in-game benchmark button was disabled in the tested configuration. Use **Singleplayer** for the playable test.

Each sample's timestamps in the CSV start at its first observed presentation. Divide the number of intervals by the final timestamp in seconds to reproduce the rates. [Android 17 presentation timestamps](../benchmarks/minkowski-kart-1.94/android-17-presentations.csv)

The same APK installed in BlueStacks 5.22.166.1003 Pie64 (Android 9). The RustEmuDroid 0.3.0 CLI opened the registered activity. Android recorded a native crash in the game process during SDL startup and returned to Home before the main menu. No comparable race measurement was available on that instance.

![Minkowski Kart accelerating at Nürburgring GP. The game's overlay reads 4/4/5 FPS and 8,773 KTris.](minkowski-kart-race.png)

## Box3D for Godot

The developer's Android sample APK from release v0.6.0 ran as native x86-64. Its Godot 4.7.2 log identified Vulkan 1.3, the Forward Mobile renderer and the RTX 5070 Ti through gfxstream. The **Samples → Toys → Ball Flood** scene renders three streams of colored balls falling into a glass tank. **Pause Emitters** holds the body count steady while physics and rendering continue. [Android demo and release](https://github.com/Stink-O/box3d-godot/releases/tag/v0.6.0) · [Scene implementation](https://github.com/Stink-O/box3d-godot/blob/v0.6.0/godot/demo/samples/ball_flood.gd)

| Displayed body count | Sample duration | Guest presentations per second | P95 interval |
|---:|---:|---:|---:|
| 2,921 | 18.22 s | 59.89 | 18.46 ms |
| 10,220 | 18.33 s | 60.00 | 18.50 ms |

Both runs used the initial camera position and paused emitters. Resting bodies can sleep, so the displayed count includes bodies with little ongoing physics work. This scene exercises both rendering and physics. These samples do not establish peak GPU throughput.

## War Thunder Mobile

The official War Thunder Mobile 1.27.0.114 APK (build 7444) contains ARM64 native code and requires Vulkan. The three runs used the same Android 17 Google Play x86-64 profile, installed game and downloaded data. The RTX and SwiftShader runs used RustEmuDroid 0.2.3. The Radeon run used 0.2.4. Each hardware run verified the active Vulkan adapter in the emulator log. [War Thunder Mobile system requirements](https://wtmobile.com/en/news/wtm-on-appgallery)

| Renderer | Last observed milestone |
|---|---|
| Host GPU, GeForce RTX 5070 Ti | Downloaded game assets. The emulator log recorded an invalid color buffer during Dagor Vulkan initialization. The Android shell and emulator console stopped responding as the game started. |
| SwiftShader Device (LLVM 10.0.0) | Android booted and the game opened a “Failed to initialize graphics subsystem” dialog. The guest shell and display remained responsive. |
| Host GPU, integrated Radeon 780M | Dagor created a Vulkan device. The emulator logged an invalid color buffer and the game displayed its own crash dialog. The guest shell and display remained responsive through the end of the 68-second observation. |

All three runs stopped before sign-in, so gameplay performance remains unknown.

## Repeat the measurements

1. Start one idle test profile and confirm the Android image, guest resolution and active renderer in the session diagnostics. Run `adb devices -l` and use the test guest’s serial for every command. Keep recording and screenshot capture idle during the sample.
2. For PUBG MOBILE, open the same Showroom view and sample it at 1600 × 900, 1280 × 720, then 1600 × 900 again. Keep the graphics preset, Lobby frame-rate setting, camera and displayed content fixed. For navigation, repeat the same Showroom → Settings → Graphics → Showroom route three times per approximately 23-second sample.
3. For BombSquad, open **Settings → Advanced → Benchmarks**, then select the stress test, eight simulated players and the default random playlist. Begin sampling during the active 30-second round and stop before the next menu or round. Keep extra participants out of the run.
4. Use `adb -s <serial> shell dumpsys SurfaceFlinger --list` to identify the game’s active surface. Query `dumpsys SurfaceFlinger --latency '<layer>'` through `adb -s <serial> shell` about twice a second during sampling. Collect the second timestamp column, discard zero and pending entries, and deduplicate timestamps across polls. If the surface changes, restart the sample. Divide the number of new presentation intervals by elapsed sampling time and compute frame intervals from consecutive timestamps. These are guest surface presentations. Use the game’s own profiler for claims about game FPS.
5. To repeat the PUBG lobby trace, collect `android.surfaceflinger.frametimeline` and Linux ftrace with `sched/sched_switch`, `sched/sched_waking` and the Android `gfx` and `view` categories. Run one stationary 24-second capture and one while closing and reopening the inventory at roughly 2.7-second intervals. In Trace Processor, group `actual_frame_timeline_slice` by process, layer and jank type. Use the start timestamps of the PUBG SurfaceView's `onFrameAvailable` spans on `RHIThread` for buffer cadence. Use `thread_state` to measure CPU and runnable time inside each gap, and check `stats` for trace loss and parser errors. [Perfetto Trace Processor](https://perfetto.dev/docs/analysis/trace-processor)
