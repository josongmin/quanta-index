# Search-plane engine status

Authority: **code**. Re-audited at clean `main@e43cda8c` on 2026-10-04.
The SEP-21 residual ledger owns host/release evidence; a separate
[proposal](../adr/OCT-04-001-search-corpus-selection-and-ingest-pressure.md)
records code-level selection and resource risks that still require controlled
reproduction. Neither document is a current-source execution receipt.

## Live path

1. SDK ingest → UDS → `SearchPlaneIngestDispatcher::dispatch` →
   `publish_idempotent` (digest → fenced claim → apply → commit).
2. Lexical materialize: Tantivy `LexicalAdapter`. Semantic: Lance
   `SemanticAdapter` under `{state_root}/indexes/semantic/...`.
3. Activation is **not** publish: `ActivateSearchCorpusGenerationCas` /
   `activate_prepared_v1` (lexical+semantic pair CAS). Rollback is the
   matching CAS.
4. Serve: `searchd` `drive` binds query/control/ingest sockets.
   `ipc_dispatcher` maps query 14 / control 12 / ingest 12 variants; no
   unimplemented match arm.
5. Query: lexical Tantivy planner; semantic Lance exact/ANN per sealed policy;
   hybrid RRF
   (`dispatch_hybrid`).

Catalog `open` recovers unfinished journal rows and stale mutation leases.
Snapshot fence / `StillReferenced` is transactional.

## Default semantic quality

`SEARCH_OWNED_SEMANTIC_MODEL_ID = "search-owned-hash-text-v1"` (FNV 64-d).
PotionCode and OpenAI embedders exist behind profiles + egress grant. Hash
tests do not qualify a neural provider.

## Typed holes (code, not residual prose)

| Hole | Symbol |
| --- | --- |
| Lexical structural leaf | `LexicalPlannerError::Unimplemented { node: "structural_block_leaf" }` |
| Some history `LqFilter`/`LqLeaf` | `CoreError::NotImplemented` |
| `LangId::Rust` grammar | still deferred (LEX-05 comment on the enum) |
| Legacy v1 catalog tables | refuse without migration |

Structural ingest/query routes and `TruthfulSubsetAuthorityMatcher` are live
subset authority, not a missing producer crate.

## Static risks requiring controlled proof

- Un-tokened `Active` selection precedes read-view acquisition without an
  admission pin spanning both. Acquired views retain handles; the proposed
  three-generation transition counterexample has not been executed.
- Maintenance boot and each tick perform full-tree disk-byte refreshes in the
  same cadence as backend freshness. Slow-root impact has not been measured.
- The SDK default I/O deadline is shorter than the ingest dispatch budget.
  Peer hang-up now cancels a budget, but an admitted publish settles durably;
  test timeout plus replay before claiming rollback or exactly-once delivery.

Current ingest observations include lexical substage and semantic phase timing.
Do not use the Sep-30 RFC's older one-field lexical timing description.

## What CURRENT-RESIDUAL still means

R0–R6 / P03–P12 remaining rows are Linux host, release-binary, real-provider
identity, paired-run receipts, and ops deploy/activate/rollback **evidence**.
They do not mean IPC handlers or CAS activate are unimplemented.
`CODE_QUALIFIED` is a qualification verdict, not a compile verdict.
