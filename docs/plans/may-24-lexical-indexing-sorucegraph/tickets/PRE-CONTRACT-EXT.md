# PRE-CONTRACT-EXT: Extend the frozen contract crate with GAP-01..06, `lq_version`, and tenant/user carriers

| field | value |
|---|---|
| Status | spec |
| Wave | 0 |
| Owner crate(s) | `quanta-index-contract` (primary); `quanta-index-core` (carrier-type re-exports); producer (`semantica-codegraph-v2`) for coordinated cut |
| Touches contract | yes — every new variant is a wire-format addition; producer must re-publish manifest binders simultaneously; **breaking-first per [CLAUDE.md](../../../../CLAUDE.md) § Agent change posture** |
| Size | M (~5–7d engineer-days; review-heavy because every shape touches CBOR wire) |
| Depends on | none — Wave-0 entry; assumes all four sibling planning docs (rfc / feature-scope / usecase / dsl) are landed |
| Blocks | PRE-NORM, PRE-CONF, LEX-00, LEX-01, LEX-02, LEX-06, LEX-07, STR-01, BRIDGE-01 (every LQ-family ticket carrying contract shape) |

## 1. Purpose

Add the seven contract-surface deltas the LQ family needs before any planner / executor / conformance work begins: symbol kind (GAP-01), commit + diff candidates (GAP-02), structural bindings (GAP-03), bridge candidate packet (GAP-04), explain schema v2 (GAP-05), typed error code enum (GAP-06), plus the `lq_version` discriminator on `LqQuery` and the `tenant_id` / `user_id` carrier surface required by [rfc.md § Security and Authz Model](../rfc.md). Until this ticket lands, every LEX-* / STR-* / RT-* / BRIDGE-* ticket would have to either invent ad-hoc shapes (forbidden — contract is the single producer / search-plane integration surface per [CLAUDE.md](../../../../CLAUDE.md) § Architecture) or stall.

This ticket is the only Wave-0 entry that touches the wire format. It is the producer-sync gate: the producer team coordinates one cut from `LQ/Core-0.x` to `LQ/Core-1.0-pre` ([implementation-plan.md § 7.1](../implementation-plan.md)) and no `#[deprecated]` shim survives the merge.

## 2. Background

Current contract crate at HEAD `736ddea` (per [implementation-plan.md § 2.1](../implementation-plan.md)):

- `LqQuery` exists with `expr / filters / options / directives` ([crates/quanta-index-contract/src/query/expression.rs:11](../../../../crates/quanta-index-contract/src/query/expression.rs#L11)) but **lacks `lq_version`** required by [rfc.md § Migration and Versioning Policy](../rfc.md) item 1.
- `LqExpr` covers `MatchAll / Raw / All / Any / Not` only ([crates/quanta-index-contract/src/query/expression.rs:103](../../../../crates/quanta-index-contract/src/query/expression.rs#L103)). No `Phrase`, `RawString`, `Regex`, `StructuralBlock` variants. PRE-NORM will need these; spec'd here so PRE-NORM has shapes to populate.
- `LqFilter` is string-typed across the eight variants ([crates/quanta-index-contract/src/query/filters.rs:67](../../../../crates/quanta-index-contract/src/query/filters.rs#L67)). [dsl.md § 6.2](../dsl.md) demands typed value grammars (`RepoFilter`, `FileFilter` with scope, `LangId`, `RevSpec`, etc.). This ticket lands the **type carriers**; planner-side wiring is LEX-01 / LEX-02.
- `LqOptionSet` covers only `limit / count_all / timeout_ms` ([crates/quanta-index-contract/src/query/options.rs:9](../../../../crates/quanta-index-contract/src/query/options.rs#L9)). Missing `case`, `patterntype`, `boost`, `index`, `count` distinct from `limit`.
- `LqDirective` carries `IntoCodeQl / ScopeResults / WithLexical / Custom` ([crates/quanta-index-contract/src/query/directives.rs:71](../../../../crates/quanta-index-contract/src/query/directives.rs#L71)) but is not wired and has no typed payload for the bridge.
- `LexicalCandidate` ([crates/quanta-index-contract/src/results/candidates.rs:11](../../../../crates/quanta-index-contract/src/results/candidates.rs#L11)) has no `symbol_kind`, no commit/diff fields, no structural bindings. UC-SYM-02 / UC-HIST-01..08 / UC-STR-01..07 cannot serialize their truth here.
- `SearchExplanation` is `{ summary: String }` only ([crates/quanta-index-contract/src/results/explanation.rs:9](../../../../crates/quanta-index-contract/src/results/explanation.rs#L9)). [usecase.md GAP-05](../usecase.md) demands planner trace + engines touched + early-stop reason.
- `SearchPlaneIpcError { code: String, message: String }` ([crates/quanta-index-contract/src/ipc/envelopes.rs:428](../../../../crates/quanta-index-contract/src/ipc/envelopes.rs#L428)) carries a free-string code. [rfc.md § Non-Negotiable Invariants](../rfc.md) item 8 forbids untyped errors. [usecase.md § 0 Error codes](../usecase.md) + [rfc.md § Error Code Taxonomy](../rfc.md) enumerate the closed SCREAMING_SNAKE_CASE set; this ticket lands it as an enum.
- `SearchPlaneLexicalQueryRequest` has `query + generation` only ([crates/quanta-index-contract/src/ipc/requests.rs:12](../../../../crates/quanta-index-contract/src/ipc/requests.rs#L12)). No `tenant_id` / `user_id`. [implementation-plan.md Appendix A.1 RFC-GAP-5](../implementation-plan.md) flags this; this ticket closes it.
- D18 / [tools/ci/semgrep/rules.yml:124](../../../../tools/ci/semgrep/rules.yml#L124) bans `#[derive(Serialize)]` / `#[derive(Deserialize)]`. Every existing contract type uses **manual `impl Serialize` / `impl<'de> Deserialize<'de>`** — the new types in §4 must follow the same pattern.

Honest gap: **no bridge / no history / no structural surface exists in code today**. This ticket adds the type carriers only; routing, planning, and execution belong to the named follow-on tickets.

## 3. Inputs (preconditions)

- Existing contract surfaces this ticket reads / extends:
  - [crates/quanta-index-contract/src/lib.rs](../../../../crates/quanta-index-contract/src/lib.rs) — module re-exports
  - [crates/quanta-index-contract/src/query/](../../../../crates/quanta-index-contract/src/query/) — `expression.rs`, `filters.rs`, `options.rs`, `directives.rs`, `pin.rs`
  - [crates/quanta-index-contract/src/results/](../../../../crates/quanta-index-contract/src/results/) — `candidates.rs`, `explanation.rs`
  - [crates/quanta-index-contract/src/ipc/](../../../../crates/quanta-index-contract/src/ipc/) — `envelopes.rs`, `requests.rs`, `responses.rs`
- Prior-ticket deliverables: none (Wave-0 entry).
- Proof artifacts that must be green at entry: `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `semgrep --config tools/ci/semgrep/rules.yml`.
- Contract types in scope (every type below must be reachable through `quanta-index-contract::*` at exit): `LexicalErrorCode`, `SymbolKind`, `CommitCandidate`, `DiffCandidate`, `StructuralBinding`, `StructuralBindings`, `BridgeCandidatePacket`, `BridgeTarget`, `SearchExplanationV2`, `PlannerTraceEntry`, `EngineTouched`, `EarlyStopReason`, `LqCanonicalHashV1`, `TenantId`, `UserId`, `LqQueryVersion`.

## 4. Deliverables

### 4.1 New / extended files

- file: `crates/quanta-index-contract/src/results/symbol_kind.rs` (new) — `SymbolKind` enum.
- file: `crates/quanta-index-contract/src/results/candidates.rs` (extend) — add optional `symbol_kind` field to `LexicalCandidate`; **or** introduce `SymbolCandidate` sibling. The decision is forced by Q-UC-1 — default position recorded in §12 is to extend `LexicalCandidate` with `symbol_kind: Option<SymbolKind>` (additive on the wire, single candidate type for the merge stage).
- file: `crates/quanta-index-contract/src/results/commit_candidate.rs` (new) — `CommitCandidate`.
- file: `crates/quanta-index-contract/src/results/diff_candidate.rs` (new) — `DiffCandidate`, `DiffHunkSide` enum.
- file: `crates/quanta-index-contract/src/results/structural.rs` (new) — `StructuralBinding`, `StructuralBindings`, `Span`.
- file: `crates/quanta-index-contract/src/results/bridge.rs` (new) — `BridgeCandidatePacket`, `BridgeTarget`, `BridgeScope`.
- file: `crates/quanta-index-contract/src/results/explanation.rs` (replace body) — `SearchExplanation` v2 with `planner_trace`, `engines_touched`, `early_stop_reason`, `summary` (kept for back-compat in same major).
- file: `crates/quanta-index-contract/src/error_code.rs` (new top-level module) — `LexicalErrorCode` enum, all SCREAMING_SNAKE_CASE; `LexicalQueryError { code: LexicalErrorCode, message: String, position: Option<TokenSpan> }`.
- file: `crates/quanta-index-contract/src/query/expression.rs` (extend) — add `LqQuery.lq_version: LqQueryVersion`; add `LqExpr::Phrase / RawString / Regex / StructuralBlock(StructuralPattern)` variants. (Detailed shape filled in by PRE-NORM; this ticket lands stub variants with a hand-impl serde so PRE-NORM has a target.)
- file: `crates/quanta-index-contract/src/query/filters.rs` (extend) — add `LqFilter::Count / Case / Fork / Archived / Content / Visibility / PatternType / Context / Boost / Index / Timeout / PredicateRepo / PredicateFile / Author / Committer / Message / Before / After / Since / Until / DiffAdded / DiffRemoved / DiffTouched / Changed / Dirty / Stale / Affected / InvalidatedBy / Snapshot / Meta`. Each new variant carries a typed payload struct per [dsl.md § 6.2](../dsl.md) value-grammar table. **The `Custom` variant is removed** (breaking-first: closed filter universe per [dsl.md § 6.1](../dsl.md) general rule "unknown filter name -> `PARSE_UNKNOWN_FILTER`").
- file: `crates/quanta-index-contract/src/query/options.rs` (extend) — add `LqOptionSet.case / patterntype / boost / index` fields. Replace `limit` + `count_all` with `LqOptionSet.count: CountBound { Bounded(NonZeroU32), All }` (one canonical knob).
- file: `crates/quanta-index-contract/src/query/directives.rs` (replace body) — `LqDirective::IntoCodeQl { target: BridgeTarget } / ScopeResults { handle: BridgeResultHandle } / WithLexical`. **The `Custom` variant is removed** (breaking-first: closed directive set per [dsl.md § 9.1](../dsl.md)).
- file: `crates/quanta-index-contract/src/query/identity.rs` (new) — `TenantId(String)`, `UserId(String)`, `LqQueryVersion(String)`, `LqCanonicalHashV1 { digest_hex: String, dsl_version: LqQueryVersion, created_at_unix_ms: u64 }`, `TokenSpan { byte_offset: u32, byte_len: u32 }`, `Span { start_line: u32, start_col: u32, end_line: u32, end_col: u32 }`, `BridgeResultHandle(String)`.
- file: `crates/quanta-index-contract/src/ipc/requests.rs` (extend) — add `tenant_id: TenantId` and `user_id: UserId` to `SearchPlaneLexicalQueryRequest`, `SearchPlaneSemanticQueryRequest`, `SearchPlaneHybridQueryRequest`, `SearchPlaneExplainQueryRequest`. **Required fields** (breaking; no fallback identity per [rfc.md § Security and Authz Model](../rfc.md) item 4).
- file: `crates/quanta-index-contract/src/ipc/envelopes.rs` (extend) — replace `SearchPlaneIpcError { code: String, ... }` with `SearchPlaneIpcError { code: LexicalErrorCode, message: String, position: Option<TokenSpan>, payload: Option<LexicalErrorPayload> }`. `LexicalErrorPayload` is a closed enum carrying the per-code structured payload from [rfc.md § Error Code Taxonomy](../rfc.md) (e.g., `{offset, expected}` for `PARSE_LEX_ERROR`, `{filter_name, value, expected_grammar}` for `PARSE_INVALID_FILTER_VALUE`, `{shard_id, reason}` for `EXEC_SHARD_UNAVAILABLE`).
- file: `crates/quanta-index-contract/src/ipc/responses.rs` (extend) — `SearchPlaneLexicalQueryResponse.results: Vec<LexicalCandidate>` stays; add new `SearchPlaneHistoryQueryResponse { generation: GenerationPin, commits: Vec<CommitCandidate>, diffs: Vec<DiffCandidate> }`, `SearchPlaneStructuralQueryResponse { generation: GenerationPin, results: Vec<StructuralBindings> }`, `SearchPlaneBridgeQueryResponse { packet: BridgeCandidatePacket }`. Add corresponding `SearchPlaneIpcRequest::{History, Structural, Bridge}` and `SearchPlaneIpcResponse::{History, Structural, Bridge}` variants.

### 4.2 New Rust types — manual serde impls (D18)

Each type lands with `impl Serialize` + `impl<'de> Deserialize<'de>` written by hand, mirroring the existing pattern at [crates/quanta-index-contract/src/results/candidates.rs:36](../../../../crates/quanta-index-contract/src/results/candidates.rs#L36). No `#[derive(Serialize)]` / `#[derive(Deserialize)]`.

Sketch (signatures only; full body lives in implementation):

```rust
pub enum SymbolKind {
    Function, Method, Struct, Enum, Trait, Interface, Class, TypeAlias,
    Module, Const, Static, Macro, Field, Variant, Parameter, Local, Other,
}

pub struct CommitCandidate {
    pub candidate_id: String,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub commit_id: String,         // SHA hex
    pub author_name: String,
    pub author_email: String,
    pub committer_name: String,
    pub committer_email: String,
    pub committed_at_unix_s: i64,  // signed: distant-past commits allowed
    pub message: String,
    pub parent_ids: Vec<String>,
    pub score: f32,
}

pub struct DiffCandidate {
    pub candidate_id: String,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub commit_id: String,
    pub repo_relative_path: RepoRelativePath,
    pub hunk_index: u32,
    pub side: DiffHunkSide,        // Added | Removed | Touched
    pub start_line: u32,
    pub end_line: u32,
    pub snippet: String,
    pub score: f32,
}

pub struct StructuralBinding {
    pub metavar: String,           // "$X", "$...ARGS", "..."
    pub span: Span,
    pub captured_text: String,
}

pub struct StructuralBindings {
    pub candidate_id: String,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub repo_relative_path: RepoRelativePath,
    pub pattern_span: Span,
    pub bindings: Vec<StructuralBinding>,
    pub score: f32,
}

pub struct BridgeCandidatePacket {
    pub generation: GenerationPin,
    pub scope: BridgeScope,
    pub target: BridgeTarget,
    pub candidates: Vec<LexicalCandidate>,    // provenance-preserving
    pub canonical_query_hash: LqCanonicalHashV1,
}

pub enum BridgeTarget { CodeQl { invocation_spec: String } }
pub enum BridgeScope { Results { handle: BridgeResultHandle }, Lexical }

pub struct SearchExplanation {
    pub canonical_query_hash: LqCanonicalHashV1,
    pub planner_trace: Vec<PlannerTraceEntry>,
    pub engines_touched: Vec<EngineTouched>,
    pub early_stop_reason: Option<EarlyStopReason>,
    pub summary: String,
}

pub struct PlannerTraceEntry {
    pub stage: String,             // "parse" | "normalize" | "plan" | "exec.fanout" | "merge" | "rerank" | "bridge"
    pub elapsed_us: u64,
    pub note: String,
}

pub enum EngineTouched { LexicalContent, LexicalPath, LexicalSymbol, History, Structural, Runtime, Bridge }

pub enum EarlyStopReason { CountReached, TimeoutSoft, TimeoutHard, Cancelled, ShardUnavailable, None }

pub enum LexicalErrorCode {
    // PARSE_*  (10)
    PARSE_LEX_ERROR, PARSE_SYNTAX_ERROR, PARSE_INVALID_REGEX,
    PARSE_INVALID_UTF8, PARSE_OVERSIZED, PARSE_FORBIDDEN_SYNTAX,
    PARSE_UNKNOWN_FILTER, PARSE_INVALID_FILTER_VALUE,
    PARSE_INVALID_PATTERNTYPE, PARSE_UNSUPPORTED_COMBO,
    // PLAN_*   (4)
    PLAN_UNKNOWN_PREDICATE, PLAN_LIMIT_EXCEEDED,
    PLAN_UNSUPPORTED_COMBO, PLAN_DEFERRED,
    // EXEC_*   (5)
    EXEC_CATALOG_MISS, EXEC_SHARD_TIMEOUT, EXEC_SHARD_UNAVAILABLE,
    EXEC_MERGE_CANCEL, EXEC_REGEX_COMPILE_EXPLOSION,
    // STATE_*  (3)
    STATE_NOT_READY, STATE_STALE_SIBLING, STATE_GENERATION_REGRESSION,
    // AUTHZ_*  (2)
    AUTHZ_TENANT_DENY, AUTHZ_ACL_MISS,
    // BRIDGE_* (5; FS-GAP-2 reconciliation lands all five in this enum)
    BRIDGE_SINK_REJECTED, BRIDGE_CANDIDATE_FORMAT_INVALID,
    BRIDGE_CANDIDATE_OVERFLOW, BRIDGE_TARGET_UNAVAILABLE,
    BRIDGE_PROVENANCE_REJECTED,
}
// 29 variants total — the FULL closed set introduced anywhere in the LQ family.
```

### 4.3 New error codes (where they fire)

This ticket **introduces** the `LexicalErrorCode` enum and the typed envelope. Where each code fires is owned by the consuming ticket (PRE-NORM, LEX-01..05, LEX-07, STR-01, BRIDGE-01) — this ticket's only obligation is that every code listed in [rfc.md § Error Code Taxonomy](../rfc.md) and [usecase.md § 0](../usecase.md) is a variant and round-trips on the wire.

Wire-format note: `LexicalErrorCode` serializes as its SCREAMING_SNAKE_CASE name (string), not as a numeric tag. Hand-impl maps `Serialize` → `serializer.serialize_str(self.as_str())` and `Deserialize` → exact-match on the closed string set.

### 4.4 New invariants (added to RFC § Non-Negotiable)

This ticket does **not** add invariants to the RFC — that is RFC-amendment work owned by LEX-00. It does land the **type carriers** that LEX-00 will lint against:

- `LqQuery.lq_version` is mandatory at construction and at deserialization (no default). Tested in §6.
- `SearchPlaneIpcError.code` deserializes only into a closed `LexicalErrorCode` set; unknown codes fail closed (no string fallback).
- Every request envelope variant carries `tenant_id` + `user_id`; absence fails deserialization.

### 4.5 Items deleted (breaking-first posture)

- `LqFilter::Custom { key, value }` removed ([crates/quanta-index-contract/src/query/filters.rs:75](../../../../crates/quanta-index-contract/src/query/filters.rs#L75)).
- `LqDirective::Custom(String)` removed ([crates/quanta-index-contract/src/query/directives.rs:75](../../../../crates/quanta-index-contract/src/query/directives.rs#L75)).
- `SearchPlaneIpcError { code: String, ... }` replaced; producer must re-serialize against the typed shape.
- `LqOptionSet.limit` + `LqOptionSet.count_all` collapsed into one `count: CountBound`.
- The existing `SearchExplanation { summary }` is replaced by the v2 shape; the `summary` field survives as one of its fields, but the type identity is reused (no v1/v2 dual surface).

### 4.6 New test files

- `crates/quanta-index-contract/tests/serde_roundtrip_new_types.rs` — one `#[test]` per new type asserting `cbor::to_vec` + `cbor::from_slice` is a fixed point on a representative instance.
- `crates/quanta-index-contract/tests/error_code_closed_set.rs` — asserts (a) 29 enum variants are exposed, (b) every SCREAMING_SNAKE_CASE string from [rfc.md § Error Code Taxonomy](../rfc.md) round-trips, (c) an unknown string fails deserialization (no silent default per [CLAUDE.md](../../../../CLAUDE.md) § Safety).
- `crates/quanta-index-contract/tests/lq_query_version_required.rs` — asserts deserializing an `LqQuery` without `lq_version` fails.
- `crates/quanta-index-contract/tests/tenant_user_required.rs` — asserts deserializing any `SearchPlaneLexical/Semantic/Hybrid/ExplainQueryRequest` without `tenant_id` or `user_id` fails.
- `crates/quanta-index-contract/tests/property_contract_roundtrip.rs` — proptest: 1k random instances of each new type round-trip identically through CBOR.

## 5. Implementation steps (strict TDD order)

1. **Write failing test**: `tests/error_code_closed_set.rs::test_29_variants_exposed` — asserts `LexicalErrorCode::all().len() == 29`. **Implement minimal pass**: declare the enum in `src/error_code.rs` with the 29 variants. **Refactor**: pub-use from `src/lib.rs`.
2. **Write failing test**: `tests/error_code_closed_set.rs::test_screaming_snake_roundtrip` — for every variant `v`, asserts `from_str(v.as_str()) == Some(v)` and `cbor::roundtrip(v)`. **Pass**: hand-impl `as_str` and the `FromStr` table; hand-impl `Serialize` as `serialize_str(self.as_str())` and `Deserialize` as an exact-match visitor that returns `de::Error::unknown_variant` on miss.
3. **Write failing test**: `tests/error_code_closed_set.rs::test_unknown_code_rejected` — asserts deserializing CBOR-encoded string `"PARSE_DOES_NOT_EXIST"` into `LexicalErrorCode` fails with an error that mentions the offending name. **Pass**: ensure the visitor's `unknown_variant` carries the name.
4. **Write failing test**: `tests/lq_query_version_required.rs::test_missing_lq_version_fails` — asserts deserializing an `LqQuery` map lacking `lq_version` errors with `missing field`. **Pass**: add `lq_version: LqQueryVersion` to the struct; thread it through the existing `LqQueryVisitor` ([crates/quanta-index-contract/src/query/expression.rs:37](../../../../crates/quanta-index-contract/src/query/expression.rs#L37)).
5. **Write failing test**: `tests/lq_query_version_required.rs::test_unknown_lq_version_rejected` — asserts an `LqQuery` carrying `lq_version = "99.0"` fails with `LexicalErrorCode::PARSE_UNSUPPORTED_COMBO` semantics (note: error code adjudication is PRE-NORM's job; this test only asserts the *string set* of accepted versions is closed: `{"1.0-pre", "1.0"}`). **Pass**: `LqQueryVersion` is a newtype with a closed allow-list at construction.
6. **Write failing test**: `tests/tenant_user_required.rs::test_lexical_request_requires_tenant` — asserts deserializing `SearchPlaneLexicalQueryRequest` without `tenant_id` errors. **Pass**: extend the visitor at [crates/quanta-index-contract/src/ipc/requests.rs:40](../../../../crates/quanta-index-contract/src/ipc/requests.rs#L40) with required `tenant_id` / `user_id`. Update `SearchPlaneSemanticQueryRequest`, `SearchPlaneHybridQueryRequest`, `SearchPlaneExplainQueryRequest` identically (one test per variant).
7. **Write failing test**: `tests/serde_roundtrip_new_types.rs::symbol_kind_roundtrip` — `SymbolKind::Function` CBOR-roundtrips. **Pass**: write the enum + hand-impl serde in `src/results/symbol_kind.rs`; pub-use.
8. **Write failing test**: `tests/serde_roundtrip_new_types.rs::lexical_candidate_with_symbol_kind` — `LexicalCandidate { symbol_kind: Some(SymbolKind::Function), ... }` round-trips and `LexicalCandidate { symbol_kind: None, ... }` round-trips. **Pass**: add `symbol_kind: Option<SymbolKind>` to the struct + extend the visitor at [crates/quanta-index-contract/src/results/candidates.rs:55](../../../../crates/quanta-index-contract/src/results/candidates.rs#L55).
9. **Write failing test**: `tests/serde_roundtrip_new_types.rs::commit_candidate_roundtrip` — minimal `CommitCandidate` round-trips. **Pass**: land `src/results/commit_candidate.rs`.
10. **Write failing test**: `tests/serde_roundtrip_new_types.rs::diff_candidate_roundtrip` — minimal `DiffCandidate` for each `DiffHunkSide` variant round-trips. **Pass**: land `src/results/diff_candidate.rs`.
11. **Write failing test**: `tests/serde_roundtrip_new_types.rs::structural_bindings_roundtrip` — `StructuralBindings` with two `StructuralBinding` entries round-trips; empty-bindings case also round-trips. **Pass**: land `src/results/structural.rs`.
12. **Write failing test**: `tests/serde_roundtrip_new_types.rs::bridge_packet_roundtrip` — `BridgeCandidatePacket` carrying one `LexicalCandidate` + `BridgeTarget::CodeQl` + `BridgeScope::Lexical` round-trips. **Pass**: land `src/results/bridge.rs`.
13. **Write failing test**: `tests/serde_roundtrip_new_types.rs::search_explanation_v2_roundtrip` — `SearchExplanation` with one `PlannerTraceEntry`, one `EngineTouched`, `early_stop_reason = Some(CountReached)`, `summary = "ok"` round-trips. **Pass**: replace body of `src/results/explanation.rs`.
14. **Write failing test**: `tests/serde_roundtrip_new_types.rs::option_set_count_bound_roundtrip` — `LqOptionSet { count: CountBound::All, ... }` and `count: CountBound::Bounded(NonZeroU32::new(100).unwrap())` round-trip. **Pass**: collapse `limit` + `count_all` into `count: CountBound` at [crates/quanta-index-contract/src/query/options.rs:9](../../../../crates/quanta-index-contract/src/query/options.rs#L9).
15. **Write failing test**: `tests/serde_roundtrip_new_types.rs::case_patterntype_index_boost_roundtrip` — `LqOptionSet` carrying `case=CaseOption::Sensitive`, `patterntype=PatternType::Standard`, `index=IndexMode::Only`, `boost=BoostFactor(1.5)` round-trips and rejects `Boost(NaN)` per [dsl.md § 11.1](../dsl.md). **Pass**: add the four fields with hand-impl serde; `BoostFactor` newtype rejects NaN / -0 at deserialize.
16. **Write failing test**: `tests/serde_roundtrip_new_types.rs::filter_typed_payloads` — for each new `LqFilter` variant (RepoFilter, FileFilter, LangId, RevSpec, CountBound, CaseOption, ForkMode, ArchivedMode, ContentFilter, VisibilityMode, PatternType, ContextFilter, BoostFactor, IndexMode, TimeoutOption, PredicateRepo, PredicateFile, Author, Committer, Message, Before, After, Since, Until, DiffAdded, DiffRemoved, DiffTouched, Changed, Dirty, Stale, Affected, InvalidatedBy, Snapshot, Meta), one round-trip test asserts construction + CBOR identity. **Pass**: add variants + hand-impl serde at [crates/quanta-index-contract/src/query/filters.rs:67](../../../../crates/quanta-index-contract/src/query/filters.rs#L67); **delete `Custom` variant**.
17. **Write failing test**: `tests/serde_roundtrip_new_types.rs::directive_typed_payloads` — `LqDirective::IntoCodeQl { target: BridgeTarget::CodeQl { invocation_spec: "..." } } / ScopeResults { handle: BridgeResultHandle("h1") } / WithLexical` round-trip. **Pass**: replace `LqDirective` body at [crates/quanta-index-contract/src/query/directives.rs:71](../../../../crates/quanta-index-contract/src/query/directives.rs#L71); **delete `Custom` variant**.
18. **Write failing test**: `tests/serde_roundtrip_new_types.rs::ipc_error_typed_code` — `SearchPlaneIpcError { code: LexicalErrorCode::PARSE_OVERSIZED, position: Some(TokenSpan{byte_offset:0,byte_len:16}), payload: Some(LexicalErrorPayload::Oversized{observed_bytes:20000,budget_bytes:16384}), message: "..." }` round-trips. **Pass**: replace envelope shape at [crates/quanta-index-contract/src/ipc/envelopes.rs:428](../../../../crates/quanta-index-contract/src/ipc/envelopes.rs#L428); add `LexicalErrorPayload` enum with one structured variant per code that carries payload per [rfc.md § Error Code Taxonomy](../rfc.md).
19. **Write failing test**: `tests/serde_roundtrip_new_types.rs::history_response_roundtrip` — `SearchPlaneHistoryQueryResponse` with one `CommitCandidate` + one `DiffCandidate` round-trips. **Pass**: add response variant in `src/ipc/responses.rs` + IPC variants in envelopes.
20. **Write failing test**: `tests/serde_roundtrip_new_types.rs::structural_response_roundtrip` — `SearchPlaneStructuralQueryResponse` round-trips. **Pass**: add response variant.
21. **Write failing test**: `tests/serde_roundtrip_new_types.rs::bridge_response_roundtrip` — `SearchPlaneBridgeQueryResponse` round-trips. **Pass**: add response variant.
22. **Write failing test**: `tests/serde_roundtrip_new_types.rs::canonical_hash_carrier` — `LqCanonicalHashV1` with a 64-hex-char digest + version `"LQ/Core-1.0"` + a UTC timestamp round-trips. **Pass**: land `LqCanonicalHashV1` in `src/query/identity.rs`. Reject non-hex / non-64-len digests at deserialize.
23. **Write failing test**: `tests/property_contract_roundtrip.rs::proptest_all_new_types` — 1k random instances per new type round-trip. **Pass**: implement `proptest::Arbitrary` (also hand-rolled, no `derive`); wire to runner.
24. **Run** `cargo clippy --workspace --all-targets -- -D warnings`. Fix until green.
25. **Run** `semgrep --config tools/ci/semgrep/rules.yml --error` and confirm `rust-no-serde-derive` ([tools/ci/semgrep/rules.yml:124](../../../../tools/ci/semgrep/rules.yml#L124)) is green; if a derive slipped in, replace with hand-impl.

## 6. Test plan

### 6.1 Unit tests (named, with assertion sketches)

- `error_code_closed_set::test_29_variants_exposed` — asserts the enum cardinality.
- `error_code_closed_set::test_screaming_snake_roundtrip` — every code's wire form round-trips.
- `error_code_closed_set::test_unknown_code_rejected` — deserialize fails on unknown string (no silent fallback).
- `lq_query_version_required::test_missing_lq_version_fails` — `missing field: lq_version`.
- `lq_query_version_required::test_unknown_lq_version_rejected` — closed allow-list.
- `tenant_user_required::test_{lexical,semantic,hybrid,explain}_requires_tenant_id` — one test per request variant.
- `tenant_user_required::test_{lexical,...}_requires_user_id` — one per variant.
- `serde_roundtrip_new_types::*` — one named test per type listed in §4.2.

### 6.2 Integration tests

- `crates/quanta-index-contract/tests/cross_arch_cbor_identity.rs` — CI matrix runs on `x86_64` + `aarch64`; asserts the CBOR bytes for one frozen `LqQuery` instance are byte-identical across architectures. (Required precondition for PRE-NORM's canonical-hash determinism property.)
- `crates/quanta-index-contract/tests/no_dual_surface.rs` — asserts (via reflection-by-test) that `LqFilter::Custom` and `LqDirective::Custom` are absent from the enum — breaking-first proof.

### 6.3 Property tests (proptest invariants)

- `property_contract_roundtrip::proptest_all_new_types` — 1k random instances per new type satisfy `from_slice(to_vec(x)) == x`.
- `property_contract_roundtrip::proptest_error_envelope_payload_pairing` — for every `(code, payload)` pair allowed by [rfc.md § Error Code Taxonomy](../rfc.md), construction is accepted and round-trips; for every disallowed pair (e.g., `EXEC_SHARD_TIMEOUT` with a `PARSE_OVERSIZED` payload), construction is rejected.

### 6.4 Conformance corpus rows greened by this ticket

None directly. PRE-CONF (next ticket) authors the corpus; PRE-CONTRACT-EXT only ensures the shapes the corpus targets are constructible. Indirectly this ticket *unblocks* (i.e., makes the shape exist for) **every** UC-* and AC-* row in [usecase.md § 2](../usecase.md) — explicit list per category:

- UC-SYM-02 (GAP-01)
- UC-HIST-01..08 (GAP-02)
- UC-STR-01..07 (GAP-03)
- UC-BR-01..04 (GAP-04)
- UC-OPS-06 (GAP-05)
- UC-EDGE-01..10 (GAP-06; all 10 typed-error rows)
- UC-OPS-01..02 (GAP-06; `CANCELLED`)
- AC-01..15 (GAP-06; every anti-usecase needs the typed code enum)

### 6.5 Bench guards

None for this ticket (no hot-path). PRE-NORM owns parser bench; LEX-05 owns merge bench.

### 6.6 Loom / sanitizer / Miri coverage

- Miri: every new type's serde roundtrip test must pass under `cargo +nightly miri test -p quanta-index-contract` (no UB; aligns with `just rust-miri` per [CLAUDE.md](../../../../CLAUDE.md) Heavy correctness rail).
- No loom (no concurrency in the contract crate).
- No TSAN / ASAN required (pure data + serde).

## 7. Observability hooks

This ticket emits no spans / metrics / log fields directly (no runtime path). It **enables** every later ticket to emit `{ticket_id="PRE-CONTRACT-EXT", wave_id=0}` against the types it lands. OBS-01 ([implementation-plan.md § 4.9](../implementation-plan.md)) consumes this dimension when computing the per-ticket attribution required by RFC § Observability Requirements.

What this ticket plants for OBS-01:

- `LexicalErrorCode` is the dimension that error-count metrics will key on (closed label set per RFC § Metric schema).
- `SearchExplanation.planner_trace[*].stage` is the dimension that span-tree metrics will key on (closed set: `parse|normalize|plan|exec.fanout|merge|rerank|bridge`).
- `LqCanonicalHashV1.digest_hex` is the audit-trail correlation id ([rfc.md § Audit trail](../rfc.md)).

## 8. Error scenarios (negative tests)

| Adversarial input | Type | Expected typed error / wire failure |
|---|---|---|
| CBOR map for `LqQuery` lacking `lq_version` | deserialize | `de::Error::missing_field("lq_version")` |
| CBOR map for `LqQuery` carrying `lq_version = "0.9"` | deserialize | `LqQueryVersion` construction rejects with `unknown variant`; surfaces as `de::Error` |
| CBOR string `"PARSE_TOTALLY_MADE_UP"` decoded into `LexicalErrorCode` | deserialize | `de::Error::unknown_variant` listing the closed 29-name set |
| CBOR map for `SearchPlaneLexicalQueryRequest` lacking `tenant_id` | deserialize | `de::Error::missing_field("tenant_id")` |
| CBOR map for `SearchPlaneLexicalQueryRequest` lacking `user_id` | deserialize | `de::Error::missing_field("user_id")` |
| CBOR map for `LqFilter` with `kind = "Custom"` | deserialize | `de::Error::unknown_variant("Custom", &[...])` — variant deleted |
| CBOR map for `LqDirective` with `kind = "Custom"` | deserialize | same as above |
| `BoostFactor(NaN)` constructed in Rust | construction | constructor returns `Err(InvalidBoost)`; `Deserialize` on the wire returns `de::Error::custom("boost must be finite, non-zero")` |
| `LqCanonicalHashV1 { digest_hex: "deadbeef" }` (8 hex chars) | construction | `Err(InvalidHash)`; deserialize fails with the same |
| `LexicalErrorPayload::Oversized { observed_bytes: 20, budget_bytes: 100 }` (observed ≤ budget — internally inconsistent) | construction | rejected at construction with `InvalidPayload`; assert by negative unit test |

## 9. Performance envelope

- Per-type CBOR encode: target ≤ 5 µs per `LqQuery` instance of typical shape (≤ 1 KiB raw), validated by `pre_norm_hash_bench` (PRE-NORM, not this ticket). This ticket only asserts *no regression* in `cargo bench` against the baseline.
- Memory: every new type is `Clone + Eq` where possible (`BoostFactor` blocks `Eq` because of `f32` — wrap accordingly; the existing `LexicalCandidate` already uses `PartialEq` for the same reason).
- Disk / network: each new type adds ≤ 1 CBOR map-entry per field; gross frame growth ≤ 25% for a typical `SearchPlaneLexicalQueryRequest`. SLO contribution: none directly — Wave-0 exit gate is "`cargo check --workspace` green + semgrep green + 100 corpus rows blocked accurately" per [implementation-plan.md § 4.1](../implementation-plan.md).

## 10. Risks & mitigations

| ID | Risk | Mitigation | Source |
|---|---|---|---|
| R1 (cite) | Contract churn breaks producer (`semantica-codegraph-v2`) | single-PR coordinated cut, no `#[deprecated]` shim; handoff doc `docs/handoffs/lq-contract-1.0-pre.md` committed in the same PR | [implementation-plan.md § 6 R1](../implementation-plan.md) |
| R7 (cite) | Producer-sync bottleneck — PRE-CONTRACT-EXT PR sits >1 week awaiting producer | weekly producer-team sync; named producer owner on the PR description | [implementation-plan.md § 6 R7](../implementation-plan.md) |
| (wave) | D18 ban + ≈40 new types ⇒ hand-rolled serde LOC explodes | per-type budget ≤ 60 LOC for `impl Serialize + Deserialize`; CI lint counts LOC per new file and warns at 80 | [implementation-plan.md § 4.1 wave risks](../implementation-plan.md) |
| R8 (cite) | semgrep / clippy rail drift breaks CI mid-ticket | pin toolchain in `rust-toolchain.toml`; do not bump rails inside this PR | [implementation-plan.md § 6 R8](../implementation-plan.md) |
| R13 (cite) | CBOR encoding is not byte-deterministic across architectures (f32 endian, map-key sort order) | use a CBOR encoder that emits RFC 8949 §4.2.1 canonical form (deterministic encoding rules); reject NaN / -0 at the `BoostFactor` boundary; cross-arch CI matrix asserts byte identity | [implementation-plan.md § 6 R13](../implementation-plan.md), [dsl.md § 11.1](../dsl.md) |
| (new) | `LexicalErrorPayload` variant set drifts from RFC § Error Code Taxonomy silently | property test pairs every `(code, payload)` against the RFC table; new payload variants require RFC amendment | [rfc.md § Error Code Taxonomy](../rfc.md) |

## 11. Definition of Done (provable)

Each item lists the command + expected output + which proof closes it.

1. **All new types compile in the workspace.**
   - command: `cargo check --workspace`
   - expected: exit 0
   - proof: CI rail green; closes [rfc.md § Claim Discipline](../rfc.md) item 1 (build hygiene).
2. **No `#[derive(Serialize/Deserialize)]` regressions.**
   - command: `semgrep --config tools/ci/semgrep/rules.yml --error crates/quanta-index-contract/`
   - expected: `0 findings`
   - proof: `rust-no-serde-derive` rule at [tools/ci/semgrep/rules.yml:124](../../../../tools/ci/semgrep/rules.yml#L124) reports clean. Closes [CLAUDE.md](../../../../CLAUDE.md) D18.
3. **`LexicalErrorCode` enum exposes exactly 29 SCREAMING_SNAKE_CASE variants.**
   - command: `cargo test -p quanta-index-contract --test error_code_closed_set`
   - expected: all tests pass; `test_29_variants_exposed` confirms cardinality
   - proof: closes GAP-06 ([usecase.md § 3](../usecase.md)).
4. **Unknown error codes fail-closed on the wire.**
   - command: `cargo test -p quanta-index-contract --test error_code_closed_set test_unknown_code_rejected`
   - expected: pass
   - proof: closes [rfc.md § Non-Negotiable Invariants](../rfc.md) item 8 ("no untyped error response").
5. **`LqQuery.lq_version` is mandatory.**
   - command: `cargo test -p quanta-index-contract --test lq_query_version_required`
   - expected: pass
   - proof: closes [rfc.md § Migration and Versioning Policy](../rfc.md) item 1.
6. **`tenant_id` / `user_id` are mandatory on every IPC request.**
   - command: `cargo test -p quanta-index-contract --test tenant_user_required`
   - expected: pass (8 named tests — 4 request shapes × 2 fields)
   - proof: closes RFC-GAP-5 ([implementation-plan.md Appendix A.1](../implementation-plan.md)) and [rfc.md § Security and Authz Model](../rfc.md) item 1.
7. **Every new type round-trips through CBOR.**
   - command: `cargo test -p quanta-index-contract --test serde_roundtrip_new_types && cargo test -p quanta-index-contract --test property_contract_roundtrip`
   - expected: pass
   - proof: closes GAP-01..05.
8. **CBOR identity is byte-stable across architectures.**
   - command: CI matrix `cargo test -p quanta-index-contract --test cross_arch_cbor_identity` on `x86_64-unknown-linux-gnu` + `aarch64-unknown-linux-gnu`
   - expected: identical bytes
   - proof: closes the precondition for PRE-NORM canonical-hash determinism per [dsl.md § 11.4](../dsl.md).
9. **`LqFilter::Custom` and `LqDirective::Custom` are removed.**
   - command: `cargo test -p quanta-index-contract --test no_dual_surface`
   - expected: pass
   - proof: breaking-first per [CLAUDE.md](../../../../CLAUDE.md) § Agent change posture.
10. **Miri reports no UB across new tests.**
    - command: `cargo +nightly miri test -p quanta-index-contract`
    - expected: exit 0
    - proof: aligns with `just rust-miri` from [CLAUDE.md](../../../../CLAUDE.md) Heavy correctness rail.
11. **Producer handoff doc committed in the same PR.**
    - command: `test -f docs/handoffs/lq-contract-1.0-pre.md && python3 tools/ci/lint/lint-doc-paths.py`
    - expected: file exists; doc-path lint exit 0
    - proof: closes [implementation-plan.md § 7.3](../implementation-plan.md) artifact requirement.
12. **`cargo deny` green, `cargo machete` green.**
    - command: `just rust-deny && just rust-machete`
    - expected: exit 0 each
    - proof: closes [CLAUDE.md](../../../../CLAUDE.md) supply-chain / unused-dep rails.

## 12. Open questions

| Q-ID | Question | Forcing function |
|---|---|---|
| Q-UC-1 | Extend `LexicalCandidate` with `Option<SymbolKind>` vs introduce sibling `SymbolCandidate` ([usecase.md § 3 GAP-01](../usecase.md)) | DoD §11.7 (`serde_roundtrip_new_types::lexical_candidate_with_symbol_kind`) cannot be authored without a chosen shape. **Default position recorded by this ticket: extend `LexicalCandidate`** (one candidate type for the merge stage; symbol planner sets the field, others leave `None`). Revisit if STR-01 / LEX-03 review surfaces a concrete reason for a sibling type. |
| Q-UC-2 | `CommitCandidate` / `DiffCandidate` field set ([usecase.md § 3 GAP-02](../usecase.md)) | DoD §11.7 (`commit_candidate_roundtrip` / `diff_candidate_roundtrip`). **Default position**: §4.2 sketch above (parent_ids included, signed `committed_at_unix_s`). LEX-07 may add fields in a later minor bump but **may not remove**. |
| Q-UC-3 | `StructuralCandidate` bindings shape — flat list vs map ([usecase.md § 3 GAP-03](../usecase.md)) | DoD §11.7. **Default**: `Vec<StructuralBinding>` (preserves order; multiple captures of the same `$X` allowed). |
| Q-UC-4 | `BridgeCandidatePacket` shape ([usecase.md § 3 GAP-04](../usecase.md)) | DoD §11.7. **Default**: §4.2 sketch. |
| Q-UC-5 | `SearchExplanation` minimum schema ([usecase.md § 3 GAP-05](../usecase.md)) | DoD §11.7. **Default**: §4.2 sketch (planner_trace + engines + early_stop + summary). LEX-06 wires the populator. |
| Q-UC-6 | `LexicalErrorCode` enum surface (final closed set — does the BRIDGE_* family include `BRIDGE_CANDIDATE_OVERFLOW` and `BRIDGE_TARGET_UNAVAILABLE` and `BRIDGE_PROVENANCE_REJECTED` from [feature-scope.md § 1.5.2](../feature-scope.md), or only the two from RFC § Error Code Taxonomy?) — this is FS-GAP-2 in [implementation-plan.md Appendix A.2](../implementation-plan.md) | DoD §11.3 (`test_29_variants_exposed`). **Default position recorded by this ticket: land all five BRIDGE_* codes (29 total)** and file a follow-up to RFC § Error Code Taxonomy to add the three feature-scope-only codes. The 29 number is load-bearing in DoD §11.3 — if RFC is amended to a different cardinality, this ticket must adjust before merge. |
| (new) | Does `BoostFactor` reject `0.0` in addition to NaN / -0? | DoD §8 (`BoostFactor(NaN)` row) only excludes NaN / -0. **Default**: also reject `0.0` (zero boost is semantically equivalent to deletion, which belongs to `NOT`, not `BOOST`). Confirm with LEX-06 owner before PR. |
| G-CONTROL-LOC | Where does control-plane state physically live ([implementation-plan.md § 2.3a](../implementation-plan.md))? | This ticket is **physically-path-agnostic** for control plane. No file under `quanta-index-control/` (or successor) is touched. Q resolution is owned by ADR-001 / ADR-011 (PRE-NORM start / LEX-07 start) — does not block this ticket's DoD, but the producer handoff doc (DoD §11.11) must name the control-plane crate that ships in the same PR window. |

## 13. References

- Parent RFC: [rfc.md](../rfc.md) — § Canonical Query Model, § Security and Authz Model, § Error Code Taxonomy, § Migration and Versioning Policy, § Non-Negotiable Invariants
- Scope catalog: [feature-scope.md](../feature-scope.md) — § 1.5.2 Bridge error codes, § 4.7 flagged gaps
- Conformance corpus: [usecase.md](../usecase.md) — § 3 contract gaps GAP-01..06, § 0 error code table
- Grammar / DSL: [dsl.md](../dsl.md) — § 6.2 filter value grammars, § 11 canonical hash, § 12 error taxonomy
- Execution plan: [implementation-plan.md](../implementation-plan.md) — § 4.1 Wave 0, § 5.1 PRE-CONTRACT-EXT DoD, § 6 risk register, § 7.1 cutover table, Appendix A.1 RFC-GAP-5
- Agent rules: [CLAUDE.md](../../../../CLAUDE.md) — § Agent change posture, § Rule Catalog (D18 serde-derive ban)
- Forward-blocks: [PRE-NORM.md](PRE-NORM.md), [PRE-CONF.md](PRE-CONF.md)
- Semgrep rule: [tools/ci/semgrep/rules.yml:124](../../../../tools/ci/semgrep/rules.yml#L124) (`rust-no-serde-derive`)
- Agent output schema: [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json)
