# RFC-LEX-05 — Parallel executor and deterministic merge (roll-up)

| field | value |
|---|---|
| Kind | roll-up spec (RFC `LEX-05` ticket roll-up) |
| Status | partially shipped — ranker tiebreak frozen; fanout dispatcher + cross-shard parallel merge open |
| Owner crates | [`quanta-index-lq-ranker`](../../../../crates/quanta-index-lq-ranker), [`quanta-index-lexical`](../../../../crates/quanta-index-lexical), [`quanta-index-search-plane`](../../../../crates/quanta-index-search-plane) |
| Constituent specs | [LEX-06](LEX-06.md) (ranker tiebreak), [OBS-01](OBS-01.md) (fanout instrumentation) |
| Last verified | 2026-05-25 |

> Roll-up bookkeeping. RFC `LEX-05` "parallel executor and deterministic merge" splits across (a) the per-shard ranker tiebreak frozen by LEX-06 and (b) the cross-shard parallel fanout dispatcher + deterministic merge that is not yet shipped.

---

## §1 Purpose

RFC [`LEX-05` "parallel executor and deterministic merge"](../rfc.md#ticket-pack) names the **fanout planner**: each lexical query fans out across N repos × M shards in parallel, then merges into one deterministic top-k. The fanout has a bounded budget (timeout / cancel / SLO); the merge is byte-identical across instances at fixed inputs.

This roll-up is satisfied when:

1. A deterministic merge tuple is frozen and applied uniformly. (Shipped via LEX-06.)
2. A fanout dispatcher schedules per-shard execution in parallel under an SLO-bounded budget with typed cancel.
3. The merge step is order-preserving under fixed shard outputs (no parallelization-induced reordering).

(1) is **shipped**. (2) and (3) are the **residual gap**.

---

## §2 Background

The RFC ticket pack lists `LEX-05` as the "parallel executor and deterministic merge" — distinct from `LEX-03` "lexical authority unification" (sequential composition; see [RFC-LEX-03](RFC-LEX-03.md)). The differences:

| Concern | RFC-LEX-03 (engine) | RFC-LEX-05 (parallel exec) |
|---|---|---|
| Shards composed | per-engine (trigram + positions + regex + symbol + scorer) | per-repo × per-shard |
| Concurrency | sequential | parallel |
| Determinism | tiebreak tuple at single-instance level | tiebreak tuple + byte-identical merge across parallel orderings |
| Budget | engine-level p99 | global p99 minus front-door overhead |

[LEX-06](LEX-06.md) §1 froze the 6-component tiebreak tuple `(score, repo, gen, path, line, doc_id)`. The fanout dispatcher consumes that tuple as its merge key but does not yet exist as a named subsystem.

Producer-authorship correction ([INDEX.md §3.6](INDEX.md)) applies: the fanout never reaches across producer/search-side authorship. All per-shard authorities decode producer-emitted records.

Persona gating: same as RFC-LEX-03 — P6 baseline corpus is the gate, in particular `UC-LEX-20` (`timeout:5s`) which asserts fail-closed timeout (no partial-result silent fallback).

---

## §3 Inputs

Subsystem specs:

- [LEX-06](LEX-06.md) — composite ranker with frozen-per-gen weights + 6-component tiebreak tuple.
- [LEX-00..05](LEX-00.md) — per-shard authorities (consumed by the fanout; see [RFC-LEX-03 §3](RFC-LEX-03.md)).
- [OBS-01](OBS-01.md) §4.1 — fanout span schema (`lq.exec.fanout`, `lq.exec.shard{shard_id}`, `lq.merge`).
- [RT-01](RT-01.md) — runtime metadata catalog for predicate-pushdown pre-fanout.

Channel ops (read at activation time, not in the request path):

- `Seal { generation }` per [channel-architecture.md §3.1](../../../ssot/channel-architecture.md) — gen alignment authority.

---

## §4 Deliverables

### 4.1 Shipped (constituent specs)

| Item | Crate / spec | Status |
|---|---|---|
| 6-component tiebreak tuple `(score, repo, gen, path, line, doc_id)` | [LEX-06 §1](LEX-06.md), [`quanta-index-lq-ranker`](../../../../crates/quanta-index-lq-ranker) | shipped |
| Per-shard ranker stability | LEX-06 | shipped |
| Frozen-per-gen weights (`weights_hash` lock) | LEX-06 | shipped |
| OBS-01 fanout span schema | [OBS-01 §4.1](OBS-01.md) | shipped |

### 4.2 Residual gap (this roll-up)

| Gap | What it is | Owner |
|---|---|---|
| Fanout dispatcher | Schedules per-repo × per-shard execution in parallel; SLO-bounded; typed cancel | new — `quanta-index-search-plane::fanout` (proposed) |
| Bounded parallelism cap | Max concurrent shards per query; max admission queue depth; fail-closed on overflow | new |
| Deterministic merge across parallel shard orderings | k-way merge consuming per-shard ordered streams; output is byte-identical to sequential merge under fixed inputs | new |
| Per-shard timeout slicing | Each shard receives `(query_budget / shard_count)` budget with typed `EXEC_SHARD_TIMEOUT` on exceed | new |
| Cancel propagation | Engine-level cancel → all in-flight shards observe `EXEC_SHARD_CANCELLED` and unwind | new |
| Partial-result discipline | NO silent partial result on timeout; `TIMEOUT_EXCEEDED` is fail-closed per `UC-LEX-20` and [rfc.md § Non-Negotiable Invariants 8](../rfc.md) | new |

### 4.3 Out of scope (other roll-ups)

- Per-shard scan internals — owned by [RFC-LEX-03](RFC-LEX-03.md).
- Front-door dispatch — owned by [RFC-LEX-02](RFC-LEX-02.md).
- Ranking weights and explainability — owned by [LEX-06](LEX-06.md).
- Cross-instance fanout (federated search-plane processes) — out of v1.

---

## §5 Implementation steps

Mostly residuals. Sequence:

1. **Define `FanoutPlan`** — `(repo_set, shard_set, per_shard_budget_us, deadline)` shape. Lives in `quanta-index-search-plane::fanout`.
2. **Land bounded parallel executor** — `tokio::JoinSet` (or equivalent) with a semaphore-capped admission. Reject queue-depth overflow with `PLAN_LIMIT_EXCEEDED`, not silent backpressure.
3. **Land typed cancel** — `CancellationToken` per query; all spawned shard tasks observe the token. On timeout, every in-flight shard returns `EXEC_SHARD_CANCELLED`.
4. **Land k-way deterministic merge** — `BinaryHeap` keyed on the 6-component tuple. Pull from each shard's stream in tuple order. Output is byte-identical across runs at fixed inputs.
5. **Partial-result discipline** — `TIMEOUT_EXCEEDED` is fail-closed. NO partial result on any shard timeout. `UC-LEX-20` (`timeout:5s` → typed error, not partial) is the canonical row.
6. **OBS-01 wiring** — every shard task opens `lq.exec.shard{shard_id}` span; the dispatcher carries `cancellation_observed: bool` and `admission_queue_depth`.
7. **Determinism property test** — `proptest! { sequential_merge(shards) ≡ parallel_merge(shards) }` at fixed shard outputs.
8. **SLO-bounded fanout cap** — pin max concurrent shards per query; pin max queue depth; pin per-shard slice of the query budget. Lock values in this spec's §9.

---

## §6 Test plan

Constituent test coverage (green):

- [LEX-06](LEX-06.md) tests — tiebreak tuple stability.
- [OBS-01](OBS-01.md) tests — span schema.

Residual gap (owed by this roll-up):

| Suite | What it asserts |
|---|---|
| `crates/quanta-index-search-plane/tests/fanout_bounded_parallelism.rs` | Admission cap honoured; overflow returns `PLAN_LIMIT_EXCEEDED` |
| `crates/quanta-index-search-plane/tests/fanout_typed_cancel.rs` | Engine-level cancel propagates to all in-flight shards |
| `crates/quanta-index-search-plane/tests/fanout_timeout_fail_closed.rs` | `timeout:5s` exceeded → `TIMEOUT_EXCEEDED`, NEVER partial result (UC-LEX-20) |
| `crates/quanta-index-search-plane/tests/merge_determinism.rs` | `parallel_merge ≡ sequential_merge` at fixed shard outputs |
| `crates/quanta-index-search-plane/tests/property_merge_order_stable.rs` | proptest — k-way merge preserves 6-tuple ordering across orderings of shard arrival |
| `crates/quanta-index-search-plane/benches/fanout_p6_baseline.rs` | criterion: P6 baseline corpus p99 within global SLO budget |

E2e: one row per global SLO scenario in `UC-LEX-20` family.

---

## §7 Observability

Per [OBS-01 §4.1](OBS-01.md):

- Span `lq.exec.fanout` with attributes:
  - `repos_scanned: u32`
  - `shards_scanned: u32`
  - `admission_queue_depth: u32`
  - `cancellation_observed: bool`
- Per-shard child span `lq.exec.shard{shard_id}` with `shard_duration_us`, `bytes_touched`, `candidates_emitted`, `timeout_hit: bool`.
- Merge span `lq.merge` with `merge_duration_us`, `merge_tuple` literal.
- Metric `lq_fanout_concurrent_shards` gauge.
- Metric `lq_fanout_admission_rejected_total{reason}` counter.
- Metric `lq_fanout_shard_timeout_total{shard_id}` counter.

---

## §8 Error scenarios

All within `LexicalErrorCode` v1 (per [PRE-CONTRACT-EXT §4](PRE-CONTRACT-EXT.md)). Fanout-specific surfacings:

- `PLAN_LIMIT_EXCEEDED` — admission queue overflow at fanout-cap; fail-closed.
- `EXEC_SHARD_TIMEOUT` — per-shard slice of the budget exceeded.
- `EXEC_SHARD_CANCELLED` — engine-level cancel propagated.
- `TIMEOUT_EXCEEDED` — global query budget exceeded; fail-closed (UC-LEX-20).
- `STATE_GENERATION_REGRESSION` — shards disagree on the active generation mid-fanout.
- `STATE_NOT_READY` — at least one shard's gen is below the activation watermark at fanout entry.

Fail-closed posture: partial results are **never** returned on any error. `UC-LEX-20` is the canonical fail-closed row.

---

## §9 Performance envelope

Aggregate fanout SLO (per [rfc.md § Capacity and SLO Targets](../rfc.md)):

| Knob | v1 lock |
|---|---|
| Max concurrent shards per query | 64 (proposed; lock in this spec's §12) |
| Max admission queue depth | 256 (proposed) |
| Per-shard slice of global budget | `global_p99_budget / live_shard_count`, capped at 80% of global |
| K-way merge cost | ≤ 5 ms p99 for k ≤ 1000 |
| Cancel propagation latency | ≤ 2 ms p99 |

Gated on P6 baseline corpus.

---

## §10 Risks

| ID | Risk | Mitigation |
|---|---|---|
| RU-LEX-05-1 | Parallelization breaks merge determinism under shard-arrival reordering | k-way heap on 6-tuple ordering; proptest gate |
| RU-LEX-05-2 | Cancel propagation race leaks zombie shard tasks | `CancellationToken` + `JoinSet::join_next_with_cancel`; soak test in CI |
| RU-LEX-05-3 | Partial-result silent fallback re-introduced under SRE pressure | `TIMEOUT_EXCEEDED` is normative; semgrep / lint guard for `Result::ok()` swallowing in this crate |
| RU-LEX-05-4 | Per-shard budget slicing starves slow shards | Slice is upper-bound, not floor; slow shards return `EXEC_SHARD_TIMEOUT`, not silently degrade |
| RU-LEX-05-5 | Fanout cap interacts badly with `repo:*` predicate-pushdown narrowing | Cap applies AFTER predicate pushdown reduces the repo set |

---

## §11 Definition of Done (provable sub-checklist)

- ✓ [LEX-06 §11 DoD](LEX-06.md) — 6-component tiebreak tuple + frozen-per-gen weights
- ✓ [OBS-01 §4.1](OBS-01.md) — fanout span schema landed
- 🔜 `FanoutPlan` shape in `quanta-index-search-plane::fanout`
- 🔜 Bounded parallel executor with admission cap
- 🔜 Typed cancel propagation across all in-flight shards
- 🔜 K-way deterministic merge consuming 6-tuple-ordered shard streams
- 🔜 `parallel_merge ≡ sequential_merge` property test green
- 🔜 `TIMEOUT_EXCEEDED` fail-closed (UC-LEX-20 green)
- 🔜 P6 baseline corpus p99 within global SLO budget
- 🔜 Cancel-propagation soak test in CI
- 🔜 Per-shard slice budget enforced via `EXEC_SHARD_TIMEOUT`

This roll-up is `done` when the 🔜 rows flip to ✓ and P6 baseline corpus passes the global SLO budget.

---

## §12 Open questions

| ID | Question | Owner |
|---|---|---|
| Q-RFC-LEX-05-1 | Max concurrent shards per query — 64 is a placeholder; what is the right number against P6 corpus? | benchmarking |
| Q-RFC-LEX-05-2 | Admission queue overflow → `PLAN_LIMIT_EXCEEDED` vs token-bucket backpressure? | front-door SLO owner |
| Q-RFC-LEX-05-3 | Per-shard budget slice — uniform vs weighted by historical p99? | SLO owner |
| Q-RFC-LEX-05-4 | Federated cross-instance fanout — explicitly v2? | composition root |

---

## §13 References

- [rfc.md §Ticket Pack](../rfc.md#ticket-pack) — RFC `LEX-05` definition
- [rfc.md §Execution Model](../rfc.md) — merge determinism rule
- [rfc.md §Non-Negotiable Invariants](../rfc.md) — invariant 8 (no silent partial result)
- [INDEX.md §1.2](INDEX.md) — bookkeeping gap that this roll-up closes
- [INDEX.md §3.6](INDEX.md) — producer-authorship correction
- [producer-handoff.md §3.5](../../../ssot/producer-handoff.md) — delta contract (gen alignment)
- [channel-architecture.md §3.1](../../../ssot/channel-architecture.md) — `Seal` authority
- Constituent specs: [LEX-06](LEX-06.md) · [OBS-01](OBS-01.md)
- Related roll-ups: [RFC-LEX-02](RFC-LEX-02.md) (front door) · [RFC-LEX-03](RFC-LEX-03.md) (sequential engine)
- [usecase.md UC-LEX-20](../usecase.md) — `timeout:5s` fail-closed canonical row
- Code: [`crates/quanta-index-search-plane/src`](../../../../crates/quanta-index-search-plane/src) · [`crates/quanta-index-lq-ranker`](../../../../crates/quanta-index-lq-ranker)
