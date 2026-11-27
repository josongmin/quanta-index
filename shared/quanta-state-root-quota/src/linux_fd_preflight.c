// SPDX-License-Identifier: UNLICENSED
// Startup-only check. The caller must not have spawned concurrent threads.
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <linux/magic.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/vfs.h>
#include <unistd.h>

int quanta_require_stock_sqlite_descriptor_preflight(unsigned int max_process_fds) {
    DIR *directory = opendir("/proc/self/fd");
    struct dirent *entry;
    struct stat fd_stat;
    struct statfs fs_stat;
    int scan_fd, error = 0;

    if (directory == NULL)
        return errno;
    scan_fd = dirfd(directory);
    if (scan_fd < 0) {
        error = errno;
        goto done;
    }
    if (fstatfs(scan_fd, &fs_stat) < 0) {
        error = errno;
        goto done;
    }
    if ((unsigned long)fs_stat.f_type != PROC_SUPER_MAGIC) {
        error = EOPNOTSUPP;
        goto done;
    }

    for (;;) {
        char *end;
        long number;
        int fd, descriptor_flags, flags;

        errno = 0;
        entry = readdir(directory);
        if (entry == NULL) {
            if (errno != 0)
                error = errno;
            break;
        }
        if (entry->d_name[0] == '.' &&
            (entry->d_name[1] == '\0' ||
             (entry->d_name[1] == '.' && entry->d_name[2] == '\0')))
            continue;
        errno = 0;
        number = strtol(entry->d_name, &end, 10);
        if (errno != 0 || *end != '\0' || number < 0 || number > INT_MAX) {
            error = EOVERFLOW;
            break;
        }
        fd = (int)number;
        /* Lowering RLIMIT_NOFILE does not close descriptors already open. */
        if ((unsigned long)number >= max_process_fds) {
            error = EMFILE;
            break;
        }
        if (fd <= STDERR_FILENO || fd == scan_fd)
            continue;
        descriptor_flags = fcntl(fd, F_GETFD);
        if (descriptor_flags < 0) {
            error = errno;
            break;
        }
        if ((descriptor_flags & FD_CLOEXEC) != 0)
            continue;
        flags = fcntl(fd, F_GETFL);
        if (flags < 0) {
            error = errno;
            break;
        }
        if ((flags & O_ACCMODE) == O_RDONLY)
            continue;
        if (fstat(fd, &fd_stat) < 0) {
            error = errno;
            break;
        }
        if (S_ISREG(fd_stat.st_mode)) {
            error = EBUSY;
            break;
        }
    }

done:
    if (closedir(directory) < 0 && error == 0)
        error = errno;
    return error;
}
