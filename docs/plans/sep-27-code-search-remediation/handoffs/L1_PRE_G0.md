# L1_PRE_G0 — query/domain/window readiness

Update: L0 task `01a0dea8-aa8e-7d73-857f-b174f7be64fe` subsequently approved
G0-L1: a generic existing `LexicalSearchPageV1`, a shared pure validated plan and
logical-empty proof/provenance. It leased L1 the core outbound/query_plan/service
files, contract-base query_window, affected test doubles and one dispatcher test
registration. The pre-G0 inventory below remains a historical snapshot, not the
current implementation verdict. Eight permanent regression tests are now written
and the first source-bound behavioral RED run is in progress. Final results will
be recorded in L1_HANDOFF; do not treat this update as a passing receipt.

State: **partial**. Implementation, behavioral RED/GREEN and public-path proof:
**NOT_RUN**. Shared-interface adoption: **BLOCKED**, because no `G0_READY` or
identified L0 coordinator has been supplied. This is a source trace and regression
design, not a remediation receipt or an accepted shared contract.

## Source and ownership

- Initial revision: `66cee47efdda7c5f3886ac58690aa645f44f691f`, dirty benchmark,
  Cargo and control-plane work. L1-owned source files were initially clean.
- A concurrent task advanced HEAD to
  `106d7abec2dd3fa03f9db5a19a3de41df2f0afad` during inspection. The committed
  delta did not change the engine/core/contract source inspected here. L3 is
  concurrently editing `budgeted_search.rs`, `ranked_page.rs`, and
  `searcher/paging.rs`; those bytes are not frozen for execution.
- L1 has changed no production Rust, common DTO, module export, build config or
  other lane's files. No agent was spawned and no commit/push/reset was run.
- Static evidence manifest: `L1_PRE_G0.source.json` beside this document. It binds
  source bytes, revision and dirty state only; it does not bind a compiled binary,
  dependency build closure or test execution.

## Current execution paths

| Boundary | Current owner and observed behavior | Required regression |
| --- | --- | --- |
| Symbol endpoint | `routes/lexical.rs::symbol_with_execution` lowers through a temporary Text request, composes language constraints, then calls `LexicalPolicy::validate_query_with_constraints` | Endpoint capability must survive lowering and reject domain/projection conflicts before empty shortcuts |
| Generic Text endpoint | `planning.rs::plan_lexical_text_query` and `port.rs::search_constrained` allow explicit Symbol domain selection | Keep `type:symbol` and `select:symbol` valid, including normal positive hits |
| Domain preparation | `prepare.rs::prepare_query_for_doc_kind` starts `doc_kind=None`; type/select overrides the default; `compile.rs::prepare_executable_query` tests unsupported Symbol content against that resolved domain | A typed Symbol endpoint cannot become Text while preserving the Symbol decoder |
| Decode | `port.rs::search_symbols_constrained` and `search_symbols_all` always use `document_to_symbol_candidate`; manual scan also uses `prepared.doc_kind` | A validated route must bind one compatible decoder for indexed and manual paths |
| Language empty | Both dispatcher routes return before read-view acquisition on `force_empty` | Pure invalid requests must reject first; capability reads remain legal when necessary |
| Predicate empty | `prepare_executable_query` returns `None` when `prepare_predicate_plan` proves empty, before `planner_preflight_expr` | Reject invalid `count:0` and pure planner conflicts even when a repo/file predicate is empty |
| Execution facts | `window.rs::pageable_window_v2` unconditionally constructs `LaneTraceV1::new(lane, true, ...)` | Logical empty must report no backend invocation; recorder and window must agree |
| Symbol count | `page_limit` applies `min(fetch,N)`; Symbol port returns only a Vec; dispatcher runs `finalize_probe_window_v1(Vec, top_k)` | Rows below top_k after a cap cannot prove exact total/exhaustion |
| Existing Text count | `LexicalSearchPageV1` carries an optional count; native counted collector and manual scan count before truncation | Preserve equivalent Symbol count semantics and totals strictly after the cursor |
| Response clipping | `fit_ranked_page` rebinds cursor to the last retained row and calls `cut_pageable_window_v2` | Preserve larger exact count or lower bound and walk the omitted suffix |

Observed conflict mapping is source-backed: `CoreError::InvalidContract` maps to
`SearchPlaneErrorCodeV2::InvalidRequest` in `quanta-index-core/src/error.rs`.
The existing `count:0` owner is `filters::plan_filters`, mapped to
`LexFilterInvalidCount` by `planner_errors.rs`.

## Requests to L0 — proposals, not adopted interfaces

### R1. One pure validated request/domain boundary

L0 owns core and `compile.rs`; a dispatcher-local duplicate of adapter validation
would create two authorities. Please provide one canonical constructor callable
by dispatcher and adapter, equivalent to the following proposed signature:

```rust
fn validate_lexical_request(
    query: &LqQuery,
    constraints: &QueryConstraintSetV1,
    endpoint: LexicalEndpoint,
) -> Result<ValidatedLexicalPlan, CoreError>;
```

Required information: endpoint capability, resolved Text/Symbol authority,
projection, and pure invalid/unsupported request outcome. Exact names are for L0
to freeze. Generic Text can resolve Symbol; typed Symbol cannot resolve Text.
Pure checks must precede predicate-result and language-empty shortcuts. Snapshot
capability admission is separate and consumes the actual pinned read handle.

`PreparedExecutableQuery` is declared in lexical `lib.rs`, while its constructor
is in L0-owned `compile.rs`. Please wire the validated plan into that constructor
so route, unsupported-Symbol validation and decoder cannot diverge. Preserve
stored-row integrity failures; do not supply missing Symbol fields or skip them.

L1 consumers: `prepare.rs`, `port.rs`, `manual_scan.rs`, `planner_errors.rs`,
dispatcher `planning.rs` and `routes/lexical.rs`.

### R2. Rows and producer-derived window facts across the Symbol port

Current public core signature requiring replacement:

```rust
fn search_symbols_constrained(
    &self,
    query: &LqQuery,
    constraints: &QueryConstraintSetV1,
    page: &LexicalPageSpec,
    budget: &RequestBudgetV1,
) -> Result<Vec<SymbolCandidate>, CoreError>;
```

Return the agreed canonical page carrier, reusing the existing window model.
Required facts are applied row cap, exact count or justified lower bound after
the cursor, whether the probe completed, and continuation eligibility. A Vec-only
fallback/default cannot prove those facts. If count is supplied, existing native
`collect_ranked_page(..., counts=true, ...)` can obtain the exact total; manual
scan can count its fully matched and boundary-filtered rows before truncation.
No count means bounded probe semantics unless another complete collector proves
more. `count:all` must not widen the row buffer or masquerade as a full count when
the producer did not count.

Direct production consumers are the Symbol dispatcher route and the convenience
`search_symbols` method. Core defaults and test doubles must migrate with the
signature; L1 will not infer facts for a default implementation that lacks them.

### R3. Logical empty provenance

`EmptyProvenanceV2` currently contains only `AvailableEmpty`, `FilteredEmpty`,
and `ZeroHitExecuted`, all documented in terms of an executed universe or known
excluded rows. `ExhaustionProofV1` contains probe, backend-count and full-scan
proofs. None explicitly describes a request constraint contradiction with no
backend invocation. Please define the accepted logical-empty representation and
wire serializer/decoder/SDK changes if required. L1 will set executed=false and
consume that representation instead of inventing a private reason or reporting
zero examined work as an observed backend universe.

### R4. Cross-lane connections and serialized validation

- L2: pinned-handle capability gate over effective pre-result repo/path/language
  scope, including generic Text plans that use Symbol authority. No separate
  coverage registry in L1. Confirm how a contradictory scope and exact-path-only
  query are represented without losing pure validation context.
- L3: retain `page_limit`, counted collector, total-order and cursor boundaries;
  L1 does not edit `paging.rs` or collectors. Coordinate any page facts API.
- L4: provide final prepared-query witness context and typed failure API for
  `port.rs`/`manual_scan.rs`; no duplicate matcher in L1.
- L0: provide G0_READY, the coordinator task ID and a Rust execution slot. No
  heavy Rust build has started from this lane.

## Permanent regression design

Use an independently specified corpus of 5 matching symbols and 5 matching text
chunks, IDs `symbol-00..04` and `chunk-00..04`, plus one unmatched symbol and one
unmatched chunk. Every matching symbol has local name `needle`, identical
qualified/container text and equal line bounds; distinct paths provide explicit
tie order. Ingest paths in reverse order. Cardinality and membership come from
these fixture IDs, never from an unbounded production search used as the oracle.
Repeat on Text-only, Symbol-only and mixed fixtures where relevant.

| Regression group | Inputs | Independent assertion |
| --- | --- | --- |
| Endpoint conflicts | Symbol × `select:file/path/content/content.match/file.owners/repo`, `type:file/path/repo`; explicit `type:symbol select:file`; indexed/manual; case yes/no; hit and absent term | `INVALID_REQUEST` for domain conflicts, never INTERNAL or successful empty |
| Regex bypass | Symbol regex alone, with forbidden select, with explicit type conflict; generic Text explicit-symbol controls | Existing unsupported-Symbol refusal remains for legal Symbol plans; forbidden projection rejects before it can switch domain |
| Force empty | Typed rust + DSL python for the above invalid requests; valid Symbol and Text requests with same contradiction | Invalid request still rejects; valid logical empty has zero backend invocations and executed=false |
| Adapter predicate empty | Absent `repo.has.content`/file predicate plus `count:0`, invalid domain/projection or unsupported Symbol text | Pure invalidity is independent of predicate match cardinality |
| Normal controls | Plain Text, plain Symbol, Text `select:symbol`, Text `type:symbol`, Symbol exact-path-only, empty unconstrained input | Exact fixture IDs for valid cases; existing empty-input refusal remains |
| Count/window | count absent/0/1/3/5/all; top_k 1/3/5/8; corpus cardinality 0/1/3/5/6; indexed/manual | count zero refuses; returned rows respect cap; exact count reflects remaining universe; nonempty omitted suffix yields truthful continuation |
| Walk/order | All count/top_k relations; reverse insertion and multiple pages | Every expected ID exactly once; strict total order within and across pages; cursor identifies final retained row |
| Clipping | Response budget admitting a strict nonempty prefix and budget below one row | Cursor at clipped prefix, full subsequent walk without omissions; oversized first row produces existing typed refusal |
| Binding | Replay after changing query/options/case/route/pin/constraints/order/cap | Existing context/invalid rejection before executor; no weakening from the validated-plan refactor |
| Corruption | Deliberately malformed stored Symbol record as a separate integrity fixture | Storage/internal corruption refusal remains distinguishable from invalid request |
| Facts validation | Missing/contradictory count, rows beyond applied cap, truncated probe mislabeled complete | Canonical carrier refuses false exact/exhausted facts |

Proposed L1-only test placements after G0:

- `crates/quanta-index-lexical/tests/l1_query_domain_window.rs`: real sealed
  adapter and both execution paths; use existing public fixture primitives.
- Dispatcher test module attached from L1-owned `routes/lexical.rs` or
  `planning.rs`: instrument opener/search calls, force-empty, IPC error mapping,
  response clipping and signed cursor binding. Avoid editing the shared test
  support module without L0 coordination.

Planned narrow rails (not executed):

```sh
./scripts/cargow test -p quanta-index-lexical --test l1_query_domain_window
./scripts/cargow test -p quanta-index-search-plane --lib l1_query_domain_window
```

First execute these against pre-fix behavior and preserve the actual behavioral
RED. Compilation failures and zero selected tests are not RED. After one coherent
implementation, run the same rails, relevant existing planner/ranked-page tests,
and the affected dispatcher/public-path gates required by L0's shared contract.
Record source and dependency/config identities before and after execution;
relevant concurrent edits make the result stale.

## All-results port census and limits

The only production consumers found are
`routes/structural/lexical_leaves.rs::LexicalSubexprEvaluator::evaluate`:
`symbol.has.name` calls `search_symbols_all`; other lexical leaves call
`search_all`. Both ports receive the parent query's cloned count options. Both
adapter methods currently apply `page_limit`, despite core docs describing full
recall for structural projection. Source trace establishes reachability, not a
new public behavioral reproduction.

Add an owner fixture with more matches than `count:N`, then a structural caller
fixture whose required match is outside that prefix. Agree with L0 whether the
all-results contract rejects bounded count or ignores only its presentation cap;
do not silently widen a public bounded request. Structural caller ownership is
outside L1. Public structural proof is **NOT_RUN**, and this packet does not label
all other ports as reproduced bugs.

## L1_HANDOFF status

- Changed: this readiness/design document and its static source manifest only.
- VERIFIED: bounded current-source call-chain inventory and existing error-code
  mapping, at the recorded file digests; no runtime claim.
- BLOCKED: G0 contract, shared plan/page/logical-empty types, L2/L4 gate/witness
  APIs and L0 build serialization are not supplied.
- NOT_RUN: behavioral RED/GREEN, production implementation, Rust compile/tests,
  public SDK/daemon proof, repository qualification, release and activation.
- NOT_APPLICABLE: commit/push/reset and extra-agent work are prohibited here.
