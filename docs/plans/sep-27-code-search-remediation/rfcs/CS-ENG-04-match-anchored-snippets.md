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

The lockfile pins `regex 1.12.4`, `regex-automata 0.4.14` and
`regex-syntax 0.8.11`. The pinned meta-engine exposes separate NFA/one-pass/DFA
limits, explicit caches and approximate `memory_usage`; it explicitly has no
high-level limit for the aggregate returned value. Cache reporting is separate.
Switching to that API improves ownership visibility but does not make planner,
compiler temporary and cache allocations fallible before admission.

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
allocation admission. Do not retain the superseded blanket `ExecutionInternal`
mapping as a current defect.

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

## Permanent regression controls

- Deployment `max_nfa_states` now gates the validated structural charge before
  literal extraction or engine creation. The default preserves the upstream
  100k admission ceiling; configured tighter limits cannot be bypassed through
  indexed/manual routes or expression/content-filter leaf shapes. This estimator
  is not an upper bound on Unicode byte-automaton states or physical allocation.
- `require_literal=false` admits explicit bounded verify-only plans. A strict
  policy rejects absent literals and any empty alternative, including empty
  patterns, zero-width assertions and optional/empty alternations. Candidate
  materialization remains under the canonical trigram cap and execution budget.
  Dead candidate-cap/threshold/trace scaffolding was removed from the internal
  Rust plan/policy; wire and persisted query shapes are unchanged.
- `engine_size_limit_is_a_typed_resource_failure` exercises the pinned engine's
  actual compiled-byte refusal. Required matching retains a typed resource
  error in both content-search routes and manual scope filtering, distinct from
  syntax or integrity failure. Malformed scope syntax keeps its existing query
  refusal; a resource ceiling is not recast as malformed input.
  Planner literal extraction operates on the prepared HIR without constructing
  an engine. Manual text/symbol scans and dense candidate admission each use
  a request-local, four-entry compiled-pattern cache across documents;
  explanation uses the same matcher with a single-document cache. Failed
  compilations are not cached. This removes repeated compilation for common
  leaves but retains up to four engines and does not provide aggregate heap
  admission.
- The manual `repo.has.*` gate cache collects each canonical predicate/options
  key once per scan, with at most 64 distinct retained sets and a separate
  logical retained-byte account capped by the configured collection-byte limit.
  The set is materialized before the cache reserves its charge; each native
  collector has its own account. These limits stop unbounded cache retention,
  but do not admit transient allocation, cumulative collector work or physical
  request heap. Cache unit tests and
  `unindexed_repo_gates_refuse_excess_distinct_materializations` exercise
  refusal through the public search route.
- Scoped content predicates select their scope-regex admission engine from
  `index:no` versus indexed execution. The manual route accepts the same
  word-boundary grammar in preflight and execution and retains the predicate
  for per-document evaluation instead of running an indexed path/content
  collector. The indexed route retains Tantivy's FST grammar. Dense admission
  and single-candidate explanation also refuse unsupported manual language
  authority before document matching.
- `every_positive_match_in_the_emitted_window_has_a_typed_span` admits exactly
  32 overlapping `aaa` witnesses and refuses the 33rd with empty preview output.
  `indexed_and_manual_preserve_overlapping_raw_witnesses` covers both adapters.
- `l4_unobserved_capture_removal_preserves_reference_ranges` compares truth and
  every range for 18 patterns across 21 sources with the original bytes Regex,
  including named/nested captures, scoped flags, Unicode and empty matches.
- `optional_preview_refusal_preserves_selected_identity_score_and_order`
  exhausts work and resident-memory accounts separately under the same policy.
  Indexed and manual selection retain the same two selected IDs, score bits,
  order and source identities while refusing every excerpt with `WorkBudget`.
- `l4_sdk_preview_survives_daemon_process_restart` checks original ingested bytes
  after checkout overwrite/deletion across two actual daemon starts. The same
  SDK route retains the pinned hit with typed `WorkBudget` refusal for 33
  overlapping witnesses. This harness does not configure an external producer
  checkout or establish aggregate allocation/RSS bounds.

Run the owner libraries and `l4_match_anchored_preview` target, followed by the
`l4_preview_sdk` runtime target through `./scripts/cargow test --locked`.
Cache/cancellation/execution-budget and Unicode/regex integration targets are
separate controls; passing them does not satisfy the physical admission items.

Context utility, payload cost and p95 overhead are separately owned by
[CS-BENCH-03](CS-BENCH-03-tracks-metrics-and-statistics.md) and
[CS-BENCH-04](CS-BENCH-04-comparators-performance-and-incremental.md).
[CS-INT-01](CS-INT-01-integration-and-qualification.md) owns final integration.
