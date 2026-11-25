#define WIN32_LEAN_AND_MEAN
#include <Windows.h>
#include <WinHvPlatform.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define VCPU_COUNT 4
#define CODE_GPA 0x1000
#define CODE_LENGTH 4096
#define ALIGN_TSC UINT64_C(1099511627776)

static int succeeded(HRESULT result, const char *name)
{
    if (SUCCEEDED(result)) {
        return 1;
    }
    fprintf(stderr, "%s: HRESULT=0x%08lx\n", name, (unsigned long)result);
    return 0;
}

static int64_t qpc(void)
{
    LARGE_INTEGER now;
    if (!QueryPerformanceCounter(&now)) {
        fprintf(stderr, "QueryPerformanceCounter failed\n");
        exit(2);
    }
    return now.QuadPart;
}

static void log_row(const char *phase, unsigned vp, int64_t begin, int64_t end, uint64_t value)
{
    printf("%s,%u,%" PRId64 ",%" PRId64 ",%" PRIu64 "\n", phase, vp, begin, end, value);
}

static int guest_read(WHV_PARTITION_HANDLE partition, unsigned vp, const char *phase)
{
    const WHV_REGISTER_NAME setup_names[] = { WHvX64RegisterCs, WHvX64RegisterRip, WHvX64RegisterRflags };
    WHV_REGISTER_VALUE setup[3] = {0};
    setup[0].Segment.Limit = 0xffff;
    setup[0].Segment.Attributes = 0x009b;
    setup[1].Reg64 = CODE_GPA;
    setup[2].Reg64 = 2;
    if (!succeeded(WHvSetVirtualProcessorRegisters(partition, vp, setup_names, ARRAYSIZE(setup_names), setup),
                   "set real-mode guest state")) {
        return 0;
    }
    WHV_RUN_VP_EXIT_CONTEXT exit_context = {0};
    int64_t before = qpc();
    HRESULT result = WHvRunVirtualProcessor(partition, vp, &exit_context, sizeof exit_context);
    int64_t after = qpc();
    if (!succeeded(result, "run guest RDTSC")) {
        return 0;
    }
    uint64_t rip = exit_context.VpContext.Rip;
    if (exit_context.ExitReason != WHvRunVpExitReasonX64Halt || (rip != CODE_GPA + 2 && rip != CODE_GPA + 3)) {
        fprintf(stderr, "Unexpected guest exit reason=0x%x rip=0x%" PRIx64 "\n",
                (unsigned)exit_context.ExitReason, (uint64_t)exit_context.VpContext.Rip);
        return 0;
    }
    const WHV_REGISTER_NAME result_names[] = { WHvX64RegisterRax, WHvX64RegisterRdx };
    WHV_REGISTER_VALUE out[2] = {0};
    if (!succeeded(WHvGetVirtualProcessorRegisters(partition, vp, result_names, ARRAYSIZE(result_names), out),
                   "read guest RDTSC result")) {
        return 0;
    }
    uint64_t tsc = ((uint64_t)(uint32_t)out[1].Reg64 << 32) | (uint32_t)out[0].Reg64;
    log_row(phase, vp, before, after, tsc);
    return 1;
}

static int parked_read(WHV_PARTITION_HANDLE partition, unsigned vp, const char *phase)
{
    WHV_REGISTER_NAME name = WHvX64RegisterTsc;
    WHV_REGISTER_VALUE value = {0};
    int64_t before = qpc();
    HRESULT result = WHvGetVirtualProcessorRegisters(partition, vp, &name, 1, &value);
    int64_t after = qpc();
    if (!succeeded(result, "read parked TSC register")) {
        return 0;
    }
    log_row(phase, vp, before, after, value.Reg64);
    return 1;
}

static int all_parked(WHV_PARTITION_HANDLE partition, const char *phase)
{
    for (unsigned vp = 0; vp < VCPU_COUNT; vp++) {
        if (!parked_read(partition, vp, phase)) {
            return 0;
        }
    }
    return 1;
}

static int all_guest(WHV_PARTITION_HANDLE partition, const char *phase)
{
    for (unsigned vp = 0; vp < VCPU_COUNT; vp++) {
        if (!guest_read(partition, vp, phase)) {
            return 0;
        }
    }
    return 1;
}

int main(int argc, char **argv)
{
    if (argc != 2 || (strcmp(argv[1], "running-clock") != 0 && strcmp(argv[1], "suspended-clock") != 0
                      && strcmp(argv[1], "suspended-auto-resume") != 0)) {
        fprintf(stderr, "Usage: probe.exe running-clock|suspended-clock|suspended-auto-resume\n");
        return 2;
    }
    const int suspend = strcmp(argv[1], "running-clock") != 0;
    const int explicit_resume = strcmp(argv[1], "suspended-clock") == 0;
    LARGE_INTEGER frequency;
    if (!QueryPerformanceFrequency(&frequency)) {
        return 2;
    }
    uint64_t tsc_hz = 0;
    if (!succeeded(WHvGetCapability(WHvCapabilityCodeProcessorClockFrequency, &tsc_hz, sizeof tsc_hz, NULL),
                   "WHP reported processor clock frequency") || tsc_hz == 0) {
        return 2;
    }
    printf("# arm=%s qpc_hz=%" PRId64 " whp_tsc_hz=%" PRIu64 " vp_count=%u\n",
           argv[1], frequency.QuadPart, tsc_hz, (unsigned)VCPU_COUNT);
    puts("phase,vp,qpc_before,qpc_after,tsc");

    int rc = 1;
    UINT32 number_of_vps = VCPU_COUNT;
    WHV_PARTITION_HANDLE partition = NULL;
    void *memory = NULL;
    unsigned created = 0;
    if (!succeeded(WHvCreatePartition(&partition), "create private partition") ||
        !succeeded(WHvSetPartitionProperty(partition, WHvPartitionPropertyCodeProcessorCount,
                                          &number_of_vps, sizeof number_of_vps), "set VP count") ||
        !succeeded(WHvSetupPartition(partition), "setup private partition")) {
        goto cleanup;
    }
    memory = VirtualAlloc(NULL, CODE_LENGTH, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (memory == NULL) {
        fprintf(stderr, "VirtualAlloc: %lu\n", GetLastError());
        goto cleanup;
    }
    /* RDTSC; HLT. No I/O, jumps, interrupts or guest stacks. */
    const unsigned char instructions[] = {0x0f, 0x31, 0xf4};
    memcpy(memory, instructions, sizeof instructions);
    if (!succeeded(WHvMapGpaRange(partition, memory, CODE_GPA, CODE_LENGTH,
                                  WHvMapGpaRangeFlagRead | WHvMapGpaRangeFlagExecute), "map one instruction page")) {
        goto cleanup;
    }
    for (; created < VCPU_COUNT; created++) {
        if (!succeeded(WHvCreateVirtualProcessor(partition, created, 0), "create disposable VP")) {
            goto cleanup;
        }
    }
    if (!all_guest(partition, "initial_guest")) {
        goto cleanup;
    }
    Sleep(40);
    if (!all_parked(partition, "baseline_first")) {
        goto cleanup;
    }
    Sleep(60);
    if (!all_parked(partition, "baseline_second")) {
        goto cleanup;
    }
    if (suspend) {
        int64_t before = qpc();
        if (!succeeded(WHvSuspendPartitionTime(partition), "suspend partition time with all VPs parked")) {
            goto cleanup;
        }
        log_row("suspended", 0, before, qpc(), 0);
    }
    const WHV_REGISTER_NAME tsc_register = WHvX64RegisterTsc;
    WHV_REGISTER_VALUE new_value = {0};
    new_value.Reg64 = ALIGN_TSC;
    for (unsigned vp = 0; vp < VCPU_COUNT; vp++) {
        int64_t before = qpc();
        HRESULT result = WHvSetVirtualProcessorRegisters(partition, vp, &tsc_register, 1, &new_value);
        int64_t after = qpc();
        if (!succeeded(result, "write equal TSC in parked VP")) {
            goto cleanup;
        }
        log_row("set", vp, before, after, ALIGN_TSC);
        if (vp + 1 < VCPU_COUNT) {
            Sleep(20);
        }
    }
    if (!all_parked(partition, "parked_first")) {
        goto cleanup;
    }
    Sleep(30);
    if (!all_parked(partition, "parked_second")) {
        goto cleanup;
    }
    if (explicit_resume) {
        int64_t before = qpc();
        if (!succeeded(WHvResumePartitionTime(partition), "resume partition time")) {
            goto cleanup;
        }
        log_row("resumed", 0, before, qpc(), 0);
    }
    if (!all_guest(partition, "final_guest_first")) {
        goto cleanup;
    }
    Sleep(80);
    if (!all_guest(partition, "final_guest_second")) {
        goto cleanup;
    }
    rc = 0;

cleanup:
    while (created > 0) {
        if (!succeeded(WHvDeleteVirtualProcessor(partition, --created), "delete disposable VP")) {
            rc = 1;
        }
    }
    if (partition && !succeeded(WHvDeletePartition(partition), "delete disposable partition")) {
        rc = 1;
    }
    if (memory && !VirtualFree(memory, 0, MEM_RELEASE)) {
        rc = 1;
    }
    if (rc == 0) {
        puts("# cleanup_verified=1");
    }
    return rc;
}
