# J7Q-05 — Operator Ergonomics

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Goal

Make the search stack diagnosable by operators without reading runtime internals
or test code.

## Current Code Fact

- `searchctl` exists
- runtime and ingest/query surfaces are real
- diagnosis workflows are still thin relative to the shipped runtime surface

## Owner Seam

- searchctl CLI
- SDK request/response rendering
- runtime route and readiness exposure

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchctl/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/search.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`

## Preferred Implementation Direction

- start with read-only diagnosis surfaces and machine-readable output
- expose generation, authority, route, readiness, and typed remote error data
  directly
- keep operator commands stable enough to script
- use stable JSON field names and stable exit semantics for automation

## Layer Boundary Clarification

- this ticket owns operator diagnosis quality
- it does not own end-user ambiguity repair copy from `J7Q-06`

## Concrete Work Items

1. Add `searchctl doctor`.
2. Add `searchctl readiness`.
3. Add generation and authority inspection surfaces.
4. Add explain and perf-tail inspection commands.
5. Improve typed remote error rendering.

## First Increment

- add read-only diagnosis commands before any mutating workflow

## Red Rail To Pin First

```bash
./scripts/cargow test -p quanta-index-searchctl --test cli_smoke -- --nocapture
```

## Worker First Commands

```bash
sed -n '1,260p' crates/quanta-index-searchctl/src/lib.rs
rg -n "usage|output|snippet|explain|history|structural" crates/quanta-index-searchctl/src/lib.rs -S
```

## No-Go

- do not add diagnosis features that guess missing runtime state
- do not hide typed remote failures behind generic CLI strings
- do not ship operator diagnosis that is human-readable only

## Reviewer Rejection Checklist

- reject if routine diagnosis still requires reading source or debug logs
- reject if commands are human-readable only and not scriptable
- reject if the CLI swallows route or generation provenance

## DoD

- common operational questions are answerable from CLI surfaces

## Not Done If

- operators still need repo-internal code reading for basic runtime state
- diagnosis commands are missing or heuristic
