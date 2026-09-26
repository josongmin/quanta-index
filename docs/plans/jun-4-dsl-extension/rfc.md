# Jun 4 DSL Extension RFC

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


Status: `landed`
Date: `2026-06-04`
Scope: remove all `부분 지원` cells from the Jun 4 DSL capability inventory without reopening shipped closeout truth

This RFC started from the live inventory in
[../../analysis/jun-4-dsl-capabilty.md](../../analysis/jun-4-dsl-capabilty.md).
It did not relitigate `jun-2-dsl-final-cut`; it converted every partial cell
into one of two honest end states:

- `지원됨`
- `미지원`

Shipped-surface closeout and benchmark authority remain owned by:

- [../jun-2-dsl-final-cut/README.md](../jun-2-dsl-final-cut/README.md)
- [../jun-2-dsl-hardening/README.md](../jun-2-dsl-hardening/README.md)
- [../jun-2-dsl-advanced/README.md](../jun-2-dsl-advanced/README.md)

---

## 1. Final Outcome

Resolved former partial cells:

- `file.has.content(path:..., <scalar>)`
- `file.has.content(file:..., <scalar>)`
- `repo.has.file` matcher combinations
- `repo.contains.content(...)` alias boolean variants
- `symbol.has.name(...)` shared shipped inventory status
- SG structural predicate sibling matrix for shipped repo gate families

Demoted former ambiguous cells to explicit unsupported:

- SG structural direct lexical `Phrase` sibling
- SG structural direct lexical `Regex` sibling
- SG structural mixed non-repo predicate sibling

Current capability inventory has **zero** `부분 지원` rows.

## 2. Source Truth

Primary source truth:

- [../../analysis/jun-4-dsl-capabilty.md](../../analysis/jun-4-dsl-capabilty.md)

Current code owners:

- lexical predicate registry and execution:
  - `crates/quanta-index-lexical/src/predicate_registry.rs`
  - `crates/quanta-index-lexical/src/planner.rs`
  - `crates/quanta-index-lexical/src/lib.rs`
- Sourcegraph bridge and SG structural legality:
  - `crates/quanta-index-lq-bridge/src/translator.rs`
  - `crates/quanta-index-search-plane/src/lowering.rs`
  - `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- proof / inventory rails:
  - `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
  - `crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
  - `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
  - `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
  - `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- Sourcegraph surface guard:
  - `tools/benchmark/sourcegraph_parity.py`

## 3. Ticket Closeout

| ticket | status | final result |
| --- | --- | --- |
| [EXT-00](tickets/EXT-00-scope-lock-and-promotion-bar.md) | landed | partial-cell owner map and promotion bar frozen |
| [EXT-01](tickets/EXT-01-file-has-content-scoped-matrix.md) | landed | `file.has.content(path/file, ...)` exact runtime/front-door/parity green |
| [EXT-02](tickets/EXT-02-repo-has-file-combination-matrix.md) | landed | `repo.has.file` combination matrix exact green |
| [EXT-03](tickets/EXT-03-repo-contains-content-alias-boolean.md) | landed | alias `OR` / `NOT` exact rails green |
| [EXT-04](tickets/EXT-04-symbol-has-name-inventory-verdict.md) | landed | promoted to shared shipped inventory via symbol route |
| [EXT-05](tickets/EXT-05-sg-structural-phrase-regex-verdict.md) | landed | direct lexical sibling surface demoted to explicit unsupported; SG structural phrase/regex remain structural bodies |
| [EXT-06](tickets/EXT-06-sg-structural-predicate-matrix.md) | landed | shipped repo gate families exact green; non-repo predicate siblings explicit typed-fail |
| [EXT-07](tickets/EXT-07-shared-inventory-and-parity-guard.md) | landed | shared front-door inventory widened; parity guard upgraded to canonical surface ids |

## 4. Promotion / Demotion Bar

A cell moved to `지원됨` only when all were true:

1. owner seam executable on live code
2. exact runtime/front-door evidence present for the claimed surface
3. native↔SG parity green for dual-syntax surfaces
4. remaining neighboring unsupported cells explicit

A cell moved to `미지원` when the direct surface was absent or ambiguous and the
route truth was clearer as explicit non-support than as silent mismatch.

## 5. Non-Goals

- no docs-only promotion
- no parse-only promotion
- no bridge-carrier runtime-row promotion
- no reopening of `jun-2-dsl-final-cut` packet status

## 6. Residue

Mandatory residue: none inside this RFC scope.

Open boundaries remain only as intentional unsupported cells, tracked in
[../../analysis/jun-4-dsl-capabilty.md](../../analysis/jun-4-dsl-capabilty.md).

## 7. Remaining Work

There is no mandatory follow-up inside `jun-4-dsl-extension`.

Any future work from this point is a new scope choice, not residue:

- add SG structural direct lexical `Phrase` support
- add SG structural direct lexical `Regex` support
- add SG structural mixed non-repo predicate sibling support
- add new Sourcegraph-style predicate families outside the shipped registry subset

## 8. Ticket Index

- [tickets/INDEX.md](tickets/INDEX.md)
