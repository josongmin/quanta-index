# Semantic ownership — remaining acceptance

Status: `ACTIVE_RESIDUAL`

Contract authority: [semantic generation ADR](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md)
and [query/publication ADR](../../../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md).
This ledger does not reinstall the superseded SEM-OWN worker/API proposals.

## Required evidence before closure

| Scope / owner | Remaining acceptance |
| --- | --- |
| Producer + SDK/search-plane | Typed-source ReplaceGeneration and Delta, explicit no-op/tombstone membership, no legacy vector or implicit chunk-text ingress; actual paired producer/consumer source binding |
| Producer aggregate publication | Bind authoritative prior semantic state for deltas; verify resolver/aggregate/outbox retained state and paired restart against both current repositories before repair or closure |
| Search-plane + semantic adapter | Restart after partial derivation, delete/tombstone/membership replacement, complete sealing, provider/model/dimension refusal and blocked activation, under one fresh source-bound integration rail |
| Query/embedding owner | Manifest-authoritative query normalization/cache identity and typed provider failures; record the policy for `FooBar`, `foobar`, `foo_bar`, `foo bar` and prove model changes cannot reuse incompatible entries |
| Provider + operator owner | Observe request latency/failures, pending work/seal lag, query failures, active model/manifest identity, policy drift, blocked reasons and cache hits/misses; compare actual exported fields to this acceptance before adding a second metric path |
| Release/integration owner | Contract round trips, real typed producer → derived semantic seal → text semantic/hybrid search, restart/delete and blocked-generation tests; attach exact raw inventories and source/dependency/model/runtime binding |

The predecessor records do not establish current completeness or missing
implementation. Revalidate each item against live source and an independent
oracle; already implemented rows need fresh proof, not duplicate implementation.
The current release/cross-repository obligations are owned by the
[SEP-21 residual plan](../../sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md).

## Deferred design and measurement leads

- Shared batched/async corpus-query provider wiring, including provisioning
  symmetry; decide lifecycle/retry/recovery before adopting a worker/job store.
  Provider burst limits, retryable/terminal failures, long-downtime backlog and
  partial-work recovery need measured acceptance if that design is selected.
- Stable producer semantic-owner identities are required for incremental reuse;
  if stability is not proven, use full-generation rebuild and report its cost.
- Ranking/over-fetch or learned fusion changes require labeled relevance
  comparison; no ranker conclusion follows from the old seam audit.
- Model/dimension/render/normalization policy changes need a coordinated manifest
  rebuild, compatibility refusal and migration cost measurement.
- Richer render-policy introspection, multi-tier query caches and additional
  projection surfaces need a new current-contract proposal before implementation.

## Closure rule

Use terminal selected/executed/pass inventories and actual observed producer,
provider and restart behavior. A README, planned interface, cached handle, empty
queue or successful compilation does not prove completeness. Retire each row
only after its owning acceptance is revalidated; completed decisions remain in
ADRs and historical task bodies in Git.
