# OCT-05-003 — Active Query and Runtime Lifecycle

Status: `Accepted`

Decided: 2026-10-05

Consolidates implemented O4-E3 contracts. It preserves
[SEP-21-002](SEP-21-002-durable-authority-and-operation-lifecycle.md),
[SEP-21-003](SEP-21-003-read-view-continuation-and-provider-policy.md) and
[SEP-27-005](SEP-27-005-catalog-recovery-supervision-and-proof-custody.md).
It does not accept the stronger admission-pin proposal or wider dispatch.
Shipping-source/release acceptance remains with [I0](../plans/oct-4-parallel-closure/tickets/INDEX.md#i0).

## Context

Selection precedes physical read-view acquisition. Disk metering can outlast
backend health cadence, and client timeout can precede an admitted operation's
durable terminal. These intervals require explicit ownership and outcome rather
than an SDK retry, an inferred rollback or a new operator endpoint.

## Decision

1. A selected generation retired before view acquisition returns typed
   `UNKNOWN_GENERATION` before opening it. Selection alone guarantees no lease.
   An acquired view owns immutable handles through request completion and protects
   them against retirement. Keep the independent G1 selection → G2/G3 activation
   → physical G1 retirement → refusal/open-count-zero → fresh G3 oracle.
   A guarantee that selection must succeed after retirement remains unaccepted.
2. Supported Active Text, Symbol, History, RuntimeMetadata, Semantic, Hybrid and
   HybridSeed queries select and execute in one query RPC. The SDK validates
   response variant/domain, selected generation/head token and row identities,
   including joint lexical/semantic selection where declared. Cursor, explicit
   pin, token and `rev:at.time` semantics remain exact; unsupported/exact-only
   routes retain typed refusal. One-RPC code does not itself prove speedup or
   live counts for every route.
3. Backend readiness advances independently of slow logical disk metering.
   Metering has owned bounded work, cancellation/shutdown/join and age/error
   reporting. Backend loss or fatal maintenance remains observable; slow work
   cannot create false freshness. Logical file length is not allocated/transient
   disk or a physical quota.
4. Client timeout/hangup after publish admission does not establish operation
   cancellation or rollback. An admitted operation settles through the journal;
   inspection/exact replay by identity returns its durable result without rebuild.
   Conflicting input under that identity refuses. Preserve old active and required
   rollback generations. The default SDK I/O timeout is 30 seconds; this decision
   introduces neither asynchronous ACK nor parallel ingest admission.
5. Existing request events/counters, SDK observability and searchctl own operator
   diagnostics. Authorize before reading the bounded ring. Preserve process
   instance, sequence/drop window, request correlation and transport bounds;
   restart/wrap/denied-principal outcomes are explicit. Request IDs/payloads stay
   outside metric labels; ring events do not replace cumulative counter authority.
   Two actual UID socket
   checks establish their component scope, not shipping Linux deployment.

## Owners and regressions

- [Selection](../../crates/quanta-index-search-plane/src/query_dispatcher/selection.rs),
  [view acquisition](../../crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs),
  [SDK binding](../../crates/quanta-index-sdk/src/binding.rs).
- [Maintenance](../../crates/quanta-index-searchd/src/app/maintenance.rs),
  [ingest dispatch](../../crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs),
  [operator dispatch](../../crates/quanta-index-search-plane/src/control_dispatcher.rs).
- Retain actual OS-child [selection](../../crates/quanta-index-searchd-runtime/tests/active_selection_process_v1.rs),
  [slow-disk](../../crates/quanta-index-searchd-runtime/src/process_slow_disk_tests.rs),
  [default-timeout](../../crates/quanta-index-searchd-runtime/src/admitted_publish_timeout_tests.rs),
  [restart/replay](../../crates/quanta-index-searchd-runtime/tests/e2e_ingest_idempotency.rs),
  [socket authorization](../../crates/quanta-index-searchd-runtime/tests/e2e_socket_access.rs)
  and [SDK roundtrip](../../benchmarks/retrieval/tests/sdk_roundtrip.rs) controls.

## Consequences

Requested owner/process scenarios were executed historically. Their exact source,
commands and failure history remain in the [plan history index](../plans/ARCHIVE-INDEX.md#oct-05-handoff-and-ticket-compaction).
Current shipping-source, hosted CI, Linux release, provider and P11 actions need
their own inputs/results; they do not reopen completed implementation by default.
