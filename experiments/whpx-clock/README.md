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

The Windows kit rebuilt with FFmpeg 4.4.2 and libvpx 1.12.0 has ZIP SHA-256 `a67d12ec5abbb220cce81ecf623174d278fab08dd5d0a08441be51c642395146`. To verify the download and its modules before choosing a destination, replace `stage` after `stage.py` with `check` and omit `--out` and `--smoke`. A successful check prints the ZIP digest and the expected **20** installation paths. Staging records the digest in `staging.json`. To switch back after testing, select **Use default engine** in the same profile, save and restart Android.

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

The temporary sibling is created with the parent folder's inherited permissions, so a folder staged from an elevated prompt stays readable by the normal launcher. Folders staged with Python 3.12.4 or newer by an earlier version of this tool from an elevated prompt kept a private access list. Reset them with `icacls <folder> /reset /T` from an elevated prompt.

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

## 4. Build the performance engine

The `performance` variant rebuilds the QEMU core and the gfxstream renderer with the changes used for the November 26 uncapped Mailbox measurements, plus the later prompt proof lines, three options that are off by default and the hypervisor local APIC option (see the build table below). On the RTX 5070 Ti, with the earlier build, the recipe below measured 356 game FPS with the Windows Balanced power mode and 375 with Best performance, against 164–176 with the stock engine. [Measurements, recipe and limits](../../docs/performance-history.md#performance-engine-in-uncapped-mailbox)

### Sources

| Project | Pinned upstream commit | Patches, applied in order |
|---|---|---|
| `external/qemu` | `538bcdea5479b3857cd8885cbb9306bfec9c7247` | [whpx-tsc-sync.patch](../../benchmarks/stk-1.5/whpx-tsc-sync.patch), [gfxstream-cached-host-qa.patch](../../benchmarks/stk-1.5/gfxstream-cached-host-qa.patch), [performance-engine-qemu.patch](../../benchmarks/stk-1.5/performance-engine-qemu.patch) |
| `hardware/google/gfxstream` | `501867e6a3b1a3be7b935cc977d291150846edab` | [performance-engine-gfxstream.patch](../../benchmarks/stk-1.5/performance-engine-gfxstream.patch) |

The QEMU patch is layered on the two published clock and cache patches, which stay byte-identical for the `clock` and `cache` variants. The other 58 projects of the [AOSP lock](../../benchmarks/stk-1.5/whpx-windows-build-manifest.xml) stay unmodified. [performance-engine.json](performance-engine.json) records the patch digests, the Git blob of every patched file, the gates and the module hashes of two builds made on the test PC. A local build of the same sources normally produces different module hashes.

| Build | QEMU `qemu-system-x86_64.exe` | gfxstream `libgfxstream_backend.dll` | Sources |
|---|---|---|---|
| Published sources | `fc990b45532c16008327a15e07a15297963716d7dadb9e46ecab4acf3c1cb94f` | `7d01cde7d9450e050effb4805c87dcf4c0cfaa274a290ac8f526f192eac085fe` | Pinned commits plus exactly these patches. Used, with and without `RUSTEMUDROID_QA_WHPX_APIC=hv`, for the daytime hypervisor APIC series. |
| v0.23.0 kit | `1c7a05e30e6d2f21a8d716e18cab7181c2e3a3e25756daa2137123b2502ea0c6` | `7d01cde7d9450e050effb4805c87dcf4c0cfaa274a290ac8f526f192eac085fe` | The same sources without R (below). Used for the daytime client and operator path series. |
| Night measurements | `a55d48c6d412693caebd7664744fd9f2f0d85d2a693e203c1d150a25a30b37f5` | `d92f6060d3dc054c54000ba5231b62bbcbcd61d6e1c08820c98ea4f2cc8360d0` | The same sources without P0, L2, P, Q, A3 and R. Used for the night figures (356 and 375 FPS) and the Cipó Livre series. |

The later builds differ from the night build in one default-on change and in changes that are off unless their variable is set:

- **P0 (default on).** QEMU on Windows fully buffers stderr after `os_set_line_buffering()`, so one-time lines printed later in start-up reached the log only when the process exited, and the client could not confirm them during the session. Each RustEmuDroid proof and diagnostic line of QEMU is now formatted as one line, written with one call and flushed immediately through new helpers in `util/qemu-error.c`. The text of the lines is unchanged.
- **L2** `RUSTEMUDROID_QA_RAM_SECTION` (QEMU): maps guest RAM from a named large-page section created by a separate privileged helper, which this repository does not ship. Never measured.
- **P** `RUSTEMUDROID_QA_ASG_SPIN_RENDERING=1` (gfxstream, needs `RUSTEMUDROID_QA_ASG_SPIN_US`): the host reports itself as rendering while it spins, so the guest skips its wake-up ping after each command commit. The measurement so far is inconclusive.
- **Q** `RUSTEMUDROID_QA_MMIO_FASTPATH=1` (QEMU): extends the LAPIC MOV fast path to every MMIO page. Not measured.
- **A3** (QEMU): extra per-vCPU timing statistics under the existing `RUSTEMUDROID_QA_WHPX_EXIT_STATS` gate.
- **R** `RUSTEMUDROID_QA_WHPX_APIC=hv` (QEMU, published-source build only): the Windows hypervisor emulates each vCPU's local APIC (xAPIC) instead of QEMU's user-mode APIC. The engine requires the hypervisor's local APIC emulation, the INIT/SIPI exit and `WHvRequestInterrupt`; MSIs and I/O APIC interrupts are delivered through `WHvRequestInterrupt`, while the I/O APIC, PIC, PIT and HPET stay in QEMU. x2APIC, TSC-deadline and MONITOR are hidden from the guest. It forces `RUSTEMUDROID_QA_KVM_PV_EOI` off, so the guest no longer reports KVM, and the LAPIC fast path has nothing left to handle. Proof line: `RustEmuDroid QA: WHPX local APIC emulated by the hypervisor (xAPIC)`. When the host cannot provide it, the engine prints `RustEmuDroid QA: WHPX hypervisor local APIC unavailable (<reason>, hr=…), using the user-mode APIC` and continues with the user-mode APIC. In one daytime series it raised SuperTuxKart from 345.2 to 380.1 FPS. Tested with cold boots of one image on one AMD PC only; snapshots, guest reboots, injected NMIs and x2APIC are untested. [Measurements](../../docs/performance-history.md#hypervisor-local-apic)

P and Q have no client capability and the client never enables them. R is the client capability `whpx_apic_hv` (v0.24.0).

The patches contain every change compiled into the published build. Apart from P0, each change is behind its own environment variable and inactive when the variable is unset. The client and the recipe use:

| Variable | Module | Value | Effect |
|---|---|---|---|
| `RUSTEMUDROID_QA_WHPX_SYNC_TSC` | QEMU | `1` | Synchronizes the guest TSC across vCPUs |
| `RUSTEMUDROID_QA_VULKAN_CACHED_HOST` | QEMU | `1` | Backs uncached host-visible Vulkan memory with cached memory |
| `RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL` | gfxstream | `2` | Allocates mappable device-local Vulkan memory from cached host memory, except dedicated image allocations |
| `RUSTEMUDROID_QA_ASG_SPIN_US` | gfxstream | `3000` | Keeps the shared-memory graphics consumer polling for up to this many microseconds before it sleeps. Needs `hw.gltransport=asg` in the AVD. |
| `RUSTEMUDROID_QA_VCPU_PLACEMENT` | QEMU | `sets-high` | Gives each vCPU its own physical core at the top of the core list and keeps the other engine threads on the remaining cores. Needs at least vCPUs + 2 physical cores. |
| `RUSTEMUDROID_QA_LAPIC_FASTPATH` | QEMU | `1` | Handles 32-bit MOV accesses to the local APIC page without the generic instruction emulator |
| `RUSTEMUDROID_QA_KVM_PV_EOI` | QEMU | `1` | Offers the KVM paravirtual end-of-interrupt protocol. The guest then reports KVM as its hypervisor and x2APIC is hidden. |
| `RUSTEMUDROID_QA_WHPX_APIC` | QEMU | `hv` | Hypervisor-emulated local APIC (R above). Requested by the v0.24.0 client together with the CPU virtualization switch when the manifest lists `whpx_apic_hv`. Overrides the two lines above. |

These are also present and were tested off or rejected: `RUSTEMUDROID_QA_VCPU_CORES`, `RUSTEMUDROID_QA_VCPU_PRIORITY`, `RUSTEMUDROID_QA_PRERUN_LOCKLESS`, `RUSTEMUDROID_QA_HALT_POLL_US`, `RUSTEMUDROID_QA_WHPX_EXIT_STATS` (diagnostic exit counters), `RUSTEMUDROID_QA_PIPE_RECV_WAIT_US`, `RUSTEMUDROID_QA_PIPE_LOCKLESS`, `RUSTEMUDROID_QA_RAM_LARGE_PAGES` (QEMU), `RUSTEMUDROID_QA_RENDER_SPIN_US` and `RUSTEMUDROID_QA_READBACK_UNLOCK` (gfxstream). `PIPE_LOCKLESS` and `RAM_LARGE_PAGES` were compiled but never enabled in a measured boot. The v0.23.0 client removes every inherited `RUSTEMUDROID_QA_*` variable before it starts this engine. The recipe also uses the stock `-feature VulkanNativeSwapchain` and, optionally, the AVD key `kernel.parameters=mitigations=off init_on_alloc=0 transparent_hugepage=always`.

### Build and stage

Prepare the same checkout as in section 1, then build once from the clean source tree:

```powershell
python -B $Stage build --variant performance --source $Source --out $Build
$Perf = Join-Path $Lab 'performance-engine'
python -B $Stage stage --stock-sdk $Stock --artifacts $Build --out $Perf --smoke
```

The build applies the three QEMU patches in order and the gfxstream patch, checks the resulting files against the blobs in `performance-engine.json`, and builds the `qemu-system-x86_64` and `gfxstream_backend` targets. The receipt and staging checks also require each gate variable in the built core and renderer. Staging adds `lib64/libgfxstream_backend.dll` to the 20 layout paths. `--strict-reference` and `--overlay-zip` do not apply to this variant, because no archived reference binaries are published. These source and staging checks run in the offline test suite with synthetic projects; a complete build with this command has not yet been repeated from a fresh checkout.

### Write the client manifest

The v0.23.0 client accepts a folder that holds `rustemudroid-engine.json` next to `sdk`. [engine_manifest.py](engine_manifest.py) writes it from the staged files. `evidence` reports which known capabilities have their marker strings in the binaries. `write` hashes the launcher, core and renderer and lists only the capabilities you name, after checking each one.

```powershell
$Manifest = Join-Path $Project 'experiments\whpx-clock\engine_manifest.py'
python -B $Manifest evidence --engine $Perf
python -B $Manifest write --engine $Perf `
    --capability tsc_sync --capability vulkan_cached_host --capability vulkan_cached_device_local_v2 `
    --capability asg_spin --capability vulkan_native_swapchain --capability vcpu_placement_high `
    --capability lapic_fastpath --capability kvm_pv_eoi --capability guest_kernel_parameters `
    --capability whpx_apic_hv `
    --extra emulator/lib64/libemugl_common.dll --build-id 'qa-whpx-16970787 local'
```

A capability is refused when its gate variable or proof line is missing from the module that implements it. `vulkan_native_swapchain` and `guest_kernel_parameters` are stock emulator features, so their check only shows that the engine knows the feature name. The first word of `--build-id` must occur in the core. `--extra` adds the SHA-256 of another file under `emulator/`; the client then rejects the engine if that file changes. Then open **Settings → Runtime, image, and profile → Performance engine (experimental)** in v0.23.0, choose the folder and restart Android.

### Licenses and corresponding source

QEMU is licensed under GPL-2.0. The QEMU patches are distributed under the same license. A rebuilt `qemu-system-x86_64.exe` is identified by the pinned `external/qemu` commit above plus the three QEMU patches in order, built with the AOSP lock and the commands in this guide. Anyone who distributes such a binary must offer that corresponding source under the GPL. gfxstream is licensed under Apache-2.0, and the gfxstream patch is distributed under Apache-2.0. Two QEMU changes follow ideas from Kemu commits `21a3456b69` (exit statistics) and `96f854ad13` (vCPU placement), both GPL-2.0-or-later; the code was written for this tree and the source comments name the origin. This repository publishes sources and patches only, no engine binaries.
