# CS-BENCH-02 — Native-derived evidence acceptance

Status: `ACTIVE_RESIDUAL` for selected format/entrypoint coverage and actual
capture qualification. Shared native decoders, unchanged-native mutation refusal
and scoped acquired-reader witnesses are implemented under
[OCT-05-002](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md).
Current captures/index authority are owned by
[OCT-04 E2](../../oct-4-parallel-closure/tickets/INDEX.md#e2).

## Remaining acceptance

Inventory every selected acquisition/scoring/replay entrypoint and native format.
Require canonical native validation for every qualified path and explicit
supported/unsupported/failed/blocked scope. Reuse current pure decoders; add
coverage only for a demonstrated uncovered format or refusal seam.

The bare [lexical scorer](../../../../tools/benchmark/retrieval/lexical_file_comparison.py)
reads normalized rows and remains diagnostic. The
[external verifier](../../../../tools/benchmark/retrieval/live_lexical_external.py)
re-derives rows from retained native bytes/metadata; the
[workflow](../../../../tools/benchmark/code_search_workflow.py) binds the components.
Native agreement alone does not attest independent relevance or the backend's
whole searchable universe.

| Fact | Authority |
| --- | --- |
| Result path/span/order and completion/error | Pinned native response decoder |
| Original/submitted query, config and endpoint | Bound request/transport; response echo where available |
| Elapsed boundary/process exit | Capture owner and actual terminal |
| Index/source identity | Corpus binding plus actual index/publication/read-view evidence |
| Relevance/hit | Independent gold and validated result identity |

A response need not echo query/source facts; retain their separate authority.
Digests prove consistency and cannot authenticate an entirely forged acquisition.
Disk/file-view and per-request acquired-reader observations keep their exact
attested/excluded scopes under OCT-05-002; global flags remain false when unproved.

## Required native/refusal controls

- HTTP 200 with native error, partial SSE/frame, timeout/cap, malformed encoding,
  invalid span/total, duplicate/missing task and oversized output follow their
  typed meanings. Complete zero-result queries remain legitimate. CLI exit codes
  use the pinned product contract rather than generic nonzero-as-no-matches.
- Canonicalize against the admitted root. Refuse escapes, ambiguous stripping,
  wrong case, unknown path/source and invalid ranges. Preserve independently
  authored ASCII/non-ASCII line/span boundaries. File-only results cannot acquire
  declaration/span credit from an emitted path or enlarged context.
- Mutate normalized path/span/order/score/status/hit, query/profile/runtime/index
  bindings and raw metadata independently, including recomputed inner digests.
  Contradictions must refuse. Legal repeated native events require an explicit
  product policy rather than generic duplicate acceptance.
- Capture and replay share decoder/config/version and produce identical canonical
  output from fixed independent native fixtures. Explicit grouping/dedup and
  observed-versus-relevance order remain part of request/result identity.
- Bind actual cs process stdout/stderr/exit, Sourcegraph terminal streams,
  OpenGrok fields/readers, Semble native records and Quanta typed SDK responses.
  Missing raw evidence blocks its dependent claim. Historical readers retain
  their immutable scope; old formats do not acquire current admission by relabeling.
- Execute each selected real-product capture and independent replay under the
  final source/request/unit/index binding. Missing products or formats remain
  excluded/blocked; focused fixtures do not replace the real execution.

Use existing [evidence envelope](../../../../tools/benchmark/evidence.py),
[bench protocol](../../../../benchmarks/bench-protocol), MISC publication/process/
bounded-I/O owners and current product decoders. Streaming must retain complete
replay bytes/terminal state and bounded refusal. No second parser/run store.

[BENCH-01](CS-BENCH-01-corpus-gold-and-holdout.md),
[BENCH-03](CS-BENCH-03-tracks-metrics-and-statistics.md) and
[CS-INT-01](CS-INT-01-integration-and-qualification.md#required-controls) own
independent gold, denominators and integration. Preserve source-bound registry
identity for Quanta hits; fixed native expected results cannot come solely from
the serializer being tested. Historical bodies are recoverable through
[the history index](../../ARCHIVE-INDEX.md#oct-05-residual-owner-clarification).
