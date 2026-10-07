# OCT-04-001 — Search-corpus Ingest Pressure and Optional Selection Strengthening

Status: `Proposed` — open design, not implementation authority or qualification.

Updated: 2026-10-05. The original clean-main audit at `e43cda8c` remains in Git.
Completed selection, maintenance and timeout contracts are now consolidated in
[OCT-05-003](OCT-05-003-active-query-and-runtime-lifecycle.md); measured-cost and
conditional optimization boundaries are in
[OCT-05-004](OCT-05-004-cost-capacity-and-qualification-boundaries.md).
This proposal does not override the Accepted retire-first refusal contract.

## Completed scope removed from this proposal

Actual runtime OS-child tests covered selected G1 physical retirement before
acquisition, slow disk metering/readiness and default SDK30s timeout followed by
admitted publish/restart/exact replay. These are owner/process scope results;
shipping current-source/Linux release remains in the
[active residual ledger](../plans/oct-4-parallel-closure/tickets/INDEX.md#i0).
The old statements that these scenarios were entirely unexecuted no longer apply.

## Open decisions

1. A stronger guarantee that an un-tokened Active selection must survive physical
   retirement remains unaccepted. If that product guarantee is required, define
   its linearization point, retention authority and bounded short-lived claim
   transfer/release, then prove panic/cancel/GC and many active pairs beyond cache
   capacity. Current typed refusal without opening retired G1 is expected behavior,
   not sufficient evidence for adding admission pins.
2. Resource admission or wider dispatch requires actual full/delta/delete/seal and
   concurrent-query phase time, RSS and free/allocated/transient disk. Search-corpus
   ingress currently uses one process-wide serial slot; query admission is separate.
   Reconcile the global mutation-coordinator wording with actual build/journal/
   auxiliary-finalization ownership before widening concurrency. Do not infer an
   unsafe cross-process write merely from that scope distinction.
3. Retention/logical disk metering is not a hard physical quota. A hard cap needs
   declared filesystem/OS enforcement and must preserve active, rollback-required
   and live-reader generations. Source-bound physical pressure/merge high-water
   measurements and real storage power-loss proof remain unexecuted here.
4. Keep immutable generation publication and synchronous terminal receipts.
   Concurrent pairs require measured serial contention plus resource/durable-owner
   proof. Durable asynchronous acknowledgment additionally requires replayable
   source-byte custody and explicit queued/sealed/active milestones. Neither is
   accepted by the completed timeout/replay tests.

## Owners

- [O4-E4-01](../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-e4-01) owns causal
  cost and any measured follow-up durable-barrier decision.
- [S30-B07](OCT-05-004-cost-capacity-and-qualification-boundaries.md#whole-pipeline-measurement-acceptance)
  owns actual performance and indexing acceptance.
- [SEP-21 residual plan](../plans/oct-4-parallel-closure/tickets/INDEX.md#release-and-proof)
  owns shipping process/release qualification.

This compaction neither accepts the open designs nor supplies physical-pressure,
performance, power-loss or release evidence.
