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
| Default semantic embedder | [Daemon config](../../crates/quanta-index-searchd/src/app/config.rs): unset/empty `QUANTA_INDEX_EMBEDDER` selects `potion-code` with `Pinned512V1`; pinned model assets need [provisioning](../potion-code-embedder.md). Explicit `potion-code-full-v2` selects `FullLengthV2` and requires a newly indexed generation. Config selection does not qualify actual model execution or retrieval quality. |
| Development embedder | Explicit `hash-dev` and `from_test_state_root` select the development hash profile; the default 64-d FNV implementation is in [query_embedder.rs](../../crates/quanta-index-search-plane/src/query_embedder.rs). Hash tests do not qualify PotionCode/OpenAI identity, egress or quality. |
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
rather than reopening completed E3 implementation scopes.

[Index closeout](../plans/oct-22-finalizing/README.md) owns remaining code and conditional
optimization, missing inputs and remaining execution.
The lexical file authority stores immutable bucket packs and postings, publishes
its durable root before the sealed manifest, and reuses unchanged committed
objects on delta. Cold open validates the complete source/posting census and
retains a bounded term/range/hash directory; queries read admitted posting ranges
under one request work budget. Logical heap admission does not establish an RSS
bound. Architecture decisions are recorded in the accepted ADRs; source-specific
Large/XL failures and passes belong to
[VALIDATION](../history/oct-22-finalizing/oct-10-index-closeout/VALIDATION.md#performance-inputs).
Explicit timeout/retention diagnostic success does not close default capacity.
[E08](../plans/oct-22-finalizing/E08-integration-release.md)
owns installed/paired/Linux/provider/release work; [SEP-21 execution records](../history/oct-22-finalizing/oct-10-index-closeout/VALIDATION.md#release-and-consumer)
remain evidence. Independent expected semantic partition/omission checking before
dispatch is producer-owned under [E01](../plans/oct-22-finalizing/E01-producer-generation.md).
The R3 fixed test oracle has an implementation history; its actual Runtime/pair
execution is separate. Do not infer an absent Quanta engine or rewrite the oracle.
Supplied-scope duplicate/conflict validation is already implemented.
P11's common typed deploy/activate/restore-forward producer, recipes and
parser/checker/aggregate are implemented under
[OCT-05-004](../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#operational-actions).
[E08](../plans/oct-22-finalizing/E08-integration-release.md#release-inputs)
owns the [S21-12 contract](../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md#installed-paired-and-operational-acceptance)
for concrete target adapters, independent pre/post observers and authorized inputs.
The [existing cross-repo recipe](../../scripts/verify-repomap-cross-repo.sh) and
[paired component archive](../../tools/ci/paired_r5_result.py) bind caller/kernel
selection, canonical resolver mapping and CLI-owned completion receipts. Their
`runner-candidate-only` result is distinct from actual exact-pair qualification
and concrete operational target/observer acceptance. Current work status belongs
to E08; [I0-03 records](../history/oct-22-finalizing/oct-10-index-closeout/VALIDATION.md#release-and-consumer)
retain their source-scoped results.
Local tests, code presence and documentation cleanup do not issue qualification.
