# NC-H · History source custody and Native adapter

<a id="current-nc-h-status--2026-11-21"></a>
## Current NC-H status · 2026-11-21

- The existing local lanes verified Q Runtime 4/4, Q UDS retry 1/1,
  S Original replay 2/2, S CLI compilation and S production Runtime
  compilation. The [owner results](#current-actionable-backlog--2026-11-16-re-audit)
  bind each check to its recorded inputs. The current S Legacy V2 source
  matches the module that passed 7/7 on macOS. Current S Linux CLI and
  signed installed acceptance remain deferred. The separately supplied
  [Source/History Resync](#source-epoch-replacement-and-history-publication)
  requires affected Q and S test execution.
- The Native History and Legacy checkpoint readers now decode framed JSON
  without building a temporary untyped body tree. The Q decoder reuses the
  Source JSON prepass and the existing History typed body visitor. The
  Legacy checkpoint reader borrows the body slice from the admitted frame
  until its kind is available. Source JSON and the generic History serde
  compatibility path retain their existing wire behavior.
  [Framed JSON verification](#history-export-frame-json-budget--2026-11-23).
- A follow-up Q Catalog audit found a missed single-workspace path in
  `boot_quiescence`: BOOT, Source-root readout and rollback could skip the
  global registered-row/orphan-root scan when no protected route was supplied.
  The Q source now checks the existing ingress/foreign-workspace refusal
  first, scans registered rows before operation scratch, and uses the same
  canonical guard as protected routes. The new single-workspace orphan-root
  regression and eight existing H Catalog selectors **VERIFIED 9/9** in
  Nextest `61d2f67d-6a74-4215-b711-c76aad391735` on a 382-file
  Catalog/Contract/Core/IPC and Cargo closure whose SHA-256
  `ae4faf2fb36ce1a33d7c28988c1cc91f32657d643104531ccd47815f562e8d51`
  matched before and after. Compilation took 8m 34s and the nine bodies
  passed in 10.097s, with 403 filtered.
  This local Q closure includes the new guard and is not F/Root's previously
  pinned Q 49/49 source. Signed acceptance must bind a successor containing
  this Catalog change and recheck affected evidence on that source.
  Q HEAD moved from `df090e56a72f` to `d8a490565` during the audit; the
  Catalog proof binds its selected source bytes rather than a moving HEAD. S
  HEAD also moved while its H code files stayed clean. That nine-body result
  predates the additional pin-integrity change below.
- The follow-up static Catalog audit found that a validly digested but
  noncanonical workspace encoding in a pin row could evade exact-byte pin
  queries during view/work retirement and read settlement. Catalog open now
  validates persisted pins in the initialization transaction. View retirement,
  terminal work retirement, held-ticket release and settlement use the
  workspace-first index after checking the connection-local SQLite data
  version in their own transaction. A change from another connection requires
  a new budgeted validation of that snapshot before it can certify pin absence.
  Ticket issue encodes canonical keys, and renewal and release preserve that
  invariant. Point reads and dead-session recovery still check row shape and
  digest. Digest and workspace verification stream their encodings and
  preserve the v2 digest bytes. New regressions cover 641 unrelated pins
  under a 640-read page allowance and across reopen, a held read snapshot
  followed by revalidation and release rollback after an external change,
  persisted digest equivalence and scratch refusal. Catalog open preserves
  the existing engine error mapping for SQLite capacity and held locks.
  A host SQLite WAL probe confirmed the snapshot transition and that PASSIVE,
  FULL and TRUNCATE checkpoints on the owning connection keep its version.
  Catalog4 compiled and the three focused pin bodies **VERIFIED** in the
  matching-input run recorded below.
  [Pin validation](../../../../crates/quanta-index-catalog/src/native/authority/lifetime.rs#L499)
- The follow-up S Native→Native audit found that an empty History target list
  bypassed the old Q inventory before sealing the candidate. The NativeSelected
  path now requires the committed repository's selected Native inventory to
  complete with count zero; nonempty targets must all belong to that repository.
  The frame-level regression accepts only zero completion for an empty target
  list and rejects a History member. The empty target reserves one inventory
  slot rather than the 4,096-slot ceiling; a low-memory regression checks the
  grant and refund while preserving the full bound for nonempty inventories.
  The existing Source-only scenario now builds a second Native candidate from
  an empty persisted inventory and checks its exact profile and protected
  Source transfer. The History scenario requests an empty target list against
  two retained generations, requires inventory refusal and verifies the old
  selection, receipt and readback. S Rust compilation and current installed
  bodies are **NOT_RUN**. [S receiver](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/history_native_v1.rs#L239) · [installed scenarios](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/tests/server_socket_daemon_e2e/native_offline_recovery_v1/scenario_v1.rs#L1049)
- S commit `e1da1279713` preserves an already received resident FULL terminal
  when connection shutdown fails. The forwarder still validates request ID,
  terminal status and the unique handoff row. Current-source execution is
  **NOT_RUN**. [Resident forwarder](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1.rs#L567)
  Successor `d99ed16f76b` classifies ACK and AST witness failures after FULL
  as published handoff failures and carries an unknown publication outcome
  through resident delivery with the original SDK error in custody. The
  installed-Q custody fixture distinguishes a pre-Source refusal from a
  continuation error after Q has supplied Source, AST and ACK. The transport
  cancellation and deadline regressions exercise terminal delivery. Direct
  coverage of the CLI ACK and AST witness failure branches remains unverified.
  `b17c0fd9aaf` also checks the request stop before the first Q effect and adds
  typed profile reconciliation. `e042dc6395c` propagates the typed polling
  failure through the profile path. Readback of S `9f30ca0ba64` found the
  candidate and profile files clean, with a separate resident cutover change
  still in the worktree. The existing H lane has no succeeding CLI result.
  Capture the resident, profile and transport inputs together for
  current-source proof.
  [Installed-Q custody fixture](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/indexing_api/native_initial_full_failure_v3/installed_q_custody_tests_v1.rs#L139)
- S `2fa4d58ebbd` binds the candidate profile to the installed Q root, config
  and observed root nonce. `c12079a0be0` checks the nonce again before FULL.
  The installed-Q custody fixture now requires an exact root-incarnation
  refusal for the same live peer and profile with a different root nonce.
  Current-source execution is **NOT_RUN**.
  [Candidate authority](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/indexing_api/native_q_candidate_authority_v1.rs#L144)
- The repaired Q SearchPlane History selection passed 43/43 in Nextest
  `d45e4764-a2fa-415c-9e0c-4bddc5e867d2` on its recorded worktree. The
  Catalog orphan-root/BOOT 2/2 result `a6dcf064-be31-4c9e-b4ce-ed2c3f5b30ec`
  belongs to an earlier source cohort. The current Catalog inputs changed
  afterward; those two bodies and closeout 03.7's six distinct protected-route,
  Source quiescence and History export bodies now **VERIFIED 8/8** within the
  nine-body run above. The new single-workspace regression is the ninth body.
  The separate Catalog 19/19 closeout selection does not include these nine.
  The focused replay regression now records the later Commit backing grant,
  the subsequent DiffHunk refusal and the refund to the still-funded
  original input. The changed body **VERIFIED** in the focused run below.
- The History adapter cases now mount on the no-default
  `process_readiness_owner_v1` target. Its JSON FULL/DELTA and reopen case
  **VERIFIED** 1/1 in Nextest `5a54ca77-a6a9-4b36-a1bf-d553d2d62382` on
  a captured 3,264-file Q source. The History fixture, query and replay
  leaves checked in this audit match that capture. A first whole-input scan
  found 96 differences from the live worktree and a second found 97 as shared
  edits continued, including the test mount. The earlier capture ran only
  that one body. The later [owner results](#current-actionable-backlog--2026-11-16-re-audit)
  record the 4/4 local sweep, including large Original DELTA, compact
  Original FULL/DELTA and reopen, and replay-memory retirement.
  An intervening Runtime4 request entered
  Cargo, but selected mount, SearchPlane,
  Kernel and Contract inputs changed during its 42-minute dependency build.
  The owned diagnostic run was stopped with zero bodies; its wrapper exit 126
  after SIGINT is not a Rust compiler or test failure. A disposable fixed-HEAD
  worktree captured 742 dirty/untracked paths with matching pre/post live hash
  `510f16b4b14fa40bda7b770507b21d033eb748ab941a5af8d6941ec940daac72`.
  That frozen capture was not executed: a new checkout has a cold target,
  internal free space fell to about 43 GiB, the external SSD was absent, and
  other work held Q admission. The disposable snapshot was removed.
- The S CLI owner request uses `history-source-readback`. An earlier
  attempt exited 1 before Cargo when `search_plane_query_ports.rs`
  changed during hashing. The later CLI compile passed on its recorded
  frozen inputs. See the [owner results](#current-actionable-backlog--2026-11-16-re-audit)
  for the S CLI, Required DATA and production no-default receipts.
- H-INTEGRATE-01/04/05 installed assertions remain in the existing
  H-ACCEPT-01 fixture. F/Root verified Q 49/49 and the selected Legacy and
  Native Linux ELF inputs. The S shipping and private late-peak Linux checks
  passed on their recorded frozen source; a retained postbind successor is in
  the existing Root queue.
  Signed FULL → successor DELTA → restart, ACK-loss retry, cross-revision
  and Q-only SQL snapshot fallback bodies are **NOT_RUN**. No production
  History root, complete Original inventory and terminal ACK set was supplied
  for a real-data preservation claim.
- H-INTEGRATE-02/03 have supplied source and existing candidate retry
  assertions. The signed History fixture checks ordered FULL and a second
  launcher build returning the same published operation before selection;
  its Linux body is **NOT_RUN**. The Q UDS fixture waits for both Exact refusal
  and deletion of the original Catalog view before restart and original Submit
  retry. Its earlier 1/1 result predates the current feature boundary.
  Current-source execution is **NOT_RUN**. These are separate acceptance
  boundaries because candidate mode
  disables retention, and selection changes the launcher's authenticated
  selection CAS input. An installed same-candidate retry *after* Exact
  retirement is not a reachable requirement of this preselection fixture.
- The Linux Legacy exporter fixture now checks the closure, emission order and
  object hash per page. Two large rows share an emitted part and retain their
  independent row keys, order, original bytes and the terminal ACK. The
  existing broad Native recipe and `just rust-test-native-history` now validate
  the explicit old-Q executable and SHA-256 before Cargo. The focused recipe
  selects six existing History bodies on Linux, with the offline exporter
  excluded by its platform gate elsewhere. Dry-run, missing-input refusal and
  correct and incorrect SHA checks passed. Linux export and UDS bodies are
  **NOT_RUN** on this source.
  The existing arm64 macOS debug Legacy executable also passed
  `just require-native-legacy-searchd` with the SHA in its
  [local reference manifest](</Users/songmin/Library/Caches/quanta-index/legacy-history-reference-20261118-j7an3chk/reference.json>).
  The signed Linux suite uses the approved Linux ELF input.
- The canonical Q lease probe timed out before entering its command
  (exit 124). The Q cache and external SSD mount path both resolved to the
  internal filesystem with about 14 GiB free.
  On recheck the lease again timed out before Cargo, the internal volume
  fell below 5 GiB free, and macOS listed no external physical disk.
  No matching Q History build was running. Repository Cargo metadata
  resolved the Catalog and SearchPlane libraries and the three Runtime test
  targets with no required features. This checks target registration only.
  Free space later recovered from about 13 to 36 GiB. Two Catalog4
  requests on `test-workspace-lane` exited 124 after lease waits of 30
  and 120 seconds. Another Runtime compile held admission when checked.
  Neither of those two requests reached Cargo.
  At about 49 GiB free, the existing lane admitted Catalog4. Cargo reached
  `quanta-index-catalog` and exited 101 with E0425 for a missing `decode`
  import in the pin test module. Zero test bodies ran. The test now imports
  the existing Catalog decoder. Resubmissions with 180- and 300-second wait
  limits both exited 124 at the shared lease while the Lexical owner was
  active. The 300-second request observed matching content hashes across 429
  Catalog, Contract, Core, IPC and Cargo input paths before and after the
  300-second wait. After the Lexical owner ended, the existing Native
  large-restart body took admission. Catalog4 then exited 124 after 60
  seconds without entering Cargo. The 429-path digest changed during this
  wait. Among these paths, only `native/authority.rs` had a modification
  timestamp in that interval.
  After the F15 owner ended, Catalog4 entered Cargo. The next compiler
  diagnostic was E0716 in the test digest comparison. The test now binds
  the digest before comparing its bytes. Nextest
  `fc6b864a-730b-41de-b51b-1c4ad97b951a` compiled and ran 4 bodies, with 3
  passes and one fixture failure (exit 100). The BOOT negative fixture
  updated all 5 registered route edges while asserting a count of 2. It now
  corrupts the single named view-marker edge. The before/after inputs for
  that run differed in `read_only.rs` and `tests/offline_protected_routes.rs`.
  The three passed bodies are evidence for the built binary.
  The corrected Catalog4 run **VERIFIED 4/4** in Nextest
  `0437c8ab-adb3-4f06-bb76-e87918f486fe` (exit 0, 411 skipped, 4.264s).
  Compile completed in 1m 45s. The 429 Catalog, Contract, Core, IPC and
  Cargo inputs matched before and after at SHA-256
  `da240a8d773374ed8b53538108c91ab16523811098f4c796972e29189c574d31`.
  The orphan-root import and BOOT selection **VERIFIED 3/3** on the same
  input digest in Nextest `27ea2ba1-91c3-4e13-a879-6734654e0a33`
  (exit 0, 412 skipped, 8.466s).
  The focused Q SearchPlane replay request then exited 124 after 240 seconds
  waiting for Lexical at the shared lease. On Nov 22 a later request entered
  Cargo and exited 101 at `-D unused-results` in the test's reservation
  ledger. The test now binds the previous live byte count returned by the
  atomic update. A corrected request exited 124 after 240 seconds at the
  shared lease. The later run **VERIFIED 1/1** in Nextest
  `c7dd3860-c6c5-423d-8bbd-3df4383e18e2` (exit 0, 668 skipped, 0.185s).
  Compile completed in 5m 22s. The 641 SearchPlane, Contract, Core, IPC,
  LQ and Cargo inputs matched before and after at SHA-256
  `f368f35e142786f9354c63836908bf38c02fb5dc307de04d5abebf792532ea95`.
  Later changes touched `Cargo.toml`, `Cargo.lock` and IPC server tests in
  that watched set.
  The Catalog proof remains tied to its recorded 429 inputs. The newer Q
  worktree changed two watched Contract protocol paths for root-scoped views.
  The external SSD returned with about 92 GiB / 4.95% free and the existing
  `history-source-readback` target was available. CLI request
  `c9c34b69ff14f9e7e6fa1dc7743822db` still stopped at source hashing (exit 1)
  when `repo_map_export_bundle/native_digest_v3.rs` changed in the live S
  tree. On Nov 22, the same CLI request with S at `c12079a0be0` stopped
  during Q dependency closure capture. A separate source-only attempt using
  the existing QBC snapshot helper also refused the changing Q sibling
  overlay before issuing an execution root. No current CLI compiler or DATA
  result was issued.
  The separate existing Root 04.2 follow-up exited 143
  while waiting for capacity at its first stage. Its terminal has zero
  executed stages. [Root terminal](../../nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md#L11)
  Use the existing Q lane for the remaining History checks, S CLI and DATA
  in `history-source-readback`, and signed acceptance through the existing
  D/F Root request.
- Session hardening removed a second protected rollback quiescence scan in
  the same Catalog snapshot. The route validator authenticates the RepoMap
  members and route registration and edges before invoking its publication
  visitor, then shares the validated scan with rollback and Source readout.
  The focused Catalog regression checks one attached-view-root scan and
  verifies that a corrupt registered route edge leaves the visitor untouched.
  The body **VERIFIED** in Catalog4 above.
  [Route validation](../../../../crates/quanta-index-catalog/src/native/authority/boot_manifest.rs#L170)
- A follow-up found that Legacy Source frame serialization reclassified work
  admission refusal and cancellation as a storage error in Source and RepoMap
  callbacks. The exporter now carries `IndexError` out of the frame writer and
  retains its type through the Source visitor boundary. The regression checks
  that both census and final frame admission refusals return `ResourceLimited`
  and that a cancelled request returns `Cancelled`. All three cases must
  refuse before writing any bytes. On Nov 22 the focused library run entered
  Cargo and was stopped during dependency compilation after 248 watched
  paths changed, including `Cargo.toml` and `Cargo.lock`. The owned wrapper
  exited 143 and released admission. The Rust body is **NOT_RUN**.
  [Source frame writer](../../../../crates/quanta-index-searchd-runtime/src/history_checkpoint_export/source_v1.rs#L35)
- The 2026-11-22 follow-up observed Q `d8a490565e6f` and S `c3caf34bfb5d`.
  Across three comparisons, the changed-path counts were 0, 2 and 4 among
  3,025 watched Q paths and 4, 2 and 9 among 43,369 watched S paths. A later
  read found four more mismatches in the recorded Q set and 15 in S as S
  HEAD advanced to `8462134711aa`. The
  existing Q shared lease was held by a Legacy inventory build. The S
  external `history-source-readback` target existed and was unlocked. Its
  source-only capture through the QBC snapshot helper exited 1 with
  `SnapshotSourceDriftV1` for the Q dependency before issuing an execution
  root. The existing CLI and Required DATA requests remain
  `c9c34b69ff14f9e7e6fa1dc7743822db` and `1a60cf98354ccd4ad794fee4cda31a89`.
  Q exporter, S CLI and S Required DATA remain **NOT_RUN** on current inputs.
  Process and signed Linux checks remain deferred.
- The S Required DATA exact request started source capture after a matching
  Q/S source comparison at S `f80c234a0b2a`. QBC exited 1 before Cargo after
  `primary overlay changed during post-copy capture`. By readback S HEAD
  was `5143e2040a3` and candidate and daemon files had further staged edits.
  No execution root or current DATA result was issued. A later bounded
  observation found continued changes to the S source overlay in all five
  intervals and did not start a second DATA request.
- The Q exporter exact unit request waited 600 seconds in the existing
  `test-workspace-lane` while the Legacy inventory build held admission.
  It exited 124 before Cargo. The watched set gained three paths and had
  19 path differences between the before and after manifests, including
  Core, Lexical and Semantic inputs. The exporter body remains **NOT_RUN**.
- During the later S candidate-effect split, a nested `pub(super)` enum
  could not be re-exported across the CLI crate. A minimal `rustc` case with
  the same module boundaries reproduced E0365 and E0603. The enum now uses
  `pub(crate)` and its module remains private. The same visibility case
  compiled after the fix. The full S CLI owner check remains **NOT_RUN**.
  [Candidate terminal visibility](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/candidate_effect_v1.rs#L11)
- Q History restore now reserves the retained generation key and optional
  manifest identity before cloning the cut or scanning rows. Both Native
  map readback and fenced snapshot restore keep the lease in replay custody.
  FULL and DELTA reuse their decoded fence, and replay checks incoming
  generation fields by reference before admission. The new
  `snapshot_restore_admits_generation_identity_before_scanning_rows`
  regression checks a memory denial before any row callback and zero live
  bytes afterward. The focused SearchPlane run selected this regression and
  `source_replay_admits_large_delta_and_refunds_a_partial_refusal`. The
  existing Q lane exited 124 after 60 seconds waiting for admission. Cargo
  and both test bodies remain **NOT_RUN** for this change.
  [Restore regression](../../../../crates/quanta-index-search-plane/src/native_history.rs#L1827)
- The follow-up RR found the same retained identity gap in the public
  `HistorySourceReplayV1::finish` path. A fresh fold now funds repository,
  revision and manifest identity before returning. Same-generation DELTA
  carries the existing lease through `resume`. The SearchPlane regression
  asserts that the completed source owns the exact identity bytes, including
  a nonempty manifest, and releases them with the replay result. The
  focused SearchPlane request exited 124 after 120 seconds at Q admission
  before Cargo. Two watched inputs changed while waiting. The Rust bodies
  for the current edits remain **NOT_RUN**.
  [Replay completion](../../../../crates/quanta-index-search-plane/src/auxiliary_authority/history_source.rs#L376)
- The replay regression finishes FULL after its first ACK, resumes the
  same generation, and applies the larger DELTA. Its successful path
  requires exactly one retained identity grant and zero live bytes after
  disposal. Its low-budget path still checks refund after the later row
  refusal. The existing Q admission owner was still compiling. Both
  source stability samples saw changes. The later sample included Q
  Semantic embedding and S snapshot tooling, so a new Q/S frozen run
  did not start. The updated replay regression, Q exporter, S CLI check
  and Required DATA body remain **NOT_RUN** on the current inputs.
  [FULL and DELTA regression](../../../../crates/quanta-index-search-plane/src/auxiliary_authority/history_source/tests.rs#L306)
- The next Catalog audit found unreserved workspace and digest copies when
  validating, renewing, settling or recovering a persisted pin. The point
  reads and recovery now acquire grants using the pin-integrity scan's
  scratch calculation. The grouped observation carries its original
  request's grant in the queued entry. The regressions require a memory
  refusal to preserve the pin and clock floor, refund scratch, and permit
  retry. The existing Q lane request exited 124 after 120 seconds waiting
  for admission. Cargo and all three test bodies are **NOT_RUN**.
  [Pin read scratch](../../../../crates/quanta-index-catalog/src/native/authority/lifetime.rs#L587)
- Read-session recovery now reserves a nonempty candidate page before copying
  fixed-width identity blobs. The same rows hold the OS locks through the
  retirement transaction. The page lease is released after those locks and
  before retired-lock cleanup. The existing pin regression distinguishes an
  empty page, page refusal, per-pin refusal while the page is funded, and
  successful retry. Its expanded body remains **NOT_RUN**.
  The shared Q lane was still in Legacy inventory dependency compilation.
  A new sample changed Q Semantic and S Runtime/SDK source. The S CLI
  check and Required DATA test still lack a current frozen source capture.
  [Recovery page admission](../../../../crates/quanta-index-catalog/src/native/authority/lifetime.rs#L1273)
- Open-intent, active-session and retired-lock pages now share the Catalog
  recovery buffer admission. Open-intent rows carry their OS probes in the
  same page. If the retired-lock page cannot obtain memory, cleanup leaves
  the tombstone and reports further work while returning the already
  committed cursor and counts. A new regression checks that refusal keeps
  the open intent and that a deferred cleanup retry preserves progress.
  A zero-memory probe of an invalid retired row still requires the storage
  error before cleanup can defer. Q compilation and these bodies remain
  **NOT_RUN** in the existing Q lane.
  [Recovery regression](../../../../crates/quanta-index-catalog/src/native/authority.rs#L16325)
- Recovery now probes active and orphan locks outside SQLite, then reaps
  open intents and retires the selected sessions and pins in one Catalog
  transaction. If a later pin reservation fails, the open-intent deletion
  rolls back with the session changes. The focused regression admits both
  pages before refusing the pin and verifies that retry accounts for both
  progress counters. A preparation with no open intents releases the
  creation lock before the transaction. Retired-file cleanup runs after
  commit. The Q shared lane still held the Legacy dependency build.
  The Q and S source samples changed before a successor capture.
  [Recovery transaction](../../../../crates/quanta-index-catalog/src/native/authority/lifetime.rs#L1409)
- An intermediate S candidate-args change removed the parent imports for
  `GroupActivation` and `InputDomain` while the new Linux child still used
  them. The existing owner corrected the child imports in `fa8127e9d4f`.
  Readback confirms the correction. Current S CLI and signed Linux checks
  remain **NOT_RUN**.
  [Candidate imports](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/candidate_args_v1.rs#L6)
- A stability sample matched Q `d8a490565e6f`, S `a0020c75d2d9` and GQLang
  `f9533c4a7c86`. The subsequent Required DATA exact owner run reused
  `history-source-readback` and exited 1 before Cargo because the Q sibling
  overlay changed during capture. No frozen root or test result was issued.
  Required DATA and the same-source S CLI check remain **NOT_RUN**.
- The focused Native History successor regression now exercises a revision
  and generation change. It refuses the next identity grant with one byte
  less than the live prior state plus the next identity, verifies that this
  is the first successor grant, and checks a funded retry retains only the
  successor identity and current rows. The new body remains **NOT_RUN**.
  The Q lane was still compiling the Legacy inventory dependencies, and the
  latest source sample changed Q Core and S Runtime/CLI input paths.
  [Successor identity regression](../../../../crates/quanta-index-search-plane/src/auxiliary_authority/history_source/tests.rs#L517)
- The live Q lane owner is compiling `boot_inventory_data`. A separate
  Catalog request is also waiting for admission. Its prepared-publication
  quarantine and single-view resume selectors do not include the NC-H pin
  and recovery bodies. Q Runtime and S Runtime inputs changed in the next
  source sample. The NC-H current-source body and compiler statuses above
  remain **NOT_RUN**.
  The Lexical build entered the ordinary SearchPlane library compile.
  Its rustc command selects the library dependency only. Two later samples
  changed Q Catalog or Contract and S SDK or CLI ingress inputs.
- The Lexical run produced its SearchPlane library artifact. The NC-H
  request for three SearchPlane regressions waited in the same existing
  lane while a Semantic request held admission. It exited 124 after 300
  seconds before entering Cargo. The before and after manifests each
  covered 672 paths and differed in `quanta-index-contract`'s
  `native_protocol.rs`. NC-H Rust compilation and the three bodies remain
  **NOT_RUN**. S continued changing during the source sample.
- The S candidate owner moved Native History verification to
  `ExportedHistoryMemberV1` and protected transfer to
  `ProtectedTransferTargetV1`. Both call sites matched the new signatures
  on worktree readback. QBC still resolves the existing S CLI bin check, and
  the new CLI test dependency is present in the working Cargo lockfile.
  The S candidate edits remained in flight, with Runtime and SDK inputs
  changing during source sampling. The Semantic Cargo owner still held Q
  admission. Current NC-H Rust verification remains **NOT_RUN**.
  [History member check](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/history_native_v1.rs#L608)
- The S candidate owner moved Legacy checkpoint staging into
  `checkpoint_stage_v1`. The caller still reaches `stage_v1` through the
  parent re-export. Five candidate files parsed with Rust 2021 rustfmt.
  A minimal rustc probe with the same module nesting compiled the widened
  visibility and re-export. The full CLI check and Required DATA body
  remain **NOT_RUN**.
  [Checkpoint stage](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/history_snapshot_v1/checkpoint_stage_v1.rs#L431)
- S snapshot maintenance now defers optional recycle and cleanup when process
  references cannot be verified. Exact owner capture still requires two
  matching observations of the live Q sibling before sealing it. The Q
  SearchPlane manifest recheck found further changes in `Cargo.toml`,
  `Cargo.lock` and the IPC manifest since the last admission timeout.
  The Semantic compiler still held Q admission.
  [Sibling capture](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/casectl/external_sibling_overlay_v1.py#L78)
- Q `./scripts/cargow --lane test-workspace-lane metadata --locked --offline
  --format-version 1` resolved the current Cargo graph and confirmed the
  Catalog, SearchPlane and Runtime library targets. The SearchPlane graph
  includes `shared/quanta-explicit-fd-process` through IPC and
  `vendor/unicode-normalization-0.1.25` through the text normalizer. Both
  roots were absent from the previous timed-out request's watched manifest.
  The next focused run needs a fresh manifest from the resolved local
  dependency roots. Q Rust bodies and S CLI/DATA remain **NOT_RUN**.
- The next SearchPlane request waited 3,600 seconds behind the Semantic
  owner and exited 124 before Cargo. The 704-path before and after manifests
  differ in Contract `native_protocol.rs` and Core `index/ports.rs`. A
  locked offline metadata refresh resolved 704 paths for the same three
  selectors. Two captures matched before the single retry entered the
  existing `test-workspace-lane` queue with a 7,200-second timeout.
  Compilation and test execution remain **NOT_RUN**.
- The Source exporter request resolved 2,356 Runtime inputs across 41
  roots. Two captures matched before the exact exporter library test
  entered the existing `test-workspace-lane` queue. It uses the same build
  admission lock as SearchPlane. Compilation and execution are **NOT_RUN**.
- The Catalog pin and recovery request resolved 666 inputs across 17 roots.
  Two captures matched before its 11 exact selectors entered the same
  existing `test-workspace-lane` queue. Compilation and execution are
  **NOT_RUN**.
- All three waiting Q requests now include new RepoMap prior-file Contract
  inputs in their live closure. The Cargo manifests and lockfile matched
  the enqueue observations. Initial readback found missing request match
  arms after the wire variant appeared. The owning change then wired Runtime
  dispatch and SDK request/reply pairing, including exact view, instance and
  owner checks. The SDK selects the admitted prior-file reply decoder. The
  reader resolves its member through the pinned Exact view and the Runtime
  retains reply memory through serialization. The Source exporter selector
  is still reachable through the `native_product` module. The three previous
  processes ended with logs showing only admission wait and no final result.
  Locked offline metadata resolved again. A single sequence in the same
  lane starts with Catalog's 11 selectors, followed by SearchPlane's three
  and the Source exporter selector. Compare the admitted and completed
  inputs before crediting any Rust result.
- S `dfe1b082342` classifies candidate profile authority read failures as
  `RequiresReconciliation`. The current `ReadView` and `Status` callers
  use the helper, preserving the unknown outcome of an earlier publication
  when readback fails. S Runtime and SDK source inputs continued changing.
  The CLI check and profile response-loss body remain **NOT_RUN**.
  [Profile authority readback](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/profile_v1.rs#L270)
- The known local and SSD casectl roots had no primary snapshot for S
  `e7ab9f28fe6b`. A fresh exact owner capture is required for that source.
  The existing Q SearchPlane request remained in admission wait.
- S `72da69d7b171` returns typed `InvalidNativeProducer` from the generic
  RepoMap reader when a Generation-backed read requires an admitted visitor.
  The metadata and handoff callers propagate the refusal. The Source
  snapshot state split preserves the ordered frame checks and keeps the
  transcript and head leases through exporter completion. The candidate
  source was still changing. S compile and DATA execution remain **NOT_RUN**.
  [RepoMap reader](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/repo_map_export_bundle.rs#L148)

- Q Runtime Legacy module retirement preserves the Source exporter dispatch,
  selected-root lock and frame refusal selector. The Legacy fixture still
  uses the digest-pinned external binary from the Harness. All 15 current
  Catalog, SearchPlane and Source exporter selectors resolve to source test
  definitions. The current Q exporter body remains **NOT_RUN**.
  [Source exporter dispatch](../../../../crates/quanta-index-searchd-runtime/src/bin/quanta-index-searchd.rs#L84)
- S `421551223cf1` with the in-flight Source route extraction preserves the
  bind, submit and completion order and discards both Source and RepoMap
  staging on route failure. The changed CLI files parsed with Rust 2021
  rustfmt. Read-only preflight retries with the exact snapshot collector
  stopped on changes between observations of the Q sibling or S primary
  overlay. The current CLI and Required DATA checks remain **NOT_RUN**.
  [Source route handoff](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/source_import_v1.rs)

- QBC retried the existing S Required DATA request after the candidate code
  was committed. S HEAD advanced from `b06ad804d79` to `3525e82fa6d` during
  capture. QBC exited 1 with `primary overlay HEAD changed during capture`
  before issuing an execution root or entering Cargo. At S `317367c436e2`,
  exact preflight passed for both siblings, then the QBC request exited 1
  because the Q sibling overlay changed during capture. At S `123ceeaee6b`,
  direct QBC capture reached the post-capture check and refused a Q sibling
  overlay change there. At S `04f6732c2f4`, preflight passed again and QBC
  refused a Q sibling change during capture. At S `8a005363356`, preflight
  passed and QBC again detected a Q change after capture. No execution root
  was issued. The `11cd7c07155` attempt exited 1 with `workspace inputs
  changed during index capture` before Cargo. Required DATA remains
  **NOT_RUN**.
- S `0747c8b3a96` sealed the current tracked and untracked overlays with
  snapshot digest
  `0747c8b3a961e0cd1e21c1e8ab21e6f288e927dd+ovl-c928d5c52fa809b2fde2985a6c16e6ea+ext-7123ce7f17e6e6d68531`.
  Required DATA and the CLI check acquired host compile admission on the
  same validated execution root, digest and existing
  `history-source-readback-snapshot-artifact` lane. CLI run
  `20261121T201714.427578Z-e27c10485846` and DATA run
  `20261121T201632.538860Z-d0538ff61f71` both **FAILED** with Cargo exit
  101. The compiler reported E0425 for an unbound `index_v144` in
  `quanta-contract-statement-shared`'s `original_borrowed_v3.rs`. DATA ran
  zero bodies. Live S already has the enumerated parameter loop and
  corrected identity check in `0875a08df85`. Live S also changed cold FULL
  ItemIndex restore and committed Source memo recovery. Both checks need the
  updated cohort. DATA retries at `231c56be335` and `5eeff8b83a9` stopped
  during Q sibling capture before Cargo. Current CLI and DATA verification
  remain **NOT_RUN**. Preparing the exact source snapshot before resolving
  the owner request at `1e0aa87a952` also hit Q sibling drift after capture.
  The following source-only attempt reached the primary post-copy check and
  refused an S overlay change before issuing a snapshot.
- The in-flight incremental Store change adds Source stage replay receipt
  methods to Core `InputStagingPort` and calls them from the Kernel. The
  initial readback found unresolved type imports and missing Catalog methods.
  The incremental Store owner then wired the Contract `native_protocol`
  types and added both `SqliteCatalog` delegates to the persistence and
  lookup module. An in-memory SQLite check verified the installed trigger
  text, same-fence retention, rotation, rollback and deletion using the
  current DDL, with foreign keys enabled and disabled. The sibling operation
  retained its receipt. Rust compilation remains **NOT_RUN**.
- S `f6f71795f1f` retries a retained DELTA seal only after exact replay absence
  and keeps the original ingress, root, count and History context. Its
  terminal readback checks the rebuilt receipt digest before consuming root
  data. S `25e9f5b2a3b` resumes an acknowledged Source prefix from the root
  confirmed by Q for the same ingress. The S uploader calls the SDK's
  root-bound reopen request before sending subsequent pages. Their ACKs
  advance the durable prefix before the History witness is bound. S
  `1c84291ad96` also checks the RepoMap request scope, Product root locator
  and Source ACK before serving the bound member. Source sampling still
  found Q and S changes. Current S CLI compilation and Required DATA
  execution remain **NOT_RUN**.
- S `04f6732c2f4` connects the initial FULL commitment to the prepared
  artifact issuer with its body count and digest. Live and recovered
  transfers promote DELTA staged bytes only through the admitted pending
  grant and its exact artifact reference. The pending reader uses the same
  Required DATA validation for the staged body. These routes were checked in
  source. Rust compilation and execution remain **NOT_RUN**.
- The RepoMap file-owner implementation in S `cc775fc3480` reads both
  Source owners and prior projection owners at the exact Source view,
  checks the per-page cut, and binds its ACK to the retained operation,
  target Source cut and projection certificate. Rust execution is **NOT_RUN**.
- S `8a005363356` aligns the cold FULL restore wrapper with its callee and
  checks the initial Source snapshot and storage root after Required
  readback and before visiting terminal DATA. This was checked in source.
- At S `9b60ca32f83`, the initial FULL executor calls
  `NativeInitialCommittedSourceV3::try_from_store_manifest_v4` and the
  `snapshot_v3` and `storage_root_v3` accessors. The initial readback found
  no matching methods. The active E02 change then supplied all three and
  verifies the Store manifest digest, expected snapshot and C1 root under
  the original execution. The caller and definitions now match in source.
  This change still needs a current-cohort CLI and DATA run.
  [Initial FULL caller](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/indexing_api/native_initial_full_committed_source_v1.rs)
- S `1e0aa87a952` derives the installed root and SDK writer root from the
  same workspace and snapshot-store authority. Its retained Source tree
  reader rechecks the committed manifest, Store identity, HEAD, Source
  roots and Original execution before lending it to DELTA. Source review
  is complete for this change. Rust verification remains **NOT_RUN**.
- S `4274bd6cefe` parsed the Native workspace artifact opt-in before its
  bootstrap wiring landed. The owning change `39aba225e4d` now forwards
  the flag and gates artifact-store activation on it. The Native root
  factory is scoped to the selected canonical repository. The following
  source-only attempts, including the attempt started at `baf6b1bfa9d`,
  refused a Q sibling overlay change after capture and before Cargo. That
  revision's Native journal path retains its fence and failure slot through
  the late checkpoint. Current CLI and Required DATA remain **NOT_RUN**.
- S `9ddd370c367` sealed the S primary and Q sibling overlays as
  `9ddd370c367422b1e06286e1e764151e40f5e0f4+ovl-303d3ae1f16bb1fb5d26595ab3730c0d+ext-b4a6ee5f20eca2aa430f`.
  The sealed parameter loop includes the fix for the previous E0425. The
  CLI request uses the validated root on `history-source-readback-snapshot-artifact`.
  CLI run `20261121T222554.815289Z-0c492d76094a` **FAILED** with Cargo
  exit 101 on E0412 and E0063 in `quanta-adapters-pta`. S `d90bc6a62c1`
  contains both fixes. It qualifies `ProductionLiveNativeFileEvidenceV1`
  through its parent module and initializes the SelectedOne relation
  projection with no open receiver caller. SelectedOne accepts one Map or
  recursive profile, while open receiver requires a declaration roster of
  two or three members. Rustfmt parsed both fixed files. Two post-fix
  capture attempts refused later changes to the live Q sibling before
  Cargo. A later attempt at `6717a0d97c2` sealed both overlays, and the
  two fixes were read back from that root. CLI run
  `20261121T225129.138866Z-5cb6d1c98025` **FAILED** before crate compilation
  because Cargo refused the stale lockfile under `--locked`. The live
  S change `449d9041767` adds SearchPlane's `ciborium` dependency to
  `Cargo.lock`. Live locked offline metadata resolved with the CLI feature
  selection. The next exact captures at `e33d197f56e` and `8523ece2143`
  stopped on Q sibling changes before Cargo. S `4fa704950f7` then sealed
  both overlays and resolved the locked offline CLI metadata inside that
  root. Its first CLI request returned 75 after 300 seconds on host compile
  admission. The retry uses the same sealed root and existing child lane
  with a longer wait. CLI run `20261121T232126.849535Z-07f7faa6aae3`
  acquired admission and **FAILED** with exit 101 on two unused insertion
  results in Q Contract's RepoMap input stamps. The live Q owner already
  handles both results. Required DATA ran zero bodies. The next capture at
  S `48afeb27a96` refused Q overlay drift before Cargo.
- Q Catalog ran 11/11 (`b618e957-9ad2-4580-a1ef-28b068e6cc57`, exit 0).
  Its 668 observed input paths match from admission through completion.
  Since completion, eight Catalog Rust inputs and its README changed.
  Q SearchPlane ran 3/3 (`8f8cf72c-3808-495e-9b75-fcd9e7bcf29b`, exit 0).
  Its 706-path manifest matches the run and the subsequent live observation.
  Q Source exporter ran 1/1 (`4bd33309-ca38-4ad2-874b-1f416722f51e`, exit 0).
  Five Catalog inputs changed between admission and completion. The pass
  supports that executed body. Catalog rerun
  `401e1d5a-bfc6-4959-a7eb-7d8b5dc0ffe4` ran 11/11 with exit 0. The 668-path
  preflight, admission, completion and live readback all match. The
  Source exporter rerun entered Cargo after shared Q admission and
  **FAILED** with exit 101 and zero bodies on the same two Q Graph
  predecessor `unused-results` errors as S History DATA. Its manifest
  changed on 17 paths before admission and four after admission. The later
  Lindera lockfile change leaves the resolved Catalog and SearchPlane package
  and feature graphs unchanged. It changes the Runtime graph.
- The frozen Q exporter retry included the Graph repair and passed that
  crate. It **FAILED** at exit 101 with zero bodies on unused `OperationStatus`
  results in Kernel `update.rs` at lines 2477 and 2636. The raw terminal is
  `/private/tmp/nc-h-close03-current.CweB4k/q-exporter-retry-output.log` and
  the run released Q admission. B owns the affected owned-work changes.
  The next retry seals the B repair in Q `a3dda0ea2` with overlay
  `5642ed30a69e862ff6192a7dce81c39f`. Both Graph and Kernel repair files
  match their captured digests. Its raw stream at
  `/private/tmp/nc-h-after-kernel-fix.IT7Nzy/q-exporter-output.log` stopped
  during compilation without a terminal or surviving exporter processes.
  The recorded zero exit in that directory belongs to source capture.
  The existing detached executor reused the same frozen source, selector
  and warm `test-workspace-lane` with the approved shared lock. Nextest
  `f541bfcd-f10a-4ba0-8af3-7a8b6018fa44` **VERIFIED** the single frame
  budget and cancellation body (1 pass, 652 skipped, real exit 0).
  Its raw stream and terminal are under `/private/tmp/nc-h-exporter-owned.3ismr9zy`.
  After that Q terminal, H-CLOSE-03 entered the existing
  `history-source-readback` admission with the same paired snapshot and
  two-body owner request. The detached S executor's raw stream and receipt
  escrow are under `/private/tmp/nc-h-s-data-owned.d9ur9h2r`. The S executor
  ended at 18:38 KST with exit 75 and no canonical result because the
  Source509 host compile-activity admission lock stayed occupied. Cargo
  did not start and both S bodies are **NOT_RUN** for that attempt.
  Source509 reached `STOPPED_ON_OWNER_FAILURE`, its controller exited,
  and the host claim was released. The same S request was then resubmitted
  once in `history-source-readback` on the existing paired snapshot.
  Run `20261122T094844.703285Z-f0692c009f52` cleared admission and **FAILED**
  with Cargo and executor exit 101, zero bodies and an owner `RED` result.
  `quanta-runtime-analysis` could not resolve the recovered MayAlias member
  and paid preimage module through its PTA intraprocedural facade. The
  canonical receipt and raw terminal paths are recorded in
  `/private/tmp/nc-h-s-data-post-source509.jstw6sq2/data-result.json`.
- The existing QRA/PTA source owner for this boundary is M23-147 C
  (`01a20228-a063-7463-bb45-70c06bd482ec`). Its ownership is recorded in
  [the M23-147 C handoff](/Users/songmin/.codex/artifacts/scg-m23-147-parallel-20261122.yQQmVc/C.md).
  The H feature closure enables `live-native-java-generation`,
  `live-native-dfg-output`, `cfg-dfg-surface`, `runtime-cfg-dfg`, and
  `pta-rail`. The failing frozen callers use the thin `intraprocedural`
  facade. S `ccc94119317` routes the observed MayAlias callers and paid
  preimage birth directly through `intraprocedural_owner`.
  H retains one whole-target request `a10740d1c02c9248cd1ff50637c9022c`
  in `history-source-readback` with class `0ac60ceaae02`.
- Fresh S capture `60bdd170af3` includes the current QRA caller routes and
  Q `187e9668a` with its fixed overlay. The owner request ID, Cargo argv,
  and Runtime and QRA feature sets match the earlier run. Run
  `20261122T102411.386113Z-9a2845f6434a` reached Cargo and compilation
  **FAILED** (Cargo and owner exit 101, owner `RED`). The two History DATA
  bodies are **NOT_RUN**, with zero witnesses in the receipt. QRA reported
  three diagnostics. `pta_query_facade.rs:133` re-exports the member
  through the private `intraprocedural_owner` module (`E0603`), and
  `original_program_point_v3.rs:373,379` calls `consume_work_v1` with
  `NativeAllocationAdmissionV3` out of scope (`E0599`). The previous
  `E0432`, `E0433`, `E0412`, `E0614`, and `E0282` diagnostics did
  not appear. The owner result, receipt and terminal log paths are in
  `/private/tmp/nc-h-current-qra.drLVJy/owned/data-result.json`.
- M23-147 C repaired all three compiler diagnostics in S `ff3c911222cd`.
  The MayAlias query exposes the single recovered member type through the
  existing Java-gated PTA facade. The program-point path imports
  `NativeAllocationAdmissionV3` for the two existing budget charges.
  The same H request retains the designated Q `187e9668a` overlay for
  exact re-verification. S `ff3c911222cd` was paired with that overlay
  using exact snapshot custody. The request ID, feature sets and Cargo argv
  match the previous run.
  Run `20261122T104013.874347Z-9de09aca5fd4` reached the
  `quanta-runtime` library with none of the prior three QRA diagnostics.
  Runtime compilation **FAILED** with 18 Rust diagnostics (Cargo and
  owner exit 101, owner `RED`). Both Required History DATA bodies are
  **NOT_RUN** (0/2 witnesses). The result and hash-bound receipt, stderr
  and stdout are recorded in
  `/private/tmp/nc-h-qra-repaired-pinned.uarcqlfr/owned/data-result.json`.
  The diagnostics cover these Runtime boundaries:
  - Native prepare and SDK resume reference a payload through the wrong
    parent and a keeper through a disabled facade. The cold watch V12
    method exposes an argument whose Runtime feature gate is absent.
  - Cold parent transfer has two missing scoped-memory type references.
    Change-event reconciliation omits the new pending memo argument in
    `native_pending_transfer_v3::reconcile_v3`.
  - Source closure issuance passes typed keys to a string-keyed API, and
    one binding validation loop iterates an extra reference. Required
    RepoMap preparation has an ambiguous conversion and Ordinary resume
    omits the new `NativeRepoMapPublishedReplay` error variant.
  - Native DELTA final handoff uses stale retained-artifact accessors and
    omits prior required read data. Two Native semantic staging sites
    require a serving-source lease capability that the supplied memory
    group does not implement.
- The H follow-up addresses three Runtime boundaries from that receipt.
  Native source birth references the original execution keeper through
  its internal owner module. Cold Watch RecordOnly activation shares the
  V12 checkpoint birth's feature requirements, and the SDK published
  indexing feature requests the Runtime artifact registry for that caller.
  Targeted source capture borrows each canonical path as a string and
  owns that string only in the input map passed to the issuance API.
  The keeper and Watch changes are in S `7ea6d7273ab`. The path change
  and the existing owner's Native preparation, pending transfer,
  RepoMap, DELTA and V6 staging repairs are in the S `1fe07adb943`
  source selected for the same whole-target request. The SDK feature
  tree resolves the V12 caller's Runtime features. The existing H
  Runtime and QRA feature sets are unchanged.
- Run `20261122T113741.378748Z-a917d8e8537a` bound S `1fe07adb943`
  to the existing Q `187e9668a` overlay and reused the same request,
  Cargo arguments and feature closure. Compilation **FAILED** (Cargo
  and owner exit 101, owner `RED`) before Runtime: S imports
  `InstalledNativeRecoveryV2` from Q's shared installed-artifact trust
  crate, and the pinned Q source does not export it (`E0432`). The
  export exists in the Q working tree as of this check. The previously
  frozen Q does not satisfy the new S dependency. Both DATA bodies are
  **NOT_RUN** (0/2) in this attempt. It supplied no Runtime compile proof.
  The result and its receipt-bound logs are recorded in
  `/private/tmp/nc-h-runtime-repaired-pinned.fYmiMazA/owned/data-result.json`.
- The compatible capture pairs S `1fe07adb943` with Q `252ec2ec7`
  overlay `15bf9af2e2136a82b082e3d25f6549f3`. The validated sibling
  carries `InstalledNativeRecoveryV2` and `NativeRepoMapPriorAffectedV1`.
  The two Q files match the captured working bytes by SHA-256. The owner
  request and reviewed Runtime/QRA feature sets match the previous run.
  Run `20261122T114342.990945Z-9e514429e4ab` compiled and linked the
  integration binary. The two-body harness started and aborted with SIGABRT
  when the SDK failure-handoff test overflowed the libtest stack. QBC
  recorded **FAILED**, exit 101 and zero completed witnesses. An isolated
  replay of that exact binary passed the private-terminal budget body.
  With an 8 MiB worker stack, the SDK failure-handoff body reached the
  assertion and failed because it expected a plain writer rejection.
  The Native History reader wraps `InvalidNotApplicable` in a preadmitted
  typed Original failure. S `d75e8d6b987` uses the existing
  synchronous large-stack harness and checks that exact History route and
  cause at the writer and inside the SDK failure packet. The direct
  coordinator check drops the error before measuring the DATA lifetime.
  Result and receipt-bound logs are recorded in
  `/private/tmp/nc-h-runtime-trust-paired.3BYnW4LJ/owned/data-result.json`.
- H-CLOSE-03 is **VERIFIED** on S `d75e8d6b987` and Q `252ec2ec7`
  overlay `15bf9af2e2136a82b082e3d25f6549f3`. Existing
  `history-source-readback-snapshot-artifact` run
  `20261122T121408.682064Z-b5d727533865` kept request
  `a10740d1c02c9248cd1ff50637c9022c`, the reviewed compile class
  `0ac60ceaae02`, and the two original selectors. The QBC receipt
  records `GREEN`, Cargo exit 0 and both completed witnesses.
  `persisted_required_data_survives_late_private_terminal_budget_refusal_v3`
  verifies the persisted DATA path after the late private refusal.
  `rejected_required_history_readback_reaches_sdk_failure_handoff_v93`
  verifies the typed History refusal, unpromoted DATA ownership in the SDK
  failure packet and refund after the last owner drops.
  The published receipt, marker and raw terminal logs are under
  `/private/tmp/nc-h-typed-oracle.AopXfi3R/owned/terminal/`.
- S coordinator readback used to drop fresh Required DATA on writer or
  phase rejection before SDK failure handoff could take it. The coordinator
  now stages the existing pending slot before the read and releases it on
  success. Writer refusal, phase rejection and publication readmission
  refusal retain it for the same Original failure handoff. Initial transfer,
  recovery, previsibility and terminal closeout callers use the shared
  path through their phase-specific checks. The existing
  `history_required_terminal_budget_owner_v3` integration target has a
  second body, `rejected_required_history_readback_reaches_sdk_failure_handoff_v93`.
  It persists a well-formed but forged History recency absence, checks the
  exact writer refusal, and accounts for the grant through handoff and drop.
  S `15865f1d2a6` completes the transfer-phase callers. The owner target's
  locked offline metadata resolves its Python floor, retrieval owner test,
  SQLite and SDK ingress features. After primary overlay drift refusals,
  S `e3c1370e218` sealed both overlays. History DATA run
  `20261122T061002.854465Z-2c6f512e8440` selected the whole owner target
  and **FAILED** with Cargo exit 101 and zero bodies on an unresolved
  lifecycle codec import in `quanta-runtime-analysis`. The live call now
  imports the lifecycle types through their mounted portable module.
  The post-fix observation found no stable interval for another exact
  capture. Corrected-cohort DATA and CLI run remain **NOT_RUN**.
- The Legacy restore-forward route without protected Source routes uses the
  existing whole-root activation scope check before and after physical
  readback. Catalog quiescence also rejects active RepoMap outside the
  selected scope. The Linux Runtime library now exercises the existing
  activation reader against real files for primary acceptance and sibling
  refusal. Its body is **NOT_RUN**.
- Native first-FULL cold Acked retry now transfers the coordinator's
  Required readback into its Original failure packet when revalidation,
  pin comparison, root restore or memo checkpoint work refuses. The
  earlier coordinator-only regression now also checks the real retry
  packet retains unpromoted DATA and releases funding at the last drop.
  The same Required History owner target uses feature isolation
  `quanta-runtime.no-default.0ac60ceaae02` for this branch. The previous
  narrow profile omits the SDK workspace bundle and skips the cold retry
  assertion. Package-scoped Cargo feature resolution confirmed both SDK
  gates in the corrected request. Exact S `e796fb7b0fa` with Q overlay
  ran `20261122T071110.826166Z-e894231a2587` in the existing History lane.
  Cargo **FAILED** with exit 101 and zero bodies on two `unused-results`
  errors in the live Q Graph predecessor file. Those inserts belong to the
  [existing RepoMap DELTA lane](../../nov-20-native-cutover-closeout/tickets/LANES.md).
  History test bodies and S CLI check remain **NOT_RUN**. CLI compilation
  uses the same blocking Q Contract dependency.
- The History owner test target now requires Original and the Python SDK
  workspace bundle in Cargo. The named SDK failure-handoff test executes
  its packet assertions unconditionally whenever the target is selected.
  Locked offline Cargo metadata and package-scoped feature resolution show
  the old profile lacks the required Python bundle and the History profile
  supplies all four required features.
  This body verifies the real retained failure packet and lease release.
  The installed cold retry entry, pin and root restoration, memo backfill
  and restart remain separate execution obligations.

## Current NC-H implementation map

This map locates the NC-H work and its direct callers in Q and S. The current
checkout contains concurrent work. The links identify the owning functions
and tests in those files.

| Work | Implementation and regression |
| --- | --- |
| Q signed SQL History, source cut and terminal ACK | [Latest applied receipt](../../../../crates/quanta-index-catalog/src/idempotency.rs#L1577) · [Held History snapshot](../../../../crates/quanta-index-catalog/src/native/authority/read_only.rs#L1180) · [Legacy checkpoint page reader](../../../../crates/quanta-index-catalog/src/native/authority/read_only/history_pages.rs#L352) |
| Q Exact, Latest and materialization pin issue, retirement and settlement | [Ticket issue and pin integrity](../../../../crates/quanta-index-catalog/src/native/authority/lifetime.rs) · [Read settlement](../../../../crates/quanta-index-catalog/src/native/authority/read_settlement.rs) · [View retirement](../../../../crates/quanta-index-catalog/src/native/authority/retention.rs) · [Terminal work retirement](../../../../crates/quanta-index-catalog/src/native/authority/terminal_work.rs) · [Shared Catalog tests](../../../../crates/quanta-index-catalog/src/native/authority.rs) |
| Q BOOT, orphan roots, protected siblings and rollback | [Workspace and view-root custody](../../../../crates/quanta-index-catalog/src/native/authority/workspace_integrity.rs) · [BOOT quiescence](../../../../crates/quanta-index-catalog/src/native/authority/boot_quiescence.rs) · [Route validation](../../../../crates/quanta-index-catalog/src/native/authority/boot_manifest.rs#L170) · [Rollback](../../../../crates/quanta-index-catalog/src/native/authority/read_only.rs#L3214) · [Protected route regression](../../../../crates/quanta-index-catalog/src/native/authority/tests/offline_protected_routes.rs#L394) |
| Q Legacy export, Native checkpoint transfer and multi-head Source supply | [Legacy History exporter](../../../../crates/quanta-index-searchd-runtime/src/history_checkpoint_export.rs) · [Native History exporter](../../../../crates/quanta-index-searchd-runtime/src/history_checkpoint_export/native_v1.rs) · [Source exporter](../../../../crates/quanta-index-searchd-runtime/src/history_checkpoint_export/source_v1.rs) · [Protected snapshot import](../../../../crates/quanta-index-catalog/src/native/authority/protected_native_import.rs) · [Page closure and ACK process fixture](../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/native_boot.rs) |
| Q Native History query, replay, funding and retry | [Replay fold](../../../../crates/quanta-index-search-plane/src/auxiliary_authority/history_source.rs) · [Replay grant regression](../../../../crates/quanta-index-search-plane/src/auxiliary_authority/history_source/tests.rs#L352) · [Native build](../../../../crates/quanta-index-search-plane/src/native_history/build.rs) · [Query](../../../../crates/quanta-index-search-plane/src/native_history/query.rs) · [Runtime4 fixture](../../../../crates/quanta-index-searchd-runtime/tests/e2e_auxiliary_catalog/history_native.rs) · [UDS FULL retry](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/history.rs#L545) · [old-Q pin and selectors](../../../../Justfile#L455) |
| S producer Original custody, Source cut, durable ACK and retained replay | [Producer handoff](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/history_handoff_dispatch.rs) · [Retained replay inventory](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/history_handoff_dispatch/history_replay_inventory_v1.rs) |
| S standalone and resident Native candidate, History inventory and head preservation | [Candidate continuation](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1.rs) · [NativeSelected History inventory](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/history_native_v1.rs) · [Protected Native transfer](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/native_protected_v1.rs) · [Resident recovery](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_offline_cutover_v1.rs) |
| S installed empty inventory, FULL→DELTA, lost ACK and restart | [Installed recovery fixture](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/tests/server_socket_daemon_e2e/native_offline_recovery_v1/scenario_v1.rs) · [Old and new Q readback](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/tests/server_socket_daemon_e2e/native_offline_recovery_v1/history_readback_v1.rs) · [ACK loss hook](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_history_ack_loss_process_proof_v1.rs) · [Existing 03.7 D/F intake](../../nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md#완료-조건) |

Q Catalog 4 uses the command below. Q Runtime4 requires `process_readiness_owner_v1`;
`just rust-test-native-history` requires five named old-Q UDS bodies and adds
the offline export body on Linux. It validates each completed body from
Nextest events so a stale selector cannot silently reduce coverage. The
Q BOOT modules and Runtime4 mount currently include untracked source files.
Include those paths when capturing the existing owner proof inputs.

**Recorded Q Catalog4 command · VERIFIED on its frozen input.** Three pin
bodies and the protected rollback scan and visitor regression in the existing
Q lane.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-catalog --lib --no-tests fail --no-fail-fast -E 'test(pin_absence_operations_fit_a_page_budget_with_many_unrelated_pins) | test(read_settlement_rechecks_pin_rows_after_an_external_catalog_change) | test(widest_canonical_workspace_pin_is_accepted_and_corrupt_shapes_are_rejected) | test(protected_route_refuses_corrupt_registered_edge_before_visiting_publications)'
```

**Recorded Q Catalog orphan-root command · VERIFIED on its frozen input.**
Exact retry import, protected current and retained BOOT, and single-workspace
BOOT and rollback.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-catalog --lib --no-tests fail --no-fail-fast -E 'test(=native::authority::tests::offline_protected_routes::orphan_view_root_with_registered_workspace_refuses_current_and_retained_routes) | test(=native::authority::tests::offline_protected_routes::protected_native_import_is_atomic_exact_and_retryable) | test(=native::authority::tests::offline_source_quiescence::single_workspace_boot_and_rollback_refuse_orphan_view_root)'
```

**Q Catalog pin and recovery checks · VERIFIED.** These select
Exact, Latest and materialization pin issuance, including root memory refusal,
pin seal failure and refund, and the bounded view decoder. They also cover
existing pin operations, grouped clock observation, the widest stored key,
durable and partial recovery cursors, bounded open intents and corrupt
retirement rows. Run `401e1d5a-bfc6-4959-a7eb-7d8b5dc0ffe4` ran all 11
with exit 0 and matching observed inputs.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-catalog --lib --no-tests fail --no-fail-fast -E 'test(=native::authority::tests::read_ticket_holds_scratch_through_pin_seal_and_refunds_it) | test(=native::authority::tests::read_ticket_refuses_root_scratch_without_pin_or_leaked_lease) | test(=native::authority::tests::materialization_pin_is_atomic_and_does_not_extend_execution_claim) | test(=native::authority::tests::read_ticket_refuses_unbounded_or_unowned_view_rows_without_a_pin) | test(=native::authority::tests::live_pin_read_refuses_unfunded_scratch_and_preserves_lease) | test(=native::authority::tests::read_recovery_preserves_committed_cursor_when_retired_cleanup_lacks_memory) | test(=native::authority::tests::read_recovery_keeps_partial_session_cursor_and_retries_bounded_batch) | test(=native::authority::tests::read_session_open_intents_are_bounded_and_budget_atomic) | test(=native::authority::lifetime::read_attempt::tests::read_observation_refuses_unfunded_pin_scratch_before_advancing_clock) | test(=native::authority::lifetime::tests::widest_canonical_workspace_pin_is_accepted_and_corrupt_shapes_are_rejected) | test(=native::authority::lifetime::tests::retired_snapshot_rejects_corrupt_blob_shapes_without_deleting_tombstone)'
```

**Current Q Runtime4 command · VERIFIED 4/4.** This selects the no-default
Runtime owner target in the existing Q lane. Nextest
`6a988b77-fcb1-4851-99be-e7fed604315e` passed all four bodies, with
248 skipped and exit 0. The 1,374 checked Rust/Cargo files matched between
the in-run capture and completion as Q HEAD advanced from `11cc2785d8a3`
to `b766fa676dfb`. The receipt applies to those observed source bytes.
A transitive Semantic file outside that manifest,
`quanta-index-semantic/src/native/embedding/ann_directory.rs`, changed
around terminal publication. The four passes apply to the built binary.
The terminal output was inspected before the host reboot. The transient
`/private/tmp` file was absent after reboot.
[Runtime fixture](../../../../crates/quanta-index-searchd-runtime/tests/e2e_auxiliary_catalog/history_native.rs#L332).

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-searchd-runtime --test process_readiness_owner_v1 --no-tests fail --no-fail-fast -E 'test(=e2e_auxiliary_catalog_history::history_native::native_history_full_delta_query_old_view_and_reopen) | test(=e2e_auxiliary_catalog_history::history_native::native_history_large_original_delta_reopens_under_64_mib) | test(=e2e_auxiliary_catalog_history::history_native::native_history_compact_original_full_delta_reopens_under_64_mib) | test(=e2e_auxiliary_catalog_history::history_native::native_history_replay_memory_retires_superseded_rows)'
```

**Q SearchPlane replay and restore command · VERIFIED.** This selects
the FULL-to-DELTA identity lease, Native successor lease transfer and
pre-scan restore admission regressions in the same Q lane. Nextest
`8f8cf72c-3808-495e-9b75-fcd9e7bcf29b` ran all three with exit 0 and
matching observed Rust inputs. Nextest
`c7dd3860-c6c5-423d-8bbd-3df4383e18e2` verified an earlier revision of the
replay regression on its recorded inputs.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-search-plane --lib --no-tests fail --no-fail-fast -E 'test(=auxiliary_authority::history_source::tests::source_replay_admits_large_delta_and_refunds_a_partial_refusal) | test(=auxiliary_authority::history_source::tests::native_history_successor_funds_next_identity_before_releasing_prior) | test(=native_history::large_row_tests::snapshot_restore_admits_generation_identity_before_scanning_rows)'
```

**Q Source exporter frame refusal command · VERIFIED on its frozen source.**
This uses the Runtime library target in the same Q lane and requires no old-Q
image. Run `f541bfcd-f10a-4ba0-8af3-7a8b6018fa44` passed one body with
exit 0 on the captured Graph and Kernel repair overlay. The preceding
Kernel `OperationStatus` compile failure exited 101 with zero bodies. Earlier run
`4bd33309-ca38-4ad2-874b-1f416722f51e` executed one passing body while the
Catalog input manifest changed. The selected frame body covers budget and
cancellation refusal. Complete Source and RepoMap inventory export remains
a separate execution obligation.

```sh
./scripts/cargow --lane test-workspace-lane nextest run --locked --offline --no-default-features -p quanta-index-searchd-runtime --lib --no-tests fail -E 'test(=native_product::history_checkpoint_export::source_v1::tests::source_frame_refuses_budget_and_cancellation_without_output)'
```

**Recorded Q Catalog9 command · VERIFIED on its frozen input.** This selects
the eight earlier H Catalog bodies and the single-workspace orphan-root body.

```sh
NEXTEST_TEST_THREADS=1 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=600 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-catalog --lib --no-tests fail --no-fail-fast -E 'test(protected_route_accepts_selected_workspace_floors_and_refuses_foreign_or_corrupt_floor) | test(orphan_protected_workspace_rows_refuse_routes_and_bootstrap) | test(orphan_view_root_with_registered_workspace_refuses_current_and_retained_routes) | test(protected_native_import_is_atomic_exact_and_retryable) | test(source_root_quiescence_refuses_a_second_encoding_of_a_published_key) | test(source_root_quiescence_refuses_work_payload_at_another_key) | test(offline_floor_scan_refuses_valid_digest_beyond_epoch_head) | test(legacy_history_exports_an_unrepresentable_v1_row_at_its_original_ordinal) | test(single_workspace_boot_and_rollback_refuse_orphan_view_root)' --test-threads 1 --status-level fail --final-status-level fail
```

## Protected publication root ownership · 2026-11-20

- The shared Catalog check requires every view-root ledger row to have its
  exact `(workspace, epoch, sequence)` parent. The primary-key scan charges
  each row and parent lookup to the caller's work budget. First import and
  exact retry run the check before any publication writes. Protected BOOT and
  snapshot export reach the same check through route validation.
- A ledger row can outlive its parent if an earlier writer disabled foreign
  keys. Such a row could be omitted by the per-view readers and still keep
  an object live in reclamation. Protected validation returns `DigestConflict`
  for the orphan. The regressions inject a correctly checksummed row under a
  registered workspace with a missing parent view. They check first import
  and retry leave the current heads unchanged, and current and retained BOOT
  accept the routes again after the row is removed.
- **VERIFIED** Q Catalog library compilation on the `32b5fed0c1a7` working
  tree with Rust 1.92.0 on macOS. `./scripts/cargow --lane test-workspace-lane
  check --locked --offline -p quanta-index-catalog --lib` exited 0 in 5m 21s.
- The first test build stopped at `-D unused-results` in three Catalog
  test-helper counter updates (exit 101, zero bodies). After those fixes, the
  BOOT regression passed and the import fixture stopped with `ResourceLimited`
  while publishing its 512 predecessor Source events. The fixture now uses a
  separate setup budget and exercises snapshot export under its 8 MiB limit.
- **VERIFIED** both Catalog bodies on the `e861e0b631c1` working tree using
  `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline
  -p quanta-index-catalog --lib --no-tests fail --no-fail-fast --test-threads 1`
  with `-E` selecting exactly
  `orphan_view_root_with_registered_workspace_refuses_current_and_retained_routes`
  and `protected_native_import_is_atomic_exact_and_retryable`. Nextest run
  `a6dcf064-be31-4c9e-b4ce-ed2c3f5b30ec` exited 0 with 2 pass, 0 fail and
  407 filtered in 6.412s. The checked Cargo inputs and relevant Catalog source
  blobs matched before and after the run. The later 429-path Catalog
  selection above reran both bodies and the single-workspace BOOT body.
  Signed Linux install remains **NOT_RUN**.

## Read ticket root admission · 2026-11-20

- Exact, Latest and materialization share the Catalog pin issuer. It holds
  ticket scratch through the root checks and the pin seal. One encoded
  workspace key serves the view lookup, Exact floor check, pin insertion
  and pin checksum. Before decoding a root
  registration, the issuer reserves the registration backing and the sorted
  edge-key buffer from the original request memory scope. That temporary grant
  stays live through the edge scan. The scan borrows fixed-width fields and
  hashes the existing wire form into a fixed-size buffer.
- The Catalog regression publishes a root with a registered child edge, denies
  root scratch after admitting view scratch, and checks `ResourceLimited`,
  no persisted pin and full scratch refund on Exact and Latest. A normal-budget
  retry selects the same view. The materialization atomicity regression also
  checks root scratch refusal and refund. All three issuance paths now observe
  live scratch at the SQLite pin seal on success and on a trigger-injected
  seal failure. The failure leaves no pin and both outcomes refund scratch.
- **VERIFIED** on the `df090e56a72f` working tree. The focused
  `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline
  -p quanta-index-catalog --lib --no-tests fail --no-fail-fast --test-threads 1`
  request used exact `-E` selectors for
  `read_ticket_holds_scratch_through_pin_seal_and_refunds_it`,
  `read_ticket_refuses_root_scratch_without_pin_or_leaked_lease`, and
  `materialization_pin_is_atomic_and_does_not_extend_execution_claim`.
  Nextest run `1951be0f-a123-461c-86bd-1b80b2d557dd` exited 0 with 3 pass,
  0 fail and 406 filtered in 0.422s. The checked Cargo and Catalog source
  blobs matched before and after the run. The preceding request exited 124
  before Cargo after 1,800 seconds of shared D3 build-slot contention.
- The bounded-view regression now stores valid oversized JSON with the correct
  row digest in the current and retained view tables. It requires the payload
  shape error and zero pins. Earlier input also failed checksum validation, so
  it could not distinguish a missing size limit. The focused Catalog `--lib`
  nextest on the `df090e56a72f` working tree selected
  `read_ticket_refuses_unbounded_or_unowned_view_rows_without_a_pin` in
  `test-workspace-lane`. Run `1a1eee63-0d33-421a-9c65-644266bfc3a0` exited 0
  with 1 pass and 408 filtered. Checked Cargo, authority and pin issuer blobs
  matched before and after execution.

## BOOT, Catalog admission and signed bind recovery · 2026-11-19

- The existing immutable closure reader authenticates each ObjectRef once per
  traversal across all protected roots. Its admitted DFS still bounds pending
  reads separately from finish markers. It preserves selected body aliases,
  rejects an active back edge, and refunds the visit and frontier allocations.
  The Storage regression holds twelve generations of shared route ancestry
  under a fixed read budget.
- The Catalog checks registered workspace membership for views, frontiers and
  Source authority rows. The indexed scan visits each stored workspace key
  once per family and runs in the import transaction on first use and retry.
  BOOT, predecessor census and installed comparison use the same validation
  through the retained or quiescent route reader. Bootstrap refuses existing
  child rows for a new workspace. The schema and object format stay compatible.
- Exact and Latest pins use the bounded view decoder and a fixed root census.
  Exact also requires a registered workspace in the issuing transaction.
  Materialization carries the original work budget from Kernel to the same
  pin issuer. The decoder rejects malformed rows before pin commit.
- The Legacy History scanner carries typed work refusals through rows, replay
  observation and page delivery. The exporter preserves that error when a
  provisional page write fails. The Legacy auxiliary port still maps its
  admitted errors at the Core boundary. The regressions assert the original
  refusal after partial row and page delivery.
- The signed Source fixture now loses the reply at the actual writer COMMIT
  boundary. The existing candidate caller observes a bind error, re-reads
  the exact durable V9 request and retains its Q ingress for the same-root
  retry. The installed multiactive failure suite includes this witness.

Source repair is present in the working Q/S checkouts. Scoped diff checks,
Rust parse, Q test authority and document links are **VERIFIED**. The
S installed multiactive failure selector resolved seven registered witnesses.
Additional Rust targets and test bodies, signed installation and production
History census are **NOT_RUN**.
NC-D continues to own the installer. NC-F continues to own signed Linux
execution and the installed FULL, successor DELTA and restart acceptance.

## Current static handoff · 2026-11-19

<a id="q-main-protected-snapshot-supplier-20261119"></a>

**Q main supplier.** `3a5dddbae1bc` carries the routed Source decoder, physical snapshot plan, page writer and authenticated page reader through Seal and restore-forward. The Contract public API and Contract/Core module baselines were generated from the isolated source. `7d721f33c` integrates BOOT marker custody, its Exact retirement and orphan guards, and unique-object History export. `61e3ff261` integrates the funded protected Catalog import and predecessor witness, streamed candidate object verification, and installed ledger comparison. `fbaaf2ae8` adds the per-page legacy marker index and reconciles the Catalog test reexports. The first Runtime check timed out while waiting for admission, before Cargo. The retry at `3a5dddbae1bc` with `cargow check --locked --offline -p quanta-index-searchd-runtime --lib` was **VERIFIED** in 12m 11s with exit 0. That result belongs to the earlier paging supplier. The later BOOT and Catalog changes, owner bodies and installed acceptance have separate checks.

- The S CLI evidence reader calls the admitted Legacy terminal reader with
  all three callbacks in order. The History callback copies the Required
  generation, batch digest and durable sequence from the batch and its
  Original receipt. The RepoMap callback retains its disposition. The
  SearchCorpus callback consumes both to construct the evidence. This
  source inspection leaves current S CLI compilation open.
- The retained History replay builds the complete per-revision inventory
  under the producer writer fence and passes the retained terminal ACK to
  candidate preparation. The resident path supports History continuation.
  Legacy multi-active Source and RepoMap import uses the standalone candidate
  path with its pre-Source callback. The resident entrypoint rejects that
  import mode before it publishes Source.
- The Contract and Core module baseline sections now reflect the protected
  snapshot plan, encoding error, page stream and decoder declarations. The
  snapshot plan and its original method signature were copied from the
  cached Rustdoc's `cargo-public-api` output. The shared
  `NativeProtectedEncodingErrorV1` and all five checkpointed declarations
  are reflected from source in the Contract baseline. The wire encoder's
  public error exposes the typed Work cause through `Error::source` and
  retains the existing Format message. Scoped error-shape and workspace
  wire inventory pass. The workspace error-shape gate still reports the
  separate `NativeCutoverSourceJoinErrorV1`. The five-module Catalog BOOT
  cycle had identical edges at `ca1dc5c67892` and in that worktree. The
  supplier commit includes the isolated Contract API and Contract/Core
  module renders. Workspace-wide generation for the full current source
  remains **NOT_RUN**.
- The seven protected Contract records, the Core route head census and the
  snapshot field decoder now use manual serde implementations. The same
  JSON field order, bounded arrays, nullable map fields and positional
  inputs remain in the contract. The tests pin the snapshot JSON order,
  omitted route origin, positional input and duplicate-null refusal. Scoped
  derive checks pass. The workspace derive gate still fails on other
  declarations in the shared worktree. The Rust bodies and exact generated
  API comparison are **NOT_RUN**.
- The BOOT claim and ingress regressions are registered as Catalog library
  tests. The workspace test-authority check passes after the independent
  Legacy reference metrics test was moved to a directly registered file.
  The recorded Q Runtime2, S History DATA and S no-default results apply to
  their frozen cohorts. Current Rust bodies and signed FULL, successor DELTA
  and restart remain **NOT_RUN** under the [active backlog](#current-actionable-backlog--2026-11-16-re-audit).
- The existing Q Runtime2 fixture now requires the original typed Storage
  error after the SQL rows have been consumed and the exact FULL and DELTA
  commit identities after reopening the immutable store. A count-preserving
  substitution fails the readback assertion. The S DATA fixture covers
  durable Required receipt custody with an empty History batch.
- The Core snapshot decoder is tracked in `3a5dddbae1bc`. The QBC exact owner
  source capture is **VERIFIED** at Q `fbaaf2ae86bbc01297708e711610ecef13bf112c`
  and S `6a7d21a99fd7be8862e3108bdd880ed16734035b`, including the transitive
  GQLang path dependency. Validation checked the detached custody manifest,
  both `Cargo.lock` files, Q Catalog and exporter repairs, the Core decoder,
  and the S signed writer regression against the observed bytes.
  The resolved identity is
  `6a7d21a99fd7be8862e3108bdd880ed16734035b+ovl-9c8112a658e9248d06d5a27192f0535e+ext-653fb6175e56ec61fb5b`.
  The S execution root is
  `/Volumes/Extreme SSD 1/casectl-nc-h-snapshots/6a7d21a99fd7be8862e3108bdd880ed16734035b_ovl-9c8112a658e9248d06d5a27192f0535e_ext-653fb6175e56ec61fb5b/semantica-codegraph-v2`.
  Compilation and runtime remain **NOT_RUN**.
- Q's formal benchmark source closure requires clean scoped source. S's
  exact owner test snapshot can materialize live Q/S overlays, including
  the decoder. The pinned mode materializes committed bytes. The exact
  snapshot switch applies to tests. The CLI and production compilation
  entries use their owning build paths and remain **NOT_RUN**.

## Shared-object transfer across Native History roots · 2026-11-19

- A compact route references both its predecessor and the snapshot that
  retains that predecessor. The previous export traversed it along both
  paths at every generation and restarted traversal for each snapshot root.
  Valid repeated migrations could exhaust read and frame budgets as ancestry
  grew.
- The Q exporter now shares a funded visit table across the root set. It
  emits each object once after its children, preserves the full ordered child
  vector, and rejects any cycle with `DigestConflict`. The frontier and visit
  table have separate memory loans. A visit table resize holds both loans
  until it releases the old allocation. The existing History checkpoint
  export uses the same traversal. The S receiver continues to stage the
  unchanged object format and receive the new snapshot manifest last.
- The regression uses twelve compact ancestors and a shared root under a
  fixed read limit. It checks two physical reads and one completion frame
  per object, ordered parent edges, and full memory refund. The existing
  repeated-edge fixture keeps its child vector and now expects one child
  transfer per identity. Rust formatting, test registration and the wire
  inventory are **VERIFIED**. Rust compilation and test bodies are **NOT_RUN**.

## Exact view retention across compact Native routes · 2026-11-19

- A Native carry-over route whose direct children exceed the object limit can
  retain its Exact-view markers through the previous route. Seal authenticates
  the inline or paged snapshot links and records the registration that directly
  holds the complete marker set in the selected BOOT record. Retirement checks
  that registration once per page and uses indexed in-memory marker membership
  for each Exact view. Terminal custody release leaves the physical edges live.
  Earlier BOOT records can omit the anchor. For those records, the first
  unpinned candidate without a direct marker builds an admitted marker index
  for the retirement page. The reader checks each visited registration and its
  edges once, follows the existing two-child ancestry boundary, and accepts the
  physical page order. A marker hit rechecks its exact edge. A cycle or work
  refusal aborts the page. Read-only BOOT preserves the original record bytes.
  New seal derives the carrier from the authenticated route and snapshot.
- The Catalog guard regression supplies two compact ancestors with no
  operation custody and requires protection of the original Exact marker.
  It checks the paged snapshot's original anchor, two lookups under four read
  units after loading, unrelated marker exclusion and missing-edge refusal.
  A second fixture uses twelve shared compact ancestors with physical page
  order distinct from sort order. It checks bounded reads, lazy admission,
  lookup reuse and memory refund on both success and refusal. Rust bodies are
  **NOT_RUN**.
- The Catalog orphan scan refuses to reclaim a marker carrier still named by
  the selected BOOT when every ordinary incoming reference is missing. This
  preserves the carrier for diagnosis and recovery. The existing BOOT
  regressions cover this guard and the 4,096-byte record bound with an anchor.
  The Exact-view admission floor remains governed by the ordinary read-ticket
  contract.

## BOOT claim and ingress admission · 2026-11-19

- The predecessor and candidate census and both Catalog import branches
  reject dangling event claims before the workspace join. The shared check
  uses the claim-operation index, rejects malformed key widths, and preserves
  request cancellation on SQLite interruption. The first import and exact
  retry fixtures require refusal for a claim inserted with foreign keys
  disabled, even when the joined row count is unchanged.
- Candidate resume, Catalog import and route selection now share the measured
  route-seal ingress reader. It checks BLOB widths before loading the row and
  holds the original WorkBudget grant through decode. A submitted operation
  makes the ingress ineligible via an indexed existence check, independent
  of the operation payload. Oversized ingress and submitted-operation rows
  are covered under a tight SQLite result limit. Rust bodies and installed
  recovery remain **NOT_RUN**.

## Snapshot refusal and retry bounds · 2026-11-19

- Seal preserves the physical page read and authentication failure when
  malformed JSON is encountered first. It finishes outstanding authenticated
  reads with bounded scratch. A stream I/O refusal, cancellation or budget
  denial ends the original attempt without reopening a page or importing
  Catalog rows.
- The canonical snapshot length and page digest passes check the original
  WorkBudget at 64 KiB intervals. Seal, inline receiver validation, Catalog
  export and import, and BOOT witness construction preserve the typed cause
  from a refused checkpoint.
- Protected route validation checks work between heads and while streaming
  canonical length and hash. Catalog calculates the identity, wire length
  and ordered children together and reuses the children for the registration
  check. Core uses the same funded child validation when it holds route bytes.
- Core checks work during both route JSON passes, including a long ignored
  value in the head-count pass. Bodies up to 64 KiB use serde's slice reader.
  Larger bodies share the snapshot JSON checkpoint reader without copying
  the full payload. The token preflight and original parser and head loans
  still apply. The cancellation regression body and reader performance
  measurement are **NOT_RUN**.
- Raw route and owned snapshot authentication now checks the original work
  request between 64 KiB payload chunks. The shared immutable-object prefix,
  ordered children and resulting digest remain the same. The regression
  compares a fixed wire digest and refuses at the second payload chunk.
  Its Rust body and the new Contract API render are **NOT_RUN**.
- Exact import retry projects only 32-byte BLOB digests for frontier and
  Source authority rows and compares borrowed bytes. The fixture injects a
  2 MiB digest in each table under a 1 MiB SQLite result limit and checks
  `DigestConflict`. Rust bodies, generated API and installed acceptance
  are **NOT_RUN**.

## Streamed Native Seal pages · 2026-11-19

- Seal opens the paged snapshot's immutable object files sequentially. One
  64 KiB buffer serves token preflight and decoding. The same admitted file
  descriptor provides the bytes, ordered edges, wire digest and EOF check.
  Seal checks the namespace binding and closes the first file before opening
  the second, using the original WorkBudget for both reads. An interrupted
  payload read checks the original cancellation and deadline before retry.
- The decoder treats bytes as provisional. Both file checks and the canonical
  snapshot reference and ordered-root check must succeed before Catalog
  import. A decode or authentication error releases the inputs without
  calling import. Inline snapshots still use their single owned payload.
- The Core regression uses a near-32 MiB snapshot with 48 MiB already
  reserved under the 64 MiB limit. It checks page finish order, callback
  admission and memory refund. Storage fixtures cover corrupt ordered edges,
  corrupt payloads and a truncated file. Rust bodies and installed 64 MiB
  acceptance are **NOT_RUN**. The SQLite import reservation is unchanged.

## Native Seal preflight and admission · 2026-11-19

- Q verifies staged view markers and observes the candidate root before
  importing Native rows. A missing physical marker refuses before the
  provisional Catalog transaction. The UDS fixture withholds one genuine
  marker, checks both imported workspaces are absent and the ingress fence
  is unchanged, then restores the file for the existing exact retry.
- The materialized Core decoder releases each consumed payload and retains
  the first page's ordered edges for canonical verification. Its near-32 MiB
  fixture checks complete refund with a 20 MiB concurrent loan under a 64 MiB
  limit.
- Exact import retry bounds stored payloads, digests and companion key bytes
  inside SQLite using the authenticated expected lengths. The operation row
  loan also covers the admitted SQLite result. The fixture writes an
  oversized row and requires the original digest refusal below the SQLite
  result size limit. Rust bodies and installed acceptance are **NOT_RUN**.

## Installed snapshot comparison · 2026-11-19

- Restore-forward derives the snapshot reference, page references, row
  counts and ordered per-family digests from the held previous Catalog.
  The decoded rows and their grants are released before the candidate
  Catalog opens.
- Candidate BOOT authenticates the exact original manifest and page files
  with the existing streaming immutable-object reader. It compares the
  installed row counts and digests in the same SQL scan as before, then
  checks the declared Exact views. Old-root claim validation, Q Seal's
  canonical decode and the held-root comparisons remain in force.
- The Catalog fixture checks memory refund while the compact witness is
  retained and rejects a same-count frontier change. The existing missing,
  extra and reordered row cases now exercise the digest comparator. The
  Storage fixture covers one 128 KiB object under 96 KiB with ordered-edge
  and payload corruption refusals. Rust bodies, generated API checks and
  installed 64 MiB acceptance are **NOT_RUN**.

## History predecessor lookup · 2026-11-19

- The first Catalog settlement scan can record exact Published view and
  operation keys in a transaction-local index. The visitor uses that index
  and checks the selected operation row through the canonical reader.
  Duplicate view claims still refuse, and missing full view identities
  still return `SnapshotUnavailable`.
- The index uses the WorkBudget's discardable allocation. Growth is funded
  before replacing the backing, and mandatory memory can evict it before
  retrying. Unfunded and evicted readers use the existing workspace/epoch
  scan. The index is dropped before the SQLite connection closes.
- The existing VFS fixture checks a point lookup's read credits, forged
  and absent identities, real memory eviction, identical fallback results,
  and final refund. Its body, generated API checks and installed replay
  are **NOT_RUN**.

## Signed Source publication census · 2026-11-19

- Previous-root verification builds one caller-funded operation-key lookup
  over the signed Reservation, Replay and AST fact rows. The existing
  metered lineage sort orders row ordinals once. Each Published row then
  scans only its matching facts. Duplicate and phase checks, root nonce
  checks and the final complete-publication count remain with Runtime.
- The protected route's immutable object is verified before entering the
  Catalog publication visitor. The visitor validates the selected route
  and settlement in its own transaction, so this path avoids a separate
  preliminary full Catalog scan. The same held root and Catalog head
  comparisons still apply.
- The lookup regression covers original ordering, duplicate reservations,
  AST phases, unknown operations and insufficient scratch. The Rust body,
  generated Contract API baseline and signed install verification are
  **NOT_RUN**. Exact History predecessor lookup now follows the
  transaction-local index described above when its allocation is available.

## Protected snapshot receive admission · 2026-11-19

- The S receiver uses the Q Core decoders for protected routes and inline
  snapshots. It lends the staging bytes to Core and reserves parser and
  decoded backing according to their actual shapes. The former separate
  decoder reserved eight times the received payload for both cases.
- Inline validation still precedes Seal. It checks the canonical bytes,
  ordered roots, prior root nonce and route identity. A known failure
  follows the existing ingress discard path. Paged content still reaches
  Q Seal for canonical reassembly. A repeated old route keeps the retained
  heads and verifies the repeated object's staging hash.
- The S fixture admits an inline payload whose former reservation plus
  staging bytes exceeds 64 MiB. It checks the old-root nonce mismatch and
  refund after a denied decoder growth while the input loan remains live.
  The fixture body, generated API checks and signed installed replay are
  **NOT_RUN**.
- The S installed receiver requires the Q supplier changes and generated
  API baselines in the same cohort. The [D receiver intake](NC-D-boot.md)
  tracks that dependency and its installed verification.

## Bounded protected snapshot export · 2026-11-19

- The exporter computes page references and the manifest through the
  canonical snapshot hash. It then serializes into one 64 KiB buffer.
  The previous export retained a full encoded copy of up to 32 MiB.
  The physical page boundary, page and manifest references, ordered
  children and child-before-parent transfer remain the same.
- Contract and Runtime regressions compare against the original materialized
  layout and exact frame bytes, including offsets across two objects. The
  Runtime regression uses a one-chunk memory limit and verifies that a write
  budget denial after Begin preserves its typed cause and releases memory.
  Cargo, generated Contract API/module baselines, and installed 64 MiB
  acceptance are **NOT_RUN**.

## Legacy History checkpoint page admission · 2026-11-20

- Catalog keeps the existing page grouping for ordinary rows so a retry
  derives the same immutable inputs. A row that exceeds that grouping limit
  gets its own page when it fits the existing 16 MiB encoded page limit.
  The S candidate's peer-bound multi-frame request supports the resulting
  staging envelope size. The regression checks the neighboring page groups
  and the staging envelope size of a 3 MiB Legacy diff hunk.
  `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline
  -p quanta-index-catalog --lib --no-tests fail --no-fail-fast --test-threads 1`
  with exact selectors for `legacy_history_isolates_a_large_row_and_preserves_ordinary_packing`
  and `ordinary_history_page_packing_fits_one_canonical_cbor_frame`
  **VERIFIED** both bodies (2/2, exit 0, run
  `f12e4627-aa85-433b-9710-889bf0c18186`). The selected Catalog and Contract
  sources and Cargo inputs matched before and after execution.
- The new Legacy checkpoint transport sends canonical V1 payload bytes as
  bounded object frames. The page grouping, object IDs, ordered checkpoint
  roots and operation retry inputs remain the same. The S candidate validates
  the exact completed object and page order before appending a root. It holds
  one funded object buffer and stages the received bytes directly. The private
  checkpoint stream requires the paired Q exporter and S receiver. The S
  continuation launches the pinned candidate Q image for this export.
- A committed 5 MiB ASCII diff hunk can expand to roughly 20 MiB in V1 page
  JSON, above the 16 MiB immutable payload limit. Catalog retains the original
  row and ordinal for this case. Q emits a V2 directory binding generation,
  ordinal, family, exact lengths and ordered raw key/value fragments. The
  physical child set is sorted and unique; the directory preserves repeated
  logical fragments. S stages each page's complete child-before-parent
  object set. Q apply and BOOT validate the same directory and use the
  original physical row for the existing digest and terminal ACK checks.
- Q preflights the V1 page dimensions and funds the decoded backing before
  allocating it. Native row preparation measures the exact legacy typed JSON
  output and uses the fragmented representation only above the physical
  object limit. Readback accepts both persisted forms under the original
  budget. Focused regressions cover exact V1 bytes and references, adjacent
  fragmented rows, incorrect child closure, and reservation refund. Signed
  installed acceptance still needs 3 MiB V1 and 5 MiB fragmented rows through
  the paired producer, apply, BOOT and reopen. The earlier Catalog bodies
  above cover the original packing change only.
- Commit decoding budgets account for actual text lengths and parent and tag
  collection backing, including empty tags. The Native JSON reader checks the
  expected Commit, Ref, Tag or DiffHunk family before applying its shape
  grant. The parser scratch bound uses decoded UTF-8 token widths, including
  escapes and surrogate pairs. This preserves the stored JSON bytes. Focused
  regressions use a 5 MiB commit message and an escaped V1 diff row near the
  16 MiB object limit with a 64 MiB original scope. S reserves the exporter
  frame fallibly and verifies the allocation before reading it.
- Restore and Native readback hold the decode peak only through typed row
  acceptance. The retained state keeps the completed row's backing grant.
  Native preparation borrows that state for V1 JSON instead of decoding the
  freshly emitted source CBOR a second time. The ordered CBOR row remains
  the digest input and the V2 fallback. Focused regressions cover a 12 MiB
  commit during CBOR reread and byte-for-byte V1 projection for all six
  History row families.
- Core's generated module tree, test authority and prompt-manager checks
  passed for the revised source. The first focused Core request reached the
  shared build admission timeout (exit 124 before Cargo). A four-body Core and
  Catalog request in `test-workspace-lane` also timed out at shared admission
  after 1,800 seconds (exit 124 before Cargo, zero bodies). The two-body
  SearchPlane request subsequently had the same admission outcome. A combined
  six-body retry acquired the slot and Cargo **FAILED** before any body. The
  History fixture fixes move the original manifest and pass the existing
  optional memory loan directly. The other diagnostics are in the default
  Lexical feature path: SearchPlane callers refer to a method compiled by
  `legacy-tantivy`. The targeted History follow-up uses that existing feature
  for the Legacy-backed cohort. Runtime process acceptance remains **NOT_RUN**.
  A separate Core/Catalog request in `test-workspace-lane` **VERIFIED** 4/4
  bodies (exit 0, nextest `943ed132-201f-4ec7-a932-c039581147a7`). It includes
  original V1 packing, consecutive fragmented rows, duplicate-part reuse, a
  missing final fragment and refund of partially reconstructed rows.
  The seven-body SearchPlane follow-up with the Legacy test feature timed
  out at shared admission after 1,800 seconds (exit 124 before Cargo, zero
  bodies). The three-body Runtime follow-up also timed out at shared admission
  after 1,800 seconds (exit 124 before Cargo, zero bodies). Its selection
  covers canonical V1 object framing, FULL/DELTA/reopen and replay memory
  return. Both selections remain **NOT_RUN**.
  A subsequent admitted SearchPlane retry reached Cargo and **FAILED** while
  writing a Contract dependency on the internal disk (`ENOSPC`, zero bodies).
  It was stopped while Cargo waited for other jobs. The revised nine-body
  selection used an external SSD target and temporary directory and reached
  the existing shared lane's 1,800-second admission timeout (exit 124 before
  Cargo, zero bodies). SearchPlane's test-only Lexical dependency now enables
  the Legacy capability its fixtures call. `cargo tree --locked --offline`
  confirmed the resolved dev feature. The next run used the default
  SearchPlane test configuration and compiled successfully. Nextest run
  `643195cc-b2d2-499e-8ece-a1de37cf0ba1` **FAILED** (8 passed, 1 failed, exit
  100). The BOOT fixture expected a child graph error for a Replay manifest
  paired with a Snapshot source. BOOT rejects the incompatible format first
  with `DigestConflict`. The fixture now asserts that refusal and uses the
  actual sealed source decoder before checking the cut and row digest.
  The default-feature retry in `test-workspace-lane` **VERIFIED** 9/9 bodies
  (exit 0, nextest `4e67dd3b-a636-4757-9829-8065c6af5809`), including the
  BOOT row and source checks, 12 MiB Commit reread and V1 JSON byte equality.
  The parallel lexical fixture change then replaced the remaining Legacy
  calls with test doubles and removed the dev-dependency. The locked offline
  feature tree resolves on that updated manifest with no direct Lexical
  dependency. The 9/9 result applies to the earlier manifest.
  The three-case Runtime retry used `./scripts/cargow --lane test-workspace-lane
  nextest run --locked --offline -p quanta-index-searchd-runtime --features
  legacy-tantivy --lib --test runtime_extended_suite` with exact selectors for
  canonical V1 framing, FULL/DELTA/reopen and replay memory return. The shared
  build slot was occupied for the 720-second admission window (exit 124 before
  Cargo). Current Runtime bodies remain **NOT_RUN**.
  The S receiver regression resolves for `dist-full,installed-product-authority`
  in the existing owner recipe. Its Linux body is **NOT_RUN**.
- Retained-original replay and Native append/successor now validate a borrowed
  History delta. Replay reserves retained typed state directly from its text,
  key and collection dimensions. It frees validation scratch before admitting
  the state and does not construct transient Catalog CBOR rows. Superseded
  grants live through state replacement. The replay regression holds the
  original input loan while applying a 3 MiB Commit and a 5 MiB DiffHunk under
  64 MiB. A 14 MiB request refuses the later row and refunds the provisional
  fold while leaving the caller's original input loan live.
- Native append and successor use the same History original decoder. FULL,
  DELTA, BOOT and offline source reads apply the same row shapes to nested
  publications in the retained-original carrier. The decoder admits parser
  scratch from token widths and retained records from their collection shapes.
  The scratch loan ends at decode completion. One regression covers standalone
  and nested 5 MiB originals, typed refusal under a smaller cap and full
  refund. A Runtime regression derives a FULL and a 5 MiB DELTA under 64 MiB,
  then reopens the file object store and verifies both cuts.
- Current SearchPlane `--lib` verification with
  `./scripts/cargow --lane test-workspace-lane nextest run --locked --offline`
  selected `test(native_history::) | test(auxiliary_authority::history_source::tests::)`.
  Nextest `4674117b-164e-4e6b-b3fa-c4d1e0b3d3aa` **VERIFIED** 18/18 bodies
  (exit 0, 630 skipped) on the manifest without the direct Legacy test
  dependency. The first compile detected a moved length counter (`E0382`).
  The retry passed after making the `usize` counter copyable.
- The Runtime build then detected a test-only import in the normal SearchPlane
  library (exit 101, zero bodies). The import now lives in the test module.
  The decoder regression now requires the typed overload error for both
  standalone and nested inputs. The post-fix Runtime retry selected the
  three existing bodies and `native_history_large_original_delta_reopens_under_64_mib`
  in `test-workspace-lane`. It reached the 900-second shared admission timeout
  (exit 124 before Cargo, zero bodies). The Runtime retry and the tightened
  SearchPlane assertion have no current execution result.
  S CLI request `c9c34b69ff14f9e7e6fa1dc7743822db` resolved its existing
  `history-source-readback` recipe. The SSD-backed run exited 1 when
  `literal_reads_v3.rs` changed during the QBC source hash. Current S CLI
  compilation has no verified result.
- The new History modules passed `rustfmt --check`. `git diff --check`,
  prompt-manager lint and the final workspace test-authority audit also passed.

## History read and restore admission · 2026-11-20

- Restoring the typed snapshot charges an original-scope read for each Commit
  parent, ref, tag and hunk target lookup before checking the target. The check
  runs for both BOOT row verification and Native member loads. Four valid
  references fit a four-read scope. The fixture requires typed overload with
  three reads and preserves the existing missing-target error.
- Native History queries measure the fields used by the selected Commit or
  Diff route. The admitted execution inputs include the owning Commit fields
  a Diff filter uses. Candidate backing follows the projected fields and the
  same snippet selector used by the result builder. Heap and anchor loans
  end with execution. The fixture exercises both Commit orders under 64 MiB
  with a retained 5 MiB diff and checks candidate custody and refund.
- The current `test-workspace-lane` request used SearchPlane `--lib` with
  `test(native_history::) | test(auxiliary_authority::history_source::tests::)`.
  The shared slot remained occupied for 750 seconds (exit 124 before Cargo,
  zero bodies). The new regressions are **NOT_RUN**. Scoped formatting,
  `git diff --check`, prompt-manager and test-authority checks passed.
- The next admitted SearchPlane run selected the History modules plus the
  legacy `history_page_tests` through the same front door. Nextest
  `c495543b-30d8-4106-b8a6-d9206fc4fbf3` **VERIFIED** 25/25 bodies (exit 0),
  including reference-read admission, unrelated-row refusal prevention and
  typed decoder overload. The changes below follow that run.

## Native History query and successor follow-up · 2026-11-20

- Native recency and relevance use the same row predicates and bounded text
  matcher. NFC owns its original-budget loan. Phrase matching and BM25
  consume token positions incrementally, including gaps for oversized runs.
  Raw text and metadata filters stream folded characters. Diff text reserves
  its exact concatenation size on first use and is shared by content and
  expression filters. The regression queries 5 MiB Commit and Diff rows in
  both orders under 64 MiB and retains the existing BM25 and cursor goldens.
- Native generation successor now decodes the file-contributor child with the
  same shape-based funding as the original History batch. The decoder counts
  the entry and identity vectors and text separately from its parser scratch.
  A fixture with 2,048 file paths exceeds the former four-MiB refusal point,
  retains the original wire loan through decode, and checks typed refusal and
  full refund at a smaller limit.
- SearchPlane retries in `test-workspace-lane` selected the Native History,
  replay and legacy page tests with the same `nextest --lib` command. Shared
  admission expired after 900 seconds and again after 240 seconds (both exit
  124 before Cargo, zero bodies). The streaming query and contributor input
  regressions are **NOT_RUN**. Formatting, diff, test-authority and
  prompt-manager checks passed.

## History selection and cancellation follow-up · 2026-11-20

- Recency and Native relevance select borrowed rows from the pinned History
  member. The heap reserves slots from the page limit and row count. Cursor
  comparison borrows paths. The response reserves the selected candidates
  and continuation path before projection, and the retained grant follows
  the page through response encoding. A pressured-memory regression selects
  a small Commit while a large Commit is excluded, requires typed overload
  when selecting the large row, and checks refund in both cases and orders.
  A scored Diff fixture covers tied paths and continuation counts.
- The canonical token cursor checks cancellation at 4 KiB boundaries inside
  a run and inside separator tails. A checkpoint failure preserves cursor
  progress from the last completed run. History phrase matching stops on a
  complete match and checks the original work budget before returning.
- Scoped `rustfmt --check`, `git diff --check`, test-authority and
  prompt-manager lint passed. The SearchPlane `test-workspace-lane` request
  ran `nextest run --locked --offline -p quanta-index-search-plane --lib`
  with `test(native_history::) | test(auxiliary_authority::history_source::tests::) | test(history_page_tests::)`.
  Shared build admission expired after 1,500 seconds (exit 124 before Cargo,
  zero bodies). SearchPlane compilation, the focused regressions and the
  normalizer scanner test are **NOT_RUN** on the current changes. Signed Linux
  installation remains on hold.

## History relevance phrase counting · 2026-11-20

- A relevance document previously compared each token with every scoring
  phrase. With 64 four-token phrases over a 5 MiB repeated message, the
  scorer could perform hundreds of millions of term comparisons. It now
  walks one sparse token trie per query and counts visited states in a single
  pass per document. Reverse failure-link accumulation recovers suffix and
  overlapping frequencies. Repeated leaves share a terminal and retain their
  individual score contributions. Unknown or oversized runs break adjacency.
- Reference-window counts and a fixed 64-leaf BM25 score cover duplicate,
  suffix, overlap and gap behavior. A host restart removed the queued
  request before a verifiable result was available. The first admitted retry
  compiled the normalizer but failed in the SearchPlane History statistics
  visitor (exit 101, zero nextest bodies). The visitor now passes its closure
  by value so the borrow of the work budget is generic at each call.
  The next compile cleared that diagnostic and failed the test-only
  `unused_must_use` check for the token visitor's `ControlFlow` result
  (exit 101, zero nextest bodies). The test now asserts that a complete scan
  returns `Continue`.
  The freshly built normalizer test executable ran the exact
  `tokens::tests::cursor_checkpoints_inside_long_runs_and_separator_tails`
  body with 1/1 passed. Its other token-module tests passed 22/22 and the
  case, provenance, and version tests passed 19/19. All 42 bodies passed
  across those three disjoint selections (exit 0 each). The next SearchPlane
  retry selected the History, replay, paging, and normalizer scanner cases
  in the existing `test-workspace-lane`. On Nov 21 its 1,800-second shared
  admission wait expired (exit 124 before Cargo, zero bodies). Compilation
  and SearchPlane tests for the final lint fix remain **NOT_RUN**.

## History original decode and refusal handling · 2026-11-21

- History retains the original wire allocation while admitting the decoded
  original. Shape decoding bounds Serde diagnostics during formatting while
  leaving the original scalar available to the shape visitor. It funds the
  escaped-string scratch by the actual sequence of appends and refunds the
  shape grant before typed decode. The reader's temporary grant covers its
  scratch Vec's maximum old and new capacities and its input buffer. The
  direct and nested fixtures each use the full 16 MiB wire limit for a
  plain-text Commit under 64 MiB. They keep the raw grant live and check
  typed refusal and grant return under a smaller scope.
- Native V1 row readback retains the projected fields and funds its slice
  parser's escaped strings separately. The regression decodes a 12 MiB
  Commit while a previous 12 MiB row and the original JSON grant are live.
- BOOT now admits slice scratch before reading the source `wire_version` and
  caps diagnostics for a malformed discriminator. The version-pass grant
  ends before decoding the selected carrier. A regression keeps the raw
  object grant live, checks bounded diagnostics for a long plain string and
  typed capacity refusal for an escaped string, and checks both grants return.
  The 16 MiB retained-original fixture now decodes through the BOOT dispatcher.
  Diff, Rust parser/formatting, test-authority and prompt-manager checks
  passed. The focused BOOT bodies are **NOT_RUN**.
- Native cursor request hashing runs after canonical query lowering under
  the original Control budget, before interactive execution. The planner
  regression checks that a byte refusal precedes a mismatched digest and
  that a valid cursor reaches the plan with its original boundary.
- An independent object or map `ResourceLimited` refusal stays a capacity
  error across History's typed error conversion. Cancellation and deadline
  interruptions observed while handling that refusal retain their typed
  classification. A recorded original request denial takes precedence.
- The owning `test-workspace-lane` SearchPlane and normalizer library
  request expired after 3,600 seconds of shared admission (exit 124 before
  Cargo, zero bodies). SearchPlane compilation, the new History
  regressions, and Runtime caller execution are **NOT_RUN** for these edits.
- The S retained-replay inventory already bounds canonical CBOR at 16 MiB.
  Candidate staging preserves existing JSON bytes and object IDs when the
  JSON fits. A JSON size refusal selects one compact CBOR object with the
  same original publications, ACKs and replay fence. Cancellation and other
  sizing errors still refuse the operation. Retry computes the same bytes
  from the same retained input before checking the submitted request.
- Q checks the compact object's childless registration, pays for the shape
  pass and typed carrier, and compares its canonical bytes under the original
  work budget. FULL, DELTA and BOOT then use the same typed replay verifier.
  Focused fixtures cover a 3 MiB body that exceeds the JSON limit after
  escaping, noncanonical bytes, typed capacity refusal and grant return.
  Locked offline Cargo metadata resolved in Q and both S workspaces. The S
  writer is present in `4bd0f00ada4`.
  At that source-change observation, Q/S compilation and focused bodies were
  **NOT_RUN**. The later SearchPlane 43/43 result is recorded in the current
  status above; S CLI, Runtime adapter and installed execution remain open.
- The S replay staging callback owns the full prepared input. It refunds the
  encoded payload after StageProjectionOutput or retry identity computation
  and passes only the funded cut to Submit. Immutable identity computation
  checks the original request between payload chunks and before returning.
  The focused CLI regression derives and identifies the same JSON and compact
  CBOR inputs twice, checks their exact bytes, reference and cut, and checks
  refund on both normal return and cancellation. The S implementation and
  regression are in `2fcbca31cc9`. The CLI test body is **NOT_RUN**.
- A fresh `test-workspace-lane` request selected SearchPlane History, replay
  and legacy paging library bodies on the existing SSD target. The host
  restarted while it was waiting. The process was absent after restart and
  its log held only the admission wait. Its replacement obtained admission
  and compiled through the SearchPlane test target. It stopped on
  `let_underscore_drop` in the History work-denial regression (exit 101,
  zero bodies). The test now asserts the returned `WorkLimitExceeded` before
  checking the latched denial. The same selector was resubmitted in that
  lane. At that retry, selected bodies were **NOT_RUN**. The later admitted
  SearchPlane 43/43 result is recorded in the current status above.
- The S retained inventory read now consumes the admitted file count and
  encoded bytes before allocating each buffer. The pinned reader fills an
  exact-size buffer with Original checkpoints and refuses early EOF, trailing
  bytes or a census mismatch before lending a cut. It reserves file buffers,
  record and publication slots separately. The Original decoder funds the
  actual backing of each retained body and ACK, and the borrowed publication
  list keeps those grants through staging. The reader checks the existing
  canonical CBOR and filename with the same Original encoder used to write
  the records. This replaces the 64-times read reservation. A focused S
  fixture uses the installed 64 MiB policy to read an applied ACK and its
  replay observation for a 3 MiB Commit and requires one exact publication.
  The S body is **NOT_RUN**.
- The 16 MiB immutable limit applies to the encoded carrier after ACK
  coalescing. The disk census retains its per-file and count limits and
  charges all observed bytes to Original. A 9 MiB regression checks that
  the applied and replayed files together exceed 16 MiB while the one exact
  publication still fits. This body is **NOT_RUN**.
- Runtime Extended keeps the existing JSON replay fixture and adds a compact
  CBOR FULL and DELTA case using the same 64 MiB fixture. It checks that the
  DELTA exceeds the JSON cap, reopens the immutable store, and reads both
  cuts through their own tickets. The compact fixture and signed Linux
  process run are **NOT_RUN**.
- Q now derives each replay advance from the original batch and complete ACK
  under the original work budget before applying rows. FULL and DELTA refuse
  a forged advance even when the supplied fence and next cut agree. Legacy
  BOOT continues to authenticate the opening position, ACKs and row set
  against the retained producer and Catalog. The fold and the DELTA adapter
  fixtures require the replay-specific corruption error with unchanged rows.
  The fold also checks that a budget covering only the batch digest refuses
  the additional replay hash with zero row work charged.
- The first admitted Q retry compiled the library and **FAILED** the lib test
  build on `let_underscore_drop` in the work-denial fixture (exit 101, zero
  bodies). The fixture now asserts the typed refusal. The next admission
  **FAILED** before bodies (exit 101) on the budget fixture's unused digest
  and active Structural exports. The budget fixture now verifies the measured
  digest against the recorded token. Structural's current sources expose the
  route through its public Native DSL module. The retry uses the same
  `test-workspace-lane`.
- Nextest `949335b4-2959-4c12-980f-892f6d8ec34a` compiled the SearchPlane
  library and ran 42 selected bodies (39 pass, 3 fail, 626 skipped, exit 100).
  The replay advance and its byte-budget regressions passed. Two valid
  unescaped Commit readback cases failed because the decoder tried to reserve
  zero scratch when its peak equaled retained backing. Empty scratch now
  requires no lease. A focused regression checks that the retained grant is
  charged and refunded. The third failure expected `WorkLimitExceeded` from
  the raw budget; the fixture now requires `ResourceLimited` there and still
  requires `WorkLimitExceeded` at the History adapter boundary. The replay
  fold also skips allocation for an empty Ref/Tag delete key, which the
  Legacy transition accepts. The funded DELTA fixture makes and removes an
  empty tag and checks the grants refund. The same selector ran as Nextest
  `d45e4764-a2fa-415c-9e0c-4bddc5e867d2` on the repaired SearchPlane
  library and **VERIFIED** 43/43 bodies (626 skipped, exit 0). This covers the
  replay, row-budget and History page unit tests. Runtime adapter execution
  and signed Linux installation are **NOT_RUN**.
- S CLI request `c9c34b69ff14f9e7e6fa1dc7743822db` retried on the recovered SSD
  using `history-source-readback`. Source capture exited 1 when `resume_v2.rs`
  changed during hashing. `owner resolve` still selects that request with
  `dist-full,installed-product-authority`. The current owner check is
  **BLOCKED** on a stable source capture. The external target recovered above
  the free-space floor. A sealed casectl capture then exited 1 with
  `primary overlay HEAD changed during capture`. No execution root was
  validated. Cargo is **NOT_RUN** for those attempts.

## Export memory lifetime · 2026-11-19

- The Catalog finishes the protected census, closes SQLite and rechecks the
  main and WAL input identities before invoking the producer or BOOT visitor.
  The snapshot rows retain their memory loan through that callback. The
  SQLite connection reservation ends before the producer allocates encoded
  snapshot bytes or walks immutable objects. The caller holds the previous
  root lease throughout. A Catalog fixture checks that the scope has ended
  and the rows remain funded in the visitor, with full refund after return.
- The final old-root recheck reuses the Catalog observation made while
  validating the protected routes. It sends that observation to the existing
  selection, current-head and physical custody checks without reopening the
  same Catalog for a second identical quiescence scan.
- The History exporter's postorder DFS reserves slots for the pending
  references it actually visits, up to the configured inventory limit.
  During growth it keeps both allocations funded until the old buffer is
  released. The fixture uses a 500,000-entry ceiling and a 64 KiB memory
  limit, then checks order, repeated edges, byte offsets and final refund.
  It also covers a refusal when the old and new buffers cannot coexist.
  Rust bodies and the installed 64 MiB qualification are **NOT_RUN**.

## Previous Native publication lookup · 2026-11-19

- The BOOT and restore-forward settlement scans now verify the exact stored
  operation and work keys. A different JSON encoding of an operation key
  could previously resolve to its canonical row. An aliased work row could
  resolve through the ready-discovery recovery lookup. The offline census
  refuses both while live ready discovery retains its established lookup
  behavior. Owner-local fixtures cover both persisted mismatches and the
  healthy census. Their Rust execution is **NOT_RUN**.
- The Previous Native publication visitor now supplies a reader for its
  SQLite transaction. Restore-forward uses it for the AST Source publication,
  the History predecessor view and the Source publication bound to the
  current History head. These reads use the held root's existing exclusive
  connection and publication scope. The former callback opened a second
  connection to the same inode, whose exclusive lock could return `BUSY`.
- The fallback predecessor lookup uses the Catalog's workspace/epoch index and
  checks the operation key and immutable Published metadata. The VFS fixture
  exercises current and predecessor reads inside the visitor, a missing key,
  and unchanged input bytes. The floor scan also bounds the workspace BLOB
  inside SQLite. Cargo and installed replay are **NOT_RUN**.
- The snapshot census bounds frontier and authority BLOBs in the SQL
  projection. The aggregate measures operation and claim payload lengths in
  bytes, including when a stored value has the wrong SQL type. A regression
  uses a SQLite length limit below each corrupt BLOB's size and checks the
  typed refusal. The healthy census passes through the same limit in the
  fixture. The regression body is **NOT_RUN**.

## Snapshot custody and admission follow-up · 2026-11-18

- Snapshot identity hashes canonical JSON directly into the existing immutable
  object hash, splitting at the same 16 MiB boundary as the exporter. The
  decoder compares canonical JSON to the authenticated bytes without building
  a second payload. BOOT verifies the predecessor's exact route heads and
  snapshot identity, releases its Catalog data, and then reads the installed
  candidate. Both root leases remain held. The installed census no longer
  reserves a reserialization buffer.
- A predecessor with 4,096 distinct route children cannot add two direct
  origin references. When the resulting direct list exceeds the store limit,
  the Native successor uses the old route and snapshot as its direct children.
  The old route retains the original marker and head closure. Existing
  in-limit identities and non-Native overflow refusal are preserved.
- **Core memory admission:** the decoder reads directly from the authenticated
  page slices. It replaces the joined buffer and `8 * payload_bytes` loan
  with loans for actual vector growth, including old/new capacity during
  reallocation. The parser checks bounded V1 tokens and aggregate row count,
  retains decoded backing through the visitor, and drops the physical pages
  before Catalog work. The Core regression covers a near-32 MiB input under
  a 64 MiB ledger, reordered pages, and refund after a growth refusal.
- **Catalog memory admission.** The predecessor and installed censuses bound
  row counts before allocating snapshot storage. The supplier admits raw rows
  and typed slots, then reserves decode scratch per operation. It uses the
  same exact operation/event claim check as restore. Claim verification no
  longer rereads a Source operation for each event. Restore writes borrowed
  canonical bytes and drops its indices before route validation. Exact retry
  aggregates expected workspace counts once and checks stored digests before
  comparing canonical bytes. Protected route validation admits operations by
  measured row length. BOOT compares the installed Catalog to the already
  authenticated snapshot using the same SQL scan as export. It checks complete
  counts and compares each operation, claim, frontier, authority and genesis
  in scan order. The installed Exact view check borrows the route views and
  uses 16 KiB of fixed scratch. Each workspace must have exactly its declared
  head and any map-only genesis. A shared Source/RepoMap pair verifies the
  same published view once. The import regression includes a positive Exact
  check under an 18 KiB live memory limit. The installed comparator now
  uses the compact predecessor witness above. The scan reads operation and claim
  keys through the workspace and operation indexes, sorts the keys in the
  original V1 byte order, and admits each measured BLOB before fetching it.
  The selected-root regression compares the new order to the original SQL
  order and covers missing, extra and reordered rows and a corrupt digest.
  The Catalog regression publishes one operation with 512 Source events, uses
  an 8 MiB work-byte budget for the census, then covers import, exact retry,
  and refusal after a stored payload is damaged. Protected quiescence,
  publication lookup and BOOT use one measured operation reader and hold its
  loan through the publication visitor. Current, Exact and retiring view rows
  check SQLite BLOB sizes before reading the payload. BOOT metadata and key
  scans also bound their SELECT results. Seal uses Interactive and BOOT uses
  Control. The read-only VFS binds preflight main/WAL byte limits to the
  actual SQLite handles. It refuses excessive file-size observations and
  reads within the admitted prefix, preserving SQLite short reads at EOF.
  The crash-WAL regression supplies a stale limit to require an I/O refusal.
  WAL index and recovery-frame backing are bounded by the admitted file
  size. On verified main handles, SQLite applies the configured page-cache
  size before loading the schema. Catalog derives temporary per-row and
  per-statement limits from its supported DDL and opens the read snapshot
  under those limits. The SQLite bootstrap also measures aggregate schema
  input by record length before it passes DDL to the parser. Regressions
  cover one oversized DDL and many small DDL definitions that exceed the
  aggregate allowance. Catalog restores the ordinary row and SQL limits
  after the schema loads. Verification opens a private page cache and uses
  default planner estimates, so persisted planner samples do not add a
  separate heap load during bootstrap. The connection still reserves
  `2 × (main + WAL) + 2 MiB`. A complete bound for the SQLite parser and
  query scratch remains open.
  Near-32 MiB installed admission under a 64 MiB ledger remains unverified.
  The current Core, Catalog, and installed Rust bodies are **NOT_RUN**.
- **Frozen R38 compiler receipt.** The Catalog build on frozen `c81fadda1`
  reported one SQL `usize` bind error and three borrowed `ViewRef` errors.
  Current source binds the BOOT width as checked `i64` and returns the
  copied ceiling value. Fresh compiler and replay results are **NOT_RUN**.
- **S Native route staging.** S `869fa08b221` passes the retained Original
  loan for the moved heads into the shared staging code. Staging admits
  the encoded route, child and marker backing, and temporary allocations.
  SDK admission still covers IPC and the reply. The S build and installed
  staging replay are **NOT_RUN**.

## Current review and repairs · 2026-11-18

- **Exporter failure diagnostics:** the S settlement path retains the bounded
  stderr bytes alongside the child's nonzero exit. It drains again after
  observing exit and preserves exit as the primary failure if the final drain
  also refuses. Clean EOF, frame truncation, cancellation, and the 16 KiB
  diagnostic limit keep their existing policies. The focused diagnostic buffer
  regression is **NOT_RUN**.
- **Old-route UDS EOF:** [NC-F's codec RCA](NC-F-integration.md) reproduced
  the 6,383-byte/13-child request in guarded decode. The owned immutable
  payload uses `deserialize_byte_buf` after the fix. The recorded Contract/IPC
  run `122e2219-250f-4d61-b403-ae1cf7c05b71` passed **3/3** for that cohort.
  The current two-head UDS and signed-install replay remain **NOT_RUN**.
- **IPC scalar backing:** the current IPC/CBOR changes reserve exact backing
  for definite slices. The preceding calculation charged triple for those
  slices and could exceed 256 MiB admission for a large valid wire request.
  The current codec has a guarded 60 MiB multiframe regression, **NOT_RUN**
  in this static follow-up. This work is tracked in [NC-F](NC-F-integration.md).
- **Protected snapshot physical limit:** the old Native carry-over emitted one
  object for the 32 MiB logical snapshot, while FileObjectStore refused payloads
  above 16 MiB. The shared layout now keeps the existing identity at or below
  16 MiB. Above it, the exporter emits two bounded pages and their manifest
  in child-before-parent order. The first page carries up to 4,096 original
  roots and the second is childless. The manifest binds page order and total
  length. S stages the manifest under the same ingress; Q Seal authenticates
  the page objects, canonical snapshot, nonce and old route before Catalog
  import. BOOT uses the same identity to compare the held old snapshot and
  installed rows. Snapshot deserialization applies per-field sequence limits
  before semantic shape validation. Contract regressions cover the split, max
  roots, ordered references and malformed manifests. The Core regression
  reconstructs a large snapshot and rejects reordered pages under a validly
  hashed parent. Cargo, installed replay and the generated public API check are
  **NOT_RUN** in this static follow-up.
- **Review coverage:** traced the exclusive old-root lease through Catalog
  snapshot, candidate import, repeated installation and final BOOT joining.
  The installed Source/RepoMap retry now re-arms the existing post-bind probe
  for the exact previously bound operation and checks the same root nonce.
  Restaging that operation would fire the probe. A different Source head can
  still stage normally. The installed fault fires after a successful bind.
  S `9c5bf54e840` extends the signed writer test with a failure immediately
  after the real request COMMIT. It reopens the signed writer, loads the exact
  request and rebinds it to recover the same digest. This covers the writer
  boundary. Current test bodies and installed ingress recovery are **NOT_RUN**.

## Static funding follow-up · 2026-11-18

- The S candidate's Original replay, Legacy checkpoint and Native carry-over
  paths all hand a funded History cut to the common Submit call. The streaming
  paths move the decoded cut identity and its existing loan. The replay path
  admits a separate loan before copying the two owned generation identifiers.
  The submitter holds that loan through SDK request consumption on fresh and
  retried operations.
- The S History encoder admits the measured payload before allocation and
  writes into a fixed-width buffer. The existing 16 MiB cap applies to Legacy
  page and checkpoint payloads. Native carrier hashing and staging use the
  same encoder. The Legacy checkpoint moves its page list into staging after
  encoding, preserving page order in the body and sorted order in the edges.
- The Q protected snapshot exporter writes into its admitted payload slice.
  It refuses allocator-reported capacity above the grant for that payload and
  the closure frontier. Serialization, object identity and frame order are
  unchanged for accepted inputs.
- The Source transcript, signed bind cleanup and candidate retry predicates
  were rechecked against their current owners. Static checks cover the changed
  Rust, the S child-spawn boundary and repository doc sync.
  Current-source Rust compilation, tests, UDS restart and signed installation
  are **NOT_RUN** by request. No execution was queued for this follow-up.

## Session hardening · 2026-11-18

- **Repeated candidate start:** the Catalog now checks for the exact ingress
  key and stops at the first foreign key. The earlier check counted the entire
  ingress table on this path. A missing exact key and any foreign key still
  refuse startup. SQLite 3.54.0 ran the actual DDL and predicates for empty,
  exact, foreign, mixed and 20,001-row cases, **5/5**. The exact lookup uses the
  primary key, and the replacement query has no `Count` opcode.
- **Producer failure precedence:** S now settles the exporter on every short
  read, including an empty body and a partial header or body. The nonzero exit
  takes precedence. Successful EOF inside a frame still reports truncation.
  The existing request deadline, diagnostic bound and child cleanup remain in
  force. A disposable Rust probe with the extracted reader and settlement code
  used real child pipes. The prior and current versions each matched **8/8**
  outcomes, including the formerly hidden exit status 29 and successful EOF.
  The probe used a standard child adapter and did not run pinned spawning or
  the signed installer. S file revision `3fd15e6d9c9`.
- **Export refund oracle:** the existing Q refusal regression now uses a
  counting lease owner. It checks that the frontier loan is live at the exact
  missing-child refusal and refunded before the caller drops the budget. It
  also checks that the original refusal survives already-emitted child frames.
- **Execution:** Q's `test-workspace-lane` attempt for the seal-key golden,
  repeated ingress and exporter refund timed out at the build/test lock,
  exit **124**, **BLOCKED**, zero bodies. The concurrent check read frozen
  `02c244af1a` and excluded the current candidate and UDS regressions. The S
  `history-source-readback` lane had about **29.9 GiB / 1.61%** available against
  **50 GiB / 2.75%**, so its Cargo checks remain **BLOCKED**. The current Linux
  physical census, two-head UDS ACK-loss restart and signed FULL → DELTA →
  restart are **NOT_RUN**. The old-route staging EOF diagnosis and codec
  evidence are recorded above.

The original S CLI caller now passes the History, RepoMap and SearchCorpus
callbacks. The historical Q Runtime2, S Required History DATA and S production
no-default receipts still cover their recorded sources. Current S CLI compile
and signed installer acceptance remain open. This review is scoped to the
History, protected Source/RepoMap and Original funding changes in this ticket.
At this 2026-11-18 readback, the shared heads were Q `5604cb269` plus
working-tree changes and S `429de8b0464`. They are historical observations,
not the current source heads. The exporter reader was unchanged from
`3fd15e6d9c9` at that readback.

## Static closeout: receipts, cleanup and sealed-candidate retry · 2026-11-18

The follow-up found a reachable installation defect that the earlier static
retry audit missed. Q can commit the protected-route Seal before S adopts its
reply. The next launcher attempt starts `serve-candidate` on the same physical
root, but the old `require_unselected_candidate` rejected its selected BOOT
before the producer could replay the operation.

### Source changes

- **Stored ACK versus returned ACK:** the Source census now validates the
  journal's actual pre-sequence receipt (`applied=true`, `durable_sequence=0`,
  matching generation/batch) and its separately authenticated nonzero commit
  sequence. It retains `receipt.recorded_at(sequence)`, following the existing
  terminal verifier. Comparing the raw stored sequence directly to the commit
  sequence previously rejected a valid Source head. The latest-commit check
  still refuses later unapplied work, duplicate sealed ACKs and orphan scopes.
- Q `require_candidate_start_v1` retains the fresh/unselected path. A selected
  BOOT is resumable only with protected routes: the existing authenticator
  checks the actual manifest, namespace, immutable closure, route body and
  current/quiescent Catalog heads. The guard then rereads the selected manifest
  and nonce and rechecks the held root identity. A routeless selected BOOT,
  stale route, unrelated pending work or missing route object remains a
  refusal. The pinned config and launcher's candidate lock remain the outer
  root admission.
- A subsequent retry can itself stop after `BeginInput` and before `Seal`.
  The candidate-only Catalog read now accepts the single authenticated,
  unsealed ingress for the canonical seal key derived from the selected
  operation and actual root nonce. It requires a positive fence and absence of
  a submitted operation at that key. Other ingress, multiple ingress rows and
  nonquiescent heads still refuse. The read does not remove the retained
  ingress. S reopens the same key under a fresh fence. Q and S now share the
  existing key derivation with a fixed BLAKE3 compatibility golden. Installed
  and selection verification continue to use their existing strict policy.
- Q adds six actual activation/Catalog ACK census regressions and four
  sealed-candidate guard regressions. The ACK fixture uses the existing
  `test-runtime-barriers` dev feature to call the guarded activation writer;
  normal dependency features and production readiness admission are unchanged.
  Its negative cases include a pre-stamped stored ACK and a replayed stored
  ACK. The census cases use the real Linux directory port and are built on
  Linux only. The guard fixture opens an ingress, stops, restarts and checks
  that only the matching route key is eligible. The selected-route deletion
  fixture uses the actual `native-objects/objects/<shard>/<digest>` layout.
- The existing two-head Native UDS fixture now sends a real Seal request,
  observes its commit while leaving the response unread, drops that response,
  restarts the service and repeats the same route. Keeping the connection open
  until commit avoids confusing peer-disconnect cancellation with ACK loss.
  This is a UDS/service-restart component proof, not an installed S process.
- S extends the existing installed History recovery fixture with genuine
  foreign Source and RepoMap data. Its proof-feature-only fault occurs after
  a successful Q Seal but before caller adoption; the same signed launcher
  inputs relaunch the candidate. The fixture requires a real old protected
  route and checks foreign Source and RepoMap results before/after migration
  and cold restart. The Native successor removes the inherited Legacy-only
  Source exporter mode and places `--producer-env` before the launcher `--`
  delimiter. The Q UDS case separately exercises an unread wire reply.
- **Current Source stage cleanup:** the publication loop explicitly retires
  its local unsubmitted `Staged` on preparation/pre-bind failure. Previously
  that stage had already left `VisitState`, and its best-effort Drop discarded
  a cleanup error. The primary failure now precedes any cleanup refusal. The
  ingress is still disarmed before a signed bind attempt; uncertain bind and
  publication failures preserve it.
- **Route staging cleanup:** the imported Source route wrapper now stages and
  seals separately. A staging failure discards its exact ingress and preserves
  both typed causes through `NativeSourceStageErrorV1::IngressCleanup`. A seal
  dispatch failure retains the ingress. The durable V9 route intent remains
  unchanged and can restage its canonical bytes; this is distinct from
  discarding an uncertain signed Source request. The unused combined helper
  was removed. The added error-chain unit case proves only error structure.
- Six installed Source/RepoMap cases exercise mid-export failure, pre-bind
  cleanup, cleanup refusal and post-bind retry. Their exact-root/nonce/phase
  probes are absent from ordinary product builds. Every remaining unsubmitted
  ingress is checked for the SDK's actual typed `FenceLost` refusal. A signed
  bound ingress is recorded separately and is verified through ordinary retry.
  Proof-only census vectors use actual-count Original Temporary loans and
  fallible reservation; no maximum-size stack array remains. The cleanup-refusal
  case performs a real Q discard before owner cleanup and checks that its
  primary failure precedes the resulting refusal. The existing signed intake
  can select the exact six witnesses through
  `native-installed-multiactive-source-failure-process-owner`.
- S funding regressions use a genuine installed-budget Original, actual Vec
  backing and the production retained-route constructor across a `RefCell`
  handoff. A read-only observer retains the existing canonical root before a
  memory refusal, so refund checks do not reproject a failed Source. A fresh
  projection must still report the original refusal. Only the existing replay
  proof feature enables the existing backing-byte counter; it cannot issue or
  refill authority. These tests do not independently prove every child-process
  allocation or field drop order.

### Current verification

S scoped boundary preflight passed **53/53** policies in the preceding pass.
The latest S change set passed its scoped **5/5** preflight, Rust syntax and
diff checks. The suite registry resolved the integration module and all six
declared witnesses. A check using the launcher's actual argparse declarations
confirmed the original argument order loses the probe and the corrected order
forwards it. An in-memory SQLite check using the Catalog DDL exercised absent,
exact, foreign, oversized and submitted ingress queries. These checks do not
exercise the Rust daemon or its installed release.

Earlier Q owning compiles exited **101** on six test-fixture import/API/unused
result errors; those source errors are corrected without weakening warnings
or the production writer. Bounded admission attempts also exited **124** before
compiler/test execution.

The already-started combined Runtime lib/UDS run later finished compilation and
executed **13 bodies: 6 passed / 7 failed**, exit **100**, nextest run
`23da7b59-721e-4d57-9748-3204f6727503`. Three exporter and three candidate-guard
cases passed in that build. It predates the final ACK normalization and fixture
updates, so it is not current-source qualification. The failures were:

- Four Source census fixtures reported `RebuildRequired` before their intended
  ACK assertions. The non-Linux production metadata port returns that exact
  refusal for every directory visit. The current fixture preserves its physical
  reader and ACK oracle and runs on Linux. The corresponding macOS run cannot
  exercise the intended behavior.
- Missing-route rejection used an object path without the store's `objects`
  component. The fixture path is corrected; its current body is **NOT_RUN**.
- The two-head UDS fixture received `ipc frame truncated` while staging the old
  route. The later [NC-F reproduction](NC-F-integration.md) isolated the owned
  CBOR payload decoder and verified its repair at the codec boundary. The
  complete two-head fixture still requires a current-source execution.
- Cross-root History restore lacked `QUANTA_INDEX_LEGACY_SEARCHD_BIN`. This is
  an absent execution input, not an observed History product failure.

The newest Rust tests and generated public API verification are **NOT_RUN**.
The contract baseline includes the new key helper signature. The current user
request prioritizes static closeout and no additional build queue was created.
There is no remaining H execution from the recorded attempts.

The historical S `history-source-readback-snapshot-artifact` name is no longer
registered. The retry reuses this chat's registered `history-source-readback`
lane. Its before-run capacity guard refused execution at **23.2 GiB / 1.2%**
available against **50 GiB AND 2.75%**: **BLOCKED**, zero compiler/test bodies.
No new lane, capacity override, external message or waiting S job was created.
Signed worker/binary pins are also absent from the current process environment;
installed Linux cases are **NOT_RUN** and retain the existing D/F intake.

## Failure preservation and Source session follow-up · 2026-11-18

This follow-up inspects the actual failure and allocation lifetimes of the
supplied migration path. The earlier Immediate SQL reads, resident rejection
and protected Native import are retained. No outbound chat messages were sent;
no additional integration coordinator or waiting build queue was created.

### Source-present repairs

- **Exporter failure identity:** EOF now settles the pinned exporter before
  callers report missing completion, retaining a nonzero child exit. Successful
  child exit still drains trailing stderr under the existing 16 KiB bound.
  Q protected-export frame/closure errors retain their original typed
  `IndexError` (including cancellation and resource refusal). Snapshot JSON is
  encoded once under its counted allocation loan and hashed from those bytes.
- **Known failure versus uncertain publication:** the Native route path now
  separates immutable route staging from tag-46 Seal dispatch. A pre-Seal
  staging failure attempts exact-ingress discard and appends cleanup refusal
  after its primary cause. A failed/uncertain Seal reply retains the operation
  and ingress for replay. Source request staging likewise relinquishes abort
  cleanup *before* signed bind is attempted: SQL bind can commit before its
  reply/readback fails, so discarding then could destroy a persisted request's
  immutable input. RepoMap uses the same signed-bind boundary.
- **Source/RepoMap sessions:** exporter, transcript, seal and later per-head
  failures explicitly retire remaining unsubmitted Source and RepoMap ingresses.
  Cleanup attempts all remaining owned stages and preserves the first failure.
  Sibling profile recovery first asks Q Resume; Pending/Published reuse and
  authenticate the deterministic Reconfigure request instead of opening a new
  input fence. Only Unknown starts a new ingress. Pre-Submit profile failure
  discards that exact input; typed `IngressCleanup` retains both causes.
- **DATA/funding lifetime:** decoded-frame APIs now return the lease before
  DATA for safe reverse local drop order. Stored tuples/structs retain DATA
  before the lease for safe field drop order. Source profile/destination/map/
  staged/route working vectors obtain a count-derived Temporary Original loan
  before allocation, with fallible reserve and actual-capacity checks. The
  final `FundedImportedRoutesV1` retains its Original grant through the parent
  pre-Source handoff and final journal/stage/seal. Seal consumes the funded DTO
  and moves its vectors; it does not duplicate the entire route. Native route
  decoding also uses a DATA-before-loan holder on malformed/early-return paths.

### Verification and limits

**VERIFIED:** scoped S boundary `preflight` over the 11 affected paths passed
17/17 policies; owned Rust leaves passed rustfmt syntax checks and scoped
`git diff --check`. Ticket synchronization is checked with
`python3 tools/prompt-manager/pm.py lint`.
The final caller/type-path review also corrected the working-set helper's
`NativeCutoverEvidenceTargetV1` import to its actual public module,
`quanta_index_contract::native_cutover_evidence_v1`. Syntax/boundary checks do
not establish compiler type correctness.

New focused Rust cases cover pre-Seal cleanup preserving the original refusal,
uncertain Seal avoiding discard, typed terminal error chains, provisional Q
export followed by a missing child, exhausted work and zero-frontier refusal.
The genuine signed SQL test additionally refuses a changed transcript bind and
then recovers the unchanged original request and unfinished census.
These are regression *sources*, not executed behavior proof: **NOT_RUN**.

One bounded Q attempt used the existing lane, with a one-second admission wait:

```sh
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1 ./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-searchd-runtime --features legacy-tantivy --lib --locked --no-tests fail --no-fail-fast -E 'test(history_checkpoint_export::native_v1::tests::)' --status-level fail --final-status-level fail
```

It exited 124 before admission (zero compiler/test bodies); execution admission
is **BLOCKED**, and no job remains queued. S Rust/Linux/signed worker execution
is **NOT_RUN**. No quota, price, deadline or wire format was relaxed. Generic
snapshot visitors were left unchanged because current callers do not return
escaping DATA; hypothetical future misuse was not counted as a new defect.

## Three-defect repair · 2026-11-18

This source update owns the History carry-over defects only. D retains the
installation/rollback receiver and F retains signed Linux execution; their
existing lanes are reused.

- **Signed Source SQL reads:** both imported-origin request recovery and
  completed-head census now use the Native SQL adapter's supported Immediate
  transaction. The regression opens the real signed Native AST writer and
  checks exact request recovery and the completed census; a mock connection
  cannot qualify this boundary.
- **Protected Native carry-over:** the selected old route is read under its
  retained offline root and a coherent Catalog snapshot. The bounded immutable
  snapshot contains original terminal operation payloads (including their
  events/fences), event claims, frontiers, Source authority and RepoMap-only
  genesis. The exporter transfers the retained route closure child-first. S
  stages it under one ingress and seals an identical head/RepoMap mapping with
  `native_origin` provenance through the existing tag-46 operation. Q imports
  the ledger transactionally, retains query heads and checks the actual old
  snapshot against both the candidate artifact and installed SQL before CAS.
  Destination-only Exact-view census rejects undeclared query views. Presealed
  candidate selection uses the existing route-aware current/quiescence check;
  it must not first apply the single-workspace candidate check.
  No Source ACK, replay cursor or retired historical query view is fabricated.
- **Resident callback omission:** the direct resident request parser now
  rejects `legacy-multiactive` before SDK publication. That entrypoint lacks
  the pre-Source callback required by the standalone import path; forwarding
  flags are not required to trigger the guard. Native-selected continuation
  runs even when the primary History target list is empty.

Historical terminal request objects and Source-basis query views can already
be reclaimed. Their canonical operation metadata is preserved without requiring
those retired objects; all objects actually protected by the route remain
required. The snapshot is limited to 32 MiB and 16,384 total rows, and the
physical closure remains subject to the configured object inventory and
Original work/memory admission. Each repeated migration retains the previous
route/snapshot provenance DAG, so total transfer cost can grow across migrations;
the per-snapshot cap is not a constant bound on that ancestry. Protected sibling
Latest must still match its pinned route head; unrelated or unsettled workspaces are refused.

The initial Catalog compile failed on five test/unused-argument type checks;
the first combined owning compile later failed on three fixture visibility/move
checks. All eight source errors are corrected. The combined run entered after
25m32s of shared admission wait and executed no bodies. The subsequent latest
source retry timed out before admission after 1,800 seconds (exit 124, zero
bodies); two earlier 300-second attempts had also timed out. The final ordinary
300-second retry also ended with exit 124 before admission. No H execution or
queued duplicate remains active after these terminals. No capacity, price or
execution limit was relaxed. Current Rust regression status is **NOT_RUN**,
with shared execution admission **BLOCKED**. The existing D/F owner retains
the next execution window; the focused commands below are the continuation.

Large historical terminal rows in a protected route are now read under an
actual-length Original memory loan retained through publication visitation.
The routed prior-History-view lookup uses the same admitted read; the old
route-less lookup keeps its original bound. The Catalog fixture includes a
50-event terminal row larger than 4 KiB, retired request custody, and refusal
without an allocation scope. Its funding is a bounded test-only memory scope,
not fabricated production Original authority.
The retained focused command selects Catalog protected-route/import/snapshot
regressions, Q CLI exporter parsing and Runtime immutable exporter tests:

```sh
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-catalog -p quanta-index-searchd -p quanta-index-searchd-runtime --features quanta-index-searchd-runtime/legacy-tantivy --lib --locked --no-tests fail --no-fail-fast -E 'test(/offline_protected/) | test(/protected_native/) | test(/history_export/) | test(/native_export/) | test(restore_forward_retained_terminal_binds_history_presence_and_cut)' --status-level fail --final-status-level fail
```

New UDS regression `native_protected_two_source_heads_import_seal_and_reopen`
uses actual old Source publications, copies their persisted SQL DATA, seals
through tag 46, removes the old root, then checks cold route lookup, text search
and terminal replay. Its body and the fresh existing FULL/DELTA/restart rerun
are **NOT_RUN** until the owning queue executes them. This component fixture
cannot qualify signed installation.

**VERIFIED:** scoped S boundary preflight (5/5), new Q/S leaf format and diff
checks, Contract/Core module baseline generation and Contract public API
baseline generation. These source checks do not qualify runtime behavior.
Signed SQL and resident Linux bodies are **NOT_RUN**:
the local host is Darwin, the discovered retained worker image is Linux-only
and predates the new Source/RepoMap statements, and current image pins are
absent. These are not installed migration acceptance.

<a id="execution-update--current-user-requested-closeout"></a>
## Execution state · 2026-11-17

**Partial.** Q History components have executed bodies for typed FULL/DELTA,
readback, replay custody, source-inventory verification, SDK/UDS queries
and restart. The installed S producer → Legacy source → Native candidate route has
not completed a qualified migration of an actual existing History root. The
Q BOOT source now checks contiguous History FULL publications after the
attested Source/AST view against the complete physical Legacy generation
census. S can stream the exact original inventories in generation order under
one producer fence and append one FULL per selected generation. The
Linux installed execution is pending. Export of generations with
discarded originals requires an authenticated snapshot checkpoint.

The S production no-default class **VERIFIED** compilation on its recorded
source. The separate Required History DATA regression **VERIFIED**
persistence after a later terminal budget refusal. The newest S CLI check
**FAILED** before Rust compilation because its frozen lock lacked two
declared test-support dependencies. Current S has the lock correction and
the DFG, Runtime, Taint and PTA Map kind corrections.
The [active backlog](#current-actionable-backlog--2026-11-16-re-audit)
records the frozen runs and remaining acceptance.

The current Q Catalog verifier additionally recomputes replay genesis from the
FD6-admitted writer-state root, workspace root, contract key, and first original
body/ACK. Its focused two-case nextest run **VERIFIED** the scope and root
regressions (`5dcc1052-7770-4fdf-a88e-59bba7799250`, 2 PASS / 659 skipped /
exit0); the earlier seven-case receipt covers the previous verifier revision.
The Q exporter and S candidate can transfer a paged checkpoint for settled
Legacy rows with a real applied ACK before candidate sealing. Final BOOT still
checks the physical source and writer fence. The runtime installation has not
executed this path.
S assigns the next generation to each new manifest and binds History to that
generation and manifest digest. Q's `SourceHistoryAdvance` takes that real
successor batch and the exact prior cut under one Source and History CAS. Q
retains the exact Original, commit-recency and file-contributor inputs under the
new member. Final restore carries the verified History member and cut in the
same FD3 success terminal that S archives before restarting. S can decode the
combined canonical V9 request under Original admission, including the typed
prior History cut and its retained repository/revision identity backing.

The current Q focused nextest run **VERIFIED** all 10 executed cases
(`4e109dca-3d5a-4a81-877b-ffe46ff02bd0`, exit 0). The
`source_and_next_history_generation_publish_in_one_view_and_reopen` case
executes Q Source and successor History in one Catalog view, refuses incorrect
generation/derived inputs, and reads the committed member after reopening Q.
The same run covers the installed-terminal wire binding, admitted SDK readback,
Legacy census/ACK refusal, and the launcher authority gates. The positive
offline Legacy exporter and malformed History FD7 process cases are Linux-only
and remain **NOT_RUN** on this run.

Ordinary S Product uses the exact committed Source generation digest as its
History revision, so publishing the next S manifest changes that revision.
Q now checks same-repository generation succession under the exact prior cut
and binds the new revision from the immutable original. The Q UDS run
`06784c8b-5cb1-4a1c-8afe-6858dfd65d77` **VERIFIED** three cases after this
change. The combined case rejects a forged prior revision, preserves the
old member and reads the new revision and prior commits after restart.
The readout now returns the predecessor cut commitment from the authenticated
successor manifest. The follow-up run `0a018076-bcf3-429c-9cd3-c50a259b4f65`
**VERIFIED** the exact commitment after reopen and its admitted SDK decoding
(2 PASS, exit 0).

The existing combined Q UDS case now captures the original published Catalog
view before shutdown. After reopen it checks that view, Published operation,
both History cuts and the successor query before submitting any retry.
Submitting the original request must then return the same view and leave
Latest unchanged. This prevents replay from masking a cold persistence
failure. The Q owning feature class **VERIFIED** that cold assertion and both
Catalog inventory/quiescence cases in `459aaed5-981a-425d-a575-6becd60d7022`
(3 PASS, 737 skipped, exit 0). The first attempt reached E0432 before any body
because `--no-default-features` disabled `E2eRuntime`'s `legacy-tantivy`
dependency. The later regression now publishes actual History in two
revisions with the same ordinal and asserts correct order, exact page/ACK
readback, and refusal for either missing revision. The modified body is
**VERIFIED** in `3eccd731-5e31-4d07-915b-f18d56f9d63b` (1 PASS / 1994 skipped /
exit 0).

The existing Linux `offline_legacy_history_export_streams_real_rows_and_terminal_ack`
case now seals and activates two generations in the same Legacy revision
through the actual Q daemon. It publishes a distinct History batch against each
sealed lexical manifest and keeps both applied ACKs. The fixture installs a
manifest-backed empty restored root before publishing and uses that same
selected root for both generations. The Linux ARM64 body **VERIFIED** the
ordered inventory and each original commit, snapshot digest and terminal ACK
(1 PASS / 59 filtered / exit 0). The selection and Legacy files stayed
unchanged across export. The previous attempt failed before the exporter
because a post-publication backup refused a Lance hardlink. The independent
malformed FD7 process body passed in that attempt. These are Q component and
process results. The signed S producer and installed candidate still require
H-ACCEPT-01.

The current Q receiver checks that a successful retained terminal and
the selected BOOT agree on History presence as well as the exact installed
witness. The Darwin no-default Runtime library regression
`restore_forward_retained_terminal_binds_history_presence_and_cut`
**VERIFIED** the retry comparison in
`7f9f7eb4-4d18-4bb2-9ea3-9e697f425a58` (1 PASS / 620 skipped / exit 0).
It covers a new History cut without a prior terminal, matching successful
terminals, addition and omission across a retry, and a changed replay start.
The signed launcher and installed recovery bodies remain **NOT_RUN**.

### Native source carry-over implementation · 2026-11-18

The selected-Native exporter now holds the previous root and checks its actual
current head, profile, History members and complete row maps. Generation export
streams the immutable closure child-before-parent in bounded fragments. The
ordered immutable-stage request preserves child order and repeated edges; its
contract tests **VERIFIED** 3 bodies with
`./scripts/cargow --lane test-workspace-lane test -p quanta-index-contract --lib native_protocol::immutable --locked`,
including compact byte-string payload encoding and ordered repeated children.

S explicitly selects `--native-candidate-history-source native`, receives the
same active-selection/config pins as snapshot mode, compares the complete
inventory independently of profile order, and binds each generation to that
inventory before staging the v3 source carrier. Q authenticates that carrier
against the held old root and independently checks the final candidate's rows,
profile and retained terminal. Source signatures remain mandatory for Source
publications; History-only rows preserve their physical prior Source cut.
The focused deployment launcher checks **VERIFIED** 10 cases for prepared
route receipt validation and forwarding. A separate candidate fixture group
**VERIFIED** 4 cases with 3 Linux-only cases skipped. These results do not
constitute installed migration acceptance.

The first search-plane Rust test attempt **FAILED** during dependency compilation
on `OrderedColdRun` visibility in the concurrent lexical changes, before any
History test body. Its export visibility was corrected without replacing that
owner's implementation. That earlier attempt did not run a History body. The current component
results are recorded below; signed installed migration remains **NOT_RUN**.

The multiactive Legacy census now streams all active Source and RepoMap heads
under retained physical/SQL owners. The complete original files were located in
the sealed lexical `PagedFileAuthority`; the new Source exporter reads them
through the same admitted physical port, preserving binary bytes and foreign
source-repository identities. `export-native-source-snapshot` is an explicit
Legacy-image command. Its first Rust compile observed the source supplier while
it was being edited; its existing-only codecs now compile without Tantivy.
The physical Source export rail **VERIFIED** 3 bodies with
`./scripts/cargow --lane test-workspace-lane test -p quanta-index-lexical --features legacy-tantivy --test legacy_existing lexical_existing_source_export --locked -- --quiet`:
complete foreign-repository binary/text bytes, corrupt physical-body refusal,
and unfunded Original refusal. The capability-dump binary's missing `plan`
module alias and obsolete codec reexports were corrected on that feature
closure. This fixture exercises the held supplier, not installed CLI handoff.

S's complete stream consumer and the Original-owned imported-Source purpose
are source-present. The existing signed journal binds the old physical origin,
accepted transcript, exact per-head Source manifest, actual Native request and
completed publication. Sibling workspaces configure Source-local projections
before FULL; durable origin mapping and completed-head retry reuse the same
workspace and request. RepoMap import uses its original compiled payload in
bounded immutable pages, then ProjectionAdvance, profile Reconfigure and exact
member observation. The combined S caller/journal is now source-present through the final seal.
The journal checks Source/map-only workspace collisions in both directions
under one IMMEDIATE transaction. Boundary preflight **VERIFIED** 24/24 for the
combined caller and 17/17 for the collision follow-up. The SQLite three-case
collision probe also passed. Earlier S compilation was **BLOCKED** before
Cargo by missing external snapshot/build-cache inputs. Current compilation
and executable bodies are **NOT_RUN**.

The Catalog route object protects complete query roots and exact view markers.
Atomic ingress sealing transfers its custody into BOOT. The focused Catalog
rail **VERIFIED** 3 cases for separate primary, Source-free RepoMap scope,
actual sequence0 genesis, exact-pair/foreign-pair resolution and exact-view
retention/reopen, with the current map-only assertions included. Repository-aware Native Source readback
**VERIFIED** 1 storage case, including binary bytes and foreign repositories.
The imported RepoMap adapter oracle **VERIFIED** 1 body with
`./scripts/cargow --lane test-workspace-lane test -p quanta-index-repomap --lib imported_compiler_graph_builds_exact_maps_and_reopens_paginated_rows --locked -- --quiet`.
It builds a real Native member from fixed Legacy compiled graph input,
reopens 1,025 edges over 1024+1 pages, checks rank rows and rejects changed
Source, header, raw page and full-key collision. Before installation, the
builder now checks the configured Interactive memory credit needed to open
that payload; successful Core build alone cannot establish query capacity.
The initial receiver now joins the complete old census to genuine imported
Source records, compares physical file bytes against the actual Native cut,
and joins every RepoMap member to the held old compiled payload. It selects the
route-aware publication observer only when the sealed route is present. These
new receiver bodies remain **NOT_RUN**.

The full contract test rail **VERIFIED** 442/442 bodies with
`./scripts/cargow --lane test-workspace-lane test -p quanta-index-contract --lib --locked -- --quiet`.
It includes the historical Source purpose, optional map-only genesis route and
wire additions through tag50. The earlier duplicate-refusal regression was
resolved by restoring its original error string. Runtime no-default compilation
now **VERIFIED** both its lib and selected UDS test target. The Native immutable
closure exporter **VERIFIED** 2 bodies in nextest run
`1922a41e-fa08-4902-acdc-0d8744e6299e`: ordered repeated edges/fragments and
missing-child/frontier refusal. The requested History restore case was absent from that feature closure and
was subsequently run with its Legacy SDK bootstrap feature enabled.

The actual Native → Native History UDS body **VERIFIED** 1/1 in nextest run
`4df92b4f-6d05-4de6-a8de-638775261f52` with the historical command
`./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-searchd-runtime --features legacy-tantivy --test native_index_uds --locked --no-tests fail --no-fail-fast -E 'test(native_history_restore_full_append_and_reopen_cross_root)' --status-level fail --final-status-level fail`.
It stages the real old Native immutable closure into another root, restores
History beside a different current Source cut, appends a commit, removes the old
root, restarts the destination daemon and verifies old/new commits plus the
preserved destination Source cut. Early fixture failures were repaired by
loading the captured SDK profile, checking the release boolean's actual
remaining-pin meaning, and capturing the Source expectation before retirement.
This is historical component process proof. The Runtime `legacy-tantivy`
feature has since been removed. The current fixture is selected by
`just rust-test-native-history` with the pinned old-Q binary and SHA-256.
Current process verification is deferred.

The all-head receiver retains D's separate Legacy RepoMap saved selection,
locator, actual file identities and receipt proof through final CAS. Same-root
RepoMap checks now compare signed publish/activation receipts to current
Catalog rows despite sibling scopes, and compare projection metadata to actual
compiled files. Global quiescence rejects unactivated Sealed candidates and
requires each active Source ACK to equal the latest committed sequence; an
earlier unsealed batch remains valid, while later unapplied activation work or
a committed Source scope without an active head cannot be silently discarded.
The exact receipt and Sealed-candidate regressions **VERIFIED** 2 bodies
alongside a fresh rerun of the History case: nextest run
`f8ebd3cf-ef97-4d02-9841-af13e8c5a14d`, 3/3 passed, with
`./scripts/cargow --lane test-workspace-lane nextest run -p quanta-index-searchd-runtime --features legacy-tantivy --lib --test native_index_uds --locked --no-tests fail --no-fail-fast -E 'test(legacy_active_repomap_verifies_real_objects_without_reconciliation_writes) | test(legacy_multiscope_census_refuses_sealed_repomap_candidate) | test(native_history_restore_full_append_and_reopen_cross_root)' --status-level fail --final-status-level fail`.
The Source high-water branch is compiler-checked but has no separately executed
behavior case on this run. The combined initial receiver still needs its owning
S/Q process acceptance. Signed installed migration remains **NOT_RUN**.

## Current actionable backlog · 2026-11-16 re-audit

**H-CLOSE-01/02/04 follow-up · 2026-11-22.** The existing S lane resolved
the three requested local operations on fixed S `9b22d1c9bf8` and Q
`ff15e8172afc` overlay `20bed006fc2f773ea34c3c6206b4f00f`. CLI
check `c9c34b69ff14f9e7e6fa1dc7743822db` reached Cargo. Run
`20261122T130338.509436Z-46a49130a8a2` **FAILED**, exit 101, with two
QRA `E0616` diagnostics for `field_cfg_phase_failure_v3`. Existing S
`5c17585d21b` already scoped the field to the parent projection module
used by the sibling caller. S `c0e0c267f6f` included that change and
reached Runtime without those QRA diagnostics. The same CLI request
in run `20261122T131651.045914Z-782627b33f62` **FAILED**, exit 101,
with 12 diagnostics across five repairs. The CallGraph method now uses
relative module visibility. Ancestry SQL explicitly selects the Store
error type. Completed SDK readback imports the generation trait and
consumes the validation view before transferring Required custody. The
Original install refusal takes its first cause without moving the later
failure slot. The next capture batches all five changes.
[CLI result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-resolve.wp2EnOqC/cli/data-result.json>).
[CLI retry result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-repaired.65j1H1n0/cli/data-result.json>).
CLI Original replay request
`63446d40adb1d494862dcc7326d7b2c5` selects the candidate History module
with both JSON/compact retry and cancellation witnesses. Production
compile remains the existing lib check
`7a5d32463bc71f1b298bdf4c00199257`. The preceding S batch used
`e6f4ffb37ccc+ovl-301681f09bbc1f381fa23aac3e54d24e` with the same
sealed Q sibling. The owning lane runs CLI check, Original replay,
and production check in that order. QBC recovered enough space to admit
the CLI compiler invocation after falling below the 50 GiB and 2.75%
activation requirements.
[Frozen repair input for that batch](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-batched.Jkt3KlQb/source.json>).
That batch's CLI run `20261122T134145.620440Z-1fd74f08cc06` returned
**FAILED**, exit 101, with one `E0308` diagnostic in the signed ancestry
seal schema reader. The earlier 12 diagnostics were absent. The signed
`AstTransaction::query_row` now takes the empty argument slice by
reference. The next batch resealed S
`c002ad74ed9+ovl-3228655d518b7634994531553c518154` with the same Q sibling
and the three original requests.
[CLI result and raw log custody](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-batched.Jkt3KlQb/cli/data-result.json>).
[Resealed source](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-seal-schema.xjz9iXvj/source.json>).
The next CLI run `20261122T141008.658687Z-e8250569ba02` returned
**FAILED**, exit 101, before Runtime on a separate `E0308` in the S
retrieval-kernel's RepoMap publication caller. The new shared egress
helper requires a borrowed Native UDS client. The caller now passes
`&access.client`, preserving the same admitted request and budget.
The same three owner requests were resealed on
`b9c6ae2df6d+ovl-3eea5764a7a906d731b324e090be89fb` with both fixes and
the same sealed Q sibling. CLI run `20261122T142433.660393Z-fe9442f67012`
**FAILED**, exit 101, after reaching Runtime. The two transport
diagnostics were absent. This source had two Rust 2021 let chains in
PTA recovery and two duplicate writer-fence reexports. The live PTA
fix keeps the original boolean gates around both failure takes. The
existing Native cleanup owner removed the duplicate reexports in
`07393a9619a`.
The next serial proof fixed the two transport callers on S
`c002ad74ed9+ovl-d580a8e19901c1e66f00310c6224903a` and used the same
Q sibling. It **FAILED**, exit 101, in CLI run
`20261122T150049.037062Z-05519cb22654`. The S caller already selected
`ReadExactNativeCleanupCurrentHeadV2`. Its SQL and statement 263
registration landed together in `f024f965d8c`. The next fixed input
uses that revision plus the same two transport corrections,
`f024f965d8c+ovl-c3c32820f0455d1b82acb912f83e6a29`, with the same Q
sibling. It predates the separately landed PTA and cleanup errors.
The validated source capsule carries all seven repaired source
locations, the existing QRA fix, and both registered SQL files.
[The f024 CLI result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-sql-cohort.Gx1Itc7s/cli/data-result.json>)
is **FAILED**, exit 101, in `20261122T153325.603464Z-2f074d456642`.
Runtime compiled. The downstream SDK session owner had two
`unreachable-pub` diagnostics and one dropped callback `Result`.
Existing S `53ddab9cb3b` scopes both accessors to their owner module
and propagates the inner callback error. The production Runtime lib
check **VERIFIED**, exit 0, in `20261122T160639.418133Z-f4c56d306c5e`
on the fixed f024 input. The CLI attempt on the later full
`53ddab9cb3b` tree **FAILED**, exit 101, in
`20261122T161534.564636Z-0b8dd10adf84`. Its older SQL reader had four
trailing-comma diagnostics and three inner-doc-comment diagnostics.
The live SQL reader accepts a trailing separator and uses ordinary
comments at the included module boundary.
The CLI and Original replay now use the validated f024 Runtime/SQL
base plus just the three SDK fixes and the two transport corrections.
The same Q sibling is held in
`f024f965d8c+ovl-31b2f9b681abbda10325c87be7c56bcf`.
That owner attempt was **BLOCKED** before Cargo. QBC exited 1 with
18.9 GiB / 1.0% available after automatic maintenance and no run ID.
Admission requires at least 50 GiB and 2.75% on the existing artifact
volume. A guarded incremental-only cleanup of the completed production
target recovered about 16 GiB of free space. The production receipt and
both raw logs still match their original hashes. CLI compilation and
both Original replay bodies were **NOT_RUN** in that attempt.
When the volume recovered to 94.49 GiB, the same two requests were
queued with the same source capsule and feature class. The CLI retry
`20261122T163532.240231Z-8dd7460e1ccd` **FAILED**, exit 101, after
the SDK session owner compiled. The remaining compiler diagnostic was
a Rust 2021 let chain in the CompletedAttach pointer check.
An AST scan of 2,772 CLI and SDK Rust files identified that site, two
exact-baseline comparisons and one test-gated cancellation probe.
All four checks now use nested conditions. The pointer and baseline
refusal strings, comparison sequence and cancellation gate are preserved.
The scoped boundary and Rust 2021 parse checks passed. The CLI and
Original requests use the expanded repair snapshot
`f024f965d8c+ovl-425f34a33ad3dfdfe97465abc8386659` in the same lane.
On that input, CLI run `20261122T170334.799567Z-7a92e050cc1c`
**FAILED**, exit 101, in the SDK runtime executor. Of its 205 diagnostics,
189 came from three bridge closures capturing owners that contained
non-`Sync` recovery data. Those closures now capture the existing
runtime/session handles, terminal reference and checkpoint values they
need. The blocking bridge and its `Send` requirement stay intact.
The remaining diagnostics were addressed at the canonical API imports,
typed replay digest, committed Watch closure and rollback Code custody.
The Watch read retains the Original pin validation and reads the exact
snapshot ID and Source digest from the committed closure. The rollback
mint uses the same admitted Code session through its fallible retain.
Scoped boundary, Rust 2021 parse and diff checks passed. The same CLI
and replay requests were resubmitted on
`f024f965d8c+ovl-06a7dfaf20ffa7772e9c1b17ef8e8b85` with the existing Q
sibling.
The resubmitted CLI run `20261122T172919.799722Z-6cc8a5a55e8e` compiled
through the SDK executor and stopped in CLI ingress, **FAILED**, exit
101. The four ingress diagnostics are the IDE outcome annotation, two
borrows across the IDE replay-state update, and a const panic formatter.
The live fix gives the outcome closure its string type, borrows the
existing submitter through its observation field, and uses a const-safe
literal with the original panic text. Scoped boundary and parse checks
passed. The CLI and Original replay use the same Q sibling and the
fixed S snapshot `f024f965d8c+ovl-b7c5db8423e5211df97d376b01c0e09f`.
That CLI run `20261122T175535.328801Z-a22ff0fbe105` cleared ingress
and **FAILED**, exit 101, at the CLI binary with six diagnostics.
macOS startup now uses the existing library socket-parent helper.
The two provider types come through the existing ingress dependency,
and three unused imports were removed. The next CLI and Original
requests use `f024f965d8c+ovl-deda0679d99a786d432c080033d4eeba` and
the same Q sibling. CLI **VERIFIED**, exit 0, in
`20261122T182431.287276Z-c4208e5f9d37` with
`dist-full,installed-product-authority`. The signed installed target
remains deferred. On the same source the Original replay request
`63446d40adb1d494862dcc7326d7b2c5` **FAILED**, exit 101 and 0/2
bodies, in `20261122T184525.786571Z-86be1a0c6f30`. The binary test
harness found three compile errors. The private publication commit
helper now allows the parent module's fixture to call it, and the
committed generation fixture validates the authority constructor's
`Result` before issuance. Both repairs passed Rust 2021 parsing and
scoped diff checks. The same owner request was resubmitted on the fixed
S snapshot `f024f965d8c+ovl-7b87f11b3e385319a3858f407cfcfa63` and the
same Q sibling. The host rebooted at 04:53 KST during Cargo compilation.
Both bodies are **NOT_RUN** for that attempt. QBC normalized
`20261122T191947.386875Z-eee16a6d7b5a` to exit 124 after detecting the
dead owner PID. The compiler exit is null and no owner result was
published. The original logs were preserved. The same request, source and
warm target were submitted once after reboot. Run `20261122T213410.985224Z-4e68729b56dc`
built the CLI test binary, then returned exit 125 with **NOT_RUN** (0/2).
Its module selector matched no libtest names on macOS because the entire
History module was gated on Linux. The macOS test harness now includes
the same replay and immutable object staging functions. Snapshot export,
installed transfers and publication orchestration remain Linux-gated.
The test names and payload, identity and budget assertions are unchanged.
The existing request was submitted on S `f024f965d8c+ovl-0100bc41…`
with the same Q sibling. Run `20261122T222538.962949Z-e6535a942dfd`
**FAILED**, exit 101, before executing either body. The portable test
build found three references to `Duration` from one missing import.
The test module now imports it directly. The existing owner request was
resubmitted with S `f024f965d8c+ovl-6226e82a…` and the same Q sibling.
The retry `20261122T231637.103774Z-42eab1cab035` is **VERIFIED**, exit 0
and 2/2 on macOS. Both named witnesses executed in 1.94 seconds with
237 other cases filtered. The JSON overflow test preserves
cancellation. The JSON and compact CBOR retry test preserves the
immutable object identity and refunds staged bytes. The QBC receipt and
raw output match their custody hashes.
[Verified Original replay result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-original-test-import.dycAcfb4/original-replay/data-result.json>).
[Replay test import source](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-original-test-import.dycAcfb4/source.json>).
[Portable compile result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-original-portable.z6xKTIPm/original-replay/data-result.json>).
[Portable replay source](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-original-portable.z6xKTIPm/source.json>).
[Selector refusal](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-original-reboot-retry.20261123-0633/original-replay/data-result.json>).
[Interrupted attempt](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-original-harness-fixed.Smonijbj/original-replay/interrupted-20261123-host-reboot/observation.json>).
[Original replay repair source](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-original-harness-fixed.Smonijbj/source.json>).
[Original replay compiler result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-cli-bin-fixed.HhYA3vkE/original-replay/data-result.json>).
[CLI admission log](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-sdk-leaf.DuwZ9EQO/cli/operator.log>).
[Same-source compiler result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-capacity-retry.oqanxq98/cli/data-result.json>).
[Expanded Rust 2021 repair source](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-sdk-complete-edition.qZSxlAMe/source.json>).
[SDK executor compiler result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-sdk-complete-edition.qZSxlAMe/cli/data-result.json>).
[SDK executor repair source](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-executor-fixed.7mPOarfd/source.json>).
[CLI ingress compiler result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-executor-fixed.7mPOarfd/cli/data-result.json>).
[CLI ingress repair source](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-cli-ingress-fixed.2FqB7KmF/source.json>).
[CLI binary compiler result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-cli-ingress-fixed.2FqB7KmF/cli/data-result.json>).
[CLI binary repair source](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-cli-bin-fixed.HhYA3vkE/source.json>).
[Verified CLI compiler result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-cli-bin-fixed.HhYA3vkE/cli/data-result.json>).
[S production result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-sql-cohort.Gx1Itc7s/production/data-result.json>).
[Later SQL reader compiler result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-sdk-resolved.J7zazY2S/cli/data-result.json>).
[S CLI and Original source](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-sdk-leaf.DuwZ9EQO/source.json>).
[RepoMap compiler result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-seal-schema.xjz9iXvj/cli/data-result.json>).
[Later CLI compiler result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-repomap-borrow.qWzNOJ0h/cli/data-result.json>).
[Earlier fixed CLI result](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-fixed-base.EjcA0mp1/cli/data-result.json>).
[Fixed S replay and compile input](</Volumes/Extreme SSD 1/build-cache/nc-h-close01-04-sql-cohort.Gx1Itc7s/source.json>).

The existing Q Kernel first held the `test-workspace-lane` compile lock
while the internal volume had approximately 548 MiB available. It released
the lane at 22:39 KST. Q Runtime4 then passed **4/4** in
`6a988b77-fcb1-4851-99be-e7fed604315e`. The existing Native overlay
request released the same lane at 22:57 KST. The UDS retry then passed
**1/1** in `7c17e7d7-3828-43a1-a7e5-48ff8d57b9ae` on the previously
compiled RepoMap source. After that dependency changed, Nextest
`9a2a521c-a90e-4e21-8308-68654fd083d8` passed **1/1**, exit 0, on Q
`65ef552d702d`. It checks retirement of the FULL view and Catalog row,
reopen on the successor DELTA, stable Published identity for the original
FULL Submit, and `DigestConflict` on changed bytes. The 1,354 Rust/Cargo
files consumed by this target and the Legacy reference binary matched
their pre-run hashes at completion. Signed Linux installation and
installed History acceptance remain deferred.
The terminal output was inspected before the host reboot. The transient
`/private/tmp` file was absent after reboot.
[UDS fixture](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/history.rs#L545).

The six original closeout items are mapped below. The focused Q/S checks have
recorded frozen results. Current-source qualification and the installed replay
remain open. The installed parts of H-INTEGRATE-01/02/03/04/05 use the existing
H-ACCEPT-01 fixture and its F guest and release inputs. Q's post-retirement
Submit proof uses the separate existing UDS fixture.

| ID | Concrete remaining work | Current evidence / completion condition | Writer boundary |
| --- | --- | --- | --- |
| Q Catalog single-workspace orphan-root guard · additional check | `boot_quiescence` now applies the canonical registered-row scan to the no-route BOOT, Source-root readout and rollback path. `single_workspace_boot_and_rollback_refuse_orphan_view_root` injects a checksummed root at a missing parent view, checks typed refusal and unchanged BOOT/root state, removes the row and checks acceptance. The current-source body **VERIFIED** 1/1 in Nextest `61d2f67d-6a74-4215-b711-c76aad391735` on the bound 382-file Q closure. F/Root must rebind this fix into its signed source. | Q Catalog owns code and the same existing lane; F/Root consumes the result without a second fixture. |
| Q Catalog orphan-root/BOOT · additional check | Rerun `orphan_view_root_with_registered_workspace_refuses_current_and_retained_routes` and `protected_native_import_is_atomic_exact_and_retryable` on the current Catalog source. Earlier Nextest `a6dcf064-be31-4c9e-b4ce-ed2c3f5b30ec` passed 2/2 on its recorded source, before the current guard and callsite changes. Current-source bodies **VERIFIED** 2/2 in Nextest `61d2f67d-6a74-4215-b711-c76aad391735` on the bound Q closure. | Q Catalog owns the existing `test-workspace-lane` check. |
| Q Catalog protected route/Source/export · closeout 03.7 | The six distinct bodies listed in [closeout 03.7](../../nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md#완료-조건) cover selected workspace floors, orphan protected rows, a second Source-key encoding, a foreign Work payload, an epoch-head floor, and an unrepresentable V1 History row. All six current-source bodies **VERIFIED** 6/6 in Nextest `61d2f67d-6a74-4215-b711-c76aad391735` on the bound Q closure. F/Root still needs this fix in its selected signed source. The closeout Catalog 19/19 uses different selectors and does not discharge this row or the two orphan-root/BOOT bodies above. | Q Catalog owns the existing lane; F/Root consumes its result without a duplicate fixture. |
| H-CLOSE-01 · 1 | Verify the adopted S CLI History/RepoMap/SearchCorpus callbacks and Original JSON/compact replay staging | The caller passes all three callbacks with the Original aggregate's Required History receipt identity. CLI **VERIFIED** compile, exit 0, in `20261122T182431.287276Z-c4208e5f9d37` on the fixed source above. Original replay **VERIFIED** 2/2, exit 0, in `20261122T231637.103774Z-42eab1cab035` on the fixed macOS test source. The two witnesses cover cancellation, retry object identity and staged byte return. | S CLI and resident candidate owners own callback and Linux checks. |
| H-CLOSE-02 · 2 | Verify checkpoint framing, FULL/DELTA reopen, replay memory return and a large DELTA under 64 MiB | The four `e2e_auxiliary_catalog_history::history_native` bodies **VERIFIED** 4/4 with exit 0 in Nextest `6a988b77-fcb1-4851-99be-e7fed604315e`. The observed 1,374-file Rust/Cargo closure matched through completion while checked-out HEAD advanced. UDS retry **VERIFIED** 1/1 with exit 0 in `9a2a521c-a90e-4e21-8308-68654fd083d8` on Q `65ef552d702d`. | Q Runtime owns the no-default `process_readiness_owner_v1` target and the no-default `native_index_uds` retry target in `test-workspace-lane`. |
| H-CLOSE-03 · 3 | Verify S Required History DATA after a private-terminal budget refusal and in the SDK retry failure packet | **VERIFIED** 2/2 on frozen S `d75e8d6b987` and Q `252ec2ec7` overlay `15bf9af2e2136a82b082e3d25f6549f3`. Run `20261122T121408.682064Z-b5d727533865` records owner `GREEN` and exit 0. Both the budget refusal and SDK failure-handoff witnesses executed. | Existing `history-source-readback` request `a10740d1c02c9248cd1ff50637c9022c`, class `0ac60ceaae02`. Installed/restart proof remains on H-ACCEPT-01. |
| H-CLOSE-04 · 4 | Verify the S production no-default class | **VERIFIED** compile, exit 0, QBC run `20261122T160639.418133Z-f4c56d306c5e` on fixed S `f024f965d8c+ovl-c3c32820…` and the recorded Q sibling. The receipt and raw compiler log match their recorded hashes. Class `73ab9f2c701b` uses `original-snapshot-v3,search-plane-sdk-ingress-product-proof`. | S Runtime production request `7a5d32463bc71f1b298bdf4c00199257`. |
| H-INTEGRATE-01 · 5 | Qualify authenticated SQL snapshot bootstrap when S originals are unavailable | The [ADR's pre-ingress inventory, checkpoint and FD3 handoff](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover) are supplied. The signed fixture checks incomplete inventory refusal, seal, BOOT and the exact retained terminal for a Q-only cut with no S original. The installed body and production-root census, terminal ACKs and readback are **NOT_RUN**. | Q owns physical read and final verification. D owns launcher and S candidate preparation. S cutover journal owns the retained terminal. |
| H-INTEGRATE-04 | Verify the adopted Source/History DELTA and retry across Q commit and S terminal append | Reuse the [Source/History CAS and V9 contract](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover). Verify owning compilation and terminal persistence, then bind same-operation retry to the installed Published view, predecessor cut and immutable History input. Installed successor execution is **NOT_RUN**. | Q owns transition and CAS. S producer journal owns live/cold binding and terminal evidence. |
| H-INTEGRATE-02/03 · installed subconditions | Ordered retained-generation FULL and same-candidate Published retry are source supplied. Q's earlier ordered-FULL component run `e50615f9-ce4a-4ea1-86cc-5ef8c1eeb76a` passed 4 selected cases. The Q UDS History fixture covers original Submit after Exact retirement and reopen ([History UDS fixture](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/history.rs#L545)). Its local no-default body **VERIFIED** 1/1 in `9a2a521c-a90e-4e21-8308-68654fd083d8`. The signed H fixture checks direct S History publication, launcher reuse and a second identical launcher build before selection. That Linux body is **NOT_RUN**. Candidate mode has retention disabled. After cutover, the launcher uses a new selection CAS input. | H owns History input and retry assertion. D owns candidate orchestration. F owns the signed run. |
| H-ACCEPT-01 · 6 | Qualify S producer → installed History FULL, successor DELTA and restart | `native-installed-history-full-delta-process-owner` selects `installed_native_history_full_then_delta_retains_generations_v1` on F's signed private retained bundle. The [History ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover) owns the exact two cuts, Source-bound successor, ACK-loss and restart oracle. The installed body is **NOT_RUN**. | [D](NC-D-boot.md) owns install and launcher. [F/Root](NC-F-integration.md) owns signed Linux execution. H owns the History fixture and writer. |
| H-INTEGRATE-05 | Qualify installed cross-revision History | Q BOOT snapshot identity and installed-row rejection are **VERIFIED** in `4340d420-fc85-446b-8bfa-6a339a15739a`. The same-ordinal multi-revision inventory is **VERIFIED** in `3eccd731-5e31-4d07-915b-f18d56f9d63b`. The installed cross-revision cut is **NOT_RUN** and uses H-ACCEPT-01. | H owns History candidate targets and cross-revision proof. |

**S admission history before the 2026-11-22 compiler runs.**
H-CLOSE-01, H-CLOSE-03 and H-CLOSE-04 were routed to
the existing `history-source-readback` lane on the recovered SSD target.
Their first owner runs were **BLOCKED** by the host compile-activity lock
(exit 75, no Cargo execution). After the lock opened, three CLI retries with
the existing SSD snapshot root exited 1 before Cargo. Concurrent Java and
Taint input edits interrupted raw hashing or changed the live S/Q/GQLang
source closure during capture. The first attempts ran no compiler checks
or Required DATA bodies. On recheck, Q was still changing within the
S/Q/GQLang closure and the host lock was occupied again.
When admission opened again, CLI request `c9c34b69ff14f9e7e6fa1dc7743822db`
resolved on `history-source-readback` to `quanta-cli-server` with
`dist-full,installed-product-authority`. Its SSD-backed run exited 1 before
Cargo because the live S source changed during closure capture. Current CLI
compilation and the downstream S checks were **NOT_RUN** at that point. The host
lock was available on recheck and no run occupied the owning lane. S HEAD
continued changing during the recheck, so the same capture was not retried.
The latest `quanta-build-cli owner resolve` confirmed the same request ID
and feature pair. S source edits were active again after the host restart.
On Nov 21 the existing registered SSD lane accepted request
`c9c34b69ff14f9e7e6fa1dc7743822db`. Source capture failed before Cargo when
`third_party/roxmltree/src/parse.rs` changed during raw hashing (exit 1).
Current CLI compilation remains **NOT_RUN**.
The next request in the same lane resolved the existing CLI check with the
SSD capacity available. Capture exited 1 before Cargo when the live
`ordinary_aggregate_prepare_v2/resume_v2.rs` changed during raw hashing.
CLI compilation and the new Original replay bodies remain **NOT_RUN**.
The latest observational QBC receipt for this existing CLI request is
`20261120T071930.313723Z-a53bc5bfb5c5`: exit 125, no Cargo body or compiler
result. The owning lane was unlocked at the 2026-11-21 recheck, but S HEAD
changed again during the audit, so no matching current-source snapshot was
established. The SSD then had about 54.9 GiB / 2.95% free, above the QBC
fresh-activation floor of 50 GiB / 2.75%; capacity at that instant does not
explain the failed source capture.
On the latest Nov 21 recheck, the registered `history-source-readback` lane
was unlocked with no target, the SSD had about 164.2 GiB / 8.21% free, and
other owners held the host compile lock. H-CLOSE-01/03/04 were not resubmitted;
their current-source compiler and DATA bodies remain **NOT_RUN**.
The existing owner request IDs are CLI `c9c34b69ff14f9e7e6fa1dc7743822db`,
Required DATA `1a60cf98354ccd4ad794fee4cda31a89` and production no-default
`7a5d32463bc71f1b298bdf4c00199257`; no parallel request was created.
A later bounded CLI `owner run --wait-seconds 120` on the same SSD lane exited
1 before Cargo: live `search_plane_query_ports.rs` changed during raw source
hashing. S History producer/candidate code files stayed clean through the
HEAD changes observed by this audit. CLI compilation, DATA and production
no-default current-source results remain **NOT_RUN**.

After the external volume remounted, the same CLI owner request again stopped
before Cargo because `repo_map_export_bundle/native_digest_v3.rs` changed
during source hashing. The owner front door supports CLI checks against a
validated detached casectl `--execution-root` bound to matching
`CASECTL_EXACT_SNAPSHOT_RESOLVED_DIGEST_V1`. Its `--snapshot-source` convenience
option applies to test requests. The existing Required DATA exact selector
still resolves to request `1a60cf98354ccd4ad794fee4cda31a89`. No current
detached source root or compiler result was issued by this retry.
The next live check and a source-only capture through the existing QBC
snapshot helper both stopped when the Q sibling changed during capture.

**Earlier Runtime request.** The `df090e56a72f` working tree ran
`./scripts/cargow --lane test-workspace-lane nextest run --locked --offline
-p quanta-index-searchd-runtime --features legacy-tantivy --test
runtime_extended_suite --no-tests fail --no-fail-fast --test-threads 1`
with exact `-E` selectors for `native_history_full_delta_query_old_view_and_reopen`
and `native_history_replay_memory_retires_superseded_rows` in
`e2e_auxiliary_catalog::history_native`. Cargo exited 101 and both bodies
are **NOT_RUN**. Shared target metadata omitted the current Ciborium
`from_slice_with_definite_scalars` and normalizer `phrase_ranges_by` APIs.
The Lexical diagnostic for the latter names the previous D3 checkout.
The active NC-TEXT segmented reader also failed type inference for three
posting cursors. [NC-F](NC-F-integration.md) owns the scoped shared dependency
rebuild and [NC-TEXT](NC-TEXT-build-final.md) owns the segmented reader.
The checked Cargo and History test inputs matched before and after the run.
After the shared dependency refresh and the TEXT reader source repair, the
same exact selection was retried. Shared build admission timed out after
1,800 seconds (exit 124 before Cargo, zero bodies, checked inputs unchanged).
The History module subsequently moved to the no-default
`process_readiness_owner_v1` target. A separate frozen-source run passed the
first case 1/1 as recorded in the current status above. The three other
current cases and a live-source four-case result remain open.

The QBC detached S/Q source capture is verified for its recorded cohort above.
F/Root verified Q 49/49 and the selected Legacy and Native Linux ELF inputs.
S shipping and private late-peak Linux checks passed on their recorded
source; the retained postbind successor remains with the existing Root queue.
Signed installation and H fixture bodies are **NOT_RUN**.
The [F queue and Published handoff](NC-F-integration.md#root-단일-큐-현재-검증--2026-11-16)
owns current R16, Caller and supplier progress.

Two wider production routes require their own inputs and verification with
[D](NC-D-boot.md).

- Legacy roots with sibling active SearchCorpus or RepoMap heads require a
  complete physical active-head inventory and queryable revision-to-view
  mapping or a preserved serving route. The new protected-route, historical
  Source issuer and imported RepoMap adapter are source-present; combined S
  publication, current Q receiver bodies and per-revision query readback still
  require verification. The old single-head path remains strict when no sealed
  route is supplied. Q Catalog and D Source/RepoMap own their carry-over.
- Previous Native → fresh Native with History now has a v3 source carrier,
  immutable closure exporter/stager and exact old-root/candidate BOOT checks.
  The recorded Q Runtime receipt covers cross-root FULL/append/restart on
  its frozen source. Compilation and regression bodies for later changes,
  the current S compiler and signed installed recovery remain **NOT_RUN**.
  D and Q BOOT own selection and the physical receiver. H supplies immutable
  History coverage and row validation.
- The existing S installation fixture resolves and ranks the protected
  RepoMap over UDS before and after Native replacement and restart. It
  compares the Exact view, instance, member binding and rank result. It also
  compares the foreign Source's Exact view and query result, and checks all
  three retained readbacks on cold Q before resident reattachment. The
  fixture seeds two Source heads and one RepoMap head. Higher cardinality
  and separate RepoMap-only workspaces have Catalog coverage scopes. S checks
  `ReadImportedRepoMapMember` during imported member publication. The
  installed fixture remains **NOT_RUN**.

The BOOT regression also rejects a replay manifest pointing at a snapshot
checkpoint, a different durable sequence, a different source manifest, and a
content-addressed checkpoint page with a well-formed row outside the
authenticated cut. The admitted IPC regression checks one retained wire
allocation. The Linux
`native_uds_bound_admitted_projection_stage_crosses_single_frame_with_child_ref`
case also checks the actual peer-bound SDK request, child-bearing immutable
object identity and ingress discard. Run S CLI compilation on its existing owner rail. Reuse H-CLOSE-03's
recorded DATA body and refresh affected proof on the same S source when qualifying
that build. Take the admitted S/Q pair to the existing F signed retained guest
for the installation, ACK interruption, and restart checks.

The additional S Kernel paid-parts retry was **BLOCKED** before Cargo. Artifact
capacity was 50.1 GiB and 2.7% after automatic GC, below the required 2.75%.

Retry13 used the existing `history-source-readback-snapshot-artifact` owner lane
with paired source `1ede354134f932f60219080bd1d50e73e8013cd9+ext-34d5edae4531633270f4`.
The `quanta-runtime` integration target selected
`persisted_required_data_survives_late_private_terminal_budget_refusal_v3`
under `feature-isolation:quanta-runtime.no-default.7f44320823fb`.
The queued CLI check did not start because the same admission condition remained.

Retry14 of the isolated DATA/private-budget regression on frozen S
`4f6d7c59b9a` cleared host admission. Cargo **FAILED** before the test body
on an unused `SinkCategory` import in `quanta-contract-security-types-shared`
(`20261116T100842.758344Z-54030d4f1ff8`, exit101, 0 bodies). Live S commit
`a8944939cb1` removes that import. The pending Linux and isolated Runtime proof
must observe the repaired source; the earlier receipt retains its exact scope.
The retry on `history-source-readback` captured a predecessor source snapshot
and exited 1 without a Rust terminal result or DATA body. The later repaired
DATA run is recorded below. The signed Linux suite requires its installed Q/S
binaries, digests and signed inputs.

Two live CLI attempts on `history-source-readback` exited before Cargo when a
tracked S source changed during hashing. The frozen S/Q snapshot
`b2472b870ff07c61fa9b343205c218e3ee9e628b+ext-f326170806f8cd42a359` reached Cargo on
`history-source-readback-snapshot-artifact`. The S CLI binary check used
`feature-isolation:quanta-cli-server.no-default.69a9257c2d02` and **FAILED** before
the CLI crate (`20261116T164934.659198Z-27102e5a41ea`, exit 101).
`quanta-adapters-dfg` passed a native allocation admission where
`validate_target_scope_v1` requires a canonical semantic output admission
(`E0308`) and omitted `recursive_heap_path_v1` when rebuilding the Java direct
call state (`E0063`). The Java owners supplied both fixes in S `fd8bffaefe3`.
The second CLI run `20261116T171641.527410Z-fa7a579dd737` used the source pair
`fd8bffaefe37e6822a9a1ee2b052c06f5b1e6510+ext-52148576d32bb9e60912` and passed
the earlier DFG diagnostics. It **FAILED** (exit 101) in `quanta-taint-solver`
because the frozen commit lacked `signed_java_return_v1/original_v3.rs`.
S `29e297cd4f3` added the file. The isolated DATA feature class reaches DFG
through `retrieval-authority-python-min` and does not select the CLI's Taint
Solver feature.

The isolated DATA run on that repaired DFG pair reached
`quanta-runtime-analysis` (`20261116T172318.081792Z-fe2db3e6daed`). It **FAILED**
with exit 101 before executing a body. `recursive_conditional_v1.rs` compared
the `u32` argument index with a `usize` enumeration ordinal. The existing S02
Java recursive profile claim owns that leaf. `ternary_source_v1.rs` passed four
use-index slots into the shared five-slot row carrier. The live ternary path
uses five slots throughout while retaining its existing shape and use count.
The adjustment passed scoped `rustfmt` parsing and `git diff --check`. S02
fixed the recursive comparison in `0de47bce1157` by using typed `u32` ordinals.

The third CLI check on
`971e7b66d939ae253beb74a0095e7c4e170d241a+ext-75334b8382e5d83b0647`
reached Taint Runtime Analysis
(`20261116T181052.982392Z-0941b5da7e9f`, exit 101). It found an
`EntityId`/optional DFG entity mismatch in `python_write_binding_v1.rs`, a
typed authority file comparison in `metadata_read_v11.rs`, and a Runtime
error propagation mismatch in `python_paid_v14.rs`. The isolated DATA
selector uses its existing narrower feature class on the same source pair.
The authority-file comparison now borrows its canonical string. In the current
S source the Python paid callback retains its typed Runtime result and returns
it when the DFG lender has completed. S05 supplied the canonical `EntityId`
comparison in `13ee4e88245`. Frozen S/Q
`6c217a59de3a60ba1aa859ec555b51d9061523ac+ext-c391187367dbca17ce7a`
contained the three source fixes. That check
(`20261116T190134.784695Z-f31f33a3f3cf`) **FAILED** in
`quanta-taint-solver` with exit 101 before compiling the repaired Taint
Runtime Analysis crate or the CLI binary.
There was one root cause in `original_full_flow_v1/switch_v1.rs`: the
import retained the old scalar-shape helper name after both calls switched
to `try_same_full_scalar_shape_v1`. The Solver owner corrected the import in
`1b7a598b2cc`.

Frozen S/Q
`f07b05c35e777230370d70ee43f986f4fcedd094+ext-de14324ff024d34f84d5`
passed the Solver import and Taint Runtime Analysis diagnostics and reached
`quanta-runtime` (`20261116T191634.386771Z-8f47f2bc57db`, exit 101).
The SDK model identity, synthetic honesty digest and request sizing matches
did not handle the new `StoredValue` sink variant. S commit `334cd4899a7`
adds typed site identity and matching digest and budget inputs to all three
consumers. The follow-up `e7aa67c57c7` adds a distinct-site model identity
regression. The CLI owner rail verified compilation past the SDK consumers
on the repaired pair
`e7aa67c57c73110d4bca072ea053fba87acc9104+ext-6fe4a4cdd9e500bb4b8b`.
It then **FAILED** in `quanta-sdk-runtime-composition-owner`
(`20261116T194339.454737Z-a91c83525562`, exit 101). The current Local fragment
had an inner doc comment despite being included as items. The extracted
current-role module referenced the analysis key and pipeline one level too
low. S `2bb9f88e74d` fixes the comment and both references. The current
composition crate's 20 Rust include targets also pass a static inner-doc
scan. The repaired CLI compile remains pending.

S `782db77f479` aligns the History candidate replay and writer-fence
helpers with the retrieval bundle that provides their implementation.
The current CLI distribution and Native SDK indexing entry points both
enable the Python-min retrieval bundle. Static inspection found no
unsupported call site under the narrowed gates.

The live Q overlay changed during both ordinary and sealed snapshot
capture, so those attempts ended before Cargo. The retry isolated Q's
3,274 tracked and standard untracked files outside `docs/` and verified
their paths, executable bits and contents against both live reads.
The CLI owner lane ran
`6d624ca802678bb36fda9900ac804303f8071f8c+ext-88463302332e99f6c4c5`
as `20261117T011344.045385Z-469312752a14` and **FAILED** in Java Map DFG
(exit 101). The Port use receiver needs `u16` slots. The DFG owner
corrected the checked conversion and routed both Map uses through the
existing `AssignCall` emitter. The call context still binds the exact
method and argument. Taint and PTA now compare that kind. H reconciled with
the existing Runtime writer and adopted the four matching `AssignCall`
comparisons in the two Runtime validators. Rust parsing and scoped diff
checks passed, and all referenced Port statement variants exist in the live
enum.

The retry uses `7cd7c315b75` with the committed DFG, Taint and PTA fixes.
It tests the two Runtime comparisons as an isolated overlay and reuses
the exact Q and GQLang inputs from the previous run. All 17 referenced
`PortDfgStmtKind` variants resolve to declarations in this source.
The frozen CLI input was
`7cd7c315b75bf8d6a06ccf94e8a030d7eeb7292a+ovl-1000173d0f601b5f4326867a95a0a9ce+ext-88463302332e99f6c4c5`.
Run `20261117T015405.067830Z-622e5e309813` **FAILED** with exit 101 before
Rust compilation because `--locked` refused the outdated lock.
S `60dc03485e8` adds the declared `ed25519-dalek` and `tempfile`
dependencies to the test-support package's root lock entry and updates
the nested lock. The external SSD disconnected before the next
corrected-source run. On 2026-11-19, the external volume and the QBC
build-cache root were visible again. The old temporary runner-state files
were absent. Current source capture and CLI compilation are **NOT_RUN**.
F retains its own release intake.

<a id="s-cli-and-required-history-data-proof"></a>

The isolated DATA run on that pair reached `quanta-runtime`
(`20261116T182136.122109Z-181eac4d1000`, exit 101, zero bodies). The
Product consuming bridge wrapper was exposed when its generic method and
error type were disabled. The wrapper and imports now share their guard.
The Product taint caller's feature set already implies that guard.
The repaired pair
`2f7adcdb25dc5ae3191d23d140c96a7682b2867e+ovl-31528a4c686c78dcc7c5cefe477a97cd+ext-7f406ab277f6e498a90b`
executed H-CLOSE-03 in `20261116T184029.973262Z-d3d4c6ba39a3`.
The QBC integration target `history_required_terminal_budget_owner_v3` under
`feature-isolation:quanta-runtime.no-default.7f44320823fb` executed
`persisted_required_data_survives_late_private_terminal_budget_refusal_v3`
(1 PASS, 0 ignored, Cargo exit 0). The publication marker matches the
receipt SHA-256. The verification index separately records owner-level
exit 125 with `command_exit_code_v1=0`. This records the frozen isolated
test outcome. Current-source execution is **NOT_RUN**.

The modified `legacy_history_source_requires_exact_original_ack_and_complete_inventory`
case first expired twice at shared admission (180 s and 360 s, exit 124).
The same lane then admitted and **VERIFIED** the body in
`3eccd731-5e31-4d07-915b-f18d56f9d63b`.

The exact selector is
`installed_native_history_full_then_delta_retains_generations_v1`. The registry
selects `quanta-cli-server --test server_socket_daemon_e2e` on Linux with
default features disabled and `dist-full,gqlang-process-proof,native-retained-recovery-process-proof`.
Both proof features are needed for the test target. The separate installed
resident uses the signed four-role
`quanta-cli-product.release-full.linux-native-retained-recovery-process-build`
recipe, which enables its HistoryMain fault hook. The selector shares
fixture setup and recovery with the existing Source-only required suite. It
checks the selected FULL and the successor before reattaching S, so a cold
readback failure cannot be covered up by another publish. The fixture requires
Q Latest to resolve to the exact combined Published view both before and
after restart. The ordinary Product roster matches the 37 tests in its
feature closure. History and Host recovery share the signed test-only
retained bundle. The [F retained execution](NC-F-integration.md#root-단일-큐-현재-검증--2026-11-16)
runs both exact selectors in the same guest and requires both raw receipts.
The History fixture
checks every History slot in the selected profile and rejects
unexpected generations or activation. The existing
cutover response relay interrupts selection reconciliation. Writer recovery
uses `HistoryPublishedBeforeTerminalAck` from the combined HistoryMain branch.
The Linux resident compiled with `native-retained-recovery-process-proof`
reports its PID, Q operation and actual V9 closeout and attempt IDs over the
fixture's private socket and exits before writing the V9 terminal. The fixture opens Q cold first, checks exact
retained and successor cuts without S replay, then reattaches S and requires the
same operation and unchanged Q Latest. This selector is excluded from the
ordinary installed test inventory. Rebuild the existing retained-recovery
private recipe with the current S/Q sources and run the exact History suite
in the signed Linux environment. The required environment includes the
pinned Q runtime, launcher and source custody.

The registry names this single `expected_witness_names` entry and declares
`expected_ignored_names: []`. The recorded resolve-only result returned one witness under
`quanta-cli-server.no-default.daa6083623eb`. The fixture also opens S's
completed cutover archive before the resident restarts. It decodes the first
and completion FD3 datagrams with the Q contract decoder and compares the
operation, view, instance, member and cut with the BOOT receipt and live Q
readback. After same-request reconciliation it requires the original Applied
terminal and a Reconciled History completion with the same History binding.
This assertion is part of the existing signed selector and remains
**NOT_RUN**. The later HistoryMain restart fixture checks
the S V9 checkpoint ACK against the aggregate terminal, reopens the bounded
HistoryMain CBOR receipt by its BLAKE3 identity, and compares its typed Q
operation and Published view with the same request's authenticated binding.
The scenario compares that operation and the persisted V9 closeout and attempt
IDs with the original fault event.

The Apply runner now retains the raw CLI output. A missing JSON response still
leads to a bounded wait for the exact resident fault event. Ordinary Rename
continues to require a typed successful response. The fixture creates its
fault socket under a dedicated `0700` directory and passes the selected Q
workspace and epoch. The hook fires only for HistoryMain in that namespace.

The second Legacy History cut now carries a distinct detached Git commit
under a History-only revision at the same generation ordinal. HEAD and the
selected Legacy Source stay fixed. Bootstrap sorts by generation and
revision. The History-only cut sorts last so the successor must select the
earlier cut that matches its actual Source predecessor. Each cut has its own
expected commit set. The Git CLI confirmed distinct object identity with the
same tree and unchanged HEAD and checkout.

The archived History batch has no Source manifest. Its fixture compares the
terminal ACK with the optional manifest value of the batch it actually
published. The signed original Source binding stays at the active cut. Rust
compilation and signed Linux are **NOT_RUN**.

The signed snapshot fixture first builds a separate candidate with one of the
two physical History generations omitted. It requires the producer's exact
inventory-mismatch error and the absence of a candidate selection and BOOT
receipt before proceeding with the complete target list. This installed case
is **NOT_RUN**.

### Already complete; do not recreate

- **H-INTEGRATE-02 — Ordered FULL for retained History generations (source supplied).** Q captures intermediate publications with the selected BOOT in one Catalog transaction, checks each immutable request and predecessor profile, and streams input to the Legacy census. S compares the complete ordered selection with the producer inventory under one fence. The candidate accepts `--native-candidate-history-targets-json`, appends one FULL per verified generation and reports the final operation. It rejects History operations on the live Source stream before contacting Q, so bootstrap cannot consume the stream used by the later Source DELTA. The Q component run `e50615f9-ce4a-4ea1-86cc-5ef8c1eeb76a` **VERIFIED** four selected cases. S compiler and installed proof remain pending. H producer and candidate codec, Q Kernel/BOOT and Catalog owners.
- **H-INTEGRATE-03 — Same-candidate retry after History commits (source supplied).** Resident and standalone preparation start from the attested AST operation and pass each checked predecessor to the next step. For an existing operation, S derives the retained replay or checkpoint input reference without an ingress and verifies the exact immutable request through idempotent Submit. Pending can Resume after verification. Q Published Submit must return the same terminal view after Exact query view retirement; the Q UDS fixture covers this operation boundary separately. S candidate retry runs before selection, when retention is disabled. Current-source Q UDS and signed S retry bodies are **NOT_RUN**. See the [ADR retry contract](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover). D owns candidate orchestration. H supplies exact input and request verification.

- The signed History fixture publishes through standalone installed S on the
  unselected launcher root and drains Q while retaining the terminal only in
  the test. The real launcher then restarts Q on the same root, invokes S, and
  returns the same root nonce, History operation, source cut and Original FULL
  sequence. The later launcher retry separately checks retained control-file
  reuse. The signed fixture is **NOT_RUN**.
- The direct candidate in the same signed fixture now reuses its published
  Source and AST to replay the first S-owned History generation. It compares
  the final operation and queried Git commit against the S publication and
  verifies the direct BOOT receipt. It then reopens the same Legacy
  SearchCorpus root to add the second, Q-only History generation. The existing
  two-generation launcher path still uses the physical checkpoint and
  verifies the complete Legacy census during restore-forward. The Q-only
  cut never had an S original. S's V3 retention cleanup is a separate
  lifecycle and remains unexercised by this fixture. Both signed paths remain
  **NOT_RUN**.
- The six installed candidate and recovery suites require Legacy Q executable
  and SHA inputs alongside the Native pair. The shared S fixture checks both
  hashes, starts and reopens SearchCorpus and RepoMap with the Legacy image,
  and uses the Native image for the candidate, BOOT and recovery helper. It
  checks each selection document's executable and SHA. Recovery installs
  manifest-backed Legacy bootstrap roots before publishing into them and
  reuses those same roots for History census and rollback. The existing
  single-ELF Product runner supplies its verified default-feature Q in both
  roles and validates those exact values in the retained receipt. F's separate
  dual-ELF witness requires distinct hashes. F's public and retained suite
  input checks and the focused Product runner tests (5 PASS) are **VERIFIED**.
  The empty Legacy root sequence was exercised on Darwin using a retained
  Legacy-enabled Q binary with SHA256
  `c85716969d33f6e41efcc279e666ad4891a2de5fdd3bc5b30c9173296e5ecadf`.
  It rejected a manifestless serving root under `verify-state`, then passed
  backup, restore, verification, launcher install and restart. The fresh
  Legacy binary build timed out before acquiring the shared build slot. The
  signed installation remains **NOT_RUN**.
- The existing Q IPC/SDK lane **VERIFIED** both libraries compiled and the
  three selected codec bodies passed in `8e44f333-d58c-4099-803a-ddbfe552ff1a`.
  The bodies cover the admitted single-frame wire, large multi-frame round
  trip and corruption, and the retained allocation boundary. The Linux
  pinned-peer Native UDS stage and installed History paths remain separate.
- The actual Linux ARM64 peer-bound UDS path **VERIFIED**
  `native_uds_bound_admitted_projection_stage_crosses_single_frame_with_child_ref`
  with 1 PASS / 78 filtered / exit 0. The first complete compiler attempt
  caught a Linux-only ignored atomic return value in the test, now fixed.
  The body exercised a retained peer and a real child-bearing Stage request
  exceeding the single-frame limit, and compared Q's installed object
  identity. This used the frozen Q sibling of S `971e7b66d93` with the single
  test fix applied in its Docker source copy. Signed S/Q installation is
  **NOT_RUN**.
- The Q `test-workspace-lane` run `4340d420-fc85-446b-8bfa-6a339a15739a`
  **VERIFIED** four bodies (4 PASS / 1991 skipped / exit 0). The tests cover the
  canonical stage-page frame bound, BOOT rejection for invalid snapshot
  identity and rows, existing-only immutable map scanning, and actual SDK/UDS
  FULL → DELTA → restart. The UDS case checks that the old Exact view is
  unavailable after retirement, identical FULL retry returns its recorded view,
  and changed input with the same key returns `DigestConflict` before and
  after restart. This run used Darwin. The signed installed S/Q path is
  **NOT_RUN**.
- The same Q SDK/UDS FULL→DELTA→restart case and
  `source_and_next_history_generation_publish_in_one_view_and_reopen` also
  **VERIFIED** on Linux ARM64 (2 PASS / 77 filtered / exit 0). Both exercised
  real Q publication and reopening. The Linux run used
  `scripts/cargow --lane test-workspace-lane test --locked --offline -p quanta-index-searchd-runtime --test native_index_uds`
  with the two exact `read_sessions::query::history` selectors in one process.
  The signed S/Q installation remains **NOT_RUN**.
- Q admitted Catalog publication-chain capture, exact Legacy original and row
  observation, fresh Source/root binding, and applied History journal ACK lookup
  **VERIFIED** in `e50615f9-ce4a-4ea1-86cc-5ef8c1eeb76a`, 4 PASS / 1045 skipped /
  exit0, on dirty Q `ade5fadc1`. The run does not cover installed candidate
  retention policy or the S Linux CLI.
- Q Native candidate restart, predecessor retention, and the existing BOOT
  and active retention process cases **VERIFIED** in
  `78eda16f-ce1a-478b-bd2a-667e5b4b6c8e`, 3 PASS / 55 skipped / exit0, on dirty
  Q `ade5fadc1`. Candidate mode preserves the operation chain, and regular
  serving applies the same configured automatic retention policy.
- Q paged snapshot FULL → Native append → restart and same-key retry, retained
  original replay, immutable page/ACK validation, independent Catalog checkpoint
  verification, and History fence identity **VERIFIED** in
  `e4c35a20-efc3-4698-bcae-e26f57f4eb49`, 5 PASS / 1371 skipped / exit0, on dirty
  Q `ade5fadc1`. This run covers the native UDS and component paths. Installed S
  producer export, checkpoint adoption and the signed subprocess remain **NOT_RUN**.
- Q shared immutable input identity **VERIFIED** in
  `a6f706a4-1900-4865-b8cc-fbde225fad96`, 3 PASS / 219 skipped / exit0. The
  Storage tests compare prediction with durable install, retain the physical
  child-existence check, and preserve the fixed `QIOBJ001` wire bytes.
- The combined Q build exited 101 at a concurrent `DiagnosticCounterV1`
  change. The next source observed the repaired enum and found a hardcoded
  Kernel test fixture array length, now removed. The next build exited 101 on
  an unused `Write` import in the borrowed exporter encoder. The repaired
  `test-workspace-lane` run includes SDK readout, FD7 startup, the combined CAS
  with derived outputs, paid request decoder, Catalog pager, exporter CLI and
  the real stale-selection process case. The malformed-FD7 case is Linux-only.
- `python3 -m pytest -q tools/ci/tests/test_deployment_launcher.py -k
  build_native_candidate_reaps_before_boot_and_reuses_durable_full` **VERIFIED**
  two cases with two Linux-only skips on Darwin. A later isolated Linux ARM64
  run with dependencies pinned by `uv.lock` **VERIFIED** six bodies
  (6 PASS / 141 deselected / exit 0), including the long procfd socket
  configurations, rejection of a listener owned by another PID, and BOOT
  refusal after interruption during drain. The tests use fixture child
  processes. The signed installed S/Q drain/export/FULL/restart remains
  **NOT_RUN**.
- Nextest `3fb00849-5762-4343-b0ff-3482c700ad47` executed seven bodies with
  four passes and three failures. The SDK admitted reader lacked fixed-size
  backing rules for the derived object references. The successor round-trip
  omitted profile admission after restart. The Legacy switch fixture omitted
  its three published Source handoff axes. All three corrections are in live Q.
  The expanded run that added installed witness wire coverage stopped before
  bodies on the existing HistoryCut fixture's raw array `ObjectRef` argument.
  The fixture now passes a typed `Digest`.

Representative Q commands for these receipts:

```sh
NEXTEST_TEST_THREADS=1 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -j 1 -p quanta-index-searchd-runtime -p quanta-index-catalog --lib --no-tests fail --no-fail-fast -E 'test(legacy_history_recovery_observes_every_intermediate_publication) | test(legacy_history_source_requires_exact_original_ack_and_complete_inventory) | test(fresh_restore_candidate_requires_exact_boot_and_root_scoped_source) | test(admitted_terminal_read_uses_original_sequence_and_refuses_retired_floor)' --status-level fail --final-status-level fail
NEXTEST_TEST_THREADS=1 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -j 1 -p quanta-index-searchd-runtime --test native_product_process --no-tests fail --no-fail-fast -E 'test(native_candidate_retains_predecessors_under_installed_retention_config) | test(native_boot_selection_verification_and_selected_serve_preserve_real_publication) | test(product_daemon_applies_retention_config_preserves_current_query_and_reopens)' --status-level fail --final-status-level fail
```

- **H-CLOSE-04 — production no-default after cfg repair.** **VERIFIED** compile, exit0 (`20261116T022818.404013Z-91bce8b69094`) on `feature-isolation:quanta-runtime.no-default.73ab9f2c701b` with `original-snapshot-v3,search-plane-sdk-ingress-product-proof`. Existing S production owning rail. This receipt covers compilation at its recorded source. The CLI and DATA owning requests are tracked above.
- Q producer-scope genesis and fresh restore/root binding: **VERIFIED** in
  `5dcc1052-7770-4fdf-a88e-59bba7799250`, 2 PASS / 659 skipped / exit0,
  bodies 4.927s, dirty main base `ade5fadc1a084fbb81eba831861e996081d80867`.
  The exact selectors were
  `legacy_history_source_requires_exact_original_ack_and_complete_inventory`
  and `fresh_restore_candidate_requires_exact_boot_and_root_scoped_source`.
  They reject foreign writer-state root, workspace root, contract key and a
  recomputed chain over a forged ACK, while preserving the fresh restore
  candidate's exact boot/root Source binding. This is Q focused component proof.
  Command:

  ```sh
  NEXTEST_TEST_THREADS=1 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -j 1 \
    -p quanta-index-searchd-runtime --lib --no-tests fail --no-fail-fast \
    -E 'test(legacy_history_source_requires_exact_original_ack_and_complete_inventory) | test(fresh_restore_candidate_requires_exact_boot_and_root_scoped_source)' \
    --status-level fail --final-status-level fail
  ```

- Earlier Q Runtime2 and bootstrap component bodies: **VERIFIED** on their
  recorded source with
  `./scripts/cargow --lane test-workspace-lane nextest run` on
  `quanta-index-searchd-runtime --lib --test runtime_extended_suite`.
  Run `b143dd0c-58e5-487f-a53e-85dabeab1e84`: 4 PASS, 763 skipped, exit0.
  The selected cases cover FULL/DELTA, query and reopen, replay memory,
  the exact Legacy generation scan, and Legacy terminal/quiescence source
  byte preservation. This is Q component coverage.
- Q producer-inventory and recovery components: **VERIFIED** seven selected
  `cargow nextest` cases in run `c404948a-ab0c-4c5e-8384-14e563c48708`, exit0.
  That receipt predates the new producer-scope genesis check. The physical
  Legacy proof rejects missing inventory and a recomputed
  replay chain over a foreign ACK. The existing Native UDS case then ran with
  two actual Legacy SDK History publications, canonical replay positions,
  Native FULL and DELTA, and a query after restart. Run
  `ebf6f64e-6125-400e-8405-6fc4112cdd4c`, 1 PASS, 74 skipped, exit0. The
  Legacy fixture runs in its own process because each runtime mode installs
  a process-global SQLite VFS registration. Installed S/Q migration is a
  separate process qualification.
- Exact-generation durable readback and legacy publication/reopen source proof2:
  VERIFIED `14353f03-daf2-4e98-b0bf-219ce466f24a`.
- Q carrier/replay/scoring/planning library7: VERIFIED
  `500f188f-be51-4f62-8463-f47426e27221`.
- C83 actual Q SDK/ordinary CLI/UDS fixed-input FULL/DELTA/pages/reopen1:
  VERIFIED `8e36bbaa-3cef-40d8-959a-4f0b79302be0`; snippet gold repair closed.
- S authenticated per-publication History loan kernel fixture1: VERIFIED in the
  recorded owning run. This is independent of the actual caller mismatch above.
- Read-only snapshot authority and Native V5 RepoMap original-getter investigations:
  complete. Native RepoMap output implementation remains its existing owner;
  it is not a new H adapter task. ANN, SDK60, Tantivy/lock/vendor removal and
  broad Root installation work are not added to H's independent coding scope.

### Executed Q Runtime2 command · VERIFIED bodies2

```sh
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=7200 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-searchd-runtime --test runtime_extended_suite \
  --no-tests fail --no-fail-fast \
  -E 'test(native_history_full_delta_query_old_view_and_reopen) | test(native_history_replay_memory_retires_superseded_rows)' \
  --test-threads 1 --status-level fail --final-status-level fail
```

Original limits and default features stayed unchanged. This user-requested run
completed and released its cooperative Q resource lease. Runtime leaf SHA256:
`99d00718c65a1d1868ee16bef714dcb640d652d4fea3185de76f5164a0e1a4a5`.
The historical `runtime_extended_suite` target and Runtime `legacy-tantivy`
feature have been removed. The current History cases mount on the no-default
`process_readiness_owner_v1` target.
This re-audit used local source and recorded receipts, not external chat status.

## Recorded Q component bodies · library7, Runtime2 and C83 VERIFIED

Root's single admitted queue returned the actual library result:

- nextest run: `500f188f-be51-4f62-8463-f47426e27221`
- **7 run / 7 PASS / 1032 skipped / exit0**
- compile:25.79s; bodies:0.092s; terminal result collected and execution slot returned
- Scope: source carrier1, source replay2, pure scoring2, public planning2.
  Owning packages: `quanta-index-contract`, `quanta-index-search-plane`, `--lib`.
  The seven exact selectors are the non-Runtime entries in the request below.
- This supersedes the previous two compile101/body0 attempts; those were not
  body REDs. The 22 recorded source identities are historical and retained in the
  [recovery entry](../../../ARCHIVE-INDEX.md#2026-11-16-history-contract-and-text-plan-consolidation);
  they are not a claim that today's shared source is still frozen.

### C83 initial fixed-input SDK/CLI/UDS fixture VERIFIED

Root collected the Text single-queue terminal result:

- nextest `8e36bbaa-3cef-40d8-959a-4f0b79302be0`
- **1 PASS / 74 skipped / exit0**, bodies:3.742s
- exact selector: `native_history_sdk_full_delta_retained_pages_and_reopen`
- Current input boundary: the History module mounts unconditionally in
  `native_index_uds`. `just rust-test-native-history` selects it with no
  default features and validates the pinned external old-Q binary and its
  SHA-256. The current local UDS result is in H-CLOSE-02 above.
  Signed installed process verification is deferred.
- fixture `searchd-runtime/tests/native_index_uds/read_sessions/query/history.rs`
  SHA256 `4213971c90321b0d816286ae714d460259dd1bab1217bdbe71332aefd7c5dfc2`
- scope: fixed original batch/ACK input through actual History port, Kernel CAS,
  public SDK, ordinary unprefixed CLI and UDS; FULL/DELTA, retained pages and reopen.
- The preceding actual C83 FAIL (`ca9417bf...`, 1 FAIL/74 skipped/exit100) was the
  fixture's added-text snippet expectation conflicting with the existing common
  touched→added→removed→header projection. C repaired only the exact fixture gold
  to literal equality plus actual-row diagnostic. H producer/SDK code and the
  existing converter were unchanged. No additional snippet RCA/patch/run is needed.

**VERIFIED:** standalone Runtime immutable storage/typed build/prior/FULL→DELTA/
reopen2 in `f366f01b-b802-471c-9b08-9196df56a1e2` above.
**NOT_RUN:** broader managed GC qualification and installed source snapshot
export/bootstrap. C83's fixture passes do not prove S's installed producer
or actual whole-History migration. The complete existing-History authenticated input is not supplied;
this does not establish data absence or forbid source-authority snapshot export.
Read-only findings and the narrower boundary are in the audit below.

## Implemented History contract — reuse

The existing source carrier, typed build/read ports, row map and query semantics
are consolidated in the [Native History ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#native-history-publication-and-read-contract).
Library7, Runtime2 and C83 above supersede the H-API-07 compiler-retry staging.
Do not reopen carrier/codec/scoring/planner/SDK fixture wiring as new work.
Root owns shared profile/CAS/GC and Runtime mounts; H owns row/hash/adapter
semantics; C owns public wire/SDK/CLI leaves. The actual S caller, source-owned
snapshot export/high-water association and installed migration remain in the
active backlog and design below.

### Historical compiler repairs

The first two H library attempts were compile101/body0, not behavioral REDs.
The recorded repairs imported HistorySourceReplayV1, removed unused imports,
matched CborPayloadLengthProgressErrorV1::Codec, and bound/reused the validated
top_k in the existing reader/planner. API, error priority, limits and selector
sets did not change. Changed5 and changed2 rustfmt checks passed in their
recorded source scope; the later library7 receipt supersedes retry instructions.
No source freeze or runner reservation is issued by this historical note.

## Recorded Q verification scope

Library7 and Runtime2 executed the original nine selectors below in separate
owning runs; C83 has its own SDK/CLI/UDS fixture receipt above. This combined
request is historical, not a pending rerun or execution reservation. The runs
used shared HEAD `ade5fadc1a084fbb81eba831861e996081d80867` plus their recorded
dirty source with Runtime's default `legacy-tantivy` feature. This is a
historical command. The current History Runtime cases mount on no-default
`process_readiness_owner_v1` after retirement of `runtime_extended_suite`
and the Runtime `legacy-tantivy` feature.

```sh
QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline \
  -p quanta-index-contract -p quanta-index-search-plane -p quanta-index-searchd-runtime --lib --test runtime_extended_suite \
  --no-tests fail --no-fail-fast \
  -E 'test(history_source_carrier_keeps_originals_and_rejects_schema_drift) | test(complete_source_replay_keeps_moves_deletes_and_independent_row_digest) | test(source_replay_refuses_missing_duplicate_foreign_body_and_incomplete_boundary) | test(native_history_boolean_bm25_uses_only_matching_branches_and_complete_population) | test(native_history_relevance_refuses_unscored_alternatives_and_preserves_phrase_positions) | test(native_history_public_plan_reuses_canonical_lowering_and_rejects_wrong_kind) | test(native_history_public_plan_retains_original_byte_and_cancel_refusal) | test(native_history_full_delta_query_old_view_and_reopen) | test(native_history_replay_memory_retires_superseded_rows)' \
  --test-threads 1 --status-level fail --final-status-level fail
```

The carrier contract test uses independent fixed wire data and rejects missing,
unknown and duplicate schema fields. Runtime adapter test now exercises the actual
Core typed port for FULL and DELTA with distinct source/artifact refs, plus wrong
mode, full-as-delta, forged actual prior context, changed original body/ACK with
unchanged next cut, and generated artifact substituted as source. Existing fixed
BM25 gold, filter-before-top-k, both record populations, old view/reopen, ref move,
tag delete, cursor refusal and DATA release remain. Memory test replays 301 unique
publications with bounded final state under unchanged 512 MiB scope.

Recorded static checks on the nine H leaves: rustfmt, test-authority,
hexagonal and derive audit passed (violations0). Whole-workspace derive check
FAILED118 on its dirty cohort; H removed its initial serde derives and left
foreign findings intact. Expanded22 rustfmt failed on concurrent readiness mount
formatting; the narrower H9 check passed. The carrier test now checks all three
Map::remove results; the superseded carrier hash is retained in the recovery
entry. These static checks do not replace the separate body receipts.

Broader managed GC, source snapshot bootstrap, S actual producer caller and
installed migration remain outside the Q fixture results. Their required proof
is retained in the backlog and source-authority design below.

## Existing body evidence and outstanding producer scope

- **VERIFIED Q source readback:** nextest `14353f03-daf2-4e98-b0bf-219ce466f24a`,
  2/2 passed, 1124 skipped, exit0. Exact-generation WAL snapshot + real two-legacy-
  publication socket/reopen readback. Command:
  `QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1800 ./scripts/cargow --lane test-workspace-lane nextest run --locked --offline -p quanta-index-catalog -p quanta-index-searchd-runtime --lib --test runtime_extended_suite --no-tests fail --no-fail-fast -E 'test(history_scan_keeps_epoch_and_rows_in_one_wal_snapshot) | test(history_source_readback_binds_two_legacy_publications_after_reopen)' --test-threads 1 --status-level fail --final-status-level fail`.
- **VERIFIED S authenticated per-publication History loan:** selector
  `index_sdk_ingress::terminal_receipt_v1::tests::funded_history_loan_checks_receipt_before_visiting_original_batch_v3`,
  1/1 body with five refusal/lifetime branches. QBC run
  `20261114T153227.680089Z-d5d4ee2c38ae`, exit0, source
  `4fc2c406d5c6209271b70a06730f98a5612cde90fe846dcb3af6a1574b9fd4af`.
  `with_native_cutover_legacy_terminal_with_original_v1` supplies authenticated
  Required body/full ACK/source identity, or authenticated NotApplicable policy.
  Aggregate validation must succeed before callback output becomes authoritative.
- **Historical FAILED S Runtime compile / body0:** exec2595,
  run `20261114T161859.819264Z-957f1ef88e3a`, exit101, source
  `ea428519079703a545c23e5d86c361da8b8bb45011ab5c7666d463ae3420dc2b`,
  snapshot `97d4837547e91bf0e80078b67de184a5ad9a2017+ext-b2c982e760815b96eda5`,
  result `/private/tmp/history-loan-runtime-test-fixed-result.json`.
  Exact selector:
  `retrieval::port_impls::index_projection_writer::prepared_artifact_readback_v3::tests::terminal_integration_v3::persisted_required_data_survives_late_private_terminal_budget_refusal_v3`.
  Earliest foreign PTA E0502:
  `queries/pta/precise_query/extraction_solve/worker_cfg_dfg_file_batch_builder/original_core_file_birth_v4.rs:1702`
  (`final_body_v4` immutable borrow overlaps `final_uses_v3` closure mutation).
  H did not edit that owner hunk. Later attempts and the current exact selector
  state are reported in the active backlog; this receipt alone has zero bodies.
- S production no-default class
  `feature-isolation:quanta-runtime.no-default.73ab9f2c701b` previously FAILED6
  E0599 in `resolution_source_context_cache.rs`: PTA-off placeholder args invoked
  a PTA-only getter. H cfg-gated only that getter in the shared Original/cache-
  bypass predicate, preserving Original bypass. Static boundary preflight exit0
  (exec79514). The later exact production class compiled successfully as
  recorded in H-CLOSE-04. Parser E0283 prerequisite was repaired using explicit
  `S::Error::from(WireErrorV3::NonCanonicalEncoding)` and compiled97d.
- **Unverified actual migration input, narrower than a data-absence claim:** an
  actual preserved-root census and settled source-snapshot/replay binding have
  not been executed. It is incorrect to infer that no complete History data
  exists, or to require every genesis publication archive for every valid
  migration route. The H07 carrier is publication-replay based. The Q snapshot preparation helper
  exists, but no installed source-authority exporter/boot join is qualified. See the read-only audit
  below. A Native target scan still cannot issue its own expected source digest.

## Read-only authority audit · snapshot-first is a valid distinct route

The prior source audit selected retained replay and a source-authorized
snapshot checkpoint as valid routes. Both contracts are supplied in the
[History ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover).
The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) retains its original source references and checkout.

### Retained V9/V5 and original body/ACK

Retained V9/V5 and an ACK alone establish no complete original-body
inventory. The producer validates complete receipt and Source association
before lending original data. Cleanup may retire that artifact. The
[exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) retains the original cleanup and decoder paths. Signed
production input completeness remains H-ACCEPT-01's acceptance.

### Authoritative Legacy snapshot bootstrap

The Legacy-owned WAL snapshot binds typed rows, epoch and applied ACK
under the writer fence. The Native checkpoint is checked against that held
source before selection. The [exact source and execution record](../../../ARCHIVE-INDEX.md#2026-11-17-six-ticket-historical-body-compaction) preserves limits on historical
cursor and overwritten event recovery. H's active backlog owns installed
qualification and wider Source/RepoMap routes.

## Bootstrap exporter and installed source binding

The authority boundaries found in the earlier audit still apply. The source
path below supplies the exporter and checkpoint join that were missing then;
the preceding preservation limits remain unchanged.

### Supplied source path

- The installed S coordinator retains the Host offline handoff, SDK committed
  Source/Original authority, and the exclusive History writer fence
  (`native_offline_cutover_v1.rs`). It passes the fence as FD7. The Q recovery
  child authenticates that inherited fence and holds the existing Legacy root
  lease (`native_recovery/custody.rs`). Host custody alone does not fence the
  SDK store, and the online auxiliary mutation lease does not cover this
  offline handoff.
- The admitted Q Catalog reader observes the exact generation's epoch, full
  physical row digest and latest **applied** History journal ACK in one
  quiescent Deferred transaction
  (`native/authority/read_only.rs:598..648`). The private exporter streams
  bounded pages and completion from that observation under the selected
  Legacy root. Ordinary rows preserve the original grouping. The exporter
  sends canonical V1 page bytes and oversized V2 row fragments as bounded
  immutable object frames. It also exposes the physical generation inventory
  and lends the actual terminal ACK.
- Before the first History ingress, the S snapshot candidate compares its
  complete selected target list with the exported physical inventory
  (`native_candidate_prepare_v1/history_snapshot_v1.rs:51..107`). For each
  generation it stages the pages and a checkpoint whose replay position binds
  the producer namespace, actual Legacy root, generation, epoch, digest and
  observed terminal ACK (`history_snapshot_v1.rs:385..495`). Retained originals
  use the separate authenticated replay route. Its peer-bound projection stage
  can use admitted IPC fragments when the envelope exceeds a single frame.
  Both routes compare the exact immutable stage ACK and attempt to discard the
  owned ingress on pre-Submit staging failure while the request budget is live.
- Q BOOT independently checks the ordered publications against the complete
  Legacy census under one held Catalog read, rederives the checkpoint replay
  commitment from the physical observation, verifies immutable pages and
  installed rows, and returns the selected History member and cut in the FD3
  terminal (`native_boot.rs:367..538`). S retains that terminal before
  restarting its producer. The selected Native target cannot issue its own
  expected source digest.

The original owner split still matters: Q's `LegacyReadAuthorityV1` supplies
existing-only physical read custody; S Host/CE owns the offline admission
seal; S SDK owns committed pointer and Original; Q Catalog owns row/journal
truth. The 30-second online AuxiliaryMutationCoordinator lease is not an
indefinite producer fence. The implemented FD7 handoff and root lease are the
fenced installed route; no parallel archive or fabricated replay cursor is
needed.

### Acceptance still open

The supplied source path has component coverage recorded above. The signed
installed S→Q drain, BOOT, restore-forward and subsequent real Source/History
DELTA across Q restart and S terminal recovery are **NOT_RUN**. The signed
fixture has a Q-only cut without an S original. No production root with
retired S originals was inventoried. H-INTEGRATE-01 and H-ACCEPT-01 own those
installed execution claims. The isolated DATA proof and CLI scope are
recorded in the active backlog. Previous-Native restore-forward now supplies
an authenticated primary History inventory and immutable carry-over, with the
Q cross-root FULL/append/restart body recorded above. The protected sibling
Source/RepoMap carry-over update and its current verification limits are in
[the three-defect repair](#three-defect-repair--2026-11-18). The snapshot's
historical preservation limits are stated above.

The [History ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#legacy-history-replay-during-cutover)
contains the current producer, exporter, BOOT and successor contract. The
[active backlog](#current-actionable-backlog--2026-11-16-re-audit) owns the
remaining verification without reopening the historical issuer audit.

<a id="independent-native-v5-repomap-source-lookup--reader-supply-still-incomplete"></a>
### Independent Native V5 RepoMap source lookup — supplied

Native aggregate/replay and FULL/DELTA/first-DELTA CLI now supply the RepoMap
loan; Q's V5 repomap carrier and selected physical verifier consume it. The
old no-field/no-Native-loan finding is retired. The durable contract is in the
[receiver ADR](../../../adr/NOV-07-001-native-index-execution-and-custody.md#recovery-receiver-and-selection-custody).
D/F retain owning, physical and installed acceptance. Qualification of the
supplied History snapshot bootstrap and active successor remains in
H-INTEGRATE-01, H-INTEGRATE-04 and H-ACCEPT-01.

## Recorded H source identities

The 22 full-file identities and superseded carrier identity are retained in the
[recovery entry](../../../ARCHIVE-INDEX.md#2026-11-16-history-contract-and-text-plan-consolidation).
They identify their original proof cohort, including concurrent owners' hunks;
they neither assign exclusive ownership nor attest current shared-main bytes.

## Source epoch replacement and History publication

Working-tree Q adds `SourceHistoryResync` as request tag 9. It compares the
prior producer epoch and History cut under the same expected Catalog View.
The existing Source replacement and History-generation builders commit one
View and one operation receipt. A stale Source epoch or invalid History
generation leaves the old Source owners and History visible.

The existing `source_and_next_history_generation_publish_in_one_view_and_reopen`
exact in `rust-test-native-history` now covers both Advance and Resync. Its
Resync case replaces the old owner, checks the new epoch and exact SourceRef
after reopening Q, reads the next History cut and frozen inputs, and replays
the same operation. The older History receipt predates this Resync case.
S's V9 producer already builds the combined request from the staged Source
Resync and Required History, retains the prior epoch and binds the owner census.
The changed Q UDS body **VERIFIED** 1/1, exit 0, in Nextest
`4e8fd36e-9962-4c36-9b6e-d4c609abe49c`. The test executed both Advance
and Resync in 25.320 seconds on the local old-Q reference with its
expected SHA-256. The Resync Contract, Kernel and UDS fixture bytes matched
before and after. Concurrent ANN and other test inputs changed during the
run, so this receipt applies to the built binary. Same-candidate installed
multi-epoch acceptance remains deferred.
[Q UDS result](</Volumes/Extreme SSD 1/build-cache/nc-h-resync-owner.OIsYrsN4/q-uds.log>).
[S V9 producer](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/prepared_aggregate_owner_pipeline_v1.rs:1860) ·
[S owner selectors](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/testing/suite-registry-v1.yaml:9006).

The Q request decoder now has a focused Resync unit test. It removes the
required History input from an otherwise valid request and checks that
decoding reaches retained admission once, refuses the request as corrupt,
and returns all bytes. The existing `test-workspace-lane` ran the production
decoder, request preparation and staging modules with Q Contract and Core
in a small macOS harness. All 12 unit tests passed in 0.01 seconds after
a 46-second compile. The Q UDS publication and reopen test remains separate.
[Unit test](../../../../crates/quanta-index-kernel/src/request_decode.rs#L873) ·
[Unit output](</Volumes/Extreme SSD 1/build-cache/nc-h-resync-unit.pTtdC0Bs/unit.log>).
The follow-up unit test exercises a valid Resync at the exact decode budget
and one byte below it. It checks `ResourceLimited` and a full refund on
refusal, and retained-byte accounting and refund on success. A one-byte
undercharge in the diagnostic copy made the new test fail. The unmodified
source passed **13/13** in the isolated macOS harness.
[Boundary test](../../../../crates/quanta-index-kernel/src/request_decode.rs#L905) ·
[Undercharge run](</Volumes/Extreme SSD 1/build-cache/nc-h-resync-budget-unit.qr1zQDly/undercharge.log>) ·
[Current unit run](</Volumes/Extreme SSD 1/build-cache/nc-h-resync-budget-unit.qr1zQDly/isolated-unit.log>).
The first frozen S wire run exited 101 before executing its body. An isolated
resolver run identified one missing `quanta-runtime-analysis` dev dependency
in `Cargo.lock`. The run adopted the existing S fix, retaining the same
verified Q supplier overlay. The existing S wire, binding, and witness exact
tests then **VERIFIED 3/3**, each with exit 0, on S `678089183fa2` and Q
`70d17e3cb6cc+ovl-d2e61e1ff5ab`. The three receipts share source digest
`fa9633e7e4a26d20368572148f30e4fd24d191a40e5ac8eeaa2c0fab83182f1f`.
[Initial S receipt](</Volumes/Extreme SSD 1/build-cache/nc-h-resync-owner.OIsYrsN4/s-wire-result.json>) ·
[S wire](</Volumes/Extreme SSD 1/build-cache/nc-h-resync-owner.OIsYrsN4/s-wire-fixed-result.json>) ·
[S binding](</Volumes/Extreme SSD 1/build-cache/nc-h-resync-owner.OIsYrsN4/s-binding-result.json>) ·
[S witness](</Volumes/Extreme SSD 1/build-cache/semantica-codegraph-v2-target/quanta-build-cli/_state/execution-roots/semantica-codegraph-v2-4250cacec3df/lanes/history-source-readback-snapshot-artifact/verification-results/20261123T031042.670539Z-4f51b054ac49/receipt.json>).

## History export frame JSON budget · 2026-11-23

S uses the bounded Q JSON decoder for Native History and protected History
frames. The Q direct path handles a body before its kind without entering the
generic serde body replay. The Legacy checkpoint V2 receiver separately
borrows the framed JSON body and dispatches directly to the selected typed
payload. Its completion and frame rejection paths retain the memory grant
until the decoded value is dropped.

- Q Contract library **VERIFIED** 485/485, exit 0, Nextest
  `ab5d681b-9991-40a0-b4dc-1f04c266c4e6`. The existing
  `test-workspace-lane` used Q `70d17e3cb` with only the decoder,
  tests and API baseline overlaid. The live attempt failed before tests
  because of an unused result in the new H regression and another in a
  concurrent Source join test. The owned regression was corrected.
  The focused no-replay test checks a valid 64 KiB Native chunk.
  [Q test output](</Volumes/Extreme SSD 1/build-cache/nc-h-frame-codec-q.kUrw8fvl/contract-projection.log>).
  The Contract API check matched the new History method against the updated
  baseline. The package gate **FAILED** on pre-existing API drift outside
  this method.
  [API output](</Volumes/Extreme SSD 1/build-cache/nc-h-frame-codec-q.kUrw8fvl/public-api.log>).
- The exact S `checkpoint_stage_v1/wire_v2.rs` module **VERIFIED** 3/3,
  exit 0, in a macOS diagnostic with the Q overlay, the same S module
  bytes and the S-vendored serde_json `raw_value` feature. It verifies
  body borrowing from the original frame buffer, typed page and
  chunk decoding, and malformed frame rejection. The separate owning
  Linux CLI test request `b8575d4ef55de9c89018d00e3d77fc0f` is
  **NOT_RUN**. Installed Linux acceptance remains deferred.
  [S codec test output](</Volumes/Extreme SSD 1/build-cache/nc-h-frame-codec-q.kUrw8fvl/s-wire.log>).
- The Legacy V2 body-first decoder now validates the original envelope
  without retaining a replay tree. The original adjacent-tag decoder
  refused out-of-range numbers, unpaired surrogate escapes and nesting
  beyond the JSON recursion limit, even inside ignored body fields.
  Each regression reproduced before its fix (exit 101). The exact S
  module passed all 6 tests after both fixes (exit 0) in the macOS
  diagnostic. The tested module SHA-256 is
  `093511320d461aae974246957712ac61ee231bbfb674c6c28f26ca41d99abbdf`.
  [Scalar RED](</Volumes/Extreme SSD 1/build-cache/nc-h-codec-tdd.aEB1K9Ke/red.log>) ·
  [Recursion RED](</Volumes/Extreme SSD 1/build-cache/nc-h-codec-tdd.aEB1K9Ke/recursion-red.log>) ·
  [Final GREEN](</Volumes/Extreme SSD 1/build-cache/nc-h-codec-tdd.aEB1K9Ke/wire-final.log>).
- A Legacy completion unit test now checks both field orders and verifies
  the typed generation, root, epoch, digest, count and exact terminal ACK.
  Omitting the ACK is rejected. The isolated macOS module ran **7/7**.
  Both follow-up unit harnesses used separate targets from the active Runtime
  build after shared admission timed out. They built serially with one job.
  [Completion test](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/history_snapshot_v1/checkpoint_stage_v1/wire_v2.rs:257) ·
  [Current codec output](</Volumes/Extreme SSD 1/build-cache/nc-h-legacy-complete-unit.xmqDHa2u/isolated-unit.log>).

## Maintained History test gates · 2026-11-23

The S gate `ag5-history-required-data-cold-full-retry-owner` now selects the
whole `history_required_terminal_budget_owner_v3` integration target and
requires both test names. The old library test wrapper was redundant with
the integration target and has been removed. The four-feature class and
the Just entry point remain the same. Registry resolution found two tests.
The witness validator accepted the two original passed names and rejected
an incomplete result containing only the first. The original execution is
recorded separately in H-CLOSE-03.
[S gate registry](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/testing/suite-registry-v1.yaml) ·
[Resolve output](</Volumes/Extreme SSD 1/build-cache/nc-h-history-gate.7nRxiK3D/s-required-history-resolve-recheck.log>).

The three required S `SourceHistoryResync` suites now have a literal runner
in `justfiles::gates::gate-native-source-history-resync-owners`. It invokes
the registered wire, binding and witness selectors once each. This wiring
uses the exact selectors already exercised on the recorded S/Q input above.
All three selectors resolved to one expected test each. The pre-fix wiring
reported all three runners missing. The current audit found runners for all
six required S History suites, including the two Git suites in the existing
Linux Watchman gate. The scoped consumer and exact-witness check passed **9/9**.
[S Resync gate](/Users/songmin/Documents/code-new/semantica-codegraph-v2/justfiles/gates.just:546) ·
[S regression tests](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/testing/tests/test_history_required_suite_contract_v1.py) ·
[S regression output](</Volumes/Extreme SSD 1/build-cache/nc-h-history-gate-followup.Ql8zAEr9/s-history-regression.log>).

The Q History recipe requires the platform's full roster from Nextest
terminal events. The fixture-producer recipe now checks the same old-Q
path and digest before building. Regression tests verify complete and
missing-body results, and the platform-specific target selection.
The gate and Nextest event checks passed **76/76**. The local recipe timed
out before Cargo admission, so the five selected macOS bodies ran directly
from the existing UDS test binary with the hash-checked old-Q executable.
All five passed in 204 seconds. The Linux branch selected six names in a
gate diagnostic. Its Linux exporter body remains unexecuted in this run.
[Q recipe](../../../../Justfile#L474) ·
[Gate tests](../../../../tools/ci/tests/test_circleci_authority.py) ·
[Gate test results](</Volumes/Extreme SSD 1/build-cache/nc-h-history-gate.7nRxiK3D/q-gate-pytest-final.log>) ·
[Q UDS output](</Volumes/Extreme SSD 1/build-cache/nc-h-history-gate.7nRxiK3D/q-existing-uds-bodies.log>).

The no-default Q process readiness recipe now performs the same Legacy
path and SHA-256 check before Cargo. Its mounted History and recovery
bodies use that executable. The local readiness and DSL fixture recipes
both refuse missing inputs and mismatched digests before building. The
gate, event and retirement checks passed **93/93** in the follow-up.
The follow-up invocation of the full Q History recipe timed out after 600
seconds waiting for the shared build lock. It returned exit 124 before
Cargo, with zero new bodies executed. The recorded macOS binary result
above still covers the five bodies from that build.
[Readiness recipe](../../../../Justfile#L332) ·
[Follow-up gate tests](</Volumes/Extreme SSD 1/build-cache/nc-h-history-gate-followup.Ql8zAEr9/q-gate-pytest.log>) ·
[Q lock timeout](</Volumes/Extreme SSD 1/build-cache/nc-h-history-gate-followup.Ql8zAEr9/q-gate.log>).

The closeout recheck passed **Q 93/93** and **S 9/9**. The first Q recheck
observed one catalog failure while the separate RepoMap retirement removed
`repo_map_end_to_end.rs`. The existing owner removed the obsolete mount and
catalog entries, and the same Q check passed in full afterward.
[First Q run](</Volumes/Extreme SSD 1/build-cache/nc-h-history-gate-closeout.DWnNkka1/q-fast-gates.log>) ·
[Q recheck](</Volumes/Extreme SSD 1/build-cache/nc-h-history-gate-closeout.DWnNkka1/q-fast-gates-recheck.log>) ·
[S recheck](</Volumes/Extreme SSD 1/build-cache/nc-h-history-gate-closeout.DWnNkka1/s-fast-gates.log>).

The closeout retry used the pinned old-Q executable and the same
`test-workspace-lane`. It timed out at **14:31 KST** after waiting **1,800
seconds** for shared build admission. The holder was still compiling. The
command returned **exit 124** before Cargo and ran **0/5** macOS History
bodies. The earlier **5/5** remains evidence for its existing binary.
[Closeout retry log](</Volumes/Extreme SSD 1/build-cache/nc-h-history-gate-closeout.DWnNkka1/q-gate.log>).

## History owner gate audit · 2026-11-23

The Linux Candidate parser suite carried two obsolete multiactive Source
test names. The S source resolver reported **2 unresolved of 3** before
the registry fix. The optional Linux owner gate now runs this parser suite
and the seven Legacy History checkpoint codec tests with the existing CLI
bin class `quanta-cli-server.no-default.1e9ebdacdb1c`. The codec child had
an implicit module path under a parent loaded with `#[path]`. Rust reproduced
`E0583` for that layout. An explicit child path now resolves every codec
test through the production CLI module. The focused S checks passed
**11/11**. The byte-identical codec module passed **7/7** in its existing
isolated macOS binary. The Linux owner binary and signed installation proof
are **NOT_RUN**.
[S Linux owner gate](/Users/songmin/Documents/code-new/semantica-codegraph-v2/justfiles/gates.just:553) ·
[S checkpoint module](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/history_snapshot_v1/checkpoint_stage_v1.rs:32) ·
[S scoped output](</Volumes/Extreme SSD 1/build-cache/nc-h-owner-audit.QAbLAKAE/s-history-gates-recheck.log>) ·
[S isolated codec output](</Volumes/Extreme SSD 1/build-cache/nc-h-owner-audit.QAbLAKAE/s-codec-isolated-replayed.log>).

`just rust-test-native-history-owners` selects seven named Catalog bodies
and three Kernel decoder bodies in the existing Q lane. It checks the
completed test identities and fails if a required body is omitted even when
Nextest exits zero. These owner lib targets use no installed or old-Q
executable. The shared witness parser also accepts real libtest-json-plus
events from a disposable Nextest library probe. Q fast checks passed
**95/95**. The full owner lane is awaiting shared build availability.
[Q owner gate](../../../../Justfile#L524) ·
[Q fast output](</Volumes/Extreme SSD 1/build-cache/nc-h-owner-audit.QAbLAKAE/q-owner-gates.log>).

The Kernel cancellation regression now checks both ProjectionAdvance and
SourceHistoryResync before admission and after reserving retained bytes.
It requires a typed cancellation and a full refund. The current decoder
module ran **13/13** in an isolated macOS diagnostic linked against the
recorded cached Contract and Core artifacts. A scratch copy that reclassified
the Resync cancellation as corruption failed the strengthened test.
[Decoder test](../../../../crates/quanta-index-kernel/src/request_decode.rs#L934) ·
[Diagnostic output](</Volumes/Extreme SSD 1/build-cache/nc-h-owner-audit.QAbLAKAE/q-kernel-diagnostic-green.log>) ·
[Mutation output](</Volumes/Extreme SSD 1/build-cache/nc-h-owner-audit.QAbLAKAE/q-kernel-diagnostic-red.log>).

The S static regression now resolves all five required exact History tests
through their Cargo library roots. For Required DATA it checks the declared
integration target path and features, the two executable names and the full
source test count. The check rejects both renaming a listed test and adding
an undeclared test in a scratch target. The focused Python checks passed
**12/12** without compiling Rust. The current S Candidate audit index also
points to the Linux parser and checkpoint recipe with the current Source-only
policy and its Linux execution still pending.
[S source and consumer regression](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/testing/tests/test_history_required_suite_contract_v1.py) ·
[S scope output](/tmp/nc-h-owner-local.RCYNrxF1/s-history-regression.log) ·
[S mutation output](/tmp/nc-h-owner-local.RCYNrxF1/s-history-source-negative.log) ·
[Q fast recheck](/tmp/nc-h-owner-local.RCYNrxF1/q-fast-recheck.log).

The next Q owner run entered the existing `test-workspace-lane` from the
internal cache after the external SSD detached. The Catalog test binary
failed to compile because the shared `quanta-index-contract` retirement
decoder returned a borrowed view without identifying its lifetime
(`E0106`). The run returned **exit 101**, with **Catalog 0/7** and **Kernel
0/3**. The in-progress retired-projection decoder is part of the separate
Product retirement work. The view borrows only the supplied payload bytes.
The owner gate is ready to rerun once that Contract input is stable.
[Q owner terminal](/tmp/nc-h-owner-local.RCYNrxF1/q-owner.log) ·
[Retirement dependency record](../../nov-20-native-cutover-closeout/tickets/07-product-qualification.md#L234).

The shared Contract decoder now ties the returned view lifetime to its
payload. A scratch compile reproduced `E0106` on the former signature and
ran the decoder's two existing tests after applying only the lifetime fix.
The canonical Q owner gate was resubmitted once with the corrected Contract.
[Scratch compiler refusal](/tmp/nc-h-contract-lifetime.5NIU9EVs/before.log) ·
[Scratch decoder result](/tmp/nc-h-contract-lifetime.5NIU9EVs/after.log) ·
[Q owner retry](/tmp/nc-h-owner-after-lifetime.hIJKJie6/q-owner.log).

The retry compiled Contract and reached the Catalog test crate. Thirteen
terminal-custody fixture calls still passed six protection slots after the
reader moved to seven. The fixtures now infer the array width from the
reader. A new assertion in the existing custody test checks that the final
slot protects the matched operation object before bytes are charged. The
production SQL probe returned the object when that slot was empty and
excluded it when the slot contained the reference.

The next run `b641ff4e-ab8f-48c0-a339-3214980f4dd9` executed seven Catalog
tests, with **6 passed and 1 failed**. The quiescence fixture injected a
1 MiB payload and expected budget refusal even though the row-width cap is
4 KiB. It now compares two malformed payloads within the admitted width.
The assertions require the exact payload byte delta, a checksum refusal
with sufficient budget and `ResourceLimited` for the larger row under the
smaller row's budget. Both current and historical Source reads are checked,
along with the historical rollback path. The stored payload length is
unchanged after refusal. The first retry stopped before compilation with
**exit 102** because `--locked` detected Manifest and lockfile drift.
Both files were updated by the existing work and the same locked metadata
command passed. The owner gate was resubmitted once on the reconciled inputs.
[Catalog compiler output](/tmp/nc-h-owner-after-lifetime.hIJKJie6/q-owner.log) ·
[First body result](/tmp/nc-h-owner-after-custody.iANkSiEU/q-owner.log) ·
[Protected slot SQL probe](/tmp/nc-h-owner-after-custody.iANkSiEU/q-custody-sql-probe.log) ·
[Locked metadata refusal](/tmp/nc-h-owner-width-boundary.2VFGOqRf/q-owner.log) ·
[Reconciled owner gate](/tmp/nc-h-owner-lock-synced.klFuMqBt/q-owner.log).

The reconciled run `231bfe91-1837-4db4-b1df-bdd211962131` passed all
**7/7 Catalog** bodies, including the corrected byte-admission test. The
test source SHA-256 matched the pre-admission copy. A separate policy-only
BOOT test was added afterward. Removing that addition reproduces the
executed fixture's source SHA-256. The same compiled
Catalog test binary separately passed the terminal-custody fixture **1/1**,
including the final protection slot. The Kernel phase of the same recipe
ran `eada82f8-4f75-41e2-a855-8acd45ed9c7e` and passed **3/3**. The full recipe
returned **exit 0**. The Kernel test source and custody fixture matched
before and after. The shared Cargo lock and Contract input changed while
the request was alive. These are test results for the binaries that ran.
[Catalog result](/tmp/nc-h-owner-lock-synced.klFuMqBt/q-owner.log) ·
[Custody fixture result](/tmp/nc-h-owner-lock-synced.klFuMqBt/q-custody-runtime.log) ·
[Fixture source comparison](/tmp/nc-h-owner-lock-synced.klFuMqBt/q-catalog-fixture-after-neighbor.log).

The S Linux Candidate gate now selects Native→Native inventory tests for
exact zero completion, memory refund, and foreign, ambiguous or repeated
members. The empty case now rejects a zero completion for another repository.
The portable Original replay gate selects the existing retry identity and
cancellation tests on Linux and macOS. Both suites use the explicit bin
feature class `quanta-cli-server.no-default.1e9ebdacdb1c`. A resolve-only
attempt with the old feature pair refused the Original bin because
`daemon-runtime` is required. The source regression now checks the Cargo
bin manifest alongside the witness names.

The S fast scope passed **14/14**. The three Native inventory bodies passed
in a macOS diagnostic built from the selected production functions and
cached Q dependencies. The foreign-repository assertion failed when the
repository comparison was removed from the diagnostic. Current Linux CLI
execution and installed qualification remain separate.
[S gates](/Users/songmin/Documents/code-new/semantica-codegraph-v2/justfiles/gates.just:553) ·
[Inventory diagnostic](/tmp/nc-h-native-history-inventory.GnUV5d48/green.log) ·
[Foreign-repository mutation](/tmp/nc-h-native-history-inventory.GnUV5d48/red.log) ·
[S fast scope](/tmp/nc-h-native-history-inventory.GnUV5d48/s-history-gates-final.log).

The maintained S consumer regression now also checks that the four optional
CLI suites have a reachable canonical runner. The production registry keeps
them optional. Removing any one of the four runner calls from a scratch gate
fails the check. The current scoped Python checks passed **14/14**.
[Consumer regression](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/testing/tests/test_history_required_suite_contract_v1.py:51) ·
[Optional consumer mutations](/tmp/nc-h-consumer-regression.6DlXJgHR/s-optional-consumer-negative.log) ·
[S recheck](/tmp/nc-h-consumer-regression.6DlXJgHR/s-fast.log).

The focused macOS History process gate verified the local old-Q executable
digest and resolved locked metadata. It entered the existing build lane and
returned **exit 101 · 0/5** while compiling the current Runtime. The active
RepoMap retirement changes produce four diagnostics in the new retirement
modules. They concern `OperationJournalPort` method resolution, sibling
visibility of `check_retired_request_v2` and two unused imports, including
Linux-only `PathBuf` on macOS. The tracked History fixture and selected Cargo
hashes matched before and after this run. The previous built-binary 5/5 and
the separate owner-gate 10/10 retain their recorded scopes.
[Process compiler result](/tmp/nc-h-uds-current.1BSMs1YL/q-uds.log) ·
[Retirement owner scope](../../nov-23-repomap-retirement/02-quanta-index.md).

The optional S installed Source failure suite now declares the five tests
mounted by the Linux daemon integration target. They cover export and
prebind cleanup, primary error preservation when cleanup refuses, retry
after bind, and exact retry after signed COMMIT acknowledgment loss. The old
entry named seven removed tests and claimed RepoMap failure coverage. The
source roster check is part of the existing Python CI step. It reproduced the
old mismatch and refused each of the five single-witness omissions. The
canonical runner resolved five tests in the existing feature class. The
fast focused suite passed **15/15** and the local reproduction of the
existing CI test group passed **230/230**. Signed Linux process bodies
remain **NOT_RUN · 0/5**.
[S registry](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/testing/suite-registry-v1.yaml:15300) ·
[Fast checks](/tmp/nc-h-source-witness.LajuzHY8/green.log) ·
[Local CI test group](/tmp/nc-h-source-witness.LajuzHY8/ci-fast.log) ·
[Old roster failure](/tmp/nc-h-source-witness.LajuzHY8/red-stale-roster.log) ·
[Omission probes](/tmp/nc-h-source-witness.LajuzHY8/missing-witness.log) ·
[Runner resolution](/tmp/nc-h-source-witness.LajuzHY8/resolve.log).

The Linux Q History recipe selects eight Rust bodies. Its Python gate test
now accounts for the protected two-Source import and reopen in
`native_index_uds` and the Legacy-to-fresh-Native rollback refusal in
`native_product_process`. The old six-name expectation failed on Linux.
The updated test checks the exact selector list, accepts all eight test
events and refuses an omitted result from either binary. The local Linux
roster and gate checks passed **9/9** with Just 1.58.0. The macOS Python
policy and proof checks passed **290/290**. These runs exercise the gate
with controlled events. The Linux Rust bodies remain **NOT_RUN · 0/8**.
[Q gate regression](../../../../tools/ci/tests/test_circleci_authority.py#L214) ·
[Linux before](/tmp/nc-h-linux-roster.dw3EFdjE/linux-red.log) ·
[Linux after](/tmp/nc-h-linux-roster.dw3EFdjE/linux-green.log) ·
[macOS Python checks](/tmp/nc-h-linux-roster.dw3EFdjE/macos-green.log).

The existing macOS process lane passed the earlier four Runtime diagnostics
after the retirement owner repaired the imports and visibility. The next
compiler result was **exit 101 · 0/5** at `verify_selected_v2`. Its
`pub(crate)` signature exposed the parent module's private
`ValidatedConfig` and `-D private-interfaces` stopped compilation. Both
platform variants now use parent-module visibility. The follow-up with the
same old-Q pin entered the existing build lane and returned
**exit 101 · 0/5**. Catalog's new retirement-policy row and reader used
`Digest` before the import was present. The current Catalog file contains
that import. Catalog, Core, SDK and Runtime inputs changed during the run.
A fresh runtime result needs the retirement owner's stable source.
[Compiler result](/tmp/nc-h-uds-repaired.JYm1lHTy/q-uds.log) ·
[Follow-up compiler result](/tmp/nc-h-uds-visibility.7mWV1GJt/q-uds.log) ·
[Changed inputs](/tmp/nc-h-uds-visibility.7mWV1GJt/input-drift.diff).

The S History suite registry and exact-witness Python checks also passed
**15/15** in the local Linux container. The external build volume was
absent. Current S Linux Candidate CLI and signed process bodies remain
unexecuted.
[S Linux source checks](/tmp/nc-h-linux-roster.dw3EFdjE/s-linux-static.log).

## Retired section links

These fragments recover historical source/dispatch sections; current work follows
the active owner ledger above. Exact old bodies are in the [recovery entry](../../../ARCHIVE-INDEX.md#2026-11-16-history-contract-and-text-plan-consolidation).

<a id="h-api-07-r2--validated-top-k-results-reused-frozen-for-root-retry"></a><!-- H-API-07-r2 · validated top-k results reused, frozen for Root retry -->
<a id="h-api-07-r1--compiler-correction-ready-frozen-for-root-retry"></a><!-- H-API-07-r1 · compiler correction ready, frozen for Root retry -->
<a id="h-api-07--source-ready-for-roots-single-compiletest-queue"></a><!-- H-API-07 · source ready for Root's single compile/test queue -->
<a id="canonical-contract-and-exact-mount-loan"></a><!-- Canonical contract and exact mount loan -->
<a id="typed-build-and-leaf-apis"></a><!-- Typed build and leaf APIs -->
<a id="implementation-boundaries"></a><!-- Implementation boundaries -->
<a id="verification-request--root-queue-only"></a><!-- Verification request · Root queue only -->
<a id="queue-admission-confirmation--2026-11-15"></a><!-- Queue admission confirmation · 2026-11-15 -->
<a id="current-h-source-identities"></a><!-- Current H source identities -->
