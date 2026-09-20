# Copy/paste prompt — P10 State Migration, Backup and Restore

당신은 S21-11 owner다. S21-01/02/04/09가 DONE이고 new state-root format, catalog authority, state-root lease가
current source에 존재할 때만 시작한다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-11-state-migration-backup-and-restore.md`,
predecessor handoff와 state-root ADR.

목표: live compatibility fallback 없이 whole state root를 offline migration/backup/restore/verify/cutover하는 executable
workflow를 제공한다.

owner files:

- `crates/quanta-index-searchd/src/app/command.rs`
- runtime `StateRootLease`
- `crates/quanta-index-searchd/src/app/{legacy_semantic_migration,semantic_boot}.rs`
- `crates/quanta-index-catalog/src/{connection,open,idempotency,auxiliary}.rs`
- RepoMap/lexical/semantic persistence and importer modules
- wire/persisted inventory and operator docs

workflow:

1. daemon fully stopped; exclusive lease acquire; uid/mode/regular-file/`nlink == 1` verify.
2. source root manifest/version verify; source root read-only.
3. SQLite backup API snapshot. DB/WAL raw copy 금지.
4. canonical identity/size/digest object inventory and staging restore.
5. staging schema migration, sequence high-water reconciliation, stale activation invalidation, deep open/scrub.
6. root manifest last write+fsync and parent directory fsync.
7. same-filesystem atomic cutover and release binary re-open.
8. first new-format mutation 이후 old binary/root rollback 금지; restore-forward only.

필수 CLI: offline `migrate-state`, `backup-state`, `restore-state`, `verify-state`. daemon과 같은 lease/check owner를
재사용하고 product boot path에 legacy decoder/migrator를 남기지 않는다.

금지: live source mutation, runtime dual reader/writer, raw DB/WAL copy, incomplete staging ready, collision/ambiguity
winner selection, verification 전 cutover.

DoD fixtures:

- clean legacy, tuple collision, missing snapshot/stale activation, wrong valid activation body
- sequence next below max, in-progress/uncertain operation, corrupt/truncated object, incomplete WAL
- interrupted migration/restore, large multi-repo retained generations
- old binary/new root 및 new binary/old root typed refusal
- source inode/mtime/content unchanged
- restored active identities/object inventory/terminal receipts/replay floor/high-water exact manifest match
- production boot legacy readers/migrators count 0

최종 보고에 source freeze, CLI surface, root/backup manifest schema, rollback cutoff, fixture results/counts, NOT_RUN,
P11이 사용할 migration receipt를 남겨라. commit/push는 요청 시에만 한다.
