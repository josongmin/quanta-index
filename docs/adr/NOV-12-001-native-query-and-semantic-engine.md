# Native Query and Semantic Engine

Status: `Accepted`

Decided: 2026-11-12

Scope: implemented Native selected-view contracts. This record does not qualify
the current source, installed product, default switch or performance.

## Context

The Nov-12 Native work replaces the Native ANN build/query adapter and joins
file-ranked lexical and semantic readers. The Legacy semantic generation still
uses the LanceDB backend recorded in
[MAY-31-001](MAY-31-001-lancedb-semantic-generation-authority.md).
The earlier Native Lance implementation and its fixed-cohort results remain
historical evidence. They do not describe the current Native graph consumer.

## Decision

1. **Native ANN representation.** Native semantic publication uses bounded,
   immutable HNSW regions and a persistent region directory alongside the
   existing exact vectors and sealed delta. Deterministic recipe/input binding,
   selected membership, original-vector re-scoring and explicit traversal/cap
   results remain required. Catalog owns publication and retained-view lifetime;
   the graph does not publish or reclaim selected state independently. Region
   routing, graph visits, authentication and re-scoring share the original work
   and memory admission. This choice makes no universal quality, scaling or
   comparative speed claim. See
   [graph](../../crates/quanta-index-semantic/src/native/embedding/native_graph.rs),
   [materialization](../../crates/quanta-index-semantic/src/native/embedding/ann_materialization.rs)
   and [query](../../crates/quanta-index-semantic/src/native/embedding/ann_query.rs).
2. **Prepared publication.** The producer lends or moves the existing canonical
   artifact, child references and allocation custody through
   [SearchablePreparation](../../crates/quanta-index-core/src/index/searchable.rs).
   The [Kernel](../../crates/quanta-index-kernel/src/update.rs) checks the original
   attempt and child-first installation result before publishing coverage.
   Preparation, reservation, installation or confirmation failure cannot issue
   a successful new root. Scalar/no-child cases retain their original path.
   This is a shared preparation contract, not another codec or physical writer.
3. **File identity.** File candidates use the selected source-file `OwnerKey`,
   authenticated source reference and record. A projection owner or pathname
   cannot substitute for that identity. Semantic vectors are deduplicated by
   file before the candidate window, using the best score and deterministic
   vector tie break. Source-free projections do not acquire file authority.
   See [source-file custody](../../crates/quanta-index-semantic/src/native/embedding/source_file.rs).
4. **Retained pages and Hybrid.** Text pages bind continuation to the same
   view, instance, query and ranking artifact, with descending score and
   ascending full-owner ties. Eligibility and strict rank-after selection run
   before the bounded heap. `matched` counts all eligible matches; `remaining`
   counts the strict suffix. Seek, heap and final sort share one comparator.
   The anchor must be an eligible owner with its exact score; drain validation
   before refusing a missing/ineligible anchor so it cannot hide late corruption.
   Producer identity, source/path authentication and post-filter-before-top-k
   remain required. Empty pages retain no unused hit-memory lease. See the
   [lexical ranker](../../crates/quanta-index-lexical/src/native/text_query.rs).
   Hybrid runs independent Text and Semantic readers
   through one retained read attempt and cumulative budget. Shared eligibility
   applies to both lanes; dense-only files remain eligible. Unsupported inline
   lexical-only filtering is refused before the two-reader execution. File RRF
   uses `k=60` and explicit lane witnesses; a bounded union does not establish
   exhaustive global top-k. See the
   [wire](../../crates/quanta-index-contract/src/native_protocol/read_session.rs),
   [Text pages](../../crates/quanta-index-searchd-runtime/src/native_read_session_transport/page.rs)
   and [Hybrid](../../crates/quanta-index-searchd-runtime/src/native_read_session_transport/hybrid.rs).
5. **Read and response custody.** Query access does not authorize publication or
   administration. Session identity, view and instance checks remain on the
   existing authority path. Attempt completion retains response memory through
   emission and does not release the session pin. Only closed error/submission
   outcomes opt into bounded terminal reply emission; successful reads,
   Accepted and successful status replies keep their original dispatch deadline. The SDK
   keeps the original client budget and durable operation key. See
   [dispatch](../../crates/quanta-index-searchd-runtime/src/native_dispatch.rs),
   [reply](../../crates/quanta-index-searchd-runtime/src/native_reply.rs) and
   [Native SDK](../../crates/quanta-index-sdk/src/native_uds.rs).

## Request-budgeted control and ticket custody

Ordinary Status/ReadView inspection carries the request's `WorkBudget` through
Catalog lock acquisition and the post-read checkpoint. `OperationInspectAccess`
separates `Request(&WorkBudget)` from mandatory `PostFailureReconcile`; expiration
cannot suppress reconciliation of a durable terminal or Unknown outcome. See the
[control port](../../crates/quanta-index-core/src/index/ports.rs) and
[Catalog implementation](../../crates/quanta-index-catalog/src/native/authority.rs).

`pin_view` passes the same budget into ticket acquisition. Check it immediately
before COMMIT; refusal rolls back. After successful COMMIT the ticket must reach
its custody owner. ReadSession Open owns that ticket before its final request
checkpoint, so a lost/cancelled reply retains the same idempotent Open and pin.
A retained query ticket does not keep its issuing request's diagnostic context;
short materialization observation and mandatory release retain their own scope.
See [ticket acquisition](../../crates/quanta-index-catalog/src/native/authority/lifetime.rs),
[Kernel pinning](../../crates/quanta-index-kernel/src/update.rs) and
[session registry](../../crates/quanta-index-searchd-runtime/src/native_read_sessions.rs).

## Retained profile and terminal reply details

`ReadProfile` consumes the existing UID/session ticket without reopening or
refreshing Latest. It returns the same retained view and instance profile;
the SDK checks session, view and canonical profile identity under its original
request budget. See [profile consumer](../../crates/quanta-index-sdk/src/native_uds/read_session/profile.rs)
and [ticket dispatch](../../crates/quanta-index-searchd-runtime/src/native_read_session_transport.rs).

Bounded terminal emission preserves explicit cancellation as `Cancelled` and
request-time exhaustion as `ResourceLimited`. An uncertain durable mutation
keeps its original operation key; status/resume uses a separate request budget.
These contracts do not qualify an installed DELTA or a different-UID/default
caller merely because the local reply/reader tests passed.

## Root-scoped view observations

`ObserveRootView` returns the selected CatalogView with the actual root nonce,
device and inode. The live daemon validates its retained StateRootLease before
and after reading the immutable stamp through its own canonical Catalog
connection. View selection, canonical digest and the original request/work
budget remain on the existing read path. The offline read-only verifier still
excludes a live writer; it is not reused to reacquire that writer's root.
See [live dispatcher](../../crates/quanta-index-searchd-runtime/src/native.rs) and
[Catalog stamp observation](../../crates/quanta-index-catalog/src/native/authority/capture_format.rs).

The admitted SDK method retains one exact response, raw/decoded memory grants
and kernel UID/GID/PID from the same connected socket. Peer observation occurs
before request bytes are written; reconnecting or trusting a self-reported PID
cannot substitute it. Missing/zero/unreadable PID refuses the admitted route
before writing. The ordinary metadata API returns DATA without this peer
custody. See [SDK owner](../../crates/quanta-index-sdk/src/native_uds/root_scoped_view.rs)
and [connected transport](../../crates/quanta-index-ipc/src/server.rs).

These are observations, not Source, publication, repository EX or cutover
permits. The installed provider must independently bind the actual peer image
and expected root while borrowing the same owner. Linux peer-PID positive,
provider/Source connection and installed recovery retain their own acceptance.
Existing ReadView and PublishedViewMetadata contracts remain separate.

## Native History publication and read contract

The [source carrier](../../crates/quanta-index-contract/src/index/history/source_input.rs),
[replay](../../crates/quanta-index-search-plane/src/auxiliary_authority/history_source.rs),
[typed builder](../../crates/quanta-index-search-plane/src/native_history/build.rs),
[row-map materializer](../../crates/quanta-index-search-plane/src/native_history.rs),
[reader/planner](../../crates/quanta-index-search-plane/src/native_history/reader.rs)
and [query](../../crates/quanta-index-search-plane/src/native_history/query.rs)
are implemented source contracts. This consolidation does not qualify a new
source cohort or supply an actual installed producer/migration result.

- **One source carrier.** Contract owns HistoryReplayFenceV1 and the existing
  HistorySourceInputV1 v1; SearchPlane reexports the fence. The replay input carries original HistoryIngestBatch/full BatchPublishReceipt
  publications with exact ordinals and before/after replay positions.
  The authenticated inventory owner supplies generation, optional manifest,
  count, first/final replay and final durable sequence under its writer fence.
  Shape validation alone grants neither completeness nor publication authority.
  The existing opaque stager uses empty children; this is immutable staging,
  not Projection publication or a new RPC/upload/session/IR/authority.
- **Original admission.** The existing immutable input ceiling is 16 MiB;
  IPC/frame/Original may refuse sooner. Decode reserves scratch before allocation
  and observes the same WorkBudget with bounded 4 KiB buffered reads. CPU/memory
  demand is positive and immutable; provider_units is zero. Oversized or
  incomplete input fails closed. The replay carrier is bounded to one input.
  Snapshot FULL uses the immutable pages described below.
- **FULL and DELTA.** NativeHistoryBuildV1 implements IndexHistoryBuildPort using
  the actual AttemptContext, actual prior member and original budget/custody.
  Replay FULL consumes every declared ordinal from genesis and finishes at the
  exact complete fence. Snapshot FULL binds the checkpoint and exact immutable
  page closure. DELTA opens the committed prior binding and cut, requires
  exactly one successor epoch and original publication, and compares the
  reconstructed cut with next. A full inventory cannot impersonate DELTA.
  Batch bodies are independently hashed; ACK scope/domain/sequence are checked.
  Global durable ACK sequence gaps from unrelated domains are permitted.
  Cancellation and work refusal retain the original cause.
- **Derived member.** prepare_native_history_full_v1 and the existing
  NativeHistoryReadV1::prepare_delta produce PreparedNativeHistoryV1; its
  artifact/into_member methods retain the existing typed member contract.
  verify_native_history_input_v1 structurally verifies the generated artifact,
  not source-wire completeness. Profile/bootstrap/base-view CAS and publication
  stay with Kernel. Runtime registers the actual typed build and query ports.
  The manifest has exactly row-map and source-input children; prior cut is
  metadata, not a physical prior-artifact edge retaining the entire chain.
  CoreRequired/InputDomain::History uses the existing kind/schema/format/policy;
  member input.root is the row map, context is the common cut digest and
  logical_commitment is the canonical complete source-row digest.
- **Rows and custody.** Source-only replay determines expected rows independently
  of target Catalog, ordered epoch/commit/diff-hunk/ref/state-meta/tag. Typed JSON
  rows reuse Core HistoryReadbackRowV1 with manual codecs; commitment keeps the
  canonical source CBOR framing. Untrusted sequence hints are not storage rows.
  Keys are family + SHA256(raw key); rows retain/validate the full key, including
  the 4,083-byte diff-path regression. DELTA retains ordered same-key ref/tag
  updates and explicit deletes. Complete snapshot hashing visits all rows;
  it is not an incremental full-row-hash speed claim. Superseded grants retire
  after old rows are destroyed, not per historical publication; late failures
  release provisional DATA.
- **Reads and ranking.** plan_native_history_read_v1 and NativeHistoryReaderV1
  reuse IndexQueryPort, the common lowerer/grammar, registry and
  with_prepared_stage_for_read_attempt. Planning charges original bytes before
  allocation and refuses unsupported current-file language/exact-path clauses;
  History file: diff predicates remain supported. Native cursors bind actual
  view/member/query/order/page size; inner HistoryCursor is only a positional
  boundary, not a Legacy pin/epoch alias. Reader validates member and exact
  cursor row/time/score; the whole NativeHistoryQueryResultV1 retains decoded
  source/response DATA through encoding/write, without an into_page escape.
  Recency keeps canonical predicates/selector and per-row Work. Relevance uses
  separate complete commit/diff populations, canonical tokenization and
  exact-length BM25 k1=1.2/b=.75; only matching Boolean branches score.
  Filtering precedes top-k; matched and strict suffix remaining are exact.
  No Tantivy compressed-fieldnorm bit-identity claim is made.

### Legacy History replay during cutover

Semantica retains the original publication body and actual Q ACK before outbox
unlink or aggregate terminal completion. The candidate reader lends the bounded
inventory under one producer writer fence, after comparing the complete
generation list with the authorized selection. The candidate path appends a
History FULL for each selected generation after the attested AST publication
and binds the terminal operation before BOOT.

Q captures the ordered candidate publications in one Catalog transaction and
checks the exact predecessor and History profile addition at every step. Under
one held Legacy Catalog snapshot it counts retained generations, matches
terminal journal ACKs, verifies the exact row-set digest and durable epoch, and
recomputes replay genesis from the admitted producer namespace and first
original body/ACK. BOOT matches every verified cut and input against its
History member in the final selected map, even after intermediate Exact views
retire. It compares the member manifest's format, durable sequence and source
manifest with the authenticated replay or checkpoint input. It releases the
input payload before reading the member's immutable rows with the existing
paged map scanner. The History restorer checks row keys, inventory and commit
relationships and recomputes the canonical snapshot digest under the recovery
work and memory budget. It releases restored rows before
checking the next generation. The unselected candidate holds its publication
chain through BOOT verification. Automatic retention resumes when the root
serves the selected configuration.
[Candidate verifier](../../crates/quanta-index-searchd-runtime/src/native_boot.rs) ·
[Catalog source verifier](../../crates/quanta-index-catalog/src/native/authority/read_only.rs)

Generations with discarded originals need a producer-authenticated snapshot
checkpoint. Q can observe the settled Legacy row commitment, epoch and last
applied journal ACK under one WAL snapshot. Native History FULL can stage an
ordered set of immutable pages and its checkpoint. Before selection, the Q
recovery child compares the checkpoint with the live Legacy rows under the root
lease. BOOT streams the sealed pages through the same reader used by FULL and
recomputes the authenticated row digest before checking the installed Native
row map. Page identity, order and row count are checked under the existing
recovery budget. It authenticates the inherited FD7 producer writer lock, rederives the
fresh replay position from the physical snapshot and original ACK, and binds it
to the FD6 producer identity and Legacy root. The launcher's snapshot mode binds
the active Legacy selection path and SHA. The private Q exporter checks that
selection under the deployment lock and takes the existing old-root lease. After
the old-root drain, S first compares the physical History key inventory from that
exporter with its selected targets. It requires a complete, exact set and a
successful child exit before opening the first History ingress. The stream
must contain strictly ordered physical keys, the expected repository and exact
count. S checks every key against its candidate list. The installed process
fixture omits one physical key and requires refusal before selection or BOOT.
The same exporter lends each generation's pages and terminal observation
before the launcher seals the candidate Catalog digest. Page grouping accounts
for the admitted single-frame CBOR StageProjectionOutput envelope, including
the widest request and ingress IDs and the byte-array encoding. Retained replay
can prepare from S originals while Legacy serves and still passes Q's
independent physical verification during BOOT. A peer-bound Native
StageProjectionOutput can send an admitted replay object over the existing
fragment wire when its CBOR envelope exceeds one frame. The encoder serializes
the counted CBOR body into the retained Original allocation and inserts
fragment headers in place. The same allocation backs the write. The
History source object keeps its existing storage and producer admission bounds.
S checks the returned immutable object reference in both source modes. A
failure during profile or input staging attempts to discard the exact ingress
while its request budget is live. After Submit, S reconciles by operation key.
Q reads the selected History member from its immutable map, checks the artifact
against the verified FULL input, and returns that operation, view, member and
cut in the FD3 success terminal. S archives the terminal before restarting the
producer. Reconciliation verifies the same witness again when it has a retained
terminal.

Source and RepoMap have separate active-head selection. BOOT also requires
preservation of active sibling SearchCorpus and RepoMap heads. The Source and
RepoMap owners must verify the complete active inventory and carry it into the
candidate or retain a verified serving route for each head.

Replacing an already-installed Native root is a separate path. Its current
Published-operation census accepts signed SourceAdvance and AST work. Roots
with published History FULL, append or SourceHistoryAdvance require a History
carry-over contract before that path can replace them.

Installed acceptance also covers the interruption after History commits and
before the launcher receives the handoff. The shared candidate caller checks
Latest against the exact published History operation before resuming the
authenticated Source/AST replay and original inventory. The retry uses durable
Published metadata for the AST anchor and each checked predecessor.
For an existing History operation, S recomputes the input reference from
retained originals or from the authenticated snapshot export without opening
an ingress. It reissues the exact Submit so Q compares the immutable request
reference. A pending request can Resume after that comparison. A Published
request must return the same terminal view. The request comparison survives
retirement of the intermediate Exact view.
H-ACCEPT-01 requires original Legacy publications, complete generation coverage
after FULL, a real successor DELTA and a query after restart. The Native
`HistoryAppend` update keeps one History generation. A new S manifest uses
`SourceHistoryAdvance` to publish Source and the immediate History generation
in one Catalog operation. The adapter derives the successor from the exact
prior cut and original batch, preserving parent, commit and ref closure. The
new Source generation can have a different revision in the same repository.
The original binds the new revision, and the committed prior cut remains
part of the successor manifest. A retry uses the immutable operation key and
request. Q verifies the exact
`RepoCommitRecency` and `FileContributor` values against the original and
publishes their object references with the History member. `ReadHistoryCut`
and `ReadHistoryGeneration` report that member and its input references from
one pinned view. The generation readout also returns the committed
predecessor cut commitment for replay after prior view retirement.
The S V9 replay decoder retains the exact prior cut under Original admission.
On an active History profile, ordinary dispatch selects the predecessor at
HistoryMain and stages a distinct successor instance and frozen input under
the Source ingress. It commits the exact combined request to V9 before Q
Submit. HistoryMain, the required derived members and Lexical verify the same
Published operation and view before their admitted V9 receipt append.
The optional installed process fixture seeds two durably acknowledged Legacy
History cuts with the same ordinal at different revisions. The later sorted
cut is History-only. A real SDK edit binds its predecessor to the current
Source revision's cut. The fixture exits S after History publication before
the V9 ACK and checks exact readback after Q restart and S reattach. Signed
execution remains an acceptance condition.

The History pager and BOOT census bind exact revision/generation pairs
within a repository. Candidate cuts and retained replay use the ordered
`(generation, revision)` key, so the same ordinal can occur at different
revisions. S preflights each selected revision's inventory under one producer
fence before staging FULL. Q independently checks physical coverage and
applied ACKs. The handoff verifies one selected active SearchCorpus and its
required RepoMap closure. Sibling Legacy active heads require their own
verified migration or preserved routing.
[History backlog](../plans/nov-12-native-cutover/tickets/NC-H-history.md#current-actionable-backlog--2026-11-16-re-audit)

### Required History data survives terminal refusal

The History Required DATA readback keeps its validated bytes and seal when
additional memory for the private terminal read is refused. The Runtime
regression reopens the persisted V3 artifact first, then verifies that a refused
terminal loan leaves the callback uncalled and stored terminal bytes intact.
S CLI callback and signed Native V5 terminal recovery have separate acceptance.
[History owning proof](../plans/nov-12-native-cutover/tickets/NC-H-history.md#s-cli-and-required-history-data-proof)

## Engine composition and retirement

Ordinary [SDK queries](../../crates/quanta-index-sdk/src/client.rs) and
[searchctl dispatch](../../crates/quanta-index-searchctl/src/lib.rs) select
Native. Lexical and Runtime have empty default feature lists. The current
default graph omits Tantivy. Explicit `legacy-tantivy` and `legacy-bench`
builds still supply Legacy execution and comparisons. The workspace dep/patch,
nine Tantivy-family lock entries and vendor sources remain. F owns their
coherent removal and release qualification and retains the existing physical
recovery readers for Legacy state.
[Lexical features](../../crates/quanta-index-lexical/Cargo.toml) ·
[Runtime features](../../crates/quanta-index-searchd-runtime/Cargo.toml)
[F retirement scope](../plans/nov-12-native-cutover/tickets/NC-F-integration.md#tantivy-separation-and-remaining-removal)

## Scope of prior decisions

- MAY-31-001 remains the Legacy generation decision. Native graph selection
  does not mean LanceDB, Lance I/O or Tantivy have left the shipped dependency
  closure or that default callers have migrated.
- The `ann_lance` link in
  [NOV-07-001 decision 11](NOV-07-001-native-index-execution-and-custody.md#decision)
  records the earlier Native adapter's admission/refill boundary. That module
  is now test-only; current production build/query follows the graph path above.
  Original work, identity and custody requirements remain in force.
- The [Native RFC](../rfc/oct-10-native-incremental-vnext.md#92-semantic-native-incremental)
  and [E04](../plans/oct-22-finalizing/E04-semantic-ann.md#original-4k-gate)
  keep their original correctness, resource and product acceptance criteria.

## Semantic mutation and admission

Native graph/directory/file readers are implemented; old B compile/mount and
Lance-replacement dispatch is historical. Reuse the existing graph, exact oracle,
source-file authentication and selected-view publication/pin contract above.
The [graph decoder](../../crates/quanta-index-semantic/src/native/embedding/native_graph.rs)
charges original byte Work before header interpretation or decoded allocation.
Ranking/seal scratch must fit original admission. After comparing a better
top-k candidate, the ranker releases the displaced hit's retained identity
before funding its replacement. Exact and overlay ranking share this lifetime.
In-place sorting uses the same comparator under the original visit, region,
rescore, recipe, wire and quality bounds.

[Directory mutation](../../crates/quanta-index-semantic/src/native/embedding/ann_directory.rs)
and [maintenance](../../crates/quanta-index-semantic/src/native/embedding/ann_directory/maintenance.rs)
validate existing and prospective owner/row counts before COW or prepared edits.
Every retained delta owner contributes a row; valid empty output removes the
member rather than publishing owner-with-zero-rows metadata. Capacity refusal
remains ResourceLimited, corrupt counters CorruptObject. Removal and consolidation
use local metadata and assign the caller only after the full operation succeeds.
A late failure cannot expose a partial directory; already staged unreachable COW
objects remain the original attempt's abort/reclaim responsibility, not a claim
of filesystem transactional rollback.

Consolidation authenticates every selected seal and member before applying
region removals. It builds the final member map and applies one sorted exact-CAS
removal batch to the original region root using the original descriptor refs.
Single-region retirement shares the root and count checks. The focused B129
regressions cover this change. The ANN4K and whole SDK product use the original
workload and deadlines. A refusal of the final region CAS after member staging preserves the original
caller directory and reachability. The fixed carry test verifies exact
original descriptors, refund on refusal and the final selected member map
on retry.
[Consolidation](../../crates/quanta-index-semantic/src/native/embedding/ann_directory/consolidation.rs) ·
[B129 evidence](../plans/nov-12-native-cutover/tickets/NC-B-semantic.md#b-product-compaction-deadline-129) ·
[B133 atomic refusal](../plans/nov-12-native-cutover/tickets/NC-B-semantic.md#b-carry-final-cas-refusal-133)

The pure [model metadata](../../crates/quanta-index-embed/src/openai.rs) and
[recipe/instance validation](../../crates/quanta-index-semantic/src/native/embedding.rs)
reuse canonical identity and recipe validation without executable providers.
Metadata preserves the pinned local identity or exact untrimmed openai:model,
revision, declared dimension and raw normalization. Raw None becomes L2Unit only
through the existing Runtime L2 wrapper. Pure validation calls no credentials,
environment, provider health/manifest, HTTP, pool or transport. Live construction
retains its auth-first and operational admission/error rules. Source/projection/
chunk recipe, schema/domain, AsyncCoverage activation and policy fingerprint
must match the actual instance; pure metadata is not a live-service permit.

Recall and result oracles remain independent fixed expectations. Duplicate graph
rows cannot count repeatedly toward recall; score/provenance, complete owner
membership and order checks precede quality claims. A fixture's configured owner
cap8 legitimately bounds a top_k64 response only with its capped/traversal witness;
that bounded process result does not certify all64 owners or the full4K product.
The [active B ticket](../plans/nov-12-native-cutover/tickets/NC-B-semantic.md#current-remaining-acceptance)
keeps dedicated ANN4K, original SDK product and consumer/installed acceptance.
Focused admission/custody/oracle repairs do not close those gates.

## Admitted SDK read reply ownership

The existing [admitted reply decoder](../../crates/quanta-index-sdk/src/native_uds/admitted_reply_v1.rs)
and [admitted read methods](../../crates/quanta-index-sdk/src/native_uds/read_session/admitted.rs)
serve Open/Profile/Text/Page/Hybrid/Release. They borrow the same canonical
request through request_publication_admitted_ref and return the existing
AdmittedNativePublicationReply owner; no second response IR/decoder is created.
Raw and decoded DATA grants remain live through validation and scoped emission.
Canonical Error responses remain charged values rather than cloned uncharged
errors. Local transport/protocol failures retain NativeUdsError semantics.

Allocation-free CBOR preflight accounts for bounded dynamic fields before typed
decode; malformed, oversize, truncated, wrong-mode and trailing input refuses.
Parser/regex and profile/owner/RRF validator scratch is admitted before allocation,
using the same canonical validators as ordinary reads. Scratch DATA is destroyed
before its grant; empty scratch does not request an invalid zero-byte lease.
Profile uses borrowed-spec indices, Text/Page typed owner indices and Hybrid its
bounded canonical union. Identity/page hashes stream canonical representation.
Existing request cancellation/deadline/error priority and no-send refusal remain.

Open ACK and profile loading are separate: a later profile/validation refusal
cannot discard the acknowledged session or invent another open key/pin. Copy ACK
fields may be retained while full DATA borrows require the reply owner.
The low-level MemoryScope fixtures prove SDK custody, not genuine upstream
Original authority; the actual facade supplies the original grants and exact
configured InstanceSpec binding. See the [facade](../../crates/quanta-index-sdk/src/client/native_query.rs).

## Published operation metadata

The [ordinary/admitted metadata methods](../../crates/quanta-index-sdk/src/native_uds/published_view_metadata.rs)
read the original operation's CatalogView without pinning a retired view or
reviving object access/CAS authority. The same canonical decoder and original
raw/decoded custody apply. Validate full operation, workspace, epoch and canonical
view digest; cancellation, funding, backend and corruption failures retain their
typed cause. Runtime Query/Control admission delegates through the existing
Kernel/Catalog path. Metadata remains in the original operation commit; length
observation and decode admission use the same SQLite snapshot. There is no
second journal, Legacy fallback or synthetic read ticket.

## Native caller disposition

Ordinary SDK/CLI Native selection and the existing retained-session facade are
implemented. The facade consumes the captured view/profile and full configured
instance, not an arbitrary first index. Native file-ranked Text/Hybrid, vector
Semantic, RepoMap adjacency/subjects and History retain their actual result and
authority contracts. A file hit is not a Legacy span/candidate, and a Native view
or cursor is not a Legacy generation/pin/continuation token. Projection owners
cannot substitute for authenticated source-file owners.

Legacy-only span/seed/cluster/symbol/runtime/structural and other unavailable
contracts fail explicitly through [typed unsupported capability](../../crates/quanta-index-sdk/src/unsupported.rs).
No zero-hit success, fabricated IDs, Legacy retry or fake empty lexical query
supplies missing semantics. Native History has its own implemented route above;
the old C40 caller table predates that route and the Native default constructors.
Actual Semantica/API/MCP, different-UID/default installed callers and maintained
product semantics still require their own original acceptance through
[NC-F](../plans/nov-12-native-cutover/tickets/NC-F-integration.md),
[NC-D](../plans/nov-12-native-cutover/tickets/NC-D-boot.md) and
[NC-H](../plans/nov-12-native-cutover/tickets/NC-H-history.md).

### Retained cold caller and typed failure

The cold candidate restores graph and Java provider state from the same
committed FULL, authenticated seed and retained Java build inventory. It
checks the Source and pin before publishing the graph. Cold preseed and
ordinary cold install reuse the Java owner preparation and retain typed
Bootstrap, Product and Workspace causes under the existing Original lease.
Nested Java errors lend their actual cause through `Error::source`. The final
alias drop releases the charge. Caller3 and signed candidate execution
remain tracked in F and D.
[Caller proof](../plans/nov-12-native-cutover/tickets/NC-F-integration.md#p07-final-actions-20261115)

## Closed Query supplier scope

NC-C completed its bounded Query supplier implementation and C85 interface
closeout: HistoryPage inventory, selected Contract/SDK API and
Contract/Core/Kernel module outputs. Its active ticket is retired; exact
source/commands, prior failures, C01-C85 history and source-scoped results are
preserved in the [recovery entry](../ARCHIVE-INDEX.md#2026-11-16-query-supplier-ticket-retirement).
Do not reissue its completed wire/reader/validator/CLI/metadata/History fixture
implementation or completed baseline adoption as a pending C task.

C83's fixed-input SDK/UDS pass predates F's later bound-peer request routing;
API/module closure does not requalify that runtime change. C85 selected interface
checks are not the full guard set. Broader wire-artifact findings, SDK-wide strict
lint, actual Original/producer/caller migration, installation, SDK60, independent
quality and engine removal retain their existing acceptance and owners.

## Native search design basis

- Immutable HNSW regions use bounded exact evaluation for eligible vectors when
  the remaining budget can cover them. Compare Recall and total query and merge
  cost across multiple regions, sparse and correlated filters, and accumulated
  deletes. Consider ACORN if measured filter traversal fails, or DiskANN if
  resident memory or SSD I/O fails its gate. [HNSW](https://arxiv.org/abs/1603.09320) ·
  [Lucene kNN](https://lucene.apache.org/core/9_8_0/core/org/apache/lucene/search/KnnFloatVectorQuery.html) ·
  [Qdrant segment optimization](https://qdrant.tech/documentation/ops-optimization/optimizer/) ·
  [ACORN](https://arxiv.org/abs/2403.04871) ·
  [DiskANN](https://www.microsoft.com/en-us/research/publication/diskann-fast-accurate-billion-point-nearest-neighbor-search-on-a-single-node/)
- Use RRF with k=60 on the same view's lexical and dense file candidates.
  Compare score fusion at the same candidate depth and budget after reproducing
  rank loss within the candidate union on DEV. Freeze the selected policy before
  independent EVAL. [RRF](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf) ·
  [Score fusion](https://arxiv.org/abs/2210.11934) ·
  [Vespa phased ranking](https://docs.vespa.ai/en/ranking/phased-ranking.html)
- Persist sealed files before Catalog commits head, receipt and retention.
  Measure compaction and checkpoints separately. The
  [custody ADR](NOV-07-001-native-index-execution-and-custody.md#recovery-receiver-and-selection-custody)
  owns the restart, lost ACK, old pin and reclaim conditions.
  [Lucene commit and retention](https://lucene.apache.org/core/10_4_0/core/org/apache/lucene/index/IndexWriter.html) ·
  [SQLite WAL](https://www.sqlite.org/wal.html)

### Native DSL diagnostics

The Native warm and cold runners list the same Text and History scenarios.
Each Text sample checks Exact Published view, complete count and independent
file oracle. Cold samples take the first call in a new process. History
reopens the same verified fixture and binds the daemon, configuration, view
and cut. The comparison tool checks the input digests and writes a complete
matrix after every declared scenario succeeds. The original Linux
`dsl-authority` suite retains its separate acceptance.
[Native DSL runner](../../tools/benchmark/run_dsl_cold_matrix.py) ·
[Remaining DSL qualification](../plans/nov-12-native-cutover/tickets/NC-F-integration.md#이-채팅의-잔여-코드-범위--2026-11-15)

## Remaining acceptance

NC-A's lexical implementation and owning hardening are closed; the
[recovery entry](../ARCHIVE-INDEX.md#2026-11-12-completed-lexical-and-runner-packets)
preserves its source-bound proof and original ticket. Later consumer changes
remain subject to their own affected-scope validation.

Source-scoped tests and fixed-cohort products are recorded in the existing
[NC-B](../plans/nov-12-native-cutover/tickets/NC-B-semantic.md) and
[NC-F](../plans/nov-12-native-cutover/tickets/NC-F-integration.md) packets.
Current-source full ANN repeat, filtered/churn quality, SDK60, original XL/Mixed,
independent holdout, actual non-operator/default callers, installed producer
fencing/rollback and shipped engine removal require their own evidence under
the [Nov-12 DoD](../plans/nov-12-native-cutover/DOD.md).
Those active packets retain their existing writers. This consolidation does
not retire their execution or weaken the original acceptance denominator.
