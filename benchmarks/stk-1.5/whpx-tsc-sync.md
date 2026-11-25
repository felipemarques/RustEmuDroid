# Experimental WHPX TSC synchronization on Windows

The official Android 17 SDK test guest selected HPET after reporting a cross-vCPU TSC synchronization failure. The Kemu guest selected TSC. The [QA patch](whpx-tsc-sync.patch) isolates one potential cause on the SDK QEMU code. It applies to AOSP commit `538bcdea5479b3857cd8885cbb9306bfec9c7247`, whose snapshot message records official Android Emulator build **16970787** (37.3.10). [Clock selection evidence](windows-guest-clocksource.md) · [AOSP snapshot](https://android.googlesource.com/platform/external/qemu/+/538bcdea5479b3857cd8885cbb9306bfec9c7247)

## What the patch changes

Set `RUSTEMUDROID_QA_WHPX_SYNC_TSC=1` in the private candidate process environment. Generic register writes then omit TSC and retain the other registers in the same order. At the end of each all-vCPU reset or initialization, the patch checks the QEMU iothread lock and every VP's stopped state. It suspends partition time once and writes each VP's own TSC value before any VP resumes. The existing reads and snapshot state fields stay in place. CPUID, APIC, the Windows driver and the Android image remain the same. [Microsoft WHPX suspension contract](https://learn.microsoft.com/en-us/virtualization/api/hypervisor-platform/funcs/whvsuspendpartitiontime)

The matching control uses the same rebuilt binary with the environment variable absent. Keep the experiment on cold boots with `-no-snapshot` and a fixed four-vCPU profile. Snapshot restore and topology changes remain outside runtime validation. Reject a candidate whose log lacks the four-vCPU synchronization marker or reports an API, pause, or write failure.

On an x86 CPU reset in this revision, QEMU clears the part of `CPUX86State` that contains `tsc`. Each newly reset VP can therefore have the same saved counter value even when the host programs the VPs at different times. A late write of that value can leave one VP behind if the partition clock advances between writes. The public Windows probe below tests this mechanism with deliberate delays. [x86 reset](https://android.googlesource.com/platform/external/qemu/+/538bcdea5479b3857cd8885cbb9306bfec9c7247/target/i386/cpu.c#3840) · [TSC field and reset boundary](https://android.googlesource.com/platform/external/qemu/+/538bcdea5479b3857cd8885cbb9306bfec9c7247/target/i386/cpu.h#1153)

## Compile and verify

Apply the patch in an isolated checkout at the pinned commit.

```sh
git apply --check --whitespace=error-all /path/to/RustEmuDroid/benchmarks/stk-1.5/whpx-tsc-sync.patch
git apply /path/to/RustEmuDroid/benchmarks/stk-1.5/whpx-tsc-sync.patch
```

The two real QEMU translation units compiled to Windows AMD64 COFF using Clang in MSVC target mode, Windows SDK 10.0.26100.0, the checked-in Windows/x86-64 QEMU configuration, upstream QAPI/trace generation and public GLib and AEMU headers. The WHPX object defines `whpx_qa_synchronize_tsc`. The CPU object references it from the post-init and post-reset paths. A separate host test compiled the actual batch function with recording callbacks in place of WHPX calls. It verified one suspension before four ordered writes, the saved per-VP values, the disabled path and failure when a VP is running, the iothread lock is absent or an API call fails. This test exercises the control flow without running a guest.

| Artifact | SHA-256 |
| --- | --- |
| `whpx-tsc-sync.patch` | `8b81a6900924fb9f434ef1cab81580f00c804d0a94479971bd0bb41e2f532eee` |
| `whpx-all.obj` | `982365cade7e3d74e62367b7af3bc8b6c6fcefa2ef7413cd99841fb3631f621d` |
| `cpus.obj` | `d8292883d20b87dc80682a402ed9d682299a218c9a73174cb47cd60aaa0a35e5` |

The object check used GLib from `qemu-android-deps` revision `b06aee625002d0738c5576bbf437b9378229bef0` and the Windows compatibility headers from AEMU revision `36815ef31763c79dfc42b461e6ce35264ead08c4`. A runnable build also requires the source and prebuilts from the emulator host manifest. Record the exact dependency revisions used. The build snapshot identifies QEMU's source while the official installation supplies no complete dependency lock. A rebuild can provide an A/B comparison within its own binary. [Upstream Windows build guide](https://android.googlesource.com/platform/external/qemu/+/538bcdea5479b3857cd8885cbb9306bfec9c7247/android/docs/WINDOWS-DEV.md)

### Reproduce the Windows source checkout

The [Windows build manifest](whpx-windows-build-manifest.xml) pins all 60 AOSP projects from the local build checkout. All 60 revisions were checked against their actual Git HEADs. The standalone XML uses the absolute AOSP fetch URL and has SHA-256 `f3b9420ea72b0161e45595096752b4cbefa7c031e9d4f59128b239e95d762f65`. These were the dependencies used for this rebuild. Google identifies the official QEMU snapshot but has not supplied the complete dependency lock for the shipped package here.

Open a Visual Studio x64 developer shell in an empty directory with a short Windows path. The Android Repo tool, Git, Python, Windows SDK and C++ toolchain must be available. Supply a file URL to your downloaded copy of the XML or the raw GitHub URL. A separate local `repo init` and `repo list --all --path` with the standalone XML recovered the same 60 project paths. The full source checkout and prebuilts used about 54 GiB of free space on this PC before compilation.

```powershell
repo init --standalone-manifest -u https://raw.githubusercontent.com/felipemarques/RustEmuDroid/main/benchmarks/stk-1.5/whpx-windows-build-manifest.xml -g default,platform-windows --depth=1 --partial-clone --clone-filter=blob:none
repo sync -j2 --no-tags --current-branch --optimized-fetch
```

Apply the patch in `external/qemu` as described above. From the checkout root, these are the configuration and target used for the Windows test build. Launch both steps from the x64 developer shell so the compiler can find the UCRT headers.

```powershell
$buildDir = [IO.Path]::GetFullPath((Join-Path (Get-Location).Path '..\build'))
python external/qemu/android/build/python/cmake.py --task Configure --target windows --out $buildDir --ccache none --crash none --sdk_build_number qa-whpx-16970787 --cmake_option OPTION_MINBUILD=ON --cmake_option BUILD_TESTING=OFF
& ./prebuilts/cmake/windows-x86/bin/cmake.exe --build $buildDir --parallel 2 --target qemu-system-x86_64
```

## What ran on the Windows hypervisor

A separate test on four real WHP virtual processors wrote identical TSC values at staggered times. With time advancing during writes, counter spread tracked the 92 ms and 96 ms write spans. Suspending partition time for the writes bounded the observed spread below 53 µs in both explicit-resume runs and a run that resumed time by executing the first VP. These bounds reflect the measurement resolution. [Probe source, measurements and reproduction](whpx-clock-probe/README.md)

The Android test still needs two cold boots of the same rebuilt QEMU on the same image with the option off and on. Check the Android-selected clocksource, cross-vCPU offsets and elapsed time relative to the host before repeating the kart replay. The Windows experiment establishes the behavior of the hypervisor API on this PC. Android boot behavior and game FPS after the change are still open. [Android probe and method](windows-guest-clock-alignment.md)
