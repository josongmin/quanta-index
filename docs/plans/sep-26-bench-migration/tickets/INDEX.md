# SEP-26 Benchmark Migration — ticket index

Status: **PARTIAL implementation; see [CURRENT-AUDIT.md](CURRENT-AUDIT.md)** for current source-backed gaps and corrections. The older [CLOSEOUT.md](CLOSEOUT.md) is historical, not a full-completion receipt. Registration, typed fixtures and policy checks do not prove every profile can execute. See [CLOSEOUT.md](CLOSEOUT.md) for the historical per-ticket
implementation / focused-verification / integration / qualification verdicts and
the exact receipts. The `PLAN / NOT_RUN` status below is the plan-time state. Initially written against `f16bad934ec6b5b902eaf2d8633d734a0c34d37c` on 2026-09-26 while the shared checkout was dirty; re-audited at `8d23137cad89f2118377743824b8c841899593b1` with no intervening committed benchmark-control-plane change. The worktree remains dirty and may move again. This packet authorizes no benchmark result, baseline, or product win. Re-audit source and ownership before implementation; do not overwrite concurrent retrieval/RBR edits. See the [source-backed audit and corrections](AUDIT.md).

## Objective

Make every Quanta Index benchmark discoverable and reproducible through one registration, execution, evidence, and verdict control plane. Keep measurement methods and scorers specific to their question: a Criterion microbenchmark, daemon load test, retrieval judgment, and recorded agent outcome must not be collapsed into one number or one fake latency-row schema.

Breaking changes to the **current** benchmark protocol and CLI are permitted. Historical captures remain immutable and are never silently upgraded into current qualification. Product behavior and the [RB-00–RB-06](../../sep-23-retrieval-bench/tickets/INDEX.md) / [RBR-00–RBR-12](../../sep-26-retrieval-remediation/tickets/INDEX.md) relevance contracts are not relaxed by this migration.

## Placement decision

| Component | Target location | Boundary |
| --- | --- | --- |
| Small Rust hot-path benchmarks | Owning `crates/<product>/benches/` | Measure public/hot-path APIs; no benchmark dependency in production dependency edges. |
| Shared test and benchmark fixtures | Keep `crates/quanta-index-searchd-harness/` initially; `testing/searchd-harness/` is conditional | Existing harness also serves tests. Move only if BM-01 demonstrates a dependency, ownership, or measured build-cost benefit; directory cosmetics are insufficient. |
| Product/system benchmark producers | `benchmarks/system/` and existing `benchmarks/retrieval/` | Separate Cargo packages; real daemon/SDK and resource lifecycle. |
| Rust evidence protocol and candidate CLI | `benchmarks/bench-protocol/`; `benchmarks/benchctl/` only after BM-03 go/no-go | Benchmark-only workspace packages; product crates must not depend on them. If Rust CLI is rejected, Python remains the sole current CLI. |
| External product adapters/scorers | `tools/benchmark/` | Rust CLI invokes allowlisted tools; Python may remain the metric owner where appropriate. |
| Corpora, gold, model assets, raw traces, run artifacts | Outside the Git checkout | Repository keeps schemas, generator, manifests/recipes, and small fixtures only. |

New benchmark-only packages stay in the **root Cargo workspace** and share its toolchain and `Cargo.lock`. A nested workspace or second Git repository requires a measured incompatibility that cannot be handled by package selection and CI lane isolation. Add `default-members` only after checking existing root commands; `--workspace` still includes all members. Moving files is not itself a performance or quality improvement. The Rust CLI is a design target, **not a mandate to discard already-proven Python guards**: BM-03 must pass a vertical-slice parity/cost decision before replacement proceeds.

## Current source anchors and observed gaps

- `tools/benchmark/registry.toml` is the sole registration authority;
  `manifest.py` is a read-only native projection, not a second manifest.
  `benchctl.py` remains the selected current orchestrator; the Rust CLI was not
  adopted. `manifest.json` is removed.
- `benchmarks/bench-protocol` owns the typed purpose-specific envelope.
  Python promotion uses the same canonical evidence representation. Complete
  profile records and custody/GC checks are shared, not fabricated latency rows.
- Both crate-local Criterion targets are registered and have an execution,
  capture and raw-replay adapter. The complete runtime/LQ measurement sequence
  failed at the runtime build deadline; registration/focused tests are not
  a complete micro measurement receipt.
- `recorded_capture.py` imports external inputs with explicit unauthenticated
  scope. Clean snapshot `9e0f9371` passed 272 focused tests and real CLI
  import/validate/two-family fresh-process replay on fixed fixtures. This is
  contract proof, not authenticated real-agent or performance qualification.
- Retrieval scorers/producers remain their existing owners. The common CLI
  now has a contract-proof adapter using typed test counts and frozen SDK
  binaries, a five-product lexical recorded-scoring capture adapter and a
  paired diagnostic bridge to the existing native execution/verdict owner.
  Frozen paired/CLI contract proof passed 112 tests; the complete selected
  canonical Python recipe passed 579 tests. The common external corpus release
  manager now materializes and replays code/developer-search views from retained
  Git objects; actual gin/ripgrep input creation and fresh validation passed.
  Common lexical corpus-view/query binding and retained Git-object replay are
  implemented, with actual comparator universe attestation explicitly false.
  Fresh lexical search and the actual two-repo comparison pilot remain open. Mandatory compiled
  test executable custody is implemented with context v2 and 204 frozen focused
  Python tests; paired-consumer integration and fresh terminal native proof
  remain open. Monitored host-lease admission and hosted CI are separate proof/code
  boundaries, detailed in [CURRENT-AUDIT.md](CURRENT-AUDIT.md).
- `tools/ci/source_closure.py` binds the benchmark control plane and owning
  micro crates. Planning history is intentionally excluded from that closure;
  documentation edits must not invalidate unchanged normative code evidence.

The registry separates `retrieval-diagnostic` pair execution from
`lexical-diagnostic` five-product scoring. The lexical producer/schema and proof,
pair and lexical validator owners are distinct; file hit rate and query-macro
file recall are distinct measurements. Current proof/lexical owner tests do not
constitute complete diagnostic profile execution or product qualification.

## Tickets

| ID | Purpose | Depends on | Terminal deliverable |
| --- | --- | --- | --- |
| [BM-00](BM-00-inventory-and-authority.md) | Exact inventory and authority map | None | Every producer, scorer, CI call, artifact and source-closure owner accounted for |
| [BM-01](BM-01-workspace-boundaries.md) | Cargo placement and dependency direction | BM-00 | Benchmark packages separated without product dependency inversion |
| [BM-02](BM-02-evidence-protocol.md) | Typed common evidence and immutable runs | BM-00 | One validated envelope with purpose-specific payloads and adversarial replay |
| [BM-03](BM-03-registry-and-cli.md) | Single registry and Rust CLI | BM-02 | `list/plan/run/validate/compare/replay` through allowlisted producers |
| [BM-04](BM-04-system-and-micro.md) | System/load/freshness and Rust micro rails | BM-01/03 | Existing measured behavior preserved under one control plane |
| [BM-05](BM-05-retrieval-bridge.md) | Shared corpus/capture/proof; separate relevance metrics | BM-02/03; coordinate with RBR-12 | Retrieval registered without fake spans or duplicate scorer |
| [BM-06](BM-06-recorded-and-experiments.md) | Agent outcome and recorded experiments | BM-02/03 | Recorded-only evaluation with explicit authenticity boundary |
| [BM-07](BM-07-ci-baselines-cutover.md) | CI, baseline admission, migration closeout | BM-04/05/06 | One current authority, old authority removed, full replay/negative proof |

The [TEST-PLAN](TEST-PLAN.md) defines common evidence, failure, and cutover gates. Ticket DoD is **implementation DoD**, not automatic benchmark qualification. Implementation artifacts: [BM-00-INVENTORY.md](BM-00-INVENTORY.md), [BM-03-DECISION.md](BM-03-DECISION.md), [BM-07-MIGRATION-MATRIX.md](BM-07-MIGRATION-MATRIX.md), [CLOSEOUT.md](CLOSEOUT.md).

## Execution and stop rules

1. Freeze BM-00 inventory and decide which existing path remains authoritative until cutover. No dual-current schema or simultaneous baseline authority.
2. Build BM-02 protocol and negative fixtures before switching producers. BM-01 layout moves can happen only after the dirty owners release shared files; move+behavior change must be separate reviewable changes.
3. Prototype BM-03 CLI on one representative DSL rail. Compare raw samples, refusals, verdicts, execution cost, and maintenance surface against the Python path. If Rust cannot preserve the current fail-closed guarantees at reasonable cost, keep Python `benchctl` as the orchestration authority and implement the same typed registry/evidence contract there; document the decision rather than shipping two live CLIs.
4. Replace CI/direct Just paths only when the target family reaches parity. BM-07 removes the old current path atomically; historical replay is read-only.
5. Run clean, pinned-source qualification from an exact snapshot, not from the actively edited main checkout. Main can remain the implementation checkout. Missing external gold, license approval, model assets, or quiet host block only the corresponding qualified claim; do not synthesize them.

**Migration infrastructure DoD** and **benchmark qualification DoD** are separate. The former requires complete code/contract/CI integration and representative real or fixed-fixture replay; the latter additionally requires admitted external inputs and the declared host. A missing independent gold or quiet host cannot turn a correct control-plane migration into a fabricated pass or an unfinishable implementation ticket.

## Completion language

Each ticket reports `implementation`, `focused verification`, `integration`, and `qualification` separately as `VERIFIED`, `FAILED`, `BLOCKED`, `NOT_RUN`, or `NOT_APPLICABLE`. A compiled bench is not a measured bench; a valid artifact is not a valid comparison; a benchmark verdict is not a release/deployment verdict.
