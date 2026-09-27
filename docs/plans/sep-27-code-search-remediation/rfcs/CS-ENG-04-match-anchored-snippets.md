# CS-ENG-04 — Match-anchored snippets and context budgets

Status: matcher-aligned source previews **IMPLEMENTED** with historical owner and
real-daemon SDK execution in [L4 handoff](../handoffs/L4_HANDOFF.md).
Aggregate regex heap qualification: **BLOCKED**. Context-quality/performance and
final combined-source qualification: **NOT_RUN**.
Category: engine result presentation. Finding: F04; ranking remains ENG-03-owned.
Depends on ENG-01; definition anchoring also uses ENG-02/03 source facts/policy.

Final audit: E08 in [engine-audit.md](../engine-audit.md). The engine already has
a deterministic 240-byte renderer with snippet-relative offsets/highlights. This
RFC replaces its approximate anchor selection, not an absent snippet feature.

## Current disposition and remaining acceptance

At `b42a9b5d` plus dirty overlay, selected immutable candidates receive canonical
Boolean witnesses and verified original-byte provenance. False branches/NOT,
NFC/folded mapping, complete 240-byte focus, overlapping witnesses and explicit
optional refusal are implemented. Earlier passing receipts remain source-scoped;
later owner/property/SDK attempts are not a combined current-source qualification.

- **L4-R1, BLOCKED aggregate heap claim:** the 16 MiB base reservation plus
  256 bytes per estimated NFA state is a logical policy charge. AST/HIR,
  compiler temporaries, retained forward/reverse automata and search caches do
  not share an allocator-enforced admission boundary. Coordinate allocation
  admission before expensive work, preserve truth/range semantics and keep
  optional refusal from changing selected hits. No current runtime aggregate
  overrun has been reproduced. Acceptance and the historical standalone probe:
  [L4 regex residual](../handoffs/L4_REGEX_BUDGET_RESIDUAL.md).
- **INT-R1, NOT_RUN:** combined-source owner/SDK tests after the ranked-key merge,
  including 32/33-overlap edge, capture-removal differential truth/ranges,
  restart and mutable-checkout drift/deletion controls.
- **BENCH-03/04, NOT_RUN:** context utility, payload cost and p95 overhead under
  independently admitted labels/measurement. Do not infer those from correctness
  regressions or source inspection.

See [CS-INT-01](CS-INT-01-integration-and-qualification.md#current-remaining-work-audit--2026-09-27).

## Purpose and RCA

The renderer defects below describe the historical baseline. The current matcher
and provenance implementation replaces that approximate renderer.

The diagnostic has 142/180 first-file successes but 83/180 first-result full-span
successes. A correct file can return a reference or a window that misses the
declaration. The difference of 59 is not an estimate of the number recoverable
by snippet formatting: selecting a different match is a ranking decision.

The existing renderer collects literal query strings, including those under NOT,
and runs case-sensitive `find`/`match_indices` on stored text. Regex contributes
no anchor. Current-source owner-function probes miss the actual focus for folded
case, regex, token-boundary, NFC-source and negated-OR cases; a 200-byte literal
also gets cut despite fitting the 240-byte budget. The ordinary literal control
works. These are renderer-owner proofs with contract stubs, not full SDK reruns.

Symbol snippets are currently synthetic `local_name + container` labels, not
source excerpts. Candidate line ranges describe indexed chunks/declarations,
not the actual emitted window. Do not interpret either as source-context evidence.

Separate three authorities already required by SEP-26-001:

1. Indexed hit identity/span determines what matched and was ranked.
2. Returned context span determines preview usefulness and bytes/tokens sent.
3. Independently labeled gold spans determine task relevance/coverage.

No preview expansion can invent a different ranked hit, borrow another source
revision, or treat evaluator gold as a context selector.

## Decision

Return a bounded preview centered on the selected match or selected declaration
fact. For grouped-file results, the ranker chooses the representative match and
exposes its identity; the formatter does not secretly choose a new best result.

The response must identify the source hash/revision, indexed unit/hit, focus
span, returned context spans, truncation reason and measured bytes/tokens when
those quantities are available. Use existing response/registry fields where
equivalent; version an extension once, not duplicate adapter-specific envelopes.

## One match authority, bounded witness reconstruction

After the existing collector selects top-k candidates, reconstruct only the
needed witnesses using the **same prepared query and matching primitives**. Do
not materialize every matching span in the corpus or let presentation rerank.

A typed witness identifies the candidate/source, leaf and field, match semantics,
normalized and original spans, and completeness. Content, path and symbol-label
witnesses are distinct. A path-only match is not a content highlight.

- Keyword/phrase: reuse the normalizer's token positions/ranges and phrase-position
  authority. Raw substring search cannot stand in for whole-token matching.
- Raw substring: reuse the admitted NFC/case semantics and exact verification.
- Regex: extend the existing regex executor's boolean verifier with a bounded
  range API using that same compiled pattern and flags, not extracted literals.
- Boolean: only true branches contribute positive witnesses. AND preserves all
  required child truth; OR uses true children; NOT supplies absence evidence, not
  a forbidden-string highlight. Pure NOT/metadata-only results explicitly have
  no positive content anchor. Do not re-evaluate NOT over only the preview window.
- Definition: use producer declaration/name/signature facts and source-bound spans.
  An optional source excerpt and a synthetic display label have different types.

Reuse owners in
[normalizer](../../../../crates/quanta-index-lq-text-normalizer/src/lib.rs),
[phrase positions](../../../../crates/quanta-index-lq-positions/src/phrase_query.rs),
[match sets](../../../../crates/quanta-index-lexical/src/searcher/match_sets.rs) and
[regex executor](../../../../crates/quanta-index-lq-regex/src/executor.rs).
There must not be a second handwritten query matcher inside the renderer.

### Normalized matching is not original-byte matching

The native engine deliberately matches NFC text; keyword/raw case-insensitivity
uses per-character lowercase, while regex uses its own admitted case semantics.
Preserve that contract. Existing token offsets are into NFC text, not raw source.
Do not silently rename this mode raw-byte-exact or change normalization in a
snippet patch. BENCH-01/03 common-semantic oracles must account for this difference.

For selected chunks, generate a bounded request-local provenance map from raw
source through the actual indexed normalization. Composition, combining-mark
reordering and expanding mappings such as `İ` require interval provenance, not
one constant offset or one-to-one character indices. Verify transformed bytes
against the indexed authority. Return the covering original ranges and label
normalization equivalence honestly; do not assert byte equality when absent.

Mapping is relative to the indexed chunk. Whole-file and per-chunk normalization
are not interchangeable across a combining sequence at a chunk boundary. Add the
validated chunk source offset only after mapping; broader context reads use the
immutable published source, never a mutable checkout. Persistent global maps are
not required for the first implementation.

For a definition result, prefer the declaration's name/signature and a bounded
surrounding extent when source facts identify them. Full bodies are optional
context, not a requirement to return an enormous declaration unboundedly.
For content results, anchor the actual selected literal/regex match. Additional
matches may be included within the same declared budget and ordering policy.

## Span and budget invariants

- All spans are source-bound byte ranges; line/column values are checked
  projections with an explicit position encoding. Never mix UTF-16 and bytes.
- Bound per-result bytes, aggregate response bytes and tokenizer-versioned
  context budgets independently. Record both indexed and returned quantities.
- Preserve UTF-8 boundaries and CRLF interpretation. Do not silently move a
  match to the nearest valid-looking offset or use a newer file's text.
- Merge overlapping context ranges for cost/coverage accounting without erasing
  distinct match identities. Preserve deterministic result and range order.
- If a selected match/signature cannot fit, return a documented truncation or
  infeasible-context outcome. Do not exceed the budget and mark it compliant.
- If the complete focus fits the budget, reduce surrounding context first;
  a fixed 120-byte lead must not cut a 200-byte match in a 240-byte window.
- Unsupported zero-width or unusual regex span semantics require a typed policy,
  not a fabricated one-byte match. Missing source bytes produce a typed error.
- Snapshot identity is checked when reading context; stale sources cannot be
  presented under an otherwise valid indexed result.

Preview integrity failures (wrong source/hash/map) fail typed; do not serve a
misbound excerpt. Optional preview work-budget exhaustion returns explicit
`preview_unavailable`/clipping metadata without silently dropping or reranking a
valid hit. If a profile requires a complete preview, it refuses that request.
Freeze these semantics in the response contract before implementation.

Bound witness count, scanned source bytes, transformed bytes, map entries and
response bytes separately, with deadline/cancellation checks inside long-document
work. Reuse the existing regex compiler and bounded bitmap cache; share a compiled
executor within the request where available. A compiled-pattern cache is a
separate measured change. A matching-document bitmap is not a span witness.
Start with request-local reuse. A later leaf-witness cache binds source/candidate,
normalizer, field/leaf semantics, pattern/case flags and witness policy, and each
request recomposes Boolean truth using its prepared AST. A whole-query witness
cache additionally binds the canonical prepared-query digest and scope; otherwise
`A OR B` and `NOT A OR B` could incorrectly share positive anchors. Charge in-flight
references and temporaries, not only resident cache entries; incomplete witnesses
cannot enter a complete-result cache.

An indexed chunk remains the hit authority for chunk policies. Adding verified
context outside that chunk can improve returned-context coverage, but it cannot
retroactively improve indexed-hit rank metrics. Report both views separately.

## Owners and implementation boundary

- [Response contract](../../../../crates/quanta-index-contract/src/results/query_responses.rs)
  and [candidate extraction](../../../../crates/quanta-index-lexical/src/searcher/candidates.rs):
  enumerate existing snippet/source-read owners before extending the schema.
- [Chunking](../../../../benchmarks/retrieval/src/chunking/mod.rs) remains an index
  construction profile; changing fixed-window size is not the only preview lever.
- [SDK runner](../../../../benchmarks/retrieval/src/sdk.rs) records native values.
- [Evaluator](../../../../tools/benchmark/retrieval/evaluator.py) verifies separate
  hit/context spans against the published registry and immutable source.

ENG-03 owns any alternative representative selection. BENCH-03 owns metric
interpretation. Do not change both under an unlabeled single "snippet" ablation.

## Tests and DoD

- [ ] Fixture matrix covers a reference before a definition, long signatures,
  decorators, nested declarations, multiple matches and matches at window edges.
- [ ] UTF-8 multibyte text, CRLF, empty/last lines, oversized match, zero-width
  behavior and overlapping ranges have independent byte-exact expectations.
- [ ] The six negative owner probes plus positive control become native SDK
  regressions; add NFC composition/reordering, expanding lowercase, regex case,
  pure NOT, false OR branches, path-only and synthetic-symbol-label cases.
- [ ] Offset maps preserve the actual indexed chunk's semantics and source bytes;
  normalization does not masquerade as original-byte equality.
- [ ] Small and aggregate budgets never expand silently; returned metadata
  reflects actual clipping and tokenizer identity.
- [ ] Forged focus spans, wrong revision/hash, unregistered hits and cross-file
  context are rejected at the applicable product/evidence boundary.
- [ ] A preview-only experiment leaves candidate identity/rank unchanged and
  reports context-coverage changes separately from indexed-hit quality.
- [ ] Real SDK payload size, p95 preview overhead and context utility are measured
  under BENCH-03/04 before default admission.
- [ ] Witness/map/cache budgets cover concurrent in-flight allocations and long
  single documents; preview failure preserves truthful retrieval/window metadata.

Tradeoff: source-bound dynamic context adds source-read and serialization cost;
fixed indexed windows are simpler but can miss useful boundaries. Start with a
bounded deterministic policy, not a learned snippet generator. Source occurrences
and position encodings follow the distinction in [S05](../references.md).
