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
an empty lexical seed skips the producer, memoizing validated pattern identities
for that request instead of silently returning an empty result. The temporary
compiled engine is released after validation because no candidate consumes it. It
checks later Boolean siblings before returning an empty intersection, without
calling the structural producer or reading a generation.
The producer refuses a ninth distinct `where` engine within one structural
block; the skipped-path evaluator independently limits its retained preflight
set to eight. Repeated identical patterns share one slot. These are local
cardinality guards, not a request-wide physical heap bound.
The subsequent current-main audit found that the per-block limit still let a
Boolean query spread distinct regexes across multiple leaves. Boolean dispatch
now counts distinct `where` patterns across the complete expression tree and
refuses a ninth before universe construction or producer work. The producer's
single-block gate remains for direct callers. This bounds an input/work
dimension across structural leaves; it does not admit compiler temporaries,
cache allocations or aggregate physical heap.
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

## Implementation decision (Sep-29)

**Claim to implement:** a request-scoped ceiling on the sum of live,
regex-owned heap allocation requests, including peak overlap during growth.
The unit is allocated `Layout` bytes, not estimated NFA states, retained
capacity reported after construction, allocator metadata, physical pages or
process RSS. Define an independent process-memory claim if one is needed.
Choose the numerical ceiling only after the allocation inventory and workload
distribution are measured; the existing 64 MiB lexical collection allowance
is a different resource policy and cannot silently serve as this ceiling.

The pinned `regex-automata` meta engine documents approximate `memory_usage()`
and no high-level limit for that aggregate; explicit caches make ownership
visible but do not authorize parser/compiler allocation. RE2's `max_mem` is
also an approximate compiled-program/DFA budget, split among engines, not an
exact whole-request allocation bound. A `GlobalAlloc` or post-allocation RSS
counter cannot provide typed pre-allocation refusal: standard Rust allocation
failure can abort the process. Linux cgroup v2 `memory.max` can contain an
isolated worker's process memory, but it can temporarily overshoot, may kill
the worker, includes non-regex memory and does not supply the portable,
regex-owned typed admission claimed here.

Primary design, subject to the feasibility gate below:

1. Introduce one small allocation-authority layer below `quanta-index-core`
   and `quanta-index-lq-regex`. A request creates a finite, shared ledger in
   `RequestBudgetV1`; every clone shares it. A non-cloneable RAII allocation
   lease reserves checked `Layout` bytes before allocation, keeps the charge
   while the allocation lives, and releases on drop/error/cancellation. Growth
   must reserve the new allocation while the old one is still live, then
   release the old lease after transfer. Never infer physical bytes from the
   existing logical collection/work budget.
2. Use controlled, pinned, fallible allocation hooks in the regex parser and
   engine dependencies for AST/HIR, capture erasure, literal extraction,
   compile temporaries, retained forward/reverse automata, first-search
   scratch and mutable caches. An allocator-aware wrapper around the current
   opaque `regex::bytes::Regex` is insufficient. If moving to
   `regex-automata::meta` with `build_from_hir` and explicit caches reduces
   duplicate parsing or hidden pool allocation, preserve its current bytes
   configuration and prove equivalent truth and whole-match ranges. No engine
   or cache may outlive its request lease or be reused by another request
   without a separately bounded global-cache policy.
3. Give the distinct `tantivy-fst` scope compiler the same fallible authority
   (or replace it with a proven equivalent, allocator-aware FST compiler).
   Include its parse/compile/state allocations and the lifetime of the
   `Arc`-shared path/name automaton. The existing 64 KiB pattern gate and
   per-engine state limits remain useful secondary guards.
4. Thread the authority through primitive admission, indexed and manual
   execution, predicate matching, selected preview, Boolean structural
   preflight, structural universe, and searchd's live structural producer.
   In particular, extend `StructuralProducerPort::execute` to receive the
   request resource context: its current signature has only the query and
   cannot charge the producer's `PreparedStructuralRegexes` or filters to the
   caller. Remove or confine unmetered production constructors so a new call
   site cannot bypass the authority.
5. Map budget exhaustion to the existing typed regex plan-limit family on
   required query paths. Keep syntax/dialect and integrity failures distinct.
   Optional preview reports `WorkBudget` while retaining selected hit IDs,
   scores, order and truthful window metadata. A missing production authority,
   arithmetic overflow or dependency path that cannot be charged fails closed;
   no empty-result or generic producer-error fallback.

**Go/no-go before rollout:** make one vertical slice, using the pinned
dependency sources, for parse → compile → first search → cache growth → drop
under a deliberately tiny finite budget. Inventory every allocation and
demonstrate typed refusal *before* every charged allocation, correct peak
accounting, and release after each failure. Include Tantivy FST in the
inventory. If any required allocation cannot be made fallible without an
unmaintainable dependency fork, stop the in-process hard-cap claim. The
alternative is a request-exclusive worker with an OS-enforced process envelope
on supported platforms, parent-owned timeout/cancellation/IPC/restart and
typed worker-death mapping; describe that as a whole-worker containment
contract, not regex-owned admission or an exact cross-platform RSS cap.

**Proof sequence after implementation:** allocation-site fault injection and
boundary values; concurrent requests and lease lifetime; independent
old/new truth-and-range oracles on capture-heavy, Unicode, zero-width,
case/NFC, repetition, Boolean and 32/33 focus-edge inputs; then actual
SDK/daemon/restart routes. Record regex allocation peak and process RSS as
separate observations. A source-only review or focused compile does not close
this acceptance. No runtime verification was run for this planning update.

Primary references: [pinned regex-automata aggregate-memory contract](https://docs.rs/regex-automata/0.4.14/regex_automata/meta/struct.Regex.html#method.memory_usage),
[RE2 memory policy](https://github.com/google/re2/blob/main/re2/re2.h),
[Rust allocation-failure behavior](https://doc.rust-lang.org/stable/std/alloc/fn.handle_alloc_error.html),
[Linux cgroup v2 memory limit](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html).

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
