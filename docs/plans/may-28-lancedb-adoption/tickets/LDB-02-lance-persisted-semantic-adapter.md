# LDB-02 — Lance Persisted Semantic Adapter

Status: `done` (2026-05-29)
Parent: [../README.md](../README.md)
Depends on: [LDB-01-semantic-generation-layout-and-manifest-contract.md](LDB-01-semantic-generation-layout-and-manifest-contract.md)

## 0. Outcome

`SemanticAdapter` rewritten off the in-memory-only store onto durable,
generation-scoped persistence (`build.rs` writes rows + READY per batch, builds
the HNSW graph once and writes manifest + SEALED on seal; `search.rs` opens a
sealed generation directly — manifest/shape/checksum validated — and loads the
persisted graph rather than rebuilding it; `graph.rs` is the HNSW CBOR codec).
Vendor/layout knowledge stays inside the crate. `tests/persisted_semantic.rs`
covers build/open roundtrip, restart open by a fresh adapter, replace/tombstone,
generation-pin isolation, contract + query dimension mismatch, and corruption
fail-closed. Per LDB-00 the durable backend is in-house, not the `lance` crate.

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
