# OCT-05-004 — Cost, Capacity and Qualification Boundaries

Status: `Accepted`

Decided: 2026-10-05

Consolidates implemented O4-E4/I0 contracts under
[SEP-26-002](SEP-26-002-retrieval-observation-experiment-and-default-policy.md),
[SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md) and
[SEP-27-004](SEP-27-004-benchmark-capture-and-resource-custody.md).
No conditional optimization or staged operation becomes accepted implementation.
Measurements and missing authority remain in the [Oct-17 residual ledger](../plans/oct-22-finalizing/README.md); [performance records](../plans/oct-22-finalizing/E07-performance-operations.md) retain source-scoped execution evidence.

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

Owners: [document census](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/doc_census.rs),
[live statistics](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/sealed_generation/live_bm25.rs),
[native writer](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/writer_cache.rs) and
[selected scorer port](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/searcher/port.rs).
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

Owners: [harness publication](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-searchd-harness/src/harness.rs)
and [Scale producer](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-searchd-harness/src/scale.rs).
Owner execution and matching-release stage attribution remain separate checks
under [E4-01](../plans/oct-22-finalizing/E07-performance-operations.md).

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

The legacy XL diagnostics used 32,768 files, 64 repositories, seed
5864059738136528177 and the same admission limits. On `82bca2f2`, six
generation proof envelopes consumed 75.644s of a 90.611s delta seal. The
61.645s file-authority reconstruction was nested. Post-repair `e227757a`
passed full/delta/no-op/delete/query/reopen and independent replay. Delta
seal took 42.517s, including 32.596s of proof envelopes. Activation remained
a separate operation. These are two source-bound, instrumented macOS samples.

The packed planner now binary-searches strictly ordered `SourceFileKey`
rows and validates the full binding. In one publication, the non-cloneable,
non-serialized `PublicationValidationOwner` carries identity, root/manifest
and policy commitments from independent membership verification. Refusals
still authenticate current digest, path, length and inventory. Query/scrub
and new-generation validation retain independent proof. This introduces no
source-body, query-handle or metadata success cache.

The frozen coupled scope passed 1,035 lib tests, 36 mutation tests and 300
E2E tests with 10 skipped, plus the stated strict checks. The original two
builds, binaries, nested attribution and fault selections are recoverable below.
[Run details and original receipts](../ARCHIVE-INDEX.md#earlier-benchmark-diagnostics)

### Publication plan custody and remaining structural cost

The repeated plan computation is separate from the six proof envelopes above.
`preflight_file_authority_batch` originally derived the complete file manifest
before intent, under the operation lock, and during build; the writer derived
it a fourth time against the cloned target. Every derivation decoded the root,
rebuilt the ordered source map, counted memberships and encoded the manifest.
This is whole-corpus metadata work even for one changed file. The earlier
binary-search repair removed a quadratic lookup inside each invocation, not
these repeated invocations. The residual 2.959s in the XL partition is not a
measurement of this planner; it contains other work too.

The lexical adapter now keeps one canonical `FileAuthorityDelta` in the
publication owner's private `FileAuthorityPlanCache`. Reuse requires the exact
current root and staged-manifest bytes, policy and file-affecting operations.
Every invocation still opens current staged sources without following symlinks
and checks byte-size admission. The writer checks the actual target, consumes
the plan once, and attaches the matching changed payloads before applying any
index mutation. A different target root/manifest, including a partial retry,
derives a fresh plan. Custody retains only bounded metadata, source lengths
and encoded manifest bytes; changed source bodies live only in the writer's
materialized plan. Raw channel callers use the same planner with fresh custody.

This removes repeated semantic plan construction without changing the
same-inode/size/restored-mtime fault contract. It does not remove the six
generation proofs, repeated current-byte reads, or all unchanged-source stats.
The new prepare/revalidate diagnostic spans cover derivation/storage admission;
root/manifest reads and operation binding occur before those spans. They must
not be presented as the complete planner wall time. No XL timing is attributed
to this new change until a matching executable is measured.

Owner verification used `./scripts/cargow --lane test-scale-f15-lane` with
`-p quanta-index-lexical --all-features --locked`. The library plus
`--test l2_file_mutation -- --test-threads=1 --quiet` passed368 library tests
(one private subprocess entrypoint ignored) and36 integration tests. The
initial two-thread library run failed one history BM25 test with writer
`LockBusy`; its isolated run and the complete serial rerun passed. Following
mechanical Clippy fixes, `test --lib plan -- --test-threads=1` passed31 and
`test --test l2_file_mutation -- --test-threads=1 --quiet` passed36 again.
`clippy --lib --tests -- -D warnings`, format, hexagonal boundaries, module
discipline/cycles and no-allow checks passed. Matching XL execution was `NOT_RUN`
at this owner checkpoint; the later diagnostic below covers both publication
repairs. Broader daemon suites remain a separate scope.

Another redundant cost was cold publication validation using a query-oriented
`verify_authority` and a `DiscardingVisitor`, reconstructing source/normalization/
posting evidence before dropping query materialization. The following repair
removes that materialization while proving the same invariants.
Inherited packs/postings and Tantivy segments
already reuse immutable objects; swapping the underlying engine alone does
not remove caller-owned whole-generation verification. Coverage already uses
a persistent shared tree; cloning coverage is not an additional whole-map
copy defect.

An end-to-end O(delta) claim also conflicts with the current requirement to
detect arbitrary edits of unchanged local files at every refusal boundary.
Inode, length, restored mtime, or a Merkle root alone cannot prove that current
mutable file bytes still match their commitments. A future storage capability
that enforces immutability could change that cost boundary; relaxing validation
silently is not a repair. Until then, separate unavoidable byte authentication
from redundant reconstruction and measure both under the existing contract.

### Publication-only verification output

Cold publication and serving now execute one canonical
`verify_authority<Output>` walk. `ServingOutput` retains source bodies and
normalized fields plus posting directories; `PublicationOutput` carries no
serving fields. The latter consumes no `SourceFile` factory and retains no
posting page vector. Both still decode each page descriptor, normalize sources,
compare exact posting memberships, enforce serving resident/directory/scratch
policy and authenticate all source/posting object bytes. A shared pinned-reader
wrapper checks object identity and inventory before either output escapes.

The cold publication result is a distinct typed proof, which may establish
the existing publication witness but cannot enter `from_v15_verified` or
construct a query handle. Independent query open and scrub keep the serving
output. This avoids copying each source into a serving Arc and retaining all
normalized source fields and posting-directory pages merely to discard them.
Normalization and per-bucket membership maps remain required verification
scratch; this is not removal of the two cold proofs or their byte reads.

The fixed-census regression observes one source and two directory
materializations for serving and zero of either for publication, while both
read the same three committed objects in the same order. Corruption oracles
require identical refusals for forged resident charges, source counts, term
metadata, retired IDs, rehashed malformed objects, invalid UTF-8, missing or
mutated objects and six reduced policy dimensions. A repeated digest with
changed pinned identity is also refused by both consumers. These structural
checks establish allocation-path removal, not measured RSS or XL latency.

`VERIFIED`: `./scripts/cargow --lane test-scale-f15-lane test
-p quanta-index-lexical --lib --test l2_file_mutation --test f15_file_authority
--all-features --locked -- --test-threads=1 --quiet` passed373 library tests
(one private subprocess entrypoint ignored),3 F15 integration tests and36
file-mutation integration tests. The earlier focused `file_authority::` run
passed55. Lexical library/test strict Clippy, format, hexagonal boundaries,
module discipline/cycles and no-allow checks also passed. The matching XL timing
and RSS measurement below now covers this change. A broader daemon-suite run
remains `NOT_RUN` for this owner checkpoint.

Whole-root metadata and unchanged-source inventory traversal still scale with
the corpus. The F15 producer also requires eight prefix bits, so a changed
source rebuilds its affected fixed bucket, whose size can grow with the corpus.
These remaining costs are distinct from source-body retention and repeated
proof construction; neither repair establishes end-to-end O(delta) behavior.

### Post-repair matching XL diagnostic

Frozen `703d0e68` exited 0 in 361.748s after the plan-custody and
publication-only repairs. The original XL corpus, configuration and 4 GiB /
history / registry admission matched `e227757a`. All ten phases completed
and independent replay matched the canonical profile after excluding its
capture-only binding metadata. The controller had refused an earlier stale
binary; the successful run pinned Cargo's actual executable.

Delta seal took 40.971s, with 32.609s in six generation-proof envelopes. Nested
file-authority work was 23.881s. One plan derivation and three storage
revalidations remained, along with full metadata and inventory traversals.
Same-process reopen is the recorded scope. Sampled RSS and retained logical
bytes do not establish continuous RSS or physical allocated space. A single
profiled macOS run provides no paired speed or Linux release qualification.
[Later SDK replay-CAS proof](OCT-05-003-active-query-and-runtime-lifecycle.md#replay-cas-preflight-authority-repair) · [Run details and original receipts](../ARCHIVE-INDEX.md#earlier-benchmark-diagnostics)

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

The former `text_authority/reader.rs` owner was replaced by the
[paged Native reader](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/text_authority/paged_reader.rs).
Related owners:
[candidate integrity](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/searcher/candidates.rs),
[snapshot accounting](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/adapter_open.rs) and
[harness deadline](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-searchd-harness/src/harness.rs).
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
Owners: [coverage reader/cache](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/sealed_generation/coverage.rs)
and [delta planning](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/adapter_ingest.rs).
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

Owners: [selected scorer/Explain](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/searcher/code_search.rs),
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

- [Lexical authority](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/file_authority.rs),
  [query execution](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/searcher/code_search.rs),
  [scanner comparison](../../tools/benchmark/retrieval/query_timing_overhead.py).
- [Scale](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-searchd-harness/src/scale.rs),
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
[paged File codec/producer](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/file_authority/paged.rs),
[scale profile](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-searchd-harness/src/scale.rs) and
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
[publication oracle](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/publication_faults/tests.rs)
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
below; [E4](../plans/oct-22-finalizing/E07-performance-operations.md) directs
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

The dated hosted main receipts run from `c6a9120d` through
`6c5cda3c`. At `b9c058e1`, the final verify executor initially failed before
checkout; a failed-only retry completed using the same five worker receipts.
At `b2599e70`, Rust 4,286/4,286 passed, while the Linux Python job failed on
the macOS-only `/private/tmp` default. The subsequent `48fdc700` workflow
passed all six regular contexts. Bench context counts compilation. Rust,
overlapping F15/P00 selections, Python and workflow outcomes keep their
original source and denominator.

The matching-release Medium, Large/XL causal, XL open-loop and selected
OS-process restart/delete checks were separate results. The `5bf6b152`
open-loop run offered and served 3,743 requests; the selected XL OS-process
case passed in 284.838s on its pinned daemon. The `492d2fdc` XL lifecycle
passed on the frozen binary with a 600,000ms dispatch budget. Those scopes
leave current-host cost, installed acceptance and affected changes to their
own gates. [Run details and original receipts](../ARCHIVE-INDEX.md#earlier-benchmark-diagnostics)

## Observed scanner two-arm diagnostic

Frozen `a5e87614` covered the original Bat 79 files, 338 queries and
six incomplete files. The baseline initially refused strict completeness and
then both arms completed explicitly admitted partial captures. Comparison
matched all 338 rows, including 325 successful and 13 capped outcomes, plus
path/order/score bits, cursor, work and configuration. The interrupted initial
candidate build and baseline strict refusal remain separate events. This was
`diagnostic_unqualified`; canonical closed scanner receipts were `NOT_RUN`
for this source. [Run details and original receipts](../ARCHIVE-INDEX.md#earlier-benchmark-diagnostics)

## Closed scanner build and capture diagnostic

The separate clean `f60609fe` pair ran both fresh release builds and
captures under the same Bat corpus, top-k10, seed1907, one warmup and three
measured repetitions. Its approved Unicode overlay is the only source
difference. Canonical `query_timing_overhead --scanner-ab` validated both
closed `scanner_build_custody` receipts and all 338 parity rows. The verdict
was `diagnostic_unqualified`. Lease wait is outside build/query latency;
admitted repeated performance and adoption retain their separate gates.
[Run details and original receipts](../ARCHIVE-INDEX.md#earlier-benchmark-diagnostics)

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
