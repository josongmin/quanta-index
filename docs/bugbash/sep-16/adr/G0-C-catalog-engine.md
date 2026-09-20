# G0-C — catalog storage engine

**Status: PASS — `SQLite` (via `rusqlite`, bundled) is admitted for the W2
catalog, under four conditions the evidence imposes. `redb` is rejected on
evidence, not on preference.**

- Decided: 2026-09-16
- Gate owner: W0, blocking W2 (catalog/lifecycle), C1, and QI-BB-020/026/029/032
- Probes: [`crates/quanta-index-catalog-probe/tests/g0c_sqlite_catalog_probe.rs`](https://github.com/josongmin/quanta-index/blob/7bcea4555ac6e4cb5c784b7e32110eb273a1e3cb/crates/quanta-index-catalog-probe/tests/g0c_sqlite_catalog_probe.rs), [`crates/quanta-index-catalog-probe/tests/g0c_redb_alternative_probe.rs`](https://github.com/josongmin/quanta-index/blob/7bcea4555ac6e4cb5c784b7e32110eb273a1e3cb/crates/quanta-index-catalog-probe/tests/g0c_redb_alternative_probe.rs)
- Command: `just rust-w0-storage-gates`, or `./scripts/cargow --lane test-integration-lane test --all-features --locked -p quanta-index-catalog-probe -- --nocapture --test-threads=1`
- Engines: `rusqlite 0.40.2` / `libsqlite3-sys 0.38.2` (bundled, links **SQLite 3.53.2**, features `bundled`, `backup`); `redb 4.3.0`
- Host: macOS 24.6.0, 16 cores, **loadavg 11–16 during every measurement** (other agents' builds running). Latency figures are shapes, not budgets.

## What the gate asked

W2 wants one durable catalog: operation/session journal with replay lookup by
`(session, sequence)`, durable receipts, snapshot/artifact/dependency/
activation/health rows, per-key auxiliary epochs, an invisible staging epoch
and a short publish transaction, a `Live -> Deleting` state machine, and a
`scope gate -> catalog transaction` lock order. The plan required real-workload
evidence for commit latency, WAL behavior, busy/deadline semantics and
crash/export/restore, plus (M3) the vendor's build cost, MSRV fit, supply-chain
rails, and a named alternative with its own evidence.

## Evidence

Raw `G0C-EVIDENCE` lines, 2026-09-16 (`--test-threads=1`):

```
sqlite engine sqlite_version=3.53.2 rusqlite_version=3.53.2 journal_mode=wal synchronous=1 foreign_keys=1 busy_timeout_ms=50 page_size=4096
sqlite commit_latency per_key_commits=2048 per_key_p50_us=20 per_key_p95_us=29 per_key_p99_us=67 batched_commits=8 batched_rows_per_commit=256 sync_full_per_key_commits=256 sync_full_per_key_p50_us=37 sync_full_per_key_p95_us=58 sync_full_per_key_p99_us=89 batched_p50_us=1886 batched_p95_us=2178 fullfsync_effective=1 fullfsync_per_key_commits=256 fullfsync_per_key_p50_us=5000 fullfsync_per_key_p95_us=6229 fullfsync_per_key_p99_us=8746 final_rows=256
sqlite wal_snapshot seen_before_writes=8 writer_total_after_writes=64 seen_inside_open_snapshot=8 seen_after_snapshot_closed=64
sqlite busy contender_outcome=SqliteFailure(Error_{_code:_DatabaseBusy,_extended_code:_5_},_Some("database_is_locked")) contender_waited_ms=58 typed_busy=true acquired_after_release=true
sqlite crash_recovery child_exit=ExitStatus(unix_wait_status(6)) child_crashed=true committed_rows_expected=16 rows_after_reopen=16 wal_bytes_after_crash=12392 integrity_check=ok
sqlite export method=VACUUM_INTO backup_api_note=sqlite3_backup_step_restarts_when_another_connection_writes;_does_not_converge_under_continuous_writes export_ms=7 writer_commits_during_export=57 export_integrity=ok export_rows=20000 export_epoch_max=7 source_rows=20000
sqlite content_bit_rot integrity_check=ok occurrences_corrupted=2 written=sha256:00042:000 served=sha256:00042:fff served_matches_written=false

redb engine crate=redb version=4.3.0_(Cargo.lock;_redb_exposes_no_runtime_version) native_build=none (pure Rust)
redb commit_latency durability=Immediate_(fsync_per_commit;_redb_default) per_key_commits=2048 per_key_p50_us=5161 per_key_p95_us=6992 per_key_p99_us=9226 batched_commits=8 batched_rows_per_commit=256 batched_p50_us=7165 batched_p95_us=11986 final_rows=256
redb mvcc_snapshot seen_before_writes=8 seen_inside_open_snapshot=8 seen_after_snapshot_closed=64
redb contention timeout_api=none acquired_before_release=false parked_ms_before_release=204 acquired_ms_after_release=0
redb crash_recovery child_crashed=true committed_rows_expected=16 rows_after_reopen=16 integrity_check=Ok(true)
redb content_bit_rot written=sha256:00042:000 read_outcome=served:sha256:00042:fff integrity_check=err:DB_corrupted:_Primary_is_corrupted_despite_2-phase_commit served_altered_content=true
```

| Property | `SQLite` 3.53.2 | `redb` 4.3.0 |
| --- | --- | --- |
| Durable commit, one key (macOS `F_FULLFSYNC`) | p50 **5.0 ms**, p95 6.2 ms, p99 8.7 ms | p50 **5.2 ms**, p95 7.0 ms, p99 9.2 ms |
| Commit without drive flush | `synchronous=NORMAL` p50 20 µs; `FULL` (plain `fsync`) p50 37 µs | not probed (`Durability::Eventual` exists) |
| 256-row batched commit | 1.9 ms (NORMAL) | 7.2 ms (Immediate) |
| Reader snapshot while writer commits | WAL: 8 seen inside, 64 after | MVCC: 8 seen inside, 64 after |
| Second writer on a held lock | typed `DatabaseBusy` after the 50 ms `busy_timeout` | blocks with no timeout API; released in 0 ms after the holder aborts |
| Real crash (child `SIGABRT` mid-transaction) | 16 committed rows present, 16 uncommitted absent, WAL had 12,392 B of orphaned frames, `integrity_check=ok` | 16 present, 16 absent, `check_integrity()=Ok(true)` |
| Export while a writer keeps committing | `VACUUM INTO` copied 20,000 rows in 7 ms while 57 commits landed; export integrity ok. **The online backup API restarts whenever another connection writes and never converged** (an earlier probe revision looped forever). | no export API probed |
| Content bit-rot inside a page | **served as-is (`…:fff`) with `integrity_check=ok`** — no page checksums | served as-is on read; `check_integrity()` **does** detect it (page checksums) |
| Native build | C amalgamation via `cc`; build script 1.80 s + crate 0.30 s + `rusqlite` 0.40 s | pure Rust, ~2.0 s |
| MSRV | crates declare none; compile on 1.92.0 | `rust-version = 1.90` ≤ 1.92.0 |
| Query shape | SQL: multi-table transactions, secondary indexes, `(session, sequence)` lookups | KV tables; secondary indexes are hand-built tables |
| Supply chain | `just rust-deny` ok, `just rust-machete` ok | same |

## The measurement that mattered

The first cut of this gate showed `SQLite` committing 100× faster than `redb`.
That was macOS `fsync` semantics, not the engine: `synchronous=FULL` issues a
plain `fsync`, which macOS does not push to the drive; only `fullfsync=ON`
issues `F_FULLFSYNC`. With that set, both engines land at ~5 ms per durable
commit, bound by the drive. **The catalog's durable-commit cost on this class
of host is ~5 ms regardless of engine**, which is the number W2's publish
transaction design has to absorb (one transaction per publish, never one per
row).

## Decision

`SQLite` through `rusqlite` (bundled), with these conditions binding on W2:

1. **Every catalog row carries its own digest.** The engine serves bit-rotted
   cell content with a clean `integrity_check`. Row-level digests (operation
   body hash, artifact digest, receipt digest) are not an optimization; they
   are the only content-integrity check the catalog will have.
2. **Authority rows commit under `synchronous=FULL`, and on macOS
   `fullfsync=ON`.** `NORMAL` is forbidden for rows that decide "this was
   committed": a power loss can drop the last commits under `NORMAL` (never
   corrupt), which is a receipt saying yes for work the engine forgot.
3. **Export is `VACUUM INTO` (or a quiesced-writer file copy), never the
   online backup API under writes.** The backup API's restart-on-foreign-write
   behavior does not converge, which is why W7's frozen export design is
   correct rather than conservative.
4. **`busy_timeout` maps to the caller's deadline** and surfaces as a typed
   `DatabaseBusy`; the probe shows the engine honors it (58 ms observed for a
   50 ms budget). A catalog write that meets a held lock returns typed busy,
   not a blocked thread.

## Rejected alternative: `redb`

Not rejected on speed — durable commits are equal. Rejected because:

- **No timeout on writer contention.** `begin_write` parks indefinitely. W2's
  busy/deadline requirement would need a wrapper thread and a channel, which
  turns a deadline into an orphaned blocked thread (the G0-R shape).
- **KV shape against a relational catalog.** Replay lookup by `(session,
  sequence)`, activation by `(repo, revision)`, artifacts by digest, and
  multi-table publish transactions all need secondary indexes that `redb`
  leaves to the application. That is catalog code W2 would have to write and
  keep consistent by hand.
- **Its checksum does not remove condition 1.** `check_integrity()` detects
  bit-rot but an ordinary read still returned the altered value, so rows need
  digests either way; the checksum is not a reason to prefer it.
- **No export primitive** comparable to `VACUUM INTO`.

## Limitations

- All latencies were taken on a host running other agents' builds (loadavg
  11–16). Ratios and orders of magnitude are trustworthy; absolute p99s are not
  a budget. The durable-commit figure should be re-taken on a quiet host before
  it is written into a W2 SLO.
- The auxiliary workload is a synthetic per-key upsert. The real catalog's
  publish transaction touches several tables; its cost is bounded below by the
  ~5 ms durable commit and must be measured once W2 exists.
- Build-cost figures were taken warm for everything except the three engine
  crates. `cc` was already in the tree (via `lance`/`zstd`), so no new
  build-time dependency chain was introduced; `rkyv_derive`'s `syn 3` arrival
  is from the advisory bumps (IMPL-F), not from this gate.
- The probe crate (`quanta-index-catalog-probe`) holds both engines as
  dev-dependencies so no non-test lane links them. It is deleted when the real
  catalog adapter crate lands and takes the dependency.
