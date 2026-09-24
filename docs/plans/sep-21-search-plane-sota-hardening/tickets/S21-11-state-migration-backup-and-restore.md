# S21-11 — Current-format state backup, restore, and rollback

Status: owner code exists; current-source owner receipt and Linux release
qualification must be checked separately.

## Current contract

The prerelease breaking cutover retired the legacy `migrate-state` importer.
`crates/quanta-index-searchd/src/cli/command.rs` accepts
`backup-state`, `restore-state`, and `verify-state`. Unknown
`migrate-state` is refused. `crates/quanta-index-searchd/src/app/state_format.rs`
refuses legacy roots at boot. The offline implementation is in
`crates/quanta-index-searchd-runtime/src/state_migration.rs`.

A historical materialized RepoMap snapshot does not contain the original
source graph, exactness, and authority commitment needed to construct a
current `RepoMapSourceBundle`. Copying old `activations/` and
`snapshots/` into a new root would falsely claim an equivalent active
candidate. No snapshot-to-source-IR conversion is part of this contract.
A target with retained legacy data needs an explicit producer rebuild and
data-retention decision before cutover.

## Supported workflow

1. Stop the daemon and take the exclusive state-root lease.
2. Back up a current-format root with the SQLite backup API and an inventory
   of immutable objects; do not raw-copy a live database/WAL pair.
3. Verify the backup manifest, restore to a separate destination, and verify
   the restored root before publication.
4. Reopen with the attested release binary. Refuse incomplete staging,
   wrong manifest/version, old-root/new-binary, and old-binary/new-root pairs.
5. After any new-format mutation, use restore-forward. Do not claim a
   backward-compatible rollback path.

The [operator runbook](../../../operator/state-cutover-runbook.md) owns
command syntax and operational order.

## Evidence required

- Disposable-root owner tests for backup/restore/verify, interruption,
  corruption, lease/path ownership, typed legacy refusal, and unchanged
  source on failure.
- Exact source, commands, counts, raw evidence, and artifact digests in the
  `p10-state-migration-owner` manifest.
- A separate `p10-state-migration` Linux production-like release receipt
  bound to the release daemon. The registry currently marks this release
  node staged; an owner pass does not promote it.
- Inventory of every real target root's format and retained-data obligation
  before operational cutover. A policy decision or disposable test is not a
  migration or deployment receipt.

Historical importer designs and their dated RCA remain in Git history.
They are not implementation instructions.
