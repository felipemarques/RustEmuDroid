# Windows presentation events from the SuperTuxKart replay viewers

Glow and GPUI replayed the same 120-frame SuperTuxKart Black Forest clip from disk at 90 source updates/s. Both used the Radeon 780M and the same position and 1040 × 585 physical client area on its 1920 × 1080, 100 Hz display. The 640 × 360 input, both viewer executables and the PresentMon console are pinned by SHA-256 in the results. The viewer executables are the initial standalone builds from the earlier A/B/B/A. No VM or live game was started for this comparison.

| Pass | Viewer | New source sequences painted/s | ETW present events/s | ETW-attributed display starts/s | ETW dropped submissions | Present interval P99 |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | Glow | 85.56 | 82.09 | 82.09 | 0 | 27.19 ms |
| 2 | GPUI | 85.19 | 69.91 | 69.73 | 2 | 41.40 ms |
| 3 | GPUI | 85.62 | 71.18 | 71.09 | 2 | 34.74 ms |
| 4 | Glow | 85.38 | 84.82 | 84.73 | 0 | 31.84 ms |

Across two passes each, Glow painted **85.47 new source sequences/s** and GPUI painted **85.41/s**. PresentMon attributed **83.45 present events/s** to Glow and **70.55/s** to GPUI, with display-start rates of **83.41/s** and **70.41/s**, respectively. Glow appeared as OpenGL runtime Other without a swapchain address, using Composed Copy with GPU GDI. GPUI's single DXGI stream used Composed Flip. Each pass verified one bracketed stream for the viewer process.

The paint counter uses each application's callback over the full 16-second post-warm-up interval. PresentMon used an 11-second QPC-aligned window inside it, beginning 6.75 seconds after the source-start marker. Both types of measurement are pooled by their own elapsed times. A present can reuse an already painted image, and Windows events carry no source-sequence ID. This comparison leaves physical scanout, panel response and the cause of the event-rate difference unmeasured. The results apply to these two builds and the verified window configuration.

[Sanitized per-run data and executable hashes](windows-viewers-stk-presentmon.json) · [PresentMon 1.10.0 CSV fields](https://github.com/GameTechDev/PresentMon/blob/v1.10.0/README-ConsoleApplication.md#csv-columns)
