/* Android x86-64: measure the CPU cost of writing identical data to a normal
 * allocation and to each host-visible type allowed for a Vulkan storage buffer. */
#include <inttypes.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <vulkan/vulkan.h>

enum { BYTES = 64 * 1024, PASSES = 64, ROUNDS = 6 };
struct target {
    int type;
    void *address;
    VkBuffer buffer;
    VkDeviceMemory memory;
};

static void check_vk(VkResult result, const char *where) {
    if (result != VK_SUCCESS) {
        fprintf(stderr, "%s returned Vulkan result %d\n", where, result);
        exit(2);
    }
}

static uint64_t now(clockid_t clock_id) {
    struct timespec t;
    if (clock_gettime(clock_id, &t) != 0) {
        perror("clock_gettime");
        exit(2);
    }
    return (uint64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}

/* Volatile 32-bit stores deliberately model many small writes. This does not
 * reproduce the instruction stream of the game's object initialization. */
__attribute__((noinline)) static void write_words(void *target, const uint32_t *source) {
    volatile uint32_t *dst = target;
    for (size_t i = 0; i < BYTES / sizeof(*source); ++i)
        dst[i] = source[i];
}

static void measure(const struct target *target, const uint32_t *src, int bulk, int round) {
    /* Pre-touch backing pages and make warmth independent of the previous target. */
    memset(target->address, 0xa5, BYTES);
    atomic_thread_fence(memory_order_seq_cst);
    uint64_t wall_start = now(CLOCK_MONOTONIC);
    uint64_t cpu_start = now(CLOCK_THREAD_CPUTIME_ID);
    for (int pass = 0; pass < PASSES; ++pass) {
        if (bulk)
            memcpy(target->address, src, BYTES);
        else
            write_words(target->address, src);
        __asm__ volatile("" : : "r"(target->address) : "memory");
    }
    atomic_thread_fence(memory_order_seq_cst);
    uint64_t cpu = now(CLOCK_THREAD_CPUTIME_ID) - cpu_start;
    uint64_t wall = now(CLOCK_MONOTONIC) - wall_start;
    if (memcmp(target->address, src, BYTES) != 0) {
        fprintf(stderr, "Write verification failed on type %d\n", target->type);
        exit(2);
    }
    printf("sample\t%d\t%d\t%s\t%" PRIu64 "\t%" PRIu64 "\n", round, target->type,
           bulk ? "memcpy" : "scalar32", wall, cpu);
}

int main(void) {
    VkApplicationInfo app = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
                             .pApplicationName = "RustEmuDroid mapped writes",
                             .apiVersion = VK_API_VERSION_1_0};
    VkInstanceCreateInfo ci = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
                               .pApplicationInfo = &app};
    VkInstance instance;
    check_vk(vkCreateInstance(&ci, NULL, &instance), "vkCreateInstance");
    uint32_t device_count = 0;
    check_vk(vkEnumeratePhysicalDevices(instance, &device_count, NULL), "physical device count");
    if (device_count != 1) {
        fprintf(stderr, "Expected one physical device, got %u\n", device_count);
        return 2;
    }
    VkPhysicalDevice gpu;
    check_vk(vkEnumeratePhysicalDevices(instance, &device_count, &gpu),
             "vkEnumeratePhysicalDevices");
    VkPhysicalDeviceProperties props;
    vkGetPhysicalDeviceProperties(gpu, &props);
    uint32_t queue_count = 0;
    vkGetPhysicalDeviceQueueFamilyProperties(gpu, &queue_count, NULL);
    if (queue_count == 0)
        return 2;
    VkQueueFamilyProperties *families = calloc(queue_count, sizeof(*families));
    if (!families)
        return 2;
    vkGetPhysicalDeviceQueueFamilyProperties(gpu, &queue_count, families);
    uint32_t family = 0;
    while (family < queue_count && !(families[family].queueFlags & VK_QUEUE_GRAPHICS_BIT))
        ++family;
    free(families);
    if (family == queue_count) {
        fprintf(stderr, "No graphics family\n");
        return 2;
    }
    float priority = 1.f;
    VkDeviceQueueCreateInfo qi = {.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
                                  .queueFamilyIndex = family,
                                  .queueCount = 1,
                                  .pQueuePriorities = &priority};
    VkDeviceCreateInfo di = {.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
                             .queueCreateInfoCount = 1,
                             .pQueueCreateInfos = &qi};
    VkDevice device;
    check_vk(vkCreateDevice(gpu, &di, NULL, &device), "vkCreateDevice");
    VkBufferCreateInfo bi = {.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
                             .size = BYTES,
                             .sharingMode = VK_SHARING_MODE_EXCLUSIVE,
                             .usage = VK_BUFFER_USAGE_STORAGE_BUFFER_BIT |
                                      VK_BUFFER_USAGE_TRANSFER_SRC_BIT};
    VkBuffer reference;
    check_vk(vkCreateBuffer(device, &bi, NULL, &reference), "reference buffer");
    VkMemoryRequirements requirements;
    vkGetBufferMemoryRequirements(device, reference, &requirements);
    vkDestroyBuffer(device, reference, NULL);
    VkPhysicalDeviceMemoryProperties memory;
    vkGetPhysicalDeviceMemoryProperties(gpu, &memory);
    printf("meta\t%s\t%u\t%u\t%u\t%u\t%u\t%u\t%" PRIu64 "\t%" PRIu64 "\n", props.deviceName,
           props.vendorID, family, BYTES, PASSES, ROUNDS, requirements.memoryTypeBits,
           requirements.size, requirements.alignment);
    struct target targets[VK_MAX_MEMORY_TYPES + 1] = {{.type = -1}};
    size_t target_count = 1;
    uint32_t *source = NULL;
    if (posix_memalign((void **)&source, 4096, BYTES) ||
        posix_memalign(&targets[0].address, 4096, BYTES))
        return 2;
    uint32_t state = 123456789u;
    for (size_t i = 0; i < BYTES / sizeof(*source); ++i)
        source[i] = state = (1664525u * state + 1013904223u);
    for (uint32_t i = 0; i < memory.memoryTypeCount; ++i) {
        VkMemoryType type = memory.memoryTypes[i];
        if (!(requirements.memoryTypeBits & (1u << i)) ||
            !(type.propertyFlags & VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT))
            continue;
        struct target *slot = &targets[target_count++];
        slot->type = (int)i;
        check_vk(vkCreateBuffer(device, &bi, NULL, &slot->buffer), "vkCreateBuffer");
        VkMemoryRequirements actual;
        vkGetBufferMemoryRequirements(device, slot->buffer, &actual);
        if (actual.size != requirements.size || actual.alignment != requirements.alignment ||
            actual.memoryTypeBits != requirements.memoryTypeBits)
            return 2;
        VkMemoryAllocateInfo ai = {.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
                                   .allocationSize = actual.size,
                                   .memoryTypeIndex = i};
        check_vk(vkAllocateMemory(device, &ai, NULL, &slot->memory), "vkAllocateMemory");
        check_vk(vkBindBufferMemory(device, slot->buffer, slot->memory, 0), "vkBindBufferMemory");
        check_vk(vkMapMemory(device, slot->memory, 0, BYTES, 0, &slot->address), "vkMapMemory");
        printf("type\t%u\t%u\t%u\n", i, type.heapIndex, type.propertyFlags);
    }
    if (target_count < 2) {
        fprintf(stderr, "No host-visible storage memory type\n");
        return 2;
    }
    for (int round = 0; round < ROUNDS; ++round) {
        for (size_t i = 0; i < target_count; ++i) {
            size_t index = (round % 2) == 0 ? i : target_count - 1 - i;
            int first = (round + (int)i) % 2;
            measure(&targets[index], source, first, round);
            measure(&targets[index], source, !first, round);
        }
    }
    for (size_t i = 1; i < target_count; ++i) {
        vkUnmapMemory(device, targets[i].memory);
        vkDestroyBuffer(device, targets[i].buffer, NULL);
        vkFreeMemory(device, targets[i].memory, NULL);
    }
    free(source);
    free(targets[0].address);
    vkDestroyDevice(device, NULL);
    vkDestroyInstance(instance, NULL);
    puts("complete");
    return 0;
}
