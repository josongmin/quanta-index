/* Insert into pinned sqlite3.c immediately after struct unixShm, inside its
 * SQLITE_OS_UNIX block. This is Quanta Index authored code; it inspects the
 * actual native SQLite FD, never a reopened pathname. */
#include <stdint.h>
#include <limits.h>
#if defined(__DJGPP__)
#error "Quanta native FD witness is unsupported on DJGPP"
#endif

static int unixShmRegionPerMap(void);

/* Match unixShmMap's rounded nReqRegion before any native SHM write. */
int qiv_unix_shm_growth_target(int region, int region_size,
                               sqlite3_int64 *target) {
  sqlite3_int64 requested, rounded, per_map;
  if (!target || region < 0 || region_size <= 0) return SQLITE_IOERR_SHMMAP;
  per_map = unixShmRegionPerMap();
  if (per_map <= 0) return SQLITE_IOERR_SHMMAP;
  requested = (sqlite3_int64)region + 1;
  rounded = ((requested + per_map - 1) / per_map) * per_map;
  if (rounded > INT_MAX || rounded > INT64_MAX / region_size)
    return SQLITE_IOERR_SHMMAP;
  *target = rounded * region_size;
  return SQLITE_OK;
}

int qiv_unix_file_descriptor(sqlite3_file *native_file, int shared_memory) {
  unixFile *file;
  if (!native_file) return -1;
  file = (unixFile *)native_file;
  if (!file->pVfs || strcmp(file->pVfs->zName, "unix") != 0)
    return -1;
  if (shared_memory) {
    if (!file->pShm || !file->pShm->pShmNode) return -1;
    return file->pShm->pShmNode->hShm;
  }
  return file->h;
}

/* Only a controlled xSync arms this per-unixFile slot. Nested xSync on the
 * same file invalidates both observations rather than borrowing an outcome. */
void qiv_unix_sync_probe_begin(sqlite3_file *native_file) {
  unixFile *file;
  if( !native_file ) return;
  file = (unixFile *)native_file;
  if( !file->pVfs || strcmp(file->pVfs->zName, "unix") != 0 || file->h < 0 )
    return;
  if( file->qiv_sync_probe_depth == 0 ){
    memset(&file->qiv_sync_probe, 0, sizeof(file->qiv_sync_probe));
    file->qiv_sync_probe_fd = file->h;
  }else{
    file->qiv_sync_probe.ambiguous = 1;
  }
  if( file->qiv_sync_probe_depth < UINT_MAX ) file->qiv_sync_probe_depth++;
}

void qiv_unix_sync_probe_end(sqlite3_file *native_file,
                             qiv_native_sync_observation *observation) {
  unixFile *file;
  if( !observation ) return;
  if( !native_file ){
    observation->ambiguous = 1;
    return;
  }
  file = (unixFile *)native_file;
  if( !file->pVfs || strcmp(file->pVfs->zName, "unix") != 0 ){
    observation->ambiguous = 1;
    file->qiv_sync_probe_depth = 0;
    return;
  }
  if( file->qiv_sync_probe_depth == 0 ){
    observation->ambiguous = 1;
    return;
  }
  if( file->h != file->qiv_sync_probe_fd ){
    file->qiv_sync_probe.same_native_fd = 0;
    file->qiv_sync_probe.ambiguous = 1;
  }
  if( file->qiv_sync_probe_depth > 1 ){
    file->qiv_sync_probe.ambiguous = 1;
    observation->ambiguous = 1;
    file->qiv_sync_probe_depth--;
    return;
  }
  *observation = file->qiv_sync_probe;
  file->qiv_sync_probe_depth = 0;
}

/* A nested Off call must not be attributed to its outer Record call. The
 * normal Off path only reads the slot and leaves it untouched when idle. */
int qiv_unix_sync_probe_nested_off_begin(sqlite3_file *native_file) {
  unixFile *file;
  if( !native_file ) return 0;
  file = (unixFile *)native_file;
  if( file->qiv_sync_probe_depth == 0 || !file->pVfs ||
      strcmp(file->pVfs->zName, "unix") != 0 ) return 0;
  file->qiv_sync_probe.ambiguous = 1;
  if( file->qiv_sync_probe_depth < UINT_MAX ) file->qiv_sync_probe_depth++;
  return 1;
}

void qiv_unix_sync_probe_nested_off_end(sqlite3_file *native_file) {
  unixFile *file;
  if( !native_file ) return;
  file = (unixFile *)native_file;
  if( file->qiv_sync_probe_depth > 1 ) file->qiv_sync_probe_depth--;
}

#ifdef QUANTA_CAPACITY_VFS_TEST_HOOKS
#if HAVE_FULLFSYNC
static int qiv_test_fullsync_success(int descriptor, int command, ...) {
  (void)descriptor;
  if( command == F_FULLFSYNC ) return 0;
  errno = EINVAL;
  return -1;
}

static int qiv_test_fullsync_failure(int descriptor, int command, ...) {
  (void)descriptor;
  (void)command;
  errno = EIO;
  return -1;
}
#endif

/* Inject only into this call's private argument. No process-global SQLite
 * syscall pointer is changed. The oracle uses the production sync body on
 * the caller's borrowed regular-file FD. */
int qiv_test_native_sync_syscall_outcomes(int descriptor) {
  qiv_native_sync_observation observation;
  struct stat metadata;
  int original_errno = errno;
  int result = SQLITE_OK;
  int sync_result;

  if( qiv_test_native_sync_probe_reset() != SQLITE_OK ) result = SQLITE_ERROR;

  if( descriptor < 0 || osFstat(descriptor, &metadata) != 0 ||
      !S_ISREG(metadata.st_mode) ){
    result = SQLITE_MISUSE;
    goto finish;
  }
#ifdef SQLITE_NO_SYNC
  result = SQLITE_NOTFOUND;
  goto finish;
#endif

  memset(&observation, 0, sizeof(observation));
  sync_result = full_fsync_observed(descriptor, 0, 0, &observation);
  if( sync_result != 0 || observation.full_requested ||
      observation.full_attempted || observation.full_default_syscall ||
      !observation.posix_attempted || observation.posix_result != 0 ||
      observation.posix_errno != 0 ) result = SQLITE_ERROR;

#if HAVE_FULLFSYNC
  memset(&observation, 0, sizeof(observation));
  sync_result = full_fsync_with_fcntl(
      descriptor, 1, 0, &observation,
      (sqlite3_syscall_ptr)qiv_test_fullsync_success);
  if( sync_result != 0 || !observation.full_requested ||
      !observation.full_attempted || observation.full_default_syscall ||
      observation.full_result != 0 || observation.full_errno != 0 ||
      observation.posix_attempted ) result = SQLITE_ERROR;

  memset(&observation, 0, sizeof(observation));
  sync_result = full_fsync_with_fcntl(
      descriptor, 1, 0, &observation,
      (sqlite3_syscall_ptr)qiv_test_fullsync_failure);
  if( sync_result != 0 || !observation.full_requested ||
      !observation.full_attempted || observation.full_default_syscall ||
      observation.full_result != -1 || observation.full_errno != EIO ||
      !observation.posix_attempted || observation.posix_result != 0 ||
      observation.posix_errno != 0 ) result = SQLITE_ERROR;

  memset(&observation, 0, sizeof(observation));
  sync_result = full_fsync_with_fcntl(
      -1, 1, 0, &observation,
      (sqlite3_syscall_ptr)qiv_test_fullsync_failure);
  if( sync_result != -1 || !observation.full_requested ||
      !observation.full_attempted || observation.full_default_syscall ||
      observation.full_result != -1 || observation.full_errno != EIO ||
      !observation.posix_attempted || observation.posix_result != -1 ||
      observation.posix_errno != EBADF ) result = SQLITE_ERROR;
#else
  /* F_FULLFSYNC is unavailable: verify only the POSIX failure path. */
  memset(&observation, 0, sizeof(observation));
  sync_result = full_fsync_observed(-1, 0, 0, &observation);
  if( sync_result != -1 || observation.full_requested ||
      observation.full_attempted || observation.full_default_syscall ||
      !observation.posix_attempted || observation.posix_result != -1 ||
      observation.posix_errno != EBADF ) result = SQLITE_ERROR;
#endif

finish:
  errno = original_errno;
  return result;
}

int qiv_test_native_sync_probe_reset(void) {
  unixFile file;
  sqlite3_vfs vfs;
  qiv_native_sync_observation observation;
  memset(&file, 0, sizeof(file));
  memset(&vfs, 0, sizeof(vfs));
  memset(&observation, 0, sizeof(observation));
  vfs.zName = "unix";
  file.pVfs = &vfs;
  file.h = 7;
  qiv_unix_sync_probe_begin((sqlite3_file *)&file);
  file.qiv_sync_probe.observed = 1;
  qiv_unix_sync_probe_end((sqlite3_file *)&file, &observation);
  if( !observation.observed || file.qiv_sync_probe_depth != 0 )
    return SQLITE_ERROR;
  qiv_unix_sync_probe_begin((sqlite3_file *)&file);
  if( file.qiv_sync_probe.observed || file.qiv_sync_probe.ambiguous )
    return SQLITE_ERROR;
  if( !qiv_unix_sync_probe_nested_off_begin((sqlite3_file *)&file) ||
      file.qiv_sync_probe_depth != 2 ) return SQLITE_ERROR;
  qiv_unix_sync_probe_nested_off_end((sqlite3_file *)&file);
  qiv_unix_sync_probe_end((sqlite3_file *)&file, &observation);
  if( !observation.ambiguous || file.qiv_sync_probe_depth != 0 )
    return SQLITE_ERROR;
  qiv_unix_sync_probe_begin((sqlite3_file *)&file);
  qiv_unix_sync_probe_end((sqlite3_file *)&file, &observation);
  return observation.observed == 0 && observation.ambiguous == 0 &&
         file.qiv_sync_probe_depth == 0 ? SQLITE_OK : SQLITE_ERROR;
}
#endif

int qiv_unix_descriptor_witness(
    int descriptor,
    uint64_t *device, uint64_t *inode, uint64_t *bytes, uint64_t *links) {
  struct stat metadata;
  if (!device || !inode || !bytes || !links) return SQLITE_MISUSE;
  if (descriptor < 0) return SQLITE_NOTFOUND;
  if (osFstat(descriptor, &metadata) != 0) return SQLITE_IOERR_FSTAT;
  if (metadata.st_size < 0) return SQLITE_IOERR_FSTAT;
  *device = (uint64_t)metadata.st_dev;
  *inode = (uint64_t)metadata.st_ino;
  *bytes = (uint64_t)metadata.st_size;
  *links = (uint64_t)metadata.st_nlink;
  return SQLITE_OK;
}

int qiv_unix_file_witness(
    sqlite3_file *native_file, int shared_memory,
    uint64_t *device, uint64_t *inode, uint64_t *bytes, uint64_t *links) {
  return qiv_unix_descriptor_witness(
      qiv_unix_file_descriptor(native_file, shared_memory),
      device, inode, bytes, links);
}

/* xShmMap may extend the SHM file without a later xSync. Synchronize the
 * actual SQLite-owned SHM FD, never a separately opened descriptor. */
int qiv_unix_sync_shm(sqlite3_file *native_file) {
  int descriptor = qiv_unix_file_descriptor(native_file, 1);
  if (descriptor < 0) return SQLITE_IOERR_FSYNC;
  return full_fsync(descriptor, 0, 0) == 0 ? SQLITE_OK : SQLITE_IOERR_FSYNC;
}
