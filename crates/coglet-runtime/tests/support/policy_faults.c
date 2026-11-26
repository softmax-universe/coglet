// Subprocess-local Linux fault injection for ADR-0033 producer tests. Loaded only
// through LD_PRELOAD into a test-spawned cogletd. No production switch exists.
//
// WAL: while COGLET_FAULT_ARM exists, a write to the descriptor whose
//   /proc/self/fd target is EXACTLY COGLET_FAULT_WAL allows
//   COGLET_FAULT_LEFT bytes in total (counted per process), then fails with
//   ENOSPC. Each injection appends one byte to COGLET_FAULT_HITS.
// Clock: while COGLET_FAULT_CLOCK exists, CLOCK_BOOTTIME is changed by its
//   one-line contents: "sub <seconds>" or "set <seconds> <nanos>".
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <time.h>
#include <unistd.h>

static int exists(const char *path) {
    return path && syscall(SYS_faccessat, AT_FDCWD, path, F_OK, 0) == 0;
}
static int targeted(int fd) {
    const char *arm = getenv("COGLET_FAULT_ARM");
    const char *wal = getenv("COGLET_FAULT_WAL");
    if (!arm || !wal || !exists(arm)) return 0;
    char link[64], path[4096];
    snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
    ssize_t n = readlink(link, path, sizeof(path) - 1);
    if (n < 0) return 0;
    path[n] = 0;
    return strcmp(path, wal) == 0;
}
static void hit(void) {
    const char *hits = getenv("COGLET_FAULT_HITS");
    if (!hits) return;
    int fd = (int)syscall(SYS_openat, AT_FDCWD, hits, O_WRONLY | O_CREAT | O_APPEND, 0600);
    if (fd >= 0) {
        (void)!syscall(SYS_write, fd, "x", 1);
        syscall(SYS_close, fd);
    }
}
static size_t allowance(size_t count) {
    static long left = -1;
    if (left < 0) {
        const char *value = getenv("COGLET_FAULT_LEFT");
        left = value ? atol(value) : 0;
    }
    size_t take = (size_t)left < count ? (size_t)left : count;
    left -= (long)take;
    return take;
}
ssize_t write(int fd, const void *buf, size_t count) {
    if (targeted(fd)) {
        size_t take = allowance(count);
        if (take == 0) {
            hit();
            errno = ENOSPC;
            return -1;
        }
        return syscall(SYS_write, fd, buf, take);
    }
    return syscall(SYS_write, fd, buf, count);
}
ssize_t pwrite(int fd, const void *buf, size_t count, off_t offset) {
    if (targeted(fd)) {
        hit();
        errno = ENOSPC;
        return -1;
    }
    return syscall(SYS_pwrite64, fd, buf, count, offset);
}
ssize_t pwrite64(int fd, const void *buf, size_t count, off_t offset) {
    return pwrite(fd, buf, count, offset);
}
ssize_t writev(int fd, const struct iovec *iov, int count) {
    if (targeted(fd)) {
        hit();
        errno = ENOSPC;
        return -1;
    }
    return syscall(SYS_writev, fd, iov, count);
}
int clock_gettime(clockid_t clock, struct timespec *out) {
    long result = syscall(SYS_clock_gettime, clock, out);
    const char *file = getenv("COGLET_FAULT_CLOCK");
    if (result != 0 || clock != CLOCK_BOOTTIME || !exists(file)) return (int)result;
    int fd = (int)syscall(SYS_openat, AT_FDCWD, file, O_RDONLY, 0);
    if (fd < 0) return (int)result;
    char text[128];
    long n = syscall(SYS_read, fd, text, sizeof(text) - 1);
    syscall(SYS_close, fd);
    if (n <= 0) return (int)result;
    text[n] = 0;
    long long a = 0, b = 0;
    if (sscanf(text, "sub %lld", &a) == 1) {
        out->tv_sec -= (time_t)a;
    } else if (sscanf(text, "set %lld %lld", &a, &b) == 2) {
        out->tv_sec = (time_t)a;
        out->tv_nsec = (long)b;
    }
    return 0;
}
