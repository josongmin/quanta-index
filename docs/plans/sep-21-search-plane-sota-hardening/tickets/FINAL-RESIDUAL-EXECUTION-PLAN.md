# SEP-21 final residual execution plan

Status: planned; this document is not a proof receipt or implementation result.
Scope: current-source residual work after prior P03–P10 code checkpoints;
historical handoff completeness is separately unproven.
The exact HEAD, dirty digest, paired checkout, binary, commands, and verdicts
belong to fresh handoffs and proof manifests, never to this tracked plan.

Original planning baseline: Quanta `d4db8bbb49a7b98e75531ca7dab93fb4c771bf0a`
and Semantica `e702c4d05513aae5667f62a8f991bf2352c8ddfc` (both historical,
not an execution freeze). Current static re-audit below supersedes baseline
implementation-status statements, not the required clean-source proof gates.

### Current-source re-audit (2026-09-24; static only)

Quanta source snapshot `a0ac1853256d9b507ae8dc76f7437a4c7568434c`
(`main`, ahead of `origin/main` by six and behind by two commits at inspection).
This shared commit absorbed the P09 IPC and P12A handoff code together with
unrelated retrieval work. The checkout still had dirty `Justfile`, SEP-21
progress/plan docs and a retrieval proof test. Re-freeze before execution;
the repository is not a clean result SHA. Semantica HEAD/dirty state and
actual deployed state roots were **not** revalidated in this pass. No
release proof, deployment, activation or rollback drill ran for this audit.
Local focused tests are recorded separately in `EXECUTION-PROGRESS.md` and
are not a clean-source qualification receipt.

| Lane | Confirmed current source | Residual / no-duplicate decision |
| --- | --- | --- |
| P06 | SDK `binding.rs` compares complete typed activation/rollback identity; the old `same_identity` field subset is gone. `ActivationCatalog::ActiveRoots` remains `BTreeMap<ActivationKey, SearchCorpusGenerationV1>` and persists `PersistedSearchCorpusGenerationRootV1`. | Treat the SDK fix as partial, not another task. Add one catalog-owned event token and extend the existing contract, resolution, read-view and SDK path. No parallel head map or SDK epoch. |
| P09 | IPC code at this HEAD has `RequestEventScope`, a bounded 1024-entry `IpcServerCounters` tail and a dropped count; searchd's generic adapter projects closed route and top-level typed error. Overload encode/write events are distinguished. Query and ingest `PlaneDispatch` still ignore `DispatchContextV1`; the tail has no operator read path. `RuntimeReadiness` still sets `required_backend: true`, and its active proof cache has no age horizon. | Finish provider-stage correlation and a bounded, authorized diagnostic read path on the existing control plane; do not create a second event bus, request-ID allocator or provider-usage ledger. Define the backend/physical-proof detection interval. |
| P10 | `LegacyStateImporterV1` explicitly refuses materialized V1 RepoMap and generic unconsumed legacy objects; no source-bundle replay exists. | Preserve refusal. Replay is conditional on real root plus exact original bundles; do not build an importer-only graph IR or guess a no-legacy waiver. |
| P11 | Public V1 SDK/wire/dispatcher/store mutation remains reachable. `RepoMapActivateGenerationRequestV2.request_v1` is a V1 DTO, and `RepoMapGenerationStore::activate_generation_v2` calls public `activate_generation`. | Extract one private commit primitive, then remove all V1 success surfaces in one breaking cutover. No V2-to-V1 adapter and no duplicate catalog transaction. |
| P12A/Q | `handoff_validation.py` is already the single Git/archive/chain leaf used by CLI and aggregate; aggregate schema/writer/checker have product and infrastructure handoff fields. | Do not reimplement P12A from the old ticket prose. Validate the leaf/aggregate against independent negatives and real artifacts, then issue clean-source proofs. Missing historical handoffs and final-source receipts are not repaired by synthetic records. |

Status of every row above: source observation only (`NOT_RUN` qualification).
No assertion about Semantica's present producer wrappers, deployed state, or
historical proof validity is made from this Quanta-only inspection.

### Writer and SSOT collision fence

The following are **serial integration seams**, even if independent owners
develop elsewhere. Before editing, re-freeze the named files and assign one
writer; a dirty file is not an invitation to overwrite its current owner.

| Shared seam | Order / single authority | Forbidden overlap |
| --- | --- | --- |
| `contract/src/ipc/{control,split,ingest}.rs`, `sdk/src/{client,binding,repomap}.rs` | P06 defines the catalog-owned active-head token and its query/control binding; P11 then removes V1 RepoMap mutation wire and SDK success surfaces against that result. Update the one wire inventory/public API baseline only after both semantic cuts integrate. | Separate P06/P11 compatibility decoders, SDK-minted epochs, or two `ResolveActiveGeneration` routes. The proposed Sep-24 SDK DSL is a later ergonomic projection over these contracts, not a concurrent protocol owner. |
| `search-plane/src/readiness/{activation_catalog,search_corpus_generation}.rs`, `searchd/src/app/{state_format,state_migration,runtime}.rs` | P06 owns head event and root-incarnation format. P10 consumes that exact format through the existing offline staging/import path. | Migration-local head schema, inferred incarnation from an empty directory, or a second runtime catalog. |
| `ipc/src/{server,counters}.rs`, `searchd/src/app/ipc_dispatcher.rs`, `search-plane/src/query_embedder.rs` | P09 completes the dirty transport sink and projects backend/provider stages from the existing dispatch budget and provider audit. | A route-local ring, new request ID, duplicate cost ledger, or labels containing request IDs. |
| `repomap/src/{materializer,store}.rs`, `contract/src/repomap/terminal_receipt_v2.rs` | P11 first extracts the production V2 private commit path; P10 replay calls only that path and the existing compiler/catalog sequence authority. If migration work precedes P11, it may define offline input/negative tests but cannot mint an importer-only mutation API. | `activate_generation_v2 → activate_generation(V1)`, snapshot-to-source reconstruction, or a second replay sequence allocator. |
| `tools/ci/lint/handoff_validation.py`, proof aggregate schema/writer/checker | P12A validates the now-committed leaf once. Registry declarations and independent expected DAG remain separate by design. | Duplicate Git/archive parser in writer, checker-import cycle, or a registry-derived expected graph. |

The Sep-24 single-IR note concerns internal IR and benchmark artifacts; it
does not authorize removing persisted state/wire format stamps or changing
RepoMap source-bundle versus compiled-candidate authority. The Sep-24 SDK DSL
RFC does not supersede P06/P11 safety contracts. Its API redesign must wait
for those contracts or merge into their sole SDK writer explicitly.

## 0. Decisions and authority boundaries

1. Breaking cutover is allowed. There is one successful RepoMap mutation wire
   contract: V2 publish and V2 activate. Do not preserve V1 success, dual-write,
   or an implicit fallback. An old V1 wire request may be rejected at decode;
   a typed incompatibility response requires a separately designed versioned
   envelope/handshake, not a promise made by deleting a decoder variant.
   P06 and P11 must share this one IPC compatibility decision and wire
   inventory; do not implement separate tombstones or negotiation layers.
2. Producer `RepoMapSourceBundle` is the source graph contract. Its versioned
   canonical-CBOR digest identifies exact producer input. The P02A
   `CompiledRepoMapCandidateV1` is a validated, sorted, derived graph/projection
   IR; its compiled-graph commitment has a different domain and purpose. A
   materialized V1 snapshot is neither of those inputs. Never reconstruct a
   purported source graph from snapshot rows or substitute compiled commitment
   for source-bundle digest.
3. The search-corpus activation catalog owns the serving active-head event.
   The sealed generation owns content identity. The SDK and read view consume
   catalog authority; they do not mint an independent epoch or commitment.
   A content commitment is derived by one versioned canonical encoder from
   the validated complete generation, not maintained as a second writable
   field. Physical artifact-open proof remains separate from metadata binding.
4. Direct immutable `Pinned` reads remain legal without an active-head token.
   Only a request that claims to read the current `Active` head must carry and
   prove the resolved active-head token. Lexical/semantic head identity does
   not claim a single instant for auxiliary history/runtime/structural reads.
5. `proof-authority.toml` is the operational proof registry. The checker's
   fixed expected proof graph/verdict set and lane-handoff policy are
   independent acceptance oracles against a weakened or forged registry;
   do not delete them in the name of SSOT. Share validation *logic*, not an
   untrusted declaration and its sole expected answer.

### Static SSOT / duplicate-implementation audit at the planning baseline

| Concern | Existing owner to extend | Do not add or duplicate |
| --- | --- | --- |
| Corpus content vs serving event | `SearchCorpusGenerationV1` validates the sealed lexical+semantic identity; `ActivationCatalog` persists and serves the active root. | A second mutable content commitment, SDK-owned activation epoch, per-track serving head, or a replacement generation type. Add the event token *around* the existing generation. |
| Read evidence | `QueryReadViewV2` and `ReadIdentityV2` already acquire and describe declared domains. `ResolveActiveGeneration` is the query-plane resolution route. | A second read-view acquisition, parallel active-resolution endpoint, or a purported globally atomic history/runtime/structural snapshot. Extend the existing resolution/identity path only for an `Active` claim. |
| Request correlation/usage | IPC `DispatchContextV1` and `RequestBudgetV1::correlation` already carry admitted IDs; IPC counters, query execution trace and provider audit already have separate purposes. | A new request-ID allocator, replaying provider billing into a second ledger, or treating metric samples as terminal request events. One bounded diagnostic event sink may reference the existing provider audit by correlation. |
| RepoMap source and derived data | `RepoMapSourceBundle` plus its versioned digest is producer input; the existing compiler produces `CompiledRepoMapCandidateV1`; the V2 store/journal owns publish, activate and sequence. | Snapshot-to-source reconstruction, importer-only compiler, independently allocated replay sequence, or source digest inferred from compiled commitment. |
| Proof acceptance | `proof-authority.toml` declares operational nodes; `check-proof-authority.py` has fixed expected DAG/verdicts; `check-lane-handoff.py` has the lane policy and Git/archive checks. | Registry-derived expected answers, a second handoff validator in the aggregate writer, or historical handoffs masquerading as final-source proof. |

R1 SDK checkpoint already present in current source:
`crates/quanta-index-sdk/src/binding.rs` compares full typed identities,
including `semantic_content`, for activation/rollback ACKs. Retain its
swapped-root negative test. The remaining SDK work is the new catalog-owned
head-token comparison; do not restore `same_identity` or implement a second
field-by-field matcher.

These are source-traced design boundaries, not executed correctness proof. Before
editing each owner, compare its current exported types and consumers again;
the baseline HEAD and dirty state can change while other writers work.

## 1. Admission / freeze — no product edits until this gate

- Freeze Quanta and Semantica HEAD, dirty paths, `Cargo.lock` digests, and path
  ownership immediately before each writer. Do not overwrite unrelated work
  in either shared main checkout. The paired source and dependency roots must
  be clean for exact-pair proof issuance.
- Read-only inventory the actual target state roots: absent/current/legacy
  format, **all** legacy RepoMap snapshot keys and activation records, original
  producer bundle availability for every retained snapshot, active identities,
  terminal sequence high-water, and required rollback boundary. Record
  file/object digests, not just counts or a human summary.
- Decide P10 from that inventory: `REPLAY_REQUIRED` only when a target has
  materialized V1 RepoMap and trustworthy original graph bundles for every
  retained snapshot; `BLOCKED` when it has such data without a required
  bundle; `NOT_APPLICABLE` to a no-legacy target only with an explicit
  scope-bound release waiver. If inactive snapshots may be intentionally
  discarded, that data-loss boundary needs a separate explicit operator
  decision and receipt, not an importer default. The existing typed refusal
  always remains.
- Freeze the release guarantee for P09 backend health: either an on-demand
  check or a stated maximum detection interval. Do not claim immediate
  detection from a cached boot proof.
- Freeze the supported restore boundary. A local root cannot distinguish an
  out-of-band byte-for-byte copy from its original. Require the controlled
  restore entrypoint and deny serving an unadmitted copy, or introduce an
  external incarnation/fencing registry; a local epoch alone is insufficient.

Stop: missing source bundle, ambiguous state-root ownership, dirty paired
checkout, or absent deployment/activation/rollback authority is a typed
`BLOCKED` node, never a guessed input or successful aggregate.

## 2. R1 — P06 active-head identity and restore fencing (serial first)

Root cause: a generation/content identity is currently also used as the
activation event and as the CAS expectation. Rollback and reactivation of the
same generation cause ABA; restoring an older backup can reuse an epoch.

| Owner files | Action |
| --- | --- |
| `crates/quanta-index-search-plane/src/readiness/search_corpus_generation.rs`, `readiness/activation_catalog.rs`, `search_corpus_lifecycle.rs` | Keep `SearchCorpusGenerationV1` as the content identity; delegate its shared wire-shape rules to `SearchCorpusGenerationIdentityV1::validate_v1` instead of growing a second validator. Replace the generation-only active map/root with one validated `ActiveSearchCorpusHeadV2` containing that generation, the root incarnation and checked pair-local activation sequence. Activation and rollback advance the sequence under the existing pair mutation guard, persist before visibility, and refuse overflow/uncertain durability. Extend the existing catalog snapshot and lifecycle construction; do not add a second catalog. |
| `crates/quanta-index-searchd/src/app/state_format.rs`, `app/state_migration.rs`, `app/runtime.rs`, `crates/quanta-index-searchd-runtime/src/lib.rs`, `state_migration.rs` | Introduce one versioned durable root-incarnation authority under state-root custody and inject it into the existing lifecycle. Capture fresh-vs-existing root status immediately after lease acquisition and before lexical/semantic adapter constructors create directories; do not infer freshness later from an empty `activations/` directory. Heads reference the incarnation; the offline manifest's object digest can bind its record without becoming a second live head authority. Mint a new incarnation only for a proven fresh root, offline V1→V2 conversion, and controlled restore; rewrite heads in staging before manifest-last publication. Existing nonempty roots with no provable identity fail closed; no live dual-read. |
| `crates/quanta-index-contract/src/ipc/control.rs`, `ipc/split.rs`, query request/response DTO owners | Introduce one breaking typed active-head token (root incarnation, sequence, complete generation/content digest). Place the versioned canonical full-generation commitment encoder here; never use the pair lock-name digest as content evidence. Activation/rollback CAS expectation and ACK, the existing `ResolveActiveGeneration` route, and active query request share its canonical type/encoder. Retire incompatible V1 wire shapes; do not keep a generation-only active-resolution success route. A direct pinned selector does not require this token. |
| `crates/quanta-index-search-plane/src/query_dispatcher/{selection,dispatcher}.rs`, `query_dispatcher/read_view/view.rs`, `crates/quanta-index-core/src/domains/read_view/identity.rs` | Resolve one composite catalog head, compare the request token against the head, then compare acquired lexical/semantic artifacts with the sealed generation. Extend the existing `QueryReadViewV2`/`ReadIdentityV2` evidence for active claims rather than opening another view. Populate active-head evidence from the catalog-owned token; do not synthesize it from result rows or RepoMap's separate activation epoch. |
| `crates/quanta-index-sdk/src/{client,binding}.rs` and relevant SDK route modules | Keep the already-landed complete typed identity equality. Freeze the new catalog-owned token via the existing query-plane resolution route before the final active request and exact-bind the response to it. Preserve query-only and legal `rev:at.time` behavior. A successful first resolution is server authority, not independent proof against a server that forges both calls. |

Independent negatives / DoD: B resolve → A rollback → B reactivation
rejects the old token; old-backup restore rejects old token even if generation
and pair sequence repeat; stale activation/rollback CAS mutates zero bytes;
wrong content, missing incarnation, overflow, parent-fsync uncertainty, and
reopen mismatch all refuse; fresh direct-pinned query still works. Add contract
decoder, catalog crash/reopen, real query-only UDS, SDK same-variant and
semantic-content-root swap,
offline restore and old/new format refusal tests. Re-run the registered P06
owner rail, wire inventory, public API, fuzz smoke, and changed package checks
on one clean result HEAD. An out-of-band clone cannot serve under the declared
deployment admission/fencing policy. Old V1 roots are offline input only.

## 3. R2 — P09 request events and bounded readiness freshness (can develop beside R1)

Root cause: IPC creates `DispatchContextV1`, but query/ingest adapters discard
it; metric aggregates and tails are not a request-stage terminal record.
`required_backend=true` reports boot assembly, not necessarily current health.

| Owner files | Action |
| --- | --- |
| `crates/quanta-index-ipc/src/{server,counters}.rs` | Preserve the committed one bounded request-event tail (1024 entries), nonzero envelope ID, connection ID, dropped count and corrected overload encode/write classification. Keep transport-owned admission/terminal events and counter authority. Add/retain panic/abort terminal tests, and make explicit that ring loss prevents a complete per-request trace. Do not add another sink or ID allocator. |
| `crates/quanta-index-searchd/src/app/ipc_dispatcher.rs`, `crates/quanta-index-search-plane/src/{query_dispatcher,query_embedder,observability}.rs`, existing provider-audit owner | Keep the generic backend start/return and closed route/typed-error projection in one adapter. Pass `DispatchContextV1` or its established correlation through query/ingest instead of discarding it. Provider stage references the existing `RequestBudgetV1::correlation`/`ProviderBudgetLedger` audit, never duplicates usage settlement. Never emit query/source bytes, credentials or high-cardinality metric labels. |
| control-plane diagnostic contract/dispatcher, `crates/quanta-index-searchd/src/app/runtime.rs`, SDK/searchctl observability owners | Expose a fixed-limit read of the existing IPC event tail under the existing diagnostic/control capability; include dropped count, process/plane identity and bounded time/window semantics so clients cannot infer a complete trace after loss. Reuse the same sink; a new ring or metrics labels keyed by request ID is forbidden. |
| `crates/quanta-index-searchd/src/app/readiness.rs`, maintenance/scrub owners, process-readiness contract | Replace the unconditional backend component with an observed proof/freshness status. Define the detection horizon and invalidate cached active integrity on catalog mutation, scrub findings and expired proof. Keep process readiness separate from generation status. |

DoD: `(process instance, plane, connection ID, nonzero envelope ID)` links
queue→backend/provider→response/close (including overload, deadline,
disconnect and panic); ring overflow is visible and never treated as a
complete trace; no payload/secret leak; required child or backend failure and
stale proof make `ready=false` within the declared horizon. P09 real-UDS owner
and release process rail must be executed separately. Existing
executed/contributed engine metrics are not reimplemented.

## 4. R3 — P10 offline state conversion, conditional on admission inventory

Root cause: the old RepoMap snapshot is a materialized view, not the graph IR
the current compiler and V2 terminal contract require. Copying it as current
authority would produce a false-success cutover.

| Owner files | Action |
| --- | --- |
| `crates/quanta-index-searchd-runtime/src/state_migration.rs`, owner integration tests | Preserve materialized-only refusal. If `REPLAY_REQUIRED`, accept a frozen exact original `RepoMapSourceBundle` for each retained legacy snapshot key, plus the old activation map, source fingerprint and original bytes. Canonically decode/re-encode and compare bytes before calculating the existing versioned source-bundle digest. Reject absent, duplicate, ambiguous and noncanonical input before publishing a destination. Mark legacy files consumed only after their corresponding V2 replay is verified; the generic unconsumed-object refusal must still catch residue. |
| `crates/quanta-index-repomap/src/{materializer,store}.rs`, catalog sequence/replay owners | For `REPLAY_REQUIRED`, first land the P11 production V2 private-commit extraction and V1 success removal at this store seam. Then replay through that same V2 compiler/publish/activate authority. Do not introduce an importer-only graph compiler or a second sequence allocator. Compare source-bundle digest, compiled commitment and active identity against independently frozen evidence; define and check an explicit monotonic mapping for legacy high-water/replay floor instead of assuming old and new sequence numbers are equal. |
| state-format/backup/restore owners and operator runbook | Perform R1 root-incarnation conversion in the same offline staging flow, deep-open/scrub, write manifest last, atomically cut over and state the exact pre/post-first-mutation rollback policy. |

DoD: source remains read-only; interruption or disagreement never publishes a
serving destination; every retained snapshot and active mapping has an explicit
old→new inventory record, and complete source bundles reproduce the required
observable identity/sequence contract. An intentionally discarded inactive
snapshot is named in a separately approved data-loss receipt. If actual
targets have no legacy RepoMap, retain negative refusal tests and record the
scope waiver; do not pretend replay ran.

## 5. R4 — P11 one successful RepoMap mutation protocol (after R1/R3)

Root cause: V2 was added while V1 SDK, wire, dispatcher and public store
mutation remain reachable. `activate_generation_v2` currently calls the V1
store mutation after its V2 precheck.

| Owner files | Action |
| --- | --- |
| `crates/quanta-index-repomap/src/store.rs` | Extract the existing compile/seal/catalog and activation commit/replay bodies into private primitives with validated V2 authority as input. Preserve the existing catalog transaction, object verifier, journal and terminal sequence allocator. V2 does not call a public V1 mutation. Remove or make inaccessible V1 public mutation methods after fixtures are rewritten. |
| `crates/quanta-index-contract/src/repomap/terminal_receipt_v2.rs`, `ipc/{ingest,split}.rs`, `crates/quanta-index-search-plane/src/{ingest_dispatcher/dispatcher,control_dispatcher}.rs`, `crates/quanta-index-sdk/src/{repomap,client,binding}.rs` | Remove V1 publish/activate success variants and SDK methods, update exhaustive validation, wire inventory, public API baseline and tests. Replace V2 activation's nested `request_v1` mutation DTO with one V2-owned identity shape as part of the breaking wire cutover; value identity fields may be reused internally, but no V1 mutation request remains the V2 authority. Do not leave a decode-to-V1-to-private-write route. Hard removal means old wire decode refusal unless a separately approved versioned-envelope handshake is built. |
| Semantica `quanta-runtime-retrieval-kernel/src/index_sdk_ingress/{repomap,facade}.rs`, producer handoff tests | Delete public V1 mutation wrappers and convert remaining fixtures. Preserve the already-used V2 prepared-member → publish receipt → activate receipt chain and bind it to exact source bundle, candidate, and activation axes. |

DoD: every V1 wire/API/direct-store mutation is absent or non-successful;
old/new producer-daemon pairings cannot mutate; exact V2 replay does not
recompile/reseal on publish or advance the catalog transaction/terminal
sequence on activate (read-only custody verification is allowed); one clean Quanta/Semantica source pair,
lock digest, release daemon binary and host are attested. Coordinate a stopped
producer/daemon cutover; do not infer deploy/activate/rollback from code proof.

## 6. R5 — P12A proof closure, then P12Q qualification

Historical root cause: aggregate validation consumed proof manifests but not
the product handoff graph. The current implementation adds that graph;
it still needs reconciliation and clean-source proof. The operational registry
and fixed checker oracles must not be collapsed into self-validation.

| Owner files | Action |
| --- | --- |
| `tools/ci/lint/{check-lane-handoff,handoff_validation}.py` | Inspect the committed acyclic leaf and CLI adapter before changing semantics. Preserve its one fixed lane policy, single-handoff Git/archive checks and historical-chain fork/join. Historical result SHA need not be current HEAD. Keep proof-checker injection; no leaf import back into the aggregate checker. |
| `tools/ci/{proof-aggregate.schema.json,write-proof-aggregate.py}`, `tools/ci/lint/check-proof-authority.py` | Reconcile the already-present `product_handoffs`, `product_chain_status` and `infrastructure_handoff` fields. Validate archive/proof bytes using the same leaf and independently compare the aggregate to fixed expected dependencies/verdicts. Four final-source verdicts derive from final-current receipts only; historical chain gates `production_ready` separately. |
| `tools/ci/tests/{test_check_lane_handoff,test_write_proof_aggregate,test_check_proof_authority}.py`, `Justfile`, test/proof authority registry | Retain and extend missing/duplicate/reordered, wrong fork/join, tampered/symlink/archive, wrong pair/binary/host, and historical-versus-final negatives; add a real positive chain only from authentic artifacts. Re-run the executable P12A owner rail on a clean result SHA. Do not issue P12 final proof from P12A. |

Historical P00/P01/P02A/P02B/P02I handoffs do not exist locally. Recover only
authentic archived source-bound results if available; do not fabricate
backdated commits, counts or manifests. If history cannot be reconstructed,
record a new explicitly named clean-source integration baseline and revise
the acceptance contract under review before P12Q; never silently weaken the
checker to accept the gap. P11 and P12A handoffs are issued only at their real
future result HEADs.

P12Q DoD: same clean source pair and same attested release binary satisfy all
mandatory owner/release nodes; `CODE_QUALIFIED`, `DEPLOYED`, `ACTIVATED` and
`ROLLBACK_PROVEN` are separate results; absent, stale, wrong-host, zero-run,
failed or unapproved input cannot become ready. Linux process, deployment,
activation and rollback are distinct receipts and approvals.

## 7. Execution order and stop rules

1. R0 inventory/authority freeze; no legacy replay without data and source IR.
2. R1 P06 root/head/contract breaking change; integrate and verify before
   any state migration or paired producer edit.
3. R2 P09 can be coded independently. R5 validator *code* can be developed
   independently, but P12A proof issuance still depends on the P11 exact pair.
   Share no product files or proof artifacts between writers.
4. R3 P10 inventory/input validation can proceed after R1. If replay is
   required, complete P11's **single production store cutover** (private V2
   commit, no public V1 success) before implementing the importer against it;
   do not add a temporary importer adapter and later remove it. If no real
   legacy target exists, issue the explicit scope waiver and retain refusal.
   The registered P10 owner proof depends on the P09 owner proof; the P10
   release proof depends on the P09 release proof. This is proof ordering,
   not a requirement to write the importer before its production primitive.
5. R4 completes the remaining P11 wire/SDK/producer cutover serially after
   R1 and the conditional P10 implementation, with a clean paired checkout
   and named authority. Its registered cross-repo proof follows the P10
   release node. Do not edit another writer's dirty Semantica main.
6. R5 P12A handoff/proof, then P12Q on one final clean source pair and binary.

At each result checkpoint: inspect HEAD/dirty state → run cheapest owner
checks → run registered owner proof on clean result source → validate immutable
handoff → non-force publish and verify remote SHA. A later source change
invalidates final-source receipts. Compilation is not a test; owner tests are
not release proof; release proof is not deployment or activation. No material
state-root mutation, external provider egress, deployment, activation or
rollback drill is authorized by this planning document.

Registered proof commands at the corresponding clean-source checkpoints:

| Gate | Exact registered command | Current authority |
| --- | --- | --- |
| P06 owner → release | `just proof-p06-sdk-binding-owner` → `just rust-profile test-daemon` | owner executable; release staged |
| P09 owner → release | `just proof-p09-control-readiness-owner` → `just rust-profile test-daemon` | owner executable; release staged |
| P10 owner → release | `just proof-p10-state-migration-owner` → `just rust-profile test-daemon-all` | owner executable; release staged |
| P11 pair → deploy → activate → rollback | `just rust-verify-hellgate-cross-repo` → `just proof-p11-deployment` → `just proof-p11-activation` → `just proof-p11-rollback` | all staged |
| P12A → P12Q | `just proof-p12a-proof-infrastructure` → `just proof-authority-final-qualification` | P12A executable but dependency-blocked; P12Q staged |

The repeated `rust-profile` commands have different registered targets and
manifest identities; the command alone is not a receipt. A staged node must
first land its missing target and pass registry/checker admission. The
registered proof DAG, required host, exact source pair, binary binding, raw
result, artifact and digest govern acceptance; these commands have not been
run as part of this planning review.
