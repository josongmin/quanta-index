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
   [S21-12](OCT-05-004-cost-capacity-and-qualification-boundaries.md#installed-paired-and-operational-acceptance).
   Entries stay staged until that authority is registered and actually executed.
   Shell exit zero or caller-written success JSON cannot promote them.

The paired caller/kernel component uses the existing cross-repository recipe
and CLI-owned immutable completion custody. Its typed archive binds
the exact selected tests, source/dependency mapping and daemon bytes; the archive
is `runner-candidate-only`. This component is separate from concrete
operational target/observer acceptance in decision 9. Owner tests do
not establish an executed clean pair or promote an operational registry node.

The paired recipe requires a registered `QUANTA_P11_R5_QBC_LANE` for both
resolver preflight and selected Nextest execution. QBC auxiliary metadata does
not forward Cargo JSON to terminal stdout and does not issue a test receipt.
The resolver consumes its own invocation's bounded regular output only after
checking a fresh run ID, exact command/cwd/lane, nonce, effective exit zero,
stable owner status and file bytes/identity, and clean source/manifest/lock
bindings before and after. Missing auxiliary raw-exit detail cannot qualify a
test; an explicitly nonzero raw exit or effective exit125 is refused.
Selected list/run always uses the existing immutable completion archive,
including the default recipe path. Metadata cannot substitute for that proof.
See [paired usage](../operator/p11-operational-proof.md#paired-caller-and-kernel).

The shell resolves every Python helper through the producer's canonical
`scripts/lib/python-env.sh` interpreter resolver. Before the fresh Rust build,
the file guard checks paired-root-relative manifest/lock bindings and imports
the three QBC owner modules from the exact frozen producer directory. The
first clean-pair attempt built successfully but refused a doubly anchored lock
path; the second built successfully but refused host Python3.9's missing
`tomllib`. Neither reached Nextest list/run. Their original logs/daemon bytes
remain separately pinned under `/private/tmp/qna7vj2zq6/`. The corrected actual
Python3.12.12 import/file preflight and133 focused regressions passed; this does
not substitute for executed caller/kernel tests.

The third clean pair, Quanta2df/fixed77bc, completed a5m28s fresh daemon
build but QBC refused caller/list's manual raw feature composition before
test compilation. Actual assertions stayed `NOT_RUN`; the original failure
and daemon are pinned in `/private/tmp/qna7vj2zq6/r5-2df-failure.json`.
The fixed paired owner recipe now admits only the exact two caller/kernel
list/run shapes through QBC's typed command-model admission and canonical
lane-command port. Unregistered cases, phase/feature/target/selector drift
and a foreign admission module refuse before execution. QBC still owns lane
locking, source/environment checks and immutable completion publication;
Quanta consumes the same locator/receipt and before/after daemon guards.
Four actual frozen-QBC admission model preflights and137 focused tests passed.
This adapter correction is separate from executed Nextest and operational proof.

The fourth actual pair, Quanta86b/fixed77bc, completed its fresh daemon build
in3m17s and reached caller/list's completion stage, but completion
publication refused the omitted expected-source digest in the explicit completion
environment. Both checkouts remained clean at their original HEADs; assertions
and the kernel phase stayed `NOT_RUN`. Original raw and fresh daemon are pinned
in `/private/tmp/qna7vj2zq6/r5-86b-failure.json`. The correction forwards the same
canonical source digest to both compile admission and completion publication;
the producer's source/HEAD equality guard remains intact. The accompanying
external-helper ShellCheck annotation fixes hosted pre-commit SC1091. At2df,
Python1594 executed4,388 passing tests/33 skipped, then failed that hook; the
Python context remains `FAILED` despite its passing test step.

The fifth actual pair, Quantae227/fixed77bc, completed a2m45s fresh daemon
build and correctly published caller/list's immutable QBC completion. Cargo
returned101: the frozen producer's `quanta-adapters-parser` emitted56 compiler
diagnostics, including inaccessible/missing retained projection types/functions
and a missing `first_refusal_v1` trait implementation. Caller assertions remain
`NOT_RUN`; the full R5 pair is `FAILED`. Failed locator/receipt/raw and the frozen
daemon are preserved in `/private/tmp/qna7vj2zq6/r5-e227-failure.json`.

The independent kernel exact list/run completed at the same clean pair:
selected1/executed1/passed1,failed0. Original inventories/events, immutable QBC
receipts, fresh daemon equality and before/after Cargo resolution were verified
under `/private/tmp/qna7vj2zq6/r5-kernel-e227/`. This is only the selected
`repomap_v2_receipts_require_exact_full_bundle_and_transition_v2` kernel test;
it neither repairs the caller compilation nor qualifies installed operations.

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

## Per-seal ingest observation

The Scale caller now retains the `SearchCorpusPublishOutcome` returned by each
timed seal instead of discarding its ingest observation. The transient carrier
binds the actual request ID, requested source publication, accepted receipt and
owner clocks from that invocation. Staged publication binds the final commit
request ID rather than an upload-part acknowledgement. Existing receipt
validation, generation advance and source-event acceptance remain in place.

`summary.json` carries `ingest_observations.by_seal` for full, delta and no-op,
plus delete in scoped tiers. The writer rejects absent/replayed observations,
identity or event-lineage mismatch, nonsequential generations, stale request
IDs, missing clocks, overflow, child clocks outside their parents and a clock
sum exceeding its containing operation. It adds only semantic total, lexical
total and finalize; nested stages overlap those parents. Activation remains a
separate control request. Small full wall time includes ingest and seal;
scoped full wall time covers seal; delta/delete include staging. The wall clock
stops before the additional observation projection and validation.

`unattributed_ns` is the remaining wall time. It can include preflight,
validation, transport, locks and harness work; it does not identify a single
cause or physical I/O. This addition makes the existing XL seal bottleneck
measurable without skipping validation, changing admission bounds or issuing a
performance improvement claim. `TierMeasurement` gains a crate-visible field;
external struct-literal construction is consequently restricted. Existing
repository callers obtain measurements through the producer functions.

Owners: [harness publication](../../crates/quanta-index-searchd-harness/src/harness.rs)
and [Scale producer](../../crates/quanta-index-searchd-harness/src/scale.rs).
Owner execution and matching-release stage attribution remain separate checks
under [E4-01](../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-e4-01).

With `QUANTA_INDEX_CAUSAL_PROFILE_V1=1`, `QI_INGEST_TRACE_V1` stderr spans
also expose pre-intent and paired-base validation, lexical generation/file/
text/segment/overlay proofs, semantic row commitment and source finalization.
Each wrapper retains the original operation's result and validation. Spans
overlap and can recur within one publication; their durations cannot be summed
as exclusive cost. Bind the complete stderr to the enclosing Scale phase,
source and executable. An XL trace on the matching candidate remains a
separate diagnostic, not a speed or incremental-update qualification.

Focused owner execution passed 44/44 with
`./scripts/cargow --lane test-scale-f15-lane test -p quanta-index-searchd-harness
--lib scale:: --all-features --locked -- --test-threads=1`. This includes the
real daemon's full/delta/no-op/delete observations and independent identity,
clock, lineage and corruption predicates. It does not qualify Large/XL cost.
Strict Clippy also passed for lexical, semantic, search-plane and harness
libraries/tests with all features and `-D warnings`. The first executed checks
found type/ownership and lint errors; those were repaired before these passes.

### XL repeated validation trace

The clean `82bca2f25a94e8c33bcf73f72bf38c7ed6315efd` source completed one
instrumented XL execution with the original 32,768 files, 64 repositories,
113,023,546 source bytes, seed5864059738136528177 and unchanged admission.
The release executable was built with Rust1.92 and `-C lto=off`; SHA-256 is
`a428f424ddc426f795993e236868ece95bb5c7e6c7c29d0a9e48e534f18700b3`.
The matching source and binary before/after, successful phase endings and
stdout/stderr/profile digests are recorded in
`/private/tmp/qi-delta-trace-5pgjzkan/xlarge/execution.json`.
The execution exited0 in492.750s as `VERIFIED_DIAGNOSTIC`.

A single 3,437-byte file replacement took90.610748458s through ingest/seal.
The nonoverlapping partition was75.644136585s for six whole lexical generation
proofs,7.093390917s for the remaining lexical build,0.687121541s for semantic
build,1.274530542s for finalization and5.911568873s for other work. The proof
calls comprise base and coverage checks before intent, the same two checks
under the publication lock, build preparation coverage, and the new generation
proof after build. File-authority reconstruction consumed61.645064999s
*inside* those six proofs; it must not be added to that partition. No-op seal
took91.640092666s, including74.690540167s of the same six proofs. Delta
activation was a separate13.184379875s control operation.

The per-seal carrier observed21.614164333s inside delta ingest, leaving
68.996584125s unattributed by that carrier. The trace resolves most of that
remainder to preflight and repeated validation; the two observation domains
overlap. These are shared macOS diagnostics. The original5bf93.969s result
had no such trace and used a different source/build configuration. Neither
speed improvement nor current-main performance is established by this run.

The packed-source planner additionally performed a linear inherited-root
search for each unchanged source. It now binary-searches the strictly ordered
`SourceFileKey` rows while retaining complete revision/digest equality and
the existing byte-size, storage and symlink refusal paths. Owner execution
with `./scripts/cargow --lane test-scale-f15-lane` passed the four `packed_plan`
library tests,33 `l2_file_mutation` integration tests and lexical library/test
Clippy with all features and `-D warnings`. The lookup repair alone does not
remove the75.644s whole-proof bottleneck; the coupled repair is measured below.

The structural repair now carries a non-cloneable, non-serialized
`PublicationValidationOwner` through one publication. Adapter-private typed
proofs retain only identity, root/manifest and policy commitments after a full
independent source/normalization/posting walk. Every existing refusal boundary
still authenticates current committed objects, lengths, digests, paths and
inventory. Query and scrub doors retain independent full validation; no source
bodies, postings, query handles or filesystem-metadata success cache are retained.
Exact payload identity prevents owner reuse across different publications.

The complete coupled core/lexical/search-plane/runtime bundle passed1,035 library
tests (one ignored),36 `l2_file_mutation` tests and
`just rust-test-e2e test-scale-f15-lane` (300 passed,10 skipped), plus strict
Clippy, format, hexagonal boundaries and module snapshots in its isolated
worktree. Main integration preserved exact tested bytes, including the packed
planner repair. Regressions reject same-inode/size/restored-mtime tampering,
missing/symlink/orphan objects, lineage/owner mismatches and changed sealed
manifests; retry, rebuild/query parity and process restart remain covered.
The clean main `e227757ab57e680801483e4d443853bfb768d8f2` completed the
matching post-repair XL execution and independent canonical profile replay.
`./scripts/cargow --lane test-scale-f15-lane rustc -p quanta-index-searchd-harness
--bin scale_matrix --all-features --release --locked -- -C lto=off` built the
Rust1.92 executable in12m35s. SHA-256 is
`c821b71467bb396cbf687651f4977808887b8b7df2b30825c92253c19c6c5630`.
The actual capture exited0 in352.747s. Source, binary, all phase endings and
independent replay are bound under `/private/tmp/qi-publication-proof-xl-e227757a/`.
The replay equals the canonical capture profile after excluding only its added
`scope.binary_source_binding` capture metadata.

Original32,768files/64repos/113,023,546bytes, seed5864059738136528177,
tier-manifest bytes, corpus digest and runtime configuration match the82bc trace.
Process ceiling4GiB, history1GiB per pair/2GiB total and registry512MiB admission
were not raised. Full/delta/no-op/delete/query/reopen passed. The3,437-byte delta
seal took42.517229542s; its exclusive partition is32.596106375s for six proof
envelopes,5.017497083s for the remaining lexical build,0.712974417s for semantic
build,1.231631167s for finalization and2.959020500s for other work. Two independent
cold walks remain12.585253833s and12.393015750s; four repeated authentications are
1.757–2.298s. Nested file authority23.880954126s overlaps this partition.
No-op seal45.034310666s and delete seal43.148191166s include six proof envelopes
of33.357206292s and31.809186999s respectively; delta activation13.215161500s
remains a separate operation. These are single profiled samples on shared macOS.
They establish executed diagnostic scope, not qualified speed or a guarantee
that all validation cost is removed. No broader source-body or mtime cache was
introduced; cold custody and new-generation validation remain independent.

## Paged term directory

Main `b262925b` replaces the retained per-term offset/count/hash directory with
one fence and authenticated table-page identity per 128 terms. Queries verify
the selected table page and posting-list range/hash; producer, root admission
and cold verifier use the same page-based charge. The 32 MiB directory ceiling
remains unchanged. Posting wire v2 and the changed policy identity reject old
roots as rebuild-required. Encoded/resident admission is not measured RSS.

The original Ready9 fixed-directory failures accumulated roughly 520,000 term
rows across source-key buckets. There was no observed defect in the old
64-byte-per-term plus 128-byte-per-block charge; the retained structure itself
exceeded its ceiling. This repair changes that structure rather than raising
the ceiling or exposing a new startup override.

Owner verification completed: `./scripts/cargow --lane test-fast-lane nextest run
-p quanta-index-lexical --lib --test f15_file_authority --all-features --locked
-E 'test(file_authority::) | binary(f15_file_authority)'` passed 41/41;
`./scripts/cargow --lane clippy-lane clippy -p quanta-index-lexical --all-targets
--all-features --locked -- -D warnings` passed. Fixedb262 daemon verification
`just rust-test-e2e test-fast-lane` passed 214 across four binaries, one skipped
(Nextest `3d7dac7a-73f7-4c10-bc18-a4d421dfb7ac`). Debug runner/searchd build and
the 1,529-file source closure passed; the copied binaries are pinned in
`ready9-native-paged-20261007-01a10d0b/binary-pins.json` under task evidence.
Actual CLI/Django/Nushell follow-ups are retained in the
[capture ADR](OCT-05-002-native-capture-clock-and-index-scope.md#completed-ready9-capture-scope).
Remaining repository capture, independent judgments and release qualification
retain their own acceptance; owner regressions do not substitute for them.

## Paged text authority and query deadlines

Main `5c2b6843` retains compact document/source identities and authenticated
immutable shard descriptors instead of every decoded text posting. Cold open
still verifies every shard's length, digest, decoded rows and extrema. Raw/phrase
queries decode and verify one shard at a time under the existing request budget;
native candidate checks use compact indexed-source identities. Regex preserves
its aggregate candidate admission across shards. Fixture shapes, formats and
declared source/history/query ceilings are unchanged.

Each nonempty shard pins its opened file. Positioned reads preserve the admitted
inode across path replacement, unlink, generation retirement and concurrent
queries; an open directory alone would not preserve a collected shard. Request
scratch is excluded from snapshot resident accounting. One decoded shard and
regex candidate strings can still occupy request memory, and sparse generations
can retain many file descriptors. There is no declared query-byte or descriptor
service envelope: the fixed Native scale fixture does not qualify those other
input distributions. No memory/RLIMIT limit was raised to obtain this repair.

The harness preserves one absolute query deadline across readiness retries and
I/O. The readiness window limits new retries, while an admitted request retains
its original response deadline. The former 15-second readiness window must not
truncate a declared 600-second scale query. Bounded socket fixtures independently
check late valid responses, terminal read timeout and non-refreshed deadlines.

Owners: [text reader](../../crates/quanta-index-lexical/src/text_authority/reader.rs),
[candidate integrity](../../crates/quanta-index-lexical/src/searcher/candidates.rs),
[snapshot accounting](../../crates/quanta-index-lexical/src/adapter_open.rs) and
[harness deadline](../../crates/quanta-index-searchd-harness/src/harness.rs).
Retain fixed raw/phrase/regex result sets, aggregate candidate refusals, pinned
old-reader GC/unlink/concurrency controls and corruption/cancellation outcomes.

The first paged XL diagnostic at `5c2b6843` still exceeded the snapshot registry
admission. Its actual sealed g1 source rows retained raw, NFC and folded bodies
separately. File authority (364,146,926 bytes), unique native mappings
(124,864,778 bytes) and the coverage structural lower bound (49,348,608 bytes)
alone sum to 538,360,312 bytes, above the unchanged 536,870,912-byte registry
ceiling, before other snapshot components. This is a source/immutable-input
lower bound, not a captured runtime cache gauge or physical RSS measurement.
The fixed `scale_needle_token` query lowers to a native Standard keyword and
does not traverse text-authority shards. Repeated uncached snapshot opening is
therefore a distinct remaining cause; text paging alone does not close XL.

After full ingest/seal/activation, the owner interrupted that diagnostic with
SIGTERM to repair retention. Its execution is `FAILED` (exit -15, 1,149.223s,
not a timeout), not completed capacity evidence. Immutable RCA inputs and the
three original release executables remain preserved under
`/Users/songmin/.codex/task-evidence/scale-paged-final-20261007-5c2b6843-mwatpufl`.
Main `5bf6b152` now aliases byte-identical verified raw/NFC/folded surfaces with
`Arc` backing allocations and charges each distinct retained allocation. Binary
sources retain byte buffers and acquire no text view. Sealed logical row charges,
formats and policy validation remain unchanged. Fixed cold-open tests cover
ASCII, NFD, lower-case NFD, mixed case and invalid UTF-8, including pointer
identity and independent normalized byte expectations. Matching-release Large
and XL execution, independent causal replay, XL offered-load and selected
release-daemon OS restart/delete completed at this source. The original XL
corpus and all declared ceilings were retained; no smaller fixture or cap
increase was used. These are shared-macOS diagnostic and functional results,
with qualified-host performance remaining under E4-06.

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
substitute for cuts inside this sequence. Main `8599f2e8` adds the independent
[publication oracle](../../crates/quanta-index-lexical/src/publication_faults/tests.rs)
and test-only before/after instrumentation: 32 injected I/O cases and 32 actual
child SIGKILL cases. Fixed source bytes and independently decoded root, source
packs and postings determine the expected result; inherited inode custody,
unsealed refusal, recovery and forged-source negatives remain explicit.

The regression exposed a replay durability defect: absence of staging after an
interrupted unlink did not prove its parent directory had been synchronized.
Replay now reissues that barrier and retires nested atomic-root temporaries
before sealing. Already sealed generations still refuse extra temporaries.
Follow-up `b09c4aa7` recovered-query parity, `70521514` interrupted clone retry
and the 36 I/O + 36 SIGKILL matrix are implemented and their focused owner
checks passed. The fixed705 daemon scope passed 214 tests; `eb97e7c2` Medium
restart/delete and release-binary custody passed 2. Their completion is retained
below; [E4](../plans/oct-4-parallel-closure/tickets/INDEX.md#e4) directs
new hosted and Large/XL qualification to their owners. Process-kill testing
does not qualify power loss.

## Proof execution and CI

Causal capture binds one raw binary identity/epoch before and after execution,
replay and successful publication. Portable proof rechecks every role's admitted
executable epoch before completion. Mutation refuses and cannot leave a passing
execution; relocated replay preserves original receipt provenance. A causal
profile is admitted only with its verified execution record. Failure cleanup
accepts an already missing owned profile; another cleanup I/O error is recorded
separately while preserving the original refusal and `FAILED` execution. A
profile retained because cleanup was refused is unqualified. Missing-profile
and permission-refusal controls independently check the persisted failed record.

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

## Completed source-bound checkpoints

Rechecked on 2026-10-07 against current main, owned worktrees, actual terminal
output and original artifacts. These completions retain their consumed source;
later source changes and new measurement profiles require affected checks.

| Source / scope | Observed completion | Boundary |
| --- | --- | --- |
| `c6a9120dc71d9878ea62e37384a289535191b42f` hosted main | Required docs/static/Python/tests/bench/verify GitHub contexts succeeded. [CircleCI tests job2011 artifacts](https://circleci.com/api/v2/project/circleci/Q3G2VbitoZmaQSKihvptcF/MdEMYnJmwKC7e6XooHrif4/2011/artifacts): receipt revision matches; raw/inventory SHA-256 matches; 159 terminal suites, 4,241 passed, zero failed | Auxiliary PR coverage was pending; it is separate from this completed main scope. Bench is compilation. This is not a receipt for later source |
| `8642fa9b3b47ac58e4b3d9f2feaea66c599be29d` hosted main | Scale/Scanner owners checked all six terminal jobs in [workflow1019](https://app.circleci.com/pipelines/circleci/Q3G2VbitoZmaQSKihvptcF/MdEMYnJmwKC7e6XooHrif4/1019/workflows/a9eb1372-fe07-468c-a8b0-f809cff35b7c). Rust: 4,268 passed, zero failed, 30 skipped; source SHA, command, inventory and raw/result digests matched the receipt. F15 selected9 also passed | Complete regular main CI at this source; bench is compilation. Later cache test-only extensions and their hosted result remain separate |
| `888c434dc4891f8db79ab495e66595df015cc5e8` hosted main | All six terminal jobs in [workflow1031](https://app.circleci.com/pipelines/circleci/Q3G2VbitoZmaQSKihvptcF/MdEMYnJmwKC7e6XooHrif4/1031/workflows/bc9b8b80-5178-4881-8288-1b3e9974652a) succeeded with matching job revisions. Rust receipt/inventory/raw digests independently agree: 4,280 selected/executed/passed, zero failed, 30 ignored. Python: 4,321 passed, 30 skipped; Ruff check/format passed | Complete regular main CI at this source; bench is compilation. Later SourceFile buffer sharing requires its own source-bound result |
| `5bf6b15216968afde597606033870cc710cb9bc8` hosted source-sharing checkpoint | All six terminal jobs in [workflow1033](https://app.circleci.com/pipelines/circleci/Q3G2VbitoZmaQSKihvptcF/MdEMYnJmwKC7e6XooHrif4/1033/workflows/35192a12-61e0-4be6-b485-5c587a415019) succeeded: tests2247, bench2248, docs2249, static2250, verify2251 and Python2252. Pipeline and all job revisions match5bf. Rust receipt/inventory/raw digests independently agree: 4,281 selected/executed/passed, zero failed, 30 ignored. Python: 4,321 passed, 30 skipped; subsequent checks passed | Complete regular CI at this code checkpoint. Bench is compilation; matching-release runtime diagnostics are separate rows. Later docs-only revisions retain their own lint/source identity and are not relabeled as this CI run |
| `0d0eaa9312baf429e836f5a04be4f7364f7fc1c7` hosted capture-refusal checkpoint | All six terminal jobs in [workflow1036](https://app.circleci.com/pipelines/circleci/Q3G2VbitoZmaQSKihvptcF/MdEMYnJmwKC7e6XooHrif4/1036/workflows/548df841-2b6d-4d32-8b1b-b792ff4f7509) succeeded: tests2265, Python2266, docs2267, bench2268, static2269 and verify2270. Pipeline and every job revision match `0d`. Rust source-bound receipt, inventory and raw digests independently agree: 4,281 selected/executed/passed, zero failed, 30 ignored. Python: 4,323 passed, 30 skipped; Ruff check/format passed | Complete regular CI at this code checkpoint, including the two new cleanup-refusal regressions. Bench is compilation. Selected Contract/TSan and runtime diagnostics retain their own scopes; later documentation is not relabeled as this CI source. Superseded docs-only workflows1034/1035 were canceled while preserving their completed job outcomes |
| `b9c058e15d7349ed76adaa408abbd43f123aa65a` hosted closure checkpoint | Pipeline1037's original workers docs2271, bench2272, static2273, tests2274 and Python2276 succeeded at matching source. Final verify2275 failed before executor start (`infrastructure_fail`), so its checkout/source command was `NOT_RUN`. A failed-only retry in [workflow1037](https://app.circleci.com/pipelines/circleci/Q3G2VbitoZmaQSKihvptcF/MdEMYnJmwKC7e6XooHrif4/1037/workflows/7f0e4a3e-87e3-4846-84ba-3f2d624398b7) succeeded: actual checkout and source-equality command in verify2277 exited0, inheriting the same five successful worker IDs. Original Rust raw/inventory/receipt agree: 4,281 selected/executed/passed, zero failed, 30 ignored; Python4,323 passed/30 skipped and Ruff check/format passed | Regular CI is complete at `b9` through the successful retry; original workflow `03c67383-3d37-4395-979a-b1267b555eaa` remains failed and is preserved. Workers were not rerun or counted twice. Bench remains compilation; SDK and selected Bat runtime capture retain separate scope. Artifacts: `/Users/songmin/.codex/task-evidence/scale-shared-final-20261007-1bl40iuo/ci-b9c058e1/` and its `retry-7f0e4a3e/` subtree |
| `b2599e70b262d707771a69440f0e9a0f0c587e2b` hosted Rust observation/tooling checkpoint | Rust tests1570 succeeded at matching revision. Original receipt/raw/inventory digests agree: 4,286 selected/executed/passed, zero failed, 30 ignored inventory entries. Rust static/docs/bench and final verify contexts succeeded in [workflow](https://app.circleci.com/workflow/5b689098-0c80-4c51-9952-9dadab89d3d0). Python1569 executed and failed at the default paired-runner temporary directory: `/private/tmp` is absent on Linux | Rust scope is `VERIFIED`; regular CI at this source is `FAILED`. Bench is compilation. The Python failure is a product portability defect and is preserved separately from its later repair; local macOS focused passes cannot replace the Linux result |
| `48fdc70045f3edcfc30378c8f9a048adc2cd56f9` hosted Python portability repair | All six regular contexts in [workflow](https://app.circleci.com/workflow/86a9d214-6bf7-4acb-81e7-ae24bd8a105c) succeeded. [Python1576](https://circleci.com/gh/josongmin/quanta-index/1576):4,355 passed,30 skipped; Ruff check/406-file format passed. Overlapping P00 selection386 is separate. [Rust1578](https://circleci.com/gh/josongmin/quanta-index/1578) receipt/source/raw/inventory digests independently agree:4,286 selected/executed/passed,zero failed,30 ignored inventory entries. Docs1575/static1579/bench1577 match source | Regular CI is `VERIFIED`; bench is compilation. Do not sum P00 or qualify later paired-tooling source with this receipt |
| `c38decbf04bdd9522e5c859423f05dc346cdecaf` hosted packed-source planner checkpoint | All six regular contexts in [workflow](https://app.circleci.com/workflow/3b302ad1-2cc4-419b-bbde-7c05ef71ad67) succeeded at matching source. Rust1584 receipt/raw/inventory/source independently agree:4,290 selected/executed/passed,zero failed,30 ignored. Python1582:4,355 passed,30 skipped; its overlapping P00 selection386 is separate. Docs1581/static1585/bench1583/final1586 succeeded | Regular CI is `VERIFIED` at this source; bench remains compilation and the later paired adapter/ARB scorer need their own affected results |
| `e227757ab57e680801483e4d443853bfb768d8f2` hosted coupled publication/scorer/R5 checkpoint | All six regular jobs in [workflow](https://app.circleci.com/workflow/24bb4cb4-8663-4e2a-9cdf-41885a015188) succeeded: bench1605/docs1606/Python1607/static1608/Rust1609/final1610. Rust receipt/raw/inventory/source independently agree:4,299 selected/executed/passed,zero failed,30 ignored inventory. Python4,409passed/35skipped; repaired pre-commit hook succeeded | Regular CI is `VERIFIED` at e227. Repeated reporter suite ignored counters are not summed. Bench is compilation; later6c5 scorer projection code has45 focused tests and separate actual corrected score/replay. Its hosted results retain their own source. Artifacts: `/private/tmp/quanta-ci-e227-python/` |
| `6c5cda3cc0a5051d76ad780ce320ed95e5cc02df` ARB projection correction | All six regular jobs in [workflow](https://app.circleci.com/workflow/124014dc-b8e8-40d1-9c4c-bd9a92f3725c) succeeded at matching source: bench1611/static1612/docs1613/Python1614/Rust1615/final1616. Rust receipt/raw/inventory/source independently agree: 4,299 selected/executed/passed, zero failed, 30 ignored inventory entries. Python: 4,410 passed/35 skipped in 911.82s; pre-commit hooks succeeded. The focused scorer/corpus selection passed 45/45 and all three actual official score/replay pairs agree on 128 captured cases/384 route outcomes | Regular CI and corrected diagnostic scoring are `VERIFIED` at 6c5. Bench is compilation. Rust source, exact inventory/event names and raw/receipt digests are under `/private/tmp/quanta-ci-6c5-rust/`; Python raw/action digests are under `/private/tmp/quanta-ci-6c5-python/`. Later documentation-only publication has its own source and focused lint result |
| F15 candidate integrated as main `8599f2e8` | Five publication tests passed (72.842s), exercising 32 I/O and 32 SIGKILL cuts. Selected mutation/seal/cost integration tests: 73 passed. Strict lexical all-target Clippy: exit0 | Owner worktree execution preceded integration. Later recovered-query assertions and daemon/scale execution are separate |
| F15 follow-ups `b09c4aa7` / `70521514` | Recovered-query parity and interrupted delta clone retry implemented. Owner reports 36 I/O + 36 SIGKILL cuts, corrected partial-clone controls, 73 storage regressions and strict lexical Clippy passing. Fixed705 daemon214 also passed | Complete focused code/recovery scope; latest hosted and Large/XL cost/RSS retain their own source binding |
| `eb97e7c2` restart custody | Medium OS-process restart/delete and release-binary identity controls: 2 passed. The process harness checks daemon SHA-256 before each restart | Existing fixture and selected binary; not Large/XL or real-provider cache proof |
| `8642fa9b` Medium causal capture | Registered matching-release capture exited0 in22.062s; source and binary identities match before/after, with no dirty overlay. 256 files across four source repositories, 367,801 source bytes; sampled phase RSS max101,580,800bytes, largest RSS observation gap116.192ms within500ms. Full/delta/no-op/delete retained bytes and same-process reopen are recorded in `/Users/songmin/.codex/task-evidence/scale-final-20261007-8642fa9b-f8xdnze5/medium-causal/{execution.json,artifact/summary.json}` | Shared macOS `VERIFIED_DIAGNOSTIC`; sampled maxima are not true peaks. No Linux physical-I/O, quiet-host qualification, Large/XL or OS-child restart result is issued by this capture |
| `5c2b6843` text/deadline repair | Focused 15/15, lexical 417/417 and `just rust-profile test-daemon` 300/300 passed; lexical and harness strict Clippy, format, hexagonal and Cargo-module checks passed. The daemon scope includes independent Large4,096 OS-child restart and ranked-page assertions | These selections overlap and are not additive unique-test counts. Ten daemon extras were skipped, including separately selected scale-restart tests; these owner checks do not issue capacity or speed qualification |
| `5bf6b152` verified source-surface sharing | Stable owned changes passed lexical/harness `./scripts/cargow --lane test-scale-f15-lane clippy -p quanta-index-lexical -p quanta-index-searchd-harness --lib --tests --all-features --locked -- -D warnings`, lexical library plus resident/sealed/regex/preview integration selection 418/418 (one skipped, 127.751s), and `just rust-profile test-daemon` 300/300 (ten skipped, 413.285s). Format and staged diff checks passed before the three-path checkpoint was pushed | Selections overlap; this is code/query/storage/restart regression proof. Matching-release XL cache admission, sampled RSS, offered-load and selected OS-restart remain separate executions |
| `5bf6b152` Large causal capture | Matching preserved release exit0 in59.592s; original4,096files/16repos and corpus digest `6c51fe0d9c33679eb6b5e5687b5e5a0c16aab5acbc8bfcefc48a71ac79a085ca`. Registry resident30,246,387bytes; sampled RSS max730,087,424bytes and observation gap109.087ms. Full/delta/no-op/delete retained bytes88,348,385/108,065,497/88,560,752/90,429,887. Independent replay plus the capture producer's fixed binary-source annotation reproduces the stored canonical profile bytes | Shared macOS `VERIFIED_DIAGNOSTIC`; same-process reopen and fixed query/probe assertions passed. Historical5c and new5bf runs are not randomized paired speed/RSS comparisons |
| `5bf6b152` XL causal capture | Matching preserved release exit0 in518.054s; original32,768files/64repos,113,023,546raw source bytes and corpus digest `dbc4b0d39b458aa4fd838a28e01caf95c0146601b1fae50bfc5d3b197dd59821`. All ten lifecycle phase endings passed, including same-process reopen. Registry resident326,772,711bytes is below unchanged536,870,912-byte admission. Sampled RSS max3,167,567,872bytes and gap114.778ms; full/delta/no-op/delete retained bytes923,860,540/960,956,447/924,117,321/939,220,849. Independent replay plus the same fixed producer annotation reproduces the stored canonical profile bytes | Shared macOS `VERIFIED_DIAGNOSTIC`; samples are not true RSS peaks, physical-I/O attribution or qualified Linux performance. Registry accounting, retained files and process RSS are distinct measurements |
| `5bf6b152` XL open-loop | Matching preserved release exit0 in191.424s. Default25/50/100/200QPS ladder,10s/point, seeded Poisson,32 client workers,256 queue and2s request timeout; offered=served3,743 with zero drops, timeouts, typed/transport errors or invalid results. Independent JSON/hash/accounting checks passed | Shared macOS `VERIFIED_DIAGNOSTIC`. Target200QPS was saturated: offered195.8/s, achieved180.039/s, p99 latency1,089.176ms. This does not qualify stable200QPS service |
| `5bf6b152` selected XL OS-process restart/delete | Original Nextest process exit0; exactly one selected ignored test passed in284.838s using the SHA-bound preserved release daemon. Original inventory95cases/9ignored, six raw events, stderr summary, source and executable identities independently agree. Separate `xlarge-restart-verification-v2.json` is `VERIFIED_DIAGNOSTIC`; 13 missing/additional/altered-input controls were rejected, including integer-type and duplicate-JSON-key controls | The external execution wrapper remains `FAILED` because its generic Nextest parser rejected ignored-test reporter counters. The original wrapper record is preserved; the actual test ran once. Inventory was collected after execution at the same frozen source/selector; this is not a pre-run required-CI receipt or performance result |
| `5c2b6843` Large causal capture | Matching release exit0 in97.888s; clean source and executable epoch agree before/after. Original4,096files/16repos and corpus digest `6c51fe0d9c33679eb6b5e5687b5e5a0c16aab5acbc8bfcefc48a71ac79a085ca` are unchanged. Sampled phase RSS max518,455,296bytes, largest RSS gap117.495ms; full/delta/no-op/delete retained bytes are88,365,586/108,082,699/88,577,959/90,435,321. Artifacts: `/Users/songmin/.codex/task-evidence/scale-paged-final-20261007-5c2b6843-mwatpufl/large-causal/{execution.json,causal-profile.json,artifact/summary.json}` | Shared macOS `VERIFIED_DIAGNOSTIC`; one instrumented run, no paired speed claim, true peak or Linux physical-I/O claim. Profile defaults and same-process reopen remain distinct from the XL/open-loop/release OS-restart scopes |
| CI admission integrated as main `9def97ac` | Config and test-authority preflight admit supported receipt tier/context combinations and reject unsupported ones | Implementation is present; the new source's hosted result is separate from C6 |
| Frozen `492d2fdccc0fc42ec42e4da19db7833ecbf032ea` XL lifecycle | Original `xl-result.json`: exit0, `VERIFIED`, 287.843s, existing frozen binary, `query_dispatch_budget_ms=600000` | Functional publish/delete/restart scope. Latest main Large/XL cost/RSS and qualified performance remain separate |

C6's actual test command was `./scripts/cargow nextest run --workspace
--all-features --locked --message-format libtest-json-plus
--message-format-version 0.1`.
The F15 owner ran `./scripts/cargow --lane test-scale-f15-lane nextest run
-p quanta-index-lexical --lib --all-features --locked --no-tests fail
--test-threads 1 --no-fail-fast -E 'test(/^publication_faults::tests::f15_/)'`.
The 73-test run selected `l2_file_mutation`, `sealed_manifest` and
`sealed_commitment_cost`; Clippy selected the lexical package with
`--all-targets --all-features --locked -- -D warnings`.
These are observed prior owner terminals, not Rust runs performed by the doc edit.
The original XL result remains at
`/Users/songmin/.codex/task-evidence/quanta-scale-recovery-20261006-01a10d0b/xl-result.json`.
The matching `5bf6b152` execution and replay inputs remain under
`/Users/songmin/.codex/task-evidence/scale-shared-final-20261007-1bl40iuo`:
`build-binding.json`, `preserved-release/`,
`{large,xlarge}-causal/{execution.json,causal-profile.json,artifact/summary.json}`,
`xlarge-open-loop/{summary.json}`, `xlarge-open-loop-execution.json` and
`xlarge-restart-verification-v2.json`. The failed original restart-wrapper record
and the interrupted5c XL attempt remain separately preserved. Product parser
required-test admission was not relaxed for this selected ignored diagnostic.

## Observed scanner two-arm diagnostic

On 2026-10-07 the recovered Bat corpus (revision
`4608fc959aa8abf80d32198836511a570b7ae9ea`) was captured in both actual scanner arms:
79 files, 338 queries, six incomplete files, warmup1 and measurement repetitions3.
The frozen source is `a5e87614bee79bf9d67c8358df01313242548798`; Unicode-control/ASCII
source identities are `221118289b91942f420053fb347dc0b32f9e60dab86d46d04de5ddb6a6fbaa77`
and `8870872d6e69f854b2dcc95247158ef31f2c6cc3902f23f8474f44d145111e1d`.
Socket-path preflight repairs `a5e87614`/`a80eb83d` precede expensive builds and
refuse excessive UTF-8 byte lengths and resolved-parent paths. Custody/source47,
existing runner-path2 and additional byte-versus-character/symlink3 regressions passed.

Baseline v3 release build completed in30m37s, but its default `require-complete`
capture refused ParseFailed6/79. A separate explicit `allow-incomplete` capture
exited0. Candidate's initial fresh build was interrupted at14m16s with exit143
to yield to code validation. Normal resume of that owned target then exited0
in1793.091s including resource admission wait, and candidate live capture exited0.
The strict failure and interrupted build remain separate from these completions.
Earlier v2's42m58s build and failed155-byte macOS socket path are not replay success.

Actual pre/post source inventories, binaries, tool bytes/versions, inputs and
capture-output digests were rechecked; the pair differs only by the approved
scanner source overlay. Individual environment fingerprints remained stable;
two-arm execution-path differences are `CARGO_TARGET_DIR`, `PATH` and `PYTHONPATH`.
The existing `compare_scanner` component verified338 rows with matching status,
path/order/score bits, completed responses, cursor, work counts and configuration.
Both arms retain325 success and13 capped outcomes; capped is not successful coverage.
Result: `diagnostic_unqualified`, with no speed/adoption decision. Evidence stays
outside the checkout at `/Users/songmin/.codex/sab7v3/`, including
`baseline-diagnostic-observed.json`, `candidate-diagnostic-observed.json` and
`observed-scanner-comparison.json`. Executed comparison command:
`PYTHONDONTWRITEBYTECODE=1 .venv/bin/python -B /Users/songmin/.codex/sab7v3/compare-observed.py`.

These are observed local builds/captures and external identity rechecks, not
completed `scanner_build_custody` combined receipts. Canonical
`query_timing_overhead.py --scanner-ab` two closed-receipt validation at that
source is `NOT_RUN`.
Frozen a5 does not qualify later lazy-reader/deadline or buffer-sharing source,
and this fresh diagnostic does not recover the absent historical binary replay.
Admitted-host inputs, declared repetitions and whole-caller acceptance retain
[E4-03](../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-e4-03)/
[E4-06](../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-e4-06) boundaries.

## Closed scanner build and capture diagnostic

On 2026-10-08, a separate clean-build pair at
`f60609fe2d2882f1193d4ba68deb8704e8972138` completed both fresh release builds
and native captures with exit0. The Unicode control applies only the declared
`code_search.rs` overlay; the ASCII candidate uses the unmodified source.
The same Bat revision, 79 files, 338 queries, six admitted incomplete files,
top-k10, seed1907, warmup1 and three measured repetitions were retained.
The original interrupted a5 build is not part of these new receipts.

`python -m tools.benchmark.retrieval.query_timing_overhead --scanner-ab`
verified the two closed `scanner_build_custody` receipts and all 338 response,
score-bit, ordering, cursor, configuration and work-counter parity rows.
Receipt SHA-256 is
`5367af80a40aa8bb31e9a84fb958c362fe5736d8565936d788d62085674b4b73`
for Unicode and
`1c9480c957cebe78c109e5d07f286f4a5a653b3664ae92f8aa8a6b918d2921f4`
for ASCII. Source inventories, original inputs, tool bytes/versions, execution
environment, fresh binaries and capture bytes are bound by those receipts.
Original specs, build/capture logs, both receipts and `closed-comparison.json`
remain at `/private/tmp/qs1iztd4ov/`; the frozen source worktrees remain available
for verification. The canonical comparison exited0.

Status remains `diagnostic_unqualified`: local source/build custody is
`VERIFIED`, while admitted-host performance, repeated schedules and adoption
criteria remain absent. Orchestrator elapsed time includes lease waiting and
cannot be used as a build or query latency metric. This scope does not recover
the missing historical replay, qualify current-main performance, or promote
the earlier observed a5 capture to a closed receipt.

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

## Whole-pipeline measurement acceptance

The retired CS-ENG-02/BENCH-04, S30-B07, J7Q-03/04 and MISC-06 packets share
this boundary. It does not assert that remaining measurements were executed.

- Increasing one-file, delete and mixed updates include index/ranked keys/coverage,
  text and file authority, catalog, semantic work, publication and independent open.
  Attribute pre-intent and lock-held preflight, build, seal/hash and open separately;
  report physical I/O, rows/pages/hash work, transient/retained heap and disk. Changed
  page bytes or logical charges do not prove sublinear total ingest or physical heap.
- Removing repeated verification needs an authenticated immutable base capability
  preserving pre-intent refusal, lock-time ownership, lineage and tamper/eviction/
  repaired-retry controls. Process-local locking does not exclude external mutation.
  Cached/inherited/root/semantic corruption, symlink/orphan/oversized inputs and
  over-limit definite/indefinite decode keep independent refusal/rebuild oracles.
- Measure complete client construction-to-decoded-output and observable server phases,
  output units, process-tree CPU/RSS, construction and every authority sidecar. Cold
  process/model/index/page-cache and warm query are separate states; fresh directories
  do not prove cold OS cache. Unavailable observers retain named limitations.
- Selected B07 timing requires at least five fresh roots and 1,000 warm observations
  per route, warmup at least one, randomized paired schedules, independent rounds and
  continuous load/frequency/thermal/power/disk admission. Report p50/p95 uncertainty;
  p99 needs adequate tails. No retry-until-favorable sampling. Open-loop reconciles
  offered/achieved/errors/timeouts/drops and queue delay independently of completion.
- Existing medium/large/XL fixtures retain original source shapes and independent
  planted-query/count/restart truth. Whole-call ceilings and typed over-limit refusal
  govern capacity; sampled maxima and logical retention keep their narrower scope.
- Test-cost optimization uses at least five warm paired samples of the same selector,
  count and assertions, reporting median/p95/min/max. Query observer on/off and hybrid
  fetch-floor experiments retain parity and whole-caller/deadline controls. Existing
  DSL floors (200 warm, 20 cold) and Criterion minima (10 samples, 1,000 resamples)
  keep their selected contracts; short Criterion runs are diagnostics.

## Installed, paired and operational acceptance

The retired S21-11/12/13 and CS-INT-01 packets retain these release distinctions:

- Execute the selected actual producer/SDK/daemon/installed CLI, publish/activate,
  crash/restart/retention/rollback and supported Linux scopes on matching source and
  binaries. Unit fixtures, local owner rails and relocated replay keep separate proof.
- An exact clean producer/consumer pair binds both sources, dependency resolver locks,
  build/tests and attested daemon bytes. Preserve the V2 chain from source/prepared
  payload/request through journal/sealed candidate/CAS activation and exact ACK replay.
  Wrong identity, old wire and same-identity/different-payload refuse without mutation.
  Runner-candidate-only archives cannot issue operational qualification.
- Current-format backup/restore uses the exclusive lease and original immutable
  manifest/catalog/incarnation authority, with replayed append/clear/replace/tombstone
  windows. Target-root/release proof is separate from disposable owner fixtures;
  retired formats require producer rebuild, not silent legacy import.
- Deploy, activate and restore-forward each require a concrete authorized Linux target,
  independent observed pre/post contract, actual action and same host/config/state/
  source continuity. Distinct actor bytes and exit zero are insufficient domain
  oracles. Registry entries remain staged until these inputs and executions exist.
- The live proof registry and independent checker own the selected P00–P12 DAG,
  target/staging and required raw inventory. P12A infrastructure is independent of
  P11; final --require-all --bind-source qualification needs all selected dependencies.
  CODE_QUALIFIED, DEPLOYED, ACTIVATED and ROLLBACK_PROVEN remain distinct verdicts.

Usage stays in the P11/state operator runbooks, Justfile and executable authorities.
Unissued release or operational results stay in the residual ledger.

## Consequences

Completed implementation has an ADR owner; unmeasured optimizations and release
requirements keep their active owner. Neither documentation consolidation nor
past test totals issue new benchmark, code, release or operational qualification.
