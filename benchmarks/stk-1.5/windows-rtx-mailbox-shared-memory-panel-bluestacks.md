# SuperTuxKart Vulkan Mailbox: the panel on shared memory, the in-band path and BlueStacks, alternated

On November 27, 2026 between 10:23 and 11:07 (local time, UTC−3), one session on the Windows test PC alternated BlueStacks 5 Pie64 with three arms of a **pre-release RustEmuDroid client** on the separately built performance engine: the panel reading Android's screen from shared memory at two panel sizes, and the same build forced to the in-band path of v0.24, where every frame travels inside its gRPC message. Only the Windows Best performance power mode was measured. Kemu was not measured.

| Arm | Laps (game FPS) | Mean |
|---|---|---:|
| BlueStacks 5 Pie64 · Android 9 | 342.71 · 375.28, 374.88 · 371.12, 356.18 · 346.03 (3 boots, 6 laps) | 361.0 |
| RustEmuDroid, shared memory, 800 × 450 panel | 408.49 · 403.98, 386.54 · 391.57 (2 boots, 4 laps) | 397.6 |
| RustEmuDroid, shared memory, default 1040 × 585 panel | 401.67 · 414.97, 372.55 · 414.63 (2 boots, 4 laps) | 401.0 |
| RustEmuDroid, in-band (the v0.24 path), 800 × 450 panel | 367.37 · 389.79, 369.70 · 362.65 (2 boots, 4 laps) | 372.4 |

Each mean is the mean of the lap values.

**What this series shows.** In this session the three RustEmuDroid arms measured more than BlueStacks, by 10.1%, 11.1% and 3.1%. At the 800 × 450 panel, shared memory measured 6.8% more than the in-band path of the same build in the same session (397.6 against 372.4).

**What it does not show.** It does not establish a lead over BlueStacks. BlueStacks measured 361.0 here against 419.3 in the [series of November 26](windows-rtx-mailbox-hypervisor-apic-bluestacks.md) on the same PC, workload and power mode, about 14% lower, and the cause is not known. RustEmuDroid's in-band arm, the one nearest to what was measured then, was 372.4 against 379.1–393.6 in its Best performance series of that day, 2–5% lower, on a different client build. A repeat on an idle desktop, in both power modes and with Kemu, is pending. [What is known about the BlueStacks figure](#the-bluestacks-figure)

**The client was not the published v0.25.0 executable.** It was a build of the shared memory feature made before the release: the first two feature commits of v0.25.0. The feature commits made after that build and the release itself were not timed in this session. [Identities](#identities)

## Workload and conditions

Both players ran SuperTuxKart 1.5's built-in Black Forest replay at High with Vulkan, 1600 × 900 and 100% render scale in Android, Android at 90 Hz and four vCPUs, one guest at a time on the GeForce RTX 5070 Ti of the same PC (Ryzen 9 8945HS). The game's VSync setting was off and `max_fps=90` was written, which the built-in benchmark does not apply. Every replay logged `VkPresentModeKHR: 1` (Mailbox). All 21 graphics fields match between the players and with the series of November 26, and are listed in the data file. Game FPS is the frame count of the game's own frame report divided by its profiled duration, about 38.1 seconds per lap. Each step is one cold boot with one warm-up lap, which is not counted, and two timed laps.

The session driver ran nine timed steps in this order: BlueStacks, shared memory at 800 × 450, in-band at 800 × 450, shared memory at the default panel, BlueStacks, shared memory at the default panel, in-band at 800 × 450, shared memory at 800 × 450, BlueStacks. It held the benchmark lock for the whole session.

**The owner's everyday Android VM.** The BlueStacks operator expects to find the owner's everyday RustEmuDroid VM running, closes it for its timed boot and reopens it afterwards. The RustEmuDroid steps of this session need it closed. So before each BlueStacks step the session opened that VM, which took 45 seconds, and after each BlueStacks step it closed it again. Each BlueStacks step began 25 seconds after that VM had been opened, with its engine using 0.40–0.43 core. In the series of November 26 that VM was also running before every step (0.21–0.43 core) and both operators closed it for their timed boots. Whether booting it this shortly before BlueStacks changes BlueStacks' result was not measured.

| Background load | Host cores |
|---|---:|
| Whole machine before each BlueStacks step, including the engine of the everyday Android VM just opened (0.40–0.43 core) | 0.90 · 1.08 · 1.05 |
| Whole machine before each RustEmuDroid step | 0.32–0.63 |
| Other programs during BlueStacks' timed laps | 0.83 · 0.86 in the first boot, 0.75 · 0.74 in the second, 1.01 · 1.09 in the third |
| Other programs during RustEmuDroid's timed laps | 0.74–1.00 |

The desktop was in use during the session; a web browser and the antivirus service are among the busiest processes in the samples. The two players' samples are not taken the same way: the RustEmuDroid harness samples the whole 48 seconds of a lap run, and the BlueStacks operator samples 19–22 seconds inside each lap.

**Power mode.** Every step, of both players, recorded the Best performance overlay `ded574b5-45a0-4f42-8737-46345c09c238` as the overlay before it, the active one and the one it put back: the mode was already on before the session and stayed on through it. The Windows power plan was not recorded. During BlueStacks' timed laps the first eight logical processors averaged 104.0–104.7% of nominal frequency, against 103.3–103.6% in its Best performance laps of November 26. The RustEmuDroid harness did not sample the processor frequency.

**Panel sizes.** The client ran with **Fit video frames to the window** on, which is off by default: the engine then hands over each frame already reduced to the size of the panel. In the 800 × 450 arms the harness sized the window to 840 × 545 and the client reported an 800 × 450 panel; the resize happened during the warm-up lap, and every timed lap shows only 800 × 450 frames in the client's log. In the default arm the window was left as the client opens it, 1080 × 680, and every lap shows 1040 × 585 frames. A frame is 1.44 MB at 800 × 450 and 2.43 MB at 1040 × 585. Full-size 1600 × 900 frames, which the client uses when that option is off, were not measured. The in-band arm ran at 800 × 450 only.

## Laps in the order they ran

| Start | Player and arm | Run · lap | Game frames | Game FPS | Other programs (cores) | The player's own processes (cores) | Panel pixels that changed between captures | Frames drawn by the panel per second |
|---|---|---|---:|---:|---:|---|---|---:|
| 10:25 | BlueStacks | `blue-profile-albsduel11` · 1 | 13,067 | 342.71 | 0.83 | 1.62 player + 0.17 helper | not captured | not sampled |
| 10:26 | BlueStacks | `blue-profile-albsduel11` · 2 | 14,307 | 375.28 | 0.86 | 1.61 player + 0.26 helper | not captured | not sampled |
| 10:30 | RustEmuDroid, shared, 800 × 450 | `sbsduel1-cshm-1` · 1 | 15,575 | 408.49 | 0.90 | 2.97 client + engine | 91–95% | 85.0 |
| 10:31 | RustEmuDroid, shared, 800 × 450 | `sbsduel1-cshm-1` · 2 | 15,403 | 403.98 | 0.78 | 2.85 client + engine | 91–96% | 82.0 |
| 10:35 | RustEmuDroid, in-band, 800 × 450 | `sbsduel1-cinl-1` · 1 | 14,006 | 367.37 | 0.74 | 3.06 client + engine | 90–96% | 80.5 |
| 10:35 | RustEmuDroid, in-band, 800 × 450 | `sbsduel1-cinl-1` · 2 | 14,861 | 389.79 | 0.76 | 3.05 client + engine | 90–94% | 81.4 |
| 10:39 | RustEmuDroid, shared, default panel | `sbsduel1-cshmd-1` · 1 | 15,314 | 401.67 | 1.00 | 2.88 client + engine | 90–95% | 83.4 |
| 10:40 | RustEmuDroid, shared, default panel | `sbsduel1-cshmd-1` · 2 | 15,821 | 414.97 | 0.80 | 2.90 client + engine | 90–95% | 83.2 |
| 10:44 | BlueStacks | `blue-profile-albsduel12` · 1 | 14,292 | 374.88 | 0.75 | 1.48 player + 0.29 helper | not captured | not sampled |
| 10:45 | BlueStacks | `blue-profile-albsduel12` · 2 | 14,149 | 371.12 | 0.74 | 1.63 player + 0.23 helper | not captured | not sampled |
| 10:50 | RustEmuDroid, shared, default panel | `sbsduel1-cshmd-2` · 1 | 14,205 | 372.55 | 0.85 | 2.88 client + engine | 89–95% | 85.4 |
| 10:50 | RustEmuDroid, shared, default panel | `sbsduel1-cshmd-2` · 2 | 15,808 | 414.63 | 0.81 | 2.90 client + engine | 91–94% | 81.7 |
| 10:54 | RustEmuDroid, in-band, 800 × 450 | `sbsduel1-cinl-2` · 1 | 14,095 | 369.70 | 0.82 | 2.92 client + engine | 90–94% | 80.2 |
| 10:55 | RustEmuDroid, in-band, 800 × 450 | `sbsduel1-cinl-2` · 2 | 13,828 | 362.65 | 0.86 | 2.96 client + engine | 91–94% | 84.3 |
| 10:58 | RustEmuDroid, shared, 800 × 450 | `sbsduel1-cshm-2` · 1 | 14,738 | 386.54 | 0.89 | 2.87 client + engine | 90–94% | 85.0 |
| 10:59 | RustEmuDroid, shared, 800 × 450 | `sbsduel1-cshm-2` · 2 | 14,929 | 391.57 | 0.95 | 2.83 client + engine | 91–94% | 83.2 |
| 11:04 | BlueStacks | `blue-profile-albsduel13` · 1 | 13,580 | 356.18 | 1.01 | 1.68 player + 0.25 helper | not captured | not sampled |
| 11:05 | BlueStacks | `blue-profile-albsduel13` · 2 | 13,193 | 346.03 | 1.09 | 1.43 player + 0.23 helper | not captured | not sampled |

Warm-up laps, not counted: BlueStacks 367.15 · 373.71 · 351.42; shared memory at 800 × 450, 414.05 · 390.83; in-band, 376.11 · 354.64; shared memory at the default panel, 416.45 · 397.29.

| Arm | Laps | Mean | Lowest | Highest | Sample standard deviation |
|---|---:|---:|---:|---:|---:|
| BlueStacks 5 Pie64 | 6 | 361.03 | 342.71 | 375.28 | 14.70 |
| RustEmuDroid, shared memory, 800 × 450 | 4 | 397.65 | 386.54 | 408.49 | 10.30 |
| RustEmuDroid, shared memory, default panel | 4 | 400.95 | 372.55 | 414.97 | 19.92 |
| RustEmuDroid, in-band, 800 × 450 | 4 | 372.38 | 362.65 | 389.79 | 11.97 |

With four laps per RustEmuDroid arm and spreads of this size, the series shows the direction and not the size of the differences. Three of the four in-band laps (362.65–369.70) are below every shared memory lap at the same panel size (386.54–408.49), and one in-band lap (389.79) is inside that range. The two shared memory arms can not be told apart: the default panel has both the fastest lap of the session and one lap at 372.55. The RustEmuDroid arms ran in mirrored order around the second BlueStacks step, and the 800 × 450 shared memory arm measured more in its first boot, at the start of the session, than in its second, near the end (408.49 · 403.98 against 386.54 · 391.57).

### The picture in the panel was live

An earlier version of this feature showed a frozen panel while its counters looked healthy, and its figures were discarded. In this session the harness therefore captured the client window six times during every lap and compared consecutive captures. In every timed lap of the three arms, 89–96% of the panel's pixels changed from one capture to the next, and the harness's measure of a panel filled with one colour stayed at or below 0.3%: the panel showed the moving race. The client reported `SharedMemory` as its video transport in the four shared memory steps and `Inline` in the two in-band steps.

The panel processed 80.2–85.4 frames per second for drawing in the timed laps (the client's own frame statistics, averaged by the harness over each lap), with the game rendering 363–415 frames per second into a 90 Hz Android display. Mailbox replaces a pending image before the next refresh, so game FPS measures how fast the game renders, not how many frames reach the screen. Android presentations, physical display scanout and input latency were not measured in this series. No frame file of the client was found in the temporary folder before or after any step.

### The BlueStacks figure

| | November 26, 16:06–16:47 | November 27, 10:23–11:07 |
|---|---|---|
| BlueStacks, Best performance | 432.66 · 428.16, 399.54 · 416.67 → 419.3 | 342.71 · 375.28, 374.88 · 371.12, 356.18 · 346.03 → 361.0 |
| Other programs during its laps (cores) | 0.56 · 0.56, 0.95 · 0.76 | 0.83 · 0.86, 0.75 · 0.74, 1.01 · 1.09 |
| Processor frequency during its laps (first eight logical processors, % of nominal) | 103.3–103.6 | 104.0–104.7 |
| The player's own processes (cores) | 1.56–1.58 player + 0.16–0.25 helper | 1.43–1.68 player + 0.17–0.29 helper |
| Everyday Android VM before the step | running (0.21–0.43 core); the operators closed it for their timed boots and reopened it afterwards | opened by the session 25 seconds before the step (0.40–0.43 core); the operator closed it for its timed boot |

The game package, the 21 graphics settings, the instance, the Android build and the GPU line are the same in both series. What differs in the receipts is the time of day, the background load, and how shortly before each BlueStacks boot the everyday Android VM had itself been booted. Nothing in the receipts attributes the difference to one of them. The slowest BlueStacks laps of this session are the first lap after the first of those boots (342.71) and the two laps of the third boot (356.18 · 346.03), the boot with the most background load.

## Identities

| Item | RustEmuDroid | BlueStacks |
|---|---|---|
| Desktop client or player | A Windows build made before the release, SHA-256 `60561afa04a51343bf7538811400d49905d75fa095b0a533ff2ba7390df15dd5`, the same file in all six steps. The lab notes of the session name commit `a7efd99` of the branch `feature/display-shared-memory` as its source: the first two feature commits of v0.25.0 on top of the v0.24.0 source, where the Cargo version is 0.24.0 and the Linux-only change of v0.24.1 is absent. The display code of that commit (`src/display.rs`, `src/display/`, `src/ui.rs`) is identical to the release branch at its second feature commit, "Keep the shared memory panel live and fall back when the engine writes elsewhere". The receipts hold the hash only: none ties it to a commit or records the version the executable reports. **It is not the published v0.25.0 executable.** | BlueStacks 5 official player with the lab instance `Pie64_1`. The player version was not recorded in these receipts. |
| Who applied the settings | A lab harness started the client (`ui --play`, English) on a disposable lab profile with `--sdk` pointing at the engine, set the engine environment and, for the in-band arm, `RUSTEMUDROID_VIDEO_TRANSPORT=inline`. The client's own performance engine setting was not used. Preferences: Host GPU, 90 Hz, **Fit video frames to the window** on. | The operator's preference transaction prepares the lab instance for the run and puts its frame-rate preferences back afterwards (these receipts record the restore, not the values), and the operator installed the QA APK. Android reported a 90 Hz display. |
| Engine | Android Emulator 37.3.10.0 (build 16970787) from a stage folder whose staging record of November 26 lists the stock `emulator.exe` (`7476c5d7cf249182b7310d45e5304778bd5a337e8d6d11890edb27954b0f9717`), the rebuilt QEMU core `fc990b45532c16008327a15e07a15297963716d7dadb9e46ecab4acf3c1cb94f` and the rebuilt gfxstream renderer `7d01cde7d9450e050effb4805c87dcf4c0cfaa274a290ac8f526f192eac085fe`: the same three files as in the series of November 26, the "published sources" build of the [engine source kit](../../experiments/whpx-clock/README.md#4-build-the-performance-engine). They were not hashed again during this session. | BlueStacks' own engine |
| Android image | Official Android 17 Google Play x86-64 image, `google/sdk_gphone64_x86_64/emu64xa:17/CE2A.260420.051.A2/16418371:user/release-keys`, API 37. The package revision was not recorded in these receipts. | Android 9, API 28, `samsung/b0qxxx/b0q:9/TP1A.220624.014/S908EXXS2BWA2:user/release-keys` |
| GPU | `Selecting Vulkan device: NVIDIA GeForce RTX 5070 Ti, Version: 1.4.341` in the engine log, driver 610.88. The game reported `Goldfish GFXStream (NVIDIA GeForce RTX 5070 Ti)`. | The player logged `GL_RENDERER = NVIDIA GeForce RTX 5070 Ti/PCIe/SSE2`. The game reported `NVIDIA GeForce RTX 5070 Ti`, driver 610.88.0.0. |
| Game package | QA build of SuperTuxKart 1.5 (version code 328, x86-64), SHA-256 `dbde91eb1451e5f165cbc09b8d1144d13d35283d5f6752cec52704e3e696ff8e` | The operator's profiling build of that package, SHA-256 `f0ef0810317ec667e93a5d79dbaea67bc7849964463508508b71deee483427de`, which Android lists as debuggable; its receipt names `dbde91eb…` as the base. No profiler was requested for these laps. |
| Measurement source | The game's frame report | The game's frame report |

The two players therefore did not run byte-identical game packages, as in the series of November 26. Whether the debuggable flag changes the game's speed on Android 9 was not measured.

**Engine settings in every RustEmuDroid boot.** Environment set by the harness: `RUSTEMUDROID_QA_WHPX_SYNC_TSC=1`, `RUSTEMUDROID_QA_WHPX_APIC=hv`, `RUSTEMUDROID_QA_VULKAN_CACHED_HOST=1`, `RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL=2`, `RUSTEMUDROID_QA_ASG_SPIN_US=3000`, `RUSTEMUDROID_QA_VCPU_PLACEMENT=sets-high` and `ANDROID_EMULATOR_FEATURES=VulkanNativeSwapchain,-QuickbootFileBacked`. Unlike the series of November 26 it did not set `RUSTEMUDROID_QA_LAPIC_FASTPATH` and `RUSTEMUDROID_QA_KVM_PV_EOI`, which the hypervisor interrupt controller replaces. The lab profile's `config.ini` holds `hw.gltransport=asg` and `kernel.parameters=mitigations=off init_on_alloc=0 transparent_hugepage=always`; it was read after the session and had last been written on November 26. Android reported `gltransport=asg` in every step. The guest's kernel command line was not read back in this series. In the client's Settings these correspond to the performance engine switches, including **Reduce Android kernel protections**, which is off by default.

The engine log of all six boots has the line `RustEmuDroid QA: WHPX local APIC emulated by the hypervisor (xAPIC)`, the TSC markers `synchronized init vcpus=4` and `synchronized reset vcpus=4`, the consumer spin of 3000 µs, the `sets-high` vCPU placement, both Vulkan cache lines and `VulkanNativeSwapchain` enabled. The client confirmed each boot in 20.7–22.3 seconds, and the harness waited 25–30 seconds until the engine was quiet before installing the game.

## What failed

- **BlueStacks, all three steps.** The timed laps completed and their game reports were kept. Each step then ended with exit status 1 and its receipt says `failed`, as in the series of November 26: closing the player through its exit dialog did not work (`Another HWND covers the Rust client`), and the restore transaction refused to run while the player was open (`Close all BlueStacks players and Instance Manager first: hd-player.exe`). The operator then ended `hd-player.exe`, stopped its wrapper and ran the restore. The receipts record the frame-rate preferences and the wrapper preferences as restored, the QA package as removed, the stock game and its configuration as preserved, no leftover process, and BlueStacks' configuration file as not byte-identical to the copy taken before the run (`host_config_byte_identity: false`).
- **RustEmuDroid, all six steps.** They ended with exit status 0, with no crash line in the game log, no leftover process to end and no change recorded to the owner's own installation.
- **Not sampled.** Android presentations in either player, the processor frequency during RustEmuDroid's laps, and the panel of BlueStacks.

## What this record does not show

- A lead over BlueStacks, for the reasons given at the top. The comparison that would show one, on an idle desktop, in both power modes, with Kemu and with the everyday VM in the same state for every step, has not been run.
- The speed of the **published v0.25.0** executable, of the feature commits made after the timed build, or of a client applying the engine settings from its own preferences.
- Shared memory with any other engine. It has run only with this performance engine build: not with the stock Android Emulator engine that first-time setup installs and the client uses by default, with the Vulkan cache engine or with Kemu's engine. The performance engine is built separately from the published source kit and is not part of the Windows download.
- The gain of shared memory with **Fit video frames to the window** off, the default, where every frame is 1600 × 900; at the default panel size against the in-band path; or with the Balanced power mode.
- Linux, where shared memory is off unless it is asked for and has never run against a real engine.
- Other games, other GPUs, other PCs, longer sessions or snapshots.

## Recalculate the results

Game FPS is `1000 × frames / profiled duration in ms` from each lap's report. The means are arithmetic means of the lap values; pooling frames over durations gives the same figures to one decimal. The data file lists every lap with its frame count, duration, start and end time, the SHA-256 of its game report and per-frame profile and the host load sample, and for RustEmuDroid the fraction of panel pixels that changed between captures and the frames the panel drew. It also lists every step of the session with its exit status and the load before it, the checks of each boot, and what is not established. [Data, identities and hashes](windows-rtx-mailbox-shared-memory-panel-bluestacks.json)

The lap files themselves (game reports, per-frame profiles, window captures, engine and client logs) stay on the test PC and are not published with this record. Earlier Mailbox measurements of the same workload: [the alternated series of November 26](windows-rtx-mailbox-hypervisor-apic-bluestacks.md) · [three players on the v0.22.0 client with the cache engine](windows-rtx-mailbox-peers.md) · [the series of November 25–26](../../docs/performance-history.md#performance-engine-in-uncapped-mailbox).
