#include <errno.h>
#include <inttypes.h>
#include <stdio.h>
#include <sys/mman.h>
#include <unistd.h>
#include "timing.h"

enum { PAGES = 1024, PASSES = 60, TOTAL_PAGES = PAGES * PASSES, MAX_SAMPLES = 9 };

static uint64_t touch(volatile unsigned char *memory, size_t stride) {
    uint64_t sum = 0;
    for (unsigned pass = 1; pass <= PASSES; ++pass) {
        for (unsigned page = 0; page < PAGES; ++page) {
            size_t offset = ((pass - 1) * PAGES + page) * stride;
            memory[offset] = (unsigned char)pass;
            sum += memory[offset];
        }
    }
    return sum;
}

JNIEXPORT jstring JNICALL Java_org_rustemudroid_qa_abicost_BenchActivity_measure(
        JNIEnv *env, jclass activity, jint count) {
    (void)activity;
    long stride = sysconf(_SC_PAGESIZE);
    if (count < 1 || count > MAX_SAMPLES || stride <= 0 || (size_t)stride > SIZE_MAX / TOTAL_PAGES)
        return error(env, "Invalid page size or sample count");
    size_t size = (size_t)stride * TOTAL_PAGES;
    volatile unsigned char *memory = mmap(NULL, size, PROT_READ | PROT_WRITE,
                                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (memory == MAP_FAILED) return error(env, "Could not allocate test pages");
    touch(memory, (size_t)stride);

    struct sample samples[2][MAX_SAMPLES];
    for (int index = 0; index < count; ++index) {
        for (int order = 0; order < 2; ++order) {
            int discard = (order + index) % 2;
            if (discard && madvise((void *)memory, size, MADV_DONTNEED) != 0) {
                int code = errno;
                char message[96];
                snprintf(message, sizeof(message), "MADV_DONTNEED failed (errno %d)", code);
                munmap((void *)memory, size);
                return error(env, message);
            }
            struct sample_start start = begin_sample();
            uint64_t sum = touch(memory, (size_t)stride);
            struct sample measured = end_sample(&start, sum);
            const char *failure = NULL;
            if (sum != (uint64_t)PAGES * PASSES * (PASSES + 1) / 2)
                failure = "Timed writes returned an unexpected sum";
            else if (!measured.wall_ns || !measured.cpu_ns)
                failure = "Timers did not advance over a complete 61440-page write";
            else if (measured.minor_faults < 0 || measured.major_faults < 0)
                failure = "Cannot read per-thread fault counters";
            if (failure) {
                munmap((void *)memory, size);
                return error(env, failure);
            }
            samples[discard][index] = measured;
        }
    }
    if (munmap((void *)memory, size) != 0) return error(env, "Could not release test pages");

    char json[3500];
    size_t used = (size_t)snprintf(json, sizeof(json),
        "{\"abi\":\"%s\",\"pointer_bits\":%zu,\"pages\":%d,\"passes\":%d,\"page_bytes\":%ld,"
        "\"touch_digest\":%" PRIu64 ",\"sample_columns\":[\"wall_ns\",\"thread_cpu_ns\","
        "\"minor_faults\",\"major_faults\"]", BENCH_ABI, sizeof(void *) * 8, PAGES, PASSES,
        stride, samples[0][0].digest);
    const char *names[] = {"resident", "discarded"};
    for (unsigned mode = 0; mode < 2; ++mode) {
        used += (size_t)snprintf(json + used, sizeof(json) - used, ",\"%s\":[", names[mode]);
        for (int index = 0; index < count; ++index) {
            struct sample s = samples[mode][index];
            int n = snprintf(json + used, sizeof(json) - used, "%s[%" PRIu64 ",%" PRIu64 ",%ld,%ld]",
                             index ? "," : "", s.wall_ns, s.cpu_ns, s.minor_faults, s.major_faults);
            if (n < 0 || (size_t)n + 4 >= sizeof(json) - used) return error(env, "Report too long");
            used += (size_t)n;
        }
        json[used++] = ']';
    }
    json[used++] = '}';
    json[used] = '\0';
    return (*env)->NewStringUTF(env, json);
}
