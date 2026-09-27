# S21-11 — Current-format state qualification

Status: `ACTIVE — owner, Linux release and authorized-target proof remain separate`.
Completed CLI/original-manifest/copy-custody decisions are in
[SEP-27-005](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md).
[Residual R4](CURRENT-RESIDUAL-2026-09-26.md) owns status;
[the operator runbook](../../../operator/state-cutover-runbook.md) owns commands.

## Remaining acceptance

- Execute disposable-root backup/restore/verify with interruption, corruption,
  exclusive lease/path ownership, original manifest replacement/removal,
  catalog row/digest mismatch, staging cleanup and unchanged source on failure.
- Issue the registered `p10-state-migration-owner` proof with exact relevant
  source, commands, terminal outcomes and required artifacts. An old focused
  pass or a broad profile name does not establish this receipt.
- Execute separate `p10-state-migration` Linux production-like release proof
  using the attested daemon and unchanged required process targets.
- Inventory every authorized real target root's exact schema/format and
  retained-data obligations. Obtain rebuild/retention/rollback authority before
  target cutover; a disposable fixture is not that operational action.
- Prove stopped daemon plus exclusive lease, current-format SQLite backup,
  manifest/inventory verification, separate-destination restore and incarnation
  rotation, attested reopen and restore-forward after mutation. Refuse old-root/
  new-binary, old-binary/new-root and partial staging without mutation.

## Scope

Owners: `quanta-index-searchd` CLI/state-format/offline adapters,
`quanta-index-searchd-runtime/src/state_migration.rs`, the registered
`state_migration_owner_v1` tests and the operator runbook.

Legacy import is retired. Historical materialized RepoMap state cannot recreate
source authority; retained legacy roots need producer rebuild and a data policy.
No backup, deployment, activation or rollback is inferred from implementation.
Historical checkpoints remain in Git, not live qualification.
