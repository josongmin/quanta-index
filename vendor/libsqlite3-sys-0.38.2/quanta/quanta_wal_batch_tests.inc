/* Independent test oracle for bounded WAL append packing. Included only from
 * the TEST_HOOKS section after qiv_write_wal_append_batch is defined. */
#define QIV_BATCH_TEST_PAGES 9
#define QIV_BATCH_TEST_BYTES 45000
#define QIV_BATCH_TEST_OFFSET 32

typedef struct qiv_batch_test_file {
  sqlite3_file base;
  u8 bytes[QIV_BATCH_TEST_BYTES];
  u8 synced[QIV_BATCH_TEST_BYTES];
  int writes, syncs, end, synced_end;
  int fail_write, fail_sync;
} qiv_batch_test_file;

static int qiv_batch_test_write(
    sqlite3_file *base, const void *data, int amount, sqlite3_int64 offset) {
  qiv_batch_test_file *file = (qiv_batch_test_file *)base;
  file->writes++;
  if (file->writes == file->fail_write) return SQLITE_FULL;
  if (amount <= 0 || offset < 0 ||
      offset > (i64)sizeof(file->bytes) - amount) return SQLITE_IOERR_WRITE;
  memcpy(file->bytes + offset, data, amount);
  if (offset + amount > file->end) file->end = (int)(offset + amount);
  return SQLITE_OK;
}

static int qiv_batch_test_sync(sqlite3_file *base, int flags) {
  qiv_batch_test_file *file = (qiv_batch_test_file *)base;
  file->syncs++;
  if (flags != SQLITE_SYNC_FULL) return SQLITE_ERROR;
  memcpy(file->synced, file->bytes, sizeof(file->bytes));
  file->synced_end = file->end;
  return file->fail_sync ? SQLITE_IOERR_FSYNC : SQLITE_OK;
}

static const sqlite3_io_methods qiv_batch_test_methods = {
  .iVersion = 1,
  .xWrite = qiv_batch_test_write,
  .xSync = qiv_batch_test_sync
};

static void qiv_batch_test_init(
    qiv_batch_test_file *file, WalWriter *writer, Wal *wal) {
  memset(file, 0, sizeof(*file));
  memset(writer, 0, sizeof(*writer));
  memset(wal, 0, sizeof(*wal));
  memset(file->bytes, 0xa5, sizeof(file->bytes));
  memset(file->synced, 0xa5, sizeof(file->synced));
  file->base.pMethods = &qiv_batch_test_methods;
  writer->pFd = &file->base;
  writer->pWal = wal;
  writer->szPage = QIV_MAX_PACKED_WAL_PAGE;
  writer->syncFlags = SQLITE_SYNC_FULL;
  wal->szPage = QIV_MAX_PACKED_WAL_PAGE;
  wal->hdr.bigEndCksum = SQLITE_BIGENDIAN;
  wal->hdr.aSalt[0] = 0x10203040;
  wal->hdr.aSalt[1] = 0x50607080;
  wal->hdr.aFrameCksum[0] = 0x12345678;
  wal->hdr.aFrameCksum[1] = 0x23456789;
}

static void qiv_batch_test_pages(
    PgHdr pages[QIV_BATCH_TEST_PAGES],
    u8 data[QIV_BATCH_TEST_PAGES][QIV_MAX_PACKED_WAL_PAGE], int count) {
  int i, j;
  memset(pages, 0, QIV_BATCH_TEST_PAGES * sizeof(PgHdr));
  for (i = 0; i < count; i++) {
    pages[i].pgno = (Pgno)(11 + i * 3);
    pages[i].pData = data[i];
    pages[i].pDirty = i + 1 < count ? &pages[i + 1] : 0;
    for (j = 0; j < QIV_MAX_PACKED_WAL_PAGE; j++)
      data[i][j] = (u8)(i * 37 + j * 13 + 7);
  }
}

/* Deliberately do not call the new packer or walWriteOneFrame: the old
 * canonical encoder and separate header/page write path are the oracle. */
static int qiv_batch_test_reference(
    WalWriter *writer, PgHdr *first, int nTruncate, int isCommit,
    i64 offset) {
  PgHdr *page;
  for (page = first; page; page = page->pDirty) {
    u8 header[WAL_FRAME_HDRSIZE];
    int mark = (isCommit && page->pDirty == 0) ? nTruncate : 0;
    int rc;
    walEncodeFrame(writer->pWal, page->pgno, mark, page->pData, header);
    rc = walWriteToLog(writer, header, WAL_FRAME_HDRSIZE, offset);
    if (rc != SQLITE_OK) return rc;
    rc = walWriteToLog(writer, page->pData, writer->szPage,
                       offset + WAL_FRAME_HDRSIZE);
    if (rc != SQLITE_OK) return rc;
    offset += WAL_FRAME_HDRSIZE + writer->szPage;
  }
  return SQLITE_OK;
}

static int qiv_batch_test_same_file(
    const qiv_batch_test_file *reference, const qiv_batch_test_file *candidate) {
  return reference->end == candidate->end &&
      reference->syncs == candidate->syncs &&
      reference->synced_end == candidate->synced_end &&
      memcmp(reference->bytes, candidate->bytes, sizeof(reference->bytes)) == 0 &&
      memcmp(reference->synced, candidate->synced,
             sizeof(reference->synced)) == 0;
}

static int qiv_batch_test_success(int count, int isCommit, i64 sync_point) {
  qiv_batch_test_file reference, candidate;
  WalWriter old_writer, new_writer;
  Wal old_wal, new_wal;
  PgHdr pages[QIV_BATCH_TEST_PAGES], *cursor, *last;
  u8 data[QIV_BATCH_TEST_PAGES][QIV_MAX_PACKED_WAL_PAGE];
  int consumed = 0, written, i;
  i64 offset = QIV_BATCH_TEST_OFFSET;
  qiv_batch_test_pages(pages, data, count);
  qiv_batch_test_init(&reference, &old_writer, &old_wal);
  qiv_batch_test_init(&candidate, &new_writer, &new_wal);
  old_writer.iSyncPoint = new_writer.iSyncPoint = sync_point;
  if (qiv_batch_test_reference(&old_writer, pages, 211, isCommit,
                               QIV_BATCH_TEST_OFFSET) != SQLITE_OK)
    return SQLITE_ERROR;
  cursor = pages;
  while (cursor != 0) {
    last = 0;
    written = 0;
    if (qiv_write_wal_append_batch(&new_writer, cursor, 211, isCommit,
                                   offset, &last, &written) != SQLITE_OK ||
        written < 1 || written > QIV_MAX_WAL_APPEND_BATCH || last == 0 ||
        last != &pages[consumed + written - 1])
      return SQLITE_ERROR;
    consumed += written;
    offset += (i64)written * (WAL_FRAME_HDRSIZE + new_writer.szPage);
    if (count == 9 && consumed == QIV_MAX_WAL_APPEND_BATCH) {
      int j;
      if (candidate.end != offset) return SQLITE_ERROR;
      for (j = candidate.end; j < QIV_BATCH_TEST_BYTES; j++) {
        if (candidate.bytes[j] != 0xa5) return SQLITE_ERROR;
      }
    }
    cursor = last->pDirty;
  }
  if (consumed != count || !qiv_batch_test_same_file(&reference, &candidate) ||
      old_wal.hdr.aFrameCksum[0] != new_wal.hdr.aFrameCksum[0] ||
      old_wal.hdr.aFrameCksum[1] != new_wal.hdr.aFrameCksum[1])
    return SQLITE_ERROR;
  for (i = 0; i < count; i++) {
    const i64 frame = QIV_BATCH_TEST_OFFSET +
        (i64)i * (WAL_FRAME_HDRSIZE + new_writer.szPage);
    const u8 *header = candidate.bytes + frame;
    const int mark = isCommit && i == count - 1 ? 211 : 0;
    if (sqlite3Get4byte(header) != pages[i].pgno ||
        sqlite3Get4byte(header + 4) != (u32)mark ||
        memcmp(header + 8, new_wal.hdr.aSalt, 8) != 0 ||
        memcmp(header + 16, reference.bytes + frame + 16, 8) != 0)
      return SQLITE_ERROR;
  }
  if (sync_point == 0 &&
      (reference.writes != 2 * count ||
       candidate.writes != (count + QIV_MAX_WAL_APPEND_BATCH - 1) /
                           QIV_MAX_WAL_APPEND_BATCH))
    return SQLITE_ERROR;
  return SQLITE_OK;
}

static int qiv_batch_test_fault(int old_fail_write, int new_fail_write,
                                int fail_sync, i64 sync_point, int expected) {
  qiv_batch_test_file reference, candidate;
  WalWriter old_writer, new_writer;
  Wal old_wal, new_wal;
  PgHdr pages[QIV_BATCH_TEST_PAGES], sentinel, *last = &sentinel;
  u8 data[QIV_BATCH_TEST_PAGES][QIV_MAX_PACKED_WAL_PAGE];
  int written = -7;
  qiv_batch_test_pages(pages, data, 2);
  qiv_batch_test_init(&reference, &old_writer, &old_wal);
  qiv_batch_test_init(&candidate, &new_writer, &new_wal);
  old_writer.iSyncPoint = new_writer.iSyncPoint = sync_point;
  reference.fail_write = old_fail_write;
  candidate.fail_write = new_fail_write;
  reference.fail_sync = candidate.fail_sync = fail_sync;
  if (qiv_batch_test_reference(&old_writer, pages, 211, 1,
                               QIV_BATCH_TEST_OFFSET) != expected ||
      qiv_write_wal_append_batch(&new_writer, pages, 211, 1,
                                 QIV_BATCH_TEST_OFFSET,
                                 &last, &written) != expected ||
      last != &sentinel || written != -7 ||
      new_wal.hdr.aFrameCksum[0] != 0x12345678 ||
      new_wal.hdr.aFrameCksum[1] != 0x23456789 ||
      !qiv_batch_test_same_file(&reference, &candidate))
    return SQLITE_ERROR;
  return SQLITE_OK;
}

static int qiv_batch_test_reject(
    qiv_batch_test_file *file, WalWriter *writer, PgHdr *first) {
  PgHdr sentinel, *last = &sentinel;
  int count = -7;
  if (qiv_write_wal_append_batch(writer, first, 211, 1,
                                 QIV_BATCH_TEST_OFFSET,
                                 &last, &count) != SQLITE_IOERR_WRITE ||
      last != &sentinel || count != -7 || file->writes != 0 ||
      file->syncs != 0 ||
      writer->pWal->hdr.aFrameCksum[0] != 0x12345678 ||
      writer->pWal->hdr.aFrameCksum[1] != 0x23456789)
    return SQLITE_ERROR;
  return SQLITE_OK;
}

int qiv_test_wal_append_batch(void) {
  static const i64 points[] = {
    0, 31, 32, 33, 56, 57, 4152, 4176, 8272, 8273, 32991
  };
  qiv_batch_test_file file;
  WalWriter writer;
  Wal wal;
  PgHdr pages[QIV_BATCH_TEST_PAGES];
  u8 data[QIV_BATCH_TEST_PAGES][QIV_MAX_PACKED_WAL_PAGE];
  unsigned i;
  for (i = 0; i < sizeof(points)/sizeof(points[0]); i++) {
    if (qiv_batch_test_success(2, 1, points[i]) != SQLITE_OK ||
        qiv_batch_test_success(8, 1, points[i]) != SQLITE_OK ||
        qiv_batch_test_success(9, 1, points[i]) != SQLITE_OK ||
        qiv_batch_test_success(9, 0, points[i]) != SQLITE_OK)
      return SQLITE_ERROR;
  }
  if (qiv_batch_test_fault(1, 1, 0, 0, SQLITE_FULL) != SQLITE_OK ||
      qiv_batch_test_fault(2, 2, 0, 56, SQLITE_FULL) != SQLITE_OK ||
      /* At 57 the old path writes header and one page byte separately. */
      qiv_batch_test_fault(3, 2, 0, 57, SQLITE_FULL) != SQLITE_OK ||
      qiv_batch_test_fault(0, 0, 1, 56, SQLITE_IOERR_FSYNC) != SQLITE_OK)
    return SQLITE_ERROR;

  qiv_batch_test_pages(pages, data, 2);
  qiv_batch_test_init(&file, &writer, &wal);
  writer.szPage = 8192;
  if (qiv_batch_test_reject(&file, &writer, pages) != SQLITE_OK)
    return SQLITE_ERROR;
  writer.szPage = 0;
  if (qiv_batch_test_reject(&file, &writer, pages) != SQLITE_OK)
    return SQLITE_ERROR;
  writer.szPage = -1;
  if (qiv_batch_test_reject(&file, &writer, pages) != SQLITE_OK)
    return SQLITE_ERROR;
  writer.szPage = 0x7fffffff;
  if (qiv_batch_test_reject(&file, &writer, pages) != SQLITE_OK)
    return SQLITE_ERROR;
  writer.szPage = QIV_MAX_PACKED_WAL_PAGE;
  if (qiv_batch_test_reject(&file, &writer, 0) != SQLITE_OK)
    return SQLITE_ERROR;
  return SQLITE_OK;
}
