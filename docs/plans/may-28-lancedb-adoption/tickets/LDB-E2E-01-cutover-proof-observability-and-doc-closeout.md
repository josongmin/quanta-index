# LDB-E2E-01 — Cutover Proof, Observability, and Doc Closeout

> Archive status: `Historical program record`. Current architecture: [MAY-31-001](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `done` (lancedb rewrite 2026-05-30; R1+R2+R3 hardening 2026-05-31)

## 0. Outcome

- **Cold-boot / restart proof** (lancedb-backed): `searchd-runtime` e2e
  `reopen_preserves_semantic_scope_ids_and_explanation` serves a sealed
  generation after restart from durable lancedb state (no replay);
  `restart_opens_prior_generation_without_replay` (semantic crate integration)
  proves a fresh adapter opens prior durable state.
- **Not-ready proofs**: `unsealed_generation_open_fails_closed`,
  `seed_skips_unsealed_generations`, `delta_with_unsealed_base_fails_closed`,
  `delta_with_missing_base_fails_closed`, `missing_lancedb_dataset_open_fails_
  closed`, `corrupt_manifest_open_fails_closed`,
  `manifest_row_count_mismatch_fails_closed` (real CBOR re-encode hitting the
  cross-check branch, not garbage-bytes covered by an adjacent test).
- **Migration proof**:
  `migration_imports_journal_idempotently_and_matches_clean_build` and
  `migration_resume_mid_generation_matches_clean_build` (semantic_boot tests)
  +`migrated_runtime_exposes_populated_semantic_boot_report` (runtime e2e).
- **Observability**: `SearchdRuntime.semantic_boot` (`SemanticBootReport`)
  exposes migration outcome, sealed-generation count, migration_micros,
  seed_micros — bounded enums/counts/duration only, asserted in both e2e
  tests (`fresh_runtime…` and `migrated_runtime…`).
- **Quality floor**: `ivf_hnsw_sq_index_built_at_seal_serves_vector_search`
  cross-checks `lancedb::Table::list_indices` to prove the IVF index actually
  exists (not just that a self-query happens to work under brute-force).
- **Boot-cost evidence**: a literal old-vs-new boot benchmark is not
  constructible — the replay path is removed, so there is no live "replay
  boot" to time against. The cost claim is therefore (a) structural —
  seeding is `O(sealed generations)` marker+manifest reads versus the former
  `O(all historical batches)` decode + rebuild — and (b) live-measured via
  `SemanticBootReport.{seed,migration}_micros`.
- **Doc closeout**: `README.md` semantic-backend lines describe the lancedb
  backend; prompt-manager source `buildctl.md` references migration scaffolding
  + durable backend (was claiming the removed `replay_into`); CLAUDE.md is
  regenerated (`pm.py sync`/`lint` ✓).
Parent: [../README.md](../README.md)
Depends on: [LDB-04-legacy-semantic-journal-migration.md](LDB-04-legacy-semantic-journal-migration.md)

## 1. Purpose

Prove that the Lance cutover is real, fail-closed, and reflected in the repo's
operator-facing docs.

## 2. Owner files

- `crates/quanta-index-searchd-runtime/tests/...`
- `crates/quanta-index-semantic/tests/...`
- benchmark/perf harnesses as needed
- prompt-manager source docs that feed generated `README.md` / `CLAUDE.md`

## 3. Proof obligations

- cold boot proof: semantic generations open from durable state without full
  replay
- restart proof: restarted daemon serves the same semantic results for a sealed
  generation
- not-ready proof: incomplete or manifest-mismatched persisted semantic state
  fails closed
- migration proof: legacy journal import yields the same semantic results as a
  clean durable build
- doc proof: generated operator docs stop claiming a shipped backend that the
  code does not implement

## 4. Observability requirements

- boot path must expose whether semantic generations were opened directly or a
  migration step ran
- bounded metrics/log labels only; no raw vectors, snippets, or unbounded path
  text in metric labels
- emit cold-boot/open timings so the replay-removal claim is measurable

## 5. Test and bench plan

- e2e restart test over real persisted semantic state
- e2e negative rows for missing marker, manifest mismatch, and incomplete
  generation
- migration equivalence test comparing legacy-imported results vs fresh durable
  build
- benchmark comparing old replay boot cost vs persisted-open boot cost on a
  representative corpus

## 6. DoD

- the cutover has executable proof, not only source inspection
- observability can distinguish durable open, migration, and typed failure
- generated docs no longer contradict the live semantic backend

## 7. Failure modes

- claiming replay removal without measuring the boot path
- updating generated docs before proof exists
- letting observability leak unbounded semantic payloads
