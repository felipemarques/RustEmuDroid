# SuperTuxKart Vulkan Mailbox: the v0.26.0 candidate with the engine's surface in the panel, BlueStacks, Kemu and LDPlayer 14, alternated

On November 28, 2026 between 07:15 and 08:37 (local time, UTC−3), one session on the Windows test PC alternated the **v0.26.0 release candidate of the RustEmuDroid client**, the v0.25.0 display path, BlueStacks 5 Pie64, Kemu 0.7.1 and LDPlayer 14. The candidate showed the engine's own render surface in a child window of the panel, the Windows default of v0.26.0, at a 1600 × 900 panel and with the window maximised. Only the Windows Best performance power mode was measured.

| Player and condition | Laps (game FPS) | Mean |
|---|---|---:|
| RustEmuDroid v0.26.0 candidate, 1600 × 900 panel | 434.30 · 449.84, 445.74 · 453.50, 439.23 · 439.71 (3 boots, 6 laps) | **443.7** |
| RustEmuDroid v0.26.0 candidate, window maximised | 433.26 · 407.13, 391.37 · 452.10, 452.39 · 456.30 (3 boots, 6 laps) | **432.1** |
| BlueStacks 5 Pie64 · Android 9, window as the player opens it | 345.27 · 384.42 (1 boot) | 364.9 |
| BlueStacks 5 Pie64, window maximised | 328.38 · 331.70 (1 boot) | 330.0 |
| Kemu 0.7.1 · Android 16, window as the player opens it | 326.67 · 335.53 (1 boot) | 331.1 |
| Kemu 0.7.1, maximise requested but not applied | 336.79 · 322.33 (1 boot) | 329.6 |
| LDPlayer 14 · Android 14, window as the player opens it | 301.50 · 303.94, 303.72 · 299.29 (2 boots) | 302.1 |
| RustEmuDroid, the v0.25.0 path (frames copied through shared memory), 1600 × 900 panel | 313.25 · 306.05, 308.26 · 292.89 (2 boots) | 305.1 |

Each mean is the mean of the lap values.

**What this series shows.** On this PC and in this replay, the v0.26.0 candidate measured more than BlueStacks, Kemu and LDPlayer 14 in both window sizes: 21.6% more than BlueStacks, 34.0% more than Kemu and 46.9% more than LDPlayer 14 at 1600 × 900, and 30.9% more than BlueStacks with both windows maximised. Its slowest lap (391.37, maximised; 434.30 at 1600 × 900) is above the fastest lap of any other player or path in the session (BlueStacks, 384.42). Against the v0.25.0 path at the same panel size, in the same session, it measured 45.4% more.

**What it does not show.** It is one game, one replay and one PC. BlueStacks and Kemu have one boot per condition here, because the third cycle of the session could not run them (below). BlueStacks' figures vary between sessions on this PC: 419.3 on November 26, 361.0 on November 27, and 392.0 and 364.9 in the two sessions of November 28. An earlier session the same night, [`final1`](#the-earlier-session-of-november-28-final1), with a pre-release build of the same feature, points the same way with smaller margins and overlapping laps. The candidate was timed before publication, and the published v0.26.0 download is a separate build of the same branch by the release workflow. [Identities](#identities)

## Workload and conditions

Every player ran SuperTuxKart 1.5's built-in Black Forest replay at High with Vulkan, 1600 × 900 and 100% render scale in Android, four vCPUs in RustEmuDroid, BlueStacks and LDPlayer (Kemu's count was not recorded), one guest at a time on the GeForce RTX 5070 Ti of the test PC (Ryzen 9 8945HS). On this PC the RTX 5070 Ti is an external GPU connected over Thunderbolt 3. The game's VSync setting was off and `max_fps=90` was written, which the built-in benchmark does not apply. Every replay of every player logged `VkPresentModeKHR: 1` (Mailbox). All 21 graphics fields were the same in every lap of both sessions and are listed in the data file. Game FPS is the frame count of the game's own frame report divided by its profiled duration, about 38.1 seconds per lap. Each step is one cold boot with one warm-up lap, which is not counted, and two timed laps.

Android's display ran at 90 Hz in RustEmuDroid, BlueStacks and Kemu, and at 240 Hz in LDPlayer, as LDPlayer sets it (`basicSettings.fps=240`, its VSync off). With the game's VSync off and Mailbox, the game renders as fast as it can in all of them.

**Order.** The session driver held the benchmark lock for the whole session and ran three cycles, players alternated, with nothing else under test on the PC:

| Cycle | Steps |
|---|---|
| 1 | BlueStacks, Kemu, candidate 1600 × 900, LDPlayer 14, candidate maximised, v0.25.0 path |
| 2 | BlueStacks maximised, Kemu maximise requested, v0.25.0 path, candidate maximised, LDPlayer 14, candidate 1600 × 900 |
| 3 | Kemu maximise requested, BlueStacks maximised, Kemu, BlueStacks (all four refused at start), candidate 1600 × 900, candidate maximised |

**The owner's everyday Android VM.** The BlueStacks and Kemu operators expect to find the owner's everyday RustEmuDroid VM running, close it for their timed boot and reopen it afterwards. So before the BlueStacks and Kemu steps of each cycle the session opened that VM and rested 120 seconds, and before the RustEmuDroid and LDPlayer steps it closed it again. In the third cycle that VM did not finish booting within 301.6 seconds (the session record says `the owner's emulator did not finish booting`), and the four BlueStacks and Kemu steps of that cycle refused to start, each in 1.4 seconds, with `Expected one normal launcher, SDK emulator and QEMU.` Nothing of theirs was timed. The session's closing step then found no everyday VM to close (`expected one habitual client, found 0`), and the cycle's two RustEmuDroid steps ran as in the other cycles.

**The owner's LDPlayer 14 instance.** The owner's own LDPlayer 14 instance stayed open in the background through every step of the session, for every player. Its VM process, `Ld9BoxHeadless.exe`, used 0.05–0.14 core in the samples. LDPlayer's timed boots used a separate instance, `rustemudroid-bench`.

| Background load | Host cores |
|---|---:|
| Whole machine before each BlueStacks and Kemu step, including the everyday Android VM (0.31–0.38 core) | 1.25 · 1.16 · 1.14 · 1.38 |
| Whole machine before each RustEmuDroid and LDPlayer step | 0.53–0.89 |
| Other programs during the candidate's timed laps | 0.97–1.14 |
| Other programs during the v0.25.0 path's timed laps | 1.25–1.38 |
| Other programs during BlueStacks' timed laps | 1.30 · 1.05, 1.07 · 1.06 |
| Other programs during LDPlayer's timed laps | 0.95–1.05 |
| Other programs during Kemu's timed laps | not sampled by its operator |

The two kinds of sample are not taken the same way: the RustEmuDroid harness samples the whole 48 seconds of a lap run, and the BlueStacks and LDPlayer operators sample 17–22 seconds inside each lap.

**Power mode.** The overlay before the session was Balanced (`00000000-0000-0000-0000-000000000000`). Every step, of every player, recorded Balanced as the overlay before it, Best performance (`ded574b5-45a0-4f42-8737-46345c09c238`) while it ran and Balanced put back after it. The Windows power plan was not recorded. During BlueStacks' timed laps the first eight logical processors averaged 104.3–106.4% of nominal frequency, during LDPlayer's 104.4–104.8%. The RustEmuDroid harness and the Kemu operator did not sample the processor frequency.

**Window sizes.**

| Player and condition | Window | Picture |
|---|---|---|
| Candidate, 1600 × 900 | 1646 × 1000, sized by the harness | the engine's surface at 1600 × 900 in the panel |
| Candidate, maximised | maximised, 1936 × 1048 (1920 × 1032 on screen) | the engine's surface at 1762 × 992 |
| v0.25.0 path, 1600 × 900 | 1640 × 995, sized by the harness | 1600 × 900 frames copied and drawn by the panel |
| BlueStacks | the session saw its largest window at 1634 × 934 at first and at 1921 × 934 for the rest of the step | Android's display 1600 × 900 before every lap; the size of the game's area on the screen was not recorded |
| BlueStacks, maximised | maximised at 1920 × 1032 at first, then a 2236 × 1032 window not reported as maximised for the rest of the step | as above |
| Kemu, both steps | 1334 × 762 before and after the laps in Kemu's own viewport record | a 1280 × 720 picture of Android's 1600 × 900 display |
| LDPlayer 14 | 1648 × 943 through boot and laps | a 1600 × 900 render window |

**Kemu was not maximised.** The session's window watcher found no Kemu window in either Kemu step, so its maximise request was never applied, and Kemu's viewport record shows the same window and picture size before and after the laps of both steps. The step labelled "maximised" is therefore a second boot of Kemu at its usual window: 331.1 and 329.6. LDPlayer was not asked to maximise in this session.

## Laps in the order they ran

| Start | Player and condition | Run · lap | Game frames | Game FPS | Other programs (cores) | The player's own processes (cores) | Panel pixels that changed between captures |
|---|---|---|---:|---:|---:|---|---|
| 07:19 | BlueStacks | `stk-blue-profile-dfinal21` · 1 | 13,164 | 345.27 | 1.30 | 1.73 player + 0.31 helper | not captured |
| 07:20 | BlueStacks | `stk-blue-profile-dfinal21` · 2 | 14,656 | 384.42 | 1.05 | 1.24 player + 0.08 helper | not captured |
| 07:24 | Kemu | `stk-mailbox-rtx-kemu-final21` · 1 | 12,456 | 326.67 | not sampled | not sampled | not captured |
| 07:25 | Kemu | `stk-mailbox-rtx-kemu-final21` · 2 | 12,793 | 335.53 | not sampled | not sampled | not captured |
| 07:29 | Candidate, 1600 × 900 | `dfinal2-rc16-1` · 1 | 16,558 | 434.30 | 1.11 | 2.83 client + engine | 81–95% |
| 07:30 | Candidate, 1600 × 900 | `dfinal2-rc16-1` · 2 | 17,151 | 449.84 | 1.04 | 2.84 client + engine | 90–94% |
| 07:34 | LDPlayer 14 | `stk-ld14-dfinal21` · 1 | 11,495 | 301.50 | 1.05 | 1.84 VM + 0.02 player | not captured |
| 07:35 | LDPlayer 14 | `stk-ld14-dfinal21` · 2 | 11,587 | 303.94 | 0.96 | 1.85 VM + 0.02 player | not captured |
| 07:39 | Candidate, maximised | `dfinal2-rcmax-1` · 1 | 16,517 | 433.26 | 0.97 | 2.86 client + engine | 89–93% |
| 07:39 | Candidate, maximised | `dfinal2-rcmax-1` · 2 | 15,523 | 407.13 | 1.14 | 2.86 client + engine | 80–94% |
| 07:43 | v0.25.0 path, 1600 × 900 | `dfinal2-new16-1` · 1 | 11,943 | 313.25 | 1.25 | 3.02 client + engine | 74–95% |
| 07:44 | v0.25.0 path, 1600 × 900 | `dfinal2-new16-1` · 2 | 11,670 | 306.05 | 1.26 | 2.97 client + engine | 91–94% |
| 07:51 | BlueStacks, maximised | `stk-blue-profile-dfinal2m1` · 1 | 12,520 | 328.38 | 1.07 | 1.52 player + 0.30 helper | not captured |
| 07:52 | BlueStacks, maximised | `stk-blue-profile-dfinal2m1` · 2 | 12,646 | 331.70 | 1.06 | 1.32 player + 0.14 helper | not captured |
| 07:55 | Kemu, maximise requested | `stk-mailbox-rtx-kemu-final2m1` · 1 | 12,841 | 336.79 | not sampled | not sampled | not captured |
| 07:56 | Kemu, maximise requested | `stk-mailbox-rtx-kemu-final2m1` · 2 | 12,290 | 322.33 | not sampled | not sampled | not captured |
| 08:00 | v0.25.0 path, 1600 × 900 | `dfinal2-new16-2` · 1 | 11,752 | 308.26 | 1.38 | 3.04 client + engine | 73–95% |
| 08:01 | v0.25.0 path, 1600 × 900 | `dfinal2-new16-2` · 2 | 11,167 | 292.89 | 1.29 | 2.91 client + engine | 81–95% |
| 08:05 | Candidate, maximised | `dfinal2-rcmax-2` · 1 | 14,922 | 391.37 | 1.12 | 2.80 client + engine | 90–94% |
| 08:06 | Candidate, maximised | `dfinal2-rcmax-2` · 2 | 17,236 | 452.10 | 1.09 | 2.82 client + engine | 90–94% |
| 08:10 | LDPlayer 14 | `stk-ld14-dfinal22` · 1 | 11,580 | 303.72 | 0.95 | 1.85 VM + 0.01 player | not captured |
| 08:11 | LDPlayer 14 | `stk-ld14-dfinal22` · 2 | 11,411 | 299.29 | 0.98 | 1.83 VM + 0.02 player | not captured |
| 08:15 | Candidate, 1600 × 900 | `dfinal2-rc16-2` · 1 | 16,996 | 445.74 | 1.00 | 2.83 client + engine | 76–95% |
| 08:15 | Candidate, 1600 × 900 | `dfinal2-rc16-2` · 2 | 17,290 | 453.50 | 1.05 | 2.82 client + engine | 79–94% |
| 08:31 | Candidate, 1600 × 900 | `dfinal2-rc16-3` · 1 | 16,746 | 439.23 | 1.00 | 2.84 client + engine | 90–94% |
| 08:31 | Candidate, 1600 × 900 | `dfinal2-rc16-3` · 2 | 16,764 | 439.71 | 1.04 | 2.80 client + engine | 88–94% |
| 08:35 | Candidate, maximised | `dfinal2-rcmax-3` · 1 | 17,248 | 452.39 | 1.07 | 2.85 client + engine | 88–93% |
| 08:36 | Candidate, maximised | `dfinal2-rcmax-3` · 2 | 17,397 | 456.30 | 1.10 | 2.81 client + engine | 73–94% |

Warm-up laps, not counted: candidate at 1600 × 900, 428.77 · 442.49 · 452.19; maximised, 418.48 · 449.21 · 375.55; v0.25.0 path, 322.41 · 324.88; BlueStacks 380.32, maximised 321.03; Kemu 324.51, second boot 351.28; LDPlayer 302.33 · 313.34.

| Player and condition | Laps | Mean | Lowest | Highest | Sample standard deviation |
|---|---:|---:|---:|---:|---:|
| Candidate, 1600 × 900 | 6 | 443.72 | 434.30 | 453.50 | 7.24 |
| Candidate, maximised | 6 | 432.09 | 391.37 | 456.30 | 27.14 |
| BlueStacks | 2 | 364.84 | 345.27 | 384.42 | 27.69 |
| BlueStacks, maximised | 2 | 330.04 | 328.38 | 331.70 | 2.34 |
| Kemu | 2 | 331.10 | 326.67 | 335.53 | 6.26 |
| Kemu, maximise requested | 2 | 329.56 | 322.33 | 336.79 | 10.22 |
| LDPlayer 14 | 4 | 302.11 | 299.29 | 303.94 | 2.18 |
| v0.25.0 path, 1600 × 900 | 4 | 305.11 | 292.89 | 313.25 | 8.69 |

The candidate at 1600 × 900 was the most even arm of the session: its six laps span 19.2 FPS. Maximised, two of its laps were slower (407.13 and 391.37), both in the first two cycles, and its four other laps measured 433–456. The v0.25.0 path ran here with full-size 1600 × 900 frames, and it measured 305.1 where an earlier build of the shared memory path measured 397.6–401.0 on November 27 with frames reduced to an 800 × 450 or 1040 × 585 panel; this record does not establish what makes full-size copies cost that much on this PC.

### The picture in the panel was live

The harness captured the client window six times during every lap and compared consecutive captures. In every timed lap of the candidate, 73–95% of the panel's pixels changed from one capture to the next, and the harness's measure of a panel filled with one colour stayed at or below 0.8%: the panel showed the moving race. Every candidate step reported the panel line `Panel: the engine's render surface, inside the client's window; the engine presents straight to the screen and no frame is copied.`, and the client processed no frames for drawing during its laps, because the engine presents straight to the screen on this path. The v0.25.0 path processed 83.4–83.8 frames per second for drawing, all of them 1600 × 900. Mailbox replaces a pending image before the next refresh, so game FPS measures how fast the game renders, not how many frames reach the screen. Android presentations were sampled only for Kemu (89.9–90.0 per second); physical display scanout and input latency were not measured.

## The earlier session of November 28 (`final1`)

Between 01:19 and 02:53 the same night, the same driver, PC, workload, engine stage and operators ran a session with a **pre-release build of the child-window panel**, commit `30faa6d`, where the mode had to be asked for (`RUSTEMUDROID_PANEL=engine-child`). The overlays drawn over the picture, the setting, the watchdog and the fixes made after review came later. Best performance was already on before that session and stayed on. Every condition had two boots, and the owner's LDPlayer 14 instance was open as well.

| Player and condition | Laps (game FPS) | Mean |
|---|---|---:|
| RustEmuDroid `30faa6d`, engine surface in a child window, 1600 × 900 panel | 436.12 · 468.07, 386.53 · 381.07 | **417.9** |
| RustEmuDroid `30faa6d`, window maximised | 432.14 · 389.62, 451.55 · 416.37 | **422.4** |
| BlueStacks 5 Pie64 | 407.40 · 412.38, 353.47 · 394.61 | 392.0 |
| BlueStacks 5 Pie64, window maximised | 354.46 · 406.91, 312.65 · 320.03 | 348.5 |
| Kemu 0.7.1 | 347.23 · 325.92, 341.33 · 345.13 | 339.9 |
| Kemu 0.7.1, maximise requested but not applied | 381.37 · 352.65, 334.70 · 349.57 | 354.6 |
| LDPlayer 14 | 299.41 · 297.36, 301.31 · 300.65 | 299.7 |
| LDPlayer 14, window maximised | 309.57 · 302.49, 295.46 · 299.30 | 301.7 |
| RustEmuDroid, the v0.25.0 path, 1600 × 900 panel | 310.86 · 312.35, 303.98 · 294.68 | 305.5 |

In that session the child-window build measured 6.6% more than BlueStacks at 1600 × 900 and 21.2% more maximised, 23.0% more than Kemu and 39.5% more than LDPlayer 14. The laps overlap at 1600 × 900: its second boot (386.53 · 381.07) is below BlueStacks' first (407.40 · 412.38). As in `final2`, the window watcher found no Kemu window and Kemu's viewport record shows its usual 1334 × 762 window in the "maximised" steps. **LDPlayer's maximised steps did maximise:** its operator recorded the window maximised at 1936 × 1048 with a 1786 × 1005 render window through both laps, against 1648 × 943 and 1600 × 900 in its other steps; the session's own watcher agrees (1936 × 1048, maximised, in 39 and 40 samples). All BlueStacks steps of that session ended with exit status 1 after their laps, as below. [Its laps, steps and identities are in the data file](windows-rtx-child-window-panel-peers.json)

## Identities

| Item | RustEmuDroid | BlueStacks | Kemu | LDPlayer 14 |
|---|---|---|---|---|
| Client or player | **v0.26.0 release candidate**, SHA-256 `26559c35bf8fdb970a5e735d04fa18ba971ba5c2bb8cb49515607daf696f6f48`, the same file in all six candidate steps. Built from the branch `release/v0.26.0` for its delivering commit `71b5c41`: the build output in that branch's target folder has the same hash, was written at 07:13, a minute before the commit was recorded, and carries the version string 0.26.0. The receipts hold only the hash. It is not the published v0.26.0 download, which the release workflow builds separately. The v0.25.0 path ran SHA-256 `7406d413fd89b2906bea75774387cbcc41d62a9a1b22d6471e02e62f142f4788`, which the lab notes name as commit `3e42366`: the three feature commits of v0.25.0 on the v0.24.0 source, whose display code is identical to the v0.25.0 release; **not the published v0.25.0 executable**. | BlueStacks 5 official player with the lab instance `Pie64_1`; version 5.22.166.1003, read from the installed player after the sessions (not in the receipts) | Kemu v0.7.1 (`Kemu.exe` `e5052772…978a`, QEMU `9b6dd661…d3ea`) | LDPlayer 14.0.36.0, dedicated instance `rustemudroid-bench` (4 CPUs, 8 GB, 1600 × 900 at 240 dpi, 240 FPS, VSync off) |
| Who applied the settings | A lab harness started the client (`ui --play`, English) on a disposable lab profile with `--sdk` pointing at the engine and set the engine environment and the panel path (`RUSTEMUDROID_PANEL=engine-child` for the candidate, the default of v0.26.0 on Windows). The client's own performance engine setting was not used. Preferences: Host GPU, 90 Hz. | The operator's preference transaction prepares the instance and puts its preferences back afterwards; the operator installed the QA APK | The operator boots Kemu's own runtime, sets Android to 90 Hz and puts the policy back afterwards | The operator backs up the instance's configuration, sets the values above and restores it by hash afterwards |
| Engine | Android Emulator 37.3.10.0 (build 16970787) from the stage folder of the earlier series: stock `emulator.exe` (`7476c5d7cf249182b7310d45e5304778bd5a337e8d6d11890edb27954b0f9717`), rebuilt QEMU core `fc990b45532c16008327a15e07a15297963716d7dadb9e46ecab4acf3c1cb94f` and gfxstream renderer `7d01cde7d9450e050effb4805c87dcf4c0cfaa274a290ac8f526f192eac085fe`, hashed after the sessions and the same as the stage's staging record of November 26: the "published sources" build of the [engine source kit](../../experiments/whpx-clock/README.md#4-build-the-performance-engine) | BlueStacks' own engine | Kemu's own engine (user-mode APIC) | LDPlayer's own engine |
| Android image | Official Android 17 Google Play x86-64 image, package revision 7, `google/sdk_gphone64_x86_64/emu64xa:17/CE2A.260420.051.A2/16418371:user/release-keys` | Android 9, API 28, `samsung/b0qxxx/b0q:9/TP1A.220624.014/S908EXXS2BWA2:user/release-keys` | Android 16, API 36, `Kemu/sdk_phone64_x86_64_digitalis/emu64xa:Baklava/BP4A.251205.006/eng.kulopo:userdebug/test-keys` | Android 14, API 34, `samsung/star2qltezh/star2qltechn:14/UQ1A.240205.11101510/ucsm.20261110.151005:user/release-keys` |
| GPU | `Selecting Vulkan device: NVIDIA GeForce RTX 5070 Ti` in the engine log, driver 610.88. The game reported `Goldfish GFXStream (NVIDIA GeForce RTX 5070 Ti)`. | The player logged `GL_RENDERER = NVIDIA GeForce RTX 5070 Ti/PCIe/SSE2`; the game reported `NVIDIA GeForce RTX 5070 Ti`, driver 610.88.0.0 | Kemu's engine selected the RTX 5070 Ti (driver 610.88); the game reported `Adreno (TM) 830`, the name Kemu presents to Android | The game reported `Adreno (TM) 750`, the name LDPlayer presents to Android; Windows' GPU counters showed LDPlayer's VM process using the RTX 5070 Ti (3D 41–43%, copy 44–45% during the laps) |
| Game package | QA build of SuperTuxKart 1.5 (version code 328, x86-64), SHA-256 `dbde91eb1451e5f165cbc09b8d1144d13d35283d5f6752cec52704e3e696ff8e` | The operator's profiling build of that package, SHA-256 `f0ef0810317ec667e93a5d79dbaea67bc7849964463508508b71deee483427de`, which Android lists as debuggable; base `dbde91eb…`. No profiler was requested. | `dbde91eb…`, the same file as RustEmuDroid | `dbde91eb…`, the same file as RustEmuDroid |
| Measurement source | The game's frame report | The game's frame report | The game's frame report | The game's frame report |

As in the earlier series, BlueStacks did not run a byte-identical game package. Whether the debuggable flag changes the game's speed on Android 9 was not measured.

**Engine settings in every RustEmuDroid boot.** Environment set by the harness: `RUSTEMUDROID_QA_WHPX_SYNC_TSC=1`, `RUSTEMUDROID_QA_WHPX_APIC=hv`, `RUSTEMUDROID_QA_VULKAN_CACHED_HOST=1`, `RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL=2`, `RUSTEMUDROID_QA_ASG_SPIN_US=3000`, `RUSTEMUDROID_QA_VCPU_PLACEMENT=sets-high` and `ANDROID_EMULATOR_FEATURES=VulkanNativeSwapchain,-QuickbootFileBacked`, as on November 27. The lab profile's `config.ini` holds `hw.gltransport=asg` and `kernel.parameters=mitigations=off init_on_alloc=0 transparent_hugepage=always`; Android reported `gltransport=asg` in every step, and the guest's kernel command line was not read back. In the client's Settings these correspond to the performance engine switches, including **Reduce Android kernel protections**, which is off by default. The engine log of every boot, in both sessions, has the hypervisor APIC line, both TSC markers, the consumer spin of 3000 µs, the `sets-high` vCPU placement, both Vulkan cache lines and `VulkanNativeSwapchain` enabled. The candidate started the engine with `-fixed-scale`, as v0.26.0 does. The client confirmed each boot in 21.1–21.7 seconds.

## What failed

- **The third cycle's BlueStacks and Kemu steps.** The owner's everyday Android VM did not finish booting in time, so the four steps refused at start and timed nothing. BlueStacks and Kemu therefore have one boot per condition in this session.
- **BlueStacks, both timed steps.** The laps completed and their game reports were kept. Each step then ended with exit status 1 and its receipt says `failed`, as in the earlier series: closing the player through its exit dialog did not work (`Another HWND covers the Rust client`), and the restore transaction refused to run while the player was open (`Close all BlueStacks players and Instance Manager first: hd-player.exe`). The operator then ended `hd-player.exe`, stopped its wrapper and ran the restore. The receipts record the frame-rate and wrapper preferences as restored, the QA package as removed, the stock game and its configuration as preserved, no leftover process, and BlueStacks' configuration file as not byte-identical to the copy taken before the run (`host_config_byte_identity: false`). The instance's window keys were found unchanged after each step, and the everyday VM was reopened in 28.6 and 29.7 seconds.
- **Kemu's maximise request** was never applied (above).
- **RustEmuDroid, all eight steps, and LDPlayer, both steps,** ended with exit status 0. RustEmuDroid had no crash line in the game log, no leftover process and no change recorded to the owner's own installation; LDPlayer's operator restored its configuration by hash, removed the QA package and stopped its instance.
- **Not sampled.** The host load and processor frequency during Kemu's laps, the processor frequency during RustEmuDroid's laps, Android presentations in RustEmuDroid, BlueStacks and LDPlayer, and the picture of the other players.

## What this record does not show

- A lead in other games, other scenes, on other GPUs or PCs, or with the GPU connected directly rather than over Thunderbolt 3.
- The speed of the **published v0.26.0** download, or of a client applying the engine settings from its own preferences.
- The child-window panel with any other engine: it has run here only with this performance engine build, not with the engine that first-time setup installs or the Vulkan cache engine. With Kemu's engine the panel copies the frames.
- BlueStacks and Kemu over more than one boot per condition in this session, Kemu maximised, LDPlayer maximised in this session, the Balanced power mode or an idle desktop.
- The game area of BlueStacks on the screen: the session recorded only the size of its largest window.
- Linux, where the panel keeps copying the frames.

## Recalculate the results

Game FPS is `1000 × frames / profiled duration in ms` from each lap's report. The means are arithmetic means of the lap values; pooling frames over durations gives the same figures to one decimal. The data file was generated from the receipts and lists every lap of both sessions with its frame count, duration, start and end time, the SHA-256 of its game report and per-frame profile, the host load sample, and for RustEmuDroid the panel captures and the engine surface size. It also lists every step of both sessions in order with its exit status, the load before it and what it refused, the window sizes each player recorded, the checks of each boot, and what is not established. [Data, identities and hashes](windows-rtx-child-window-panel-peers.json)

The lap files themselves (game reports, per-frame profiles, window captures, engine and client logs) stay on the test PC and are not published with this record. Earlier Mailbox measurements of the same workload: [the panel on shared memory, November 27](windows-rtx-mailbox-shared-memory-panel-bluestacks.md) · [the alternated series of November 26](windows-rtx-mailbox-hypervisor-apic-bluestacks.md) · [the series of November 25–28](../../docs/performance-history.md#performance-engine-in-uncapped-mailbox).
