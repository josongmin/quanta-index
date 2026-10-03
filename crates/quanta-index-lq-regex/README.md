# Regex executor

Validates the supported regex dialect, estimates execution cost, extracts
literals for prefiltering, and executes bounded matches. It is not a
standalone query service.

Start with [dialect validation](src/dialect.rs), [executor](src/executor.rs),
[estimator](src/estimator.rs), and [literal extraction](src/literal_extract.rs).
The LQ regex contract is in the [JUN-02-001 ADR](../../docs/adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md).
