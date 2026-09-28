# CS-BENCH-02 — Native-derived normalization and evidence rejection

Status: **OPEN** for remaining entrypoint/format coverage and real capture
qualification. Permanent enrolled cs/Sourcegraph/OpenGrok negative controls are
implemented in the live owner test. Current live verification rejects fixed-native normalized
path/hit mutations for cs, Sourcegraph and OpenGrok in a local fixture control.
Complete native-bound scoring/replay qualification remains **NOT_RUN**.
Category: benchmark evidence correctness. Finding: F08; supports F01/F09.

## Current remaining work

`lexical_file_comparison.py::product_result` is a normalized-row diagnostic
scorer. It validates task/query/gold, paths and hit-flag consistency, but does
not itself read native responses. Its historical F08 0→1 control establishes
that boundary, not a current bypass of every guarded caller.

Current `live_lexical_external.py::verify` reuses `_cs_response`,
`_sourcegraph_response` and `_opengrok_response` to rederive rows from retained
native bytes plus bound process/HTTP metadata, then compares canonical rows.
`code_search_workflow.py` calls this verifier in capture and replay verification
and binds native/component identities. Do not request this implementation again
or describe raw retention as the only current protection.

A current local control changed an empty-result task's normalized path/hit,
recomputed the row digest and kept all native bytes unchanged. For each of cs,
Sourcegraph and OpenGrok, the normalized scorer's total changed 1→2 while the
live verifier refused with `external row disagrees with retained native
response`. This verifies those fixture refusals; it does not establish actual
backend indexed-universe attestation or full workflow execution. The control
now runs in the permanent fake-service owner fixture selected by the existing
benchmark-control rail. It is not a production capture.

Remaining integration is in
[CS-INT-01](CS-INT-01-integration-and-qualification.md#serial-acceptance-boundary).
Historical reproduction bodies are recoverable through the
[plan archive](../../ARCHIVE-INDEX.md).
Inventory every admitted acquisition/scoring/replay entrypoint. Qualification
must require native validation on each reachable path; the bare diagnostic
scorer cannot independently attest native agreement. Retain the current live
refusals in enrolled owner tests, complete the negative matrix below and
audit Semble/Quanta under their existing native owners before qualification.

## Purpose and RCA

Normalized path/hit consistency cannot establish agreement with native results.
The unchanged-native mutation is now refused by the current live verifier for
the three local fixtures above. Remaining work is caller enforcement and
coverage of each admitted format, not a reproduced live-workflow bypass.

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

- [ ] Enroll permanent cs/Sourcegraph/OpenGrok unchanged-native path/hit controls
  that recompute normalized-row digests and still require native disagreement
  refusal. Preserve a bare-scorer control to distinguish diagnostic scope.
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
