# PUBG MOBILE startup on the original Kemu Windows frontend

On 2026-11-25, PUBG MOBILE 4.7.0 (21718, ARM64) failed to reach the Lobby in the original Kemu Qt 0.7.1 frontend. The standard translator setting failed during startup. The lightweight setting reached `GameActivity` in the activity manager on both attempts, but the process crashed and Android Home returned before a sustained game scene appeared. There is no new Kemu Lobby FPS measurement. [Sanitized receipt](windows-kemu-qt-21718.json)

Both cold boots used an isolated Android 16/API 36 Kemu profile, four vCPUs, 8 GiB memory and a 1600×900 display. SurfaceFlinger reported a period of 16,666,666 ns. The emulator selected host Vulkan on the physical NVIDIA GeForce RTX 5070 Ti with driver 610.88, and Android's EGL property reported ANGLE. The game's own graphics backend could not be verified. The package manager and installed file digest confirmed the same APK, version code and ARM64 selection in both boots. The APK and runtime hashes are in the receipt.

| ARM translation | Launches | Observed startup |
| --- | ---: | --- |
| Kemu default | 1 | Berberis logged SIGILL/ILL_ILLOPN about 0.35 s after Android started the `SplashActivity` process. The activity manager later recorded a start timeout and returned to Home. |
| `lite-translate-or-interpret` | 2 | The boot started with the mode property empty and readback confirmed the Kemu workspace's documented one-boot lightweight setting. The Android activity manager named `GameActivity` at 3.38 and 3.13 s. Each distinct process then exited with SIGILL/ILL_ILLOPN on `MainThread-UE4`. The repeat briefly displayed the Android PUBG icon splash, then returned to Home. |

In the two lightweight crash reports the host instruction pointer was in a generated executable mapping. Both guest stacks put the bridged ARM64 libc's `native_bridge_malloc+0` above the same call site in `libGCloudVoice.so` (ELF offset `0x2caa34`). The stack identifies the failing path. The underlying cause remains unresolved.

The tests stopped before login or character creation. Each removed only its own temporary APK, verified package absence and restored the test profile's performance preferences byte for byte. The normal RustEmuDroid 0.21.1 Android 17 Home returned at 60 Hz as the only VM, with its launcher binary and preferences unchanged.

## Historical tutorial sample

A separate Kemu record from 2026-11-22 reports PUBG MOBILE 4.7.0 (21783, ARM64) in a playable 3D tutorial at Balanced, 1600×900 and 60 Hz, using lightweight translation on the RTX 5070 Ti. The workspace notes record movement, camera and item pickup. That run used the local Kemu launcher with BlueStacks also open. Its one surviving summary row records 1,812 new Android SurfaceView presentations over 30.19 s of host wall time, or 60.02 presentations/s. The first-to-last timestamp span was 30.18 s. The reported p50, p95 and p99 presentation intervals were 16.66, 18.08 and 19.32 ms.

The historical PowerShell collector extracted positive, non-sentinel timestamps from the second column of `SurfaceFlinger --latency` for the PUBG layer. It verified overlap between successive buffers, deduplicated timestamps, divided new presentations by the host stopwatch duration, and took nearest-rank percentiles of adjacent timestamp differences. The surviving CSV and collector checksums are in the JSON. The raw timestamps were unavailable for independent percentile recomputation.

The historical tutorial used a different PUBG build and scene and cannot fill the missing Kemu row in a matched Lobby comparison. A separate historical 21718 test under the Rust frontend reached a character-preview screen, also a different scene. [Character-preview result](../../docs/game-testing.md#character-preview-on-android-16)
