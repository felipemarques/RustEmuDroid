# Android 17 Google Play on Linux

The Linux x86-64 executable from the [v0.7.0 release](https://github.com/felipemarques/RustEmuDroid/releases/tag/v0.7.0) booted the Android 17 Google Play image in the integrated desktop window on Ubuntu 24.04 under WSL2 and WSLg. The test used the Linux Android Emulator 37.3.10, KVM and a dedicated AVD. Android reported version 17, API 37 and the `x86_64` ABI. The package manager found Google Play services (`com.google.android.gms`) and the Play Store (`com.android.vending`). The test used no Google account.

| Startup | Time reported by RustEmuDroid to `sys.boot_completed=1` |
|---|---:|
| First boot of the new AVD | 166.8 s |
| Next startup of the same AVD | 29.4 s |

Both times precede any check that a game can draw or respond to input. This setup stored the AVD on a Windows volume mounted in WSL. Each time represents a single observation on that configuration.

The guest used Google's SwiftShader software renderer. The Ubuntu Vulkan loader exposed only the CPU device `llvmpipe (LLVM 20.1.2, 256 bits)`. These results cover Android startup with KVM and software graphics. Hardware-accelerated game FPS on native Linux remains unmeasured.

## Home and game input

Pressing Android Home brought the bundled RustEmuDroid Home to the foreground. Android also resolved the default Home action to `org.rustemudroid.home/.HomeActivity`. The Games tab showed BombSquad after installation.

BombSquad 1.8.10 (build 23095, x86-64) displayed its main menu at 1600 × 900. A touch sent through ADB to **Play** opened the three game modes pictured below. The test ended at mode selection. It did not measure a match or the desktop input latency. [Test APK from the publisher](https://ballistica.net/downloads)

![BombSquad on the Linux Android guest after opening Play, showing the single player, teams and free-for-all choices.](linux-bombsquad.webp)

## Repeat the boot

Extract the Linux release, install the Linux Android SDK components shown in the [setup guide](../README.md#prepare-the-android-runtime), and verify that your Linux user can access `/dev/kvm`. From the directory containing the extracted executable, run the following commands. Set `sdk` to your Linux SDK path and choose an unused directory for `state` for a fresh boot.

```sh
sdk="$HOME/Android/Sdk"
state="$HOME/rustemudroid-play17-test"
./rustemudroid --sdk "$sdk" --state-dir "$state" --lang en-US init --image 'system-images;android-37.0;google_apis_playstore;x86_64'
./rustemudroid --sdk "$sdk" --state-dir "$state" --lang en-US ui --gpu swiftshader
```

Use the serial shown in the boot output with the `adb` from that SDK. `emu avd path` identifies the running profile before the remaining commands query it.

```sh
serial=emulator-5554  # Replace with your session's serial.
adb="$sdk/platform-tools/adb"
"$adb" -s "$serial" emu avd path
"$adb" -s "$serial" shell getprop sys.boot_completed
"$adb" -s "$serial" shell getprop ro.build.version.sdk
"$adb" -s "$serial" shell pm path com.google.android.gms
"$adb" -s "$serial" shell pm path com.android.vending
"$adb" -s "$serial" shell dumpsys SurfaceFlinger | grep 'GLES:'
```
