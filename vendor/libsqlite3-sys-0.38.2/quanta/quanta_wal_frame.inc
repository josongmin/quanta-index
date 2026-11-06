/* Quanta-authored bounded WAL frame packing. Keep SQLite's canonical
 * encoding and walWriteToLog sync-point/error protocol unchanged. */
#define QIV_MAX_PACKED_WAL_PAGE 4096
#define QIV_MAX_WAL_APPEND_BATCH 8

static int qiv_write_packed_wal_frame(
    WalWriter *writer, const u8 *header, const void *page, i64 offset) {
  u8 frame[WAL_FRAME_HDRSIZE + QIV_MAX_PACKED_WAL_PAGE];
  if (writer->szPage <= 0 || writer->szPage > QIV_MAX_PACKED_WAL_PAGE)
    return SQLITE_IOERR_WRITE;
  memcpy(frame, header, WAL_FRAME_HDRSIZE);
  memcpy(frame + WAL_FRAME_HDRSIZE, page, writer->szPage);
  /* This is one real admitted write, not a buffered success or a direct-FD
   * bypass. walWriteToLog still splits at the original sync boundary. */
  return walWriteToLog(writer, frame, WAL_FRAME_HDRSIZE + writer->szPage, offset);
}

/* Encode consecutive append frames in SQLite's original dirty-list order.
 * The result is visible to the caller only after one real WAL write succeeds. */
static int qiv_write_wal_append_batch(
    WalWriter *writer, PgHdr *first, int nTruncate, int isCommit,
    i64 offset, PgHdr **last_out, int *count_out) {
  u8 frames[QIV_MAX_WAL_APPEND_BATCH *
            (WAL_FRAME_HDRSIZE + QIV_MAX_PACKED_WAL_PAGE)];
  int frame_size;
  PgHdr *page = first;
  PgHdr *last = 0;
  int count = 0;
  int rc;
  u32 saved_checksum[2];
  if (writer->szPage <= 0 || writer->szPage > QIV_MAX_PACKED_WAL_PAGE ||
      first == 0 || last_out == 0 || count_out == 0)
    return SQLITE_IOERR_WRITE;
  frame_size = WAL_FRAME_HDRSIZE + writer->szPage;
  saved_checksum[0] = writer->pWal->hdr.aFrameCksum[0];
  saved_checksum[1] = writer->pWal->hdr.aFrameCksum[1];
  while (page != 0 && count < QIV_MAX_WAL_APPEND_BATCH) {
    u8 *frame = frames + count * frame_size;
    int nDbSize = (isCommit && page->pDirty == 0) ? nTruncate : 0;
    walEncodeFrame(writer->pWal, page->pgno, nDbSize, page->pData, frame);
    memcpy(frame + WAL_FRAME_HDRSIZE, page->pData, writer->szPage);
    last = page;
    count++;
    page = page->pDirty;
  }
  rc = walWriteToLog(writer, frames, count * frame_size, offset);
  if (rc != SQLITE_OK) {
    writer->pWal->hdr.aFrameCksum[0] = saved_checksum[0];
    writer->pWal->hdr.aFrameCksum[1] = saved_checksum[1];
    return rc;
  }
  *last_out = last;
  *count_out = count;
  return SQLITE_OK;
}

#ifdef QUANTA_CAPACITY_VFS_TEST_HOOKS
typedef struct qiv_frame_test_file {
  sqlite3_file base;
  u8 bytes[8192];
  u8 synced[8192];
  int writes, syncs, end, synced_end;
  int fail_write, fail_sync;
} qiv_frame_test_file;

static int qiv_frame_test_write(
    sqlite3_file *base, const void *data, int amount, sqlite3_int64 offset) {
  qiv_frame_test_file *file = (qiv_frame_test_file *)base;
  file->writes++;
  if (file->writes == file->fail_write) return SQLITE_FULL;
  if (amount <= 0 || offset < 0 || offset > (i64)sizeof(file->bytes) - amount)
    return SQLITE_IOERR_WRITE;
  memcpy(file->bytes + offset, data, amount);
  if (offset + amount > file->end) file->end = (int)(offset + amount);
  return SQLITE_OK;
}

static int qiv_frame_test_sync(sqlite3_file *base, int flags) {
  qiv_frame_test_file *file = (qiv_frame_test_file *)base;
  file->syncs++;
  if (flags != SQLITE_SYNC_FULL) return SQLITE_ERROR;
  memcpy(file->synced, file->bytes, sizeof(file->bytes));
  file->synced_end = file->end;
  return file->fail_sync ? SQLITE_IOERR_FSYNC : SQLITE_OK;
}

static const sqlite3_io_methods qiv_frame_test_methods = {
  .iVersion = 1,
  .xWrite = qiv_frame_test_write,
  .xSync = qiv_frame_test_sync
};

static void qiv_frame_test_init(qiv_frame_test_file *file, WalWriter *writer) {
  memset(file, 0, sizeof(*file));
  memset(writer, 0, sizeof(*writer));
  file->base.pMethods = &qiv_frame_test_methods;
  writer->pFd = &file->base;
  writer->szPage = QIV_MAX_PACKED_WAL_PAGE;
  writer->syncFlags = SQLITE_SYNC_FULL;
}

int qiv_test_packed_wal_frame(void) {
  u8 header[WAL_FRAME_HDRSIZE], page[QIV_MAX_PACKED_WAL_PAGE];
  const i64 points[] = {0, 48, 56, 2080, 4152};
  qiv_frame_test_file original, packed;
  WalWriter reference, candidate;
  unsigned i, point;
  int rc;
  for (i = 0; i < sizeof(header); i++) header[i] = (u8)i;
  for (i = 0; i < sizeof(page); i++) page[i] = (u8)(i * 13 + 7);
  for (point = 0; point < sizeof(points)/sizeof(points[0]); point++) {
    qiv_frame_test_init(&original, &reference);
    qiv_frame_test_init(&packed, &candidate);
    reference.iSyncPoint = candidate.iSyncPoint = points[point];
    rc = walWriteToLog(&reference, header, sizeof(header), 32);
    if (rc == SQLITE_OK)
      rc = walWriteToLog(&reference, page, sizeof(page), 32 + sizeof(header));
    if (rc != SQLITE_OK ||
        qiv_write_packed_wal_frame(&candidate, header, page, 32) != SQLITE_OK ||
        memcmp(original.bytes, packed.bytes, sizeof(original.bytes)) != 0 ||
        original.end != packed.end || original.syncs != packed.syncs ||
        original.synced_end != packed.synced_end ||
        memcmp(original.synced, packed.synced, sizeof(original.synced)) != 0)
      return SQLITE_ERROR;
    if (point == 0 && (original.writes != 2 || packed.writes != 1))
      return SQLITE_ERROR;
    if (qiv_can_pack_wal_frame(&packed.base)) return SQLITE_ERROR;
  }
  qiv_frame_test_init(&packed, &candidate);
  packed.fail_write = 1;
  if (qiv_write_packed_wal_frame(&candidate, header, page, 32) != SQLITE_FULL ||
      packed.writes != 1 || packed.syncs != 0 || packed.end != 0)
    return SQLITE_ERROR;
  qiv_frame_test_init(&packed, &candidate);
  candidate.iSyncPoint = 2080;
  packed.fail_sync = 1;
  if (qiv_write_packed_wal_frame(&candidate, header, page, 32) != SQLITE_IOERR_FSYNC ||
      packed.writes != 1 || packed.syncs != 1 || packed.end != 2080)
    return SQLITE_ERROR;
  qiv_frame_test_init(&packed, &candidate);
  candidate.iSyncPoint = 2080;
  packed.fail_write = 2;
  if (qiv_write_packed_wal_frame(&candidate, header, page, 32) != SQLITE_FULL ||
      packed.writes != 2 || packed.syncs != 1 || packed.end != 2080)
    return SQLITE_ERROR;
  qiv_frame_test_init(&packed, &candidate);
  candidate.szPage = QIV_MAX_PACKED_WAL_PAGE + 1;
  if (qiv_write_packed_wal_frame(&candidate, header, page, 32) != SQLITE_IOERR_WRITE ||
      packed.writes != 0 || packed.syncs != 0)
    return SQLITE_ERROR;
  return SQLITE_OK;
}
#include "quanta_wal_batch_tests.inc"
#endif
