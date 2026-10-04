# 2026-10-04 agent-1 handoff — retrieval, engine audit, documentation

Status: session handoff, **not** a product, performance, or release qualification
receipt. Recheck HEAD, dirty state, inputs, binaries, and external artifacts
before resuming. This file records what this chat investigated, what reached
local `main`, and what remains open; active tickets and current source retain
authority over this snapshot.

## 1. Current checkout and ownership boundary

- Checkout: `/Users/songmin/Documents/code-new/quanta-index`, branch `main`,
  HEAD `2062fed3ff86287a0914ece99dc2fe0af829c29b` at handoff.
- Local `origin/main` was `e43cda8c87b4a06fecac82a266011e30f84a2986`;
  `git rev-list --left-right --count origin/main...HEAD` returned `0 2`.
  This was a local-ref comparison, **not** a fresh remote fetch. The two local
  commits are `8ee2f1ea` and `2062fed3`; neither was pushed in the latest
  documentation turns.
- Before this handoff file, the shared main index already had **76 staged
  paths**, zero unstaged paths, and zero other untracked paths. The staged
  overlay spans benchmark code/tickets, lexical/ingest observations, CI,
  dependencies, and vendored JavaScript/Unicode data. It belongs to concurrent
  work and was neither reviewed as a unit nor committed by this handoff.
  Saving this file added one staged path; status then showed 77 staged paths,
  zero unstaged and zero untracked. Do not use `git add -A` or interpret a
  clean committed HEAD as a clean working tree.
- The latest doc work was prepared in an isolated managed worktree, committed,
  fast-forwarded into local main, and that worktree was archived. Original Gin
  captures and shared staged changes were not edited by it.

## 2. Session scope and completed findings

### Frozen Gin semantic-only 300-query RCA

The original diagnostic is bound to clean
`quanta-index@c64f6af5d5e2817e346336ceee0e57f5fb25d74f` and
`gin@d3ffc9985281dcf4d3bef604cce4e662b1a327a6`: 99 `code_only`
files, universe SHA-256
`d4e1ea025c067f344af640568b5bfd0835ed09c2bfbcf9668b8e9782bc68bd67`.
The original searchd SHA-256 was
`a82e8f841b48fc9fd2a446eada13ae4816402a48f6842c2af438e32610ccc234`.
It used 300 generated bare-symbol queries, Quanta 4096-byte fixed windows with
256-byte overlap, and native top 10. Quanta found the generated gold file in
266/300 responses; Semble found it in 277/300. Misses were Quanta 34, Semble
23, shared 11, union 46. All 600 search responses succeeded. Quanta's 300
`capped` statuses indicate bounded top-k/continuation, not execution errors.

- The 46-row source audit checked gold file and byte hashes and declaration
  locations. Every Quanta gold definition is in its 240 indexed chunks; every
  Semble gold identifier occurs in its reconstructed 1,171 chunks. Six Semble
  full gold spans cross a native chunk boundary. The original Quanta trace
  used exact-vector search because 240 rows were below its ANN threshold.
  Thus corpus omission, Quanta ANN recall, and budget interruption do not
  explain this frozen set of misses.
- The generated label is a **mechanically valid target**, not an independent
  semantic relevance judgment. `S027=Param` has another declaration/type
  target; several names have many call sites. Bare-name intent, dense ranking,
  native chunk granularity, and file versus definition ranking need separate
  treatment. Chunk NDCG includes gold-byte density and is not the cause of a
  top-10 file miss by itself.
- Isolated controls found deeper ranks for selected cases and showed mixed
  effects from context queries, file grouping, chunk size, and encoder policy.
  Quanta's historical V1 encoder effectively retained a 512-tokenizer-ID cap.
  Opt-in full-input V2 reached **268/300** file hits on fresh 99-file native
  states: 13 V1 misses recovered, 11 new misses. It is not a qualified default
  change. The distinct exact-symbol declaration route found the mechanically
  identified definition span for **300/300** names on its source-bound
  control; this does not improve the semantic-only score.
- The later fresh native V1/V2 diagnostic at `main@938251d2` reproduced the
  earlier ordered top-10 rows for all 300 queries per mode. It kept the same
  266/268 file-hit counts. This is one generated-gold diagnostic, not a
  reviewed product-quality or latency result.

Primary evidence:

- `/Users/songmin/Documents/code-new/qi-gin-300-rca-20260929/FINAL_RCA.md`
  and `semantic-46-task-appendix.md` / `semantic-46-task-details.json`:
  per-task gold, candidates, ranks, hashes, labels, controls, and limitations.
- `/Users/songmin/Documents/code-new/qi-gin-quality-current-20260929-938251d2/RESULTS.md`
  and its V1/V2 records: later native reproducibility control.
- [Product-quality owner](../../plans/jun-7-search-product-quality/README.md)
  and [retrieval guide](../../../tools/benchmark/retrieval/README.md).
- The historical `/private/tmp/g4` receipt was cleared by a host reboot.
  Read the matching immutable `native-tree.zip` under
  `/Users/songmin/Documents/code-new/qi-smoke-gin-300-20260928/semantic-evidence/runs/`
  or the hash-checked convenience copies in the RCA root. Do not assume the
  old `/private/tmp/g4` path currently exists, and do not overwrite the
  archive, original captures, or regenerated controls.

### Lexical, corpus, and benchmark distinction

- The same Gin source-exposed exact-name file-mode diagnostic reported
  Quanta `keyword_file` **1,192/1,196** and Semble `lexical-file`
  **1,190/1,196**. This is a separate query route and score unit from semantic
  chunks; it is not an independent unseen holdout.
- The original 300-query semantic, lexical, and hybrid modes share task/query/
  gold projection but have different search paths. Their totals must not be
  combined. Exact declaration lookup, identifier robustness, semantic intent,
  and repository workflow retrieval need separate scoreboards.
- The current benchmark owner is [S30-B01–B09](../../plans/sep-30-code-search-benchmark-trust/tickets/INDEX.md).
  Current staged B07/B08/B09 edits and their external receipts belong to
  parallel work; this handoff did not rerun, approve, or merge their result.
  B07 records query/IPC and indexing phase instrumentation, but the 8-repository
  matrix's Quanta/Semble indexing envelopes (`352.44 s` / `37.26 s`) were on
  unequal chunk workloads and a contended host. They are cost-location
  diagnostics, **not** an equal-work speed verdict. The 12-repository C5
  matrix rejected four stale mechanical suite/exclusion inputs before a full
  run; eight eligible repositories produced a separate 32-suite diagnostic.
  B08 supplemental review, qrels, external indexed-universe admission, and
  all-product decision remain open at this snapshot. Follow the ticket's
  latest actual terminal receipts rather than this abbreviated status.

### Engine and source audit

The current route is SDK ingest over UDS → search-plane idempotent publish →
Tantivy lexical/Lance semantic generation materialization → separate
lexical+semantic activation CAS → searchd query. A generation is not active
merely because publish or seal completed. Lexical ingest already uses a
buffered Tantivy writer with synchronous commit and seal-time merge wait;
semantic ingest uses bounded windows and a staged Lance dataset. Therefore
the old claim that every file is immediately flushed directly to disk is not
an accurate description of current code.

Code-level risks recorded in [OCT-04-001](../../adr/OCT-04-001-search-corpus-selection-and-ingest-pressure.md):

1. Un-tokened `Active` resolution in
   `query_dispatcher/selection.rs` precedes
   `query_dispatcher/read_view/view.rs::acquire_read_view` without an admission
   pin spanning the interval. An acquired read view retains handles, but that
   proves a later lifetime boundary. The proposed select-G1 / activate-G2 /
   retain-G3 / acquire-G1 interleaving is **not yet reproduced**.
2. Search-corpus ingest has one process-wide serial dispatch slot; query
   admission is separate. The SDK default I/O deadline is 30 s and ingest
   dispatch budget 120 s. Peer hang-up cancellation exists, but a previously
   admitted publish deliberately settles durably. Timeout is not rollback;
   operation-identity inspection/replay needs a controlled test.
3. Maintenance boot/tick walks track roots for logical disk-byte gauges in the
   same cadence as backend freshness. A deliberately slow walk and query/
   readiness impact have not been measured. Logical retained bytes are not a
   physical allocation or transient merge high-water quota.

The two related existing search-plane tests passed individually on clean
`8ee2f1ea`: `sealed_search_corpus_history_reaps_max_plus_one_and_preserves_predecessor`
and `query_plane_resolves_only_catalog_active_generation` (one test each,
534 filtered each). They confirm component behavior, **not** the combined
interleaving. No P04 code fix was made.

### Current-source documentation cleanup on local main

- Commit `8ee2f1ea`: removed seven outdated engine/config/SDK proposal
  documents, preserved recovery paths, and reduced still-open decisions to
  three explicitly **Proposed** ADRs:
  [OCT-04-001](../../adr/OCT-04-001-search-corpus-selection-and-ingest-pressure.md),
  [OCT-04-002](../../adr/OCT-04-002-configuration-and-generation-policy.md),
  [OCT-04-003](../../adr/OCT-04-003-source-preparation-sdk.md).
  Updated [engine status](../../ssot/engine-status-v1.md) and indexes.
  These proposals are not implementation authorization or runtime proof.
- Commit `2062fed3`: removed the dated `expect` call-count snapshot from live
  SSOT, retained Git recovery in the [history index](../../ARCHIVE-INDEX.md),
  and removed stale static size counts from
  [crate ownership](../../ssot/crate-ownership.md). For example the old
  `code_search.rs` `.expect(` line count was 93 versus 97 in selected source;
  search-plane source lines were documented as 68,241 versus 69,076.
- Verification for this doc range: changed-link path check, prompt-manager
  `lint` (`5/5 targets in sync`), Git preimage recovery, and committed-range
  `git diff --check` passed. No Rust or release qualification follows from
  document checks.

## 3. Remaining work and decision order

| Priority | Owner / next decisive action | Current boundary |
| --- | --- | --- |
| P0 | Freeze the exact source/overlay owner before any new proof. Reconcile the 76 pre-existing staged paths by owner; do not stage or commit them as this handoff's work. Recheck local/remote divergence before publication. | Current main is dirty; local docs commits are not freshly remote-verified. |
| P0 | [P04 read-view](../../plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md): add a deterministic G1→G2/G3 selection/retention/acquisition interleaving in an isolated checkout. If baseline refuses, design the shortest catalog/retention admission pin, transfer it to the acquired view, and separately test tokened and explicit-pin refusal. | `NOT_RUN` combined race reproduction; no product fix. |
| P0 | Semantic quality: independently author natural-language intents, freeze a holdout, obtain independent file/declaration judgments and ambiguity/no-answer labels, and predeclare quality/resource acceptance. Keep original 300 generated bare-name rows immutable. | V1/V2/Semble Gin scores are diagnostic only; no qualified default V2 decision. |
| P1 | [B08](../../plans/sep-30-code-search-benchmark-trust/tickets/S30-B08-fresh-multirepo-holdout.md): finish actual supplemental reviews/adjudication, reissue four stale C5 suite inputs under new roots, bind external indexed universes, replay qrels/reports, then score the full admitted cohort. | Current staged controller/preflight repairs are not final reviews or all-product qualification. |
| P1 | [B07](../../plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md): after correctness/source admission, measure same API/request boundaries and full/delta/delete/reopen indexing phases on a quiet host with exact binaries, chunk counts, RSS and disk high water. | Current timings are contended diagnostics with unequal work. |
| P1 | [P09 process](../../plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md): script a full-tree disk walk longer than three maintenance cadences; test readiness freshness independently. Test client timeout during admitted slow publish and exact replay. | Slow-walk, timeout/replay, physical-pressure and power-loss proof `NOT_RUN`. |
| P2 | Decide whether operators need [effective config/generation policy](../../adr/OCT-04-002-configuration-and-generation-policy.md) and whether a producer fixture justifies the [source-preparation SDK](../../adr/OCT-04-003-source-preparation-sdk.md). Do not implement from the superseded RFCs. | Both ADRs remain Proposed; APIs, migration and cross-producer proof `NOT_RUN`. |
| Release | Follow [SEP-21 residual execution](../../plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md): exact-source proof authority, Linux/release binaries, real provider and paired producer, deploy/activate/rollback receipts. | Focused local tests and benchmark completion do not imply `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED` or `ROLLBACK_PROVEN`. |

Stop conditions: do not edit the frozen Gin capture or old `/private/tmp/g4`;
do not merge changed-query/model/chunk controls into original 300 scores;
do not change ranking, ANN, chunking, or ingest concurrency based on the
unreviewed single-repository diagnostic. If a source/input/binary boundary
changes, start a new external output root and report it separately.

## 4. Verification ledger for this handoff

| Scope | Verdict | Evidence and limit |
| --- | --- | --- |
| Original Gin 300 and 46-task RCA | `VERIFIED` for the frozen source/corpus/capture | External `FINAL_RCA.md`, 46-row appendix, archived native report/verdict; no current-main product qualification. |
| Fresh Gin V1/V2 reproducibility | `VERIFIED` for the cited `938251d2` diagnostic | External `RESULTS.md`: 300/300 ordered-prefix match per mode, 266/268 file hits; no independent relevance or quiet-host performance. |
| Read-view selection and retention components | `VERIFIED` at clean `8ee2f1ea` | Two focused `./scripts/cargow --lane read-view-race-audit-lane test -p quanta-index-search-plane --lib --all-features --locked <test-name>` invocations, one pass each. |
| Combined G1/G2/G3 race, slow maintenance, timeout/replay, physical disk pressure | `NOT_RUN` | Static path and component tests are insufficient to claim an observed request failure or repaired behavior. |
| Local documentation commits | `VERIFIED` for doc hygiene and local-main fast-forward | `8ee2f1ea`, `2062fed3`; links, `pm.py lint`, committed-range diff check. No runtime gate. |
| Entire shared staged overlay | `NOT_RUN` as an integrated build/test/review | 76 staged paths were concurrent work. `git diff --cached --check` currently **FAILED** on pre-existing trailing whitespace at `vendor/tree-sitter-javascript/quanta-compatibility.patch:5`; this is outside the documentation commit. |
| Human-reviewed semantic quality, equal-boundary speed, release/host qualification | `NOT_RUN` | Follow product-quality, B07/B08, and SEP-21 owners; never infer these verdicts from the 300 diagnostic. |

Useful read-only resumption commands:

```sh
git -C /Users/songmin/Documents/code-new/quanta-index status --short
git -C /Users/songmin/Documents/code-new/quanta-index rev-parse HEAD
git -C /Users/songmin/Documents/code-new/quanta-index rev-list --left-right --count origin/main...HEAD
git -C /Users/songmin/Documents/code-new/quanta-index show --stat --oneline 8ee2f1ea
git -C /Users/songmin/Documents/code-new/quanta-index show --stat --oneline 2062fed3
```

For new experiments, first select a clean source revision, record the exact
suite/model/corpus/binary identities, and use a fresh external output root.
Run only the narrow owner-local rail needed to decide the concrete hypothesis;
full rebench and result promotion require their separate admission contracts.
