# CS-BENCH-02 — Native-derived normalization and evidence rejection

Status: **OPEN — native/normalized rejection invariant FAILED** on the
`b42a9b5d` dirty audit source. A fresh fixed-native cs probe reproduces F08;
complete native-bound scoring/replay qualification is **NOT_RUN**.
Category: benchmark evidence correctness. Finding: F08; supports F01/F09.

## Current remaining work

`lexical_file_comparison.py::product_result` validates normalized task/query/gold,
paths and hit-flag consistency, but does not load/rederive the retained native cs
stdout. The new exploratory producer retains native bytes/digests; that does not
bind them at the scorer. A fixed native `other.rs` response scores zero against
`answer.rs` gold. Mutating only normalized `paths` and `file_hit_at_10` scores one
while native bytes and `stdout_sha256` remain unchanged. The required refusal
was not observed. The fixture uses an independent fixed native oracle and does
not allege actual capture tampering.

Reproduction command, raw result and source identity:
[CS-INT-01](CS-INT-01-integration-and-qualification.md#current-remaining-work-audit--2026-09-27).
The Sourcegraph validator and exploratory live producer are partial implementation;
do not label all native adapters absent or their exploratory output qualified.
Complete one shared acquisition/replay decoder per admitted product, bind native
bytes/request/source to evaluation, and execute the negative matrix below before
using these rows for comparative quality qualification.

## Purpose and RCA

An in-memory mutation of a cs capture preserved native stdout but replaced the
normalized paths and hit flag. The normal comparator accepted the score changing
from 19/20 to 20/20. It checked consistency between two derived fields, not their
consistency with the native response. No actual capture tampering was observed.

Owner: [lexical file comparison](../../../../tools/benchmark/retrieval/lexical_file_comparison.py),
especially the product result reader. Existing native adapters include
[Sourcegraph](../../../../tools/benchmark/retrieval/sourcegraph.py),
[Semble](../../../../tools/benchmark/retrieval/semble.py) and the
[Quanta SDK recorder](../../../../benchmarks/retrieval/src/sdk.rs).

## Decision

Use one versioned, deterministic native decoder per product/response format for
both capture normalization and replay validation. The capture stores immutable
native bytes; the scorer re-derives the normalized result and refuses disagreement.
Remove normalized hit flags as trusted inputs. Compute hits from validated results
and independently bound gold.

Do not create two parsers with subtly different acceptance rules, one for capture
and one for scoring. Native decoders are pure domain adapters under the current
benchmark owner; [evidence.py](../../../../tools/benchmark/evidence.py) and
[bench-protocol](../../../../benchmarks/bench-protocol) retain envelope authority.

| Evidence | Authority |
| --- | --- |
| Result path/span/order and server completion/error | Native response under a pinned decoder contract |
| Submitted query/config/endpoint | Bound request/transport capture; response echo when available |
| Client elapsed time/process exit | Trusted capture boundary, not server-result text |
| Index scope/source identity | Corpus binding and index publication/readiness evidence |
| Relevance/hit flags | Independent gold plus validated result identities |

Not every server response echoes query or source identity. Preserve the separate
bound request/index evidence instead of claiming those fields were decoded from
bytes that do not contain them. Hashes prove byte consistency, not truthfulness
of an entirely forged capture; acquisition custody remains a separate guarantee.

## Native contract requirements

- HTTP 200 alone is not success: validate native errors, terminal completion,
  timeout/limit flags and partial streaming state. Likewise validate each CLI's
  declared exit semantics rather than treating every nonzero code as no matches.
- Parse Sourcegraph event types and terminal state, OpenGrok fields/map semantics,
  cs stdout/stderr and exit status, Semble native records and Quanta typed outputs
  according to pinned formats. Missing native bytes means the dependent claim is
  BLOCKED, not reconstructed from normalized fields.
- Preserve native relevance order only when attested by the product contract.
  Otherwise label observed order. Dedup/grouping must be an explicit transformation
  recorded with policy and input identity, not silently applied by one decoder.
- Reject malformed/truncated frames, missing tasks, duplicate task IDs, invalid
  ranges and inconsistent totals. Legal repeated stream events require a defined
  native policy; do not weaken existing duplicate-rejection gates generically.
- Canonicalize paths against the admitted repository root: reject escapes,
  ambiguous prefix stripping, wrong case mapping, unknown files and wrong source
  snapshots. A path string alone cannot establish an indexed span.
- Validate source-bound Quanta hits against the published registry as today;
  external products with only file results cannot fabricate span credit.
- Decoder version/config and raw digests participate in capture identity. Preserve
  immutable historical readers where explicitly supported, not current admission
  by relabeling old payloads.

## Shared-control-plane boundary

Reuse MISC-01 publication/GC custody and MISC-03 bounded raw/archive/log I/O.
Streaming decode can be incremental and bounded but must retain the exact native
bytes needed for replay, terminal completion and resource refusal. This RFC owns
semantic native-to-normalized consistency, not another run store or process owner.

## Tests and DoD

- [ ] Retain the cs 19→20 mutation as a regression: unchanged native bytes plus
  altered normalized path/hit fields is rejected before scoring.
- [ ] Equivalent path/span/order/query-binding mutations are tested for each
  participating product, including legitimate zero-result complete responses.
- [ ] HTTP-success/error-body, partial SSE, timeout, duplicate/missing task,
  malformed encoding, oversized payload and wrong-index controls reject correctly.
- [ ] Acquisition and replay use the same decoder version and produce identical
  canonical normalized bytes on valid frozen native fixtures.
- [ ] Changes to raw, derived or bound-request metadata cannot pass by merely
  recomputing a self-consistent inner hit flag or untrusted digest.
- [ ] Historical unsupported formats remain diagnostic/BLOCKED for affected
  claims; all new qualification captures include sufficient native evidence.
- [ ] Real captures for Sourcegraph, OpenGrok, cs, Semble and Quanta replay through
  the canonical pipeline; unavailable products are explicitly excluded, not zeroed.

Expected results come from small independently written native-response fixtures
and documented format contracts, not the capture serializer's own output alone.
Coordinate negative tests with BENCH-03 denominator handling and CS-INT-01 source
closure. No ranking or engine schema change is prerequisite for the core refusal.
