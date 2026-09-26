# SEP-27 — Code search remediation RFCs

Status: **PROPOSED packet; current implementation evidence is scoped per RFC**.
L5 producer/parser and its shared integration are verified on the exact dirty
snapshot recorded in [L5_HANDOFF](handoffs/L5_HANDOFF.md). This does not qualify
the other RFCs or whole-repository CI/release.
Prepared: 2026-09-27. Source baseline:
`main@66cee47efdda7c5f3886ac58690aa645f44f691f`, clean before these documentation edits.

**Engine re-audit, 2026-09-27:** [engine-audit.md](engine-audit.md) supersedes the
initial engine inventory where noted. It adds count/exhaustion and scope-mutation
counterexamples, preserves the existing grouped collector and snapshot cursors,
and specifies semantic snippet witnesses. This re-audit ran in the concurrently
dirty checkout; pinned-binary/component evidence is not current-build qualification.

This packet turns the code-search RCA into separately owned engine, producer,
benchmark and integration proposals. It records what must change, why, how to
verify it and which claims remain unproved. It does not implement a ranker,
change an accepted ADR, publish a new corpus or qualify a benchmark.

## 1. Read order and authority

1. Read the [final engine audit](engine-audit.md), then the original
   [evidence register](evidence.md) for diagnostic context and proof scope.
2. Select an RFC from the table below; each owns its detailed proposal and DoD.
3. Use [CS-INT-01](rfcs/CS-INT-01-integration-and-qualification.md) for dependencies,
   source freeze, verification and handoff.
4. Consult [references](references.md) for production precedents and research
   available by 2026-09-27. They support design choices, not a Quanta speed claim.

Existing accepted contracts remain authoritative:

- [SEP-26-001: query, publication and result proof](../../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md).
- [SEP-26-002: observation, experiments and default policy](../../adr/SEP-26-002-retrieval-observation-experiment-and-default-policy.md).
- [SEP-26-003: evidence custody and qualification](../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md).
- [SEP-27-002: one benchmark orchestrator](../../adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md).
- [SEP-27-MISC execution SSOT](../sep-27-misc/tickets/INDEX.md): common benchmark
  orchestration, publication/GC, process lifecycle, bounded I/O and qualification.

This directory is the design authority for the **proposed code-search changes**,
not a second common benchmark execution tracker. On implementation, reference
these RFCs from the execution SSOT and update affected ADRs in the same change.
Do not duplicate MISC ticket bodies or create competing statuses. RFC acceptance
does not establish implementation, tests or product qualification.

The current source-closure allowlists include the existing MISC execution
contract, not this proposed packet. Before these RFCs govern an executable run,
include the adopted normative documents in the appropriate closure or consolidate
their adopted semantics into already bound ADR/execution documents. Record that
choice and test it; do not use an unbound proposal as a qualification contract.

## 2. RFC map

| Category | RFC | Purpose | Dependencies |
| --- | --- | --- | --- |
| Engine correctness | [CS-ENG-01](rfcs/CS-ENG-01-query-domain-and-result-contract.md) | Typed domain/projection, decoder and truthful bounded result windows | None |
| Engine publication | [CS-ENG-02](rfcs/CS-ENG-02-capability-publication-and-freshness.md) | Canonical file mutation, new product coverage authority and atomic source revisions | ENG-01 contract; payload agreement with PROD-01 |
| Engine relevance | [CS-ENG-03](rfcs/CS-ENG-03-definition-and-file-ranking.md) | Exact definition policy; existing grouped collector identity/resource fixes | ENG-01/02; BENCH-01/03 before tuning |
| Engine result presentation | [CS-ENG-04](rfcs/CS-ENG-04-match-anchored-snippets.md) | Semantic match witnesses, original-byte mapping and bounded previews | ENG-01; ENG-02/03 source/name authority |
| Producer coverage | [CS-PROD-01](rfcs/CS-PROD-01-parser-coverage-and-vite.md) | Parser compatibility, Vite blockers and all-file preflight | ENG-02 coverage contract; parser probes independent |
| Benchmark inputs | [CS-BENCH-01](rfcs/CS-BENCH-01-corpus-gold-and-holdout.md) | External corpus releases, independent gold and sealed holdout | Existing corpus/release owner |
| Benchmark evidence | [CS-BENCH-02](rfcs/CS-BENCH-02-native-response-validation.md) | Native-derived normalization and inconsistent-capture refusal | Existing evidence/custody owner |
| Benchmark evaluation | [CS-BENCH-03](rfcs/CS-BENCH-03-tracks-metrics-and-statistics.md) | Purpose-specific units, metrics, statistical admission and ablation | BENCH-01; BENCH-02 for real captures |
| Benchmark measurement | [CS-BENCH-04](rfcs/CS-BENCH-04-comparators-performance-and-incremental.md) | Local comparators, equivalent work, latency, resources and updates | BENCH-01/02/03; shared MISC execution prerequisites |
| Integration | [CS-INT-01](rfcs/CS-INT-01-integration-and-qualification.md) | Coordinated cutover, serial integration and final proof | Required scope of the nine RFCs above |

All ten RFCs originated as proposals. Current implementation and regression
status is recorded in each RFC and its source-bound handoff; the L5 update is in
[CS-PROD-01](rfcs/CS-PROD-01-parser-coverage-and-vite.md). Packet-wide final
qualification is **NOT_RUN**. Existing failed diagnostics are identified
individually in the evidence register.

## 3. Findings-to-work traceability

| ID | Finding | Evidence class | Primary RFC | Supporting RFC |
| --- | --- | --- | --- | --- |
| F01 | Symbol endpoint searches Text after select:file, then uses Symbol decoder; six INTERNAL results | Reproduced failed invariant | ENG-01 | BENCH-02, INT-01 |
| F02 | Missing top-10 gold files are actually lexical ranks 14, 31, 34; symbol rank 1 in three probes | Verified diagnostic, not overall quality | ENG-03 | BENCH-01/03 |
| F03 | Repeated file chunks consume top-k; 393 repeated-file slots in 710 returned chunks | Verified observation | ENG-03 | BENCH-03/04 |
| F04 | First-file success 142/180 versus first-span coverage 83/180 | Verified under existing mechanical gold | ENG-04 | ENG-03, BENCH-03 |
| F05 | Two valid TS forms fail pinned parsing; intentional invalid fixture also blocks Vite | Reproduced parser refusals | PROD-01 | ENG-02 |
| F06 | Lexical admission requires all-file symbols; zero definitions and parsing failure need different states | Existing strict contract; capability limitation | ENG-02 | PROD-01 |
| F07 | Gold contains docstring examples and omits real alternate definitions | Verified oracle defects | BENCH-01 | BENCH-03 |
| F08 | Normalized cs paths can contradict native stdout and still change score 19/20 to 20/20 | Reproduced failed rejection invariant | BENCH-02 | INT-01 |
| F09 | Chunk/file/span units, observed order and count:all/top-k work differ | Verified comparison limitations | BENCH-03 | BENCH-04 |
| G01 | Incremental freshness, failed-parse symbol invalidation and concurrent update proof absent | NOT_RUN; not a reproduced stale-data defect | ENG-02 | BENCH-04 |
| G02 | Full holdout improvement, equal-work speed and current-source qualification absent | NOT_RUN | INT-01 | BENCH-01/03/04 |

## 4. Final design boundaries

- One validated query plan binds snapshot, domain, matching semantics, output
  unit, ranking policy, completeness policy and budgets. A projection cannot
  silently replace the domain while preserving an incompatible decoder.
- Producer-owned parsing emits source-bound capability facts. Text, symbols and
  capability state advance atomically; stale symbols cannot survive in a newer
  file revision after parse failure. Strict completeness currently exists in the
  benchmark producer; the proposed product gate must be implemented explicitly.
- Content matching, distinct-file ranking, definition ranking and context
  presentation have separate objectives. Exact-name/case features are tested on
  development data; no global default change is justified by three examples.
- Reuse the existing native grouped collector and 240-byte renderer boundary.
  Correct their source-identity/budget and semantic-witness gaps, respectively.
  Native NFC matching remains distinct from original-byte coordinates.
- Native responses determine normalized results. The scorer does not trust a
  precomputed hit flag or transport success as independent proof.
- Corpus payloads, generated gold, models and runs remain outside Git. Schemas,
  recipes, independent oracle adapters and small fixtures live in this repository.
- Keep `registry.toml`, `benchctl.py`, `bench-protocol` and domain evaluators as
  their existing owners. Benchmark-only crates stay in the root Cargo workspace;
  product crates gain no normal dependency on benchmark-only packages.
- Keep Sourcegraph local in the comparison. Semble hybrid is labeled as hybrid.
  A supported lexical-only mode must be proved before using that label.

## 5. Sequencing and parallel ownership

Start ENG-01, BENCH-01 and BENCH-02 as independent lanes. Parser compatibility
probes in PROD-01 may proceed independently. Agree the ENG-02 capability payload
before changing producer and publication consumers. Ranking/snippet work can be
implemented after the engine contracts stabilize; tuning waits for independent
development labels and metric contracts. The holdout is sealed before tuning.

One integrator resolves shared contract, SDK and evaluator boundaries. Final
native runs use a single frozen combined source. Reuse one compatible capture
across claims rather than repeating it under multiple ticket names.

Read [CS-INT-01](rfcs/CS-INT-01-integration-and-qualification.md) for lane ownership,
cutover, required test classes, admission and stop conditions.

## 6. Explicitly deferred

ANN replacement, dense/graph expansion, learned reranking, full precise-reference
resolution and a new Rust orchestration CLI are not required to close these
lexical findings. SCIP can be a producer adapter when precise facts are
available. Context-quality research is an additional benchmark track, not a
prerequisite for automated literal/identifier/definition checks. User-owned
manual approvals and annotation work are not automatic coding subtasks.

No claim of a new product ranking, exact speedup, deployed fix or completed
benchmark follows from this documentation packet.
