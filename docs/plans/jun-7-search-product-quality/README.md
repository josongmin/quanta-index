# Search product quality — remaining acceptance

Status: `ACTIVE_RESIDUAL`

Quality producers, typed operator/repair/highlight contracts and aggregate
registration exist in current source. Their decisions are consolidated in
[JUN-08-001](../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md)
and [the code-search ADR](../../adr/SEP-27-003-code-search-source-and-preview-contract.md).
The historical J7Q scope/implementation packet is retired; removal does not
qualify product quality or assert that a current Rust rail passed.

Remaining relevance, snippet/explain, scale and tail acceptance lives in
[the ticket index](tickets-wave2/INDEX.md). Additional consumer/operational
acceptance is retained there. [Benchmark usage](../../../tools/benchmark/README.md)
and [searchctl usage](../../../crates/quanta-index-searchctl/README.md) own commands.

External comparisons stay restricted to overlapping shipped lexical surfaces;
semantic/hybrid quality has its own retrieval owner. Current independent-gold,
corpus and comparator admission is in the
[code-search benchmark ledger](../sep-27-code-search-remediation/readme.md).
Historical bodies are recoverable through [the plan archive](../ARCHIVE-INDEX.md).

## Gin 300-query diagnostic boundary (2026-09-29)

The frozen gin bare-symbol semantic-only run is a diagnostic of one 99-file
corpus, not an admitted product-quality score: Quanta V1 found the generated
gold file in 266/300 top-10 responses and Semble in 277/300. An isolated V2
encoder control found 268/300, with 13 recovered and 11 newly missed queries.
Quanta's indexed chunks contained all 46 union-miss gold definitions; Semble's
indexed chunks contained all 46 gold identifiers. The Quanta run used
exact-vector search, so that run does not establish an ANN recall defect. The
generated gold has no independent relevance review, and bare names can refer
to other declarations or uses.

At local `main@938251d2`, freshly built runner and searchd binaries repeated
the original bare-symbol 300-query diagnostic on new states: V1 found the gold
file in 266/300 top-10 responses and opt-in V2 in 268/300. Both ordered top-10
outputs matched the earlier source-bound native controls for all 300 queries.
All 600 responses completed with `capped` status. These are generated,
unreviewed file labels and a contended, single-run diagnostic, not an admitted
quality or speed result. Source, binary, input and output boundaries are in
`/Users/songmin/Documents/code-new/qi-gin-quality-current-20260929-938251d2/RESULTS.md`.

The same current binaries also found the mechanically identified exact
definition span for 300/300 names through the distinct exact-symbol route
(297 at rank 1, three at rank 2). That result tests declaration lookup, not
semantic natural-language relevance. Path-limited inspection found no semantic
encoder/search/route change since `cca9477f`; the new native captures now
confirm the observed gin rank prefix at `938251d2`. They do not qualify a
default V2 decision.

Remaining acceptance is an independently authored and reviewed semantic-intent
pool with a frozen holdout, source-bound file/declaration judgments, separate
route-unit metrics, and a predeclared quality/resource decision. V2 also needs
quiet-host repeated latency and peak-memory evidence; larger-corpus ANN quality
requires its own control. The [retrieval benchmark guide](../../../tools/benchmark/retrieval/README.md)
describes the distinct exact-symbol, file-ranked lexical and semantic profiles.
