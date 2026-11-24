# Game smoke tests on Windows

The RustEmuDroid runs used Android Emulator 37.3.10 with the Android 17 Google Play x86-64 image, four guest vCPUs and 8 GiB of guest RAM. Unless a run specifies another setting, the guest display was 1600 × 900 and the active GPU was the GeForce RTX 5070 Ti.

## PUBG MOBILE

The stationary test sampled the same Showroom view for about 18 seconds per run. Only guest display resolution changed between the three samples.

| Run | Guest display | Guest presentations per second | Median interval | P95 interval |
|---|---:|---:|---:|---:|
| A1 | 1600 × 900 | 29.30 | 33.37 ms | 49.57 ms |
| B | 1280 × 720 | 29.25 | 33.35 ms | 49.30 ms |
| A2 | 1600 × 900 | 29.34 | 33.33 ms | 48.94 ms |

For the navigation test, each approximately 23-second sample included three passes from Showroom through Settings and Graphics and back to Showroom. Balanced averaged 28.31 guest presentations per second, with intervals up to 284 ms. The tested Custom configuration averaged 27.80. Balanced at 1600 × 900 was restored after testing. **High** was the highest Lobby frame-rate option the game offered. The game controls which Showroom slots can be disabled. These menu samples cover presentation cadence during the named workloads. Match performance and the effect of changing Showroom slots were outside the test.

On this Android 17 image, `adb shell cmd game list-modes com.tencent.ig` reported Standard as active and offered Standard and Custom. `cmd game list-configs` found no package intervention. The image did not offer Android's Performance game mode for PUBG. On the BlueStacks Pie64 installation, PUBG opened and requested age verification before a comparative Showroom run could begin. [Android Game Mode API](https://developer.android.com/games/optimize/adpf/gamemode/gamemode-api)

## BombSquad

The publisher’s Android Generic test APK, version 1.8.10 build 23095, matched SHA-256 `247ec801df3b4c569637c51388515e9b8c76ea74f478409034ad53a32ef121a4` and ran as native x86-64 in RustEmuDroid. One sample of the 30-second stress-test round with eight simulated players and the default random playlist recorded 1,092 guest presentations during an 18.21-second window, or 59.95 per second. The P95 interval was 18.57 ms. [Ballistica downloads](https://ballistica.net/downloads)

The RustEmuDroid CLI opened the same APK in BlueStacks 5.22.166.1003, instance Pie64. The player reported the same RTX 5070 Ti. Android reported a 1600 × 900 display, and the instance configuration specified four vCPUs and 8 GiB RAM. With the same stress-test settings, the BlueStacks guest presented 1,084 frames in 18.13 seconds, averaging 59.78 per second with a P95 interval of 16.67 ms. Both runs used the random playlist, so the scene and simulated player actions can differ. Each result represents a single short sample. [Windows CLI instructions](../README.md#open-an-installed-bluestacks-instance-on-windows)

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
