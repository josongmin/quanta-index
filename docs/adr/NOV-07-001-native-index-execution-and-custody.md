# Native Index Execution and Custody

Status: `Accepted`

Decided: 2026-11-07

## Context

The November integration landed changes to Native lexical publication and
reading, catalog installation, query ingress, and IPC. The October E02 packet
also tracked work across the Semantica publication and Index consumer boundary.
This record consolidates the implemented contracts and superseded approaches.
The [Native RFC](../rfc/oct-10-native-incremental-vnext.md) and
[full-product DoD](../plans/oct-22-finalizing/DOD.md) retain their acceptance scope.

## Decision

1. **Lexical occurrence packing.** `QITXOC01` bounds an owner-local block to
   8,192 bytes and 512 canonical term rows. Terms may share a positions vector
   inside a block. Block lookup and count bind the canonical term as well as
   the object reference. Oversize vectors and changed delta terms use the
   existing leaf path. Unchanged references can remain in a pinned view. The
   earlier 4,096-byte/128-row policy is retired. Storage and reader validation
   follow the [block codec](../../crates/quanta-index-lexical/src/native/text_occurrence/block.rs)
   and [occurrence reader](../../crates/quanta-index-lexical/src/native/text_occurrence.rs).
2. **Publication and reuse.** The current entry map keeps the posting tree
   separate from owner inventory packing. A content comparison can support
   reuse only within the authenticated owner/read context. Physical object
   custody, bounded work, and old-view reads remain subject to the existing
   registration, pin and retirement rules. See the
   [entry map](../../crates/quanta-index-lexical/src/native/text/entry_map.rs),
   [Catalog installer](../../crates/quanta-index-catalog/src/native/authority/managed_install.rs)
   and [read-session registry](../../crates/quanta-index-searchd-runtime/src/native_read_sessions.rs).
3. **E02 Acked readback.** The strict Native decoder lends the same completed
   DTO to private readback checks. Readback still validates SQL identity,
   revision, phase and published predicates. Failure keeps the partial DTO,
   backing and first typed cause with the existing owner through final drop.
   The public/raw DTO retains full validation. The existing 4 MiB bound and
   wire and digest contracts hold. The canonical implementation boundary is the
   [Acked contract](../plans/oct-22-finalizing/E02-publication-recovery.md#acked-row-readback-contract).
4. **IPC settlement.** On peer hangup, publish the closed-peer state and set
   budget cancellation before reporting the hangup counter. Before and between
   partial response writes, honor the same absolute deadline and cancellation.
   Once the socket accepts the last frame byte, that write is settled. The
   client enforces its own read deadline. Preserve nested typed decode and
   validation causes across the owning error chain. See the
   [peer watcher](../../crates/quanta-index-ipc/src/server/peer_watch.rs),
   [response writer](../../crates/quanta-index-ipc/src/server.rs) and
   [admitted decoder](../../crates/quanta-index-ipc/src/admitted_decode.rs).
5. **Configured embedding.** The macOS SDK uses the configured owned Voyage
   Host path. The separate borrowed entrypoint can refuse nonempty input before
   transport. Use the existing selected provider, admission and typed refusal
   path without copying an Ordinary result into Native. Live HTTP/TLS, partial
   backing and refund, and remote SDK acceptance belong to
   [E02 Provider](../plans/oct-28-e02-native-closeout/tickets/P07-native-provider.md).
6. **Go trace retrieval.** The Go trace ranker aggregates lexical and dense
   candidates by file under the selected generation pin. A parsed frame hint
   contributes only when it matches the explicitly bound source repository
   and exact relative path. Shared candidate acquisition feeds separate rank
   paths. Go reports file RRF and its file count, with lexical and dense
   windows in chunk units. With exact lexical admission, Go takes one
   canonically ordered dense prefix up to the existing candidate ceiling.
   Admission still widens the logical prefix as needed. Hybrid ranks and
   explains chunks. See the
   [dispatcher](../../crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs),
   [file ranker](../../crates/quanta-index-core/src/domains/hybrid/trace_file_rank.rs)
   and [result contract](../../crates/quanta-index-contract/src/results/go_test_trace_file.rs).
7. **SQLite observation.** Pager counters are non-resetting diagnostic samples
   from the existing connection, with deltas only between available monotonic
   samples. A scratch connection keeps its selected VFS binding through close.
   Typed capacity refusal requires the active operation's VFS attribution. See
   [Catalog I/O](../../crates/quanta-index-catalog/src/io_admission.rs), the
   [pinned rusqlite interface](../../vendor/rusqlite-0.40.2/src/pager_cache_status.rs)
   and the [scratch connection](../../crates/quanta-index-scratch-sqlite/src/connection.rs).
8. **Search-corpus publication proof.** The dispatcher retains the immutable
   request in its publication owner. The same batch can reuse its checked
   source-event payload and surface validation through preflight, build and
   Pending recovery. Footprint reuse binds the resource policy and embedding
   dimension. The canonical wire digest still binds the journal, and direct
   inputs retain their own checks. See the
   [publication owner](../../crates/quanta-index-core/src/publication_validation.rs)
   and [Pending recovery](../../crates/quanta-index-lexical/src/adapter_lifecycle.rs).
9. **Native boot.** A non-Materialize operation must advance its expected
   sequence by exactly one, with overflow refused. Materialize retains its
   rebase contract. Boot retains the authenticated request and profile from
   the bounded protected-root traversal for domain validation. See the
   [sequence check](../../crates/quanta-index-kernel/src/boot.rs) and
   [closure reader](../../crates/quanta-index-storage/src/object/read_only.rs).
10. **Source member and Map reads.** Literal, Text and Path share Source-local
    member authentication under the pinned view. Graph retains its projection
    checks. General and staged Map batch reads share 64-key shape admission,
    key work and the lookup loop. Their node loaders retain their respective
    custody.
    See [source member validation](../../crates/quanta-index-core/src/index/query.rs)
    and [batch lookup](../../crates/quanta-index-storage/src/map/original_lookup.rs).
11. **Native ANN admission and refill.** Charge each valid get or copy before
    the backing lookup, including a missing object. Get charges its returned
    byte range before exposing data. Copy fixes the source size under the
    mutation gate through admission and copy. Within one region, refill may
    reuse a candidate decision with the same owner, member, vector ID and
    input stamp. Changed stamps or duplicate rows within a prefix are refused.
    Each backend prefix still consumes its original work budget. See the
    [ANN store](../../crates/quanta-index-semantic/src/native/ann_store.rs)
    and [refill](../../crates/quanta-index-semantic/src/native/ann_lance.rs).
12. **Public read sessions.** Open replay of the same UID, key and selection
    returns the retained session state and pin within the registry lifetime.
    Sweep and drain share terminal-state handling. Only successful public
    Release can issue a replayable Release ACK. The retained Text path shares
    profile completion with first-stage lease validation before reader
    resolution and reservation. Later stages and the Registry's entry and
    final validation keep their original checks. See the
    [registry](../../crates/quanta-index-searchd-runtime/src/native_read_sessions.rs)
    and [Kernel stage](../../crates/quanta-index-kernel/src/update.rs).

## Acceptance

The [P01 owning run](../plans/oct-22-finalizing/VALIDATION.md#e02-deduplicated-current-work-20261030-root)
closed the fixed-cohort Acked regression (76/76). Physical SQL-worker, SDK,
independent-process and Product acceptance remain in the
[E02 ledger](../plans/oct-22-finalizing/README.md#e02). Current Native performance,
including the original Large deadline and mixed-query bounds, is tracked in the
[Native bugbash](../plans/nov-5-indexing-bugbash/README.md).
