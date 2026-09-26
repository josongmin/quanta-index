# SEP-26 benchmark migration — closeout verdicts

**Historical receipt — superseded by [CURRENT-AUDIT.md](CURRENT-AUDIT.md).**
The implementation/integration claims below overstate non-native profile
support. Before the current repair, systems promotion was latency-only and
concurrency fan-out was refused. Current repairs cover native systems/load/
freshness capture and replay; Criterion/retrieval/recorded adapters are still
implementation work. Do not use this document as an all-tickets-complete claim.

Source state for every receipt in this file:

- `HEAD` moved **four times** during this session under a concurrent worker:
  `33924335` → `51d3d342` → `420a6452` → `79bb8d23312d48d5ef8c0dba972e337c3e72e041`. The receipts below were
  re-taken at `79bb8d23312d48d5ef8c0dba972e337c3e72e041`; the shared checkout is still **dirty** (18 files, all
  belonging to that worker's in-flight retrieval/RBR and verification-receipt
  work — none of this packet's files).
- That worker also swept this packet into its commits with a repo-wide `git add`,
  so the packet's files are now tracked, but under unrelated commit messages.
  **No clean-source claim is made here.** All receipts below are dirty-checkout
  implementation/contract evidence; every qualified benchmark claim is `BLOCKED`
  or `NOT_RUN`.
- Registry canonical digest:
  `sha256:f2066046b23aad113491157ce777f5fa25ef669a9326b939f3bfbfc7eae9291f`.

Axes are judged separately, as the packet requires: **implementation**,
**focused verification**, **integration**, **qualification**.

## Receipts actually executed

| # | Command | Result |
| --- | --- | --- |
| R1 | `./scripts/cargow --lane bench-lane test -p quanta-index-bench-protocol` | 46 passed (37 adversarial, 2 conformance, 7 round-trip) |
| R2 | `./scripts/cargow --lane bench-lane clippy -p quanta-index-bench-protocol --all-targets --all-features -- -D warnings` | clean |
| R12 | `python3 tools/ci/lint/check-rust-derive-allowlist.py` | `All #[derive(...)] sites are on the allowlist.` (after the manual-codec rewrite) |
| R3 | `./scripts/cargow --lane bench-lane fmt -p quanta-index-bench-protocol -- --check` | clean |
| R4 | `uv run --frozen --extra dev python -m pytest <14-file benchmark control-plane suite>` | **255 passed, 0 failed** (the 15th file, `test_write_verification_receipt.py`, is the concurrent worker's and is currently red — see below) |
| R5 | `python3 tools/ci/lint/check-benchmark-policy.py --print-registry-digest` | `policy ok` + digest `sha256:f2066046…9291f` |
| R6 | `python3 tools/benchmark/benchctl.py list` / `plan dsl-authority` (×2) | 10 profiles + digest; byte-identical `plan`, `mutates: false` |
| R7 | `python3 tools/benchmark/benchctl.py preflight dsl-authority --receipt /tmp/bm-preflight.json` | exit 1, `TIMING_PREFLIGHT_BLOCKED reason=unsupported_host expected_os=linux actual_os=darwin` |
| R8 | `python3 tools/benchmark/benchctl.py validate dsl-authority` | refused: `worktree is dirty: current-source benchmark evidence requires a clean checkout` |
| R9 | `uv run --frozen --extra dev ruff check <new/changed surfaces>` | `All checks passed!` |
| R10 | `python3 tools/benchmark/benchctl.py replay <promoted run>` (fresh process, real `BenchArtifactV1` fixture) | `artifact_oracle: pass`, `replay: re_derived`; tampered raw → exit 2 `digest mismatch` |
| R11 | `jsonschema.Draft202012Validator` over `tools/benchmark/evidence.schema.json` vs the committed golden | accepts the sealed sample; rejects unknown protocol version, an empty latency payload and an unregistered unit |

R4 file list: `test_benchmark_manifest`, `test_benchmark_policy`,
`test_bench_protocol_conformance`, `test_benchmark_evidence_bridge`,
`test_benchmark_source_closure`, `test_benchctl`, `test_check_bench_artifacts`,
`test_check_host_contention`, `test_compare_dsl_bench`,
`test_quality_integration_summary`, `test_retrieval_contract_proof`,
`test_retrieval_sdk_proof`, `test_write_verification_receipt`,
`test_agent_outcome_benchmark`, `test_validate_agent_output`.

## Pre-existing failure outside this packet (recorded, not hidden)

`tools/ci/tests/test_retrieval_benchmark.py` (the pre-existing retrieval
benchmark file, separately executed) reports **27 failed / 278 passed**. This is
**not** caused by this packet:

- the failing assertions are in retrieval verdict logic (`PAIR_VALID` states);
- the tests import nothing from this packet (`benchctl`, `manifest`, `registry`,
  `evidence`, `source_closure`);
- `tools/benchmark/retrieval/{evaluator.py,run.py}` and
  `tools/ci/tests/test_retrieval_benchmark.py` already carried large uncommitted
  edits (243/222/379 changed lines) by a concurrent worker **before** this
  session's first write, and were never touched here.

It is left as the concurrent owner's work in progress. No cutover in this packet
depends on it.

## Per-ticket verdicts

| Ticket | implementation | focused verification | integration | qualification |
| --- | --- | --- | --- | --- |
| BM-00 | VERIFIED | VERIFIED | VERIFIED | NOT_APPLICABLE |
| BM-01 | VERIFIED | VERIFIED | VERIFIED | NOT_APPLICABLE |
| BM-02 | VERIFIED | VERIFIED | VERIFIED | NOT_APPLICABLE |
| BM-03 | VERIFIED | VERIFIED | VERIFIED | NOT_APPLICABLE |
| BM-04 | VERIFIED | VERIFIED | NOT_RUN | BLOCKED |
| BM-05 | VERIFIED | VERIFIED | VERIFIED (rails) / NOT_RUN (pilot) | BLOCKED |
| BM-06 | VERIFIED | VERIFIED | VERIFIED (fixtures) / NOT_RUN (real recording) | NOT_RUN |
| BM-07 | VERIFIED | VERIFIED | VERIFIED (local CI-equivalent) / NOT_RUN (hosted CI) | BLOCKED |

### BM-00 — inventory and authority freeze

- **implementation VERIFIED.** `tools/benchmark/registry.toml` (+ `registry.py`)
  is the single data-only registry: 23 families, 10 profiles, 20 producers,
  7 validators, 6 scorers, 3 closures, 3 external-input declarations.
  `tools/benchmark/manifest.json` was removed and `manifest.py` became a
  read-only projection, so the artifact checker, the quality aggregate and the
  CLI resolve one table. `BM-00-INVENTORY.md` records the family table, Cargo
  bench targets, correctness-only owners, CI inventory, baseline state and
  closure scope. New `source_closure.py` profile `benchmark-control-plane`
  binds the normative subset and excludes `docs/plans/**`.
- **focused verification VERIFIED.** R5, R9 plus
  `test_benchmark_manifest.py`, `test_benchmark_policy.py`,
  `test_benchmark_source_closure.py` (28 tests): projection equality,
  reachability of every named producer, refusal of a family without a required
  key, refusal of an unreachable producer, refusal of ambiguous profile
  membership, closure digest/revision/file-set mutation invalidations, and the
  planning-history exclusion.
- **integration VERIFIED.** `check-bench-artifacts.py`,
  `quality_integration_summary.py` and `benchctl` all load `registry.toml`; the
  pre-existing artifact/quality/comparator suites (R4) pass unchanged in
  behaviour; `ci.yml: rust-policy` now runs the policy guard.
- **qualification NOT_APPLICABLE** — BM-00 owns no benchmark claim.
- **Known consequence:** adding the profile changes the digest of the `retrieval`
  closure (which binds `source_closure.py` itself), so previously issued
  retrieval closure receipts are invalidated and must be re-issued from a clean
  exact-source snapshot. Fail-closed by design.

### BM-01 — Cargo placement and dependency direction

- **implementation VERIFIED.** No file was moved: crate-local Criterion benches
  stay in `crates/quanta-index-lq-norm/benches/` and
  `crates/quanta-index-searchd-runtime/benches/`;
  `crates/quanta-index-searchd-harness/` stays put (it is still a runtime
  dev-dependency). One new benchmark-only package,
  `benchmarks/bench-protocol`, joins the **root** workspace and shares
  `Cargo.lock`/toolchain. A dependency-direction guard was added to
  `check-benchmark-policy.py`.
- **focused verification VERIFIED.** `test_benchmark_policy.py`: on real
  `cargo metadata`, `dependency_inversions == []`; a synthetic normal
  `crates/* → benchmarks/*` edge is refused; a dev-dependency edge is allowed;
  every declared bench target is registered and no phantom one is accepted.
- **integration VERIFIED.** `cargo metadata --no-deps` resolves
  `quanta-index-bench-protocol` exactly once with the shared lockfile (R1/R5);
  the crate compiles and its tests run in the shared `bench-lane`.
- **Exclusion:** the workspace-wide `cargow bench --workspace --no-run` compile
  lane was not run here; it is a heavy, contended, compile-only rail
  (`ci.yml: rust-bench-build`) and produces no measurement.
- **qualification NOT_APPLICABLE.**

### BM-02 — typed evidence and immutable runs

- **implementation VERIFIED.** `benchmarks/bench-protocol` defines
  `BenchmarkEvidenceV1` with seven typed payloads, canonical JSON, digest rules,
  path/run-id safety, and a run store with staging → raw verification → atomic
  promotion → advisory `latest` → baseline admission → retention. Payload rules
  make metric confusion impossible: `micro` unit must agree with
  `instrumentation` (instruction counts can never be `ms`), retrieval
  `span` space can never be `mechanically_labeled`, unjudged/timeout/unsupported
  rows may not carry a score, open-loop points require `offered_rate` while
  closed-loop points may not claim one, unmeasured latency rows carry a reason
  and no percentile, agent arms must be exactly `[A,B,C]`, and a recorded
  experiment must be `diagnostic_only`.
- **focused verification VERIFIED.** R1–R3, R4
  (`test_bench_protocol_conformance.py` 27 tests, `test_benchmark_evidence_bridge.py`
  10 tests): round-trip, sealing/digest sensitivity, **cross-language byte and
  digest equality** against the committed vectors, and refusal of duplicate JSON
  keys, unknown fields, unknown protocol/version, malformed/tampered digests,
  dirty-source contradiction, `canonical-linux` off Linux, performance scope
  without an exclusive monitored lease, timeout/partial producer with a pass
  verdict, non-pass verdict without a reason, payload confusion, raw path escape,
  duplicate raw path, empty raw set, missing/extra/tampered/reordered raw bytes,
  symlinked raw reference, repeated run id, crash-before-promotion, and
  non-comparable baselines.
- **integration VERIFIED.** R10 promotes a real `BenchArtifactV1` fixture into
  an immutable run and replays it in a fresh process; the run store is also
  exercised end-to-end for `latest`, baseline admission and retention.
- **qualification NOT_APPLICABLE** — this ticket defines structure, not a
  measurement.

### BM-03 — one registry, one CLI

- **implementation VERIFIED.** `benchctl` now implements
  `list`, `plan`, `run`, `validate`, `compare`, `replay`, `summarize`,
  `preflight`, driven by `registry.toml`. `plan` is deterministic and
  digest-bound; `run --evidence-root` promotes immutable runs; `replay`
  re-validates from raw in a fresh process and re-derives the verdict through
  the independent artifact checker. Go/no-go is recorded in `BM-03-DECISION.md`:
  **no-go on the Rust CLI; Python is the single current orchestrator.**
- **focused verification VERIFIED.** R6, `test_benchctl.py` (36 tests) and
  `test_benchmark_manifest.py`: byte-identical plans, `mutates: false`, resolved
  `just`/`cargo`/Python commands, recorded families marked non-runnable, replay
  refused on a tampered run and on a missing evidence root, evidence root inside
  the checkout refused, registry mutation refusals.
- **integration VERIFIED.** R4, R8, R10; the hosted CI job now calls this CLI.
- **qualification NOT_APPLICABLE** — CLI selection is a tooling decision.

### BM-04 — system, load, freshness and Rust micro rails

- **implementation VERIFIED.** All 23 families including the DSL/quality/system
  rails and the two crate-local Criterion targets are registered with purpose,
  payload, result unit, host policy, gate tier, sample floor, baseline
  compatibility and closure. `evidence_bridge.latency_payload_from_artifact`
  maps native `BenchArtifactV1` rows verbatim, and `micro_payload_from_criterion`
  keeps wall and instruction instrumentation separate.
- **focused verification VERIFIED.** R4 (`test_benchmark_evidence_bridge.py`):
  row/error/timeout preservation, unmeasured rows, refusal of a rowless or
  schema-1 artifact (no fabricated payload), instruction-vs-wall separation.
- **integration NOT_RUN.** The DSL/system rails need a clean worktree and (for
  the authority families) the quiet canonical Linux host. R8 shows the
  clean-worktree requirement is enforced before any producer runs, and R7 shows
  the canonical-Linux host policy is enforced before any producer runs. No
  diagnostic DSL/system capture was run on this contended macOS checkout
  because `benchctl run` refuses a dirty source first — producing one would have
  required a clean snapshot this session could not create on a shared, actively
  committed `main`.
- **qualification BLOCKED.** No canonical Linux host, no admitted DSL warm/cold
  baselines (`tools/benchmark/baselines/` is absent), no quiet-host lease, no
  capture-time interference monitoring. `dsl-authority` correctly fails typed
  instead of reporting a number. Open-loop capacity thresholds remain unset.

### BM-05 — retrieval bridge

- **implementation VERIFIED.** `retrieval-sdk`, `retrieval-contract`,
  `retrieval-pair` and `lexical-file-comparison` are registered against the
  **existing** `run.py`/`evaluator.py`/`lexical_file_comparison.py` owners; no
  third scorer was forked. The typed `RetrievalPayload` forces an explicit
  `lane` (`native_default` / `controlled_mechanism`), an explicit `metric_space`
  (`file`/`line`/`span`/`context`), an explicit judgment provenance, per-query
  state, pool `unjudged` separately from irrelevant, and an
  `universe_attested` flag. RBR-12's custody/gold ownership is untouched.
- **focused verification VERIFIED.** R4 including `test_retrieval_contract_proof.py`
  and `test_retrieval_sdk_proof.py`; plus the retrieval payload refusal tests in
  both languages (span ≠ mechanical labels, unjudged ≠ scored).
- **integration VERIFIED for the existing rails.** The `retrieval` source-closure
  profile and both proof rails still pass their own tests unchanged in
  behaviour. **NOT_RUN for the two-repo pilot**: it needs the external corpus
  release.
- **qualification BLOCKED.** No admitted external corpus release, holdout/gold
  or product index-universe attestation. File metrics remain file-space; no span
  credit is manufactured from file-only captures; no cross-product winner is
  inferred.

### BM-06 — recorded agent outcomes and experiments

- **implementation VERIFIED.** `agent-outcome` is registered with producer
  `none` (recorded-only, so `benchctl run` cannot manufacture a capture), and
  `scan-vs-index` is registered as a `recorded_experiment` payload with
  `diagnostic_only` enforced. The `AgentOutcomePayload` requires exactly arms
  `[A,B,C]`, a non-zero pair count, the input JSONL digest, and an explicit
  `capture` authenticity label (`recorded_unauthenticated` / `authenticated`).
- **focused verification VERIFIED.** R4 including
  `test_agent_outcome_benchmark.py` and `test_validate_agent_output.py`
  (unchanged evaluator behaviour), plus the arm/authenticity/diagnostic-only
  refusal tests and `test_plan_marks_recorded_families_as_not_runnable`.
- **integration VERIFIED for the fixture contract** (the existing evaluator
  suites still pass); **NOT_RUN for a real recorded input** — none is present in
  or near this checkout.
- **qualification NOT_RUN.** No recorded A/B/C trajectories, frozen task
  digests, or underlying test receipts. A fixture-only replay is contract proof,
  not agent-outcome proof; condition evidence coverage stays distinct from
  time-to-first-useful-evidence.

### BM-07 — CI, baselines and authority cutover

- **implementation VERIFIED.** `correctness.yml: dsl-bench-latency` now drives
  `benchctl run dsl-authority`, `benchctl run systems`, `benchctl validate` and
  `benchctl replay` and uploads the immutable runs; every direct producer and
  comparator step is gone. `ci.yml: rust-policy` runs the policy guard.
  `Justfile` gained `benchmark-policy-local` / `benchmark-plan` /
  `benchmark-replay` and folds the protocol crate and new suites into
  `benchmark-prep-local`. `BM-07-MIGRATION-MATRIX.md` states one authority per
  family, the legacy-reader status and the external inputs still required.
- **focused verification VERIFIED.** R5 proves zero direct bypasses and zero
  dependency inversions at the current source; `test_benchmark_policy.py`
  injects a bypass and an unregistered bench target and both are refused.
- **integration VERIFIED for the local CI-equivalent path** (R5, R6, R7, R8,
  R10). **NOT_RUN for hosted CI**: GitHub Actions was not executed here, so the
  cutover job's first real run is still owed. Promotion through
  `--evidence-root` is new in CI and its first green run will also confirm that
  the freshness/open-loop native artifacts carry promotable rows.
- **qualification BLOCKED.** External retrieval/agent quality and DSL
  performance still require separately admitted inputs and the canonical host.

## Adversarial self-audit of this packet

Re-read of the final source found and fixed two real defects, both with a
regression test:

1. `benchctl run --evidence-root` and `benchctl validate --evidence-root`
   iterated **every** registered artifact family instead of the selected
   profile's families, so `run systems` would have tried to promote the DSL
   families too. Fixed to iterate `profiles[profile]["families"]`
   (`test_promotion_is_scoped_to_the_profile_families`,
   `test_promoted_run_validation_is_scoped_to_the_profile`).
2. `manifest.py` imported `registry` without guaranteeing its own directory on
   `sys.path`, so a direct import from another entrypoint could fail. Fixed
   with an explicit `sys.path` guard (matching `benchctl.py`/`evidence_bridge.py`).
3. **Policy violation, found by running the repo's own CI linters rather than
   only the tests.** `benchmarks/bench-protocol` used
   `#[derive(Serialize, Deserialize)]`, which
   `tools/ci/lint/check-rust-derive-allowlist.py` bans outright across
   `crates/**` and `benchmarks/**` (58 offences). The crate was rewritten with a
   manual wire codec (`src/codec.rs`): a `Wire` trait with explicit
   `encode`/`decode` per struct generated by an `impl_wire!` `macro_rules!`,
   plus a hand-written `Deserialize` for a `StrictValue` wrapper that rejects
   duplicate object keys at any depth. The `serde` `derive` feature is removed.
   **The committed canonical golden and both cross-language fixtures are
   unchanged**, and all 46 Rust tests still pass — so the rewrite is a pure
   re-implementation of the same wire form, not a format change.
4. `tools/ci/test-authority.toml` gained catalog entries for the crate's three
   integration targets (`bench-protocol-{roundtrip,adversarial,conformance-vectors}`),
   which `check-test-authority.py` requires.

Also verified in the same pass: the policy guard flags all six legacy direct CI
steps before the cutover and none after; the registry project check proves no
phantom bench target and no dependency inversion against the **real** Cargo
graph; the `retrieval` closure receipt invalidation is documented rather than
papered over; and `evidence.schema.json` is checked against the same golden the
Rust and Python writers must reproduce.

## Pre-existing CI-gate failures on `main` (not this packet)

Running the repository's own lint set surfaced three failures that exist at
`HEAD = 51d3d342` independently of this packet. Each was verified as
unmodified-in-tree, so it is committed state, not a working-tree artifact:

| Gate | Offence | Verification |
| --- | --- | --- |
| `check-test-authority.py` | `crates/quanta-index-semantic/tests/exact_ann_decomposition.rs`: orphan integration target | file exists at `HEAD` (introduced in `36b2f8c5`); `HEAD:tools/ci/test-authority.toml` has no entry; file is unmodified in the tree. This packet's own three targets *are* now registered. |
| `check-module-cycles.py` | `quanta-index-catalog`: cycle `candidate → sequence → idempotency → sequence` | `crates/quanta-index-catalog/src/*.rs` unmodified in the tree; introduced in `f16bad93`. |
| `check-digest-fallibility.py` | `crates/quanta-index-catalog/src/sequence.rs:234,253` return `[u8; 32]` without `Result` or an "infallible by construction" doc | same files, unmodified in the tree. |
| `test_write_verification_receipt.py` (pytest) | 8 failures in receipt/nextest-event handling | `tools/ci/write-verification-receipt.py`, its test and `verification-receipt.schema.json` were modified in the tree at 18:28-18:30 local, i.e. by the concurrent worker, after this packet's earlier green run of the same file. |

They are reported rather than silently fixed: they belong to other owners and
touching them would collide with in-flight work this packet was told to
preserve.

## Remaining gaps (honest list)

1. **No clean exact-source snapshot.** `main` is dirty and was actively
   committed to by another worker during this session. Every receipt above is
   dirty-state evidence. Re-issue from a frozen snapshot before any qualified
   claim.
2. **No admitted DSL baselines and no canonical Linux host** → DSL
   performance qualification `BLOCKED`; `dsl-authority` fails typed by design.
3. **No external corpus/gold** → retrieval quality qualification `BLOCKED`.
4. **No recorded agent trajectories** → agent-outcome qualification `NOT_RUN`.
5. **Hosted CI not executed** → the cutover job's first run is owed; a promotion
   failure inside it (a family whose native artifact has no promotable rows)
   would be visible and attributable rather than silent.
6. **Retrieval benchmark suite** has 27 pre-existing failures owned by the
   concurrent retrieval/RBR worker (see above). Not caused by, and not fixed by,
   this packet.
7. **`cargow bench --workspace --no-run`** and any timing-bearing rail were not
   run on this contended shared checkout.
