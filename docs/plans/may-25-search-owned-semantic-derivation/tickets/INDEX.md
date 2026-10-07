# Semantic ownership — remaining acceptance

Status: `ACTIVE_RESIDUAL`

Contract authority: [semantic generation ADR](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md)
and [query/publication ADR](../../../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md).
This ledger does not reinstall the superseded SEM-OWN worker/API proposals.

## Current source boundary

Compared with `6a3f6afc8c286176962e722ce75524aefcfa7607` on 2026-10-07.
Typed-source derivation, manifest model/vector normalization, exact-text cache
identity and common query gating are implemented under
[MAY-31-001](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md#text-vector-and-cache-identity).
They are not new implementation tasks.

Daemon composition registers provider retry/HTTP/transport counters, raw-vector
normalization tallies, cache hit/miss/retention counters and cache-open reports.
`e2e_metrics_scrape.rs` checks their boot-time presence under the OpenAI profile
without issuing a real embedding request. This is narrower than live provider
behavior, request latency or pending-work/seal-lag acceptance. Inspect actual
exports before selecting an additional metric; do not recreate these counters.

## Required evidence before closure

| Scope / owner | Remaining acceptance |
| --- | --- |
| Producer + SDK/search-plane | Typed-source ReplaceGeneration and Delta, explicit no-op/tombstone membership, no legacy vector or implicit chunk-text ingress; actual paired producer/consumer source binding |
| Producer aggregate publication | Bind authoritative prior semantic state for deltas; verify resolver/aggregate/outbox retained state and paired restart against both current repositories before repair or closure |
| Search-plane + semantic adapter | Restart after partial derivation, delete/tombstone/membership replacement, complete sealing, provider/model/dimension refusal and blocked activation, under one fresh source-bound integration rail |
| Query/embedding owner | Execute public semantic/hybrid text-policy and model/revision/dimension/normalization rotation cases against independent expected cache/provider observations, including `FooBar`, `foobar`, `foo_bar`, `foo bar`; existing unit model/cache refusals do not cover this integrated matrix |
| Provider + operator owner | Observe real requests, failures and cache behavior through existing exports. Decide the required request-latency, pending-work/seal-lag, active model/manifest, policy-drift and blocked-reason projection from actual available fields; boot-time counter presence does not establish their live semantics |
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
