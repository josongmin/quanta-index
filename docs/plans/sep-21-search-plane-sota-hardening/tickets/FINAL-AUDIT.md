# SEP-21 Search Plane SOTA Hardening — Final Static Audit

> Archive classification: historical pre-implementation audit. Current status
> is owned by [INDEX.md](INDEX.md) and
> [CURRENT-RESIDUAL-2026-09-26.md](CURRENT-RESIDUAL-2026-09-26.md); custody
> follows [SEP-27-001](../../../adr/SEP-27-001-documentation-authority-and-historical-record-custody.md).

Audit date: 2026-09-21

Audit type: static source and plan review only

Snapshot note: this verdict describes the pre-implementation audit at the recorded source/digests. Current gate status
is owned by [INDEX.md](INDEX.md) and source-bound proof manifests; this snapshot is not a live completion signal.

Verdict: `PLAN_READY_WITH_GATE`; M0 proof machinery exists, but implementation remains `BLOCKED` until the amended
S21-00 contract has a corrected current-source manifest/handoff.

## 1. Scope and evidence boundary

Reviewed inputs:

- current source at audit start: `3ad279a08879de35fa96a5495a3382af28f095d0`
- tracked-diff digest at audit start: `f8b647f71ecac82de299382c956726b0702f6ab4efab11eeddf475cfc7709534`
- tracked-diff digest observed at audit close: `e3509147bb1e1cc6b0fa62e4d7549434199bd5133c92d1ed605e5aeff407ddcf`
- purpose checklist and three static audit passes listed in [INDEX.md](INDEX.md)
- current implementation owners across contract, core, catalog, RepoMap, search-plane, IPC, daemon, SDK and CI
- all S21-00 through S21-13 ticket drafts

This review did not run tests, build, lint, benchmark, process probes, migration drills, provider calls or
cross-repo qualification. Therefore it proves plan coverage and source-backed risk only. It does not prove code
correctness, release qualification, deployability or activation.

The working tree was already dirty and this ticket packet was untracked during the audit. The tracked diff changed
concurrently while the plan was being updated; this audit changed only the new plan directory. The source SHA and
digests above are provenance observations, not a clean-source qualification receipt. Implementation must re-freeze
HEAD, dirty digest and owner paths before editing.

## 2. Final findings

### FA-01 — P0 runtime guards and state-root lease are dropped before serving

Evidence:

- `crates/quanta-index-searchd/src/app/searchd.rs:21-27` consumes `SearchdRuntime`, moves only three servers and
  discards the remaining owned fields through `..`.
- `crates/quanta-index-searchd/src/app/runtime.rs:1036-1043` states that maintenance, corpus lifecycle and the
  state-root lease must live until runtime drop, after servers join.

Consequence:

- the lifetime guarantee is false on the release `drive` path;
- maintenance/lifecycle can stop before the UDS servers;
- the state-root lease can be released while the daemon still serves, allowing a second daemon to acquire the
  same root.

Required structural correction: S21-09 introduces a supervisor-owned `RuntimeGuards` bundle and releases it only
after every child, connection and provider task has terminated or the process takes the frozen hard-escalation
path. A real two-process lease proof is mandatory.

### FA-02 — P0 RepoMap visibility would retain two durable authorities without an explicit deletion rule

The earlier S21-02 draft allowed a self-digested activation file while S21-04 introduced a catalog journal. That
creates a file/catalog reconciliation problem and permits disagreement after crash or restore.

Required structural correction:

- SQLite `repomap_candidate_v1` and `repomap_activation_v1` are the only visibility/lifecycle authority;
- the object store owns immutable candidate bytes only;
- runtime `activations/` readers/writers are deleted, not shadowed or dual-written;
- in-memory registries are derived caches published only after catalog commit.

### FA-03 — P0 dependency order allowed sealing invalid or unjournaled candidates

S21-02 previously depended only on identity/layout. That allowed activation machinery to land before graph
validation and the operation protocol.

Required structural correction: S21-01A pure contract lands first; S21-03 compiler and S21-04 journal may land in
parallel; P02I joins them; P03 closes S21-01B and S21-02 while consuming only `CompiledRepoMapCandidateV1` and the
frozen journal API. The dependency graph and execution waves encode this order.

### FA-04 — P1 operation protocol conflated inspection, mutable claim and semantic refusal

Current `crates/quanta-index-core/src/domains/idempotency.rs:19-32,72-115` exposes `begin/finalize` and permits an
unfinished attempt to be re-applied. This is not sufficient for exact replay, stale-owner fencing or deterministic
terminal refusal.

Required structural correction: split `inspect`, immutable prepare, `claim_prepared`, `record_refused`, `commit`
and `recover`; run terminal replay before current mutable preflight; make terminal sequence and receipt commitments
catalog constraints rather than caller conventions.

### FA-05 — P1 RepoMap read declaration is not a resource pin

Evidence:

- `query_dispatcher/routes/repo_map.rs:28-39` acquires a declared view but executes through the ambient
  `self.repo_map_query` port.
- `query_dispatcher/read_view/view.rs:138-163` has no RepoMap handle in `LedgerParts` or `QueryReadViewV1`.

Required structural correction: acquire the active catalog identity and `Arc` atomically into
`QueryReadViewV2`; execute only through `view.repo_map()`. DoD is one evidence record per declared domain, not
the incorrect assumption that declared-domain count equals physical-handle count.

### FA-06 — P1 result windows cannot honestly encode capped unknown remainder

A returned row count smaller than `top_k` does not prove exhaustion when ANN, refill ceilings, nested caps or
cancellation were involved. One generic exact/partial shape cannot distinguish those cases.

Required structural correction: `ExecutionOutcomeV2` and `QueryResultWindowV2` carry `Exact`, `LowerBound`,
`CappedUnknown`, `InterruptedPartial` and `Approximate` explicitly. `has_more=false` requires an exhaustion proof.
Pageable and bounded-top-k routes have separate capability contracts.

### FA-07 — P1 SDK response validation is variant-level rather than request-authority-level

Required structural correction: construct plane-specific closed expected-response enums before moving the request
payload, then apply intrinsic decoding and contextual binding after request-ID verification. Active selectors are
validated by resolution proof and activation epoch, not by comparing an unresolved selector with a resolved pin.
Global sequence monotonicity remains S21-04 catalog authority.

### FA-08 — P1 provider policy omitted source-content egress and process ownership

Provider admission must cover both query embedding and source derive egress, including
`crates/quanta-index-search-plane/src/semantic_derive.rs`. S21-08 owns policy/admission/reservation; S21-09 owns
task lifecycle and joins. Both close atomically in M3 so neither detached work nor policy-free egress is temporarily
accepted.

### FA-09 — P1 transport discards credentials needed for operation authorization

`crates/quanta-index-ipc/src/server.rs:66-76` passes only request and budget to the dispatcher after socket-level
screening. Operation-level capability checks need kernel-derived peer identity and request context.

Required structural correction: transport creates `DispatchContextV1`; authorization is an exhaustive opcode to
capability map before control dispatch. Readiness is process-wide and distinct from repository generation status.
Request correlation uses bounded diagnostics, not high-cardinality metric labels.

### FA-10 — P1 migration/qualification plans lacked executable authority details

Required structural correction:

- migration is offline-only with exclusive lease, SQLite backup API, staging restore, deep scrub, manifest-last
  fsync and same-filesystem atomic cutover;
- boot-time live legacy migration is removed after cutover;
- proof infrastructure lands early as S21-13 phase A;
- final evidence distinguishes `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED` and `ROLLBACK_PROVEN` and uses the same
  attested release daemon binary across process proofs.

## 3. Contradictions resolved in the final packet

| Earlier ambiguity | Final rule |
|---|---|
| activation file plus catalog | catalog is sole visibility authority; activation files deleted |
| S21-02 before compiler/journal | S21-03 and S21-04 precede S21-02 |
| claim then validate invalid intent | immutable prepare first; rejected requests become terminal without claim |
| domain count equals handle count | one evidence per declared domain; handles per physical resource group |
| all routes paginate | pageable and bounded-top-k capability classes are separate |
| active selector response equals request pin | activation resolution proof and epoch bind the response |
| sequence monotonicity checked by SDK | catalog owns global uniqueness/monotonicity; SDK checks intrinsic receipt validity |
| metrics carry request correlation | bounded trace/diagnostic sink carries correlation; metrics stay low-cardinality |
| migration may occur during boot | v1 migration is offline-only; production boot has no legacy reader/migrator |
| release qualification is one boolean | code/deploy/activate/rollback verdicts are independent |
| P01 owns live layout and quarantine | P01A owns pure codec/error/security only; P03 owns S21-01B live cutover |
| quarantine can mint a local incident ID | P02B global event sequence plus P03 catalog-first crash protocol |
| runtime removes legacy activation bytes | P03 refuses without mutation; current policy also refuses old roots rather than importing them |
| every lane validates all old handoffs | immediate predecessor only; P02I validates fork; P12A builds and P12Q validates transitive DAG |
| dependency validation is P12 evidence | P12A aggregate writer/verdict producer must emit receipt; P12Q issues the P12 manifest |

## 4. Coverage judgment

- P0/P1 finding routing: complete at plan level in [INDEX.md](INDEX.md) section 7.
- file/symbol action ownership: complete in S21-00 through S21-13 and [ACTION-LIST.md](ACTION-LIST.md).
- dependency/order closure: complete at plan level; M0 decision freeze remains intentionally blocking.
- runtime/external proof: specified but `NOT_RUN` by user instruction.
- implementation, migration and compatibility closure: not started.

No ticket may be marked done from static review alone. Closing evidence must include its named negative, recovery,
consumer and process/external proofs on a re-frozen final source.

## 5. Contract-repair resolution and remaining gate

The amended ADR/registry now freezes:

- exact digest framing and distinct repository/logical domains;
- exact candidate/artifact/quarantine integer-key schemas and digest inputs;
- `%`/slash/dot logical-byte policy with zero path projection;
- P01A/P03/P10 ownership, generic global event ledger and quarantine crash order;
- only-P02A/P02B parallelism, immediate/transitive handoff validation and a real P12A aggregate producer plus P12Q qualifier.

P00 does not falsely claim a final error-code table while source remains free-form. It freezes a source-bound baseline
inventory and the migration rules. P01A owns the final closed table/cardinality/digest and cannot hand off until
free-form paths are zero.

The following product decisions were already frozen by the original S21-00 packet:

1. identifier normalization/rejection and canonical byte encoding;
2. persisted receipt version and old/new compatibility refusal;
3. terminal `Refused` retention/replay policy and replay floor;
4. serial-ingest enforcement versus multi-writer fencing;
5. state-root format/cutover/rollback boundary;
6. hard-drain escalation and exit-code semantics;
7. cursor integrity model and `focus_subjects` semantics;
8. tenant/source/query provider egress policy;
9. process readiness versus repository readiness semantics.

The remaining gate is operational evidence: regenerate and validate the corrected P00 manifest/handoff on one clean
current source. Until it explicitly permits P01A, downstream product implementation is `BLOCKED`.
