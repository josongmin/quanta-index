/* Quanta-authored, attempt-local checkpoint main-file writes. The caller
 * selects only safe consecutive database pages from SQLite's WAL iterator. */
#define QIV_CKPT_MAX_PAGE 4096
#define QIV_CKPT_MAX_PAGES 8

typedef struct qiv_checkpoint_batch {
  u8 bytes[QIV_CKPT_MAX_PAGES * QIV_CKPT_MAX_PAGE];
  int nPage;
  int szPage;
  i64 offset;
} qiv_checkpoint_batch;

static void qiv_checkpoint_batch_init(qiv_checkpoint_batch *state) {
  state->nPage = 0;
  state->szPage = 0;
  state->offset = 0;
}

static int qiv_checkpoint_batch_valid(const qiv_checkpoint_batch *state) {
  i64 width;
  if (state == 0 || state->nPage < 0 || state->nPage > QIV_CKPT_MAX_PAGES)
    return 0;
  if (state->nPage == 0) return 1;
  if (state->szPage <= 0 || state->szPage > QIV_CKPT_MAX_PAGE ||
      state->offset < 0 || state->offset % state->szPage != 0)
    return 0;
  width = (i64)state->nPage * state->szPage;
  return state->offset <= LARGEST_INT64 - width;
}

static int qiv_checkpoint_batch_flush(
    qiv_checkpoint_batch *state, sqlite3_file *file) {
  int rc;
  int amount;
  if (file == 0 || !qiv_checkpoint_batch_valid(state))
    return SQLITE_IOERR_WRITE;
  if (state->nPage == 0) return SQLITE_OK;
  amount = state->nPage * state->szPage;
  /* A failed or short physical write is terminal to this checkpoint. Keep
   * the buffer intact; only the caller may discard it after propagating rc. */
  rc = sqlite3OsWrite(file, state->bytes, amount, state->offset);
  if (rc == SQLITE_OK) qiv_checkpoint_batch_init(state);
  return rc;
}

static int qiv_checkpoint_batch_append(
    qiv_checkpoint_batch *state, sqlite3_file *file,
    const u8 *page, int szPage, i64 offset) {
  i64 expected;
  int rc;
  if (file == 0 || page == 0 || !qiv_checkpoint_batch_valid(state) ||
      szPage <= 0 || szPage > QIV_CKPT_MAX_PAGE || offset < 0 ||
      offset % szPage != 0 || offset > LARGEST_INT64 - szPage)
    return SQLITE_IOERR_WRITE;
  if (state->nPage != 0) {
    if (state->szPage != szPage) return SQLITE_IOERR_WRITE;
    expected = state->offset + (i64)state->nPage * state->szPage;
    if (state->nPage == QIV_CKPT_MAX_PAGES || offset != expected) {
      rc = qiv_checkpoint_batch_flush(state, file);
      if (rc != SQLITE_OK) return rc;
    }
  }
  if (state->nPage == 0) {
    state->szPage = szPage;
    state->offset = offset;
  }
  memcpy(state->bytes + state->nPage * szPage, page, szPage);
  state->nPage++;
  if (state->nPage == QIV_CKPT_MAX_PAGES)
    return qiv_checkpoint_batch_flush(state, file);
  return SQLITE_OK;
}

#ifdef QUANTA_CAPACITY_VFS_TEST_HOOKS
#include "quanta_checkpoint_batch_tests.inc"
#endif
