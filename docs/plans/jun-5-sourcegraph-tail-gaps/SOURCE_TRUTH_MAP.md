# SOURCE_TRUTH_MAP

Use these anchors before changing any ticket.

## Official Baseline

- Sourcegraph query language reference:
  - [https://sourcegraph.com/docs/code-search/queries/language](https://sourcegraph.com/docs/code-search/queries/language)
- Sourcegraph query syntax:
  - [https://sourcegraph.com/docs/code-search/queries](https://sourcegraph.com/docs/code-search/queries)

## Current Capability Truth

- analysis inventory:
  - [jun-4-dsl-capabilty.md](../../analysis/jun-4-dsl-capabilty.md)
- parity report:
  - [../../../tools/benchmark/SOURCEGRAPH_PARITY.md](../../../tools/benchmark/SOURCEGRAPH_PARITY.md)
- parity guard:
  - [../../../tools/benchmark/sourcegraph_parity.py](../../../tools/benchmark/sourcegraph_parity.py)

## Repo-File Family

- predicate registry:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/predicate_registry.rs`
- lexical executor:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- bridge translator:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lq-bridge/src/translator.rs`

## Repo-Meta / Topic / Contributor / Owner Surfaces

- predicate registry:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/predicate_registry.rs`
- lexical authority evaluation:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- authority ports:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-core/src/domains/lexical/outbound.rs`
- SDK history publish surface:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/history.rs`

## Structural Route

- lowering owner seam:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/lowering.rs`
- runtime dispatch context:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`

## Shared Proof Rails

- runtime corpus:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- front-door scenarios:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
- filter execution:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- SG/native parity:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
