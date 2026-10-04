# OCT-04-003 — Optional source-preparation SDK

Status: `Proposed` — no new public API or format support is approved.

Source audit: clean `main@e43cda8c87b4a06fecac82a266011e30f84a2986` on 2026-10-04.
The Sep-24 SDK DSL, source-preparation and repository-format drafts are
recoverable from Git history. Accepted
[MAY-27-002](MAY-27-002-sdk-ingress-and-public-surface-boundary.md) owns the
current public ingress boundary.

## Current boundary

- `quanta-index-sdk/src/lexical.rs` publishes prepared `SearchCorpusBatch`
  values and exposes `publish_and_activate` with a separate CAS result.
  Lexical replacement supplies source bytes/chunks/symbols; semantic derivation
  requires typed `SemanticSourceRecordV1`. No public `SourceAdapter`,
  `PreparedSource` or `SourceBatch` abstraction exists.
- Contract admission now validates lexical file mutations by source-file key
  (`contract/src/ipc/ingest/validation.rs`), including duplicate replacement
  and record/source-path consistency. The old draft's request to first tighten
  this contract is stale; its Semantica producer assumptions are unverified at
  this source revision.

## Open decision

A compile-time preparation adapter may lower plain text/Markdown and external
formats to the existing typed batch and receipt/CAS flow. It must declare
lexical-only or semantic coverage explicitly and preserve the direct batch
path. Native media requires a separate typed engine/query contract; a text
field is not a media fallback. Do not add a daemon plugin registry or a second
durable publication protocol merely to accept another source format.

Before accepting an API, freeze a real producer fixture with co-located
chunk/symbol rows, replace/delete/clear and replay; prove deterministic source
keys, exact source offsets where claimed, bounds and digest identity. Then run
SDK-to-daemon publish, activate, restart and query, plus cross-repository
producer compatibility. These gates and format support are `NOT_RUN`.
