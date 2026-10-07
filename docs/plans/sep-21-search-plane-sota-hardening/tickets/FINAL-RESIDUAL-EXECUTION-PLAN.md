# SEP-21 residual execution map

Status: `ACTIVE_RESIDUAL`. This is the single R0–R6 acceptance/execution map;
S21-11/12/13 retain detailed state/pair/release conditions.
Current proof IDs/commands/targets/staging are owned by
[proof authority](../../../../tools/ci/proof-authority.toml),
[test authority](../../../../tools/ci/test-authority.toml) and their independent
checkers. Completed process/receipt contracts live in
[SEP-27-005](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md)
and [OCT-05 ADRs](../../../adr/README.md#oct-05-implemented-contracts).

This map preserves remaining release oracles and dependency order. Typed proof
results, Active selection/view behavior, separated maintenance and bounded
operator projection are implemented owners; reuse their matching proof instead
of treating the old creation worklist as missing APIs.

## R0 — Actual proof production and trusted scope

Use existing result producers/schema/parser for complete selected/executed/
passed/failed/ignored/timeout/exit outcomes and retained raw identity. Revalidate
forged counts, wrong executable/source/binary, stale/reordered/partial output,
wrong host/run and tampered evidence. Caller-written success and arbitrary logs
cannot issue passing proof. Bind qualified host class to trusted runner/inventory
and CI bundles to their actual producing run. [S21-13](S21-13-release-evidence-and-sota-qualification.md)
owns final source/dependency/config/binary/host and graph admission. P11's common
operational producer/result mode is implemented; R6 retains concrete targets and
independent observer/authorized-input acceptance.

Execute final source-bound portable/conditional SDK/contract/native production
and relocated replay, retaining independently collected full-file identities and
exact sibling consumers, including paired-run/frozen-receipt paths. Run registered
strict JSON, JUnit/Nextest, shell/argv, no-follow captured-byte, command/tool and
owned-process controls against live required inventory. Invocation-local DAG
reuse needs independent aggregate timing before a speedup claim; no second
parser/cache owner. [MISC](../../sep-27-misc/tickets/INDEX.md) owns common regression,
retrieval, resource and measurement execution once; do not duplicate heavy rails.

## R1 — P03–P08 selected release counterexamples

| Node | Required independent release distinction |
| --- | --- |
| P03 | Actual activation/recovery, exact ACK replay and publish-only nonactivation |
| P04 | Request-held physical view across activation/retirement/GC; old-view compaction/quarantine, panic/cancel release, cross-repository and auxiliary-epoch churn |
| P05 | Fixed expected IDs/order/window/count/completeness and every selected quality subcommand |
| P06 | Actual SDK/daemon same-variant and wrong-identity refusal; scripted peers retain their narrower scope |
| P07 | Approved real-provider identity, egress/budget/cancellation and actual release target; hash/spy proof is separate |
| P08 | Release-process signals/child loss/readiness, FD/lease residue and shutdown |

P04 preserves per-domain evidence/resource groups, one physical identity per
request, no ambient latest reads and no deletion while a handle/flight lives.
The deterministic un-tokened select/retire-before-acquire counterexample is
implemented/executed under [OCT-05-003](../../../adr/OCT-05-003-active-query-and-runtime-lifecycle.md).
Selection itself is not a lease; preserve explicit-pin/tokened refusal semantics.
Only a selected stronger pin guarantee reopens that admission design.

Each staged node needs its concrete registered target, independent oracle,
attested release/host and actual parser/terminal result. A recipe name or broad
daemon pass cannot promote staged authority. Inspect current target coverage
before adding code; owner-local proof does not close the release node.

## R2 — P09 process truth and operator acceptance

Revalidate existing bounded active-backend identity/freshness probes on the
selected release process: stale/missing/divergent track roots, responsive control
with backend loss, supervised child/maintenance loss, zero-active behavior,
authorized restoration and declared detection cadence. Deep-open/scrub remains
the separate full-content check. Retain oversized identity/marker negatives;
root liveness does not prove full content health. Slow-root metering separation already has a
deterministic owner test; final resource/load claims keep their actual scope.

Use the existing Admin-authorized bounded IPC ring projection. Retain observer
denial before ring read, count/byte caps, wrap/drop/gap/process-instance facts,
restart, two-UID admission and exact queue/backend/provider/terminal correlation.
Existing component/OS-child/Linux proofs are reused only under compatible binding;
selected installed/release proof remains distinct. No new ring/principal/metrics
backdoor is required. Current source impact is owned by OCT-04 I0/E3.

## R3 — Independent semantic omission oracle

The selected Semantica production handoff constructs a semantic plan from
lexical emissions/prior state and adds cluster mutations. Its aggregate prepare
path does not yet compare the batch against a separately derived expected
replace/tombstone/unchanged partition. This is missing producer verification
code, not an observed omission incident. [The OCT-04 audit](../../oct-4-parallel-closure/tickets/INDEX.md#o4-i0-03)
records the source boundary; authorized Linux inputs are not prerequisites for
this upstream implementation.

Source comparison on 2026-10-07 used the sibling Semantica checkout at
`0b69971be2892f6119921fe8e2b7c494890850c4`, including its current working tree.
Within `quanta-runtime/src/retrieval/port_impls/index_projection_writer/`
`source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/`,
`aggregate_prepare.rs` resolves the prior shadow state and builds/prepares the
lexical/semantic batch; `semantic_state.rs` derives replacement/tombstone scopes
from lexical emissions and the prior state. Their supplied-scope and prior-base
checks are implemented. This selected call path has no independently derived
expected replace/tombstone/unchanged partition comparison. That is the remaining
verification-code seam, not a missing semantic planner or an observed omission.
No upstream source was edited or its tests executed by this documentation audit.

Producer source-plan/shadow policy/prior sealed owner/cluster plan must independently
enumerate replace/tombstone/unchanged scope before dispatch. Bind that expected
partition/digest to the existing batch/terminal chain and reject omissions or
duplicates. Without this upstream oracle, keep valid lexical-only/unchanged empty
semantic deltas and explicit unknown completeness; producer self-attestation
cannot establish the fact being checked. The producer owner supplies actual proof.

## R4 — P10 current-format state

[S21-11](S21-11-state-migration-backup-and-restore.md) and the
[operator runbook](../../../operator/state-cutover-runbook.md) retain exact state
oracles/commands: stopped daemon/exclusive lease, original manifest/copy/catalog
custody, backup/verify/separate restore, incarnation rotation, attested reopen and
restore-forward. Execute disposable and separately authorized target scopes with
actual formats/retained-data policy; no legacy importer or target mutation follows
from a local pass. Installed/Linux release proof has its own selected target.

Execute actual native append/clear/membership-replace/replace/tombstone exporters
and independent complete-row/window/delete/commit replay at the selected source.
Retain the unchanged maximum-page/cursor scenario and reissue matching owner/
daemon proof. Schema/retained-data inventory and authorized target restoration
remain separate from disposable-root and exporter tests.

## R5 — P11 exact pair and terminal build/test

[S21-12](S21-12-cross-repo-terminal-receipt-cutover.md) owns canonical resolved
dependency roots/nested locks, typed paired receipt and the frozen source pair.
Execute actual fresh producer/daemon build, live V2 publish/activate/restart/query,
negative bundle/transition/wrong-tree/binary and exact ACK replay through the
registered cross-repo/QBC rail. Bind build/test terminal results; mapping logs,
binary hashes and Quanta-local CAS alone cannot close the terminal chain.

The existing recipe's optional typed caller/kernel archive and Semantica
nextest completion custody are implemented; their owner tests are separate from
the remaining clean-pair execution. Keep `runner-candidate-only` distinct from
P11 release/operational nodes. Freeze clean sibling checkouts so Semantica's
actual relative Cargo paths resolve the selected Quanta tree and nested lock.
Use the recipe's independent fresh release lane; never pass the SDK proof target
to its `clean` command. A provided old SDK binary must not stand in for current
fresh-build byte parity. [I0-03](../../oct-4-parallel-closure/tickets/INDEX.md#o4-i0-03)
retains the current implementation and actual execution checkpoints.

## R6 — P11 actions and P12 aggregate

Use the implemented typed deploy/activate/restore-forward producer/recipes and
result checker/aggregate. Register concrete target adapters and distinct
independent pre/post success contracts under S21-12. Required inputs: authorized
Linux host, install/config/state roots, retention and rollback window. Preserve
staging until concrete target/observer authority is registered and executed.
Each action binds one shared operational host and attested release; deploy does not imply serving
activation or an actual P10-compatible restore-forward drill.

After selected integration, execute current P12A and final exact-pair graph/
aggregate with `--require-all --bind-source`; code/deploy/activate/rollback remain
separate verdicts. Audit authentic historical handoffs independently; missing old
records cannot be reconstructed and are not current release dependencies.
[Execution entrypoints](prompts/README.md) own commands. Relevant source/input
changes recheck affected proof; no stale/staged/partial receipt issues readiness.
Exact older creation plans and execution history are recoverable through
[the history index](../../../ARCHIVE-INDEX.md#historical-record-recovery).
