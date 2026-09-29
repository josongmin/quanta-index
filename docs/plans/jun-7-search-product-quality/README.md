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

The separate exact-symbol route found the mechanically identified definition
span for 300/300 names on an older clean source. That result tests declaration
lookup, not semantic natural-language relevance. At local `main@9c880476`,
focused searcher and dispatcher tests cover case-sensitive exact names with a
typed file anchor; a current-HEAD native 300-query capture has not been run.
Path-limited inspection found no semantic encoder/search/route change since
the earlier `cca9477f` V1/V2 control, but that inspection does not substitute
for a latest-binary capture or a qualified default decision.

Remaining acceptance is an independently authored and reviewed semantic-intent
pool with a frozen holdout, source-bound file/declaration judgments, separate
route-unit metrics, and a predeclared quality/resource decision. V2 also needs
quiet-host repeated latency and peak-memory evidence; larger-corpus ANN quality
requires its own control. The [retrieval benchmark guide](../../../tools/benchmark/retrieval/README.md)
describes the distinct exact-symbol, file-ranked lexical and semantic profiles.
