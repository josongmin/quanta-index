# SEP-21 current residual audit — 2026-09-26

This is the current residual implementation/status index, not a qualification
receipt. Earlier dated ticket sections are historical observations.

## Source and ownership

- Review started at Quanta main `7cefac4a10a06ed56b6f5b9f42b3726468b1f198`.
- During integration another writer advanced main to
  `8eac12c5b45fedfa4aa7cb27da82979ecdbfbb10` and changed contract, semantic,
  ingest, query and benchmark files. Those changes were preserved.
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

| Area | Implementation / proof state | Concrete next action and acceptance |
| --- | --- | --- |
| R0 / S21-13 | Parser strengthened; OPEN_PROOF_GAP | Bind actual executable, complete recipe/subcommand outcomes, observed local/Linux host and trusted producing run. Reject fabricated inventory and wrong runner/source/host, not merely mismatched counters. No GitHub requirement for the local workflow. |
| R0 validation cost | OPEN_PERFORMANCE | Split intrinsic archive parsing from incoming-edge validation; use invocation-local content/config-bound cache only with rehash of every referenced runner/artifact/binary. Current full-present DAG expands 25 unique nodes/32 edges into 936 recursive visits (structural count, not observed staged-run calls). A proof-ID-only visited set is forbidden. Require diamond parse-count oracle and warm-cache tamper/cycle/wrong-source negatives before changing validation. |
| R1 / P03–P06 | Existing owners; release NOT_RUN | Keep release nodes staged. P03 activation/recovery/ACK/publish-negative; P04 pinned lifetime/GC in addition to trace identity; P05 fixed fixture IDs plus every quality subcommand result; P06 actual daemon frontdoor distinct from scripted peer negatives. Bind fresh release daemon and Linux source/host. Staged reasons corrected; no blanket owner rewrite. |
| P07 | BLOCKED approved-provider inputs | Register concrete release target after approved provider identity, egress, budget and cancellation config exist. Empty targets are not executable proof. |
| P08/P09 | Lifecycle patched; process/release NOT_RUN | Add actual daemon signal, child/maintenance loss with live control, readiness downgrade within deadline and FD/custody residue scenarios. Do not expose production fault-control solely for tests. Include different-UID kernel authorization and wire fuzz where applicable. |
| R3 semantic omission | OPEN_PROOF_GAP; producer owner required | In Semantica bind source-plan, shadow policy, prior sealed owner state and cluster-plan identities. Use independent fixed goldens for exact replace/tombstone/unchanged partition before dispatch. Quanta owns admitted mutation validity, not reconstruction of producer IR. Empty lexical-only/unchanged deltas remain valid. |
| R4/P10 | Hardened local owner; release/target NOT_RUN | Reissue frozen-source owner proof; inventory authorized retained roots and validate current backup/restore/verify + typed legacy refusal. No legacy importer and no production-root mutation authorized here. |
| R5/P11 | Preflight repaired; OPEN_PROOF_GAP | Bind resolver mapping as typed receipt evidence, actual build/test terminal outcomes and exact clean source pair. Run live publish/activate/restart/query and negative bundle/transition cases. Mapping log alone is not the paired receipt. |
| R6/P12 | BLOCKED operational inputs; final qualification NOT_RUN | Obtain host/root/config/retention/rollback authority. Separate deploy/activate/rollback typed action receipts; independently audit authentic historical handoffs; reissue final-source DAG. Never infer activation from deployment or recreate absent handoffs. |

## Local verification

Focused commands and final results are recorded in `EXECUTION-PROGRESS.md`.
Raw local logs: `/tmp/quanta-residual-audit.r8ZupY/` (temporary, not a durable
release archive). Shared moving source and unrelated writers exclude exact-source
qualification. No passed proof manifest or production verdict was issued.
