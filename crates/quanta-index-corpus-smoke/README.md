# LQ corpus smoke runner

Loads TOML conformance cases, runs them against normalizer/executor traits,
and emits JUnit. Its mocks and runner test the corpus contract; they do not
start or qualify the search daemon.

Start with the [corpus loader](src/corpus/loader.rs), [runner](src/runner/core.rs),
and [CLI](src/bin/corpus_smoke.rs). The DSL contract is in the
[JUN-02-001 ADR](../../docs/adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md).
