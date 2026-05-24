# BRIDGE-01 — Sourcegraph ↔ LQ Bridge Translator + Producer-Bridge Surface

> Wave 8 ticket. Source: [rfc.md § BRIDGE-01](../rfc.md), [rfc.md § Claim Discipline](../rfc.md) (claim 8–10 Sourcegraph subset claim), [feature-scope.md § 1.4 Bridge](../feature-scope.md), [feature-scope.md § 1.5 Bridge directives](../feature-scope.md), [usecase.md § UC-BR-01..04](../usecase.md), [dsl.md § 9 Directive grammar](../dsl.md), [implementation-plan.md § 5.16 BRIDGE-01](../implementation-plan.md), [implementation-plan.md Appendix A — FS-GAP-2](../implementation-plan.md).
> Posture: **breaking-first**. No long-lived shims. No silent fallback. No silent widening of subset claim.

---

## 1. Purpose

BRIDGE-01 ships the **Sourcegraph-syntax → LQ-AST translator** and the **producer-bridge wire surface** that anchors the RFC's `Sourcegraph query ⊂ LQ query` claim (RFC § Claim Discipline, item 4 Operational Definitions, claim 8–10).

Two related but distinct surfaces ship in this ticket:

1. **Sourcegraph syntax translator** (`sg2lq`) — takes a Sourcegraph query string as input and produces a canonical `LqQueryV1` AST. This is the **one-way** translation surface that backs the subset claim. The reverse direction (LQ → Sourcegraph syntax) is **explicitly out of scope** because (a) LQ has QI-extensions (`meta.*`, `into:codeql`, `scope:results`, `with:lexical`, `changed:`, `affected:`, etc. — [feature-scope.md § 5.4](../feature-scope.md)) that have no Sourcegraph projection, and (b) the bridge invariant is "Sourcegraph subset translates losslessly into LQ", not "Sourcegraph is bijective with LQ".
2. **Producer-bridge wire surface** (`BridgeCandidatePacket`) — closes [usecase.md GAP-04](../usecase.md) and [implementation-plan.md § 5.16](../implementation-plan.md). Turns a materialized lexical candidate set into a typed downstream-sink payload (CodeQL Phase 1; namespace reserved for additional sinks). This is the `into:codeql` / `scope:results` / `with:lexical` directive wire shape.

The subset claim becomes provable only when the translator+packet pair round-trips every `usecase.md` row tagged `SG=` (identical-to-Sourcegraph) or `SG~` (normalized-from-Sourcegraph) **and** every refused construct surfaces a typed `BRIDGE_UNSUPPORTED_*` error code rather than a degraded result.

Anti-purpose: this ticket does **not** add new lexical semantics, new planner routes, or new executor lanes. It is a **surface-glue ticket**: it binds existing LQ family parser (LEX-01) and existing bridge sink (BRIDGE-01 placeholder ports from PRE-CONTRACT-EXT) to a Sourcegraph-syntax intake plus a typed candidate-packet wire format. New error codes (§8) are recorded for parser/planner refusal of constructs LQ does not implement.

## 2. Background

The RFC's *Compatibility Rules → hard compatibility goal* asserts `Sourcegraph query ⊂ LQ/Core-1.0`. Up to and including Wave 7, the LQ parser accepts canonical LQ syntax — including most Sourcegraph constructs by accident — but there is no **named translator**, no **tagged conformance corpus** asserting the subset claim end-to-end, and no **typed refusal taxonomy** for Sourcegraph constructs LQ refuses to accept.

[feature-scope.md § 5](../feature-scope.md) enumerates which Sourcegraph features are adopted as-is (§ 5.1), normalized (§ 5.2), rejected (§ 5.3), and extended beyond (§ 5.4). [dsl.md § 9](../dsl.md) defines the directive grammar that the bridge wire surface consumes. [implementation-plan.md § 5.16](../implementation-plan.md) names the contract crate ports and the new error codes (`BRIDGE_SINK_REJECTED`, `BRIDGE_CANDIDATE_FORMAT_INVALID`, plus the feature-scope set `BRIDGE_CANDIDATE_OVERFLOW`, `BRIDGE_TARGET_UNAVAILABLE`, `BRIDGE_PROVENANCE_REJECTED`).

[implementation-plan.md Appendix A FS-GAP-2](../implementation-plan.md) records the divergence between feature-scope.md's bridge error set and the RFC's bridge error taxonomy. **This ticket reconciles FS-GAP-2** by locking a canonical bridge error set in §8 below and binding both feature-scope.md and the RFC error taxonomy to that lock.

[usecase.md UC-BR-01..04](../usecase.md) and [usecase.md AC-13](../usecase.md) are currently in `pending` state because no ticket has owned the bridge surface. BRIDGE-01 promotes those rows to green.

Per CLAUDE.md § Agent change posture (`breaking-first`), this ticket does **not** introduce a "best-effort" Sourcegraph translator that silently drops unsupported filters. Every refusal must be typed; every accepted construct must be byte-for-byte reproducible across instances.

## 3. Inputs

### 3.1 Required upstream tickets (must be green before BRIDGE-01 entry)

- `PRE-CONTRACT-EXT` ([implementation-plan.md § 5.1](../implementation-plan.md)) — lands `BridgeCandidatePacket`, `LexicalErrorCode` SCREAMING_SNAKE_CASE enum, `lq_version` field, and the SearchPlaneBridgePort trait stub. GAP-04 resolution must be in place.
- `LEX-01` ([implementation-plan.md § 5.5](../implementation-plan.md)) — LQ canonical parser + AST + printer + canonical-hash. The translator emits `LqQueryV1` from Sourcegraph syntax; if `LEX-01` is not green, the translator has no canonical sink.
- `LEX-02` ([implementation-plan.md § 5.6](../implementation-plan.md)) — global front-door + tenant-id surface, since the bridge packet carries `tenant_id` and `repo_id` for provenance.
- `LEX-03` — `LEX-07` ([implementation-plan.md § 5.7–5.11](../implementation-plan.md)) — executor lanes for the candidate-set source. The bridge does **not** materialize candidates itself; it consumes a materialized `Vec<LexicalCandidate>` from the prior pipeline.

### 3.2 Required reading (must be re-read at ticket start)

- [rfc.md § Compatibility Rules](../rfc.md) — hard compatibility goal, normalization examples.
- [rfc.md § Claim Discipline](../rfc.md) — items 1, 8, 9, 10. Items 1 (Sourcegraph-compatible lexical core), 8 (cross-instance reproducibility), 9 (global fanout), 10 (golden IR-evaluation) all gate on subset claim.
- [rfc.md § Engine Decomposition § Bridge engine](../rfc.md) — bridge owner constraints (must not become authority, must preserve candidate identity).
- [rfc.md § Error Code Taxonomy § BRIDGE_*](../rfc.md) — current RFC error set.
- [feature-scope.md § 1.5](../feature-scope.md) — bridge directive table + error semantics rows.
- [feature-scope.md § 5](../feature-scope.md) — Sourcegraph compat delta (adopted / normalized / rejected / extended).
- [feature-scope.md § 6.5](../feature-scope.md) — bridge authority chain.
- [usecase.md UC-BR-01..04 + AC-13](../usecase.md).
- [dsl.md § 9](../dsl.md) — directive grammar.
- [dsl.md § 10](../dsl.md) — normalization rules (`repo:foo@bar` → `repo:foo rev:bar`, etc.).
- [dsl.md § 14](../dsl.md) — compatibility test corpus reference.

### 3.3 Input data shapes

| Input | Shape | Provider |
|---|---|---|
| Sourcegraph query string | UTF-8 bytes ≤ 16 KiB ([dsl.md § 1.6](../dsl.md)) | producer-bridge intake |
| Sourcegraph reference release tag | static string e.g. `5.4.0` | pinned in [feature-scope.md § 5](../feature-scope.md) and reaffirmed by this ticket; bump policy in §10 |
| `LexicalCandidate` set (post-execution) | `Vec<LexicalCandidate>` ([usecase.md § 0](../usecase.md)) | executor (LEX-05) |
| `PublishedGenerationSet` | per RFC § Atomicity contract | catalog (T1.2 / LEX-04) |
| `BridgeTarget` enum | `{ CodeQl(CodeQlQuerySpec), Reserved(_) }` | contract crate ([implementation-plan.md § 5.1](../implementation-plan.md)) |

## 4. Deliverables

1. **`SourcegraphToLqTranslator`** — pure function `translate(sg: &str) -> Result<LqQueryV1, BridgeTranslationError>`. Lives in `quanta-index-bridge::sg2lq` (crate decision deferred to ADR-015; see §12).
2. **Subset table** — the canonical map of every Sourcegraph filter / directive / leaf, locked at `Sourcegraph reference release = <PIN>` (see §10). Three buckets: **adopted as-is**, **normalized**, **refused**. Locked in this ticket; bumps require an RFC amendment per [feature-scope.md § 5](../feature-scope.md) and [rfc.md § Conformance corpus ownership](../rfc.md).
3. **`BridgeCandidatePacket` wire-shape extension** — adds `source_syntax: Option<String>` (Sourcegraph original-input retention for explainability) plus `translator_version: TranslatorVersion` so a downstream consumer can correlate a refusal with the translator version that emitted it. Closes [usecase.md GAP-04](../usecase.md). Wire format: hand-rolled `impl serde::Serialize` / `Deserialize` per **D18** ([CLAUDE.md § Rule Catalog § Build hygiene](../../../../CLAUDE.md)) — no proc-macro derives.
4. **`SearchPlaneBridgePort::route(packet: &BridgeCandidatePacket) -> Result<BridgeInvocation, LexicalErrorCode>`** — wired implementation backing `into:codeql` / `scope:results` / `with:lexical` directives.
5. **Bridge error taxonomy lock** — canonical set in §8, reconciling FS-GAP-2.
6. **Round-trip golden corpus** — pinned to `tools/ci/conformance/lq/bridge/`. Every Sourcegraph-tagged corpus row (`SG=` and `SG~` per [usecase.md § 0](../usecase.md)) runs Sourcegraph-syntax-input → `sg2lq` → `LqQueryV1` → execute → result. Result shape must equal the reference. Golden output is CBOR-encoded per [dsl.md § 11.1](../dsl.md) so it is byte-stable across instances.
7. **Conformance promotion** — UC-BR-01..04 (currently `pending`) plus AC-13 (currently expected `UNSUPPORTED_COMBO`) promoted to green. UC-BR-04 (`error:BRIDGE_REJECTED`) realigned to the canonical bridge error code per §8.
8. **Negative-test pack** — every Sourcegraph construct in the **refused** bucket of the subset table has at least one golden negative test asserting the typed refusal code.
9. **Performance budget** — `cargo bench` row `bridge_01_translate_p99` asserts ≤ 1 ms p99 for the 100-row Sourcegraph corpus on the CI x86_64 runner.
10. **Sourcegraph version-tag pin** — concrete release tag + Zoekt commit hash recorded in [feature-scope.md § 5](../feature-scope.md) Sourcegraph compatibility delta and re-asserted in this ticket's §10. **No floating reference**.

## 5. Implementation steps (TDD order)

> Each step is "red → green → refactor". No silent skipping.

### Step 5.1 — Reconcile FS-GAP-2 (error taxonomy lock)

1. Open [rfc.md § Error Code Taxonomy § BRIDGE_*](../rfc.md) and propose the canonical set listed in §8.
2. Open [feature-scope.md § 1.5.2](../feature-scope.md) and replace `BRIDGE_TARGET_UNAVAILABLE` / `BRIDGE_PROVENANCE_REJECTED` / `BRIDGE_CANDIDATE_OVERFLOW` with the locked canonical names from §8 (or extend the RFC taxonomy — whichever is cheaper; the lock is in §8 either way).
3. Land the reconciled error names in `LexicalErrorCode` (PRE-CONTRACT-EXT). Update [usecase.md UC-BR-04 + AC-13](../usecase.md) to use the canonical names. Run `python3 tools/ci/lint/lint-doc-paths.py`.

### Step 5.2 — Pin the Sourcegraph reference release tag

1. Add a new subsection to [feature-scope.md § 5](../feature-scope.md) named **§ 5.0 Anchor pin** containing: Sourcegraph release tag, Zoekt commit hash, ISO-date of pin, expected next-bump cadence (default: 6 months). Re-assert this lock in §10 of this ticket.
2. Add a CI check `tools/ci/lint/lint-sg-pin.py` that fails if any `usecase.md` row tagged `SG=` or `SG~` references a divergent Sourcegraph behavior from the pinned release. Drift is reported, not auto-accepted, per [rfc.md § Conformance corpus ownership](../rfc.md).

### Step 5.3 — Author the subset table tests (red)

1. Create `crates/quanta-index-bridge/tests/sg_subset_table.rs` with one test per row of the subset table in §6 of this ticket.
2. Each test asserts one of three outcomes per row:
   - `adopted-as-is`: `translate("<sg syntax>") == LqQueryV1 { ... canonical AST ... }`
   - `normalized`: `translate("<sg syntax>") == translate("<lq canonical>")` after `normalize()` per [dsl.md § 10](../dsl.md)
   - `refused`: `translate("<sg syntax>") == Err(BridgeTranslationError { code: BRIDGE_UNSUPPORTED_FILTER, ... })` with a concrete reason string
3. All tests start red (translator not yet implemented).

### Step 5.4 — Implement `SourcegraphToLqTranslator::translate`

1. Build the translator on top of the LEX-01 parser. Sourcegraph syntax that is already a strict subset of LQ syntax parses directly into `LqQueryV1` with zero rewrites.
2. For normalized rows ([dsl.md § 10 step 6](../dsl.md)): apply the documented desugar (`repo:foo@bar`, `:[X]`, `:[...ARGS]`, `(?i)pat`, lang aliases). The translator MUST delegate to the canonical normalization pass; it MUST NOT carry a parallel normalization implementation.
3. For refused rows: emit `BridgeTranslationError { code: BRIDGE_UNSUPPORTED_FILTER | BRIDGE_UNSUPPORTED_DIRECTIVE | BRIDGE_AMBIGUOUS_FILTER, source_offset, refused_construct, reason }`. Refusal is **fail-closed**: there is no "best matching" silent rewrite.
4. The translator emits its `TranslatorVersion` (= ticket commit hash) on every output so consumers can correlate downstream behavior with translator version.

### Step 5.5 — Wire `BridgeCandidatePacket` round-trip

1. Extend the `BridgeCandidatePacket` wire shape (already landed via PRE-CONTRACT-EXT) with `source_syntax: Option<String>` (the original Sourcegraph input, retained for explanation only — never trusted as authority) and `translator_version`. **D18: hand-rolled serde impls only**; no proc-macro derives ([CLAUDE.md § Rule Catalog § Build hygiene](../../../../CLAUDE.md)).
2. Property-test the round-trip: `packet -> CBOR -> packet` is identity for 10k random valid packets. Run on x86_64 + aarch64 CI matrix per [implementation-plan.md § 8.2](../implementation-plan.md).
3. The packet's `generation_set` MUST equal the per-query pinned generation (T4.2 / D11); no auto-rebase on mid-flight activation per [feature-scope.md Q8](../feature-scope.md). The packet construction asserts this with `debug_assert_eq!`.

### Step 5.6 — Implement `SearchPlaneBridgePort::route`

1. Wire `into:codeql` to a mock CodeQL sink in integration tests. The mock sink accepts a `BridgeInvocation` and emits a recorded transcript for assertion.
2. Wire `scope:results` to resolve a `BridgeResultHandle` (carrier type from PRE-CONTRACT-EXT) to a bounded candidate set.
3. Wire `with:lexical` to assert downstream `candidate-not-authority` discipline ([feature-scope.md § 1.5.1](../feature-scope.md)) — the downstream invocation's payload MUST carry the `WithLexical` marker; absence is a `BRIDGE_PROVENANCE_REJECTED` error.

### Step 5.7 — Promote pending conformance rows

1. UC-BR-01 through UC-BR-04 in [usecase.md](../usecase.md) move from `pending` to active. UC-BR-04's expected error code is realigned to the canonical name per §8.
2. AC-13 (`type:diff` + `into:codeql`) returns `PLAN_UNSUPPORTED_COMBO` per [dsl.md § 9.3](../dsl.md) and [feature-scope.md § AC-13](../usecase.md).
3. Run `cargo test -p quanta-index-contract --test lq_conformance`. Output must include `evidence` for each promoted row per `tools/ci/agent/agent_output.schema.json`.

### Step 5.8 — Routing decision lock (ADR-008 candidate)

1. Decide: do Sourcegraph queries arrive on a separate IPC endpoint, or do they auto-detect on the existing endpoint? Lock in ADR-008 candidate ([implementation-plan.md § 10 ADR-008](../implementation-plan.md) — re-purpose ADR-008 slot from ACL source to "Sourcegraph routing"; if the slot is already taken, allocate a new ADR slot and record cross-reference).
2. Default position (this ticket): **auto-detect on the existing endpoint** using a leading `sg:` magic prefix that the producer strips before forwarding. Magic prefix avoids a second IPC port while keeping LQ-native traffic statistically dominant. Open question §12 Q1 records the lock.

### Step 5.9 — Performance + observability hookup

1. Criterion bench `bridge_01_translate_p99` over the 100-row corpus; asserts p99 ≤ 1 ms ([§9](#9-perf-envelope)).
2. OpenTelemetry span `lq.bridge` emits attributes `{source_syntax_present: bool, translator_version, refusal_code: Option<String>, candidate_count: u32, generation_set: String}`. See [OBS-01](OBS-01.md) for the span-tree integration.

### Step 5.10 — Run conformance + lints + heavy rails

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
just rust-machete
just rust-deny
python3 tools/ci/lint/lint-doc-paths.py
python3 tools/ci/lint/lint-hexagonal-boundaries.py
python3 tools/ci/lint/lint-sg-pin.py
```

All commands must exit `0`. The agent output schema (`tools/ci/agent/agent_output.schema.json`) requires concrete evidence per command; missing evidence is `blocked`, not `ok`.

## 6. Test plan

### 6.1 Subset table tests (canonical)

> Each row generates at least one test. This is the **subset claim conformance corpus** referenced by RFC § Claim Discipline item 1.

| Sourcegraph construct | Bucket | LQ canonical form | Refusal code (if refused) |
|---|---|---|---|
| bare keyword | adopted | `LqExpr::Keyword(_)` | — |
| `"phrase"` | adopted | `LqExpr::Phrase(_)` | — |
| `'raw'` | adopted | `LqExpr::RawString(_)` | — |
| `/regex/` (RE2) | adopted | `LqExpr::Regex(_)` | — |
| `AND`/`OR`/`NOT`/`-` | adopted | `LqExpr::All/Any/Not` | — |
| `repo:` | adopted | `LqFilter::Repo` | — |
| `file:` | adopted | `LqFilter::File` | — |
| `path:` | normalized | `file:` with `scope: PathOnly` per [dsl.md § 6.2](../dsl.md) | — |
| `lang:` | adopted (with alias normalize per [dsl.md § 6.3](../dsl.md)) | `LqFilter::Lang` | — |
| `rev:` | adopted | `LqFilter::Rev` | — |
| `type:` (`file`/`path`/`symbol`/`commit`/`diff`/`repo`) | adopted | `LqFilter::Type` | — |
| `select:` | adopted | `LqFilter::Select` | — |
| `count:<n\|all>` | adopted | `LqOption::Count` | — |
| `case:yes\|no` | adopted | `LqOption::Case` | — |
| `fork:`/`archived:`/`visibility:` | adopted | `LqFilter::Fork/Archived/Visibility` | — |
| `content:` | adopted | `LqFilter::Content` | — |
| `patterntype:literal\|keyword\|standard\|regexp\|structural` | adopted | `LqOption::PatternType` | — |
| `context:` | adopted (parser-only); planner emits typed defer | `LqFilter::Context` | — |
| `boost:` | adopted (parser-only); planner refuses Phase-1 | `LqOption::Boost` | — |
| `index:yes\|only` | adopted | `LqOption::Index` | — |
| `index:no` | **refused** | — | `BRIDGE_UNSUPPORTED_DIRECTIVE` ([dsl.md § 6.2 `index:`](../dsl.md): "this stack is always indexed") |
| `timeout:` | adopted | `LqOption::Timeout` | — |
| `repo:<pat>@rev` sugar | normalized | `repo:<pat> rev:<rev>` per [dsl.md § 10 step 6](../dsl.md) | — |
| `repo:has.file(...)` | adopted | `RepoPredicate::HasFile` | — |
| `repo:has.commit.after(...)` | adopted | `RepoPredicate::HasCommitAfter` | — |
| `repo:has.path(...)` | adopted | `RepoPredicate::HasPath` | — |
| `repo:contains.path(...)` | normalized | `repo:has.path(...)` per [feature-scope.md § 3](../feature-scope.md) | — |
| `repo:contains.commit(...)` | normalized | `repo:has.commit.after(...)` per [feature-scope.md § 3](../feature-scope.md) | — |
| `file:contains(...)` | adopted | `FilePredicate::Contains` | — |
| `file:has.content(...)` | normalized | `file:contains(...)` per [dsl.md § 7.3](../dsl.md) | — |
| `author:`/`committer:`/`message:` | adopted | `LqFilter::Author/Committer/Message` | — |
| `before:`/`after:`/`since:`/`until:` | adopted | `LqFilter::Before/After/Since/Until` ([feature-scope.md Q2](../feature-scope.md) still pending) | — |
| `diff.added/removed/touched:` | adopted | `LqFilter::DiffAdded/Removed/Touched` | — |
| `:[X]` / `:[...ARGS]` | normalized | `$X` / `$...ARGS` per [dsl.md § 8.6](../dsl.md) | — |
| fuzzy `~similar` | **refused** | — | `BRIDGE_UNSUPPORTED_DIRECTIVE` ([rfc.md § Non-Negotiable Invariants item 2](../rfc.md), [feature-scope.md § 5.3](../feature-scope.md)) |
| `r:` / `f:` UI alias shorthand | **refused** | — | `BRIDGE_UNSUPPORTED_FILTER` ([feature-scope.md § 5.3](../feature-scope.md)) |
| `@lang=...` / `@file=...` generic `@` shorthand | **refused** | — | `BRIDGE_UNSUPPORTED_DIRECTIVE` ([rfc.md § `@` policy](../rfc.md), [feature-scope.md § 1.1.8](../feature-scope.md)) |
| saved-query reference | **refused** | — | `BRIDGE_UNSUPPORTED_FILTER` ([feature-scope.md § 3](../feature-scope.md)) |
| named search `context:` evaluation (not parse) | adopted-parse / refused-eval | parser accepts; planner returns `PLAN_DEFERRED` per [dsl.md § 6.2](../dsl.md) | (planner refusal, not translator) |
| ambiguous filter (e.g. SG-future `repo:<x>:<y>` colon-in-value) | **refused** | — | `BRIDGE_AMBIGUOUS_FILTER` (defensive fail-closed — see §8) |

### 6.2 Round-trip golden corpus

- Every `UC-LEX-*`, `UC-PRED-*`, `UC-SYM-*`, `UC-HIST-*`, `UC-STR-*` row tagged `SG=` or `SG~` ([usecase.md § 0](../usecase.md)) executes its Sourcegraph-syntax input through `sg2lq → LqQueryV1 → executor → SearchPlaneLexicalQueryResponse`. Output is compared CBOR-canonically.
- Every refused construct row (anti-usecases AC-01, AC-02, AC-03, AC-05, AC-06, plus the new bridge-specific refusals from §6.1) has at least one golden file.

### 6.3 Negative tests

- `index:no foo` → `BRIDGE_UNSUPPORTED_DIRECTIVE { reason: "search-plane is index-only Phase 1" }`.
- `~fooBar` → `BRIDGE_UNSUPPORTED_DIRECTIVE { reason: "fuzzy by default forbidden" }`.
- `r:foo bar` → `BRIDGE_UNSUPPORTED_FILTER { filter: "r" }`.
- Future SG filter we have not registered (e.g. invented `colorscheme:`) → `BRIDGE_UNSUPPORTED_FILTER { filter: "colorscheme" }`.
- A filter name that is grammar-legal but resolves to two LQ filter targets (defensive) → `BRIDGE_AMBIGUOUS_FILTER { filter, candidates }`.
- `type:diff into:codeql` → `PLAN_UNSUPPORTED_COMBO` (planner-layer; AC-13 row).
- Sourcegraph 5.x feature added after the §10 pin that has not been ratified → translator must fail-closed with `BRIDGE_UNSUPPORTED_FILTER` plus the actual SG release tag in the error payload so the conformance gate's parity drift report ([implementation-plan.md § Glossary](../implementation-plan.md)) names it.

### 6.4 Property tests

- 10k random `LqQueryV1` × `printer.print()` → `sg2lq` must round-trip when the printed form is Sourcegraph-syntax-valid. Failures must produce a typed translator error, not a panic and not a partial AST.
- Tenant isolation: 100 random Sourcegraph queries × 2 tenants → the translator never carries tenant data; the wire surface enforces tenant scope per [rfc.md § Security and Authz Model](../rfc.md).

### 6.5 Cross-instance reproducibility

- Two-process single-binary CI step ([implementation-plan.md § 8.2](../implementation-plan.md)) runs each `SG=` corpus row twice and asserts byte-identical CBOR envelope. Required for RFC § Claim Discipline item 8.

### 6.6 Criterion benches

- `bridge_01_translate_p99` — 100-row corpus translate-only p99 ≤ 1 ms (§9 perf envelope).
- `bridge_01_packet_serialize_p99` — `BridgeCandidatePacket` CBOR round-trip p99 ≤ 200 µs.

## 7. Observability

> Span / metric / log fields are owned by [OBS-01](OBS-01.md). This ticket emits the **bridge subset** so OBS-01 has a full historical record by Wave-8 entry per [implementation-plan.md § 9.1](../implementation-plan.md).

### 7.1 Spans

- Root span: `lq.bridge` — attributes:
  - `bridge.translator_version: String` (commit hash)
  - `bridge.source_syntax_present: bool`
  - `bridge.refusal_code: Option<String>` (one of the §8 codes)
  - `bridge.candidate_count: u32`
  - `bridge.target: String` (e.g. `codeql`)
  - `bridge.generation_set: String` (per-query pin)
  - Inherited attrs: `ticket_id="BRIDGE-01"`, `wave_id="8"`, `tenant_id`, `repo_id`.

### 7.2 Metrics

- `bridge_translate_total{outcome=adopted|normalized|refused, ticket_id, wave_id, tenant_id, repo_id}` — counter; **cardinality budget** caps `tenant_id × repo_id` per the OBS-01 cardinality guard.
- `bridge_translate_duration_ms{ticket_id, wave_id, tenant_id}` — histogram.
- `bridge_packet_size_bytes{ticket_id, wave_id, target}` — histogram.
- `bridge_refusal_total{code, filter_name, ticket_id, wave_id}` — counter; `filter_name` cardinality bounded to the registered filter universe (≤ 100).

### 7.3 Logs

- One structured log line per translate call: `{ticket_id, wave_id, canonical_query_hash, tenant_id, repo_id, translator_version, refusal_code?, candidate_count, latency_ms}`.
- One audit row per route per [rfc.md § Security and Authz Model item 3](../rfc.md): `{tenant_id, user_id, canonical_query_hash, generation_set, latency_ms, result_count, error_code?}`. Audit sink is separate from operational sink.

## 8. Error scenarios

### 8.1 Canonical bridge error set (lock — reconciles FS-GAP-2)

| Code | When fires | Payload | Retry semantics | Source |
|---|---|---|---|---|
| `BRIDGE_UNSUPPORTED_FILTER` | Sourcegraph filter name has no LQ projection | `{filter_name, source_offset, reason}` | not retryable | this ticket |
| `BRIDGE_UNSUPPORTED_DIRECTIVE` | Sourcegraph directive (e.g. `index:no`, fuzzy `~`, generic `@`) refused | `{construct, source_offset, reason}` | not retryable | this ticket |
| `BRIDGE_AMBIGUOUS_FILTER` | Sourcegraph filter resolves to ≥ 2 LQ targets (defensive) | `{filter_name, candidates: Vec<String>}` | not retryable | this ticket |
| `BRIDGE_SINK_REJECTED` | downstream sink (CodeQL etc.) refused the candidate packet | `{sink, reason}` | not retryable | [rfc.md § Error Code Taxonomy](../rfc.md) |
| `BRIDGE_CANDIDATE_FORMAT_INVALID` | packet failed contract-crate validation at route time | `{field, expected, observed}` | not retryable | [rfc.md § Error Code Taxonomy](../rfc.md) |
| `BRIDGE_CANDIDATE_OVERFLOW` | candidate set exceeds downstream capacity ceiling per [feature-scope.md § 1.5.2](../feature-scope.md) and [feature-scope.md § 7](../feature-scope.md) (100,000 hard ceiling) | `{observed, ceiling}` | not retryable | this ticket (reconciliation — adds to RFC taxonomy) |
| `BRIDGE_TARGET_UNAVAILABLE` | downstream target unreachable / timeout | `{target, reason}` | retryable | this ticket (reconciliation) |
| `BRIDGE_PROVENANCE_REJECTED` | downstream rejected candidate identity (e.g. missing `(repo, rev, generation)` triple) | `{candidate_id, missing_field}` | not retryable | this ticket (reconciliation) |
| `BRIDGE_TRANSLATOR_VERSION_SKEW` | producer's translator version disagrees with consumer's expected window per [rfc.md § Migration and Versioning Policy](../rfc.md) | `{producer_version, consumer_window}` | not retryable | this ticket |

> FS-GAP-2 callback (per [implementation-plan.md Appendix A.2](../implementation-plan.md)): this table is the **single canonical lock**. The RFC § Error Code Taxonomy MUST be extended (or the feature-scope.md set MUST be aligned with the RFC's) such that one of the two doc edits ratifies §8.1 verbatim. The bump cannot land in a different PR than this ticket — the lock is meaningless if the docs disagree at any point on the main branch.

### 8.2 Edge-case decision matrix

| Scenario | Decision | Source |
|---|---|---|
| Empty Sourcegraph input | `BRIDGE_UNSUPPORTED_DIRECTIVE { reason: "empty query" }` (matches LQ refusal per [dsl.md § 5.5](../dsl.md) and [usecase.md UC-EDGE-01](../usecase.md)) | this ticket |
| Sourcegraph query that parses successfully but produces zero candidates | not an error; `BridgeCandidatePacket { candidates: vec![] }` per [feature-scope.md § 1.5.2](../feature-scope.md) "candidate set empty" row → `CoreError::NotFound` at the engine boundary | [feature-scope.md § 1.5.2](../feature-scope.md) |
| Mid-flight generation activation while bridge packet in transit | packet carries pinned generation; downstream invocation receives pinned generation; no auto-rebase per [feature-scope.md Q8](../feature-scope.md) | [feature-scope.md Q8](../feature-scope.md) |
| Sourcegraph adds a new filter post-pin that we have not registered | `BRIDGE_UNSUPPORTED_FILTER`; the parity drift report flags it; promotion requires explicit RFC amendment per [rfc.md § Conformance corpus ownership](../rfc.md) | [rfc.md § Conformance corpus ownership](../rfc.md), this ticket §10 |
| Sourcegraph-syntax-tagged corpus row drifts from upstream behavior | conformance gate emits drift entry; never auto-accepted; PR must include the new pin in the same commit per [usecase.md § 6](../usecase.md) versioning policy | [usecase.md § 6](../usecase.md), this ticket §10 |

### 8.3 Failure-classification invariants

- Every refusal path emits exactly one of the §8.1 codes. There is **no untyped error** ([rfc.md § Non-Negotiable Invariants item 8](../rfc.md)).
- No "best-effort" success path for a refused construct ([CLAUDE.md § Agent change posture](../../../../CLAUDE.md): no heuristic success path when authority is absent).
- No silent widening of the subset claim. New Sourcegraph constructs must move from "refused" to "adopted" through an explicit RFC amendment; this ticket does not pre-accept future SG syntax.

## 9. Perf envelope

| Workload | p50 | p95 | p99 | Source |
|---|---|---|---|---|
| `translate(sg)` — 100-row corpus (CPU-only, no I/O) | ≤ 100 µs | ≤ 500 µs | ≤ 1 ms | this ticket |
| `BridgeCandidatePacket` CBOR serialize | ≤ 50 µs | ≤ 120 µs | ≤ 200 µs | this ticket |
| `SearchPlaneBridgePort::route` (end-to-end with mock sink) | ≤ 5 ms | ≤ 50 ms | ≤ 500 ms | [implementation-plan.md § 9.2 Wave 6 exit](../implementation-plan.md) (re-used) |

Hard caps:

- Translate is pure CPU + bounded (no I/O, no async); the `≤ 1 ms p99` is enforced by the `bridge_01_translate_p99` criterion bench. Regression budget per [implementation-plan.md § 8.2](../implementation-plan.md): p99 may not increase > 5% across a wave without an ADR.
- Packet payload size is bounded by [feature-scope.md § 7](../feature-scope.md): default 1,000 candidates, hard ceiling 100,000.

Anti-perf: there is no opt-in to silently shrink the candidate set to fit a downstream sink ([feature-scope.md § 1.5.2](../feature-scope.md), [feature-scope.md § 6.5](../feature-scope.md)). Overflow → `BRIDGE_CANDIDATE_OVERFLOW`.

## 10. Risks

| ID | Risk | Probability | Impact | Mitigation |
|---|---|---|---|---|
| BR-R1 | Sourcegraph version drift breaks subset claim mid-program | H | H | Pin `<SG-release-tag>` + Zoekt commit hash in [feature-scope.md § 5.0 Anchor pin](../feature-scope.md). Bump policy: every 6 months by default; emergency bump requires an RFC amendment per [rfc.md § Conformance corpus ownership](../rfc.md). CI lint `tools/ci/lint/lint-sg-pin.py` (Step 5.2) blocks PRs that reference unpinned behavior. |
| BR-R2 | FS-GAP-2 doc lock not actually atomic (RFC and feature-scope.md drift again post-merge) | M | H | The ticket's exit gate requires §8.1 verbatim to appear in both `rfc.md § Error Code Taxonomy` and `feature-scope.md § 1.5.2`. CI lint `tools/ci/lint/lint-doc-paths.py` cannot enforce content equality across files; add an inline assertion test `crates/quanta-index-bridge/tests/error_taxonomy_lock.rs` that reads both docs and asserts the §8.1 set is present in both. |
| BR-R3 | Translator carries a parallel normalization implementation that drifts from LEX-01 | M | M | Step 5.4 mandates the translator delegate to LEX-01 normalize. Property test in §6.4 asserts equivalence on 10k random inputs. |
| BR-R4 | Bridge candidate packet allows generation skew at downstream sink | L | H | Step 5.5 asserts pinned generation via `debug_assert_eq!`; cross-instance reproducibility CI step ([implementation-plan.md § 8.2](../implementation-plan.md)) covers it. |
| BR-R5 | Translator version skew between producer and consumer | M | M | `translator_version` field on every packet; `BRIDGE_TRANSLATOR_VERSION_SKEW` error code; one-minor-version skew window per [rfc.md § Migration and Versioning Policy](../rfc.md). |
| BR-R6 | D18 hand-rolled serde explosion on `BridgeCandidatePacket` extension | L | M | Size budget per type ≤ 60 LOC of `impl Serialize`/`Deserialize` per [implementation-plan.md § 5.1](../implementation-plan.md). Code-review checklist enforces. |
| BR-R7 | `BRIDGE_AMBIGUOUS_FILTER` over-fires on legal SG syntax we haven't catalogued | L | M | Ambiguous filter is defensive; if conformance corpus catches an over-fire, the offending row demotes to a refusal with explicit reason text. The fail-closed posture is preferred over silent acceptance per CLAUDE.md. |
| BR-R8 | Auto-detect routing (Step 5.8) misclassifies LQ-native traffic as Sourcegraph | L | H | Magic prefix `sg:` is explicit and not a valid LQ keyword start ([dsl.md § 1.4 identifier characters](../dsl.md): identifiers cannot start with a digit, but `sg:` is filter-shaped — collision check in test). Default: prefix is consumed at producer-bridge edge, not at LQ parser. |

## 11. Definition of Done (provable)

Each item is provable via a concrete artifact path. Missing evidence = `blocked`, not `ok`, per [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) and [CLAUDE.md § Claude Supplements](../../../../CLAUDE.md).

1. **`SourcegraphToLqTranslator::translate` shipped.** Provable by: `crates/quanta-index-bridge/src/sg2lq.rs` exists; `cargo test -p quanta-index-bridge` green.
2. **Subset table locked.** Provable by: §6.1 table in this ticket plus matching test in `crates/quanta-index-bridge/tests/sg_subset_table.rs`; every row corresponds to a green or expected-failure test.
3. **`BridgeCandidatePacket` round-trips contract validator.** Provable by: `cargo test -p quanta-index-contract --test bridge_candidate_packet_roundtrip` green; 10k property iterations on x86_64 + aarch64.
4. **CodeQL invocation builder accepts.** Provable by: integration test in `crates/quanta-index-bridge/tests/codeql_route_mock.rs` green with `into:codeql` + mock sink.
5. **UC-BR-01..04 promoted to green.** Provable by: `cargo test -p quanta-index-contract --test lq_conformance --filter UC-BR-` green; corpus rows in `tools/ci/conformance/lq/bridge/` exist and pass.
6. **AC-13 returns `PLAN_UNSUPPORTED_COMBO`.** Provable by: `tools/ci/conformance/lq/bridge/AC-13.toml` exists; `cargo test --filter AC-13` green.
7. **Bridge error set §8.1 verbatim in both RFC and feature-scope.md.** Provable by: `crates/quanta-index-bridge/tests/error_taxonomy_lock.rs` green (reads both docs, asserts the set).
8. **Sourcegraph pin recorded.** Provable by: [feature-scope.md § 5.0 Anchor pin](../feature-scope.md) subsection exists with concrete release tag + Zoekt commit hash + ISO date; `tools/ci/lint/lint-sg-pin.py` green.
9. **Negative tests for refused constructs pass.** Provable by: §6.3 list — one golden file per row in `tools/ci/conformance/lq/bridge/refused/` with expected `BRIDGE_UNSUPPORTED_*` code.
10. **p99 translate ≤ 1 ms.** Provable by: criterion output `bridge_01_translate_p99` p99 ≤ 1000 µs on x86_64 CI runner; bench artifact uploaded.
11. **OTel span `lq.bridge` emits attributes per §7.1.** Provable by: `tools/ci/lint/lint-span-schema.py` green (cardinality + attribute set check) — see OBS-01 §6.3.
12. **No proc-macro serde derives in `quanta-index-bridge`.** Provable by: semgrep rule `rust-no-serde-derive` ([tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml)) green.
13. **Cross-instance reproducibility step covers bridge envelope.** Provable by: CI step `cross-instance-reproducibility` runs the bridge corpus and asserts byte-identical CBOR.
14. **ADR-008 (or successor) recorded for routing decision.** Provable by: `docs/adr/ADR-008-sourcegraph-routing.md` exists with the lock from Step 5.8.
15. **RFC § Claim Discipline item 1 ("Sourcegraph-compatible lexical core") is now provable.** Provable by: every `SG=` and `SG~` row in [usecase.md](../usecase.md) green; subset table §6.1 complete; parity drift report from [implementation-plan.md § Glossary](../implementation-plan.md) is empty or all entries explicitly accepted via RFC amendment commits.

## 12. Open questions

| ID | Question | Default | Forcing function |
|---|---|---|---|
| Q1 | Sourcegraph routing: separate IPC endpoint or auto-detect on existing endpoint? | auto-detect via `sg:` magic prefix (Step 5.8) | ADR-008 candidate ([implementation-plan.md § 10](../implementation-plan.md)) |
| Q2 | Sourcegraph anchor pin bump cadence | 6 months default; emergency bumps require RFC amendment | this ticket §10 (BR-R1) |
| Q3 | `BRIDGE_AMBIGUOUS_FILTER` — keep as defensive over-fail-closed or fold into `BRIDGE_UNSUPPORTED_FILTER`? | keep separate; ambiguous (multiple LQ targets) is a real fail-closed posture distinct from "not in registry" | this ticket §8.1 |
| Q4 | Should `source_syntax` retention be a hard requirement on every packet, or opt-in per request? | hard requirement: explainability requires source — but the field is `Option<String>` to allow systems that strip it for tenant-isolation reasons | this ticket §4.3 |
| Q5 | Per-tenant translator version pinning — required or not? | not required Phase 1; one cluster-wide pin per [rfc.md § Migration and Versioning Policy](../rfc.md) | post-Wave-8 |
| Q6 | `BRIDGE_CANDIDATE_OVERFLOW` recovery — partial packet opt-in via future `partial:allow` directive? | no Phase 1; fail-closed only ([rfc.md § 6.5 failure model](../rfc.md)) | future RFC amendment if downstream sinks force it |
| Q7 | Bridge engine crate location: new `quanta-index-bridge` or in-tree? | ADR-015 ([implementation-plan.md § 10](../implementation-plan.md)) — default new crate; this ticket scopes to new crate but does not block on the ADR landing if the in-tree path is chosen | ADR-015 |
| Q8 | Sourcegraph-syntax rows in `usecase.md` — pin per-row to a SG release, or one cluster-wide pin? | cluster-wide pin in [feature-scope.md § 5.0](../feature-scope.md); per-row override only when a row deliberately diverges via `SG!` parity | this ticket §10 + [usecase.md § 6](../usecase.md) versioning |

## 13. References

- [rfc.md](../rfc.md) — Sourcegraph-class Lexical Kernel RFC. Sections: § Compatibility Rules, § Claim Discipline, § Engine Decomposition § Bridge engine, § Error Code Taxonomy, § Security and Authz Model, § Migration and Versioning Policy, § Conformance corpus ownership, § 6.5 Failure model.
- [feature-scope.md](../feature-scope.md) — § 1.4 Bridge family, § 1.5 Bridge directives, § 1.5.2 Bridge error semantics, § 5 Sourcegraph compatibility delta, § 6.5 Bridge authority chain, § 7 Scale, Q8 bridge generation stability.
- [usecase.md](../usecase.md) — § 0 Error codes, UC-BR-01..04, AC-13, § 3 GAP-04, § 6 Versioning policy.
- [dsl.md](../dsl.md) — § 1.6 Max query length, § 6 Filter semantics, § 7 Predicate sub-grammar, § 8.6 Sourcegraph alias normalization, § 9 Directive grammar, § 10 Normalization rules, § 11 Canonical hash, § 14 Compatibility corpus reference.
- [implementation-plan.md](../implementation-plan.md) — § 5.1 PRE-CONTRACT-EXT (BridgeCandidatePacket, LexicalErrorCode), § 5.16 BRIDGE-01, § 7 Cutover and migration, § 8.2 Cross-cutting rails, § 9.1 Per-wave OBS subset, § 10 ADR-008 / ADR-015, Appendix A.2 FS-GAP-2, § Glossary (parity drift report).
- [OBS-01.md](OBS-01.md) — sibling ticket; observability hookup for `lq.bridge` span and bridge metrics.
- [CLAUDE.md](../../../../CLAUDE.md) — Agent change posture, Rule Catalog (D18 serde derive ban), Claude Supplements.
- [AGENT_RULE_CATALOG.md](../../../../AGENT_RULE_CATALOG.md) — D18 verbatim.
- [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) — structured agent output schema.
- [tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive`.
- [tools/ci/lint/lint-doc-paths.py](../../../../tools/ci/lint/lint-doc-paths.py) — doc path linter.

> End of BRIDGE-01.
