# SuperTuxKart Vulkan Mailbox: performance engine with the hypervisor APIC and BlueStacks, alternated

On November 26, 2026 between 16:06 and 16:47 (local time, UTC−3), RustEmuDroid with the separately built performance engine and BlueStacks 5 Pie64 took turns on the Windows test PC in both Windows power modes. BlueStacks was ahead in both: **402.5 against 392.9 game FPS with Balanced** and **419.3 against 393.6 with Best performance**. The Rust window was the **v0.22.0** client with a test operator applying the engine settings, so this is a historical comparison on an older client. It does not time the v0.24.1 client. Kemu was not measured.

| Windows power mode | RustEmuDroid, performance engine, hypervisor APIC | BlueStacks 5 Pie64 · Android 9 | BlueStacks ahead by |
|---|---|---|---:|
| Balanced | 391.60 · 394.28 → **392.9** (1 boot, 2 laps) | 396.21 · 391.12 · 404.72 · 417.81 → **402.5** (2 boots, 4 laps) | 2.4% |
| Best performance | 390.13 · 362.94 · 404.24 · 412.33 · 402.65 · 395.27 · 367.06 · 414.13 → **393.6** (4 boots, 8 laps) | 432.66 · 428.16 · 399.54 · 416.67 → **419.3** (2 boots, 4 laps) | 6.5% |

Each mean is the mean of the lap values. The percentage is BlueStacks' mean divided by RustEmuDroid's, minus one. Seen from the other side, RustEmuDroid was 2.4% and 6.1% below BlueStacks. The Balanced figure for RustEmuDroid rests on two laps of one boot, because the other two attempts failed as described under [What failed](#what-failed).

## Workload and conditions

Both players ran SuperTuxKart 1.5's built-in Black Forest replay at High with Vulkan, 1600 × 900 and 100% render scale in Android, Android at 90 Hz and four vCPUs, one guest at a time on the GeForce RTX 5070 Ti of the same PC (Ryzen 9 8945HS). The game's VSync setting was off and `max_fps=90` was written, which the built-in benchmark does not apply. Every replay logged `VkPresentModeKHR: 1` (Mailbox). All 21 graphics fields match between the players and are listed in the data file. Game FPS is the frame count of the game's own frame report divided by its profiled duration, about 38.1 seconds per lap. Each boot was cold and ran one warm-up lap, which is not counted, and two timed laps.

The session driver ran eight steps in this order: RustEmuDroid Balanced, BlueStacks Balanced, RustEmuDroid Best performance, BlueStacks Best performance, and the same four again. A RustEmuDroid step is two boots and a BlueStacks step is one boot. The driver held the benchmark lock, waited until no compiler was running and paused 45 seconds between steps. Its record calls this the idle-desktop series. The desktop was quieter than in the earlier daytime series of the same day, not idle:

| Background load | Host cores |
|---|---:|
| Whole machine in the 10 seconds before each step, including the owner's everyday Android VM (0.21–0.43 core), which the operator closes for the timed boots and reopens afterwards | 0.73–0.88 |
| Other programs during RustEmuDroid's timed laps | 0.54–0.68 |
| Other programs during BlueStacks' timed laps with Balanced | 0.59–0.62 |
| Other programs during BlueStacks' timed laps with Best performance | 0.56 · 0.56 in the first boot, 0.95 · 0.76 in the second |

The second BlueStacks boot with Best performance, the one with more background load, is also its slower one (399.54 · 416.67 against 432.66 · 428.16). This series does not separate that load from ordinary boot-to-boot variation.

**Power mode.** Balanced means no Windows power mode overlay. For Best performance the operators activated the overlay `ded574b5-45a0-4f42-8737-46345c09c238` over the previous one (`00000000-0000-0000-0000-000000000000`, none). The log of every Best performance boot shows both values, and the BlueStacks steps also log the restore to none. The Balanced steps did not record the overlay themselves. Every reading the other steps took outside an active Best performance boot was none: before each of the six Best performance boots, between 16:12 and 16:43, and after both BlueStacks Best performance steps. The Windows power plan was not recorded. During the timed laps the first eight logical processors averaged 95.3–95.8% of nominal frequency for RustEmuDroid with Balanced and 107.7–109.0% with Best performance, and 84.0–86.0% and 103.3–103.6% for BlueStacks.

## Laps in the order they ran

| Start | Player | Power mode | Run · boot · lap | Game frames | Game FPS | Other programs (cores) | The player's own processes (cores) |
|---|---|---|---|---:|---:|---:|---|
| 16:08 | BlueStacks | Balanced | `blue-profile-idleb1` · 1 | 15,104 | 396.21 | 0.59 | 1.56 player + 0.18 helper |
| 16:09 | BlueStacks | Balanced | `blue-profile-idleb1` · 2 | 14,912 | 391.12 | 0.60 | 1.64 player + 0.29 helper |
| 16:13 | RustEmuDroid | Best performance | `arms-mailbox-idleo1` · h1 · 1 | 14,874 | 390.13 | 0.61 | 2.61 QEMU + 0.26 client |
| 16:14 | RustEmuDroid | Best performance | `arms-mailbox-idleo1` · h1 · 2 | 13,838 | 362.94 | 0.56 | 2.43 QEMU + 0.22 client |
| 16:17 | RustEmuDroid | Best performance | `arms-mailbox-idleo1` · h2 · 1 | 15,413 | 404.24 | 0.54 | 2.60 QEMU + 0.26 client |
| 16:17 | RustEmuDroid | Best performance | `arms-mailbox-idleo1` · h2 · 2 | 15,721 | 412.33 | 0.66 | 2.60 QEMU + 0.26 client |
| 16:21 | BlueStacks | Best performance | `blue-profile-ov-idle1` · 1 | 16,494 | 432.66 | 0.56 | 1.58 player + 0.16 helper |
| 16:22 | BlueStacks | Best performance | `blue-profile-ov-idle1` · 2 | 16,324 | 428.16 | 0.56 | 1.57 player + 0.25 helper |
| 16:26 | RustEmuDroid | Balanced | `arms-mailbox-idleb2` · hb1 · 1 | 14,930 | 391.60 | 0.68 | 2.78 QEMU + 0.28 client |
| 16:27 | RustEmuDroid | Balanced | `arms-mailbox-idleb2` · hb1 · 2 | 15,033 | 394.28 | 0.67 | 2.73 QEMU + 0.25 client |
| 16:32 | BlueStacks | Balanced | `blue-profile-idleb2` · 1 | 15,429 | 404.72 | 0.62 | 1.58 player + 0.16 helper |
| 16:32 | BlueStacks | Balanced | `blue-profile-idleb2` · 2 | 15,928 | 417.81 | 0.60 | 1.54 player + 0.08 helper |
| 16:37 | RustEmuDroid | Best performance | `arms-mailbox-idleo2` · h1 · 1 | 15,352 | 402.65 | 0.61 | 2.74 QEMU + 0.26 client |
| 16:37 | RustEmuDroid | Best performance | `arms-mailbox-idleo2` · h1 · 2 | 15,070 | 395.27 | 0.60 | 2.67 QEMU + 0.26 client |
| 16:40 | RustEmuDroid | Best performance | `arms-mailbox-idleo2` · h2 · 1 | 13,996 | 367.06 | 0.65 | 2.61 QEMU + 0.27 client |
| 16:41 | RustEmuDroid | Best performance | `arms-mailbox-idleo2` · h2 · 2 | 15,788 | 414.13 | 0.57 | 2.67 QEMU + 0.26 client |
| 16:45 | BlueStacks | Best performance | `blue-profile-ov-idle2` · 1 | 15,232 | 399.54 | 0.95 | 1.56 player + 0.23 helper |
| 16:45 | BlueStacks | Best performance | `blue-profile-ov-idle2` · 2 | 15,885 | 416.67 | 0.76 | 1.56 player + 0.20 helper |

Warm-up laps, not counted: RustEmuDroid 411.67 (Balanced) and 426.71 · 420.98 · 430.93 · 395.11 (Best performance); BlueStacks 404.22 · 399.37 (Balanced) and 448.05 · 425.94 (Best performance).

| Player and power mode | Laps | Mean | Lowest | Highest | Sample standard deviation |
|---|---:|---:|---:|---:|---:|
| RustEmuDroid, Balanced | 2 | 392.94 | 391.60 | 394.28 | 1.90 |
| BlueStacks, Balanced | 4 | 402.46 | 391.12 | 417.81 | 11.66 |
| RustEmuDroid, Best performance | 8 | 393.60 | 362.94 | 414.13 | 19.37 |
| BlueStacks, Best performance | 4 | 419.26 | 399.54 | 432.66 | 14.77 |

Two of RustEmuDroid's eight laps with Best performance were slow, 362.94 and 367.06, the second lap of one boot and the first lap of another. The other six ranged from 390.13 to 414.13. No cause was identified. RustEmuDroid measured nearly the same in both power modes, and BlueStacks measured about 4% more with Best performance than with Balanced (419.3 against 402.5). With two to eight laps per cell and lap-to-lap spreads of this size, the series shows that BlueStacks was ahead here. It does not fix the size of the difference to better than a few percent.

### RustEmuDroid frame scopes and Android presentation

| Power mode | Boot · lap | Game FPS | Main loop (ms/frame) | drawAll (ms/frame) | Vulkan acquire (ms/frame) | Android presentations/s | Android P99 interval (ms) |
|---|---|---:|---:|---:|---:|---:|---:|
| Best performance | `idleo1` h1 · 1 | 390.13 | 2.563 | 1.882 | 0.085 | 89.98 | 12.79 |
| Best performance | `idleo1` h1 · 2 | 362.94 | 2.755 | 2.016 | 0.091 | 89.98 | 12.79 |
| Best performance | `idleo1` h2 · 1 | 404.24 | 2.474 | 1.820 | 0.079 | 89.98 | 12.64 |
| Best performance | `idleo1` h2 · 2 | 412.33 | 2.425 | 1.773 | 0.080 | 90.08 | 12.82 |
| Balanced | `idleb2` hb1 · 1 | 391.60 | 2.554 | 1.863 | 0.089 | 90.08 | 12.89 |
| Balanced | `idleb2` hb1 · 2 | 394.28 | 2.536 | 1.847 | 0.089 | 90.01 | 12.98 |
| Best performance | `idleo2` h1 · 1 | 402.65 | 2.484 | 1.820 | 0.083 | 90.03 | 12.92 |
| Best performance | `idleo2` h1 · 2 | 395.27 | 2.530 | 1.871 | 0.082 | 90.03 | 12.67 |
| Best performance | `idleo2` h2 · 1 | 367.06 | 2.724 | 2.023 | 0.084 | 90.03 | 12.81 |
| Best performance | `idleo2` h2 · 2 | 414.13 | 2.415 | 1.755 | 0.082 | 89.94 | 13.00 |

The game rendered about 390 frames per second while Android presented at the 90 Hz display rate. Mailbox replaces a pending image before the next refresh, so these figures measure how fast the game renders, not how many frames reach the screen. The scopes are elapsed time on the game's main thread and include waits. The BlueStacks runs used the operator's lap-only mode: they kept the game report and the per-frame profile and took no Android presentation sample.

The client drew the game in an 800 × 450 panel; its log shows the 800 × 450 frames arriving during the laps. The panel frame rate was not sampled, because the operator's check that the preview is the Windows foreground window did not pass in any lap (`Expected preview Windows foreground`). Physical display scanout and input latency were outside this measurement.

## Identities

| Item | RustEmuDroid | BlueStacks |
|---|---|---|
| Desktop client or player | RustEmuDroid **0.22.0** Windows executable, SHA-256 `226e6b0b69b10092d42674b6e256f00f02796b07662fcd6b4fef785feadf10cb`. Earlier records in this folder identify this hash as the official v0.22.0 release executable. | BlueStacks 5 official player with the lab instance `Pie64_1`. The player version was not recorded in these receipts. |
| Who applied the settings | A test operator started the client with a scoped `--sdk` and set the engine environment, the AVD keys and the power mode. The v0.22.0 client has no performance engine setting. | The operator's preference transaction prepares the lab instance for the run (it writes `enable_high_fps=1`, `max_fps=90` and `enable_vsync=0` and puts back 0, 60 and 1 afterwards; these receipts record the restore, not the values), and the operator installed the QA APK. Android reported a 90 Hz display. |
| Engine | Android Emulator 37.3.10.0 (build 16970787) with the stock `emulator.exe` (`7476c5d7cf249182b7310d45e5304778bd5a337e8d6d11890edb27954b0f9717`), the rebuilt QEMU core `fc990b45532c16008327a15e07a15297963716d7dadb9e46ecab4acf3c1cb94f` and the rebuilt gfxstream renderer `7d01cde7d9450e050effb4805c87dcf4c0cfaa274a290ac8f526f192eac085fe`. These are the "published sources" build of the [engine source kit](../../experiments/whpx-clock/README.md#4-build-the-performance-engine), built on the test PC. | BlueStacks' own engine |
| Android image | Official Android 17 Google Play x86-64 image, `google/sdk_gphone64_x86_64/emu64xa:17/CE2A.260420.051.A2/16418371:user/release-keys`, API 37. The package revision was not recorded in these receipts. | Android 9, API 28, `samsung/b0qxxx/b0q:9/TP1A.220624.014/S908EXXS2BWA2:user/release-keys` |
| GPU | `Selecting Vulkan device: NVIDIA GeForce RTX 5070 Ti, Version: 1.4.341` in the engine log. The game reported `Goldfish GFXStream (NVIDIA GeForce RTX 5070 Ti)`. | The player logged `GL_RENDERER = NVIDIA GeForce RTX 5070 Ti/PCIe/SSE2`. The game reported `NVIDIA GeForce RTX 5070 Ti`. |
| Game package | QA build of SuperTuxKart 1.5 (version code 328, x86-64), SHA-256 `dbde91eb1451e5f165cbc09b8d1144d13d35283d5f6752cec52704e3e696ff8e` | The operator's profiling build of that package, SHA-256 `f0ef0810317ec667e93a5d79dbaea67bc7849964463508508b71deee483427de`, which Android lists as debuggable; its receipt names `dbde91eb…` as the base. No profiler ran during these laps. |
| Measurement source | The game's frame report | The game's frame report |

The two players therefore did not run byte-identical game packages. Whether the debuggable flag changes the game's speed on Android 9 was not measured.

**Engine settings in every RustEmuDroid boot.** Environment: `RUSTEMUDROID_QA_WHPX_SYNC_TSC=1`, `RUSTEMUDROID_QA_WHPX_APIC=hv`, `RUSTEMUDROID_QA_LAPIC_FASTPATH=1`, `RUSTEMUDROID_QA_KVM_PV_EOI=1`, `RUSTEMUDROID_QA_VCPU_PLACEMENT=sets-high`, `RUSTEMUDROID_QA_ASG_SPIN_US=3000`, `RUSTEMUDROID_QA_VULKAN_CACHED_HOST=1`, `RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL=2` and the emulator feature `VulkanNativeSwapchain`. AVD keys: `hw.gltransport=asg` and `kernel.parameters=mitigations=off init_on_alloc=0 transparent_hugepage=always`. In the client's Settings of v0.23.0 and later these correspond to all six performance engine switches, including **Reduce Android kernel protections**, which is off by default, plus the hypervisor interrupt controller that engines listing `whpx_apic_hv` add to **CPU virtualization tuning** since v0.24.0.

The engine log of all five completed boots has the line `RustEmuDroid QA: WHPX local APIC emulated by the hypervisor (xAPIC); RUSTEMUDROID_QA_KVM_PV_EOI overridden (off)`, the TSC markers `synchronized init vcpus=4` and `synchronized reset vcpus=4`, the consumer spin of 3000 µs, the `sets-high` vCPU placement and both Vulkan cache lines. Android reported `ro.boot.hardware.gltransport=asg` and `ro.hardware.egl=angle`, and the kernel command line contained the three parameters. Each boot reached Android Home in 21.8–23.0 seconds and waited 20 seconds until QEMU stayed below 0.6 host core before the game was installed. After every boot the test profile's `config.ini` was back to its original SHA-256 (`f5590dd5…6b1f`).

## What failed

- **RustEmuDroid, first Balanced step (16:06).** The operator refused to start after 3.5 seconds: `Expected one normal launcher, SDK emulator and QEMU.` No boot, no lap.
- **RustEmuDroid, second Balanced step (16:24).** The first boot completed with two laps. The second boot was lost: `adb emu avd path` did not answer within 15 seconds while the operator was identifying the new VM. The operator closed that VM and reopened the owner's everyday Android Home. RustEmuDroid's Balanced figure therefore has two laps where four were planned, from one boot.
- **BlueStacks, all four steps.** The timed laps completed and their game reports were kept. Each step then ended with exit status 1 and its receipt says `failed`: closing the player through its exit dialog did not work (`Another HWND covers the Rust client`), and the restore transaction refused to run while the player was open. The operator then ended `hd-player.exe`, stopped its wrapper and ran the restore. The receipts record the frame-rate preferences and the wrapper preferences as restored, the QA package as removed, the stock game and its configuration as preserved, no leftover process, and BlueStacks' configuration file as not byte-identical to the copy taken before the run (`host_config_byte_identity: false`).
- **Panel sample.** Not taken in any RustEmuDroid lap, as described above.

## What this record does not show

- The speed of the **v0.24.1** client, or of any client applying these settings from its own preferences. An earlier series on the same day compared the v0.24.0 client's own path with the operator's and did not show the two to be equal in speed. [Client path with the hypervisor APIC](../../docs/performance-history.md#hypervisor-local-apic)
- Other games, other GPUs, other PCs, longer sessions or snapshots.
- Kemu, which was not measured in this series.
- The stock Android Emulator engine that first-time setup installs and the client uses by default. The performance engine is built separately from the published source kit and is not part of the Windows download.

## Recalculate the results

Game FPS is `1000 × frames / profiled duration in ms` from each lap's report. The means are arithmetic means of the lap values; pooling frames over durations gives the same figures to one decimal. The data file lists every lap with its frame count, duration, start and end time, the SHA-256 of its game report and per-frame profile, the host load sample, and for RustEmuDroid the scope means, the Android sample and the checks of each boot. It also lists the eight steps with their exit status and the load before each. [Data, identities and hashes](windows-rtx-mailbox-hypervisor-apic-bluestacks.json)

The lap files themselves (game reports, per-frame profiles, engine logs) stay on the test PC and are not published with this record. Earlier Mailbox measurements of the same workload: [three players on the v0.22.0 client with the cache engine](windows-rtx-mailbox-peers.md) · [the series of November 25–26](../../docs/performance-history.md#performance-engine-in-uncapped-mailbox).
