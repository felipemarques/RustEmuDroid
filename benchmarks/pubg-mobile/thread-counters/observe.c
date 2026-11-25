// Sample ordinary procfs accounting for the selected PUBG process on Android.
#include <dirent.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static double seconds(clockid_t id) {
    struct timespec ts;
    if (clock_gettime(id, &ts) != 0) exit(9);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}

static int line(const char *path, char *dst, size_t cap) {
    FILE *f = fopen(path, "r");
    if (!f) return 0;
    char *found = fgets(dst, cap, f);
    fclose(f);
    if (!found) return 0;
    dst[strcspn(dst, "\r\n")] = 0;
    return 1;
}

static long parse(const char *str, long lower, long upper) {
    char *end;
    errno = 0;
    long value = strtol(str, &end, 10);
    if (errno != 0 || end == str || *end || value < lower || value > upper) exit(2);
    return value;
}

int main(int argc, char **argv) {
    if (argc != 3 && argc != 4) return 2;
    long pid = parse(argv[1], 1, INT_MAX);
    long samples = parse(argv[2], 2, 3600);
    long delay_ms = argc == 4 ? parse(argv[3], 25, 1000) : 200;
    if (samples * delay_ms > 90000) return 2;
    char dir[80], path[128], buf[4096], paths[65][128];
    snprintf(path, sizeof path, "/proc/%ld/cmdline", pid);
    if (!line(path, buf, sizeof buf) || strcmp(buf, "com.tencent.ig")) return 3;
    snprintf(dir, sizeof dir, "/proc/%ld/task", pid);
    DIR *tasks = opendir(dir);
    if (!tasks) return 4;
    int n = 1;
    snprintf(paths[0], sizeof paths[0], "/proc/%ld/stat", pid);
    struct dirent *e;
    while ((e = readdir(tasks))) {
        char *end;
        long tid = strtol(e->d_name, &end, 10);
        if (*end || tid < 1 || tid > INT_MAX) continue;
        snprintf(path, sizeof path, "%s/%ld/comm", dir, tid);
        if (!line(path, buf, sizeof buf)) continue;
        if (strcmp(buf, "UEGameThread") && strcmp(buf, "RHIThread") &&
            strcmp(buf, "RenderThread 1") && strcmp(buf, "FAsyncLoading")) continue;
        if (n == 65) return 5;
        snprintf(paths[n++], sizeof paths[0], "%s/%ld/stat", dir, tid);
    }
    closedir(tasks);
    if (n == 1) return 4;
    const struct timespec delay = {.tv_sec = delay_ms / 1000, .tv_nsec = (delay_ms % 1000) * 1000000};
    printf("H sample_interval_ms %ld clock monotonic tick_hz %ld realtime %.9f boottime %.9f\n",
           delay_ms, sysconf(_SC_CLK_TCK), seconds(CLOCK_REALTIME), seconds(CLOCK_BOOTTIME));
    for (long i = 0; i < samples; ++i) {
        printf("B %ld %.9f\n", i, seconds(CLOCK_MONOTONIC));
        for (int j = 0; j < n; ++j) {
            if (!line(paths[j], buf, sizeof buf)) return 6;
            printf("P %s %s\n", paths[j], buf);
        }
        printf("E %ld %.9f\n", i, seconds(CLOCK_MONOTONIC));
        if (fflush(stdout) == EOF) return 7;
        if (i + 1 < samples) {
            struct timespec remaining = delay;
            while (nanosleep(&remaining, &remaining) == -1) if (errno != EINTR) return 8;
        }
    }
    return 0;
}
