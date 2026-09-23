# Single-IR breaking cutover plan

Status: planned, not implemented. Initial source snapshot: `1306325e81189a8c0df5d38c56567664a6f20c1d` on 2026-09-24. HEAD and unrelated dirty files changed during planning; this is not a source-closed audit. Re-inventory revision and ownership before assigning writers.

## Decision and boundary

- The service has not been deployed. Internal IR has no backward-compatibility obligation. At each semantic boundary there is one current typed IR; change its producer, consumers, tests, and docs in one cutover. Do not create `IrV1`/`IrV2`, compatibility adapters, upgrade-on-read, or dual execution paths.
- This does **not** mean one universal type for lexical planning, structural matching, benchmark scoring, and other unrelated semantics. AST, canonical IR, executable plan, and wire DTOs may remain distinct when they have distinct authority; each boundary must have one owner and one current shape.
- Persisted state, IPC, SDK, and benchmark JSON are not internal IR. Retain an exact schema/format identity where it prevents misinterpretation, but accept only the current shape. Unknown or old bytes fail with a typed rebuild/re-capture instruction; no fallback parser or silent migration. Do not delete user/source data merely because derived artifacts are obsolete.
- No feature is accepted by extending a legacy IR branch. Edit the canonical type and all reachable constructors/pattern matches, remove superseded branches and stale tests, then requalify. A temporary compile failure on an isolated branch is preferable to two live authorities.

## Source-backed inventory and limits

| Boundary | Current observation | Cutover action |
| --- | --- | --- |
| Retrieval suite/query pack/runner/scoring | `tools/benchmark/retrieval/evaluator.py` still dispatches v1/v2/v3 suites and records. `run.py`, `semble.py`, Rust runner, and JSON schemas already require/emit v3 for paired work. | First concrete cleanup: single current evaluator model and one scorer; remove v1/v2 read/score paths and version-dependent report logic. Keep an exact current artifact stamp only as a rejection check. |
| Lexical query planning | `quanta-index-contract::LqQuery` lowers through `quanta-index-lexical::LexicalPlanner` to `LexicalPlan`/`PlanNode`; no parallel `LexicalPlanVn` was found in the inspected source. | Keep this IR. For future shapes, change planner, executor, explain, and tests together; do not rename or fork it just to appear versionless. |
| Structural pattern matching | `LqStructuralBlock` lowers to `StructuralPattern`/`PatternNode`; inspected source shows one active pattern IR. | Keep this IR; extend its typed nodes and matcher together. |
| Other `V1`/`V2` types and on-disk formats | Names such as `RequiredDomainsV1`, `QueryRouteV1`, `ReadResourceGroupV2`, and RepoMap layout/state types exist. A suffix alone does not prove two active IRs; some are public contracts or persisted formats. | Inventory producer, consumer, storage/wire boundary, and live alternatives before changing. Do not mass-rename or remove migration/state code without an exact replacement/rebuild decision. |

This is a targeted source scan, not an exhaustive proof that no other dual IR exists. The first wave below closes that gap before any broad deletion.

## Execution waves

### W0 — freeze and classify (read-only)

1. Record HEAD, dirty paths, file owners, public consumers, active artifact roots, and exact benchmark/proof inputs. Do not edit files owned by another in-flight writer.
2. Inventory each IR-like type and each `schema_version`/format dispatch in `crates/`, `benchmarks/`, `tools/`, and live docs. Classify it as internal IR, AST/plan projection, wire DTO, durable state, cache, benchmark artifact, or historical document. For each live alternative, record producer -> consumer -> rejection/rebuild path.
3. Freeze the intended current semantics independently of implementation: one valid suite, blind pack, runner record, and expected hand-calculated scores; negative cases for old formats, wrong hashes, duplicate keys, leakage, malformed provenance, and source mismatch. Keep corpus and reviewed labels; invalidate only derived captures/receipts when their bound code or schema changes.
4. Stop if an actual external producer/consumer or irreplaceable local state is discovered. Decide its coordinated cutover/rebuild explicitly; do not quietly revive compatibility.

### W1 — one retrieval benchmark artifact model and scorer

Owner: `tools/benchmark/retrieval/evaluator.py` and its focused tests. Separate from concurrent `run.py`/`semble.py`/Rust writers until their edits are integrated.

1. Make current suite/pack/record validators non-version-dispatched. Delete `_validate_suite_v1`, `_validate_suite_v2`, `_load_run_v1`, `_load_run_v2`, and the v1/v2 scoring/report branches after tracing all calls. Rename `_v3` helpers to role names only when their behavior is now canonical. Keep exact current JSON schema checks and fail closed on old/unknown stamps.
2. Preserve v3 semantics: comparison-contract binding, blinded pack, source/file-universe hashes, byte coverage, typed failures, provenance, nullable timing, deterministic scoring, and independent expected-value oracles. Do not replace these with a generic permissive parser.
3. Replace legacy v1/v2 positive fixtures/tests with one canonical fixture plus targeted negative old-format rejection tests. Keep distinct security/quality mutants; remove only redundant migration-roundtrip assertions. Split the large retrieval test module by behavior when ownership is stable, not by schema generation.

### W2 — atomic producer/consumer cutover

Owner seams: `tools/benchmark/retrieval/{run.py,semble.py,*schema.json}`, `benchmarks/retrieval/src/{record.rs,lib.rs,main.rs}`, retrieval CLI/Just recipes, and proof/receipt validators. Integrate current dirty edits first; do not overwrite them.

1. Make Rust runner, Semble adapter, driver, evaluator, JSON schemas, CLI examples, and receipt/verdict checks emit/accept the same current fields. Reject stale captures before merge or score. Keep the artifact schema identity only for strict equality/rejection, not migration.
2. Decide whether existing v3 artifact bytes stay the canonical wire shape. Prefer keeping `schema_version: 3` if fields are unchanged; if fields change incompatibly, give the new artifact an unambiguous identity and reject prior artifacts. Never label altered bytes as the same contract.
3. Regenerate derived example artifacts and source-closure/receipt inputs from the cutover source. Old receipts cannot qualify the new source; do not rewrite them to appear current. Keep raw historical evidence outside the current authority path if retention is needed.

### W3 — repository-wide IR rule and stale-surface cleanup

1. Inspect `quanta-index-contract/src/query/expression.rs`, `quanta-index-lq-norm/src/ast.rs`, `quanta-index-lexical/src/{plan,planner}.rs`, `quanta-index-lq-structural/src/pattern.rs`, and `quanta-index-search-plane/src/lowering.rs` as the first AST/IR/consumer chain. Apply the no-dual-IR decision only to *confirmed* active alternatives found in W0. For each, update constructors, exhaustive matches, execution, explain/serialization boundaries, and tests in one owner lane. Already-single `LexicalPlan` and `StructuralPattern` need no churn.
2. Update active retrieval docs (`tools/benchmark/retrieval/README.md`, `docs/plans/sep-23-retrieval-bench/tickets/{INDEX.md,RB-01-suite-and-scoring.md,TEST-PLAN.md}`) to describe only the current contract. Preserve historical plans as history only when clearly marked non-authoritative; remove live instructions to maintain v1/v2 readers.
3. Review test-authority, CI selectors, generated API/schema snapshots, fixtures, and command recipes for stale legacy expectations. Do not weaken negative evidence tests or drop a required rail merely to lower the count. Deduplicate local prep/proof execution structurally while preserving source-bound raw machine evidence for qualification.

### W4 — qualification and closeout

1. Run prompt-manager sync/lint after policy-source edits. Run focused evaluator/driver/Rust contract checks first, then relevant build and benchmark-prep rails. Minimum local sequence: `python3 tools/prompt-manager/pm.py lint`; `python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py tools/ci/tests/test_retrieval_contract_proof.py tools/ci/tests/test_retrieval_sdk_proof.py -q`; `just benchmark-prep-local`. Use the canonical `Justfile`/`./scripts/cargow` front door.
2. Require negative oracles proving v1/v2 retrieval artifacts and unknown stamps fail, and positive hand-calculated v3/current scores plus Rust/SDK roundtrip. Collect raw JUnit/nextest evidence and source-bound receipts at the final HEAD with `just retrieval-contract-proof <fresh-absolute-output>` and `just retrieval-sdk-proof <fresh-absolute-output>`; a collect-only count, compile, or focused pass is not qualification.
3. Run a fresh end-to-end paired capture only with admitted external corpus, model, Semble environment, quiet host, and exact-source inputs. If unavailable, report `BLOCKED`/`NOT_RUN`, not a quality or speed claim.
4. Close only after source, schema, docs, tests, generated prompt files, and proof artifacts agree. Reopen on any newly reachable legacy reader, dual IR, stale receipt, or undocumented rebuild path.

## Immediate ownership / non-goals

- This document and the prompt-manager policy are the only edits in this planning step. No runtime migration, data deletion, evaluator rewrite, or benchmark result is claimed.
- Avoid simultaneous writes to the currently dirty retrieval driver/adapter/Rust files. The W1 evaluator owner can proceed independently after W0; W2 integration is serial at the shared contract/schema seam.
- Do not confuse `schema_version` used on independently stored/wire artifacts with internal IR versioning. Keep fail-closed format discrimination even while deleting legacy readers.
