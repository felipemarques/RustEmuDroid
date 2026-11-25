# GPUI viewer lab

Extract the release archive and open a terminal in that folder. The GPUI and Glow viewers draw the same RGBA8 input in a desktop window. Both can run independently of Android.

## Open the prototype

On Windows, run these commands in PowerShell.

```powershell
.\gpui-viewer.exe --demo
.\glow-baseline.exe --width 1600 --height 900 --fps 90 --seconds 10
```

On Linux, run the corresponding binaries in a graphical session.

```sh
./gpui-viewer --demo
./glow-baseline --width 1600 --height 900 --fps 90 --seconds 10
```

The GPUI demo has pause and fullscreen controls. It starts in Brazilian Portuguese. Use `--lang en-US` or `--lang es-ES` to change the interface. The timed commands use a moving synthetic image and print a JSON result when they exit. Both executables support `--help`. Windows uses Direct3D 11 for GPUI and OpenGL for Glow. On Linux, GPUI needs a working Vulkan driver and X11 or Wayland. Glow needs OpenGL 3 or later.

## Compare both viewers on the same input

Use Python 3.11 or newer. Run the viewers sequentially on the same monitor and adapter. Use a new output filename per comparison.

```powershell
python -m pip install "psutil==7.2.2"
python compare.py --glow .\glow-baseline.exe --gpui .\gpui-viewer.exe --output synthetic-abba.json
```

On Linux, use `python3`, `./glow-baseline` and `./gpui-viewer`. To compare a saved recording, add `--raw race.rgba --width 640 --height 360` and specify the recording's actual dimensions. The comparison runs Glow, GPUI, GPUI and Glow on the same scheduled source. It reports distinct frames submitted for drawing and sampled CPU and memory. The viewers do not measure game FPS or monitor scanout.

## Optional Android bridge

The bridge reads screenshots from one authorized, already running Android Emulator session with an authenticated controller on loopback. Use the matching SDK's `adb devices` to identify the guest. If its serial is `emulator-5554`, use `adb -s emulator-5554 emu avd path` to find the AVD directory. Substitute your own serial's port and its AVD path below. Match the requested aspect ratio to the current Android display.

```powershell
python -m pip install -r android-stream/requirements.txt
$avd = 'D:\path\to\your\RustEmuDroid.avd'
python android-stream/capture.py --avd "$avd" --console 5554 --width 640 --height 360 --viewer .\gpui-viewer.exe --demo
python android-stream/capture.py --avd "$avd" --console 5554 --width 640 --height 360 --output race.rgba --frames 120
```

On Linux, use `python3`, the Linux AVD path and `--viewer ./gpui-viewer`. A recording contains the screen pixels of your local guest. The bridge keeps the existing session running and sends no guest input. [GPUI development notes](README-GPUI.md) · [Glow baseline development notes](README-Glow.md).
