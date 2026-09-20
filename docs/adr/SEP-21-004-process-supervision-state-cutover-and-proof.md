# SEP-21-004 — Process Supervision, State Cutover and Proof

Status: `Accepted`

Decided: 2026-09-21

Gate owner: S21-00; blocks S21-09, S21-10, S21-11, S21-12 and S21-13

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

Migration/backup/restore is offline-only under the exclusive state-root lease:

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

## Cross-repository cutover

Mutation protocol version 2 plus contract digest handshake occurs before body decode. Live dual decoder is forbidden.
Order: publish Quanta contract/SDK V2, pin Semantica exact dependency, quiesce ingress, backup/offline migrate, start
attested V2 daemon, handshake/canary, switch producer, then activate. Mismatched producer/daemon is
`PROTOCOL_VERSION_UNSUPPORTED` with zero mutation.

## Proof authority

- registry: `tools/ci/proof-authority.toml`;
- manifest schema: `tools/ci/proof-manifest.schema.json`;
- validator: `tools/ci/lint/check-proof-authority.py`;
- proof families: `S/U/A/D/P/F/Q/X`;
- source SHA is exactly 40 lowercase hex;
- dirty digest hashes `git diff --binary` plus sorted untracked paths and content digests;
- host identity is a SHA-256 digest over stable OS/arch/CPU/memory/runner identity inputs;
- `selected = executed + ignored`; `executed = passed + failed`;
- mandatory pass requires selected/executed/passed > 0, failed=0 and ignored=0;
- non-test proof uses assertion counts `1/1/1/0/0`;
- timestamps are UTC RFC3339 and end cannot precede start;
- validator re-hashes daemon and every artifact from disk;
- process/external/release proofs require one pinned `linux-production-like` host profile and the same attested daemon binary.

Verdicts remain separate: `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED`, `ROLLBACK_PROVEN`. None implies another.

## Rejected alternatives

- signal wiring without child ownership;
- socket liveness as process readiness;
- boot-time legacy migration;
- label-only host identity;
- artifact schema without a blocking validator;
- one `PRODUCTION_READY` boolean derived from partial evidence.
