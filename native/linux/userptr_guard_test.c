/*
 * Test of userptr_guard.c against a simulated driver. It includes the library source, calls its ioctl() and
 * checks the real state of the address ranges in /proc/self/smaps ("dc" in VmFlags: not copied by fork()).
 *
 * Build and run: cc -O1 -g -Wall -Wextra -o userptr_guard_test native/linux/userptr_guard_test.c -ldl -lpthread
 *                ./userptr_guard_test        (prints "ok" and exits with 0)
 * `cargo test` on Linux does this in src/android/amd_linux.rs.
 */
#define GUARD_SELFTEST 1
#define GUARD_RETRY_LIMIT_MS 400
#include "userptr_guard.c"

#include <assert.h>
#include <sys/wait.h>

#define MIB (1024 * 1024)
#define CHECK(condition)                                                                                       \
    do {                                                                                                       \
        if (!(condition)) {                                                                                    \
            fprintf(stderr, "FAILED line %d: %s\n", __LINE__, #condition);                                     \
            exit(1);                                                                                           \
        }                                                                                                      \
    } while (0)

/* The simulated driver. */
static atomic_uint next_handle = 1;
static atomic_int busy_answers;         /* submissions still to be answered with EBUSY; negative: all of them */
static atomic_int submissions_seen;
static atomic_int fail_next_register, fail_next_close;
static struct guard_cs expected_submission;

static int simulated_driver(int fd, unsigned long request, void *argument) {
    (void)fd;
    if (request == GUARD_IOCTL_AMDGPU_GEM_USERPTR) {
        struct guard_gem_userptr *user = argument;
        if (atomic_exchange(&fail_next_register, 0)) {
            errno = EINVAL;
            return -1;
        }
        user->handle = atomic_fetch_add(&next_handle, 1);
        return 0;
    }
    if (request == GUARD_IOCTL_GEM_CLOSE) {
        if (atomic_exchange(&fail_next_close, 0)) {
            errno = EINVAL;
            return -1;
        }
        return 0;
    }
    if (request == GUARD_IOCTL_AMDGPU_CS) {
        struct guard_cs *submission = argument;
        atomic_fetch_add(&submissions_seen, 1);
        /* Every attempt must carry the original input. */
        CHECK(memcmp(submission, &expected_submission, sizeof *submission) == 0);
        const int left = atomic_load(&busy_answers);
        if (left != 0) {
            if (left > 0) {
                atomic_fetch_sub(&busy_answers, 1);
            }
            memset(submission, 0xee, sizeof *submission);       /* what a careless driver could leave behind */
            errno = EBUSY;
            return -1;
        }
        memset(submission, 0, sizeof *submission);
        *(uint64_t *)argument = 0x1234;                          /* the output handle */
        return 0;
    }
    if (request == GUARD_IOCTL_AMDGPU_INFO) {
        struct guard_info *info = argument;
        if (info->query == GUARD_INFO_MEMORY) {
            struct guard_memory *memory = (struct guard_memory *)(uintptr_t)info->return_pointer;
            memset(memory, 0, sizeof *memory);
            memory->vram.total_heap_size = 8192ull * MIB;
            memory->vram.usable_heap_size = 8000ull * MIB;
            memory->vram.max_allocation = 7000ull * MIB;
            memory->cpu_accessible_vram.total_heap_size = 256ull * MIB;
            memory->cpu_accessible_vram.usable_heap_size = 250ull * MIB;
            memory->cpu_accessible_vram.max_allocation = 200ull * MIB;
        } else if (info->query == GUARD_INFO_VRAM_GTT) {
            struct guard_vram_gtt *sizes = (struct guard_vram_gtt *)(uintptr_t)info->return_pointer;
            sizes->vram_size = 8192ull * MIB;
            sizes->vram_cpu_accessible_size = 256ull * MIB;
            sizes->gtt_size = 16384ull * MIB;
        }
        return 0;
    }
    errno = ENOTTY;
    return -1;
}

/* 1 when the mapping that holds `address` is left out of fork(), 0 when it is copied. */
static int left_out_of_fork(const void *address) {
    FILE *maps = fopen("/proc/self/smaps", "r");
    CHECK(maps != NULL);
    char line[512];
    int inside = 0, answer = -1;
    while (fgets(line, sizeof line, maps)) {
        unsigned long start, end;
        if (sscanf(line, "%lx-%lx ", &start, &end) == 2 && strchr(line, '-') < strchr(line, ' ')) {
            inside = (uintptr_t)address >= start && (uintptr_t)address < end;
        } else if (inside && strncmp(line, "VmFlags:", 8) == 0) {
            answer = strstr(line, " dc") != NULL;
            break;
        }
    }
    fclose(maps);
    CHECK(answer >= 0);
    return answer;
}

static int whole_range_left_out(const void *address, size_t size) {
    return left_out_of_fork(address) && left_out_of_fork((const char *)address + size - 4096);
}

static int no_page_left_out(const void *address, size_t size) {
    return !left_out_of_fork(address) && !left_out_of_fork((const char *)address + size - 4096);
}

static void *anonymous(size_t size) {
    void *memory = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(memory != MAP_FAILED);
    return memory;
}

static uint32_t register_range(int fd, void *address, size_t size) {
    struct guard_gem_userptr user = {(uintptr_t)address, size, 0xe, 0};
    CHECK(ioctl(fd, GUARD_IOCTL_AMDGPU_GEM_USERPTR, &user) == 0);
    return user.handle;
}

static int close_handle(int fd, uint32_t handle) {
    struct guard_gem_close close_arg = {handle, 0};
    return ioctl(fd, GUARD_IOCTL_GEM_CLOSE, &close_arg);
}

static unsigned live_entries(void) {
    pthread_mutex_lock(&lock);
    const unsigned count = live;
    pthread_mutex_unlock(&lock);
    return count;
}

static int submit(int fd) {
    struct guard_cs submission = {7, 9, 3, 0, 0xabcdef};
    expected_submission = submission;
    atomic_store(&submissions_seen, 0);
    return ioctl(fd, GUARD_IOCTL_AMDGPU_CS, &submission);
}

static void register_and_close(void) {
    void *memory = anonymous(4 * MIB);
    CHECK(no_page_left_out(memory, 4 * MIB));
    const uint32_t handle = register_range(100, memory, 4 * MIB);
    CHECK(whole_range_left_out(memory, 4 * MIB));
    CHECK(live_entries() == 1);
    /* An ordinary buffer with another handle, or the same handle on another device, changes nothing. */
    CHECK(close_handle(100, handle + 1000) == 0);
    CHECK(close_handle(101, handle) == 0);
    CHECK(whole_range_left_out(memory, 4 * MIB));
    CHECK(close_handle(100, handle) == 0);
    CHECK(no_page_left_out(memory, 4 * MIB));
    CHECK(live_entries() == 0);
    munmap(memory, 4 * MIB);
}

static void a_late_close_keeps_the_new_buffer_protected(void) {
    void *memory = anonymous(4 * MIB);
    const uint32_t old = register_range(100, memory, 4 * MIB);
    /* The memory was reused for a new buffer before the close of the old one arrived. */
    const uint32_t new = register_range(100, memory, 4 * MIB);
    CHECK(close_handle(100, old) == 0);
    CHECK(whole_range_left_out(memory, 4 * MIB));
    CHECK(close_handle(100, new) == 0);
    CHECK(no_page_left_out(memory, 4 * MIB));
    /* The same with ranges that only share a part. */
    const uint32_t whole = register_range(100, memory, 4 * MIB);
    const uint32_t half = register_range(100, (char *)memory + 2 * MIB, 2 * MIB);
    CHECK(close_handle(100, whole) == 0);
    CHECK(left_out_of_fork((char *)memory + 3 * MIB));
    CHECK(close_handle(100, half) == 0);
    CHECK(!left_out_of_fork((char *)memory + 3 * MIB));
    CHECK(live_entries() == 0);
    munmap(memory, 4 * MIB);
}

static void failures_of_the_driver_keep_the_state_true(void) {
    void *memory = anonymous(2 * MIB);
    /* A registration the driver refuses leaves nothing behind and keeps its error. */
    struct guard_gem_userptr user = {(uintptr_t)memory, 2 * MIB, 0xe, 0};
    atomic_store(&fail_next_register, 1);
    errno = 0;
    CHECK(ioctl(100, GUARD_IOCTL_AMDGPU_GEM_USERPTR, &user) == -1 && errno == EINVAL);
    CHECK(no_page_left_out(memory, 2 * MIB));
    CHECK(live_entries() == 0);
    /* A close the driver refuses keeps the buffer protected and known. */
    const uint32_t handle = register_range(100, memory, 2 * MIB);
    atomic_store(&fail_next_close, 1);
    errno = 0;
    CHECK(close_handle(100, handle) == -1 && errno == EINVAL);
    CHECK(whole_range_left_out(memory, 2 * MIB));
    CHECK(live_entries() == 1);
    CHECK(close_handle(100, handle) == 0);
    CHECK(no_page_left_out(memory, 2 * MIB));
    CHECK(live_entries() == 0);
    munmap(memory, 2 * MIB);
}

static void a_handle_that_comes_back_replaces_its_leftover(void) {
    void *first = anonymous(2 * MIB), *second = anonymous(2 * MIB);
    const uint32_t handle = register_range(100, first, 2 * MIB);
    /* The device file was closed without closing the buffer; the driver hands the same number out again. */
    atomic_store(&next_handle, handle);
    CHECK(register_range(100, second, 2 * MIB) == handle);
    CHECK(live_entries() == 1);
    CHECK(close_handle(100, handle) == 0);
    CHECK(no_page_left_out(second, 2 * MIB));
    CHECK(live_entries() == 0);
    munmap(first, 2 * MIB);
    munmap(second, 2 * MIB);
}

static void a_rejected_submission_is_sent_again(void) {
    /* Without a registered buffer a busy answer is passed on at once. */
    atomic_store(&busy_answers, -1);
    errno = 0;
    CHECK(submit(100) == -1 && errno == EBUSY);
    CHECK(atomic_load(&submissions_seen) == 1);

    void *memory = anonymous(2 * MIB);
    const uint32_t handle = register_range(100, memory, 2 * MIB);
    /* Buffers of another device do not count for this one. */
    errno = 0;
    CHECK(submit(101) == -1 && errno == EBUSY);
    CHECK(atomic_load(&submissions_seen) == 1);
    /* Accepted at the fourth attempt, with the original input each time. */
    atomic_store(&busy_answers, 3);
    CHECK(submit(100) == 0);
    CHECK(atomic_load(&submissions_seen) == 4);
    /* A submission the driver accepts is sent once. */
    atomic_store(&busy_answers, 0);
    CHECK(submit(100) == 0);
    CHECK(atomic_load(&submissions_seen) == 1);
    /* Busy for ever: the library gives up after its limit and passes the error on. */
    atomic_store(&busy_answers, -1);
    const uint64_t start = monotonic_ms();
    errno = 0;
    CHECK(submit(100) == -1 && errno == EBUSY);
    const uint64_t waited = monotonic_ms() - start;
    CHECK(waited >= GUARD_RETRY_LIMIT_MS && waited < GUARD_RETRY_LIMIT_MS + 3000);
    CHECK(atomic_load(&submissions_seen) > 2);
    atomic_store(&busy_answers, 0);
    CHECK(close_handle(100, handle) == 0);
    munmap(memory, 2 * MIB);
}

static void the_memory_queries_report_the_whole_video_memory(void) {
    struct guard_memory memory;
    struct guard_info info = {(uintptr_t)&memory, sizeof memory, GUARD_INFO_MEMORY, {0}};
    CHECK(ioctl(100, GUARD_IOCTL_AMDGPU_INFO, &info) == 0);
    CHECK(memory.cpu_accessible_vram.total_heap_size == 8192ull * MIB);
    CHECK(memory.cpu_accessible_vram.usable_heap_size == 8000ull * MIB);
    CHECK(memory.cpu_accessible_vram.max_allocation == 7000ull * MIB);
    struct guard_vram_gtt sizes;
    struct guard_info second = {(uintptr_t)&sizes, sizeof sizes, GUARD_INFO_VRAM_GTT, {0}};
    CHECK(ioctl(100, GUARD_IOCTL_AMDGPU_INFO, &second) == 0);
    CHECK(sizes.vram_cpu_accessible_size == 8192ull * MIB && sizes.gtt_size == 16384ull * MIB);
}

/* Several threads register, submit and close buffers on a few shared address ranges, like an allocator that
 * hands freed memory out again at once, some of them closing only after the range was taken again. */
#define SLOTS 6
#define SLOT_BYTES (1 * MIB)
static void *slots[SLOTS];
static pthread_mutex_t slot_locks[SLOTS];
static atomic_int stress_failures;

static void *stress_thread(void *argument) {
    unsigned seed = (unsigned)(uintptr_t)argument * 2654435761u + 1;
    const int fd = 200 + (int)((uintptr_t)argument % 2);
    for (int round = 0; round < 1500; round++) {
        seed = seed * 1103515245u + 12345u;
        const unsigned slot = (seed >> 16) % SLOTS;
        pthread_mutex_lock(&slot_locks[slot]);
        const uint32_t handle = register_range(fd, slots[slot], SLOT_BYTES);
        if (round % 50 == 0 && !whole_range_left_out(slots[slot], SLOT_BYTES)) {
            atomic_fetch_add(&stress_failures, 1);
        }
        const int late_close = (seed >> 8) % 4 == 0;
        if (!late_close) {
            if (close_handle(fd, handle) != 0) {
                atomic_fetch_add(&stress_failures, 1);
            }
        }
        pthread_mutex_unlock(&slot_locks[slot]);
        if (late_close) {
            /* Another thread may already have registered the same range. */
            if (close_handle(fd, handle) != 0) {
                atomic_fetch_add(&stress_failures, 1);
            }
        }
    }
    return NULL;
}

static void threads_sharing_addresses(void) {
    pthread_t threads[8];
    for (int slot = 0; slot < SLOTS; slot++) {
        slots[slot] = anonymous(SLOT_BYTES);
        pthread_mutex_init(&slot_locks[slot], NULL);
    }
    for (uintptr_t index = 0; index < 8; index++) {
        CHECK(pthread_create(&threads[index], NULL, stress_thread, (void *)index) == 0);
    }
    for (int index = 0; index < 8; index++) {
        pthread_join(threads[index], NULL);
    }
    CHECK(atomic_load(&stress_failures) == 0);
    CHECK(live_entries() == 0);
    for (int slot = 0; slot < SLOTS; slot++) {
        CHECK(no_page_left_out(slots[slot], SLOT_BYTES));
        munmap(slots[slot], SLOT_BYTES);
    }
}

static void a_child_of_fork_does_not_wait_for_the_lock(void) {
    void *memory = anonymous(2 * MIB);
    const uint32_t handle = register_range(100, memory, 2 * MIB);
    pthread_mutex_lock(&lock);              /* as if another thread held it at the moment of the fork */
    const pid_t child = fork();
    CHECK(child >= 0);
    if (child == 0) {
        struct guard_gem_close close_arg = {handle, 0};
        alarm(5);
        _exit(ioctl(100, GUARD_IOCTL_GEM_CLOSE, &close_arg) == 0 ? 0 : 3);
    }
    pthread_mutex_unlock(&lock);
    int status = 0;
    CHECK(waitpid(child, &status, 0) == child);
    CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    CHECK(whole_range_left_out(memory, 2 * MIB));
    CHECK(close_handle(100, handle) == 0);
    munmap(memory, 2 * MIB);
}

int main(void) {
    setenv("RUSTEMUDROID_AMD_VISIBLE_VRAM", "all", 1);
    unsetenv("RUSTEMUDROID_USERPTR_GUARD");
    unsetenv("RUSTEMUDROID_USERPTR_GUARD_TEST");
    test_driver = simulated_driver;
    register_and_close();
    a_late_close_keeps_the_new_buffer_protected();
    failures_of_the_driver_keep_the_state_true();
    a_handle_that_comes_back_replaces_its_leftover();
    a_rejected_submission_is_sent_again();
    the_memory_queries_report_the_whole_video_memory();
    threads_sharing_addresses();
    a_child_of_fork_does_not_wait_for_the_lock();
    puts("ok");
    return 0;
}
