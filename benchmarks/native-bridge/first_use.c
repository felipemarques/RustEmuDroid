#include <inttypes.h>
#include <stdio.h>
#include "timing.h"
#include "first_use.inc"

static struct sample traverse(unsigned bank) {
    uint64_t value = UINT64_C(0x34199a4dbe31f47a) ^ bank;
    struct sample_start start = begin_sample();
    __asm__ __volatile__("" : "+r"(value) : : "memory");
    for (unsigned i = 0; i < LEAVES; ++i) {
        unsigned index = bank * LEAVES + (i * 421 + 113) % LEAVES;
        value = leaves[index](value);
    }
    __asm__ __volatile__("" : "+r"(value) : : "memory");
    return end_sample(&start, value);
}

JNIEXPORT jstring JNICALL Java_org_rustemudroid_qa_abicost_BenchActivity_measure(
        JNIEnv *env, jclass activity, jint count) {
    (void)activity;
    if (count < 2 || count > 9) return error(env, "Require a first traversal and at least one repeat");
    struct sample measurements[BANKS][9];
    for (unsigned bank = 0; bank < BANKS; ++bank) {
        for (int visit = 0; visit < count; ++visit) {
            struct sample s = traverse(bank);
            if (!s.cpu_ns || !s.wall_ns || (visit && s.digest != measurements[bank][0].digest))
                return error(env, "Clock or traversal digest mismatch");
            measurements[bank][visit] = s;
        }
    }
    char json[3600];
    size_t used = (size_t)snprintf(json, sizeof(json),
        "{\"abi\":\"%s\",\"pointer_bits\":%zu,\"banks\":%u,\"leaves_per_bank\":%u,"
        "\"sample_columns\":[\"wall_ns\",\"thread_cpu_ns\",\"minor_faults\",\"major_faults\"],"
        "\"bank_digests\":[", BENCH_ABI, sizeof(void *) * 8, (unsigned)BANKS, (unsigned)LEAVES);
    for (unsigned bank = 0; bank < BANKS; ++bank) {
        used += (size_t)snprintf(json + used, sizeof(json) - used, "%s\"%016" PRIx64 "\"",
                                bank ? "," : "", measurements[bank][0].digest);
    }
    json[used++] = ']';
    for (unsigned bank = 0; bank < BANKS; ++bank) {
        used += (size_t)snprintf(json + used, sizeof(json) - used, ",\"bank_%u\":[", bank);
        for (int visit = 0; visit < count; ++visit) {
            struct sample s = measurements[bank][visit];
            int written = snprintf(json + used, sizeof(json) - used,
                "%s[%" PRIu64 ",%" PRIu64 ",%ld,%ld]", visit ? "," : "",
                s.wall_ns, s.cpu_ns, s.minor_faults, s.major_faults);
            if (written < 0 || (size_t)written + 4 >= sizeof(json) - used)
                return error(env, "Traversal report too long");
            used += (size_t)written;
        }
        json[used++] = ']';
    }
    json[used++] = '}';
    json[used] = '\0';
    return (*env)->NewStringUTF(env, json);
}
