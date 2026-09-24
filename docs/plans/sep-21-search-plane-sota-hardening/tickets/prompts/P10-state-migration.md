# P10 — Current-format state custody

Use current source, [S21-11](../S21-11-state-migration-backup-and-restore.md),
and the [operator runbook](../../../../operator/state-cutover-runbook.md).
The former legacy importer prompt is in Git history. Do not implement
`migrate-state` or synthesize a source graph from a materialized snapshot.

Before editing or issuing proof, freeze HEAD, branch/upstream, dirty state,
the exact P09 prerequisite, CLI surface, state-root format, and registered
owner/release targets.

- Owner work: current-format `backup-state`, `restore-state`,
  `verify-state`, typed old-root refusal, interrupted operation cleanup,
  immutable-object inventory, manifest-last publication, and restore-forward
  behavior on disposable roots.
- Safety checks: source unchanged on refusal, no destination publication
  before complete verification, no raw SQLite DB/WAL copy, and no path,
  symlink, lease, or ownership bypass.
- Real-target work: inventory every target root and retained-data obligation.
  A legacy target requiring retained data is blocked until an explicit
  producer rebuild and data decision.
- Proof: run the `p10-state-migration-owner` registered command and issue its
  exact-source receipt only from raw successful execution. The distinct
  `p10-state-migration` release node remains staged until its Linux
  production-like process and binary authority is implemented and run.
  An owner receipt does not close the release node.

Report exact source, commands, selected/executed/failed/ignored counts,
raw evidence paths and digests, target-root scope, exclusions, and remaining
`BLOCKED` or `NOT_RUN` items. Never infer an operational rollback drill from
a disposable backup/restore test.
