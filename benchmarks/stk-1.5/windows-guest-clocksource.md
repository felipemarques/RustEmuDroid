# Guest clock selection in two Windows WHPX runtimes

The Android 17 Google Play guest selected **HPET** after its kernel found a vCPU TSC synchronization failure. The Kemu Android 16 guest switched from **tsc-early** to **TSC**. Both cold diagnostic boots used WHPX, four guest vCPUs and the same host. Both kernel command lines requested `clocksource=pit`. The SDK's observed last switch was already HPET, so the diagnostic stopped without testing an empty PIT override.

| Guest and engine | Clocksource switches (seconds into boot) | TSC calibration or rejection |
| --- | --- | --- |
| Android 17 SDK 37.3.10 | hpet (0.920525 s) | Cross-vCPU warp of 6,195,110,760 cycles. The kernel marked TSC unstable after the synchronization check. |
| Kemu Android 16 v0.7.1 | tsc-early (0.585476 s) → tsc (1.651779 s) | PIT calibration matched HPET. TSC refined to 3992.588 MHz. |

Both guests separately reported CPUID leaf `0x80000007` EDX=`0x100` and the Linux `constant_tsc` and `nonstop_tsc` flags on all four vCPUs. The invariant bit addresses clock frequency. The SDK guest still found a cross-vCPU synchronization failure. Neither kernel included the native Hyper-V reference-clock driver. The documented emulator `-show-kernel` console exposed the actual Linux selections without privileged guest reads. [Emulator console logging](https://developer.android.com/studio/run/emulator-commandline) · [Linux Hyper-V clock mechanisms](https://docs.kernel.org/6.10/virt/hyperv/clocks.html)

| Same x86-64 ELF, median elapsed µs/call | SDK Android 17 | Kemu Android 16 |
| --- | ---: | ---: |
| `libc_clock_monotonic` | 8.5568 | 0.0176 |
| `libc_gettimeofday` | 8.7752 | 0.0180 |
| `syscall_gettimeofday` | 8.4813 | 0.2473 |

The earlier microbenchmark used six batches of 20,000 calls per method per guest. The much faster Kemu libc calls relative to its explicit syscall fit the x86-64 vDSO TSC path. The SDK libc and syscall costs were close, consistent with the costly HPET fallback. These are separate boot and microbenchmark observations. They establish the clocksource outcome for the tested engines and images, with no game FPS attribution. [Bionic clock dispatch](https://android.googlesource.com/platform/bionic/+/8880cab65/libc/bionic/vdso.cpp)

Kemu 0.7.1's published WHPX code suspends partition time while setting a vCPU TSC and takes that path during post-init and post-reset. Ordinary register synchronization leaves TSC out. This gives a candidate explanation for the different synchronization outcome. The engines and guest kernels differ in the measured pair, so isolating this mechanism would require an engine-only change on the same image. [Kemu TSC synchronization](https://github.com/kuloPo/Kemu-emulator_external_qemu/blob/1622371aee57f26f5446ca42ec32bd4faddfc440/target/i386/whpx-all.c#L799-L852) · [Kemu post-init and reset hooks](https://github.com/kuloPo/Kemu-emulator_external_qemu/blob/1622371aee57f26f5446ca42ec32bd4faddfc440/target/i386/whpx-all.c#L2561-L2573)

[Sanitized data and evidence hashes](windows-guest-clocksource.json)

Post-boot measurements separately read the counters on each vCPU and bound their offsets using alternating migration order. They found a persistent difference of about 1.877 s on Android 17 and a sub-millisecond bound on Kemu. [Clock alignment and host-time checks](windows-guest-clock-alignment.md)

## Android Emulator 37.4.2 on the same Android 17 image

On November 25, 2026, the Windows package for **37.4.2 (build 16982698)** from Google's preview channel booted the same four-vCPU Android 17 Google Play r7 test AVD on the RTX 5070 Ti. The package ran from a separate directory. Both boots reported the same Android fingerprint. The newer engine again recorded a synchronization failure between vCPUs 0 and 1 and selected **HPET at 1.529 s** after measuring a TSC skew of 8,427,190,359 cycles. This reproduces the fallback seen in stable 37.3.10 (build 16970787). Each build contributed one diagnostic cold boot. Skew magnitude can vary with startup scheduling. [Google package catalog](https://dl.google.com/android/repository/repository2-3.xml) · [Package digest, fingerprint and kernel timestamps](windows-guest-clocksource.json)

On the preview build, median guest **thread CPU** time per call was 9.24 µs for libc `gettimeofday`, 9.79 µs for the explicit syscall and 10.08 µs for libc `CLOCK_MONOTONIC`. The same probe ran six batches of 20,000 calls per method. Concurrent host compilation was possible. The preview check measured clock selection and clock cost. No game FPS was collected in this run. The original installation returned to its usual single Android 17 Home at 60 Hz.
