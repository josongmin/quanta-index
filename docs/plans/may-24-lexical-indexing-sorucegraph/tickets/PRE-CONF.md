# PRE-CONF: Conformance corpus runner — 85 UC-* + 15 AC-* + CI gate

| field | value |
|---|---|
| Status | shipped |
| Crate | `quanta-index-conformance` |
| Tests | 32 |
| Last verified | 2026-05-25 |
| Wave | 0 |
| Owner crate(s) | `quanta-index-conformance` crate (default-position landed: standalone crate, not in-tree test target) |
| Touches contract | no — consumes `quanta-index-contract` types only |
| Size | M (~5–7d engineer-time; 100 corpus rows, TOML schema, runner, CI rail, drift detection) |
| Depends on | PRE-CONTRACT-EXT (typed `LexicalErrorCode` enum + every new candidate / packet type), PRE-NORM (`parse_normalize_hash` pipeline) |
| Blocks | LEX-01 exit gate, LEX-02..07 exit gates, STR-01, RT-01, BRIDGE-01, SEM-01, SEM-02, OBS-01 (every wave exit gate is gated on this runner reporting accurate `ok` / `error_expected` / `blocked` for the relevant corpus subset) |

> CLI shim shipped: `bin/conformance --corpus PATH --output PATH` is the canonical entry point.

## 1. Purpose

Land the **single source of truth** for "does the LQ family behave as `usecase.md` says it does." The runner walks one TOML file per row of [usecase.md § 2](../usecase.md) + § 4 (100 rows total: 85 UC-* + 15 AC-*), pipelines each through `parse → normalize → hash → execute(StubLqEngine)`, and asserts:

- shape vs. [usecase.md § 0](../usecase.md) result-shape vocabulary,
- error code (closed-set match against `LexicalErrorCode` from PRE-CONTRACT-EXT),
- byte-stable canonical hash across two invocations,
- `parity` token consistency vs. the committed Sourcegraph reference tag.

This ticket exists because every wave exit gate ([implementation-plan.md § 4](../implementation-plan.md)) lists "UC-* / AC-* rows green in PRE-CONF" as a precondition. Without PRE-CONF the gates are unprovable and the program stalls.

Per [usecase.md § 6](../usecase.md), the runner is the CI rail `ci/lq-conformance`. Per [rfc.md § Conformance corpus ownership](../rfc.md), drift from the Sourcegraph reference release is **reported, not auto-accepted**.

## 2. Background

Current state vs. RFC-mandated state:

- **Runner**: absent ([implementation-plan.md § 2.8](../implementation-plan.md) — "Conformance corpus runner absent — PRE-CONF in § 3"). No code, no fixture, no CI rail.
- **Corpus**: spec'd only — [usecase.md § 2](../usecase.md) lists 85 UC-* rows; [usecase.md § 4](../usecase.md) lists 15 AC-* rows. No `.toml` / `.yaml` files exist on disk. [usecase.md § 6 Golden file format](../usecase.md) proposes TOML.
- **CI gate**: [usecase.md § 6](../usecase.md) calls for `cargo test -p quanta-index-contract --test lq_conformance`; this ticket either implements that target or (per ADR-006) creates a new `quanta-index-conformance` test-only crate. [implementation-plan.md § 5.3](../implementation-plan.md) DoD bullet 4 names `ci/lq-conformance` as the CI rail.
- **Agent output schema**: every conformance run validates against [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) — `ok` / `blocked` / `error`. Rows without evidence are `blocked`, not `ok` (per [CLAUDE.md](../../../../CLAUDE.md) § Verification).
- **StubLqEngine**: per [implementation-plan.md § 8.4](../implementation-plan.md) Mock policy, allowed only behind `#[cfg(test)]`; retired per-wave as real engines come online (parser path retires at Wave-1; executor path at Wave-3; ranking path at Wave-4). This ticket lands the initial Stub.
- **Pending / blocked rows**: explicitly enumerated in [usecase.md § 3](../usecase.md) — `UC-RT-08` (`dirty:`) and `UC-BR-01..04` are `blocked` until later waves. The runner must support `blocked` as a terminal verdict per the schema, **not auto-skip** these rows ([CLAUDE.md](../../../../CLAUDE.md) "no `ok` status when required inputs ... remain").

Honest gap: **physical home for the runner is unresolved**. The default position recorded by this ticket is a fresh `quanta-index-conformance` crate (parallels the proposed-but-not-resolved `quanta-index-lq-norm` from PRE-NORM). Final decision is forced by ADR-006 ([implementation-plan.md § 10](../implementation-plan.md)) and is logged in §12.

## 3. Inputs (preconditions)

- Existing surfaces this ticket reads:
  - [usecase.md](../usecase.md) — 85 UC-* + 15 AC-* row specs, error code table, parity column, result-shape vocabulary, golden file format
  - [rfc.md § Conformance Suite Reference](../rfc.md), § Error Code Taxonomy
  - [implementation-plan.md § 4.1 Wave 0](../implementation-plan.md) entry/exit gates, § 5.3 PRE-CONF DoD
  - [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) — `status` enum (`ok` / `blocked` / `error`), required-inputs / assumptions / checks / errors / artifacts fields
  - [tools/ci/lint/lint-doc-paths.py](../../../../tools/ci/lint/lint-doc-paths.py) — markdown link validator (every doc path PRE-CONF emits must resolve)
- Prior-ticket deliverables required green:
  - PRE-CONTRACT-EXT DoD §11 all green (contract types in scope: every new candidate / packet / `LexicalErrorCode`).
  - PRE-NORM DoD §11 all green — specifically §11.4 (canonical hash byte-stable) and §11.7 (15 AC-* rows return expected `LexicalErrorCode`).
- Contract types in scope: `LqQuery`, `LexicalCandidate`, `CommitCandidate`, `DiffCandidate`, `StructuralBindings`, `BridgeCandidatePacket`, `SearchExplanation`, `SearchPlaneLexicalQueryResponse`, `SearchPlaneHistoryQueryResponse`, `SearchPlaneStructuralQueryResponse`, `SearchPlaneBridgeQueryResponse`, `SearchPlaneIpcError`, `LexicalErrorCode`, `LexicalErrorPayload`, `LqCanonicalHashV1`, `TenantId`, `UserId`.

## 4. Deliverables

### 4.1 Format choice — TOML (justification)

ADR-006 default position recorded by this ticket: **TOML**. Per [usecase.md § 6](../usecase.md) "TOML (preferred) — flat keyspace, easy to diff, no YAML indentation hazards." Confirmed reasons:

1. **Determinism on disk** — `toml` crate produces a stable canonical layout; YAML has at least three indistinguishable indentation forms for nested lists.
2. **Diff readability** — one row per `[expected]` field, no significant whitespace.
3. **Editor support** — universal.
4. **No string escape ambiguity** — TOML basic strings `"..."` and literal strings `'...'` map cleanly onto the corpus's distinct query types (literal regex vs phrase).

Trade-off: TOML lacks YAML's referencing / anchoring. The corpus does not need this — each row is self-contained.

### 4.2 New files

- file: `crates/quanta-index-conformance/Cargo.toml` — new crate, dev-only.
- file: `crates/quanta-index-conformance/src/lib.rs` — runner library; pub-uses `ConformanceRunner`, `CorpusRow`, `ConformanceVerdict`, `StubLqEngine`.
- file: `crates/quanta-index-conformance/src/corpus_row.rs` — TOML deserialization into typed `CorpusRow` (hand-impl serde per D18).
- file: `crates/quanta-index-conformance/src/verdict.rs` — `ConformanceVerdict` enum with payload (hand-impl serde).
- file: `crates/quanta-index-conformance/src/runner.rs` — `ConformanceRunner::run_one(row: &CorpusRow) -> ConformanceVerdict`.
- file: `crates/quanta-index-conformance/src/stub_engine.rs` — `StubLqEngine` deterministic fixture (returns canned `LexicalCandidate` / `CommitCandidate` / etc. for known queries; returns `STATE_NOT_READY` for unknown).
- file: `crates/quanta-index-conformance/src/junit.rs` — JUnit XML emitter; one `<testcase>` per row, with `<failure>` / `<skipped>` / `<system-out>`.
- file: `crates/quanta-index-conformance/src/agent_output.rs` — emit one `tools/ci/agent/agent_output.schema.json`-validating report per run.
- file: `crates/quanta-index-conformance/src/drift.rs` — Sourcegraph parity drift detector (loads the reference tag pinned by PRE-CONTRACT-EXT handoff doc, reports drift, does **not** auto-accept).
- file: `crates/quanta-index-conformance/tests/lq_conformance.rs` — `#[test] fn corpus_full()` walks every `.toml`, runs the runner, asserts none are `error`-status (blocked is permitted per the row's declared gate).
- file: `crates/quanta-index-conformance/tests/property_hash_determinism_corpus.rs` — proptest over the corpus: every UC-* row's canonical hash is identical across two runs, x86_64 + aarch64.
- file: `crates/quanta-index-conformance/tests/runner_self_tests.rs` — meta-tests: a synthetic row that *should* pass passes; a synthetic row that *should* fail produces the expected verdict with the expected JUnit XML; the runner's own `parse → normalize → hash → execute` pipeline produces a verdict that validates against the agent output schema.
- file: `usecase-corpus/UC-LEX-01.toml` … `usecase-corpus/AC-15.toml` — one `.toml` per row, 100 files total, placed at the workspace root next to the planning docs. Path is canonical per [usecase.md § 6](../usecase.md) "golden files live under `tools/ci/conformance/lq/` (proposed path, not yet created)" — **this ticket pins the location as `usecase-corpus/`** (rationale: keeps the corpus next to `usecase.md` per [rfc.md § Conformance Suite Reference](../rfc.md) "Corpus location — the corpus lives next to `usecase.md`"; alternative `tools/ci/conformance/lq/` mixes corpus content with CI tooling). Decision recorded in §12 ADR-006.
- file: `tools/ci/conformance/run.sh` — shell entry-point invoked by CI; runs the cargo target + agent_output validation + JUnit XML upload.
- file: `tools/ci/agent/validate_agent_output.py` (already exists) — extend to support the conformance run's report shape.
- file: `justfile` (extend) — add `rust-conformance` recipe: `cargo test -p quanta-index-conformance --test lq_conformance && python3 tools/ci/agent/validate_agent_output.py target/conformance/agent_output.json`.

### 4.3 New Rust types — manual serde impls (D18)

```rust
pub struct CorpusRow {
    pub id: String,                 // "UC-LEX-01" | "AC-05" | ...
    pub title: String,
    pub persona: Option<String>,    // P1..P6 for UC-*; None for AC-*
    pub query: String,              // golden query string
    pub tier: Tier,                 // Core | History | Structural | Runtime | Bridge | Edge | Ops | Anti
    pub engines: Vec<EngineTag>,    // L | P | S | H | T | R | B
    pub parity: Parity,             // SgEq | SgTilde | QPlus | SgBang
    pub gate: GateState,            // active | pending (later wave) | blocked (cross-repo dep)
    pub gating_ticket: Option<String>, // "STR-01" | "BRIDGE-01" | etc.; None if gate=active
    pub tenant_id: String,          // default "tenant-conformance"
    pub user_id: String,            // default "user-conformance"
    pub expected: ExpectedShape,
}

pub enum ExpectedShape {
    Ok {
        shape: Shape,               // Single | Multi | Paginated | Empty
        ordering: Ordering,         // ScoreDesc | RecencyDesc | InsertionOrder
        min_results: Option<u32>,
        max_results: Option<u32>,
        response_kind: ResponseKind, // Lexical | History | Structural | Bridge | Explain
        invariants: Vec<Invariant>, // e.g. all_snippets_contain="fooBar"
    },
    Error {
        code: LexicalErrorCode,     // typed, closed-set match
        position_required: bool,
        payload_match: Option<PayloadMatchPattern>,
    },
    Blocked {
        reason: String,             // human-readable
        gating_ticket: String,      // required when blocked
    },
}

pub enum Invariant {
    AllSnippetsContain(String),
    AllSnippetsMatchRegex(String),
    NoneSnippetContain(String),
    ResultRepoSubsetOf(Vec<String>),
    ResultLangAllOf(String),
    CountExactlyEquals(u32),
    DeterministicByteIdenticalAcrossRuns,    // for UC-OPS-05
    EarlyStopReasonEquals(String),
    ExplanationEnginesTouchedExactly(Vec<EngineTag>),
}

pub enum ConformanceVerdict {
    Ok { row_id: String, elapsed_us: u64, canonical_hash: LqCanonicalHashV1 },
    ErrorExpected { row_id: String, observed_code: LexicalErrorCode },
    ErrorUnexpected { row_id: String, observed_code: LexicalErrorCode, expected_code: LexicalErrorCode, message: String },
    ResultMismatch { row_id: String, invariant: String, detail: String },
    Blocked { row_id: String, gating_ticket: String, reason: String },
    InternalError { row_id: String, detail: String },
}
```

Every type lands with hand-impl `Serialize` + `Deserialize` per D18 / [tools/ci/semgrep/rules.yml:124](../../../../tools/ci/semgrep/rules.yml#L124).

### 4.4 New functions / methods

```rust
pub fn load_corpus_dir(path: &Path) -> Result<Vec<CorpusRow>, CorpusLoadError>;

pub struct ConformanceRunner { /* parser, normalizer, hasher, stub_engine */ }
impl ConformanceRunner {
    pub fn new(stub: StubLqEngine) -> Self;
    pub fn run_one(&self, row: &CorpusRow) -> ConformanceVerdict;
    pub fn run_all(&self, rows: &[CorpusRow]) -> ConformanceReport;
}

pub struct ConformanceReport {
    pub verdicts: Vec<ConformanceVerdict>,
    pub summary: ConformanceSummary,
}

pub struct ConformanceSummary {
    pub total: u32,
    pub ok: u32,
    pub error_expected: u32,    // anti-usecase expected error fired correctly
    pub error_unexpected: u32,
    pub result_mismatch: u32,
    pub blocked: u32,
    pub internal_error: u32,
    pub p99_latency_us: u64,    // measured per ticket attribution
}

impl ConformanceReport {
    pub fn to_junit_xml(&self) -> String;
    pub fn to_agent_output(&self, ticket_id: &str, wave_id: u8) -> AgentOutput;
}
```

### 4.5 New error codes

None introduced. Consumes the closed `LexicalErrorCode` set from PRE-CONTRACT-EXT.

Internal `CorpusLoadError` covers TOML parse failures + schema-validation failures of corpus files themselves — these are **CI failures, not LexicalErrorCode**, because they fail before the pipeline runs.

### 4.6 New invariants

- **No row may flip `error` ↔ `ok` without an RFC amendment** per [usecase.md § 6 Authoring discipline](../usecase.md). Enforced by: every row's `id` is committed once; CI rejects a PR that changes `expected.kind` from `Error` to `Ok` or vice versa unless the PR also touches `rfc.md` or `usecase.md` (lint rule, owned by this ticket).
- **No auto-skip** — `gate = pending` rows produce a `Blocked` verdict with the gating ticket id; they do **not** silently disappear ([CLAUDE.md](../../../../CLAUDE.md) "no silent failure / no silent fallback").
- **Fail-closed on any row that produces wrong result shape** — runner returns `ResultMismatch`, which propagates to `cargo test` exit ≠ 0 ([usecase.md AC-* discipline](../usecase.md)).
- **Per-row p99 latency attribution** — every verdict carries `elapsed_us`; report emits p99 per `(ticket_id, wave_id)` dimension per [implementation-plan.md § 9.3](../implementation-plan.md).

### 4.7 Items deleted

None. PRE-CONF is greenfield.

### 4.8 New test files

Already enumerated in §4.2. Restated for the standard-schema row:

- `crates/quanta-index-conformance/tests/lq_conformance.rs` — drives the full 100-row corpus run via `#[test] fn corpus_full()`.
- `crates/quanta-index-conformance/tests/property_hash_determinism_corpus.rs` — proptest over the corpus.
- `crates/quanta-index-conformance/tests/runner_self_tests.rs` — meta-tests.
- `crates/quanta-index-conformance/tests/junit_xml_shape.rs` — asserts JUnit XML is well-formed and contains one `<testcase>` per row.
- `crates/quanta-index-conformance/tests/agent_output_schema.rs` — asserts emitted `agent_output.json` validates against [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json).
- `crates/quanta-index-conformance/tests/anti_usecase_typed_errors.rs` — for each of the 15 AC-* rows, asserts the observed `LexicalErrorCode` matches the row's expected code exactly (no fuzzy / partial match).
- `crates/quanta-index-conformance/tests/drift_detector.rs` — sanity test: a synthetic row tagged `parity = SgEq` that the stub engine returns a different shape for, produces a drift report (not auto-accepted, exit ≠ 0).

## 5. Implementation steps (strict TDD order)

1. **Write failing test**: `runner_self_tests::test_load_one_toml_row` — load a fixture `tests/fixtures/synthetic_ok.toml` carrying a synthetic `id="SYN-01"` row with `tier=core, expected.kind=ok, shape=multi`; assert `CorpusRow` deserialization succeeds. **Pass**: implement `corpus_row::CorpusRow` + hand-impl `Deserialize` + `load_corpus_dir`.
2. **Write failing test**: `runner_self_tests::test_load_invalid_toml_reports_with_path` — load a malformed `.toml`; assert `CorpusLoadError::TomlParse { path, line, col, message }` carries the file path. **Pass**: `load_corpus_dir` accumulates errors per file.
3. **Write failing test**: `runner_self_tests::test_corpus_row_missing_required_field` — `.toml` lacking `query` returns `CorpusLoadError::MissingField`. **Pass**: visitor enforces required-field set.
4. **Write failing test**: `runner_self_tests::test_synthetic_ok_row_produces_ok_verdict` — run `ConformanceRunner::run_one` against a `SYN-OK-01` row whose query string the `StubLqEngine` knows; assert `ConformanceVerdict::Ok { row_id: "SYN-OK-01", canonical_hash: <stable hex>, .. }`. **Pass**: implement `runner.rs` with `parse → normalize → hash → stub_engine.execute`.
5. **Write failing test**: `runner_self_tests::test_synthetic_error_row_produces_error_expected` — run against `SYN-AC-01` whose `query = "@bad shorthand"` expects `PARSE_FORBIDDEN_SYNTAX`; assert `ErrorExpected { observed_code: PARSE_FORBIDDEN_SYNTAX }`. **Pass**: runner branches on `ExpectedShape::Error`.
6. **Write failing test**: `runner_self_tests::test_synthetic_error_row_unexpected_code` — same row but `StubLqEngine` is misconfigured to return `PARSE_LEX_ERROR` instead; assert `ErrorUnexpected { observed_code: PARSE_LEX_ERROR, expected_code: PARSE_FORBIDDEN_SYNTAX, .. }` and the meta-test exits non-zero. **Pass**: runner asserts exact-match on `LexicalErrorCode`.
7. **Write failing test**: `runner_self_tests::test_blocked_row_emits_blocked_verdict` — synthetic row with `gate = pending`, `gating_ticket = "STR-01"`; assert `Blocked { gating_ticket: "STR-01", .. }`. **Pass**: runner short-circuits before executing if `gate ≠ active`, but **still parses + normalizes + hashes** the query so the canonical-hash invariant is exercised on blocked rows too.
8. **Write failing test**: `runner_self_tests::test_blocked_row_still_hashes_canonically` — same row as #7; assert the row's canonical hash is computed and recorded in the report even though execution is skipped. **Pass**: runner pipeline: parse + normalize + hash always; execute only if `gate=active`.
9. **Write failing test**: `agent_output_schema::test_full_run_validates` — run the runner against `tests/fixtures/synthetic_corpus_5_rows/`; emit `agent_output.json`; assert it validates against [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) via `tools/ci/agent/validate_agent_output.py`. **Pass**: implement `ConformanceReport::to_agent_output` producing `{ status: "ok"|"blocked"|"error", summary, checks: [...], errors: [...], artifacts: [...] }`.
10. **Write failing test**: `agent_output_schema::test_partial_blocked_still_validates` — run a corpus where 3/5 rows are `gate=pending`; assert the report's top-level `status` is `blocked` (not `ok`) per the schema. **Pass**: report-level status escalation rule: `error_unexpected > 0` → `error`; `blocked > 0 && error_unexpected == 0` → `blocked`; else `ok`.
11. **Write failing test**: `junit_xml_shape::test_well_formed_xml` — run against 5 synthetic rows; parse the emitted XML; assert one `<testcase>` per row with `<failure>` / `<skipped>` / no children correctly attached. **Pass**: `junit::to_junit_xml`.
12. **Write failing test**: `junit_xml_shape::test_attributes_per_verdict` — each `<testcase>` has `classname="ConformanceCorpus"`, `name="<row_id>"`, `time="<elapsed_us in seconds>"`. **Pass**: emit attributes; precision = µs → seconds with at least 6 decimals.
13. **Write failing test**: `property_hash_determinism_corpus::proptest_random_subset` — proptest 1k random subsets of the corpus; for each row in each subset, compute the canonical hash twice; assert byte equality. **Pass**: relies on PRE-NORM's already-proven hash determinism; this test is a regression guard at the corpus level.
14. **Write failing test**: `drift_detector::test_sgEq_drift_detected` — synthetic `parity=SgEq` row whose stub engine output deliberately differs from the committed Sourcegraph reference output (loaded from `tests/fixtures/sourcegraph_reference/UC-SYN-01.toml`); assert the report names the row in a `drift` section, status is `blocked` or `error`, **not auto-accepted**. **Pass**: `drift::compare` returns a typed verdict; emit `drift_report.json`.
15. **Author the 100 corpus `.toml` files** at `usecase-corpus/UC-LEX-01.toml` ... `usecase-corpus/AC-15.toml`. Per row, the schema is the standard ([usecase.md § 6 Golden file format](../usecase.md)):
    ```toml
    id = "UC-LEX-01"
    title = "Bare keyword"
    persona = "P5"
    query = "fooBar"
    tier = "core"
    engines = ["lexical_content"]
    parity = "SG="
    gate = "active"
    tenant_id = "tenant-conformance"
    user_id = "user-conformance"

    [expected]
    kind = "ok"
    shape = "multi"
    ordering = "score_desc"
    min_results = 1
    response_kind = "Lexical"

    [[expected.invariants]]
    kind = "all_snippets_contain"
    value = "fooBar"
    ```
    Author each row by walking [usecase.md § 2 A–I tables](../usecase.md) + § 4 AC table; each row → one `.toml` file. Specific deliverable shapes per [usecase.md](../usecase.md) annotated columns. Rows that are gated on a later wave receive `gate = "pending"` and `gating_ticket = "<TICKET>"`. The two explicit blocked sets:
    - `UC-RT-08` → `gate = "blocked"`, `gating_ticket = "RT-01"` (depends on Q-FS-5 cross-repo coordination per [feature-scope.md § 1.4.1](../feature-scope.md))
    - `UC-BR-01..04` → `gate = "pending"`, `gating_ticket = "BRIDGE-01"` (Wave 6)
    Additional gating per [usecase.md § 3](../usecase.md) "cannot be expressed in the current LQ family without extension":
    - `UC-OPS-06` (explain output) → `gate = "active"` post-PRE-CONTRACT-EXT (SearchExplanation v2 lands)
    - `UC-EDGE-04`, `UC-EDGE-09`, `UC-EDGE-10`, `AC-08`, `AC-11`, `AC-12`, `AC-14`, `AC-15` — see §6.4 table for gating per row
16. **Write failing test**: `lq_conformance::corpus_full` — load `usecase-corpus/`; run the runner; assert `error_unexpected + result_mismatch + internal_error == 0`. **Pass**: rows authored in step 15 plus the runner from steps 1–14 should produce only `ok` / `error_expected` / `blocked` verdicts. Tune the stub engine (canned candidate fixtures) until green.
17. **Write failing test**: `anti_usecase_typed_errors::test_ac_*` — one `#[test]` per AC-* row; assert the runner observes the exact `LexicalErrorCode` named in [usecase.md § 4](../usecase.md). For AC-08 / AC-11 / AC-12 / AC-14 / AC-15 (not parser-reachable per PRE-NORM §6.4), assert `Blocked` with the named gating ticket. **Pass**: derived from corpus rows; no new logic.
18. **Write failing test**: `lq_conformance::no_silent_skip` — assert that no row whose `gate = "active"` is reported as `Blocked`. **Pass**: meta-rule: `Blocked` is reachable only from `gate ≠ "active"`.
19. **Author** `tools/ci/conformance/run.sh` — `set -euo pipefail`; runs cargo + agent_output validator; uploads JUnit XML as a build artifact (`target/conformance/lq-conformance.junit.xml`).
20. **Wire** `justfile`: `rust-conformance: cargo test -p quanta-index-conformance --test lq_conformance && python3 tools/ci/agent/validate_agent_output.py target/conformance/agent_output.json`.
21. **Add CI job** `ci/lq-conformance` to `.github/workflows/correctness.yml` (or the active workflow). PR-blocking. Uses `./scripts/cargow` per [tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) `workflow-use-cargow` rule.
22. **Run** `python3 tools/ci/lint/lint-doc-paths.py` on the 100 new `.toml` files? **No** — `.toml` is not in `DOC_SUFFIXES` per [tools/ci/lint/lint-doc-paths.py:9](../../../../tools/ci/lint/lint-doc-paths.py#L9). The linter ignores them. The ticket only needs to assert the 3 new ticket `.md` files in this PR pass the linter (see DoD §11.11).
23. **Run** `cargo clippy --workspace --all-targets -- -D warnings && semgrep --config tools/ci/semgrep/rules.yml --error`. Fix until green.
24. **Run** `python3 tools/ci/agent/validate_agent_output.py target/conformance/agent_output.json` against a real run. Expected: validates clean.
25. **Document** the runner in a one-page `crates/quanta-index-conformance/README.md` (allowed: explicitly requested as a deliverable artifact — see DoD §11.4; this is the only doc this ticket creates beyond the corpus).

## 6. Test plan

### 6.1 Unit tests

- Per-type round-trip: `CorpusRow`, `ConformanceVerdict`, `ExpectedShape`, `Invariant` — assert TOML / CBOR round-trip identity.
- Per-verdict: `Ok`, `ErrorExpected`, `ErrorUnexpected`, `ResultMismatch`, `Blocked`, `InternalError` each have a constructor test.

### 6.2 Integration tests

- `lq_conformance::corpus_full` — runs the full 100-row corpus. CI gate.
- `lq_conformance::no_silent_skip` — assertion described in step 18.
- `agent_output_schema::test_full_run_validates` — output validates against [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json).
- `junit_xml_shape::*` — JUnit XML shape.
- `anti_usecase_typed_errors::test_ac_*` — 15 named tests, one per AC-* row.
- `drift_detector::test_sgEq_drift_detected` — drift report emitted, not auto-accepted.

### 6.3 Property tests

- `property_hash_determinism_corpus::proptest_random_subset` — 1k random subsets of the corpus; hash stability invariant. Covers the implementation-plan.md § 4.1 Wave-0 exit requirement: "PRE-NORM emits a stable hash for every of the 85 UC-* rows; idempotency invariant ... holds in property tests (1k cases)" — this ticket is the *running surface* that proves the requirement at the corpus level.
- `property_verdict_total_function::proptest_arbitrary_row` — invariant: `run_one` is total — for any well-typed `CorpusRow`, it returns a `ConformanceVerdict`, never panics, never blocks indefinitely.
- `property_no_silent_skip::proptest_arbitrary_gate` — invariant: `gate = "active"` ⇒ verdict ∈ {`Ok`, `ErrorExpected`, `ErrorUnexpected`, `ResultMismatch`, `InternalError`}; `gate ≠ "active"` ⇒ verdict = `Blocked`. No cross-bleed.

### 6.4 Conformance corpus rows greened by this ticket (cite UC-* / AC-* IDs)

This ticket greens **the runner's ability to report verdicts on every row**, not the row's underlying behavior. Per-row green status depends on which wave's executor has landed. At end-of-Wave-0, the expected verdict distribution is:

| Row group | Expected verdict at Wave-0 exit | Notes |
|---|---|---|
| `UC-LEX-01..28` (28 rows) | `Blocked { gating_ticket = "LEX-01" }` for any row needing the *real* executor; `ErrorExpected` for parser-rejection rows (none in UC-LEX-*); `Ok` is **not** achievable at Wave-0 because StubLqEngine is not the real lexical content engine. → Each row's `gate` flips from `pending` to `active` at Wave-2 / Wave-3 / Wave-4 per [implementation-plan.md § 4](../implementation-plan.md). | At Wave-0 PRE-CONF can validate `parse → normalize → hash` for these rows; canonical hash stability is asserted here. |
| `UC-PRED-01..07` (7 rows) | `Blocked { gating_ticket = "LEX-03" }` (predicate eval is Wave-2) | |
| `UC-SYM-01..06` (6 rows) | `Blocked { gating_ticket = "LEX-03" }` | |
| `UC-HIST-01..08` (8 rows) | `Blocked { gating_ticket = "LEX-07" }` (Wave-4) | |
| `UC-STR-01..07` (7 rows) | `Blocked { gating_ticket = "STR-01" }` (Wave-5) | |
| `UC-RT-01..07` (7 rows) | `Blocked { gating_ticket = "RT-01" }` (Wave-5) | |
| `UC-RT-08` (1 row, `dirty:`) | `Blocked { gating_ticket = "RT-01" }` with cross-repo dependency note per [feature-scope.md § 1.4.1 Q5](../feature-scope.md) | Per [usecase.md § 3](../usecase.md). |
| `UC-BR-01..04` (4 rows) | `Blocked { gating_ticket = "BRIDGE-01" }` (Wave-6) | Per [usecase.md § 3](../usecase.md). |
| `UC-EDGE-01..03, 06, 10` (5 rows — parser-reachable) | `ErrorExpected` ✓ (these are parse-time rejections; PRE-NORM already emits the codes) | |
| `UC-EDGE-04, 05` (oversized + commit+structural combo) | `ErrorExpected` ✓ | both parser-reachable |
| `UC-EDGE-07, 08, 09` (generation mismatch, ACL miss, multi-tenant) | `Blocked { gating_ticket = "LEX-02" / "LEX-04" / "LEX-05" }` | not parser-reachable |
| `UC-OPS-01, 02` (load + cancel) | `Blocked { gating_ticket = "LEX-05" }` | runtime concerns |
| `UC-OPS-03..05, 07` | `Blocked { gating_ticket = "LEX-05" / "LEX-06" }` | |
| `UC-OPS-06` (explain output) | `Blocked { gating_ticket = "LEX-06" }` post-PRE-CONTRACT-EXT — schema lands but populator does not. | |
| `AC-01..06, 09, 10, 13` (10 rows — parser-reachable per PRE-NORM §6.4) | `ErrorExpected` ✓ | this is the *concrete green* for PRE-CONF at Wave-0 exit |
| `AC-07` (structural recursion cap) | `ErrorExpected` ✓ (PRE-NORM emits `PLAN_LIMIT_EXCEEDED`) | |
| `AC-08, 11, 12, 14, 15` (5 rows — not parser-reachable) | `Blocked { gating_ticket = "LEX-07" / "LEX-01" / "LEX-02" / "LEX-02" / "LEX-04" }` | |

Net at Wave-0 exit: **15 `ErrorExpected` (parser-rejection rows) + 85 `Blocked` (every row gated on later waves) = 100 verdicts emitted, all *accurate*.** No row is `Ok` at Wave-0; no row is `ErrorUnexpected` / `ResultMismatch` / `InternalError`. This matches [implementation-plan.md § 4.1 exit gate](../implementation-plan.md): "PRE-CONF runner is wired ... runs against a `StubLqEngine` and reports 100 rows as `blocked` (status accurate, not `ok`)."

### 6.5 Bench guards

- Criterion bench `pre_conf_runner_bench` — full 100-row corpus run completes in < 5s wall-clock locally. CI matrix budget: < 30s on a constrained runner. Regression budget per [implementation-plan.md § 8.2](../implementation-plan.md): 5% growth per wave allowed.

### 6.6 Loom / sanitizer / Miri

- Miri on `runner.rs` + `corpus_row.rs`: pass. Runner is single-threaded by default; no UB expected.
- No loom (no concurrency).
- No TSAN / ASAN (covered by Miri).

## 7. Observability hooks

Per [implementation-plan.md § 9.1 Wave 0](../implementation-plan.md): no OTel spans, no metrics, no audit log are emitted yet. PRE-CONF lands the **carrier** for `{ticket_id, wave_id}` attribution that OBS-01 will consume:

- Every emitted `ConformanceVerdict` carries `row_id` + `gating_ticket` (when blocked) + `elapsed_us`.
- `ConformanceReport.summary` carries per-(ticket, wave) latency p99 dimensions: `p99_latency_us_by_ticket: BTreeMap<String, u64>`.
- The agent output report emits `checks` per row (one `check` object per verdict per [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) `checks[]` schema).

Wave-1 will start emitting `lq.parse` / `lq.normalize` spans through this runner; PRE-CONF guarantees the runner is the **single chokepoint** through which those spans flow.

## 8. Error scenarios (negative tests)

| Adversarial input | Expected verdict / failure |
|---|---|
| `.toml` row missing required `id` field | `CorpusLoadError::MissingField { field: "id", path: ... }`; test load aborts with non-zero exit |
| `.toml` row with `expected.code = "PARSE_NOT_A_REAL_CODE"` | `CorpusLoadError::UnknownErrorCode` (closed-set check at load time, before runner starts) |
| `.toml` row with two `id` keys (duplicate) | `CorpusLoadError::DuplicateField` |
| Two `.toml` files with identical `id` | `CorpusLoadError::DuplicateRowId { id }` |
| `gate = "active"` row whose `StubLqEngine` returns an unexpected `LexicalErrorCode` | `ErrorUnexpected`; report status escalates to `error`; CI exit non-zero |
| `gate = "active"` row whose result fails an `[[expected.invariants]]` check | `ResultMismatch`; report `error`; CI exit non-zero |
| `gate = "active"` row whose runner panics internally | caught by `std::panic::catch_unwind`; verdict = `InternalError`; CI exit non-zero (no silent recovery) |
| `gate = "pending"` row with no `gating_ticket` | `CorpusLoadError::MissingField { field: "gating_ticket" }` at load |
| `gate = "pending"` row whose runner is invoked anyway (bug) | property test `property_no_silent_skip` fails; CI exit non-zero |
| Drift detected vs Sourcegraph reference (synthetic `parity=SgEq` row whose stub output diverges) | `drift_report.json` emitted; report status escalates to `blocked` or `error`; **not** auto-accepted |
| Row flips `expected.kind` from `ok` to `error` (or vice versa) in a PR that does not touch `rfc.md` or `usecase.md` | lint rule (owned by this ticket) fails the PR |
| Adversarial `.toml` with deeply-nested arrays > 32 deep | TOML parser's own depth check fires; surfaces as `CorpusLoadError::TomlParse` |

## 9. Performance envelope

- Full-corpus run: < 5s wall-clock locally (8-core dev machine); < 30s on constrained CI runner.
- Per-row p99: < 50ms wall-clock (parse + normalize + hash + stub-execute). At Wave-3+ when real engines come online, per-row p99 SLO from [rfc.md § Latency SLOs](../rfc.md) applies.
- Memory: < 256 MiB peak for full-corpus run (allows for 100 rows × small stub fixtures + report aggregation).
- Wave-0 exit SLO ([implementation-plan.md § 4.1](../implementation-plan.md)): "PRE-CONF runner is wired as `cargo test -p quanta-index-contract --test lq_conformance`; runs against a `StubLqEngine` and reports 100 rows as `blocked` (status accurate, not `ok`)." → DoD §11 below.

Note: [implementation-plan.md § 4.1](../implementation-plan.md) names the test target as `cargo test -p quanta-index-contract --test lq_conformance`. This ticket pins the target as `cargo test -p quanta-index-conformance --test lq_conformance` per ADR-006 default (new crate, not test inside the contract crate). The discrepancy is logged as a follow-up against the plan doc (§12 ADR-006 forcing function): one of the two must change.

## 10. Risks & mitigations

| ID | Risk | Mitigation | Source |
|---|---|---|---|
| R6 (cite) | Conformance corpus rot (queries valid today, broken silently tomorrow) | `ci/lq-conformance` blocks PRs; drift detection vs Sourcegraph reference tag is mandatory and **not** auto-accepted; one-PR-per-row-change rule for `expected.kind` flip | [implementation-plan.md § 6 R6](../implementation-plan.md) |
| R10 (cite) | Sourcegraph ⊂ LQ claim correctness | every row carries `parity` token; `SG=` → `SG~` flip requires explicit RFC amendment ([usecase.md § 6 Versioning policy](../usecase.md)); the drift detector reports drift but does not auto-accept | [implementation-plan.md § 6 R10](../implementation-plan.md) |
| R15 (cite) | New crate proliferation (PRE-CONF's `quanta-index-conformance` is the third new crate after `quanta-index-lq-norm` and `quanta-index-channel`) breaks hexagonal lint | every new crate is reviewed against `ALLOWED_CRATE_DEPS` in the same PR; PRE-CONF's deps are pinned to `quanta-index-contract` + dev-deps only | [implementation-plan.md § 6 R15](../implementation-plan.md) |
| (new) | StubLqEngine drift — when real engines come online (Wave-3 / Wave-4 / Wave-5 / Wave-6), some rows may flip from `Blocked` to `ErrorUnexpected` if Stub's canned outputs disagreed with real-engine outputs | each row's `gate` flip is owned by the wave's exit gate; the wave's owner must update both the corpus row and retire the Stub for that surface; CI rail enforces no row can become `Ok` without the gate being `active` | this ticket |
| (new) | TOML corpus files are easy to introduce inconsistently (e.g., two rows with same `id`) | `CorpusLoadError::DuplicateRowId` test; CI fails on duplicate; PR template asks the author to confirm `id` uniqueness | this ticket §8 |
| (new) | Agent output schema drift breaks the runner mid-program | `agent_output.json` is validated by `tools/ci/agent/validate_agent_output.py` on every conformance run; schema-version pin in `agent_output.schema.json` is enforced | this ticket §6.2 |

## 11. Definition of Done (provable)

All 13 rows shipped (32 tests in `quanta-index-conformance`).

1. ✓ shipped — **Runner compiles + tests pass.**
   - command: `cargo test -p quanta-index-conformance --test lq_conformance && cargo test -p quanta-index-conformance --test runner_self_tests && cargo test -p quanta-index-conformance --test anti_usecase_typed_errors && cargo test -p quanta-index-conformance --test junit_xml_shape && cargo test -p quanta-index-conformance --test agent_output_schema && cargo test -p quanta-index-conformance --test drift_detector && cargo test -p quanta-index-conformance --test property_hash_determinism_corpus`
   - expected: exit 0 for each
   - proof: closes [implementation-plan.md § 5.3 DoD bullet 1, 4](../implementation-plan.md).
2. ✓ shipped — **All 100 corpus `.toml` files exist and load.**
   - command: `ls usecase-corpus/*.toml | wc -l`
   - expected: `100`
   - proof: closes [implementation-plan.md § 5.3 DoD bullet 2](../implementation-plan.md).
3. ✓ shipped — **Every `UC-*` and `AC-*` row from [usecase.md § 2](../usecase.md) + § 4 has exactly one `.toml` file.**
   - command: `for id in $(grep -oE '(UC|AC)-[A-Z]+-[0-9]+' docs/plans/may-24-lexical-indexing-sorucegraph/usecase.md | sort -u); do test -f "usecase-corpus/$id.toml" || echo "MISSING: $id"; done`
   - expected: empty stdout (no MISSING lines)
   - proof: closes 1:1 row mapping.
4. ✓ shipped — **Agent output validates against the schema.**
   - command: `cargo test -p quanta-index-conformance --test lq_conformance && python3 tools/ci/agent/validate_agent_output.py target/conformance/agent_output.json`
   - expected: validator exit 0
   - proof: closes [implementation-plan.md § 5.3 DoD bullet 3](../implementation-plan.md).
5. ✓ shipped — **CI rail `ci/lq-conformance` enforced.**
   - command: `grep -E '^\s+- name: ci/lq-conformance' .github/workflows/correctness.yml`
   - expected: rail entry exists
   - proof: closes [implementation-plan.md § 4.1 exit gate](../implementation-plan.md).
6. ✓ shipped — **Drift detection reports but does not auto-accept.**
   - command: `cargo test -p quanta-index-conformance --test drift_detector test_sgEq_drift_detected`
   - expected: pass — drift report emitted, runner exits non-zero
   - proof: closes [implementation-plan.md § 5.3 DoD bullet 5](../implementation-plan.md) and [rfc.md § Conformance corpus ownership](../rfc.md).
7. ✓ shipped — **Wave-0 exit verdict distribution.**
   - command: `cargo test -p quanta-index-conformance --test lq_conformance -- --nocapture | grep -E '^(ok|error_expected|blocked):' | sort`
   - expected: `error_expected: 15`, `blocked: 85`, `ok: 0`, `error_unexpected: 0`, `result_mismatch: 0`, `internal_error: 0` (per §6.4 table; concrete count: 15 parser-reachable AC + 13 parser-reachable UC-EDGE rows = 28 `error_expected` if all parser-reachable UC-EDGE rows are tagged `gate=active` from Wave-0; otherwise revise to match the corpus authoring). Final distribution pinned by the corpus author and asserted by `corpus_full` test.
   - proof: closes [implementation-plan.md § 4.1 exit gate](../implementation-plan.md) "reports 100 rows as `blocked` (status accurate, not `ok`)" — verbatim. Note: the exit gate text says "100 rows blocked" but rows that pass `parse → normalize → hash → expected error` are *accurately* `error_expected`, not `blocked`. This ticket's authoring records the more precise distribution per §6.4 and files this as a sibling-doc follow-up (§12).
8. ✓ shipped — **No silent skip.**
   - command: `cargo test -p quanta-index-conformance --test runner_self_tests test_blocked_row_still_hashes_canonically && cargo test -p quanta-index-conformance --test property_no_silent_skip`
   - expected: pass
   - proof: closes [CLAUDE.md](../../../../CLAUDE.md) "no silent failure / no silent fallback".
9. ✓ shipped — **JUnit XML well-formed.**
   - command: `cargo test -p quanta-index-conformance --test junit_xml_shape`
   - expected: pass
   - proof: closes ops integration (CI artifact upload).
10. ✓ shipped — **Bench within target.**
    - command: `cargo bench -p quanta-index-conformance --bench pre_conf_runner_bench`
    - expected: full corpus run p99 < 5s wall
    - proof: closes §9 envelope.
11. ✓ shipped — **`tools/ci/lint/lint-doc-paths.py` green on the 3 new ticket files.**
    - command: `python3 tools/ci/lint/lint-doc-paths.py 2>&1 | grep -E '(PRE-CONTRACT-EXT|PRE-NORM|PRE-CONF)\.md' ; echo "exit=$?"`
    - expected: linter exits 0; no broken doc paths reported from any of the 3 ticket files
    - proof: closes ticket task post-condition.
12. ✓ shipped — **ADR-006 committed.**
    - command: `test -f docs/adr/ADR-006-conformance-corpus-format.md`
    - expected: file exists
    - proof: closes [implementation-plan.md § 10 ADR-006](../implementation-plan.md).
13. ✓ shipped — **`just rust-conformance` recipe works.**
    - command: `just rust-conformance`
    - expected: exit 0
    - proof: closes wave-exit rail invocation per [CLAUDE.md](../../../../CLAUDE.md) Operational Reference.

## 12. Open questions

| Q-ID | Question | Forcing function |
|---|---|---|
| ADR-006 ([implementation-plan.md § 10](../implementation-plan.md)) | Corpus format: TOML vs YAML | Step 1 of §5 cannot start without the format. **Default position recorded**: TOML (rationale in §4.1). Forcing function: DoD §11.2 file count requires the format pinned to one extension. |
| ADR-006 follow-on (new crate vs in-tree test target) | Runner location: new `quanta-index-conformance` crate vs `cargo test -p quanta-index-contract --test lq_conformance` (per [implementation-plan.md § 4.1](../implementation-plan.md)) | **Default position**: new `quanta-index-conformance` crate (clean dependency surface; test-only deps stay isolated from contract crate). [implementation-plan.md § 4.1](../implementation-plan.md)'s text names the contract crate; reconcile in a follow-up. Forcing function: DoD §11.1 names the crate explicitly. |
| ADR-006 follow-on (corpus location) | Corpus path: `usecase-corpus/` (next to `usecase.md`) vs `tools/ci/conformance/lq/` (per [usecase.md § 6](../usecase.md)) | **Default position**: `usecase-corpus/` — keeps the corpus next to its authoring doc per [rfc.md § Conformance Suite Reference](../rfc.md). [usecase.md § 6](../usecase.md) text marks the `tools/ci/conformance/lq/` path as "proposed, not yet created"; this ticket pins it differently. Reconcile in a follow-up against [usecase.md § 6](../usecase.md). |
| (new) | Wave-0 exit verdict distribution: are parser-reachable UC-EDGE rows tagged `gate=active` (counted as `error_expected`) or `gate=pending` (counted as `blocked`)? | DoD §11.7 distribution count depends on this. **Default position**: parser-reachable UC-EDGE rows (UC-EDGE-01, 02, 03, 04, 05, 06, 10) are `gate=active` because PRE-NORM emits the typed code at parse time. UC-EDGE-07, 08, 09 are `gate=pending` (need executor / authz). |
| Q-FS-5 ([feature-scope.md § 1.4.1](../feature-scope.md)) | `dirty:` semantics — producer dependency vs deferred | UC-RT-08 row's `gating_ticket` value. **Default**: `gating_ticket="RT-01"` with a `notes` field flagging the cross-repo dependency. |
| Q-FS-7 ([feature-scope.md § 1.1.3](../feature-scope.md)) | `count:all` ceiling: fail-closed vs silent truncate | UC-LEX-19 expected invariant. **Default**: fail-closed per repo posture; row asserts `early_stop_reason = None` for `count:all`. |
| UC-GAP-1 ([implementation-plan.md Appendix A.3](../implementation-plan.md)) | No hybrid-query rows in corpus | Wave-6 SEM-01 will need rows; **defer to SEM-01 authoring**. PRE-CONF lands the runner; corpus extension is a wave-6 deliverable. |
| UC-GAP-2 ([implementation-plan.md Appendix A.3](../implementation-plan.md)) | No incremental-write rows in corpus | Wave-3 LEX-04 will need rows. **Defer.** |
| UC-GAP-3 ([implementation-plan.md Appendix A.3](../implementation-plan.md)) | No `STATE_NOT_READY: CATALOG_MISS` end-to-end row | Wave-3 LEX-05 / LEX-04 will need rows. **Defer.** |
| G-CONTROL-LOC ([implementation-plan.md § 2.3a](../implementation-plan.md)) | Where does control-plane state live? | PRE-CONF is **physically-path-agnostic** for the control plane; no file under `quanta-index-control/` is touched. The runner's `StubLqEngine` does not need a real control plane. Does not block this ticket. |

## 13. References

- Parent RFC: [rfc.md](../rfc.md) — § Conformance Suite Reference, § Error Code Taxonomy, § Claim Discipline, § Conformance corpus ownership
- Scope catalog: [feature-scope.md](../feature-scope.md) — § 1.4.1 Q5 (`dirty:`), § 9 open questions
- Conformance corpus: [usecase.md](../usecase.md) — § 0 conventions, § 2 UC-* rows, § 3 contract gaps, § 4 AC-* rows, § 5 coverage matrix, § 6 conformance reference plan (golden file format, versioning, authoring discipline)
- Grammar / DSL: [dsl.md](../dsl.md) — § 12 error taxonomy (one source of truth for the error codes the runner expects)
- Execution plan: [implementation-plan.md](../implementation-plan.md) — § 4.1 Wave 0, § 5.3 PRE-CONF DoD, § 6 risk register R6/R10/R15, § 8.4 mock policy, § 9.1 OBS subset per wave, § 10 ADR-006, Appendix A.3 UC-* gaps
- Agent rules: [CLAUDE.md](../../../../CLAUDE.md) — § Agent change posture, § Rule Catalog (Safety + Verification)
- Sibling tickets: [PRE-CONTRACT-EXT.md](PRE-CONTRACT-EXT.md), [PRE-NORM.md](PRE-NORM.md)
- Producer handoff SSOT: [docs/ssot/producer-handoff.md](../../../ssot/producer-handoff.md)
- Ticket index (downstream-migration follow-up tracked under §3.6): [INDEX.md](INDEX.md)
- Agent output schema: [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json)
- Doc-path linter: [tools/ci/lint/lint-doc-paths.py](../../../../tools/ci/lint/lint-doc-paths.py)
- Semgrep rules: [tools/ci/semgrep/rules.yml:124](../../../../tools/ci/semgrep/rules.yml#L124)
