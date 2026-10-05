# SEP-21-004 — Process Supervision, State Cutover and Proof

Status: `Accepted`

Decided: 2026-09-21

Amended: 2026-09-21 — proof receipts gained immutable source-pair-aware archive identity and transitive dependency
archive edges; lane handoffs gained a semantic validator. Current aliases remain operational conveniences only.

Gate owner: S21-00; blocks S21-09, S21-10, S21-11, S21-12 and S21-13

Consolidated: 2026-09-27 — [SEP-27-005](SEP-27-005-catalog-recovery-supervision-and-proof-custody.md)
owns completed report/deadline/backend, original-backup and proof-custody repairs.
Legacy import is retired; this record does not authorize a live or offline importer.

## Supervision and shutdown

`SearchdSupervisor` owns every plane, maintenance task, connection/peer watch, provider task and runtime guard,
including the state-root lease. Owned partial destructure of `SearchdRuntime` is forbidden.

First SIGINT/SIGTERM stops accepts, cancels query/provider work and allows already accepted durable mutation to reach
a terminal state. The hard deadline is the maximum admitted uninterruptible mutation budget plus five seconds;
the initial value is 125 seconds. Outcomes:

- clean operator shutdown: exit 0;
- required child death, startup rollback or hard deadline: exit 70;
- second signal: immediate `128 + signal`.

Hard-deadline termination does not claim graceful cleanup and must not release the lease while live work continues.

## Readiness

`ProcessReadinessV1` is true only while the process owns the lease, all required query/control/ingest accept loops and
maintenance tasks are alive, mutation coordinator is healthy, required provider executor is healthy and shutdown has
not begun. A process may be ready with zero active repositories. Repository/generation status remains a distinct DTO.

## State-root format and migration

`state-root-manifest.cbor` has envelope schema 1 and `state_root_format=2`. It binds root UUID, catalog/object/receipt
versions, cursor key ID/digest, migration receipt digest and binary/contract digest. Missing, V1 or malformed root
is refused as `STATE_ROOT_FORMAT_UNSUPPORTED` before adapters open.

Current-format backup/restore is offline-only under the exclusive state-root lease:

1. preserve source root read-only;
2. use the SQLite backup API, never raw live DB/WAL copy;
3. inventory immutable objects by canonical identity, size and digest;
4. build and deep-scrub a staging root;
5. write+fsync the root manifest last and fsync its parent;
6. perform same-filesystem atomic cutover;
7. re-open with the attested release daemon.

Before the first V2 mutation, rollback may return to the untouched V1 root/binary pair. After it, rollback to V1 is
forbidden; only verified V2 backup restore or forward repair is allowed. Deployment must never hand a V2 root to an
old binary.

Legacy roots require an explicit producer rebuild and retained-data decision.
No snapshot-to-source conversion or automatic migration is implemented by this
workflow. Preserve the original backup manifest through admission and copied-byte
verification before intentional incarnation rotation.

## Cross-repository cutover

Mutation protocol version 2 plus contract digest handshake occurs before body decode. Live dual decoder is forbidden.
Order: publish Quanta contract/SDK V2, pin Semantica exact dependency, quiesce ingress, back up and rebuild/verify the admitted current-format root, start
attested V2 daemon, handshake/canary, switch producer, then activate. Mismatched producer/daemon is
`PROTOCOL_VERSION_UNSUPPORTED` with zero mutation.

## Proof authority

- registry: `tools/ci/proof-authority.toml`;
- manifest schema: `tools/ci/proof-manifest.schema.json`;
- validator: `tools/ci/lint/check-proof-authority.py`;
- proof families: `S/U/A/D/P/F/Q/X`;
- source SHA is exactly 40 lowercase hex;
- dirty digest domain-separately hashes staged index bytes, unstaged worktree bytes, scoped untracked bytes and file
  metadata; proof outputs and declared terminal artifacts are excluded;
- host identity is a SHA-256 digest over stable OS/arch/CPU/memory/runner identity inputs;
- `selected = executed + ignored`; `executed = passed + failed`;
- mandatory pass requires selected/executed/passed > 0, failed=0 and ignored=0;
- non-test proof uses assertion counts `1/1/1/0/0`;
- timestamps are UTC RFC3339 and end cannot precede start;
- passed receipts require clean primary and paired sources;
- writer copies the executed daemon and every terminal artifact into immutable content-addressed binary/evidence
  archives; historical validation re-hashes those archive objects rather than mutable raw/current paths;
- process/external/release proofs require one pinned `linux-production-like` host profile and the same attested daemon binary.

Every manifest is published both as a mutable current alias and as an immutable archive leaf. The archive key is:

```text
source-binding-digest = SHA256(canonical-json({
  "domain": "quanta-proof-source-binding-v1",
  "source": source,
  "source_pair": source_pair_or_null
}))
archive/<proof-id>/<source-binding-digest>/<manifest-sha256>.json
```

Canonical JSON uses UTF-8, sorted keys, no insignificant whitespace and no ASCII escaping. `manifest-sha256` is the
SHA-256 of the final manifest bytes. Exact-byte retry is idempotent; a different byte sequence can never replace an
existing leaf. `dependency_receipts` records the dependency's exact archive path and digest, never its current alias.
Historical validation follows this archive DAG without requiring registry-alias equality, so a later run may advance
the alias without invalidating earlier evidence. Each source-binding namespace has an issuance index; an indexed leaf
that is missing or modified fails closed rather than being silently recreated. Archive directories must be real
repo-contained directories, not symlinks. Missing, renamed or mutated archive/evidence/binary bytes fail closed.

Lane handoffs are schema-validated and then semantically checked by `tools/ci/lint/check-lane-handoff.py`. It binds
the canonical lane/ticket/proof/status tuple, exact `base..result` Git write set, recorded proof counts and current-clean
source identity to the result commit. It requires immutable archive paths, checks base/result ancestry, cross-binds
paired-repository state to exact-pair manifests, validates paired push identity, and verifies P02I merge/cherry-pick
provenance. Handoff prose or schema validity alone is not authority.

Handoff objects follow result commits and are excluded from source dirty digest;
tracked source cannot self-reference its result SHA. Implementation custody is
checkpoint → clean result proof/manifest → semantic validation → non-force push
→ observed remote SHA. P02I reruns integrated P02A/P02B and binds original/applied
commits plus merge/cherry-pick mode. Paired repositories are ordered Quanta then
Semantica; top-level base/result/dirty equals Quanta and each PUSHED remote SHA
equals result. Exported contracts bind result-blob SHA-256. Proof references use
immutable source-binding/manifest-digest archive leaves and immutable terminal/
daemon objects; later aliases cannot overwrite historical edges. Strict current
and historical ancestry/archive checks remain different validator modes.
Historical full-chain audit detects omission/duplicate/order/fork/join/adjacent
SHA mismatch; it is not a prerequisite for current-source release qualification.

Verdicts remain separate: `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, `ROLLBACK_PROVEN`. None implies another.

## Rejected alternatives

- signal wiring without child ownership;
- socket liveness as process readiness;
- boot-time legacy migration;
- label-only host identity;
- artifact schema without a blocking validator;
- one `PRODUCTION_READY` boolean derived from partial evidence.
