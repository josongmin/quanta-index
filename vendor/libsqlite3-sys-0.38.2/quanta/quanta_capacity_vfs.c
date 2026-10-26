/* Quanta Index authored named, delegating SQLite VFS.
 * No SQLite quota test implementation is copied.
 * Build only with the same sqlite3.h and bundled SQLite library as rusqlite.
 */
#include "quanta_capacity_vfs.h"
#include <limits.h>
#include <errno.h>
#include <stddef.h>
#include <stdatomic.h>
#include <string.h>
#include <unistd.h>

int qiv_close_owned_fd(int descriptor) {
  if (descriptor < 0) return EBADF;
  return close(descriptor) == 0 ? 0 : errno;
}

int qiv_event_layout(uint32_t *slots, size_t count) {
  if (!slots || count != 37) return SQLITE_MISUSE;
  slots[0] = (uint32_t)sizeof(qiv_event);
  slots[1] = (uint32_t)_Alignof(qiv_event);
  slots[2] = (uint32_t)offsetof(qiv_event, abi_version);
  slots[3] = (uint32_t)offsetof(qiv_event, kind);
  slots[4] = (uint32_t)offsetof(qiv_event, scope_id);
  slots[5] = (uint32_t)offsetof(qiv_event, handle_id);
  slots[6] = (uint32_t)offsetof(qiv_event, open_flags);
  slots[7] = (uint32_t)offsetof(qiv_event, sqlite_result);
  slots[8] = (uint32_t)offsetof(qiv_event, operation);
  slots[9] = (uint32_t)offsetof(qiv_event, observed_size_valid);
  slots[10] = (uint32_t)offsetof(qiv_event, offset);
  slots[11] = (uint32_t)offsetof(qiv_event, amount);
  slots[12] = (uint32_t)offsetof(qiv_event, target_size);
  slots[13] = (uint32_t)offsetof(qiv_event, observed_size);
  slots[14] = (uint32_t)offsetof(qiv_event, witness_valid);
  slots[15] = (uint32_t)offsetof(qiv_event, fd_retired);
  slots[16] = (uint32_t)offsetof(qiv_event, first_shm_fd);
  slots[17] = (uint32_t)offsetof(qiv_event, witness_device);
  slots[18] = (uint32_t)offsetof(qiv_event, witness_inode);
  slots[19] = (uint32_t)offsetof(qiv_event, witness_bytes);
  slots[20] = (uint32_t)offsetof(qiv_event, witness_links);
  slots[21] = (uint32_t)offsetof(qiv_event, path);
  slots[22] = (uint32_t)offsetof(qiv_event, redirect_len);
  slots[23] = (uint32_t)offsetof(qiv_event, redirect);
  slots[24] = (uint32_t)offsetof(qiv_event, native_sync_opt_in);
  slots[25] = (uint32_t)offsetof(qiv_event, native_sync);
  slots[26] = (uint32_t)offsetof(qiv_event, native_sync.observed);
  slots[27] = (uint32_t)offsetof(qiv_event, native_sync.same_native_fd);
  slots[28] = (uint32_t)offsetof(qiv_event, native_sync.ambiguous);
  slots[29] = (uint32_t)offsetof(qiv_event, native_sync.full_requested);
  slots[30] = (uint32_t)offsetof(qiv_event, native_sync.full_attempted);
  slots[31] = (uint32_t)offsetof(qiv_event, native_sync.full_default_syscall);
  slots[32] = (uint32_t)offsetof(qiv_event, native_sync.full_result);
  slots[33] = (uint32_t)offsetof(qiv_event, native_sync.full_errno);
  slots[34] = (uint32_t)offsetof(qiv_event, native_sync.posix_attempted);
  slots[35] = (uint32_t)offsetof(qiv_event, native_sync.posix_result);
  slots[36] = (uint32_t)offsetof(qiv_event, native_sync.posix_errno);
  return SQLITE_OK;
}

#if defined(_MSC_VER)
#define QIV_THREAD_LOCAL __declspec(thread)
#else
#define QIV_THREAD_LOCAL _Thread_local
#endif

typedef struct qiv_file {
  sqlite3_file base;
  sqlite3_file *inner;
  const char *path;
  char *owned_path;
  uint64_t scope_id;
  uint64_t handle_id;
  unsigned record_index;
  int open_flags;
  int verification_only;
  max_align_t inner_alignment;
  /* The wrapped VFS's sqlite3_file occupies the szOsFile bytes immediately
   * after this structure, at max_align_t alignment. */
} qiv_file;

typedef struct qiv_witness {
  int valid;
  uint64_t device, inode, bytes, links;
} qiv_witness;

typedef struct qiv_record {
  uint64_t scope_id, handle_id;
  int main_fd, shm_fd;
  unsigned used : 1;
  unsigned logical_closed : 1;
  unsigned main_retired : 1;
  unsigned main_notified : 1;
  unsigned shm_present : 1;
  unsigned shm_unmapped : 1;
  unsigned shm_retired : 1;
  unsigned shm_notified : 1;
  unsigned main_inflight : 1;
  unsigned shm_inflight : 1;
  qiv_witness main_witness, shm_witness;
} qiv_record;

typedef struct qiv_state {
  sqlite3_vfs wrapper;
  sqlite3_vfs *original;
  sqlite3_io_methods io_v1;
  sqlite3_io_methods io_v2;
  sqlite3_io_methods io_v3;
  sqlite3_mutex *mutex;
  qiv_callback callback;
  void *context;
  char *name;
  uint64_t next_handle;
  unsigned max_open_handles;
  qiv_record *records;
  unsigned retained_records;
  int registered;
} qiv_state;

static qiv_state qiv;
static atomic_int qiv_active = ATOMIC_VAR_INIT(0);
static QIV_THREAD_LOCAL uint64_t qiv_scope_id;
static QIV_THREAD_LOCAL int qiv_verification_only;

int qiv_enter_verification(int enabled) {
  int previous = qiv_verification_only;
  qiv_verification_only = enabled != 0;
  return previous;
}

uint64_t qiv_enter_scope(uint64_t scope_id) {
  uint64_t previous = qiv_scope_id;
  qiv_scope_id = scope_id;
  return previous;
}

static int qiv_emit(qiv_event *event) {
  int rc;
  if (!qiv.callback) return SQLITE_MISUSE;
  event->abi_version = QIV_ABI_VERSION;
  rc = qiv.callback(qiv.context, event);
  switch (rc) {
  case SQLITE_OK:
  case SQLITE_FULL:
  case QIV_CAPACITY_FULL:
  case SQLITE_IOERR:
  case SQLITE_NOMEM:
  case SQLITE_CANTOPEN:
  case SQLITE_BUSY:
  case SQLITE_NOTFOUND:
    return rc;
  default:
    return SQLITE_IOERR;
  }
}

static qiv_event qiv_event_for(uint32_t kind, uint64_t scope_id,
                               uint64_t handle_id, const char *path,
                               int open_flags) {
  qiv_event event;
  memset(&event, 0, sizeof(event));
  event.kind = kind;
  event.scope_id = scope_id;
  event.handle_id = handle_id;
  event.path = path;
  event.open_flags = open_flags;
  return event;
}

static int qiv_scope_matches(qiv_file *file) {
  return qiv_scope_id != 0 && qiv_scope_id == file->scope_id;
}

static qiv_event qiv_file_event(qiv_file *file, uint32_t kind) {
  return qiv_event_for(kind, file->scope_id, file->handle_id, file->path,
                       file->open_flags);
}

static void qiv_observe_size(qiv_file *file, qiv_event *event) {
  sqlite3_int64 size = 0;
  if (file->inner && file->inner->pMethods &&
      file->inner->pMethods->xFileSize(file->inner, &size) == SQLITE_OK &&
      size >= 0) {
    event->observed_size_valid = 1;
    event->observed_size = size;
  }
}

static void qiv_observe_witness(qiv_file *file, qiv_event *event,
                                int shared_memory) {
  uint64_t device = 0, inode = 0, bytes = 0, links = 0;
  event->witness_valid = 0;
  event->witness_device = 0;
  event->witness_inode = 0;
  event->witness_bytes = 0;
  event->witness_links = 0;
  if (!file->inner || !file->inner->pMethods) return;
  if (qiv_unix_file_witness(file->inner, shared_memory,
                            &device, &inode, &bytes, &links) == SQLITE_OK) {
    event->witness_valid = 1;
    event->witness_device = device;
    event->witness_inode = inode;
    event->witness_bytes = bytes;
    event->witness_links = links;
  }
}

static void qiv_witness_to_record(qiv_witness *destination,
                                  const qiv_event *event) {
  if (!event->witness_valid) return;
  destination->valid = 1;
  destination->device = event->witness_device;
  destination->inode = event->witness_inode;
  destination->bytes = event->witness_bytes;
  destination->links = event->witness_links;
}

static void qiv_record_to_event(qiv_event *event, const qiv_witness *source) {
  event->witness_valid = source->valid;
  event->witness_device = source->device;
  event->witness_inode = source->inode;
  event->witness_bytes = source->bytes;
  event->witness_links = source->links;
}

static void qiv_try_release_record(unsigned index) {
  qiv_record *record = &qiv.records[index];
  if (record->used && record->logical_closed && record->main_retired &&
      record->main_notified && !record->main_inflight && !record->shm_inflight &&
      (!record->shm_present || (record->shm_retired && record->shm_notified))) {
    memset(record, 0, sizeof(*record));
    if (qiv.retained_records > 0) qiv.retained_records--;
  }
}

static void qiv_release_open_slot(unsigned index) {
  sqlite3_mutex_enter(qiv.mutex);
  if (qiv.records[index].used) {
    memset(&qiv.records[index], 0, sizeof(qiv.records[index]));
    if (qiv.retained_records > 0) qiv.retained_records--;
  }
  sqlite3_mutex_leave(qiv.mutex);
}

static int qiv_allocate_open_slot(uint64_t scope_id, uint64_t *handle_id,
                                  unsigned *index) {
  unsigned i;
  int rc = QIV_CAPACITY_FULL;
  sqlite3_mutex_enter(qiv.mutex);
  if (qiv.next_handle != UINT64_MAX) {
    for (i=0; i<qiv.max_open_handles; i++) {
      if (!qiv.records[i].used) {
        qiv_record *record = &qiv.records[i];
        memset(record, 0, sizeof(*record));
        record->used = 1;
        record->scope_id = scope_id;
        record->handle_id = ++qiv.next_handle;
        record->main_fd = -1;
        record->shm_fd = -1;
        *handle_id = record->handle_id;
        *index = i;
        qiv.retained_records++;
        rc = SQLITE_OK;
        break;
      }
    }
  }
  sqlite3_mutex_leave(qiv.mutex);
  return rc;
}

static void qiv_record_native_descriptor(unsigned index, int fd,
                                         int shared_memory,
                                         const qiv_event *event) {
  qiv_record *record;
  sqlite3_mutex_enter(qiv.mutex);
  record = &qiv.records[index];
  if (shared_memory) {
    if (fd < 0 && !event->witness_valid) {
      sqlite3_mutex_leave(qiv.mutex);
      return;
    }
    record->shm_present = 1;
    if (fd >= 0 || record->shm_fd < 0) record->shm_fd = fd;
    qiv_witness_to_record(&record->shm_witness, event);
  } else {
    if (fd >= 0 || record->main_fd < 0) record->main_fd = fd;
    qiv_witness_to_record(&record->main_witness, event);
  }
  sqlite3_mutex_leave(qiv.mutex);
}

/* Serialize the actual close and retirement scan against all new native
 * descriptor registrations. Otherwise a reused numeric FD can retire a new
 * record after the old close. This function never invokes a callback. */
int qiv_native_close_fd(int descriptor, int (*close_fn)(int)) {
  unsigned i;
  int close_result, close_errno;
  qiv_witness final_witness;
  if (!close_fn) { errno = EINVAL; return -1; }
  if (!atomic_load_explicit(&qiv_active, memory_order_acquire))
    return close_fn(descriptor);
  sqlite3_mutex_enter(qiv.mutex);
  memset(&final_witness, 0, sizeof(final_witness));
  /* Lower SQLite may unlink SHM after xShmUnmap's initial witness. Capture
   * the same actual FD after unlink and immediately before its final close;
   * neither a pathname reopen nor a stale pre-unlink nlink can prove release. */
  if (qiv_unix_descriptor_witness(descriptor, &final_witness.device,
                                   &final_witness.inode, &final_witness.bytes,
                                   &final_witness.links) == SQLITE_OK)
    final_witness.valid = 1;
  close_result = close_fn(descriptor);
  close_errno = errno;
  for (i=0; i<qiv.max_open_handles; i++) {
    qiv_record *record = &qiv.records[i];
    if (!record->used) continue;
    if (close_result == 0) {
      if (!record->main_retired && record->main_fd == descriptor) {
        record->main_witness = final_witness;
        record->main_retired = 1;
        record->main_fd = -1;
      }
      if (record->shm_present && !record->shm_retired &&
          record->shm_fd == descriptor) {
        record->shm_witness = final_witness;
        record->shm_retired = 1;
        record->shm_fd = -1;
      }
    } else {
      /* EINTR may mean this numeric FD was already closed. Never allow a
       * later unrelated close after reuse to retire this record. Retain the
       * unretired record and its charge until process-level recovery. */
      if (record->main_fd == descriptor) record->main_fd = -1;
      if (record->shm_present && record->shm_fd == descriptor)
        record->shm_fd = -1;
    }
  }
  sqlite3_mutex_leave(qiv.mutex);
  errno = close_errno;
  return close_result;
}

int qiv_drain_retirements(void) {
  unsigned i;
  if (!qiv.registered) return SQLITE_MISUSE;
  for (i=0; i<qiv.max_open_handles; i++) {
    int shared_memory;
    for (shared_memory=0; shared_memory<=1; shared_memory++) {
      qiv_record *record;
      qiv_event event;
      int should_emit = 0;
      int rc;
      sqlite3_mutex_enter(qiv.mutex);
      record = &qiv.records[i];
      if (record->used &&
          (shared_memory ?
             (record->shm_present && record->shm_unmapped &&
              record->shm_retired && !record->shm_notified &&
              !record->shm_inflight) :
             (record->logical_closed && record->main_retired &&
              !record->main_notified && !record->main_inflight))) {
        if (shared_memory) record->shm_inflight = 1;
        else record->main_inflight = 1;
        event = qiv_event_for(QIV_FD_RETIRED, record->scope_id,
                              record->handle_id, NULL, 0);
        event.operation = shared_memory;
        event.fd_retired = 1;
        qiv_record_to_event(&event, shared_memory ? &record->shm_witness
                                                 : &record->main_witness);
        should_emit = 1;
      }
      sqlite3_mutex_leave(qiv.mutex);
      if (!should_emit) continue;
      rc = qiv_emit(&event);
      sqlite3_mutex_enter(qiv.mutex);
      record = &qiv.records[i];
      if (record->used && record->handle_id == event.handle_id) {
        if (shared_memory) {
          record->shm_inflight = 0;
          if (rc == SQLITE_OK) record->shm_notified = 1;
        } else {
          record->main_inflight = 0;
          if (rc == SQLITE_OK) record->main_notified = 1;
        }
        qiv_try_release_record(i);
      }
      sqlite3_mutex_leave(qiv.mutex);
      if (rc != SQLITE_OK) return rc;
    }
  }
  return SQLITE_OK;
}

static int qiv_close(sqlite3_file *base) {
  qiv_file *file = (qiv_file *)base;
  qiv_event event = qiv_file_event(file, QIV_AFTER_CLOSE);
  qiv_record *record;
  int lower_rc, callback_rc;
  qiv_observe_witness(file, &event, 0);
  qiv_record_native_descriptor(file->record_index,
                               qiv_unix_file_descriptor(file->inner, 0),
                               0, &event);
  lower_rc = file->inner->pMethods->xClose(file->inner);
  event.sqlite_result = lower_rc;
  sqlite3_mutex_enter(qiv.mutex);
  record = &qiv.records[file->record_index];
  record->logical_closed = 1;
  event.fd_retired = record->main_retired;
  if (record->main_retired)
    qiv_record_to_event(&event, &record->main_witness);
  /* Reserve the immediate callback against the process-global drain even if
   * physical retirement is still pending. The latter may arrive meanwhile. */
  record->main_inflight = 1;
  sqlite3_mutex_leave(qiv.mutex);
  callback_rc = qiv_emit(&event);
  sqlite3_mutex_enter(qiv.mutex);
  record = &qiv.records[file->record_index];
  record->main_inflight = 0;
  if (callback_rc == SQLITE_OK && event.fd_retired)
    record->main_notified = 1;
  qiv_try_release_record(file->record_index);
  sqlite3_mutex_leave(qiv.mutex);
  sqlite3_free(file->owned_path);
  file->owned_path = NULL;
  file->base.pMethods = NULL;
  return lower_rc == SQLITE_OK ? callback_rc : lower_rc;
}

static int qiv_read(sqlite3_file *base, void *buffer, int amount,
                    sqlite3_int64 offset) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR_READ;
  if (file->verification_only || qiv_verification_only) {
    qiv_event event = qiv_file_event(file, QIV_BEFORE_VERIFICATION_READ);
    event.offset = offset;
    event.amount = amount;
    int callback_rc = qiv_emit(&event);
    if (callback_rc != SQLITE_OK) return callback_rc;
  }
  return file->inner->pMethods->xRead(file->inner, buffer, amount, offset);
}

static int qiv_write(sqlite3_file *base, const void *buffer, int amount,
                     sqlite3_int64 offset) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR_WRITE;
  if (file->verification_only || qiv_verification_only) return SQLITE_READONLY;
  qiv_event event = qiv_file_event(file, QIV_BEFORE_WRITE);
  if (offset < 0 || amount < 0 || offset > INT64_MAX - amount) return SQLITE_IOERR_WRITE;
  event.offset = offset;
  event.amount = amount;
  event.target_size = offset + amount;
  qiv_observe_witness(file, &event, 0);
  int callback_rc = qiv_emit(&event);
  if (callback_rc != SQLITE_OK) return callback_rc;
  int lower_rc = file->inner->pMethods->xWrite(file->inner, buffer, amount, offset);
  event.kind = QIV_AFTER_WRITE;
  event.sqlite_result = lower_rc;
  qiv_observe_size(file, &event);
  qiv_observe_witness(file, &event, 0);
  callback_rc = qiv_emit(&event);
  return lower_rc == SQLITE_OK ? callback_rc : lower_rc;
}

static int qiv_sync(sqlite3_file *base, int flags);

static int qiv_truncate(sqlite3_file *base, sqlite3_int64 size) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR_TRUNCATE;
  if (file->verification_only || qiv_verification_only) return SQLITE_READONLY;
  qiv_event event = qiv_file_event(file, QIV_BEFORE_TRUNCATE);
  if (size < 0) return SQLITE_IOERR_TRUNCATE;
  event.target_size = size;
  qiv_observe_witness(file, &event, 0);
  int callback_rc = qiv_emit(&event);
  if (callback_rc != SQLITE_OK) return callback_rc;
  int lower_rc = file->inner->pMethods->xTruncate(file->inner, size);
  event.kind = QIV_AFTER_TRUNCATE;
  event.sqlite_result = lower_rc;
  qiv_observe_size(file, &event);
  qiv_observe_witness(file, &event, 0);
  callback_rc = qiv_emit(&event);
  if (lower_rc != SQLITE_OK) return lower_rc;
  if (callback_rc != SQLITE_OK) return callback_rc;
  /* SQLite's TRUNCATE checkpoint ends with xTruncate, without a later WAL
   * xSync. Retire this original-FD mutation under the same funding before
   * returning; a later foreground writer must not inherit its parent. */
  if (size == 0 && (file->open_flags & SQLITE_OPEN_WAL)) {
    return qiv_sync(base, SQLITE_SYNC_FULL);
  }
  return SQLITE_OK;
}

static int qiv_sync(sqlite3_file *base, int flags) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR_FSYNC;
  if (file->verification_only || qiv_verification_only) return SQLITE_READONLY;
  qiv_event event = qiv_file_event(file, QIV_BEFORE_SYNC);
  event.operation = flags;
  qiv_observe_witness(file, &event, 0);
  int callback_rc = qiv_emit(&event);
  if (callback_rc != SQLITE_OK) return callback_rc;
  int lower_rc;
  if (event.native_sync_opt_in) {
    int saved_errno = errno;
    qiv_unix_sync_probe_begin(file->inner);
    errno = saved_errno;
    lower_rc = file->inner->pMethods->xSync(file->inner, flags);
    saved_errno = errno;
    qiv_unix_sync_probe_end(file->inner, &event.native_sync);
    errno = saved_errno;
  } else {
    int nested_off = qiv_unix_sync_probe_nested_off_begin(file->inner);
    lower_rc = file->inner->pMethods->xSync(file->inner, flags);
    if (nested_off) {
      int saved_errno = errno;
      qiv_unix_sync_probe_nested_off_end(file->inner);
      errno = saved_errno;
    }
  }
  event.kind = QIV_AFTER_SYNC;
  event.sqlite_result = lower_rc;
  qiv_observe_size(file, &event);
  qiv_observe_witness(file, &event, 0);
  callback_rc = qiv_emit(&event);
  return lower_rc == SQLITE_OK ? callback_rc : lower_rc;
}

static int qiv_file_size(sqlite3_file *base, sqlite3_int64 *size) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR_FSTAT;
  return file->inner->pMethods->xFileSize(file->inner, size);
}

static int qiv_lock(sqlite3_file *base, int level) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR_LOCK;
  return file->inner->pMethods->xLock(file->inner, level);
}

static int qiv_unlock(sqlite3_file *base, int level) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR_UNLOCK;
  return file->inner->pMethods->xUnlock(file->inner, level);
}

static int qiv_check_reserved_lock(sqlite3_file *base, int *result) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR_CHECKRESERVEDLOCK;
  return file->inner->pMethods->xCheckReservedLock(file->inner, result);
}

static int qiv_file_control(sqlite3_file *base, int operation, void *argument) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR;
  /* Unknown file controls must not bypass the write/truncate gates. Optional
   * controls return NOTFOUND, allowing SQLite's ordinary SQL implementation. */
  if (file->verification_only || qiv_verification_only) {
    switch (operation) {
    case SQLITE_FCNTL_LOCKSTATE:
    case SQLITE_FCNTL_LAST_ERRNO:
    case SQLITE_FCNTL_HAS_MOVED:
    case SQLITE_FCNTL_VFSNAME:
    case SQLITE_FCNTL_VFS_POINTER:
      break;
    default:
      return SQLITE_NOTFOUND;
    }
  }
  qiv_event event = qiv_file_event(file, QIV_BEFORE_FILE_CONTROL);
  event.operation = operation;
  qiv_observe_witness(file, &event, 0);
  if (operation == SQLITE_FCNTL_SIZE_HINT && argument)
    event.target_size = *(sqlite3_int64 *)argument;
  if (operation == SQLITE_FCNTL_CHUNK_SIZE && argument)
    event.target_size = *(int *)argument;
  int callback_rc = qiv_emit(&event);
  if (callback_rc != SQLITE_OK) return callback_rc;
  /* The native VFS may preallocate blocks beyond the logical file width for
   * these hints. They have no admitted physical-block bound here. SQLite
   * treats NOTFOUND as an unsupported optional file-control operation. */
  int lower_rc = (operation == SQLITE_FCNTL_SIZE_HINT ||
                  operation == SQLITE_FCNTL_CHUNK_SIZE ||
                  operation == SQLITE_FCNTL_NULL_IO ||
                  operation == SQLITE_FCNTL_BEGIN_ATOMIC_WRITE ||
                  operation == SQLITE_FCNTL_COMMIT_ATOMIC_WRITE ||
                  operation == SQLITE_FCNTL_ROLLBACK_ATOMIC_WRITE)
                     ? SQLITE_NOTFOUND
                     : file->inner->pMethods->xFileControl(file->inner,
                                                           operation, argument);
  event.kind = QIV_AFTER_FILE_CONTROL;
  event.sqlite_result = lower_rc;
  qiv_observe_size(file, &event);
  qiv_observe_witness(file, &event, 0);
  callback_rc = qiv_emit(&event);
  return lower_rc == SQLITE_OK ? callback_rc : lower_rc;
}

static int qiv_sector_size(sqlite3_file *base) {
  qiv_file *file = (qiv_file *)base;
  return file->inner->pMethods->xSectorSize(file->inner);
}

static int qiv_device_characteristics(sqlite3_file *base) {
  qiv_file *file = (qiv_file *)base;
  return file->inner->pMethods->xDeviceCharacteristics(file->inner);
}

static int qiv_shm_map(sqlite3_file *base, int region, int region_size,
                       int extend, void volatile **mapping) {
  qiv_file *file = (qiv_file *)base;
  qiv_record *record;
  int stale_shm = 0;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR_SHMMAP;
  if (file->verification_only || qiv_verification_only) {
    *mapping = NULL;
    return SQLITE_READONLY;
  }
  sqlite3_mutex_enter(qiv.mutex);
  record = &qiv.records[file->record_index];
  if (record->shm_present && record->shm_unmapped) {
    if (!record->shm_retired || !record->shm_notified)
      stale_shm = 1;
    else {
      record->shm_present = 0;
      record->shm_unmapped = 0;
      record->shm_retired = 0;
      record->shm_notified = 0;
      record->shm_fd = -1;
      memset(&record->shm_witness, 0, sizeof(record->shm_witness));
    }
  }
  sqlite3_mutex_leave(qiv.mutex);
  if (stale_shm) return SQLITE_FULL;
  qiv_event event = qiv_file_event(file, QIV_BEFORE_SHM_MAP);
  if (qiv_unix_shm_growth_target(region, region_size,
                                 &event.target_size) != SQLITE_OK)
    return SQLITE_IOERR_SHMMAP;
  event.offset = region;
  event.amount = region_size;
  event.operation = extend;
  int first_shm_fd = qiv_unix_file_descriptor(file->inner, 1) < 0;
  event.first_shm_fd = first_shm_fd;
  qiv_observe_witness(file, &event, 1);
  int callback_rc = qiv_emit(&event);
  if (callback_rc != SQLITE_OK) return callback_rc;
  if (file->inner->pMethods->iVersion < 2 || !file->inner->pMethods->xShmMap)
    return SQLITE_IOERR_SHMMAP;
  int lower_rc = file->inner->pMethods->xShmMap(file->inner, region,
                                                region_size, extend, mapping);
  if (lower_rc == SQLITE_OK && (extend || first_shm_fd)) {
    int sync_rc = qiv_unix_sync_shm(file->inner);
    if (sync_rc != SQLITE_OK) lower_rc = sync_rc;
  }
  event.kind = QIV_AFTER_SHM_MAP;
  event.sqlite_result = lower_rc;
  qiv_observe_witness(file, &event, 1);
  int shm_fd = qiv_unix_file_descriptor(file->inner, 1);
  event.fd_retired = shm_fd < 0;
  qiv_record_native_descriptor(file->record_index, shm_fd, 1, &event);
  callback_rc = qiv_emit(&event);
  return lower_rc == SQLITE_OK ? callback_rc : lower_rc;
}

static int qiv_shm_lock(sqlite3_file *base, int offset, int count, int flags) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR_SHMLOCK;
  return file->inner->pMethods->xShmLock(file->inner, offset, count, flags);
}

static void qiv_shm_barrier(sqlite3_file *base) {
  qiv_file *file = (qiv_file *)base;
  file->inner->pMethods->xShmBarrier(file->inner);
}

static int qiv_shm_unmap(sqlite3_file *base, int delete_flag) {
  qiv_file *file = (qiv_file *)base;
  qiv_event event = qiv_file_event(file, QIV_AFTER_SHM_UNMAP);
  qiv_record *record;
  int lower_rc, callback_rc;
  qiv_observe_witness(file, &event, 1);
  if (event.witness_valid)
    qiv_record_native_descriptor(file->record_index,
                                 qiv_unix_file_descriptor(file->inner, 1),
                                 1, &event);
  lower_rc = file->inner->pMethods->xShmUnmap(file->inner, delete_flag);
  event.operation = delete_flag;
  event.sqlite_result = lower_rc;
  sqlite3_mutex_enter(qiv.mutex);
  record = &qiv.records[file->record_index];
  record->shm_unmapped = 1;
  event.fd_retired = record->shm_retired;
  if (record->shm_retired)
    qiv_record_to_event(&event, &record->shm_witness);
  /* Reserve the immediate callback against the process-global drain. */
  record->shm_inflight = 1;
  sqlite3_mutex_leave(qiv.mutex);
  callback_rc = qiv_emit(&event);
  sqlite3_mutex_enter(qiv.mutex);
  record = &qiv.records[file->record_index];
  record->shm_inflight = 0;
  if (callback_rc == SQLITE_OK && event.fd_retired)
    record->shm_notified = 1;
  qiv_try_release_record(file->record_index);
  sqlite3_mutex_leave(qiv.mutex);
  return lower_rc == SQLITE_OK ? callback_rc : lower_rc;
}

static int qiv_fetch(sqlite3_file *base, sqlite3_int64 offset, int amount,
                     void **mapping) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR;
  return file->inner->pMethods->xFetch(file->inner, offset, amount, mapping);
}

static int qiv_unfetch(sqlite3_file *base, sqlite3_int64 offset, void *mapping) {
  qiv_file *file = (qiv_file *)base;
  if (!qiv_scope_matches(file)) return SQLITE_IOERR;
  return file->inner->pMethods->xUnfetch(file->inner, offset, mapping);
}

static void qiv_initialize_io_methods(void) {
  sqlite3_io_methods base;
  memset(&base, 0, sizeof(base));
  base.iVersion = 1;
  base.xClose = qiv_close;
  base.xRead = qiv_read;
  base.xWrite = qiv_write;
  base.xTruncate = qiv_truncate;
  base.xSync = qiv_sync;
  base.xFileSize = qiv_file_size;
  base.xLock = qiv_lock;
  base.xUnlock = qiv_unlock;
  base.xCheckReservedLock = qiv_check_reserved_lock;
  base.xFileControl = qiv_file_control;
  base.xSectorSize = qiv_sector_size;
  base.xDeviceCharacteristics = qiv_device_characteristics;
  qiv.io_v1 = base;
  base.iVersion = 2;
  base.xShmMap = qiv_shm_map;
  base.xShmLock = qiv_shm_lock;
  base.xShmBarrier = qiv_shm_barrier;
  base.xShmUnmap = qiv_shm_unmap;
  qiv.io_v2 = base;
  base.iVersion = 3;
  base.xFetch = qiv_fetch;
  base.xUnfetch = qiv_unfetch;
  qiv.io_v3 = base;
}

static int qiv_open(sqlite3_vfs *wrapper, sqlite3_filename name,
                    sqlite3_file *base, int flags, int *out_flags) {
  qiv_file *file = (qiv_file *)base;
  uint64_t handle_id = 0;
  unsigned record_index = 0;
  int rc;
  int callback_rc;
  qiv_event event;
  (void)wrapper;
  memset(file, 0, sizeof(*file));
  if (qiv_scope_id == 0) return SQLITE_CANTOPEN;
  if (qiv_verification_only) {
    /* The main FD needs O_RDWR for POSIX exclusive byte-range locks, but
     * every write path remains denied for this handle through its close.
     * WAL is an existing read-only file; no source sidecar may be born. */
    if (!name || (flags & (SQLITE_OPEN_MAIN_DB | SQLITE_OPEN_WAL)) == 0)
      return SQLITE_CANTOPEN;
    flags &= ~(SQLITE_OPEN_CREATE | SQLITE_OPEN_DELETEONCLOSE);
    flags |= SQLITE_OPEN_NOFOLLOW;
    if (flags & SQLITE_OPEN_WAL) {
      flags &= ~SQLITE_OPEN_READWRITE;
      flags |= SQLITE_OPEN_READONLY;
    } else {
      flags &= ~SQLITE_OPEN_READONLY;
      flags |= SQLITE_OPEN_READWRITE;
    }
    file->verification_only = 1;
  }
  rc = qiv_allocate_open_slot(qiv_scope_id, &handle_id, &record_index);
  if (rc != SQLITE_OK) return rc;
  event = qiv_event_for(QIV_BEFORE_OPEN, qiv_scope_id, handle_id,
                        name, flags);
  callback_rc = qiv_emit(&event);
  if (callback_rc != SQLITE_OK) {
    qiv_release_open_slot(record_index);
    return callback_rc;
  }
  /* SQLite can pass NULL for delete-on-close temp files. The binder must
   * return an authenticated, root-private redirect before native xOpen. */
  if (!name) {
    if ((flags & SQLITE_OPEN_DELETEONCLOSE) == 0 ||
        event.redirect_len == 0 ||
        event.redirect_len >= QIV_REDIRECT_MAX ||
        event.redirect_len > (uint32_t)qiv.wrapper.mxPathname ||
        event.redirect[event.redirect_len] != '\0') {
      event.kind = QIV_AFTER_OPEN;
      event.sqlite_result = SQLITE_CANTOPEN;
      qiv_emit(&event);
      event.kind = QIV_OPEN_ABORTED_NO_FD;
      event.fd_retired = 1;
      qiv_emit(&event);
      qiv_release_open_slot(record_index);
      return SQLITE_CANTOPEN;
    }
    file->owned_path = sqlite3_malloc64((sqlite3_uint64)event.redirect_len + 1);
    if (!file->owned_path) {
      event.kind = QIV_AFTER_OPEN;
      event.sqlite_result = SQLITE_NOMEM;
      qiv_emit(&event);
      event.kind = QIV_OPEN_ABORTED_NO_FD;
      event.fd_retired = 1;
      qiv_emit(&event);
      qiv_release_open_slot(record_index);
      return SQLITE_NOMEM;
    }
    memcpy(file->owned_path, event.redirect, event.redirect_len + 1);
    name = file->owned_path;
  } else if (event.redirect_len != 0) {
    event.kind = QIV_AFTER_OPEN;
    event.sqlite_result = SQLITE_MISUSE;
    qiv_emit(&event);
    event.kind = QIV_OPEN_ABORTED_NO_FD;
    event.fd_retired = 1;
    qiv_emit(&event);
    qiv_release_open_slot(record_index);
    return SQLITE_MISUSE;
  } else {
    size_t path_length = strlen(name);
    if (path_length > (size_t)qiv.wrapper.mxPathname) {
      event.kind = QIV_AFTER_OPEN;
      event.sqlite_result = SQLITE_CANTOPEN;
      qiv_emit(&event);
      event.kind = QIV_OPEN_ABORTED_NO_FD;
      event.fd_retired = 1;
      qiv_emit(&event);
      qiv_release_open_slot(record_index);
      return SQLITE_CANTOPEN;
    }
    file->owned_path = sqlite3_malloc64((sqlite3_uint64)path_length + 1);
    if (!file->owned_path) {
      event.kind = QIV_AFTER_OPEN;
      event.sqlite_result = SQLITE_NOMEM;
      qiv_emit(&event);
      event.kind = QIV_OPEN_ABORTED_NO_FD;
      event.fd_retired = 1;
      qiv_emit(&event);
      qiv_release_open_slot(record_index);
      return SQLITE_NOMEM;
    }
    memcpy(file->owned_path, name, path_length + 1);
  }
  file->path = file->owned_path;
  file->scope_id = qiv_scope_id;
  file->handle_id = handle_id;
  file->record_index = record_index;
  file->open_flags = flags;
  file->inner = (sqlite3_file *)((unsigned char *)file + sizeof(*file));
  memset(file->inner, 0, (size_t)qiv.original->szOsFile);
  event.kind = QIV_BEFORE_NATIVE_OPEN;
  event.path = file->path;
  callback_rc = qiv_emit(&event);
  if (callback_rc != SQLITE_OK) {
    event.kind = QIV_OPEN_ABORTED_NO_FD;
    event.fd_retired = 1;
    qiv_emit(&event);
    qiv_release_open_slot(record_index);
    sqlite3_free(file->owned_path);
    file->owned_path = NULL;
    return callback_rc;
  }
  rc = qiv.original->xOpen(qiv.original, name, file->inner,
                           flags, out_flags);
  if (rc == SQLITE_OK) {
    int descriptor = qiv_unix_file_descriptor(file->inner, 0);
    qiv_observe_witness(file, &event, 0);
    qiv_record_native_descriptor(record_index, descriptor, 0, &event);
  }
  event.kind = QIV_AFTER_OPEN;
  event.path = file->path;
  event.sqlite_result = rc;
  if (rc == SQLITE_OK) {
    const sqlite3_io_methods *methods = file->inner->pMethods;
    if (!methods || methods->iVersion < 1 || !methods->xClose ||
        !methods->xRead || !methods->xWrite || !methods->xTruncate ||
        !methods->xSync || !methods->xFileSize || !methods->xLock || !methods->xUnlock) {
      rc = SQLITE_IOERR;
    } else if (methods->iVersion >= 3 && methods->xFetch &&
               methods->xUnfetch && methods->xShmMap && methods->xShmLock &&
               methods->xShmBarrier && methods->xShmUnmap) {
      file->base.pMethods = &qiv.io_v3;
    } else if (methods->iVersion >= 2 && methods->xShmMap &&
               methods->xShmLock && methods->xShmBarrier &&
               methods->xShmUnmap) {
      file->base.pMethods = &qiv.io_v2;
    } else {
      file->base.pMethods = &qiv.io_v1;
    }
    if (rc == SQLITE_OK) {
      qiv_observe_size(file, &event);
      qiv_observe_witness(file, &event, 0);
      /* Lock before SQLite reads any header/schema, including an immutable
       * main-only open when no committed WAL can exist. Native unix locking
       * also detects other SQLite connections in this same process. */
      if (file->verification_only && (flags & SQLITE_OPEN_MAIN_DB)) {
        rc = methods->xLock(file->inner, SQLITE_LOCK_SHARED);
        if (rc == SQLITE_OK)
          rc = methods->xLock(file->inner, SQLITE_LOCK_EXCLUSIVE);
      }
    }
  }
  if (rc == SQLITE_OK &&
      (qiv.records[record_index].main_fd < 0 || !event.witness_valid))
    rc = SQLITE_IOERR;
  event.sqlite_result = rc;
  callback_rc = qiv_emit(&event);
  if (rc != SQLITE_OK || callback_rc != SQLITE_OK) {
    if (file->inner->pMethods && file->inner->pMethods->xClose) {
      qiv_event close_event = qiv_file_event(file, QIV_AFTER_CLOSE);
      qiv_record *record;
      qiv_observe_witness(file, &close_event, 0);
      int close_rc = file->inner->pMethods->xClose(file->inner);
      close_event.sqlite_result = close_rc;
      sqlite3_mutex_enter(qiv.mutex);
      record = &qiv.records[record_index];
      record->logical_closed = 1;
      close_event.fd_retired = record->main_retired;
      if (record->main_retired)
        qiv_record_to_event(&close_event, &record->main_witness);
      record->main_inflight = 1;
      sqlite3_mutex_leave(qiv.mutex);
      int close_callback_rc = qiv_emit(&close_event);
      sqlite3_mutex_enter(qiv.mutex);
      record = &qiv.records[record_index];
      record->main_inflight = 0;
      if (close_callback_rc == SQLITE_OK && close_event.fd_retired)
        record->main_notified = 1;
      qiv_try_release_record(record_index);
      sqlite3_mutex_leave(qiv.mutex);
    } else if (qiv.records[record_index].main_fd < 0) {
      qiv_release_open_slot(record_index);
    }
    file->base.pMethods = NULL;
    sqlite3_free(file->owned_path);
    file->owned_path = NULL;
    return rc == SQLITE_OK ? callback_rc : rc;
  }
  return SQLITE_OK;
}

static int qiv_delete(sqlite3_vfs *wrapper, const char *name,
                      int sync_directory) {
  qiv_event event;
  int callback_rc;
  int lower_rc;
  (void)wrapper;
  if (qiv_scope_id == 0 || !name) return SQLITE_IOERR_DELETE;
  if (qiv_verification_only) return SQLITE_READONLY;
  event = qiv_event_for(QIV_BEFORE_DELETE, qiv_scope_id, 0, name, 0);
  event.operation = sync_directory;
  callback_rc = qiv_emit(&event);
  if (callback_rc != SQLITE_OK) return callback_rc;
  lower_rc = qiv.original->xDelete(qiv.original, name, sync_directory);
  event.kind = QIV_AFTER_DELETE;
  event.sqlite_result = lower_rc;
  callback_rc = qiv_emit(&event);
  return lower_rc == SQLITE_OK ? callback_rc : lower_rc;
}

static int qiv_access(sqlite3_vfs *wrapper, const char *name,
                      int flags, int *result) {
  (void)wrapper;
  return qiv.original->xAccess(qiv.original, name, flags, result);
}

static int qiv_full_pathname(sqlite3_vfs *wrapper, const char *name,
                             int length, char *output) {
  (void)wrapper;
  return qiv.original->xFullPathname(qiv.original, name, length, output);
}

static void *qiv_dl_open(sqlite3_vfs *wrapper, const char *name) {
  (void)wrapper;
  return qiv.original->xDlOpen(qiv.original, name);
}

static void qiv_dl_error(sqlite3_vfs *wrapper, int length, char *message) {
  (void)wrapper;
  qiv.original->xDlError(qiv.original, length, message);
}

static void (*qiv_dl_sym(sqlite3_vfs *wrapper, void *handle,
                         const char *name))(void) {
  (void)wrapper;
  return qiv.original->xDlSym(qiv.original, handle, name);
}

static void qiv_dl_close(sqlite3_vfs *wrapper, void *handle) {
  (void)wrapper;
  qiv.original->xDlClose(qiv.original, handle);
}

static int qiv_randomness(sqlite3_vfs *wrapper, int length, char *buffer) {
  (void)wrapper;
  return qiv.original->xRandomness(qiv.original, length, buffer);
}

static int qiv_sleep(sqlite3_vfs *wrapper, int microseconds) {
  (void)wrapper;
  return qiv.original->xSleep(qiv.original, microseconds);
}

static int qiv_current_time(sqlite3_vfs *wrapper, double *time) {
  (void)wrapper;
  return qiv.original->xCurrentTime(qiv.original, time);
}

static int qiv_get_last_error(sqlite3_vfs *wrapper, int length, char *message) {
  (void)wrapper;
  return qiv.original->xGetLastError(qiv.original, length, message);
}

static int qiv_current_time_int64(sqlite3_vfs *wrapper, sqlite3_int64 *time) {
  (void)wrapper;
  return qiv.original->xCurrentTimeInt64(qiv.original, time);
}

static int qiv_set_system_call(sqlite3_vfs *wrapper, const char *name,
                               sqlite3_syscall_ptr pointer) {
  (void)wrapper;
  return qiv.original->xSetSystemCall(qiv.original, name, pointer);
}

static sqlite3_syscall_ptr qiv_get_system_call(sqlite3_vfs *wrapper,
                                               const char *name) {
  (void)wrapper;
  return qiv.original->xGetSystemCall(qiv.original, name);
}

static const char *qiv_next_system_call(sqlite3_vfs *wrapper,
                                        const char *name) {
  (void)wrapper;
  return qiv.original->xNextSystemCall(qiv.original, name);
}

int qiv_register(const char *name, unsigned max_open_handles,
                 qiv_callback callback, void *context) {
  sqlite3_vfs *original;
  size_t name_len;
  int rc;
  if (!name || !callback || !context || max_open_handles == 0 || qiv.registered)
    return SQLITE_MISUSE;
  name_len = strlen(name);
  if (name_len == 0 || name_len > 63) return SQLITE_MISUSE;
  original = sqlite3_vfs_find(NULL);
  if (!original || sqlite3_vfs_find(name) || original->szOsFile < 0 ||
      original->szOsFile > INT_MAX - (int)sizeof(qiv_file) ||
      strcmp(original->zName, "unix") != 0) return SQLITE_MISUSE;
#if SIZE_MAX <= UINT32_MAX
  if (max_open_handles > SIZE_MAX / sizeof(qiv_record)) return SQLITE_NOMEM;
#endif
  qiv.mutex = sqlite3_mutex_alloc(SQLITE_MUTEX_FAST);
  if (!qiv.mutex) return SQLITE_NOMEM;
  qiv.records = sqlite3_malloc64((sqlite3_uint64)max_open_handles *
                                  sizeof(qiv_record));
  if (!qiv.records) {
    sqlite3_mutex_free(qiv.mutex);
    qiv.mutex = NULL;
    return SQLITE_NOMEM;
  }
  memset(qiv.records, 0, (size_t)max_open_handles * sizeof(qiv_record));
  qiv.name = sqlite3_malloc64((sqlite3_uint64)name_len + 1);
  if (!qiv.name) {
    sqlite3_free(qiv.records);
    qiv.records = NULL;
    sqlite3_mutex_free(qiv.mutex);
    qiv.mutex = NULL;
    return SQLITE_NOMEM;
  }
  memcpy(qiv.name, name, name_len + 1);
  qiv.original = original;
  qiv.callback = callback;
  qiv.context = context;
  qiv.max_open_handles = max_open_handles;
  qiv.wrapper = *original;
  qiv.wrapper.zName = qiv.name;
  qiv.wrapper.szOsFile = (int)sizeof(qiv_file) + original->szOsFile;
  qiv.wrapper.pAppData = NULL;
  qiv.wrapper.pNext = NULL;
  qiv.wrapper.xOpen = qiv_open;
  qiv.wrapper.xDelete = qiv_delete;
  qiv.wrapper.xAccess = qiv_access;
  qiv.wrapper.xFullPathname = qiv_full_pathname;
  qiv.wrapper.xDlOpen = original->xDlOpen ? qiv_dl_open : NULL;
  qiv.wrapper.xDlError = original->xDlError ? qiv_dl_error : NULL;
  qiv.wrapper.xDlSym = original->xDlSym ? qiv_dl_sym : NULL;
  qiv.wrapper.xDlClose = original->xDlClose ? qiv_dl_close : NULL;
  qiv.wrapper.xRandomness = qiv_randomness;
  qiv.wrapper.xSleep = qiv_sleep;
  qiv.wrapper.xCurrentTime = qiv_current_time;
  qiv.wrapper.xGetLastError = original->xGetLastError ? qiv_get_last_error : NULL;
  if (original->iVersion >= 2 && original->xCurrentTimeInt64)
    qiv.wrapper.xCurrentTimeInt64 = qiv_current_time_int64;
  if (original->iVersion >= 3) {
    qiv.wrapper.xSetSystemCall = original->xSetSystemCall ? qiv_set_system_call : NULL;
    qiv.wrapper.xGetSystemCall = original->xGetSystemCall ? qiv_get_system_call : NULL;
    qiv.wrapper.xNextSystemCall = original->xNextSystemCall ? qiv_next_system_call : NULL;
  }
  qiv_initialize_io_methods();
  rc = sqlite3_vfs_register(&qiv.wrapper, 0);
  if (rc == SQLITE_OK) {
    qiv.registered = 1;
    atomic_store_explicit(&qiv_active, 1, memory_order_release);
  } else {
    sqlite3_free(qiv.name);
    sqlite3_free(qiv.records);
    sqlite3_mutex_free(qiv.mutex);
    memset(&qiv, 0, sizeof(qiv));
  }
  return rc;
}

#ifdef QUANTA_CAPACITY_VFS_TEST_HOOKS
int qiv_test_first_shm_read_map(const char *path, int *mapping_present) {
  sqlite3 *db = NULL;
  sqlite3_file *file = NULL;
  const sqlite3_io_methods *methods = NULL;
  void volatile *mapping = NULL;
  int primary_rc, unmap_rc = SQLITE_OK, close_rc = SQLITE_OK;
  int attempted_map = 0;
  if (!qiv.registered || qiv_scope_id == 0 || !path || !mapping_present)
    return SQLITE_MISUSE;
  *mapping_present = 0;
  /* Let Pager construct the native sqlite3_filename: unixOpenSharedMemory
   * calls sqlite3_uri_boolean(), which requires Pager's filename prefix and
   * double-NUL layout. A raw C string passed to xOpen is not valid here. */
  primary_rc = sqlite3_open_v2(path, &db, SQLITE_OPEN_READWRITE, qiv.name);
  if (primary_rc != SQLITE_OK) {
    if (db) sqlite3_close(db);
    return primary_rc;
  }
  primary_rc = sqlite3_file_control(db, "main", SQLITE_FCNTL_FILE_POINTER,
                                    &file);
  if (primary_rc != SQLITE_OK) goto probe_close;
  if (!file || !file->pMethods) {
    primary_rc = SQLITE_IOERR;
    goto probe_close;
  }
  methods = file->pMethods;
  if (sqlite3_db_readonly(db, "main") != 0) {
    primary_rc = SQLITE_READONLY;
  } else if (methods->iVersion < 2 || !methods->xShmMap ||
             !methods->xShmUnmap) {
    primary_rc = SQLITE_IOERR_SHMMAP;
  } else {
    attempted_map = 1;
    primary_rc = methods->xShmMap(file, 0, 32768, 0, &mapping);
    if (primary_rc == SQLITE_OK) *mapping_present = mapping != NULL;
  }
  /* A failed xShmMap can still have opened the native SHM FD. Unmap before
   * closing, but never replace the original refusal with a cleanup error. */
  if (attempted_map)
    unmap_rc = methods->xShmUnmap(file, 0);
probe_close:
  close_rc = sqlite3_close(db);
  if (primary_rc != SQLITE_OK) return primary_rc;
  if (unmap_rc != SQLITE_OK) return unmap_rc;
  return close_rc;
}
#endif
