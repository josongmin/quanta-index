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

## Completed execution scopes

O4-E3-01 (retire-before-acquire), E3-03 (seven single-RPC bindings), E3-04
(slow-disk readiness), E3-05 (admitted timeout/replay) and E3-06 (authorized
operator projection) are completed implementation/owner scopes. E3-02's stronger
selection pin was not adopted and is `NOT_APPLICABLE` under the current decision.
Their individual ticket headings are retired. A later source change uses the
existing regression owners; installed Linux/release/pair/action acceptance stays
with I0/SEP-21 rather than reopening the same implementation tickets.

## Consequences

Requested owner/process scenarios were executed historically. Their exact source,
commands and failure history remain in the [plan history index](../ARCHIVE-INDEX.md#historical-record-recovery).
Current shipping-source, hosted CI, Linux release, provider and P11 actions need
their own inputs/results; they do not reopen completed implementation by default.

## Publication success with activation failure

The SDK previously returned only the activation error after a verified sealed
publication. That discarded the durable publication result and allowed a caller
to mistake activation uncertainty for an unpublished batch. Source-event replay
also permits an original publication target to differ from the submitted target;
the receipt alone cannot reconstruct the original binding.

Quanta local main `0760680de8367bb9078f31438b017c227b776c27` retains both
`SourcePublicationBinding` and `BatchPublishReceipt` in boxed
`SdkError::ActivationAfterPublish` evidence, with the original typed cause.
The getters return the verified original publication and receipt. All errors
after the validated publish outcome use that boundary, including observation,
activation request/response, transport and timing errors. Invalid expected-CAS
input still refuses before publication. The box allocates only on the failure
path; successful SDK results retain their existing shape. Public API inventory
was updated with the complete producer change.

The complete Semantica consumer bundle is on local main
`6bab127740a3c22eba96df5f6ce597047efb8a18`:

- Ingress retains the evidence and original remote error code, and disables
  automatic retry after verified publication.
- The existing outbox stores publication evidence separately from a successful
  delivery receipt. Submitted binding stays immutable; replay may retain a
  different original target. Applied and replay receipts both validate identity,
  counts, canonical digest and original publication/receipt consistency.
- SQLite schema21 adds nullable publication evidence to schema20 rows. Retry
  selection and claim/completion CAS fences refuse rows that already contain
  publication evidence. Restart never interprets that evidence as delivery ACK.
- The existing V9 aggregate replay row stores member/lease-bound evidence before
  returning ordinary or recovered dispatch failure. Capture advances its canonical
  replay JSON and state revision in one immediate transaction. Lease recovery,
  phase mutation and further member dispatch refuse the retained evidence;
  SQL triggers reject clearing/replacing it or promoting the row to terminal.
- Capture/store/CAS failure preserves the evidence in owner-local boxed errors.
  The legacy outer Runtime error remains string-based and serializes the evidence
  into its context; this is not a new typed public Runtime error contract.

These barriers require reconciliation with the actual active head before another
publish decision. They establish neither activation success nor automatic repair.
The kernel and all outbox/aggregate producers, SQL readers/writers, callers and
regressions were integrated together; no partial SDK-only consumer cut was used.

### Verification and publication boundary

The completed candidate uses SDK `906f3b5d` plus `13b00554` and Semantica
`32833735dd3` plus `7fea67b1063`. All7 SDK and19 consumer owned files match their
respective integrated main bytes; unrelated Semantica dirty work was preserved.

- `./scripts/cargow test -p quanta-index-sdk --lib --locked`: candidate and
  integrated Quanta main `0760680d` each passed128/128. `./scripts/cargow clippy
  -p quanta-index-sdk --lib --tests
  --all-features --locked -- -D warnings` and public API check passed. Integration
  targets compiled with `--no-run`; this does not establish daemon execution.
- Final coupled kernel: `./scripts/quanta-build-cli owner run --lane
  root-sdk-recovery-final --package quanta-runtime-retrieval-kernel
  --execution-kind test --target-kind lib --selector-mode exact --selector
  index_sdk_ingress::publish::sdk_error_classification_v1::tests::activation_failure_preserves_publish_evidence_and_original_remote_code
  --compile-policy feature-isolation:quanta-runtime-retrieval-kernel.no-default.ed0772b29304
  --jobs 2`, with its issued lane token:1/1 passed. Run
  `20261008T030313.080841Z-09e4b1239304` has QBC
  `publication_state_v1=committed`; the earlier pending result is not reused.
- Python SQLite exercised the actual schema and extracted migration/trigger SQL:
  evidence capture succeeded; clearing it, changing to terminal and mismatched
  replay revision raised `IntegrityError`. This verifies SQL guards only.
- Both full and narrowed Runtime Rust attempts failed before tests in the existing
  parser on candidate base `f574fa82` (105 diagnostics). Runtime test bodies,
  including restart/migration/capture refusal, are `NOT_RUN`. Later primary main
  parser commit `d29ac508` changes two files; the old105 count is not a current-main
  compile result. Coupled product/restart qualification remains `BLOCKED`.
- Semantica canonical commit passed staged integrity, all18 changed Rust formatting
  checks and changed safety fences. Its canonical push refused because remote
  `035961a8efc` is not the task commit's exact parent `d29ac508`:110 earlier local
  commits belong to other work. Root held SDK-only remote publication, but the
  separately authorized existing commit/push heartbeat subsequently published
  Quanta `0760680d` and documentation `6870a635`. Its actual push/fetch verified
  Quanta HEAD and origin/main equality. Semantica consumer remote publication
  remains incomplete; Quanta remote publication does not establish pair completion.
  The publishing chat received the coupled owner boundary. Local integration,
  individual repository publication and release qualification remain distinct.
