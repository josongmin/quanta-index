# Copy/paste prompt — P10 State Migration, Backup and Restore

당신은 S21-11 owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. immediate P09 handoff/result SHA가 start
HEAD와 exact match하고 current tracked source의 state-root/catalog/sequence/lease contract digest를 freeze한 뒤 시작한다.
closed S21-01/S21-02 new state-root format, catalog authority, state-root lease가 없으면
`BLOCKED`다.

읽을 문서: repo instructions, `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`,
`docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-11-state-migration-backup-and-restore.md`,
predecessor handoff와 state-root ADR.

목표: live compatibility fallback 없이 whole state root를 offline migration/backup/restore/verify/cutover하는 executable
workflow를 제공한다.

owner files:

- `crates/quanta-index-searchd/src/cli/command.rs`
- runtime `StateRootLease`
- `crates/quanta-index-searchd/src/app/{legacy_semantic_migration,semantic_boot}.rs`
- `crates/quanta-index-catalog/src/{connection,open,idempotency,auxiliary}.rs`
- RepoMap/lexical/semantic persistence and importer modules
- wire/persisted inventory and operator docs
- disposable migration/backup/restore fault tests and suite modules, `tools/ci/{test-authority,proof-authority}.toml`, Just recipe

위 symbolic owner group은 wildcard write 권한이 아니다. preflight에서 실제 수정할 exact file/symbol manifest를
handoff에 고정하고 추가 owner가 발견되면 패치 전에 `BLOCKED`다.

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
winner selection, verification 전 cutover, migration 편의를 위한 P02B/P03 runtime authority 의미 변경.

DoD fixtures:

- clean legacy, tuple collision, missing snapshot/stale activation, wrong valid activation body
- sequence next below max, in-progress/uncertain operation, corrupt/truncated object, incomplete WAL
- interrupted migration/restore, large multi-repo retained generations
- old binary/new root 및 new binary/old root typed refusal
- source inode/mtime/content unchanged
- restored active identities/object inventory/terminal receipts/replay floor/high-water exact manifest match
- production boot legacy readers/migrators count 0
- interrupted migration/restore는 incomplete staging을 authority로 만들지 않고 idempotent resume/retry 또는 typed
  cleanup으로 수렴
- non-empty destination, symlink/path traversal, wrong owner/mode, disk-full, fsync/rename crash를 fail-closed
- broad/root/home/unresolved target과 source=destination destructive operation을 mutation 전에 거부

owner node expected tuple은 `id=p10-state-migration-owner`, `family=X`, `required_host=any`,
`dependencies=[p09-control-readiness-owner]`이며 disposable state roots만 사용한다. release node는
`id=p10-state-migration`, `family=X`, `required_host=linux-production-like`,
`dependencies=[p10-state-migration-owner,p09-control-readiness]`다. transitive
identity/activation/journal/supervisor contracts는 current source digest로 소비한다. `test_authority_targets`가 비어 있으면
production implementation 전에 same lane에서 exact fixture/suite/target/recipe/selector를 bootstrap한다. disposable
migration/backup/restore/rollback/corruption scenarios를 dry-run에서 실제 선택하지 못할 때 `BLOCKED`다. canonical release command는
`release-daemon-fresh`의 exact path/hash로 reopen/process proof를 실행하는 dedicated recipe여야 한다. registry의
`just rust-profile test-daemon-all`이 Cargo test binary만 실행하면 recipe/registry를 먼저 고친다. Linux
production-like/release-daemon proof가 필요하다. 모든 drill은 disposable state root에서 수행한다. 실제/공유 state
root cutover는 별도 명시 승인 없이는 NOT_RUN이다. 최종 보고에 source freeze, CLI surface, root/backup manifest
schema, rollback cutoff, fixture results/counts, NOT_RUN, P11 migration receipt와 `artifacts/sep-21/handoffs/P10.json`을
남겨라. explicit owner path만 checkpoint commit하고 current lane branch에 non-force push한다.
