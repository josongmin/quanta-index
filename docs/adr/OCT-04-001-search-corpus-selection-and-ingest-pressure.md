# OCT-04-001 — Search-corpus selection and ingest pressure

Status: `Proposed` — open design, not implementation authority or qualification.

Source audit: clean `main@e43cda8c87b4a06fecac82a266011e30f84a2986` on 2026-10-04.
Recheck the selected source before implementation. The September RFC is recoverable
from Git history; its source snapshot, performance claims and proposed gates are
not current evidence. Accepted [SEP-21-002](SEP-21-002-durable-authority-and-operation-lifecycle.md),
[SEP-21-003](SEP-21-003-read-view-continuation-and-provider-policy.md) and
[SEP-27-005](SEP-27-005-catalog-recovery-supervision-and-proof-custody.md)
remain authoritative.

## Current implementation and limits

- `query_dispatcher/selection.rs` resolves an un-tokened `Active` head before
  `query_dispatcher/read_view/view.rs::acquire_read_view` checks the ledger and
  acquires handles. The selected generation has no admission pin across that
  interval. Once acquired, `QueryReadViewV2` and `snapshot_registry.rs` retain
  handles against retirement. A transition-induced refusal in the interval is
  statically possible; no deterministic three-generation reproduction was run.
- Search-corpus ingest uses one process-wide serial dispatch slot
  (`quanta-index-ipc/src/admission.rs`), while query has separate admission.
  The SDK default I/O timeout is 30 s and ingest dispatch budget is 120 s.
  `server/peer_watch.rs` now detects hang-up and cancels the budget, but
  `ingest_dispatcher/dispatcher.rs::dispatch` checkpoints only at entry and
  deliberately settles an admitted publish. A timed-out caller must inspect or
  replay by operation identity; timeout alone does not establish rollback.
- [SEP-21-002](SEP-21-002-durable-authority-and-operation-lifecycle.md)
  describes one global `MutationCoordinatorV1` for all durable mutation.
  Production wires its port through `AuxiliaryMutationCoordinator`; the
  search-corpus path takes that guard for auxiliary finalization, while its
  preceding track build has separate operation locks and journal fences.
  Thus the accepted blanket wording should not be read as one guard over the
  whole publish. This scope discrepancy is not proof of an unsafe
  cross-process write. Reconcile authority and tests before wider dispatch.
- Lexical ingest already uses a buffered Tantivy writer, synchronous commit and
  seal-time merge wait. Semantic ingest uses bounded windows and a staging
  Lance dataset. The current `SearchCorpusIngestObservation` includes lexical
  substage and semantic phase durations. Neither a per-file direct-write claim
  nor the dominant indexing cost follows from the old benchmark.
- Retention measures deduplicated regular-file logical lengths, not physical
  allocated blocks or transient build/merge high water. The maintenance tick
  refreshes both track disk gauges through full-tree walkers and boot performs
  that refresh synchronously (`app/maintenance.rs`). Readiness uses the tick's
  freshness. No slow-walk latency or disk-pressure experiment was run here.

## Decisions requiring proof

1. Force `Active` selection of G1, then activate G2 and retain G3 so G1 is
   eligible for removal before view acquisition. If the baseline refuses due
   to retirement, add a short-lived admission pin under the catalog/retention
   authority, transfer custody to the acquired read view, and preserve exact
   pinned/token-bound refusal semantics. Prove active-pair cardinality above
   snapshot-cache capacity without pinning every active handle indefinitely.
2. Measure full/delta/delete/seal and concurrent-query phase time, RSS and
   actual free/allocated/transient disk before changing storage or dispatch.
   Resource admission must preserve the previous active and rollback-required
   generations and live readers. Logical retention is not a hard physical
   quota; a hard cap requires filesystem/OS enforcement.
3. If a scripted full-tree walk delays backend freshness, separate bounded
   backend health from paced disk metering with age/error reporting. Test the
   30 s client timeout against an admitted slow publish and exact replay.
4. Keep immutable generation publication and synchronous terminal receipts.
   Consider concurrent pairs only if measured serial-slot contention is
   material and resource permits plus durable ownership are proved. Durable
   asynchronous acknowledgement additionally requires replayable source-byte
   custody and explicit queued/sealed/active milestones.

Owner-local race, timeout and slow-walk tests are `NOT_RUN`; physical-pressure,
mixed-load performance, power-loss and release qualification are `NOT_RUN`.
The current performance owner is
[S30-B07](../plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md);
read-view and process proof remain in the
[SEP-21 residual plan](../plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md).
