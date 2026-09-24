# RFC: Quanta Index Rust SDK interface, scoped to current contracts

- Status: **proposed; adversarial revision**. This document implements no SDK, wire, daemon, or producer change.
- Source audited: the original SDK review used Quanta HEAD `1a1458f00652ab53d2bcc170304b4d26ba2556cb`. The external producer-input boundary in §8 was rechecked at `a0ac1853256d9b507ae8dc76f7437a4c7568434c` on 2026-09-24; the later `0743cda05fb07f38442eec3d0233daf9d3a5b3dd` commit changed only `Justfile` and a retrieval-proof test. The checkout has concurrent dirty work. Semantica `index_sdk_ingress` was read as a consumer, but its revision and dirty-state identity were not frozen. Re-freeze both before implementation or qualification.
- Scope: Rust SDK over local query, ingest, and control UDS planes; not an in-process Tantivy/LanceDB handle.
- Related: [search configuration RFC](sep-23-search-config-profiles/rfc.md), [earlier SDK target](sep-23-search-config-profiles/sdk-interface.md). This RFC supersedes the earlier target where its public-shape proposals conflict. Server profile/config policy stays with the configuration RFC.

## 1. Purpose and supported functions

The SDK turns caller intent into typed IPC requests, sends each request to its plane, and checks that the response belongs to that request and its selected generation. The daemon owns indexing, semantic derivation, ranking, admission, and durable activation. The SDK covers lexical, semantic, hybrid, and hybrid-seed search; symbol/history/runtime/structural/repo-map reads; typed corpus publishing; generation control; and diagnostics. Search consumers need only the query socket. Producer/control use separate sockets and OS permissions.

Do not model this as an arbitrary query language, a client-side ranker, a server-config editor, or a per-request embedding-model selector. New search behavior needs a server contract and evidence, not a fluent setter alone.

## 2. Adversarial audit of the previous proposal

| Previous proposal | Current-source evidence | Decision |
|---|---|---|
| Mandatory owned `ReaderClient`/`OperatorClient`; remove `QuantaIndex`/`ClientProfile` | `client.rs` already has `connect_query_only`, borrowed `reader`/`producer`/`control` views, and typed `PlaneUnavailable`; `config.rs` constructs no dummy control/ingest transport in query-only mode. No observed misuse establishes a need for owned role types. | **Drop.** Keep existing client and views. OS permissions remain authoritative. |
| Replace all const-generic builders | `lexical.rs` and `semantic.rs` use typestate to prevent `execute` before required text, selection, and limit. Replacing history/runtime/structural builders would remove compile-time checks and cause broad migration without measured benefit. | **Drop wholesale rewrite.** Keep typestate for required inputs. Narrow only the semantic lexical-scope pair if implemented. |
| New `SearchScope`, `Filter`, `CandidateLimit`, `CorpusScope`, `ExpectedActive`, `PreparedRequest`, page wrappers | Existing `GenerationSelector`, `QueryConstraintSetV1`, route DTOs, and canonical IPC requests cover current semantics. Proposed filter merge rules differ from current setter replacement behavior. | **Drop.** Reuse existing types/results; no SDK AST or implicit filter algebra. |
| Internal sealed operation-descriptor framework | `binding.rs` already publishes `SDK_WIRE_ROUTES_V1`; `client.rs` has explicit plane dispatch/exhaustive response matches; `tests/sdk_binding_owner_v1.rs` covers adversarial binding. A descriptor risks a second route inventory. | **Drop.** Extend existing inventory/binder for a new route; factor only a concrete duplicated invariant. |
| Explicit seal type hierarchy and `PublishedCorpus` hierarchy | `SearchCorpusBatch<true>` plus `.without_seal()` encodes seal choice; `publish` validates the receipt. No production misuse of the default was established. | **Defer.** Document default; keep unsealed explicit. |
| One-call `publish_and_activate` as primary workflow | `lexical.rs` publishes and obtains a validated receipt, then uses `?` on candidate/CAS paths. A later error returns `SdkError` alone, losing receipt in the return value. Semantica calls this in two `index_sdk_ingress/publish.rs` paths. | **Fix this boundary.** Provide a safe two-stage path; migrate producer before removing/changing the helper. |
| Semantic scoped-rerank syntax | `semantic.rs` requires `scope_native`/`scope_sourcegraph` **and** `scope_top_k`; `text_query_builder.rs` rejects a half-configured pair. Hybrid is a distinct independent-recall route. | **Simplify narrowly.** One lexical-scope setter carries syntax, text, and candidate cap; same wire route. |

This source audit identifies one **partial-success contract gap** and one **ergonomic complexity**. It does not prove a throughput issue, route correctness failure, or need for a new transport layer. Source inspection cannot prove those negative claims under load.

## 3. Target SDK DSL

Keep `QuantaIndex::connect` and `QuantaIndex::connect_query_only`, existing route namespaces, `GenerationSelector`/`.active`/`.pinned`, `QueryConstraintSetV1`, route-specific responses, and SDK error variants. `ClientProfile` remains transport selection, distinct from server build/query profiles. Existing borrowed `reader`/`producer`/`control` views aid discoverability; they are not a security claim.

Current lexical and independent hybrid calls remain valid. The *only proposed query DSL change* is a composite semantic scope setter. Illustrative code; `within_native` is **not implemented today**:

```rust
let client = QuantaIndex::connect_query_only(options)?;

let lexical = client.lexical().query()
    .native("symbol:Parser")
    .active(repo_id.clone(), revision_id.clone())
    .top_k(20)
    .execute()?;

let reranked = client.semantic().query()
    .text("how is parsing recovered?")
    .within_native("parser", 100) // lexical candidate cap
    .active(repo_id.clone(), revision_id.clone())
    .top_k(20)                      // final semantic result cap
    .execute()?;

let hybrid = client.search().hybrid()
    .native("parser")
    .semantic_text("error recovery")
    .active(repo_id, revision_id)
    .top_k(20)
    .execute()?;
```

`within_native(text, candidate_cap)` and `within_sourcegraph(text, candidate_cap)` would be SDK-only setters for the existing semantic request's lexical scope. Set both scope fields in one transition; preserve syntax, constraints, selector, route, and cap validation before I/O. No new `SearchScope` or query-text parser. Keep distinct names for scoped semantic rerank and independent hybrid fusion. Native/Sourcegraph text is **query syntax**, not escaped literal-safe text. Unscoped semantic retains `.text(...).active/pinned(...).top_k(...).execute()`.

The old `scope_native`/`scope_sourcegraph` plus `scope_top_k` sequence can remain temporarily for source compatibility. If removed in a permitted breaking window, delete both in one coordinated cutover and update all call sites; no permanent dual DSL. Keep required-input typestate. A composite setter may reduce the two scope flags to one; this does not justify rewriting unrelated builders.

## 4. Publish and activation: visible two-stage result

Add a narrow `SearchCorpusNamespace::activate_published(&SearchCorpusBatch<true>, &BatchReceipt, expected_active)` method. It must revalidate the receipt against the exact batch/digest/seal before deriving the composite candidate from batch identity and receipt semantic content roots. Prevalidate the CAS request, dispatch on control, and exact-bind the acknowledgement. Local receipt comparison is **not authentication** of a caller-supplied receipt: the daemon must still prove the candidate matches the physically sealed generation before promotion. Batch/receipt references stay with the caller if activation refuses or transport result is unknown. Proposed flow:

```rust
let batch = SearchCorpusBatch::replace_generation(repo, revision, generation, manifest_digest)
    .replace_scope(scope, scope_digest, chunks, symbols);
let receipt = client.search_corpus().publish(&batch)?;
// Keep receipt and batch in producer recovery context before CAS.
let activation = client.search_corpus()
    .activate_published(&batch, &receipt, expected_active);
```

This is two calls and **not atomic**. CAS refusal means sealed/published but not promoted; CAS transport error means unknown until status/reconciliation. Publish transport error is likewise unknown until deterministic replay/receipt reconciliation. Do not infer failure from absent acknowledgement. The SDK cannot guarantee crash-safe receipt retention: the producer must persist or reconstruct the same canonical batch and reconcile after restart. Local pre-publish validation failure differs from every post-publish error.

`publish_and_activate` hides the receipt on a post-publish `Err`. Migrate its two Semantica callers to two-stage flow and preserve receipt/recovery metadata in their error/result contract. Then remove the helper in the intentional breaking cutover, or retain it only with a typed staged outcome that preserves all partial states. Avoid a generic workflow/result framework solely for this case. Do not expose raw activation that can pair a candidate with an unrelated receipt.

## 5. Stable extension rules

| Change | Owner and admission rule |
|---|---|
| Convenience method or optional bound expressible in current contract | SDK builder lowers to one existing typed request; preserve route, canonical body, binding, and request count. |
| New server-backed filter, budget, or pagination option | Typed contract field/route plus validation, continuation compatibility, daemon support, negative binding tests, and effective-policy visibility where relevant. No generic `extras` bag. |
| New model/ranking/corpus semantics | Explicit server recipe/profile/generation compatibility decision with quality/cost evidence; never a silent SDK default. |
| New route | Exhaustive IPC variant/codec, daemon dispatcher, `SDK_WIRE_ROUTES_V1`, exact binder, SDK entrypoint, and real-daemon scenario. |

All-public wire DTOs constrain source evolution, but hiding every DTO behind new private result types creates immediate cross-repository churn. Keep the typed contract crate and SDK re-exports for this cutover. Use private fields on **new SDK-only** types when needed; revisit raw DTO exposure when a specific extension is blocked. Preserve `SdkError` variants and remote metadata; Semantica matches variants exhaustively. No automatic retry of CAS or paid semantic calls.

Keep one canonical IPC request per operation. Builder state is temporary construction state, not a second semantic IR. `execute`/`publish` is the I/O boundary. Existing active-generation resolution can add a documented query-plane RPC; new setters add none. No async twin, streaming layer, macro DSL, or code generation here. Performance objective: no extra RPC or large-batch clone from ergonomics; measure against the same daemon before claiming speedup.

## 6. Implementation boundaries and integration

| Owner | Required work |
|---|---|
| `crates/quanta-index-sdk/src/semantic.rs`, `src/text_query_builder.rs`, `src/tests.rs` | Add composite scope setter(s); preserve typestate/exact lowering. Remove old setters only if source break selected. Test lexical cap versus final cap and route identity. |
| `crates/quanta-index-sdk/src/lexical.rs`, `src/client.rs`, `src/tests.rs` | Extract existing validated candidate/CAS path into `activate_published`; revalidate supplied receipt; retain caller batch/receipt on failure. Remove/redesign one-call helper after migration. `client.rs` changes only if producer view forwards new method. |
| `crates/quanta-index-sdk/src/binding.rs`, `tests/sdk_binding_owner_v1.rs` | No new descriptor. Extend tests only for new path and uncovered wrong receipt/CAS acknowledgement; retain exact binding. |
| Semantica `packages/analysis/quanta-v2/crates/quanta-runtime-retrieval-kernel/src/index_sdk_ingress/{publish.rs,facade.rs}` and related result/error consumers | Re-freeze Semantica; migrate two one-call paths; carry validated receipt and unknown/refused activation state through recovery. Inspect all SDK usages/exhaustive error matches before source break. |
| Contract/daemon/config | **No change expected** for these SDK-only operations. Reopen if canonical equivalence or safe recovery cannot be met with current contract. |

Sequence:

1. Freeze both revisions, dirty-path ownership, SDK dependency path/version, and producer binary/config. Inventory call sites and produce a minimal old→new map. Do not stage concurrent work.
2. Add composite semantic setter against current IPC. Compare independently built canonical fixtures for unscoped semantic, scoped semantic, and hybrid; prove cap, constraints, and selector preservation. Keep binding negative matrix.
3. Add `activate_published` by extracting current receipt/candidate/CAS validation. Exercise mismatched batch/receipt, missing or forged semantic roots, CAS refusal, wrong acknowledgement, and control transport uncertainty. A forged but locally well-formed receipt must still be refused by the daemon's sealed-root proof. Keep receipt in caller state on every post-publish path.
4. Migrate Semantica's two production paths and recovery/result contract. Run publish → sealed receipt → CAS conflict → reconcile/retry; publish → CAS transport loss → status/reconciliation; and restart after publish before CAS. In-memory receipt alone is not restart recovery.
5. In one coordinated source break, remove unsafe one-call helper and optionally old half-scope setters. Run `just rust-public-api`; focused SDK tests; `tests/sdk_binding_owner_v1.rs`; real-daemon SDK frontdoor/lifecycle; source-bound Semantica build/integration. Use `Justfile` and `./scripts/cargow`. Broad qualification, Linux/production activation, and benchmark validity are separate receipts.

Stop/reopen if supplied receipt cannot be validated against its batch, Semantica cannot carry a partial publish outcome, active-selection semantics change, an SDK setter adds a hidden RPC, or a wire/profile change becomes necessary. Write the changed contract first; do not stack compatibility shims or claim DSL-only equivalence.

## 7. External API precedents, with limits

| Reference | Useful precedent | Boundary here |
|---|---|---|
| [Elasticsearch Rust client](https://www.elastic.co/guide/en/elasticsearch/client/rust-api/current/overview.html) | Endpoint-specific fluent builders. | Existing Quanta namespaces already provide this; no new root DSL required. |
| [Qdrant `QueryPointsBuilder`](https://docs.rs/qdrant-client/latest/qdrant_client/qdrant/struct.QueryPointsBuilder.html) | Typed limits/options around one operation. | Quanta scoped rerank and hybrid remain distinct. |
| [Cargo SemVer guide](https://doc.rust-lang.org/cargo/reference/semver.html) | Public shapes have source-compatibility cost. | A permitted break still has migration cost; change only demonstrated pain points. |

These references are design examples, not proof of Quanta correctness/performance. This RFC is a static source audit. Implementation, execution, integration, and production behavior are **NOT_RUN** here.

## 8. External producer input boundary

The supported input is **producer-authored indexed material**, not an arbitrary raw-source upload. A producer creates stable `ChunkRecord` IDs, paths, byte/line spans, text and optional structural metadata, then sends `SearchCorpusBatch::replace_scope`/`tombstone_scope`/`clear_surface` under a generation. For semantic search, it may send typed `SemanticSourceRecordV1` groups through `replace_semantic_scope`; the search plane validates those sources and derives embeddings with the daemon's model. The batch carries shared generation, digest, mutation and seal identity. The daemon owns lexical/semantic materialization, idempotency, receipts and activation.

| Input | Current support | Boundary |
|---|---|---|
| External pre-chunked code/text | `ChunkRecord` inside `SearchCorpusReplaceScope` | Producer chooses boundaries and IDs; Quanta validates/indexes them. Quanta does not parse arbitrary source into chunks. |
| External semantic cards/sections/summaries | `SemanticSourceRecordV1` inside `SemanticSourceReplaceScopeV1` | Producer owns source text, owner/provenance and render-policy identity; Quanta owns embedding and sealed generation. The corpus-kind vocabulary is closed and has kind-specific rules. |
| Raw file bytes with automatic chunking | No general SDK entrypoint. Wire has `bundle_payload`, but `SearchCorpusBatch` emits `None` and the Semantica SDK ingress rejects non-`None`. | Do not advertise this as a generic source-ingest API or infer its semantics from the optional wire field. |

Commonality is at the **batch lifecycle** (scope mutations, canonical digest, resource preflight, idempotent publish, seal, receipt and CAS). Representation is deliberately separate: lexical chunks/symbols, semantic source records/cluster memberships, and other domain batches have different authority and validation. `SearchCorpusBatch` should remain the shared SDK gateway for search-corpus ingest; do not introduce a schema-free `Source`, universal `Document`, or pluggable chunker in the daemon. A new producer can map its own parser/chunker output to existing records without changing the SDK. A genuinely new searchable corpus kind, provenance field or ranking behavior requires explicit contract/daemon changes and migration proof.

Current working-tree `semantic_derive.rs` consumes only typed semantic sources; an empty typed-source set is a semantic no-op, not a chunk-text fallback. `ChunkRecord` has `source_repo_id` while `SemanticSourceRecordV1` has no parallel field, so a multi-repository semantic-source producer needs an explicit provenance decision. Re-freeze this concurrent source before implementation.

When onboarding a second external producer, first add producer-local mapping and fixtures, then prove stable IDs/spans, deterministic batch digest, replacement/tombstone/delete behavior, typed-source validation or explicit semantic no-op, receipt replay and publish→activate→query→restart. Owners: `crates/quanta-index-contract/src/{channel/records.rs,ipc/ingest.rs,ipc/semantic_source.rs}` for shared input; `crates/quanta-index-sdk/src/lexical.rs` for the SDK gateway; `crates/quanta-index-search-plane/src/{semantic_derive.rs,ingest_dispatcher/search_corpus.rs}` for build behavior. Change these only for a demonstrated missing contract, not to accommodate a producer's private parsing algorithm.

## 9. Decision on optional source preparation

Producer-owned **semantic** chunking remains the authority for code/HIR/cards: the index cannot infer parser symbols, graph ownership or card provenance from raw bytes. The earlier wording was too absolute for ordinary text sources. If Quanta is to accept raw Markdown/plain text from another producer, add an **optional preparation helper before `SearchCorpusBatch`** that emits both lexical chunks and typed `DocumentLeaf` semantic sources. The caller can still supply prebuilt records unchanged. The helper does not run inside `searchd`, mutate a generation, choose an embedding model or create a second ingest protocol. The detailed target is [source preparation RFC](sep-24-source-preparation-sdk-rfc.md).

Proposed shape, not an implemented API:

```rust
let key = SourceKey::new("docs/guide.md", "guide-001")?;
let prepared = source_prep::prepare_text(
    TextSource::markdown(repo.clone(), revision.clone(), key, text)?,
    &TextPreparationPolicy::v1(),
)?;
let batch = SearchCorpusBatch::replace_generation(repo, revision, generation, manifest_digest)
    .replace_prepared(prepared)?;
let receipt = client.search_corpus().publish(&batch)?;
```

Start with one maintained text policy; keep language/parser-specific code chunkers in their producers. The helper must produce exact UTF-8 byte and line spans for lexical chunks, typed semantic document leaves, deterministic IDs/order, bounded size and explicit policy identity bound by the producer into its manifest/scope digest. The existing wire does not independently attest that policy: document and test how the producer computes the digest before claiming reproducibility. A parse failure must not silently switch a semantic-code source to generic text chunks. Do not infer semantic code cards from raw text.

Implement this helper only alongside a concrete raw-text producer/onboarding scenario; a library-only helper with no caller adds API maintenance without demonstrated value. Place it in a small producer-side module or separate prep crate if dependencies require it, then feed the existing SDK batch. Validate against independent fixtures for empty/Unicode/long text, stable IDs and spans, changed policy/content, scoped replacement/deletion, replay and query results. Do not touch `semantic_derive.rs` or the IPC contract unless one of those scenarios proves a missing server responsibility.
