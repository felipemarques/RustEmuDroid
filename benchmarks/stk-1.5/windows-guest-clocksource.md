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
