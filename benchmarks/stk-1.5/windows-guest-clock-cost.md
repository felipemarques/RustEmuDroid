# Reading the clock in the two Windows Android guests

SuperTuxKart 1.5 uses `gettimeofday` for its Android CPU profiler. The main loop uses a monotonic clock to calculate simulation steps. An identical x86-64 executable ran as the ordinary Android shell on the original Windows test PC. Its clock reads were much cheaper on the Kemu 0.7.1 Android 16 guest than on the SDK 37.3.10 Android 17 Google Play guest. Both guests used WHPX and four virtual CPUs. [STK profiler](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/utils/profiler.cpp#L75-L81) ? [STK main loop](https://github.com/supertuxkart/stk-code/blob/1fb491f507216c5d181ccd85f29ff08eca003827/src/main_loop.cpp#L457-L464)

The table shows the median elapsed nanoseconds per call over six batches, taken after each guest finished the same instrumented kart replay. Each method ran 20,000 times per batch, after 128 warm-up calls. Its position rotated between batches. The probe sampled elapsed time and its own thread CPU time around each batch. The JSON includes guest thread CPU time and the individual elapsed readings for each batch, with independent repetitions and the sampled CPU capabilities. [Full measurements and hashes](windows-guest-clock-cost.json)

| Guest | libc gettimeofday | libc clock_gettime(CLOCK_MONOTONIC) | gettimeofday via syscall |
| --- | ---: | ---: | ---: |
| Android 17 Google Play, SDK | 8,436 ns | 8,488 ns | 8,264 ns |
| Android 16, Kemu | 18 ns | 17 ns | 233 ns |

Both guests advertised the invariant-TSC bit (CPUID leaf `0x80000007`, EDX `0x100`). Each reported `constant_tsc` and `nonstop_tsc` on all four vCPUs and requested `clocksource=pit` on its kernel command line. Subsequent diagnostic boots recorded which clock Linux actually selected. SDK Android 17 switched to HPET after detecting 6,195,110,760 cycles of skew between vCPU counters. Kemu Android 16 selected TSC. The Kemu WHPX source also explicitly supplies the invariant-TSC bit. [Both kernel diagnostics](windows-guest-clocksource.md) · [Kemu CPUID reporting](https://github.com/kuloPo/Kemu-emulator_external_qemu/blob/1622371aee57f26f5446ca42ec32bd4faddfc440/target/i386/whpx-all.c#L2477-L2492)

In Kemu, the libc calls took less time than an explicit system call. The difference is consistent with a fast userspace time read. The SDK guest showed similar costs for libc and the explicit system call. Measuring thousands of calls amplifies this distinction. The probe does not count clock reads in a game frame. STK's enabled profiler also takes a lock and performs other bookkeeping around markers. A causal FPS experiment must change the timing path on the same engine, verify its accuracy against the host, and measure the game and Android cadence again. [Race-update scopes](windows-race-update-scopes.md)

## Reproduce

Compile both small programs with Android NDK Clang for the same x86-64 target. The tested NDK was 30.0.14904198. These sample commands assume the NDK toolchain's `bin` directory is on the path.

```sh
clang --target=x86_64-linux-android28 -O2 -Wall -Wextra -Werror android-clock-cost.c -o android-clock-cost
clang --target=x86_64-linux-android28 -O2 -Wall -Wextra -Werror android-timestamp-cpuid.c -o timestamp-cpuid
```

Use an owned QA guest for each engine, one at a time. Copy the same two executables to a fresh subdirectory of `/data/local/tmp` on the selected ADB serial, make them executable and invoke them after the timed replay. Save their output, check the executable hashes and remove both temporary files after the run. The probes use normal userspace APIs and instructions. [Clock probe](android-clock-cost.c) ? [CPUID probe](android-timestamp-cpuid.c)
