# State Cutover Runbook (S21-11 / S21-12)

Offline state backup/restore and the RepoMap single-mutation
producer cutover. Every command below runs with the daemon fully stopped.

## Offline state workflow (S21-11)

CLI owner: `crates/quanta-index-searchd/src/cli/command.rs`.
Execution owner: `crates/quanta-index-searchd-runtime/src/state_migration.rs`
(`backup-state`, `restore-state`, `verify-state`).

1. Stop the daemon. Acquire the exclusive `StateRootLease` (shared by the
   daemon and the offline CLI, `searchd/src/app/runtime.rs`); the lease
   validates uid/mode/regular-file/`nlink == 1`.
2. `backup-state --source <current> --destination <backup>` freezes a
   current-format root. SQLite travels via the backup API, never raw file copy.
   A daemon-created root may lack a root manifest; backup validates and
   writes the backup manifest.
3. `verify-state --source <backup>` checks the produced backup manifest.
4. `restore-state --source <backup> --destination <new>` and
   `verify-state --source <new>` validate the restored root. Do not use
   `verify-state` as a pre-backup check on a manifest-less daemon root.
5. Same-filesystem atomic cutover of the configured root, then re-open
   with the release binary. Never configure the daemon against an unpublished
   staging directory: `verify-state` refuses a manifest-less staging root,
   and the root manifest is written+fsynced last before publication.
6. `backup-state` moves one freeze boundary: catalog+object+root manifests
   with every file/object digest and size. `restore-state` preserves the
   backed-up data identities, receipts, replay floor, and sequence high-water,
   but rotates the activation-catalog root incarnation in its unpublished
   staging root. The restored root manifest attests that new incarnation;
   it must not be byte-for-byte identical to the backup manifest. A token
   issued for the source root is not authority for the restored root.

Boot never migrates: a legacy root is refused typed
(`state_format::refuse_legacy_state_root_v1`), and a manifest whose
`format-version` differs from the build's is refused typed — old
binary/new root and new binary/old root are both explicit refusals, with
zero mutation.

There is no offline legacy importer. `migrate-state` is rejected at the CLI;
legacy semantic journals and pre-catalog auxiliary snapshots are markers for
typed refusal only. Rebuild from current typed producer input. Backup and
restore never convert an old format into the current one.

## Rollback boundary

- Before the first new-format mutation commits, rollback is the old
  binary/root pair only.
- After any new-format mutation, old binary/root rollback is forbidden;
  restore-forward from a backup is the only path back.
- There is no post-cutover backward write compatibility.

## RepoMap single-mutation producer contract (S21-12)

- The SDK exposes only `repomap().publish(&RepoMapPublishBundleRequestV2)` and
  `repomap().activate(RepoMapActivateGenerationRequestV2)`. The old V1 IPC
  opcodes, SDK methods, core mutation ports, and direct-store V1 methods are
  removed. `RepoMapActivateGenerationRequestV2` carries its identity fields
  directly; a nested `request_v1` map is refused.
- V2 publish carries the exact full source-bundle digest; the daemon
  recomputes it before mutation and seals manifest digest + source-bundle
  digest into durable `projection_meta`.
- Activation compares repo/revision/generation, manifest digest,
  snapshot id, projection version, authority digest, and source-bundle
  digest against the sealed candidate. Projection metadata without both
  custody digests is rejected at decode/open; it is never relabeled in place.
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

- Offline workflow: `state_migration_owner_v1` (current-format
  backup/restore/verify and legacy refusal tests).
- Mutation matrix: `candidate_activation_owner_v1` (bind/replay, legacy
  metadata refusal, custody corruption, axis substitution, ACK bindings).
- Cross-repo qualification receipt: P11 family in
  `tools/ci/proof-authority.toml` (requires a frozen producer pair and
  the attested daemon binary; see the S21-12 ticket).

## Cutover scope still open

The RepoMap V1 success route removal is a local dirty-tree candidate, not a
cross-repo cutover receipt. The Semantica producer must build against the flat
V2 activation request before qualification. Separately, the intended semantic
cutover removes `QUANTA_INDEX_SEMANTIC_DERIVE_MODE` and all live chunk-text
derivation fallback. It requires typed semantic-source producer fixtures and
an exact-source search/ingest proof; a chunk-only publish now yields no dense
vectors, not a hidden chunk-text fallback. The previous Semantica handoff
could build a lexical batch with empty semantic replacement scopes when
`shadow_semantic_publish` was false; the single-path producer change removes
that switch. A lexical delta can still intentionally omit unchanged semantic
scopes. The wire currently has no independent semantic coverage witness, so
an empty vector alone cannot distinguish unchanged from an erroneous omission;
producer tests and paired qualification must establish that invariant. The
typed `RawCodeFallback` budget and card production remain required. The
semantic manifest format is now 11 and its pre-seal build-contract format is
3; prior-format generations are refused and must be rebuilt from typed
producer input, not resumed or relabeled. This version bump is local source
behavior, not a completed candidate-root or cross-repo qualification. The
retired `migrate-state` importer is unavailable. Old semantic generations
are typed-refusal/rebuild candidates, with no in-place migration. The live
semantic runtime cutover is still in progress; these docs do not prove that
a running binary or the Semantica producer has completed it.
