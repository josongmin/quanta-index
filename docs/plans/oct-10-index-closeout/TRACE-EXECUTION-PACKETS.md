# Trace waterfall execution packets

Date: 2026-10-11. Status: **P00–P08 source-level reconciliation is complete. The isolated logical-source revision `47f904a` passed the release build and Large product/lifecycle capture (87.881 s, Complete/lossless timeline). XL FAILED the 900 s producer deadline after all ten phases and post-reopen queries; no product artifact/footer exists. Latest owned-root teardown repair `a833970` passed harness strict Clippy, 13 cleanup/custody/reopen tests, and one real small-scale trace test; it has no new Large/XL result. Full checkout qualification is not closed by combining the earlier failing whole suite with later focused successes. Quiet-host Off/Record pairs are BLOCKED; Perfetto UI NOT_RUN, upload approval pending**.

Earlier P09 managed-worktree results below are historical and source-specific:
Large timing was not qualified under contention; XL Off/Record workloads failed
the 600 s build-seal request deadline while the XL Record trace closed Complete.
They are not the later `47f904a` run. Current scope and evidence are maintained
in [VALIDATION.md](VALIDATION.md), separately from those earlier packet reports.

Contract SSOT: [TRACE-WATERFALL-PLAN.md](TRACE-WATERFALL-PLAN.md). This file assigns exact write permissions and protected surfaces. Logical repository root is `/Users/songmin/Documents/code-new/quanta-index`; P00–P09 were originally executed in `/Users/songmin/.codex/worktrees/trace-waterfall/quanta-index` at the same HEAD plus a different dirty tree; the current reconciliation operates in the logical main checkout. Paths below are relative to the logical repository root. A path omitted from every write set is read-only. Directories and globs never grant write permission.

## 1. Execution rules

- An `existing-files` list is an upper bound, not a request to touch every file. A listed file may only receive the packet's named hunk. `new-files` lists the only files that may be created by that packet.
- Preserve unrelated dirty hunks. No whole-file replacement, drive-by formatting, renames, `git reset/clean/checkout`, bulk baseline acceptance, or staging/committing the shared dirty snapshot.
- Before each packet, recheck HEAD, dirty ownership, function signatures, and callers. Design was based on `HEAD=8f11cad9587aec1668a9fb3f5f1f055337487071` plus the then-current dirty source. A HEAD-only checkout that omits dirty source is not equivalent.
- At packet start and handoff, capture `git status --short --untracked-files=all`; every changed path must resolve to exactly one packet allowlist, one of the two ROOT-only plan documents in §1, or the exact `Cargo.lock` exception. Any other path is no-touch. Review the diff at the named function/hunk level: an allowlisted filename does not authorize unrelated hunks. If a new required path is discovered, stop that hunk and amend this packet with its owner, exact path, allowed function, forbidden adjacent behavior, and narrow verification before editing it.
- Packet permission is calculated per active packet, not per repository plan: `touch(Pn) = that packet's exact existing-files + new-files list + its explicitly assigned Cargo.lock hunk, if any`. `no-touch(Pn) = every other path`, including paths allowlisted to a different packet, every frozen snapshot path in §6, and any newly discovered/unlisted path. An overlapping/shared path is writable only by the owner named in §2's handoff table and only during that handoff window.
- The lists below are the concrete file inventory: §3 gives each packet's exact writable paths and named hunk/behavior; each packet's `Read-only`/`Forbidden` clauses add protected surfaces; §6 enumerates the current dirty no-touch paths individually. No directory, sibling, glob, generated file, or existing dirty state grants implied permission. If a packet needs a path absent from its list, amend the packet before editing; do not treat this as an automatic expansion.
- **Effective permission is exact-path based:** for packet `Pn`, touch only that packet's §3 paths and the explicitly named hunk in each path; `Cargo.lock` is allowed only under §4. Every other repository path is no-touch for that packet, including another packet's allowlist, all §6 frozen paths, and clean/untracked files not listed. The per-packet `Read-only`/`Forbidden` clauses further restrict allowed files; they never expand the path list.
- P00 contract files freeze after handoff. A signature change requires a root amendment naming producer, consumer, exact files, and narrow verification before edits continue. No fake defaults, no-op stubs, or compatibility shims to hide an incomplete caller migration.
- P01→P02, P03→P04→P05, and P06→P07 are three independent sequential chains. Each file has one active writer. P08 starts only after explicit lane handoff. Root serializes heavy Rust builds through `test-daemon-lane`.
- Reports name exact files/functions/hunks, commands, exit/results and test counts, `NOT_RUN` scopes, and remaining callers. Compilation is not test success; source checks are not runtime qualification.

**Execution-document files allowed to ROOT only:**

```text
docs/plans/oct-10-index-closeout/TRACE-EXECUTION-PACKETS.md
docs/plans/oct-10-index-closeout/TRACE-WATERFALL-PLAN.md
```

These two files define this task's SSOT and design. No other plan, ADR, RFC, or validation document is in scope.
- Put transient logs, receipts, and binaries outside the checkout. Do not add one-off evidence files to the repository.

## 2. Order, ownership, and file handoff

The table below states the intended write purpose for each packet. The exact path lists in §3 are the only files that may be edited; an allowed file is still read-only outside its named function/hunk. Anything not explicitly listed in a packet write set is no-touch, including already-dirty files. The separate frozen snapshot at the end names the currently dirty no-touch paths individually.

| Packet | Files to touch, by role | Explicitly do not touch |
| --- | --- | --- |
| P00 | Only the exact P00 paths in §3: core context/lease contract, request-only diagnostic token serde, listed port signatures, schema v2 vocabulary, and contract tests. | Every response envelope; product observation/digest/usage/ack; port implementation bodies; ambient context; dependency graph. |
| P01 | Only the exact P01 paths in §3: SDK attempt creation/transport and IPC transport preface, request decode, server handling, and named tests. | Product request identity/response wire, admission, retry/timeout/connection policy, ring semantics, and frozen P00 files. |
| P02 | Only the exact P02 paths in §3: searchd recorder/lifecycle/runtime-builder plus the single existing-workspace `serde_json` edge if needed. | Config equality, state format, recovery/GC/active-proof policy, socket/lease semantics, harness writer, dependency versions/new crates. |
| P03 | Only the exact P03 paths in §3: lexical/semantic producer boundaries for build, seal/proof, object/map mutation, sync and adoption aggregates; listed producer tests. | Ranking/BM25 or embedding algorithms, persisted layouts/manifests, publication decisions, retention/query hunks, per-object spans. |
| P04 | Only the exact P04 paths in §3: retention/closure lock, history/reap, SQLite accounting, unique closure, inode census; named history tests. | Activation/retirement/repair authority, lock order, dedup/traversal/hash/SQL behavior, P03 producer and P05 query hunks. |
| P05 | Only the exact P05 paths in §3: query dispatch/read/verify/rank scopes, existing query-observation tests, parent propagation and error snapshots. | Query algorithms, candidate ordering/limits, result meaning, budget/cap behavior, P04 closure hunks, new fixtures/goldens. |
| P06 | Only the exact P06 paths in §3: harness capture lifetime, phase identity, observer lane, explanation preservation, same-process reopen context forwarding. | Benchmark inputs, timer boundaries, assertions/caps, searchd writer, Python exporter, runtime test suites. |
| P07 | Only the exact P07 paths in §3: capture/export/parser/profile Python modules, explicitly listed schema changes, and parser/export tests. | Rust except the explicitly listed schema contract; benchmark inputs/gold/adapters/evaluator; binary binding/failure policy/caps; existing artifacts. |
| P08 | Only the exact P08 caller/test/baseline paths in §3 and their named hunks: mechanical forwarding, two process regressions, parity-gated marker removal, four baselines. | Feature behavior, fixtures/gold/budgets/ranking, CI authority/registries, all other baselines, unfinished lane-owned hunks. |
| P09 | No files. Validation only. | All source/config/test edits; any repair returns to its owning packet through a new exact handoff. |

**Frozen snapshot classification (2026-10-11):** the current 293 changed paths split into 149 paths covered by a packet write set or the exact §4 `Cargo.lock` exception, 2 ROOT-only plan documents listed in §1, and 142 paths frozen as no-touch in §6. There are no unclassified paths. Packet write sets overlap at documented handoff files, so these counts are repository paths counted once, not a sum of per-packet lists. P09 has no writable paths. Recompute this classification at every handoff; these counts are not standing permission.

| Packet | Owner | Precondition | Output |
| --- | --- | --- | --- |
| P00 | ROOT-CONTRACT | freeze HEAD/dirty ownership | typed context, attempt token, stages/counters, schema, explicit port signatures |
| P01 | A-TRANSPORT | P00 | client attempt → validated IPC request correlation |
| P02 | A-RUNTIME | P01 | bounded recorder, close barrier, daemon lifecycle, parent-aware runtime builder |
| P03 | B-INDEX | P00 | lexical/semantic build and sync aggregates |
| P04 | B-RETENTION | P03 | lock/retention/closure/physical census spans and counters |
| P05 | B-QUERY | P04 | candidate/read/byte/verify/rank/project work on success and failure |
| P06 | C-HARNESS | P00 | capture lifetime, observer, boot/reopen, phase identity, query explanation |
| P07 | C-EXPORT | P06 | strict canonical parser/profile, legacy comparison, one-file waterfall export |
| P08 | ROOT-INTEGRATION | P02/P05/P07 handoff | caller migration, two process regressions, narrow coupled build/tests |
| P09 | ROOT-VALIDATION | P08 and readiness review | lifecycle/kill, Off/Record, then required Large/XL run |

| Shared file | Write handoff | Boundary |
| --- | --- | --- |
| `crates/quanta-index-search-plane/src/ingest_dispatcher/ports.rs`, `search_corpus_retention.rs` | P00 ROOT → P04 B | P00 signatures freeze; P04 changes implementation/forwarding only |
| `crates/quanta-index-semantic/src/lib.rs`, `crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs` | P03 B → P04 B | Same owner, sequential; P03 producer then P04 retention context only |
| `crates/quanta-index-lexical/src/lexical_store/store.rs` | P03 B → P04 B → P05 B → P08 ROOT | Build/sync → closure/retention → query read; P08 only marks the convenience wrapper `#[cfg(test)]` after all owner handoffs |
| `crates/quanta-index-searchd/src/app/runtime.rs` | P02 A → P08 ROOT | P08 only BootReclaim parent and active-proof binding hunks after P02 handoff |
| `crates/quanta-index-searchd-harness/src/bin/scale_matrix.rs` | P06 C → P08 ROOT | P08 only bounded trace close/status separation after P06 handoff |
| `crates/quanta-index-lexical/src/adapter_lifecycle.rs` | P03 B → P04 B | P03 forwards existing owner sync meter only; P04 adds explicit reclamation context/census after handoff |
| P00 core/contract/schema files | ROOT retains ownership | All lanes read-only after handoff |
| Other lane files | Lane owner → ROOT at handoff | P08 only completes named mechanical caller hunks |

## 3. Exact packet write sets and protected boundaries

The following paths are exact relative paths. Braced names below are only compact notation for the listed individual files; they do not authorize siblings.

### P00 — common contracts and ports

**Existing files allowed:**

```text
crates/quanta-index-core/src/lib.rs
crates/quanta-index-core/src/request_budget.rs
crates/quanta-index-core/src/domains/lexical/outbound.rs
crates/quanta-index-core/src/domains/semantic/outbound.rs
crates/quanta-index-core/src/domains/semantic/stream.rs
crates/quanta-index-core/src/domains/generation.rs
crates/quanta-index-core/src/domains/reclaim_area.rs
crates/quanta-index-core/src/domains/lexical/history_text.rs
crates/quanta-index-core/tests/semantic_stream.rs
crates/quanta-index-contract/src/lib.rs
crates/quanta-index-contract/src/ipc/mod.rs
crates/quanta-index-contract/src/ipc/split.rs
crates/quanta-index-contract/src/ipc/ingest.rs
crates/quanta-index-contract/src/ipc/ingest/envelope.rs
crates/quanta-index-contract/src/ipc/ingest/tests.rs
crates/quanta-index-contract/tests/ipc_control_metrics_contract.rs
crates/quanta-index-contract/tests/ipc_control_quarantine_contract.rs
crates/quanta-index-contract/tests/ipc_query_result_v2_contract.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/ports.rs
crates/quanta-index-search-plane/src/search_corpus_retention.rs
```

**New files allowed:**

```text
crates/quanta-index-core/src/diagnostic_trace.rs
crates/quanta-index-contract/src/ipc/diagnostic_context.rs
tools/benchmark/retrieval/causal_timeline.schema.json
```

Only the span/clock/sink/lease/context contract, request token serde, explicit context parameters, re-exports, fixed tests, schema, and inode-census diagnostic forwarding may change. `PhysicalInodeCensus` emits one bounded span with `inodes_visited`, `files_visited`, `unique_inodes`, and `physical_bytes_measured`; the last is metadata size after inode dedup, not bytes read. Existing traversal, dedup, access checks, product errors, and checkpoint frequency stay unchanged.

Port signatures in this packet: `HistoryTextIndexPort::{publish_epoch,discard_epoch,discard_generation}` optional diagnostic context; lexical batch build/preflight; semantic preflight/publish stream; semantic scope preflight/build; reclaim/settlement/inventory/measure/finish and inode census entrypoints. Pass `Option<&DiagnosticContextV1>` explicitly; do not add trace-free default wrappers. The resident semantic test helper remains context-free and calls the port with `None`.

Closed phase identities: `full_ingest_seal`, `full_ingest`, `full_seal`, `full_activate`, `delta_ingest_seal`, `delta_activate`, `noop_seal`, `noop_activate`, `delete_seal`, `delete_activate`, `same_process_reopen`. Store identity as a `PhaseIdentity` event on the phase span; never infer it from time or order. Closed stages include `retention_root_lock_wait`, `retention_root_lock_hold`, `source_transaction_one`, `source_transaction_two`; counters include `physical_bytes_measured` and `sqlite_elapsed_ns`.

**P00 additive contract amendment (ROOT-owned, exact scope):** `crates/quanta-index-core/src/diagnostic_trace.rs` may add the `Health` event and closed health codes `foreign_capture_span`, `counter_overflow`, `invalid_attempt_token`, `duplicate_attempt_token`, and `unlinked_attempt`. A duplicate diagnostic request field is represented as `DiagnosticAttemptFieldV1::Duplicate`; malformed and duplicate attempt fields remain token-only fail-open and do not change product request decoding. A foreign-capture child context cannot create either child or root spans. Add closed stages `semantic_prepare`, `semantic_stream`, `semantic_delete`, `semantic_append`, `semantic_seal`, `semantic_promote`, `retention_write`, and `retention_reap`; add counters `retained_pairs`, `measure_calls`, `removed_roots`, `semantic_windows`, `semantic_owner_scopes`, `semantic_delete_calls`, `semantic_append_calls`, `semantic_rows_appended`, `semantic_rows_deleted`, `semantic_vector_bytes`, `query_admitted_limit`, `query_read_limit`, `query_byte_limit`, `regex_literal_count`, `regex_alternative_count`, and `regex_necessary_gram_count`. `tools/benchmark/retrieval/causal_timeline.schema.json` already carries the closed attempt health values and may represent any remaining same-vocabulary changes. These are trace vocabulary only: no product response, ingestion observation, query result, persisted generation, or digest schema change. A foreign-capture span is omitted and marks the capture `invalid`; inode-counter overflow is checked, marks health `counter_overflow`, and does not alter product success/failure. Invalid/duplicate attempt data is unlinked and marks trace health without changing the admitted request outcome. Writer close preserves `invalid` over a nominal close status. No other P00 contract file or field is reopened by this amendment.

**P00 additive contract amendment 2 (ROOT-owned, exact scope):** in `diagnostic_trace.rs`, add closed `DiagnosticSyncLabelV1` values for the ten existing lexical sync markers (`generation_directory`, `overlay_directory`, `history_manifest_file`, `coverage_orphan_directory`, `history_layout_directory`, `atomic_file`, `atomic_parent`, `atomic_at_file`, `sealed_leftover_directory`), fixed publication-root labels `publication_directory` / `publication_parent`, twelve pool labels (`pool_new_directory_full`, `pool_new_parent_full`, `pool_adoption_full_barrier`, `pool_data_file_posix`, `pool_object_file_full`, `pool_control_file_full`, `pool_data_shard_posix`, `pool_object_shard_full`, `pool_control_parent_full`, `pool_reused_file_posix`, `pool_reused_shard_posix`, `pool_adoption_journal_full`), and the two currently unlabeled calls (`pool_sync_all`, `state_sync_all`). Add `SyncAggregate(span,label,calls,elapsed_sum_ns,elapsed_max_ns,failures)`; no free-form label strings. Add `AdoptionGroupAggregate(span,reason,refs_bucket,bytes_bucket,groups,failed_groups)`, with reasons `ref_limit`, `byte_limit`, `explicit`, deterministic ref-limit priority if both thresholds fire, occupancy bands `zero`, `up_to_25_percent`, `up_to_50_percent`, `up_to_75_percent`, `up_to_100_percent`, `over_100_percent`. The managed trace snapshot used caps of 64 refs / 2,094,425 data bytes. The current logical checkout uses 128 refs / 2,091,801 data bytes after a separate storage-contract change; occupancy buckets must use the active runtime caps. Count attempted nonempty groups and distinguish failed from completed groups; explicit scope-end drain is `explicit`. Add counters `unique_closure_refs`, `closure_dependency_edges`, and `closure_authenticated_bytes`; these count successfully authenticated unique references, authenticated dependency edges, and bytes belonging to authenticated objects, not attempted reads. The exact P00 schema file is `tools/benchmark/retrieval/causal_timeline.schema.json`; bump its ID and emitted header/footer to schema version 2. P07 must continue to read legacy schema-v1 streams but reject v2 aggregate events in a v1 stream. Serializer and parser consumers are P02 `crates/quanta-index-searchd/src/app/trace_recorder.rs` and P07 `tools/benchmark/retrieval/causal_cost_timeline.py`; tests remain in their existing packet-allowlisted files. This reopens no product contract, storage layout, flush threshold, or behavior. P03 owns the sync producer and P04 owns closure producers; P03/P04 must not emit these until P02/P07 consumers are handed off or implement their exact consumer hunks under ROOT coordination.

**P00 additive contract amendment 3 (ROOT-owned, exact scope):** add closed stages `lexical_seal` and `lexical_proof` to `DiagnosticStageV1` in `crates/quanta-index-core/src/diagnostic_trace.rs` and the matching closed stage enum in `tools/benchmark/retrieval/causal_timeline.schema.json`. Add only those two names to P02's serializer whitelist in `crates/quanta-index-searchd/src/app/trace_recorder.rs`. P03 may use `lexical_seal` around the actual `seal_generation` call and `lexical_proof` around `walk_sealed_generation_at_with_mode`; these denote those real owner boundaries. `Finalize` remains the parent work scope. No stage boundary or product decision changes. The existing recorder test covers the serializer's closed-stage allowlist.

**P00 additive contract amendment 4 (ROOT-owned, exact scope):** add `DiagnosticObjectKindV1` with exactly the eleven lexical pool kinds (`file_body`, `file_postings`, `native_body`, `native_trigrams`, `native_positions`, `segment`, `record`, `map_page`, `blob_block`, `blob_node`, `attempt_batch`) and `ObjectKindAggregate(span,kind,new_objects,reused_objects)`. Add `compacted_segments` to `DiagnosticCounterV1`; preserve existing total new/reused counters. Use the existing schema v2 and add the fixed event/counter definitions to `tools/benchmark/retrieval/causal_timeline.schema.json`; P02 serializes the event and counter in `crates/quanta-index-searchd/src/app/trace_recorder.rs`; P07 validates/summarizes the event in `tools/benchmark/retrieval/causal_cost_timeline.py`. Tests remain in those existing allowlisted files. P03 counts only successfully created/reused objects by their actual `ObjectKind`, and records compacted segment/document/byte counts at the producer boundary. No object payload, storage layout, hash, or publication behavior changes.

**P00 additive contract amendment 5 (ROOT-owned, exact scope):** add one optional `DiagnosticContextV1` parameter to `HistoryTextIndexPort::{publish_epoch,discard_epoch,discard_generation}` in `crates/quanta-index-core/src/domains/lexical/history_text.rs` and one optional context parameter to `HistoryIngestPort::publish_batch` in `crates/quanta-index-search-plane/src/ingest_dispatcher/ports.rs`. This is diagnostic custody only; it does not change history mutation, retention, durability, or query contracts. P03 forwards the request context from `DirectHistoryMaterializer` into the history-text producer; P08 updates dispatcher and existing mocks/callers with `None` where no request context exists. History manifest/layout sync calls then use existing labels under a real `Finalize` child span. No ambient context or synthetic timestamps.

**P00 additive contract amendment 6 (ROOT-owned, exact scope):** add the closed `artifact_write` stage to `DiagnosticStageV1`, the P02 recorder's closed serializer mapping, and the matching schema enum for P06's benchmark-artifact writes. Add `DiagnosticContextV1::{shares_capture_session,has_parent}`; session comparison uses `Arc::ptr_eq`, never the public capture ID alone. `has_parent` lets a harness child lease create operation spans beneath its existing invocation parent while preserving standalone operation roots. Tests remain in the current core/recorder test modules and schema test. This is diagnostic ancestry/serialization only; no response schema, timing boundary, or product behavior changes.

**Read-only:** all response envelopes and serde except the listed request token files; `crates/quanta-index-contract/src/ipc/ingest_observation.rs`; `crates/quanta-index-contract/src/ipc/diagnostic_events.rs`; `crates/quanta-index-core/src/domains/observability.rs`; search-plane port implementation bodies; every Cargo manifest/lock except the separate P02/P08 exception below; product digest/usage/ack contracts.

**Forbidden:** writer/Perfetto/vendor dependency in core or contract; new crate/dependency; usage, ack, digest, or scrape-metric meaning change; V1/V2 duplicate diagnostic IR; context stored as ambient shared state.

### P01 — IPC and SDK attempt correlation

**Existing files allowed:**

```text
crates/quanta-index-ipc/src/lib.rs
crates/quanta-index-ipc/src/server.rs
crates/quanta-index-ipc/src/server/tests.rs
crates/quanta-index-ipc/src/counters.rs
crates/quanta-index-ipc/src/codec.rs
crates/quanta-index-ipc/src/error.rs
crates/quanta-index-ipc/tests/admission.rs
crates/quanta-index-ipc/tests/repo_admission_and_slowloris.rs
crates/quanta-index-ipc/tests/socket_access.rs
crates/quanta-index-ipc/tests/g0r_runtime_cancellation_probe.rs
crates/quanta-index-sdk/src/lib.rs
crates/quanta-index-sdk/src/client.rs
crates/quanta-index-sdk/src/transport.rs
crates/quanta-index-sdk/src/tests/connect_options_tests.rs
crates/quanta-index-sdk/src/tests/query_tests.rs
crates/quanta-index-sdk/src/tests/corpus_ingest_tests.rs
crates/quanta-index-sdk/src/tests/replay_tests.rs
```

No new files. The per-file hunk contract is:

- `crates/quanta-index-ipc/src/codec.rs`: fixed-frame byte codec and the typed decoded-body-size decision used only for the optional token.
- `crates/quanta-index-ipc/src/server.rs`: read/acknowledge the preface before product decode/admission; accept a linked token only when the same stream was attested for the active capture; never turn the probe into a request event.
- `crates/quanta-index-ipc/src/lib.rs`: export only the negotiated send/size-decision APIs required by SDK transport.
- `crates/quanta-index-sdk/src/client.rs` and `transport.rs`: create one fresh token per actual send, send it only after capability attestation, and classify only the existing transport result into the attempt outcome.
- `crates/quanta-index-ipc/src/server/tests.rs` and the listed existing IPC/SDK test files: fixed wire bytes, active/mismatched/no-capture ack, same-stream token acceptance, old-peer fallback, and no probe request event. No new test file.

Wire contract: the client preface is exactly 24 bytes (`QIPCDG01` as 8 ASCII bytes followed by the 16 raw capture-ID bytes). The server ack is exactly 9 bytes (`QIPCDG01` followed by status byte `0` or `1`); any other magic/status is a failed negotiation. Status `1` means the server has the same active capture ID on this server instance. The preface/ack is transport control: it creates no product request ID/event, admission permit, dispatch, or response envelope. Only after positive same-capture acknowledgement may the SDK serialize `diagnostic_attempt`; a supported peer with no active/matching capture receives the original envelope without the field. If an older peer closes/rejects the preface, reconnect and send the original envelope under the same absolute request deadline. Handshake failure must never replace or hide the product request result. Within a positively attested same-capture request, only the typed diagnostic-field size decision may remove the token, and only when removing it changes the decoded request from over-cap to within-cap. Preserve existing product error classes and ring projection.

**Read-only:** core/contract, Searchd config/runtime, response wire, queue/admission/deadline/cancel policy, ring API/capacity/loss semantics, SDK retry/timeout/connection policy.

### P02 — searchd recorder and lifecycle

**Existing files allowed:**

```text
crates/quanta-index-searchd/Cargo.toml
crates/quanta-index-searchd/src/lib.rs
crates/quanta-index-searchd/src/app/mod.rs
crates/quanta-index-searchd/src/app/runtime.rs
crates/quanta-index-searchd/src/app/server.rs
crates/quanta-index-searchd/src/app/ipc_dispatcher.rs
crates/quanta-index-searchd/src/app/maintenance.rs
crates/quanta-index-searchd/src/app/searchd.rs
crates/quanta-index-searchd/src/app/supervisor.rs
crates/quanta-index-searchd/src/app/boot_inventory.rs
crates/quanta-index-searchd/src/app/integrity_scrub.rs
crates/quanta-index-searchd/src/app/request_events.rs
crates/quanta-index-searchd/src/cli/command.rs
crates/quanta-index-searchd-runtime/src/lib.rs
```

**New file allowed:**

```text
crates/quanta-index-searchd/src/app/trace_recorder.rs
```

This new file is P02-owned. It must not also appear in the frozen no-touch snapshot below.

Writer adapter lives only in searchd. Build a bounded queue/record/output cap, writer-owned sequence, checked loss accounting, producer leases, bounded close wait and close barrier. Add lifecycle spans without changing boot/recovery/active-proof/maintenance/stop ordering or timeouts. Existing `SearchdConfig` equality and state format remain unchanged.

P02 runtime-builder API: `build_runtime_with_memory_probe_and_capture_under(config, memory_probe, capture, parent: Option<&DiagnosticContextV1>)`. With `Some(parent)`, reject a capture-session mismatch before any state-root lease/open, including equal public capture IDs backed by distinct clock/sink sessions. Start `RuntimeBoot` under that parent. If parent span or child lease cannot be created, continue product startup without injecting runtime diagnostic context; never fall back to an unlinked capture root. With `None`, preserve the existing standalone operation-root behavior, and inject a runtime child lease only if both root and boot span creation succeed. Existing builder remains the `None` wrapper. Searchd manifest may add only already-declared workspace `serde_json` direct dependency.

**Read-only:** `crates/quanta-index-searchd/src/app/config.rs`, state format/migration/lease/socket permissions, harness/Python writer, recovery/GC/active-proof policy, every new crate or dependency version.

### P03 — lexical/semantic producer aggregates

**Existing files allowed:**

```text
crates/quanta-index-lexical/src/lib.rs
crates/quanta-index-lexical/src/adapter.rs
crates/quanta-index-lexical/src/adapter_ingest.rs
crates/quanta-index-lexical/src/writer_cache.rs
crates/quanta-index-lexical/src/adapter_open.rs
crates/quanta-index-lexical/src/causal_profile.rs
crates/quanta-index-lexical/src/doc_census.rs
crates/quanta-index-lexical/src/generation_dir.rs
crates/quanta-index-lexical/src/index_store.rs
crates/quanta-index-lexical/src/lexical_publication.rs
crates/quanta-index-lexical/src/history_text_index/layout.rs
crates/quanta-index-lexical/src/history_text_index/manifest.rs
crates/quanta-index-lexical/src/history_text_index/publish.rs
crates/quanta-index-lexical/src/history_text_index/adapter.rs
crates/quanta-index-lexical/src/adapter_lifecycle.rs
crates/quanta-index-lexical/src/lexical_store/store.rs
crates/quanta-index-lexical/src/lexical_store/map.rs
crates/quanta-index-lexical/src/lexical_store/tests.rs
crates/quanta-index-lexical/src/text_authority/paged_writer.rs
crates/quanta-index-lexical/src/file_authority/paged/producer.rs
crates/quanta-index-lexical/src/sealed_generation/coverage_pages.rs
crates/quanta-index-lexical/src/sealed_generation/coverage.rs
crates/quanta-index-lexical/src/sealed_generation/manifest.rs
crates/quanta-index-lexical/src/sealed_generation/receipts.rs
crates/quanta-index-lexical/src/sealed_generation/scrub.rs
crates/quanta-index-lexical/src/sealed_generation/live_bm25.rs
crates/quanta-index-lexical/src/sealed_generation/live_bm25/tests.rs
crates/quanta-index-lexical/src/sealed_generation/overlay.rs
crates/quanta-index-lexical/src/sealed_generation/seal.rs
crates/quanta-index-lexical/src/sealed_generation/verify.rs
crates/quanta-index-lexical/src/adapter_ingest/tests.rs
crates/quanta-index-lexical/tests/l2_file_mutation.rs
crates/quanta-index-lexical/tests/support/publication_fixture.rs
crates/quanta-index-semantic/src/lib.rs
crates/quanta-index-semantic/src/search.rs
crates/quanta-index-semantic/src/build.rs
crates/quanta-index-semantic/src/build/tests.rs
crates/quanta-index-semantic/src/semantic_row_integrity_v1.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/semantic.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/auxiliary.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/tests/search_corpus.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/tests/semantic.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/tests/support.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/tests/idempotency.rs
```

No new files. Add aggregates at actual owner work boundaries: build/preflight/prepare/stream/delete/append/embed/promote, native/file publication and compaction, coverage/BM25 census/live/boundary, sealed proof, sync wrapper. Flush partial work before propagating errors. Preserve original `QI_CAUSAL_V1` sync/BM25/process markers until P07 strict parity; retain unmatched legacy values with source provenance.

`lexical_publication.rs` is P03-only: carry request/corpus diagnostic context through `PublicationContext::begin` and object-store operation scope. Do not change adoption lease/journal/durability/recovery/success decision or persist ambient context in shared `ObjectStore`. `map.rs` records only internal map-edit/page counters unavailable at callers. No per-object writes or shared mutex.

In `causal_profile.rs::observed_sync_legacy`, keep the canonical sync span/counter around only the sync operation. The legacy `QI_CAUSAL_V1` marker may wrap that call for compatibility, but stderr formatting/output time must not be included in the canonical sync duration.

P03-only forwarding exception: the additional `sealed_generation/{coverage,manifest,receipts,scrub}.rs`, `history_text_index/{publish,adapter}.rs`, `adapter_lifecycle.rs`, and `writer_cache.rs` paths may change only to forward an already-owned optional `SyncTotalsV1`/diagnostic child to fixed sync callsites. `ingest_dispatcher/auxiliary.rs` may forward the explicit diagnostic context into history-text publication/discard. No storage decision, write ordering, recovery behavior, or history query behavior changes. `adapter_lifecycle.rs` passes to P04 only after this sync hunk is handed off; P04 then adds its named reclaim/census instrumentation without editing the P03 hunk.

**Read-only:** segment/page/manifest wire and layout; `file_authority/paged.rs` and `file_authority/paged/verify.rs`; ranking/BM25 formula, embeddings, generation publication; P04/P05 hunks.

### P04 — retention and closure accounting

**Existing files allowed:**

```text
crates/quanta-index-lexical/src/adapter_lifecycle.rs
crates/quanta-index-lexical/src/lexical_gc.rs
crates/quanta-index-lexical/src/lexical_store/store.rs
crates/quanta-index-semantic/src/lib.rs
crates/quanta-index-semantic/src/search.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/ports.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/generation_plan.rs
crates/quanta-index-search-plane/src/search_corpus_lifecycle/pair_lock.rs
crates/quanta-index-search-plane/src/search_corpus_retention.rs
crates/quanta-index-search-plane/src/readiness/search_corpus_history.rs
crates/quanta-index-search-plane/src/readiness/auxiliary_store.rs
crates/quanta-index-search-plane/src/readiness/tests/search_corpus_history.rs
```

Instrument pair/root lock wait and hold, retention stages, closure read/hash/decode, SQLite elapsed/attempted batches/expanded rows, and physical inode census. Hold duration ends after mutex release. SQL elapsed is accumulated only around actual SQL calls; do not synthesize order or continuous intervals for interleaved read/hash work.

In `lexical_gc.rs::measure_unique_closure_bytes`, exclude `Connection::open` (and temporary-directory creation) from `sqlite_elapsed_ns`; start the SQL timer at the first measured SQL operation. For `read_mark_batch`, time `prepare`, `query`, row advance, and SQLite column extraction individually; keep capacity reservation and conversion/validation into `ObjectRef` outside the SQL timer. `Connection::open` remains inside the overall closure-measurement span, so total stage time still reflects connection setup.

**Read-only:** `crates/quanta-index-search-plane/src/search_corpus_lifecycle.rs` activation/retirement/repair authority; lock scope/order, settlement decisions, inode dedup/traversal correctness, hash/SQLite behavior; P03 producer and P05 query hunks.

### P05 — query workload and error-path measurement

**Existing files allowed:**

```text
crates/quanta-index-lexical/src/searcher/code_search.rs
crates/quanta-index-lexical/src/searcher/paging.rs
crates/quanta-index-lexical/src/budgeted_search.rs
crates/quanta-index-lexical/src/file_authority/query.rs
crates/quanta-index-lexical/src/file_authority/paged/reader.rs
crates/quanta-index-lexical/src/lexical_store/store.rs
crates/quanta-index-lexical/src/stage_timing.rs
crates/quanta-index-search-plane/src/query_dispatcher/mod.rs
crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs
crates/quanta-index-search-plane/src/query_dispatcher/stage_timing.rs
crates/quanta-index-search-plane/src/control_dispatcher.rs
```

Add only named query-observation tests already present in those crates; no new fixture or golden. Record attempted collector candidates before refusal, exact verification work, ordering/rank, preview rows, partial counter at error propagation. Error paths must snapshot before returning. Disabled diagnostics pass `None`, without allocation or atomic work.

`paging.rs::collect_whole_set` owns the generic whole-set collector observation used by native, boolean, and scoped callers: when `RequestBudgetV1` has diagnostic context, enter `QueryRank`, count collector callback attempts as `Candidates`, and finish the span on success or error. Preserve partial attempts if the collector refuses. Keep `CandidatesVerified` exclusive to exact verifier attempts; posting IDs decoded under `QueryCandidate` and `QueryProject` preview attempts use `Candidates`/`FilesDecoded`, never `CandidatesVerified`.

Parent contract: the route-level `QueryPlan` scope currently encloses read/search/project work, so its wall duration is inclusive and must not be presented as isolated planner latency. Pass its child context into downstream read-view/candidate/verify scopes; derive planner-only time by child-union subtraction or existing bounded prepare timing. Do not move the route timing boundary as part of this packet.

**Read-only:** planner/candidate/regex/exact-match/ranking algorithms; collector order/selection/limit; `crates/quanta-index-lexical/src/searcher/ranked_page.rs`; contribution-ledger meaning, budget/caps/error response; all non-envelope control logic; P04 store closure hunks.

### P06 — harness, phase observer, boot/reopen linkage

**ROOT closeout amendment (2026-10-11, exact scope):** after P06 handoff, ROOT may
replace the explicit `E2eRuntime::stop` owned-tempdir `Drop` with fallible
`TempDir::close`, instrumented under the existing `RuntimeStop` stage. Keep driver
stop/join before recursive deletion, caller-owned roots untouched, and deletion
inside the existing producer deadline. Optional trace errors must not prevent
cleanup or replace its result. Add cleanup I/O/ancestry regressions only in the
existing `harness.rs` teardown/diagnostic-parent test modules; no stage/schema/API/cap changes or
background/deferred cleanup. This does not qualify the prior XL timeout.

**ROOT bounded cleanup amendment (2026-10-11, exact scope):** add the private
`crates/quanta-index-searchd-harness/src/harness/owned_cleanup.rs` module and change
only `close_owned_tempdir`'s I/O call plus its module declaration in `harness.rs`.
For a driver-stopped, harness-owned TempDir, inspect the canonical object pool
without following links, then remove disjoint sorted shards using at most four
scoped workers. Join every started worker before consuming the remaining TempDir
with `close`; retain the first path-ordered error, including on worker panic or
spawn failure. Malformed or linked layouts use the original serial close. Keep
caller-owned roots, existing RuntimeStop span, primary-error custody, and the
whole-call deadline unchanged. Add local safety/error regressions in the private
module. Performance and XL completion require a new bound execution.

**Existing files allowed:**

```text
crates/quanta-index-searchd-harness/src/harness.rs
crates/quanta-index-searchd-harness/src/harness/source_publication.rs
crates/quanta-index-searchd-harness/src/concurrency.rs
crates/quanta-index-searchd-harness/src/open_loop.rs
crates/quanta-index-searchd-harness/src/scale.rs
crates/quanta-index-searchd-harness/src/bin/scale_matrix.rs
```

Create capture before lazy start, retain one lease through reopen and artifact error handling, preserve explanation and fresh attempt token per retry. Observer setup/periodic/boundary/teardown is a separate lane; phase timers do not move. `PhaseIdentity` is direct on each known phase span.

`E2eRuntime::validate_diagnostic_parent` must validate capture-session identity as well as public capture ID (`shares_capture_session`); equal IDs from distinct sessions are foreign parents and must be rejected. Keep a same-session positive case and equal-ID foreign-session regression in `harness.rs`.

The scale invocation owns one `Operation` scope and retains its lease through every measured tier, refusal output, and success/error artifact write. Derive each runtime lease as a counted child of the invocation; each artifact write is an `ArtifactWrite` child and remains diagnostic-only. If the optional capture cannot provide an invocation lease or any child span/lease cannot be created, continue the benchmark without that diagnostic context and preserve the measured workload result. The `scale.rs` phase helper keeps existing `FnOnce()` callsites; add a context-aware sibling only for `same_process_reopen`. Create phase operation scopes under the invocation context when it already has a parent; otherwise preserve the standalone operation-root behavior. Pass phase child context through stop, join, socket cleanup, start, ensure-driver, and start-driver to P02's `_under(..., Some(parent))` builder. Existing public `try_reopen_in_place()` and `start()` remain `None` wrappers. Keep original timing boundaries and stop/start order. Do not infer parent from timing or manufacture a link.

`PhaseSampler` process-write-I/O boundary snapshots are enabled by explicit diagnostic capture (or the existing legacy opt-in), not by an environment variable that the capture launcher strips. This keeps the already-defined phase resource field available under the canonical capture path without putting process-global deltas on a false per-span timeline.

In `scale.rs` and `harness.rs`, retain legacy context-free helper entry points only under `#[cfg(test)]` when production callers have migrated to traced variants. Do not change their test behavior. Remove `mut` from the non-mutating marker-forwarding wrapper.

**Read-only:** fixture generator, seed/tier/gold/query/assertion/caps, phase timer boundaries, searchd writer implementation, Python capture/export, runtime test suites.

### P07 — strict canonical capture, parser, profile, export

**Existing files allowed:**

```text
tools/benchmark/retrieval/causal_cost_capture.py
tools/benchmark/retrieval/causal_cost_profile.py
tools/ci/tests/test_causal_cost_capture.py
tools/ci/tests/test_causal_cost_profile.py
```

**New files allowed:**

```text
tools/benchmark/retrieval/causal_cost_timeline.py
tools/ci/tests/test_causal_cost_timeline.py
```

Use one new Python module for validation, canonical profile replay, summary and Chrome JSON. The capture launcher removes `QUANTA_INDEX_CAUSAL_PROFILE_V1` from its child environment; do not re-enable the legacy stderr producer in the measured run. Keep the old `QI_CAUSAL_V1` parser independent for retained legacy inputs and fixed parity fixtures. When a separately source/bin/input-bound legacy profile is actually supplied, attach `legacy_reconciliation` to the timeline summary and Chrome waterfall; compare only semantically equivalent common aggregates. Unmatched legacy values remain source-labelled residuals; never invent timestamps, parents, overlap, or zero for missing. Preserve source/bin/input/caps/hash and failure verdicts. The live canonical capture must produce its profile from canonical events and bound product artifacts. Keep strict migration **PARTIAL** until P08 proves every required label/counter parity; elapsed values with different timer envelopes remain unsupported, even when counts agree.

In `causal_cost_capture.py::capture`, consume the committed timeline bytes once with `RawFile.consume_seekable`, enforce `MAX_CAPTURE_BYTES` before materializing them, and pass that same byte snapshot to `export_timeline`. Do not reopen the timeline path for summary or viewer generation after input verification; both exports must use the bytes whose digest is recorded.

P07 artifact names are fixed to the producer contract: always write `causal-timeline.jsonl`, `timeline-summary.json`, `timeline-status.json`, and `execution.json`; write `causal-waterfall.json` only with `--timeline`. `--offline-input` reparses the existing canonical JSONL into a fresh output directory without starting the benchmark process. Do not emit or rename `timeline.jsonl` / `waterfall.chrome.json` aliases.

**Read-only:** Rust/schema files; benchmark inputs/gold/adapters/evaluator; source/binary binding, failure classification, caps; existing artifacts.

### P08 — serial caller migration and integration

Start only after P02/P05/P07 owners hand off and stop editing. In existing code, change only diagnostic context parameters/forwarding, compiler-required mechanical caller migrations, the named `same_process_reopen` regression, and conditional legacy marker removals after P07 parity. The one active-proof binding adjustment must return the already-verified active set from inside its existing trace closure so readiness receives the same value; keep both catalog reads and the identity race check in their current order. Do not edit expected result, fixture, budget, ranking, CI authority, or feature behavior.

For listed runtime test files, mechanical `SearchPlane{Query,Ingest,Control}IpcRequestEnvelope` initializers gain only `diagnostic_attempt: DiagnosticAttemptFieldV1::Absent`; payload, request ID, assertions, and behavior remain unchanged. Test doubles for context-aware ports accept the explicit `Option<&DiagnosticContextV1>` parameter and context-free calls pass `None`.

**Existing files allowed:**

```text
crates/quanta-index-scan-experiment/src/main.rs
crates/quanta-index-lexical/src/adapter_lifecycle.rs
crates/quanta-index-lexical/src/adapter_open.rs
crates/quanta-index-lexical/src/adapter_ingest.rs
crates/quanta-index-lexical/src/causal_profile.rs
crates/quanta-index-lexical/src/doc_census.rs
crates/quanta-index-lexical/src/generation_dir.rs
crates/quanta-index-lexical/src/history_text_index/layout.rs
crates/quanta-index-lexical/src/history_text_index/manifest.rs
crates/quanta-index-lexical/src/index_store.rs
crates/quanta-index-lexical/src/lexical_store/store.rs
crates/quanta-index-lexical/src/sealed_generation/coverage_pages.rs
crates/quanta-index-lexical/src/sealed_generation/live_bm25.rs
crates/quanta-index-lexical/src/sealed_generation/overlay.rs
crates/quanta-index-lexical/src/sealed_generation/seal.rs
crates/quanta-index-lexical/src/sealed_generation/verify.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs
crates/quanta-index-search-plane/src/semantic_derive.rs
crates/quanta-index-search-plane/src/quarantine.rs
crates/quanta-index-search-plane/src/control_dispatcher.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/tests/l2_file_mutation.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/tests/auxiliary.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/tests/idempotency.rs
crates/quanta-index-search-plane/src/ingest_dispatcher/tests/search_corpus.rs
crates/quanta-index-search-plane/tests/provider_boundary_owner_v1.rs
crates/quanta-index-search-plane/src/query_dispatcher/tests/request_correlation.rs
crates/quanta-index-search-plane/src/query_dispatcher/tests/support/history_text.rs
crates/quanta-index-search-plane/src/query_dispatcher/tests/history_relevance.rs
crates/quanta-index-search-plane/src/history_text.rs
crates/quanta-index-searchd/src/app/runtime.rs
crates/quanta-index-searchd-harness/src/bin/scale_matrix.rs
crates/quanta-index-searchd-runtime/src/admitted_publish_timeout_tests.rs
crates/quanta-index-searchd-runtime/tests/active_selection_process_v1.rs
crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs
crates/quanta-index-searchd-runtime/tests/e2e_ingest_idempotency.rs
crates/quanta-index-searchd-runtime/tests/e2e_process_readiness.rs
crates/quanta-index-searchd-runtime/tests/e2e_read_view.rs
crates/quanta-index-searchd-runtime/tests/e2e_semantic_scope_cap.rs
crates/quanta-index-searchd-runtime/tests/e2e_socket_access.rs
crates/quanta-index-searchd-runtime/tests/e2e_top_k_truth_table.rs
crates/quanta-index-searchd-runtime/tests/end_to_end.rs
crates/quanta-index-searchd-runtime/tests/explain.rs
crates/quanta-index-searchd-runtime/tests/repo_map_end_to_end.rs
crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs
crates/quanta-index-searchd-runtime/tests/e2e_boot_quarantine.rs
crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1.rs
crates/quanta-index-searchd-runtime/tests/e2e_scale_process_restart.rs
crates/quanta-index-searchd-runtime/tests/e2e_snapshot_registry.rs
crates/quanta-index-searchd-runtime/tests/e2e_coverage_pipeline_cost.rs
crates/quanta-index-searchd/src/app/boot_inventory.rs
crates/quanta-index-lexical/tests/generation_delta_base_carryforward.rs
crates/quanta-index-lexical/tests/l2_file_mutation.rs
crates/quanta-index-lexical/tests/sealed_manifest.rs
crates/quanta-index-lexical/tests/regex_cache_bounds.rs
crates/quanta-index-lexical/src/history_text_index/tests.rs
crates/quanta-index-semantic/tests/sealed_manifest.rs
crates/quanta-index-semantic/tests/persisted_semantic.rs
crates/quanta-index-semantic/src/search.rs
crates/quanta-index-semantic/src/lib.rs
crates/quanta-index-semantic/src/semantic_row_integrity_v1.rs
crates/quanta-index-semantic/src/ann_proof.rs
crates/quanta-index-semantic/src/proof.rs
tools/ci/lint/baselines/public-api/quanta-index-contract.txt
tools/ci/lint/baselines/public-api/quanta-index-sdk.txt
tools/ci/lint/baselines/cargo-modules/quanta-index-core.txt
tools/ci/lint/baselines/cargo-modules/quanta-index-contract.txt
```

Caller-only exceptions, exact hunk boundaries:

- `crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs::publish_source_idempotent`: forward `budget.diagnostic_context()` to lexical preflight only.
- `crates/quanta-index-search-plane/src/control_dispatcher.rs`: forward the same context to `QuarantineService::discard_with_context` only.
- `crates/quanta-index-search-plane/src/quarantine.rs::discard` and `discard_with_context`: preserve the context-free wrapper and forward context only to orphan `reclaim_sealed_generation_with_settlement`. Adapter quarantine semantics stay unchanged; tests keep using the wrapper.
- `crates/quanta-index-lexical/src/adapter_lifecycle.rs::finish_interrupted_reclaims`: forward the `observe_result` closure context to core reclaim census; no lifecycle lock/order change.
- `crates/quanta-index-lexical/src/adapter_open.rs::open`, `open_proven`, `open_sealed`, and `resident_bytes_estimate`: pass request context from `budget.diagnostic_context()`; proven/standalone open passes `None`; forward it only to the inode census. No open admission, verification, or resident-byte formula change.
- `crates/quanta-index-semantic/src/lib.rs::finish_interrupted_reclaims`: forward the `observe_result` closure context to core reclaim census; no lifecycle lock/order change.
- `crates/quanta-index-searchd/src/app/boot_inventory.rs::finish_interrupted_reclaims`: accept and forward the explicit boot-stage child context to the reclaim port; its unit-test fake accepts `None`. No boot refusal/notice behavior change.
- `crates/quanta-index-searchd/src/app/runtime.rs`: after P02 hands off, P08 may edit only `trace_boot_phase_with_context` for BootReclaim parenting and the active-proof closure's return binding. The closure returns its already-verified active set with the count so readiness receives the same value; identity re-read and race check remain in their current order inside the span.
- `crates/quanta-index-searchd-harness/src/bin/scale_matrix.rs::run`: after P06 hands off, close the optional trace with a five-second bound and retain/report trace close status separately from the benchmark result on both success and failure. A partial/invalid trace or close error must not turn a successful measured workload into a product/benchmark failure; a workload failure remains a workload failure and must not hide the close status. Keep the workload result and trace status as separate fields through `close_capture_preserving_benchmark`.
- `crates/quanta-index-lexical/src/lexical_store/store.rs::ObjectStore::open`: after P03/P04/P05 hand off their hunks, mark only the no-longer-production-used convenience wrapper `#[cfg(test)]`; production callers use `open_observed`/`open_existing`. No storage behavior or test fixture changes.
- Mechanical trait migrations in `crates/quanta-index-search-plane/src/ingest_dispatcher/tests/l2_file_mutation.rs`, `crates/quanta-index-search-plane/src/semantic_derive.rs`, `crates/quanta-index-search-plane/src/quarantine.rs`, `crates/quanta-index-lexical/tests/l2_file_mutation.rs`, `crates/quanta-index-lexical/tests/sealed_manifest.rs`, `crates/quanta-index-lexical/tests/regex_cache_bounds.rs`, `crates/quanta-index-lexical/tests/generation_delta_base_carryforward.rs`, `crates/quanta-index-semantic/src/ann_proof.rs`, `crates/quanta-index-semantic/src/proof.rs`, `crates/quanta-index-semantic/tests/persisted_semantic.rs`, `crates/quanta-index-semantic/tests/sealed_manifest.rs`, `crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1.rs`, and the exact HistoryTextIndex caller/test files above change only explicit diagnostic-parameter forwarding, with `None` in context-free callers. In `history_text.rs`, thread that parameter through `reconcile_after_durable` → `reconcile_generation` → `retire_and_discard_epoch` / `retire_and_discard_generation` to the corresponding port calls; context-free sweeps/tests pass `None`. They do not change fixture/assertion/order.

In the listed lexical legacy-marker files, removal is allowed only after strict parity passes and every label/counter has a canonical producer. Otherwise retain marker, env, parser, and record residual migration as partial.

`e2e_scale_process_restart.rs` may add exactly these independent regressions: `trace_timeline_process_close_keeps_clock_domains_separate` and `trace_timeline_process_kill_keeps_partial_without_cross_clock_join`. The normal same-process reopen case must assert phase parent ID on RuntimeStop/RuntimeJoin/RuntimeBoot descendants. Separate daemon processes retain separate clock domains; never align them by timestamp or merge critical path. Existing `runtime_extended_suite` membership remains; no standalone test target.

P08 may update only these four baselines after reviewing API/module deltas, never by accepting the shared dirty tree wholesale:

```text
tools/ci/lint/baselines/public-api/quanta-index-contract.txt
tools/ci/lint/baselines/public-api/quanta-index-sdk.txt
tools/ci/lint/baselines/cargo-modules/quanta-index-core.txt
tools/ci/lint/baselines/cargo-modules/quanta-index-contract.txt
```

**Read-only:** lane feature hunks; `crates/quanta-index-searchd-runtime/tests/runtime_fast_suite.rs`; `runtime_risk_suite.rs`; `runtime_extended_suite.rs` registry; `tools/ci/test-authority.toml`; lint scripts/guards; all other files/baselines.

### P09 — validation only

No source/config/test file edits. Run only existing registered test targets and exact P00–P08 focused tests. Failure repair goes back to its owning packet by a new exact handoff. No tier/gold/cap/source change.

**Observed closeout:** `small_scale_capture_negotiates_query_attempt_links` passed in the managed trace worktree and subsequently **VERIFIED** on the logical checkout, along with the runtime parent, same-process reopen, and two process close/kill regressions. The earlier current-checkout test attempt failed while compiling changing, untracked dependencies: `repomap/native/mod.rs:589` borrowed moved `prefix_key`; `kernel/update.rs:1133` compared `ObjectRef` to `Option<ObjectRef>`; the `projection.rs` missing-method output was stale against the current untracked `contract/src/index.rs`. Those paths are outside P09's write set; do not repair them without owner handoff and packet amendment.

The clean temporary snapshot at `/tmp/quanta-index-p09.HJoTCU`, commit `49e1cab793b98567ff62b111fc76120f3cc0c22d`, matches the managed trace worktree but not the logical checkout. Both trees share `HEAD=8f11cad9587aec1668a9fb3f5f1f055337487071`; at the original audit snapshot they differed in 102 dirty files (57 packet/control paths and 45 other paths), including 30 logical-checkout files absent from the trace worktree. Do not copy or rsync between them wholesale. All scale results below are managed-worktree diagnostic evidence, not logical-checkout integration or release qualification.

**Logical-main focused verification after source reconciliation:** `./scripts/cargow --lane test-daemon-lane test -p quanta-index-semantic --lib build::tests:: --locked` passed 33 tests, including the two semantic trace cases that had failed in a separately built, stale temporary snapshot. `./scripts/cargow --lane test-daemon-lane test -p quanta-index-searchd-runtime --lib diagnostic_runtime_parent_tests --locked` passed 1; the harness library filters `small_scale_capture_negotiates_query_attempt_links` and `same_process_reopen_lifecycle_spans_are_children_of_named_phase` passed 1 each; `--test runtime_extended_suite trace_timeline_process_` passed 2. The latter four Rust commands used `QUANTA_INDEX_RESOURCE_ADMISSION=0 QUANTA_INDEX_SCCACHE=0 CARGO_BUILD_JOBS=2` because unrelated build jobs repeatedly acquired the shared admission lock; they are correctness checks, not timing evidence. `uv run --frozen --extra dev python -m pytest -q` for the three `test_causal_cost_{capture,profile,timeline}.py` files passed 226 tests. The main parser also exported the existing XL Record JSONL into 21,449 Chrome timeline events. The relevant Rust source hashes were unchanged before and after these focused checks. Full checkout tests and a benchmark from main remain **NOT_RUN**.

- **VERIFIED (diagnostic execution):** Large Off and Record both completed at 4,096 files. The Record stream closed `Complete` with 18,995 events, zero dropped/unwritten events, and zero health events; strict parser/export reported 8,976 spans. Offline viewer export succeeded but says `OFFLINE_EXPORT_UNVERIFIED` and `producer_executed=false`.
- **VERIFIED (Large trace finding):** Record capture wall 103.438 s; `full_seal` 62.771 s, nested `lexical_build` 58.038 s, `build_prepare` 45.645 s, `native_publish` 22.291 s, and `file_publish` 21.826 s. The lexical span accumulated 34.318 s across 79,662 sync calls; the largest full-capture aggregates were control-file sync 10.201 s/1,874 calls, control-parent sync 9.589 s/1,874, adoption barrier 6.220 s/615, and object-file sync 3.144 s/615. These counters overlap/nest and are not additive wall time. This is actionable evidence for the 4,096-file seal path only; it does not establish the cause of the historical XL 93.969 s delta.
- **NOT_QUALIFIED (performance):** both Large runs required `QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1` after the host preflight exceeded its load limit. These timings are not comparable performance evidence.
- **FAILED (diagnostic execution):** XL Off at 32,768 files timed out in `full_seal/build_seal` at the existing 600,000 ms request timeout while the host was contended. Preserve `/tmp/quanta-index-p09-runs/xlarge-off-fixed/refusal.json`; this does not establish a product regression.
- **FAILED workload / VERIFIED trace:** XL Record at 32,768 files also timed out in `full_seal/build_seal` at the existing 600,000 ms request timeout. The trace closed `Complete` with 43,008 events, zero drops/unwritten events, zero health, and zero ambiguous links; strict timeline parse/export passed. `full_seal` was 610.19 s; request span 626.91 s was cancelled while backend ended `ok`. Largest nested work: lexical build 542.98 s, 642,932 sync calls / 259.376 s accumulated, prepare 412.54 s, native/file publish 212.03/182.42 s, finalize 118.65 s, retention lock/measure about 55/51.76 s. These spans/work counters overlap; do not sum them as wall time. This identifies the contended XL full-seal path, but does not establish the historical 93.969 s delta cause or qualify performance. Refusal: `/tmp/quanta-index-p09-runs/xlarge-record-fixed/refusal.json`; raw trace and parsed timeline are alongside it.
- **NOT_RUN:** quiet-host Off/Record pairs and Perfetto UI inspection. Latest preflight reports load 11.52 against limit 8.0 with no foreign Rust process; host sampling shows sustained CPU from the Virtualization VM, `ken-mcp`, and Docker Desktop. Do not weaken timeout/caps or relabel the contended run.

## 4. Cargo.lock exception

`Cargo.lock` is read-only except this exact P02/P08 edge, corresponding to the already-dirty `crates/quanta-index-searchd/Cargo.toml` direct workspace `serde_json` dependency:

```text
[[package]] name = "quanta-index-searchd": add "serde_json" to dependencies
```

Existing dirty lock entries for `quanta-index-lexical` (`rusqlite`) and `quanta-index-scan-experiment` (`quanta-index-search-plane`, `quanta-index-semantic`) are pre-existing and must remain byte-for-byte untouched by this task. Every `Cargo.toml`, package version/source/checksum, other dependency edge and ordering is read-only. If `--locked` asks for any additional change, stop and amend this file with exact package/name/hunk before proceeding.

## 5. Required handoff and completion evidence

Each lane reports exact path and function/hunk, outcomes/failure behavior, static checks, command and test count, `NOT_RUN` scope, and remaining caller list. Root compares every changed path against the packet before P08.

- P00/P01/P02/P03/P04/P05/P06/P07 completion is not product/runtime qualification.
- P08 must compile all affected workspace callers, pass focused independent tests, and verify the parent-chain regression and strict legacy parity policy.
- P09 runtime/kill/Off-Record/XL is not run until source/API integration is stable and confidence is at least 80%; report runtime and benchmark separately.
- The logical checkout remains dirty and diverges from the managed trace worktree; do not stage, commit, or wholesale synchronize either tree as part of P09. Source-level P00–P08 reconciliation found the required trace behavior in logical main except the runtime builder fail-open path, which is now corrected. Main focused compilation and tests have passed on a stable set of relevant file hashes; full checkout qualification remains NOT_RUN. Final benchmark qualification requires a frozen main-source identity and a quiet host. The earlier untracked native dependency errors were repaired by their separate owner; the focused main build now compiles those dependencies. Quiet-host pairs and Perfetto inspection remain open. Both contended XL attempts failed the workload request; XL Record's complete trace is diagnostic evidence, not a qualified regression or performance claim. The process close/kill regressions qualify lifecycle/clock separation only, not scale performance.
- Never mark an absent, stale, interrupted, malformed, or partial result as success. Use `VERIFIED`, `FAILED`, `BLOCKED`, `NOT_RUN`, or `NOT_APPLICABLE` for requested verification scopes.

## 6. Frozen paths outside packet write sets (worktree snapshot 2026-10-11)

**Snapshot applicability:** the 293-path inventory below is the earlier managed trace-worktree snapshot, not the current logical checkout. The latest audit counted 343 dirty paths in the logical checkout, including 30 untracked native-index files absent from the trace worktree. Those newly appearing paths are unclassified and no-touch for P00–P09; in particular, do not repair `quanta-index-repomap/src/native/mod.rs`, `quanta-index-kernel/src/{projection,update}.rs`, or `quanta-index-contract/src/index.rs` under this packet. They require the native incremental IR owner and a source freeze first.

The authoritative worktree snapshot contains 293 changed paths: 149 resolve to a packet write set or the exact `Cargo.lock` exception; the 2 ROOT-only plan documents are separately allowed by §1; and the 142 paths listed below are outside every packet write set and are frozen. These 142 paths are **no-touch for every packet**: preserve contents and Git status exactly; do not format, stage, delete, rename, or fold them into a trace packet. This is an ownership boundary, not a claim that their contents are clean or unrelated to every other project task. Any required change needs a packet amendment naming its owner, exact function/hunk, and verification before editing.

This is a point-in-time snapshot, not a permanent repository inventory. Recompute `git status --short --untracked-files=all` at each packet start/handoff and before final integration. Reclassify every added, removed, or renamed path against the active packet's exact write set; a newly changed path is no-touch until explicitly assigned. For a given packet, a file listed in another packet's write set remains no-touch until the documented handoff transfers ownership. The concrete touch list is the packet's exact file list in §3; its no-touch set is every other path, further constrained by the packet's explicit read-only clauses and this frozen-path list.

```text
.circleci/config.yml
.config/nextest.toml
Justfile
crates/quanta-index-contract/fuzz/.gitignore
crates/quanta-index-contract/fuzz/Cargo.lock
crates/quanta-index-contract/src/ipc/generation_reservation.rs
crates/quanta-index-contract/src/ipc/ingest_observation.rs
crates/quanta-index-core/src/domains/integrity.rs
crates/quanta-index-core/src/domains/lexical/service.rs
crates/quanta-index-core/src/domains/source_publication.rs
crates/quanta-index-core/src/publication_validation.rs
crates/quanta-index-lexical/Cargo.toml
crates/quanta-index-lexical/src/authority_doc_set.rs
crates/quanta-index-lexical/src/dense_admission.rs
crates/quanta-index-lexical/src/file_authority.rs
crates/quanta-index-lexical/src/file_authority/codec.rs
crates/quanta-index-lexical/src/file_authority/paged.rs
crates/quanta-index-lexical/src/file_authority/paged/verify.rs
crates/quanta-index-lexical/src/file_authority/plan.rs
crates/quanta-index-lexical/src/file_authority/producer.rs
crates/quanta-index-lexical/src/file_authority/publication_proof.rs
crates/quanta-index-lexical/src/file_authority/reader.rs
crates/quanta-index-lexical/src/file_authority/root.rs
crates/quanta-index-lexical/src/file_authority/verify.rs
crates/quanta-index-lexical/src/file_authority/verify/output.rs
crates/quanta-index-lexical/src/file_authority/verify/publication_tests.rs
crates/quanta-index-lexical/src/inventory.rs
crates/quanta-index-lexical/src/lexical_allocation.rs
crates/quanta-index-lexical/src/lexical_root_names.rs
crates/quanta-index-lexical/src/lexical_store.rs
crates/quanta-index-lexical/src/lexical_store/codec.rs
crates/quanta-index-lexical/src/planner.rs
crates/quanta-index-lexical/src/publication_faults.rs
crates/quanta-index-lexical/src/publication_faults/tests.rs
crates/quanta-index-lexical/src/query_errors.rs
crates/quanta-index-lexical/src/regex.rs
crates/quanta-index-lexical/src/regex_match_cache.rs
crates/quanta-index-lexical/src/sealed_generation/mod.rs
crates/quanta-index-lexical/src/sealed_generation/publication_proof.rs
crates/quanta-index-lexical/src/searcher/authorities.rs
crates/quanta-index-lexical/src/searcher/candidates.rs
crates/quanta-index-lexical/src/searcher/code_search/ranking.rs
crates/quanta-index-lexical/src/searcher/compile.rs
crates/quanta-index-lexical/src/searcher/manual_scan.rs
crates/quanta-index-lexical/src/searcher/match_sets.rs
crates/quanta-index-lexical/src/searcher/port.rs
crates/quanta-index-lexical/src/searcher/predicate_plan.rs
crates/quanta-index-lexical/src/searcher/predicates.rs
crates/quanta-index-lexical/src/searcher/restrictions.rs
crates/quanta-index-lexical/src/text_authority/manifest.rs
crates/quanta-index-lexical/src/text_authority/mod.rs
crates/quanta-index-lexical/src/text_authority/paged_bootstrap.rs
crates/quanta-index-lexical/src/text_authority/paged_bootstrap_tests.rs
crates/quanta-index-lexical/src/text_authority/paged_manifest.rs
crates/quanta-index-lexical/src/text_authority/paged_posting_page.rs
crates/quanta-index-lexical/src/text_authority/paged_reader.rs
crates/quanta-index-lexical/src/text_authority/paged_segment.rs
crates/quanta-index-lexical/src/text_authority/paged_transition.rs
crates/quanta-index-lexical/src/text_authority/paged_verify.rs
crates/quanta-index-lexical/src/text_authority/paged_wire.rs
crates/quanta-index-lexical/src/text_authority/reader.rs
crates/quanta-index-lexical/src/text_authority/shard.rs
crates/quanta-index-lexical/src/text_authority/types.rs
crates/quanta-index-lexical/src/text_authority/writer.rs
crates/quanta-index-lexical/src/text_authority_plan.rs
crates/quanta-index-lexical/src/text_docs.rs
crates/quanta-index-lexical/tests/boot_inventory.rs
crates/quanta-index-lexical/tests/cancellation_inside_search.rs
crates/quanta-index-lexical/tests/execution_budget.rs
crates/quanta-index-lexical/tests/explain_candidate.rs
crates/quanta-index-lexical/tests/f15_file_authority.rs
crates/quanta-index-lexical/tests/l1_query_domain_window.rs
crates/quanta-index-lexical/tests/l3_exact_source.rs
crates/quanta-index-lexical/tests/l4_match_anchored_preview.rs
crates/quanta-index-lexical/tests/planner_authority.rs
crates/quanta-index-lexical/tests/ranked_pages.rs
crates/quanta-index-lexical/tests/regex_literal_alternation.rs
crates/quanta-index-lexical/tests/resident_bytes.rs
crates/quanta-index-lexical/tests/sealed_commitment_cost.rs
crates/quanta-index-lexical/tests/tantivy_smoke.rs
crates/quanta-index-lexical/tests/text_authority_shards.rs
crates/quanta-index-lexical/tests/unicode_normalization_goldens.rs
crates/quanta-index-lexical/tests/writer_envelope.rs
crates/quanta-index-lq-norm/fuzz/.gitignore
crates/quanta-index-lq-norm/fuzz/Cargo.lock
crates/quanta-index-lq-norm/fuzz/Cargo.toml
crates/quanta-index-lq-norm/fuzz/fuzz_targets/regex_necessary_gram.rs
crates/quanta-index-lq-regex/src/executor.rs
crates/quanta-index-lq-regex/src/lib.rs
crates/quanta-index-lq-regex/src/necessary_gram.rs
crates/quanta-index-lq-trigram/src/lib.rs
crates/quanta-index-lq-trigram/src/necessary_gram.rs
crates/quanta-index-repomap/src/store.rs
crates/quanta-index-repomap/tests/read_view_lifetime_owner_v1.rs
crates/quanta-index-scan-experiment/Cargo.toml
crates/quanta-index-sdk/src/binding.rs
crates/quanta-index-sdk/src/generations.rs
crates/quanta-index-sdk/src/lexical.rs
crates/quanta-index-sdk/src/observability.rs
crates/quanta-index-sdk/src/preparation/implementation.rs
crates/quanta-index-sdk/src/quarantine.rs
crates/quanta-index-sdk/src/repomap.rs
crates/quanta-index-sdk/src/tests/control_tests.rs
crates/quanta-index-sdk/tests/l2_daemon_publication.rs
crates/quanta-index-search-plane/src/readiness/activation_catalog.rs
crates/quanta-index-search-plane/src/readiness/activation_catalog/repository_envelope.rs
crates/quanta-index-search-plane/src/readiness/activation_catalog/source_events.rs
crates/quanta-index-search-plane/src/readiness/activation_catalog/tests.rs
crates/quanta-index-search-plane/src/search_corpus_lifecycle.rs
crates/quanta-index-search-plane/src/snapshot_registry.rs
crates/quanta-index-searchctl/tests/cli_smoke.rs
crates/quanta-index-searchd-runtime/src/state_migration.rs
crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs
crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs
crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs
crates/quanta-index-searchd-runtime/tests/e2e_generation_activation_concurrency.rs
crates/quanta-index-searchd-runtime/tests/e2e_integrity_scrub.rs
crates/quanta-index-searchd-runtime/tests/e2e_lexical_sealed_overlays.rs
crates/quanta-index-searchd-runtime/tests/e2e_physical_gc.rs
crates/quanta-index-searchd-runtime/tests/e2e_predicate_authority_boolean.rs
crates/quanta-index-searchd-runtime/tests/e2e_predicate_authority_lifecycle.rs
crates/quanta-index-searchd-runtime/tests/e2e_text_route_hellgate.rs
crates/quanta-index-searchd-runtime/tests/sdk_frontdoor/binary_tests.rs
crates/quanta-index-searchd-runtime/tests/sdk_frontdoor/matrix_tests.rs
crates/quanta-index-semantic/src/generation_contract.rs
crates/quanta-index-semantic/src/integrity.rs
crates/quanta-index-semantic/src/manifest.rs
crates/quanta-index-semantic/src/sealed_manifest.rs
crates/quanta-index-semantic/tests/vector_index_contract.rs
docs/adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md
docs/adr/OCT-10-001-seal-owned-lexical-serving-metadata.md
docs/adr/OCT-10-002-shared-paged-lexical-authorities.md
docs/plans/oct-10-index-closeout/LEXICAL-QUERY-PLAN.md
docs/plans/oct-10-index-closeout/LEXICAL-STORE-CONTRACT.md
docs/plans/oct-10-index-closeout/README.md
docs/plans/oct-10-index-closeout/VALIDATION.md
docs/rfc/oct-10-native-incremental-vnext.md
tools/ci/ignored-test-policy.toml
tools/ci/inventory/wire-surface.toml
tools/ci/lint/lint-hexagonal-boundaries.py
tools/ci/test-authority.toml
tools/ci/tests/test_proof_operational_result.py
```
