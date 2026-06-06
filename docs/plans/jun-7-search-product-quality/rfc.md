# Jun 7 Search Product Quality RFC

Status: `planned`
Date: `2026-06-07`

This packet is product-quality work, not DSL-surface expansion work.

It follows the landed feature and verification packets:

- [../jun-4-sourcegraph-parity/rfc.md](../jun-4-sourcegraph-parity/rfc.md)
- [../jun-5-sourcegraph-tail-gaps/rfc.md](../jun-5-sourcegraph-tail-gaps/rfc.md)
- [../jun-6-sourcegraph-expansion/rfc.md](../jun-6-sourcegraph-expansion/rfc.md)
- [../jun-7-verification-hellgates/rfc.md](../jun-7-verification-hellgates/rfc.md)

## 1. Scope Lock

Current live truth:

- DSL coverage is strong.
- Sourcegraph-style shipped subset is broadly closed.
- fast hellgates, broad daemon lifecycle rails, cross-repo ingress proof, and
  perf compare lanes are already landed.

Current gap:

- the repo proves that search surfaces execute correctly
- it does **not yet** prove that results are product-quality-optimal across:
  - ranking quality
  - snippet / explain quality
  - large corpus scale behavior
  - latency tail
  - operator ergonomics
  - ambiguous intent handling
  - UI / UX integration contracts

This packet owns those quality layers.

It does **not** reopen:

- Sourcegraph DSL feature coverage
- structural grammar verdicts already closed as explicit unsupported
- hellgate split architecture already closed in `jun-7-verification-hellgates`
- semantic retrieval or hybrid fusion quality; those are deferred to a later
  packet

Minimum external competitive floor:

- for overlapping non-semantic code-search surfaces, Sourcegraph lexical
  behavior is the minimum external baseline
- this packet may only compare on overlapping shipped surfaces
- internal green metrics without that overlap check are not enough for any
  “best-in-class” claim

## 2. Problem Statement

Current shipped stack is strong on:

- parser / lowering correctness
- fail-closed behavior
- runtime / front-door / parity proof
- structural and history route determinism

But production search quality depends on additional layers:

1. result ordering quality
2. snippet usefulness
3. explanation fidelity
4. scale behavior on realistic corpus sizes
5. p95 / p99 stability under warm daemon conditions
6. operator diagnosis surfaces
7. user-facing query repair ergonomics

Without those layers, the stack can be technically correct but still weak as a
human-facing search product.

## 3. Architecture Bar

This packet follows these implementation defaults:

1. Relevance work is offline-evaluable.
   - judged truth, route-split metrics, deterministic tie-break policy
   - no score-only or anecdotal-query closeout
2. Snippet and explain work is provenance-backed.
   - offsets, spans, hit windows, planner/runtime contribution sections
   - no “field present” or “summary non-empty” closeout
3. Scale work is reproducible.
   - seeded synthetic corpora, tier manifests, explicit budgets
   - no one-off local corpus screenshots as evidence
4. Tail work is route-aware.
   - lexical, symbol, structural, history, and runtime-catalog families do not
     share one universal threshold
5. Operator UX work is machine-readable first.
   - `searchctl` read surfaces and JSON payloads before log spelunking
6. Ambiguity UX work is repair-oriented but fail-closed.
   - typed repair metadata
   - no silent rewrite, no best-effort fallback
7. UI contract work is DTO-first.
   - typed fields, stable semantics, SDK and CLI parity
   - no opaque consumer-side guesswork
8. Competitive claims are overlap-scoped.
   - compare against Sourcegraph lexical only on overlapping non-semantic
     surfaces
   - never pad a claim with out-of-scope feature differences

## 4. Reviewer Rejection Checklist

Reject a closeout if any of these are true:

- a metric improved but the truth source is not reproducible
- lexical and symbol or structural quality changed but are still reported as
  one score
- an external-quality claim exists without a Sourcegraph lexical overlap report
- snippet or explain “quality” is still just field presence
- scale claims are inferred from toy fixtures
- tail claims still rely on `p50` only
- operator UX depends on reading source or logs for routine diagnosis
- ambiguity handling rewrites user intent silently
- UI contract changes shipped without wire and consumer proof

## 5. Non-Goals

- no new Sourcegraph predicate family work
- no reopening landed unsupported structural SG direct lexical cells
- no docs-only quality claim
- no “bench green implies relevance green” shortcut
- no product-web frontend build inside this packet
- no semantic retrieval or hybrid fusion quality work in this packet
- no competitive “beats Sourcegraph” claim without overlap-scoped evidence
- no learned reranker or ML ranking policy unless a separate ADR lands

## 6. Workstreams

### 6.1 J7Q-00 — Scope Lock and Measurement Policy

Goal:

- freeze the distinction between correctness proof and product-quality proof

Must define:

- relevance metrics vs latency metrics
- correctness gates vs advisory diagnostics
- what is blocking vs non-blocking

DoD:

- packet docs explicitly separate:
  - correctness
  - relevance
  - scale
  - latency tail
  - ops UX

Preferred implementation direction:

- freeze one quality vocabulary table and one claim matrix before any ticket
  broadens behavior
- every later ticket must map its evidence to that claim matrix explicitly

### 6.2 J7Q-01 — Ranking Quality and Relevance Corpus

Goal:

- prove search ordering quality, not just executable correctness

Owner seams:

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scenarios.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`

Required work:

1. add a relevance golden corpus with query-intent labels
2. add expected top-1 / top-k containment assertions
3. compute blocking relevance metrics:
   - `MRR@10`
   - `NDCG@10`
   - `Recall@20`
4. split lexical / symbol / structural / history-backed route quality results
5. add a Sourcegraph lexical overlap suite for representative shipped query
   families
6. pin deterministic tie-break behavior separately from relevance quality

DoD:

- a lexical or symbol or structural ranking regression fails before broad
  daemon reruns
- score equality is not mistaken for relevance quality
- designated overlapping lexical query families do not materially underperform
  the Sourcegraph lexical floor

Failure modes to prevent:

- correct candidate set, wrong top result
- one route family regressing while blended reporting still looks stable
- internal relevance numbers green while external lexical overlap still loses
- route-local score changes silently degrading developer-facing relevance

Preferred implementation direction:

- graded relevance corpus with stable query IDs and route labels
- metrics computed in one dedicated evaluation rail
- include hard negatives and near-duplicate distractors, not only easy positives
- maintain one overlap-scoped external comparison sheet against Sourcegraph
  lexical query families
- deterministic tie-break policy tracked separately from relevance scores

### 6.3 J7Q-02 — Snippet and Explain Quality

Goal:

- make result payloads useful to humans, not just structurally valid

Owner seams:

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/results/explanation.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/explain.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`

Required work:

1. add snippet golden assertions for:
   - exact phrase hit
   - regex hit
   - multiple hits in one file
   - long-line truncation
   - symbol snippet payloads
2. add explain-quality assertions for:
   - required planner stages
   - required engine ordering
   - expected contribution rows
   - route-specific rationale and provenance sections
3. distinguish “field present” from “useful explanation”

DoD:

- snippet rails fail on degraded excerpt quality
- explanation rails fail on low-information or inconsistent summaries

Failure modes to prevent:

- result correct but snippet misleading
- explanation summary populated but semantically useless
- ranking rationale diverges from emitted contribution payload

Preferred implementation direction:

- use structured window offsets, highlight spans, and sectioned explanation
  payloads
- derive explanation claims from actual planner/runtime provenance, not synthetic
  post-hoc text
- keep snippet windows hit-centered and deterministic under repeated matches

### 6.4 J7Q-03 — Large Corpus Scale Tiers

Goal:

- prove search remains operational on realistic corpus sizes

Owner seams:

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/bin/dsl_warm_matrix.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/bin/dsl_cold_matrix.rs`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/README.md`

Required work:

1. define scale tiers:
   - `small`
   - `medium`
   - `large`
   - `xlarge`
2. for each tier, measure:
   - ingest time
   - open / reopen time
   - query latency
   - memory footprint
   - restart recovery
3. add synthetic large-corpus authority generator
4. record per-route budgets

DoD:

- scale tiers are reproducible
- the repo can state where current scale limits are

Failure modes to prevent:

- toy fixture green, real corpus red
- reopen / restart cliffs appearing only after deployment

Preferred implementation direction:

- seeded synthetic generator, persisted tier manifest, and explicit per-tier
  route mix
- manifests should record repo count, file-size distribution, hit density, and
  symbol density
- report current limit honestly even when not yet “large enough”

### 6.5 J7Q-04 — Latency Tail Hardening

Goal:

- promote p95 / p99 from advisory-only to route-aware quality gates where
  justified

Owner seams:

- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/README.md`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/benches/dsl_query_matrix.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/bench_support.rs`

Required work:

1. split tail budgets by route family:
   - lexical
   - symbol
   - structural
   - history
   - runtime catalog
2. add route-specific tail thresholds
3. preserve p50 gate but stop pretending p95 / p99 are merely cosmetic
4. expose enough metadata to explain tail regressions

DoD:

- recurring p95 / p99 cliffs fail where they matter
- blocking thresholds are documented, not hand-waved

Failure modes to prevent:

- p50 green masking unusable warm-tail latency
- daemon accept-loop and regex-heavy routes regressing silently

Preferred implementation direction:

- route-family budgets first, threshold hardening second
- artifact schema must carry enough metadata for diagnosis, not just pass/fail
- include candidate-count, regex-complexity, and repo-fanout buckets where
  those dimensions affect tails

### 6.6 J7Q-05 — Operator Ergonomics

Goal:

- make the stack diagnosable without code spelunking

Owner seams:

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchctl/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/search.rs`

Required work:

1. add operator-facing commands:
   - `searchctl doctor`
   - `searchctl readiness`
   - `searchctl generation show`
   - `searchctl explain --json`
   - `searchctl perf-tail`
2. surface route, generation, authority, and readiness state directly
3. make typed remote errors actionable

DoD:

- common operational questions are answerable without reading runtime internals

Failure modes to prevent:

- strong tests but poor operator diagnosis
- active-generation or authority mismatches hidden behind generic transport noise

Preferred implementation direction:

- read-only diagnosis commands with machine-readable output first
- route, generation, authority, and readiness state must be first-class fields
- diagnosis commands should have stable exit semantics and stable JSON field
  names

### 6.7 J7Q-06 — Ambiguous Intent Handling

Goal:

- improve query repair ergonomics while preserving fail-closed behavior

Owner seams:

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lq-bridge/src/errors.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchctl/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/error.rs`

Required work:

1. add repair hints to typed query failures
2. expose supported alternative shapes for common user mistakes
3. keep canonical error codes stable
4. separate unsupported, ambiguous, and wrong-route failures cleanly

DoD:

- typed failures stay fail-closed
- repair path becomes discoverable

Failure modes to prevent:

- expert-only debuggability
- ambiguity closed in code but still high-friction for real users

Preferred implementation direction:

- add repair metadata to existing typed errors
- keep canonical error code stable while adding example queries and supported
  shapes
- route hints and docs anchors should come from typed payloads, not CLI-only
  prose

### 6.8 J7Q-07 — UI / UX Contract Surface

Goal:

- improve downstream consumer ergonomics without building the actual frontend

Owner seams:

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/results/query_responses.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/search.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchctl/src/lib.rs`

Required work:

1. define UI-consumable payload additions where needed:
   - highlight spans
   - snippet window offsets
   - explanation sections
   - route used
   - confidence or provenance hooks where justified
2. keep them typed and deterministic
3. avoid frontend-only shorthand in core grammar

DoD:

- downstream UI can render useful search results without inventing semantics

Failure modes to prevent:

- opaque snippets with no highlight anchors
- explanation payload usable only by tests, not by real consumers

Preferred implementation direction:

- version typed DTOs where necessary
- add offsets, spans, and provenance fields before any rendering sugar
- UI-facing fields should be consumable without regex parsing free-form strings

### 6.9 J7Q-08 — Followthrough and Gate Integration

Goal:

- integrate quality work into a stable verification story

Owner seams:

- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/README.md`
- `/Users/songmin/Documents/code-new/quanta-index/Justfile`
- `/Users/songmin/Documents/code-new/quanta-index/docs/analysis/jun-4-dsl-capabilty.md`

Required work:

1. add aggregate commands for:
   - relevance quality
   - scale tiers
   - tail gates
   - operator contract checks
2. document what is blocking vs advisory
3. keep `jun-7-verification-hellgates` focused on correctness verification

DoD:

- quality gates are callable, scoped, and documented
- no packet mixes correctness with product quality again

Preferred implementation direction:

- stable commands must state their quality dimension and blocking/advisory role
- aggregate targets may orchestrate multiple rails but must not erase proof
  boundaries

## 7. Ticket Order

1. `J7Q-00` scope lock and measurement policy
2. `J7Q-01` ranking quality and relevance corpus
3. `J7Q-02` snippet and explain quality
4. `J7Q-03` large corpus scale tiers
5. `J7Q-04` latency tail hardening
6. `J7Q-05` operator ergonomics
7. `J7Q-06` ambiguous intent handling
8. `J7Q-07` UI / UX contract surface
9. `J7Q-08` followthrough and gate integration

Execution rule:

- `J7Q-01` and `J7Q-02` can run in parallel
- `J7Q-03` and `J7Q-04` can run in parallel after the scenario schema is
  adequate
- `J7Q-05` and `J7Q-06` can run in parallel
- `J7Q-08` is last

## 8. Verification Policy

Quality closeout must report:

- command used
- covered surface
- excluded surface
- final status
- whether the result is:
  - correctness proof
  - relevance proof
  - scale proof
  - latency proof
  - operator contract proof

Minimum command families expected by this packet:

- `./scripts/cargow test -p quanta-index-searchd-runtime --test ...`
- `./scripts/cargow test -p quanta-index-searchd-harness --test ...`
- `just rust-bench-dsl-truth`
- `just rust-verify-hellgate-fast`
- `just rust-verify-hellgate-broad`
- `just rust-bench-dsl-compare`

Preflight remains repo-global `unverified` until:

- `scripts/check-persona-target-policy.sh`
- `scripts/cg-agent-session`

actually exist and pass.

## 9. Final Claim Discipline

This packet is complete only when:

1. result quality is measured, not assumed
2. snippet / explain usefulness is asserted, not implied by field presence
3. scale and tail budgets are explicit
4. operator-facing diagnosis surfaces are real
5. ambiguity remains fail-closed but becomes repairable
6. UI contract surfaces are typed and consumable

Until then:

- DSL capability may remain strong
- but “production search product quality” is still incomplete
