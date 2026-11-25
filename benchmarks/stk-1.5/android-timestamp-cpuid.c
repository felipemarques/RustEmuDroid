#include <cpuid.h>
#include <stdio.h>

int main(void) {
    unsigned int a, b, c, d;
    const unsigned int max = __get_cpuid_max(0x80000000u, 0);
    if (max < 0x80000007u) {
        printf("max_extended=0x%x leaf_available=0\n", max);
        return 0;
    }
    __cpuid(0x80000007u, a, b, c, d);
    printf("max_extended=0x%x leaf_available=1 extended_apm_edx=0x%x invariant_tsc=%u\n",
           max, d, (d >> 8) & 1u);
    return 0;
}
