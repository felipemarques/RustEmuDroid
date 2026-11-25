/* Read a clock and the timestamp counter on each CPU allowed to this process. */
#define _GNU_SOURCE
#include <cpuid.h>
#include <inttypes.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <time.h>
#include <x86intrin.h>

static int raw_ns(uint64_t *value)
{
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC_RAW, &ts) != 0)
        return -1;
    *value = (uint64_t)ts.tv_sec * UINT64_C(1000000000) + (uint64_t)ts.tv_nsec;
    return 0;
}

int main(void)
{
    unsigned eax, ebx, ecx, edx;
    cpu_set_t allowed;
    if (!__get_cpuid(0x80000001, &eax, &ebx, &ecx, &edx) || !(edx & (1u << 27))) {
        fputs("RDTSCP unavailable\n", stderr);
        return 1;
    }
    if (sched_getaffinity(0, sizeof(allowed), &allowed) < 0) {
        perror("sched_getaffinity");
        return 1;
    }
    int count = CPU_COUNT(&allowed);
    if (count < 1)
        return 1;
    printf("READY %d\n", count);
    fflush(stdout);
    unsigned round;
    while (scanf("%u", &round) == 1) {
        for (int i = 0; i < CPU_SETSIZE; ++i) {
            int cpu = round % 2 ? CPU_SETSIZE - 1 - i : i;
            if (!CPU_ISSET(cpu, &allowed))
                continue;
            cpu_set_t selected;
            CPU_ZERO(&selected);
            CPU_SET(cpu, &selected);
            if (sched_setaffinity(0, sizeof(selected), &selected) != 0) {
                perror("sched_setaffinity");
                return 1;
            }
            uint64_t before, after;
            if (raw_ns(&before) != 0)
                return 1;
            unsigned aux;
            _mm_lfence();
            uint64_t ticks = __rdtscp(&aux);
            _mm_lfence();
            if (raw_ns(&after) != 0 || sched_getcpu() != cpu)
                return 1;
            printf("CPU %u %d %" PRIu64 " %" PRIu64 " %" PRIu64 "\n",
                   round, cpu, before, ticks, after);
        }
        printf("END %u\n", round);
        fflush(stdout);
    }
    return ferror(stdin) ? 1 : 0;
}
