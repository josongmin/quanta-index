# SEP-21 execution progress

Status: open. This is a source-inspection ledger, not a qualification receipt.
The authoritative result SHA, source digest, command counts, and push state belong
to validated lane handoffs and immutable proof manifests, not this document.

## Current checkpoint (2026-09-23)

- Local `main` was clean at inspection and ahead of `origin/main` by five
  commits. W10 execution-truth and request-correlation changes are integrated
  locally; remote publication and final-source qualification are separate.
- P03–P10 have historical owner-proof manifests and
  `RELEASE_PROOF_PENDING` handoffs. Every corresponding release proof is
  `NOT_RUN`. Those manifests name earlier source revisions and do not qualify
  the current `main` source.
- `artifacts/sep-21/handoffs/` currently contains P03–P10 only. The P00,
  P01, P02A, P02B, P02I, P11, P12A, and P12 handoff chain is absent locally.
  A missing historical handoff cannot be replaced by a fabricated current one.
- P11's four external proof nodes and P12 final qualification remain staged.
  P12A infrastructure is declared executable, but its required handoff-DAG
  aggregate producer is not implemented.
- `/private/tmp/w10-r3` has six uncommitted P10 state-custody files. Treat
  those edits as concurrent owner work. Do not copy or overwrite them while
  the owner is active; reconcile their final diff into one P10 checkpoint.

## Structural work ledger

| Order | Owner | Current finding | Required closeout |
| --- | --- | --- | --- |
| 1 | P06 / S21-07 | SDK active-selector binding checks repo/revision but has no activation-resolution proof or epoch. | A response from the same repo/revision but wrong resolved generation/epoch is rejected by a consumer-visible negative oracle. |
| 2 | P08 / S21-09 | Hard escalation can release runtime guards while a child is unfinished. | No live child can outlast state-root lease custody; required-child failure and hard drain have process-boundary evidence. |
| 3 | P09 / S21-10 | Production control dispatcher composes `readiness: None`. | Supervisor-owned process readiness is wired; component death or stale heartbeat makes readiness false without confusing it with repository generation status. |
| 4 | P10 / S21-11 | Offline semantic importer writes lock/receipt/cleanup into the legacy source; RepoMap legacy activation is copied into an inert namespace; boot still migrates auxiliary snapshots. | Source inode/mtime/content remain unchanged, legacy authority is converted into current catalog/object authority, and boot contains no legacy importer. Restore/verify proves active identity, receipt, replay floor, and high-water equality. |
| 5 | P11 / S21-12 | V1 RepoMap mutation entrypoints remain reachable; no exact Quanta/Semantica commitment-chain or four P11 receipts. | One clean source pair and attested daemon binary pass publish, activate, replay, incompatibility, deployment, activation, and rollback proofs as separate nodes. |
| 6 | P12A / S21-13B | Aggregate schema/writer/validator do not consume the product handoff DAG or separate P12A infrastructure handoff. | Exact P00–P11 fork/join and serial chain, historical and final receipt ledgers, paired source, binary, and negative tamper cases validate. |
| 7 | P12Q / S21-13B | Final proof graph and P03–P10 release nodes have not run on one final source. | Same-source and same-binary final rerun yields distinct code/deploy/activate/rollback verdicts; no missing or stale mandatory receipt is promoted to green. |

## Execution constraints

- Repair the earlier owner boundary before P11. The existing P10 handoff's
  result revision is not current `main`; P11 must not consume it as an exact
  predecessor. Reissue a clean P10 checkpoint and proof after integration.
- P11 needs separately confirmed Semantica read/edit/commit/push authority.
  Provider egress, deployment, activation, and rollback are separate
  approvals. Without them, record the specific node as `NOT_RUN` or `BLOCKED`.
- Keep P10's in-flight worktree intact. Current-main edits must avoid its six
  owner files until that work is reconciled.
- A focused code test is owner evidence only. Release, Linux process,
  external-provider, deployment, and activation proof are not inferred from it.

## Current-main implementation log

### P08 custody correction — local working tree, not a checkpoint

- Owner: `SearchdSupervisor` and its runtime owner suite. On hard escalation,
  startup rollback with unfinished children, required-child loss, or second
  signal, the supervisor transfers unfinished joins together with runtime
  guards to a custody reaper. The supervisor returns within its deadline,
  but the state-root lease is not released before the child exits. Reaper
  spawn refusal retains custody until process exit; the terminal supervision
  outcome is already non-green.
- Same-boundary correction: a second signal interrupts the drain's polling
  loop rather than waiting for a child that ignores cancellation. The
  process-parent lease fixture now waits for the completed `held` report,
  not merely for file creation. A quiet 10 ms poll is not a hard-deadline
  expiry. A reported child must actually finish before its join is taken;
  otherwise it is escalated with its guards at the hard deadline.
- Production spawn topology correction: accept loops run in the one
  registered supervisor thread. The already-running maintenance timer is
  directly adopted by its join handle, avoiding an adapter-spawn failure
  that would strand the timer. Startup enrolls maintenance and provider
  custody before starting any accept loop. The provider child retains
  attempt custody past a failed first drain, so a non-green hard-deadline
  result cannot release the state-root lease while an attempt still runs.
  A poisoned provider-pool lock is recovered for drain rather than
  misreported as an empty pool. An early drop/unwind of the supervisor
  now requests shutdown and transfers child handles with guards to the
  same custody reaper. A panicking child stop callback cannot unwind that
  reaper before it joins its children.
- Local proof on the current dirty working tree: `just fmt-check` exited 0;
  `./scripts/cargow test -p quanta-index-searchd-runtime --test
  runtime_supervisor_owner_v1` exited 0 with 15 passed, 0 failed, 0 ignored;
  `./scripts/cargow test -p quanta-index-embed
  poisoned_registry_does_not_claim_a_live_attempt_is_drained --lib -- --exact
  pool::tests::poisoned_registry_does_not_claim_a_live_attempt_is_drained`
  exited 0 with 1 passed. Earlier registered owner-suite passes reached
  13 and 14 tests before later source changes, and hexagonal/wire-inventory
  checks passed. The module-snapshot check was interrupted to continue the
  owner fix. These commands are not exact-commit P08 proof. An earlier
  owner-suite run failed one fixture because the parent observed a
  partially written helper report; its exact-content gate fixed that race.
- Remaining for P08: final owner recipe and required daemon process rail
  on a committed source. The release-bound `p08-runtime-supervisor` node
  remains `NOT_RUN`.

## Audit corrections

- W10 execution truth and request correlation are integrated on local main;
  they are no longer listed as unmerged work.
- The production semantic adapter reruns a short approximate pass through an
  exact lane when the scope has unseen rows. Therefore a short ANN result
  alone is not a proven `ExactExhausted` defect. Do not reopen that claim
  without a reachable counterexample.
- The W10-R3 custody branch has two local commits and is clean at this
  inspection. It adds source immutability proof, but its legacy migration
  owner test still asserts that RepoMap activation bytes land only under
  non-serving `legacy-import/`. This does not establish active-authority
  conversion or replay floor/high-water parity; P10 remains open.
