# Upstream SQLite WAL oracle

This nested Cargo workspace independently exercises registry `rusqlite`
0.41.0 and `libsqlite3-sys` 0.39.1 with the bundled SQLite VFS.

Run from the repository root:

```sh
just rust-test-sqlite-upstream-oracle
./scripts/cargow test --manifest-path tools/sqlite-upstream-oracle/Cargo.toml --locked --offline
./scripts/cargow tree --manifest-path tools/sqlite-upstream-oracle/Cargo.toml --locked --offline -i libsqlite3-sys@0.39.1
```

CircleCI's active `verify-rust-tests` job fetches this nested workspace's lock
dependencies, checks that both SQLite crates resolve from the registry with
`tools/ci/verify_sqlite_upstream_oracle.py`, and runs the Just recipe.
`just fmt-check` checks its source too.

The WAL test covers 1 KiB, 4 KiB, and 8 KiB pages; WAL snapshot isolation,
committed rows, refusal to backfill while an old reader is pinned, frame
completion, checkpoint page images, truncate, reopen, and `integrity_check`. It does not
measure syscall count or latency, or inject storage errors. The release
performance and physical quota tests cover these separately.

The Unix syscall integration test runs the product-owned
`quanta-index-sqlite-unix-syscall` callback adapter against registry SQLite.
It opens admitted files relative to a held root directory FD, rejects an
outside-root open and a final symlink, witnesses DB/WAL/SHM FDs in the
product-owned ledger, and checks delayed physical close and one-shot
retirement receipts. The callback table is process-global and remains in the
dedicated one-test process until exit. Root tokens and charges are test-only;
Runtime admission, SHM growth, and process isolation remain separate gates.
