# State Cutover Runbook (S21-11 / S21-12)

Current tasks, owners and next actions: [Oct-17 SSOT](../plans/oct-17-index-closeout/tickets/README.md), specifically [W2-18 cutover](../plans/oct-17-index-closeout/tickets/W2-18-native-cutover.md) and [W2-19 qualification](../plans/oct-17-index-closeout/tickets/W2-19-integration-acceptance.md). Source-scoped execution results remain in [VALIDATION](../plans/oct-10-index-closeout/VALIDATION.md).

This runbook owns procedures and safety contracts. Dated progress/implementation statements below are historical checkpoints, not a live task queue or current-source verdict. Do not recreate work or infer current completion from their percentages, diagnostics or pending lists.

Offline state backup/restore and the RepoMap single-mutation
producer cutover. Offline backup/restore commands require the daemon fully
stopped. Native SDK inventory recovery uses the running Native service.

## Current cutover limits and staging safety (2026-10-12)

This runbook describes the existing legacy state workflow, not backup/restore
qualification for the opt-in Catalog/Kernel native product. The remaining
implementation and exact acceptance owners are in
[W2-18](../plans/oct-17-index-closeout/tickets/W2-18-native-cutover.md) and
[W2-19](../plans/oct-17-index-closeout/tickets/W2-19-integration-acceptance.md).
The Native RFC retains design contracts and dated observations. Launcher `serve`, explicit `serve-native`, their state
markers and native Semantic/Auto capabilities are not interchangeable.

Native source now uses `.native-index-v12`. Source-event capture v2 binds the
immutable logical mutation count/digest separately from the exact execution
ViewRef/SourceCut and physical mutation-root layout. The Catalog verifies a
persistent state-v12/capture-v2/hash-domain singleton before DDL, allocator
seeding or recovery, including direct in-process Catalog/Kernel consumers.
A v1-v11 marker or existing Native namespace without the exact current stamp
requires a new root and explicit rebuild/new incarnation. Do not delete the
old marker, relabel the root, rewrite event payloads or reuse old capture claims.
Ordinary generic Catalog roots may already contain a Native namespace, so this
breaking refusal applies to those unstamped roots too.

The native service retains its indexed terminal-custody ready set and bounded
automatic maintenance with explicit workspace/epoch sequence windows. Existing
pins and replay receipts are preserved inside the supported current incarnation;
this statement does not promise old-format receipt migration. Source/version-transition
implementation and selected proof are recorded in
[Native RFC section 23.61](../history/oct-10-index-closeout/RFC-CHECKPOINTS-2026-10-21.md#2361-f07-logical-capture-rebase-and-native-v11-format-gate-2026-10-13).
Producer-wide source resync seals its mode with the original operation and
switches epoch only with the complete known-owner baseline, view and receipt.
Pending resync does not expose the new epoch. A baseline must explicitly
include every known owner of the producer; filesystem completeness remains
the producer responsibility. Source events start at ordinal one, independently
of the operation attempt sequence.

When the producer's local owner ledger is lost, use Native SDK
`list_producer_source_owners(selection, producer, after, limit, budget)`.
The first page may select Latest; all continuation pages use `Exact(page.view)`
and `page.next_after()`, with limit 1..128. Persist the recovered view/source
and prior epoch with the reconstruction attempt. Include every recovered owner
as unchanged, replacement or deletion in SourceResync and discover current/new
files independently. Publication or epoch changes during enumeration return
Conflict; restart from a current first page. Historical inventory enumeration
is intentionally refused; historical known-key lookup remains available.
The page API is a recovery input, not actual V9 producer delivery or backup
qualification. Implementation/verification scope: [RFC section 23.63](../history/oct-10-index-closeout/RFC-CHECKPOINTS-2026-10-21.md#2363-f07-paged-producer-inventory-and-epoch-recovery-2026-10-13).

Native backup/restore, a supported old-outbox/new-incarnation reconciliation,
sustained disk capacity and default launcher switch are still separate gates.

Backup/restore creates its derived staging directory exclusively. Any existing
staging entry (directory, file, symlink or dangling symlink) is a typed
`InvalidRequest` refusal; it is neither reused nor removed. The destination
itself must also be absent, including an empty directory. These checks precede
staging writes.

Interrupted and source-drifted staging roots remain unpublished. Inspect their
contents and custody before explicitly cleaning an operator-owned staging root,
or select another absent destination. A derived name or a sealed manifest does
not prove ownership and does not make staging an activated root. Automatic
path-based recursive cleanup is deliberately unavailable. The existing offline
command owner accepted this boundary on frozen source `bb7a8bf2`: 59 tests passed,
including source/external symlink preservation, interruption retry and drift.
This legacy workflow result does not qualify new-native product restore.

New-native cutover additionally requires
its actual backup/restore or explicit rebuild route, old-view/replay identity
checks and a supported reopen/rollback drill before default launcher selection.

## Offline state workflow (S21-11)

CLI owner: `crates/quanta-index-searchd/src/cli/command.rs`.
Execution owner: `crates/quanta-index-searchd-runtime/src/state_migration.rs`
(`backup-state`, `restore-state`, `verify-state`).

1. Stop the daemon. Acquire the exclusive `StateRootLease` (shared by the
   daemon and the offline CLI, `searchd/src/app/runtime.rs`); the lease
   validates uid/mode/regular-file/`nlink == 1`.
2. `backup-state --source <current> --destination <backup>` freezes a
   current-format root. SQLite travels via the backup API, never raw file copy.
   A daemon-created root may lack a root manifest; backup validates and
   writes the backup manifest.
3. `verify-state --source <backup>` checks the produced backup manifest.
   It rechecks the source inventory, selected manifest, and manifest kind
   after catalog verification; a detected in-command change is a refusal.
   Manifest reads reject symlinks and hard links; manifest publication uses
   exclusive creation so a dangling link cannot redirect the last write.
   Decode and publication share the same authority validator: current
   format, canonical lowercase SHA-256 digests, and unique file/directory
   identities. Decode requires the exact canonical byte encoding; duplicate
   headers, reordered fields, noncanonical numbers and line endings are
   refused rather than normalized into a different receipt digest. Writers
   accept only the two owned top-level manifest names and validate before
   creating any file. Valid current manifests retain their existing format.
   Current-root verification includes catalog bytes in its start/end freeze
   and binds the root inode to the held custody. SQLite may create native
   WAL/SHM bookkeeping during reads: only regular files at the exact paths
   `catalog/catalog-v1.sqlite-{wal,shm,journal}` are excluded. Their creation
   can change the catalog directory's size, mtime and APFS link count; these
   fields are not compared for that directory, but its inode/type/mode/owner,
   the complete advertised inventory and all payload file identities remain
   checked. Sidecar symlinks/hard links are refused; suffixes elsewhere are
   ordinary state and must be preserved.
   Filesystem paths are admitted without lossy UTF-8 conversion or separator
   rewriting. Non-UTF-8 names, control characters and backslash separators
   are refused before inventory or authority publication. Extending current
   verification with catalog bytes does not reset the session's existing
   non-catalog identities or root custody.
   Backup custody is read-only, not an exclusive lock: keep the backup root
   quiescent throughout verification and cutover. A successful point-in-time
   check does not authorize later mutation or prove a different target root.
4. `restore-state --source <backup> --destination <new>` and
   `verify-state --source <new>` validate the restored root. Do not use
   `verify-state` as a pre-backup check on a manifest-less daemon root.
5. Same-filesystem atomic cutover of the configured root, then re-open
   with the release binary. Never configure the daemon against an unpublished
   staging directory: `verify-state` refuses a manifest-less staging root,
   and the root manifest is written+fsynced last before publication.
6. `backup-state` moves one freeze boundary: catalog+object+root manifests
   with every file/object digest and size. `restore-state` preserves the
   backed-up data identities, receipts, replay floor, and sequence high-water,
   but rotates the activation-catalog root incarnation in its unpublished
   staging root. The restored root manifest attests that new incarnation;
   it must not be byte-for-byte identical to the backup manifest. A token
   issued for the source root is not authority for the restored root.

Boot never migrates: a legacy root is refused typed
(`state_format::refuse_legacy_state_root_v1`), and a manifest whose
`format-version` differs from the build's is refused typed — old
binary/new root and new binary/old root are both explicit refusals, with
zero mutation.

There is no offline legacy importer. `migrate-state` is rejected at the CLI;
legacy semantic journals and pre-catalog auxiliary snapshots are markers for
typed refusal only. Rebuild from current typed producer input. Backup and
restore never convert an old format into the current one.

## Rollback boundary

- Before the first new-format mutation commits, rollback is the old
  binary/root pair only.
- After any new-format mutation, old binary/root rollback is forbidden;
  restore-forward from a backup is the only path back.
- There is no post-cutover backward write compatibility.

## RepoMap single-mutation producer contract (S21-12)

- The SDK exposes only `repomap().publish(&RepoMapPublishBundleRequestV2)` and
  `repomap().activate(RepoMapActivateGenerationRequestV2)`. The old V1 IPC
  opcodes, SDK methods, core mutation ports, and direct-store V1 methods are
  removed. `RepoMapActivateGenerationRequestV2` carries its identity fields
  directly; a nested `request_v1` map is refused.
- V2 publish carries the exact full source-bundle digest; the daemon
  recomputes it before mutation and seals manifest digest + source-bundle
  digest into durable `projection_meta`.
- Activation compares repo/revision/generation, manifest digest,
  snapshot id, projection version, authority digest, and source-bundle
  digest against the sealed candidate. Projection metadata without both
  custody digests is rejected at decode/open; it is never relabeled in place.
- ACK loss replays the identical terminal receipt; only
  `mutation.replayed` flips to `true`. No rebuild, re-embed, or
  reactivation work repeats.
- Cutover order: freeze the receipt schema + wire inventory + semantic
  validator in one coherent commit → producer pins that exact
  contract/SDK tree → qualify the exact source pair with the built
  daemon binary (see `just rust-verify-hellgate-cross-repo`) → deploy →
  activate → rollback drill. Each stage issues its own receipt; a pass
  never implies the next stage. Deployment is frozen while a cutover
  qualification is open.

## Evidence pointers

- Offline workflow: `state_migration_owner_v1` (current-format
  backup/restore/verify and legacy refusal tests).
- Mutation matrix: `candidate_activation_owner_v1` (bind/replay, legacy
  metadata refusal, custody corruption, axis substitution, ACK bindings).
- Cross-repo qualification receipt: P11 family in
  `tools/ci/proof-authority.toml` (requires a frozen producer pair and
  the attested daemon binary; see the S21-12 ticket).

## Historical legacy paired-cutover checkpoint

The RepoMap V1 success route removal is a local dirty-tree candidate, not a
cross-repo cutover receipt. The Semantica producer must build against the flat
V2 activation request before qualification. Separately, the intended semantic
cutover removes `QUANTA_INDEX_SEMANTIC_DERIVE_MODE` and all live chunk-text
derivation fallback. It requires typed semantic-source producer fixtures and
an exact-source search/ingest proof; a chunk-only publish now yields no dense
vectors, not a hidden chunk-text fallback. The previous Semantica handoff
could build a lexical batch with empty semantic replacement scopes when
`shadow_semantic_publish` was false; the single-path producer change removes
that switch. A lexical delta can still intentionally omit unchanged semantic
scopes. The wire currently has no independent semantic coverage witness, so
an empty vector alone cannot distinguish unchanged from an erroneous omission;
producer tests and paired qualification must establish that invariant. The
typed `RawCodeFallback` budget and card production remain required. The
semantic manifest format is now 11 and its pre-seal build-contract format is
3; prior-format generations are refused and must be rebuilt from typed
producer input, not resumed or relabeled. This version bump is local source
behavior, not a completed candidate-root or cross-repo qualification. The
retired `migrate-state` importer is unavailable. Old semantic generations
are typed-refusal/rebuild candidates, with no in-place migration. The live
semantic runtime cutover is still in progress; these docs do not prove that
a running binary or the Semantica producer has completed it.

## Semantica producer replay envelope V3 boundary (F07, 2026-10-13)

The following paragraphs record the 2026-10-13 source boundary. Their format
numbers and implementation gaps are historical observations, not instructions
to rewrite current markers or recreate SDK/endpoint code. Current requirements
and residual ownership follow the W2-02/W2-05 tickets linked below.

The producer's `SourceBoundCloseoutReplayV2` JSON envelope is now format 3.
It requires explicit preparation and request-binding fields even for the
existing publisher, where both are initially None. JSON format 2 and missing
fields are refused. No importer or implicit old-row default is implemented.
This applies to Semantica's producer replay store; it does not change Quanta's
Native index state version 12.

The producer SQL carrier still has `replay_format_version = 2`: its existing
columns and CHECK are unchanged. The nested JSON envelope is independently
validated as format 3. Neither stamp may be edited to bypass the corresponding
validator. Preserve original replay rows and writer/member evidence; successful
cutover of an existing producer store has not been qualified.

Native preparation is immutable from first insertion and its exact bound
request must survive terminal/recovery. At this checkpoint the code refused the
old Lexical receipt/terminal path for Native rows. Authenticated Native endpoint/
resource custody, SDK staging/Submit and typed Native terminal cold readback
were then unimplemented. The ordinary host selected the existing publisher;
these replay fields alone did not establish a Native deployment switch.

Current recovery/producer work is owned by [W2-02](../plans/oct-17-index-closeout/tickets/W2-02-pending-recovery.md) and [W2-05](../plans/oct-17-index-closeout/tickets/W2-05-producer-full-delta.md). [RFC §23.65](../history/oct-10-index-closeout/RFC-CHECKPOINTS-2026-10-21.md#2365-f07-durable-native-preparation-and-write-once-request-custody-2026-10-13)
and [VALIDATION](../plans/oct-10-index-closeout/VALIDATION.md#2365-durable-native-preparation-and-write-once-request-checkpoint-2026-10-13) preserve historical context and source-scoped results.
Do not promote static source checks or the Quanta scalar/capture tests to
producer restart, existing-state cutover or Native performance qualification.


### Producer Native endpoint selection and receipt custody

The current F07 host composition accepts an explicit optional Native UDS path.
The host admits and retains its own path backing; publication callers borrow
that path. Never derive this endpoint from the legacy SearchPlane state root.
Same-key host revalidation includes exact endpoint equality. The SDK endpoint
entry can retain an explicitly supplied socket, but the actual producer does
not yet deliver an admitted per-attempt Native target. Endpoint configuration
alone does not enable Native source publication.

Native selection and durable V9 preparation must be supplied together. Recovery
uses the original V9 preparation and bound request. `status(operation)` alone
cannot authenticate the request; replay exact Submit to cross the immutable
request-identity check before minting a Native terminal receipt. Generic Legacy
receipt recording refuses a Native Lexical slot. Dedicated recording verifies
full Native receipt bytes before replay CAS; later Terminal/ACK/read admission
revalidates the original V9 context and writer-owned bytes.

The admitted one-shot client retains request-frame, raw-body and bounded typed
publication-reply charges through their actual lifetimes. Semantica's Original
adapter admits each backing allocation through the same original control and
observes actual Original execution through its existing source-control
checkpoint. Non-phase lifecycles retain their original absolute deadline;
owned-phase lifecycles retain phase-dependent termination policy. External
observation is cooperative; it does not wake local waiter callbacks.
The socket loop checks it during bounded waits. Optional query ledgers, child
budgets and waiter allocation are outside the initial budget admission contract.

Initial-full source staging borrows source bodies in fixed windows and returns
an independent sealed owner set. It does not Submit. The dispatcher now wires V9 stage/bind/exact Submit for an explicitly attached
target. The actual product caller still needs target delivery and eligible
first-full source authority.
Runtime/Native Large/XL acceptance remains unqualified. Preserve original request
roots and replay data across Unknown outcomes; do not reseal a different request
after binding. Initial-full staging does not implement Delta/deletion or Resync.


The live dispatcher renews the existing V9 lease before bounded staging RPCs
and caps each call to that lease without extending the original request. A
bounded Submit/Resume/exact-Submit/read-view sequence shares one current cap;
expiry retains the immutable original request for a later admitted retry.
Confirmed Refused is terminal refusal, not Unknown. Writer receipt persistence
is followed by a fresh clock sample before authoritative V9 recording.

Lifecycle compatibility code now observes the actual SDK owned phase without
requiring an invented owner bit or static timeout. Its owning Rust proof is
pending. Startup readiness still has no fresh Original execution grant. Do not
restore old keepers from persisted rows or treat query-readiness timeout as
execution authority. Fresh product recovery admission must precede cutover;
Native restart and Large/XL operating/performance acceptance remain NOT_RUN.


### SDK explicit Native selection is not a product activation switch

ApplyChanges now accepts a closed optional Native target and binds its canonical
selection digest into idempotency. Native requires an explicit key. SDKPreSource,
RecoveryRequired and Completed recovery currently refuse Native until exact V9
preparation/binding/terminal evidence is authenticated. Do not replay an ordinary
cached response as proof of Native publication.

The normal Facts route still refuses before Source CAS because its authenticated
pending source carrier is not yet coupled to committed promotion. The existence
of the later capture helper does not enable this route. A genuine Original source
must survive the same CAS and match the committed closure/keeper. Startup still
needs fresh recovery admission; persisted Original keeper resurrection is invalid.

Current scoped proof: Quanta observer/cap tests 56/56 and Semantica ingress tests
126/126 on their recorded source scopes, target codec 1/1 on a separate CAS.
Runtime check FAILED at the PTA genuine-fixture migration; SDK target execution,
product restart and Native Large/XL acceptance remain NOT_RUN. See VALIDATION
§23.68–23.69 for commands and the zero-test selector correction.


The prospective→committed Facts carrier is now wired in source, and the existing
pre-source journal stores checksum-covered Native selection. Cold and in-process
recovery inspect it and refuse before legacy Product restoration. Facts remains
guarded: ordinary mutations select Delta and the current Native source adapter
accepts initial FULL only. Do not lift this guard without eligible first-FULL
selection and Source CAS→V9 recovery coupled to fresh Original admission.
Canonical validation checkpoints were added, but V5 artifact materialization and
sorting still prevent an end-to-end interruption ceiling. Validation §23.70
separates pending pooled checks from earlier scoped GREENs.


Latest component proof at §23.70 is 65 selected Quanta tests and 127 same-CAS
Semantica ingress tests, plus changed API inventory/prompt lint. SDK DTO owning
compilation FAILED in the PTA Python birth feature boundary before executing
its test. That source-authority fix, full Runtime proof and product activation
remain outstanding; these component results are not a cutover receipt.


The §23.71 producer audit removed the proposed Ordinary bridge because it has no
production issuer. Java-only ledger access is feature-gated; unsupported Python
general-expression reads return typed UnsupportedOperation. The genuine
PythonSource literal route remains. Do not treat this as general Python support
or lift Facts admission. PTA general-callable fixtures need actual Original
producer custody, and initial FULL plus committed-source recovery are separate
remaining seams. New owning-run results remain pending until their terminal
receipt; see VALIDATION §23.71.


### F07 V5 progress and Native completion checkpoint

A Native SDK checkpoint must be joined to the same freshly admitted Original
source, exact retained selector, immutable V9 request, typed Native terminal
receipt and persisted ACK. The new Runtime readback uses the existing canonical
ACK oracle; ordinary checkpoint readback explicitly refuses a Native journal.
Cleanup phases are not completion readadmission. Mutation cold gates remain
until Native Delta changed-set authority is implemented.

Initial FULL is an explicit index-only publication from a committed source
baseline. Endpoint-only ensure does not claim Native completion. Fresh Original
entry issuance, exact operation lookup before recollection, and Native terminal
readback must be coupled before that API is accepted. The current additions are
source/static work; owning Rust and product cutover remain NOT_RUN.
Selected Contract/Core/IPC/SDK strict Clippy is VERIFIED, compiler 52.33s; see
VALIDATION §23.72. It is not a Runtime/product/performance receipt.


Initial-FULL producer recheck found a separate correctness seam: an ordinary
ensure can already publish a Legacy Lexical head. The existing aggregate emitter
then selects Delta, freezes that head and carries its delivered source-event
predecessor. These values cannot be relabeled as Native first FULL. The Native
selection must choose a complete Original FULL producer and its real backend
head/event authority before plan/emission. Do not loosen the Native validator,
clear the Legacy head, or claim the new initial API is accepted before coupling
this policy and owning validation.


The subsequent call-path check narrows the initial-head finding: endpoint-only
ensure is not proven to always publish Legacy Lexical. The existing deferred G1
contract provides a genuine NotComputed baseline with no lexical head/outbox,
no completed predecessor and first source-event ancestry. The initial Native
producer requires this exact deferred proof; an already-active Legacy head
refuses before publication rather than being relabeled. Canonical Native target
eligibility additionally requires Advance, event (0,1), and empty owners/paths/
frontiers roots. SourceCut must still match the actual selected Native view.
Fresh entry issuance and pre/post-collection exact V9 attempt inspection are
connected in source. Acked replay authenticates typed Native receipt and persisted
ACK; nonterminal replay fences recollection and remains pending/refused, not a
new request. Automatic nonterminal resume, Delta and Resync remain open.


### F07 captured component verification and later activation boundary

VALIDATION §23.73 records Contract746/746, Foundation208/208 and Kernel
ingress127/127 at one immutable source hash. They prove component behavior,
including observed V5 parity/cancellation; they do not qualify product cutover.
Later Runtime/SDK source couples exact Native ACK to the existing Product reader
visibility owner and resumes only already-bound Dispatching requests using the
existing V9 lease/terminal owners. Its owning Rust and product execution remain
NOT_RUN. Earlier phases remain pending and Native mutation Delta gates remain.


### F07 recovery admission still required

RFC/VALIDATION §23.74 distinguishes the exact-length file reader repair from
complete Native memory admission. Do not qualify recovery memory from the outer
Original scope: generic CBOR decode, re-encode and repeated member reads remain
unadmitted. Original refusal mapping is repaired in source; new owning tests
have not yet run. The latest SDK owning attempt failed on a missing build-context
import before any test; its repaired retry has no terminal result yet.


### Latest Native launcher and recovery acceptance boundary (2026-10-13)

Current `tools/deployment/launcher.py::serve_selection` chooses serve-native when
selected native_config exists, and `_run_selected_command` requires its immutable
boot receipt. Source installation of this branch supersedes an earlier inventory
that launcher always selected serve. It does not qualify Native state backup,
restore, crash recovery or deployment rollback.

The SDK owning retry is terminal FAILED(exit101, parser type inference, tests0);
current parser fixes require a new captured owning result. Exact Native ACK
reader activation and bound Dispatching source paths are implemented, but their
actual SDK→daemon→search and restart acceptance remains NOT_RUN. Before product
cutover, finish controlled CBOR/read custody, earlier replay phases, Semantica
Delta/delete/Resync and construction-time aggregate physical authority. Preserve both the earlier legacy XL timeout and later07f0430 legacy lifecycle
pass(777.167s); Native Large/XL and formal performance remain separate.


The original full scope still includes Native fresh-root rebuild/reselection and default cutover, actual
Semantica producer FULL/Delta/delete/Resync, physical constructor-time admission,
ANN/graph/IDE/Auto and integrated operating/release acceptance. The 45% requirement
tally cannot authorize default rollout or retire legacy lifetime owners. Original
A/D/C5 repairs and the later legacy XL lifecycle pass remain scoped VERIFIED.


### Native selection and restart (2026-10-14)

The Rust binary and deployment launcher now share the same selected Native
command contract. Qualification results are recorded in
[VALIDATION](../plans/oct-10-index-closeout/VALIDATION.md), separately from source
implementation. Native remains opt-in; the legacy `backup-state` and
`restore-state` commands do not support the Native root.

Use a fresh root rebuilt from the authoritative producer for format changes.
Do not migrate an unsupported root in place. After the intended operation is
Published and the candidate process has stopped:

1. Run `select-native-state --config ABS_CONFIG --manifest-operation OPERATION_JSON
   --receipt ABS_RECEIPT`. The receipt parent must be private, owned and outside
   the state root; the target must not exist. `--config-digest SHA256` may pin the
   exact config bytes during selection.
2. Run `python3 tools/deployment/launcher.py install-native --selection ABS_SELECTION
   --binary ABS_BINARY --state-root ABS_ROOT --config ABS_CONFIG --receipt ABS_RECEIPT`.
   The launcher hashes the actual config and binary, runs the real read-only
   verifier, then durably replaces its selection. Candidate verification failure
   preserves the prior selection.
3. Run `python3 tools/deployment/launcher.py serve --selection ABS_SELECTION`.
   Selected serve checks the pinned config, Catalog-issued identity and physical
   request/profile/SourceCut closure under the same root lease used by the service.
   It does not require the installation-time Catalog digest to stay constant
   after legitimate updates.

Select may commit Catalog selection before receipt creation fails. A failed
command is not a successful installation. Retry selection with an absent receipt
path after resolving the reported error; never overwrite an uncertain receipt.
Receipt publication failures remove only the newly created inode when cleanup
can be confirmed, and explicitly report uncertainty when it cannot. Installation
always independently verifies the result.

A final directory-FD close failure can report postcommit uncertainty while a
durable receipt remains. Inspect the reported outcome and retained receipt before
retrying; absence is not guaranteed. Do not interpret that error as permission
to overwrite or automatically install the uncertain result.

To roll back, stop the running candidate first. If the old root has changed
since its installation receipt was issued, run `select-native-state` against the
old root with a new receipt path, then `install-native` with that new receipt.
An old installation digest is deliberately rejected after Catalog updates.
Reissuing a receipt preserves the existing Catalog root incarnation; it does not
copy, restore or validate a backup. Launcher directory-fsync uncertainty requires
inspection of the durable selection before choosing a subsequent action.

The selected boot binding preserves the original request, profile and source
roots. It does not pin retired historical query projections or certify
Semantica FULL/Delta/delete/Resync, ANN/graph/IDE, sustained resource limits,
Native Large/XL, default cutover or release qualification.


### Native cross-root switch and explicit rollback — 2026-10-14

Use the existing authoritative producer to build a fresh root through the
Native SDK, then stop the producer and daemon. Do not copy the old Catalog,
objects or indexes. The Semantica Original FULL SDK/replay producer remains
its existing owner; this launcher is a verified deployment consumer, not an
alternative source collector. Producer source/coverage/frontier parity must
pass before a product/default cutover is qualified.

1. Issue a Catalog receipt for the candidate's actual Published operation with
   `select-native-state --config ... --manifest-operation ... --receipt ...`.
2. Ensure the currently selected root is stopped and its install receipt is
   current. After accepted publications, reissue/reselect its receipt using
   the existing same-root install path. A stale receipt is refused.
3. Run `python3 tools/deployment/launcher.py inspect-selection --selection
   /absolute/deployment/selection.json`. It prints only mode, root and canonical
   `selection_sha256`, without provider/environment secrets.
4. Run `switch-native --selection /absolute/deployment/selection.json
   --expected-selection-sha256 <inspected-digest> --binary /absolute/searchd
   --state-root /absolute/fresh-root --config /absolute/fresh-config.json
   --receipt /absolute/fresh-receipt.json`, with the existing explicit `--env`
   settings. Both selections must be Native, belong to the same workspace,
   have independent nonces and distinct non-nested physical roots. The one
   deployment lock remains held through old/candidate Verify and publication.
5. Serve through the existing selected launcher, then verify the producer's
   query/coverage/frontier gold and restart behavior.

Rollback uses the same switch command targeting the retained old root with its
fresh receipt and the newly inspected current selection digest. It is an explicit
offline selection change; it does not replay updates accepted only on the newer
root or infer equal SourceCuts. A candidate/current verification refusal leaves
selection publication untouched. If rename may have occurred, PersistenceUncertain
requires inspection of the durable selection, rather than blindly retrying the
old CAS or automatically publishing a rollback. Native cross-root plain install
is refused so it cannot bypass CAS. Initial install and same-root reselection
retain their existing paths. Legacy-to-Native default transition is gated by
W2-19 A; Native backup/restore remains unsupported.

Same-UID processes operating outside the cooperative deployment lock remain
outside this launcher contract. Root verification uses the actual root lease
while the verifier runs; it is not a claim that the launcher retains both root
leases after the verifier exits. The selected Serve reauthenticates its own root.
