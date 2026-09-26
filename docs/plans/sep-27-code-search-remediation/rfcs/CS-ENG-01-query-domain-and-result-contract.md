# CS-ENG-01 — Query domain and result contract

Status: **IMPLEMENTED** for the repaired L1 query/domain/window boundary.
Owner regressions and changed real-daemon SDK paths: **VERIFIED** in
[L1 adversarial RCA evidence](../handoffs/L1_ADVERSARIAL_AUDIT.md).
Repository qualification and the remaining preventive matrix are not implied.
Category: engine correctness. Findings: F01; preventive coverage for related plans.
Baseline and reproduced requests: [evidence](../evidence.md).

Final engine audit: [engine-audit.md](../engine-audit.md), E01/E02. The new probes
execute pinned binaries, with current-source tracing; they are not a fresh build
of the concurrently dirty checkout.

## Purpose and RCA

Make a legal request select a compatible execution domain, projection and
decoder, or reject it before execution. Six symbol-endpoint requests using
`select:file`, with and without `case:yes`, reached the Text index and then the
Symbol decoder. They returned `INTERNAL: ... missing symbol_kind field`.

The symbol endpoint supplies Symbol as a default, not an immutable endpoint
constraint. `prepare.rs` can replace that default with Text for `select:file`;
`port.rs` still constructs `SymbolCandidate`. The decoder is detecting a real
invariant violation. Adding a default `symbol_kind`, filtering broken rows, or
returning an empty list would hide the planner defect.

The audit also reproduced two related failures:

- A matching `select:path`, `select:content` or `type:file` can reach the same
  incompatible decoder. A no-match `select:file` succeeds empty instead; adding
  it to an otherwise unsupported symbol regex also bypasses that refusal.
- Dispatcher language-constraint `force_empty` returns before endpoint-domain
  validation, even for explicit `type:symbol select:file` conflict. Validate legal
  plans before all empty shortcuts, not only inside the adapter.

Current owners:

- [Typed requests](../../../../crates/quanta-index-contract/src/query/requests.rs).
- [Preparation](../../../../crates/quanta-index-lexical/src/searcher/prepare.rs),
  [compilation](../../../../crates/quanta-index-lexical/src/searcher/compile.rs).
- [Endpoint and decoder dispatch](../../../../crates/quanta-index-lexical/src/searcher/port.rs),
  [candidate decoding](../../../../crates/quanta-index-lexical/src/searcher/candidates.rs).
- [Manual scan](../../../../crates/quanta-index-lexical/src/searcher/manual_scan.rs),
  [paging](../../../../crates/quanta-index-lexical/src/searcher/paging.rs),
  [existing symbol tests](../../../../crates/quanta-index-lexical/src/symbol.rs).
- [Public route](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs)
  and [window derivation](../../../../crates/quanta-index-search-plane/src/query_dispatcher/window.rs).

## Decision

Compile one validated plan before both indexed and manual execution. It binds:

| Dimension | Required meaning |
| --- | --- |
| Endpoint capability | Admitted input and output types; not merely a default |
| Search domain | Text, Symbol or an explicitly supported other domain |
| Match policy | Native/literal/natural-language policy plus case and filters |
| Projection | Hit, distinct file or other supported output unit |
| Snapshot | Repository, revision/generation and scope identity |
| Completeness | Exhaustive, bounded or explicitly partial capability |
| Ranking and budgets | Versioned policy, top-k, scan/time/context limits |

Represent legal combinations with a validated enum or equivalent constructor
boundary. Execution and decoder dispatch consume that same value. Do not allow
an unvalidated domain override after compilation.

For the current typed Symbol endpoint, reject `select:file` with an existing
appropriate typed planner-conflict error, before search. The observed wire error
for the already rejected explicit conflict is `INVALID_REQUEST`; do not claim a
different enum reaches the wire without checking the mapping. This RFC does not add a
symbol-to-file response API. If later required, that API needs an explicit result
type and its own contract. A generic Text entry point that already supports
explicit symbol selection must keep its documented capability; making every
default immutable would be a different regression.

Preserve existing unsupported symbol-filter refusals from
[SEP-26-001](../../../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md).
No implicit text fallback and no guessed symbol identity. Bind cursor validation
to domain, projection, match/ranking policy and snapshot, not just query text.
Current cursors already bind canonical query/options, route, pin, constraints,
order and cap. Preserve those checks; add resolved-plan/policy identity only where
the new contract introduces a distinction not covered by those existing inputs.

## Bounded results must carry exhaustion evidence

The pinned native probe returns three symbols for
`Default OR generate_unique_id`. With `count:1`, indexed and manual symbol paths
return one and claim `exact_exhausted`, exact count 1, no continuation. Lexical
control preserves the larger count/lower-bound state. This is a result-truth bug,
not a request to improve ranking.

The symbol port returns only `Vec<SymbolCandidate>` after applying
`min(page.fetch, count_limit)`. The dispatcher assumes it received an untruncated
`top_k + 1` probe and infers exhaustion from vector length. That inference is invalid.

Change the port to carry rows plus producer-derived window facts through the
existing typed result-window model: applied cap, complete/probe-limited evidence,
known total or lower bound and continuation eligibility. Do not add parallel
versioned internal IRs or infer totals from truncated rows. Reuse lexical window
semantics where equivalent. A bounded count is not exhaustive no-answer/total;
`count:all` without a true count collector must remain explicitly probe-derived.
Response byte clipping must retain or regenerate truthful continuation metadata.

The empty fast path must not claim backend execution in lane observations when
no read view/search was acquired. Keep execution metadata and logical empty-result
proof separate. Existing valid route/case cursor mismatch checks are preserved.

## Implementation boundary

1. Inventory every public endpoint, SDK caller, native/Sourcegraph parser path,
   explicit type/select combination and response decoder.
2. Validate the complete request once, including conflicting implicit endpoint
   constraints. Preserve a typed error code at the SDK boundary.
3. Use the validated plan for indexed, manual, count/all and paginated paths.
4. Keep stored-row validation as corruption defense. A legal request must not
   depend on a missing-field exception to reject an illegal projection.
5. Update contract consumers together if the plan/result types change. New public
   fields require the coordinated cutover in CS-INT-01, not private adapter flags.

This is request correctness, not a ranking change. ENG-03 owns new relevance and
grouping policies; BENCH-02 owns validating recorded native output.

## Verification and failure semantics

- Retain all six observed requests as minimal independent regressions. Assert
  typed pre-execution refusal, no query executor/index scan and no INTERNAL.
  Pure request conflicts reject before snapshot reads; valid requests may need
  a pinned read view to establish capability. Do not ban those authority reads.
- Matrix: endpoint × explicit/implicit type × legal/illegal select × case mode ×
  native dialect × indexed/manual path. Include file/path/repository/content and
  other currently exposed projections after the endpoint inventory.
- Run on Text-only, Symbol-only and mixed snapshots. Include zero Text matches:
  a conflict must not become successful emptiness just because decoding is skipped.
- Exercise forced-empty/planner-short-circuit paths as well: they must validate
  endpoint constraints before returning an otherwise legitimate empty result.
- For a valid forced-empty request, assert `lane.executed=false`, zero backend
  invocation and an explicit logical empty-result reason. Empty validity and
  execution telemetry are independent invariants.
- Test ordinary Symbol requests, generic explicit-symbol selection, count/all,
  cursor continuation and mismatched cursor replay.
- Test `count:N` below/equal/above top-k with fewer/equal/more than N actual rows,
  indexed/manual modes, exact and bounded windows, response clipping and subsequent
  pages. Assert identities and no omissions, not only result-vector lengths.
- Exercise count zero/all, exact-path-only requests and the all-results ports.
  Public structural impact of bounded all-results ports remains NOT_RUN; do not
  infer it from the native page-query counterexample.
- Inject a malformed stored Symbol row separately: corruption remains a typed
  storage failure, distinguishable from invalid caller input.
- Property tests assert every constructible plan has one compatible decoder;
  invalid combinations cannot acquire a query executor.

The exact executed matrix and positive controls are in the final audit. Other
matrix cells remain preventive coverage, not alleged failures.

## DoD

- [ ] Endpoint/projection capability matrix is checked into contract tests.
- [ ] Six observed cases reject deterministically before execution.
- [ ] All existing valid domain/projection paths retain their result semantics.
- [ ] Indexed/manual/count/page paths share validation and decoder selection.
- [ ] SDK error mapping, malformed-row controls and cursor mismatch tests pass.
- [ ] No count/probe/response cap can manufacture exact exhaustion or suppress a
  valid continuation; no force-empty shortcut bypasses request validity.
- [ ] Valid force-empty responses preserve logical proof without claiming backend
  execution; route observations match instrumented invocation counts.
- [ ] Source-bound real-daemon SDK receipt covers the changed public path.
- [ ] Affected ADR/API documentation reflects the accepted behavior.

Current proof coverage: native L1 domain/window matrix, dispatcher routes and
actual daemon/SDK projection errors, count continuation, cursor mismatch,
primitive refusal and valid logical emptiness are recorded in the linked audit.
The SDK process test is explicitly opt-in and executed with `--ignored`; its final
run executed one test with zero ignored. This follow-up does not claim the entire
preventive stored-row corruption/property matrix, so the original aggregate DoD
checklist is not promoted wholesale. Accepted API behavior is documented in
[SEP-26-001](../../../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md).

## Dependencies and alternatives

No ranking or corpus expansion prerequisite. Can run in parallel with BENCH-01
and BENCH-02. Integrate before ENG-02/03/04 shared contract changes.

Reject endpoint-specific post-hoc row filtering: it leaks an invalid plan into
execution. Reject universal Symbol routing for bare identifiers: content search
and definition lookup have different user intent. Explicit domains follow the
production precedent in [S01/S04](../references.md), not a claim of identical APIs.
