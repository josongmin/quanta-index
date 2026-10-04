# SEP-21 residual execution plan

This is the current work map. The previous dated dirty-checkout overlays and
conditional importer plan are retained in Git history, not as executable
instructions. Reinspect code and proof registry at the source revision used
for each run; this document is not a receipt.

Completed contracts and structural repairs are owned by
[SEP-27-005](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md)
and the SEP-21 registry. The original dated audit and execution chronology are
recoverable through the [plan archive](../../ARCHIVE-INDEX.md).

## Code work in dependency order

Current implementation/status corrections and next-action acceptance:
[residual ledger](CURRENT-RESIDUAL-2026-09-26.md). Revalidate the selected
source before promoting a recorded open condition or proof.

### R0 — Establish the proof-result authority before release claims

Owners: `tools/ci/proof-authority.toml`, `tools/ci/proof-manifest.schema.json`,
`tools/ci/write-proof-manifest.py`, `tools/ci/lint/check-proof-authority.py`,
`tools/ci/tests/{test_write_proof_manifest,test_check_proof_authority}.py`,
`.github/workflows/correctness.yml`, and the canonical `Justfile` recipes.

- Define one versioned execution-result input per registered mode. For tests,
  derive selected/executed/passed/failed/ignored and exit status from an
  allowlisted machine-readable runner result; for operational actions, define
  typed observed pre/post state and outcome. Reject a `passed` terminal JSON
  whose result cannot be reproduced from the archived raw evidence. Keep
  evidence custody and result interpretation separate.
- Derive OS/arch from the issuing runner as today; do not accept a free-form
  `linux-production-like` claim as host qualification. Bind the host class to
  a trusted runner/host inventory, exact source, command, binary and artifact
  identity. The CI bundle consumer must bind the bundle to its trusted
  producing run, not merely download an arbitrary run ID and hash its files.
- Adversarial tests: forged counts, wrong executable, skipped/partial/timeout,
  reordered or stale result, wrong host class, tampered log, mismatched binary,
  wrong source and wrong CI run must all refuse. DoD: a hand-written terminal
  input plus an arbitrary log cannot produce an authoritative `passed` proof.

R0 is a release-evidence prerequisite, not a reason to stop independent P09
code development. Do not reissue owner receipts while source is still moving;
each commit invalidates earlier exact-source receipts.

### R1 — Close the P03-P08 release counterexample inventory, not the owners again

Use `tools/ci/proof-authority.toml` and `tools/ci/test-authority.toml` as the
target inventory, but retain independent acceptance oracles from the tickets.
For each staged node, record target path, exact negative scenario, expected
oracle, release-binary/host requirement, and result parser before promoting
`authority_state`. Candidate owners are:

| Node | Existing owner to inspect; edit only on a proved gap | Required release distinction |
|---|---|---|
| P03 | `crates/quanta-index-repomap/src/{store,pinned}.rs`, `crates/quanta-index-searchd-runtime/tests/e2e_generation_activation_concurrency.rs` | Live activation/recovery, ACK replay and no publish-only activation. |
| P04 | `crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs`, `crates/quanta-index-searchd-runtime/tests/e2e_read_view.rs` | Pinned handle across activate/retire/GC, not just an owner-local view. |
| P05 | `crates/quanta-index-contract-base/src/results/query_window.rs`, `crates/quanta-index-searchd-runtime/tests/e2e_keyset_cursors.rs`, quality oracle | Independent order/completeness oracle; a quality summary cannot replace it. |
| P06 | `crates/quanta-index-sdk/src/binding.rs`, `crates/quanta-index-sdk/tests/sdk_binding_owner_v1.rs` | Real daemon same-variant/wrong-identity refusal. |
| P07 | `crates/quanta-index-search-plane/src/query_embedder.rs`, provider owner test | Approved real-provider identity/budget/cancellation evidence, separate from spy tests. |
| P08 | `crates/quanta-index-searchd/src/app/{supervisor,searchd}.rs`, runtime supervisor owner test | Release binary signal/child-loss/FD/lease residue, separate from in-process supervisor tests. |

P04 lifetime acceptance retains activation/acquisition and retire-first barriers,
old-view GC/compaction/quarantine, panic/cancel reference release, cross-repository
churn and auxiliary-epoch changes. Require one evidence per declared domain,
explicit shared resource groups, identical physical identity throughout a request,
no ambient latest reads and no deletion until every live handle/flight permits it.
Add a deterministic un-tokened `Active` select-G1 / activate-G2 / retain-G3 /
acquire-view interleaving before changing the admission boundary; a view already
acquired proves a different lifetime. Keep tokened and explicit-pin refusals
separate. See [OCT-04-001](../../../adr/OCT-04-001-search-corpus-selection-and-ingest-pressure.md).

DoD per node: a concrete registered target and independent negative oracle;
no staged-to-executable flip based only on a broad `test-daemon` recipe or a
test name. If the existing code and target already cover the case, no product
code edit is authorized: only bind the executable rail and raw result.

### R2 — P09 process truth and one operator diagnostic path

Owners: `crates/quanta-index-searchd/src/app/readiness.rs`,
`crates/quanta-index-searchd/src/app/runtime.rs`,
`crates/quanta-index-ipc/src/counters.rs`,
`crates/quanta-index-contract/src/ipc/{control,split}.rs`,
`crates/quanta-index-search-plane/src/control_dispatcher.rs`,
`crates/quanta-index-sdk/src/{binding,observability}.rs`,
`crates/quanta-index-searchctl/src/lib.rs`, and
`crates/quanta-index-searchd-runtime/tests/e2e_process_readiness.rs`.

- Retain the implemented exact active-generation/token backend observation and
  bounded sealed-identity probes in SEP-27-005. Revalidate maximum detection
  cadence, stale/missing/wrong-identity refusal, authorized root restoration and
  zero-active behavior on the actual selected daemon. Root/identity probes do
  not establish full backend-content liveness; retain deep-open/scrub coverage.
  Script a disk-byte walk longer than three maintenance cadences and verify
  whether backend freshness is delayed; decide health/meter separation from
  that result, not from a disk-gauge value alone.
- Define a bounded control DTO and adapter projection from the *existing*
  `IpcServerCounters` ring. Bound event count and encoded bytes below the
  transport's 16 MiB frame cap; carry process instance, plane, sequence gap
  and drop totals. Never put request ID or payload in metric labels.
- Reuse the existing `Admin` owner/root admission for operator-only
  diagnostics unless a concrete distinct principal policy requires a new
  `Operate` capability. Authorization must precede ring read. Do not create a
  parallel ring, self-asserted principal, or a metrics-snapshot backdoor.
- DoD: observer denial with zero ring disclosure; operator success; invalid
  limit/oversize refusal; wrap/drop/instance restart; real daemon request ID
  queue→backend/provider→terminal correlation; supervised child, maintenance
  and backend loss independently make global readiness false. A post-boot
  rename/removal of either active lexical or semantic track root must make
  readiness false within the declared detection interval even when the
  inventory returns `Ok(empty)` and disk usage returns `Ok(0)`; the same
  missing root with zero active generations must not be misreported as loss.

### R3 — Semantic omission oracle before changing the ingest wire

Owners to inspect: `crates/quanta-index-contract/src/ipc/ingest.rs`,
`crates/quanta-index-search-plane/src/semantic_derive.rs`, and Semantica's
`search_plane_handoff_dispatch/{lexical_batch,semantic_state}.rs` plus the
source-plan/manifest that precedes them.

Prove whether that upstream source-plan independently enumerates required
semantic scope changes. If yes, bind its digest and exact scope partition to
the existing batch/terminal receipt and refuse omitted or duplicated scopes
before mutation. If not, do not invent a producer-authored completeness flag;
keep the documented lexical-only/no-op behavior and require a producer-side
oracle over source inputs. DoD distinguishes a legitimate unchanged semantic
delta from an erroneous missing scope without relying on the value being
checked to assert its own completeness.

### R4 — P10 current-format state custody

Owners: `crates/quanta-index-searchd/src/app/state_format.rs`,
`crates/quanta-index-searchd/src/app/state_migration.rs`,
`crates/quanta-index-searchd-runtime/src/state_migration.rs`,
`crates/quanta-index-searchd-runtime/tests/state_migration_owner_v1.rs`,
`crates/quanta-index-searchd/src/cli/command.rs`, and the operator runbook.
The backup/restore/verify CLI and legacy typed refusal already exist. Add
release-process counterexamples only where the existing owner suite does not
cover them; inventory actual state roots and retention obligations before any
cutover. DoD: stopped daemon + exclusive lease, current-format backup,
verify, restore-forward, activation-incarnation rotation and wrong-format
refusal on one disposable and one authorized target root. No legacy importer.

Original-manifest authority, exact paths and bounded catalog sidecar custody
are retained in SEP-27-005. Local checks do not discharge the authorized-target
or release DoD above.

### R5 — P11 exact source pair, resolved dependency roots and build result

Owners: `scripts/verify-repomap-cross-repo.sh`, Semantica's
`quanta-runtime-retrieval-kernel/Cargo.toml` and RepoMap producer/validator,
`tools/ci/lint/check-proof-authority.py::paired_source_snapshot`, the common
R0 result authority, and the P11 negative tests.

Before the fresh build, resolve every Semantica `quanta-index-*` path package
through the canonical build graph and require it to lie under the frozen
Quanta checkout at the expected package root; bind that mapping and lock
digest into the paired receipt. Then run the already selected live V2
publish/activate/restart/query and forged-receipt tests through QBC. Archive
the actual build/test result, not just the release binary bytes. DoD: a clean
Semantica checkout using a second Quanta checkout, a wrong binary, wrong
bundle, wrong transition, or ACK replay divergence is refused.

### R6 — P11 operational actions, then P12 custody

First obtain exact authorized Linux host, deployment path, config, state root,
retention and rollback window. Then add separate `Justfile` recipes and typed
result producers for `proof-p11-{deployment,activation,rollback}`. Extend the
registry/checker with an operational-action mode instead of pretending these
are test-authority selections. Deployment binds installed binary/config/root;
activation observes the serving identity and query path; rollback drills the
P10-allowed restore-forward boundary. DoD: each stage has a distinct external
pre/post observation and independently issuable manifest; deployment alone
cannot yield activated or rollback-proven.

Audit available historical P00-P11 handoffs with `handoff_validation.py`;
do not fabricate missing records. These records are not release prerequisites.
On the final frozen clean pair, reissue all current
owner/release proofs, P12A, and aggregate. `--require-all --bind-source`
must accept the exact pair and raw evidence; deployment, activation and
rollback verdicts remain separate. Missing, staged or stale evidence is not
production readiness.

Use the [residual ledger](CURRENT-RESIDUAL-2026-09-26.md) for remaining status.
This plan is an action map, not a passed receipt.
