# NC-H · History source custody and Native adapter

<a id="current-nc-h-status--2026-11-21"></a>

## Current status · 2026-11-24

**Open.** Production on S `789774da727+ext-050d3b77949a…` and Q
`28784da26+ovl-34beeebe…` reached `quanta-runtime-retrieval-kernel` and
ended **exit 101** at 18:15 KST. Six diagnostics came from the missing Q
sealed DSL type and four SDK methods. Two diagnostics came from one missing
S `SourceCut` import. The same attempt ended before checking the Runtime
crate. [QBC receipt](</Volumes/Extreme SSD 1/build-cache/semantica-codegraph-v2-target/_state/execution-roots/semantica-codegraph-v2-932b0fce7281/lanes/history-source-readback-snapshot-artifact/verification-results/20261124T090153.058706Z-9fd4cf8fe34e/receipt.json>) · [compiler stderr](</Volumes/Extreme SSD 1/build-cache/semantica-codegraph-v2-target/_state/execution-roots/semantica-codegraph-v2-932b0fce7281/lanes/history-source-readback-snapshot-artifact/verification-results/20261124T090153.058706Z-9fd4cf8fe34e/stderr.log>).

The existing sealed S `12b13824c3f+ext-550120ebe0f128721e0c`, Q
`28784da26+ovl-2255517832b20d6d793b8ef54c1325eb`, G `f9533c4a7c86` contains
all five Q APIs and the S import fix `8887e7baeb7`. The Q definitions match
the S call sites, and the three Q definition files match live Q by SHA-256.
The candidate includes the earlier checkpoint and Watch fixes
(`f163593a996`, `dd29fdb8d4f`). It has no NC-H production receipt.
[Paired custody](</Users/songmin/.cache/semantica-codegraph-v2/casectl-snapshots/12b13824c3ffde90aec00acf38f6b257699a5f38_ext-550120ebe0f128721e0c/.casectl_snapshot_custody_v1.json>) ·
[S SourceCut import](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-retrieval-kernel/src/index_sdk_ingress/native_source_publish_v1.rs#L20) ·
[Q request type](../../../../crates/quanta-index-contract/src/native_protocol.rs#L705) ·
[Q staged Source SDK](../../../../crates/quanta-index-sdk/src/native_uds.rs#L1333) ·
[Q sealed DSL SDK](../../../../crates/quanta-index-sdk/src/native_uds/dsl.rs#L395).

The follow-up found one S retained Watch SDK defect. Initial readback and
cached/fresh revalidation converted raw projection failures to strings,
dropping typed Original refusal and retained diagnostic custody. S
`9a6db6ededd` projects those failures through the existing typed Gateway. The focused
checks against the actual Gateway library failed **2/2** before the repair
and passed **2/2** afterward. This change postdates S `12b13824c3f`.
[Bridge and regression](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/indexing_api/workspace_publication_commit/retained_warm_registry_v1.rs#L632) ·
[Before](/private/tmp/nc-h-watch-bridge-audit.HXrC0570/red.test.log) · [After](/private/tmp/nc-h-watch-bridge-audit.HXrC0570/green.test.log).

The signed V7 bound-request reader also hashed a full SQL wire body twice
without charging those scans to Original. S `8add1e93969` charges both digests
per chunk before hashing and charges the exact canonical byte comparison
before it runs. The focused byte and refusal checks failed **2/2** before
the repair and passed **2/2** after it.
[Signed request reader](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/indexing_machine_v2/store/source_only_bound_request_read_v7.rs#L378) ·
[Before](/private/tmp/nc-h-signed-wire-budget.grn_b2vr/before.test.log) · [After](/private/tmp/nc-h-signed-wire-budget.grn_b2vr/after.test.log).

The audit found no additional confirmed defect in History submit, resume,
Required DATA, Q ticket issuance, or Q→S exporter/readback.

<a id="current-actionable-backlog--2026-11-16-re-audit"></a>
<a id="history-owner-gate-audit--2026-11-23"></a>
<a id="s-cli-and-required-history-data-proof"></a>

## Local execution

Use the existing logical S `history-source-readback` and physical
`history-source-readback-snapshot-artifact` lanes. The successor selects
S `8add1e93969`, with both repair file digests checked against the focused
tests. The canonical reference to Q `22555178…` and G `f9533c4a7c86`
validated and S capture started. The four checks run in order after capture.
A later live edit does not change that receipt.
[Successor status](</Volumes/Extreme SSD 1/build-cache/nc-h-watch-typed-supplier.k2q_cdxb/state.json>) ·
[Repair source hashes](</Volumes/Extreme SSD 1/build-cache/nc-h-watch-typed-supplier.k2q_cdxb/repair-inputs.sha256>).
Changes to a selected dependency or fixture require a new affected check.

S capture finished with exit 0. Production entered QBC as
`20261124T110423.909709Z-b89ccadb4002` on the repaired S/Q input. The last
compiler log passed the prior retrieval-kernel diagnostics and reached
`quanta-runtime-analysis`. By 22:52 KST both owner processes had exited
without publishing a production terminal. QBC normalized the stale lane to
exit **124** with no compiler exit code and retained the original logs.
The surviving compiler log contains no error diagnostic. The cause of the
interruption is unknown.
[Sealed input](</Volumes/Extreme SSD 1/build-cache/nc-h-watch-typed-supplier.k2q_cdxb/capture.json>) ·
[Lane receipt](</Volumes/Extreme SSD 1/build-cache/semantica-codegraph-v2-target/_state/execution-roots/semantica-codegraph-v2-3f3aec44a186/lanes/history-source-readback-snapshot-artifact/receipt.json>) ·
[Preserved compiler stderr](</Volumes/Extreme SSD 1/build-cache/semantica-codegraph-v2-target/_state/execution-roots/semantica-codegraph-v2-3f3aec44a186/lanes/history-source-readback-snapshot-artifact/last-completed-run/stderr.log>).

The single production retry reused the same frozen S/Q/G, QBC request and
warm lane. Run `20261124T150525.441406Z-32fad6836991` reached
`quanta-runtime` compilation and exited **101** on 20 errors. Eighteen
diagnostics were removed flat fields on Q `LexicalCandidate`. The other
two were mismatched error types in offline Source lineage and a borrowed
receipt passed to the Native DELTA Submit API. CLI, Original replay and
Required DATA did not start. This input predates the Q inventory codec and
installed fixture changes.
[Retry state](</Volumes/Extreme SSD 1/build-cache/nc-h-watch-typed-resume.yvpj4jlw/state.json>) ·
[QBC terminal](</Volumes/Extreme SSD 1/build-cache/nc-h-watch-typed-resume.yvpj4jlw/production-result.json>) ·
[Compiler stderr](</Volumes/Extreme SSD 1/build-cache/semantica-codegraph-v2-target/_state/execution-roots/semantica-codegraph-v2-3f3aec44a186/lanes/history-source-readback-snapshot-artifact/verification-results/20261124T150525.441406Z-32fad6836991/stderr.log>).

Current S search consumers now read the Q `lines` and `excerpt` options.
The offline lineage path maps the borrowed identity error to its existing
typed contract refusal. The Native DELTA first-submit call now passes a copy
of the sealed `SourceStageReplayReceiptV1` to the by-value API (`01833802efb`) under the
existing signed claim. The retained stage still owns the original receipt.
Rust syntax and the scoped diff check passed. The prior frozen input remains
failed. H-CLOSE-04 needs a new production compiler run on the reviewed S/Q
input with this fix.
[Search projection](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_query_ports.rs) ·
[Offline lineage](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_only_acked_query_generation_v7/offline_lineage_v7.rs#L85) ·
[Native DELTA caller](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3/consumer_v3/terminal_handoff_v3.rs#L1132).

| ID | Existing check | Candidate state |
| --- | --- | --- |
| H-CLOSE-04 | `quanta-runtime --lib` production check, class `73ab9f2c701b`, request `7a5d32463bc71f1b298bdf4c00199257`, features `original-snapshot-v3,search-plane-sdk-ingress-product-proof` | Retry **exit 101** on S `8add1e93969` with Q `22555178…`. Reuse the request after the current integration repairs are bound to one reviewed capture. |
| H-CLOSE-01 | CLI bin `quanta-cli-server`, class `1e9ebdacdb1c`. Then `native-history-original-replay-owner` with `history_json_overflow_does_not_absorb_original_cancellation` and `replay_retry_keeps_object_identity_and_refunds_staged_bytes`. | **NOT_RUN** on the paired candidate. [CLI replay tests](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/history_v1.rs#L1090) |
| H-CLOSE-03 | `ag5-history-required-data-cold-full-retry-owner` selects the whole `history_required_terminal_budget_owner_v3` target, class `0ac60ceaae02`, one test thread. It runs `persisted_required_data_survives_late_private_terminal_budget_refusal_v3` and `rejected_required_history_readback_reaches_sdk_failure_handoff_v93`. | **NOT_RUN** on the paired candidate. [Two owner bodies](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-runtime/tests/history_required_terminal_budget_owner_v3.rs) |
| H-CLOSE-02 | Q `process_readiness_owner_v1` and `native_index_uds` in `test-workspace-lane`. Reuse the recorded results for those inputs. On a new Q candidate, compare the Rust, Cargo, fixture and old-Q inputs and rerun affected selectors there. | Recorded local Runtime **4/4** and UDS retry **1/1**. [Prior receipts](../../../ARCHIVE-INDEX.md#2026-11-24-nc-h-checks-and-source-custody) |

The local Rust checks and [installed acceptance](#installed-history)
have separate evidence. The signed candidate must also include the Q
Catalog BOOT orphan-root and pin-integrity fixes. The earlier Catalog 19
selection belongs to a different roster.

## RepoMap release handoff

- D01 executed `staged_source_and_history_bootstrap_publishes_changed_profile_and_reopens`
  on local Q with a pinned Legacy executable. Nextest run
  `2551dbb4-92c8-45fa-9a10-dffd56a01292` verified **1/1** after process
  termination and restart. The two-test invocation exited **100** after its
  separate large-view literal-anchor test failed in SourceMap apply.
  [Exact PASS](</Users/songmin/Library/Caches/quanta-index/native-public-history-v7-linked.g1Mu242b/nextest.log#L27>) ·
  [D01 receipt scope](../../nov-24-native-enhancement/tickets/D01-durable-recovery.md#작업-범위).
- The all-repository History completion decoder uses a manual field visitor.
  It preserves emitted JSON and CBOR, accepts body-first JSON and rejects
  unknown Native completion fields and tagged CBOR counts. The scoped derive
  checker found **0 violations across four History files**. An isolated Rust
  check using the current decoder and Ciborium 0.2.2 reproduced the tagged
  count failure before the change and passed all **6/6** History frame tests
  on macOS arm64 after it. The full Q derive check reported four SDK and
  semantic violations outside History. [Decoder](../../../../crates/quanta-index-contract/src/index/history/snapshot_checkpoint/export_decode.rs) ·
  [Before](</private/tmp/nc-h-inventory-decode.VJyIyVqB/derive.test.log>) · [After](</private/tmp/nc-h-inventory-decode.VJyIyVqB/manual.test.log>).
- `native_history_append_original_publishes_without_legacy_ack_and_reopens`
  now checks exact Resume and Submit after request object GC, two
  `DigestConflict` refusals for the same operation key replayed with a
  different view or profile, unchanged Latest and no recreated request
  object. Existing Q `test-workspace-lane` ran **1/1**, exit **0**, in
  nextest run `ef49fde9-6d25-4576-b463-dbf74e2ff1ba` with the pinned macOS
  arm64 Legacy executable `c013632a9770…`. Focused Rust and Cargo hashes
  matched before and after execution. [Existing owner test](../../../../crates/quanta-index-searchd-runtime/tests/native_index_uds/read_sessions/query/history.rs#L1473) ·
  [Exact PASS](</Users/songmin/Library/Caches/quanta-index/nc-h-standalone-history-gc.5C7lcLhS/nextest.log#L29>).

<a id="source-epoch-replacement-and-history-publication"></a>
<a id="native-history-inventory-memory-and-c-06-gate"></a>

## Installed History

| Owner scope | Remaining acceptance |
| --- | --- |
| H-INTEGRATE-01 · Catalog export and S ingestion | On the signed Q-only fallback, bind the full physical History key inventory to the original ACKs and Legacy Source/RepoMap active heads. Verify incomplete inventory refusal before ingress and the BOOT terminal from the exact selected candidate. |
| H-INTEGRATE-02/03 · ordered FULL and retry | Compare the S target inventory to the complete source roster. Verify one FULL per selected generation followed by a same-candidate retry returning the original Published terminal before selection. Post-Exact-retirement Submit remains the separate Q UDS case. |
| H-INTEGRATE-04/05 and H-ACCEPT-01 · successor and restart | Exercise actual Source export → S ingest → installed FULL → Source-bound DELTA → restart/readback, including the cross-revision predecessor, Q COMMIT/ACK loss, original operation identity, zero republish, and the durable V9/History receipt. Run the existing AST restore-forward case and the expanded multi-repository History case on the same signed candidate. |
| Real-data preservation | Requires an actual production state root, a complete retained Original or authenticated physical checkpoint inventory, and the terminal ACK set. Those inputs are absent from the supplied fixture evidence. |

The existing `installed_native_ast_head_restore_forward_preserves_source_v1`
case compares the Q listener PID via Linux
`SO_PEERCRED` on the old selection, fresh BOOT and reopen. Its original Source
and view readbacks still run. The socket credential helper type-checked for
`x86_64-unknown-linux-gnu` with the locked libc version. The installed AST
body has no current execution result.
[AST fixture](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/tests/server_socket_daemon_e2e/native_offline_recovery_v1/postpublication_v1.rs) ·
[Helper input hashes](</private/tmp/nc-h-ast-peer-linux.uBzp7AOi/source.sha256>).

The installed `installed_native_history_full_then_delta_retains_generations_v1`
case now reads three physical generations from two repositories. It independently
verifies the second repository's distinct
Git commit in Legacy and checks its authenticated History instance in the
selected Exact view after FULL, after the primary Source DELTA, and after
Native→Native replacement and reopen. The incomplete-inventory refusal
and primary cross-revision predecessor remain in the same test body.
The fixture passed Rust syntax and scoped diff checks. The installed History
body has no current execution result.
[Two-repository input](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/tests/server_socket_daemon_e2e.rs) ·
[Native History readback](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/tests/server_socket_daemon_e2e/native_offline_recovery_v1/scenario_v1.rs).

The signed Linux checks use the existing `native-installed-history-full-delta-process-owner`
and `native-installed-ast-head-recovery-process-owner` alongside the installed
Source-failure fixture. The local macOS work has no matching Linux
Legacy/Native executables and hashes, admitted launcher hash, or retained
recovery build receipt for the signed candidate. Installed execution is open.
H supplies History inputs and witnesses, D owns installation, and F/Root
owns signed execution. [Existing installed fixture](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/tests/server_socket_daemon_e2e.rs#L3441) ·
[03.7](../../nov-20-native-cutover-closeout/tickets/03-signed-published-cp2.md#history-03-7).

## Code and decision owners

| Boundary | Current implementation |
| --- | --- |
| Q authorization and budget | [Ticket issue and pin lifetime](../../../../crates/quanta-index-catalog/src/native/authority/lifetime.rs) · [BOOT/quiescence](../../../../crates/quanta-index-catalog/src/native/authority/boot_quiescence.rs) |
| Q checkpoint supply and replay | [Legacy export](../../../../crates/quanta-index-searchd-runtime/src/history_checkpoint_export.rs) · [Native export](../../../../crates/quanta-index-searchd-runtime/src/history_checkpoint_export/native_v1.rs) · [Source export](../../../../crates/quanta-index-searchd-runtime/src/history_checkpoint_export/source_v1.rs) · [Replay fold](../../../../crates/quanta-index-search-plane/src/auxiliary_authority/history_source.rs) |
| S producer and candidate | [Producer handoff](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/history_handoff_dispatch.rs) · [Candidate submit and resume](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/history_v1.rs#L166) · [Native inventory](/Users/songmin/Documents/code-new/semantica-codegraph-v2/crates/quanta-cli-server/src/native_candidate_prepare_v1/history_native_v1.rs#L379) |

<a id="q-main-protected-snapshot-supplier-20261119"></a>
<a id="history-export-frame-json-budget--2026-11-23"></a>
<a id="maintained-history-test-gates--2026-11-23"></a>
<a id="macos-history-runtime-and-dsl-cold-catalog"></a>

Accepted contracts live in the [History ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#native-history-publication-and-read-contract).
Dated runs, diagnostics, source hashes and superseded plans are indexed in
[the archive](../../../ARCHIVE-INDEX.md#2026-11-24-nc-h-checks-and-source-custody).
