# Search-Plane Implementation Tickets

> **Status:** TDD-ready decomposition of [SSOT plan](../ssot/may-23-storage-architecture-endgame-implementation.md) into atomic tickets.
> **Authority:** SSOT is canonical. This document expands SSOT phases into actionable units. Conflicts → SSOT wins.
> **Posture:** breaking-first (no shims), fail-closed, hexagonal boundary enforced by [lint-hexagonal-boundaries.py](../../tools/ci/lint/lint-hexagonal-boundaries.py).

## Purpose

- atomic TDD units: each ticket = one **failing-test-first** cycle
- no heuristic / placeholder paths on production codepaths
- contract is **frozen** — tickets implement against existing DTO shapes; no contract edits
- every ticket independently passes `just verify` + `just rust-hexagonal`

## Cross-cutting rules

### TDD discipline (every ticket, no exceptions)

1. Write failing test(s) FIRST. Run, observe red.
2. Minimum implementation that flips green.
3. Refactor with full workspace lint rail (`unwrap_used`, `panic`, `indexing_slicing`, `arithmetic_side_effects`, `as_conversions` etc. all denied) still green.
4. Add `proptest` invariants if the change introduces or extends a policy/validator.
5. Add `criterion` bench if the change touches a hot path (validators, query dispatch, codec).

### Fail-closed posture

- No `Result::ok()`, no `unwrap_or*`, no silent default replacement on production paths ([clippy.toml](../../clippy.toml) `disallowed-methods`).
- No empty-result fallback when authoritative source is absent (`CoreError::NotReady` instead of empty `Vec`).
- No heuristic success path when an authoritative path is missing ([CLAUDE.md](../../CLAUDE.md) "Agent change posture").

### Hexagonal boundary

- `quanta-index-contract` = DTO only (no `pub trait`, no vendor types).
- `quanta-index-core::domains::*` = port traits + policies; no `rusqlite`, no `tantivy`, no `lance`, no FS layout knowledge.
- Driven adapters impl outbound ports only; no domain policy duplication.
- `quanta-index-searchd::app` is the **only** cross-domain orchestrator.
- Domain isolation: `domains::A` must not `use crate::domains::B`. Enforced by [lint-hexagonal-boundaries.py](../../tools/ci/lint/lint-hexagonal-boundaries.py).

### Per-ticket acceptance checklist

- [ ] failing test(s) committed first
- [ ] implementation flips them green
- [ ] property invariant added if policy/validator extended
- [ ] criterion bench added/updated if hot path touched
- [ ] `just verify` green
- [ ] `just rust-hexagonal` green
- [ ] `cargo machete` green (no unused workspace deps)
- [ ] no `#[allow]` (use `#[expect(..., reason = "...")]`)
- [ ] [prompt-manager source](../../tools/prompt-manager/sources/) updated if user-visible rail changed; `pm.py sync` run

### Crate naming convention

Crate name = **purpose / boundary**, never **backend / transport**. Vendor choice (SQLite, Tantivy, Lance, UDS, …) is an internal implementation detail and may change without renaming the crate.

| Crate | Purpose | Current backend (internal) |
|-------|---------|----------------------------|
| `quanta-index-control` | control-plane state | SQLite (rusqlite) |
| `quanta-index-lexical` | lexical index build/open | Tantivy (planned) |
| `quanta-index-semantic` | semantic / vector index | Lance (planned) |
| `quanta-index-ipc` | search-plane IPC transport | UDS + CBOR framing (planned) |

Types **inside** an adapter crate MAY reference the vendor (e.g. `TantivyLexicalAdapter`), but the crate's public boundary stays purpose-shaped.

**Bundle byte 모델 (D17):** producer가 디스크에 떨궈둔 bundle payload를 search-plane은 `read + sha256 verify + forget`만 한다. 별도 artifact-store/loader 크레이트 **없음**. read+verify 유틸은 `searchd::app::materialize` 안에 inline (~50줄). retention 0 — built index들만 search-plane state_root에 남는다.

**Wire/serialization 모델 (D18):** `#[derive(serde::Serialize)]` / `#[derive(serde::Deserialize)]` proc-macro derive 금지 (workspace 전체). 모든 `Serialize` / `Deserialize` impl은 수동(`impl serde::Serialize for T`, `impl<'de> serde::Deserialize<'de> for T`)으로 작성. 이유: derive expansion이 cold-build 시간 지배 + wire shape 리뷰 가능. semgrep `rust-no-serde-derive` rule이 강제.

### Scenario taxonomy

From SSOT § Scenario Matrix. Every ticket lists which scenarios it covers.

| Code | Class | Examples |
|------|-------|----------|
| `U-SP*` | usecase (happy path) | producer prepare, activate→materialize→query, cold restart, concurrent prepare |
| `E-SP*` | edge | duplicate prepare, stale activation, invalid manifest refs |
| `C-SP*` | corner | no active gen, partial readiness |
| `H-SP*` | hellgate (must-fail-closed) | activate-before-materialize, query on unready gen, UDS framing error |

---

## Epic index

| Epic | Phase | Scope | Tickets | Parallelism |
|------|-------|-------|---------|-------------|
| **E1** | Phase 1 | control adapter completion | T1.1–T1.4 | T1.1/T1.2/T1.3 parallel; T1.4 after T1.2 |
| **E3** | Phase 2 | materialization adapters | T3.1–T3.5 | T3.1/T3.2/T3.3/T3.4 parallel; T3.5 last |
| **E4** | Phase 3 | query engine + UDS transport | T4.1–T4.5 | T4.3 parallel with T4.1/T4.2; T4.4 after both |
| **E5** | cross-cutting | scenario matrix proofs | T5.1–T5.2 | sequential, last |

(E2 dropped per D17. bundle byte read+verify is a `searchd::app::materialize` inline utility, not a crate.)

Epic-level dependency:

```text
E1 ──► E3 ──► E4 ──► E5
```

---

## Epic E1 — Control adapter completion

**Goal:** every outbound port currently defined in `quanta-index-core::domains` has a real, idempotent, fail-closed impl in [quanta-index-control](../../crates/quanta-index-control/). No placeholders on the inspect path.

### T1.1 — `PublishedSearchBundleDeltaApplyPort` impl

- **Depends on:** none (parallel with T1.2, T1.3)
- **Scope:**
  - new SQLite table `bundle_delta_applied` keyed by `(repo_id, revision_id, manifest_generation, op_kind, target_id)`
  - `apply_bundle_delta`:
    1. validate request via new `BundlePolicy::validate_delta` (extension; see [bundle_ingest/service.rs](../../crates/quanta-index-core/src/domains/bundle_ingest/service.rs))
    2. open transaction
    3. for each `SearchBundleMutationOp`: INSERT OR IGNORE into `bundle_delta_applied`
    4. count newly-inserted rows; commit
    5. response: `applied = (new_rows > 0)`, `indexed_generation = request.generation.manifest_generation`, `reason` set when `applied = false`
  - referential check: request's `generation.manifest_generation` must exist in `generation_catalog`; missing → `CoreError::InvalidContract`
- **TDD test list (failing first):**
  1. fresh delta → `applied = true`, `indexed_generation` matches
  2. duplicate delta (same ops twice) → `applied = false`, `reason = "delta already applied"`
  3. delta with `operations = vec![]` → `applied = false` (no-op accepted, reason "no operations")
  4. delta against unknown generation → `CoreError::InvalidContract`
  5. mixed delta (1 new + 1 already applied) → `applied = true` (partial-new counts as progress)
  6. transaction rollback: if one op insert fails (e.g. constraint), no ops applied (verify via subsequent COUNT(*))
  7. proptest: random sequence of upsert+delete ops → repeat application → second iteration is fully idempotent
- **Files touched:**
  - new: `crates/quanta-index-control/src/store/delta_apply.rs`
  - extend: `crates/quanta-index-control/src/store/schema.rs`, `crates/quanta-index-control/src/store/mod.rs`, `crates/quanta-index-control/src/store/tests.rs`, `crates/quanta-index-control/tests/control_plane.rs`
  - extend: `crates/quanta-index-core/src/domains/bundle_ingest/service.rs` (`BundlePolicy::validate_delta`)
  - extend: `crates/quanta-index-core/tests/property_policies.rs`
- **Scenarios covered:** E-SP1, partial U-SP2
- **Complexity:** M (~3–5h)

### T1.2 — `PublishedSearchGenerationCatalogPort` impl

- **Depends on:** none (parallel with T1.1, T1.3)
- **Scope:**
  - new SQLite table `generation_manifest` keyed by `(repo_id, revision_id, manifest_generation)`, payload column = JSON-encoded `PublishedSearchBundleManifest`
  - `record_generation_manifest`:
    1. validate every `BundleArtifactRef` via `BundlePolicy::validate_artifact_ref` (new extension)
    2. JSON-encode manifest
    3. INSERT OR REPLACE into `generation_manifest`
    4. idempotent: re-recording same payload returns Ok with no observable difference (digest of stored vs new is compared; equal → no-op; different → REPLACE)
- **TDD test list:**
  1. record + read round-trips full manifest (all optional fields nullable correctly)
  2. duplicate record (identical payload) → Ok, single row in table
  3. record with same key but different payload → REPLACE (last-write-wins is documented behaviour for catalog upsert)
  4. manifest with `byte_length = 0` artifact ref → `InvalidContract`
  5. manifest with empty `content_digest` → `InvalidContract`
  6. manifest with empty `relative_path` → `InvalidContract`
  7. proptest: random manifests with valid refs always record + read equal
- **Files touched:**
  - extend: `crates/quanta-index-control/src/store/schema.rs`
  - extend: `crates/quanta-index-control/src/store/generation_registry.rs`
  - extend: `crates/quanta-index-core/src/domains/bundle_ingest/service.rs` (`BundlePolicy::validate_artifact_ref`)
  - extend: tests above + `crates/quanta-index-core/tests/property_policies.rs`
- **Scenarios:** U-SP2 prerequisite
- **Complexity:** M

### T1.3 — `PublishedSearchActivationStatePort` impl

- **Depends on:** none
- **Scope:**
  - new method `mark_active_generation(generation: &PublishedGenerationSet)` writes activation_state row WITHOUT touching catalog (catalog upsert is the activate-generation flow's responsibility)
  - distinct from existing `activate_generation` (which still owns the readiness-check path)
  - this port enables orchestrators (E3) that activate only after build success
- **TDD test list:**
  1. mark active sets active pointer; readiness reflects it
  2. mark replace: second mark for same (repo, rev) atomically replaces row
  3. mark with no readiness flags set → reads default false/false (consistent with existing schema default)
- **Files touched:** extend `crates/quanta-index-control/src/store/generation_registry.rs` + tests
- **Complexity:** S (~1–2h)

### T1.4 — replace `inspect_bundle` placeholder

- **Depends on:** T1.2
- **Scope:**
  - `inspect_bundle` reads from `generation_manifest` (T1.2) instead of returning placeholder artifacts
  - unknown gen → `CoreError::NotFound` (new variant if needed — confirm enum at impl time)
  - `artifacts` field = union of all artifact refs present in the persisted manifest
  - **does NOT** load artifact bytes (that's E2 work)
- **TDD test list:**
  1. record then inspect → manifest fields equal; artifacts list equals union of refs
  2. inspect unknown gen → typed NotFound (not `InvalidContract`, not Storage)
  3. record then inspect with optional fields None → artifacts excludes None refs
- **Files touched:** extend `crates/quanta-index-control/src/store/generation_registry.rs` + tests
- **Scenarios:** U-SP2, partial E-SP2
- **Complexity:** M

### E1 acceptance gate

- all T1.* tests green
- property invariants for delta apply + artifact-ref validation added
- `just verify` + `just verify-rust-heavy` (Miri/careful) green
- schema bootstrap idempotent across fresh + existing DBs
- inspect on known gen returns recorded manifest (no placeholders)

---

## Epic E2 — (dropped per D17)

Bundle byte 모델이 `read + verify + forget`로 결정되면서 별도 artifact-store/loader 크레이트는 불필요해짐. 대체:

- `quanta-index-artifact` 크레이트 삭제 (scaffold 제거됨)
- core의 `PublishedSearchArtifactStorePort` 제거됨
- bundle byte 읽기 + sha256 검증 유틸은 `searchd::app::materialize` 모듈 안에 inline (TZ는 E3 ticket들 안에서 다룸)

E3 ticket들이 build port 호출 시점에 inline read+verify를 책임진다. 자세한 건 아래 E3 참조.

---

## Epic E3 — Materialization adapters

**Goal:** real lexical + semantic adapters, metadata store, embedding verifier, and `searchd::app` orchestration that materializes BEFORE activating.

### T3.1 — Tantivy lexical adapter (`quanta-index-lexical`)

- **State:** crate scaffold + workspace member + `ALLOWED_CRATE_DEPS` entry already exist; vendor dep + impl missing.
- **Depends on:** E2
- **Parallel with:** T3.2, T3.3, T3.4
- **Scope:**
  - extend existing crate [crates/quanta-index-lexical/](../../crates/quanta-index-lexical/)
  - `tantivy = 0.22.x` dep (license MIT — re-verify against [deny.toml](../../deny.toml) allow-list at impl time)
  - `TantivyLexicalAdapter` impls `SearchPlaneLexicalIndexBuildPort` + `SearchPlaneLexicalIndexStorePort`
  - construction: `with_state_root(root: PathBuf)` (no artifact-store injection — D17)
  - core port signature change (D17): `build_lexical_index(&mut self, manifest: &PublishedSearchBundleManifest, chunk_rows: &[u8], symbol_rows: &[u8]) -> Result<(), CoreError>`. Caller (`searchd::app::materialize`) reads + digest-verifies bytes inline before calling.
  - body:
    1. resolve target dir: `{state_root}/lexical/{manifest_generation}/`
    2. if dir exists and contains a `MARKER_OK` sentinel → no-op (idempotent)
    3. create temp build dir alongside (`{...}.building`)
    4. decode the byte slices (Arrow IPC) and build Tantivy index in temp dir
    5. atomic rename temp → target dir
    6. write `MARKER_OK`
  - `open_lexical_store(generation)`:
    1. resolve `{state_root}/lexical/{manifest_generation}/`
    2. verify `MARKER_OK` exists; absent → `CoreError::NotReady`
    3. open Tantivy index reader, return handle (handle type defined in this crate; query crate calls through this handle)
  - schema (Phase 1):
    - `candidate_id: STRING (stored)`
    - `repo_id, revision_id: STRING (indexed)`
    - `manifest_generation: U64 (indexed, stored)`
    - `repo_relative_path: STRING (indexed, stored)`
    - `start_line, end_line: U64 (stored)`
    - `text: TEXT (en_stem tokenizer, indexed)`
- **Open design decisions:**
  - **D4 (accepted):** tokenizer = Tantivy default `en_stem` Phase 1
  - **D14 (accepted):** stale `.building` dirs retained for debug; cleanup is operator concern
  - **D15 (flag for review):** chunk_rows artifact encoding — Phase 1 reads via JSON (extend FsArtifactStore to support Arrow IPC = scope creep). Recommend: Phase 1 fixtures use JSON-encoded rows; extend FsArtifactStore to support `BundleEncoding::ArrowIpc` as Phase 2.5 follow-up.
- **TDD test list:**
  1. build from fixture manifest produces index; `open_lexical_store` reads back expected doc count
  2. `open_lexical_store` before build → `CoreError::NotReady`
  3. duplicate build over same gen → no-op (idempotent); no `.building` left behind
  4. corrupted artifact (digest mismatch via FsArtifactStore) → `InvalidContract` propagated; target dir NOT created
  5. empty `chunk_rows` (valid manifest, no rows) → empty index, MARKER_OK written, open succeeds with zero docs
  6. concurrent builds for different gens → both succeed, isolated dirs
- **Files:** new crate + integration test
- **Acceptance:** tests + clippy/deny + hexagonal lint accepts new dep edge for searchd
- **Scenarios:** H-SP1 prerequisite
- **Complexity:** L

### T3.2 — Lance semantic adapter (`quanta-index-semantic`)

- **State:** crate scaffold + workspace member + `ALLOWED_CRATE_DEPS` entry already exist; vendor dep + impl missing.
- **Depends on:** T1 (no E2)
- **Parallel with:** T3.1
- **Scope:**
  - extend existing crate [crates/quanta-index-semantic/](../../crates/quanta-index-semantic/)
  - `lance = 0.x` dep (license Apache-2.0 — re-verify at impl)
  - `LanceSemanticAdapter` impls `SearchPlaneSemanticIndexBuildPort` + `SearchPlaneVectorIndexStorePort`
  - core port signature change (D17): `build_semantic_index(&mut self, manifest: &PublishedSearchBundleManifest, embedding_bytes: Option<&[u8]>) -> Result<(), CoreError>`. Caller reads + verifies bytes inline; passes `None` when manifest has no embedding_records.
  - body:
    1. if `embedding_bytes.is_none()` → no-op Ok (lexical-only generation)
    2. resolve `{state_root}/semantic/{manifest_generation}/`
    3. idempotent via MARKER_OK sentinel
    4. decode raw F32 bytes per `BundleEncoding::RawF32`
    5. write Lance dataset with schema `{entity_id: Utf8, vector: FixedSizeList<Float32, dim>}`
    6. atomic rename, write MARKER_OK
  - `open_vector_store(generation)`:
    1. if no semantic dir AND manifest had no `embedding_records` → return Ok handle marked "no-vectors" (caller decides if semantic queries error)
    2. else verify MARKER_OK, open Lance dataset
- **Open design decisions:**
  - **D5 (flag for review):** vector dim source. Phase 1 reads dim from a fixed convention — first record's `vector.len()` defines the index dim; subsequent records with mismatched len → `InvalidContract`. Alternative (read from manifest header field) requires contract change → out.
  - **D6 (accepted):** similarity metric = cosine
- **TDD test list:**
  1. build with fixture (10 vectors, dim 768) produces Lance dataset; open reads back rows
  2. open before build (manifest had embeddings) → `NotReady`
  3. open before build (manifest had no embeddings) → Ok no-vectors handle
  4. dimension mismatch within records → `InvalidContract`, dataset NOT created
  5. duplicate build → idempotent
- **Files:** new crate + integration test
- **Scenarios:** H-SP1 prerequisite
- **Complexity:** L

### T3.3 — metadata store backend (decision + impl)

- **Depends on:** T1.2 (manifest persistence)
- **Scope:** decide WHERE metadata lives and implement.
  - **Option A:** extend `quanta-index-control` to impl `SearchPlaneMetadataStorePort` (cohabit with control plane SQLite)
  - **Option B:** new crate `quanta-index-metadata-fs` (Arrow-backed)
- **Open design decision D7 (accepted):** Option A for Phase 1. Operational simplicity (single SQLite file for control + metadata), atomic with control plane. Re-evaluate if metadata hot-path bottleneck emerges.
- **Scope (Option A):**
  - new SQLite table `manifest_metadata` populated by `record_generation_manifest` (T1.2 extension) — store metadata rows extracted from manifest
  - `open_metadata_store(generation)`:
    1. query for at least one row at `(repo_id, revision_id, manifest_generation)`
    2. no rows → `NotReady`
    3. rows present → return handle (read-only view)
- **TDD test list:**
  1. open after record → Ok with handle
  2. open before record → `NotReady`
  3. handle supports row count (smoke test)
- **Files:** extend `crates/quanta-index-control/src/store/{schema,generation_registry}.rs`, new `crates/quanta-index-control/src/store/metadata.rs`
- **Complexity:** M

### T3.4 — embedding presence assertion (verification absorbed into materialize)

- **Status:** simplified per D17. `SearchPlaneEmbeddingProviderPort::ensure_embeddings` becomes a thin presence assertion: "manifest's `embedding_records` ref is consistent with what `searchd::app::materialize` already read+verified". Since materialize reads+verifies inline (D17), this port collapses into the orchestrator.
- **Depends on:** T3.5 (orchestrator owns this)
- **Open design decision D8 (accepted):** Phase 1 = verification-only. Search-plane does NOT generate embeddings.
- **TDD test list:**
  1. manifest with embedding_records + valid file → orchestrator accepts (semantic build runs)
  2. manifest with embedding_records + missing file → orchestrator fails fast, `Storage` error
  3. manifest with embedding_records + digest mismatch → `InvalidContract`
  4. manifest without embedding_records → orchestrator accepts; semantic build no-ops
- **Files:** logic lives in `crates/quanta-index-searchd/src/app/materialize.rs` (no separate verify module)
- **Complexity:** S

### T3.5 — `searchd::app::materialize` orchestrator (owns read+verify per D17)

- **Depends on:** T3.1 + T3.2 + T3.3 + T1.3
- **Scope:**
  - new module `crates/quanta-index-searchd/src/app/materialize.rs` with `MaterializeUseCase` + inline read+verify utility (`load_artifact_bytes(ref) -> Result<Vec<u8>, CoreError>`):
    1. reject absolute or `..`-containing `relative_path` before any FS access
    2. resolve under the configured bundle root (e.g. `config.bundle_root`)
    3. `std::fs::read`
    4. assert `bytes.len() as u64 == ref.byte_length` (overflow-safe)
    5. SHA-256 hex lowercase compare to `ref.content_digest.as_str()`
    6. typed `CoreError` on any mismatch
  - flow:
    1. receive `PublishedSearchBundleManifest`
    2. inline read+verify of `lexical_chunk_rows`, `symbol_rows`, and (optional) `embedding_records`
    3. parallel via `std::thread::scope`:
       - `build_lexical_index(&manifest, &chunk_bytes, &symbol_bytes)`
       - `build_semantic_index(&manifest, embedding_bytes.as_deref())`
       - `metadata_store.open_metadata_store(&generation)` (T3.3)
    4. all must succeed → call `mark_active_generation` (T1.3) THEN `activate_generation` (existing)
    5. any failure → activation NOT invoked; partial dirs retained for debug (D14)
  - readiness flags (`lexical_ready`, `semantic_ready`) computed from build outcomes; passed to `activate_generation`
- **Open design decision D9 (accepted):** parallelism = `std::thread::scope` (CPU-bound builds, no async I/O needed)
- **TDD test list:**
  1. happy path: all builds succeed → activate called → readiness shows new gen, both flags true
  2. lexical build fails → activate NOT called → readiness reflects previous gen (H-SP1)
  3. semantic build fails → activate NOT called
  4. embedding verify fails → activate NOT called
  5. metadata open fails → activate NOT called
  6. lexical-only manifest (no embeddings) → semantic build no-ops; activate called with `semantic_ready = true` (no-vectors is a legitimate ready state)
- **Files:** new module + integration test under `crates/quanta-index-searchd/tests/`
- **Scenarios:** U-SP2, H-SP1
- **Complexity:** L

### E3 acceptance gate

- all adapters pass own unit + integration tests
- materialize-before-activate orchestration green
- H-SP1 explicit scenario test red without orchestration, green with
- hexagonal lint accepts vendor deps in their respective adapter crates only

---

## Epic E4 — Query engine + UDS transport

**Goal:** wire real query engine into `searchd::app`, real `GenerationPinPort`, and full UDS IPC.

### T4.1 — `DomainQueryEngine` (real `domains::query` inbound impls)

- **Depends on:** E3 (open_lexical_store, open_vector_store)
- **Scope:**
  - new module `crates/quanta-index-searchd/src/query/domain_engine.rs`
  - struct holds: lexical_store, vector_store, generation_pin, validator
  - implements all 4 query inbound ports + `SearchPlaneQueryContractPort`
  - each path:
    1. `validator.validate_query(&request.query)` (uses existing `QueryPolicy`)
    2. resolve generation:
       - if `request.generation.is_some()` → use it (explicit pin)
       - else `generation_pin.pinned_generation()` derived from request filters (T4.2)
       - if none → `CoreError::NotReady` (NOT empty results — H-SP2)
    3. dispatch to lexical_store / vector_store handles
    4. assemble `LexicalCandidate` from result rows
  - hybrid: `score = w_lex * lex_score + w_sem * sem_score`, weights fixed `(0.5, 0.5)` Phase 1 (D10)
  - explain: returns `SearchExplanation { summary: "..." }` with stringified breakdown (structured form is post-Phase 3)
  - `StubQueryEngine` retained only under `#[cfg(test)]` for fixtures
- **Open design decisions:**
  - **D10 (accepted):** hybrid weights fixed 0.5/0.5 Phase 1
  - **D16 (flag for review):** explain shape — string summary Phase 1, structured `Vec<ExplainTerm>` post-Phase 3
- **TDD test list:**
  1. lexical_query without pinned gen + no explicit gen → `NotReady` (H-SP2)
  2. lexical_query against indexed fixture (one repo, one rev, 3 chunks) returns expected candidates
  3. lexical_query with `LqExpr::MatchAll` → `InvalidContract` (QueryPolicy)
  4. semantic_query empty vector store (no-vectors handle) → `NotReady`
  5. semantic_query `top_k = 5` respected
  6. hybrid combines lexical+semantic; top_k applied to merged ranking
  7. explain returns non-empty summary referencing input candidate_id
- **Files:** new module; `crates/quanta-index-searchd/src/query/mod.rs` updated; existing `stub_engine.rs` retained behind `#[cfg(test)]`
- **Scenarios:** U-SP3 (cold restart wiring), C-SP1, C-SP2, H-SP2
- **Complexity:** L

### T4.2 — `GenerationPinPort` impl

- **Depends on:** T1.3
- **Scope:**
  - SSOT port signature: `pinned_generation(&self) -> Result<Option<PublishedGenerationSet>, CoreError>` (no selectors)
  - Phase 1 interpretation: the pin is constructed **per query request** with `(repo_id, revision_id)` lifted from request filters; the long-lived handle is `ControlPlaneGenerationPinFactory` which produces a per-request `BoundGenerationPin`
  - `BoundGenerationPin::pinned_generation()` reads `generation_activation_state` JOIN `generation_catalog` for the bound (repo, rev)
- **Open design decision D11 (accepted):** per-query lifetime. Long-lived pin caching is a future optimization.
- **TDD test list:**
  1. pin against active gen returns Some(generation)
  2. pin against unknown (repo, rev) returns None
  3. **U-SP4**: query started against G1 captures G1 snapshot; concurrent `activate_generation(G2)` mid-query does not retroactively switch the in-flight query's pin; next query captures G2
- **Files:** extend `crates/quanta-index-control/src/store/generation_registry.rs`; wire factory in `crates/quanta-index-searchd/src/runtime/`
- **Scenarios:** U-SP4
- **Complexity:** M

### T4.3 — UDS wire codec (`quanta-index-ipc`)

- **State:** crate scaffold + workspace member + `ALLOWED_CRATE_DEPS` entry already exist; codec impl missing. The crate already has `quanta-index-core` allowed (driving adapter touches `CoreError`).
- **Depends on:** none (parallel with T4.1, T4.2)
- **Scope:**
  - extend existing crate [crates/quanta-index-ipc/](../../crates/quanta-index-ipc/), add deps: `ciborium`, `serde`
  - frame format: `[u32 LE length][CBOR-encoded envelope bytes]`
  - max frame size: 16 MiB (constant; fail-closed if length header exceeds)
  - public API:
    - `encode_request<T: serde::Serialize>(envelope: &T) -> Result<Vec<u8>, IpcError>`
    - `encode_response<T: serde::Serialize>(envelope: &T) -> Result<Vec<u8>, IpcError>`
    - `decode_request<T: serde::de::DeserializeOwned>(reader: &mut impl io::Read) -> Result<T, IpcError>`
    - `decode_response<T: serde::de::DeserializeOwned>(reader: &mut impl io::Read) -> Result<T, IpcError>`
  - `IpcError` enum: `Truncated`, `Oversized`, `Decode(String)`, `Encode(String)`, `EmptyFrame`, `Io(io::Error)`
- **Open design decision D12 (accepted):** wire format = CBOR via `ciborium` (compact, schemaful, well-supported)
- **TDD test list:**
  1. round-trip every request variant (Lexical, Semantic, Hybrid, Explain)
  2. round-trip every response variant (Lexical, Semantic, Hybrid, Explain, Error)
  3. truncated length header (< 4 bytes) → `Truncated`
  4. length header > 16 MiB → `Oversized`, body bytes NOT read
  5. body bytes < declared length → `Truncated`
  6. valid length, garbage CBOR body → `Decode`
  7. empty body (length = 0) → `EmptyFrame`
  8. proptest: random envelope → encode → decode equal
- **Files:** new crate
- **Acceptance:** tests + clippy/deny (`ciborium` MIT)
- **Scenarios:** H-SP3
- **Complexity:** M

### T4.4 — searchd UDS listener + dispatch loop

- **Depends on:** T4.1 + T4.3
- **Scope:**
  - new module `crates/quanta-index-searchd/src/app/uds_listener.rs`
  - opens query/control `tokio::net::UnixListener`s at `config.query_socket_path` / `config.control_socket_path` (current-thread runtime)
  - per-connection task loop:
    1. read frame via T4.3 decoder
    2. decode error → close the connection fail-closed; do not emit a response frame (H-SP3)
    3. dispatch to `DomainQueryEngine`
    4. engine `Result::Ok(response)` → encode + write
    5. engine `Result::Err(CoreError::X)` → map to `SearchPlaneIpcError{code: "X", message: ...}` envelope; write Error response
  - shutdown: SIGINT/SIGTERM via `tokio::signal`; drain in-flight requests; close listener
  - removes stale socket file on startup if no other process listening (verify via connect attempt)
- **Open design decision D13 (accepted):** tokio current_thread Phase 1; multi-thread post-Phase 3
- **TDD test list:**
  1. listener binds query socket; client connects + roundtrips one query
  2. malformed frame from client → connection closes fail-closed; no response frame is emitted
  3. SIGINT during idle → listener closes; pending sockets drained
  4. stale socket file present → cleanly removed at startup
  5. end-to-end lexical query through socket → matches in-process result
- **Files:** new module; add `tokio = { version = "1", features = ["net", "rt", "macros", "signal", "io-util"] }`; extend `crates/quanta-index-searchd/tests/serve_smoke.rs`
- **Scenarios:** U-SP3, H-SP3
- **Complexity:** L

### T4.5 — CLI extension: serve options

- **Depends on:** T4.4
- **Scope:**
  - `serve` subcommand accepts `--query-socket-path PATH`, `--control-socket-path PATH`, `--state-root PATH` overrides
  - env vars remain fallback
  - parse error → stderr `error: ...` + exit code `2`
- **TDD test list:**
  1. `--query-socket-path /tmp/q.sock` and `--control-socket-path /tmp/c.sock` override config paths
  2. `--state-root /tmp/sr` overrides + derives control DB + socket paths
  3. unknown flag → exit code 2
  4. existing no-arg `serve` still works
- **Files:** extend `crates/quanta-index-searchd/src/cli/commands.rs` + tests
- **Complexity:** S

### E4 acceptance gate

- end-to-end query via UDS green (`serve_smoke` extended)
- H-SP2 + H-SP3 both covered by red→green tests
- `StubQueryEngine` no longer in production wiring

---

## Epic E5 — Scenario matrix proofs

**Goal:** every scenario in SSOT § Scenario Matrix has a green integration test.

### T5.1 — `U-SP*` scenarios

- **Depends on:** E1–E4
- **Scope:** integration tests under `crates/quanta-index-searchd/tests/`:
  - `scenarios_usp.rs`:
    - **U-SP1**: producer prepare → outbox row visible (via `prepare_bundle` + readiness count)
    - **U-SP2**: activate → materialize → query readiness (full E1+E3+E4 flow)
    - **U-SP3**: cold restart: kill searchd → restart from `state_root` → readiness preserved → query works
    - **U-SP4**: concurrent prepare while query domain pinned on G1 (T4.2 covers; this is the scenario-level assertion)
- **Complexity:** M

### T5.2 — `E-SP` / `C-SP` / `H-SP` scenarios

- **Depends on:** T5.1
- **Scope:** integration tests:
  - `scenarios_esp.rs`:
    - **E-SP1** duplicate prepare idempotency
    - **E-SP2** stale activation rejected (activate with `manifest_generation` < currently active)
    - **E-SP3** invalid manifest refs fail-closed (digest mismatch surfaced through materialize)
  - `scenarios_csp.rs`:
    - **C-SP1** no active gen → explicit `CoreError::NotReady` (not empty results)
    - **C-SP2** partial readiness (`lexical_ready=true, semantic_ready=false`) → semantic query NotReady, lexical query Ok
  - `scenarios_hsp.rs`:
    - **H-SP1** activate-before-materialize blocked at orchestrator (T3.5)
    - **H-SP2** serve query on unreadied gen → typed `NotReady` envelope, not empty
    - **H-SP3** UDS framing error on active path → connection closes fail-closed; listener remains available for new connections
- **Files:** test files above
- **Complexity:** M

### E5 acceptance gate

- all scenario tests green
- each hellgate scenario was red before its impl ticket and green after
- coverage table in README updated

---

## Hellgate coverage map

| Hellgate | Impl ticket(s) | Scenario ticket |
|----------|----------------|-----------------|
| H-SP1 (activate before materialize) | T3.5 | T5.2 (`scenarios_hsp::h_sp1_*`) |
| H-SP2 (query on unready gen) | T4.1 + T4.4 | T5.2 (`scenarios_hsp::h_sp2_*`) |
| H-SP3 (UDS framing error) | T4.3 + T4.4 | T5.2 (`scenarios_hsp::h_sp3_*`) |

Each hellgate ticket bundle MUST demonstrate red-before-green via a commit pair (or single commit with reverted-impl proof in PR description).

---

## Crate dependency matrix (post-implementation)

`tools/ci/lint/lint-hexagonal-boundaries.py` updates per epic:

Post-scaffold state (already in [lint-hexagonal-boundaries.py](../../tools/ci/lint/lint-hexagonal-boundaries.py)):

| Crate | Internal deps (lint-allowed) | Planned vendor (added per ticket) |
|-------|------------------------------|-----------------------------------|
| `quanta-index-contract` | — | `serde`, `thiserror` |
| `quanta-index-core` | contract | `thiserror` |
| `quanta-index-control` | contract, core | `rusqlite`, `serde_json` |
| `quanta-index-lexical` | contract, core | `tantivy` (E3) |
| `quanta-index-semantic` | contract, core | `lance` (E3) |
| `quanta-index-ipc` | contract, core | `ciborium`, `serde`, `tokio` (E4) |
| `quanta-index-searchd` | all above | `tokio`, `anyhow`, `sha2` (E3 inline read+verify) |

`core` must never gain `rusqlite | tantivy | lance | lancedb | sha2`. `contract` must never gain `pub trait`.

Adapters take **byte slices**, not artifact-store handles. `searchd::app::materialize` is the only place that reads the FS and verifies sha256 (D17). Lexical / semantic adapters never touch the filesystem for bundle inputs; they only own their own index dir under `{state_root}/...`.

---

## Open design decisions (central register)

| ID | Decision | Status |
|----|----------|--------|
| D1 | delta history pruning | TODO Phase 4 |
| D2 | manifest payload encoding | JSON Phase 1 → Arrow IPC Phase 2.5 |
| D3 | content_digest algorithm | SHA-256 hex lowercase ✓ |
| D4 | Tantivy tokenizer | `en_stem` Phase 1 ✓ |
| D5 | vector dim source | first-record-defines Phase 1 (flag) |
| D6 | similarity metric | cosine ✓ |
| D7 | metadata store backend | Option A: control crate extension ✓ |
| D8 | embedding provider scope | verification-only Phase 1 ✓ |
| D9 | materialize parallelism | `std::thread::scope` ✓ |
| D10 | hybrid query weights | 0.5/0.5 fixed Phase 1 ✓ |
| D11 | generation pin lifetime | per-query ✓ |
| D12 | IPC wire format | CBOR via `ciborium` ✓ |
| D13 | async runtime | tokio current_thread Phase 1 ✓ |
| D14 | partial materialize cleanup | retain stale (debug) ✓ |
| D15 | chunk_rows artifact encoding | JSON fixtures Phase 1; Arrow IPC Phase 2.5 (flag) |
| D16 | explain response shape | string summary Phase 1 (flag) |
| D17 | bundle byte 소유 모델 | producer owner; search-plane = `read+sha256 verify+forget`. retention 0. 별도 artifact-store/loader 크레이트 없음. read+verify는 `searchd::app::materialize` inline ✓ |
| D18 | serde derive 금지 | workspace 전체 `#[derive(Serialize/Deserialize)]` 금지. 수동 impl만. 이유: cold-build 시간 + wire shape 리뷰 가능성. semgrep `rust-no-serde-derive` 강제 ✓ |
| D19 | error code casing | `SCREAMING_SNAKE_CASE` (`NOT_READY`, `INVALID_CONTRACT`, `IPC_DECODE`, …) per SSOT § error code taxonomy ✓ |
| D20 | catalog state lifecycle | Phase 1 ships `prepared` (via `record_generation_manifest`) + `active` (via activate). `materialized` / `failed` deferred to Phase 3.5; needs new orchestrator hooks to call into control between build success and activate, and between build failure and post-mortem |
| D21 | semantic / hybrid query | deferred-with-reason; needs (a) text-embedding model choice + (b) Lance ANN integration. Fail-closed `NotImplemented` Phase 1. Phase 3.5 follow-up |
| D22 | delta governance | delta-apply forbidden against currently-active generation; requires recorded manifest for target; rejects on either condition ✓ |
| D23 | stale activation guard | activation rejects (a) lower or unchanged manifest_generation, (b) any component-generation regression; applied to both `activate_generation` and `mark_active_generation` ✓ |
| D24 | UNKNOWN_GENERATION mapping | query path: explicit `request.generation` overrides must match the currently-active manifest_generation for `(repo, rev)`. Mismatch → `InvalidContract("UNKNOWN_GENERATION: …")`; absent → `NotReady("UNKNOWN_GENERATION: …")` ✓ |

Decisions flagged "flag for review" must be re-confirmed before the relevant ticket lands.

---

## What this plan does NOT cover

- producer-side coordination (SSOT scope-out; owned by `semantica-codegraph-v2`)
- `quanta-storage-*`, QueryExecutor artifact store, reasoning graph storage (SSOT § Out of Scope)
- HTTP transport (UDS only Phase 3; HTTP is future)
- production observability (metrics/tracing/structured logs) — separate epic post-Phase 3
- horizontal scale / replicated control plane — explicitly Phase 4+
- TLS / authz on UDS path — UDS access controlled by FS perms Phase 1

---

## How to use this doc

1. Pick a ticket whose `depends-on` are all green
2. Write failing tests **first**; observe red
3. Implement; flip green
4. Re-confirm any flagged design decision before merging
5. `just verify` + `just rust-hexagonal` green
6. PR title: `[E{N}T{X.Y}] {short title}` (e.g. `[E1T1.1] delta apply port impl`)
7. PR description: state which scenario codes (`U/E/C/H-SP*`) move from missing → covered

When a ticket completes, update the status table below.

## Ticket status

| Ticket | State | PR | Notes |
|--------|-------|----|----|
| T1.1 | **done** | — | 11 integration tests; new `bundle_delta_applied` table + idempotent INSERT OR IGNORE; `BundlePolicy::validate_delta`; **delta-apply governance per SSOT**: rejects (a) target generation not in catalog, (b) target currently-active, (c) no recorded manifest for target; `applied_at_ms` is now real `SystemTime::now()` ms (earlier rev mistakenly wrote `manifest_generation`) |
| T1.2 | **done** | — | `generation_manifest` table + JSON encoding; `BundlePolicy::validate_artifact_ref`; 4 manifest-shape tests |
| T1.3 | **done** | — | `mark_active_generation(gen, ts)`; **stale-activation guard** (rejects lower manifest_generation OR regressing component generation) applies to both `activate_generation` and `mark_active_generation`; 5 integration tests incl. E-SP2 + component regression + orchestrator-side stale mark |
| T1.4 | **done** | — | `inspect_bundle` reads from `generation_manifest`; `CoreError::NotFound` for unknown gens; placeholder helper removed; **artifacts union includes `mutation_delta`** (also a `BundleArtifactRef`); `record_generation_manifest` now also writes a `state='prepared'` catalog row when none exists, giving the SSOT lifecycle its first transition (`materialized`/`failed` still deferred) |
| T2.* | dropped | — | E2 epic eliminated per D17 — bundle byte read+verify lives inline in T3.5 |
| T3.1 | **done** | — | Tantivy 0.22 adapter; 7 integration tests; en_stem schema; tantivy-scoped skip-tree; **`Index` handle cached per-generation in `Mutex<BTreeMap<u64, Index>>`** to avoid re-opening on every query (hot-path fix) |
| T3.2 | **done** | — | Lance 6.0.1 adapter; 9 integration tests; manual `wire.rs` RawF32 decoder; lance-scoped skip-tree |
| T3.3 | **done** | — | `SearchPlaneMetadataStorePort` existence-check via `generation_manifest`; integrated with control |
| T3.4 | **done** | — | embedding presence verification absorbed into T3.5 inline read+verify (D17) |
| T3.5 | **done** | — | `MaterializeUseCase` with inline sha256 read+verify + `std::thread::scope` parallel builds; 11 unit tests + 4 scenario tests |
| T4.1 | **partial** | — | Lexical + explain real (Arc-owned engine, Tantivy Raw/All/Any/Not, `timeout_ms` cooperative deadline, **explicit-generation override now validated against active pin = UNKNOWN_GENERATION fail-closed**). **Semantic + hybrid deferred-with-reason**: no `QueryEmbedder` shipped, both return typed `NotImplemented`. Closing this row requires picking a text-embedding model + wiring Lance ANN — flagged as Phase 3.5 follow-up |
| T4.2 | **done** | — | `BoundGenerationPin` + `ControlPlane::pin_generation`; per-query snapshot; 4 tests incl. U-SP4 |
| T4.3 | **done** | — | CBOR codec with 16 MiB cap, 21 tests (roundtrip + framing errors + 2 proptests) |
| T4.4 | **done** | — | split query/control UDS listeners on tokio; per-conn dispatch; decode error closes the connection fail-closed with no response frame; SIGINT/SIGTERM clean shutdown; `searchd` composition root wires query/control dispatchers + adapters via `Arc`; real binary e2e covers UDS roundtrip and clean shutdown |
| T4.5 | **done** | — | `--state-root` / query-control socket CLI overrides; `ServeOptions` with_overrides composition; 9 unit tests |
| T5.1 | **done** | — | U-SP1 outbox visibility; U-SP2 lexical happy-path via **real `searchd` binary + UDS** (`serve_smoke::serve_binary_happy_path_lexical_roundtrip_over_uds`); U-SP3 **real process restart** (`serve_smoke::serve_binary_restart_preserves_active_generation_and_index` spawns, SIGINTs, respawns, asserts same `LexicalCandidate`s); U-SP4 pin-survives-activate via control pin snapshot |
| T5.2 | **done** | — | E-SP1 dup prepare; E-SP2 stale activation (3 facets); E-SP3 digest-mismatch fail-closed; C-SP1 NotReady-not-empty; C-SP2 lexical-only readiness; H-SP1 build-fail blocks activation; H-SP2 query-on-unready typed `NotReady`; H-SP3 UDS framing error closes the connection fail-closed; **error codes use SCREAMING_SNAKE_CASE per SSOT** (`NOT_READY`, `INVALID_CONTRACT`, `NOT_FOUND`, `NOT_IMPLEMENTED`, `STORAGE`, `IPC_DECODE`) |
