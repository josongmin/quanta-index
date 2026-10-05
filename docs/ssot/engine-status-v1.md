# Search-plane source map and capability boundaries

Authority: current source. This is navigation and explicit capability limits,
not a current-source runtime/release receipt. Accepted decisions and current
residual owners are separate in [the documentation index](../README.md).

## Live source path

| Boundary | Owner |
| --- | --- |
| SDK ingest and query | [SDK](../../crates/quanta-index-sdk/README.md) |
| Fenced idempotent publication | [Ingest dispatcher](../../crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs) and [search-corpus materializer](../../crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs) |
| Lexical/semantic materialization | [Lexical](../../crates/quanta-index-lexical/src/lib.rs), [semantic](../../crates/quanta-index-semantic/src/lib.rs) |
| Prepared lexical/semantic pair CAS activate/rollback | [Lifecycle](../../crates/quanta-index-search-plane/src/search_corpus_lifecycle.rs); publication alone does not activate |
| Immutable query view and lifetime | [Query dispatcher](../../crates/quanta-index-search-plane/src/query_dispatcher/mod.rs), [snapshot registry](../../crates/quanta-index-search-plane/src/snapshot_registry.rs) |
| UDS serve, runtime/maintenance and diagnostics | [Daemon composition](../../crates/quanta-index-searchd/src/app/runtime.rs) |
| Recovery/idempotency authority | [Catalog](../../crates/quanta-index-catalog/README.md) and [recovery ADR](../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md) |

Lexical, semantic exact/ANN and hybrid routes have existing implementation.
Actual wire variants/selectors are code-owned inventories; static route counts
are not maintained here. Structural/runtime routes retain their live subset
authority rather than implying a missing producer crate.

## Explicit capability limits

| Boundary | Source / interpretation |
| --- | --- |
| Default semantic embedder | [query_embedder.rs](../../crates/quanta-index-search-plane/src/query_embedder.rs): `search-owned-hash-text-v1`, FNV 64-d. Hash proof does not qualify opt-in PotionCode/OpenAI provider identity, egress or quality. |
| Lexical structural-block leaf | [planner.rs](../../crates/quanta-index-lexical/src/planner.rs): typed `structural_block_leaf` refusal |
| Selected history primitives | [text_plane.rs](../../crates/quanta-index-search-plane/src/query_dispatcher/text_plane.rs): explicit `NotImplemented` for unsupported filters/leaves |
| Rust structural grammar | [LangId](../../crates/quanta-index-lq-structural/src/types.rs): deferred grammar declaration; distinct from the benchmark's independent declaration parser |
| Old persisted formats | Owner seal/catalog decoders refuse unsupported formats; [state runbook](../operator/state-cutover-runbook.md) owns explicit current-format rebuild/restore |

## Implemented behavior versus remaining work

Active selection/view acquisition, seven single-request SDK routes, separated
disk metering/freshness cadence, and admitted timeout-to-durable-terminal replay
are implemented under [OCT-05-003](../adr/OCT-05-003-active-query-and-runtime-lifecycle.md).
Selection itself does not create a lifetime lease; acquired views retain handles.
Stronger pin or resource claims keep their [proposed scope](../adr/OCT-04-001-search-corpus-selection-and-ingest-pressure.md)
and current E3 owner rather than the retired unexecuted-counterexample wording.

[OCT-04](../plans/oct-4-parallel-closure/tickets/INDEX.md) owns conditional
optimization, missing inputs and remaining execution.
[SEP-21](../plans/sep-21-search-plane-sota-hardening/tickets/CURRENT-RESIDUAL-2026-09-26.md)
owns installed/paired/Linux/provider/release acceptance. P11 additionally lacks
typed deploy/activate/restore-forward producers and recipes under
[S21-12](../plans/sep-21-search-plane-sota-hardening/tickets/S21-12-cross-repo-terminal-receipt-cutover.md).
Existing IPC/CAS handlers do not close that operational code/observer contract.
Local tests, code presence and documentation cleanup do not issue qualification.
