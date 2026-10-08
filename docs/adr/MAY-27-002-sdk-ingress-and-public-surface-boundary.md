# MAY-27-002 — SDK Ingress and Public Surface Boundary

Status: `Accepted`

Decided: 2026-05-27

Consolidated: 2026-09-27

Implementation note (2026-10-08): optional source preparation is integrated in
local main `433a9363`, using the existing `SearchCorpusBatch` primitive. The
breaking `publish_outcome`, `activate_published`, and `SdkError::AfterPublish`
contract remains on `codex/sdk-preparation-final`; paired Semantica Runtime
acceptance is pending. Main retains `ActivationAfterPublish`; CLI and benchmark
consumers now handle it (`c75668c7`). See
[OCT-04-003](OCT-04-003-source-preparation-sdk.md#remaining-coupled-integration).

Source programs: May-24 lexical/indexing closeout and May-25 SDK/ingest IPC
cutover

## Context

The first search-plane and lexical programs mixed public API design, transport
cutover, implementation order and dated local receipts. They also described a
retired channel-based runtime and several vendor-specific intermediate
architectures. Those packets are useful history but cannot remain current API
or runtime authority.

The durable boundary is the external SDK and contract relationship. Query,
publication and lifecycle behavior must converge on that boundary without
allowing a second raw transport path to become public authority.

## Decision

### Public entrypoint

The supported external Rust path uses `quanta-index-sdk`. The contract crate
owns closed DTO and wire shapes; it does not own transport, storage or
application policy. The SDK owns typed request construction, transport
invocation and exact request/response identity binding.

Raw IPC remains an adapter and negative-test seam rather than a second supported
application client. Contract and IPC types are public Rust types, so this is an
ownership rule enforced by dependency/API review and public-surface checks, not
a claim that construction is technically impossible.

### Namespace and authority split

One client exposes typed namespaces for lexical, semantic, symbol, history,
runtime, structural, RepoMap, generation/control and observability operations.
Each namespace lowers to the canonical contract route. Namespace convenience
does not create a parallel engine or a second source of query semantics.

Producer-owned authority families publish through family-specific typed
batches. Lexical source records use the search-corpus batch; history, runtime
and structural records retain their distinct identities and readiness rules.
One family cannot reuse lexical readiness or silently encode its records as
lexical payload.

### Publish lifecycle

Batch construction is deterministic and validates repository, revision,
generation, scope and mutation conflicts before transport. The canonical body
digest binds the bytes submitted to the daemon. The SDK validates that the
returned receipt and response correspond to the request it sent.

Publish, seal, activation and query visibility are separate service operations
and claims. Activation uses an explicit compare-and-swap lifecycle.

Main `433a9363` exposes `SearchCorpusNamespace::publish` and
`publish_and_activate`; verified post-publication failures retain original
evidence in `ActivationAfterPublish`. These calls are not atomic.

**Isolated breaking candidate:**
`SearchCorpusNamespace::publish_outcome(&batch)` returns the validated
`SearchCorpusPublishOutcome` with the original
`SourcePublicationBinding` and committed `BatchPublishReceipt`. This matters for
source-event replay, where the original publication target can differ from
the submitted batch target. `PublishedBatchEvidence::from(&outcome)` packages
that identity and receipt; callers use
`SearchCorpusNamespace::activate_published(&evidence, expected_active)` for a
separate, explicit activation CAS; activation requires a sealed receipt.
Invalid evidence or expected-head input is refused before control I/O, while
control failures retain the checked evidence in `AfterPublish`. The SDK also retains the convenience
`publish_and_activate` path. Both paths perform separate service operations,
not an atomic transaction.

After a verified publish, a later observation or activation error carries the
original evidence and typed cause in `SdkError::AfterPublish`, with stage
`PublishedBatchFailureStage::Observation` or `Activation`. The caller can
recover it through `SdkError::published_evidence()`,
`published_receipt()`, or `published_publication()` and reconcile the
original publication before retrying. A transport or response failure before
a validated publish outcome remains uncertain and does not manufacture a
receipt. `GenerationNamespace` and `ControlClient` do not own this
search-corpus activation CAS; the method is on `SearchCorpusNamespace`.

### Retired paths

The former channel crate and channel-specific public publishers are not part of
the live workspace or SDK dependency surface. Search-plane application code
owns ingest dispatch and durable lifecycle coordination. Historical channel,
in-memory HNSW and early Lance adapter descriptions do not override current
source or the later semantic-generation ADR.

### Client transport ownership

The production client directly owns its UDS query, control and ingest transports
inside the shared client payload. Their paths and I/O policies have exactly the
client's lifetime. They have no independent aliases or shared headers. Ordinary
client clones retain the same outer client and request-ID counter; dispatch
borrows a transport without retaining another counter. Query-only clients keep
control and ingest absent. Test-only injected transports do not change this
production representation.

Configuration consumes an explicit state-root backing instead of cloning it.
Each default socket uses one relative suffix under that root, with no
intermediate `search-plane` PathBuf. Environment precedence, explicit socket
overrides, profile selection, and timeout/deadline validation remain owned by
`ConnectOptions`. An absolute `Instant` is preserved through every request.
Client construction does not dial a socket; I/O starts at transport dispatch.

Removing the production trait-object transport headers also makes the client
and its borrowed namespaces unwind-safe by auto-trait inference. Public method
signatures and client cloning semantics are unchanged; the public API snapshot
records this auto-trait expansion.

Focused local validation (2026-10-09): SDK `test --all-features --locked` passed
167 tests, with six environment-dependent tests ignored. SDK all-targets,
all-features Clippy with `-D warnings`, scoped formatting, public-API and
hexagonal gates passed. The existing `runtime_fast_suite` test
`sdk_frontdoor::publication_tests::sdk_default_code_search_matches_terms_across_chunks_as_one_file`
also passed, exercising actual UDS publish/activation/query and fixture shutdown.
Commands use `./scripts/cargow --lane test-sdk-binding-owner-lane` for the SDK
and `--lane test-daemon-lane test -p quanta-index-searchd-runtime --test runtime_fast_suite --all-features --locked`
with the full test name and `-- --exact` for the daemon harness. These focused
results do not qualify native Source custody, the whole daemon suite, installed
process behavior, or remote CI.

### Native connect producer and open receiver co-cut

Status: **Index producer implemented as a candidate; actual Core/Original Source
receiver binding and qualification remain open**. This is distinct from the
transport ownership decision above and the native corpus decode candidate.
Ordinary `connect`, including its remaining outer std Arc, cannot be adopted as
a Source-admitted client after construction.

The current Semantica receiving functions take an explicit borrowed state root
and default, relative `Duration`, or absolute `Instant` I/O policy. A native
entrypoint for these calls must run the same configuration decisions, payload
construction and namespace/dispatch bodies. It must not add a second connector,
query implementation, allocator, root issuer, or environment-resolution policy.
The explicit-root path does not consult the environment. Native support for
environment-created input is a separate admission requirement, not an implicit
fallback to `std::env::var` inside a Source loan.

The remaining interface decision has two concrete owners:

- Index owns the nonshared client payload, canonical path construction, and
  transient producer loan. It must park complete paths, partial path backing,
  original `TryReserveError`/`IpcError`, original non-Copy admission failure,
  and candidate payload in caller-owned DATA before any late refusal. Invalid
  I/O policy is checked before path birth. Raw platform path bytes must retain
  their existing semantics, including non-Unicode Unix paths; a String bridge
  is not a substitute. Occupied/reused DATA must reject before any poll or birth.
- Semantica owns the concrete shared handle and actual funding custody. Its
  receiver calls Core's existing
  `admitted_shared_value_into_slots_v3(admission, payload_slot, shared_slot)`
  with `NativeSharedValueV3<IndexClientPayload>` as the concrete output. Paid
  aliases use the same Core
  `admitted_retain_shared_value_into_slot_v3` and counter. Neither an ordinary
  std Arc nor a newly implemented reference counter satisfies this seam.

The SDK port `native_connect_v1::NativeSdkConnectAdmissionV1` has associated
`OriginalError`, `Funding`, and
`Shared: Deref<Target = QuantaIndexClientPayloadV1>` types. Its methods are
`consume_connect_work_v1`, `admit_path_birth_v1`, and
`birth_client_into_slots_v1`. The latter receives mutable external
`Option<QuantaIndexClientPayloadV1>`, `Option<Shared>`, and `Option<Funding>`
slots. The payload has private construction and no infallible native clone.
The receiver binds `Shared` to the actual Core type; Index has no dependency on
Semantica's source, funding, or allocator crates.

`try_connect_native_into_v1(ConnectOptions<&Path>, ClientProfile, &mut policy,
&mut NativeSdkConnectDataV1<OriginalError, Funding, Shared>)` returns only a
finite attempt status. Complete admission and `IpcError` causes are observed
through `failure_v1`; an actual `TryReserveError` has its own
`reserve_failure_v1` slot, preserving it even if admission also refuses after
the physical callback. `complete_into_slot_v1` is a pure move; occupied output
and used DATA reject without polling admission or replacing state.
`client_v1` borrows only a producer-complete payload. Neither observation nor
transfer performs the receiving Source's terminal classification.

Owned and borrowed options use the SAME generic `ConnectOptions<P>` carrier,
root/socket decisions, raw path producer, and I/O-policy validator. Ordinary
and native construction call the SAME payload factory; all SDK namespaces and
dispatch methods borrow that payload. There is no native RPC implementation.
Ordinary `QuantaIndex` also implements `Deref<Target = QuantaIndexClientPayloadV1>`.
A receiving ingress can therefore use the SAME typed client-owner bound for
the ordinary client and Core's actual `NativeSharedValueV3` and return a borrowed
payload from `client_ref`. This borrow neither retains a shared counter nor
adopts the ordinary client's Arc as a Native allocation.
The native port lends path/shared birth only during the synchronous producer
call. Its receiver contract forbids Source, current-control, input references,
or callbacks in retained DATA or funding.

The connect work model, separate from the Core producer's existing header and
paid-retain costs, is:

| Operation | Work before the operation |
| --- | --- |
| Initial/final and post-path lifecycle checks | 0 (checkpoint only) |
| Root decision | 1 |
| Each selected socket decision | 1 |
| Each path reserve/fill | Planned encoded-byte capacity |
| Nonshared payload construction | 1 |

An explicit path uses its raw platform encoded-byte length. A derived socket
uses that length plus the fixed relative suffix and one conservative separator
byte, with checked arithmetic. The actual `try_reserve_exact` runs inside the
host admission callback before the SAME `PathBuf::push` producer. Capacity is
checked before and after fill; mismatch refuses publication. The separator
can overcharge one byte for an empty root or existing trailing separator. No
String conversion, environment-created input, or unadmitted path copy occurs.
The receiving owner must accept this work model and retain its complete
Original Source cause through the highest finisher.

The existing Semantica host seam is
`OriginalCanonicalAdmissionV3::try_from_original_group_v3` followed by
`native_temporary_v3(&OriginalCanonicalNativeStorageRefusalV3)`.
`OriginalCanonicalNativeStorageV3` implements Core's
`NativeAllocationAdmissionV3`; its birth method delegates to the authentic
control's `admit_owned_native_birth_v3` on that SAME externally retained group.
The receiver can lend this existing admission to both the actual path callback
and `admitted_shared_value_into_slots_v3`, retaining the group and refusal DATA
outside the client. This is source inspection of an available seam, not an
executed receiver binding. Do not substitute the normalization-only three-slot
funding carrier, create another allocator, or wrap an ordinary client in a
native header.

All physical state and output slots precede the actual funding bank in DATA's
drop order. The bank is not inside the shared payload: physical header release
can occur after the last payload destructor. The caller retains that bank
through the highest Source finisher and until every strong/weak handle and
dependent backing has dropped, including after a successful output transfer.
Late cancellation, deadline or header checkpoint refusal must leave candidate
state and full original failure in DATA for the same terminal classification;
no retry may restart the absolute deadline or replace the original cause.

Acceptance requires the actual receiving Core call and Original Source path,
pre/post-birth refusal tests, exact path/profile/deadline parity, alias/header
funding lifetime tests, and real UDS dispatch. Source review, SDK compilation,
or ordinary daemon tests alone do not establish native acceptance. Integration
of this ABI is one coupled producer/receiver change.

Focused candidate validation (2026-10-09): SDK
`./scripts/cargow --lane test-sdk-binding-owner-lane test -p quanta-index-sdk --all-features --locked`
passed 176 tests, with six environment-dependent tests ignored; SDK all-targets,
all-features Clippy with `-D warnings` passed. The nine added tests cover complete
causes/partials, malformed callbacks, used DATA, path/profile/absolute-deadline
parity, occupied transfers, and actual three-plane UDS route/codec/request-ID
behavior through the SAME payload. Their shared owner is an inline test double;
they do not exercise Core shared allocation, paid aliases, or Original Source
funding lifetime. The existing daemon harness named above was rerun after the
common-payload refactor and passed one test (67 filtered), exercising ordinary
UDS publish/activation/query and shutdown. The earlier 167-test result belongs
to the transport-owner candidate; neither ordinary daemon run establishes
Native receiver acceptance.

## Consequences

- Adding a public operation requires a contract route, SDK namespace or
  existing namespace extension, exact binding validation and an owner proof.
- Generic namespace plumbing stays internal. Public APIs remain family-shaped.
- Current support is read from SDK exports, contract route inventories and
  current runtime code, not from archived ticket status.
- Verified post-publish observation or activation failure retains original
  typed evidence. Pre-outcome uncertainty still requires event-identity and
  active-head reconciliation; neither public activation path is atomic.
- Cross-repository producer adoption, installed-process behavior and release
  qualification remain separate from this accepted repo-local boundary.

## Related decisions

- [JUN-02-001](JUN-02-001-search-dsl-authority-and-runtime-contract.md)
- [MAY-31-001](MAY-31-001-lancedb-semantic-generation-authority.md)
- [SEP-21-001](SEP-21-001-canonical-identity-and-digest-domains.md)
- [SEP-21-002](SEP-21-002-durable-authority-and-operation-lifecycle.md)
- [SEP-21-003](SEP-21-003-read-view-continuation-and-provider-policy.md)
- [Source-preparation SDK](OCT-04-003-source-preparation-sdk.md)

## Historical record

The absorbed packets and exact consolidation boundary are listed in
[the completed-plan archive](../ARCHIVE-INDEX.md#historical-record-recovery).
