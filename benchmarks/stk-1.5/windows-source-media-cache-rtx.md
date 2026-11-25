# SuperTuxKart on the Windows cache engine built with FFmpeg and libvpx sources

On November 25, 2026, RustEmuDroid v0.22.0 completed two cold boots and two scored FIFO replays on the Windows engine linked with FFmpeg 4.4.2 and libvpx 1.12.0. Turning on the Vulkan memory cache raised game throughput from **84.73 to 90.16 FPS**. The time between Android presentations at the 99th percentile fell from **23.17 to 14.46 ms**. Each condition has one scored replay. The [earlier four-run series](windows-cached-memory-kart.md) tested the previous engine build. [Run data and timestamps](windows-source-media-cache-rtx.json) · [Game reports, per-frame traces and memory probe output](windows-source-media-cache-rtx-traces.zip)

| Vulkan cache | Game frames / seconds | Game FPS | Android presentations/s | Android P99 | Android gaps over 16.667 ms | Frames processed/s by Rust panel |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Off | 3,225 / 38.063 | 84.73 | 83.51 | 23.17 ms | 130 / 1,671 | 80.22 |
| On | 3,431 / 38.053 | 90.16 | 89.96 | 14.46 ms | 0 / 1,800 | 86.32 |

The release client, QEMU executable, NVIDIA RTX 5070 Ti, WHPX, four virtual CPUs, 1600 × 900 Android 17 Google Play r7 image and 800 × 450 Rust viewport matched across boots. The emulator recorded the cache marker exactly once on the enabled boot. Both used the default clock. Each boot ran a memory probe and warmed up before scoring the same signed x86-64 SuperTuxKart 1.5 Black Forest replay in High/Vulkan. The 21 graphics fields matched. The game's renderer reported FIFO in both scored runs. An earlier attempt lost Windows foreground and was excluded from this comparison.

The game's FPS uses its full replay. Android timestamps cover a roughly 20-second sample of the foreground game's SurfaceView. P99 uses index `floor((n−1)×0.99)` in the sorted intervals. The Rust panel counts processed frames over its own complete counter intervals. Its rate still falls short of Android's presentations on the enabled boot. These counters describe different stages of delivery.

The separate mapped-write probe copied 4 MiB at a time into guest memory type 2 (host-visible and coherent, heap 1). Across six batches, median elapsed time fell from **14.552 to 0.070 ms** for a copy and from **126.209 to 0.213 ms** for 32-bit stores. In the replay, mean time acquiring the next swapchain image rose from 2.552 to 4.786 ms per game frame. The APK was removed after testing and the existing user profile returned to Android Home at 60 Hz.

## Exploratory Mailbox run

With the cache enabled and VSync off in the game, the renderer selected Mailbox (`VkPresentModeKHR = 1`). The game reported **183.63 FPS**, while the Android surface presented **90.01 frames/s** on the 90 Hz guest. The Windows foreground check failed during sampling, so there is no qualified panel result or three-player ranking for this run. Its raw game report and separate Android timestamps are retained under `exploratory_mailbox` in the [data](windows-source-media-cache-rtx.json).

Two subsequent Mailbox replays on the same relinked engine passed the Windows foreground and moving-image checks. The game measured **163.54 and 176.38 FPS**, the Android surface **89.81 and 90.05 presentations/s**, and the Rust panel **85.46 and 87.43 processed frames/s**. The same workload ran twice each on Kemu and BlueStacks. [Six matched replays and raw traces](windows-rtx-mailbox-peers.md)

## Media smoke on the relinked engine

After the two qualified Mailbox runs, a short Android MediaCodec test decoded H.264, VP8 and VP9 with the software decoders. Each returned 12 distinct frames. The three `c2.goldfish.*` implementations advertised hardware acceleration and raised `MediaCodec.CodecException` in the buffer-output test. That leaves hardware decoding and ordinary surface-output playback unverified.

The Android Emulator console's `screenrecord` path started and stopped successfully. Its 640 × 360 VP9 WebM contained 69 decoded frames and 44 distinct images after scaling to 32 × 18 grayscale for comparison. These are smoke-test counts. Temporary guest fixtures were removed and the usual 60 Hz Home was restored. The recording stays private. [Sanitized media receipt](windows-source-media-cache-rtx-media.json)

## Engine and installation

The cache module ZIP has SHA-256 `a67d12ec5abbb220cce81ecf623174d278fab08dd5d0a08441be51c642395146`. Its QEMU has SHA-256 `b9feb63b82209b4b93ef39131687d25782bc291219bfc59b1a91909076a519c8`. The public stager installed the 12 project-built modules at 20 paths in a separate SDK installation. Its checks verified the archive, both executable versions and WHPX availability. The game comparison exercised this QEMU build's graphics path. Codec playback and recording require their own validation. [Installation and verification commands](../../experiments/whpx-clock/README.md#2-create-the-private-runtime)
