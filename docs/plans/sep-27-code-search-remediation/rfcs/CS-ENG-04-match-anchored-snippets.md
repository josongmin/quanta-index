# CS-ENG-04 — Aggregate regex allocation admission

Status: `OPEN`; aggregate heap qualification is `BLOCKED` by the missing physical
allocation admission boundary. Completed witnesses, original-byte provenance,
focus rendering and optional refusal are in
[SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md).

## Remaining boundary

`SelectedPreviewContext::prepare_leaf` caps source-pattern bytes, reserves a
16 MiB base before planning and adds 256 bytes per estimated NFA state before
compilation. `RegexExecutor::prepare/compile_prepared` owns canonical AST/HIR,
dialect validation, capture removal and automata compilation. These logical
charges mitigate request work; they do not cover actual aggregate heap.

Pinned regex NFA size limits are per NFA. Forward/reverse retained engines,
compiler temporaries and search caches are distinct allocations. The public
bytes Regex wrapper does not expose one pre-allocation admission/cache owner.
No current runtime aggregate overrun has been established by this ticket.
Sep-28 mitigation caps pattern input at 64 KiB before parsing in the shared
executor and direct Tantivy lexical scope compiler paths, and preserves typed
resource refusal through Sourcegraph structural lowering. This bounds one
input dimension only; it does not implement physical aggregate admission.
`regex-automata 0.4.14` and `tantivy-fst 0.5.0` still allocate parser, compiler,
DFA and cache structures before any request-scoped reservation API can approve
growth. Completing the boundary requires controlled fallible dependency hooks,
one request-lifetime lease across indexed/manual/preview/structural routes and
typed structural/searchd resource mapping. The pinned API cannot prove a hard
ceiling from post-build `memory_usage()`.
The executor now pins the current 10 MiB NFA and 2 MiB lazy-DFA cache defaults
explicitly, preventing a dependency-default change from silently moving those
per-engine limits. This does not impose an aggregate request heap bound.
The structural negative-universe file filter now compiles once per request
instead of once per chunk. Its syntax error remains `StrInvalidRequest`, while
an engine size refusal keeps `LexRegexPlanLimitExceeded`; neither change gives
the structural route physical aggregate admission.
Regex range output now uses fallible vector growth and maps allocation refusal
to optional preview `WorkBudget`; this covers only the range carrier, not the
engine or parser. Indexed scope admission and execution now share a compiler
that bypasses Tantivy's string-valued `RegexQuery::from_pattern` error wrapper.
The pinned external `tantivy-fst =0.5.0` still keeps its error enum private.
On compilation failure only, the same default `regex-syntax` parser and an
exhaustive HIR check identify malformed syntax, byte classes, lazy repetitions
and look assertions. A valid FST grammar leaves only the pinned compiler's
NFA-size and DFA-state refusals, mapped to `LexRegexPlanLimitExceeded` without
matching diagnostic text. Syntax failures remain invalid contracts. Successful
compilation is not repeated; this classification depends on the pinned compiler
refusal set and must be rechecked when its dependency/parser contract changes.
Indexed file filters compile only the requested path/name field; combined
name-and-path filters share one `Arc`-owned FST automaton. The dependency stays
on the pinned external `tantivy-fst` release. Verified-result vector growth
uses fallible reservation with typed `PlanLimitExceeded` /
`regex-verified-results`, including overflow refusal before any prefix escapes.
These changes reduce duplicate compilation and preserve resource errors; they
do not supply parser/compiler allocation hooks.

The lockfile pins `regex 1.12.4`, `regex-automata 0.4.14` and
`regex-syntax 0.8.11`. The pinned meta-engine exposes separate NFA/one-pass/DFA
limits, explicit caches and approximate `memory_usage`; it explicitly has no
high-level limit for the aggregate returned value. Cache reporting is separate.
Switching to that API improves ownership visibility but does not make planner,
compiler temporary and cache allocations fallible before admission.

Sep-29 feasibility and owner update: the pinned `regex-syntax` parser allocates
AST/HIR nodes through ordinary `Box`/`Vec`; `regex-automata` meta construction
allocates strategy and pool state before returning, and cache creation has no
fallible request-admission callback. The current public API therefore cannot
establish the requested hard aggregate bound. The local executor now reuses one
parsed AST for dialect validation, HIR translation and explicit-capture erasure;
this removes redundant parsing but does not change that conclusion. The regex
owner suite, lexical preview owner target and Clippy passed on the updated
source. Dependency allocation hooks plus one cross-route request lease remain
the implementation boundary; a separate worker process would also require
explicit IPC, failure mapping and worker lifecycle design.

Sep-29 structural-route correction: live `where` regexes are compiled once
per request before repo/candidate short circuits and reused across chunks.
The search-plane Boolean evaluator also validates nested `where` regexes when
an empty lexical seed skips the producer, retaining one engine per distinct
pattern for that request instead of silently returning an empty result. It
checks later Boolean siblings before returning an empty intersection, without
calling the structural producer or reading a generation.
The producer refuses a ninth distinct `where` engine within one structural
block; the skipped-path evaluator independently limits its retained preflight
set to eight. Repeated identical patterns share one slot. These are local
cardinality guards, not a request-wide physical heap bound.
The shared executor's pre-parser byte gate now runs before a retained cache
key is allocated. Invalid syntax, resource refusal and engine failure preserve
distinct structural/searchd errors (`StrInvalidRequest`,
`LexRegexPlanLimitExceeded`, `StrProducerExecutionFailed`); resource refusal
no longer becomes `StrShardUnavailable`. This closes that reachable typed
refusal and empty-universe gap. It does not supply aggregate physical
allocation admission for these or other regex routes.

Owners: canonical regex executor/prepared holder, lexical preview preparation,
request resource/lifetime owner and dependency configuration. Preserve one
matcher for truth and ranges and the current query recall/case/NFC semantics.

## Required coordinated change

- First prove an API feasibility slice covering parse → compile → first search
  → retained cache → failure/drop with one request allocation owner. Inventory
  every allocation in the pinned dependencies. If required allocations bypass
  fallible admission, choose a controlled dependency change before promising a
  hard bound; an adapter wrapper or post-allocation measurement cannot supply it.
- Admit AST/HIR planning, compiler temporary and retained automata/capture/cache
  allocations before expensive allocation, with checked capacity/overflow/error
  propagation and reservations that outlive the charged values.
- Keep resource refusal distinct from invalid dialect/pattern and integrity
  errors. Optional preview refusal preserves selected IDs, scores, order and
  truthful window metadata; a required-preview profile refuses explicitly.
- If changing engine API, match existing bytes Regex flags/configuration and
  retain differential truth and every range. Post-allocation memory reporting,
  a larger fixed charge or a global compiled cache is not the requested bound.
- Do not resolve admission by disabling regex previews, narrowing recall or
  silently dropping hits. Aggregate requested allocation and process RSS are
  separately declared claims.
- Apply the owner across every live executor construction path selected by the
  claim, including indexed verification, manual scan and preview; changing only
  the selected-preview wrapper leaves other executor allocations outside it.

The implemented executor maps `CompiledTooBig` to typed `PlanLimitExceeded` /
`regex-compiled-bytes`, and selected-preview compilation turns that resource
refusal into `WorkBudget`. Owner regressions exercise the typed engine refusal
and preview work-budget path; these mitigations do not close aggregate
allocation admission.

## Acceptance

- [ ] Valid capture-heavy, Unicode-class/repetition, malformed, repeated-leaf and
  multiple-distinct-leaf patterns have independent admission/range oracles.
- [ ] Admission precedes expensive allocation. Temporary/retained lifetimes,
  failure release, interruption and cancellation are exercised.
- [ ] Capture removal, zero-width/UTF-8/Boolean witnesses and overlapping 32/33
  focus-edge controls retain canonical truth/ranges and explicit refusal.
- [ ] Optional failure leaves hit identity/rank/window unchanged; wrong source /
  hash/map remains typed integrity failure.
- [ ] Relevant owner and actual SDK/daemon/restart paths execute on the affected
  combined dependencies/configuration; native behavior does not alone prove heap.
- [ ] Any RSS claim uses separately scoped actual-process measurement.

## Regression selection

Accepted matcher, source-provenance, cache and refusal behavior is recorded in
[SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md).
Select the registered regex, lexical preview/manual/collection and structural
owner controls for any executor change. Historical per-test counts and completed
implementation chronology are recoverable through the
[plan archive](../../ARCHIVE-INDEX.md).

Run the owner libraries and `l4_match_anchored_preview` target, followed by the
`l4_preview_sdk` runtime target through `./scripts/cargow test --locked`.
Cache/cancellation/execution-budget and Unicode/regex integration targets are
separate controls; passing them does not satisfy the physical admission items.

Re-run affected routes on the selected source; functional regressions do not
qualify RSS or aggregate allocation.

Context utility, payload cost and p95 overhead are separately owned by
[CS-BENCH-03](CS-BENCH-03-tracks-metrics-and-statistics.md) and
[CS-BENCH-04](CS-BENCH-04-comparators-performance-and-incremental.md).
[CS-INT-01](CS-INT-01-integration-and-qualification.md) owns final integration.
