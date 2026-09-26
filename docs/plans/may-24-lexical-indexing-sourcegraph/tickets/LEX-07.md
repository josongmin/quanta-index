# LEX-07 — Generation governance + history extension (parent:/merge:/tag:/revisions:)

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md), and [MAY-27-002](../../../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


> Status: `shipped (architecture-corrected)`
> Crate: `quanta-index-lq-history`
> Tests: 76
> Last verified: 2026-05-25
> Wave: 4 (Canonical Execution Wave per [rfc.md § Canonical Execution Waves](../rfc.md)).
> Source: [rfc.md § LQ/History-1.1](../rfc.md), [rfc.md § Canonical Incremental Write Pipeline](../rfc.md), [rfc.md § Index Lifecycle](../rfc.md), [rfc.md § Migration and Versioning Policy](../rfc.md), [implementation-plan.md § 5.11 LEX-07](../implementation-plan.md), [implementation-plan.md § Appendix A.1 RFC-GAP-1](../implementation-plan.md), [feature-scope.md § 1.2.4](../feature-scope.md), [feature-scope.md § 4.7](../feature-scope.md), [usecase.md § UC-HIST-01..08](../usecase.md), [dsl.md § 2.2 LQ/History-1.1 extensions](../dsl.md).
> Posture: **breaking-first** per [CLAUDE.md § Agent change posture](../../../../CLAUDE.md). No long-lived shims. Every DoD item is provable. No `#[derive(serde::Serialize)]` / `#[derive(serde::Deserialize)]` per D18 — manual `impl` only.
>
> **Architecture correction:** consumes `UpsertCommit` / `UpsertRef` / `UpsertTag` channel ops (producer-authored). `add_commit` / `add_ref` / `add_tag` are channel-subscriber callbacks, not IPC endpoints. See [INDEX.md](INDEX.md) §3.6 for the producer-authorship correction context.

---

## 1. Purpose

Consume producer-supplied commit/ref/tag channel ops and index them so the LQ planner can serve history filters (`parent:`, `merge:`, `tag:`, `revisions:`, `since.time:`) from a local cache. Per [channel-architecture.md § 0](../../../ssot/channel-architecture.md) and [channel-architecture.md § 3.1](../../../ssot/channel-architecture.md), the search plane is a pure index + query plane: it never reads git, never walks remote refs, never authors commit metadata. The producer (`semantica-codegraph-v2`) authors every `CommitRecord` (including `parents` and `applied_at_ms`), every ref pointer, and every tag pointer, and ships them via `LexicalChannelOp::UpsertCommit` / `UpsertRef` / `UpsertTag` / `DeleteRef` / `DeleteTag`.

Two coupled deliverables close the Wave-4 history exit gate and the RFC-GAP-1 callback from [implementation-plan.md § Appendix A.1](../implementation-plan.md):

1. **Generation governance (search-plane authority over its own state)** — promote the manifest to the single authoritative linearization point for every `(repo, rev, generation_set)` activation. Inputs are producer-emitted channel ops; the search plane's authority is restricted to its own activation/regression-guard state:
   - manifest-first atomicity (sibling indexes never become reader-visible without `MARKER_OK`);
   - strictly monotonic `manifest_gen` per `(repo, rev)` with non-decreasing per-component generations;
   - stale-activation guard (reject any activation that regresses any component);
   - write-packet trace per accepted delta apply (verifiable record of `committer_id`, `applied_at_ms`, `before_gen`, `after_gen`, `hash`). `applied_at_ms` is carried by the producer-supplied `CommitRecord`, not stamped by the search plane;
   - idempotency: re-applying the same packet against the same prior generation is a typed no-op with a trace-match assertion.
2. **History extensions** — extend the `LQ/History-1.1` planner + history engine to cover the RFC-GAP-1 surface: `parent:`, `merge:`, `tag:`, `revisions:`, and `since.time:` (disambiguated from `since.commit:` per [feature-scope.md § 1.2.4 Q2](../feature-scope.md)). Each filter pushes down into the local cache populated by producer ops; **no request-time `git log`**, no producer-side re-extraction, and no remote ref walk is permitted on the production path.

Together the two halves enforce: (a) every history hit is bound to one canonical `manifest_generation`; (b) `since.time:` truth source is the producer-supplied `applied_at_ms` recorded in the write-packet trace, not wall-clock at request time; (c) the history sidecar index version evolves under the same monotonicity rules as content/path/symbol siblings; (d) no silent fallback exists when an authoritative input is absent (refs, ranges, tags, parents).

This ticket does **not** redefine the parser. Grammar and EBNF are owned by [dsl.md § 2.2 / § 6](../dsl.md). LEX-07 implements the channel-subscriber callbacks, the local DAG cache, the planner pushdown, and the monotonicity rails.

---

## 2. Background

### 2.1 Current-state (per [implementation-plan.md § 2](../implementation-plan.md))

- Per [channel-architecture.md § 3.1](../../../ssot/channel-architecture.md), the canonical producer→search-plane integration surface is `BundleChannelPublisher` / `BundleChannelSubscriber`. The history-track ops `UpsertCommit`, `UpsertRef`, `UpsertTag`, `DeleteRef`, `DeleteTag` are **proposed, pending producer agreement** in that SSOT — LEX-07 is the search-plane consumer ticket for that op set and cutover blocks on the producer side accepting these shapes.
- Per [channel-architecture.md § 5.2](../../../ssot/channel-architecture.md), generation state is reconstructed from channel events on startup; there is no SQLite control plane. References below to "generation governance" describe in-memory ledger state derived from observed `Seal` events.
- The current `LexicalCandidate` cannot represent commit / diff hits. **GAP-02** ([usecase.md § 3](../usecase.md)) lands typed `CommitCandidate` and `DiffCandidate` via PRE-CONTRACT-EXT (Wave 0). LEX-07 wires the planner + index + executor against those types; LEX-07 itself ships no new contract leaf beyond the bridge to those types (Q-UC-2 closed in Wave 0).
- RFC § LQ/History-1.1 lists `type:commit`, `type:diff`, `author:`, `committer:`, `message:`, `before:`/`after:`/`since:`/`until:`, `diff.added:`/`removed:`/`touched:`. It does **not** enumerate `parent:`, `merge:`, `tag:`, `revisions:`, `since.time:`/`since.commit:`. RFC-GAP-1 ([implementation-plan.md § Appendix A.1](../implementation-plan.md)) names this as a scope amendment LEX-07 must close.

### 2.2 Why manifest-first is load-bearing for history

History queries (`type:commit`, `type:diff`) read across rev-ranges that span many manifest generations. Without a strict linearization point:

- a reader could observe a commit row whose diff hunk index has not yet committed `MARKER_OK` → silent partial result;
- a stale-activation regression could re-introduce a deleted hunk after the activation was already observed → readers see different results across requests against the same `(repo, rev)`.

RFC § Atomicity contract requires the manifest write to be the **single** linearization point and storage-layer assertions to reject any read that observes a `manifest_generation` whose sibling `MARKER_OK` is absent. LEX-07 makes the history sidecar (commit metadata index + diff hunk index + commit-DAG cache) participate in that contract as **first-class siblings**.

### 2.3 Why `since.time:` is tied to `applied_at_ms`

Sourcegraph's `since:` is a time-relative filter against producer wall-clock at indexing time. Quanta's posture is fail-closed and authoritative: the only legitimate truth source for "when did this commit enter the system" is the producer-supplied `applied_at_ms` carried inside `CommitRecord` (per [channel-architecture.md § 3.1 authorship rule](../../../ssot/channel-architecture.md)) and recorded into the write-packet trace at delta apply. Reading wall-clock at request time would re-introduce a non-deterministic boundary — forbidden by RFC § Non-Negotiable Invariants §6 (no incremental claim without delta mutation proof). Hence `since.time:` lowers to a comparison against the trace's `applied_at_ms` field (sourced from the producer commit op), **not** against `now()`, and **not** against any search-plane clock.

### 2.4 Position in the dependency graph

- Depends on: LEX-05 (parallel executor + deterministic merge) — LEX-07 reuses the merge tuple `(score DESC, repo_id ASC, manifest_generation ASC, candidate_id ASC)` per [rfc.md § Merge determinism rule](../rfc.md).
- Blocks: RT-01 (`repo:has.commit.after` predicate eval relies on the LEX-07 history catalog), SEM-01 (history-aware hybrid is an out-of-wave follow-up), BRIDGE-01 (per AC-13 the bridge MUST reject `into:codeql` + `type:diff` — that planner-table row is owned here).
- Sibling: LEX-06 (ranking + explain) — LEX-07 ranks commits by `(score DESC, applied_at_ms DESC)` per the merge tuple; explain payload is reused.

---

## 3. Inputs

### 3.0 Runtime inputs (authoritative)

LEX-07 consumes only these inputs at runtime. No git, no remote ref walk, no source-bytes parsing.

| Input                                  | Source                                                                                        | Carries                                                                                                          |
| -------------------------------------- | --------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `LexicalChannelOp::UpsertCommit`       | producer (`semantica-codegraph-v2`) via `BundleChannelPublisher`                              | `CommitRecord { sha, parents: Vec<CommitSha>, applied_at_ms, author, committer, message, is_merge, tags, ... }` |
| `LexicalChannelOp::UpsertRef`          | producer                                                                                      | `{ name, sha }` — named pointer; producer authority                                                              |
| `LexicalChannelOp::UpsertTag`          | producer                                                                                      | `{ name, sha }` — named pointer; producer authority                                                              |
| `LexicalChannelOp::DeleteRef`          | producer                                                                                      | `{ name }`                                                                                                       |
| `LexicalChannelOp::DeleteTag`          | producer                                                                                      | `{ name }`                                                                                                       |
| LQ query carrying history filters      | query client via UDS                                                                          | parsed `parent:` / `merge:` / `tag:` / `revisions:` / `since.time:` / `since.commit:` predicates                |

Per [channel-architecture.md § 3.1 authorship rule](../../../ssot/channel-architecture.md), `CommitRecord` (including `parents` and `applied_at_ms`) is **authored by the producer**. The search plane decodes and indexes; it never extracts. The op set above is **proposed, pending producer agreement** per [channel-architecture.md § 3.1 status](../../../ssot/channel-architecture.md); LEX-07 ships the search-plane consumer.

### 3.1 Documents

- [rfc.md § LQ/History-1.1](../rfc.md), [§ Canonical Incremental Write Pipeline](../rfc.md), [§ Atomicity contract](../rfc.md), [§ Generation model](../rfc.md), [§ Monotonicity rules](../rfc.md), [§ Index Lifecycle](../rfc.md), [§ Migration and Versioning Policy](../rfc.md), [§ Error Code Taxonomy](../rfc.md).
- [feature-scope.md § 1.2 LQ/History-1.1](../feature-scope.md), [§ 1.2.4 RFC-omitted history filters](../feature-scope.md), [§ 4.2 History-1.1 cross-reference](../feature-scope.md), [§ 4.7 Flagged gaps](../feature-scope.md), [§ 9 Open question Q2](../feature-scope.md).
- [usecase.md § D History](../usecase.md) (UC-HIST-01..08), [§ B Predicate filters § UC-PRED-02](../usecase.md), [§ H Cross-cutting § UC-EDGE-07](../usecase.md), [§ 3 GAP-02](../usecase.md), [§ 4 AC-08, AC-09, AC-13](../usecase.md).
- [dsl.md § 2.2 LQ/History-1.1 extensions](../dsl.md), [§ 6.6 rev: grammar](../dsl.md), [§ 7 Predicate sub-grammar](../dsl.md), [§ 12 Error taxonomy](../dsl.md), [§ 13 Limits and budgets](../dsl.md).
- [implementation-plan.md § 5.11 LEX-07 DoD](../implementation-plan.md), [§ Appendix A.1 RFC-GAP-1](../implementation-plan.md), [§ 4.5 Wave 4](../implementation-plan.md), [§ 6 Risk register R3 R5](../implementation-plan.md), [§ 7 Cutover and migration plan Wave 4](../implementation-plan.md), [§ 10 ADR-011](../implementation-plan.md).
- [channel-architecture.md § 0 Scope](../../../ssot/channel-architecture.md), [§ 3.1 Traits / authorship rule / proposed history ops](../../../ssot/channel-architecture.md), [§ 5 Searchd Internals](../../../ssot/channel-architecture.md).
- [CLAUDE.md § Agent change posture](../../../../CLAUDE.md), [§ Rule Catalog](../../../../CLAUDE.md), [AGENTS.md](../../../../AGENTS.md).

### 3.2 Upstream artifacts already landed (or landing in Wave 0/1/2/3)

- PRE-CONTRACT-EXT (Wave 0): `CommitCandidate`, `DiffCandidate`, `lq_version`, `LexicalErrorCode` SCREAMING_SNAKE_CASE enum, hand-rolled serde for all new types.
- PRE-NORM (Wave 0): canonical normalizer + CBOR canonical encoding + `LqCanonicalHashV1`.
- LEX-00 (Wave 1): 13 RFC non-negotiable invariants ratified as property tests.
- LEX-01 (Wave 1): parser accepts `type:commit`, `type:diff`, `author:`, `committer:`, `message:`, `before:`, `after:`, `since:`, `until:`, `diff.added:`, `diff.removed:`, `diff.touched:`, `parent:`, `merge:`, `tag:`, `revisions:`, `since.time:`, `since.commit:`. **Parser scope is LEX-01**; LEX-07 wires the planner + executor.
- LEX-03 (Wave 2): one catalog binds `(repo, rev, generation) → siblings`. LEX-07 extends sibling set with commit metadata + diff hunk + commit-DAG cache.
- LEX-04 (Wave 3): writer-coordinator + write-packet trace skeleton. LEX-07 extends the trace with the `committer_id` field and the history-sibling identities.
- LEX-05 (Wave 3): merge tuple + cross-instance reproducibility test fixture.

### 3.3 Producer-side preconditions

- Producer (`semantica-codegraph-v2`) emits `LexicalChannelOp::UpsertCommit` / `UpsertRef` / `UpsertTag` / `DeleteRef` / `DeleteTag` ops via `BundleChannelPublisher` per [channel-architecture.md § 3.1](../../../ssot/channel-architecture.md). Producer is the codegraph and already has git history loaded; extraction is producer-side, not search-plane-side.
- Each `UpsertCommit` carries `CommitRecord { sha, parents: Vec<CommitSha>, applied_at_ms, is_merge, author, committer, message, ... }`. The `parents` field is the producer's authoritative DAG export; `applied_at_ms` is the producer's monotonic stamp at the moment the commit was integrated.
- Ref / tag pointers (`name → sha`) are authored by the producer. The search plane never reads remote refs.
- **Producer cross-team handoff doc must commit alongside the LEX-07 PR** at `docs/handoffs/lq-history-1.1.md` per [implementation-plan.md § 7.3 Producer handoff artifacts](../implementation-plan.md). The handoff doc carries the wire shape of `CommitRecord` and the ordering guarantee for `UpsertCommit` (open question Q-LEX07-10 in § 12).

---

## 4. Deliverables

### 4.1 Contract surface (consumed, not introduced)

- `CommitCandidate { candidate_id, repo_id, manifest_generation, commit_id, parent_commit_ids: Vec<CommitId>, is_merge: bool, author, committer, applied_at_ms, message_snippet, score }`. **Lands in PRE-CONTRACT-EXT**; LEX-07 consumes.
- `DiffCandidate { candidate_id, repo_id, manifest_generation, commit_id, repo_relative_path, hunk_id, side: DiffSide, snippet, applied_at_ms, score }`. **Lands in PRE-CONTRACT-EXT**; LEX-07 consumes.
- `LexicalErrorCode` extensions used by this ticket: `STATE_NOT_READY` (with `kind = HISTORY_UNINDEXED`), `STATE_GENERATION_REGRESSION`, `EXEC_CATALOG_MISS`, `PLAN_LIMIT_EXCEEDED`, `PLAN_UNSUPPORTED_COMBO`, plus new code added by this ticket (see § 8).

### 4.2 New typed surfaces (in `quanta-index-contract`)

- `HistoryRef { kind: HistoryRefKind, value: String }` with `HistoryRefKind = Branch | Tag | Sha | Range`. Hand-rolled `impl serde::Serialize`/`Deserialize` per D18.
- `RevisionRange { from: HistoryRef, to: HistoryRef, kind: RangeKind }` with `RangeKind = TwoDot | ThreeDot` per [dsl.md § 6.6 rev: grammar](../dsl.md).
- `HistoryQueryError` extension variants (new typed codes; see § 8): `HISTORY_REF_NOT_FOUND`, `HISTORY_RANGE_OVERRUN`, `HISTORY_MERGE_CYCLE`.

### 4.3 New surfaces (in core; physical home subject to G-CONTROL-LOC)

**`CommitGraph` — local DAG cache (accumulator).** Shape: in-memory map `{ CommitSha → CommitNode }` plus a tag/ref index. Mutated only by channel-subscriber callbacks below. Read by the planner. The shape mirrors what the producer ships; the search plane never derives commit metadata.

Channel-subscriber callbacks (driven by `BundleChannelSubscriber::next_event()` per [channel-architecture.md § 5.3 dispatcher loop](../../../ssot/channel-architecture.md)). These are **callbacks invoked when an op is observed**, not an API for the search plane to author commits:

- `CommitGraph::add_commit(node: CommitNode) -> Result<(), HistoryDecodeError>` — invoked on `UpsertCommit`. Decodes producer-supplied `CommitRecord`, inserts into the cache. Errors: `HISTORY_COMMIT_DECODE_FAIL`, `HISTORY_COMMIT_PARENT_UNKNOWN` (see § 8).
- `CommitGraph::add_ref(name: &str, sha: CommitSha)` / `delete_ref(name: &str)` — invoked on `UpsertRef` / `DeleteRef`. Errors: `HISTORY_REF_DECODE_FAIL`.
- `CommitGraph::add_tag(name: &str, sha: CommitSha)` / `delete_tag(name: &str)` — invoked on `UpsertTag` / `DeleteTag`. Errors: `HISTORY_REF_DECODE_FAIL`.

History filter primitives (unchanged shape; read against the local `CommitGraph`):

- `parents_within_depth(start: CommitSha, depth: u32) -> Vec<CommitSha>`
- `merge_commits(scope: RangeOrAll) -> Vec<CommitSha>`
- `tag_resolve(pattern: &TagPattern) -> Vec<CommitSha>`
- `enumerate_revisions(range: &RevisionRange) -> Vec<CommitSha>`
- `since_time(threshold_ms: i64) -> Vec<CommitSha>` (reads `applied_at_ms` carried by the producer commit op)

Index build / query ports (still search-side because they own the on-disk sibling shards):

- `SearchPlaneHistoryIndexBuildPort::build_commit_metadata(...)`, `build_diff_hunks(...)`, `build_commit_dag(...)` — populate sibling shards from the local `CommitGraph` cache; never re-derive from source bytes.
- `SearchPlaneHistoryIndexPort::query_commits(plan: &LqHistoryPlan) -> Stream<CommitCandidate>`.
- `SearchPlaneHistoryIndexPort::query_diffs(plan: &LqHistoryPlan) -> Stream<DiffCandidate>`.
- `SearchPlaneHistoryIndexPort::resolve_history_ref(repo: &RepoId, r: &HistoryRef) -> Result<ResolvedHistoryRef, LexicalErrorCode>` (resolves via `CommitGraph` ref/tag index — not via git).

Activation/regression-guard (search-plane authority over its own state; inputs come from producer ops, not from search-plane-authored packets):

- `WritePacketTrace::record_history_apply(packet: &HistoryWritePacket) -> Result<TraceRecord, CoreError>` — records the apply event of producer-emitted history ops; extends the LEX-04 trace primitive. `packet` is constructed from a decoded producer op, not authored by the search plane.
- `ManifestLedger::apply(outcome: ApplyOutcome)` — search-plane ledger flips `materialized=true` for a generation when its sibling shards finish building per [channel-architecture.md § 5.2](../../../ssot/channel-architecture.md).
- `GenerationGovernance::activate(manifest_gen: ManifestGen, siblings: &SiblingGenSet) -> Result<ActivationOutcome, LexicalErrorCode>` (rejects regression; idempotent re-activation returns `ActivationOutcome::AlreadyActive`).

### 4.4 New planner pushdown (in core)

- `LqHistoryPlanner::lower(query: &LqQueryV1, ctx: &PlanContext) -> Result<LqHistoryPlan, LexicalErrorCode>` — owns the routing decision for every history filter and emits a typed `LqHistoryPlan` that the executor consumes.
- Routing table (new, owned by this ticket):

  | Filter            | Truth source                                       | Push-down target                  |
  | ----------------- | -------------------------------------------------- | --------------------------------- |
  | `parent:<rev>`    | commit-DAG cache (`parent_commit_ids`)             | commit metadata index             |
  | `merge:yes|no|only` | commit metadata field (`is_merge`)                | commit metadata index             |
  | `tag:<pattern>`   | tag → commit resolver (commit-DAG cache)           | commit metadata index             |
  | `revisions:<range>` | revision range resolver (`rev_range`)            | commit metadata index             |
  | `since.time:<t>`  | write-packet trace `applied_at_ms`                 | commit metadata index             |
  | `since.commit:<r>` | commit-DAG walk from resolved ref                 | commit metadata index             |

### 4.5 History sidecar shard layout (per-generation)

Per-generation directory adds three sibling shards alongside content/path/symbol:

- `commits/` — commit metadata index with fields `{commit_id, author, committer, message, applied_at_ms, is_merge, parent_commit_ids, tags}`.
- `diff_hunks/` — diff hunk content index with fields `{commit_id, repo_relative_path, hunk_id, side, added_text, removed_text, touched_text}`.
- `commit_dag/` — bounded-depth DAG cache: forward edges `commit_id → children`, reverse edges `commit_id → parent_commit_ids`, and a tag→commit map.

Each sibling carries its own `MARKER_OK` and participates in the manifest-first atomicity contract per [rfc.md § Atomicity contract](../rfc.md).

### 4.6 Tests, fixtures, observability (see § 6 and § 7).

### 4.7 Producer handoff

- `docs/handoffs/lq-history-1.1.md` committed in the same PR. Includes new type catalog (serde shape + review checklist), migration rail, skew window, rollback instructions per [implementation-plan.md § 7.3](../implementation-plan.md).

---

## 5. Implementation steps (TDD)

Each step lands red → green → refactor. No step may merge with a `#[ignore]` test. The search plane is a pure consumer of producer-emitted channel ops; no step may introduce commit authorship, git reads, or remote ref walks.

### 5.0 Step Pre-A — Channel op decode + CommitGraph cache (TDD red)

1. Failing test `upsert_commit_decodes_and_inserts`: subscriber observes a well-formed `LexicalChannelOp::UpsertCommit`; `CommitGraph::add_commit` decodes `CommitRecord` and inserts a node carrying `{sha, parents, applied_at_ms, is_merge, ...}` exactly as authored by the producer.
2. Failing test `upsert_commit_malformed_payload_fails`: malformed CBOR for `CommitRecord` surfaces `HISTORY_COMMIT_DECODE_FAIL`; no partial insert.
3. Failing test `upsert_commit_parent_unknown_fails`: an `UpsertCommit` whose `parents` reference a sha not yet present in the cache surfaces `HISTORY_COMMIT_PARENT_UNKNOWN` (order-dependence violation); decision on buffer-vs-reject locked by Q-LEX07-10.
4. Failing test `upsert_ref_and_tag_update_index`: `UpsertRef` / `UpsertTag` insert `{name → sha}` into the ref/tag index; `DeleteRef` / `DeleteTag` remove. Malformed ops surface `HISTORY_REF_DECODE_FAIL`.
5. Implement `CommitGraph::{add_commit, add_ref, add_tag, delete_ref, delete_tag}` as channel-subscriber callbacks. No code path may construct a `CommitNode` outside these callbacks.

### 5.1 Step A — Generation governance: monotonicity rails (TDD red)

1. Write failing property test `prop_manifest_gen_strictly_increasing` in `crates/quanta-index-core/tests/property_history_governance.rs` (10k random gen sequences): assert any attempt to activate `manifest_gen' <= manifest_gen` returns `STATE_GENERATION_REGRESSION`.
2. Write failing property test `prop_sibling_gen_non_decreasing` (10k sequences): any sibling transitioning `non-NULL → NULL` or rolling back surfaces `STATE_GENERATION_REGRESSION`.
3. Implement `GenerationGovernance::activate` to satisfy both invariants.
4. Wire storage-layer assertion: no reader observes a `manifest_generation` whose sibling `MARKER_OK` is absent. Verified by an `H-LEX07-A` test that opens a partial generation (missing diff_hunks `MARKER_OK`) and asserts `STATE_NOT_READY: STALE_SIBLING`.

### 5.2 Step B — Stale-activation guard (TDD red)

1. Failing test `activation_regression_rejected`: a controller attempts to set `active_gen = G_old` after `G_old < current_active_gen`; assert `STATE_GENERATION_REGRESSION`.
2. Implement guard inside `GenerationGovernance::activate`.
3. Property test `prop_activation_idempotent`: re-activating the same `(manifest_gen, sibling_gen_set)` returns `ActivationOutcome::AlreadyActive` with no side-effect.

### 5.3 Step C — Write-packet trace + idempotency (TDD red)

The write packet is **constructed by the search plane from a decoded producer op**, not authored by the search plane. `applied_at_ms` is sourced from `CommitRecord.applied_at_ms` (producer), not stamped here.

1. Extend the `WritePacket` struct (lands as part of LEX-04 in Wave 3) with a `history_identities: Vec<HistoryIdentity>` field; hand-rolled serde per D18.
2. Failing test `apply_records_trace`: applying a delta (sourced from a decoded `UpsertCommit` channel event) produces a `TraceRecord { committer_id, applied_at_ms, before_gen, after_gen, hash }` where `hash = SHA-256(canonical_cbor(packet))` and `applied_at_ms` is the value carried by the producer commit op.
3. Failing test `reapply_same_packet_is_noop`: applying the same packet against the same `before_gen` returns `ApplyOutcome::Idempotent { matched_trace_id }` with no state change.
4. Implement `WritePacketTrace::record_history_apply` plus the no-op path.
5. Criterion bench `lex_07_write_trace_bench`: trace record p99 ≤ 1ms.

### 5.4 Step D — History sidecar shards land as first-class siblings (TDD red)

Sibling shards are populated **from the in-memory `CommitGraph` cache** (which is itself populated from producer ops in Step Pre-A). No source-bytes parsing, no git access.

1. Failing integration test `history_sidecar_atomicity`: build a generation with `commits/`, `diff_hunks/`, `commit_dag/` from a sequence of producer `UpsertCommit` ops; rename `commits/MARKER_OK` away; assert reader surface returns `STATE_NOT_READY: STALE_SIBLING`.
2. Implement `SearchPlaneHistoryIndexBuildPort::build_commit_metadata` / `build_diff_hunks` / `build_commit_dag` reading from `CommitGraph`.
3. Wire `MARKER_OK` per sibling.
4. Extend the per-generation reader cache pattern (LEX-03 sibling cache) for history shards.

### 5.5 Step E — `type:commit` / `type:diff` planner routing (TDD red)

1. Failing test per UC-HIST-01..08 in PRE-CONF.
2. Implement `LqHistoryPlanner::lower` covering `type:commit` and `type:diff`; route into `SearchPlaneHistoryIndexPort::query_commits` / `query_diffs`.
3. Negative tests:
   - `type:commit` + `match{}` → `PLAN_UNSUPPORTED_COMBO` (AC-09).
   - `type:diff` + `into:codeql` → `PLAN_UNSUPPORTED_COMBO` (AC-13).

### 5.6 Step F — `parent:` extension (TDD red)

1. Failing tests:
   - `parent_of_known_commit_returns_parents`: assert hits include the parent set of the named commit.
   - `parent_nonexistent_ref_fails_closed`: a nonexistent ref surfaces `HISTORY_REF_NOT_FOUND`.
   - `parent_depth_bounded`: a depth-bounded DAG walk (cap configurable; default `64`) at the cap surfaces `PLAN_LIMIT_EXCEEDED { dimension = parent-depth, limit = 64 }`.
2. Implement DAG walk from the commit-DAG cache; reuse the cache reader.

### 5.7 Step G — `merge:` extension with ambiguity lock (TDD red)

1. **Ambiguity lock** (per ticket guidance): `merge:` matches **only docs touched by the merge result** (the merge commit's own diff), **not** the docs touched on either merged-in side. Rationale: the merged-in side already appears via its own commits in the `revisions:` range, so counting it under `merge:` would double-attribute. This is the canonical semantic; any future widening requires an RFC amendment.
2. Failing tests:
   - `merge_yes_returns_merge_commits_only`: hits all have `is_merge == true` and the diff is the merge result, not the side branches.
   - `merge_no_excludes_merge_commits`.
   - `merge_only_returns_merge_commits_only` (alias-of-yes for the planner; documented).
3. Implement field filter against commit metadata `is_merge` plus diff hunk scoping.

### 5.8 Step H — `tag:` extension (TDD red)

1. Failing tests:
   - `tag_resolves_to_commit_set`: a tag pattern resolves through the tag→commit map to one or more commits and returns docs at those tag refs.
   - `tag_nonexistent_fails_closed`: a tag with no resolution surfaces `HISTORY_REF_NOT_FOUND`.
   - `tag_regex_bounded`: tag regex compile honors the RE2 NFA cap ([dsl.md § 3.4](../dsl.md)); over-cap surfaces `EXEC_REGEX_COMPILE_EXPLOSION`.
2. Implement tag→commit resolver from the commit-DAG cache.

### 5.9 Step I — `revisions:` extension with capacity cap (TDD red)

1. **Capacity proposal** (per ticket guidance): max **10,000 commits per query** for `revisions:<range>`. This is the proposed RFC § Capacity cap for the history planner; reconcile with [feature-scope.md § 7](../feature-scope.md) at the Wave-4 entry gate. Default value lands here as `HISTORY_REVISIONS_MAX = 10_000`; floor enforced at `1_000`.
2. Failing tests:
   - `revisions_two_dot_returns_range`: `a..b` returns commits reachable from `b` but not from `a`.
   - `revisions_three_dot_returns_symmetric_diff`: `a...b` returns symmetric difference per [dsl.md § 6.6](../dsl.md).
   - `revisions_overrun_fails_closed`: range size > cap surfaces `PLAN_LIMIT_EXCEEDED { dimension = history-revisions, limit = 10000 }`.
   - `revisions_either_endpoint_unknown`: surfaces `HISTORY_REF_NOT_FOUND`.
3. Implement DAG walk + range arithmetic.

### 5.10 Step J — `since.time:` extension tied to `applied_at_ms` (TDD red)

1. Failing tests:
   - `since_time_uses_applied_at_ms`: a commit whose `applied_at_ms < t` is excluded; one with `applied_at_ms >= t` is included; assertion holds even when wall-clock at request time differs from `applied_at_ms`.
   - `since_time_fail_closed_when_trace_missing`: a generation with no write-packet trace entry for the requested time surfaces `STATE_NOT_READY: HISTORY_TRACE_INCOMPLETE` (new typed code; see § 8).
   - `since_disambiguation`: `since:<token>` where token shape is RFC3339-or-duration lowers at parse time to `since.time:`; where token shape is a SHA or ref shape lowers to `since.commit:`. Closes [feature-scope.md § 9 Q2](../feature-scope.md).
2. Implement comparator + parse-time disambiguator (parser scope is LEX-01; LEX-07 owns the disambiguator inside the canonicalizer pass).

### 5.11 Step K — Predicate `repo:has.commit.after(...)` eval (TDD red)

1. Failing test UC-PRED-02 in PRE-CONF.
2. Implement eval against the commit metadata index (no request-time `git log`).
3. Negative test AC-08 (request-time gitserver call): assert ZERO process-spawn of `git` during the query path (instrumented by a CI fixture that fails on `git` subprocess invocation).

### 5.12 Step L — Defensive merge-cycle handler (TDD red)

1. Failing test `merge_cycle_typed_failure`: an artificially seeded commit-DAG cache with a back-edge cycle surfaces `HISTORY_MERGE_CYCLE` rather than infinite loop. (Real git DAGs are acyclic; this is a defense-in-depth assertion against corruption.)
2. Implement cycle detection in DAG walk (Tarjan or seen-set with bounded recursion).

### 5.13 Step M — Observability + audit (TDD green only)

1. Emit OpenTelemetry spans `lq.history.resolve_ref`, `lq.history.dag_walk`, `lq.history.commit_query`, `lq.history.diff_query` per [implementation-plan.md § 9.1 Wave-4 OBS subset](../implementation-plan.md).
2. Emit metrics `history.refs_resolved`, `history.dag_nodes_walked`, `history.revisions_in_range`, `history.applied_at_ms_compare_count`.
3. Audit row extended with `engine_routed = "history"` field (per OBS subset table).

### 5.14 Step N — Conformance corpus wiring

1. UC-HIST-01..08, UC-PRED-02, AC-08, AC-09, AC-13 all flip from `blocked` → `ok` / `error_expected` in PRE-CONF after this ticket lands.
2. Add new corpus rows for `parent:`, `merge:`, `tag:`, `revisions:`, `since.time:` under the UC-HIST-* series (proposal: UC-HIST-09 through UC-HIST-13). Authoring discipline per [usecase.md § 6](../usecase.md): each new row commits with the LEX-07 PR.

### 5.15 Step O — Cutover

1. Bump contract: `LQ/Core-1.0 → LQ/History-1.1` per [implementation-plan.md § 7.1 Wave 4 row](../implementation-plan.md) (additive minor).
2. Producer handoff doc commits in the same PR.
3. No `#[deprecated]` shim; per CLAUDE.md breaking-first posture, any legacy code path that referenced ad-hoc `git log` is **removed**, not stubbed.

---

## 6. Test plan

### 6.1 Unit (`cargo test --workspace`)

| Test                                              | Asserts                                                                          |
| ------------------------------------------------- | -------------------------------------------------------------------------------- |
| `governance::activation_regression_rejected`      | `STATE_GENERATION_REGRESSION` on manifest_gen rollback                           |
| `governance::sibling_rollback_rejected`           | `STATE_GENERATION_REGRESSION` on non-NULL → NULL transition                      |
| `governance::activation_idempotent`               | re-activate same set → `AlreadyActive`, no state delta                           |
| `trace::apply_records_hash_and_gens`              | trace carries `committer_id`, `applied_at_ms`, `before_gen`, `after_gen`, `hash` |
| `trace::reapply_same_packet_is_noop`              | idempotent re-apply matches existing trace id                                    |
| `history::parent_returns_parents`                 | `parent:<commit>` returns parent set                                             |
| `history::parent_unknown_ref_fails`               | `HISTORY_REF_NOT_FOUND`                                                          |
| `history::parent_depth_bounded`                   | `PLAN_LIMIT_EXCEEDED { dimension = parent-depth }`                               |
| `history::merge_yes_filters_to_merge_commits`     | only `is_merge == true` rows                                                     |
| `history::merge_scope_is_merge_result_only`       | merged-in side commits excluded (ambiguity lock)                                 |
| `history::tag_resolves`                           | tag pattern → commit set                                                         |
| `history::tag_unknown_fails`                      | `HISTORY_REF_NOT_FOUND`                                                          |
| `history::revisions_two_dot`                      | two-dot range arithmetic                                                         |
| `history::revisions_three_dot`                    | three-dot symmetric diff                                                         |
| `history::revisions_overrun`                      | `PLAN_LIMIT_EXCEEDED { dimension = history-revisions, limit = 10000 }`           |
| `history::since_time_uses_applied_at_ms`          | comparator runs against trace, not wall-clock                                    |
| `history::since_disambiguation_at_parse`          | `since:` → `since.time:` or `since.commit:` per token shape                      |
| `history::merge_cycle_detected`                   | `HISTORY_MERGE_CYCLE` on seeded cycle                                            |
| `planner::type_commit_plus_match_block`           | `PLAN_UNSUPPORTED_COMBO`                                                         |
| `planner::type_diff_plus_into_codeql`             | `PLAN_UNSUPPORTED_COMBO`                                                         |

### 6.2 Integration

| Test                                       | Asserts                                                                                                                       |
| ------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------- |
| `history_sidecar_atomicity`                | reader returns `STATE_NOT_READY: STALE_SIBLING` when any history-sibling `MARKER_OK` is absent                                |
| `history_no_git_subprocess`                | CI fixture asserts ZERO `git` subprocess invocation across the entire query path (AC-08)                                      |
| `history_predicate_repo_has_commit_after`  | UC-PRED-02 end-to-end: predicate resolves from commit metadata index, no request-time git scan                                |
| `history_writepacket_trace_cross_instance` | trace records on instance A reproduce byte-identically on instance B for the same applied delta (extends LEX-05 reproducibility) |
| `history_idempotent_reapply`               | reapplying the same packet across two instances converges to the same `manifest_gen` + `TraceRecord`                          |

### 6.3 Property

| Test                                    | Cases | Asserts                                                                                          |
| --------------------------------------- | ----- | ------------------------------------------------------------------------------------------------ |
| `prop_manifest_gen_strictly_increasing` | 10k   | randomized activation sequences never observe a rollback                                         |
| `prop_sibling_gen_non_decreasing`       | 10k   | randomized sibling-gen transitions never go non-NULL → NULL                                      |
| `prop_activation_idempotent`            | 10k   | duplicate activation is a typed no-op                                                            |
| `prop_history_dag_walk_bounded`         | 1k    | random DAGs (depth ≤ 64) terminate inside the depth cap; deeper DAGs surface `PLAN_LIMIT_EXCEEDED` |
| `prop_revisions_range_bounded`          | 1k    | random ranges of size > 10k surface `PLAN_LIMIT_EXCEEDED`                                        |
| `prop_since_time_monotone`              | 1k    | for any `t1 < t2`, the result set of `since.time:t2` is a subset of `since.time:t1`              |

### 6.4 Loom

| Test                            | Asserts                                                                                                                    |
| ------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `loom_activation_race`          | concurrent `GenerationGovernance::activate` from two writers serializes (the second observes `AlreadyActive` or regression error) |
| `loom_trace_idempotent_under_race` | concurrent re-apply of the same packet converges to one trace id                                                          |

### 6.5 Criterion

| Bench                            | Budget                                                              |
| -------------------------------- | ------------------------------------------------------------------- |
| `lex_07_write_trace_bench`       | trace record p99 ≤ 1ms                                              |
| `lex_07_history_query_bench`     | UC-HIST-01..08 each p99 ≤ 250 ms (warm-cache Wave-4 SLO)            |
| `lex_07_dag_walk_bench`          | depth-64 walk on a 100k-commit DAG p99 ≤ 100 ms                     |
| `lex_07_revisions_range_bench`   | 10k-commit range resolution p99 ≤ 200 ms                            |

### 6.6 Conformance (PRE-CONF)

| Row                              | After LEX-07 |
| -------------------------------- | ------------ |
| UC-HIST-01..08                   | `ok`         |
| UC-PRED-02                       | `ok`         |
| UC-HIST-09..13 (new: `parent:` / `merge:` / `tag:` / `revisions:` / `since.time:`) | `ok` |
| AC-08 (request-time gitserver)   | `error_expected: PLAN_ERROR`  |
| AC-09 (`type:commit` + `match{}`) | `error_expected: PLAN_UNSUPPORTED_COMBO` |
| AC-13 (`type:diff` + `into:codeql`) | `error_expected: PLAN_UNSUPPORTED_COMBO` |

---

## 7. Observability

OpenTelemetry spans (additive on top of Wave-3 tree):

- `lq.history.resolve_ref` — attributes `{ref_kind, repo_id, manifest_generation, resolved: bool}`
- `lq.history.dag_walk` — attributes `{repo_id, start_commit, depth_cap, nodes_walked, terminated_at_cap: bool}`
- `lq.history.commit_query` — attributes `{repo_id, manifest_generation, hits, applied_at_ms_floor, applied_at_ms_ceil}`
- `lq.history.diff_query` — attributes `{repo_id, manifest_generation, hits, side: added|removed|touched}`
- `lq.governance.activate` — attributes `{repo_id, manifest_generation_before, manifest_generation_after, outcome: activated|already_active|regression}`
- `lq.governance.write_trace` — attributes `{committer_id, before_gen, after_gen, hash, outcome: applied|idempotent}`

Metrics (closed label set per [rfc.md § Metric schema](../rfc.md)):

- `history.refs_resolved{repo_id, ref_kind}` (count)
- `history.dag_nodes_walked{repo_id}` (count)
- `history.revisions_in_range{repo_id}` (count)
- `history.applied_at_ms_compare_count{repo_id}` (count)
- `governance.activation_regressions{repo_id}` (count)
- `governance.idempotent_reapplies{repo_id}` (count)

Audit row extension: `engine_routed = "history"` and `history_ref_kind = Branch|Tag|Sha|Range` fields land in the audit envelope (still one log line per request per [rfc.md § Audit trail](../rfc.md)).

Cardinality budget: `ref_kind ∈ {Branch, Tag, Sha, Range}` (4 values), `outcome` enums bounded by typed payload. New labels MUST NOT include unbounded values like commit_id or tag string.

---

## 8. Error scenarios

All paths fail closed; no silent fallback. New typed codes introduced by this ticket:

| Code                              | When fires                                                                                                | Family       | Retry semantics |
| --------------------------------- | --------------------------------------------------------------------------------------------------------- | ------------ | --------------- |
| `HISTORY_COMMIT_DECODE_FAIL`      | producer-emitted `UpsertCommit` carries a malformed `CommitRecord` payload (CBOR decode failure, missing required field, invalid sha shape) | `STATE_*`    | not retryable   |
| `HISTORY_REF_DECODE_FAIL`         | producer-emitted `UpsertRef` / `UpsertTag` / `DeleteRef` / `DeleteTag` carries a malformed payload (invalid name encoding, invalid sha shape) | `STATE_*`    | not retryable   |
| `HISTORY_COMMIT_PARENT_UNKNOWN`   | producer emits an `UpsertCommit` whose `parents` reference a sha not yet present in `CommitGraph` (order-dependence violation between producer emission order and search-plane cache state) | `STATE_*`    | not retryable (default) — see Q-LEX07-10 for buffer-vs-reject |
| `HISTORY_REF_NOT_FOUND`           | named ref (branch, tag, sha) does not resolve in the `CommitGraph` cache for the pinned generation        | `STATE_*`    | not retryable   |
| `HISTORY_RANGE_OVERRUN`           | `revisions:<range>` exceeds the `HISTORY_REVISIONS_MAX = 10_000` cap                                      | `PLAN_*`     | not retryable   |
| `HISTORY_MERGE_CYCLE`             | `CommitGraph` cache returns a cycle on walk (defense-in-depth; real DAGs from a non-corrupt producer are acyclic) | `STATE_*`    | not retryable   |
| `HISTORY_TRACE_INCOMPLETE`        | `since.time:` queried against a generation whose write-packet trace lacks an entry for the requested time | `STATE_*`    | wait-and-retry  |
| `HISTORY_UNINDEXED`               | `type:commit` / `type:diff` queried against a `(repo, rev)` whose history shards are absent               | `STATE_*`    | wait-and-retry  |

Reused codes from the RFC taxonomy:

- `STATE_GENERATION_REGRESSION` — monotonicity violation on activation.
- `STATE_NOT_READY: STALE_SIBLING` — history sibling `MARKER_OK` absent under a published manifest.
- `EXEC_CATALOG_MISS` — `(repo, rev)` not in catalog at history-query time.
- `PLAN_LIMIT_EXCEEDED` — depth cap (parent), range cap (revisions), regex cap (tag pattern), trace-size cap.
- `PLAN_UNSUPPORTED_COMBO` — `type:commit` + `match{}`, `type:diff` + `into:codeql`, `type:diff` + `tag:` (the diff projection has no tag projection — locked here as unsupported).
- `EXEC_REGEX_COMPILE_EXPLOSION` — tag-pattern regex NFA over cap.

Anti-usecase mappings:

- AC-08 (request-time gitserver call) → `PLAN_ERROR` family, specifically `HISTORY_UNINDEXED` when the predicate cannot be answered from the indexed catalog.
- AC-09 (`type:commit` + `match{}`) → `PLAN_UNSUPPORTED_COMBO`.
- AC-13 (`type:diff` + `into:codeql`) → `PLAN_UNSUPPORTED_COMBO`.

Every failure path carries a typed payload per [rfc.md § Error Code Taxonomy](../rfc.md) and [dsl.md § 12 Error taxonomy](../dsl.md). No free-text-only error response is permitted.

---

## 9. Performance envelope

Targets are per-instance, single-node. Aligns with [rfc.md § Capacity and SLO Targets](../rfc.md) and [feature-scope.md § 7](../feature-scope.md). RFC-GAP-4 is acknowledged: SLOs for the history sublayer are extended here as a proposal pending RFC § Latency SLO amendment.

| Workload                                                      | p50      | p95      | p99      | Source        |
| ------------------------------------------------------------- | -------- | -------- | -------- | ------------- |
| single-repo `type:commit` query                               | < 50 ms  | < 250 ms | < 1 s    | LEX-07 (RFC-GAP-4 extension) |
| single-repo `type:diff` query                                 | < 75 ms  | < 300 ms | < 1.5 s  | LEX-07 (RFC-GAP-4 extension) |
| `parent:` walk (depth ≤ 64)                                   | < 25 ms  | < 100 ms | < 250 ms | LEX-07 (RFC-GAP-4 extension) |
| `revisions:<range>` resolution (≤ 10k commits)                | < 75 ms  | < 200 ms | < 500 ms | LEX-07 (RFC-GAP-4 extension) |
| `tag:` resolution                                             | < 20 ms  | < 80 ms  | < 200 ms | LEX-07 (RFC-GAP-4 extension) |
| `since.time:` comparator (against trace)                      | < 10 ms  | < 30 ms  | < 100 ms | LEX-07 (RFC-GAP-4 extension) |
| 100-repo fanout history query                                 | —        | < 2 s    | —        | [rfc.md § Latency SLOs](../rfc.md) (inherited) |
| `GenerationGovernance::activate` (incl. monotonicity check)   | < 5 ms   | < 20 ms  | < 50 ms  | LEX-07 (RFC-GAP-4 extension) |
| `WritePacketTrace::record_history_apply`                      | < 1 ms   | < 3 ms   | < 10 ms  | LEX-07 (RFC-GAP-4 extension) |

Storage budget (per [implementation-plan.md § 4.5 Risks](../implementation-plan.md) R3):

- commit metadata index ≤ 30% of content shard size per generation;
- diff hunk index ≤ 2× content shard size per generation (positions only on diff text fields per [implementation-plan.md § 4.5 mitigation](../implementation-plan.md));
- commit-DAG cache ≤ 5% of content shard size per generation;
- retention per [rfc.md § Index Lifecycle § Retention](../rfc.md): N most-recent generations per `(repo, rev)`; N configurable; vacuum gated by active-reader presence.

Capacity:

- `HISTORY_REVISIONS_MAX = 10_000` (configurable; floor `1_000`).
- `HISTORY_PARENT_DEPTH_MAX = 64` (configurable; floor `8`).
- `HISTORY_TAG_REGEX_NFA_MAX = 100_000` (mirrors RE2 cap in [dsl.md § 3.4](../dsl.md)).

---

## 10. Risks

| ID    | Description                                                                                                              | Prob | Impact | Mitigation                                                                                                                       |
| ----- | ------------------------------------------------------------------------------------------------------------------------ | ---- | ------ | -------------------------------------------------------------------------------------------------------------------------------- |
| LR-1  | History storage growth dominates per-generation footprint (esp. diff hunk index)                                         | M    | H      | retention cap per [rfc.md § Index Lifecycle](../rfc.md); operator-tunable N; OBS-01 disk metric extends with `engine = history` |
| LR-2  | Adversarial `revisions:<range>` DoS — request a 1B-commit range                                                          | M    | H      | hard cap `HISTORY_REVISIONS_MAX = 10_000`; `PLAN_LIMIT_EXCEEDED` typed error                                                     |
| LR-3  | Adversarial `parent:` depth walk DoS                                                                                     | L    | M      | hard cap `HISTORY_PARENT_DEPTH_MAX = 64`; bounded recursion with seen-set                                                        |
| LR-4  | Tag pattern regex explosion                                                                                              | L    | H      | RE2 NFA cap `HISTORY_TAG_REGEX_NFA_MAX = 100_000` per [dsl.md § 3.4](../dsl.md); `EXEC_REGEX_COMPILE_EXPLOSION`                  |
| LR-5  | History sidecar sharing the content shard's reader cache → cross-engine cache eviction                                   | M    | M      | per-sibling cache key (sibling kind in the cache key); criterion bench guards cache-hit-rate                                     |
| LR-6  | `since.time:` truth source drift if write-packet trace truncates                                                          | M    | H      | `HISTORY_TRACE_INCOMPLETE` typed error; trace retention pinned to ≥ retention of the corresponding manifest generation           |
| LR-7  | G-CONTROL-LOC ([implementation-plan.md § 2.3a](../implementation-plan.md)) — physical home of governance state unresolved | H    | H      | LEX-07 is path-agnostic; ADR-001 / ADR-011 family resolves before Wave-3 (LEX-04) per [implementation-plan.md § 11](../implementation-plan.md) |
| LR-8  | Merge ambiguity decision (merge-result-only vs include-merged-in-side) leaks into other consumers                        | L    | M      | ambiguity lock recorded in § 5.7; documented in producer handoff doc                                                             |
| LR-9  | Defensive cycle detection masks a real DAG corruption                                                                    | L    | M      | `HISTORY_MERGE_CYCLE` is a typed alarm, not a silent skip; alerted at OBS-01 audit                                               |
| LR-10 | `ADR-011` (new `quanta-index-history` crate vs extend `quanta-index-lexical`) unresolved at ticket start                 | H    | M      | wave entry gate blocks on ADR-011 per [implementation-plan.md § 10](../implementation-plan.md)                                   |
| LR-11 | Storage sharing question: history shard alongside main lexical shard or separate? Tie to candidate ADR-005 (see § 12)    | M    | M      | open question recorded in § 12; ADR-005 candidate; default = separate shard per LR-5 mitigation                                  |
| LR-12 | Producer commit-stream ordering: parents may arrive after children. Producer's emission order is not guaranteed to be topological in the current [channel-architecture.md § 3.1](../../../ssot/channel-architecture.md) draft | M    | M      | default policy = reject with `HISTORY_COMMIT_PARENT_UNKNOWN`; bounded buffer option locked behind Q-LEX07-10 pending producer ordering guarantee in the cross-team handoff doc |
| LR-13 | Producer commit payload schema drift: `CommitRecord` wire shape evolves without `lq_version` bump                         | M    | H      | hand-rolled serde rejects unknown required fields; `HISTORY_COMMIT_DECODE_FAIL` typed error; wire-shape versioning policy is Q-LEX07-11 |

---

## 11. Definition of Done (provable)

Every row cites the artifact that proves it. If any row cannot cite an artifact, the wave-4 exit gate stays `blocked` per [CLAUDE.md § Claude Supplements](../../../../CLAUDE.md). All 23 active rows shipped (76 tests in `quanta-index-lq-history`); the architecture-corrected ingress (channel-driven `UpsertCommit/UpsertRef/UpsertTag` callbacks) is load-bearing for rows 1, 4, 5, 7, 8, 9, 10, 15, 17, and 23. Row 22 is deferred (producer handoff SSOT is `docs/ssot/producer-handoff.md`; a `docs/handoffs/lq-history-1.1.md` cut is the follow-up).

1. ✓ shipped (architecture-corrected) — **Manifest atomicity contract enforced** — proven by `governance::activation_regression_rejected`, `prop_manifest_gen_strictly_increasing`, `prop_sibling_gen_non_decreasing`, `history_sidecar_atomicity` (§ 6.1, § 6.2, § 6.3).
2. ✓ shipped — **Strict monotonic activation** — proven by `prop_manifest_gen_strictly_increasing` and `prop_sibling_gen_non_decreasing` (§ 6.3).
3. ✓ shipped — **Stale-activation guard** — proven by `governance::activation_regression_rejected` (§ 6.1).
4. ✓ shipped (architecture-corrected) — **Write-packet trace records correct fields** — proven by `trace::apply_records_hash_and_gens` (§ 6.1) asserting `committer_id`, `applied_at_ms`, `before_gen`, `after_gen`, `hash` are present and `hash = SHA-256(canonical_cbor(packet))`. The producer authors `applied_at_ms`; the search plane records it.
5. ✓ shipped (architecture-corrected) — **Idempotency** — proven by `trace::reapply_same_packet_is_noop` (§ 6.1) and `loom_trace_idempotent_under_race` (§ 6.4). Idempotency is keyed on `(channel_seq, op_kind, ref_id)`.
6. ✓ shipped — **History sidecar siblings are first-class** — proven by `history_sidecar_atomicity` (§ 6.2): a reader observes `STATE_NOT_READY: STALE_SIBLING` when any of `commits/MARKER_OK`, `diff_hunks/MARKER_OK`, `commit_dag/MARKER_OK` is absent.
7. ✓ shipped (architecture-corrected) — **`parent:` extension** — proven by `history::parent_returns_parents`, `history::parent_unknown_ref_fails`, `history::parent_depth_bounded` (§ 6.1) plus `prop_history_dag_walk_bounded` (§ 6.3). DAG is built from producer-shipped `CommitRecord.parents`.
8. ✓ shipped (architecture-corrected) — **`merge:` extension with ambiguity lock** — proven by `history::merge_yes_filters_to_merge_commits` and `history::merge_scope_is_merge_result_only` (§ 6.1).
9. ✓ shipped (architecture-corrected) — **`tag:` extension** — proven by `history::tag_resolves` and `history::tag_unknown_fails` (§ 6.1). Tag pointers arrive via `UpsertTag` / `DeleteTag`.
10. ✓ shipped (architecture-corrected) — **`revisions:` extension with 10k-commit cap** — proven by `history::revisions_two_dot`, `history::revisions_three_dot`, `history::revisions_overrun` (§ 6.1) plus `prop_revisions_range_bounded` (§ 6.3).
11. ✓ shipped — **`since.time:` tied to write-packet trace** — proven by `history::since_time_uses_applied_at_ms` and `prop_since_time_monotone` (§ 6.3).
12. ✓ shipped — **`since:` disambiguation closes Q2** — proven by `history::since_disambiguation_at_parse` (§ 6.1) covering both RFC3339 / duration → `since.time:` and SHA / ref → `since.commit:`.
13. ✓ shipped — **Defensive merge-cycle handler** — proven by `history::merge_cycle_detected` (§ 6.1).
14. ✓ shipped — **`type:commit` / `type:diff` planner routing** — proven by UC-HIST-01..08 + UC-PRED-02 green in PRE-CONF (§ 6.6).
15. ✓ shipped (architecture-corrected) — **No request-time `git log`** — proven by `history_no_git_subprocess` (§ 6.2) asserting zero `git` subprocess invocations during the query path. The producer is the sole git surface.
16. ✓ shipped — **AC-08 / AC-09 / AC-13 fail with typed errors** — proven by PRE-CONF rows (§ 6.6).
17. ✓ shipped (architecture-corrected) — **Cross-instance reproducibility for history applies** — proven by `history_writepacket_trace_cross_instance` (§ 6.2) — same channel-op stream on instance A and instance B produces byte-identical trace records.
18. ✓ shipped — **Observability spans + metrics emit** — proven by [implementation-plan.md § 9.1 Wave-4 OBS subset](../implementation-plan.md) compliance test; new spans visible in OBS-01 capture.
19. ✓ shipped — **Performance envelope met** — proven by `lex_07_write_trace_bench`, `lex_07_history_query_bench`, `lex_07_dag_walk_bench`, `lex_07_revisions_range_bench` p99 targets (§ 6.5, § 9).
20. ✓ shipped — **RFC Claim Discipline §3 (history search)** provable — citing UC-HIST-01..08 corpus green + history index proven non-empty.
21. ✓ shipped — **No serde proc-macro derives** — proven by `semgrep rust-no-serde-derive` green ([tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml)).
22. 🔜 deferred (see §12) — **Producer handoff** — `docs/handoffs/lq-history-1.1.md` to be cut from the canonical [docs/ssot/producer-handoff.md](../../../ssot/producer-handoff.md) SSOT.
23. ✓ shipped (architecture-corrected) — **Wave-4 contract bump** — `LQ/Core-1.0 → LQ/History-1.1` recorded in `lq_version` field on the canonical AST per [implementation-plan.md § 7.1 Wave 4 row](../implementation-plan.md); no `#[deprecated]` shim. ADR-017 (`channel seq supersedes advisory lock`) withdrew the standalone advisory-lock ADR.
24. ✓ shipped — **CI rails green** — `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `cargo deny`, `cargo machete`, `python3 tools/ci/lint/lint-doc-paths.py`, `ci/lq-conformance` (PRE-CONF) all green.

---

## 12. Open questions

- **Q-LEX07-1** — History shard sharing posture: does the history sidecar share storage with the main lexical shard (`crates/quanta-index-lexical/`) or live in a separate crate (`crates/quanta-index-history/`)? **Default proposal: separate**, per LR-5 mitigation (per-sibling cache key) and ADR-011 ([implementation-plan.md § 10](../implementation-plan.md)). Tie to **ADR-005 candidate** (new) — "History sibling storage layout"; resolution gates the wave-4 entry gate.
- **Q-LEX07-2** — `merge:` ambiguity lock alternate semantics: the spec locks "merge-result-only" (§ 5.7). If a future RFC amendment widens this to include the merged-in side, the AST representation must distinguish — propose a new `merge.side:included|excluded` filter at that time. **Not in LEX-07 scope.**
- **Q-LEX07-3** — `HISTORY_REVISIONS_MAX` reconciliation: this ticket proposes `10_000`. [feature-scope.md § 7](../feature-scope.md) lists `max candidate set size (count:all) = 100_000` — different dimension but the operator may expect symmetry. RFC § Capacity should ratify a single number across both dimensions.
- **Q-LEX07-4** — `HISTORY_TRACE_INCOMPLETE` (new code in § 8) — should this surface as a `wait-and-retry` (proposed) or as a hard `not retryable` failure? Default proposal: `wait-and-retry`, on the theory that trace backfill happens asynchronously after the manifest activation. Confirm with OBS-01 operator playbook.
- **Q-LEX07-5** — `merge:only` aliasing — is this a UI/parser convenience or a distinct planner mode? Spec defaults to "alias of `merge:yes`" in § 5.7. RFC § Compatibility Rules § Sourcegraph compat may force a distinct mode; revisit if Sourcegraph's `merge:only` semantics drift.
- **Q-LEX07-6** — `parent:` semantics under a merge commit: the merge commit has multiple parents. `parent:<merge_commit>` returns the parent set (multiple commits). Is this the expected projection, or should we expose a `parent:<merge_commit>.first` / `parent:<merge_commit>.second` selector? Default: return all parents; selectors deferred.
- **Q-LEX07-7** — G-CONTROL-LOC ([implementation-plan.md § 2.3a](../implementation-plan.md)) gates the physical home of `GenerationGovernance` and `WritePacketTrace`. This ticket is path-agnostic; the resolution may force a follow-up PR re-homing the surfaces. The follow-up is not a re-spec — only a path move — but it must be tracked as a Wave-3-entry blocker (LEX-04 owns the writer-coordinator skeleton).
- **Q-LEX07-8** — Hybrid history+semantic query routing (e.g., `type:diff` + semantic similarity) — out of scope here, lands in SEM-01 (Wave 6) per [implementation-plan.md § 4.7](../implementation-plan.md).
- **Q-LEX07-9** — Should `revisions:<range>` honor `tag:<pattern>` as a range endpoint (e.g., `revisions:v1.0..v2.0`)? Default: yes, via tag→commit resolution at parse-canonicalize time. Tag-unknown at either endpoint surfaces `HISTORY_REF_NOT_FOUND`. Confirm with conformance corpus author.
- **Q-LEX07-10** — Producer emission ordering guarantee for `UpsertCommit`: does the producer guarantee topological order (parents before children) within a `(repo, revision, generation)` window? Default proposal: **reject with `HISTORY_COMMIT_PARENT_UNKNOWN`** on out-of-order arrival; bounded buffer of N entries is an alternative if the producer cannot give a topological guarantee. Lock before LEX-07 PR merges; resolution lives in `docs/handoffs/lq-history-1.1.md`.
- **Q-LEX07-11** — `CommitRecord` wire-shape versioning policy: when the producer adds / removes fields on `CommitRecord`, what is the version bump rule? Default proposal: additive fields bump the channel-op minor version; removed / semantically-changed fields bump major and require a full-bundle reseed. Reconcile with [channel-architecture.md § 3.1](../../../ssot/channel-architecture.md) before the producer ships `UpsertCommit` outside the proposed set.
- **Q-LEX07-12** — Need for `DeleteCommit` op: producers rarely delete history, but force-push and history rewrite are real. Default proposal: **no `DeleteCommit` op in LEX-07**; rewrites are handled by emitting a new generation with a fresh `FullBundle` and letting retention vacuum the prior generation per [channel-architecture.md § 5.2](../../../ssot/channel-architecture.md). Confirm the producer team agrees that force-push triggers a fresh generation rather than per-commit deletion.

---

## 13. References

- [rfc.md § LQ/History-1.1](../rfc.md)
- [rfc.md § Canonical Incremental Write Pipeline](../rfc.md)
- [rfc.md § Atomicity contract](../rfc.md)
- [rfc.md § Generation model](../rfc.md)
- [rfc.md § Monotonicity rules](../rfc.md)
- [rfc.md § Index Lifecycle](../rfc.md)
- [rfc.md § Migration and Versioning Policy](../rfc.md)
- [rfc.md § Error Code Taxonomy](../rfc.md)
- [rfc.md § Merge determinism rule](../rfc.md)
- [rfc.md § Claim Discipline](../rfc.md)
- [rfc.md § Capacity and SLO Targets](../rfc.md)
- [feature-scope.md § 1.2 LQ/History-1.1](../feature-scope.md)
- [feature-scope.md § 1.2.4 RFC-omitted history filters](../feature-scope.md)
- [feature-scope.md § 4.2 History-1.1](../feature-scope.md)
- [feature-scope.md § 4.7 Flagged gaps](../feature-scope.md)
- [feature-scope.md § 6.2 History-1.1 authority chain](../feature-scope.md)
- [feature-scope.md § 7 Scale & capacity scope](../feature-scope.md)
- [feature-scope.md § 9 Open questions Q2](../feature-scope.md)
- [usecase.md § D History (UC-HIST-01..08)](../usecase.md)
- [usecase.md § B Predicate filters UC-PRED-02](../usecase.md)
- [usecase.md § H UC-EDGE-07](../usecase.md)
- [usecase.md § 3 GAP-02](../usecase.md)
- [usecase.md § 4 AC-08, AC-09, AC-13](../usecase.md)
- [dsl.md § 2.2 LQ/History-1.1 extensions](../dsl.md)
- [dsl.md § 6.6 rev: grammar](../dsl.md)
- [dsl.md § 7 Predicate filter sub-grammar](../dsl.md)
- [dsl.md § 12 Error taxonomy](../dsl.md)
- [dsl.md § 13 Limits and budgets](../dsl.md)
- [implementation-plan.md § 4.5 Wave 4](../implementation-plan.md)
- [implementation-plan.md § 5.11 LEX-07 DoD](../implementation-plan.md)
- [implementation-plan.md § Appendix A.1 RFC-GAP-1](../implementation-plan.md)
- [implementation-plan.md § 7 Cutover and migration plan](../implementation-plan.md)
- [implementation-plan.md § 9.1 Wave-4 OBS subset](../implementation-plan.md)
- [implementation-plan.md § 10 ADR-011](../implementation-plan.md)
- [implementation-plan.md § 11 Open questions G-CONTROL-LOC](../implementation-plan.md)
- [channel-architecture.md § 0 Scope](../../../ssot/channel-architecture.md) — search plane is index + query only; producer authors commit/ref/tag
- [channel-architecture.md § 3.1 Channel ops + authorship rule](../../../ssot/channel-architecture.md) — authoritative input source for LEX-07; `UpsertCommit`/`UpsertRef`/`UpsertTag`/`DeleteRef`/`DeleteTag` proposed-status ops
- [channel-architecture.md § 5.2 Generation state authority](../../../ssot/channel-architecture.md) — ledger reconstructed from channel events; no SQLite control plane
- [CLAUDE.md § Agent change posture](../../../../CLAUDE.md)
- [CLAUDE.md § Rule Catalog](../../../../CLAUDE.md)
- [AGENTS.md](../../../../AGENTS.md)
- [tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive` (D18 enforcement)
- [tools/ci/lint/lint-doc-paths.py](../../../../tools/ci/lint/lint-doc-paths.py) — markdown link integrity
- [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) — structured agent output schema
- [docs/ssot/producer-handoff.md](../../../ssot/producer-handoff.md) — producer handoff SSOT (authoritative wire shape for `UpsertCommit/UpsertRef/UpsertTag`; delta-handling identity / cascade / replay contract in §3.5; force-push handling in §3.1.3)
- [INDEX.md](INDEX.md) — ticket index (architecture correction context: §3.6 producer-authorship correction, §3.8 stale-references follow-up)
