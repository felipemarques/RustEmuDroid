# RustEmuDroid

[Português brasileiro](README.pt-BR.md) · [Español](README.es-ES.md) · [Windows and Linux downloads](https://github.com/felipemarques/RustEmuDroid/actions/workflows/build.yml) · [Benchmarks](benchmarks/stk-1.5/README.md)

RustEmuDroid runs Android games and apps in one desktop window on x86-64 Windows and Linux. The Rust client starts the virtual machine, displays Android and handles input and window management. Inside the VM, the bundled Android Home opens to your installed games. The desktop client and Home are ad-free. Brazilian Portuguese is the default. Choose English (US) or Spanish (Spain) in Settings and save to change the desktop controls and Android Home together. Installed games and apps keep their own language settings.

![RustEmuDroid with the Android game library open](docs/screenshots/en-us/games.png)

*The games in this screenshot were installed on the test profile. A new profile starts with the apps supplied by its Android image.*

## Run a prebuilt version

1. Open [Desktop builds](https://github.com/felipemarques/RustEmuDroid/actions/workflows/build.yml), select a successful run and find **Artifacts**. Sign in to GitHub to download an artifact.
2. Download **RustEmuDroid-Windows-x86_64** or **RustEmuDroid-Linux-x86_64** and extract the ZIP into a writable directory. On Linux, also run `tar -xzf rustemudroid-linux-x86_64.tar.gz` to unpack the executable with its execute permission.
3. Install the Android runtime described below. From the directory containing the executable, run the commands for your platform.

| Task | Windows PowerShell | Linux shell |
|---|---|---|
| Check SDK and virtualization | `.\rustemudroid.exe doctor` | `./rustemudroid doctor` |
| Open the desktop client | `.\rustemudroid.exe ui` | `./rustemudroid ui` |

The executable embeds the RustEmuDroid Home APK. The Android SDK, system images, games and user data are separate. Rust and Cargo are only required for source builds.

## Prepare the Android runtime

| Requirement | Windows x86-64 | Linux x86-64 |
|---|---|---|
| Hardware virtualization | Enable Intel VT-x or AMD-V in firmware and **Windows Hypervisor Platform** in Windows Features. | Enable CPU virtualization and give your user read/write access to `/dev/kvm`. Check with `test -r /dev/kvm && test -w /dev/kvm`. |
| Desktop and graphics | Install the host GPU driver. | Use an X11 or Wayland session with working OpenGL. The APK picker uses the desktop file portal. |
| Android tools | Install Android Emulator, Platform-Tools and an x86-64 system image through Android Studio’s SDK Manager or the Android command-line tools. | Install the Linux versions of the same components, including under WSL2. |

With `sdkmanager` on your PATH, install the runtime and the optional Google Play image with the following command. Accept the SDK licenses when prompted.

```sh
sdkmanager --install "emulator" "platform-tools" "system-images;android-37.0;google_apis_playstore;x86_64"
```

For a Google APIs image, another supported package is `system-images;android-35;google_apis;x86_64`. Use `sdkmanager --list` to see available packages. SDK discovery checks `--sdk`, `ANDROID_HOME`, `ANDROID_SDK_ROOT` and finally `%LOCALAPPDATA%\Android\Sdk` on Windows or `~/Android/Sdk` on Linux. [SDK Manager](https://developer.android.com/tools/sdkmanager) · [Emulator acceleration](https://developer.android.com/studio/run/emulator-acceleration)

## Choose a profile

Open the gear in the title bar. Each profile has its own writable disk, installed apps, accounts and saves. **Original** is the default profile for that working directory. Its Android version depends on the chosen image.

| Configuration | Runtime and first use |
|---|---|
| Original with Kemu | On Windows, an installed Kemu can supply its Android 16 / API 36 image and matching QEMU/gfxstream engine with Host or Auto GPU. The tested setup uses ANGLE/Vulkan. Set `RUSTEMUDROID_KEMU_ROOT` for nonstandard installations. |
| Original with the SDK | On first use, the client looks for installed Google Play API 37.0, Google APIs 36 and Google APIs 35 images, in that order. Use `init --image` to select a specific package. |
| Android 17 with Google Play | Install the API 37.0 Google Play image, select **Android 17** in Settings and save. This SDK image includes the Play Store and Google Play services. The card indicates when the image is missing. |

Gaming profiles request four vCPUs, 8 GiB of guest RAM and 1600 × 900 at 60 Hz. A generic SDK profile requests four vCPUs, 3 GiB and 1080 × 1920. The Windows Kemu engine keeps a separate writable disk under `gaming/`.

![Settings with profile selection, rendering mode, Vulkan adapter and webcam](docs/screenshots/en-us/settings.png)

## Use the desktop controls

| Task | How to do it |
|---|---|
| Find and install apps | Search under **Games** or **Apps**. Long-press a card to change its category. The install button on the right accepts a single APK and refreshes the library. |
| Navigate Android | The four-square button opens Apps. The title bar also contains **Back**, **Home** and **Recent apps**. Home returns to Games. Click to tap and drag to swipe. |
| Map a keyboard or controller | Open a game and click the keyboard icon. Choose **Button**, click a touch target, then press the physical key or controller button. For WASD or the left stick, choose **Directional control**, click the center of the virtual stick and adjust its radius. Right-click a marker to remove it. **Save** stores relative coordinates for that Android package. Windows gamepads use XInput. |
| Adjust media | The right toolbar controls Android media volume, mute and screen rotation. Hold a volume button to change several levels. |
| Use a webcam | In Settings, select a front or rear host webcam, starting with index 0. Save and restart Android to apply hardware changes. Grant camera access inside the Android app when needed. A simulated front camera is also available. |
| Fullscreen and version | Press **F11**. Move the pointer to the top or right edge to reveal controls in fullscreen. The title bar and **?** dialog show the version. |

![About dialog with the application logo and version](docs/screenshots/en-us/about.png)

**Stop Android** stops the VM and returns to the panel. Closing the window with **X** or the system close shortcut shows the shutdown screen. The native window keeps responding until the VM exits, then closes automatically. This also works during boot. Closing an idle panel takes effect immediately.

![Window waiting for Android to shut down](docs/screenshots/en-us/shutdown.png)

After stopping Android, the session screen lets you start the same profile again.

![Session screen after stopping Android](docs/screenshots/en-us/idle.png)

## Open an installed BlueStacks instance on Windows

List the instances registered by BlueStacks 5, then start one by ID. Add the Android package name to open a game already installed in that instance.

```powershell
.\rustemudroid.exe bluestacks list
.\rustemudroid.exe bluestacks open --instance Pie64
.\rustemudroid.exe bluestacks open --instance Pie64 --package org.supertuxkart.stk
```

BlueStacks opens its own player window and uses that instance’s apps, saves and devices. RustEmuDroid’s embedded display, input mapping and FPS counter belong to its native Android runtime. Independent boot tests use private disk copies. See the [boot and device verification record](docs/bluestacks-lab.md).

The crab moves during startup and shutdown, in About, and when Android Home has no matching items. The game view keeps the mascot off the rendering path.

![Preview of the desktop mascot animation](docs/mascot-preview.png)

## Capture gameplay

| Action | Shortcut | Output |
|---|---|---|
| Screenshot | **Ctrl+Shift+S** | Full-resolution PNG of the guest display |
| Start or stop video | **Ctrl+Shift+R** | Silent H.264 MP4 with a 30 FPS target |
| Record a moment | **Ctrl+Shift+J** | The next three seconds as an animated JPEG XL at 10 FPS |
| Record a GIF | GIF tool in the sidebar | The next three seconds as a looping GIF at 10 FPS |

Open the folder button on the sidebar to find your files in `captures/` beside `launcher.json`. PNG works directly in Rust. Install FFmpeg with `libx264` for MP4 and `libjxl_anim` for animated JPEG XL. Set `RUSTEMUDROID_FFMPEG` to an executable path if FFmpeg is outside PATH. The two short animations fit within 720 × 720 and keep their aspect ratio. Recording starts when you press the shortcut. The window waits for the file to finish writing when you close it.

## Create a control scheme

Open a game, then press **Ctrl+Shift+A** or click the keyboard icon. Use the advanced editor to place a touch control on the game, bind an input and adjust its properties. For example, bind a D-pad to movement, set a separate relative aim control and bind the primary mouse button to the on-screen fire button. A timed swipe can have its own end point and duration. Pinch and rotation produce two simultaneous contacts. MOBA skills support directional dragging from the skill button. Save the scheme for that game. Use named profiles to switch layouts, or export and import an individual scheme as JSON.

![The control editor over a running SuperTuxKart session](docs/screenshots/en-us/editor.png)

*SuperTuxKart is shown in English. The game manages its language separately from the control editor.*

## Build from source

Install current stable Rust with Cargo. Windows also needs the Visual Studio C++ Desktop build tools and Windows SDK. On Ubuntu 24.04, install the native dependencies:

```sh
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libudev-dev libwayland-dev libxkbcommon-dev libx11-dev libxi-dev libxrandr-dev libxcursor-dev libgl1-mesa-dev
```

```sh
git clone https://github.com/felipemarques/RustEmuDroid.git
cd RustEmuDroid
cargo build --locked --release --features ui
cargo run --locked --release --features ui -- ui
```

On Windows, [Abrir RustEmuDroid.cmd](Abrir%20RustEmuDroid.cmd) builds the release client and opens it, or focuses its existing window. Omitting `--features ui` builds the CLI.

## CLI and storage

```sh
cargo run --locked -- doctor
cargo run --locked -- init --image "system-images;android-35;google_apis;x86_64"
cargo run --locked -- boot --headless --check --gpu auto
cargo run --locked -- boot --help
```

To use an installed Kemu image with a separate data directory on Windows:

```powershell
cargo run --locked -- --state-dir .rustemudroid/kemu init --system-dir "C:/Program Files/Kemu/system-img"
cargo run --locked --release --features ui -- --state-dir .rustemudroid/kemu ui
```

Use the same `--state-dir` for initialization, boot and UI. The default is `.rustemudroid/windows/` or `.rustemudroid/linux/` relative to the working directory. `launcher.json` holds preferences. The active profile contains the AVD and `boot-*.log`. `boot --check` waits for `sys.boot_completed=1` and stops that VM. `--timeout` sets the boot deadline in seconds and `--engine sdk` explicitly selects the SDK engine. The client selects free ports and verifies the AVD path before sending shutdown commands.

## Performance and troubleshooting

The [Radeon 780M benchmark](benchmarks/stk-1.5/radeon-780m/README.md) compares integrated graphics across RustEmuDroid, Kemu and BlueStacks. It includes per-run measurements and host GPU verification.

- **FPS.** The title bar counts new frames sent for drawing by the desktop panel. A static Home can show a low value. Use the game’s own counter or frame report to compare game performance.
- **Measured example.** In the SuperTuxKart 1.5 Black Forest replay, RustEmuDroid averaged 59.93 FPS at the original preset and 28.90 FPS at high quality on the tested Windows RTX 5070 Ti / Android 16 OpenGL ES setup. The report lists the driver, ABI, presets, frame times and the Kemu and BlueStacks results. [Benchmark and raw measurements](benchmarks/stk-1.5/README.md)
- **Black screen or slow graphics.** Run `doctor`, check the selected Vulkan adapter and try Host or Auto. Software/SwiftShader can help isolate driver issues. The SDK engine uses a GLES compatibility path with the Kemu image on Linux.
- **Several ADB devices.** Run `adb devices -l` and use `adb -s <serial>` to target the correct instance. Look up the session serial in the launcher output.
- **PUBG hangs during startup on Kemu Android 16.** In Settings, enable the lightweight ARM translation option for PUBG and save. This Berberis mode affects ARM apps in that profile during the boot and may change FPS. The local test reached sign-in. Game compatibility also depends on ABI support and publisher restrictions.
- **Missing games after switching.** Return to the profile where you installed them. A fresh clone contains neither installed games nor accounts.

## Architecture and verification

| Component | Role |
|---|---|
| [Desktop UI](src/ui.rs) | Rust, eframe/egui and Glow. Owns the native window, controls, settings and shutdown screen. |
| [VM lifecycle](src/android.rs) | Discovers the SDK and images, creates AVDs, launches the selected emulator and waits for its exit. |
| [Display and input](src/display.rs) | Receives authenticated local gRPC frames from `EmulatorController.streamScreenshot`, retains the latest pending RGBA frame and forwards touch and key input. |
| [Android Home](android/home) | Native Android activity. Reads launcher entries through PackageManager and uses app banners or icons. The desktop client installs the embedded development APK through ADB. |

Run `cargo test --locked --features ui` for the offline tests. [GitHub Actions](.github/workflows/build.yml) tests and builds release binaries for Windows and Ubuntu x86-64 on pushes, pull requests and manual runs. Guest tests require a local AVD. To repeat the native Windows close test, set `RUSTEMUDROID_TEST_STATE` to the absolute directory of an idle launcher profile, then run `cargo test --locked --features ui real_window_paints_until_android_exits -- --ignored --nocapture`.

To rebuild Android Home, install a JDK, a platform `android.jar` and Android Build-Tools. Run `python android/home/build.py <android.jar> <build-tools-dir>`. The script uses `~/.android/debug.keystore` by default and supports `--keystore` and `--alias`. To upgrade existing installations, retain the signing identity and update the version code in the manifest and [installation check](src/controls.rs).
