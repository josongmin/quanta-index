# S21-11 — State Migration, Backup, Restore, and Rollback

Status: `superseded for legacy migration` (2026-09-24 prerelease breaking cutover)

The legacy `migrate-state` importer and its successful old-root conversion
contract are retired. This ticket's importer design and migration acceptance
criteria below are historical, not current commands. Current-format
`backup-state`, `restore-state`, `verify-state`, and typed old-root refusal
remain; use [the operator runbook](../../../operator/state-cutover-runbook.md)
for the supported workflow.

## 2026-09-23 source-bound RCA and execution boundary

- W10-R3's read-only semantic importer and source-custody changes were merged
  locally at `22e9ba1`. Its original V1 RepoMap importer only copied
  `activations/` and `snapshots/` into non-serving `legacy-import/` while
  returning success. The current-format store then opens with no equivalent
  RepoMap candidate or active head. That is a false-success cutover.
- Historical `RepoMapSnapshot` is a materialized entry view. The current
  `RepoMapSourceBundle` requires graph nodes, edges, exactness and an authority
  commitment that the snapshot cannot reconstruct. Do not synthesize a graph
  from materialized rows or promote copied bytes to current authority.
- Immediate safety correction: `crates/quanta-index-searchd-runtime/src/state_migration.rs`
  refuses a nonempty V1 RepoMap layout typed with producer-replay guidance;
  `tests/state_migration_owner_v1.rs` proves no destination publish and an
  unchanged source. Empty V1 layout markers and valid semantic journal may
  migrate. This does not close the full ticket.
- W10-R3's read-only journal decoder returns the distinct public
  `LegacySemanticJournalCorrupt` error for malformed CBOR. The owner fixture
  checks corrupt and truncated journals against that exact code. Its two
  public-API baseline rows are one symbol exposed through the `ipc` path and
  the crate re-export. R5 must review this P10-owned delta explicitly;
  collapsing it into an unrelated error solely to satisfy R5's
  correlation-only allowlist would lose the typed corruption contract.
- Remaining owner action: define a producer-replay input containing the
  original source bundle and checked identity for every legacy active key.
  Import through the current RepoMap publish/activate authority, then compare
  active pin, source commitment, terminal receipt, replay floor and sequence
  high-water in the staged catalog before manifest publication. Missing
  bundle or any ambiguity must remain a typed refusal.
- `crates/quanta-index-searchd/src/app/runtime.rs` still calls
  `migrate_legacy_auxiliary_snapshots` during boot on a current root. Move
  that conversion to an offline, source-frozen operation only after defining
  how pre-catalog current roots are detected and refused at boot. Removing
  the call alone would silently drop history/runtime/structural authority.
- DoD: no live legacy migration; materialized-only V1 RepoMap cannot produce
  a green root; a complete replay fixture restores active identities and
  high-water exactly; boot rejects a root requiring auxiliary conversion;
  backup/restore and release-process proof run against the final same-source
  binary. Until then P10 remains `OPEN`, not `QUALIFIED`.

Depends on: S21-01, S21-02, S21-04, S21-09, S21-10

## Goal

layout/state-machine breaking change를 live fallback 없이 안전하게 전환하고, 전체 state root를 일관된
경계에서 backup/restore/verify할 executable workflow를 제공한다.

## Root cause

- persisted state별 importer/backup/restore authority가 없음
- filename collision, stale activation, sequence regression이 legacy state에 존재할 수 있음
- WAL/catalog/native artifacts를 따로 복사하면 일관된 snapshot이 아님
- runtime dual-read를 도입하면 old/new authority가 장기간 공존하게 됨

## Cutover policy

- daemon offline 또는 supervisor quiesced/frozen export에서만 migration
- old root는 read-only input
- new root는 separate path에 완전 생성
- importer는 legacy ambiguity/collision/corruption을 fail closed
- verification 성공 후 atomic configured-root cutover
- rollback은 new-format mutation 전까지 old binary/root pair로만 허용
- post-cutover backward write compatibility 없음

## Backup set

- operation/catalog DB and WAL/checkpoint metadata
- immutable lexical/semantic/RepoMap artifacts
- candidate/activation/tombstone/quarantine records
- auxiliary epochs and sequence/replay floors
- schema/profile/provider configuration digests
- manifest enumerating every file/object digest and size

## Work items

1. frozen export API/CLI and state-root lease protocol
2. legacy layout importer with collision detection
3. catalog schema migration and sequence reconciliation
4. stale activation invalidation during import
5. backup manifest, checksum verification, restore-to-new-root
6. restored root deep open/scrub before activation
7. rollback boundary and operator runbook
8. migration receipts with source/target format and binary hash
9. partial/interrupted migration cleanup and resume policy

## Frozen fixtures

- clean legacy root
- filename collision pair
- snapshot missing with stale activation
- wrong valid-JSON activation generation
- sequence next below stored max
- in-progress/uncertain operation
- corrupt/truncated file and incomplete WAL/checkpoint
- large multi-repo root with shared/retained generations

## Owner files

- offline-only migration/backup CLI owner under searchd CLI command surface
- catalog and RepoMap persistence adapters
- lexical/semantic persisted adapters
- runtime lease/supervisor integration
- `tools/ci/inventory/wire-surface.toml`
- deployment/operator docs

## Acceptance

- importer never mutates source root
- collision/ambiguity cannot silently select one record
- restored query/receipt/activation/high-water identity matches manifest
- publish-only repair does not reactivate invalidated state
- sequence allocator is greater than every restored terminal sequence
- partial restore cannot be opened as production-ready
- runtime contains no legacy live reader/writer after cutover

## Verification

- frozen migration fixture target registered in test authority
- backup during quiesced state and restore to isolated root
- process restart and SDK query comparison
- interrupted copy/import failpoints
- old binary/new root and new binary/old root explicit refusal tests
- exact file/object inventory and digest receipt

## No patch-on-patch rule

open path에 legacy decoder fallback을 넣지 않는다. offline importer가 새 root를 완성하고 검증한 뒤
단일 cutover한다.

## Final executable workflow

1. daemon이 완전히 정지한 상태에서 exclusive `StateRootLease`를 획득하고 uid/mode/regular-file/`nlink == 1`을 검증한다.
2. source root manifest/version을 읽고 source를 read-only로 고정한다.
3. SQLite는 backup API로 snapshot한다. live DB/WAL 파일의 raw copy를 허용하지 않는다.
4. immutable object inventory를 canonical identity, size, digest로 생성하고 staging root로 복원한다.
5. staging에서 schema migration, sequence reconciliation, stale activation invalidation, deep open/scrub을 수행한다.
6. 모든 proof가 성공한 뒤 root manifest를 마지막으로 write+fsync하고 parent directory까지 fsync한다.
7. same-filesystem atomic cutover 후 release binary로 재-open한다. partial staging root는 production open을 거부한다.
8. new-format mutation이 한 번이라도 commit되면 old binary/root rollback을 금지하고 restore-forward만 허용한다.

### File-level action list

- `crates/quanta-index-searchd/src/cli/command.rs`: offline `migrate-state`, `backup-state`, `restore-state`, `verify-state` 명령의 유일 UX owner.
- `crates/quanta-index-searchd/src/app/runtime.rs`의 `StateRootLease`: daemon과 offline CLI가 같은 lease/check를 공유.
- `crates/quanta-index-searchd/src/app/legacy_semantic_migration.rs` 및 `semantic_boot.rs`: 최종 cutover에서 boot-time live legacy migration 제거.
- `crates/quanta-index-catalog/src/{connection,open,idempotency,auxiliary}.rs`: backup API, schema migration,
  high-water reconciliation, row digest 및 `fullfsync` 설정 read-back.
- RepoMap/lexical/semantic persistence adapters: legacy parser는 importer 모듈에서만 link되고 runtime open path에는 없음.
- `tools/ci/inventory/wire-surface.toml`: root manifest, backup manifest, migration receipt, persisted receipt version 등록.

### DoD additions

- source root의 inode/mtime/content가 migration 전후 동일하고 staging failure가 source/cutover target을 변경하지 않는다.
- old binary/new root, new binary/old root, incomplete staging, wrong manifest digest가 각각 stable typed refusal이다.
- restored active identities, object inventory, terminal receipts, replay floor, sequence high-water가 manifest와 exact match한다.
- backup/restore proof는 catalog/object/root manifest의 하나의 freeze boundary를 증명한다.
