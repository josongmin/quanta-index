# LDB-04 — Legacy Semantic Journal Migration

Status: `proposed`
Parent: [../README.md](../README.md)
Depends on: [LDB-03-search-plane-runtime-and-readiness-cutover.md](LDB-03-search-plane-runtime-and-readiness-cutover.md)

## 1. Purpose

Provide a one-shot path off the legacy `state_root/semantic/journal.cbor`
authority without leaving indefinite dual-write behind.

## 2. Required migration posture

Accepted migration strategies:

- explicit offline import tool
- startup-gated one-shot importer with idempotent completion marker

Rejected:

- permanent dual-write to both journal and Lance-backed state
- silent import-on-open fallback that users cannot observe or control

## 3. Owner files

- migration module/tool under `crates/quanta-index-searchd` or
  `crates/quanta-index-searchd-runtime`
- `crates/quanta-index-search-plane/src/ingest_dispatcher.rs` only as needed to
  isolate legacy journal reading
- migration tests/e2e harness

## 4. Work items

- read legacy `SemanticIngestBatch` journal in order
- materialize persisted semantic generations into the new durable layout
- validate manifest/dimension compatibility during import
- write an explicit migration completion marker or equivalent authority
- define post-migration behavior:
  - ignore legacy journal
  - reject mixed legacy/new partial state
  - or require manual cleanup with typed error

## 5. Acceptance

- legacy state roots migrate deterministically
- rerunning migration is idempotent or explicitly rejected with a typed reason
- partial migration does not produce false semantic readiness
- there is one documented rollback posture, not an implicit "just replay both"

## 6. Failure modes

- leaving legacy journal and new persisted state to diverge silently
- importing partially then serving mixed old/new semantic generations
- deleting the only recoverable legacy state before the new persisted state is
  validated
