# Jun 2 DSL Hardening

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-08-001](../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


Status: `closed`
Date: `2026-06-02`
Scope: post-closeout hardening for already-executable DSL surfaces

This packet starts **after** [../jun-2-dsl-final-cut/README.md](../jun-2-dsl-final-cut/README.md).
It does not reopen the closeout verdict. It owns correctness, fail-closed
behavior, and execution-cost hardening on surfaces that already ship.

It also sits **beside** [../jun-2-dsl-advanced/README.md](../jun-2-dsl-advanced/README.md).
`jun-2-dsl-advanced` owns optional widening. This packet owns shipped-surface
hardening only.

Benchmarking adopted path for shipped surfaces lives in
[RFC-DSL-Benchmarking.md](RFC-DSL-Benchmarking.md).

---

## 1. Scope Lock

This packet owns only three hardening families:

- runtime catalog ingest integrity:
  authoritative replacement, monotonic overlay ordering, persisted batch digest,
  referential validation
- runtime metadata semantics and execution:
  real `stale:` freshness semantics, explicit `dirty:` semantics,
  runtime-only set-driven pushdown
- shipped predicate proof symmetry:
  `repo.has.file(name:...)` and `file.contains(...)` sibling proof completion

Explicitly excluded:

- new DSL widening or “advanced” claims
- bridge-packet carrier changes
- history-route admission issues outside the shipped runtime/predicate seams
- semantic / hybrid product redesign outside the current owner rails

## 2. Current Source Truth

- `RuntimeCatalogIngestBatch` ordering fields are now persisted and enforced in
  `RuntimeMetadataState`
- runtime catalog apply is now authoritative replacement for
  `changed_docs`, `doc_facets`, `snapshots`, `affected_docs`, and
  `invalidated_by_docs`
- runtime catalog ingest now rejects unknown `doc_id` values against the pinned
  generation chunk universe before state mutation
- runtime metadata query now derives direct catalog seeds instead of full
  structural chunk scans for runtime-only filters
- `stale:` now requires `producer_head_applied_at_ms >
  generation_materialized_at_ms` plus the user `before=` bound
- `dirty:only` no longer aliases `dirty:yes`; it now typed-rejects
  `RUNTIME_DIRTY_ONLY_UNSUPPORTED` while `dirty:yes` and `dirty:no` stay
  executable
- shipped predicate proof is now sibling-complete for the current subset:
  `repo.has.file(...)` covers both `path:` and `name:` rails, and
  `file.contains(...)` covers raw hit plus phrase hit/miss

## 2.1 Current Code Pointers

- closeout packet truth:
  `docs/plans/jun-2-dsl-final-cut/README.md`
- widening-only packet this plan must not blur with shipped-surface hardening:
  `docs/plans/jun-2-dsl-advanced/README.md`
- runtime catalog ingest/state seam:
  `crates/quanta-index-search-plane/src/readiness.rs`
  `apply_runtime_catalog_batch`, `RuntimeMetadataState`
- runtime catalog contract seam:
  `crates/quanta-index-contract/src/ipc/ingest.rs`
  `RuntimeCatalogIngestBatch`
- runtime catalog publish seam:
  `crates/quanta-index-search-plane/src/ingest_dispatcher.rs`
  `publish_catalog_batch`
- runtime metadata execution seam:
  `crates/quanta-index-search-plane/src/query_dispatcher.rs`
  `execute_runtime_metadata_query`, `runtime_chunk_matches`,
  `runtime_edge_matches`, `parse_runtime_stale_scope_ms`
- predicate execution seam:
  `crates/quanta-index-lexical/src/lib.rs`
  `repo_has_file_constraint`, `predicate_content_leaf`
- current proof rails to extend:
  `crates/quanta-index-lexical/tests/tantivy_smoke.rs`
  `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
  `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`
  `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## 3. Ticket Lanes

| ticket | status | concrete first increment | parallel class | red rail first |
| --- | --- | --- | --- | --- |
| [DH-00](tickets/DH-00-scope-lock-and-seam-map.md) | landed | keep packet truth aligned with landed code | serial | packet/doc truth spot-check |
| [DH-01](tickets/DH-01-runtime-catalog-integrity-and-replay-guard.md) | landed | state contract + replay/replacement unit red | lane A | readiness owner-local unit rail |
| [DH-02](tickets/DH-02-runtime-metadata-semantics-and-pushdown.md) | landed | semantics rail first, then seed planner | lane B after DH-01 state freeze | query-dispatch owner-local unit rail |
| [DH-03](tickets/DH-03-predicate-proof-symmetry.md) | landed | add missing sibling proof only | lane C in parallel | `tantivy_smoke` |

## 4. Sequencing

Execution order:

1. `DH-00` scope lock and seam map
2. `DH-01` runtime catalog state contract freeze
3. `DH-03` predicate proof symmetry in parallel with `DH-01`
4. `DH-02` runtime metadata semantics and pushdown after `DH-01` lands the
   state contract
5. packet/doc truth sync after the owning rails are green

Parallelization rules:

- `DH-01` and `DH-03` may run in parallel because one changes runtime catalog
  authority while the other changes proof inventory only
- `DH-02` may prepare fixtures and unit rails in parallel, but it must not land
  against the pre-`DH-01` state layout
- no ticket may widen DSL surface area; if a fix requires new syntax or new
  predicate families, it leaves this packet and moves to `jun-2-dsl-advanced`

## 5. Program-Level Red Rails First

- packet scope/truth spot-check:
  `rg -n "hardening|advanced|closeout|dirty:only|stale:|batch_digest|overlay_epoch_ms" docs/plans/jun-2-dsl-hardening docs/plans/jun-2-dsl-final-cut docs/plans/jun-2-dsl-advanced`
- runtime catalog authority seam:
  add the targeted owner-local rail first, then run
  `./scripts/cargow test -p quanta-index-search-plane --lib -- --nocapture`
- runtime metadata execution seam:
  add the targeted owner-local rail first, then run
  `./scripts/cargow test -p quanta-index-search-plane --lib -- --nocapture`
- predicate proof seam:
  `./scripts/cargow test -p quanta-index-lexical --test tantivy_smoke -- --nocapture`

## 6. Program-Level Deliverables

This packet is not complete until it ships all of these:

1. authoritative replacement semantics for runtime catalog ingest
2. replay / stale-batch rejection with persisted ordering evidence
3. referentially validated catalog ingest
4. non-alias `dirty:` semantics with explicit proof (`dirty:yes` / `dirty:no`
   executable, `dirty:only` typed reject)
5. `stale:` semantics that actually consume producer-head freshness
6. set-driven runtime-only execution for the direct catalog filters
7. sibling-complete proof for the shipped predicate subset

## 7. Program-Level DoD

This packet is complete only when all are true:

1. runtime catalog batches are applied as authoritative snapshots, not additive
   merges
2. older or conflicting replay batches typed-fail instead of silently mutating
   state
3. malformed runtime catalog batches with unknown `doc_id` values typed-fail
4. `stale:` semantics require a real producer-head-vs-generation freshness
   relation
5. `dirty:only` no longer aliases `dirty:yes`
6. runtime-only catalog filters use direct catalog seeds where the state already
   owns a deterministic seed set
7. shipped predicate subset proof covers every executable sibling branch that
   the docs claim as supported
8. packet/docs/proof inventory describe these as hardening on shipped surfaces,
   not advanced widening

## 8. Non-Goals

- no bridge-carrier runtime-row promotion
- no new predicate names or argument-shape widening
- no history API redesign
- no request-time repair or fallback logic for malformed producer input
