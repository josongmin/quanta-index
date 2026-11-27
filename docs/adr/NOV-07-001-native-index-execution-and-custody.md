# Native Index Execution and Custody

Status: `Accepted`

Decided: 2026-11-07

Implemented-contract consolidation updated: 2026-11-15.

Native ANN source routing updated: [NOV-12-001](NOV-12-001-native-query-and-semantic-engine.md)
records the current graph consumer. Decision 11's Lance refill source is now
test-only and remains an earlier-adapter reference; its original admission and
custody requirements are retained.

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
   client enforces its own read deadline. An empty accept queue waits for
   listener `IN` readiness using the existing shutdown-poll timeout; timeout or
   EINTR returns to the loop and invalid readiness is an I/O failure. Preserve
   peer/admission/connection ownership while removing the fixed idle sleep.
   Preserve nested typed decode and
   validation causes across the owning error chain. See the
   [peer watcher](../../crates/quanta-index-ipc/src/server/peer_watch.rs),
   [response writer](../../crates/quanta-index-ipc/src/server.rs) and
   [admitted decoder](../../crates/quanta-index-ipc/src/admitted_decode.rs).
5. **Configured embedding.** The macOS SDK uses the configured owned Voyage
   Host path. The separate borrowed entrypoint can refuse nonempty input before
   transport. Use the existing selected provider, admission and typed refusal
   path without copying an Ordinary result into Native. Live HTTP/TLS, partial
   backing and refund, and remote SDK acceptance belong to
   [E02 Provider contract](../plans/oct-22-finalizing/E02-publication-recovery.md#configured-native-provider-status-20261030-c)
   and [current NC-F acceptance](../plans/nov-12-native-cutover/tickets/NC-F-integration.md#p07-final-actions-20261115).
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
   [pinned rusqlite interface](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/vendor/rusqlite-0.40.2/src/pager_cache_status.rs)
   and the [scratch connection](../../crates/quanta-index-scratch-sqlite/src/connection.rs).
8. **Search-corpus publication proof.** The dispatcher retains the immutable
   request in its publication owner. The same batch can reuse its checked
   source-event payload and surface validation through preflight, build and
   Pending recovery. Footprint reuse binds the resource policy and embedding
   dimension. The canonical wire digest still binds the journal, and direct
   inputs retain their own checks. See the
   [publication owner](../../crates/quanta-index-core/src/publication_validation.rs)
   and [Pending recovery](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/adapter_lifecycle.rs).
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
    returns the retained session state and pin. A different selection under
    a retained key conflicts. Confirmed releases free live slots and retain
    replay records for at least the configured terminal TTL while the daemon
    is running. If the funded registry fills before records expire, new
    Opens are refused and existing records remain replayable. Sweep and drain
    share terminal-state handling. Only successful public Release can issue
    a replayable Release ACK. The retained Text path shares
    profile completion with first-stage lease validation before reader
    resolution and reservation. Later stages and the Registry's entry and
    final validation keep their original checks. See the
    [registry](../../crates/quanta-index-searchd-runtime/src/native_read_sessions.rs)
    and [Kernel stage](../../crates/quanta-index-kernel/src/update.rs).
    The CLI resolves a continuation through the selected Catalog root.
    A cursor names an exact view. Catalog authorization requires the actual
    selected historical repo/revision binding and a currently protected
    matching repo, workspace and epoch. Fresh CLI processes must also satisfy
    the exact-history floor. Existing leases keep their original read
    authority. Bindings are recorded atomically with route selection and retire
    with the view. See [Catalog selection](../../crates/quanta-index-catalog/src/native/authority/boot_manifest.rs)
    and [CLI continuation](../../crates/quanta-index-searchctl/src/native/code_search.rs).
13. **Sidecar physical custody.** Bind authenticates private parent type,
    ownership, permissions and device before starting the reserved ticket.
    Staging and target parents must share a device. Held directory FDs carry
    create, rename, cleanup and settlement through pathname replacement.
    Current parent identity and policy are checked at the physical boundaries.
    A rejected unstarted bind returns its original ticket. A published inode
    retains its charge when subsequent lease transfer fails, while writer pins
    and namespace claims are released. Pending rename and delete settlement
    remains with the original typed ticket. See the
    [Sidecar permit](../../crates/quanta-index-searchd-runtime/src/publication_sidecar.rs)
    and [physical binder](../../crates/quanta-index-searchd-runtime/src/publication_file_capacity.rs).

## Native foundation and selected storage

The delivered layout uses canonical Core index ports, Kernel coordination,
an injected Native Catalog and the existing shared Storage crate. Catalog is the
sole publication, reachability, ticket and reclamation authority. Storage owns
immutable candidates, bounded persistent maps and scratch; it has no independent
head/journal/GC policy and no Lexical dependency. Adapter payload formats remain
adapter-owned. Do not recreate Kernel/Storage from the initial RFC scaffold or
introduce a parallel internal IR while extracting reusable algorithms.

This fixes source ownership, not compiler, product, proportionality or retirement
acceptance. Existing map/corruption/crash/old-reader oracles, Original work/memory
admission and the original product DoD remain required. See
[Core](../../crates/quanta-index-core/src/index/mod.rs),
[Kernel](../../crates/quanta-index-kernel/src/lib.rs),
[Catalog](../../crates/quanta-index-catalog/src/native/authority.rs) and
[Storage boundary](../../crates/quanta-index-storage/Cargo.toml).

## Prepared searchable installation

The existing [Core preparation](../../crates/quanta-index-core/src/index/searchable.rs),
[Kernel coverage caller](../../crates/quanta-index-kernel/src/update.rs) and
[Semantic directory](../../crates/quanta-index-semantic/src/native/embedding/ann_directory.rs)
share one attempt-bound searchable child and coverage parent. Provisional refs
are canonical parent-encoding inputs; they grant no readable object, pin or
Catalog publication authority. Root/default/non-ANN scalar paths retain their
original wire, registered children and identity.

For eligible ordinary non-seal ANN updates, delta COW, descriptor and coverage
use one preparation scope. `ObjectMapBatchPort::prepare_one_map` owns the selected
backend, original base, resident COW stage, AttemptRef and allocation custody.
Consumption rechecks attempt, sink scope and coverage base before physical I/O.
The original child-first reserve → physical sync → confirm, returned ordinal/ref
validation and final Catalog CAS remain. The non-seal packet retains at most
2 opaque DATA objects and 2 short-key roots. Independent separate-sequence
oracles compare canonical bytes, identity, child closure, selected results,
logical usage/peak admission and actual reserve/confirm counts.
Counterexamples retain wrong attempt/base/return order, funding/confirm/partial
failure and reopen checks.

The same-attempt [owner scope](../../crates/quanta-index-core/src/index/ports.rs)
retains installed artifact frontiers, same-operation artifact reuse and
provisional metadata plus owners/missing original bases and old/current roots.
Its [ResidentStage](../../crates/quanta-index-storage/src/map/resident.rs)
lends canonical wire/map/selected-member/recipe/children reads only for that
scope's provisional refs. Seal/readback, projected reverse-map, retirement
unlink and other paths needing physical I/O flush before the first real write.
Prepared, transferred and installed remain distinct. Without an early flush,
owner completion retains at most 4 DATA objects and 4 independent roots. The
[physical walker](../../crates/quanta-index-storage/src/object.rs) keeps the
256-object/64 MiB split and verifies base and child→parent reachability before
durable reserve. Source/Projection/reconfigure callers consume the same identity
and ordered-return checks.

Failure, cancellation or funding refusal publishes no prepared prefix. Partial
physical installation remains charged to the original attempt's abort/reclaim
custody; token drop performs no I/O. Preserve original payload/wire grants,
deadline/cancel/error priority, physical base, old pins, delete/ABA fencing and
Catalog CAS. A selected backend error does not choose another engine. Fewer
install groups are measured on the actual source/branch; they establish neither
one COMMIT nor lower wall time nor current ANN4K/product acceptance.

Historical implementation results and unchanged product gates remain in
[E04](../plans/oct-22-finalizing/E04-semantic-ann.md#implemented-source-contract--reuse-no-new-ticket).

## Consolidated implementation boundaries

- **Logical identity across Text compaction.** Repacking may change the physical
  body root. Preserve logical ID, source, candidate ID, indexed SHA-256 and the
  committed body byte length/SHA-256. The writer verifies the old segment/local
  locator and decoded body; the independent transition proof requires unchanged
  logical identity and a replacement segment in the new segment range. A changed
  length/digest or an old-segment locator remains a refusal. See the
  [compaction writer](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/text_authority/paged_writer.rs)
  and [transition proof](https://github.com/josongmin/quanta-index/blob/d8a490565e6f9997c6a6cc0c87f92867ea7e9eb1/crates/quanta-index-lexical/src/text_authority/paged_transition.rs).
- **Cold alias handoff and packed Diff.** Reuse authenticates the persisted ordered
  key set, sealed anchor edit, same attempt and expected root/layout before the
  prepared batch transfers aliases. Refusal invalidates the enclosing batch so
  a prepared prefix cannot publish. The update path resolves packed descriptors
  through the bounded owner cursor/draft; public page scans retain their own
  paging contract. See [entry patch](../../crates/quanta-index-lexical/src/native/text/entry_patch.rs)
  and [prepared inventory](../../crates/quanta-index-lexical/src/native/text/prepared_inventory.rs).
- **Posting scratch values.** A call keeps borrowed MapValue bindings and interns
  repeated non-null values to positive IDs. Preserve canonical SQL shape, row
  work/reservations and NULL semantics. Invalid IDs, dictionary refusal and
  pre-execute failures cannot publish a Posting row. See
  [edit writer](../../crates/quanta-index-scratch-sqlite/src/edit_write.rs).
- **Posting preparation scratch.** Packed sorted runs replace per-edit scratch
  rows under the same Original work and memory admission. A direct heap merge
  can borrow optional memory credit from that same parent; unavailable credit
  retains the bounded hierarchical merge. Admission, cancellation and corrupt
  input still propagate as errors. Sensitive/folded term ranges keep independent
  seek boundaries and their existing order, authentication and EOF checks. The
  disposable spool changes no published format or product cap. See
  [packed runs](../../crates/quanta-index-lexical/src/native/text_spool/packed_runs.rs)
  and [case pages](../../crates/quanta-index-lexical/src/native/text_spool/posting_case_pages.rs).
- **Read-only recovery facts.** Original FULL, independently keyed current Native
  publication and selected BOOT are read under one existing Catalog snapshot.
  Equal operation keys reuse one lookup; different keys require both actual
  Published rows. A current DELTA cannot silently alias the historical FULL.
  Retained metadata custody is bounded; a missing/nonpublished/foreign row is
  refused. The separate verified Legacy terminal receipt remains a distinct
  typed fact. Neither loan supplies Source/physical authority or installed
  recovery acceptance. See
  [paired read-only authority](../../crates/quanta-index-catalog/src/native/authority/read_only.rs).
- **Installed baseline immutability.** Plain install cannot replace an installed
  Native BOOT baseline with a reissued receipt, even on the same root. Preserve
  operation, object, SourceCut, Catalog digest and nonce until an authorized
  transition verifies them. This launcher guard does not implement fresh-root
  production or normal rollback/restore-forward. See
  [selection publisher](../../tools/deployment/launcher.py).
- **Send observation.** The existing Native observer measures client request-frame
  and diagnostic-preface send attempts. It does not measure server terminal
  replies; those retain the raw UDS oracle. A refusal before sending adds no
  attempt. Assertions occur after releasing the observer lock so a rejected
  unarmed observation cannot poison later arm/count/disarm use. See
  [observer](../../crates/quanta-index-ipc/src/native_send_observer_v1.rs).

## Cold Text installation and packet custody

The sequential Directory cursor keeps the previous descriptor's admitted
map entry alive while borrowing its fence for the next bounded scan and child
authentication. The next descriptor replaces it only after validation.
Continuation, descriptor order, child identity and counts remain checked.
Random lookup validates its fetched record before comparing the expected key
and value.
[Directory cursor](../../crates/quanta-index-lexical/src/native/text/entry_map/directory.rs)

Cold posting install shares one Original-funded StagedObjectBatch. A draft
with no prior or reused leaf can coalesce its leaves and directory when the
backend supplies a bound and the same Original grants memory credit. The
object cap is 256, and the installer retains its 64 MiB wire bound. A pre-effect
admission miss takes the separate path. Once install starts, propagate failure
through the original Unknown/resume and reopen contract. Object confirmation
precedes the Catalog root CAS. Keep child order, ref/ordinal, EOF and old-root
checks, with the persistent Catalog write transaction outside file I/O.
[Install path](../../crates/quanta-index-lexical/src/native/text/dense_posting/write.rs)

The current terms map stores upper full-case-key fences to immutable
`QITXTD01` pages, each bounded by 16 KiB and the wire's 255 distinct children.
Greedy filling uses actual encoded bytes; the row ceiling is derived from the
seven-byte minimum valid row, not the unrelated 64-row write batch. Pages retain
the exact full keys, posting values and first-use physical child references. A
bounded deterministic child index verifies complete references after collisions;
hashing, probes, comparisons and slot writes retain their Work charges. Readers
allocate only the authenticated count's row offsets and reject duplicate child
slots through the same index. Cold and
warm writers use the same representation. Warm edits rewrite only the selected
old page, check each term's expected value, and stage changed fences; owner
edits join those fences in the existing atomic multi-root apply. A failed page
install poisons the disposable anchor stream before publication. The output
policy rejects both the previous per-term map and fixed-64-row policies; there
is no parallel reader.
Durable queries and overlay membership select authenticated upper-fence pages.
Overlay DF adjustment resolves one immutable term posting across its masked
owners while retaining each owner's inventory, source and occurrence checks.
[Term dictionary](../../crates/quanta-index-lexical/src/native/text/term_dictionary.rs) ·
[COW writer](../../crates/quanta-index-lexical/src/native/text/term_dictionary_write.rs)

Confirmed cold dense roots use the existing bounded direct cache followed by
an attempt-local scratch lookup. Only completely confirmed installations enter
the table. A SQL hit must match the newly derived canonical physical root
before it can replace a draft; the scratch row alone grants no authority.
Lookups, canonical reconstruction and actual installations retain their
original fees. These code decisions do not qualify the fixed-budget SDK until
the final binary executes the original inputs.

Frozen-source qualification keeps the wrapper's checkout-local artifact
namespace. A different checkout's target cannot be preserved as a cache
shortcut: matching source digests and binary dep-info do not authenticate
reused transitive metadata. Snapshot copies preserve content while renewing
freshness inputs, and the selected package/source/artifact origins are checked
before promotion. A current, separately pinned Nextest configuration owns the
test profile. Compilation, test execution and original SDK acceptance remain
distinct results. [Cargo path-cache failure mode](https://github.com/rust-lang/cargo/issues/12516)

The cold cursor retains an ephemeral 8-byte row in two words. It bounds key
offset, length and position before encoding and charges actual row backing.
An admitted Unicode upper bound permits one fill. The exact census and SQL
paths handle cases outside that admission. Preserve canonical normalization,
case keys and positions, packed occurrence wire and existing logical fees.
Cumulative Work bytes remain charged after the arena is released.
[Cold token builder](../../crates/quanta-index-lexical/src/native/text/cold_tokens.rs)

Prefix programs retain flat slab ownership and checked8-byte locators. A
producer-built minimum position for each256-row chunk skips only chunks whose
entire position range is outside the proved prefix; selected row count must
still equal the current cursor's prefix census. Merging the selected prefix
with a new suffix uses an exact exponential/binary boundary for consecutive
suffix keys below the current prefix key. Equal keys retain prefix-before-suffix
position order. Extra row/key probes, locator copies and chunk metadata work
remain charged. This can cost more on interleaved keys; the original1,421-file
component comparison is the adoption check, not a universal timing claim.

The bounded prefix cache retains the lower-bound search's exact neighboring
LCPs and insertion position. An exclusive cache borrow owns that lookup plan;
preparation owns the same immutable normalized source cursor until insertion
or rewind. LRU eviction adjusts the retained insertion position without a
second search. Key backing is funded before suffix mutation and copied once
only for a nonempty completed program. Optional admission failure keeps the
original arena and rewinds the cursor before any suffix mutation. The original
1,421-file component preserves token/position/wire outputs and mutation work
while removing 6,129,674B of duplicate scans; SDK acceptance remains separate.

The bounded cold-owner tournament borrows authenticated keys and establishes
full canonical key order, using owner rank to break ties. The current path
retains exact winner/loser LCPs and compares only an unproved suffix during
replacement. Successor LCP computation and retained metadata reads/writes remain
charged; this is not a full-key comparison followed by a billing discount.
Winner replacement repairs only its tournament path. The attempted 8-byte prefix cache was removed after
the original corpus increased measured work; it is not part of this decision.
The independent cursor seals, counts and EOF remain required.

The owner-head window and negotiated posting-writer width are separate bounds.
Local Original credit uses the actual64/128-row writer: its last backing growth
can retain96/192 rows, and long-key data and live-lease slots scale with that
width before any posting effect. These private construction changes preserve
the wire, logical tariffs and fixed SDK limits. Focused correctness and credit
regressions do not qualify original SDK or quiet Off acceptance; the
[Text packet](../plans/nov-12-native-cutover/tickets/NC-TEXT-build-final.md)
owns their current verification state.

A current-attempt canonical cold cursor and inline alias visitor derive counts
from the same independently fetched and fully parsed immutable block. They do
not issue an immediate second logical retained read of that block. The private
borrowed map-block stream keeps its checked DATA through EOF instead of renewing
it at artificial 64-row boundaries. Unsealed/Ready count admission and public
page continuation keep their retained-read contracts. Every requested retained
read still uses the original wire/decode tariff; the optimization removes whole
duplicate calls, not their charges. Initial get/parse, attempt binding,
key/count/descriptor seals, EOF, cancellation, poisoning and Original custody
remain necessary before owner completion or publication.

Cold Literal source seals factor the expected owner prefix once per block,
then front-code the virtual `(gram3, owner_suffix)` keys using borrowed slices.
The producer validates every source key against that prefix. Readback checks
the exact expected V4 header, decodes every key, verifies canonical order and
omissions, and reaches EOF before accepting the seal. Header and endpoint hints
alone cannot authenticate a middle key. The new cold hash domain and its
width/prefix input are charged; the warm transcript and persisted wire remain
unchanged. This removes repeated hash input without removing validation.

The posting packet authenticates local order and its fences and EOF. The range
reader saves a charged tail key at packet turnover and compares it with the
next packet's first key. A refusal keeps the current packet and boundary
available for retry. Existing epoch, seek, logical admission and owned output
checks apply. Qualify any later representation by net build and read cost at
the same logged workload before adopting it.
Cold PostingCasePages retains up to two checked packets across term windows,
one per posting surface. Reuse checks the sealed source identity, accepted
connection epoch, phase and page. Optional Original credit funds a parked
payload. Successful range completion parks the packet, and a missing credit
uses the normal lookup path. The row cursor advances after the consumer admits
the requested output, so a failed edit allocation retries the same row.
[Packed posting cursor](../../crates/quanta-index-lexical/src/native/text_spool/packed_runs.rs) ·
[SDK60 acceptance and first refusals](../plans/nov-12-native-cutover/tickets/NC-TEXT-build-final.md#현재-원-sdk-진단과-다음-수리-경계)

Cold phase-0 posting runs keep the ordered `QITXPR02` wire prefix under the
existing Original grant. On the first ordering inversion, the writer stages
the suffix in bounded term chains. It compares full term keys on fingerprint
matches, then merges the sorted suffix with the retained prefix by term and
owner at flush. Packet limits, admission and duplicate checks apply across
that boundary. Cold phase-0 runs after run zero may use transient
`QITXPR03` when a term repeats. The checked merge reader resolves a
two-byte distance only to the current term literal. Run zero and final
seekable packets retain `QITXPR02`. The shorter scratch payload goes through
the same SQL admission, insert and merge fees. The original-corpus and restart
results stay in the Text acceptance ledger.
[Cold run writer](../../crates/quanta-index-lexical/src/native/text_spool/packed_runs/cold_run.rs)

Storage's [physical install](../../crates/quanta-index-storage/src/object.rs)
checks local child references on every edge. It can reuse a successful
external child path check for the exact full reference within one physical
group under a held install gate and an admitted bounded table. A miss checks
the path under the original Work budget. Each group starts its own table.
Catalog confirmation has separate transaction-local reference and lifecycle
checks. Managed reserve and confirm bind namespace authentication to their
own locked transaction. A connection row-change watermark invalidates the
binding after a write, including a trigger write. The next adoption
reauthenticates and charges that actual read. Edge, lifecycle and queue checks
remain in the managed install path.
[Namespace proof](../../crates/quanta-index-catalog/src/native/authority/reclaim.rs)

The funded cross-group observer counts planned overlap and repeated
path checks actually reached before refusal. Its record never serves as a
path-validation witness. [Storage observer](../../crates/quanta-index-storage/src/object/physical_observation.rs)

### Sequential cold alias scan and SQL replay

The cold alias reader can continue a Storage-owned forward scan across
admitted one-row descriptor pages. It retains the DFS under a discardable
Original grant. If a mandatory child allocation needs the grant, the reader
releases the DFS and resumes through the scalar scan after the last delivered
fence. Capacity left by a deeper branch also selects that scalar tail when
it exceeds the fresh next path's backing. The decoded child is released
before Storage reads the next page. EOF, interval, child identity and
disposable partial effects keep their existing checks.
[Storage continuation](../../crates/quanta-index-storage/src/map.rs) ·
[Directory visitor](../../crates/quanta-index-lexical/src/native/text/entry_map/directory.rs)

On direct packed output grant refusal, the validated cold token arena lends
sorted SQL rows in bounded batches. The fixed statement and borrowed rows
use the original SQL admission, fees, bindings, row-count check and terminal
failure path. The arena supplies those rows without another tokenization
pass. The original fixed operation budgets still govern acceptance.
[SQL replay](../../crates/quanta-index-lexical/src/native/text/occurrence_pack.rs)

## Native lexical replay and cold-build evidence

A failure after Dense confirmation can return `Unknown` while the immutable
Source operation remains Pending. That Pending row retains its event claim.
A new operation for the same event conflicts. After reopening the same root,
`resume(op2)` runs the original immutable request under a new fence. Late
work carrying the former fence is refused. [Recovery dispatch](../../crates/quanta-index-kernel/src/update.rs) ·
[Scoped Runtime3 results](../plans/nov-12-native-cutover/tickets/NC-TEXT-build-final.md#ntx1a-runtime3-실제-green)

Nextest `8ee85ee8-371d-405e-98ff-aef809179e83` verified 3/3 in its frozen
NoFeatures Runtime scope. G1 compared the whole-window and fallback install
with the same DF/TF, positions, artifact and reopen. F2 kept the prior root
through `Unknown`, resumed the original operation, and reopened the published
nine-owner membership. A1 verified fallback after optional admission missed
an 8 MiB credit. The separate descriptor candidate stopped at ACK zero on
both original corpora. Caddy first exceeded the one-million reads limit and
etcd first exceeded the 512 MiB work-bytes limit. [Original corpus results](../plans/nov-12-native-cutover/tickets/NC-TEXT-build-final.md#descriptor-후보-원-sdk-실제-실패와-다음-경계) ·
[Original run history](../ARCHIVE-INDEX.md#2026-11-23-native-supplier-and-runtime-history)

The frozen compact-cursor candidate on dirty HEAD
`ade5fadc1a084fbb81eba831861e996081d80867` passed Lexical410 and Runtime15
in nextest `c2c3d5e8-b512-4a59-a982-ca9036ed978f`. The run covered fixed
129/300-term boundaries, Original funding, membership and positions, Dense
cross-page reuse, and same-operation resume and reopen after post-confirm
failure. F integration passed 10/10 across four binaries in
`16c936f8-7f9e-4b90-9762-4769cc84ee3a`. The no-default release built and
the locally rehashed daemon and helper matched the candidate's source binding.
Separate SDK Record runs refused both original corpora with `ResourceLimited`
at ACK zero, before query or reopen. Quiet Off stopped in preflight with zero
test bodies. Product acceptance still required the original resource limits and
observed Work reduction with the existing logical-edit charges.
[Execution preimages](../ARCHIVE-INDEX.md#2026-11-23-native-cohort-and-proof-history)

## Native Source commitment and selected reads

- **Facts reads.** Existing-file checks, disk-size accounting and disk-image
  preparation borrow the selected path through the Original Source owner.
  Inherited members follow the predecessor Current and retain the path,
  content-digest and language checks.
- **Durable DELTA commitment.** Facts and disk-attribution DELTA with a prepared C1
  candidate share the target issuer. Store admission binds the ordered changes
  and roots before Source manifest issuance. The committed pointer and E02 replay bind
  the transition to the exact predecessor, target and publication journal.
  The Source manifest records the transition commitment. The full Store target
  seal is issued against the completed Store manifest, whose identity depends
  on that transition. A prepared DELTA that loses its required C1 candidate
  is refused before Source CAS.
- **Committed selected reads.** Store V4 checks the same Store, repository,
  generation and Original Current before opening selected C1 paths and their
  bodies. SDK rollback uses that loan under the committed HEAD fence. A
  mounted Query C1 provider authenticates selected paths against the bound
  Source and roots. Its absence and refusal remain authoritative. Source
  subset admission pays for shape and the selected membership proof. Workspace
  digest lookup retains FileText precedence, and C1-unbound readers retain
  the legacy lookup contract. Original CFG and Callsite SSOT retain the whole
  Source in their keys after member admission. SemanticModule keeps its
  workspace key when it joins a subset from the same Source. Original
  SemanticModule without a subset retains the whole Source key. File-member
  HIR retains the content-addressed member key.
- **Workspace birth.** Runtime reserves the new workspace path list using the
  member count from the retained Original Source. Filling the list checks the
  count, path order and byte-count overflow under Original Current. Before
  handing off the group, Runtime checks the output count and reenters the
  retained Source and predecessor Currents. An externally supplied workspace
  is still compared path by path against Source.
- **Selected ItemIndex paths.** Small DELTAs can check the affected paths
  against the retained target census. The selected owner loader authenticates
  the requested provenance and membership. The sparse finish checks the
  remaining affected paths against the validated seed and target counts.
- **V1 compatibility.** The Original pin computes the existing Source and
  membership digests in one visit to the authenticated headers and preserves
  the frontier digest. Older readers retain the canonical V1 digest and
  membership formats.

[INC-01](../bugbash/nov-20-native-delta-cost/tickets/INC-01-source-closure.md#소유-회귀-영수증)
owns exact Source, Kernel and Wire receipts and the open V1 projection,
Store warm and ItemIndex work. [INC-03](../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md)
owns installed FULL parity, publication recovery and warm cost acceptance.

## Native Literal changed-directory publication

INC-02 closed the packed Literal implementation and its focused cost and
correctness regressions. A warm Patch seeks affected posting blocks with
`lower_bound_page` and merges sorted edits per block. An append past the maximum
key reads only the rightmost descriptor through `last_entry`. Moving an upper
fence removes the old key before inserting the new one. Duplicate posting adds,
missing deletes and overlapping block boundaries are refused. A custom Map port
must provide `last_entry` for this append path. The default port rejects it.
[Block lookup](../../crates/quanta-index-lexical/src/native/literal_blocks.rs) ·
[Map contract](../../crates/quanta-index-core/src/index/ports.rs#L542)

The candidate owns the funded, sorted edit witness through seal. Seal reads
every page of `visit_diff` and matches the exact key and before/after references
against that witness. Only changed children are re-read after installation. A
failed diff page leaves the prior view searchable and prevents publication.
Bootstrap and first format conversion use full verification. An empty Patch
reuses the prior artifact, and a posting-identical Patch can reuse its directory.
The persistent format remains compatible. [Literal writer and seal](../../crates/quanta-index-lexical/src/native/literal_packed.rs) ·
[Candidate and modes](../../crates/quanta-index-lexical/src/native/literal.rs)

The focused oracle derives `(3-byte gram, owner)` membership from Source bytes.
It compares both stored generations and their pinned searches, and bounds the
seek work for a one-file edit in directories with more than 64 descriptors.
Other cases cover empty-to-nonempty and reverse transitions, edge edits, repeated
tail appends against one batch, cancellation, and success or rejection on the
second diff page. The original integration run `547734ff-6354-4f1a-ac34-227e98aba8ec`
had 9 passes and two fixture failures before reaching the page boundary. After
the fixture correction, both cases passed in `1d3a5bb4-06c1-419e-9a65-f37e94ae1404`
and again in `e09ac454-c3fa-4327-9c32-ac33d96fddc9` on Q `70d17e3cb` with its
recorded worktree. [Work and postings oracle](../../crates/quanta-index-kernel/tests/native_lifecycle.rs#L8081) ·
[Seal cases](../../crates/quanta-index-kernel/tests/native_bootstrap/literal_seal.rs#L938) ·
[Rightmost seek regression](../../crates/quanta-index-storage/src/map/tests/structural_diff.rs#L147)

Installed SDK and Watch acceptance stays with [INC-03](../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md).
E07 owns [final performance acceptance](../plans/oct-22-finalizing/E07-performance-operations.md#dod).
The original ticket and exact run history are recoverable from the
[archive index](../ARCHIVE-INDEX.md#2026-11-23-completed-native-tickets).

## Inventory Diff observation

A `NativeTextInventoryDiff` lane-2 span records one six-counter family. Zero is
an observed value; an absent family is unobserved. These are logical validation
calls and reuse decisions, not syscalls, physical bytes or global unique refs.

| Counter / wire key | Meaning |
| --- | --- |
| `TextOccurrencePriorValidationAttempts` / `text_occurrence_prior_validation_attempts` | Prior full-payload count validation calls, including refusal/failure. |
| `TextOccurrenceNextValidationAttempts` / `text_occurrence_next_validation_attempts` | Next full-payload validation calls not satisfied by RAM or confirmed spill. |
| `TextOccurrenceRamReuses` / `text_occurrence_ram_reuses` | A successful count reused from verified RAM in the same attempt. |
| `TextOccurrenceSpillReuses` / `text_occurrence_spill_reuses` | A verified count reused from confirmed spill in the same attempt. |
| `TextOccurrenceEqualReuses` / `text_occurrence_equal_reuses` | Equal prior/next identity reuses the checked prior count. |
| `TextOccurrenceValidationFailures` / `text_occurrence_validation_failures` | Failed prior/next full-payload validation calls. |

Attempt, term or SQL refusals before payload validation remain Diff failures,
not payload-validation attempts. Failures cannot exceed prior plus next attempts;
failed results cannot be reused. Off takes the ordinary Diff path without new
clock/set/SQL/emission work. Record uses bounded stack totals and emits on success
or error before span completion. Overflow invalidates diagnostic evidence through
`CounterOverflow` while preserving the product's original result.

The V5 consumer checks the owner stage/lane, complete and nonduplicated family,
exact integer bounds and counters before span end, including equal timestamps.
Missing older families are never zero-filled. Attribute failure-prefix counts
to their request ancestry; do not promote them to full ACK or whole-request
work. Keep logical gets, new physical writes, SQLite requested bytes and nested
CPU/wall separate. Do not add unbounded sets or per-reference SQL for uniqueness.
Independent examples cover first validation, RAM/spill/equal reuse, prior and
failed calls. See the [producer](../../crates/quanta-index-lexical/src/native/text.rs),
[counter contract](../../crates/quanta-index-core/src/diagnostic_trace.rs),
[consumer](../../tools/benchmark/retrieval/causal_cost_timeline.py) and
[fixed-oracle regressions](../../crates/quanta-index-lexical/src/native/text/tests/occurrence_validation_trace_tests.rs).

## Candidate preparation and selection durability

The launcher binds a separate candidate directory, the expected Legacy
selection, the Q root/config/image and the pinned S producer to one retained
FULL handoff. It reaps the candidate Q process and persists the matching
clean-drain record before sealing the candidate BOOT. An exact retry with a
retained FULL handoff and an absent drain record reopens Q for drain without
running FULL again. BOOT requires the drain record and the candidate's actual
current view. See [launcher](../../tools/deployment/launcher.py) and
[operator procedure](../operator/state-cutover-runbook.md#installed-offline-recovery).

Selection publication becomes Durable after rename, directory sync, readback
and final file/image custody checks. A failure after rename leaves an uncertain
outcome for exact reconciliation. Same-snapshot Legacy-to-fresh-Native Q fixtures
cover the local receiver. Installed producer recovery and replay coverage for
a different SDK snapshot or a prior Native root stay with the cutover tickets.
See [selection publisher](../../crates/quanta-index-searchd-runtime/src/deployment_selection.rs)
and [NC-D](../plans/nov-12-native-cutover/tickets/NC-D-boot.md#fresh-native-candidate-preparation--2026-11-14).

## Recovery receiver and selection custody

The existing [Native receiver](../../crates/quanta-index-searchd-runtime/src/native_recovery.rs)
and [selection publisher](../../crates/quanta-index-searchd-runtime/src/deployment_selection.rs)
form the Q component boundary; old intermediate check_rollback-stub and
unmounted-helper descriptions do not describe current declarations.
Catalog's [guarded rollback check](../../crates/quanta-index-catalog/src/native/authority/read_only.rs)
uses the immutable installed baseline, actual current head and quiescent rows.
Historical FULL Source and later installed AST/BOOT remain independent facts;
a reissued BOOT cannot reset the rollback boundary. Legitimate pin/GC/WAL changes
do not equate to a new logical publication or authorize a source baseline.

Producer closure precedes deployment locking, actual Q/root verification and
canonical selection publication. Inherited producer FD/OFD custody, root/physical
read ownership, original monotonic deadline/cancellation and Work/Memory admission
remain held through rename, file/directory sync and Q postchecks. No explicit
unlock may release another owner's shared OFD; Linux's Source artifact/peer image
must have real immutable/kernel custody, not operator path/hash assertions.
Source/outbox high-water, pending/RecoveryRequired and signed ACK/cut associations
come from their original issuer; a target scan, nonterminal-empty shortcut or
successful arbitrary callback cannot supply that authority.

The sole publisher owns canonical codec/environment/CAS, original before/after
selection identity, durability and readback. The control outcome distinguishes
Applied, RefusedBeforeRename and PersistenceUncertain; child exit is not success.
Reconciliation retains the first before document and operation identity instead
of rebasing on a possibly published selection. The raw bytes are checked against
their journal SHA and the decoded schema-2 identity against expected-current.
Reconciliation repeats the domain and file checks and syncs without a second
rename. A failure after publication cannot
be reported as no rename or retried with a fresh identity. Python consumes the
resident/helper outcome and never publishes the same transition again.

Native V5 recovery carries the actual RepoMap Required/NotApplicable disposition,
including the existing receipt/locator pair, through the original aggregate loan.
Native FULL/DELTA/first-DELTA replay and CLI copying reuse that same validated
borrow; callback data is provisional until aggregate/birth completion. Q's
NativeCutoverEvidenceV5ArtifactV1.repomap and existing selected physical verifier
consume it without minting a new Source authority. Configuration absence is not
NotApplicable. The [History checkpoint handoff](NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover)
uses its own authenticated Legacy inventory and applied ACK. When an Applied or
Reconciled terminal already exists, restore-forward requires the new History
witness to match both presence and exact value. A first success can supply the
verified witness when no completed terminal is recorded. S performs the same
comparison before retaining a completion or consuming a fresh result.
[Restore-forward witness check](../../crates/quanta-index-searchd-runtime/src/native_boot.rs)

For Native-to-Native restore-forward, FD6 format 5 carries producer-verified
completed ancestry. The [receiver](../../crates/quanta-index-searchd-runtime/src/native_boot/restore_forward.rs)
matches the previous signed Source and operation against the held old root and
the current signed Source and FULL operation against the candidate before
selection publication. Ordinary rollback consumes the current V5 and original
Legacy authority. The exact installed acceptance remains in D.

These are mounted Q component contracts and source-scoped receiver/launcher
results, not current D1–D3 integration proof. The [active D packet](../plans/nov-12-native-cutover/tickets/NC-D-boot.md#current-work-packets-20261115)
keeps source-only intake, original Source/Host/SDK joins, signed normal and
postpublication/ACK-loss/restart acceptance separate. Genuine Linux producer
custody, loss0/duplicate0/head0 and the original P11 gates still require their
own actual nonzero-body evidence; local refusal and component PASS do not close them.

### Initial Source and completed Watch admission

The ordinary initial Source association comes from the producer's persisted
transition, CompletionPointer, physical target Source and snapshot. It is
retained through V9 and the signed reader before FD6 borrows it. The
[canonical join](../../crates/quanta-index-contract/src/native_cutover_source_join_v1.rs)
and [Q receiver](../../crates/quanta-index-searchd-runtime/src/native_recovery/facts.rs)
check all four fresh Source axes. This initial path does not manufacture a
pending SDK ACK or derive historical bindings from FreshSdk. Native initial
FULL retains its own immutable Source association.

Completed Watch inputs use the existing bounded content-addressed intent and
closeout readers plus Original-admitted typed decoding. Retain decoded DATA,
backing and typed failure through reader/Source close, then join the actual
operation, verified batch/event IDs and settled Native V9. Host retained units
are occupancy, not inflight count. Signed event membership, terminal/cursor
and disposition checks remain required; absence of a GC'd Host row is not a
missing Source completion. Live Host echo or a recovery label grants no
settlement authority. Prior-Native ancestry is checked against the held old
root and candidate before selection publication, as above.

The [active D packet](../plans/nov-12-native-cutover/tickets/NC-D-boot.md#d-current-disposition-20261116)
retains affected S/Q owning, CP2 mixed and signed Linux acceptance. These
supplied paths retire the old API-missing dispatch, not those execution gates.

## Existing-only Semantic recovery verification

[Semantic's existing-only verifier](../../crates/quanta-index-semantic/src/legacy/existing.rs)
reuses canonical marker/seal/scope/contract decoding, Core's admitted layout
verification and existing-file ports/streaming hasher. The caller supplies the
held physical/root authority, selected GenerationSnapshot, expected owner and
same WorkBudget. No writable engine open, live provider, second parser/hash,
Source issuer or publication authority is created.

Scratch is admitted before sidecar decode. Validate track, supplied generation
scope/identity, normalization/corpus and build contract in their existing error
order; canonical scope validation runs once. Bounded sidecars may use their
existing whole-file envelope; dataset/vector/artifact content streams through
ObjectReadIoPort and the shared canonical hash with admitted backing, per-read
work/cancellation, EOF/identity/close checks. Missing/extra content, changed
sidecars, late quarantine and original port refusal remain errors.

Both admitted tree observations span physical content verification. The verifier
lends SemanticContentRootsV1 to a callback while original input/decode/layout DATA
grants remain live, then checkpoints the same budget before releasing them.
A caller must separately fund any owned output/clone; borrowed DATA grants no
Source or read/write permission. Actual
[Runtime verify_legacy_publication_v1](../../crates/quanta-index-searchd-runtime/src/native_boot.rs)
compares these roots with selected activation semantic_content, rechecks the
activation and retains the original root/read-retirement boundary.

B's physical/CBOR/hash and pure-recipe focused receipts are source-scoped history
in the [recovery entry](../ARCHIVE-INDEX.md#2026-11-16-semantic-ticket-history-compaction).
The complete wrapper fixtures are finite ports, not genuine production Linux
read custody, authentic Legacy Source/ACK or installed recovery proof. Those
acceptance scopes remain in [NC-D](../plans/nov-12-native-cutover/tickets/NC-D-boot.md#current-work-packets-20261115)
and [NC-F](../plans/nov-12-native-cutover/tickets/NC-F-integration.md).
Future engine removal must preserve this mounted verifier, exports, canonical
types/admission and tests; obsolete B60/B81 whole patches are not current source.

## Acceptance

The [P01 owning run](../plans/oct-22-finalizing/VALIDATION.md#e02-deduplicated-current-work-20261030-root)
closed the fixed-cohort Acked regression (76/76). Physical SQL-worker, SDK,
independent-process and Product acceptance remain in the
[E02 ledger](../plans/oct-22-finalizing/README.md#e02). Current Native performance,
including the original Large deadline and mixed-query bounds, is tracked in the
[Native bugbash](../plans/nov-5-indexing-bugbash/README.md).
The completed publication-count and Sidecar acceptance scopes are recorded in
the [Nov-12 recovery entry](../ARCHIVE-INDEX.md#2026-11-12-publication-and-sidecar-closeout).

The retired P09 ticket's local observer and SDK results remain source-scoped
history in the [Nov-13 recovery entry](../ARCHIVE-INDEX.md#2026-11-13-ownership-and-ticket-consolidation).
Its unmet installed DELTA observer acceptance is owned only by
[B05](../plans/nov-12-bugbash-c0-p09/tickets/B05-final-verification.md#p09-observer-acceptance).
Packed runs, aliases and Posting normalization make no new XL/Mixed timing,
RSS or full-product claim.
