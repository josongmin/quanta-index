# SDK / Ingest IPC Cutover — Wave Plan (2026-05-25)

12 tickets across 5 waves to finish the producer / search-plane split cutover.

Each ticket carries: Owner files / Acceptance / Blockers / Proof.

Cross-repo proof for semantica is tracked separately as `SM-VRF-01` in the semantica repo. This document only records the quanta-index side.

Follow-on packet:
- [may-24 lexical ticket `SDK-ENTRY-01`](may-24-lexical-indexing-sorucegraph/tickets/SDK-ENTRY-01.md)
- purpose: after the baseline SDK / ingest IPC cutover below, freeze the
  remaining history / runtime / structural source-authority entry to
  `quanta-index-sdk` only. That repo-local follow-on is now landed on the
  current tree and keeps one external SDK front door while using dedicated
  typed namespaces and batches (`history`, `runtime`, `structural`) instead of
  overloading `LexicalBatch`.

## Current source-truth status (2026-05-27)

Landed on the current tree:

- `searchctl` production query dispatch goes through `quanta-index-sdk`; the
  non-test external consumer path no longer builds or sends raw IPC envelopes
- `sdk_frontdoor` is the happy-path owner proof for lexical, semantic, hybrid,
  explain, repo-map, history, runtime, and structural SDK front doors
- `repo_map_end_to_end` is narrowed to raw IPC transport and persistence
  invariants instead of owning public happy-path proof
- history query authority now fails closed with exact typed codes:
  `HISTORY_GENERATION_NOT_READY`, `HISTORY_PRODUCER_UNAVAILABLE`,
  `HISTORY_SHARD_UNAVAILABLE`
- `quanta-index-sdk` keeps generic namespace and transport plumbing
  crate-internal
- `quanta-index-contract` no longer exposes legacy semantic channel ops on the
  current tree; the remaining legacy channel surface is lexical/history/
  structural only
- lexical ingest always derives search-owned semantic authority on the current
  tree; query-embedder mode no longer disables semantic derivation or accepts
  numeric query text as an internal vector-compat path
- `quanta-index-semantic` now fails closed if a semantic generation is marked
  ready upstream but the backing generation bucket is missing; sealed empty
  generations remain explicit empty buckets
- the retired `quanta-index-channel` workspace crate is no longer part of the
  live verification inventory; de-channelized authority lives in
  `search-plane` / `searchd` runtime code, not a channel backend crate

Current residual scope after the public-surface closure program:

- no open repo-local SDK / ingest cutover residue remains in this packet on the
  current tree
- external producer adoption proof remains cross-repo and is tracked outside
  this repo packet
- search-owned semantic derivation is still a separate deferred follow-on;
  current public semantic/hybrid query contracts are already text-only on the
  current tree

## SEM-OWN entry criteria (2026-05-27 freeze)

- Lane A/B/C and the public-surface quarantine are green on current source
- current semantic/hybrid request surface is already text-only on the current
  tree; the follow-on must not reopen vector/handle or public semantic publish
  paths
- docs/tracker refresh must land first; `SEM-OWN-00 -> 01 -> 02 -> 03 -> 04 ->
  05` starts only after this packet is closed

## Follow-on decision lock

These decisions are frozen before the source-authority follow-on starts:

1. External Rust consumers use `quanta-index-sdk` only.
   - primary typed query route: `history().query()` / `runtime().query()` / `structural().query()`
   - raw text power-user route remains: `lexical().query().native(...)` / `sourcegraph().query()`
2. Query execution has one canonical engine.
   - typed builders own the canonical query DTO / AST surface
   - raw text routes lower into that same canonical engine; no parallel "typed engine" vs "text engine" split
3. Producer authority families publish through dedicated SDK namespaces and batches.
   - `lexical().publish()` is chunk / symbol lexical ingest only
   - `history().publish(HistoryBatch)` / `runtime().publish_dirty(DirtyBatch)` / `structural().publish(StructuralBatch)` own history / dirty / parse-tree ingress
4. Readiness authority is family-local.
   - history / runtime / structural each gate on their own local materialization or catalog watermark
   - lexical seal is not reused as a readiness proxy for other families
5. Public result carriers are family-specific.
   - history returns `CommitCandidate` / `DiffCandidate`
   - runtime returns `RuntimeMetadataCandidate`
   - structural returns `StructuralCandidate { spans, bindings, role_tags }`
6. Option behavior is frozen.
   - `timeout` ships in this packet and is enforced in executor
   - `index:only` is the only executable mode
   - `index:no` is accepted on the canonical route and fails closed with typed `NotImplemented`
   - `boost` is parser / carrier / planner-gate only; active ranking semantics stay out of scope for this packet

## Historical plan context

The Wave 1 / Wave 2 ticket inventory below is kept as the original execution
plan. The former "entry condition" residue listed in this document is no
longer current truth; the source-truth snapshot is the section above.

## Parallel lanes

- **Lane A** QI-ING-01 → QI-RT-01 → QI-SDK-01
- **Lane B** QI-QRY-01 ∥ QI-ACT-01 ∥ QI-RM-01 (contract crate merge ordering only)
- **Lane C** SM-PUB-01 scaffolding first, real cutover after QI-SDK-01
- **Lane D** SM-QRY-01 scaffolding first, real cutover after QI-QRY-01
- **Tail (serial)** SM-DEL-01 → QI-INT-01 → QI-NS-01 → QI-VRF-01

---

## Wave 1

### QI-ING-01 | quanta-index | Typed Ingest IPC Contract

Add `SearchPlaneIngestIpcRequest/Response` and the `ingest.sock` surface to the IPC contract.

**Owner files**
- [crates/quanta-index-contract/src/ipc/split.rs](../../crates/quanta-index-contract/src/ipc/split.rs) (add Ingest enum + envelope)
- [crates/quanta-index-contract/src/ipc/mod.rs](../../crates/quanta-index-contract/src/ipc/mod.rs)
- [crates/quanta-index-contract/src/lib.rs:30](../../crates/quanta-index-contract/src/lib.rs#L30)
- [tools/ci/lint/baselines/public-api/quanta-index-contract.txt](../../tools/ci/lint/baselines/public-api/quanta-index-contract.txt)
- [tools/ci/lint/baselines/cargo-modules/](../../tools/ci/lint/baselines/cargo-modules/)

**Acceptance**
- `SearchPlaneIngestIpcRequest` variants: `PublishLexicalBatch`, `PublishHistoryBatch`, `PublishDirtyBatch`, `PublishStructuralBatch`, `PublishRepoMapBundle`
- `SearchPlaneIngestIpcResponse` variants: `BatchReceipt`, `Error(SearchPlaneIpcError)`
- Each variant payload is a typed batch DTO. Raw `LexicalChannelOp` / opaque `Vec<u8>` payloads are banned (CLAUDE.md "no fat enum variant in DTO")
- All envelopes implement `serde::Serialize` / `Deserialize` manually. No proc-macro derive
- `public-api` and `cargo-modules` baselines updated in the same PR

**Blockers** none (Wave 1 head)

**Proof**
- `cargo check -p quanta-index-contract`
- `just rust-profile test-fast` (ingest envelope round-trip tests)
- `just rust-fuzz-smoke` (ingest target, 60s)
- `tools/ci/lint/check-public-api.py` / `check-cargo-modules-snapshot.py` / `check-module-discipline.py` pass

---

### QI-RT-01 | quanta-index | searchd Ingest Dispatcher

`SearchdConfig` gains `ingest_socket`. The daemon receives typed batches and
routes them to owner materializer ports; runtime-private durability mirrors may
exist behind that surface, but channel row-op fanout is no longer the public
ingest truth.

**Owner files**
- [crates/quanta-index-searchd/src/app/config.rs](../../crates/quanta-index-searchd/src/app/config.rs) (add `ingest_socket`, extend `with_socket_overrides` signature)
- [crates/quanta-index-searchd/src/app/server.rs](../../crates/quanta-index-searchd/src/app/server.rs) (bind ingest listener)
- [crates/quanta-index-searchd/src/app/ipc_dispatcher.rs](../../crates/quanta-index-searchd/src/app/ipc_dispatcher.rs) (ingest routing)
- [crates/quanta-index-searchd/src/app/runtime.rs](../../crates/quanta-index-searchd/src/app/runtime.rs) (composition root wires concrete owner materializers and any runtime-private mirrors; do not expose to SDK)
- new `crates/quanta-index-searchd-runtime/tests/ingest_end_to_end.rs`
- [crates/quanta-index-searchd-runtime/tests/explain.rs](../../crates/quanta-index-searchd-runtime/tests/explain.rs) plus other `with_socket_overrides` call sites — update together

**Acceptance**
- Default path resolves to `state_root/search-plane/ingest.sock`
- Ingest dispatcher accepts `PublishLexicalBatch` / `PublishHistoryBatch` / `PublishDirtyBatch` / `PublishStructuralBatch` / `PublishRepoMapBundle`. Search-owned semantic materialization is derived from accepted lexical batches instead of a public semantic ingest route. Each op's sequence is returned via `BatchReceipt`
- Partial failure propagates as typed `SearchPlaneIpcError`. No `Err(_) => Default::default()` or `if x.is_ok()` two-branch shapes (CLAUDE.md silent-fallback rules)
- Dispatcher fields use `Arc<dyn ...IngestPort>` (DIP). No concrete adapter types exposed
- Existing query / control behavior unchanged

**Blockers** QI-ING-01

**Proof**
- `cargo check -p quanta-index-searchd`
- `cargo clippy --workspace --all-targets -- -D warnings`
- New `ingest_end_to_end.rs` passes (publish → BatchReceipt → query verification)
- `just rust-profile test-daemon`

---

### QI-QRY-01 | quanta-index | Query Contract Residue Cleanup

Remove semantic / hybrid legacy filler. Add `top_k` to `TextQueryRequest` and `SymbolQueryRequest`.

**Owner files**
- [crates/quanta-index-contract/src/query/requests.rs:14-19](../../crates/quanta-index-contract/src/query/requests.rs#L14-L19) (clean up `SemanticCandidateScope.query_text`)
- [crates/quanta-index-contract/src/query/requests.rs:449-455](../../crates/quanta-index-contract/src/query/requests.rs#L449-L455) (`SymbolQueryRequest`: add `top_k`)
- `TextQueryRequest` definition block (add `top_k`)
- [crates/quanta-index-contract/src/results/query_responses.rs](../../crates/quanta-index-contract/src/results/query_responses.rs) (remove raw-vector duplicate fields)
- [crates/quanta-index-sdk/src/search.rs](../../crates/quanta-index-sdk/src/search.rs), [lexical.rs](../../crates/quanta-index-sdk/src/lexical.rs), [semantic.rs](../../crates/quanta-index-sdk/src/semantic.rs) (remove `String::new()` filler)
- [tools/ci/lint/baselines/public-api/quanta-index-contract.txt](../../tools/ci/lint/baselines/public-api/quanta-index-contract.txt)

**Acceptance**
- `SemanticCandidateScope` collapses to a single canonical name (no overlapping `query_text` / `semantic_query_text`)
- `TextQueryRequest` and `SymbolQueryRequest` carry mandatory `top_k: u32` (or builder enforces). `HybridQueryRequest` already has `top_k` — keep
- SDK builder maps 1:1 to contract DTO. Zero `query_text: String::new()` and zero `semantic_query_text: String::new()` filler call-sites
- Manual serde visitor rejects missing `top_k` via `missing_field` (fail-closed)
- `public-api` baseline updated in the same PR

**Blockers** none (parallel to QI-ING-01, but coordinate merge order since both touch the contract crate)

**Proof**
- `cargo check -p quanta-index-contract && cargo check -p quanta-index-sdk`
- `just rust-profile test-fast` (request round-trip + missing-`top_k` rejection)
- `just rust-fuzz-smoke` (request envelope target: missing field handled as `Err`, no panic)
- `tools/ci/lint/check-public-api.py` (intentional drift + baseline update commit)

---

## Wave 2

### QI-SDK-01 | quanta-index | SDK Publish Transport Cutover

Add `IngestTransport`. `lexical().publish()` / `repomap().publish()` route through ingest IPC only. Public semantic publish is removed; semantic corpus materialization is search-owned. SDK drops its `quanta-index-channel` dependency.

**Owner files**
- [crates/quanta-index-sdk/src/transport.rs:11](../../crates/quanta-index-sdk/src/transport.rs#L11) (add `IngestTransport` trait + `UdsIngestTransport`)
- [crates/quanta-index-sdk/src/config.rs:7](../../crates/quanta-index-sdk/src/config.rs#L7) (add `ingest_socket` field + resolve)
- [crates/quanta-index-sdk/src/client.rs:34](../../crates/quanta-index-sdk/src/client.rs#L34) (wire ingest transport)
- [crates/quanta-index-sdk/src/lexical.rs:130](../../crates/quanta-index-sdk/src/lexical.rs#L130) (drop `open_lexical_publisher` → ingest IPC `PublishLexicalBatch`)
- [crates/quanta-index-sdk/src/semantic.rs](../../crates/quanta-index-sdk/src/semantic.rs) (query-only semantic namespace after public semantic publish removal)
- [crates/quanta-index-sdk/src/repomap.rs](../../crates/quanta-index-sdk/src/repomap.rs)
- [crates/quanta-index-sdk/Cargo.toml](../../crates/quanta-index-sdk/Cargo.toml) (drop `quanta-index-channel` / `quanta-index-lexical` / `quanta-index-semantic` deps)

**Acceptance**
- `cargo tree -p quanta-index-sdk | grep quanta-index-channel` is empty
- `publish()` maps batches to `PublishLexicalBatch` / `RepoMapBatch` and calls `IngestTransport::send()`
- semantic query/hybrid callers do not have a public semantic publish path; semantic authority is derived inside `searchd`
- `BatchReceipt` comes from the server response, never synthesized from local channel sequences
- An in-memory `IngestTransport` exists for unit tests. SDK tests must not produce filesystem channel side-effects
- future source-authority families (history / dirty / parse-tree) must land as dedicated SDK namespaces and typed batches, not raw ingress paths and not `LexicalBatch` overloads

**Blockers** QI-ING-01, QI-RT-01

**Proof**
- `cargo tree -p quanta-index-sdk` grep verification
- `just rust-profile test-fast` (SDK publish smoke with in-memory transport)
- `just rust-profile test-daemon` (real-socket end-to-end)
- `tools/ci/lint/check-cargo-toml-hygiene.py`

---

### QI-ACT-01 | quanta-index | Generation Admin Surface

Add `generations().current()` and `generations().status()` to both SDK and daemon. The activation catalog is the only truth source for active-generation selection, not a replacement for family-local readiness checks in the history / runtime / structural follow-on.

**Owner files**
- [crates/quanta-index-contract/src/ipc/split.rs:66](../../crates/quanta-index-contract/src/ipc/split.rs#L66) (`SearchPlaneControlIpcRequest`: add `CurrentGeneration` / `GenerationStatus`)
- [crates/quanta-index-contract/src/ipc/split.rs:78-84](../../crates/quanta-index-contract/src/ipc/split.rs#L78-L84) (`SearchPlaneControlIpcResponse`: add `GenerationSnapshot` / `GenerationStatusReport`)
- [crates/quanta-index-sdk/src/generations.rs](../../crates/quanta-index-sdk/src/generations.rs)
- [crates/quanta-index-searchd/src/app/ipc_dispatcher.rs](../../crates/quanta-index-searchd/src/app/ipc_dispatcher.rs)
- [crates/quanta-index-searchd/src/app/runtime.rs](../../crates/quanta-index-searchd/src/app/runtime.rs) (activation catalog port wiring)
- [tools/ci/lint/baselines/public-api/quanta-index-contract.txt](../../tools/ci/lint/baselines/public-api/quanta-index-contract.txt)

**Acceptance**
- `generations().current(repo_id)` returns the active generation from the activation catalog. Caller-side sqlite pin lookups are banned
- `generations().status(repo_id)` returns activation / materialization status for the base shipped families (lexical / semantic / repomap)
- follow-on history / runtime / structural families keep their own readiness authority; `generations().status()` is not the sole readiness gate for those families
- Catalog absent or no active generation → typed `Err` (`NotReady` / `NoActiveGeneration`). No `0` / `GenerationSelector::Latest` fallback
- `public-api` baseline updated

**Blockers** none structurally, but coordinate Wave 1 contract crate edits

**Proof**
- `cargo check -p quanta-index-contract && cargo check -p quanta-index-searchd && cargo check -p quanta-index-sdk`
- New `crates/quanta-index-searchd-runtime/tests/generation_status.rs` passes
- `tools/ci/lint/check-public-api.py` passes

---

### QI-RM-01 | quanta-index | RepoMap V1 Removal

Replace `RepoMap*V1` names with the stable typed surface.

**Owner files**
- [crates/quanta-index-contract/src/repomap.rs](../../crates/quanta-index-contract/src/repomap.rs) (rename all `*V1` DTOs to canonical names)
- [crates/quanta-index-contract/src/ipc/split.rs](../../crates/quanta-index-contract/src/ipc/split.rs) (`RepoMapSourceBundleV1`, `RepoMapActivateGenerationRequestV1`, `RepoMapQueryRequestV1`, `RepoMapQueryResponseV1`, `RepoMapMutationAckV1`)
- [crates/quanta-index-sdk/src/repomap.rs:1](../../crates/quanta-index-sdk/src/repomap.rs#L1)
- [crates/quanta-index-sdk/src/lib.rs](../../crates/quanta-index-sdk/src/lib.rs) (re-exports)
- [tools/ci/lint/baselines/public-api/quanta-index-contract.txt](../../tools/ci/lint/baselines/public-api/quanta-index-contract.txt)
- [tools/ci/lint/baselines/cargo-modules/](../../tools/ci/lint/baselines/cargo-modules/)

**Acceptance**
- Zero `V1` suffix types in the public surface of `quanta-index-contract` or `quanta-index-sdk`
- All callers use the stable names
- Wire shape unchanged (rename only). Any intentional wire break is recorded in the baseline diff
- No dual-name shim that violates breaking-change posture (CLAUDE.md "breaking-first")

**Blockers** none (kickoff once Wave 1 lands)

**Proof**
- `rg "V1\b" crates/quanta-index-contract/src crates/quanta-index-sdk/src` returns zero non-fixture hits
- `cargo check --workspace`
- `tools/ci/lint/check-public-api.py` passes with baseline update

---

## Wave 3 — semantica owner (only the quanta-index enablement is recorded here)

### SM-PUB-01 | semantica | Producer Batch Publish via SDK

Producer authors batches through `quanta-index-sdk` only. Outbox stores a typed artifact.

**Owner files** semantica repo (out of scope)

**Acceptance** tracked in the semantica repo issue

**Blockers** QI-SDK-01

**Proof** quanta-index side guarantees SDK supports idempotency-keyed, replay-safe batch publish via QI-SDK-01 acceptance + smoke tests

---

### SM-ACT-01 | semantica | Publish / Activate Split

Decouple local publish success from active-generation promotion.

**Owner files** semantica repo

**Acceptance** semantica repo issue

**Blockers** QI-ACT-01

**Proof** quanta-index side: control IPC end-to-end test asserts `generations().activate()` is replay-safe (duplicate calls return a typed ack)

---

### SM-QRY-01 | semantica | Query Caller SDK Cutover

Callers use SDK namespace calls only. Direct split-IPC envelope assembly and socket path glue are removed.

**Owner files** semantica repo

**Acceptance** semantica repo issue

**Blockers** QI-QRY-01

**Proof** quanta-index side: SDK builder maps 1:1 to contract DTO via QI-QRY-01 acceptance

---

## Wave 4

### SM-DEL-01 | semantica | Fallback and SQLite Pin Deletion

Delete local lexical / semantic / hybrid fallback and caller-side sqlite generation pin resolution. `GenerationSelector::Active` remains for caller-side coordinate selection; readiness is resolved by the SDK/query path and family-local search-side authority, not caller-side catalog/sqlite heuristics.

**Owner files** semantica repo

**Acceptance** semantica repo issue

**Blockers** SM-PUB-01, SM-ACT-01, SM-QRY-01

**Proof** semantica repo issue

---

### QI-INT-01 | quanta-index | Internal Crate Quarantine

Demote `quanta-index-contract` / `quanta-index-channel` / `quanta-index-ipc` from the external integration surface. `LexicalChannelOp` / `SemanticChannelOp` stop being an owner surface.

**Owner files**
- [crates/quanta-index-contract/src/lib.rs:30](../../crates/quanta-index-contract/src/lib.rs#L30) (trim `pub use`)
- [crates/quanta-index-sdk/src/lib.rs](../../crates/quanta-index-sdk/src/lib.rs) (drop re-exports)
- [tools/prompt-manager/sources/](../../tools/prompt-manager/sources/) (architectural docs)
- [docs/plans/may-24-lexical-indexing-sorucegraph/tickets/PRE-CONTRACT-EXT.md](may-24-lexical-indexing-sorucegraph/tickets/PRE-CONTRACT-EXT.md)
- [tools/ci/lint/baselines/public-api/quanta-index-contract.txt](../../tools/ci/lint/baselines/public-api/quanta-index-contract.txt) (reduced baseline)

**Acceptance**
- `quanta-index-sdk` public surface exposes only batch / query request / response / error / `GenerationSelector` / `GenerationPin` DTOs. Zero `ChannelOp` variants
- `quanta-index-contract` doc-comments label `channel::*` as "internal — used by searchd composition root only"
- External consumers importing `quanta-index-channel` is treated as a regression — recorded in `tools/prompt-manager/sources/`
- `public-api` baseline updated to the reduced surface

**Blockers** QI-SDK-01 plus all SM-* tickets (consumers cut over first)

**Proof**
- `tools/ci/lint/check-public-api.py` passes against the reduced baseline
- `cargo check --workspace`
- `rg "LexicalChannelOp|SemanticChannelOp" crates/quanta-index-sdk/src` returns zero pub-surface hits
- `pm.py sync && pm.py lint` passes

---

## Wave 5

### QI-NS-01 | quanta-index | Generic Namespace Registry

Add an `ns::<N>()` extension registry. Built-in `lexical()` / `semantic()` / `repomap()` become sugar.

**Owner files**
- [crates/quanta-index-sdk/src/lib.rs](../../crates/quanta-index-sdk/src/lib.rs) (`Namespace` trait, `ns<N>()` entry)
- [crates/quanta-index-sdk/src/client.rs](../../crates/quanta-index-sdk/src/client.rs) (`QuantaIndex::ns` method)
- [crates/quanta-index-sdk/src/lexical.rs](../../crates/quanta-index-sdk/src/lexical.rs), [semantic.rs](../../crates/quanta-index-sdk/src/semantic.rs), [repomap.rs](../../crates/quanta-index-sdk/src/repomap.rs) (refactor to `Namespace` impls)

**Acceptance**
- `pub trait Namespace { type Query; type Batch; type Receipt; ... }` — split into narrow traits per ISP (query / publish / status are not one fat trait)
- `QuantaIndex::ns::<N>()` returns a `NamespaceHandle<N>` wired through ingest / query transports
- Adding a new namespace requires: `impl Namespace`, one typed contract DTO, and one ingest / query variant — not four parallel SDK / dispatcher edits (CLAUDE.md OCP rule)
- Existing `lexical()` / `semantic()` / `repomap()` behavior unchanged (sugar layer retained)

**Blockers** QI-SDK-01, QI-ACT-01, QI-RM-01, QI-INT-01

**Proof**
- `cargo check -p quanta-index-sdk`
- SDK example `ns::<MyDerived>().publish(batch)` / `ns::<MyDerived>().query()` compiles
- `just rust-profile test-fast`
- `tools/ci/lint/check-llvm-lines.py` (generic expansion does not blow up monomorphization budget)

---

### QI-VRF-01 | quanta-index | End-to-End Proof

Close the contract serde / ingest / query / control SDK smoke / searchd e2e proof on the quanta-index side. The semantica replay / idempotency / no-fallback proof is tracked as `SM-VRF-01` in the semantica repo.

**Owner files**
- [crates/quanta-index-contract/tests/](../../crates/quanta-index-contract/tests/) (round-trip every `SearchPlaneIngest/Query/Control` variant)
- [crates/quanta-index-sdk/tests/](../../crates/quanta-index-sdk/tests/)
- [crates/quanta-index-searchd-runtime/tests/](../../crates/quanta-index-searchd-runtime/tests/) (integration e2e across `ingest.sock` + `query.sock` + `control.sock`)
- [tools/ci/lint/baselines/public-api/quanta-index-contract.txt](../../tools/ci/lint/baselines/public-api/quanta-index-contract.txt) (frozen)
- [tools/ci/lint/baselines/cargo-modules/](../../tools/ci/lint/baselines/cargo-modules/) (frozen)

**Acceptance**
- Contract serde round-trips for every `SearchPlaneIngest/Query/Control` variant
- `searchd-runtime` e2e: publish via `ingest.sock` → `generations().current()` / base-family `generations().status()` return the expected active/materialized state → query via `query.sock` returns expected result → control via `control.sock` activates correctly
- `cargo tree -p quanta-index-sdk` contains zero `quanta-index-channel` entries
- All structural lints pass on frozen baselines: `check-public-api.py`, `check-cargo-modules-snapshot.py`, `check-cargo-toml-hygiene.py`, `check-module-discipline.py`, `check-error-shape.py`, `check-digest-fallibility.py`, `check-llvm-lines.py`
- `just rust-fuzz-smoke` runs the ingest / query / control envelope targets for 60s each. Zero panic, hang, or non-`Err` exit

**Blockers** all Wave 1–4 tickets

**Proof**
- `just rust-profile test-fast / test-integration / test-cli-smoke / test-daemon`
- `just rust-bench` (no criterion regression)
- `just rust-profile verify-rust-heavy` (Miri / careful / tsan / asan / mutants / udeps)
