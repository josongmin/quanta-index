// SPDX-License-Identifier: UNLICENSED
// Apply a Landlock write domain to the calling thread and its future children.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/landlock.h>
#include <stdint.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef LANDLOCK_ACCESS_FS_REFER
#define LANDLOCK_ACCESS_FS_REFER (1ULL << 13)
#endif
#ifndef LANDLOCK_ACCESS_FS_TRUNCATE
#define LANDLOCK_ACCESS_FS_TRUNCATE (1ULL << 14)
#endif

#define QUANTA_HANDLED_WRITES ( \
    LANDLOCK_ACCESS_FS_WRITE_FILE | LANDLOCK_ACCESS_FS_REMOVE_DIR | \
    LANDLOCK_ACCESS_FS_REMOVE_FILE | LANDLOCK_ACCESS_FS_MAKE_CHAR | \
    LANDLOCK_ACCESS_FS_MAKE_DIR | LANDLOCK_ACCESS_FS_MAKE_REG | \
    LANDLOCK_ACCESS_FS_MAKE_SOCK | LANDLOCK_ACCESS_FS_MAKE_FIFO | \
    LANDLOCK_ACCESS_FS_MAKE_BLOCK | LANDLOCK_ACCESS_FS_MAKE_SYM | \
    LANDLOCK_ACCESS_FS_REFER | LANDLOCK_ACCESS_FS_TRUNCATE)

#define QUANTA_ROOT_WRITES ( \
    LANDLOCK_ACCESS_FS_WRITE_FILE | LANDLOCK_ACCESS_FS_TRUNCATE | \
    LANDLOCK_ACCESS_FS_REMOVE_DIR | LANDLOCK_ACCESS_FS_REMOVE_FILE | \
    LANDLOCK_ACCESS_FS_MAKE_DIR | LANDLOCK_ACCESS_FS_MAKE_REG | \
    LANDLOCK_ACCESS_FS_REFER)

static int add_path_rule(int ruleset_fd, int parent_fd, uint64_t rights) {
    struct landlock_path_beneath_attr rule = {
        .allowed_access = rights,
        .parent_fd = parent_fd,
    };
    if (syscall(__NR_landlock_add_rule, ruleset_fd,
                LANDLOCK_RULE_PATH_BENEATH, &rule, 0) < 0)
        return errno;
    return 0;
}

int quanta_restrict_state_root_writes(int root_fd, int socket_fd,
                                      int has_socket_parent) {
    struct stat root_stat, socket_stat;
    struct landlock_ruleset_attr ruleset = {
        .handled_access_fs = QUANTA_HANDLED_WRITES,
    };
    int ruleset_fd, error;
    long abi;

    if (has_socket_parent != 0 && has_socket_parent != 1)
        return EINVAL;
    if (fstat(root_fd, &root_stat) < 0)
        return errno;
    if (!S_ISDIR(root_stat.st_mode))
        return ENOTDIR;
    if (has_socket_parent) {
        if (root_fd == socket_fd)
            return EINVAL;
        if (fstat(socket_fd, &socket_stat) < 0)
            return errno;
        if (!S_ISDIR(socket_stat.st_mode))
            return ENOTDIR;
        if (root_stat.st_dev == socket_stat.st_dev &&
            root_stat.st_ino == socket_stat.st_ino)
            return EINVAL;
    }

    abi = syscall(__NR_landlock_create_ruleset, NULL, 0,
                  LANDLOCK_CREATE_RULESET_VERSION);
    if (abi < 0)
        return errno;
    if (abi < 3)
        return EOPNOTSUPP;
    ruleset_fd = (int)syscall(__NR_landlock_create_ruleset,
                              &ruleset, sizeof(ruleset), 0);
    if (ruleset_fd < 0)
        return errno;

    error = add_path_rule(ruleset_fd, root_fd, QUANTA_ROOT_WRITES);
    if (error == 0 && has_socket_parent)
        error = add_path_rule(ruleset_fd, socket_fd,
                              LANDLOCK_ACCESS_FS_MAKE_SOCK |
                              LANDLOCK_ACCESS_FS_REMOVE_FILE);
    if (error == 0 && prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0)
        error = errno;
    if (error == 0 && syscall(__NR_landlock_restrict_self, ruleset_fd, 0) < 0)
        error = errno;
    // Linux closes the descriptor even if close reports EINTR. Keep the
    // policy syscall's errno as the result; close cannot undo a restriction.
    close(ruleset_fd);
    return error;
}
