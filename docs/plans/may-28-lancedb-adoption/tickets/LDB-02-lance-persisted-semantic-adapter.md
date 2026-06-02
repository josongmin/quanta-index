# LDB-02 — Lance Persisted Semantic Adapter

Status: `done` (lancedb rewrite 2026-05-30; R1+R2+R3 hardening 2026-05-31)

## 0. Outcome

The semantic adapter is the **real `lancedb` 0.30** integration
(`quanta-index-semantic`), not the in-house CBOR+HNSW from §3.1 (those files
are deleted). Modules:

- `lib.rs` — `SemanticAdapter` owns a `tokio::runtime::Runtime`; the single
  `run_blocking` helper (one `#[expect(clippy::disallowed_methods)]`) is the
  whole-crate async↔sync seam between the sync port surface and lancedb's
  async API.
- `build.rs` — `build_batch` validates every scope **before** any destructive
  delete (R1 #1); `prepare_generation_dir` clones a delta base via crash-
  atomic stage-then-rename **only** when the base carries `MARKER_SEALED`
  (R1 #2 + R2 staging); `ensure_table` cross-checks the existing-table
  FixedSizeList dimension against the batch contract dim (R3); `seal_generation`
  builds the IVF_HNSW_SQ ANN index at seal when `row_count >= 256` (SOTA++).
- `search.rs` — `open_generation` validates the manifest scope + distance
  metric (R3) + row count vs lancedb's live count; `vector_search` runs with
  `DistanceType::Cosine` and converts lancedb distance to historical cosine
  similarity in the candidate score field; `search_scoped` uses lancedb's
  `only_if` with a SQL-escaped IN filter from `sql.rs`.
- `sql.rs` — shared SQL-escape helpers with unit-tested SQL-injection
  resistance.

Per LDB-00 §3.2, vendor + tokio runtime are localized to this crate. Tests
under `tests/persisted_semantic.rs` cover durable roundtrip, restart, replace/
tombstone, generation-pin isolation, sealed-empty, contract + query dimension
mismatch, manifest corruption, manifest row-count cross-check, IVF index
existence at seal (via `lancedb::Table::list_indices`), unsealed-base delta
fail-closed, validate-before-delete preserves prior rows, missing-lancedb-
dataset open fail-closed, search_scoped allowlist filter, concurrent open,
delta-with-missing-base fail-closed, unsupported distance metric rejection.
Parent: [../README.md](../README.md)
Depends on: [LDB-01-semantic-generation-layout-and-manifest-contract.md](LDB-01-semantic-generation-layout-and-manifest-contract.md)

## 1. Purpose

Replace the current disposable in-memory semantic builder/opener with a
persisted Lance-backed adapter inside `quanta-index-semantic`.

## 2. Owner files

- `crates/quanta-index-semantic/Cargo.toml`
- `crates/quanta-index-semantic/src/lib.rs`
- new adapter-local modules as needed, for example:
  - `dataset.rs`
  - `manifest.rs`
  - `search.rs`
  - `build.rs`
- `crates/quanta-index-semantic/tests/...`

## 3. Required behavior

- `build_batch(&SemanticIngestBatch)` writes durable generation-local semantic
  state
- `open(repo, revision, generation)` opens that persisted state directly and
  returns a semantic searcher without warm-up replay
- query results still surface as `LexicalCandidate`
- replace/tombstone semantics remain generation-local and fail closed on
  manifest or layout mismatch

Important rule:

- vendor tokens, async bridging, dataset handles, and file layout knowledge stay
  inside `quanta-index-semantic`

## 4. Work items

- add the Lance-family dependency chosen by `LDB-00`
- implement durable row writes from `SemanticIngestBatch`
- implement ANN/open path for a sealed generation
- preserve the current semantic policy checks:
  - top-k validation
  - query vector validation
  - dimension mismatch fail-closed behavior
- keep sealed-empty generation semantics explicit

## 5. Test plan

- build/open roundtrip against a real persisted semantic generation
- restart test: a fresh adapter instance can open prior persisted generations
- tombstone test: deleted embeddings no longer surface in search
- generation-isolation test: `(repo, revision, generation)` pin remains strict
- manifest-mismatch test: open fails closed

## 6. DoD

- semantic adapter no longer needs a replayed RAM graph to answer queries
- persisted open path is real and covered by tests
- vendor-specific code remains localized to the semantic adapter crate

## 7. Failure modes

- wrapping a persisted dataset with another in-memory rebuild layer
- letting query behavior depend on prior-process RAM state
- leaking async/vendor handles above the adapter boundary
