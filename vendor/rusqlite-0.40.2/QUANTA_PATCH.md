# Quanta local patch

This directory copies the pinned crates.io `rusqlite` 0.40.2 source. Registry-only metadata was omitted. The only source change is `Connection::pager_cache_status` and its `PagerCacheStatus` result type in `src/pager_cache_status.rs`, with the module/export declarations in `src/lib.rs`.

The method reads SQLite's 64-bit `SQLITE_DBSTATUS_CACHE_HIT`, `CACHE_MISS`, `CACHE_WRITE`, and `CACHE_SPILL` current values with `resetFlg=0` while borrowing the connection handle. It returns SQLite errors and rejects negative counters. The API is compiled only for bundled SQLite without either `sqlcipher` or `bundled-sqlcipher`, and excludes loadable-extension and wasm builds. This matches the pinned bundled binding that declares `sqlite3_db_status64`; the `sqlcipher` feature can override `bundled` to link a different library. It does not change SQLite settings, query behavior, or the bundled SQLite version.

The file-backed test uses a small cache and one large write transaction to observe a real spill, then reads the table and checks that a second status snapshot does not reset the counters. SQLite specifies that `CACHE_WRITE` after a page-write I/O error is undefined; consumers must not use this counter to qualify an error-path result.
