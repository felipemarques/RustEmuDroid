# Visible Black Forest changes in the Windows GPUI and Glow viewers

The published GPUI lab v0.1.1 Windows executables replayed the same 120-frame SuperTuxKart Black Forest clip from disk at a scheduled 90 frames/s. The source was 640 × 360 RGBA. Tests used the same 1040 × 585 physical client area and position on the Radeon 780M at 1920 × 1080 and 100 Hz. Each viewer started fresh in Glow, GPUI, GPUI, Glow order. This test opened no emulator and used one viewer at a time. [Release package](https://github.com/felipemarques/RustEmuDroid/releases/tag/gpui-lab-v0.1.1) · [Data and hashes](windows-viewers-stk-dxgi.json)

Desktop Duplication supplied the most recent composed image and its QPC timestamp. The reader copied 12 evenly spaced scanlines inside the owned viewer client. An exact update changes at least one RGB pixel. A material update changes at least six sampled pixels by four or more in any channel. The fixed 11-second observation began 6.75 seconds after each viewer reported source startup. The viewer's internal counter covers a separate 16-second interval. Visual updates can omit repeated pixels or changes outside sampled scanlines. These measurements do not identify physical scanout, a specific source frame or in-game FPS. [Previous ETW comparison](windows-viewers-stk-presentmon.md)

| Pass | Viewer | Exact RGB updates/s | Material RGB updates/s | Desktop updates acquired | Unavailable intermediates | Eligible for rate comparison |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| 1 | glow | 85.00 | 85.00 | 1099 | 0 | Yes |
| 2 | gpui | 87.09 | 87.09 | 976 | 0 | Yes |
| 3 | gpui | 84.91 | 84.91 | 957 | 0 | Yes |
| 4 | glow | 83.27 | 83.27 | 1099 | 0 | Yes |

Pooled exact-update rates were **84.14/s for Glow** and **86.00/s for GPUI**. The corresponding material rates were **84.14** and **86.00/s**. No acquisition reported superseded desktop updates.

The comparison required both adjacent-pair differences to point in the same direction and exceed the larger of the two viewers' run-to-run spreads at both pixel thresholds. This batch fell short of that rule.
