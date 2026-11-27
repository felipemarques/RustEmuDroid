/*
 * userptr_guard: preload library for the Android Emulator engine on Linux with AMD graphics (amdgpu/RADV).
 *
 * Why: with the VulkanAllocateHostMemory feature the engine allocates ordinary process memory and imports it
 * into the graphics driver as "user pointer" buffers. The engine also fork()s helper programs (adb, chattr).
 * A fork write-protects every private page of the process, including those buffers; the driver must then get
 * write access to all of them again before the next command submission and gives up after about one second
 * ("radv/amdgpu: The CS has been rejected (-16)"), which the engine treats as a lost device and aborts.
 *
 * What it does: when the driver registers a user-pointer buffer, mark that address range so that fork() does
 * not touch it (MADV_DONTFORK) and make its pages present and writable up front (MADV_POPULATE_WRITE). When
 * the buffer is closed, restore the default (MADV_DOFORK) before the memory goes back to the allocator, unless
 * another registered buffer uses the same addresses.
 *
 * Second line of defence: the kernel driver answers a command submission with EBUSY when it could not get the
 * pages of a user-pointer buffer in time. Nothing was submitted in that case, and the Vulkan driver would report
 * the device as lost. The library makes the pages of the registered buffers present and writable again from
 * user space, where there is no time limit, and sends the same submission again, for at most 10 seconds. The
 * graphics stack already resends a submission this way when the driver answers EAGAIN.
 *
 * Optional second part (RUSTEMUDROID_AMD_VISIBLE_VRAM=all): the emulator hides from Android every Vulkan memory
 * type that is not device local when the GPU is AMD, so the only mappable heap is the part of the video memory
 * the CPU can address directly. Without Resizable BAR that window is 256 MiB and games run out of memory,
 * although their mappable allocations really live in imported process memory. With this setting the memory
 * query of the driver reports the whole video memory as CPU visible, so the heap Android sees has the real
 * size. Buffers that truly need the window still go through the kernel, which falls back when it is full.
 *
 * Build: build.rs compiles this file for Linux targets and the client embeds the result. By hand:
 *        cc -O2 -fPIC -shared -Wall -Wextra -o libuserptr_guard.so native/linux/userptr_guard.c -ldl -lpthread
 * Test:  native/linux/userptr_guard_test.c includes this file and runs it against a simulated driver.
 * Use:   the client writes the library next to its managed runtime, adds it to the engine's LD_PRELOAD and sets
 *        RUSTEMUDROID_AMD_VISIBLE_VRAM when its memory mode asks for it (src/android/amd_linux.rs).
 *        By hand: LD_PRELOAD=/path/libuserptr_guard.so emulator ...
 *        RUSTEMUDROID_USERPTR_GUARD=0 disables it; RUSTEMUDROID_USERPTR_GUARD_LOG=1 prints a line every 64 buffers.
 *        RUSTEMUDROID_USERPTR_GUARD_TEST=allow-fork is for testing the second line of defence only: the buffers
 *        stay visible to fork(), so the driver rejects submissions as it does without this library.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#ifndef MADV_POPULATE_WRITE
#define MADV_POPULATE_WRITE 23
#endif

struct guard_gem_userptr {      /* struct drm_amdgpu_gem_userptr */
    uint64_t addr;
    uint64_t size;
    uint32_t flags;
    uint32_t handle;
};
struct guard_gem_close {        /* struct drm_gem_close */
    uint32_t handle;
    uint32_t pad;
};
struct guard_info {             /* struct drm_amdgpu_info: pointer, size, query and a 16 byte union */
    uint64_t return_pointer;
    uint32_t return_size;
    uint32_t query;
    uint32_t parameters[4];
};
struct guard_cs {               /* union drm_amdgpu_cs, input side; the output (a 64 bit handle) overlaps it */
    uint32_t ctx_id;
    uint32_t bo_list_handle;
    uint32_t num_chunks;
    uint32_t flags;
    uint64_t chunks;
};
struct guard_heap {             /* struct drm_amdgpu_heap_info */
    uint64_t total_heap_size;
    uint64_t usable_heap_size;
    uint64_t heap_usage;
    uint64_t max_allocation;
};
struct guard_memory {           /* struct drm_amdgpu_memory_info */
    struct guard_heap vram;
    struct guard_heap cpu_accessible_vram;
    struct guard_heap gtt;
};
struct guard_vram_gtt {         /* struct drm_amdgpu_info_vram_gtt */
    uint64_t vram_size;
    uint64_t vram_cpu_accessible_size;
    uint64_t gtt_size;
};
#define GUARD_IOCTL_AMDGPU_GEM_USERPTR _IOWR('d', 0x40 + 0x11, struct guard_gem_userptr)
#define GUARD_IOCTL_AMDGPU_INFO _IOW('d', 0x40 + 0x05, struct guard_info)
#define GUARD_IOCTL_GEM_CLOSE _IOW('d', 0x09, struct guard_gem_close)
#define GUARD_IOCTL_AMDGPU_CS _IOWR('d', 0x40 + 0x04, struct guard_cs)
#define GUARD_INFO_VRAM_GTT 0x14
#define GUARD_INFO_MEMORY 0x19

/* Where the kernel header is installed, check our private copies of its numbers and layouts at build time. */
#if defined(__has_include)
#if __has_include(<drm/amdgpu_drm.h>)
#include <drm/drm.h>
#include <drm/amdgpu_drm.h>
_Static_assert(GUARD_IOCTL_AMDGPU_GEM_USERPTR == DRM_IOCTL_AMDGPU_GEM_USERPTR, "userptr ioctl number");
_Static_assert(GUARD_IOCTL_AMDGPU_INFO == DRM_IOCTL_AMDGPU_INFO, "info ioctl number");
_Static_assert(GUARD_IOCTL_GEM_CLOSE == DRM_IOCTL_GEM_CLOSE, "close ioctl number");
_Static_assert(GUARD_IOCTL_AMDGPU_CS == DRM_IOCTL_AMDGPU_CS, "submission ioctl number");
_Static_assert(sizeof(struct guard_cs) == sizeof(union drm_amdgpu_cs), "submission size");
_Static_assert(GUARD_INFO_VRAM_GTT == AMDGPU_INFO_VRAM_GTT, "vram/gtt query number");
_Static_assert(GUARD_INFO_MEMORY == AMDGPU_INFO_MEMORY, "memory query number");
_Static_assert(sizeof(struct guard_memory) == sizeof(struct drm_amdgpu_memory_info), "memory info size");
_Static_assert(sizeof(struct guard_vram_gtt) == sizeof(struct drm_amdgpu_info_vram_gtt), "vram/gtt info size");
_Static_assert(sizeof(struct guard_gem_userptr) == sizeof(struct drm_amdgpu_gem_userptr), "userptr size");
#endif
#endif

/* The submission is sent again for this long before the library gives up. */
#ifndef GUARD_RETRY_LIMIT_MS
#define GUARD_RETRY_LIMIT_MS 10000
#endif

/*
 * The registered buffers, in a dense array: every lookup walks only the live entries, and the engine keeps a
 * handful of them (16 MiB each). An entry exists from before the driver is asked to register the buffer (it is
 * "pending" until the driver answers with the handle) to before the driver is asked to close it, so the closing
 * of one buffer always sees every other buffer that uses the same addresses.
 */
#ifndef GUARD_CAPACITY
#define GUARD_CAPACITY 16384
#endif
struct entry {
    int fd;
    int pending;
    uint32_t handle;
    uint64_t token;
    uint64_t addr;
    uint64_t size;
};
static struct entry entries[GUARD_CAPACITY];
static unsigned live;                   /* entries in use; guarded by lock */
static atomic_uint live_hint;           /* the same number, for the paths that only ask whether there is any */
static uint64_t next_token;             /* guarded by lock */
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_once_t once = PTHREAD_ONCE_INIT;
static int (*real_ioctl)(int, unsigned long, ...);
static int enabled, verbose, report_all_visible, test_allow_fork;
static unsigned long registered, closed, dropped, populate_failed, dontfork_failed;    /* guarded by lock */
static uint64_t bytes_now, bytes_peak;                                                  /* guarded by lock */
static atomic_ulong submissions_retried, submissions_lost;

#ifdef GUARD_SELFTEST
/* The test program supplies the driver. */
static int (*test_driver)(int fd, unsigned long request, void *argument);
#endif

static int driver(int fd, unsigned long request, void *argument) {
#ifdef GUARD_SELFTEST
    if (test_driver) {
        return test_driver(fd, request, argument);
    }
#endif
    if (real_ioctl) {
        return real_ioctl(fd, request, argument);
    }
    return (int)syscall(SYS_ioctl, fd, request, argument);
}

/* A child made by fork() has none of the registered ranges and must not wait for a lock another thread held. */
static void forget_everything_in_child(void) {
    pthread_mutex_init(&lock, NULL);
    live = 0;
    atomic_store(&live_hint, 0);
}

static void setup(void) {
    const char *value = getenv("RUSTEMUDROID_USERPTR_GUARD");
    const char *log = getenv("RUSTEMUDROID_USERPTR_GUARD_LOG");
    const char *visible = getenv("RUSTEMUDROID_AMD_VISIBLE_VRAM");
    const char *test = getenv("RUSTEMUDROID_USERPTR_GUARD_TEST");
    real_ioctl = dlsym(RTLD_NEXT, "ioctl");
    verbose = log && strcmp(log, "1") == 0;
    report_all_visible = visible && strcmp(visible, "all") == 0;
    test_allow_fork = test && strcmp(test, "allow-fork") == 0;
    enabled = !(value && strcmp(value, "0") == 0);
    pthread_atfork(NULL, NULL, forget_everything_in_child);
}

/* The helpers below are called with the lock held. */

static int find_registered(int fd, uint32_t handle) {
    for (unsigned index = 0; index < live; index++) {
        if (!entries[index].pending && entries[index].fd == fd && entries[index].handle == handle) {
            return (int)index;
        }
    }
    return -1;
}

static int find_token(uint64_t token) {
    for (unsigned index = 0; index < live; index++) {
        if (entries[index].token == token) {
            return (int)index;
        }
    }
    return -1;
}

static void remove_entry(unsigned index) {
    entries[index] = entries[--live];
    atomic_store(&live_hint, live);
}

/* Adds an entry and returns its token, or 0 when the table is full. */
static uint64_t add_entry(int fd, int pending, uint32_t handle, uint64_t addr, uint64_t size) {
    if (live == GUARD_CAPACITY) {
        if (dropped++ == 0) {
            fprintf(stderr, "RustEmuDroid: userptr guard: more than %d buffers, the rest is not protected\n",
                    GUARD_CAPACITY);
        }
        return 0;
    }
    const uint64_t token = ++next_token;
    entries[live++] = (struct entry){fd, pending, handle, token, addr, size};
    atomic_store(&live_hint, live);
    return token;
}

static int range_in_use(uint64_t addr, uint64_t size) {
    for (unsigned index = 0; index < live; index++) {
        if (entries[index].addr < addr + size && addr < entries[index].addr + entries[index].size) {
            return 1;
        }
    }
    return 0;
}

/* Gives a range back to fork(), unless a registered or pending buffer still uses some of it. */
static void release_range(uint64_t addr, uint64_t size) {
    if (!range_in_use(addr, size)) {
        madvise((void *)(uintptr_t)addr, size, MADV_DOFORK);
    }
}

static int register_buffer(int fd, unsigned long request, struct guard_gem_userptr *user) {
    const uint64_t addr = user->addr, size = user->size;
    const int saved = errno;
    pthread_mutex_lock(&lock);
    /* The entry and the mark are made together, so the closing of another buffer on these addresses keeps the mark.
     * A buffer the table has no room for gets no mark: nothing would take it away again. */
    const uint64_t token = add_entry(fd, 1, 0, addr, size);
    if (token != 0 && !test_allow_fork && madvise((void *)(uintptr_t)addr, size, MADV_DONTFORK) != 0) {
        dontfork_failed++;
    }
    pthread_mutex_unlock(&lock);
    const int populated = madvise((void *)(uintptr_t)addr, size, MADV_POPULATE_WRITE);
    errno = saved;
    const int result = driver(fd, request, user);
    const int failed = result ? errno : saved;

    unsigned long count = 0, closed_now = 0, dontfork_now = 0, populate_now = 0;
    uint64_t now = 0, peak = 0;
    pthread_mutex_lock(&lock);
    if (populated != 0) {
        populate_failed++;
    }
    const int index = token ? find_token(token) : -1;
    if (result == 0) {
        /* A leftover entry with this handle belongs to a buffer that went away without a close call. */
        const int leftover = find_registered(fd, user->handle);
        if (leftover >= 0) {
            const struct entry gone = entries[leftover];
            bytes_now -= gone.size;
            remove_entry((unsigned)leftover);
            release_range(gone.addr, gone.size);
        }
        const int current = token ? find_token(token) : -1;
        if (current >= 0) {
            entries[current].pending = 0;
            entries[current].handle = user->handle;
            bytes_now += size;
            if (bytes_now > bytes_peak) {
                bytes_peak = bytes_now;
            }
        }
        count = ++registered;
        now = bytes_now;
        peak = bytes_peak;
        closed_now = closed;
        dontfork_now = dontfork_failed;
        populate_now = populate_failed;
    } else {
        if (index >= 0) {
            remove_entry((unsigned)index);
            release_range(addr, size);
        }
    }
    pthread_mutex_unlock(&lock);
    if (count == 1 || (verbose && count != 0 && count % 64 == 0)) {
        fprintf(stderr,
                "RustEmuDroid: userptr guard active: %lu buffers registered, %lu closed, %llu MiB now, "
                "%llu MiB peak, dontfork_failed=%lu populate_failed=%lu\n",
                count, closed_now, (unsigned long long)(now >> 20), (unsigned long long)(peak >> 20), dontfork_now,
                populate_now);
    }
    errno = failed;
    return result;
}

static int close_buffer(int fd, unsigned long request, struct guard_gem_close *close_arg) {
    if (atomic_load(&live_hint) == 0) {
        return driver(fd, request, close_arg);
    }
    const int saved = errno;
    struct entry entry = {0};
    pthread_mutex_lock(&lock);
    const int index = find_registered(fd, close_arg->handle);
    if (index >= 0) {
        entry = entries[index];
        remove_entry((unsigned)index);
        /* Before the driver closes it: the memory can only go back to the allocator after this call returns. */
        release_range(entry.addr, entry.size);
        closed++;
        bytes_now -= entry.size;
    }
    pthread_mutex_unlock(&lock);
    errno = saved;
    const int result = driver(fd, request, close_arg);
    if (result != 0 && index >= 0) {
        /* The driver kept the buffer: keep protecting it. */
        const int failed = errno;
        pthread_mutex_lock(&lock);
        if (add_entry(entry.fd, 0, entry.handle, entry.addr, entry.size) != 0) {
            closed--;
            bytes_now += entry.size;
        }
        if (!test_allow_fork) {
            madvise((void *)(uintptr_t)entry.addr, entry.size, MADV_DONTFORK);
        }
        pthread_mutex_unlock(&lock);
        errno = failed;
    }
    return result;
}

/* Makes the pages of every registered buffer of this device present and writable again and returns how many
 * buffers the device has. The ranges are copied out in small groups so the lock is never held during the work;
 * a buffer closed meanwhile only costs a call that fails. */
static unsigned populate_all(int fd) {
    struct {
        uint64_t addr, size;
    } group[32];
    unsigned buffers = 0, position = 0;
    for (;;) {
        unsigned count = 0;
        pthread_mutex_lock(&lock);
        while (position < live && count < sizeof group / sizeof group[0]) {
            if (!entries[position].pending && entries[position].fd == fd) {
                group[count].addr = entries[position].addr;
                group[count].size = entries[position].size;
                count++;
            }
            position++;
        }
        const int finished = position >= live;
        pthread_mutex_unlock(&lock);
        for (unsigned index = 0; index < count; index++) {
            madvise((void *)(uintptr_t)group[index].addr, group[index].size, MADV_POPULATE_WRITE);
        }
        buffers += count;
        if (finished) {
            return buffers;
        }
    }
}

static uint64_t monotonic_ms(void) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (uint64_t)now.tv_sec * 1000 + (uint64_t)now.tv_nsec / 1000000;
}

/* A submission the driver rejected with EBUSY is sent again while the device has user-pointer buffers. */
static int submit_with_retry(int fd, unsigned long request, void *argument) {
    const struct guard_cs input = *(const struct guard_cs *)argument;
    int result = driver(fd, request, argument);
    if (result == 0 || errno != EBUSY || atomic_load(&live_hint) == 0) {
        return result;
    }
    const uint64_t start = monotonic_ms();
    unsigned attempts = 0;
    uint64_t elapsed = 0;
    do {
        if (populate_all(fd) == 0) {
            errno = EBUSY;          /* no user-pointer buffer on this device: not the case handled here */
            return result;
        }
        /* The driver normally waits about a second by itself before it answers; this pause only keeps an
         * immediate answer from turning the loop into a busy one. */
        const struct timespec pause = {0, (attempts < 50 ? attempts + 1 : 50) * 1000000L};
        nanosleep(&pause, NULL);
        attempts++;
        *(struct guard_cs *)argument = input;
        result = driver(fd, request, argument);
        elapsed = monotonic_ms() - start;
    } while (result != 0 && errno == EBUSY && elapsed < GUARD_RETRY_LIMIT_MS);
    const int failed = result ? errno : 0;
    const unsigned long count = result == 0 ? atomic_fetch_add(&submissions_retried, 1) + 1
                                            : atomic_fetch_add(&submissions_lost, 1) + 1;
    /* The first ones and then every power of two, so a recurring event does not flood the log. */
    if (result != 0 || count <= 8 || (count & (count - 1)) == 0) {
        fprintf(stderr,
                "RustEmuDroid: userptr guard: the driver rejected a submission (busy pages); %s after %u more "
                "attempts in %llu ms (%lu so far)\n",
                result == 0 ? "accepted" : "still rejected", attempts, (unsigned long long)elapsed, count);
    }
    if (result != 0) {
        errno = failed;
    }
    return result;
}

/* Reports the whole video memory as CPU visible in the two memory queries of the driver. */
static int query_with_full_window(int fd, unsigned long request, struct guard_info *info) {
    const int result = driver(fd, request, info);
    if (result != 0 || !info->return_pointer) {
        return result;
    }
    static atomic_flag announced = ATOMIC_FLAG_INIT;
    uint64_t window = 0, whole = 0;
    if (info->query == GUARD_INFO_MEMORY && info->return_size >= sizeof(struct guard_memory)) {
        struct guard_memory *memory = (struct guard_memory *)(uintptr_t)info->return_pointer;
        window = memory->cpu_accessible_vram.total_heap_size;
        whole = memory->vram.total_heap_size;
        if (window < whole) {
            memory->cpu_accessible_vram.total_heap_size = memory->vram.total_heap_size;
            memory->cpu_accessible_vram.usable_heap_size = memory->vram.usable_heap_size;
            memory->cpu_accessible_vram.max_allocation = memory->vram.max_allocation;
        }
    } else if (info->query == GUARD_INFO_VRAM_GTT && info->return_size >= sizeof(struct guard_vram_gtt)) {
        struct guard_vram_gtt *sizes = (struct guard_vram_gtt *)(uintptr_t)info->return_pointer;
        window = sizes->vram_cpu_accessible_size;
        whole = sizes->vram_size;
        if (window < whole) {
            sizes->vram_cpu_accessible_size = sizes->vram_size;
        }
    }
    if (window < whole && !atomic_flag_test_and_set(&announced)) {
        fprintf(stderr,
                "RustEmuDroid: userptr guard reports all %llu MiB of video memory as CPU visible "
                "(the window is %llu MiB)\n",
                (unsigned long long)(whole >> 20), (unsigned long long)(window >> 20));
    }
    return result;
}

int ioctl(int fd, unsigned long request, ...) {
    va_list list;
    va_start(list, request);
    void *argument = va_arg(list, void *);
    va_end(list);
    pthread_once(&once, setup);
    if (!enabled || !argument) {
        return driver(fd, request, argument);
    }
    if (request == GUARD_IOCTL_AMDGPU_CS) {
        return submit_with_retry(fd, request, argument);
    }
    if (request == GUARD_IOCTL_GEM_CLOSE) {
        return close_buffer(fd, request, argument);
    }
    if (request == GUARD_IOCTL_AMDGPU_GEM_USERPTR) {
        return register_buffer(fd, request, argument);
    }
    if (request == GUARD_IOCTL_AMDGPU_INFO && report_all_visible) {
        return query_with_full_window(fd, request, argument);
    }
    return driver(fd, request, argument);
}
