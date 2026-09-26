# MAY-31-001 — LanceDB Semantic Generation Authority

Status: `Accepted`

Decided: 2026-05-30

Amended: 2026-05-31 — three hardening rounds retained the backend decision and
closed verified lifecycle, integrity and restart findings.

Amended: 2026-09-21 — the state-root V2 cutover removed the boot-time legacy
importer. Legacy journal markers now cause typed refusal before adapters open;
conversion is offline-only under SEP-21-004.

Consolidated: 2026-09-27

## Context

Semantic state was historically reconstructed from `journal.cbor` into an
in-memory index during boot. An initial replacement implemented the desired
generation layout with an in-house CBOR and HNSW store, but that contradicted
the explicit LanceDB adoption decision and created a second backend design.

## Decision

### Backend and boundary

The durable semantic backend is the real `lancedb` crate. The semantic adapter
owns the async runtime and contains the async-to-sync bridge behind the existing
synchronous ports. Vendor types and layout knowledge do not escape the adapter.

Supply-chain and build-cost exceptions are explicit, named and scoped. They do
not relax deny-by-default policy for unrelated dependencies.

### Durable authority

Semantic data is generation-scoped under the semantic state root. Each sealed
generation binds repository, revision, generation, model and manifest identity,
row-set integrity and lifecycle markers. Build occurs in staging; serving opens
only a validated sealed generation.

The adapter opens the persisted LanceDB dataset and vector index directly.
Boot-time replay is not a steady-state serving mechanism. Missing, unsealed,
schema-incompatible, model-incompatible or integrity-invalid state fails
closed and cannot become readiness.

### Activation and readiness

Generation selection stays outside vendor code. Runtime readiness is seeded
from validated persisted generations and remains scoped by the canonical
generation identity. A cached handle does not create authority beyond its
sealed manifest and active generation binding.

### Legacy migration

`state_root/semantic/journal.cbor`, old migration markers and mixed legacy/current
roots are unsupported by the live daemon. Their presence is detected before
adapter open and returns `STATE_ROOT_FORMAT_UNSUPPORTED` without mutation. The
hot path contains no legacy decoder or importer.

Any required conversion is an offline state-root operation governed by
[SEP-21-004](SEP-21-004-process-supervision-state-cutover-and-proof.md): preserve
the old root, build and scrub a current staging root, publish its manifest last
and cut over atomically. The legacy journal is never a second live writer,
serve authority or silent fallback.

## Rejected alternatives

- in-memory HNSW plus LanceDB as an optional sidecar;
- permanent dual-write to journal and LanceDB;
- boot-time legacy import;
- silent journal replay after persisted-open failure;
- vendor-specific logic in core, contract or dispatcher layers;
- the superseded in-house CBOR/HNSW backend.

## Consequences

- A LanceDB upgrade is a storage-contract and supply-chain change.
- Migration, direct-open and restart evidence remain distinct from query
  quality or ANN performance qualification.
- Exact current layout and manifest fields remain code-owned by the semantic
  adapter; this ADR owns the architectural choice and authority boundary.

## Historical record

The LDB-00 through LDB-E2E-01 packet is indexed in
[the completed-plan archive](../plans/ARCHIVE-INDEX.md).
