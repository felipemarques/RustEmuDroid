# GPUI Android image viewer experiment

This independent crate exercises GPUI's native GPU renderer with the same RGBA8 source as the Glow baseline. It draws one fitted image on black. The RustEmuDroid launcher, emulator and VM are outside this crate. A raw source can carry actual Android frames from an external, authorized reader.

## Build

Use the stable MSVC Rust toolchain, Visual Studio C++ Build Tools and a Windows SDK on Windows. GPUI 0.2.2 uses Direct3D 11 there. On Linux the manifest enables both GPUI Wayland and X11 backends. Build with a C toolchain and the platform development libraries, including `pkg-config`, `libxkbcommon-dev`, `libfontconfig1-dev`, `libfreetype6-dev` and `libwayland-dev` on Debian-family systems. Run in a graphical session with a working Vulkan driver. Linux runtime testing needs a Linux desktop. [GPUI project](https://www.gpui.rs/) · [Pinned GPUI source](https://docs.rs/crate/gpui/0.2.2/source/)

In this directory, a local Cargo cache keeps the experiment's downloaded dependencies and build products separate from the main app. On Windows PowerShell:

```powershell
$env:CARGO_HOME = Join-Path (Get-Location) '.cargo-cache'
cargo +stable build --locked --release --jobs 3
cargo +stable test --locked --no-default-features
```

On Linux use `CARGO_HOME="$PWD/.cargo-cache" cargo +stable build --locked --release --jobs 3`.

## Controlled replay

```powershell
.\target\release\gpui-viewer.exe --width 1600 --height 900 --fps 90 --seconds 10 --warmup 2 `
  --window-width 1040 --window-height 585 --expect-gpu 'Radeon 780M'
```

Use the executable at `./target/release/gpui-viewer` on Linux. `--expect-gpu` compares a case-insensitive substring to the adapter reported by `Window::gpu_specs()`. Software adapters and absent adapter metadata cause a nonzero exit. The example uses the Radeon 780M that GPUI selected on the Windows test PC. Window arguments specify **physical client pixels**. The viewer converts them using the actual window scale and waits for a matching client size before starting the producer. If the platform refuses the requested size or the drawable area or scale changes during a benchmark, the viewer exits with an error instead of publishing a comparison. The report includes the actual physical area and scale factor. Use the same monitor and adapter for both viewers. Keep the window visible, avoid interaction and run the viewers sequentially with the same arguments, preferably A/B/B/A. Benchmark release builds on an otherwise idle desktop.

The default source produces a moving solid rectangle and embeds each sequence ID in the opaque RGBA image. The producer schedules from a monotonic deadline, uses at most one pending frame, and reports overwritten frames and missed scheduling slots. The source module and producer are shared with the sibling Glow benchmark. A previous warm-up frame does not contribute to the measured count. The benchmark accepts any pending measured frame in the final callback. Both viewers emit `viewer_source_started` on stderr as the producer starts, then a single JSON receipt on stdout when the window closes.

## Replay captured Android frames

Supply a regular file with consecutive, complete, tightly packed RGBA8 frames and opaque alpha. There are no headers or stride bytes. Use the capture's width and height explicitly:

```powershell
.\target\release\gpui-viewer.exe --raw 'D:/private/frames.rgba' --width 640 --height 360 --fps 90 --seconds 10 --warmup 2
```

The file must be nonempty and divisible by `width × height × 4`. It repeats throughout warm-up and measurement, indexed by scheduled sequence modulo frame count. Both viewers therefore select the same raw frame for the same sequence even when skipping a late slot. Record the file SHA-256 separately. Use `--raw -` with a byte-preserving stdin pipe for live viewing. The pipe terminates on clean EOF and reports a partial frame as a failure. Its duration guard also closes the viewer if input stalls. Avoid text pipelines that transcode native output. The reader needs only the byte stream from the external capture process. It does not start a guest or make Android input requests.

## Interactive demo

```powershell
.\target\release\gpui-viewer.exe --demo --lang pt-BR --width 640 --height 360 --fps 60
```

**RustEmuDroid · GPUI** opens with a floating panel for pausing the visible image, resuming and switching fullscreen. Esc or the window close button ends the demo. The panel defaults to Brazilian Portuguese. `--lang en-US` and `--lang es-ES` are also available. Use `--raw PATH` for a looping Android recording or `--raw -` for a live pipe. Pausing freezes the local picture while the source continues. Demo mode has no benchmark timer or JSON report. A clean EOF leaves the last image visible for inspection.

## Reading the numbers

`processed` counts a distinct source sequence only after GPUI successfully queues its image in `Window::paint_image`. Latency is measured from producer publication to that paint callback. Frame rate and latency do not describe Android game FPS, an emulator's capture rate, completed GPU work or monitor scan-out. `produced`, `replaced`, `late_slots` and `duplicates` describe the source and its single-slot queue, so different producer rates can be spotted before attributing a difference to the viewer. Measure host CPU and RSS externally using the shared start marker.

GPUI 0.2.2 expects `RenderImage` pixels in BGRA. The viewer swizzles the owned RGBA buffer in place and passes it into the image. GPUI still uploads the texture to a GPU sprite atlas. Each frame gets its own image ID. The viewer retains the current image and one preceding image, evicting the earlier atlas entry with `Window::drop_image` as it replaces a frame. This bounds the experiment's retained images, while backend allocation and driver memory are separate. The measured path includes channel swizzle, lookup, atlas insertion and scheduling. [Image and atlas APIs](https://docs.rs/gpui/0.2.2/gpui/struct.RenderImage.html) · [GPUI paint implementation](https://docs.rs/crate/gpui/0.2.2/source/src/window.rs)
