# `expect` reachability

Audit scope: the current Rust worktree on 2026-10-03. Counts below are
**matching source lines** from `rg -n '\.expect\('`, not call-expression counts.
The worktree includes concurrent source edits, so remeasure after integration.
A raw match count includes test modules, integration tests and benchmark
harnesses; it does not measure process-serving panic risk.

| Apparent hotspot | Source classification | Production action |
| --- | --- | --- |
| `quanta-index-lexical/src/searcher/code_search.rs`: 93 lines | All in its `#[cfg(test)] mod tests`. | None from this count. |
| `quanta-index-embed/src/cache.rs`: 92 lines | All in its `#[cfg(test)] mod tests`. | None from this count. |
| `quanta-index-searchd/src/app/config.rs`: 75 lines | All in its `#[cfg(test)] mod tests`. | None. |
| `quanta-index-core/src/domains/generation.rs`: 75 lines | All in three `#[cfg(test)]` modules. | None. |
| `quanta-index-contract/src/ipc/split.rs`: 65 lines | All in its `#[cfg(test)] mod tests`. | None. |
| `quanta-index-search-plane/src/control_dispatcher.rs`: 63 lines | All in its `#[cfg(test)] mod tests`; an earlier `#[cfg(test)]` applies only to one constant. | None. |
| `quanta-index-embed/src/openai.rs`: 50 lines | All in its `#[cfg(test)] mod tests`; earlier `#[cfg(test)]` attributes apply only to two helpers. | None. |
| `quanta-index-lexical/src/file_authority.rs`: 46 lines | All in its `#[cfg(test)] mod tests`. | None. |
| `quanta-index-embed/src/model2vec/parity_fixture.rs`: 46 lines | The parent `model2vec.rs` includes this module only under `#[cfg(test)]`. | None. |
| `quanta-index-contract/src/results/query_responses.rs`: 45 lines | All in its `#[cfg(test)] mod tests`. | None. |
| `quanta-index-semantic/src/lib.rs`: 44 lines | All in a `#[cfg(test)]` module. | None. |
| `quanta-index-semantic/src/ann_proof.rs`: 41 lines | All in its `#[cfg(test)] mod tests`; the proof CLI's non-test path has none. | None. |
| `quanta-index-contract/src/ipc/control.rs`: 37 lines | All in `#[cfg(test)]` contract modules. | None. |
| `quanta-index-ipc/src/codec.rs`: 36 lines | All in its `#[cfg(test)] mod tests`. | None. |
| `quanta-index-embed/src/model2vec.rs`: 33 lines | All in its `#[cfg(test)] mod tests`. | None. |
| `quanta-index-lexical/src/ranked_page_tests.rs`: 2 lines | Included only under `#[cfg(test)]` in `ranked_page.rs`. | None. |
| `quanta-index-searchd-harness/src/harness.rs` and `src/ann.rs`: 4 lines before their test modules | Static fixture identifiers in a package consumed by the runtime as a dev dependency and by the scan experiment. | Test and experiment reliability, not daemon process serving. |

## Call-site check and limits

A read-only Rust tree-sitter AST pass examined `.expect()` call expressions
under `crates/*/src/**/*.rs`, excluding `/tests/` paths and `tests.rs` files.
Outside local `#[cfg(test)]` or `#[test]` scopes, it found exactly four calls:
`searchd-harness/src/harness.rs` at lines 875 and 883, and
`searchd-harness/src/ann.rs` at lines 182 and 190. All four construct fixed
fixture IDs in a harness absent from the normal daemon dependency graph.
For source files included as separate modules, the containing file's
attributes must also be checked: for example,
`model2vec/parity_fixture.rs` and `ranked_page_tests.rs` are included only by
`#[cfg(test)]` parent modules. A per-file AST pass alone cannot prove that
parent relationship.

The high-count files above do not contain a confirmed daemon request-path
`.expect()` failure. The current `ipc/server.rs`, `ipc/server/peer_watch.rs`,
and `searchctl/src/{lib,parse,render}.rs` contain zero `.expect(` calls after
their module splits. This audit does not cover `.unwrap()`, `panic!`, indexing,
code outside `crates/*/src`, or runtime failure modes unrelated to `expect`.
Before changing any remaining occurrence, trace its caller, whether the
containing module is compiled in production, the invalid input that reaches
it, and the typed error or refusal that should replace a panic. Test fixture
assertions can remain assertions.

Recheck after source changes. Search with
`rg -n '\.expect\(' crates --glob '*.rs'`, group by path, then inspect
`#[cfg(test)]` **module boundaries** and the dependency profile before
treating any result as an operational hotspot. A local `#[cfg(test)]` on one
item does not make the following production code test-only.
