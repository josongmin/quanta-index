/* Quanta Index authored pinned bundled SQLite adapter.
 * No code from SQLite test_quota.c is copied here. */
#ifndef QUANTA_CAPACITY_VFS_H
#define QUANTA_CAPACITY_VFS_H

#include "sqlite3.h"
#include <stddef.h>
#include <stdint.h>

#define QIV_ABI_VERSION 3
#define QIV_REDIRECT_MAX 4096
#define QIV_CAPACITY_FULL (SQLITE_FULL | (0x51 << 8))

enum qiv_kind {
  QIV_BEFORE_OPEN = 1,
  QIV_AFTER_OPEN,
  QIV_BEFORE_WRITE,
  QIV_AFTER_WRITE,
  QIV_BEFORE_TRUNCATE,
  QIV_AFTER_TRUNCATE,
  QIV_BEFORE_SYNC,
  QIV_AFTER_SYNC,
  QIV_BEFORE_SHM_MAP,
  QIV_AFTER_SHM_MAP,
  QIV_AFTER_SHM_UNMAP,
  QIV_BEFORE_DELETE,
  QIV_AFTER_DELETE,
  QIV_AFTER_CLOSE,
  QIV_BEFORE_FILE_CONTROL,
  QIV_AFTER_FILE_CONTROL,
  QIV_FD_RETIRED,
  QIV_BEFORE_NATIVE_OPEN,
  QIV_OPEN_ABORTED_NO_FD,
  QIV_BEFORE_VERIFICATION_READ
};

typedef struct qiv_native_sync_observation {
  int observed;
  int same_native_fd;
  int ambiguous;
  int full_requested;
  int full_attempted;
  int full_default_syscall;
  int full_result;
  int full_errno;
  int posix_attempted;
  int posix_result;
  int posix_errno;
} qiv_native_sync_observation;

typedef struct qiv_event {
  uint32_t abi_version;
  uint32_t kind;
  uint64_t scope_id;
  uint64_t handle_id;
  int open_flags;
  int sqlite_result;
  int operation;
  int observed_size_valid;
  sqlite3_int64 offset;
  sqlite3_int64 amount;
  sqlite3_int64 target_size;
  sqlite3_int64 observed_size;
  int witness_valid;
  int fd_retired;
  int first_shm_fd;
  uint64_t witness_device;
  uint64_t witness_inode;
  uint64_t witness_bytes;
  uint64_t witness_links;
  const char *path;
  uint32_t redirect_len;
  char redirect[QIV_REDIRECT_MAX];
  int native_sync_opt_in;
  qiv_native_sync_observation native_sync;
} qiv_event;

typedef int (*qiv_callback)(void *context, qiv_event *event);

/* Caller supplies exactly 37 u32 slots. The ABI is rejected before VFS
 * registration when the C/Rust event layouts differ. */
int qiv_event_layout(uint32_t *slots, size_t count);

/* Register once, not as the default VFS. Callback/context must outlive every
 * controlled connection. max_open_handles bounds the C-side live registry. */
int qiv_register(const char *name, unsigned max_open_handles,
                 qiv_callback callback, void *context);

/* Drain native final-close witnesses outside SQLite internal mutexes. */
int qiv_drain_retirements(void);

/* Consumes a Rust-owned regular-file FD. A nonzero result is ambiguous and
 * must retain the caller's inode charge; never retry the numeric descriptor. */
int qiv_close_owned_fd(int descriptor);

#ifdef QUANTA_CAPACITY_VFS_TEST_HOOKS
/* Open an existing database read-write through the named VFS, then probe one
 * SHM region with xShmMap(bExtend=0). The caller must hold a registered root
 * scope. Cleanup preserves the first failing SQLite result. */
int qiv_test_first_shm_read_map(const char *path, int *mapping_present);
/* In-memory unixFile state oracle; never performs native I/O. */
int qiv_test_native_sync_probe_reset(void);
/* Process-serial test hook. Borrows a live regular-file FD; never closes it. */
int qiv_test_native_sync_syscall_outcomes(int descriptor);
#endif

/* Called from pinned robust_close. The actual osClose and record retirement
 * are serialized against native descriptor registration by qiv.mutex.
 * Returns osClose's result and restores its errno; never calls Rust/SQLite
 * callbacks or retries an ambiguous close. */
int qiv_native_close_fd(int descriptor, int (*close_fn)(int));

/* The safe Rust facade wraps this in a non-Send RAII guard for every SQLite
 * operation, including open and close. Scope zero is refused by this VFS. */
uint64_t qiv_enter_scope(uint64_t scope_id);
int qiv_enter_verification(int enabled);

/* Implemented inside the pinned SQLite amalgamation, where unixFile and
 * unixShmNode are defined. Native FD fstat avoids a pathname TOCTOU witness.
 * The adapter feature is Unix-only until an equally strong Windows hook exists. */
int qiv_unix_file_descriptor(sqlite3_file *native_file, int shared_memory);
int qiv_unix_descriptor_witness(int descriptor,
                                uint64_t *device, uint64_t *inode,
                                uint64_t *bytes, uint64_t *links);
int qiv_unix_file_witness(sqlite3_file *native_file, int shared_memory,
                          uint64_t *device, uint64_t *inode,
                          uint64_t *bytes, uint64_t *links);
int qiv_unix_sync_shm(sqlite3_file *native_file);
void qiv_unix_sync_probe_begin(sqlite3_file *native_file);
void qiv_unix_sync_probe_end(sqlite3_file *native_file,
                             qiv_native_sync_observation *observation);
int qiv_unix_sync_probe_nested_off_begin(sqlite3_file *native_file);
void qiv_unix_sync_probe_nested_off_end(sqlite3_file *native_file);
int qiv_unix_shm_growth_target(int region, int region_size,
                               sqlite3_int64 *target);

#endif
