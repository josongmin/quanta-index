# Final engine audit — 2026-09-27

Status: **historical baseline audit**; remediation was NOT_RUN at this snapshot.
This document corrects the initial engine inventory and links the final proposed
solutions. It is not current-build, whole-engine or performance qualification.
Current implementation and outstanding work supersede historical dispositions
here: [CS-INT-01](rfcs/CS-INT-01-integration-and-qualification.md#current-remaining-work-audit--2026-09-27).

## 1. Source and proof boundaries

Source inspected: `main@66cee47efdda7c5f3886ac58690aa645f44f691f`. The checkout is
dirty with concurrent Cargo/benchmark/control-plane work and this RFC packet.
The audited engine directories had no working-tree modifications. Product code
was not edited by this audit; existing dirty changes were preserved.

Three parallel read-only lanes covered query/window contracts, publication and
coverage, and ranking/grouping. The coordinator checked source boundaries,
executed the unchanged snippet owner, reviewed native artifacts and integrated
the RFCs. Transport used the provided collaboration agents because the Superset
CLI was absent; no external workspace or issue was created.

| Proof | What actually executed | Limit |
| --- | --- | --- |
| A: native queries | Pinned retrieval runner, 16 tasks × lexical/symbol; 14 direct CBOR requests; independent Python AST census of 46 hash-verified files | Existing snapshot binaries, not a current-source rebuild |
| B: ingest/grouping | Tiny harness linked to cached native lexical/core/contract libraries; real validation, indexing, delta, seal/open and search | Cached Sep-26 components; no public dispatcher/SDK/daemon proof |
| C: snippets | Current `snippets.rs` included unchanged in a standalone Rust probe, six negative cases plus one positive control | Contract-shape stubs; no parser/index/SDK execution |
| D: source audit | Current call chains, contract fields, collector and normalizer examined; A's 12 critical files match measured snapshot bytes | Does not validate unrelated dirty dependencies or all configurations |

The retained searchd is SHA256
`faa24fd7e971f585d3c1f6dae656f023edfe418e9b059badad89330455b67db4`;
runner is `2a3fb0dbb40dc716cc465e4605c727381b0d28a68ba5d456466dc3b5dcbcbafc`.
Their original measured source is `3c6bc0ad3f3dd6d201774255062a509edc2705d6`.
Current dependency/build closure, full regression matrix and remediation remain
**NOT_RUN**. Source agreement plus old binaries is not a new installed release.

## 2. Findings and final owners

| ID | Finding/classification | Evidence | Owner |
| --- | --- | --- | --- |
| E01 | P1: endpoint domain/projection conflict can become INTERNAL, empty success or unsupported-filter bypass; force-empty also misreports execution | Invariant FAILED in A; current trace agrees | ENG-01 |
| E02 | P1: symbol count cap fabricates exact exhaustion and loses continuation | Invariant FAILED in A, indexed and manual | ENG-01 |
| E03 | P1: admitted surface aliases overwrite same-file data; scope/record path mismatch leaves orphan units | Invariant FAILED in B; current ingress/write keys disagree | ENG-02 |
| E04 | Product has no source-universe symbol completeness authority; strict producer admission is benchmark-only | Source-backed capability gap VERIFIED; new gate NOT_RUN | ENG-02 + PROD-01 |
| E05 | Symbol lookup indexes synthetic name/container/path text, not dedicated exact local/qualified fields | Behavior VERIFIED in A + source; exact-name capability/contract gap, not proven violation of a promised exact API | ENG-03 |
| E06 | Federated same-path files collapse and returned repo IDs identify the containing pin | Behavior VERIFIED in B; source/pin identity contract must be made explicit | ENG-02/03 |
| E07 | Grouped collector applies examined cap after all-group accumulation/merge/sort | Source-backed pre-materialization contract mismatch; peak-memory/runtime-bound test NOT_RUN | ENG-03 |
| E08 | Renderer anchors on raw literal strings instead of true normalized/regex/Boolean matches | Focus invariant FAILED in C; public-route proof NOT_RUN | ENG-04 |

### E01/E02: plan validity and result truth

The implicit Symbol domain can be replaced with Text while the decoder remains
Symbol. Current source chain: `prepare.rs:203–213,303` → `compile.rs:446–449` →
`port.rs:269,286`. Matching file/path/content projections cause INTERNAL; zero
Text matches hide it. Native symbol regex refusal is bypassed by `select:file`.
Sourcegraph `select:file Default` reproduces the incompatible decoder too.

Independent native result:

| Symbol request | Returned | Reported total/outcome |
| --- | ---: | --- |
| Default OR generate_unique_id | 3 | exact 3 / exact_exhausted |
| count:1 Default OR generate_unique_id | 1 | **exact 1 / exact_exhausted**, no cursor |
| index:no Default OR generate_unique_id | 3 | exact 3 / exact_exhausted |
| index:no count:1 Default OR generate_unique_id | 1 | **exact 1 / exact_exhausted**, no cursor |

The three definitions are independently present in the frozen AST census.
Lexical control returns one under count:1 but preserves exact total 311 and
continuation. Symbol `page_limit` truncates before the dispatcher interprets its
length as a `top_k + 1` probe. Carry authoritative window facts across the port.

With typed language `rust` and DSL
`lang:python type:symbol select:file Default`, `force_empty` bypasses even the
explicit conflict and returns exact zero with lane `executed=true`. The branch
precedes read-view acquisition/backend invocation (`routes/lexical.rs:273–297`).
Separate request validity, logical emptiness and actual execution observations.

Preserve verified positive controls: Text `select:symbol`/`type:symbol`, ordinary
Symbol pages, valid next-page cursors and route/case mismatch refusal. The existing
explicit conflict reaches the wire as `INVALID_REQUEST`. Sourcegraph OR/count
translation refused in this probe; do not count that as a reproduced count bug.

### E03/E04: file ownership before coverage metadata

Public uniqueness is `(surface,path)` (`ipc/ingest.rs:869–905`), but the adapter
deletes all documents at `path` (`adapter.rs:293–300,485–553`). Cached native cases:

| Accepted mutation | Text left | Symbols left |
| --- | ---: | ---: |
| One combined File scope | 1 | 1 |
| Chunk scope then Symbol scope, same path | 0 | 1 |
| Symbol scope then Chunk scope, same path | 1 | 0 |
| scope a.rs containing b.rs records; later tombstone a.rs | 1 | 1 |

Fix public admission and materialization around one canonical source-file owner;
do not paper over it with consumer-side filtering. Correct combined replacement
already removes old symbols. Generic stale-symbol retention is **not** established.

Add source-bound coverage over the full admitted file universe, including empty
files and inherited base entries. Bind it to the existing lexical sealed manifest
and read view. Check effective pre-result scope, not the files of returned hits.
Preserve old immutable snapshot cursors. Generation CAS does not independently
prove source-event ordering. Text capability does not imply embedding-free build.

### E05/E06/E07: retain existing collection, clarify identity and name intent

`select:file` already uses exact best-hit grouped collection, not fixed overfetch
and dedup. It merges segment representatives before pagination. Do not implement
another collector or claim this existing behavior as a new ranker improvement.

`symbol.has.name(utils)` returns symbols from `utils.py` although the independent
AST finds zero declarations named `utils`. `DefaultPlaceholder` returns its class
plus three methods through container-name text. Existing public tests only prove
keyword alias behavior; they do not promise exact local-name equality. Define a
new explicit exact local/qualified policy over fields already present in producer
`SymbolRecord`, instead of treating all broad matches as old-contract bugs.

Federation is admitted through `source_repo_id`. B returns two same-path hits
under the physical pin's repo ID, then one row for file/repo projections. Decide
source versus containing-index identity explicitly. The proposed source-discovery
contract preserves both, groups by source owner and binds them into result/cursor
and mutation keys. A metadata facet alone cannot act as a complete source identity.

Resource issue: `GroupedPageCollector` accumulates all groups, while
`paging.rs:182–184` refuses over-cap work only afterwards. The final result is not
incorrectly marked complete, but the pre-materialization bound documented by
`core/domains/lexical/service.rs:13–24` is not enforced. Add a shared work/byte
ledger with early typed stop; preserve existing deadline/cancellation behavior.

### E08: fix semantic witnesses, not window-size guesses

`snippets.rs` gathers literals through NOT, does case-sensitive substring lookup,
and has no regex anchor. The current 240-byte owner probe yields:

| Case | Intended focus in returned window? | Wrong/absent indication |
| --- | --- | --- |
| needle query, distant NEEDLE | No | No highlight |
| distant regex needle[0-9]+ match | No | No anchor |
| needlework first, actual token needle later | No | Prefix inside needlework highlighted |
| NFC query café, decomposed source later | No | No highlight |
| NOT blocked OR allow; both words present | No | blocked highlighted instead of true allow branch |
| 200-byte raw literal under a 240-byte budget | No | Fixed leading context cuts the fitting focus |
| Ordinary distant needle control | Yes | Offset 120, valid highlight |

Retrieve bounded positive witnesses for selected candidates using existing
matching owners, then map normalized coordinates to original source intervals.
Preserve native NFC/lowercase/regex semantics; raw-byte matching is not the current
default. Treat path witnesses, synthetic symbol labels and source excerpts as
different things. Keep rank, indexed span, returned context and gold separate.

## 3. Final implementation order

1. **ENG-01 correctness:** one validated endpoint/domain/output plan before empty
   shortcuts; typed errors; rows plus authoritative count/continuation facts.
2. **ENG-02 ingress correctness:** canonical source-file mutation key, record-owner
   validation and collision/refusal tests before adding coverage fields.
3. **ENG-02/03 authority cutover:** source identity, sealed file universe/coverage,
   existing symbol fields retained in the index; snapshot/cursor compatibility
   is explicit. No second mutable registry or parallel internal IR.
4. **ENG-03/04 bounded execution:** reuse grouped collection, enforce work/bytes
   during execution, reconstruct semantic witnesses and correct original spans.
   Standalone renderer tests can start before the public schema cutover.
5. **Only then tune relevance:** corrected development gold, explicit definition
   policy and finite ablation, sealed holdout plus latency/index/update budgets.
   Correctness fixes do not wait for manual judgments or a large quality benchmark.

One integrator owns shared contract/SDK/schema changes. Independent engine test
fixtures, native evidence rejection and oracle work may run in parallel. Capture
new final-source evidence after integration; this audit's diagnostic receipts do
not certify the implementation.

## 4. 2026 reference check and rejected shortcuts

The re-audit checked GitHub's content/symbol distinction, Blackbird's indexed
candidate verification, Zoekt's current ranking/API contracts and SCIP position
encoding. These support typed source/match/result boundaries, not specific Quanta
weights. See [S01–S05/S10–S11](references.md).

Actual 2026 Sourcegraph announcements add query assistance (March30), asynchronous
Search Jobs (July27) and revision selection in Code Finder (August31). See
[P26-01–03](references.md). They are relevant upper-layer precedents, not evidence
that a new ANN, sparse-gram implementation or LLM reranker fixes these bugs.

Retain Tantivy, existing trigram/position verification, sealed generations and
grouped collection. Defer engine replacement, learned reranking, graph expansion,
SCIP compiler integration and new benchmark orchestration. Measure such changes
only if later profiles identify a remaining quality/cost bottleneck.

## 5. Reproduction and evidence custody

A commands, exact native requests and source/input identities:

```sh
python3 /private/tmp/qi-eng01-audit.WKhWU0Ay/probe.py
python3 /private/tmp/qi-eng01-audit.WKhWU0Ay/ipc_probe.py
python3 /private/tmp/qi-eng01-audit.WKhWU0Ay/oracle.py
```

These scripts retain their original external paths and require fresh state/output
for another capture. The direct daemon probe's first start lacked a retention
setting; that failure and corrected successful start are both retained. Query
errors are recorded individually; runner exit zero is not query success.

B: `/private/tmp/qi-eng02-audit.PMq86c/RECEIPT.md` contains the exact cached-library
compile/run command and exclusions; `source-and-binary-identities.txt` binds the
libraries/compiler. Supply a fresh state path, not its sealed `state-final`.
The adapter probe did not exercise the canonical body digest/public dispatcher.

C command (Cargo wrapper, no product dependencies rebuilt):

```sh
./scripts/cargow --lane test-integration-lane run \
  --manifest-path /private/tmp/qi-engine-final-audit.AAOVKM/Cargo.toml \
  --offline --target-dir /private/tmp/qi-engine-final-audit.AAOVKM/target
```

C directly includes the current source module. Its initial contract stub lacked
`PartialEq` and did not compile; correcting that stub left product code unchanged.
Final compiler: rustc1.92.0, aarch64-apple-darwin. Seven rows executed, one positive
control retained the focus and six negative cases exposed the owner behavior.

Rechecked SHA256:

| Artifact | Digest |
| --- | --- |
| A record.json | 7752b37af26315ac3ce9d80c2af3a60c9b58126069e00323cd52f1cdcd21d6f4 |
| A ipc-responses.json | fbff1a2775c0f2272bf51deaab6a0ca60cf0a0a26db7909ef216b96af36949cc |
| A oracle.json | 5bc30563902bfa19b1265497a898a8ecb674d6efbe6fffe00e84447c8f9463d9 |
| B probe.stdout | e4e41ad1a41c1c38d6e9aa1b6cb5a104baa7fa4720d1c5a111a9231edffad8e8 |
| C main.rs | 75fb1be7257ef073a1ca4052bd91608ca95885380d982ad0be0e51bd0e72828c |
| C snippet.log | 50651a3decf58029f275b7316bfc11a067f2cdf8d9aa1a6a327ba86e38bafc31 |

Temporary artifacts are evidence, not implementation dependencies. Missing or
changed artifacts block their historical replay claim. Recreate small permanent
regression fixtures under the real owners when implementation is authorized.
