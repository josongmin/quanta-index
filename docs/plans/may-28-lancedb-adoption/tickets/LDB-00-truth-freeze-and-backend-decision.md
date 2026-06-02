# LDB-00 — Truth Freeze and Backend Decision

Status: `done` (revised 2026-05-30)
Parent: [../README.md](../README.md)
Depends on: none

## 0. Outcome

The decision was made twice. Both are recorded for the audit trail.

**Initial decision (2026-05-29, SUPERSEDED)** — see [../README.md](../README.md)
§3.1. An in-house CBOR + HNSW backend under the same generation-scoped layout.
Justified at the time by sync-port + strict-supply-chain + cold-build
constraints. Implemented and shipped to "done"; the entire backend was
in-house, the `lancedb` crate was not used.

**Revised decision (2026-05-30, ACTIVE)** — see [../README.md](../README.md)
§3.2. The §3.1 outcome contradicted the packet name ("LanceDB adoption"); the
right action was to pay the engineering cost and adopt the real `lancedb`
crate (currently 0.30), not refuse it on cost grounds. The semantic adapter
now owns a `tokio::runtime::Runtime` and bridges async lancedb calls to the
sync port surface via `block_on` (the adapter *is* the async↔sync seam). The
deny.toml policy is extended with named, scoped exceptions following the
existing tantivy@0.22 pattern; the supply-chain posture (deny-by-default,
justified exceptions only) is preserved.

The runtime cutover, migration scaffolding, layout, manifest, READY/SEALED
markers, fail-closed semantics, and full test/proof surface from the §3.1
implementation are **reused, not thrown away**. Only the storage bytes change:
in-house CBOR columnar shard + persisted HNSW graph → lancedb-managed Arrow
dataset with lancedb's own vector index.

Live-truth drift (`T3.2 done`, README "Tantivy + Lance" ownership) is mapped
in [HISTORICAL-MAP.md](HISTORICAL-MAP.md) and closed in code by
LDB-02..LDB-E2E-01.

## 1. Purpose

Freeze one canonical statement of current live truth and one canonical decision
for the Lance cutover so the implementation does not start from already-drifted
docs.

## 2. Required outputs

- document the current live semantic path exactly:
  - in-memory HNSW in `quanta-index-semantic`
  - `SemanticAuthorityStore` journal persistence
  - boot-time `replay_into(...)`
- record the target durable semantic shape:
  - generation-scoped persisted semantic store under `state_root/indexes/semantic`
  - no steady-state full replay on boot
  - no long-lived dual-authority journal plus Lance mode
- freeze the exact adapter-local dependency choice:
  - high-level `lancedb` crate
  - lower-level `lance*` crates
  - or a documented reason to prefer one over the other because of the current
    sync port surface and runtime model

## 3. Owner files

- `docs/plans/may-28-lancedb-adoption/README.md`
- `docs/plans/may-28-lancedb-adoption/tickets/HISTORICAL-MAP.md`
- prompt-manager source docs that currently overclaim Lance once code lands

## 4. Work items

- enumerate the exact line-level drift between live code and current generated
  docs/plans
- choose one backend-local dependency posture and record why the rejected
  alternative is worse for this repo's current port/runtime shape
- freeze the cutover rule:
  - `journal.cbor` may exist only as legacy migration input
  - it must not remain a hidden second semantic authority

## 5. Acceptance

- no remaining ambiguity about whether the live tree already ships Lance
- no ambiguity about whether the target is "Lance as cache" or "Lance as the
  durable semantic query-serving backend"
- exact dependency choice and async containment strategy are written down before
  adapter code starts

## 6. Failure modes

- starting implementation from stale "T3.2 done" planning text
- saying "LanceDB" at a marketing level while leaving the crate/runtime choice
  unstated
- treating the old semantic journal as a permanent fallback path
