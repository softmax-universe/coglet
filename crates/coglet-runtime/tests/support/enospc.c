// Linux subprocess-only injection: short WAL write then real ENOSPC errno.
// No mounts, privileges, host disk filling or production fault switches.
#define _GNU_SOURCE
#include <unistd.h>
#include <sys/syscall.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <errno.h>
ssize_t write(int fd, const void *buf, size_t count) {
    const char *marker = getenv("COGLET_FAULT_DISK_MARKER");
    const char *wal = getenv("COGLET_FAULT_DISK_WAL");
    if (marker && wal && access(marker, F_OK) == 0) {
        char link[64], path[4096];
        snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
        ssize_t n = readlink(link, path, sizeof(path)-1);
        if (n >= 0) {
            path[n] = 0;
            if (strcmp(path, wal) == 0) {
                static size_t left = 7;
                if (!left) { errno = ENOSPC; return -1; }
                if (count > left) count = left;
                ssize_t result = syscall(SYS_write, fd, buf, count);
                if (result > 0) left -= (size_t)result;
                return result;
            }
        }
    }
    return syscall(SYS_write, fd, buf, count);
}
