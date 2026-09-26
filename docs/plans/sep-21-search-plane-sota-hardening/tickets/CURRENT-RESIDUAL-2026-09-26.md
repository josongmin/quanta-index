# SEP-21 current residual audit — 2026-09-26

This is the current residual implementation/status index, not a qualification
receipt. Earlier dated ticket sections are historical observations.

## Source and ownership

- Latest 2026-09-27 continuation: main
  `7d581d0572447e4f29ce0d0ea2f9d2fbc8b1353c` plus preserved shared dirt.
  Semantic batching/private SQL-byte guard has 71/71 terminal owner tests;
  conditional custody/window focused tests have 16/16. These are local scoped
  results, not final cross-module producer, daemon, exact-pair or release proof.
  Source hashes and raw log digests are in the newest progress-ledger entry.
- Latest direct-exit repair started at
  `577d60b518344163145ad2f1afe1f3e7c656762e`; another writer advanced shared
  dirty main through `b24d11489d250613c4871df6d65fa3d7e70daef4` and
  `bc7946309a65a0dd70ee0e90f768c7386e9390b5` to
  `349090ca1ff5db875814dda848f01baf71131a10`. The earlier
  snapshots below are historical. Current owner hashes and terminal local
  results belong to the latest progress-ledger section, not release proof.
- Review started at Quanta main `7cefac4a10a06ed56b6f5b9f42b3726468b1f198`.
- During integration another writer advanced main to
  `8eac12c5b45fedfa4aa7cb27da82979ecdbfbb10` and changed contract, semantic,
  ingest, query and benchmark files. Those changes were preserved.
- Closeout main `4af3bb44ea4769205485a9ed4c7dddddb35a724f`: another writer
  captured the repairs and this status file in a mixed-scope commit. Earlier
  dirty-overlay tests are not final-commit qualification; see the progress ledger.
- Semantica read-only audit: `941903ba9f61351b1228fba99110162f64120e5c`;
  not a frozen exact-pair proof. No Semantica edits, deployment or push.
- Parallel ownership: supervisor lifecycle; Cargo resolution preflight;
  read-only producer/release audit. Integrator owns proof parsing, registry,
  test-authority, recipe and ticket status. No duplicate producer IR or
  second serving/activation authority was introduced.

## Structural repairs

### S21-09 / P09 — reporting-child loss

RCA: finished-handle observation excluded `reports_exit` children. A terminal
adapter panic or a return without its required report could leave serving
Ready; a report is not proof that the thread completed.

- `crates/quanta-index-searchd/src/app/supervisor.rs`: observe every registered
  finished handle; consume queued reports after completion; preserve typed
  Failed, classify absent required report as Failed and actual join panic as
  Panicked. Serving, drain and rollback share classification. Reported teardown
  still waits within the original deadline and retains custody if unfinished.
- `crates/quanta-index-searchd-runtime/tests/runtime_supervisor_owner_v1.rs`:
  six counterexamples: panic/no report, return/no report, queued failure,
  drain/no report, rollback/no report, optimistic Completed then panic.
- DoD: owner scenarios pass without unbounded join or losing failure class.
  This does **not** prove a release binary's maintenance loss while control
  remains responsive; that process target is still required.

### S21-12 / P11 — actual dependency roots and resolver lock

RCA: source HEAD and daemon equality did not prove that Semantica actually
compiled Quanta dependencies from that checkout. Its selected nested workspace
uses `packages/analysis/quanta-v2/Cargo.lock`, not repository-root Cargo.lock.

- `tools/ci/paired_cargo_resolution.py`: validate actual reachable Cargo resolve
  graph, canonical package manifests from the expected Quanta workspace,
  required contract/IPC/SDK and enabled consumer feature; refuse other
  checkout, registry/git source, ambiguous/missing nodes and escaping symlinks.
  Derive the expected package paths from workspace ownership, not a second map.
- `scripts/verify-repomap-cross-repo.sh`: preflight both selected feature
  profiles before expensive build, use `--locked` for metadata and four test
  invocations, compare resolver identity after execution, log stable relative
  manifest/feature/digest mapping and actual nested lock identity.
- `tools/ci/proof-authority.toml` and checker: bind the correct nested lock for
  all exact-pair nodes and aggregate; obsolete root-lock receipts are stale.
- Twenty helper counterexamples are registered in `test-authority.toml`,
  P12A's exact expected target list and the existing P12A recipe.
- DoD for **preflight only**: same-byte wrong sibling checkout is refused and
  canonical alias of the correct checkout accepted. Full P11 remains staged:
  mapping is a checked execution precondition/log, not a typed manifest field;
  build/runner provenance and frozen live pair still require proof.

### S21-13 / R0 — archived pytest semantics

RCA: zero suite counters plus direct passing cases could hide suite-level
errors or unsupported wrappers; inventory selector text was not connected to
the admitted case identities.

- `tools/ci/proof_execution_result.py`: admit only supported JUnit element
  placement, reject failure/error/skipped outcomes and unknown wrappers,
  enforce unique complete-file selectors, bind every inventory case to a
  selected file and require each declared file to be represented.
- `tools/ci/tests/test_proof_execution_result.py`: malformed outcome placement
  and outside/omitted/duplicate/partial selector counterexamples.
- DoD: these false inputs cannot establish passed execution. This parser is
  not producer attestation and cannot prove that a malicious producer did not
  forge a smaller inventory inside a selected file. Keep R0 open.

## Remaining work — one authoritative status table

### Follow-up byte/manifest custody repairs (2026-09-26)

Base: Quanta `f9c3b4dc487a1b54a260e4ab2d3dd199310d3a5e`, shared dirty
main. Implementation and proof remain separate; terminal results are in
`EXECUTION-PROGRESS.md`.

- P10 original-backup admission: `ReadOnlyRootLeaseV1` privately pins the
  decoded original manifest. Restore reuses `run_offline_verify_v1` before
  creating staging, copies the admitted inventory and verifies the copy
  against that original inventory before the intentional incarnation rotation.
  Verify and restore share catalog digest/row-count validation. Pre-publish
  drift checks include manifest content/kind; missing or malformed authority
  preserves the failure and discards sealed staging. The regression matrix
  covers pre-custody added/changed/missing files and directories, validly
  self-signed wrong catalog rows, post-custody replacement and mid-restore
  replacement/malformed/removal. Dirty-source Rust owner regression executed
  55/55 successfully; mandatory daemon gate and exact-source owner issuance
  remain separate.
  Manifest-kind admission counts dangling/non-regular reserved entries as
  present instead of silently treating them as absent; selected authority still
  uses strict no-follow decode. Backup verification/restore share the existing
  bounded SQLite catalog-directory sidecar exception while all advertised file
  bytes and other directories remain exact. Both seams have regression cases.
- Nextest inventory admission: malformed/missing/unknown `filter-match` is
  no longer an implicit exclusion. Only explicit known mismatch reasons are
  admitted, with a boolean ignored flag. All collection readers use the
  canonical `nextest_events.py`; legitimate explicit exclusions remain valid.
- Archived execution custody: `derive_test_result` captures evidence and
  inventory through the existing no-follow descriptor owner, checks their
  declared digests and parses those same bytes for both Nextest and JUnit.
  No pathname reopen occurs between the digest check and interpretation.
  `write-verification-receipt.py` captures event bytes once and avoids the
  former healthy-run double parse while preserving primary failure reporting.
- Paired daemon custody: `binary_custody.py` reuses the same descriptor-walk
  owner, pins a private executable copy and freezes its digest. The shell
  proof passes that copy to every subprocess and checks built, supplied and
  copied bytes at execution boundaries and completion. Same-file supplied/build
  aliases no longer reduce the guard to `cmp` against itself. This is protection
  against shared build-output replacement, not isolation from a malicious same
  UID or compromised kernel. Resolver mapping and real pair qualification
  remain separate obligations.

| Area | Implementation / proof state | Concrete next action and acceptance |
| --- | --- | --- |
| Current native mutation exporter | VERIFIED local dirty owner 5/5; hostile actual-output replay 85/85 refused; controlled qualification NOT_RUN | Main `907785f` plus preserved dirty Rust changes built the actual exporter through canonical cargow. Fresh-state append/clear_surface/membership_replace/replace/tombstone execution exited zero; canonical independent full-row/window/delete/commit oracle derived five passes. Binary, selected tools, no-follow plan epochs and consumed source hashes remained identical before/after. Raw SHA `38c0ad5e03066df1d9687dc44fb36a04de776156ca1e8f717d8ab8dc6d29154d`; probe SHA `1a1dfc3e4fbea356a0ea0ff8c3b93e6a8961faa0cb499693cf93ad32d464fcc1`; mutant SHA `e41cdf91377205a4cc5540daaa38e78e58b7b7e328c2dd2617f8bd4f67531647`. Preserve failed temporary probe attempts; the earlier overbroad list refused concurrent nonconsumed run.py drift. This unchanged 8D owner plan does not establish controlled conditional producer/source/model custody, encoder parity, clean-main proof, paired quality, quiet performance or release. The final copied snapshot and its actual canonical terminals remain distinct. See current progress ledger for commands and identities. |
| R0 / S21-13 | Common tool custody and T15/T16 migration implemented; local focused 16/16; final cross-module producer proof pending | The pre-patch exported-function counterexample remains historical RED, not current behavior. Canonical admission now refuses exported-function keys, Python startup/test selection overrides; conditional source capture/verify, build, reference Python and native execution share the SDK/contract custody owner. Exact command environment/argv/epochs and execution bytes are required in replay. Final conditional publication follows the terminal guard, with exclusive fsynced pending bytes and atomic hard-link publication. Shared fixtures and CI/source registration were migrated; the fixture's self-referential digest was repaired through the existing closure owner. Require final stable tool/portable/conditional source hashes, whole retrieval rail and actual native exporter/replay before closing. Local custody does not authenticate remote producers or compiler/system libraries. No GitHub requirement. |
| R0 proof JSON | Shared decoder implemented; locked owner regression 163/163 and fixture publication 1/1 | `proof_json.py` rejects duplicate authority keys at every depth, non-finite constants and float overflow before schemas. Manifest/archive/aggregate/handoff readers plus verification summaries and archived pytest inventories share it. No-follow bytes and digest custody are unchanged. Final-source qualification remains separate; see the progress ledger. |
| Proof execution lifecycle | Direct-exit custody repaired; locked owner 70/70 and caller 87/87 | Common executor bounds failure drain/reap to ten seconds and ties nested owned sessions to controller-liveness pipes. After a direct child exits, the controller retains the unreaped group leader, drains output, accepts a bounded private child-status record, kills the group before reaping, and only then interprets completion. External/ignored SIGCHLD handlers are refused before spawning. Both liveness and terminal/output watchers use the OS default selector; actual fd 2048+ coverage passed without skipping. Normal/nonzero/signalled exit, closed-pipe background descendants, externally killed reported guard, malformed/missing terminal records, held-pipe escape and nested SIGKILL scenarios executed. Criterion, lexical, retrieval and portable proof callers share the same owner. See the progress ledger for exact commands, hashes and Python-runtime scope. No final-source/release qualification or arbitrary escaped-process containment. |
| R0 validation cost | Intrinsic reuse implemented; aggregate timing NOT_RUN | `check_manifest` now uses an invocation-local content/full-authority-bound worklist and decoded-byte cache. Every incoming edge retains no-follow capture, digest/identity/ancestry checks; cache hits and final custody rehash artifacts, binaries and archives. Depth 1/3/6 diamonds assert exactly 2*depth+2 schema interpretations. Tamper, symlink, authority-change, wrong-source, cycle and cross-invocation negatives are included in the terminal 295-owner run. Reuse is per root invocation, not shared across aggregate roots; no whole-aggregate timing or staged execution claim. The former 936 visits was a structural count, not measured run time. |
| R0 shell/JUnit admission | Locked local owners 237/237; Sourcegraph producer 27/27; retrieval integration PENDING | Shared `junit_events.py` owns XML outcomes/counters and known attributes for archive and retrieval consumers. Shell command-name and complete argv admission share a literal Bash-word decoder; unknown expansions, executable globbing, prefix assignments and nonexecution/partial-selection Python options cannot establish binding. Contract/sdk Justfile output arguments have observed literal argv tests. Final source-bound proof and the active daemon gate remain separate. Sourcegraph's 16 unittest subTest mutants are now independently collected methods, producing 27 exact identities under pytest9.1.1 with strict JUnit admission. The retrieval owner has changed the import to prevent duplicate class collection and refreshed its expected inventory; full-file terminal/source-stable evidence is still required. Old 315-count/283-case XML remains refused. |
| R1 / P03–P06 | Existing owners; release NOT_RUN | Keep release nodes staged. P03 activation/recovery/ACK/publish-negative; P04 pinned lifetime/GC in addition to trace identity; P05 fixed fixture IDs plus every quality subcommand result; P06 actual daemon frontdoor distinct from scripted peer negatives. Bind fresh release daemon and Linux source/host. Staged reasons corrected; no blanket owner rewrite. |
| P07 | BLOCKED approved-provider inputs | Register concrete release target after approved provider identity, egress, budget and cancellation config exist. Empty targets are not executable proof. |
| P08/P09 | Reporting-loss and phase-origin deadline patched; source-stable local owner 24/24; process/release NOT_RUN | Pre-patch drain restarted its hard budget after cooperative waiting and required-loss began its budget after stop callbacks. Drain/required-child-loss/rollback now fix deadlines at phase entry, charge callback time, clip cooperative waiting and exhaust overflow. Registered `just rust-profile test-runtime-supervisor-owner` was reissued after the source-moving cold run terminated: 24 executed, 24 passed, zero skipped. The progress ledger binds the final raw log and source hashes. This covers child forms, shutdown ingresses, overflow and retained custody, not arbitrary blocking callbacks, actual daemon signals, component loss with live control, readiness downgrade, FD/custody residue, different-UID authorization or release. |
| R0 retrieval evidence bytes | Captured-byte implementation landed in coordinated Python lane; final current-source proof pending | Pre-patch validation hashed raw paths and later reopened them in inventory/JUnit/Nextest/SDK readers; `_canonical_receipt` parsed and hashed separately. Current portable validation has one no-follow captured-byte cache and the evidence readers accept bytes. The coordinated lint/CI root owns actual tool selection/binding; its `ratchet_finalagent` owns raw digest/parser/replay capture, with a shared portable-function boundary. This Rust lane does not edit their Python owners. Require their final terminal counterexamples, exact sibling coverage, normal relocated replay and source hashes before closing. Paired-run context and frozen-receipt consumers remain in the sibling universe; byte custody is not compiler/producer attestation. |
| R3 semantic omission | OPEN_PROOF_GAP; producer owner required | In Semantica bind source-plan, shadow policy, prior sealed owner state and cluster-plan identities. Use independent fixed goldens for exact replace/tombstone/unchanged partition before dispatch. Quanta owns admitted mutation validity, not reconstruction of producer IR. Empty lexical-only/unchanged deltas remain valid. |
| R4/P10 | Original-manifest repair implemented; historical owner 55/55; semantic batching/private SQL guard owner 71/71; parser reverse consumers 158/158; daemon actual 206/206, original gate FAILED, corrected-parser raw replay VERIFIED; release/target NOT_RUN | Historical daemon run had 203 passes and a 10001-row ingest-batch-2 timeout. Current unchanged 10001-row scenario passed in 38.851607291 seconds; the complete Nextest execution passed 206 tests in 543.337 seconds. No controlled performance or sole-cause claim follows. The original gate exited 2 on excluded ignored-event admission. The canonical immutable inventory now retains precise ignored exclusions and reconciles actual unique required outcomes and inventory-bound reporter fragments; original raw replay derives 206 selected/executed/passed and zero failed, never counting the excluded external-provider test as success. Parser owner has 30 passing cases; the same six-file reverse-consumer rail passed 158 cases after the serial owner migrated the independent portable recipe fixture. Final-source qualification remains pending. Exact tuple deletion/private 32-MiB SQL refusal retain the native 71-case proof and focused conditional 16-case proof; actual exporter/replay and final cross-module qualification remain separate. Preserve original gate FAILED versus current-parser replay VERIFIED, dirty starting HEAD, Python-only HEAD move and post-execution-only binary measurements. No legacy importer or production-root mutation authorized here. |
| R5/P11 | Preflight repaired; OPEN_PROOF_GAP | Bind resolver mapping as typed receipt evidence, actual build/test terminal outcomes and exact clean source pair. Run live publish/activate/restart/query and negative bundle/transition cases. Mapping log alone is not the paired receipt. |
| R6/P12 | BLOCKED operational inputs; final qualification NOT_RUN | Obtain host/root/config/retention/rollback authority. Separate deploy/activate/rollback typed action receipts; independently audit authentic historical handoffs; reissue final-source DAG. Never infer activation from deployment or recreate absent handoffs. |

## Local verification

Focused commands and final results are recorded in `EXECUTION-PROGRESS.md`.
Raw local logs: `/tmp/quanta-residual-audit.r8ZupY/` (temporary, not a durable
release archive). Shared moving source and unrelated writers exclude exact-source
qualification. No passed proof manifest or production verdict was issued.
