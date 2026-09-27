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
