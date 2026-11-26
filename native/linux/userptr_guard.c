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
 * the buffer is closed, restore the default (MADV_DOFORK) before the memory goes back to the allocator.
 *
 * Second line of defence: the kernel driver answers a command submission with EBUSY when it could not get the
 * pages of a user-pointer buffer in time. Nothing was submitted in that case, and the Vulkan driver would report
 * the device as lost. The library makes the pages of the registered buffers present and writable again from
 * user space, where there is no time limit, and sends the same submission again, for at most 30 seconds. The
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
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <time.h>

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

#define SLOTS 65536
struct slot {
    int fd;
    uint32_t handle;
    uint64_t addr;
    uint64_t size;
};
static struct slot table[SLOTS];
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static int (*real_ioctl)(int, unsigned long, ...);
static int enabled = -1, verbose, report_all_visible, test_allow_fork;
static unsigned long registered, closed, populate_failed, dontfork_failed;
static uint64_t bytes_now, bytes_peak;

static void setup(void) {
    const char *value = getenv("RUSTEMUDROID_USERPTR_GUARD");
    const char *log = getenv("RUSTEMUDROID_USERPTR_GUARD_LOG");
    real_ioctl = dlsym(RTLD_NEXT, "ioctl");
    const char *visible = getenv("RUSTEMUDROID_AMD_VISIBLE_VRAM");
    const char *test = getenv("RUSTEMUDROID_USERPTR_GUARD_TEST");
    test_allow_fork = test && strcmp(test, "allow-fork") == 0;
    verbose = log && strcmp(log, "1") == 0;
    report_all_visible = visible && strcmp(visible, "all") == 0;
    enabled = !(value && strcmp(value, "0") == 0);
}

static unsigned hash(int fd, uint32_t handle) {
    return (((unsigned)fd * 2654435761u) ^ (handle * 40503u)) & (SLOTS - 1);
}

static void remember(int fd, uint32_t handle, uint64_t addr, uint64_t size) {
    unsigned index = hash(fd, handle);
    for (unsigned probe = 0; probe < SLOTS; probe++, index = (index + 1) & (SLOTS - 1)) {
        if (table[index].size == 0) {
            table[index] = (struct slot){fd, handle, addr, size};
            return;
        }
    }
}

static int forget(int fd, uint32_t handle, uint64_t *addr, uint64_t *size) {
    unsigned index = hash(fd, handle);
    for (unsigned probe = 0; probe < SLOTS; probe++, index = (index + 1) & (SLOTS - 1)) {
        if (table[index].size == 0 && table[index].addr == 0) {
            return 0;                       /* never used: end of the probe chain */
        }
        if (table[index].size != 0 && table[index].fd == fd && table[index].handle == handle) {
            *addr = table[index].addr;
            *size = table[index].size;
            table[index].size = 0;          /* tombstone: addr stays non-zero so later entries stay reachable */
            return 1;
        }
    }
    return 0;
}

/* Makes the pages of every registered buffer of this device present and writable again. Returns how many
 * buffers the device has; the lock keeps a buffer from being closed and its memory freed meanwhile. */
static unsigned populate_all(int fd) {
    unsigned buffers = 0;
    pthread_mutex_lock(&lock);
    for (unsigned index = 0; index < SLOTS; index++) {
        const struct slot *entry = &table[index];
        if (entry->size != 0 && entry->fd == fd) {
            madvise((void *)(uintptr_t)entry->addr, entry->size, MADV_POPULATE_WRITE);
            buffers++;
        }
    }
    pthread_mutex_unlock(&lock);
    return buffers;
}

static uint64_t monotonic_ms(void) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (uint64_t)now.tv_sec * 1000 + (uint64_t)now.tv_nsec / 1000000;
}

#define RETRY_LIMIT_MS 30000
static unsigned long submissions_retried, submissions_lost;

/* A submission the driver rejected with EBUSY is sent again while the device has user-pointer buffers. */
static int submit_with_retry(int fd, unsigned long request, void *argument) {
    const struct guard_cs input = *(const struct guard_cs *)argument;
    int result = real_ioctl(fd, request, argument);
    if (result == 0 || errno != EBUSY) {
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
        result = real_ioctl(fd, request, argument);
        elapsed = monotonic_ms() - start;
    } while (result != 0 && errno == EBUSY && elapsed < RETRY_LIMIT_MS);
    const int failed = result ? errno : 0;
    const unsigned long count = result == 0 ? __sync_add_and_fetch(&submissions_retried, 1)
                                            : __sync_add_and_fetch(&submissions_lost, 1);
    /* The first ones and then every power of two, so a recurring event does not flood the log. */
    if (result != 0 || count <= 8 || (count & (count - 1)) == 0) {
        fprintf(stderr,
                "RustEmuDroid: userptr guard: the driver rejected a submission (busy pages); %s after %u more "
                "attempts in %llu ms (%lu so far)\n",
                result == 0 ? "accepted" : "still rejected", attempts, (unsigned long long)elapsed, count);
    }
    errno = failed;
    return result;
}

int ioctl(int fd, unsigned long request, ...) {
    va_list list;
    va_start(list, request);
    void *argument = va_arg(list, void *);
    va_end(list);
    if (enabled < 0) {
        setup();
    }
    if (!enabled || !argument) {
        return real_ioctl(fd, request, argument);
    }
    if (request == GUARD_IOCTL_AMDGPU_CS) {
        return submit_with_retry(fd, request, argument);
    }
    if (request == GUARD_IOCTL_AMDGPU_GEM_USERPTR) {
        struct guard_gem_userptr *user = argument;
        const uint64_t addr = user->addr, size = user->size;
        int saved = errno;
        if (!test_allow_fork && madvise((void *)(uintptr_t)addr, size, MADV_DONTFORK) != 0) {
            __sync_fetch_and_add(&dontfork_failed, 1);
        }
        if (madvise((void *)(uintptr_t)addr, size, MADV_POPULATE_WRITE) != 0) {
            __sync_fetch_and_add(&populate_failed, 1);
        }
        errno = saved;
        int result = real_ioctl(fd, request, argument);
        if (result == 0) {
            pthread_mutex_lock(&lock);
            remember(fd, user->handle, addr, size);
            registered++;
            bytes_now += size;
            if (bytes_now > bytes_peak) {
                bytes_peak = bytes_now;
            }
            unsigned long count = registered;
            uint64_t now = bytes_now, peak = bytes_peak;
            pthread_mutex_unlock(&lock);
            if (count == 1 || (verbose && count % 64 == 0)) {
                fprintf(stderr,
                        "RustEmuDroid: userptr guard active: %lu buffers registered, %lu closed, %llu MiB now, "
                        "%llu MiB peak, dontfork_failed=%lu populate_failed=%lu\n",
                        count, closed, (unsigned long long)(now >> 20), (unsigned long long)(peak >> 20),
                        dontfork_failed, populate_failed);
            }
        } else {
            saved = errno;
            madvise((void *)(uintptr_t)addr, size, MADV_DOFORK);
            errno = saved;
        }
        return result;
    }
    if (request == GUARD_IOCTL_AMDGPU_INFO && report_all_visible) {
        struct guard_info *info = argument;
        int result = real_ioctl(fd, request, argument);
        if (result == 0 && info->return_pointer) {
            static int announced;
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
            if (window < whole && !__sync_lock_test_and_set(&announced, 1)) {
                fprintf(stderr,
                        "RustEmuDroid: userptr guard reports all %llu MiB of video memory as CPU visible "
                        "(the window is %llu MiB)\n",
                        (unsigned long long)(whole >> 20), (unsigned long long)(window >> 20));
            }
        }
        return result;
    }
    if (request == GUARD_IOCTL_GEM_CLOSE) {
        struct guard_gem_close *close_arg = argument;
        uint64_t addr = 0, size = 0;
        pthread_mutex_lock(&lock);
        int known = forget(fd, close_arg->handle, &addr, &size);
        if (known) {
            closed++;
            bytes_now -= size;
        }
        pthread_mutex_unlock(&lock);
        int result = real_ioctl(fd, request, argument);
        if (known) {
            int saved = errno;
            madvise((void *)(uintptr_t)addr, size, MADV_DOFORK);
            errno = saved;
        }
        return result;
    }
    return real_ioctl(fd, request, argument);
}
