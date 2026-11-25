# Windows DWM native viewer

This experiment asks whether Desktop Window Manager can show the existing Qt/Kemu game surface in a separate Rust window. It uses a DWM thumbnail relationship with a crop in physical client pixels. The crate does not change the Qt window hierarchy, send input to Qt, start or stop a VM, or read the Android framebuffer. Closing the viewer releases the thumbnail and leaves Qt running.

The shipping embedded player starts the emulator with its own window hidden or disabled and receives frames over gRPC. The lab needs a visible, restored Qt source window on the same Windows desktop. The viewer accepts Escape to close. The source emulator retains control of the game. [Native session startup](../../src/android.rs) · [Microsoft DWM thumbnails](https://learn.microsoft.com/en-us/windows/win32/dwm/thumbnail-ovw)

## Build and smoke test

For a prebuilt executable, open a successful **Desktop builds** run for v0.21.0 or later, download the **Windows-DWM-viewer** artifact and extract it. In a PowerShell window in that folder, run `.\dwm-native-viewer.exe --smoke --width 640 --height 360 --warmup 0 --seconds 5`. The test opens a source window and a preview for five seconds. [Desktop builds](https://github.com/felipemarques/RustEmuDroid/actions/workflows/build.yml)

From a source checkout, build and run the same smoke test from the repository root.

```powershell
cargo test --locked --manifest-path experiments/dwm-native-viewer/Cargo.toml
cargo build --locked --release --manifest-path experiments/dwm-native-viewer/Cargo.toml
$viewer = Resolve-Path 'experiments/dwm-native-viewer/target/release/dwm-native-viewer.exe'
& $viewer --smoke --width 640 --height 360 --warmup 0 --seconds 5
```

Run the smoke test in an interactive Windows desktop running DWM. It creates two local windows. The preview should fill its client area with magenta and a green rectangle in the upper-left quarter. The smoke source has contrasting margins outside the selected rectangle. Press Esc in the preview or wait for it to close. This checks client cropping with a GDI source. A Qt/Vulkan source still needs an independent pixel check.

## Attach to a selected Qt window

On Windows, open the lab emulator with its Qt window visible. From the same PowerShell session as the build, list the running QEMU processes and choose the PID whose command line identifies your test profile. These commands use the `subWin` render child found in the tested Kemu 0.7.1 runtime.

```powershell
Get-CimInstance Win32_Process -Filter "Name='qemu-system-x86_64.exe'" |
    Format-List ProcessId, ExecutablePath, CommandLine
$qtPid = [int](Read-Host 'PID of the Qt test emulator')
$qtExe = (Get-Process -Id $qtPid -ErrorAction Stop).Path
$inventory = & $viewer --inspect --pid $qtPid --exe $qtExe | ConvertFrom-Json
$pairs = @(foreach ($window in $inventory.windows) {
    if ($window.visible -and !$window.minimized) {
        foreach ($child in $window.children) {
            if ($child.visible -and $child.class -ceq 'subWin') {
                [pscustomobject]@{ Window = $window; Render = $child }
            }
        }
    }
})
if ($pairs.Count -ne 1) { throw 'Expected one visible subWin. Inspect $inventory.windows before continuing.' }
$source = $pairs[0].Window
$render = $pairs[0].Render
$rootRect = $source.client_physical_px
$gameRect = $render.client_physical_px
$crop = '{0},{1},{2},{3}' -f ($gameRect[0] - $rootRect[0]), ($gameRect[1] - $rootRect[1]), $gameRect[2], $gameRect[3]
& $viewer --pid $qtPid --exe $qtExe --hwnd $source.hwnd --class $source.class `
  --child $render.hwnd --crop $crop `
  --width 1040 --height 585 --warmup 2 --seconds 16
```

Use the hexadecimal HWNDs and exact class from the inventory. Select the top-level Qt window containing the game. If a render child appears, pass its HWND and compute `$crop` as `left,top,width,height` relative to the **Qt client**. Subtract the Qt client's screen origin from the render child's screen origin and retain its width and height. Inspect the displayed image to ensure the selected rectangle excludes host bars and controls. For rendering directly in the top-level Qt client, omit `--child` and specify the exact game rectangle yourself. `--origin` is an optional `x,y` in physical desktop pixels for the destination client origin. The destination must fit within the source monitor's work area and defaults to its center. Keep both windows visible and fixed in position and size. Use an unobstructed region for the first pixel check.

The first JSON line, `destination_ready`, records both window identities, client geometry, and `destination_fitted_screen_px` as `[left,top,width,height]` for a desktop capture. Confirm at least two different game images in that rectangle while the game is visibly moving. DWM accepting the HWND and crop does not guarantee that its redirectable content includes a Vulkan child surface. Treat grey, black, static or incorrectly cropped output as a failed pixel check and preserve the evidence.

`viewer_source_started` marks the start of warm-up. A normal run returns `viewer_finished` with observed wall time, separate process CPU deltas, and destination geometry and foreground sample counts. A close during observation returns `viewer_closed` with `completed:false`. A source identity, visibility or geometry failure returns an error and unregisters the thumbnail. The viewer uses a 100 ms message-wait interval between checks. Its CPU figure is based on process user and kernel time. It excludes participating Windows processes such as the desktop compositor. Source CPU includes the guest and its original Qt display in this configuration.

The experiment has no count of completed frames. For a live A/B test, reuse the same AVD, replay, renderer and destination viewport while changing only the separate viewer. Track the guest SurfaceView and game metrics independently. A per-window PresentMon stream for the DWM-hosted preview may be absent or measure a different part of composition than an application swapchain. Keep pixel evidence, CPU, and ETW attribution separate. The test does not establish zero-copy, display scanout timing, or the cost of removing the existing Qt presentation.
