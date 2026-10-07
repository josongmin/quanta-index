# OCT-05-004 — Cost, Capacity and Qualification Boundaries

Status: `Accepted`

Decided: 2026-10-05

Consolidates implemented O4-E4/I0 contracts under
[SEP-26-002](SEP-26-002-retrieval-observation-experiment-and-default-policy.md),
[SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md) and
[SEP-27-004](SEP-27-004-benchmark-capture-and-resource-custody.md).
No conditional optimization or staged operation becomes accepted implementation.
Measurements and missing authority remain in the [residual ledger](../plans/oct-4-parallel-closure/tickets/INDEX.md#e4).

## Decision

1. Preserve exact live-document BM25 statistics and source-bound file/name
   witnesses across full/delta/delete/no-op/reopen and format migration. Existing
   query plans keep default literal-first versus explicit typo and lexical versus
   semantic contribution distinct. Changed policy requires independently judged
   failures/critical strata and unused holdout; diagnostic misses alone do not
   authorize a global model, RRF, chunking or storage replacement.
2. Durable artifact publication retains file sync → rename → parent sync and
   generation/terminal custody. F15 uses bounded immutable objects and durable
   root publication; further barrier changes require isolated sync cost rather
   than a timer enclosing other work. Retain old-or-complete-new roots,
   inherited-file/digest custody and independent fault/crash/reopen tests.
   Syscall faults and process kill do not establish storage power-loss guarantees.
3. Report explicit whole-call and child clock/resource domains. Request-local
   SDK/IPC and parent/worker phases explain attribution, not interchangeable
   latency. Sampled RSS is a sampled maximum; process CPU can include sampling.
   Physical I/O, logical bytes and transient disk need their actual observers.
4. ASCII scanning preserves original byte spans, Unicode fallback, bounds and
   cancellation. `query_timing_overhead.py --scanner-ab` binds separately declared
   source/binaries and checks response/status/cursor/work/config parity while
   permitting declared clock differences. Observation on/off remains a separate
   comparison. Keep/modify/withdraw requires whole-caller acceptance. Persistent
   token authority additionally needs a demonstrated repeated-scan bottleneck,
   exhaustive tokenizer/OSA witness parity and lifecycle/build/residency budgets.
5. Scale/open-loop use typed preflight, complete offered-request accounting,
   independent lifecycle/restart oracles and explicit limits. Default timeout and
   posting-cap refusal stay failures of the requested capacity gate. Diagnostic
   timeout/history overrides cannot qualify defaults; do not raise a limit or
   shrink a fixture solely to turn an observed refusal green.
6. Performance requires the declared response/output boundary, paired/randomized
   schedule, fresh roots, repetition floors and continuous admitted-host inputs.
   Missing frequency/thermal/power/load authority cannot be filled with nominal
   values. Functional, shared-host and causal samples remain scoped diagnostics.
7. One integration owner handles shared schemas/DTOs/registry/CI/dependencies and
   actual source impact. PREPARE → affected source validation → admission ISSUE
   precedes dependent captures. A later source/input change rechecks affected
   proof rather than relabeling old receipts. Normative ADRs are source-closure
   inputs; history/navigation is not a substitute for executable authority.
8. Existing result producer/schema/parser, proof registry and aggregate own
   source/binary/selector/terminal truth. Local owner proof, portable replay,
   hosted CI, Linux release, real provider, paired producer and operational action
   are distinct scopes. Zero-selected, skipped, missing or staged prerequisites
   cannot issue qualification. Product quality/holdout/performance apply only
   when that claim is requested; they do not universally block code qualification.
9. P11's common deploy/activate/restore-forward producer, recipes and typed
   operational result/parser/checker/aggregate are implemented. Concrete target
   adapters, distinct independent pre/post success contracts and authorized
   host/state/retention/rollback inputs remain required under
   [S21-12](../plans/sep-21-search-plane-sota-hardening/tickets/S21-12-cross-repo-terminal-receipt-cutover.md).
   Entries stay staged until that authority is registered and actually executed.
   Shell exit zero or caller-written success JSON cannot promote them.

The paired caller/kernel component uses the existing cross-repository recipe
and CLI-owned immutable completion custody. Its optional typed archive binds
the exact selected tests, source/dependency mapping and daemon bytes; the archive
is `runner-candidate-only`. This component is separate from concrete
operational target/observer acceptance in decision 9. Owner tests do
not establish an executed clean pair or promote an operational registry node.

## Native segment retention and committed live statistics

Sealed manifest format 15 retains the format-14 live-statistics contract and
requires `search-corpus-live-bm25.cbor`. Earlier generations require a producer
rebuild; native statistics are not a fallback
for absent or invalid live statistics. Every native document producer stores a
private census of exact indexed terms and token counts using the registered
analyzer and native term-length rules.

Full builds collect those censuses. Delta builds reuse committed segment
statistics and subtract newly deleted documents' censuses after checking native
component identity and deletion-mask consistency. The selected native scorer
uses live document counts, token totals and corrected document frequencies.
Delta writers with a retained base use `NoMergePolicy`; deletion does not force
compaction of surviving documents solely to recover live BM25 statistics.
Missing or corrupted mandatory sidecars and inconsistent statistics refuse open
or seal. The sidecar has a 16 MiB encoded bound and a conservative 64 MiB retained
decode admission; these are not measured heap or transient-peak guarantees.

Changed retained segments still compare deletion bits across `max_doc`. Native
byte reuse does not establish a whole-call CPU/read/write bound. Source/authority
custody walks, metadata publication and long-running segment/correction fanout
remain separate cost scopes. Current format-15 file authority reuses exact
unchanged committed objects and only rebuilds touched source buckets. Cold open
independently validates the complete source/posting census and materializes
source rows plus bounded term/range/hash directories. Query posting lists are
read on demand under a shared literal/typo request budget. Encoded and logical
resident admissions do not establish measured heap, RSS or transient peaks.
Inherited commitment reuse and new-object replay counters are separate from
physical I/O. Native segment reuse does not establish delta-proportional
whole-call reads/CPU. A future compaction policy requires independent
score/page parity and measured foreground, maintenance and residency budgets.

Owners: [document census](../../crates/quanta-index-lexical/src/doc_census.rs),
[live statistics](../../crates/quanta-index-lexical/src/sealed_generation/live_bm25.rs),
[native writer](../../crates/quanta-index-lexical/src/writer_cache.rs) and
[selected scorer port](../../crates/quanta-index-lexical/src/searcher/port.rs).
Retain fixed untouched-file/component and byte gates, independent fresh-rebuild
score bits and cursor oracles, consecutive delta/delete/no-op/reopen fixtures,
empty-generation cases and mandatory-sidecar corruption/refusal tests.

## Coverage decoding and base custody

One adapter-local cache retains at most one decoded coverage root under an 8 MiB
conservative decode-residency admission. Root-byte identity allows immutable row
reuse only after current root/page hash/length and directory inventory validation;
failure discards authority. Larger roots use the uncached rail. Repeated phases
may decode zero rows while still reading/hashing every page. The estimate is not
physical heap or RSS, and process-local locking does not prevent external mutation.

Coverage CBOR decoding reserves admitted definite cardinality once and grows
indefinite rows geometrically within the existing page limits. Delta planning
borrows replacement/tombstone inputs and discards displaced shared rows without
cloning unused return values. The candidate owns its new rows. These reductions
preserve authenticated base walks, source lineage, retry, publication identity,
old readers and independent open; they do not establish a whole-pipeline bound.
Owners: [coverage reader/cache](../../crates/quanta-index-lexical/src/sealed_generation/coverage.rs)
and [delta planning](../../crates/quanta-index-lexical/src/adapter_ingest.rs).
Retain fixed effective-row, cached corruption/refusal-eviction/retry, uncached
oversize/cardinality and preflight/build mutation controls.

## Explicit typo declaration priority

Explicit OSA1 orders edit distance before source-attested declaration preference,
then occurrence evidence and stable file ties. A declaration boost cannot cross
an edit-distance tier. Unknown symbol coverage retains content matches; default
literal-first eligibility stays independent. Versioned scoring/cursor identity
binds this policy. File recovery does not establish declaration-position recovery.

## Selected score and optional ranking diagnostics

- File Explain resolves immutable file authority and shares the selected scorer
  with search. Additive score components, engine, boost, total and emitted score
  must agree. Optional declaration evidence validates pinned name/definition
  identity; missing coverage is unknown, not zero or a literal exclusion.
- The complete-pool rank study retains native first-page equality before cursor
  continuation, source/generation/profile bindings, all pool pages, ablations and
  explicit exclusions. Experimental declaration/boundary/occurrence policies
  are unselected. Only proven complete eligible pools receive full-rank metrics;
  file features cannot count declaration-span recovery.
- Optional study collection-budget refusal preserves valid selected-score Explain
  with its typed refusal after cancellation/deadline checks. Storage/identity and
  interruption errors propagate. A study and refusal are mutually exclusive;
  no synthetic neutral/zero experimental score fills missing diagnostics.
- Timed queries exclude later optional study calls, while process CPU/RSS can
  include them. That capture cannot issue speed authority. Production enrichment
  batching and ranking selection remain conditional on judged unused holdout and
  complete-call/resource acceptance, not observed Gin or experimental weights.

Owners: [selected scorer/Explain](../../crates/quanta-index-lexical/src/searcher/code_search.rs),
[public Explain](../../crates/quanta-index-search-plane/src/query_dispatcher/routes/explain.rs)
and [rank study](../../tools/benchmark/retrieval/code_search_rank_study.py).
Retain independent score/cursor/source/unit mutations, long NFC/NFD/case-expansion
fixtures, optional-budget versus identity/cancel failures and complete-pool parity.

## Completed response verification

Current cold/warmup/measured observations bind
`query_timing.output_validation=normalized_row_score_bits_sha256_v1` and each
`output_sha256`. Hashing/repetition checks run after the completed-response
clock. Only top-level `timings` is excluded; scores encode fixed-width IEEE-754
f64 bits before canonical JSON. Every task/route phase agrees with its first
normalized response, and replay recomputes from retained rows. Same-size/status
with different candidates/order/scores refuses. Capability probe rejects an
older producer before indexing. Historical readable artifacts without this
marker do not satisfy every-response performance acceptance; independent gold,
host and source/binary qualification remain separate.

## Owners and retained proof

- [Lexical authority](../../crates/quanta-index-lexical/src/file_authority.rs),
  [query execution](../../crates/quanta-index-lexical/src/searcher/code_search.rs),
  [scanner comparison](../../tools/benchmark/retrieval/query_timing_overhead.py).
- [Scale](../../crates/quanta-index-searchd-harness/src/scale.rs),
  [open loop](../../crates/quanta-index-searchd-harness/src/open_loop.rs),
  [benchmark admission](../../tools/benchmark/retrieval/run.py).
- [Source closure](../../tools/ci/source_closure.py),
  [portable proof](../../tools/benchmark/retrieval/portable_proof.py),
  [proof registry](../../tools/ci/proof-authority.toml),
  [result authority](../../tools/ci/proof_execution_result.py).
- Retain independent scanner/config/clock mutations, lifecycle/fresh-rebuild
  parity, bounded-resource refusals, offered-request reconciliation and staged/
  source-mismatch/forged-terminal negatives. Exact historical executions remain
  recoverable through the [plan history index](../ARCHIVE-INDEX.md#historical-record-recovery).

## Bounded publication and interruption

Large source publications use bounded disk-staged upload parts and a small
commit through the existing batch/body/journal identity. Ordinary inline limits
remain intact. Streaming hash and CBOR preflight/decode preserve typed budget
interruption, including the checkpoint after the final EOF read. No cancellation
can be hidden behind an I/O/decode wrapper. The dispatch context uses the same
absolute deadline admitted by RequestBudgetV1 rather than creating another one.

Staging admission checks disk quota, slot count, retry prefix, explicit discard
and idle cleanup before the existing journal/CAS path. Incomplete uploads reserve
no event or generation. Commit still materializes a bounded complete DTO;
arbitrarily large single-publication streaming is outside this contract.

Memory envelope arithmetic uses checked addition/multiplication and typed refusal.
F15 admits term/ID/map/set/posting/materialization residency and native NFC
scratch before allocation. Bounds use actual normalized output and live
Unicode decomposition/recomposition/sort work. The reviewed Unicode fork retains
upstream default/std and only the exact version/features permitted by deny.toml;
workspace default-feature denial remains. These admissions do not measure RSS.

The explicit scale-supported-v1 harness profile and its emitted policy/config
identity own the larger timeout, history/source/vector/process ceilings. Full,
delta, delete, no-op and reopen retain separate causal/retention observations.
Do not call a profile's configured ceiling observed capacity or substitute it
for the ordinary SDK/inline/default request contract.

Owners: [staged transport](../../crates/quanta-index-ipc/src/source_upload.rs),
[SDK publication](../../crates/quanta-index-sdk/src/lexical.rs),
[request budget](../../crates/quanta-index-core/src/request_budget.rs),
[F15 codec/producer](../../crates/quanta-index-lexical/src/file_authority/codec.rs),
[scale profile](../../crates/quanta-index-searchd-harness/src/scale.rs) and
[feature policy](../../deny.toml). Preserve independent EOF/overflow/oversize,
Unicode/native refusal and original lifecycle/crash oracles.

## Publication test boundary

[F15 owner regressions](../../crates/quanta-index-lexical/tests/f15_file_authority.rs)
compare delta/delete/no-op/cold-open results to an independent full build and
refuse corrupted, missing or symlinked committed objects. These tests do not
inject I/O failure or kill a child at each inner publication syscall.
`file_authority.rs` syncs immutable objects/directories, publishes the durable
root through `index_store::write_atomic_durable`, then retires unused objects
and staging. The root writer uses write → file sync → rename → parent sync.
The broader daemon matrix covers declared track seal/GC points; it does not
substitute for cuts inside this sequence. Implement and execute those selected
old/complete-new and barrier-error oracles under
[E4-02](../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-e4-02), without claiming
a known serving defect or storage power-loss qualification from source review.

## Proof execution and CI

Causal capture binds one raw binary identity/epoch before and after execution,
replay and successful publication. Portable proof rechecks every role's admitted
executable epoch before completion. Mutation refuses and cannot leave a passing
profile; relocated replay preserves original receipt provenance.

CircleCI divides strict/MSRV, nextest/receipt, docs and whole-workspace bench
compilation into separate jobs. The required verify node depends on all Rust
workers; regular Python has its own terminal status. Bench keeps workspace,
all-features, locked, bench-profile/no-run scope and uses its declared four-CPU
resource allocation. Successful compilation is not benchmark execution.
Dependency caches exclude Cargo target output. Generated vendor lock/EOF
exceptions are exact paths; tracked lock/manifests and unexpected changes still
refuse receipt publication. Actual new-source job results remain I0/QIT acceptance.

Owners: [causal capture](../../tools/benchmark/retrieval/causal_cost_capture.py),
[portable proof](../../tools/benchmark/retrieval/portable_proof.py),
[CI graph](../../.circleci/config.yml) and
[receipt writer](../../tools/ci/write-verification-receipt.py).

## Operational actions

The existing canonical producer/result grammar executes separate tracked
pre/action/post actors, rejects inadmissible pre-state before mutation and binds
actual Linux host, source pair, binary, config, state format and immutable
prerequisites. Source archives cannot conceal dirty actor/contract bytes.
Distinct actor paths/bytes are a custody check; concrete domain observers still
need independent oracles. Deployment, activation and restore-forward validate
separate observed transitions with target/config continuity across dependencies.
Staged entries refuse before action or evidence issuance. Owner fixtures do not
qualify an actual operation or invent an authorized target.

Owners: [producer](../../tools/ci/operational_proof.py),
[result grammar](../../tools/ci/proof_operational_result.py),
[registry](../../tools/ci/proof-authority.toml) and
[operator usage](../operator/p11-operational-proof.md).

## Shared-source validation

O4-I0-01 coordination/control-plane implementation is complete and its ticket
is retired. Source/dirty/owner, exact consumed inputs and affected regression
validation remain standing rules. PREPARE → affected VALIDATE → admission ISSUE
is scoped per repository/claim; no label/provider/holdout/Linux global barrier
blocks an independent ready cell. Completed selected SDK/Contract/runtime/CI
results retain their original source, while I0-02 owns later-source acceptance.

## Consequences

Completed implementation has an ADR owner; unmeasured optimizations and release
requirements keep their active owner. Neither documentation consolidation nor
past test totals issue new benchmark, code, release or operational qualification.
