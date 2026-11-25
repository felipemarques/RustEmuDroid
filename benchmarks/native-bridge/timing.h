#include <jni.h>
#include <stdint.h>
#include <sys/resource.h>
#include <time.h>

#if defined(__aarch64__)
#define BENCH_ABI "arm64-v8a"
#elif defined(__x86_64__)
#define BENCH_ABI "x86_64"
#else
#error Unsupported benchmark ABI
#endif

struct sample {
    uint64_t wall_ns, cpu_ns, digest;
    long minor_faults, major_faults;
};

struct sample_start {
    uint64_t wall, cpu;
    struct rusage usage;
    int usage_ok;
};

static uint64_t now_ns(clockid_t clock) {
    struct timespec stamp;
    if (clock_gettime(clock, &stamp) != 0) return 0;
    return (uint64_t)stamp.tv_sec * UINT64_C(1000000000) + (uint64_t)stamp.tv_nsec;
}

static struct sample_start begin_sample(void) {
    struct sample_start start = {0};
    start.usage_ok = getrusage(RUSAGE_THREAD, &start.usage) == 0;
    start.wall = now_ns(CLOCK_MONOTONIC);
    start.cpu = now_ns(CLOCK_THREAD_CPUTIME_ID);
    return start;
}

static struct sample end_sample(const struct sample_start *start, uint64_t digest) {
    uint64_t cpu = now_ns(CLOCK_THREAD_CPUTIME_ID);
    uint64_t wall = now_ns(CLOCK_MONOTONIC);
    struct rusage usage;
    int ok = getrusage(RUSAGE_THREAD, &usage) == 0 && start->usage_ok;
    if (!start->wall || !start->cpu || cpu <= start->cpu || wall <= start->wall)
        return (struct sample){0, 0, digest, -1, -1};
    return (struct sample){wall - start->wall, cpu - start->cpu, digest,
        ok ? usage.ru_minflt - start->usage.ru_minflt : -1,
        ok ? usage.ru_majflt - start->usage.ru_majflt : -1};
}

static jstring error(JNIEnv *env, const char *message) {
    jclass exception = (*env)->FindClass(env, "java/lang/IllegalStateException");
    if (exception) (*env)->ThrowNew(env, exception, message);
    return NULL;
}
