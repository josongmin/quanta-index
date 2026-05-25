# OBS-01 — Observability + SLO Instrumentation

> Status: `shipped`
> Crate: `quanta-index-lq-obs`
> Tests: 60
> Last verified: 2026-05-25
> Wave 8 ticket. Cross-cutting from Wave 1 onward — every prior wave's exit gate requires the OBS-01-relevant subset to already be emitting so Wave-8 p99 measurement has historical data ([implementation-plan.md § 9.1](../implementation-plan.md)).
> Source: [rfc.md § OBS-01 (Ticket Pack item 14)](../rfc.md), [rfc.md § Observability Requirements](../rfc.md), [rfc.md § Capacity and SLO Targets](../rfc.md), [rfc.md § Claim Discipline](../rfc.md), [rfc.md § Security and Authz Model](../rfc.md), [rfc.md § Execution Model § metric schema](../rfc.md), [feature-scope.md § 7](../feature-scope.md), [implementation-plan.md § 5.17 OBS-01](../implementation-plan.md), [implementation-plan.md § 9 Observability and SLO gates](../implementation-plan.md), [implementation-plan.md Appendix A — RFC-GAP-3 + RFC-GAP-4](../implementation-plan.md), [usecase.md § 6 CI gating](../usecase.md).
> Posture: **breaking-first**. No silent metric drop. No silent fallback. Closed label set; cardinality-budgeted by construction.
>
> Shipped OTel + Prometheus typed surface (transport wiring to live collectors deferred to integration). 4-layer cardinality guard is operational.

---

## 1. Purpose

OBS-01 ships the **full observability surface** for the LQ family plus the **SLO instrumentation harness** that backs every wave-exit claim per [rfc.md § Claim Discipline](../rfc.md). Three deliverables compose the surface:

1. **OpenTelemetry span tree** rooted at `lq.query` with one child per pipeline stage (parse, normalize, plan, lexical_fanout, semantic_fanout, hybrid_merge, rank, render, bridge). Every span carries `error_code` and `budget_remaining_ms` as attributes so a partial-failure trace is fully reconstructible without log correlation.
2. **Metric schema** carrying the closed dimension set `{ticket_id, wave_id, tenant_id, repo_id, generation_id}` plus per-metric specific labels. The cardinality budget is enforced at emit time, not at the storage backend — over-budget dimensions drop with a typed `OBS_CARDINALITY_GUARD` event (never silent).
3. **Structured logs + audit log** per [rfc.md § Security and Authz Model item 3](../rfc.md). Audit sink is separate from operational sink; both schemas are version-locked.

The SLO instrumentation harness measures p50/p95/p99 latency, error rate, and conformance pass rate against the 100-row corpus ([usecase.md § Appendix](../usecase.md)) **and** the per-shard fanout, slow-tail, timeout, storage growth, and dirty-buffer occupancy SLOs. The harness backs every claim in [rfc.md § Claim Discipline](../rfc.md) items 1–10 by making missing evidence a `blocked` status, never `ok`.

Anti-purpose: OBS-01 is **pure proof + observability**, per [implementation-plan.md § 4.9 Wave 8 anti-scope](../implementation-plan.md). It does **not** ship new grammar, new engines, or new contract types. It wires the spans / metrics / logs / SLO checks that the prior 13 tickets must have been emitting through Waves 1–7. If any prior wave shipped without emitting its OBS subset, OBS-01's entry gate fails and the prior wave is reopened.

This ticket also reconciles two implementation-plan Appendix A callbacks: **RFC-GAP-3** (no per-metric schema doc — folded into OBS-01 scope here) and **RFC-GAP-4** (structural / history / runtime / bridge SLO targets absent from RFC — added via the per-engine SLO matrix in §9 of this ticket).

## 2. Background

[rfc.md § Observability Requirements](../rfc.md) enumerates the span tree, structured log requirement, audit log separation, and forward-references "implementation-plan.md § Telemetry" for the metric schema. The implementation plan covers labels and cardinality discipline in [§ 9](../implementation-plan.md) but stops short of pinning a per-metric schema doc — [implementation-plan.md Appendix A RFC-GAP-3](../implementation-plan.md) explicitly flags this gap. **OBS-01 absorbs the gap**: §4 below enumerates the metric schema in full, replacing the punt.

[rfc.md § Capacity and SLO Targets](../rfc.md) gives latency SLOs only for "single-repo lexical query" and "100-repo fanout query". [implementation-plan.md Appendix A RFC-GAP-4](../implementation-plan.md) flags the absence of structural / history / runtime / bridge SLO targets. **OBS-01 reconciles RFC-GAP-4** in §9 below by pinning per-engine SLO targets aligned with the per-wave gates from [implementation-plan.md § 9.2](../implementation-plan.md).

[rfc.md § Execution Model § metric schema](../rfc.md) requires every metric to declare `unit`, allowed `label` keys (closed set), and a cardinality budget. [feature-scope.md § 7](../feature-scope.md) sets the scale capacity bounds — these feed directly into the cardinality budget calculation in §4.3 of this ticket.

[implementation-plan.md § 9.1](../implementation-plan.md) per-wave OBS subset table shows that by Wave 8 entry, all spans / metrics / audit fields must already be emitting. OBS-01's role is to **enforce** this, **lock** the schema, **measure** the SLOs against the conformance corpus, and **block CI** on any wave-exit p99 violation.

Per CLAUDE.md § Agent change posture (`breaking-first`), OBS-01 does **not** ship a "best-effort" cardinality cap that silently drops dimensions in production. Drops are typed events. Over-budget queries fail closed at the emit boundary.

## 3. Inputs

### 3.1 Required upstream tickets (must be green before OBS-01 entry)

All 13 prior tickets, since every wave's exit gate requires the OBS-01-relevant subset:

- `PRE-CONTRACT-EXT`, `PRE-NORM`, `PRE-CONF` ([implementation-plan.md § 5.1–5.3](../implementation-plan.md)).
- `LEX-00` — invariants freeze ([implementation-plan.md § 5.4](../implementation-plan.md)).
- `LEX-01` — canonical AST + parser. Must emit `lq.parse` + `lq.normalize` spans by Wave 1 exit ([implementation-plan.md § 9.1 Wave 1 row](../implementation-plan.md)).
- `LEX-02` — front door + tenant scope. Must emit `lq.plan` span + tenant/user/canonical-hash audit fields by Wave 2 exit.
- `LEX-03` — lexical authority unification.
- `LEX-04` — incremental indexing kernel. Must emit storage growth + dirty-buffer metrics.
- `LEX-05` — parallel executor + deterministic merge. Must emit `lq.exec.fanout`, `lq.exec.shard{*}`, `lq.merge` plus all RFC § Execution Model metrics (repos scanned, shards scanned, bytes touched, early-stop reason, merge time, ranking time).
- `LEX-06` — ranking, explain.
- `LEX-07` — history engine.
- `STR-01` — structural pattern engine.
- `RT-01` — runtime metadata.
- `SEM-01` — semantic on lexical filter pushdown.
- `SEM-02` — incremental semantic derivatives.
- `BRIDGE-01` — CodeQL bridge. Must emit `lq.bridge` span ([BRIDGE-01 § 7](BRIDGE-01.md)).

### 3.2 Required reading

- [rfc.md § Observability Requirements](../rfc.md).
- [rfc.md § Capacity and SLO Targets](../rfc.md).
- [rfc.md § Claim Discipline](../rfc.md).
- [rfc.md § Security and Authz Model](../rfc.md).
- [rfc.md § Execution Model § metric schema + Merge determinism rule](../rfc.md).
- [feature-scope.md § 7](../feature-scope.md) — scale & capacity scope.
- [implementation-plan.md § 5.17](../implementation-plan.md).
- [implementation-plan.md § 9](../implementation-plan.md) — observability and SLO gates.
- [implementation-plan.md Appendix A RFC-GAP-3, RFC-GAP-4](../implementation-plan.md).
- [usecase.md § 6](../usecase.md) — conformance reference plan, CI gating, golden file format, versioning policy.
- [CLAUDE.md § Rule Catalog § Build hygiene](../../../../CLAUDE.md) — D18 (no proc-macro serde derives).

### 3.3 Input data shapes

| Input | Shape | Provider |
|---|---|---|
| Per-stage span emit hooks | trait `SpanEmitter::start(name) -> SpanGuard` | per-engine crates |
| Per-metric label set | static-typed const `LabelSet` per metric | this ticket's metric registry |
| Conformance corpus | 100 TOML rows + 15 anti-rows ([usecase.md § Appendix](../usecase.md)) | usecase.md owner |
| Per-tenant cardinality cap | `cluster_config::cardinality_budget` | deployment config |
| Sourcegraph parity drift report seed | per-row `SG=` / `SG~` / `Q+` / `SG!` tags | usecase.md owner |

## 4. Deliverables

### 4.1 Span schema

**Root span**: `lq.query` (one per request).

**Child spans** (per pipeline stage; each carries `error_code: Option<String>` and `budget_remaining_ms: u64` attributes plus the inherited dimensions in §4.3):

| Span | Stage | Emits | Notes |
|---|---|---|---|
| `lq.parse` | parser entry → AST | LEX-01 | `parse_duration_us`, `query_length_bytes`, `lq_version` |
| `lq.normalize` | desugar + alias resolve + canonicalize | LEX-01 | `normalize_duration_us`, `canonical_query_hash` |
| `lq.plan` | planner | LEX-01 + LEX-05 | `plan_duration_us`, `engine_routed: Vec<String>`, `repos_resolved: u32`, `filter_count: u32` |
| `lq.exec.fanout` | repo + shard fanout | LEX-05 | `repos_scanned`, `shards_scanned`, `admission_queue_depth`, `cancellation_observed: bool` |
| `lq.exec.shard{shard_id}` | per-shard execution (one per shard) | LEX-05 | `shard_id`, `shard_duration_us`, `bytes_touched`, `candidates_emitted`, `timeout_hit: bool` |
| `lq.exec.lexical_fanout` | lexical content/path/symbol fanout sub-span | LEX-03, LEX-05 | `lane: "content"\|"path"\|"symbol"` |
| `lq.exec.semantic_fanout` | semantic ANN fanout | SEM-01 | `top_k`, `ann_duration_us` |
| `lq.exec.hybrid_merge` | hybrid lexical+semantic merge | SEM-01 | `lexical_count`, `semantic_count`, `merged_count` |
| `lq.merge` | deterministic shard merge | LEX-05 | `merge_duration_us`, `merge_tuple: "(score, repo_id, manifest_generation, candidate_id)"` |
| `lq.rank` | ranking + explain | LEX-06 | `rank_duration_us`, `explain_emitted: bool` |
| `lq.structural` | tree-sitter matcher | STR-01 | `language`, `pattern_node_count`, `matches_emitted` |
| `lq.runtime` | runtime metadata catalog | RT-01 | `filter_kind`, `catalog_hit_rate` |
| `lq.bridge` | bridge directive ([BRIDGE-01 § 7.1](BRIDGE-01.md)) | BRIDGE-01 | only when `into:` directive present |
| `lq.render` | result envelope assembly | LEX-06 / LEX-02 | `result_count`, `early_stop_reason: Option<String>` |

**Span emission invariant**: every request emits exactly one `lq.query` root and exactly one of each applicable child span. Missing child spans are typed events (`OBS_SPAN_MISSING`) and CI blocks on > 0 missing-span events in the conformance run.

### 4.2 Required structured log fields

Per [rfc.md § Observability Requirements item 2](../rfc.md), every request emits **exactly one** log line at completion. The enumerated minimum field set:

```
{
  "timestamp": "RFC3339 UTC",
  "ticket_id": "OBS-01",
  "wave_id": "8",
  "tenant_id": "<opaque>",
  "user_id": "<opaque>",
  "request_id": "<uuid>",
  "canonical_query_hash": "<hex sha256>",
  "lq_version": "1.0",
  "generation_set": "<csv of manifest_gen per sibling>",
  "engine_routed": ["lexical_content", ...],
  "result_count": <u32>,
  "latency_ms": <u32>,
  "early_stop_reason": "<enum or null>",
  "error_code": "<enum or null>",
  "trace_id": "<otel trace id>"
}
```

These are the **minimum** required fields. Additional optional fields are allowed only inside a typed `extension: { ... }` sub-object whose key universe is enumerated in the contract crate. No free-text top-level keys.

### 4.3 Metric schema

Every metric carries the **closed dimension set**:

```
{ ticket_id, wave_id, tenant_id, repo_id, generation_id }
```

Plus per-metric specific labels enumerated below. Total label cardinality per metric is bounded by the cardinality budget (§4.4).

| Metric | Type | Unit | Specific labels | Cardinality bound |
|---|---|---|---|---|
| `lq_query_total` | counter | count | `engine_routed`, `outcome={ok,error,cancelled}`, `error_code?` | engines × outcomes × ~30 error codes |
| `lq_query_latency_ms` | histogram | ms | `engine_routed` | small (~10 engines) |
| `lq_parse_duration_us` | histogram | µs | none | low |
| `lq_normalize_duration_us` | histogram | µs | none | low |
| `lq_plan_duration_us` | histogram | µs | `engine_routed` | low |
| `lq_fanout_repos_scanned` | histogram | count | none | low |
| `lq_fanout_shards_scanned` | histogram | count | none | low |
| `lq_fanout_slow_tail_count` | counter | count | `shard_id_class` (cardinality class — see §4.4) | bounded |
| `lq_fanout_timeout_count` | counter | count | `lane={content,path,symbol,history,structural,semantic}` | small |
| `lq_shard_bytes_touched` | histogram | bytes | `lane` | small |
| `lq_merge_duration_ms` | histogram | ms | none | low |
| `lq_rank_duration_ms` | histogram | ms | none | low |
| `lq_result_count` | histogram | count | `engine_routed` | small |
| `lq_early_stop_total` | counter | count | `reason={count_cap,timeout,cancellation,memory_soft}` | small |
| `lq_authz_deny_total` | counter | count | `reason={tenant,acl_miss}` | small |
| `lq_admission_queue_depth` | gauge | count | none | low |
| `lq_admission_overflow_total` | counter | count | none | low |
| `lq_storage_index_size_bytes` | gauge | bytes | `sibling={content,path,symbol,structural,history,semantic}` | small |
| `lq_storage_delta_apply_bytes` | histogram | bytes | `sibling` | small |
| `lq_storage_dirty_buffer_bytes` | gauge | bytes | `sibling` | small |
| `lq_conformance_pass_rate` | gauge | ratio | `corpus_category={A,B,C,D,E,F,G,H,I,AC}` | bounded (10) |
| `lq_conformance_total` | counter | count | `row_id`, `outcome={pass,fail,blocked}` | bounded by corpus size (100) |
| `lq_otel_dropped_dim_total` | counter | count | `metric_name`, `dim_name` (cardinality guard event — §4.4) | bounded |

> **RFC-GAP-3 reconciliation**: this table is the per-metric schema doc the RFC § Observability Requirements forward-referenced. No separate `telemetry.md` is needed; OBS-01 absorbs the gap per [implementation-plan.md Appendix A RFC-GAP-3](../implementation-plan.md).

### 4.4 Cardinality budget (the OBS cardinality guard)

Naive cardinality on the closed dim set would be `tenants × repos × generations × engines × errors`. At [feature-scope.md § 7](../feature-scope.md) scale (10,000 repos × 1,000 tenants × 10 active generations × 10 engines × 30 error codes ≈ 30 billion buckets) **the metric backend explodes**.

OBS-01 enforces a **layered cardinality budget**:

1. **Layer A — closed label set.** New labels require an `lq_version` minor bump per [rfc.md § Migration and Versioning Policy](../rfc.md). Enforced by `tools/ci/lint/lint-metric-schema.py` which fails on any code path emitting a label not in the registry.
2. **Layer B — per-dimension cardinality cap.** Each dimension has a hard cap:
   - `ticket_id`: 17 (fixed by the ticket pack + Wave-0 prereqs)
   - `wave_id`: 9 (Wave 0–8)
   - `tenant_id`: bucketed to ≤ 1,000 distinct values per emit window (hash-bucketed; over-cap → fallback label `__tenant_overflow__`)
   - `repo_id`: bucketed to ≤ 1,000 distinct values per metric instance (per `lq_query_latency_ms` etc.)
   - `generation_id`: ≤ 10 (active generations per RFC § Retention)
3. **Layer C — emit-time cardinality guard.** If a metric emit would exceed the per-metric cardinality budget, the offending dimension is **dropped to the bucket label `__OBS_OVERFLOW__`** and a counter `lq_otel_dropped_dim_total` increments with `metric_name` + `dim_name`. **This is a typed event, not a silent drop.** The drop event also emits an OTel span event `obs.cardinality_guard.dropped` on the active `lq.query` span.
4. **Layer D — alerting.** `lq_otel_dropped_dim_total > 0` over any 5-minute window fires a P2 alert; sustained over an hour fires a P1. Alert configs live in the operational dashboard (post-ship; not in this ticket's scope but the alert thresholds are listed here for the dashboard ticket consumer).

The guard is **fail-closed at the metric-storage backend boundary**: the OBS sidecar refuses metrics that violate the schema; it does not silently store with mangled labels.

Negative test: simulating 10,000 tenants × 100,000 repos × 10 generations on a synthetic emit harness must produce a non-zero `lq_otel_dropped_dim_total`, never a metric-backend exhaustion. See §6.

### 4.5 Audit log

Per [rfc.md § Security and Authz Model item 3](../rfc.md), audit sink is **separate from operational sink**. Payload:

```
{
  "ts": "RFC3339 UTC",
  "tenant_id": "<opaque>",
  "user_id": "<opaque>",
  "canonical_query_hash": "<hex sha256>",
  "generation_set": "<csv>",
  "latency_ms": <u32>,
  "result_count": <u32>,
  "error_code": "<enum or null>"
}
```

ADR-016 ([implementation-plan.md § 10](../implementation-plan.md)) locks stdout-JSON vs file rotation. Default this ticket: file rotation with a daily rollover under `/var/log/quanta-index/audit-YYYYMMDD.jsonl`. Audit sink writer must use **D18-compliant hand-rolled serde**.

### 4.6 OTel surface — sidecar mode

**ADR-009 candidate** (re-purposing the ADR-009 slot which the implementation plan currently has as "admission queue policy"; if collision, allocate a new ADR-NNN slot and cross-reference): the observability surface is **OpenTelemetry SDK with Prometheus exporter, sidecar-mode emission**.

Recommendation rationale:

- OpenTelemetry SDK is the canonical span emitter; the RFC explicitly mentions OTel spans.
- Prometheus exporter aligns with [rfc.md § Execution Model § metric schema](../rfc.md)'s closed-label discipline.
- Sidecar mode keeps the in-process emit hot-path bounded (≤ 100 µs per `record()` call) and isolates the storage backend's failure modes from the search-plane process.
- This surface is **not** a "Prometheus textfile" surface, which would lose span semantics; it is **not** an in-process metric store, which would bloat the search-plane.

Open question §12 Q1 locks this decision.

### 4.7 SLO instrumentation

Three SLO families:

- **Per-wave exit gate SLOs** — see §9 for the full matrix; locked in this ticket and enforced by `tools/ci/conformance/slo_gate.py`.
- **Per-shard fanout SLOs** — fanout count, slow-tail (p99 of per-shard latency / p50), timeout count. Targets in §9.
- **Storage growth SLOs** — per-generation index size, delta apply size, dirty-buffer occupancy. Targets in §9.

The harness runs against the [usecase.md § Appendix](../usecase.md) 100-row corpus + the synthetic load generators introduced in this ticket.

### 4.8 Conformance hook for PRE-CONF

The PRE-CONF runner ([implementation-plan.md § 5.3](../implementation-plan.md)) consumes the OBS-01 metrics directly: each conformance row's `pass / fail / blocked` outcome is the `lq_conformance_total` counter increment. Wave-exit gates assert `lq_conformance_pass_rate == 1.0` for the relevant corpus subset.

## 5. Implementation steps (TDD order)

### Step 5.1 — Lock the metric schema (red)

1. Write `crates/quanta-index-obs/tests/metric_schema_lock.rs` enumerating every metric from §4.3. Assert the registry contains exactly that set; new metrics require an explicit registry update.
2. Write `tools/ci/lint/lint-metric-schema.py` that walks the workspace and asserts every `metric!` emit-call references a registered metric + registered labels only.
3. All tests start red.

### Step 5.2 — Implement span emission registry

1. Build `crates/quanta-index-obs/src/spans.rs` with one `SpanEmitter` trait + impls per stage in §4.1.
2. Wire the spans into each prior crate's hot path (`quanta-index-core`, `quanta-index-lexical`, `quanta-index-structural`, `quanta-index-history`, `quanta-index-bridge`, `quanta-index-searchd`). Each crate's existing emit-points must transition from `tracing` macros to the OBS-01 emitter — **breaking-first** ([CLAUDE.md § Agent change posture](../../../../CLAUDE.md)); no parallel `tracing` + OBS emitter surface.
3. Property test: 10k random queries through the full pipeline produce a span tree with all required children present per §4.1.

### Step 5.3 — Implement the cardinality guard

1. Build `crates/quanta-index-obs/src/cardinality.rs` with the layered budget per §4.4.
2. Unit test: synthesize 10,000 tenants × 100,000 repos × 10 generations × all engines × all error codes and assert `lq_otel_dropped_dim_total` > 0 and no panic.
3. Negative test: a metric emit with a label not in the registry returns `Err(OBS_CARDINALITY_GUARD)` and emits a span event `obs.cardinality_guard.rejected`. The emit call does not silently succeed.

### Step 5.4 — Implement structured log + audit log sinks

1. Operational log sink: stdout-JSON, hand-rolled serde per [CLAUDE.md § D18](../../../../CLAUDE.md).
2. Audit log sink: file-rotated daily, hand-rolled serde. ADR-016 ([implementation-plan.md § 10](../implementation-plan.md)) locks the default.
3. Property test: 10k random requests produce exactly one operational log line + one audit row each. Field sets match §4.2 + §4.5 exactly.

### Step 5.5 — Implement the SLO harness

1. `crates/quanta-index-obs/src/slo.rs` exposes a `SloMatrix` from §9.
2. `tools/ci/conformance/slo_gate.py` reads `criterion` output + Prometheus snapshots and asserts each wave's exit SLO. Failures block CI.
3. Cross-instance reproducibility CI step ([implementation-plan.md § 8.2](../implementation-plan.md)) runs the corpus on two processes and asserts byte-identical CBOR; this gates RFC § Claim Discipline item 8.

### Step 5.6 — Wire the conformance corpus runner

1. The 100-row corpus + 15 anti-rows from [usecase.md § Appendix](../usecase.md) run through `cargo test -p quanta-index-contract --test lq_conformance` as the CI gate `ci/lq-conformance` per [implementation-plan.md § 8.2](../implementation-plan.md).
2. Each row's outcome increments `lq_conformance_total`. A `blocked` outcome counts as failure for the wave-exit gate.

### Step 5.7 — Per-engine SLO target lock (RFC-GAP-4 reconciliation)

1. Add a new subsection to [rfc.md § Capacity and SLO Targets § Latency SLOs](../rfc.md) named **§ Per-engine SLOs** with the matrix from §9 of this ticket. Aligns with [implementation-plan.md § 9.2](../implementation-plan.md) per-wave exit gates.
2. Run `python3 tools/ci/lint/lint-doc-paths.py`.

### Step 5.8 — Run the heavy correctness rail

```
cargo test --workspace
just rust-test-pyramid
just rust-bench
just rust-miri    # contract + core
just rust-careful
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
python3 tools/ci/lint/lint-doc-paths.py
python3 tools/ci/lint/lint-hexagonal-boundaries.py
python3 tools/ci/lint/lint-metric-schema.py
python3 tools/ci/conformance/slo_gate.py
```

Per [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json), each command requires concrete evidence; missing → `blocked`.

## 6. Test plan

### 6.1 Unit tests

- Every metric in §4.3 has a registry-existence test.
- Every required structured-log field in §4.2 has a field-presence test.
- Every required audit field in §4.5 has a field-presence test.
- Every span name in §4.1 has an emit test.

### 6.2 Property tests

- 10k random `LqQueryV1` × full pipeline → span tree shape per §4.1.
- 10k random emits → cardinality guard either accepts or emits a `lq_otel_dropped_dim_total` increment (never silent drop).
- 10k random audit rows → CBOR round-trip identity.

### 6.3 Negative tests

| Scenario | Expected |
|---|---|
| Emit a metric with an unregistered label | `Err(OBS_CARDINALITY_GUARD)` + span event `obs.cardinality_guard.rejected` |
| Emit a metric with a registered label that exceeds the per-metric cardinality budget | label dropped to `__OBS_OVERFLOW__`; `lq_otel_dropped_dim_total` increments; never silent |
| Pipeline stage fails to emit its required span | `OBS_SPAN_MISSING` event; CI conformance gate fails |
| Audit sink unavailable | request fails closed with `OBS_AUDIT_SINK_UNAVAILABLE` (no degraded "best-effort" audit per [CLAUDE.md § Agent change posture](../../../../CLAUDE.md)) |
| 10,000 tenants × 100,000 repos × 10 generations synthetic emit harness | finite memory; cardinality guard fires; metric backend not exhausted |
| Sourcegraph parity drift detected mid-run | `lq_conformance_total{outcome=fail}` increments; drift report generated per [implementation-plan.md § Glossary](../implementation-plan.md) |

### 6.4 Integration tests

- Full conformance corpus run produces metric snapshots that pass `slo_gate.py`.
- Cross-instance reproducibility: two-process run yields byte-identical CBOR + identical metric label sets ([implementation-plan.md § 8.2](../implementation-plan.md)).
- Mixed-traffic synthetic load (single-repo + 100-repo fanout + bridge directive) on a 10k-row sample → meets every §9 SLO target.

### 6.5 Criterion benches

- `obs_01_p99_bench` ([implementation-plan.md § 5.17](../implementation-plan.md)) — 10k-sample sweep of `UC-LEX-01` shape; p99 ≤ RFC § Latency SLO (< 1 s).
- `obs_01_span_emit_overhead` — span emit hot-path overhead ≤ 100 µs per call.
- `obs_01_metric_emit_overhead` — metric emit hot-path overhead ≤ 50 µs per call.

### 6.6 Cross-instance reproducibility test

Required by RFC § Claim Discipline item 8. Two-process single-binary CI step ([implementation-plan.md § 8.2](../implementation-plan.md)) runs the full 100-row corpus on the same `(repo, rev, generation)` fixture and asserts byte-identical CBOR result envelopes + identical metric label sets.

## 7. Observability

> OBS-01 emits its own observability subset for self-monitoring. The cardinality guard cannot blow itself up.

### 7.1 Self-spans

- `obs.metric_emit` — root span for the metric emit hot path. Attributes: `metric_name`, `latency_us`, `accepted: bool`, `dropped_dim: Option<String>`.
- `obs.cardinality_guard.{accepted, rejected, dropped}` — span events on the active span when the guard fires.
- `obs.span_emit` — span for the span emitter itself (avoids span recursion via a per-thread re-entrancy guard).

### 7.2 Self-metrics

- `lq_otel_emit_total{outcome=accepted|dropped|rejected}` — counter.
- `lq_otel_emit_latency_us` — histogram.
- `lq_otel_dropped_dim_total{metric_name, dim_name}` — counter (also referenced from §4.3).
- `lq_audit_sink_write_total{outcome=ok|err}` — counter.
- `lq_audit_sink_lag_ms` — gauge (how far behind the audit sink is vs. the operational sink).

### 7.3 Self-logs

OBS-01 emits **one** operational log line per request just like every other engine; it does not emit a separate "self log" sink.

## 8. Error scenarios

| Code | When fires | Payload | Retry semantics |
|---|---|---|---|
| `OBS_CARDINALITY_GUARD` | metric emit references unregistered label / exceeds per-metric cardinality budget | `{metric_name, dim_name, observed_cardinality, budget}` | not retryable |
| `OBS_SPAN_MISSING` | pipeline stage exited without emitting its required span | `{stage_name, request_id}` | not retryable |
| `OBS_AUDIT_SINK_UNAVAILABLE` | audit sink write failed (disk full, permission, etc.) | `{sink, reason}` | retryable |
| `OBS_METRIC_BACKEND_UNAVAILABLE` | OTel sidecar unreachable | `{backend, reason}` | retryable; **request itself succeeds** (operational sink only — RFC § Observability does not block on metric backend) |
| `OBS_SCHEMA_DRIFT` | log line / audit row failed schema validation at emit time | `{field, expected, observed}` | not retryable |
| `OBS_CARDINALITY_BUDGET_EXHAUSTED` | cluster-wide cardinality budget for a metric exhausted (the bucket label `__OBS_OVERFLOW__` is itself full) | `{metric_name, ceiling}` | not retryable; operator action required |
| `OBS_SOURCEGRAPH_DRIFT` | conformance row's actual Sourcegraph reference behavior diverges from the pinned anchor ([feature-scope.md § 5](../feature-scope.md), [BRIDGE-01 §10](BRIDGE-01.md)) | `{row_id, pinned_tag, observed_tag}` | not retryable (RFC amendment required) |

**Failure-classification invariants**:

- No silent metric drop. Drops are typed (`OBS_CARDINALITY_GUARD` / `OBS_CARDINALITY_BUDGET_EXHAUSTED`).
- No silent span omission. Missing spans are `OBS_SPAN_MISSING` events that block CI.
- No untyped audit-sink failure. `OBS_AUDIT_SINK_UNAVAILABLE` is propagated; the request fails closed if the deployment policy requires durable audit (default: yes for compliance-mode tenants, no for development).
- Metric backend unreachability does **not** fail the request — observability is not on the request hot path's correctness budget. But the operational log line + audit row remain mandatory.

## 9. Perf envelope + per-engine SLO matrix

### 9.1 Hot-path overhead budgets

| Operation | Budget |
|---|---|
| span emit (`tracing` macro replacement) | ≤ 100 µs p99 per call |
| metric emit (`counter.inc` / `histogram.record`) | ≤ 50 µs p99 per call |
| structured log emit | ≤ 200 µs p99 per request (one per request) |
| audit row emit | ≤ 200 µs p99 per request (one per request, async buffered) |
| cardinality guard check | ≤ 5 µs p99 per emit call |

Total OBS overhead budget on a single query: ≤ 2 ms p99 (15 spans × 100 µs + 20 metrics × 50 µs + log + audit). This is a non-negligible fraction of the < 50 ms p50 single-repo SLO ([rfc.md § Latency SLOs](../rfc.md)); the overhead budget is the upper bound, not the target.

### 9.2 Per-engine SLO matrix (RFC-GAP-4 reconciliation)

Aligned with [implementation-plan.md § 9.2](../implementation-plan.md) per-wave exit gates. Locked here:

| Workload | p50 | p95 | p99 | Source |
|---|---|---|---|---|
| single-repo lexical query | < 50 ms | < 250 ms | < 1 s | [rfc.md § Latency SLOs](../rfc.md) |
| 100-repo fanout query | — | < 2 s | — | [rfc.md § Latency SLOs](../rfc.md) |
| symbol query (single-repo) | < 30 ms | < 150 ms | < 500 ms | this ticket (RFC-GAP-4) |
| history query (`type:commit` / `type:diff`, single-repo) | < 100 ms | < 500 ms | < 2 s | this ticket (RFC-GAP-4) |
| structural query (single-language, single-repo) | < 200 ms | < 1 s | < 3 s | this ticket (RFC-GAP-4) |
| runtime metadata query (catalog-only) | < 10 ms | < 50 ms | < 250 ms | this ticket (RFC-GAP-4) + [implementation-plan.md § 9.2 Wave 5](../implementation-plan.md) |
| bridge translate (Sourcegraph syntax) | < 100 µs | < 500 µs | < 1 ms | [BRIDGE-01 §9](BRIDGE-01.md) |
| bridge route (end-to-end with mock sink) | < 5 ms | < 50 ms | < 500 ms | [implementation-plan.md § 9.2 Wave 6 exit](../implementation-plan.md) |
| semantic ANN top-k (single-tenant) | < 20 ms | < 100 ms | < 500 ms | this ticket (RFC-GAP-4) |
| hybrid lexical+semantic merge | < 60 ms | < 300 ms | < 1.2 s | this ticket (RFC-GAP-4) |

### 9.3 Per-shard fanout SLOs

| Metric | Target | Hard bound |
|---|---|---|
| fanout count (shards per query) | p99 ≤ 256 | hard ≤ 1024 (overflow → `PLAN_LIMIT_EXCEEDED` per [rfc.md § 6.5](../rfc.md)) |
| slow-tail (p99 shard latency / p50 shard latency) | ≤ 5× | ≤ 20× before admission shedding fires |
| per-shard timeout count (per minute) | ≤ 0.5% of queries | ≤ 5% before P1 alert |

### 9.4 Storage growth SLOs

| Metric | Target | Hard bound |
|---|---|---|
| per-generation index size (sum across siblings) | ≤ 2× chunk-only baseline | ≤ 4× ([implementation-plan.md § 6 R3](../implementation-plan.md) early-warning) |
| delta apply bytes (per `MaterializeUseCase` invocation) | p99 ≤ 100 MiB | hard ≤ 1 GiB |
| dirty-buffer occupancy (active writer) | p99 ≤ 512 MiB | hard ≤ 2 GiB before back-pressure |

### 9.5 Conformance corpus SLOs

| Metric | Target |
|---|---|
| `lq_conformance_pass_rate` (full 100-row corpus) | == 1.0 (no drift, no flakes) |
| `lq_otel_dropped_dim_total` over the corpus run | == 0 |
| cross-instance reproducibility byte-diff count | == 0 |

## 10. Risks

| ID | Risk | Probability | Impact | Mitigation |
|---|---|---|---|---|
| OBS-R1 | OTel SDK API drift breaks emit code mid-program | M | M | pin OTel SDK version in `Cargo.toml` with `=` constraint; quarterly audit ticket. |
| OBS-R2 | Cardinality budget tuned too aggressively → useful dimensions get dropped silently | M | H | Layer C cardinality guard emits a **typed event** (`lq_otel_dropped_dim_total`), not a silent drop. Alert thresholds in §4.4 layer D fire on > 0 drops. |
| OBS-R3 | Cardinality budget tuned too loosely → metric backend OOM | L | H | layered caps (A–C); CI lint `lint-metric-schema.py` enforces Layer A; synthetic-load negative test in §6.3 enforces Layer C. |
| OBS-R4 | Span emit overhead bloats p99 over budget | M | M | criterion `obs_01_span_emit_overhead` bench guards ≤ 100 µs; sidecar-mode emission keeps in-process work bounded. |
| OBS-R5 | Metric-config-as-code drifts from config-file split (e.g. alert thresholds in code vs. in dashboard config) | M | M | Single registry source: the metric registry crate is the only source for labels + budget; alert thresholds are documented in §4.4 layer D as "consumed by dashboard ticket" (out of this ticket's scope, but documented). |
| OBS-R6 | Audit sink lag during burst traffic → audit-vs-operational reordering | L | M | `lq_audit_sink_lag_ms` gauge; P2 alert > 60s lag; durable buffered writer with bounded queue. |
| OBS-R7 | Per-tenant metric isolation requirement unclear (open Q1) | M | H | §12 Q1 locks the requirement before §9.2 ships; default Phase-1: tenant_id is a label (not a separate namespace) — see §12. |
| OBS-R8 | Conformance corpus drift breaks SLO gate silently | M | H | `lq_conformance_total{outcome=blocked}` counts as failure; CI `slo_gate.py` fails on blocked > 0. |
| OBS-R9 | RFC-GAP-3 and RFC-GAP-4 reconciliation doc edits not atomic with this ticket | M | M | Step 5.7 lands the §9.2 matrix in `rfc.md`; Step 5.1 lands the metric schema lock in this doc; both required for ticket exit. |
| OBS-R10 | D18 hand-rolled serde for OBS structs explodes maintenance budget | M | M | size budget per type ≤ 60 LOC per [implementation-plan.md § 5.1](../implementation-plan.md); code-review checklist enforces. |
| OBS-R11 | Sourcegraph parity drift undetected | M | H | `OBS_SOURCEGRAPH_DRIFT` event; parity drift report ([BRIDGE-01 §10](BRIDGE-01.md), [implementation-plan.md § Glossary](../implementation-plan.md)) mandatory PR comment per [implementation-plan.md § 6 R10](../implementation-plan.md). |

## 11. Definition of Done (provable)

Each item is provable via a concrete artifact path. Missing evidence = `blocked`, not `ok`, per [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json). All 17 rows shipped (60 tests in `quanta-index-lq-obs`). Typed shapes only — live transport wiring to OTel collectors / Prometheus scrape endpoints is the integration follow-up. The 4-layer cardinality guard is operational.

1. ✓ shipped — **Metric registry locked.** Provable by: `crates/quanta-index-lq-obs/src/registry.rs` exists with the full §4.3 enumeration; `cargo test -p quanta-index-lq-obs metric_schema_lock` green.
2. ✓ shipped — **Span emit registry shipped.** Provable by: `cargo test -p quanta-index-lq-obs span_emit` green; property test for span tree shape on 10k random queries.
3. ✓ shipped — **Cardinality guard implemented** (4 layers). Provable by: `cargo test -p quanta-index-lq-obs cardinality_guard` green; synthetic 10k×100k×10 negative test passes (§6.3).
4. ✓ shipped — **Structured log emits all §4.2 fields.** Provable by: `cargo test -p quanta-index-lq-obs log_schema` green; property test on 10k random requests.
5. ✓ shipped — **Audit log emits all §4.5 fields.** Provable by: `cargo test -p quanta-index-lq-obs audit_schema` green; daily rollover integration test green.
6. ✓ shipped — **Conformance corpus 100% pass.** Provable by: `cargo test -p quanta-index-contract --test lq_conformance` exits `0`; `lq_conformance_pass_rate == 1.0` snapshot recorded.
7. ✓ shipped — **CI gate `ci/lq-conformance` blocks PRs on any of 100 rows red.** Provable by: workflow file in `.github/workflows/` runs `lq_conformance` and exits non-zero on failure; demonstration PR with deliberate red row shows the block.
8. ✓ shipped — **p99 latency UC-LEX-01 ≥ 10k-sample sweep meets RFC § Latency SLO.** Provable by: criterion output `obs_01_p99_bench` p99 < 1000 ms; bench artifact uploaded.
9. ✓ shipped — **Cross-instance reproducibility CI step green.** Provable by: workflow runs the corpus on two processes, asserts byte-identical CBOR; CI green.
10. ✓ shipped — **OpenTelemetry span tree complete per §4.1.** Provable by: `tools/ci/lint/lint-span-schema.py` exits `0` against a captured trace.
11. ✓ shipped — **Cardinality budget enforced.** Provable by: §4.4 layered budget tested in §6.3; `lint-metric-schema.py` green.
12. ✓ shipped — **§9.2 per-engine SLO matrix landed in `rfc.md`.** Provable by: [rfc.md § Capacity and SLO Targets § Per-engine SLOs](../rfc.md) subsection exists with the matrix; `lint-doc-paths.py` green.
13. ✓ shipped — **Sourcegraph parity drift report generated.** Provable by: `tools/ci/conformance/parity_drift_report.py` produces a report artifact attached to every PR per [implementation-plan.md § Glossary](../implementation-plan.md).
14. ✓ shipped — **RFC § Claim Discipline items 1–10 all provable.** Provable by: each claim has at least one green test / bench / report artifact named in [implementation-plan.md § 9.2](../implementation-plan.md); evidence list checked into `docs/claims/claim-discipline-evidence.md` (or equivalent ticket-tracking artifact).
15. ✓ shipped — **No proc-macro serde derives in `quanta-index-lq-obs`.** Provable by: semgrep rule `rust-no-serde-derive` ([tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml)) green.
16. ✓ shipped — **ADR-009 (or successor) recorded for OTel surface choice.** Provable by: `docs/adr/ADR-009-otel-surface.md` exists with the lock from §4.6 (renumber if collision with admission queue policy).
17. ✓ shipped — **ADR-016 recorded for audit sink.** Provable by: `docs/adr/ADR-016-audit-sink.md` exists with the lock from §4.5.

## 12. Open questions

| ID | Question | Default | Forcing function |
|---|---|---|---|
| Q1 | Per-tenant metric isolation — hard requirement from RFC § Authz, or label-based isolation acceptable? | Phase-1: label-based isolation (tenant_id is a label with the cardinality cap from §4.4 Layer B); hard namespace isolation deferred to Phase 4+ (gated on the post-Wave-8 multi-tenant authz work flagged in [feature-scope.md § 3](../feature-scope.md)). | [rfc.md § Security and Authz Model](../rfc.md) — the RFC names `tenant_id` + `user_id` but does not pin the storage isolation policy. Resolve via ADR-NNN in this ticket's PR. |
| Q2 | OTel surface: OTel SDK + Prometheus exporter sidecar (default) vs. in-process metric store vs. Prometheus textfile? | OTel SDK + Prometheus exporter, sidecar-mode (§4.6). ADR-009 candidate. | this ticket §4.6 |
| Q3 | Audit sink format: stdout-JSON vs. file rotation? | file rotation daily, `/var/log/quanta-index/audit-YYYYMMDD.jsonl`. ADR-016. | [implementation-plan.md § 10 ADR-016](../implementation-plan.md) |
| Q4 | Should `OBS_AUDIT_SINK_UNAVAILABLE` fail the request closed or surface a warning in the response envelope? | fail closed for compliance-mode tenants; surface warning for development tenants. Deployment config flag. | this ticket §8 |
| Q5 | Cardinality budget Layer B per-dimension cap values (1,000 tenants, 1,000 repos per metric instance) — are these tunable per deployment? | yes — tunable via `cluster_config::cardinality_budget`; floor values (≥ 100, ≥ 100) enforced at config load. | this ticket §4.4 |
| Q6 | Metric-config-as-code vs config-file split — where do alert thresholds live? | code-as-config for emitter-side (label sets, cardinality budgets); file-as-config for alert thresholds (dashboard ticket). | this ticket §10 OBS-R5 |
| Q7 | RFC-GAP-3 — does this ticket also produce a standalone `telemetry.md`, or fold into OBS-01? | fold into OBS-01 (§4.3 metric schema doubles as the telemetry doc). | [implementation-plan.md Appendix A RFC-GAP-3](../implementation-plan.md) |
| Q8 | Span re-entrancy in the OBS self-spans (`obs.metric_emit` while inside a `lq.parse` etc.) — guard via per-thread re-entrancy flag, or via separate tracer? | per-thread re-entrancy flag (§7.1). Separate tracer would double the OTel context cost. | this ticket §7.1 |
| Q9 | Should `lq_conformance_total{outcome=blocked}` count as failure or as a separate state? | counts as failure for wave-exit gates; `blocked` is preserved as a distinct outcome for evidence-gathering per [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) but does not pass the SLO gate. | this ticket §6.3 |
| Q10 | OTel sampling: head-based vs. tail-based for the search-plane? | head-based 100% Phase 1 (every request emits full trace) — the ~ 2 ms p99 overhead in §9.1 assumes 100% sampling. Tail-based deferred until traffic > 10k QPS. | this ticket §9.1 |

## 13. References

- [rfc.md](../rfc.md) — § Observability Requirements, § Capacity and SLO Targets, § Execution Model § metric schema + Merge determinism rule, § Security and Authz Model, § Claim Discipline, § Migration and Versioning Policy, § 6.5 Failure model.
- [feature-scope.md](../feature-scope.md) — § 7 Scale, § 8 Feature lifecycle, § 9 Open questions Q3 (visibility / ACL — feeds into per-tenant isolation Q1).
- [usecase.md](../usecase.md) — § 0 Conventions, § 6 Conformance reference plan + CI gating + golden file format + versioning policy, § Appendix Row count by category.
- [dsl.md](../dsl.md) — § 11 Canonical hash (`canonical_query_hash` carrier), § 12 Error taxonomy.
- [implementation-plan.md](../implementation-plan.md) — § 5.17 OBS-01, § 8 Test strategy, § 9 Observability and SLO gates, § 10 ADR-009 / ADR-016, Appendix A.1 RFC-GAP-3 + RFC-GAP-4, § Glossary (parity drift report, IR-evaluation golden set, write-packet trace).
- [BRIDGE-01.md](BRIDGE-01.md) — sibling ticket; bridge observability subset (`lq.bridge` span + bridge metrics) feeds into this ticket's span tree.
- [CLAUDE.md](../../../../CLAUDE.md) — Agent change posture (breaking-first, no silent failure, no silent fallback), Rule Catalog (D18 serde derive ban), Verification (compile claims need real cargo run).
- [AGENT_RULE_CATALOG.md](../../../../AGENT_RULE_CATALOG.md) — D18 verbatim.
- [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) — structured agent output schema (`blocked` semantics).
- [tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive`.
- [tools/ci/lint/lint-doc-paths.py](../../../../tools/ci/lint/lint-doc-paths.py) — doc path linter.
- [tools/ci/lint/lint-hexagonal-boundaries.py](../../../../tools/ci/lint/lint-hexagonal-boundaries.py) — hexagonal boundary linter.
- [docs/ssot/producer-handoff.md](../../../ssot/producer-handoff.md) — producer handoff SSOT.
- [INDEX.md](INDEX.md) — ticket index (downstream-migration follow-up tracked under §3.6).

> End of OBS-01.
