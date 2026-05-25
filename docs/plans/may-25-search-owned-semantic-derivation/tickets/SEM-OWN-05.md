# SEM-OWN-05 — Legacy Vector Ingress Removal, Observability, and Final Proof

Status: `proposed`
Parent: [../README.md](../README.md)
Depends on: [SEM-OWN-04.md](SEM-OWN-04.md)

## 1. Purpose

Remove the old producer-authored semantic assumptions, add operator-facing
observability for the new embedder path, and close the final proof rails.

## 2. Deliverables

### 2.1 Cleanup

- remove or clearly demote producer-authored semantic vector docs
- remove or clearly demote public query-vector surfaces
- mark semantic publishing as internal-only implementation detail

### 2.2 Observability

Add metrics and typed events for:

- embedding request latency
- embedding request failure counts
- pending semantic jobs per generation
- semantic seal lag after lexical seal
- query-time embedder failures
- manifest/provider/model identity for active semantic generations
- render-policy drift detection
- blocked-generation reason counts
- query embedding cache hit/miss rates

### 2.3 Final proof

Required proof matrix:

- contract round-trip tests
- derivation worker unit tests
- restart/replay tests
- semantic e2e text query
- hybrid e2e text query
- delete-cascade e2e
- readiness/activation e2e

## 3. Acceptance

- no user-facing documentation tells external producers to publish embeddings
- no public semantic/hybrid happy path requires caller-side vector creation
- one real end-to-end flow demonstrates:
  - producer publishes chunk text
  - `quanta-index` derives embeddings
  - semantic seal is emitted
  - text semantic query succeeds
  - hybrid query succeeds
- one blocked-generation flow demonstrates a typed blocked reason and no false
  activation

## 4. Explicit residual risks to report if still open

- provider rate-limit behavior under bursty chunk ingestion
- semantic backlog recovery time after long downtime
- reindex cost when model changes between generations

## 5. Deferred extension lane

This ticket must leave one explicit follow-on hook for symbol-aware semantic
projection.

Deferred lane:

- symbol projection semantic corpus for owner localization, API retrieval, and
  callsite-intent retrieval

The current program does not ship that lane, but it must not foreclose it by
hard-coding chunk-only assumptions into public query semantics.
