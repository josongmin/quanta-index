# RFC-BRIDGE-01-CodeQL — CodeQL bridge and candidate export (roll-up)

| field | value |
|---|---|
| Kind | roll-up spec (RFC `BRIDGE-01` original-scope roll-up) |
| Status | **deferred to v2** — nothing shipped in v1 |
| Owner crate(s) | TBD — placeholder `quanta-index-lq-bridge-codeql` (NOT yet created) |
| Constituent specs | NONE in v1 |
| Sibling NOT in scope | [BRIDGE-01.md](BRIDGE-01.md) ships **Sourcegraph→LQ** translator under `quanta-index-lq-bridge`; that is a different target (see [INDEX.md §3.1 RFC-GAP-BRIDGE-TARGET](INDEX.md)) |
| Last verified | 2026-05-25 |

> Roll-up bookkeeping. **NOT** the same as the shipped `BRIDGE-01.md` spec — that spec ships the Sourcegraph translator. This roll-up tracks RFC `BRIDGE-01` "CodeQL bridge and candidate export", which is **entirely deferred to v2**. The file exists to close the [INDEX.md §1.2](INDEX.md) bookkeeping gap and to scope what v2 owes.

---

## §1 Purpose

RFC [`BRIDGE-01` "CodeQL bridge and candidate export"](../rfc.md#ticket-pack) names the **CodeQL** sink: a lexical-candidate set materialized by the LQ planner is exported as a typed `BridgeCandidatePacket` and routed via `into:codeql` / `scope:results` / `with:lexical` directives to a CodeQL query / data-flow target.

This roll-up is **deferred to v2**. v1 ships the Sourcegraph translator under the existing [BRIDGE-01.md](BRIDGE-01.md) spec name (a scope drift documented in [INDEX.md §3.1 RFC-GAP-BRIDGE-TARGET](INDEX.md)). Nothing CodeQL-specific ships in v1.

The roll-up is satisfied in v2 when:

1. A `CodeQlQuerySpec` payload exists on the `BridgeTarget::CodeQl(...)` enum variant.
2. The LQ planner can compose `into:codeql` directives that materialize a `BridgeCandidatePacket` and hand it to a CodeQL invocation surface.
3. Provenance — `tenant_id` / `repo_id` / `manifest_generation` / `translator_version` — round-trips with the packet.
4. A subset-claim discipline (analogous to the Sourcegraph subset in [BRIDGE-01](BRIDGE-01.md)) exists for the LQ → CodeQL direction.

None of (1)–(4) ship in v1.

---

## §2 Background

The RFC ticket pack named `BRIDGE-01` "CodeQL bridge and candidate export"; the spec sheet authored under that filename in this directory ([BRIDGE-01.md](BRIDGE-01.md)) instead ships a **Sourcegraph syntax translator** (`sg2lq`). [INDEX.md §3.1 RFC-GAP-BRIDGE-TARGET](INDEX.md) tracks the scope drift; resolution is either (a) author a separate `RFC-BRIDGE-01-CodeQL.md` for the CodeQL target (this file), or (b) amend the RFC ticket pack to retarget `BRIDGE-01` to Sourcegraph.

This file takes path (a) and explicitly defers the CodeQL work to v2.

Producer-authorship correction ([INDEX.md §3.6](INDEX.md)) does not affect the CodeQL bridge directly — it concerns inbound producer-emitted records, not outbound bridge invocations. The bridge is a **downstream sink**, not a producer authority.

The `BridgeTarget::CodeQl(CodeQlQuerySpec)` enum variant is already reserved at the contract level (per [BRIDGE-01 §3.3](BRIDGE-01.md) Inputs table), but `CodeQlQuerySpec` is a placeholder; no wire shape is locked.

---

## §3 Inputs

Subsystem specs (v1 — already shipped; consumed by v2):

- [PRE-CONTRACT-EXT](PRE-CONTRACT-EXT.md) — `LexicalErrorCode` v1, `BridgeCandidatePacket` envelope (Sourcegraph-tagged carriers; CodeQL fields TBD in v2).
- [BRIDGE-01](BRIDGE-01.md) — Sourcegraph subset-table discipline; the CodeQL bridge MUST reuse the bucket-table pattern (adopted / normalized / refused) for query-syntax compatibility.
- [PRE-NORM](PRE-NORM.md) — canonical LQ AST; the source of the candidate set that the CodeQL bridge exports.

v2 dependencies (not yet started):

- A CodeQL invocation surface (out-of-process; protocol TBD — CLI? RPC? Library FFI?).
- A `CodeQlQuerySpec` wire shape pinned in [feature-scope.md § 5.4](../feature-scope.md) (placeholder today).
- An ADR locking the CodeQL reference release version (analogous to the Sourcegraph release pin in [BRIDGE-01 §10](BRIDGE-01.md)).

---

## §4 Deliverables

### 4.1 Shipped (v1)

**NONE.** No CodeQL-specific code, contract, or test exists in v1.

The `BridgeTarget` enum reserves a `CodeQl(_)` slot in the contract crate (per [BRIDGE-01 §3.3](BRIDGE-01.md)), but the payload is an empty placeholder.

### 4.2 Residual gap (this roll-up, v2)

| Gap | Scope | Owner |
|---|---|---|
| `CodeQlQuerySpec` wire shape | Pin fields: `query_path: String`, `qls_version: SemVer`, `parameters: BTreeMap<String, CodeQlValue>`, `result_columns: Vec<String>`, `extension_pack: Option<ExtensionPackPin>` | v2 spec author |
| `BridgeCandidatePacket` CodeQL extension | Add `target: BridgeTarget::CodeQl(CodeQlQuerySpec)` round-trip | contract owner (v2) |
| `into:codeql` directive translator | LQ planner directive lowering to `BridgeInvocation::CodeQl(...)` | bridge owner (v2) |
| CodeQL invocation surface | Out-of-process call protocol — CLI vs RPC vs FFI | v2 ADR |
| LQ→CodeQL subset table | Adopted / normalized / refused buckets for which LQ predicates project into CodeQL queries | v2 spec |
| Provenance round-trip | `tenant_id` / `repo_id` / `manifest_generation` / `translator_version` carried through CodeQL invocation | v2 |
| Typed error taxonomy | Reuse `BRIDGE_*` codes from [BRIDGE-01 §8](BRIDGE-01.md); add `BRIDGE_CODEQL_TARGET_UNAVAILABLE` if invocation is out-of-process | v2 |
| Performance budget | p99 candidate-export ≤ TBD ms; CodeQL execution is out-of-budget (downstream) | v2 SLO owner |
| Conformance corpus | New `UC-BR-CODEQL-*` rows | v2 spec |

### 4.3 v2 entry criteria

Before any v2 work starts:

1. CodeQL reference release tag MUST be pinned in [feature-scope.md § 5.4](../feature-scope.md).
2. Invocation surface ADR MUST land (CLI vs RPC vs FFI).
3. `BridgeCandidatePacket::target` discriminant must be extended; this is a contract bump and forces a `wire_version` increment per [producer-handoff.md §5](../../../ssot/producer-handoff.md).
4. v1 [BRIDGE-01](BRIDGE-01.md) Sourcegraph subset table must be stable (no v1 churn during v2 design).

---

## §5 Implementation steps

**v1 — none.** This roll-up explicitly ships nothing.

**v2 — proposed sequence (placeholder for future authoring):**

1. Land CodeQL reference release pin in [feature-scope.md § 5.4](../feature-scope.md).
2. ADR for invocation surface (CLI vs RPC vs FFI).
3. Pin `CodeQlQuerySpec` wire shape in `quanta-index-contract::lex::bridge`.
4. Author LQ→CodeQL subset table (adopted / normalized / refused).
5. Implement `into:codeql` directive translator (planner-side).
6. Implement `BridgeInvocation::CodeQl` execution path (out-of-process call).
7. Conformance corpus rows `UC-BR-CODEQL-*`.
8. Performance budget guard (`cargo bench` row).

---

## §6 Test plan

**v1 — none.**

**v2 — proposed (placeholder):**

| Suite | What it asserts |
|---|---|
| `quanta-index-lq-bridge-codeql/tests/codeql_spec_round_trip.rs` | `CodeQlQuerySpec` CBOR-round-trips with byte-stable encoding |
| `quanta-index-lq-bridge-codeql/tests/into_codeql_directive.rs` | `into:codeql` directive materializes a `BridgeCandidatePacket::target = CodeQl(...)` |
| `quanta-index-lq-bridge-codeql/tests/lq_to_codeql_subset_table.rs` | LQ predicates project per subset table (adopted/normalized/refused) |
| `quanta-index-lq-bridge-codeql/tests/codeql_provenance.rs` | tenant / repo / gen / translator-version round-trips |
| `quanta-index-lq-bridge-codeql/tests/codeql_target_unavailable.rs` | Out-of-process invocation failure surfaces typed `BRIDGE_CODEQL_TARGET_UNAVAILABLE` |

---

## §7 Observability

**v1 — N/A.**

**v2 — proposed (placeholder):**

- Span `lq.bridge.codeql` opens on `into:codeql` directive routing.
- Metric `lq_bridge_codeql_invocations_total{outcome}` counter.
- Metric `lq_bridge_codeql_candidate_export_ms` histogram.
- Audit event `BRIDGE_CODEQL_INVOKED` with `(tenant_id, repo_id, gen, qls_version)`.

---

## §8 Error scenarios

**v1 — N/A.**

**v2 — proposed:**

- `BRIDGE_CANDIDATE_OVERFLOW` — candidate set exceeds CodeQL invocation cap. (Reuse from `LexicalErrorCode` v1.)
- `BRIDGE_TARGET_UNAVAILABLE` — CodeQL CLI/RPC/FFI absent at invocation. (Reuse from v1.)
- `BRIDGE_PROVENANCE_REJECTED` — downstream sink rejects provenance fields. (Reuse from v1.)
- `BRIDGE_CODEQL_TARGET_UNAVAILABLE` — proposed new code (subsumed by `BRIDGE_TARGET_UNAVAILABLE` if no CodeQL-specific framing needed).

Fail-closed posture: no silent fallback to "candidate set exported without target invocation". Every `into:codeql` either invokes the target or returns typed error.

---

## §9 Performance envelope

**v1 — N/A.**

**v2 — proposed:**

| Path | p99 budget |
|---|---|
| `into:codeql` directive lowering | ≤ 1 ms |
| Candidate-export packet construction | ≤ 5 ms for ≤ 10k candidates |
| Provenance attachment | ≤ 0.5 ms |
| Out-of-process invocation | OUT OF BUDGET (downstream owns) |

Bridge-side export is bounded; CodeQL execution itself is downstream and unbudgeted from the LQ side.

---

## §10 Risks

| ID | Risk | Mitigation |
|---|---|---|
| RU-BRIDGE-CODEQL-1 | CodeQL surface drift between releases | Pin reference release in feature-scope.md (analogous to Sourcegraph pin) |
| RU-BRIDGE-CODEQL-2 | Invocation surface choice (CLI vs RPC vs FFI) churns the contract | ADR locks before implementation |
| RU-BRIDGE-CODEQL-3 | Subset-claim becomes unprovable if CodeQL has no LQ-comparable feature set | v2 may instead ship a "candidate-export-only" mode without bidirectional claim |
| RU-BRIDGE-CODEQL-4 | v2 is never prioritized; the `BridgeTarget::CodeQl` placeholder rots | Mark as `Reserved` until v2 entry criteria land; bumping requires ADR |

---

## §11 Definition of Done (provable sub-checklist)

**v1 DoD — deferred status acknowledged:**

- ✓ This roll-up file exists and closes the [INDEX.md §1.2](INDEX.md) bookkeeping gap
- ✓ Scope explicitly deferred to v2 (no v1 work scheduled)
- ✓ Cross-link to sibling [BRIDGE-01](BRIDGE-01.md) clarifies the name collision

**v2 DoD — full implementation:**

- 🔜 CodeQL reference release tag pinned in [feature-scope.md § 5.4](../feature-scope.md)
- 🔜 Invocation surface ADR landed
- 🔜 `CodeQlQuerySpec` wire shape pinned in contract crate
- 🔜 LQ→CodeQL subset table authored
- 🔜 `into:codeql` directive translator implemented
- 🔜 Out-of-process invocation path implemented
- 🔜 Provenance round-trip asserted in CI
- 🔜 Conformance corpus rows `UC-BR-CODEQL-*` green
- 🔜 Performance budget guard landed
- 🔜 Typed error taxonomy reconciled (no new codes vs reuse of `BRIDGE_*`)
- 🔜 [`quanta-index-lq-bridge-codeql`](../../../../crates) crate created (or merged into existing `quanta-index-lq-bridge` — ADR decides)

This roll-up's v1 status is **`deferred — by-design`**. The v2 DoD becomes active when CodeQL integration is funded.

---

## §12 Open questions

| ID | Question | Owner |
|---|---|---|
| Q-RFC-BRIDGE-CL-1 | Invocation surface — CodeQL CLI (process spawn), local RPC, or FFI binding? | v2 ADR |
| Q-RFC-BRIDGE-CL-2 | CodeQL reference release — track LTS or rolling? | v2 spec author |
| Q-RFC-BRIDGE-CL-3 | Crate placement — extend `quanta-index-lq-bridge` or new `quanta-index-lq-bridge-codeql`? | crate-graph owner |
| Q-RFC-BRIDGE-CL-4 | Bidirectional or one-way? v1 [BRIDGE-01](BRIDGE-01.md) is one-way (Sourcegraph→LQ); CodeQL likely one-way (LQ→CodeQL); but a CodeQL→LQ feedback mode (data-flow seed → LQ candidate refresh) is conceivable. v2 decides. | v2 spec author |
| Q-RFC-BRIDGE-CL-5 | Does CodeQL invocation count against the LQ per-query budget, or is it explicitly out-of-budget (async downstream)? | SLO owner |

---

## §13 References

- [rfc.md §Ticket Pack](../rfc.md#ticket-pack) — RFC `BRIDGE-01` definition (original CodeQL target)
- [INDEX.md §1.2](INDEX.md) — bookkeeping gap that this roll-up closes (as deferred)
- [INDEX.md §3.1 RFC-GAP-BRIDGE-TARGET](INDEX.md) — scope drift (v1 ships Sourcegraph, not CodeQL)
- [INDEX.md §3.6](INDEX.md) — producer-authorship correction (bridge is downstream sink, not authority)
- [producer-handoff.md §5](../../../ssot/producer-handoff.md) — `wire_version` bump policy (any `BridgeTarget` extension forces a bump)
- Sibling (different scope): [BRIDGE-01](BRIDGE-01.md) (Sourcegraph→LQ translator, shipped in v1 under `quanta-index-lq-bridge`)
- Constituent v1 specs (consumed by v2): [PRE-CONTRACT-EXT](PRE-CONTRACT-EXT.md) · [PRE-NORM](PRE-NORM.md)
- [feature-scope.md § 5](../feature-scope.md) — v1 covers Sourcegraph pin; v2 must extend with CodeQL pin
- Code (v1 placeholder reservation): [`crates/quanta-index-lq-bridge`](../../../../crates/quanta-index-lq-bridge) — `BridgeTarget::CodeQl(_)` variant slot reserved
