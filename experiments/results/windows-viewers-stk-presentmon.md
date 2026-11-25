# Windows presentation events from the SuperTuxKart replay viewers

Glow and GPUI replayed the same 120-frame SuperTuxKart Black Forest clip from disk at 90 source updates/s. Both used the Radeon 780M and the same position and 1040 × 585 physical client area on its 1920 × 1080, 100 Hz display. The 640 × 360 input, both viewer executables and the PresentMon console are pinned by SHA-256 in the results. The initial standalone builds and the published v0.1.1 builds were tested in separate batches. No VM or live game was started for this comparison.

## Initial standalone builds

| Pass | Viewer | New source sequences painted/s | ETW present events/s | ETW-attributed display starts/s | ETW dropped submissions | Present interval P99 |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | Glow | 85.56 | 82.09 | 82.09 | 0 | 27.19 ms |
| 2 | GPUI | 85.19 | 69.91 | 69.73 | 2 | 41.40 ms |
| 3 | GPUI | 85.62 | 71.18 | 71.09 | 2 | 34.74 ms |
| 4 | Glow | 85.38 | 84.82 | 84.73 | 0 | 31.84 ms |

Across two passes each, Glow painted **85.47 new source sequences/s** and GPUI painted **85.41/s**. PresentMon attributed **83.45 present events/s** to Glow and **70.55/s** to GPUI, with display-start rates of **83.41/s** and **70.41/s**, respectively. Glow appeared as OpenGL runtime Other without a swapchain address, using Composed Copy with GPU GDI. GPUI's single DXGI stream used Composed Flip. Each pass verified one bracketed stream for the viewer process.

The paint counter uses each application's callback over the full 16-second post-warm-up interval. PresentMon used an 11-second QPC-aligned window inside it, beginning 6.75 seconds after the source-start marker. Both types of measurement are pooled by their own elapsed times. A present can reuse an already painted image, and Windows events carry no source-sequence ID. This comparison leaves physical scanout, panel response and the cause of the event-rate difference unmeasured. The results apply to these two builds and the verified window configuration.

[Sanitized per-run data and executable hashes](windows-viewers-stk-presentmon.json) · [PresentMon 1.10.0 CSV fields](https://github.com/GameTechDev/PresentMon/blob/v1.10.0/README-ConsoleApplication.md#csv-columns)

## Published GPUI lab v0.1.1

Both x86-64 executables came from the same official Windows ZIP. Its SHA-256 matched the GitHub release checksum and the tagged CI artifact. Each executable matched the corresponding archive member. The four fresh processes reused the clip, monitor, adapter, client size, viewport location, A/B/B/A order and observation windows from the initial test. The operator verified the titled GPUI window by process, title and class. An earlier launch that failed the foreground check was excluded before ETW started. [Official prerelease](https://github.com/felipemarques/RustEmuDroid/releases/tag/gpui-lab-v0.1.1) · [Tagged workflow](https://github.com/felipemarques/RustEmuDroid/actions/runs/45628469031)

| Pass | Viewer | New source sequences painted/s | ETW present events/s | ETW-attributed display starts/s | ETW dropped submissions | Present interval P99 |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | Glow | 87.75 | 81.73 | 81.55 | 0 | 27.82 ms |
| 2 | GPUI | 87.88 | 71.18 | 70.82 | 4 | 30.42 ms |
| 3 | GPUI | 87.75 | 71.18 | 71.00 | 3 | 33.07 ms |
| 4 | Glow | 87.12 | 83.27 | 83.18 | 0 | 27.28 ms |

Glow and GPUI painted **87.44 and 87.81 new source sequences/s**, respectively. PresentMon recorded **82.50 and 71.18 present events/s** and attributed **82.36 and 70.91 display starts/s** to their respective processes. The compiled release reproduced the lower ETW event rate for GPUI despite similar paint-callback throughput in this setup. Glow's window used Composed Copy with GPU GDI and GPUI used DXGI Composed Flip. ETW identifies events from those paths. The replay's source sequences and the physical monitor output require separate evidence. [Published-build data, per-run counters and archive hash](windows-viewers-stk-presentmon-release.json)
