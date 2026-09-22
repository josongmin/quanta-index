# State Cutover Runbook (S21-11 / S21-12)

Offline state migration, backup/restore, and the RepoMap V1→V2 producer
cutover. Every command below runs with the daemon fully stopped.

## Offline state workflow (S21-11)

Owner: `crates/quanta-index-searchd/src/cli/command.rs`
(`migrate-state`, `backup-state`, `restore-state`, `verify-state`).

1. Stop the daemon. Acquire the exclusive `StateRootLease` (shared by the
   daemon and the offline CLI, `searchd/src/app/runtime.rs`); the lease
   validates uid/mode/regular-file/`nlink == 1`.
2. `verify-state --source <root>` — deep open/scrub before any mutation.
3. `migrate-state --source <old> --destination <new>` — the ONLY surface
   that links a legacy parser (`LegacyStateImporterV1`). The source root
   stays read-only; SQLite travels via the backup API, never raw file copy.
4. `verify-state --source <new>` on the staging root.
5. Same-filesystem atomic cutover of the configured root, then re-open
   with the release binary. A partial staging root refuses production open
   typed; the root manifest is written+fsynced last.
6. `backup-state` / `restore-state` move one freeze boundary:
   catalog+object+root manifests with every file/object digest and size.
   Restored identities, receipts, replay floor, and sequence high-water
   must exact-match the manifest.

Boot never migrates: a legacy root is refused typed
(`state_format::refuse_legacy_state_root_v1`), and a manifest whose
`format-version` differs from the build's is refused typed — old
binary/new root and new binary/old root are both explicit refusals, with
zero mutation.

## Rollback boundary

- Before the first new-format mutation commits, rollback is the old
  binary/root pair only.
- After any new-format mutation, old binary/root rollback is forbidden;
  restore-forward from a backup is the only path back.
- There is no post-cutover backward write compatibility.

## RepoMap V1→V2 producer cutover (S21-12)

- V1 (`publish`) is compatibility-only: identity-centered ACK, no payload
  manifest proof. New producers must use `publish_v2` / `activate_v2`.
- V2 publish carries the exact full source-bundle digest; the daemon
  recomputes it before mutation and seals manifest digest + source-bundle
  digest into durable `projection_meta`.
- V2 activation compares repo/revision/generation, manifest digest,
  snapshot id, projection version, authority digest, and source-bundle
  digest against the sealed candidate. A candidate that predates strong
  custody (V1 row) is refused typed; it is never relabeled in place —
  a V2 publish over differing custody fails with
  `CandidateCommitmentConflict` and leaves the durable row unchanged.
- ACK loss replays the identical terminal receipt; only
  `mutation.replayed` flips to `true`. No rebuild, re-embed, or
  reactivation work repeats.
- Cutover order: freeze the receipt schema + wire inventory + semantic
  validator in one coherent commit → producer pins that exact
  contract/SDK tree → qualify the exact source pair with the built
  daemon binary (see `just rust-verify-hellgate-cross-repo`) → deploy →
  activate → rollback drill. Each stage issues its own receipt; a pass
  never implies the next stage. Deployment is frozen while a cutover
  qualification is open.

## Evidence pointers

- Offline workflow: `state_migration_owner_v1` (28 tests).
- V2 matrix: `candidate_activation_owner_v1` (V2 bind/replay, V1
  non-upgrade, custody corruption, axis substitution, ACK bindings).
- Cross-repo qualification receipt: P11 family in
  `tools/ci/proof-authority.toml` (requires a frozen producer pair and
  the attested daemon binary; see the S21-12 ticket).
