# RB-02 — Independent Real-Repository SDK Runner

Status: `planned`

Depends on: RB-00 stage A

Owner: benchmark-only Rust package; product SDK is a dependency, not a fork

## Goal

Run `pinned repository → ChunkRecord → Quanta SDK publish/activate → Quanta SDK query` without Semantica, direct IPC, or the fixture `E2eRuntime::ingest_text*` helpers.

## Work

1. Add `benchmarks/retrieval` as an explicit Cargo workspace member with no production crate depending on it. Load only the RB-00 admitted tracked files from a clean checkout; never infer corpus from whatever happens to be on disk.
2. Start a real searchd process in an isolated, explicit state root and wait for readiness; use `QuantaIndex::connect(ConnectOptions::from_state_root(...))`. Do not use an in-process adapter timing as daemon/SDK latency. Ensure bounded shutdown and cleanup of only runner-owned temporary state. Verify initial state is empty to exclude cached-index reuse.
3. Assemble `SearchCorpusBatch::replace_generation`, `replace_scope` and, for qualified semantic profiles, `replace_semantic_scope` from chunker output. Populate real byte/line bounds, text, stable IDs, metadata, scope/manifest digests and repo/revision/generation identity. Call `client.search_corpus().publish_and_activate(&batch, expected_active)` and validate both `BatchReceipt` and composite activation ACK before querying. Structural/symbol tracks must be either supplied via the matching SDK contract or declared out of scope for that profile; no implicit substitute.
4. Query lexical, semantic and hybrid routes through SDK namespaces using the same query pack and declared `top_k`. Preserve typed errors, unavailable/degraded status, timeouts and actual result spans. No silent fallback to another route.
5. Emit a deterministic, schema-validated runner record, raw per-query durations and phase timings. Keep gold inaccessible to this process; record the run's `blinding`, `isolation_method`, and `access_block_log`. Use an explicit admitted-file manifest; never process extra tracked files merely because they exist.

## Planned files

- `Cargo.toml` (workspace membership only)
- `benchmarks/retrieval/Cargo.toml`
- `benchmarks/retrieval/src/{main,corpus,batch,sdk,record}.rs` — this ticket is the single owner of `record.rs`; RB-05 consumes its records read-only and must not edit it
- `benchmarks/retrieval/tests/sdk_roundtrip.rs`

## Acceptance / verification

- Static guard/tests prove runner imports `quanta-index-sdk` and does not call direct `SearchPlaneIngestIpcRequest`, `E2eRuntime::ingest_text*`, or a synthetic result provider.
- A real multi-file/multi-chunk corpus publishes, activates, survives a reader query, and returns spans within original file bounds; malformed metadata and failed activation refuse scoring.
- [TEST-PLAN.md](TEST-PLAN.md) T05–T07 process scenarios cover wrong state root, activation conflict, model unavailable, timeout, stale index and bounded shutdown.
- All per-route records are bound to exact source SHA, corpus digest, strategy/model config, binary and SDK receipt. Retry/replay does not change authority claims.
- Use `./scripts/cargow test -p quanta-index-retrieval-bench` and owner-local integration tests once the package exists. SDK public-surface changes, if truly required, separately trigger `just rust-public-api`; none are assumed by this ticket.
