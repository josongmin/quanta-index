# LDB-00 — Truth Freeze and Backend Decision

Status: `proposed`
Parent: [../README.md](../README.md)
Depends on: none

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
