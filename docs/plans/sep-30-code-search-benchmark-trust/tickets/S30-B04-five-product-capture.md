# S30-B04 — native-bound five-product capture

Status: `PARTIAL` (2026-09-30); see receipt below. Priority: P1. Exact-name lane depends
on S30-B01; the 20-query and robustness lanes also depend on S30-B02/B03 input
freezes. Parent: [Sep 30 plan](../README.md). Contract owners:
[CS-BENCH-02](../../sep-27-code-search-remediation/rfcs/CS-BENCH-02-native-response-validation.md)
and [CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md).

## Matrix and work

Capture Quanta, Semble, Sourcegraph, cs and OpenGrok where each has an actual
supported mode. Do the **full 1,196 exact-name set**, not only the historical
300 or a selected success subset. A lane whose input or product is unavailable
remains visibly `BLOCKED` or unsupported; other admitted lanes may proceed.

For each lane, freeze one of two modes before capture:

- Matched semantics: same case, normalization, literal/identifier meaning,
  path scope, candidate universe, result unit, cutoff and completion rule.
- Native workflow: product's documented user-facing mode, with its own actual
  query transformation and output semantics. Semantic/hybrid vs lexical-only
  belongs here unless an exact common contract is proved.

For a distinct-file exact-name comparison, use Quanta's `keyword_file` policy
(scored, `select:file case:yes name`) for a ranked comparison. `literal_file`
(quoted content phrase) and `substring_file` (raw substring) are constant-score
restrictions returned in path order, so their top 10 is an observed path-ordered
prefix, not a relevance ranking (correction 2026-10-01). Semble or any other chunk-native
product needs an explicit bounded collect-to-ten-unique-files policy and source
rank preservation; if unsupported, retain its ten-chunk observed-prefix result
as a separate diagnostic. Never call a deduplicated ten-chunk prefix file top-10.
Keep declaration/symbol requests separate from bare content search.

Capture raw response bytes, endpoint/process exit, native errors, partial/cap
signals, ordered result identities, source/index generation, request and
effective-query digests, model/index config, timers and output bytes. Replay
with the same pinned native decoder; normalized rows do not substitute for raw.
Attest each product's **actual indexed file universe** and source freshness.
The existing opt-in OpenGrok full indexed-view probe is one available source
check; it does not attest Sourcegraph/cs or Lucene posting freshness. Missing
proof limits cross-product claims rather than fabricating equivalence.

## Verification and deliverable

- Every expected task has one complete native-bound row or a typed
  unsupported/error/timeout/incomplete status. HTTP 200 with native error,
  truncated stream, stale source, duplicate ID, path escape and changed
  normalized hit/order all refuse in focused decoder fixtures.
- Product-specific index inventory and before/after source observations are
  attached or the comparison remains diagnostic with an exact blocker.
- Each capture uses a fresh external root and never overwrites the 300/1,196
  historical inputs, `/private/tmp/g3`, or another product's raw evidence.
- Same input/host captures are repeatable and replayable; a successful capture
  alone is neither relevance nor performance qualification.

Use the existing [live external adapter](../../../../tools/benchmark/retrieval/live_lexical_external.py),
[lexical scorer](../../../../tools/benchmark/retrieval/lexical_file_comparison.py),
[runbook](../../../../tools/benchmark/CODE_SEARCH_RUNBOOK.md) and
[registry](../../../../tools/benchmark/registry.toml). Repair only a reproduced
missing adapter boundary, with focused native fixtures under its current owner.

## Execution receipt (2026-09-30)

`PARTIAL`: exact/symbol/robustness/gin 20 captured and replayed; exact lane repeat capture identical on 1,196/1,196 for 4 products; Semble robustness pairs refused (harness defect, fixed in `59249da8`; see v2 below); SG/OG universes unattested. Producer `quanta-index@0d21914e` clean worktree, except robustness inputs (RESULTS custody);
results, digests and residuals: [qi-s30-bench-trust-20260930-0d21914e/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/RESULTS.md).

v2 (2026-10-01, clean `f318e832`): three Quanta file policies (literal/keyword/substring) and SG/OG/cs on exact and six robustness lanes; all seven Semble lexical-only pair lanes captured and replayed (fix `59249da8`). Semble file top-10 still BLOCKED; SG/OG universes unattested. [qi-s30-v2-f318e832/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-v2-f318e832/RESULTS.md).

The v2 five-product rows are **native-workflow diagnostics**, not a matched-semantics
file-ranking cohort. Quanta `keyword_file` matches content and path with explicit
case sensitivity; the recorded Sourcegraph request uses `patternType:keyword`
without an explicit case clause, OpenGrok uses `full` search, and cs uses its
native command. Before any matched claim, pin and test case, token, path/content
scope, file-filter, ordering and output-unit behavior for each product on the
same source view. Keep an unsupported product outside that cohort. A request
string or source revision alone does not prove an equal indexed universe.

Isolated two-query control (2026-10-01): pinned Semble 0.6.0 indexed the 99
manifest files into 1,171 chunks under a new external root
[`two-query control`](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/probe/probe.json).
For `Param`, the native ten-chunk prefix covered two files; collecting all
positive BM25 chunks and taking each file's first occurrence put `context.go`
at file rank 5 (source chunk rank 23). For `writeContentType`, the gold
`render/render.go` was file rank 14 (source chunk rank 19). Requesting ten
chunks versus all chunks changed the order of equal-score hits, so an adapter
must declare a deterministic tie rule and prove its collection depth. This
control is not an integrated Semble file route or a new 1,196-task score.

After that control, `lexical-file` was added as a separate Semble adapter
profile. It retains every positive-score native BM25 chunk in the raw capture,
then proves its span and projects the first ten distinct files. The frozen
two-query adapter run and evaluator replay succeeded at
[`adapter-run/record.json`](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/probe/adapter-run/record.json)
and [`adapter-report.json`](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/probe/adapter-report.json):
`Param` hit at file rank 5; `writeContentType` missed at file rank 14 in the
full native list. This is a two-query diagnostic on the local working source;
it does not replace the full-suite capture below.

Full 1,196-query follow-up (2026-10-01, clean `5d345a52`): Semble's new
`lexical-file` profile completed 1,196/1,196 rows over the same 99-file gin
manifest and 1,171 indexed chunks; the independent file-gold checker found
1,190 top-10 hits and six misses. Its 62 lists shorter than ten files are
complete under full indexed-chunk BM25 collection. Quanta `keyword_file`
completed 1,196/1,196 with 1,192 hits and 2,408 finite per-candidate SDK
scores in descending score/path order. New raw records and independent counts:
[`Semble summary`](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/semble/summary.json) and
[`Quanta summary`](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/quanta/summary.json).
All 1,196 Quanta paths, statuses and ordering match the frozen v2 `keyword_file`
record; score preservation changed no ranked result. The copied evidence is
hashed in [`MANIFEST.sha256`](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/MANIFEST.sha256).
These are separately executed diagnostic modes with different token, case and
path/content behavior; the prior Sourcegraph/cs/OpenGrok rows were not rerun.
Sourcegraph and OpenGrok ports 7080/7081 refused connections at this follow-up,
and their actual indexed universes remain unattested. The five-product
matched-semantics cohort and a qualified cross-product ranking remain blocked.
This scored-file follow-up covers the **exact-name lane only**. Prefix, infix,
components, typo and both no-answer lanes were captured and replayed in v2 at
`f318e832`, where Semble still used ten BM25 chunks rather than the new
ten-distinct-file profile. A separate follow-up from the same `5d345a52` code
then ran Semble `lexical-file` across all six robustness lanes and Quanta's
scored `keyword_file` on its five supported lanes. The six Semble records and
five Quanta records all passed current evaluator replay and an independent
native/source-file checker. See the [robustness result and raw archive](/Users/songmin/Documents/code-new/qi-s30-robust-filemodes-20261001-xh1F3Xoh/README.md).
Two infix queries and all component queries were excluded by the frozen
Quanta `keyword_file` policy admission before search. Sourcegraph/OpenGrok were
unavailable, so this is not a fresh five-product matched cohort.

An isolated four-query cursor continuation of the archived scored
`keyword_file` generation resolved its exact-name misses. It used a copy of
the state, the archived search daemon binary and a fresh external output root;
the first page of each task matched the archived record by path and score.
The declaration gold files were present at ranks 12 (`writeContentType`), 16
(`ContentType`), 12 (`New`), and 12/13/14/17 (`Write`), with all cursors
exhausted. Thus these four misses are ranked beyond top 10, not absent from
the indexed file set. This does not establish a BM25 implementation defect:
`keyword_file` scores content and path, while the gold is declaration-derived.
The [cursor result](/Users/songmin/Documents/code-new/qi-keyword-cursor-evidence-20261001-utlYjc/cursor-result.json)
and [source/binary/input binding](/Users/songmin/Documents/code-new/qi-keyword-cursor-evidence-20261001-utlYjc/binding.json)
are separate diagnostic evidence; they are not added to the 1,196-query capture.
On a second copy of that **same activation**, the product CLI's typed
`symbol.local_name.exact(NAME) case:yes` route returned each missing
declaration: `writeContentType`, `ContentType`, and `New` at rank 1, and the
four `Write` declarations at ranks 1–4. Symbol IDs, declaration lines and
source hashes matched the pinned gin files. The [symbol result](/Users/songmin/Documents/code-new/qi-current-symbol-evidence-20261001-qEtsXN/symbol-result.json)
and [binding](/Users/songmin/Documents/code-new/qi-current-symbol-evidence-20261001-qEtsXN/binding.json)
confirm that a suitable exact-declaration route existed in the archived
keyword generation. This is a four-query control, not a rerun of 1,196 tasks.

The NOC v2 repair at clean `112c6c7e` was also exercised on all 99 admitted
probes. A runner built at that HEAD produced a fresh Quanta `keyword_file`
record with 99/99 abstentions and zero candidates; current-head evaluator and
robustness-report replay passed with
`content_absence_replay_verified=true`. A separate pinned Semble 0.6.0
`lexical-file` run returned nonempty files for 99/99 probes and passed the same
replay. The frozen census requested 100 probes and excluded one before
submission. [Quanta binding](/Users/songmin/Documents/code-new/qi-noc-v2-capture-20261001-112c6c7e-p4/binding.json),
[Quanta report](/Users/songmin/Documents/code-new/qi-noc-v2-capture-20261001-112c6c7e-p4/quanta-robustness.json),
[Semble report](/Users/songmin/Documents/code-new/qi-noc-v2-capture-20261001-112c6c7e-p3/semble-robustness.json).
The first Quanta attempt used an older runner and is explicitly
[superseded](/Users/songmin/Documents/code-new/qi-noc-v2-capture-20261001-112c6c7e-p3/ERRATUM.md).
These are separate native-policy diagnostics; the 99-query lane is not the
1,196-query exact set or a matched-semantics speed comparison.

Independent read-only audit of the archived inputs, native rows and records
(`audit.py` / `audit.json` in the same external root) checked all 99 source-file
hashes and all 1,196 task/query/gold identities. Semble's complete BM25 lists
project to exactly the recorded first ten distinct files on every task; every
gold path in the six missed tasks is present beyond rank ten (nearest ranks:
`With` 11, `writeContentType` 14, `File` 15, `GET` 16, `Type` 17,
`Name` 27). Quanta's four misses are all `capped` ten-file responses;
their gold rank beyond the captured window is unknown. These observations
locate the miss stage without promoting a bare-name gold to independent
relevance judgment or inferring an engine defect.

## 2026-10-02 current CodeSearch diagnostic capture

The new default `code_search_file` route was captured with the all-features
release daemon and runner built from the `main@53ca51e3` dirty implementation.
The binaries are pinned by SHA256 (`157c29ce9ca5a82157437337e2b682b0763e7f69e87ba9fe2ef68970faa8d5cb`
for searchd; `599b9bc936fc31e040d0e186776e2d4eb327fb0ae7ee51f61d8f7b11bb6bb780`
for the runner). Seven separate Quanta/Semble pair roots are
`/private/tmp/a0` through `/private/tmp/a6`; each has `PAIR_VALID=pass` and
`diagnostic_unqualified` status. The live external collector recaptured and
replayed seven corresponding Sourcegraph, OpenGrok and cs lanes under
`/private/tmp/qi-p2-current-file-final15-juqw3g51`. Its current-file scorer
binds the raw rows, pair records, run manifest, and both phase-metric byte
digests; the [complete summary](/private/tmp/qi-p2-current-file-final15-juqw3g51/five-product-lanes-summary.json)
has per-product status, timing boundary, unsupported IDs and commands.

Common-eligible file Hit@10 counts, with unsupported Sourcegraph queries
excluded from every product in that lane:

| Lane | Common tasks | Quanta | Semble | Sourcegraph | OpenGrok | cs |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Exact | 1,196 | 1,187 | 1,190 | 1,193 | 1,185 | 1,187 |
| Prefix | 351 | 347 | 257 | 349 | 23 | 345 |
| Infix | 339 | 335 | 164 | 338 | 11 | 336 |
| Components | 271 | 270 | 269 | 267 | 146 | 264 |
| Typo | 363 | 17 | 284 | 15 | 0 | 17 |

Sourcegraph explicitly rejected one prefix and seven components queries; the
other products' own-lane denominators remain 352 and 278 respectively. The
99 content-absence probes returned zero files for Quanta, Sourcegraph,
OpenGrok and cs, versus zero of 99 for Semble. The 342 typo-content-absence
probes returned zero files for those same four products, versus 55 of 342 for
Semble. `capped` is an executed search state, not an execution failure:
Quanta's 87 capped exact responses include all nine exact top-10 misses.

This capture does not qualify product ranking or performance. The 1,196 gold
labels are declaration-derived without human relevance review, the binaries
were built from a dirty checkout whose manifest has no source-closure digest,
and Sourcegraph/OpenGrok posting-level indexed-universe identity remains
unverified. Query timers differ for local SDK/worker, loopback HTTP and cs
process spawn, and the host was contended. The raw metrics are diagnostic
inputs for independent gold review, external index attestation and an admitted
fresh-root performance run.

## Offline external-index audit (2026-10-02)

A read-only inspection of copied service data found exactly the 99 manifest Go
files in Sourcegraph's Zoekt shard, with all 99 shard-served source hashes
matching. The shard contains 31 additional non-Go files. OpenGrok's copied
Lucene index has exactly 99 live file documents; a fresh isolated reindex from
the same verified source produced matching `full`, `defs` and `refs` posting
digests for all 297 file/field pairs
([attestation](/private/tmp/qi-index-attest-Z11o05Kx/attestation.json)).

Neither original seven-lane capture recorded the served shard or segment
digest together with a service process/mount identity at request time. The
offline audit therefore does not retroactively qualify those scores. The
current collector now records `opengrok_indexed_universe_attested=false` even
when its API path/served-byte probe passes, and replay refuses a forged `true`.
Future qualification requires before/after backend artifact and service mount
binding in the existing collector, followed by a fresh capture.
