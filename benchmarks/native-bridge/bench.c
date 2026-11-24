#include <inttypes.h>
#include <jni.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <time.h>

#if defined(__aarch64__)
#define BENCH_ABI "arm64-v8a"
#elif defined(__x86_64__)
#define BENCH_ABI "x86_64"
#else
#error Unsupported benchmark ABI
#endif

#define ARITHMETIC_ROUNDS UINT32_C(6000000)
#define MEMORY_ROUNDS UINT32_C(1048576)
#define MEMORY_WORDS UINT32_C(262144)
#define SEED UINT64_C(0x34199a4dbe31f47a)
#define WARMUPS 2
#define MAX_SAMPLES 9

static uint64_t now_ns(clockid_t clock) {
    struct timespec stamp;
    if (clock_gettime(clock, &stamp) != 0) {
        return 0;
    }
    return (uint64_t)stamp.tv_sec * UINT64_C(1000000000) + (uint64_t)stamp.tv_nsec;
}

__attribute__((noinline))
static uint64_t arithmetic(uint64_t state) {
    for (uint32_t i = 0; i < ARITHMETIC_ROUNDS; ++i) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state = state * UINT64_C(0x9e3779b97f4a7c15) + i;
    }
    return state;
}

__attribute__((noinline))
static uint64_t indexed(const uint32_t *next, uint32_t index) {
    uint64_t sum = 0;
    for (uint32_t i = 0; i < MEMORY_ROUNDS; ++i) {
        index = next[index];
        sum += (uint64_t)index * (i + 1);
    }
    return sum;
}

static void cycle(uint32_t *next, uint32_t *order) {
    uint32_t rng = UINT32_C(0x52ed42a9);
    for (uint32_t i = 0; i < MEMORY_WORDS; ++i) {
        order[i] = i;
    }
    for (uint32_t i = MEMORY_WORDS - 1; i > 0; --i) {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        uint32_t j = rng % (i + 1);
        uint32_t word = order[i];
        order[i] = order[j];
        order[j] = word;
    }
    for (uint32_t i = 0; i < MEMORY_WORDS; ++i) {
        next[order[i]] = order[(i + 1) % MEMORY_WORDS];
    }
}

struct sample {
    uint64_t wall_ns;
    uint64_t cpu_ns;
    uint64_t digest;
    long minor_faults;
    long major_faults;
};

static struct sample trial(const uint32_t *next, int task) {
    uint64_t seed = task == 0 ? SEED : 0;
    struct rusage usage_start, usage_end;
    int usage_ok = getrusage(RUSAGE_THREAD, &usage_start) == 0;
    uint64_t wall_start = now_ns(CLOCK_MONOTONIC);
    uint64_t cpu_start = now_ns(CLOCK_THREAD_CPUTIME_ID);
    // Keep the workload between the clock reads, including when every trial uses the same seed.
    __asm__ __volatile__("" : "+r"(seed) : : "memory");
    uint64_t digest = task == 0 ? arithmetic(seed) : indexed(next, (uint32_t)seed);
    __asm__ __volatile__("" : "+r"(digest) : : "memory");
    uint64_t cpu_end = now_ns(CLOCK_THREAD_CPUTIME_ID);
    uint64_t wall_end = now_ns(CLOCK_MONOTONIC);
    usage_ok = getrusage(RUSAGE_THREAD, &usage_end) == 0 && usage_ok;
    if (!wall_start || !cpu_start || cpu_end <= cpu_start || wall_end <= wall_start) {
        return (struct sample){0, 0, digest, -1, -1};
    }
    return (struct sample){wall_end - wall_start, cpu_end - cpu_start, digest,
        usage_ok ? usage_end.ru_minflt - usage_start.ru_minflt : -1,
        usage_ok ? usage_end.ru_majflt - usage_start.ru_majflt : -1};
}

static jstring error(JNIEnv *env, const char *message) {
    jclass exception = (*env)->FindClass(env, "java/lang/IllegalStateException");
    if (exception) {
        (*env)->ThrowNew(env, exception, message);
    }
    return NULL;
}

JNIEXPORT jstring JNICALL Java_org_rustemudroid_qa_abicost_BenchActivity_measure(
        JNIEnv *env, jclass activity, jint count) {
    (void)activity;
    if (count < 1 || count > MAX_SAMPLES) {
        return error(env, "Invalid sample count");
    }
    uint32_t *next = malloc(MEMORY_WORDS * sizeof(*next));
    uint32_t *order = malloc(MEMORY_WORDS * sizeof(*order));
    if (!next || !order) {
        free(next);
        free(order);
        return error(env, "Allocation failed");
    }
    cycle(next, order);
    free(order);
    uint64_t digests[2] = {0, 0};
    struct sample samples[2][MAX_SAMPLES];
    for (int i = -WARMUPS; i < count; ++i) {
        for (int task = 0; task < 2; ++task) {
            struct sample value = trial(next, task);
            if (!value.wall_ns || !value.cpu_ns || (i != -WARMUPS && value.digest != digests[task])) {
                free(next);
                return error(env, "Clock or workload digest mismatch");
            }
            digests[task] = value.digest;
            if (i >= 0) {
                samples[task][i] = value;
            }
        }
    }
    free(next);
    char json[3000];
    size_t used = (size_t)snprintf(json, sizeof(json),
        "{\"abi\":\"%s\",\"pointer_bits\":%zu,\"warmups\":%d,\"arithmetic_rounds\":%u,"
        "\"memory_rounds\":%u,\"memory_words\":%u,\"arith_digest\":\"%016" PRIx64
        "\",\"memory_digest\":\"%016" PRIx64 "\","
        "\"sample_columns\":[\"wall_ns\",\"thread_cpu_ns\",\"minor_faults\",\"major_faults\"],",
        BENCH_ABI, sizeof(void *) * 8, WARMUPS, ARITHMETIC_ROUNDS, MEMORY_ROUNDS,
        MEMORY_WORDS, digests[0], digests[1]);
    for (int task = 0; task < 2; ++task) {
        used += (size_t)snprintf(json + used, sizeof(json) - used, "%s%s[", task ? "," : "",
                                 task ? "\"memory\":" : "\"arithmetic\":");
        for (int i = 0; i < count; ++i) {
            struct sample value = samples[task][i];
            used += (size_t)snprintf(json + used, sizeof(json) - used,
                    "%s[%" PRIu64 ",%" PRIu64 ",%ld,%ld]", i ? "," : "",
                    value.wall_ns, value.cpu_ns, value.minor_faults, value.major_faults);
        }
        json[used++] = ']';
    }
    json[used++] = '}';
    json[used] = '\0';
    return (*env)->NewStringUTF(env, json);
}
