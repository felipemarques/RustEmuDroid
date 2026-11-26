# RustEmuDroid, Kemu and BlueStacks performance on Windows

**November 25, 2026.** The first RustEmuDroid prototype delivered **45.86 FPS** in SuperTuxKart's Black Forest replay. By the end of the initial v0.1.0 integration, it reached **59.98 FPS** on the Original preset, alongside Kemu at **59.90**. At High quality and 90 Hz, the Rust frontend later reached **90.02 FPS** with Kemu's Android 16 image and engine. With its own Android 17 Google Play profile, Rust v0.21.1 measured **85.62 FPS** alongside **90.02 for Kemu** and **89.99 for BlueStacks**. [English PDF summary](../output/pdf/windows-performance-2026-11-25.pdf) · [Português brasileiro](performance-history.pt-BR.md)

**November 26, 2026.** In uncapped Vulkan Mailbox on the RTX 5070 Ti, a patched Android Emulator 37.3.10 engine with host and guest settings raised SuperTuxKart on the unmodified Android 17 Google Play image from **164–176 FPS** to **356 FPS** with the Windows Balanced power mode and **375 FPS** with Best performance. Kemu measured **325 and 358** in the same session. BlueStacks measured **376** with Balanced that night; with Best performance, six later daytime laps put it at **427.5**, well ahead of RustEmuDroid. In that daytime condition, a new engine option that lets the Windows hypervisor emulate the local APIC raised RustEmuDroid from 345.2 to **380.1 FPS**, about 12% behind BlueStacks. [Hypervisor local APIC](#hypervisor-local-apic) A test operator set the engine environment around the v0.22.0 client. In a later daytime check with the same engine sources, the v0.23.0 client applying the settings from its own preferences matched the operator path (341.8 against 342.5 FPS). [Client and operator paths](#client-and-operator-paths) [Conditions, recipe and limits](#performance-engine-in-uncapped-mailbox) · [Engine source kit](../experiments/whpx-clock/README.md#4-build-the-performance-engine)

With the same **v0.21.1 Rust window** and a separately rebuilt Android engine, a Windows virtual clock fix improved the game from **85.65 to 87.46 FPS (+2.11%)** in four cold-boot replays. Android presentation gaps over 16.667 ms fell from **5.08% to 3.18%**. The release still ships with its regular runtime setup. [Per-lap results and frame traces](../benchmarks/stk-1.5/windows-whpx-clock-kart.md)

With the v0.22.0 client and the newly linked Windows engine, the same kart replay measured **84.73 game FPS with the cache off and 90.16 with it on**. Android P99 fell from 23.17 to 14.46 ms. The Rust panel processed 86.32 frames/s with the cache on. This pair qualified the build made with FFmpeg and libvpx sources. [Build identity, sampling and traces](../benchmarks/stk-1.5/windows-source-media-cache-rtx.md)

In two Mailbox replays per runtime, the game measured **163.54–176.38 FPS on RustEmuDroid**, **289.67–294.92 on Kemu** and **350.42–372.41 on BlueStacks**. Android SurfaceView stayed near 90 presentations/s on each. Internal render throughput and guest presentation have separate counters. [Matched Mailbox runs](../benchmarks/stk-1.5/windows-rtx-mailbox-peers.md)

With the **official v0.22.0 Windows client and stock Android Emulator 37.3.10**, enabling `VulkanAllocateHostMemory` reduced SuperTuxKart throughput from **85.74 to 71.76 FPS (−16.31%)**. Android P99 rose from **22.91–23.41 ms** to **41.91–42.05 ms**. All four cold boots used the same game, RTX and image. An earlier v0.21.1 series also slowed down with this option. Its separate synthetic memory probe measured faster 32-bit writes, from 91.843 to 0.290 ms per 4 MiB. The cache in the rebuilt engine above uses a different allocation path. [Current client and per-frame measurements](../benchmarks/stk-1.5/windows-stock-vulkan-host-memory-replay-v0220-rtx.md) · [Historical v0.21.1 runs](../benchmarks/stk-1.5/windows-stock-vulkan-host-memory-replay-rtx.md) · [Synthetic probe](../benchmarks/stk-1.5/windows-stock-vulkan-host-memory.md)

The relinked engine decoded video in software and recorded VP9 in a separate media test. Three Goldfish hardware-advertised decoders raised errors in the tested acquisition path. [Decode and recording results](../benchmarks/stk-1.5/windows-source-media-cache-rtx.md#media-smoke-on-the-relinked-engine)

Cipó Livre has a separate interactive test. At 1600 × 900 and 60 Hz on the RTX, RustEmuDroid presented **59.68–59.98 frames/s** and Kemu **59.85–60.04** with the same locally adjusted APK. End-of-run captures showed a 3D render scale of 80% on the RTX. On the integrated Radeon 780M, the two Rust walks reached **55.13 and 57.19 frames/s**, ending at 80% and 70% scale. [Android audio changes, gameplay and frame intervals](../benchmarks/cipo-livre/README.md)

## Performance engine in uncapped Mailbox

All runs on November 25–26 used the same Windows PC (Ryzen 9 8945HS, eight cores, GeForce RTX 5070 Ti), SuperTuxKart 1.5's built-in Black Forest replay at High, Vulkan with game VSync off (Mailbox, uncapped), 1600 × 900 in Android, Android at 90 Hz and four vCPUs. FPS comes from the game's own profiler report: frames over the measured lap. Each value below is the mean of the lap results. Android ran the official Android 17 Google Play x86-64 image, revision 7, unmodified. The Rust window was the **v0.22.0 desktop client** with an 800 × 450 panel. A test operator started it and set the engine environment, AVD keys and power mode itself, one cold boot at a time, with one warm-up lap and two measured laps per boot. The **v0.23.0** Settings preset applies the same values; a separate check below compares the two paths. From step 6 on, each boot waited before installing the game until QEMU used less than 0.6 of a host core.

The engine is Android Emulator 37.3.10 (stock `emulator.exe`) with a rebuilt QEMU core (SHA-256 `a55d48c6…37f5`) and gfxstream renderer (`d92f6060…60d0`). Later builds of the same sources (v0.23.0 kit: QEMU `1c7a05e3…a0c6`; current kit: QEMU `fc990b45…b94f`; both with gfxstream `7d01cde7…85fe`) add prompt writing of the engine's proof lines and options that are off unless enabled. None of them was used for the night measurements. The current kit adds the hypervisor local APIC option described below. [Source kit, gates and build](../experiments/whpx-clock/README.md#4-build-the-performance-engine)

| Windows power mode | RustEmuDroid, performance engine | Kemu 0.7.1 · Android 16 | BlueStacks 5 Pie64 · Android 9 |
|---|---:|---:|---:|
| Balanced | **356** (1 run, 2 boots, 4 laps) | 325 (3 boots, 6 laps) | 376 (1 boot, 2 laps) |
| Best performance | **375** (3 runs, 6 boots, 12 laps) | 358 (1 boot, 2 laps) | 396 (1 boot, 2 laps) |

Later daytime measurements, same PC, GPU and workload, with other desktop programs using about 0.7–0.9 host core:

| Windows power mode, daytime | RustEmuDroid, performance engine | BlueStacks 5 Pie64 · Android 9 |
|---|---:|---:|
| Balanced | **369.7** with the hypervisor APIC (4 boots, 8 laps; 375.6 without one slow lap of 328.5) | 352.5 (1 boot, 2 laps: 347.39 · 357.70); **391.8** in the interleaved session with RustEmuDroid (2 boots, 4 laps); 378.7 over all six laps |
| Best performance | 351.0 (1 series, 4 laps); 342.5 and 341.8 (operator and client paths, 4 laps each); **380.1** with the hypervisor APIC (2 boots, 4 laps; 378–392 in two later boots) | **427.5** (3 boots, 6 laps: 395.33 · 445.74, 435.2 · 438.0, 402.5 · 448.3; warm-up laps not counted) |

Kemu and BlueStacks were measured by the same operator in the same session, between the RustEmuDroid runs, with their own players and windows. The Best performance values of both peers and the Balanced BlueStacks value come from one boot each. At the start of the night the same client and workload measured 164–176 FPS with the stock engine. RustEmuDroid was 9.5% ahead of Kemu with Balanced and 4.7% ahead with Best performance. With Balanced, BlueStacks was 5–6% ahead at night (376 against 356). With Best performance, the night comparison rested on a single BlueStacks boot (laps 364.41 and 427.16) and understated it: six later daytime laps over three boots averaged 427.5, about 14% above RustEmuDroid's night figure of 375 and about 22–25% above the 342–351 RustEmuDroid measured in the same daytime period. During BlueStacks' daytime Balanced laps its player process used about 1.6–1.8 host cores, no thread above 0.35 core, with logical processors at 74–107% of nominal frequency. The Best performance mode raised BlueStacks by about 21% (352.5 → 427.5), against about 5% for RustEmuDroid, whose busy threads already run boosted with Balanced. With the hypervisor APIC described below, RustEmuDroid measured 380.1 FPS in the same daytime condition (378–392 in later boots), and the gap to BlueStacks' 427.5 in that mode narrowed to about 12%. With Balanced, in an interleaved session the same afternoon, BlueStacks was about 6% ahead (391.8 against 369.7); counting all six BlueStacks Balanced laps of the day, the difference is about 2%. RustEmuDroid with the hypervisor APIC measured nearly the same with Balanced (369.7, or 375.6 without the slow lap) as with Best performance (380.1), whereas BlueStacks gained from Best performance (427.5). An idle-desktop measurement with the hypervisor APIC is still missing. The aim of passing Kemu and 300 FPS was met. BlueStacks remains ahead, by about 12% with Best performance.

### Recipe and measured effect of each step

Each step was measured against the preceding configuration within one alternating run (A/B/B/A or similar, same binary). Run-to-run drift of the same configuration was 2–3%, and single laps varied by 5–8%. Steps 1–9 were measured with the Balanced mode unless noted.

| # | Change | How it is enabled | Measured result |
|---|---|---|---|
| 1 | Synchronized guest TSC on WHPX | `RUSTEMUDROID_QA_WHPX_SYNC_TSC=1` (QEMU) | 179.53 → 228.12 FPS with the Vulkan cache on. Other disk-heavy work overlapped this run. Later clean runs of cache + clock measured 245.57 and 248.03. |
| 2 | Cached host-visible Vulkan memory (type 2) | `RUSTEMUDROID_QA_VULKAN_CACHED_HOST=1` (QEMU) | Part of every run; not measured separately in Mailbox |
| 3 | Shared-memory graphics transport with a spinning host consumer | AVD `hw.gltransport=asg` + `RUSTEMUDROID_QA_ASG_SPIN_US` (gfxstream) | ASG alone: 245.57 → 247.27. ASG + 200 µs spin: 244.02 → 260.45 |
| 4 | Vulkan composition with ANGLE in the guest | `-feature VulkanNativeSwapchain` (stock feature) | 253.63 → 269.05. This arm ran last in its run. |
| 5 | Cached memory behind mappable device-local Vulkan allocations | `RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL=2` (gfxstream) | 268.42 → 310.25 (three boots each) |
| 6 | Guest kernel parameters | AVD `kernel.parameters=mitigations=off init_on_alloc=0 transparent_hugepage=always` | 301.54 → 310.08 (+2.8%, near the noise) |
| 7 | vCPUs on dedicated physical cores, never core 0 | `RUSTEMUDROID_QA_VCPU_PLACEMENT=sets-high` (QEMU) | `sets` (cores 0–3): 304.46 → 341.84. `sets-high` (cores 4–7) instead of `sets`: 314.0 → 345.7 |
| 8 | Fewer VM exits | `RUSTEMUDROID_QA_LAPIC_FASTPATH=1`, `RUSTEMUDROID_QA_KVM_PV_EOI=1` (QEMU) | 327.51 → 339.83 and 332.57 → 341.31, both within the noise. LAPIC handler time fell from 86 to 43 ms/s. |
| 9 | Longer consumer spin | `RUSTEMUDROID_QA_ASG_SPIN_US=3000` instead of 200, with Best performance | 373.7 → 384.6 (+2.9%) |
| 10 | Windows Best performance power mode while Android runs | Power mode overlay `ded574b5-45a0-4f42-8737-46345c09c238` | With `sets-high`: 345.7 → 362.7. Final recipe: 355.5 → 359.6 including one 300.90 lap, 379.2 without it |

With `sets` the result depended on the vCPU that ran the game's main thread: 330–375 FPS on vCPUs 2–3, 270–305 FPS on the vCPU bound to host core 0. `sets-high` removed that split. During the measured laps QEMU used about 1.8–2.0 host cores with a 200 µs spin and 2.7 with 3000 µs. The busiest vCPU, which ran the game's main thread, stayed near one full core.

**Tested and discarded:** Vulkan descriptor batching (`VulkanBatchedDescriptorSetUpdate`, 379.8 → 355.7); waiting in the OpenGL ES pipe while holding the global lock plus a render-thread spin (227.12 → 172.2, one boot each); `idle=poll` in the guest (319.74 → 293.66); HLT polling (339.83 → 291.50); `ideal` vCPU placement (320.94 → 305.05); 5000 µs and 1000 µs consumer spins; larger ASG buffers (260.45 → 227.48); disabling the file-backed guest RAM (304.19 → 294.57); unlocking the scaled screenshot readback, the lockless pre-run check and above-normal vCPU priority (all within the noise); the stock `VulkanAllocateHostMemory` option (−16% in an earlier FIFO series); `isolcpus=0` in the guest kernel (332 FPS against 337 for the control in a daytime series, no gain); raising the engine process to ABOVE_NORMAL priority under daytime background load (344.5 against 351.0 for the control, no gain). Guest RAM in host large pages was not measured: the account lacks the Lock pages in memory privilege, and a guest memory probe measured slower random memory access on BlueStacks than in this guest, which removed the motivation.

**Remaining gap.** BlueStacks was 5–6% ahead with Balanced at night with the user-mode APIC, and about 6% (interleaved session) or 2% (all of its Balanced laps that day) ahead of RustEmuDroid with the hypervisor APIC during the day. With Best performance during the day it is about 12% ahead of RustEmuDroid with the hypervisor APIC, and was 22–25% ahead without it; the cause of most of the remaining difference is analysed under [Where BlueStacks is faster](#where-bluestacks-is-faster). In the night scope comparison with Best performance, the main loop took 2.633 ms per frame against 2.527 ms on BlueStacks; that BlueStacks value comes from the single night boot, which later measurements show was slow. The whole graphics update was nearly equal (2.393 vs 2.346 ms). BlueStacks spent less time in `drawAll` and game logic, and more in `endScene`. A likely reason for the different gain from the power mode, not measured separately: no BlueStacks host thread was near a full core, so Windows kept its processors at a lower clock with Balanced, while the RustEmuDroid thread that runs the game already used a full core and ran boosted.

**Sensitivity to background load.** On November 26 during the day, with other desktop programs using roughly 0.7 to 1.2 host cores, the same recipe and binaries measured 309 to 350 FPS per lap, pooling 313 and 337 in two series, instead of about 375 with Best performance. The laps followed the background load: about 1.0 core gave 309–331 FPS and about 0.75 core 343–350. The night figures above were taken on an otherwise idle desktop. BlueStacks was measured later in the day with about 0.8 core of background load (table above); Kemu was not measured under load.

### Hypervisor local APIC

Engine option R, `RUSTEMUDROID_QA_WHPX_APIC=hv`, has the Windows hypervisor emulate each vCPU's local APIC (xAPIC) instead of QEMU's user-mode APIC. All runs below were on November 26 during the day with Best performance and other desktop programs using about 0.8 host core, on the published-source engine with R added (QEMU `fc990b45…b94f`, gfxstream `7d01cde7…85fe`). One interleaved series n, h, hp, hp, h, n used the same binary in every arm:

| Arm | Laps (FPS) | Mean |
|---|---|---:|
| n · recipe, user-mode APIC | 362.12 · 360.53 · 351.69 · 306.45 | 345.2 |
| h · recipe + hypervisor APIC | 386.23 · 382.57 · 373.55 · 377.90 | **380.1** |
| hp · h + `RUSTEMUDROID_QA_ASG_SPIN_RENDERING=1` | 377.90 · 372.30 · 387.86 · 375.94 | 378.5 |

Two further boots with the hypervisor APIC the same afternoon measured 378.3 · 390.6 (with exit statistics on) and 378.5 · 392.4. No lap in these three series fell below 372 FPS.

**Balanced.** On November 26 between 14:06 and 14:31 (daytime, other desktop programs using about 0.7–0.9 host core, Windows Balanced, no power overlay), one interleaved session ran RustEmuDroid for two boots, BlueStacks for one boot, RustEmuDroid for two boots and BlueStacks for one boot, with two timed laps per boot. RustEmuDroid used engine build R with the hypervisor APIC and the operator-applied recipe.

| Player | Laps (FPS) | Mean |
|---|---|---:|
| RustEmuDroid, hypervisor APIC | 370.87 · 386.39, 366.02 · 381.38, 380.93 · 328.54, 369.13 · 374.46 | 369.7 (375.6 without the 328.5 lap) |
| BlueStacks 5 Pie64 | 417.7 · 394.2, 374.0 · 381.3 (warm-up laps 405.0 and 397.5 not counted) | 391.8 |

BlueStacks' earlier Balanced boot the same day (11:20) measured 347.39 · 357.70, so its three Balanced boots that day ranged from 352.5 to 406.0 per boot (378.7 over six laps). Its processor clock did not change with it: its logical processors averaged 86–94% of nominal frequency in both the slowest and the fastest boot.

VM exits that reached QEMU during the race fell from about 20,000–21,000/s to 5,502/s. Before, local-APIC MMIO accounted for about 8,500/s, vCPU kicks about 3,000/s, HLT 800–2,100/s and interrupt windows about 450/s. With the hypervisor APIC these were 0, 4, 0 and 0. The remaining exits were transport pings (3,130/s), other device MMIO (1,734/s), port I/O (499/s) and APIC end-of-interrupt notifications for the I/O APIC (89/s). `ASG_SPIN_RENDERING` alone had cut the transport pings from 2,364/s to 315/s in earlier series without a measurable FPS change (350.8 vs 343.1 and 320.4 vs 310.2).

**Client path with the hypervisor APIC.** On November 26 between 13:49 and 14:03 (daytime, other desktop programs using about 0.7–0.8 host core, Best performance), one interleaved series o, c, c, o with two timed laps per boot used the same engine build R and the v0.24.0 client executable as built for release in both arms. In arm o the test operator applied the settings, including `RUSTEMUDROID_QA_WHPX_APIC=hv`. In arm c the client applied them from its own preferences: the performance engine with the default options plus kernel parameters and the client's power-mode option, with an engine manifest listing `whpx_apic_hv`.

| Arm | Laps (FPS) | Mean |
|---|---|---:|
| o · operator applies the settings | 381.36 · 376.74 · 376.22 · 382.13 | 379.1 |
| c · v0.24.0 client applies the settings | 360.53 · 374.41 · 366.65 · 287.53 | 347.3 (367.2 over the first three laps) |

The engine log and the running processes showed the same options active in all four boots, including the hypervisor APIC proof line. The client's own processor use was the same in both arms (0.23–0.29 core), and the background load was similar for every lap (0.68–0.83 core). The 287.5 lap is one of the occasional slow laps this workload shows, about one in thirty laps measured with the hypervisor APIC that day; no cause was identified in this series. With four laps per arm, this series does not show the two paths to be equal in speed: the client arm was about 3% lower on its three ordinary laps, within that day's lap-to-lap spread, and it had the slow lap. Separately, from a normal (non-elevated) desktop session on a fresh profile, the v0.24.0 client's startup status listed "interrupt controller emulated by Windows" among the confirmed options, boot confirmation took 18 s, and turning the engine off restored `config.ini` to its original bytes.

### Where BlueStacks is faster

The game's own profiler splits the frame. In milliseconds per frame, BlueStacks (daytime, Best performance) against RustEmuDroid before the hypervisor APIC (daytime and night series):

| Scope | BlueStacks | RustEmuDroid |
|---|---:|---:|
| Main loop | 2.264 | 2.574–2.722 |
| Scene traversal and drawing (`drawAll`) | 1.297 | 1.855–1.952 |
| Game logic (`Update race`, exclusive) | 0.093 | 0.143–0.149 |
| Frame presentation (`VK endScene`) | 0.673 | 0.324–0.350 |

RustEmuDroid is faster in presentation; BlueStacks is faster in the game's own code, including logic that makes no graphics call. With the hypervisor APIC, `drawAll` measured 1.93 ms.

A probe ran inside both guests with the same static and dynamically linked binaries, pinned to one CPU, median of three runs. Pure computation was identical: a dependent add chain took 0.212 ns in both (4.7 GHz implied), and integer multiply, floating point, matrix-vector work, virtual calls in random and sorted order and data-dependent branches were all within 0.97–1.03. `clock_gettime` took 17.1 ns in RustEmuDroid against 26.3 ns, and `memcpy` of 1–16 KiB about 0.55 of BlueStacks' time. Through the platform allocator the result reverses:

| Probe (ns) | RustEmuDroid · Android 17 | BlueStacks · Android 9 | Ratio |
|---|---:|---:|---:|
| Small vector and string churn | 171.5 | 129.2 | 1.33 |
| Heap string | 19.4 | 14.6 | 1.33 |
| Traversal of a 20,000-node scene-like tree allocated with the platform `new`, per node | 73.5 | 51.9 | 1.42 |
| The same objects carved from one contiguous block, per node | 35.1 | 28.8 | 1.22 |
| Contiguous block with huge pages requested, per node | 33.5 | 34.9 | 0.96 |

Sixteen consecutive `malloc(368)` calls returned addresses in shuffled order in the Android 17 guest, which uses Scudo (deltas such as −896, +6720, +8512 and −18816 bytes), and strictly 384 bytes apart in BlueStacks' Android 9 guest, which uses jemalloc. In the tree, 0.7% of nodes landed right after the previous one in RustEmuDroid against 18.5% in BlueStacks. After the hypervisor APIC, the remaining gap in this workload follows the Android image's allocator rather than the engine: SuperTuxKart walks thousands of small heap objects per frame. Games with their own allocators may behave differently; that has not been measured.

The log files that BlueStacks writes on this PC show that it also runs on the Windows Hypervisor Platform API, with the local APIC emulated by the hypervisor, a KVM-style paravirtual clock, and without large pages. Over a 188-second run including boot, its log counted about 2,640 user-mode exits per second, none of them local-APIC, HLT or interrupt-window exits.

### Client and operator paths

On November 26 between 10:59 and 11:13, the same PC, RTX 5070 Ti, image and SuperTuxKart workload ran one interleaved series o, c, c, o with two timed laps per boot and the Best performance power mode. Both arms used a pre-release build of the v0.23.0 client as the Rust window (the same sources, compiled before the version number was raised from 0.22.0) and the v0.23.0 kit engine build (QEMU `1c7a05e3…a0c6`, gfxstream `7d01cde7…85fe`, new gates off). In arm o the test operator applied the settings: environment, feature flag, AVD keys and power mode. In arm c the client applied them from its preferences: the performance engine selected for the profile with the default options plus kernel parameters, and the client's own power-mode option. The operator set nothing in arm c.

| Arm | Laps (FPS) | Mean |
|---|---|---:|
| o · operator applies the settings | 345.18 · 335.13 · 354.77 · 334.88 | 342.5 |
| c · v0.23.0 client applies the settings | 310.55 · 359.95 · 346.21 · 350.43 | 341.8 |

The engine log and the running processes showed the same options active in all four boots. The client passes `-feature VulkanNativeSwapchain` as an argument where the operator used the equivalent environment variable. After the series the test profile's `config.ini` was back to its original bytes. Other desktop programs were running, so the level of about 342 FPS reflects that daytime condition, described under background load above, not the idle-desktop figures. The series shows that the client path reproduces the operator path under the same conditions. Separately, from a normal (non-elevated) session on a fresh profile with this engine build, the client's startup status confirmed all seven default options, and boot confirmation was not delayed (18 s).

### Cipó Livre on the integrated Radeon 780M

The same recipe was also run with Cipó Livre (Godot, Vulkan) on the integrated Radeon 780M, with the same Android 17 Google Play image, Android at 60 Hz and the Windows Balanced mode. The Rust window was a local build of the v0.22.0 client. Six cold boots alternated in the order s, r, u, u, r, s, with one 20-second sample per scene and boot. The metric counts distinct SurfaceFlinger presentation timestamps of the game surface. The published configuration (s) used the v0.22.0 cache engine with the goldfish pipe, OpenGL composition and the AMD integrated-GPU option. The recipe (r) used the measured performance engine. The third arm (u) added the AMD option (`-VulkanAllocateDeviceMemoryOnly`) to the recipe.

| Configuration | Scene | Presentations/s | Intervals > 1.5 periods | Mean P99 |
|---|---|---:|---:|---:|
| Published (s) | Forest | 58.61 | 4.86% | 31.2 ms |
| Published (s) | Walking | 57.38 | 9.15% | 33.8 ms |
| Recipe (r) | Forest | 59.93 | 0.17% | 19.7 ms |
| Recipe (r) | Walking | 59.89 | 0.08% | 19.4 ms |
| Recipe + AMD option (u) | Forest | 60.01 | 0.00% | 19.2 ms |
| Recipe + AMD option (u) | Walking | 59.99 | 0.08% | 20.6 ms |

All six boots rendered the game correctly, and the AMD option worked together with `VulkanNativeSwapchain`. Each value combines two 20-second samples from two boots. The 60 Hz display caps the rate, and the game adjusts its render resolution on its own, so these figures show smoother delivery rather than more rendering work. [Earlier Cipó Livre tests](../benchmarks/cipo-livre/README.md)

The hypervisor local APIC was checked on the same integrated GPU and second game on November 26 between 13:29 and 13:39 (daytime, Windows Balanced, Radeon 780M at 60 Hz). Both arms used the engine build with gate R and the AMD integrated-GPU option, in the order u, h, h, u, with one 20-second sample per scene and boot and the same metric as above:

| APIC | Scene | Presentations/s | Intervals > 1.5 periods | Mean P99 |
|---|---|---:|---:|---:|
| User mode (u) | Forest | 60.00 | 0.08% | 19.90 ms |
| User mode (u) | Walking | 59.87 | 0.21% | 20.33 ms |
| Hypervisor (h) | Forest | 59.97 | 0.04% | 18.54 ms |
| Hypervisor (h) | Walking | 60.00 | 0.00% | 18.87 ms |

All four boots were valid and the game rendered correctly. Both arms stayed at the 60 Hz cap, so this validates the hypervisor APIC on the integrated GPU and a second game rather than measuring a speed-up. Each value combines two 20-second samples from two boots.

**Limits.** SuperTuxKart on the RTX 5070 Ti and Cipó Livre on the Radeon 780M, both on the same PC; PUBG has not been tested with this engine. The engine prints a warning that GuestAngle is unstable for API levels above 35. The 3000 µs spin costs about one extra host core while the game runs. About one lap in twelve was an outlier near 300 FPS with that spin, and background programs lower the result, as described above. With `kvm_pv_eoi`, the guest identifies its hypervisor as KVM. The hypervisor APIC was validated only with cold boots of one image on one AMD PC, in SuperTuxKart on the RTX 5070 Ti and Cipó Livre on the Radeon 780M; snapshots, guest reboots, injected NMIs and x2APIC were not tested. An attempt to profile BlueStacks' guest with root on its lab instance failed because the player reset `bst.feature.rooting` to 0 at start; the instance configuration was restored. In SuperTuxKart Android still presented at the 90 Hz display rate; those figures measure how fast the game renders. Longer sessions and snapshots remain untested with this engine.

## SuperTuxKart from the first prototype through v0.21.1

With Vulkan caching active in all four RTX runs, synchronizing the virtual clock reduced QEMU CPU use by **18.7%**, from 2,188 to 1,778 ms of CPU time per second. Pooled game rates were **90.17 FPS with the regular clock** and **90.02 with synchronization**. The Android P99 ranges overlapped. Both configurations approached the 90 Hz FIFO ceiling. [Combined cache and clock experiment](../benchmarks/stk-1.5/windows-cache-tsc-kart-rtx.md)

A second Windows engine experiment brought the Android 17 game to **90.19 FPS** in both enabled runs. With cached Vulkan allocations disabled, the same engine delivered **85.56 FPS** across the two controls. Android presented **89.98 frames/s** with the cache enabled. Across the measured windows it went from **205 of 3,396** intervals longer than 16.667 ms to **0 of 3,601**. The virtual clock fix stayed off. The stock BlueStacks runs still had a lower Android P99 at 11.11 ms, compared with 14.53–14.63 ms on the experimental Rust setup. [Four replays, frame traces and conditions](../benchmarks/stk-1.5/windows-cached-memory-kart.md)

One Windows PC with a Ryzen 9 8945HS and GeForce RTX 5070 Ti ran SuperTuxKart 1.5's built-in Black Forest replay at a guest resolution of 1600 × 900. The table reports **game FPS** from the game's frame count and full replay duration.

| RTX 5070 Ti test | RustEmuDroid | Kemu 0.7.1 | BlueStacks 5.22 Pie64 | Replays per player |
|---|---:|---:|---:|---|
| Nov 23 · First prototype v0.1.0, Original, OpenGL ES, target 60 | 45.86 | 59.69 | No completed run | 1 |
| Nov 23 · Integrated v0.1.0 frontend, Kemu engine, Original, OpenGL ES, target 60 | 59.98 | 59.90 | — | 3 |
| Nov 23 · First three-player series, Original, OpenGL ES, target 60 | 59.93 | 59.98 | 60.02 | 3 |
| Nov 23 · Same series, **High**, OpenGL ES, target 60 | 28.90 | 28.68 | 59.99 | 3 |
| Nov 25 · v0.18.1, High, Vulkan FIFO, Android at 90 Hz, identical x86-64 APK | 82.85 | 90.02 | 89.99 | 2 |

With a 60 FPS target on Original, the integrated prototype delivered **14.12 more FPS (+30.8%)** than the first one. The integration also changed the engine and OpenGL path. In the first High comparison, Rust and Kemu ran Android 16/x86-64 through ANGLE. BlueStacks ran Android 9/32-bit ARM. In the later 90 Hz comparison, all three ran the same x86-64 APK with 21 matching graphics settings. The players still used different Android versions and graphics stacks. [Initial tests and per-run data](../benchmarks/stk-1.5/README.md) · [Six 90 Hz replays](../benchmarks/stk-1.5/vulkan-end-scene.md#windows-results-at-90-hz)

On Nov 23 the Rust Android 17 profile on High scored **14.00 FPS** with the default OpenGL ES translator, **25.36** with ANGLE and **56.15** with direct Vulkan. Measurements around 25–29 FPS in those early High configurations give context to the user's 24–30 FPS observation. By v0.2.4 through v0.7.0, the High/Vulkan replay delivered **59.94–60.03 FPS** at the 60 target. [Renderer comparison](../benchmarks/stk-1.5/README.md#home-do-android-e-imagem-google-play) · [Release comparison](game-testing.md#supertuxkart-release-comparison)

## Three-player 90 Hz baseline with v0.21.1

| Player and Android | Game FPS | Android presentations/s | P99 per run (ms) | Game area in the Windows window |
|---|---:|---:|---:|---:|
| RustEmuDroid v0.21.1 · Android 17 | **85.62** | 85.24 | 23.51 / 23.24 | 800 × 450 px |
| Kemu Qt 0.7.1 · Android 16 | **90.02** | 89.97 | 13.56 / 13.34 | 1280 × 720 px |
| BlueStacks 5.22.166.1003 · Android 9 | **89.99** | 89.98 | 11.11 / 11.11 | 1600 × 900 px |

Rust with Android 17 finished **4.37 game FPS (4.85%) below BlueStacks**. It delivered **2.77 more FPS** than the v0.18.1 measurement of 82.85. The v0.21.1 client fitted its video to 800 × 450, while the v0.18.1 test streamed 1600 × 900. Android rendered the game at 1600 × 900 in both. The Rust panel processed **82.69 frames/s** in the new test.

Each player completed two fresh replays, one player at a time, with four vCPUs, the same x86-64 APK, 21 matching High graphics fields, Vulkan FIFO and a verified 90 Hz Android display. BlueStacks used its official player and a clone of the Pie64 instance. Host viewport sizes, warm-up and Android sampling offsets differed among players. Small differences between two-run averages give little basis for ranking players. [Six replays, conditions and data](../benchmarks/stk-1.5/windows-current-three-players.md)

**Game FPS** covers the full replay. **Android presentations/s** counts new frames on the foreground game surface in a separate window of roughly 20 seconds. **P99 interval** is the time within which 99% of the observed Android presentation intervals fell. At 90 Hz, 22 ms is close to two refresh periods. Physical monitor scanout requires separate measurement.

Pooling the two Android samples per player, **192 of 3,411** intervals on Rust, **0 of 3,600** on Kemu and **1 of 3,601** on BlueStacks exceeded 1.5 periods at 90 Hz (16.667 ms before rounding). On the separate game timeline, elapsed time in `drawAll() for kart 0` was **7.70 and 8.08 ms longer** on average in the slow game loops of the two Rust replays. That clock includes waits and descheduling. The Android samples and game loops cover different time windows. [Counts, timing scopes and traces for all six replays](../benchmarks/stk-1.5/windows-current-three-players.md#where-the-longer-game-frames-spent-time)

## What accounts for the kart results?

In a separate Black Forest diagnostic on Android 17 and the RTX 5070 Ti, object records accounted for **96.295%** of the measured CPU-side Vulkan write bytes across 3,419 frames. Uniform data and indirect commands made up **2.688%**, averaging **5,769 bytes per frame**. These proportions measure write volume. The time spent on each buffer remains open, and the diagnostic runs are excluded from the FPS comparison. [Per-frame distribution, traces and patch](../benchmarks/stk-1.5/windows-vulkan-draw-bytes.md)

Using the same Rust window, RTX and APK, the kart's render thread spent an average of **3.47 ms per frame in Vulkan draw-call generation in both SDK/Android 17 runs**, versus **0.55 and 0.54 ms on Kemu/Android 16**. The difference was about **2.9 ms per frame** in this phase. On Android 17 the averages reached **10.01 and 9.66 ms** across the 148 and 144 long frames, respectively. On Kemu just 3 and 1 frames crossed the same threshold. With detailed timing off and on, pooled game FPS was **85.33 and 85.02** on the SDK and **90.02 and 90.03** on Kemu. All eight game reports were complete. For the last SDK run the Windows preview lost focus before the separate Android presentation sample. [Per-replay and per-phase measurements](../benchmarks/stk-1.5/windows-draw-all-android17.md)

- **Rust frontend.** RustEmuDroid v0.20.0 and Kemu Qt on the same Android 16 image, QEMU/gfxstream engine, APK and RTX both delivered **90.02 game FPS**. Android presented **89.98** and **90.07 frames/s**, respectively. The Rust panel processed **84.07 frames/s**, leaving room to improve delivery to the window even when the game reached 90. The separate GPUI prototype showed 86.00 composed image changes/s versus 84.14 with Glow on Windows. The difference was smaller than the spread between GPUI passes. [Same engine with both frontends](../benchmarks/stk-1.5/vulkan-end-scene.md#how-the-kemu-and-rust-windows-receive-game-frames) · [GPUI experiment](../experiments/results/windows-viewers-stk-dxgi.md)
- **GPU.** Driving both engines from the same v0.21.1 Rust window on the RTX, the instrumented Vulkan queue interval had a P95 of **0.7065 ms** on SDK Android 17 and **0.6997 ms** on Kemu Android 16. The corresponding Android presentation intervals had a P99 of **22.97 and 13.18 ms**. Work outside the GPU timestamps includes image acquisition, synchronization and presentation. These results support investigating those stages and the virtual clock before investing in a GPU driver. [GPU and presentation measurements](../benchmarks/stk-1.5/windows-vulkan-gpu-timestamps.md)
- **Image and virtual clock.** With the SDK engine and Rust window held constant, Android 16 Google APIs delivered **83.67 FPS** and Android 17 Google Play delivered **85.61**. The Android 17 kernel detected mismatched virtual CPU counters and selected **HPET**, while Kemu retained **TSC**. After boot, a separate idle probe estimated a persistent TSC spread of **1.877 s** across the SDK guest's four vCPUs. Independent bounds based on reversing the read order put Kemu's offsets relative to vCPU 0 within **0.26 ms**. After the replay, the same `gettimeofday()` read cost **8.44 µs** on the SDK and **0.018 µs** on Kemu. The subsequent clock-only replay measured a 2.11% increase in game FPS in the Rust window. [Official Android images on the same engine](../benchmarks/stk-1.5/windows-sdk-android16-vs17.md) · [Boot logs](../benchmarks/stk-1.5/windows-guest-clocksource.md) · [Clock alignment](../benchmarks/stk-1.5/windows-guest-clock-alignment.md) · [Clock microbenchmark](../benchmarks/stk-1.5/windows-guest-clock-cost.md)
- **Cost of the game profiler.** In a separate A/B/B/A series, both engines used the same released Rust v0.21.1 window at 800 × 450 px, with Black Forest High Vulkan FIFO at 1600 × 900 and 90 Hz in Android. With the same QA APK, switching from detailed CPU scopes to frame totals only changed pooled game FPS from **85.38 to 85.93** on SDK Android 17 and **90.02 to 90.01** on Kemu Android 16. The SDK gain of 0.55 FPS fell short of the planned materiality check. Kemu still led by **4.08 FPS** with frame totals only. Game FPS covers the whole replay, and the Android SurfaceView measure uses a separate 20-second sample. The official three-player series above includes BlueStacks and the original Kemu window. [Eight replays and test criteria](../benchmarks/stk-1.5/windows-profiler-overhead.md)
- **CPU.** On v0.21.1, an experimental Vulkan composition and ANGLE setting cut QEMU CPU time per Android presentation by **14.03%**. Game FPS changed from **85.61 to 85.77**. Lower CPU use was the measurable benefit under this workload. [Four replays on the same Android 17 image](../benchmarks/stk-1.5/windows-android17-vulkan-composition.md)

An experimental Windows QEMU build now boots the same Google Play image with the virtual CPU counters synchronized. In a matched off/on pair, Linux switched from HPET to TSC, the independent cross-CPU offset bound fell from 2.258 seconds to 0.252 milliseconds, and a libc time read fell from 9,114 to 18 ns of thread CPU time. Four cold-boot replays in the engine's Qt window then measured **85.45 game FPS disabled and 87.38 enabled (+2.26%)**. Long Android presentation intervals fell from 5.32% to 3.41%. The three-player averages use the published v0.21.1. [Cold boot validation](../benchmarks/stk-1.5/whpx-tsc-sync.md#android-17-cold-boots-on-the-same-executable) · [Experimental game results](../benchmarks/stk-1.5/windows-whpx-clock-kart.md)

## Integrated Radeon 780M, PUBG and memory

| Radeon 780M · High Vulkan, 90 Hz | Game FPS | Android presentations/s | Android P99 by run |
|---|---:|---:|---:|
| RustEmuDroid · Android 17, cached allocations | 90.17 | 89.95 | 12.88 / 13.34 ms |
| Kemu Qt · Android 16 | 90.02 | 90.09 | 12.78 / 13.02 ms |
| BlueStacks · Android 9 Pie64 | 89.72 | 89.79 | 11.11 / 11.11 ms |

All three reached the vicinity of the 90 Hz target with the same game APK and guest rendering settings. Each row combines two full replays. The Rust panel displayed the game in an 800 × 450 area, Kemu in 1280 × 720 and BlueStacks in 1600 × 900. [Per-run measurements, timing windows and raw data](../benchmarks/stk-1.5/windows-current-three-players-radeon780m.md)

At **90 Hz on the integrated Radeon 780M**, the same Android 17 Rust window and experimental QEMU delivered **84.36 game FPS with cached allocations off and 90.17 with them on (+6.89%)**. Android P99 fell from **23.10–23.22 ms to 12.88–13.34 ms**. The Rust panel processed 86.72 frames/s with the option enabled. Windows GPU counters confirmed Radeon activity in all four replays. The v0.22.0 Windows client now supports selecting this engine per profile. [Four cold boots, render settings and raw frame traces](../benchmarks/stk-1.5/windows-cached-memory-radeon780m.md) · [v0.22.0 setup](releases/v0.22.0.md)

| Measurement | RustEmuDroid | Kemu | BlueStacks |
|---|---:|---:|---:|
| Kart, Radeon 780M, High, OpenGL ES, target 60 · game FPS | 55.49 | 59.87 | 59.99 |
| Kart, Radeon 780M, High, Vulkan, target 60 · game FPS | 59.71 | — | — |
| PUBG, RTX, Lobby and Inventory, Balanced/High · Android presentations/s | 25.20 | — | 29.58 |
| PUBG, RTX, Training Grounds camera pans, Smooth/High · Android presentations/s | 29.29 | — | 29.98 |
| Resident memory of the main VM process at Android Home · GiB | 5.73 | 3.04 | 1.67 |

The integrated GPU series includes three replays per configuration. The OpenGL ES tests used Android 16 for Rust and Kemu and Android 9 for BlueStacks. The Vulkan series used Android 17 with 16× anisotropic filtering, compared with 8× in OpenGL ES. The first prototype was originally timed on the RTX. [Radeon 780M run data](../benchmarks/stk-1.5/radeon-780m/README.md)

In PUBG the biggest visible issue is the **first Inventory close**. It paused for **1.60–1.68 s** in Rust v0.18.0 and **0.17 s** in BlueStacks. In v0.21.1, three accepted runs still paused for **1.61–1.65 s**, although the stationary Lobby presented 29.37 frames/s. In Training Grounds, BlueStacks reached 59.89 presentations/s with *Extreme* selected. *High* was the highest option offered to the tested Android 17 profile. A comparable Kemu route is missing. [PUBG head-to-head](game-testing.md#fps-comparisons-on-the-windows-test-pc) · [v0.21.1 retest](../benchmarks/pubg-mobile/v0.21.1-android17-first-close-meminfo.md)

On the isolated Android 16 profile, Kemu Qt 0.7.1 launched PUBG 4.7.0/21718 ARM64 on the RTX 5070 Ti with a 60 Hz display. The process exited with `SIGILL` under the default translator and in both lightweight attempts. Lightweight reached `GameActivity` and briefly displayed the Android splash. The Lobby never appeared. A separate historical run used the local Kemu launcher, build **21783** and the 3D tutorial on Balanced. It recorded **1,812 presentations over 30.19 s (60.02/s)** with a **19.32 ms P99**. The different build, scene, launcher and concurrent BlueStacks session rule out treating that tutorial as a Kemu result for Lobby, Inventory or Training Grounds. [New startup trial and historical measurement method](../benchmarks/pubg-mobile/windows-kemu-qt-21718.md)

All three VMs had 8 GiB of configured RAM. The table shows the median of three Windows working-set readings for the main VM process of each player. The separate Rust UI used another **118 MiB**. The sessions differed in uptime and Home app. All conclusions are scoped to the tested Windows PC, games and builds. [Memory counters and boot ages](../benchmarks/windows-home-memory/README.md)
