# 2026-09-26 benchmark-migration plan audit

> Archive classification: historical plan audit. Current implementation gaps
> and corrections are owned by [CURRENT-AUDIT.md](CURRENT-AUDIT.md); custody
> follows [SEP-27-001](../../../adr/SEP-27-001-documentation-authority-and-historical-record-custody.md).

Audit state: **inspected clean control-plane structure VERIFIED; implementation and benchmark qualification NOT_RUN**. Source HEAD at recheck: `8d23137cad89f2118377743824b8c841899593b1`, dirty shared checkout. `Cargo.toml`, `Justfile`, `tools/benchmark/{manifest.json,manifest.py,benchctl.py}`, `crates/quanta-index-searchd-harness/{Cargo.toml,src/artifact.rs}`, `.github/workflows/correctness.yml`, `tools/ci/source_closure.py`, `tools/benchmark/retrieval/sourcegraph.py` and `tools/benchmark/agent_outcome/__main__.py` were clean tracked files at recheck. Retrieval evaluator/runner/schema files were dirty; their final semantics and proof are **not** verified here. The packet was first written at `f16bad934ec6b5b902eaf2d8633d734a0c34d37c`; `git diff --name-only f16bad93..8d23137 -- Cargo.toml Justfile tools/benchmark benchmarks .github/workflows tools/ci/source_closure.py` returned no committed path change in the audited benchmark control plane. This is a code-and-plan audit, not a completed migration or a new benchmark result.

## Findings and disposition

| ID | Classification | Evidence and failure mode | Plan correction |
| --- | --- | --- | --- |
| A1 | VERIFIED current structure | [`manifest.py`](../../../../tools/benchmark/manifest.py) admits only `dsl-latency` as a baseline comparator; the plan-time `manifest.json` (since replaced by [`registry.toml`](../../../../tools/benchmark/registry.toml), see [BM-00-INVENTORY.md](BM-00-INVENTORY.md)) omitted retrieval, agent outcome and crate-local Criterion as registered evidence families. One current control plane did not yet cover all benches. | BM-00 exact inventory; BM-03 typed registry and one CLI; BM-04/05/06 register by purpose. |
| A2 | VERIFIED current structure | [CI DSL job](../../../../.github/workflows/correctness.yml) directly calls warm/cold producers, validators and comparators while [`benchctl.py`](../../../../tools/benchmark/benchctl.py) has guarded `run` logic. CI and local paths can diverge. | BM-07 replaces direct job steps only after per-family parity and negative tests. |
| A3 | VERIFIED current structure | [`BenchArtifactV1`](../../../../crates/quanta-index-searchd-harness/src/artifact.rs) uses a latency-row shape and a direct-file writer; [retrieval](../../../../tools/benchmark/retrieval/evaluator.py) and [agent outcome](../../../../tools/benchmark/agent_outcome/__main__.py) have different valid payloads. A universal timing row would falsify their semantics. | BM-02 one envelope with typed payloads, immutable run directories and historical read-only replay. |
| A4 | Design risk corrected | Python [`benchctl.py`](../../../../tools/benchmark/benchctl.py) already checks clean source, host preflight, frozen HEAD and DSL baseline admission. Rewriting it in Rust without parity could reduce guarantees and consume work without user-visible benefit. | BM-03 now has a measured Rust vertical-slice go/no-go. If it fails, retain one Python orchestrator and still implement the registry/evidence contract. |
| A5 | Design risk corrected | [`Cargo.toml`](../../../../Cargo.toml) has one virtual workspace, no `default-members`, and both retrieval and [`searchd-harness`](../../../../crates/quanta-index-searchd-harness/Cargo.toml) as members. The latter is also a runtime test dev-dependency. Moving it solely because of its directory name creates churn. | BM-01 defaults to no harness move; require dependency or measured build-cost benefit. Keep one workspace/lockfile unless a real incompatibility appears. |
| A6 | Plan defect corrected | Initial DoD coupled infrastructure migration to actual qualified retrieval/agent results. Independent gold, product index attestation, model assets and quiet host may be absent even when registry/protocol/CI integration is correct. | [TEST-PLAN](TEST-PLAN.md) now has separate infrastructure and benchmark-claim axes. BM-04/05/06/07 allow explicit `BLOCKED`/`NOT_RUN` qualification without a fake result. |
| A7 | Proof gap, not a proven product defect | Current [`benchctl.py`](../../../../tools/benchmark/benchctl.py) captures host preflight before producers. A clean preflight alone cannot establish that the host stayed isolated during a long measurement. | BM-04 requires an exclusive lease plus capture-time host/load observations; missing intervals cannot qualify performance. |
| A8 | Proof gap, not a proven product defect | [Sourcegraph capture validator](../../../../tools/benchmark/retrieval/sourcegraph.py) labels its result `diagnostic_unqualified` and stream order as observed only. External adapters and indexed universe are not currently a common qualified product protocol. | BM-05 defines native-default versus controlled lanes, searchable-universe/rank proof, sandboxed adapters and typed unsupported/timeout/unjudged results. |
| A9 | VERIFIED current structure | [`source_closure.py`](../../../../tools/ci/source_closure.py) binds Sep-23 and Sep-26 retrieval contracts but not this new packet. Blindly binding every planning-status edit would also create unnecessary proof churn. | BM-00 classifies normative contract files versus planning/history before changing closure, and tests inclusion/exclusion mutations. |

`VERIFIED current structure` means the named source behavior/path was inspected, not that a benchmark passed. `Design risk corrected` means this packet was amended; code behavior has not changed. `Proof gap` means insufficient qualifying evidence, not an asserted functional failure.

## Accepted architecture after audit

- Same Git repository and root Cargo workspace; crate-local microbenches remain with their product crate. E2E producers and shared benchmark protocol are separate benchmark-only packages. Existing shared test harness stays put by default.
- One **data-only registry**, one **current orchestration CLI**, one common evidence envelope, and payload/scorer ownership by metric family. Rust CLI replaces Python only after BM-03 parity/cost acceptance; both cannot remain current.
- External corpus/gold/model/raw captures stay outside checkout. Code source closure and external-input identity are separate, both checked at replay. `latest` is a pointer, never an admitted baseline.
- Control-plane migration and qualified quality/performance/agent claims close independently. No score, speedup or tool-selection claim follows from completing these tickets.

## Remaining implementation decisions

1. BM-00 must produce exact current inventory and normative closure list. The family count and CI mapping are not asserted by this audit.
2. BM-03 must settle Rust versus Python with one real vertical slice and adversarial parity; the target language is not decided by preference alone.
3. BM-05 must settle typed file/line/span qrels and external product rank/index proof in concert with RBR-12; do not create a second current relevance scorer.
4. BM-07 must obtain clean-source integration receipts after the final normative doc/code revision. Actual external qualified claims remain separately gated by the RB/RBR contracts.

No Cargo-heavy build, full test suite, clean-host timing run, admitted product pair, or agent trajectory run was executed for this audit. Documentation link/format validation is a document check only.
