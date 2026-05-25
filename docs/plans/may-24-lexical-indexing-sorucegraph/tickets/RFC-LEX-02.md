# RFC-LEX-02 — Global front door and surface contract cutover (roll-up)

| field | value |
|---|---|
| Kind | roll-up spec (RFC `LEX-02` ticket roll-up) |
| Status | partially shipped — surface complete, dispatcher wiring in flight |
| Owner crates | [`quanta-index-contract`](../../../../crates/quanta-index-contract), [`quanta-index-ipc`](../../../../crates/quanta-index-ipc), [`quanta-index-searchd`](../../../../crates/quanta-index-searchd) |
| Constituent specs | [PRE-CONTRACT-EXT](PRE-CONTRACT-EXT.md), [PRE-NORM](PRE-NORM.md), [BRIDGE-01](BRIDGE-01.md) |
| Last verified | 2026-05-25 |

> Roll-up bookkeeping only. The substantive surface lives in the constituent spec sheets; this file maps RFC `LEX-02` (per [rfc.md §Ticket Pack](../rfc.md#ticket-pack)) onto them and pins the residual gap (composition-root dispatcher wiring).

---

## §1 Purpose

RFC [`LEX-02` "global front door and surface contract cutover"](../rfc.md#ticket-pack) names the **IPC / searchd cutover layer**: the single typed entry point that accepts every LQ query variant (lexical / semantic / hybrid / history / structural / bridge / repomap / explain), routes it to the engine track that owns it, and surfaces a typed `LexicalErrorCode` on every refusal.

This roll-up is satisfied when:

1. The contract crate carries the complete request/response envelope set ([§4](#4-deliverables)).
2. The IPC layer round-trips the envelope set without behaviour drift across process boundaries.
3. The `searchd` composition root wires every variant to the engine track that owns it, with no silent fallback.

The constituent spec sheets close (1) and (2). Composition-root dispatcher wiring (3) is the **residual gap**.

---

## §2 Background

The RFC ticket pack collapsed three concerns under `LEX-02`: contract surface, transport envelope, and composition-root wiring. The 17 per-subsystem spec sheets in this directory split (1) and (2) into [PRE-CONTRACT-EXT](PRE-CONTRACT-EXT.md) and [PRE-NORM](PRE-NORM.md) under the Wave-0 prerequisite umbrella, and the per-engine surface bumps land in [BRIDGE-01](BRIDGE-01.md) and the LEX-04..07 family. No single spec sheet owns (3).

Producer-authorship correction ([INDEX.md §3.6](INDEX.md), [producer-handoff.md §3.5](../../../ssot/producer-handoff.md)) further constrained `LEX-02`: the front door MUST NOT accept a write-side `apply_changes` IPC. All producer-side mutation flows through the channel ops catalogue ([channel-architecture.md §3.1](../../../ssot/channel-architecture.md)); the IPC layer carries **read** requests only.

A separate `Sourcegraph` IPC variant was discussed in a Round-6b parallel-agent thread; not yet adopted. Tracked in [§12](#12-open-questions).

---

## §3 Inputs

Subsystem specs:

- [PRE-CONTRACT-EXT](PRE-CONTRACT-EXT.md) — `LexicalErrorCode` v1 (29 variants), `tenant_id`/`user_id` carrier, request/response shapes, no-shim discipline.
- [PRE-NORM](PRE-NORM.md) — canonical AST + parser + `LqCanonicalHashV1` (CBOR + SHA-256), bounded inputs (16 KiB / depth 32 / fan-out 64).
- [BRIDGE-01](BRIDGE-01.md) — `Bridge` variant payload + subset-table claim discipline.

Channel ops (read at composition-root setup time, not in the request path):

- `FullBundle`, `UpsertChunk`, `DeleteChunk`, `UpsertSymbol`, `DeleteSymbol`, `Seal`, `UpsertEmbedding`, `DeleteEmbedding` (per [channel-architecture.md §3.1](../../../ssot/channel-architecture.md)).

---

## §4 Deliverables

### 4.1 Shipped (constituent specs)

| Constituent | Crate | What it ships |
|---|---|---|
| PRE-CONTRACT-EXT | `quanta-index-contract::lex::*` | `SearchPlaneIpcRequest` enum, `SearchPlaneIpcResponse` enum, `SearchPlaneIpcRequestEnvelope`, `LexicalErrorCode` v1 (29 variants), `tenant_id` / `user_id` carrier, hand-rolled serde impls (per D18) |
| PRE-NORM | `quanta-index-contract::lex::query` + `quanta-index-lq-norm` | `LqQueryV1` AST, parser, `LqCanonicalHashV1` (CBOR+SHA-256), bounded inputs |
| BRIDGE-01 | `quanta-index-lq-bridge` | `Bridge` variant payload + `BridgeCandidatePacket` |
| IPC envelope codec | `quanta-index-ipc` ([codec.rs](../../../../crates/quanta-index-ipc/src/codec.rs), [server.rs](../../../../crates/quanta-index-ipc/src/server.rs)) | Length-prefixed CBOR codec; `SearchPlaneIpcServer::dispatch` trait stub |

### 4.2 IPC request variant set (locked v1)

Per [envelopes.rs](../../../../crates/quanta-index-contract/src/ipc/envelopes.rs) `SearchPlaneIpcRequest`:

1. `Lexical` — text-query path (LEX-00..06 backed).
2. `Semantic` — semantic-vector path (SEM-01 backed).
3. `Hybrid` — RRF / weighted fusion (SEM-02 backed).
4. `History` — commit / ref / tag / diff-range (LEX-07 backed).
5. `Structural` — tree-sitter pattern (STR-01 backed).
6. `Bridge` — Sourcegraph-syntax intake (BRIDGE-01 backed).
7. `RepoMapIngest` / `RepoMapActivate` / `RepoMapQuery` — repomap track ([quanta-index-repomap](../../../../crates/quanta-index-repomap/src)).
8. `Explain` — explainability path (LEX-06 backed).

Total: **10 variants** (not the "7" placeholder in earlier drafts; the dispatch surface grew during BRIDGE-01 and repomap-track integration). A proposed 11th `Sourcegraph` variant (Round 6b parallel-agent thread) is tracked in [§12](#12-open-questions); not yet adopted.

### 4.3 Residual gap (this roll-up)

| Gap | Where it lives | Disposition |
|---|---|---|
| Composition-root dispatcher wiring | [`crates/quanta-index-searchd/src/app/ipc_dispatcher.rs`](../../../../crates/quanta-index-searchd/src/app/ipc_dispatcher.rs), [`crates/quanta-index-searchd/src/app/dispatcher.rs`](../../../../crates/quanta-index-searchd/src/app/dispatcher.rs) | `cargo check --workspace` passes; live-wire spec for routing each `SearchPlaneIpcRequest` variant to its engine track is the integration item. Tracked as the only remaining work for this roll-up. |
| `Sourcegraph` IPC variant | proposed; not landed | Open question — see [§12](#12-open-questions). |

---

## §5 Implementation steps

The composition-root wiring is the only remaining work. Sequence:

1. **Read inventory** — re-read [PRE-CONTRACT-EXT](PRE-CONTRACT-EXT.md) §4 (request enum), [PRE-NORM](PRE-NORM.md) §4.1 (AST surface), [BRIDGE-01](BRIDGE-01.md) §4 (bridge packet).
2. **Compose dispatch table** — for each of the 10 variants, identify the owning engine track and the typed error mapping.
3. **Wire `SearchPlaneIpcServer::dispatch`** — implement on the searchd `IpcDispatcher` so every variant routes to its track. No silent fallback; an unknown variant returns `IPC_UNSUPPORTED_VARIANT` (PRE-CONTRACT-EXT taxonomy).
4. **Round-trip integration test** — one request per variant, asserting the response shape matches the contract.
5. **Cardinality cap** — front-door request-size cap from [PRE-NORM §4.6 bounded inputs](PRE-NORM.md) (16 KiB / depth 32 / fan-out 64); enforced before dispatch.

All steps reduce to integration glue across already-shipped surfaces. No new traits, no new error codes.

---

## §6 Test plan

Constituent test coverage (already green):

| Suite | Owned by | What it asserts |
|---|---|---|
| `quanta-index-contract/tests/ipc_envelope_round_trip.rs` | PRE-CONTRACT-EXT | Every variant CBOR-round-trips with byte-stable encoding |
| `quanta-index-contract/tests/lexical_error_code_v1.rs` | PRE-CONTRACT-EXT | 29 codes; no aliasing |
| `quanta-index-ipc/tests/server_dispatch.rs` | this roll-up (integration) | Length-prefixed framing, partial-read recovery, oversize rejection |
| `quanta-index-lq-norm/tests/parser_canonical_hash.rs` | PRE-NORM | AST canonicalization + `LqCanonicalHashV1` stability |
| `quanta-index-lq-bridge/tests/sg_subset_table.rs` | BRIDGE-01 | Adopted/normalized/refused buckets |

Residual e2e gap: **per-variant front-door integration test in `quanta-index-searchd`**, one row per `SearchPlaneIpcRequest` variant. Owned by composition-root wiring (above).

---

## §7 Observability

Per [OBS-01](OBS-01.md) §4.1:

- Root span `lq.query` opens at the IPC server entry, carries `variant_kind` attribute.
- Child spans `lq.parse` / `lq.normalize` / `lq.plan` / `lq.exec.*` / `lq.merge` / `lq.rank` populate per variant.
- Metric `lq_ipc_request_total{variant}` per dispatch.
- Metric `lq_ipc_request_rejected_total{variant, code}` per typed refusal.

No new instrumentation owed by this roll-up beyond the OBS-01 contract.

---

## §8 Error scenarios

All refusals route through [PRE-CONTRACT-EXT §4.4](PRE-CONTRACT-EXT.md) `LexicalErrorCode` v1 (29 variants). Front-door-specific codes already present:

- `IPC_UNSUPPORTED_VARIANT` — unknown enum kind.
- `PARSE_OVERSIZE_INPUT` — request > 16 KiB.
- `PARSE_DEPTH_EXCEEDED` — AST depth > 32.
- `PARSE_FAN_OUT_EXCEEDED` — fan-out > 64.
- `STATE_NOT_READY` — composition root not yet wired.
- `TIMEOUT_EXCEEDED` — request budget exceeded.

No new codes required by this roll-up. All `RFC-GAP-*-CODES` rows in [INDEX.md §3.1](INDEX.md) fold into the same `LexicalErrorCode` v1 bump.

---

## §9 Performance envelope

Aggregate SLO across the front door (per [rfc.md § Capacity and SLO Targets](../rfc.md)):

| Stage | p99 budget |
|---|---|
| IPC envelope decode | ≤ 0.5 ms |
| Parser + canonicalization | ≤ 2 ms (PRE-NORM cap) |
| Front-door cardinality check | ≤ 0.05 ms |
| Dispatch overhead (table lookup) | ≤ 0.05 ms |

Sum ≤ 3 ms of the global query budget; the remainder is owned by the engine track.

---

## §10 Risks

Roll-up-level only (sub-risks owned by constituent specs):

| ID | Risk | Mitigation |
|---|---|---|
| RU-LEX-02-1 | Composition-root wiring lands without integration e2e and drifts from spec | DoD gate: one e2e per variant, asserted in CI |
| RU-LEX-02-2 | Sourcegraph variant adopted post-cutover and forces a contract bump | Pin variant set as `v1`; bumping is a `wire_version` event per [producer-handoff.md §5](../../../ssot/producer-handoff.md) |
| RU-LEX-02-3 | Write-side `apply_changes` IPC re-introduced under pressure | Producer-authorship rule [§11 of channel-architecture.md](../../../ssot/channel-architecture.md); IPC is read-only |

---

## §11 Definition of Done (provable sub-checklist)

Constituent DoDs (✓ = shipped per constituent spec; 🔜 = deferred to this roll-up's integration work):

- ✓ [PRE-CONTRACT-EXT §11 DoD](PRE-CONTRACT-EXT.md) — `LexicalErrorCode` v1 + envelope types compile + hand-rolled serde
- ✓ [PRE-NORM §11 DoD](PRE-NORM.md) — parser + canonical-hash byte-stable
- ✓ [BRIDGE-01 §11 DoD](BRIDGE-01.md) — subset table + bridge packet
- ✓ IPC codec round-trip ([quanta-index-ipc tests](../../../../crates/quanta-index-ipc/tests))
- 🔜 `SearchPlaneIpcServer::dispatch` wired in [searchd composition root](../../../../crates/quanta-index-searchd/src/app/ipc_dispatcher.rs) for all 10 variants
- 🔜 Per-variant e2e integration test (one row per variant) green
- 🔜 OBS-01 span attributes per variant emitted at front door
- 🔜 Front-door cardinality check applied before dispatch (PRE-NORM caps)

This roll-up is `done` when the four 🔜 rows flip to ✓.

---

## §12 Open questions

| ID | Question | Owner |
|---|---|---|
| Q-RFC-LEX-02-1 | Adopt a separate `Sourcegraph` IPC variant (Round 6b parallel-agent proposal), or keep Sourcegraph syntax tunnelled through `Bridge`? | RFC author |
| Q-RFC-LEX-02-2 | Should `STATE_NOT_READY` carry per-track readiness (`lexical_ready` / `semantic_ready` / `history_ready`) or a single bit? | composition-root owner |
| Q-RFC-LEX-02-3 | Front-door request budget — single per-variant cap, or per-tenant token bucket above the cap? | SLO owner |

---

## §13 References

- [rfc.md §Ticket Pack](../rfc.md#ticket-pack) — RFC `LEX-02` definition
- [rfc.md §Canonical Read Pipeline](../rfc.md) — front-door scope
- [INDEX.md §1.2](INDEX.md) — bookkeeping gap that this roll-up closes
- [INDEX.md §3.6](INDEX.md) — producer-authorship correction
- [producer-handoff.md §3.5](../../../ssot/producer-handoff.md) — delta contract (read-only IPC scope)
- [channel-architecture.md §3.1](../../../ssot/channel-architecture.md) — op catalogue (write-side does NOT flow through IPC)
- Constituent specs: [PRE-CONTRACT-EXT](PRE-CONTRACT-EXT.md) · [PRE-NORM](PRE-NORM.md) · [BRIDGE-01](BRIDGE-01.md)
- [OBS-01](OBS-01.md) §4.1 — front-door spans + metrics
- Code: [`crates/quanta-index-contract/src/ipc/envelopes.rs`](../../../../crates/quanta-index-contract/src/ipc/envelopes.rs) · [`crates/quanta-index-ipc/src/`](../../../../crates/quanta-index-ipc/src) · [`crates/quanta-index-searchd/src/app/`](../../../../crates/quanta-index-searchd/src/app)
