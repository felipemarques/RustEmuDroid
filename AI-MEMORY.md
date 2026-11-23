# Engineering memory

| Decision | Consequence for future work |
|---|---|
| The Rust desktop client uses eframe/egui and Glow. Android Home is a native Android activity inside the guest. | Games, apps and Home navigation share one Android session. Host controls and preferences belong to the desktop window. |
| The native runtime uses the Android Emulator controller and ADB. The Windows Kemu engine can provide its matching Android image and graphics stack. | A different hypervisor or disk format alone does not supply the controller, guest drivers or display/input protocol the embedded UI uses. |
| Each Android profile owns its writable storage. | Check the profile before attributing missing games, different Android versions or different GPU settings to a regression. |
| Home is bundled in the executable as a signed APK. | Rebuild the APK when changing Home sources or resources. Preserve the signing key when upgrading a profile in place. |
| New desktop installations default to pt-BR. The client also supports en-US and es-ES. | Store the selected locale with the launcher preferences and apply it to the bundled Home; third-party apps manage their own language. |

## Verification history

- Game measurements on the integrated Radeon 780M include each emulator, game presets, individual runs and adapter verification. [Benchmark and measurements](benchmarks/stk-1.5/radeon-780m/README.md)
- Refer to the [Windows and Linux workflow](.github/workflows/build.yml) for the current toolchain and artifacts. Native UI captures use the [Windows window test](src/ui/live_window_test.rs) with an actual Android guest.
- BlueStacks 5 on Windows opened SuperTuxKart in Pie64. Independent QEMU reached the original Linux 4.19.195 kernel. Android userspace, graphics and games on that copied image remain unverified. [Device list and boot experiments](docs/bluestacks-lab.md)
- In the Android 17 Google Play profile, the package manager retained the version of Home after rejecting an incomplete APK during an earlier shutdown. Check the installed package path before trusting the version, reinstall in place when necessary, and flush guest writes before shutting down.
- The Windows live run verified both Home boots in each locale, the editor in SuperTuxKart and capture of PNG, H.264 MP4, animated JPEG XL and GIF. JPEG XL and GIF decoded to 30 frames at 10 FPS. The opt-in Android input test observed swipe and simultaneous opposing pinch contacts through the virtual touchscreen. [Live input test](src/display/live_input_test.rs)
- The Windows launcher and the native test harness share one window factory. Set both Win32 icon sizes from group resource 1. The real-window regression test checks the handles Windows returns for the caption and taskbar.
- The animated mascot uses the original logo. Guest Home v3 animates only a visible, focused empty state. The desktop animates boot, shutdown and About. The window and taskbar icons use the static brand resource. Android 17 offscreen rendering verified portrait, landscape, compact height and both directions of the step while a different app stayed in the foreground.
