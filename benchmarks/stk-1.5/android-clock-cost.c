// Measure the clocks used by STK's profiler and main loop on Android x86-64.
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

static int libc_timeofday(void) {
    struct timeval tv;
    return gettimeofday(&tv, NULL);
}

static int syscall_timeofday(void) {
    struct timeval tv;
    return (int)syscall(SYS_gettimeofday, &tv, NULL);
}

static int libc_monotonic(void) {
    struct timespec ts;
    return clock_gettime(CLOCK_MONOTONIC, &ts);
}

static int64_t elapsed_ns(struct timespec start, struct timespec end) {
    return (int64_t)(end.tv_sec - start.tv_sec) * 1000000000 + end.tv_nsec - start.tv_nsec;
}

int main(void) {
    const struct {
        const char *name;
        int (*read)(void);
    } methods[] = {
        {"libc_gettimeofday", libc_timeofday},
        {"syscall_gettimeofday", syscall_timeofday},
        {"libc_clock_monotonic", libc_monotonic},
    };
    enum { METHODS = sizeof(methods) / sizeof(methods[0]), CALLS = 20000 };

    puts("batch,method,calls,elapsed_ns,thread_cpu_ns");
    for (int batch = 0; batch < 6; ++batch) {
        for (int position = 0; position < METHODS; ++position) {
            const int idx = (batch + position) % METHODS;
            for (int i = 0; i < 128; ++i)
                if (methods[idx].read() != 0) return 1;
            struct timespec start, end, cpu_start, cpu_end;
            if (clock_gettime(CLOCK_MONOTONIC, &start) ||
                clock_gettime(CLOCK_THREAD_CPUTIME_ID, &cpu_start)) return 1;
            for (int i = 0; i < CALLS; ++i)
                if (methods[idx].read() != 0) return 1;
            if (clock_gettime(CLOCK_THREAD_CPUTIME_ID, &cpu_end) ||
                clock_gettime(CLOCK_MONOTONIC, &end)) return 1;
            printf("%d,%s,%d,%" PRId64 ",%" PRId64 "\n", batch + 1, methods[idx].name,
                   CALLS, elapsed_ns(start, end), elapsed_ns(cpu_start, cpu_end));
        }
    }
    return 0;
}
