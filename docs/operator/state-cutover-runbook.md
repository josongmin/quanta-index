# State Cutover Runbook (S21-11 / S21-12)

Current owners: [E06 cutover](../plans/oct-22-finalizing/E06-sdk-service-cutover.md)
and [E08 qualification](../plans/oct-22-finalizing/E08-integration-release.md),
under the [Oct-22 closeout](../plans/oct-22-finalizing/README.md).
Source-scoped results remain in [VALIDATION](../plans/oct-22-finalizing/VALIDATION.md).

This runbook owns procedures and safety contracts. Dated progress/implementation statements below are historical checkpoints, not a live task queue or current-source verdict. Do not recreate work or infer current completion from their percentages, diagnostics or pending lists.

Offline state backup/restore and the RepoMap single-mutation
producer cutover. Offline backup/restore commands require the daemon fully
stopped. Native SDK inventory recovery uses the running Native service.

## Current command and format authority

The current release executable serves only an admitted Native configuration
with `serve-native`; `serve` and the old offline state commands return exit 70.
The Native marker and
layout refusal are defined by the [runtime](../../crates/quanta-index-searchd-runtime/src/native.rs),
and the catalog stamp is defined independently by the
[Catalog](../../crates/quanta-index-catalog/src/native/authority/capture_format.rs).
Use the selected binary and its matching source to determine the current format.
Prior Native markers require an explicit rebuild or a new incarnation.

## Historical cutover checkpoint (2026-10-12)

The v12/capture-v2 marker and `bb7a8bf2` legacy workflow's 59-test result were
source-bound checkpoints. Their exact narrative is in the
[recovery index](../ARCHIVE-INDEX.md#2026-11-12-public-usage-and-operator-consolidation).
They do not qualify Native backup/restore, default selection or a newer format.
Use the current runtime/Catalog format authorities above and the
[Native transition boundary](#native-cross-root-cutover-and-rollback-boundary--2026-11-12).
Do not remove markers, relabel roots or reuse old-format capture claims.

## Native producer inventory recovery

The running Native service exposes
[producer-owner paging](../../crates/quanta-index-sdk/src/native_uds.rs)
through `list_producer_source_owners(selection, producer, after, limit, budget)`.
The first page may select Latest; continuations use `Exact(page.view)` and
`page.next_after()`, with limit 1..128. Persist the recovered view/source and
prior epoch with the reconstruction attempt. Publication or epoch changes
return Conflict; restart from a current first page. Historical inventory
enumeration is refused; historical known-key lookup remains separate.

SourceResync must include every known producer owner as unchanged, replacement
or deletion, and independently discover current/new files. It seals mode with
the original operation and switches epoch only with the complete baseline,
view and receipt; pending resync does not expose the new epoch. Source events
start at ordinal one independently of operation attempts. Paging is recovery
input, not proof of actual V9 delivery or supported Native backup/restore.
[Original API/verification scope](../history/oct-10-index-closeout/RFC-CHECKPOINTS-2026-10-21.md#2363-f07-paged-producer-inventory-and-epoch-recovery-2026-10-13)

## Offline staging custody

Backup/restore creates its derived staging directory exclusively. An existing
directory, file, symlink or dangling symlink is typed `InvalidRequest`; it is
neither reused nor removed. The destination must also be absent, including an
empty directory. These checks precede staging writes.

Interrupted or source-drifted staging remains unpublished. Inspect its contents
and custody before explicitly cleaning an operator-owned root, or choose a new
absent destination. A derived name or sealed manifest establishes neither
ownership nor activation. Automatic path-based recursive cleanup is unavailable.
Native fresh-root rebuild, old-view/replay identity and supported recovery
acceptance remain in E06/E08 and the Native transition contract below.

## Offline state workflow (S21-11)

This section records the retired Legacy generation workflow for interpreting
old roots and receipts. Its `backup-state`, `restore-state`, and `verify-state`
commands require the matching archived old-Q executable; the current release
binary refuses them. Native rebuild/recovery and cross-root acceptance follow
the explicit Native boundary below.

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

The RepoMap V1→V2 and semantic format11/build-contract3 observations were local
source checkpoints, not a paired deployment receipt. Their exact pre-compaction
body is recoverable from Git as recorded in the recovery index.

Legacy semantic derivation consumes typed semantic-source scopes, including
explicit `RawCodeFallback`; retired chunk-text fallback and
`QUANTA_INDEX_SEMANTIC_DERIVE_MODE` are not live alternatives. An empty scope can
mean unchanged semantic input, so empty vectors alone do not prove complete
producer coverage. Independent producer and exact-pair tests must establish
that invariant. Prior-format generations require typed rebuild, not relabeling
or the retired `migrate-state` importer. The
[Legacy semantic ADR](../adr/MAY-31-001-lancedb-semantic-generation-authority.md)
and [Native ADR](../adr/NOV-12-001-native-query-and-semantic-engine.md)
retain their separate backend and acceptance scopes.

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

Current recovery/producer work is owned by [W2-02](../plans/oct-22-finalizing/E02-publication-recovery.md) and [W2-05](../plans/oct-22-finalizing/E01-producer-generation.md). [RFC §23.65](../history/oct-10-index-closeout/RFC-CHECKPOINTS-2026-10-21.md#2365-f07-durable-native-preparation-and-write-once-request-custody-2026-10-13)
and [VALIDATION](../history/oct-22-finalizing/oct-10-index-closeout/VALIDATION.md#2365-durable-native-preparation-and-write-once-request-checkpoint-2026-10-13) preserve historical context and source-scoped results.
Do not promote static source checks or the Quanta scalar/capture tests to
producer restart, existing-state cutover or Native performance qualification.


### Producer Native endpoint selection and receipt custody

The current F07 host composition accepts an explicit optional Native UDS path.
The host admits and retains its own path backing; publication callers borrow
that path. Never derive this endpoint from the legacy SearchPlane state root.
Same-key host revalidation includes exact endpoint equality. The SDK endpoint
entry can retain an explicitly supplied socket. Endpoint custody and the
admitted per-attempt Native target are separate inputs; actual publication
must carry and authenticate both. Endpoint configuration alone does not enable
Native source publication.

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
target. The actual product caller must carry that target with eligible
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

Lifecycle compatibility observes the actual SDK owned phase rather than an
invented owner bit or static timeout. Startup readiness is not a fresh Original
execution grant. Do not restore keepers from persisted rows or treat query-readiness
timeout as execution authority. Fresh recovery admission must precede cutover;
current restart, installed recovery and performance results belong to NC-D/F.


### SDK explicit Native selection is not a product activation switch

ApplyChanges' closed optional Native target binds the canonical selection digest
into idempotency and requires an explicit key. SDKPreSource, RecoveryRequired
and Completed recovery must authenticate exact V9 preparation/binding/terminal
evidence before treating a Native journal as complete. An ordinary cached reply
cannot prove Native publication.

Facts promotion requires a genuine Original source, the same Source CAS and
committed closure/keeper. The admitted target and actual FULL/Delta/Resync
capability must match that producer; endpoint configuration or source staging
alone cannot authorize publication. Startup recovery needs fresh Original
admission, never resurrection of a persisted keeper. Current producer/caller
and installed acceptance are owned by [E02](../plans/oct-22-finalizing/E02-publication-recovery.md)
and [E06](../plans/oct-22-finalizing/E06-sdk-service-cutover.md).

The following are dated F07 results, not a current unfinished-work inventory:
§23.68–23.69 recorded Q56/56·S126/126 and separate codec1/1; §23.70 recorded
Q65·S127 component bodies. Runtime/SDK attempts failed before bodies in their
PTA feature/birth boundary and did not qualify product restart or Large/XL.
§23.70's carrier/journal source wiring superseded the earlier missing-carrier
observation. §23.71 removed a proposed Ordinary bridge without a production
issuer and recorded feature-gated/typed-refusal Python limits. Those historical
limits and pending counts do not define today's capability matrix or new tasks.
Use the owning source and current acceptance packet before changing admission.

### F07 V5 progress and Native completion checkpoint

A Native SDK checkpoint binds the same fresh Original source, retained selector,
immutable V9 request, typed terminal receipt and persisted ACK. Ordinary
checkpoint readback refuses a Native journal; cleanup is not completion.

Initial FULL uses a committed source baseline and complete Original producer,
not endpoint-only ensure or a relabeled Legacy head. The deferred G1 baseline
has no lexical head/outbox or completed predecessor. Native initial eligibility
requires Advance, event (0,1), empty owners/paths/frontiers and SourceCut matching
the selected Native view. Couple fresh entry issuance with exact attempt
inspection before and after collection. Bound nonterminal replay fences
recollection; it does not become a new operation. Actual FULL/Delta/delete/
Resync and later replay acceptance follow the existing E02/E06 owners.

### F07 captured component verification and later activation boundary

The historical Contract746/746, Foundation208/208 and Kernel ingress127/127
results shared one immutable source. They proved component V5/cancellation
behavior, not product cutover. Exact Native ACK, actual SDK→daemon visibility,
bound Dispatching replay and restart still need their matched owning/product
scope; older component counts do not supply it.

### F07 recovery admission still required

Recovery must admit raw/decoded memory, CBOR decode/re-encode and repeated
member reads under their actual request and retained custody. An enclosing
Original scope alone is insufficient. The old exact-reader fixes, compiler
errors and body-zero SDK retries are historical records, not current repair
assignments. Current source and acceptance belong to
[E02](../plans/oct-22-finalizing/E02-publication-recovery.md) and
[E06](../plans/oct-22-finalizing/E06-sdk-service-cutover.md).

### Latest Native launcher and recovery acceptance boundary (2026-10-13)

This heading retains the dated launcher/SDK checkpoint. Its branch selection,
compiler failures and legacy lifecycle pass are source-scoped history. Current
selection, producer fencing, rollback refusals and restore-forward limitations
are in the Nov-12 transition contract below. Original FULL/Delta/delete/Resync,
physical admission, old-view recovery, ANN/graph/IDE/Auto and operating/release
requirements remain in the [full-product DoD](../plans/oct-22-finalizing/DOD.md).
No percentage tally or local pass authorizes default rollout.

### Native selection and restart (2026-10-14)

The Rust binary and deployment launcher now share the same selected Native
command contract. Qualification results are recorded in
[VALIDATION](../plans/oct-22-finalizing/VALIDATION.md), separately from source
implementation. The historical Legacy `backup-state` and `restore-state`
commands do not support the Native root. Current release serving uses Native only.

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


### Native cross-root cutover and rollback boundary — 2026-11-12

Use the existing authoritative producer to build a fresh root through the
Native SDK, then stop the producer and daemon. Do not copy the old Catalog,
objects or indexes. The Semantica Original FULL SDK/replay producer remains
its existing owner; this launcher is a verified deployment consumer, not an
alternative source collector. Producer source/coverage/frontier parity must
pass before a product/default cutover is qualified.

The launcher currently refuses `rollback-legacy` and Native-to-Native
`switch-native`. BOOT verification cannot prove that a separate producer has
no durable undelivered work, or that no publication occurred after cutover.
Reissuing a BOOT receipt does not lift these refusals. Plain same-root Native
installation preserves the exact first installed BOOT receipt and refuses a
replacement receipt before invoking verification. Its operation, object,
SourceCut, installation attestation and root incarnation cannot be refreshed
into a new rollback baseline.

The following
Legacy-to-Native path retains the FULL handoff and selection CAS checks; it
does not qualify a product/default cutover without the required integration
and producer parity gates.

1. Stop producer admission, settle accepted work, and stop/join the daemon.
   Keep the same deployment directory for all cooperating launcher operations.
2. Issue a Catalog receipt for the candidate's actual Published operation with
   `select-native-state --config ... --manifest-operation ... --receipt ...`.
3. Run `python3 tools/deployment/launcher.py inspect-selection --selection
   /absolute/deployment/selection.json`. It prints only mode, root and canonical
   `selection_sha256`, without provider/environment secrets.
4. Preserve the exact Legacy head with `preserve-legacy --selection
   /absolute/deployment/selection.json --previous-selection
   /absolute/deployment/previous-legacy.json --expected-selection-sha256
   <inspected-digest>`.
5. Run `switch-native --selection /absolute/deployment/selection.json
   --expected-selection-sha256 <inspected-digest> --binary /absolute/searchd
   --state-root /absolute/fresh-root --config /absolute/fresh-config.json
   --receipt /absolute/fresh-receipt.json --previous-selection
   /absolute/deployment/previous-legacy.json
   --expected-previous-selection-sha256 <preserved-digest>
   --target-operation <producer-target-json>
   --published-operation <producer-published-json>
   --published-source-cut-digest <producer-published-digest>`, with the existing
   explicit `--env` settings. The operations and SourceCut digest must agree
   with the candidate BOOT receipt. The roots must be distinct and non-nested.
   The deployment lock remains held through candidate Verify and publication.
6. Serve through the existing selected launcher, then verify the producer's
   query/coverage/frontier gold and restart behavior.

Enabling backward selection requires the canonical offline Catalog reader and
the real producer's fenced outbox authority. With producer admission closed and
the daemon stopped/joined, retain the producer fence through the deployment
lock, both stores' identity/high-water/replay-floor checks, selection CAS and
directory fsync. Require no publication after the durable cutover baseline and
no pending or undelivered work in either store. Missing, foreign, cancelled,
expired or uncertain evidence refuses selection publication. A client ACK is
not the Catalog commit point; lost ACKs must settle the original operation's
durable receipt. A caller-supplied approval or manually authored receipt cannot
substitute for these authorities.

### Installed offline recovery

Current implementation and installed qualification are tracked in the
[NC-D work packets](../plans/nov-12-native-cutover/tickets/NC-D-boot.md#current-work-packets-20261115).
The committed FULL graph builder, paid Java inventory lender, Java DFG visitor,
initial ordinary Source/V9 binding, Native RepoMap/FD6 evidence wiring and
normal Exact-query/ACK-loss fixture are present in current source. Reuse those
producer and caller paths. Current paired-source compiler/owning bodies and
post-publication coverage/restart acceptance remain with NC-D/F; code presence
alone is not signed installation proof. Fresh Store/mixed Attach repairs apply
only when that source creation path is used. Signed Linux normal recovery is NOT_RUN.

The installed Semantica resident invokes Q `recover-native` with its pinned
helper image, retained Host and SDK locks, and sealed Source evidence. Use
the installed bundle and a resident configured with
`--native-recovery-helper-executable`, `--native-recovery-helper-sha256` and
`--native-recovery-helper-memory-bytes`. Stop and join the Q serving process
before the request so it releases the deployment lock.

- When the Legacy publication uses a separate RepoMap process, preserve that
  process's actual schema-2 Legacy selection in the active deployment directory.
  In the Native config, set `legacy_repomap_selection.path` to the absolute saved
  selection path and `legacy_repomap_selection.sha256` to the 32 bytes of its
  canonical `inspect-selection` digest. Pin the resulting Native config before
  preparing or selecting it. Stop and join the separate RepoMap process as well.
- For `rollback-legacy`, use the original saved Legacy selection in the
  active deployment directory. Use the selected Native root's config and
  the producer's original Native FULL sequence.
- For `restore-forward` from Legacy, run `build-native-candidate` with the
  active selection digest, a separate candidate control directory, the pinned
  Q binary and config, and the installed S server image and SHA256. After
  `--`, pass the S startup options for the existing Legacy repository and the
  Native namespace, the expected Legacy snapshot, and the configured Q
  profile, control operation and object reference. Set `--producer-env` to
  select the same durable SDK store as the Legacy resident. The launcher starts
  Q, runs installed S FULL, and persists the FULL handoff. It records a clean
  drain after Q exits and is reaped, then seals the AST only if it matches Q's
  current view. It returns `candidate_path`, `candidate_sha256` and
  `original_full_sequence` for the recovery request. When the FULL handoff
  exists but the clean-drain record is absent, rerun the same command with the
  same inputs. The launcher reopens and drains Q before attempting BOOT with
  the retained FULL sequence.

Obtain both canonical selection digests with `inspect-selection`. Submit the
request through the attached workspace session:

```sh
quanta --workspace "$repo_root" --server-socket "$resident_socket" \
  --runtime-session-id "$session" \
  daemon native-offline-cutover --mode "$mode" \
  --selection-path "$selection" --expected-current-sha256 "$current_sha256" \
  --candidate-path "$candidate" --expected-candidate-sha256 "$candidate_sha256" \
  --config-path "$native_config" --config-sha256 "$native_config_sha256" \
  --helper-executable "$helper" --helper-executable-sha256 "$helper_sha256" \
  --original-full-sequence "$original_full_sequence"
```

Restore-forward from a selected Native root or a different Legacy SDK
snapshot requires the producer's old-head replay coverage. Installed Linux
acceptance remains open in
[NC-D](../plans/nov-12-native-cutover/tickets/NC-D-boot.md).

For a fresh operation, require `Applied`, inspect the active selection and
confirm the actual change to the candidate digest. When Q completed the original
publication, a same-operation retry after response loss requires `Reconciled`
with that publication identity and no additional publication. Restart Q through the selected launcher
and verify the expected source and query results. The resident requests
`PrepareRestart` after cutover. Observe its exit and socket cleanup before
starting the resident against the selected endpoint and obtaining a new session.
Confirm the committed Source pointer, manifest and snapshot remain the intended ones.
If the response was lost
or reported `PersistenceUncertain`, inspect the selection first. To
reconcile that operation, submit the original request fields, including the
original current digest and FULL sequence.
An unfinished request can retry publication from the original head only while
the original selection file version remains in place. A republished file with
the same canonical digest is a different selection version. The resident
retains the original version in its recovery journal. The Q publisher records
a recovery publication attribute on the temporary selection before file fsync
and rename. Candidate reconciliation requires the attribute for the same
request, original version and selected inode. The deployment filesystem must
support file extended attributes. Earlier intents without a recorded file
version or matching candidate attribute require operator inspection after an
uncertain response.

After rollback, prepare a fresh Native candidate and derive a new recovery
request from its exact FULL/BOOT/config identity. The completed first forward
request remains bound to its original selection version. Verify the full
forward, rollback and fresh-forward cycle with selected queries and restarts.

Native cross-root plain install remains refused. Initial install and same-root
reselection retain their existing paths. Legacy-to-Native default transition
remains gated by W2-19 A.

Same-UID processes operating outside the cooperative deployment lock remain
outside this launcher contract. Root verification uses the actual root lease
while the verifier runs; it is not a claim that the launcher retains both root
leases after the verifier exits. The selected Serve reauthenticates its own root.
