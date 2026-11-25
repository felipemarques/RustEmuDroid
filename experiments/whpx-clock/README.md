# Windows WHPX clock lab

[Guia de uso em português](README.pt-BR.md)

This lab builds the experimental WHPX clock fix from pinned Android Emulator source and stages it beside a locally obtained official runtime. The 0.21.1 panel benchmark used Android 17 Google Play, four vCPUs, WHPX, an RTX 5070 Ti, and cold boots. The environment option changed whether Linux selected HPET or synchronized TSC. The experiment is scoped to the tested profile. [Patch and clock evidence](../../benchmarks/stk-1.5/whpx-tsc-sync.md) [Game measurements](../../benchmarks/stk-1.5/windows-whpx-clock-kart.md)

## 1. Get the host tools and source

Use Windows x64, a Python 3.11 or newer interpreter, Git for Windows, the Android Repo launcher and a Visual Studio 2022 x64 developer shell with the MSVC C++ tools, ATL, the Windows SDK and the Visual Studio DIA SDK. Repo needs the Git and GPG signing tools and the ability to create source symlinks. The pinned AOSP checkout and prebuilts used about 54 GiB on the test PC before compilation. Choose a short private workspace with at least 80 GiB free. The procedure uses only temporary shell variables and the paths supplied below. [Android Repo setup](https://source.android.com/docs/setup/download/downloading) [AOSP Windows build](https://android.googlesource.com/platform/external/qemu/+/538bcdea5479b3857cd8885cbb9306bfec9c7247/android/docs/WINDOWS-DEV.md)

Set `$Repo` to your official local Repo launcher. In the Windows x64 developer shell, start at the root of the RustEmuDroid clone and use a new directory for `$Lab`. The standalone manifest pins the 60 AOSP projects used in our build.

```powershell
$Project = (Get-Location).Path
$Lab = 'C:\whpx-clock-lab'
$Source = Join-Path $Lab 'src'
$Build = Join-Path $Lab 'build'
$Repo = 'C:\Tools\git-repo\repo'
$Stage = Join-Path $Project 'experiments\whpx-clock\stage.py'
$Lock = Join-Path $Project 'benchmarks\stk-1.5\whpx-windows-build-manifest.xml'
$LockUri = [uri]::new($Lock).AbsoluteUri

New-Item -ItemType Directory -Path $Lab,$Source | Out-Null
$env:REPO_CONFIG_DIR = Join-Path $Lab 'repo-config'
$env:GIT_CONFIG_COUNT = '3'
$env:GIT_CONFIG_KEY_0 = 'core.autocrlf'
$env:GIT_CONFIG_VALUE_0 = 'false'
$env:GIT_CONFIG_KEY_1 = 'core.longpaths'
$env:GIT_CONFIG_VALUE_1 = 'true'
$env:GIT_CONFIG_KEY_2 = 'core.symlinks'
$env:GIT_CONFIG_VALUE_2 = 'true'

Push-Location $Source
try {
    python -B $Repo init --standalone-manifest -u $LockUri -g default,platform-windows --depth=1 --partial-clone --clone-filter=blob:none
    if ($LASTEXITCODE -ne 0) { throw 'Repo setup failed' }
    python -B $Repo sync -j2 --no-tags --current-branch --optimized-fetch
    if ($LASTEXITCODE -ne 0) { throw 'Source sync failed' }
} finally {
    Pop-Location
}

python -B $Stage build --source $Source --out $Build
```

Use `build --variant cache --source $Source --out $Build` in place of the last command to include the cached Vulkan option. The default variant is `clock`. The `cache` variant adds the pinned `opengles.cpp` bridge patch to the same QEMU source revision. Both options remain off unless a selected launch enables one of them. Build from the clean source checkout once per chosen variant. [Cached Vulkan diagnostic and patch](../../benchmarks/stk-1.5/windows-vulkan-mapped-writes.md)

The build uses the pinned Clang, native Windows SDK/MSVC tooling, CMake and two Ninja jobs. It verifies the 60 Git revisions and tracked sources, applies the published TSC patch to the dedicated QEMU checkout, and verifies the three modified file contents against a temporary Git index made from the base and patch. The source stays patched in this workspace. On an interrupted build, fix the reported toolchain error and repeat with `build --resume --source $Source --out $Build` in the same x64 developer shell. Configure and Ninja output go to `configure.log` and `build.log` in the private build directory. Keep the checkout and outputs in place for staging.

The completed build writes `build-receipt.json`. It records the source revisions, patched blobs, configure/link and Ninja logs, CMake configuration, and the sizes and SHA-256 digests of the allowlisted AMD64 PE modules. It checks the actual QEMU marker and checks the requested patch variant, including the cache markers when building `cache`. Staging rechecks those inputs. This recipe identifies a locally controlled build from its source and actual outputs. Toolchain servicing, build directories and linked debug/PDB data can change the resulting hashes. A local receipt is evidence from a build you control. Treat a binary and receipt supplied by someone else as untrusted.

## 2. Create the private runtime

If you have the reference cache binary kit, you can skip the compiler and source checkout. Use Python 3.11 or newer from the project folder and supply the stock Windows runtime described below. Choose a new destination folder. The stager verifies the ZIP modules against the project's SHA-256 list and copies the separately installed emulator tools. The Android image and games continue to use their usual locations.

```powershell
python .\experiments\whpx-clock\stage.py stage --overlay-zip 'D:\Downloads\whpx-cache-windows-x86_64.zip' --stock-sdk "$env:LOCALAPPDATA\Android\Sdk" --out 'D:\Android\CacheEngine' --smoke
```

For a RustEmuDroid-managed installation of those tool versions, set `--stock-sdk` to `"$env:LOCALAPPDATA\RustEmuDroid\runtime"`. After the three checks pass, open the v0.22.0 or newer desktop client and select the new folder in the profile's [Vulkan cache setting](../../README.md#choose-a-profile).

Obtain Google's x64 Android Emulator 37.3.10 build 16970787 and Platform Tools 37.0.0 separately. Extract them so the stock directory contains `emulator/` and `platform-tools/`. Google's catalog gives the emulator archive size as 456,613,348 bytes and SHA-1 as `7599293267e61ce67423f87848ff8219da09258b`. The stager separately verifies the stock launcher, original core, gfxstream renderer and ADB by SHA-256. [Google release notes](https://developer.android.com/studio/releases/emulator) [Emulator ZIP](https://dl.google.com/android/repository/emulator-windows_x64-16970787.zip) [Platform Tools ZIP](https://dl.google.com/android/repository/platform-tools_r37.0.0-win.zip)

```powershell
$Stock = Join-Path $Lab 'stock'
$Out = Join-Path $Lab 'tsc-lab'
python -B $Stage check --stock-sdk $Stock --artifacts $Build
python -B $Stage stage --stock-sdk $Stock --artifacts $Build --out $Out --smoke
python -B (Join-Path $Project 'experiments\whpx-clock\test_stage.py')
```

The default stages 20 destinations from 12 linked files, including the same bytes at both layout paths where the original staging preserved them. The manifest has the full 21-path historical comparison including the separately obtained Visual Studio DIA DLL. The stock launcher, Qt, gfxstream, ANGLE, firmware and ADB remain the user's local Google components. Windows supplies WHPX. The two stock tool subdirectories are copied into a fresh output. The installer neither downloads nor copies any Android image, AVD, SDK license state, account or credentials. [Overlay manifest](overlay-manifest.json)

The build uses DIA headers and `diaguids.lib` through its build tools. Static PE imports of the staged launcher, core, renderer and rebuilt companions contain no `msdia140.dll` reference. A local DIA DLL can still be supplied with `--dia <Visual Studio DIA SDK/bin/amd64/msdia140.dll>` if you need the historical layout or are testing diagnostics. It must match the DIA file identified by the local build or by the archival pin in reference mode. Preserve the built `liblibprotoc.dll` and original duplicate destination paths until a reduced runtime has completed separate boot and panel checks.

`--strict-reference` skips local source attestation and instead requires the exact archived host binaries recorded in the manifest. It is for checking a copy of the historical experiment. It never retrieves those binaries. Select the reference cache build with `--strict-reference --variant cache`; with the switch absent, `clock` is the reference. The reference cache build differs in the core and `libemugl_common.dll`, whose root and `lib64` files contain the same bytes. Staging a local build automatically reads its variant from `build-receipt.json`; pass `--variant` to assert the expected variant. Supply the archived-match DIA file for the exact 21-path layout. A fresh clone and an independent build use the default path without that switch.

Before publishing the new directory locally, staging verifies the copied files in a temporary sibling. An interrupted copy leaves no accepted output. `--smoke` runs `-version` on the staged launcher and core and `-accel-check` on the staged launcher in a private environment with both experimental options absent. The smoke runs in the final directory because Windows can retain handles to a tool directory after the executable exits. A failed smoke keeps the isolated output with `staging-error.json`. A successful stage writes `<out>/staging.json`, which records the two final executable hashes, next to `<out>/sdk`. The original SDK and user data are read-only inputs. You can use another output label for a retry. These commands do not start Android.

The offline test suite uses tiny Git repositories and synthetic PE files. It covers different local output hashes, an incorrect patch, tampered outputs, variant mismatch, protected destination paths, interrupted copying and loader-error receipts. It does not compile QEMU. On the Windows test PC, the actual archived `clock` and `cache` runtime overlays were each staged with 20 destinations and no local DIA DLL. All three checks passed for each stage. A separate QEMU-only copy of the `clock` core also passed the same checks against stock companion DLLs. The Windows host had the Visual Studio DIA SDK installed for the earlier build. The reduced layouts were tested through loader and WHPX checks. The gameplay evidence used the full matched overlay, and the local-build recipe has not been rebuilt from scratch during this staging pass.

## 3. Try the clock option on a disposable Android profile

Obtain and prepare a separate Google-provided system image. The measured image was Android 17 Google Play x86-64 revision 7. Keep the image and new AVD outside the stock and staged SDK tools. For a RustEmuDroid 0.21.1 or newer executable, an isolated profile can be initialized with `--sdk <out/sdk> --state-dir <profile> init --system-dir <local Android image>`. Use a disposable profile containing `avd/` and keep only one VM open. Leave snapshots disabled. The result depends on which clock Linux actually selects.

The [launcher runner](run.py) checks the staged SDK receipt and the selected existing profile. `check` verifies the inputs. `off` starts the control. `on` starts with the selected option in that process environment. The default is `--fix clock`. To test cached Vulkan allocations, use the staged `cache` variant and add `--fix vulkan-cache` to each runner command. The runner enables one option per test. The runner uses temporary settings. Use the same four-vCPU profile, game build, resolution, renderer and refresh rate in the two arms. Close the active test session before starting the other arm.

```powershell
$Runner = Join-Path $Project 'experiments\whpx-clock\run.py'
$App = 'C:\Lab\rustemudroid.exe'
$Profile = 'C:\Lab\disposable-state'
$Sdk = Join-Path $Out 'sdk'
python -B $Runner check --app $App --sdk $Sdk --profile $Profile --hz 90 --gpu-device 'NVIDIA GeForce RTX 5070 Ti' --lang en-us
python -B $Runner off --app $App --sdk $Sdk --profile $Profile --hz 90 --gpu-device 'NVIDIA GeForce RTX 5070 Ti' --lang en-us
python -B $Runner on --app $App --sdk $Sdk --profile $Profile --hz 90 --gpu-device 'NVIDIA GeForce RTX 5070 Ti' --lang en-us
```

The runner checks Android boot completion and the selected engine markers after the window closes. [Game measurements](../../benchmarks/stk-1.5/windows-cached-memory-kart.md)

The two conditions need the same executable. For clock comparisons, confirm the four-vCPU synchronization markers and stable guest TSC in the enabled boot. The normal desktop archives and release workflow are separate from these lab tools. [Desktop release guide](../../docs/releases.md)
