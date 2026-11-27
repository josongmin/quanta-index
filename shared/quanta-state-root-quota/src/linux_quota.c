// SPDX-License-Identifier: UNLICENSED
// Read-only Linux XFS UAPI adapter. Inputs are already-held file descriptors.
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/dqblk_xfs.h>
#include <linux/capability.h>
#include <linux/fs.h>
#include <linux/magic.h>
#include <linux/quota.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <poll.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/vfs.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>
#include <stdlib.h>

struct quanta_xfs_membership {
    uint32_t project_id;
    uint8_t inherits_project;
    uint8_t reserved[3];
};

struct quanta_xfs_quota {
    uint32_t project_id;
    uint8_t inherits_project;
    uint8_t accounting_enabled;
    uint8_t enforcement_enabled;
    uint8_t reserved;
    uint64_t block_hard_limit_bytes;
    uint64_t inode_hard_limit;
    uint64_t used_bytes;
    uint64_t used_inodes;
};

_Static_assert(sizeof(struct quanta_xfs_membership) == 8, "membership ABI changed");
_Static_assert(sizeof(struct quanta_xfs_quota) == 40, "quota ABI changed");

static int read_membership(int fd, struct quanta_xfs_membership *out) {
    struct stat st;
    struct statfs fs;
    struct fsxattr xattr;
    if (out == NULL)
        return EINVAL;
    if (fstat(fd, &st) < 0)
        return errno;
    if (!S_ISDIR(st.st_mode) && !S_ISREG(st.st_mode))
        return EINVAL;
    if (fstatfs(fd, &fs) < 0)
        return errno;
    if ((unsigned long)fs.f_type != XFS_SUPER_MAGIC)
        return EOPNOTSUPP;
    memset(&xattr, 0, sizeof(xattr));
    if (ioctl(fd, FS_IOC_FSGETXATTR, &xattr) < 0)
        return errno;
    if (xattr.fsx_projid == 0)
        return ENODATA;
    out->project_id = xattr.fsx_projid;
    out->inherits_project = (xattr.fsx_xflags & FS_XFLAG_PROJINHERIT) != 0;
    memset(out->reserved, 0, sizeof(out->reserved));
    return 0;
}

int quanta_read_xfs_project_membership(int fd, struct quanta_xfs_membership *out) {
    return read_membership(fd, out);
}

int quanta_read_xfs_project_quota(int fd, struct quanta_xfs_quota *out) {
    struct stat st;
    struct fsxattr xattr;
    struct fs_quota_statv status;
    struct fs_quota_statv status_after;
    fs_disk_quota_t disk_quota;
    struct quanta_xfs_membership before, after;
    int error;
    if (out == NULL)
        return EINVAL;
    if (fstat(fd, &st) < 0)
        return errno;
    if (!S_ISDIR(st.st_mode))
        return ENOTDIR;
    error = read_membership(fd, &before);
    if (error != 0)
        return error;
    if (!before.inherits_project)
        return ENODATA;
    memset(&xattr, 0, sizeof(xattr));
    if (ioctl(fd, FS_IOC_FSGETXATTR, &xattr) < 0)
        return errno;
    if ((xattr.fsx_xflags & FS_XFLAG_RTINHERIT) != 0)
        return EOPNOTSUPP;
#ifndef __NR_quotactl_fd
    return ENOSYS;
#else
    memset(&status, 0, sizeof(status));
    status.qs_version = FS_QSTATV_VERSION1;
    if (syscall(__NR_quotactl_fd, fd, QCMD(Q_XGETQSTATV, PRJQUOTA), 0, &status) < 0)
        return errno;
    if (status.qs_version != FS_QSTATV_VERSION1)
        return EPROTO;
    if ((status.qs_flags & (FS_QUOTA_PDQ_ACCT | FS_QUOTA_PDQ_ENFD)) !=
        (FS_QUOTA_PDQ_ACCT | FS_QUOTA_PDQ_ENFD))
        return ENODATA;
    memset(&disk_quota, 0, sizeof(disk_quota));
    if (syscall(__NR_quotactl_fd, fd, QCMD(Q_XGETQUOTA, PRJQUOTA),
                before.project_id, &disk_quota) < 0)
        return errno;
    if (disk_quota.d_version != FS_DQUOT_VERSION ||
        disk_quota.d_flags != FS_PROJ_QUOTA ||
        disk_quota.d_id != before.project_id)
        return EPROTO;
    if (disk_quota.d_blk_hardlimit == 0 ||
        disk_quota.d_blk_hardlimit > UINT64_MAX / 512 ||
        disk_quota.d_bcount > UINT64_MAX / 512 ||
        disk_quota.d_ino_hardlimit == 0)
        return ERANGE;
    memset(&status_after, 0, sizeof(status_after));
    status_after.qs_version = FS_QSTATV_VERSION1;
    if (syscall(__NR_quotactl_fd, fd, QCMD(Q_XGETQSTATV, PRJQUOTA),
                0, &status_after) < 0)
        return errno;
    if (status_after.qs_version != FS_QSTATV_VERSION1 ||
        (status_after.qs_flags & (FS_QUOTA_PDQ_ACCT | FS_QUOTA_PDQ_ENFD)) !=
        (FS_QUOTA_PDQ_ACCT | FS_QUOTA_PDQ_ENFD))
        return ESTALE;
    error = read_membership(fd, &after);
    if (error != 0)
        return error;
    if (after.project_id != before.project_id ||
        after.inherits_project != before.inherits_project)
        return ESTALE;
    memset(out, 0, sizeof(*out));
    out->project_id = before.project_id;
    out->inherits_project = before.inherits_project;
    out->accounting_enabled = 1;
    out->enforcement_enabled = 1;
    out->block_hard_limit_bytes = disk_quota.d_blk_hardlimit * 512;
    out->inode_hard_limit = disk_quota.d_ino_hardlimit;
    out->used_bytes = disk_quota.d_bcount * 512;
    out->used_inodes = disk_quota.d_icount;
    return 0;
#endif
}

int quanta_socket_peer_uid(int socket_fd, uint32_t *out) {
    struct ucred credentials;
    socklen_t length = sizeof(credentials);
    if (out == NULL)
        return EINVAL;
    if (getsockopt(socket_fd, SOL_SOCKET, SO_PEERCRED, &credentials, &length) < 0)
        return errno;
    if (length != sizeof(credentials))
        return EPROTO;
    *out = credentials.uid;
    return 0;
}

int quanta_require_unprivileged_xfs_writer(void) {
    struct __user_cap_header_struct header;
    struct __user_cap_data_struct data[2];
    const uint32_t forbidden = (1U << CAP_SYS_ADMIN) | (1U << CAP_SYS_RESOURCE);
    if (geteuid() == 0)
        return EPERM;
#ifndef __NR_capget
    return ENOSYS;
#else
    memset(&header, 0, sizeof(header));
    memset(data, 0, sizeof(data));
    header.version = _LINUX_CAPABILITY_VERSION_3;
    header.pid = 0;
    if (syscall(__NR_capget, &header, data) < 0)
        return errno;
    if (header.version != _LINUX_CAPABILITY_VERSION_3)
        return EPROTO;
    if (((data[0].effective | data[0].permitted) & forbidden) != 0)
        return EPERM;
    return 0;
#endif
}

int quanta_require_privileged_xfs_reader(void) {
    struct __user_cap_header_struct header;
    struct __user_cap_data_struct data[2];
    if (geteuid() != 0)
        return EPERM;
#ifndef __NR_capget
    return ENOSYS;
#else
    memset(&header, 0, sizeof(header));
    memset(data, 0, sizeof(data));
    header.version = _LINUX_CAPABILITY_VERSION_3;
    header.pid = 0;
    if (syscall(__NR_capget, &header, data) < 0)
        return errno;
    if (header.version != _LINUX_CAPABILITY_VERSION_3)
        return EPROTO;
    if ((data[0].effective & (1U << CAP_SYS_ADMIN)) == 0)
        return EPERM;
    return 0;
#endif
}

int quanta_connect_unix_socket(const char *path, uint32_t timeout_ms, int *out) {
    struct sockaddr_un address;
    struct timespec now;
    struct pollfd wait_fd;
    int fd, error, remaining, state;
    socklen_t length = sizeof(error);
    int64_t deadline;
    size_t path_length;
    if (path == NULL || out == NULL || timeout_ms == 0 || timeout_ms > 5000)
        return EINVAL;
    *out = -1;
    path_length = strlen(path);
    if (path_length == 0 || path_length >= sizeof(address.sun_path))
        return ENAMETOOLONG;
    fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (fd < 0)
        return errno;
    memset(&address, 0, sizeof(address));
    address.sun_family = AF_UNIX;
    memcpy(address.sun_path, path, path_length + 1);
    state = connect(fd, (struct sockaddr *)&address,
                    offsetof(struct sockaddr_un, sun_path) + path_length + 1);
    if (state < 0 && errno != EINPROGRESS) {
        error = errno;
        close(fd);
        return error;
    }
    if (state < 0) {
        if (clock_gettime(CLOCK_MONOTONIC, &now) < 0) {
            error = errno;
            close(fd);
            return error;
        }
        deadline = (int64_t)now.tv_sec * 1000 + now.tv_nsec / 1000000 + timeout_ms;
        wait_fd.fd = fd;
        wait_fd.events = POLLOUT;
        do {
            if (clock_gettime(CLOCK_MONOTONIC, &now) < 0) {
                error = errno;
                close(fd);
                return error;
            }
            remaining = deadline - ((int64_t)now.tv_sec * 1000 + now.tv_nsec / 1000000);
            if (remaining <= 0) {
                close(fd);
                return ETIMEDOUT;
            }
            state = poll(&wait_fd, 1, remaining);
        } while (state < 0 && errno == EINTR);
        if (state < 0) {
            error = errno;
            close(fd);
            return error;
        }
        if (state == 0) {
            close(fd);
            return ETIMEDOUT;
        }
        if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &length) < 0) {
            error = errno;
            close(fd);
            return error;
        }
        if (length != sizeof(error) || error != 0) {
            close(fd);
            return error != 0 ? error : EPROTO;
        }
    }
    if (fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) & ~O_NONBLOCK) < 0) {
        error = errno;
        close(fd);
        return error;
    }
    *out = fd;
    return 0;
}

int quanta_send_root_fd(int socket_fd, int root_fd) {
    union {
        struct cmsghdr align;
        unsigned char bytes[CMSG_SPACE(sizeof(int))];
    } control;
    struct msghdr message;
    struct iovec vector;
    struct cmsghdr *header;
    char payload = 'Q';
    ssize_t sent;
    memset(&control, 0, sizeof(control));
    memset(&message, 0, sizeof(message));
    vector.iov_base = &payload;
    vector.iov_len = 1;
    message.msg_iov = &vector;
    message.msg_iovlen = 1;
    message.msg_control = control.bytes;
    message.msg_controllen = sizeof(control.bytes);
    header = CMSG_FIRSTHDR(&message);
    if (header == NULL)
        return EPROTO;
    header->cmsg_level = SOL_SOCKET;
    header->cmsg_type = SCM_RIGHTS;
    header->cmsg_len = CMSG_LEN(sizeof(int));
    memcpy(CMSG_DATA(header), &root_fd, sizeof(root_fd));
    sent = sendmsg(socket_fd, &message, MSG_NOSIGNAL);
    if (sent < 0)
        return errno;
    return sent == 1 ? 0 : EPROTO;
}

int quanta_receive_root_fd(int socket_fd, int *out) {
    union {
        struct cmsghdr align;
        unsigned char bytes[CMSG_SPACE(sizeof(int) * 2)];
    } control;
    struct msghdr message;
    struct iovec vector;
    struct cmsghdr *header;
    char payload = 0;
    ssize_t received;
    int descriptors[2] = {-1, -1};
    size_t count = 0;
    int invalid = 0;
    if (out == NULL)
        return EINVAL;
    *out = -1;
    memset(&control, 0, sizeof(control));
    memset(&message, 0, sizeof(message));
    vector.iov_base = &payload;
    vector.iov_len = 1;
    message.msg_iov = &vector;
    message.msg_iovlen = 1;
    message.msg_control = control.bytes;
    message.msg_controllen = sizeof(control.bytes);
    do {
        received = recvmsg(socket_fd, &message, MSG_CMSG_CLOEXEC);
    } while (received < 0 && errno == EINTR);
    if (received < 0)
        return errno;
    for (header = CMSG_FIRSTHDR(&message); header != NULL;
         header = CMSG_NXTHDR(&message, header)) {
        size_t next;
        if (header->cmsg_level != SOL_SOCKET ||
            header->cmsg_type != SCM_RIGHTS ||
            header->cmsg_len < CMSG_LEN(0)) {
            invalid = 1;
            continue;
        }
        if ((header->cmsg_len - CMSG_LEN(0)) % sizeof(int) != 0)
            invalid = 1;
        next = (header->cmsg_len - CMSG_LEN(0)) / sizeof(int);
        if (next > 2 - count) {
            invalid = 1;
            next = 2 - count;
        }
        if (next > 0) {
            memcpy(descriptors + count, CMSG_DATA(header), next * sizeof(int));
            count += next;
        }
    }
    if (received != 1 || payload != 'Q' ||
        (message.msg_flags & (MSG_CTRUNC | MSG_TRUNC)) != 0 ||
        count != 1 || invalid) {
        if (descriptors[0] >= 0)
            close(descriptors[0]);
        if (descriptors[1] >= 0)
            close(descriptors[1]);
        return EPROTO;
    }
    *out = descriptors[0];
    return 0;
}

// The only socket listener accepted by the service is a single, filesystem-
// named Unix stream listener inherited from the root-owned service manager.
// Its socket and immediate parent must be owned by root; only the owner/group
// may connect, and neither location may be changed by the service uid/group.
int quanta_take_systemd_listener(int *out) {
    const char *pid_text = getenv("LISTEN_PID");
    const char *fds_text = getenv("LISTEN_FDS");
    struct sockaddr_un address;
    struct stat socket_stat, parent_stat;
    socklen_t address_len = sizeof(address);
    socklen_t option_len;
    char parent[sizeof(address.sun_path)];
    char *end;
    unsigned long pid_value;
    size_t path_len;
    int type, accepting;
    if (out == NULL)
        return EINVAL;
    *out = -1;
    if (geteuid() != 0 || pid_text == NULL || fds_text == NULL ||
        strcmp(fds_text, "1") != 0)
        return EPERM;
    errno = 0;
    pid_value = strtoul(pid_text, &end, 10);
    if (errno != 0 || end == pid_text || *end != '\0' ||
        pid_value != (unsigned long)getpid())
        return EPERM;
    memset(&address, 0, sizeof(address));
    if (getsockname(3, (struct sockaddr *)&address, &address_len) < 0)
        return errno;
    if (address.sun_family != AF_UNIX ||
        address_len <= offsetof(struct sockaddr_un, sun_path) + 1 ||
        address.sun_path[0] != '/')
        return EINVAL;
    path_len = strnlen(address.sun_path, sizeof(address.sun_path));
    if (path_len == sizeof(address.sun_path) ||
        path_len + offsetof(struct sockaddr_un, sun_path) + 1 > address_len)
        return EINVAL;
    option_len = sizeof(type);
    if (getsockopt(3, SOL_SOCKET, SO_TYPE, &type, &option_len) < 0)
        return errno;
    if (option_len != sizeof(type) || type != SOCK_STREAM)
        return EINVAL;
    option_len = sizeof(accepting);
    if (getsockopt(3, SOL_SOCKET, SO_ACCEPTCONN, &accepting, &option_len) < 0)
        return errno;
    if (option_len != sizeof(accepting) || accepting != 1)
        return EINVAL;
    if (lstat(address.sun_path, &socket_stat) < 0)
        return errno;
    if (!S_ISSOCK(socket_stat.st_mode) || socket_stat.st_uid != 0 ||
        socket_stat.st_gid == 0 ||
        (socket_stat.st_mode & 0007) != 0 ||
        (socket_stat.st_mode & 0020) == 0 ||
        (socket_stat.st_mode & 0111) != 0)
        return EPERM;
    memcpy(parent, address.sun_path, path_len + 1);
    char *slash = strrchr(parent, '/');
    if (slash == NULL)
        return EINVAL;
    if (slash == parent)
        parent[1] = '\0';
    else
        *slash = '\0';
    // Reject writable or symlinked ancestors all the way to /. This also
    // rejects /tmp and user-controlled home directories as socket parents.
    for (;;) {
        if (lstat(parent, &parent_stat) < 0)
            return errno;
        if (!S_ISDIR(parent_stat.st_mode) || parent_stat.st_uid != 0 ||
            (parent_stat.st_mode & 0022) != 0)
            return EPERM;
        if (strcmp(parent, "/") == 0)
            break;
        slash = strrchr(parent, '/');
        if (slash == NULL)
            return EINVAL;
        if (slash == parent)
            parent[1] = '\0';
        else
            *slash = '\0';
    }
    if (fcntl(3, F_SETFD, FD_CLOEXEC) < 0)
        return errno;
    *out = 3;
    return 0;
}
