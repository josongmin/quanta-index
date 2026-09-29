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
   `StructuralProducerPort::execute` now receives `RequestBudgetV1`; its
   checkpoints only propagate cancellation/deadline, not allocation charges.
   Use that seam for the producer's `PreparedStructuralRegexes` and filters.
   Remove or confine unmetered production constructors so a new call site
   cannot bypass the authority.
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

### Current-source wiring audit

| Owner | Files and necessary change |
| --- | --- |
| Admission root | `crates/quanta-index-core/src/request_budget.rs` owns the shared finite regex allocation ledger; `crates/quanta-index-ipc/src/server.rs` creates the served budget. Keep even `RequestBudgetV1::unbounded()` finite for regex memory. Put the allocation primitive in a lower-level crate so core, the executor and the FST adapter can depend on it without a cycle. |
| Shared engine | `crates/quanta-index-lq-regex/src/executor.rs` and `literal_extract.rs`: make planning, extraction, compilation and first-search/cache growth fallible under that ledger. `verify(&self) -> bool` must become a fallible operation or use fully admitted scratch; a hidden cache allocation cannot be mapped to typed refusal through `bool`. The original HIR remains the literal oracle; an engine built from HIR must use the **capture-erased** execution HIR, not the original capture-bearing HIR. |
| Pinned dependencies | Workspace `Cargo.toml`, `Cargo.lock`, and the relevant crate manifests: a controlled `regex-syntax` / `regex-automata` / `tantivy-fst` allocation path is a feasibility prerequisite, not merely a version bump. The pinned FST constructor allocates parser HIR, instructions, DFA states and temporary `HashMap` / `HashSet` before returning. |
| Lexical routes | `crates/quanta-index-lexical/src/regex.rs`, `query_admission.rs`, `query_errors.rs`, `searcher/{restrictions,match_sets,manual_scan,predicates,prepare,candidates,snippets}.rs`: pass the same owner to preflight, indexed FST filters, manual scans, metadata predicates and optional preview. Replace Boolean `verify` uses (`all`, `is_some_and`, `filter`) with error-propagating loops; never turn resource refusal into a false match. Preflight compilation and execution compilation must both be charged, even if the first is dropped. |
| Structural routes | `crates/quanta-index-search-plane/src/{lowering.rs,query_dispatcher/routes/structural/{lowering,route,universe,eval}.rs}`, `crates/quanta-index-core/src/domains/structural/{service,outbound}.rs`, `crates/quanta-index-lq-structural/src/matcher.rs`, and `crates/quanta-index-searchd/src/app/runtime.rs`: thread the owner through Sourcegraph lowering, skipped-leaf validation, universe filters and the producer port; make the direct authority matcher path take an owner too. Surface search-time refusal as the existing structural regex resource code. |
| Cache boundary | `crates/quanta-index-lexical/src/searcher/match_sets.rs` and `regex_match_cache.rs`: the shared cache holds result bitmaps, not compiled engines. Its current retained-byte estimate is a separate global-cache policy; request allocation cannot be released while its result storage is still owned by the request, or be transferred to that cache without an explicit ownership/accounting handoff. Define whether bitmap construction is inside the claimed regex allocation domain before claiming request-wide bytes. |

Do not make the old unmetered `RegexExecutor::compile/prepare` a production
escape hatch after migration. Use explicit test fixtures for standalone owner
tests. Verification must force refusal during **search**, not only compile, and
assert that no partial match set, narrowed hit list or wrong preview metadata
escapes. The required whole-request ceiling remains `OPEN` until every live
route and dependency allocation in the declared scope meets that contract.

### Dependency feasibility checkpoint

The unmodified pinned public APIs fail the primary design's first gate:

- `regex-syntax 0.8.11` constructs AST nodes with infallible `Box::new` and
  grows parser/translator containers without a caller-supplied allocator.
  The 64 KiB input cap limits the source string, not the Unicode-expanded HIR
  or a typed allocation-failure path.
- `regex-automata 0.4.14` builds an opaque meta engine and an internal cache
  pool; `build_from_hir`, explicit caches and `memory_usage()` do not make
  those constructions fallible under a request ledger. `verify() -> bool` in
  the local wrapper also cannot propagate later search-cache refusal.
- `tantivy-fst 0.5.0` creates parser HIR, NFA instructions, DFA states and
  temporary `HashMap`/`HashSet` storage. Its 1,000-state check runs during
  DFA expansion, after those allocations begin; the adapter cannot impose a
  hard pre-allocation ceiling through `Regex::new`.
- The searchd supervisor's children are threads, not request-exclusive
  processes. A Linux cgroup fallback would need a new process/read-view/IPC
  boundary, not a configuration flag on that supervisor.

No ledger-only patch or post-build measurement closes this. The next code
milestone is a controlled dependency-allocation slice or a separately
specified process-isolation contract. This checkpoint is static source
analysis; no dependency fork, worker, runtime allocation proof or physical
ceiling was implemented by it.

Subsequent code work threads `RequestBudgetV1` through structural Boolean
evaluation, `StructuralService`, and the live searchd producer. It preserves
request cancellation/deadline codes at the domain boundary and checks between
filter compilation, skipped-leaf validation and producer chunk traversal.
These are cooperative interruption checks and a necessary transport seam for
future regex allocation ownership. They do **not** charge regex allocations,
bound dependency internals or satisfy the request-wide physical ceiling.
Owner-local `./scripts/cargow test --locked` structural scopes passed in
`quanta-index-core` (12), `quanta-index-search-plane` (3 eval and 23 route),
and `quanta-index-searchd` (7 producer tests); affected-package all-target
Clippy with `-D warnings` passed. This is functional proof of the transport
and interruption seam, not allocation or daemon proof.

### Remaining implementation order (current-source audit)

The exact live-allocation claim above remains the target. The unmodified pinned
dependency APIs are a **no-go** for that claim; a larger up-front reservation,
`memory_usage()` reading, or the existing preview estimate cannot substitute
for allocation-site admission. A Linux worker envelope is a different,
platform-scoped containment contract and is not silently selected here.

1. Prove a **controlled dependency slice** against the pinned
   `regex-syntax 0.8.11`, `regex-automata 0.4.14` / `regex 1.12.4`, and
   `tantivy-fst 0.5.0` sources. Use exact Git revisions rather than an
   untracked local patch. Exercise parse, capture-erased HIR compilation,
   first search, lazy cache growth, FST construction, failure and drop with a
   tiny shared allowance. The slice must identify every uncharged allocation;
   if it cannot make one fallible, stop before changing public executor APIs.
2. Once the slice passes, put the checked shared ledger/RAII lease below core
   and both regex adapters, then attach one finite owner to every
   `RequestBudgetV1` constructor, including `unbounded()`; the IPC server must
   retain it through response encoding. Count requested `Layout` bytes and
   peak overlap on growth, not logical states or post-build capacity.
3. Replace `RegexExecutor::prepare/compile_prepared/verify/find_ranges_bounded`
   with owner-required, fallible operations. The existing builder reparses the
   capture-erased pattern; an HIR-based replacement must compile that erased
   execution HIR while retaining the original HIR for literal extraction.
   Build a separate owner-aware FST constructor for indexed scope filters;
   charge its parser, instructions, DFA states and temporary maps/sets.
4. Migrate and statically gate all production construction paths: lexical
   admission, indexed scope filters, match sets, manual scan, metadata
   predicates, selected preview; Sourcegraph structural preflight, skipped
   Boolean leaves, pinned universe and live producer. Convert every Boolean
   `verify` use to error propagation. Keep optional-preview refusal separate
   from required-match refusal and preserve hit/window invariants.
5. Run allocation-site fault injection, arithmetic and peak-overlap tests,
   concurrent-request and release tests, fixed truth/range oracles, then the
   affected owner, SDK and daemon/restart rails. A CI guard must reject a new
   production unmetered constructor. Only then close the exact allocation
   item; report process RSS separately.

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
