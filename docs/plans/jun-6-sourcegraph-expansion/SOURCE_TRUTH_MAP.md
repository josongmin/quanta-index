# SOURCE_TRUTH_MAP

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


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

## Text-Route Closeout

- predicate registry:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/predicate_registry.rs`
- lexical evaluator:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- lexical service boundary:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-core/src/domains/lexical/service.rs`
- lexical outbound authority ports:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-core/src/domains/lexical/outbound.rs`

Current live text-route truth to verify before reopening any ticket:

- `repo:has.file(path:... content:...)` is already executable on the current tree
- `repo:has.description(...)` is already executable on the current tree
- `repo:has.meta(key)` and `repo:has.meta(tag:)` are already executable on the current tree
- slash-delimited regex key-only, mixed exact/regex key-value, and regex pair
  shapes are also executable on the current tree
- `file:has.contributor(<name-or-email regex>)` is executable on the current tree
- remaining text-route widening cells are: 없음
- final packet closeout residue: 없음
- final external semantica producer proof residue: 없음
- verification follow-on moved to
  `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-7-verification-hellgates/rfc.md`

## Structural Route Closeout

- lowering owner seam:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/lowering.rs`
- dispatch / candidate execution seam:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`
- SG parser / bridge:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lq-bridge/src/syntax.rs`
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lq-bridge/src/translator.rs`

Current live structural truth to verify before reopening any ticket:

- direct SG structural lexical `Phrase` / `Regex` siblings are permanently
  explicit unsupported on the current tree
  - `patterntype:structural "foo bar"` lowers the quoted token as structural
    body syntax
  - `patterntype:structural /foo.*/` lowers the slash token as structural regex
    body syntax
  - the demotion witness is a grammar/lowering fact, not a runtime typed-fail
    fact, because no distinct SG query surface exists for those direct lexical
    siblings
- SG structural mixed non-repo predicate siblings are already executable on the
  current tree for:
  - `file.contains(path|file:...)`
  - `file.has.content(path|file:...)`
  - `symbol.has.name(...)`
    - projects same-path, line-overlapping symbol hits into all matching
      structural chunks with deterministic union
- remaining structural backlog: 없음

## Shared Proof Rails

- runtime corpus:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- front-door scenarios:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
- filter execution:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- SG/native parity:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- full corpus:
  - `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
