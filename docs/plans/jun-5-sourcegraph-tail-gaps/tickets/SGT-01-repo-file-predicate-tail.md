# SGT-01 Repo File Predicate Tail

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `done`

## Goal

Close the remaining repo-file family gaps against current Sourcegraph docs:

- `repo:contains.file(...)`
- `repo:contains.path(...)`
- `repo:has.file(path:... content:...)`

## Current Code Fact

- lexical registry alias exists for `repo.has.path`, not for `repo.contains.file` or `repo.contains.path`
- `parse_repo_file_matchers` admits only:
  - scalar path shorthand
  - `path:`
  - `name:`
  - `lang:`
- nested `content:` matcher seam does not exist on the current repo gate executor

## Official Sourcegraph Baseline

- Sourcegraph docs list `repo:contains.file(...)` as an alias of `repo:has.file(...)`
- Sourcegraph docs list `repo:contains.path(...)` as an alias of `repo:has.path(...)`
- Sourcegraph docs list `repo:has.file(path:... content:...)` as a built-in repo predicate shape

## Owner Seam

- `crates/quanta-index-lexical/src/predicate_registry.rs`
- `crates/quanta-index-lexical/src/lib.rs`
- `crates/quanta-index-lq-bridge/src/translator.rs`

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/predicate_registry.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lq-bridge/src/translator.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`

## First Increment

Pin the current exact behavior of the three cells before any widening:

1. `repo:contains.file(...)`
2. `repo:contains.path(...)`
3. `repo:has.file(path:... content:...)`

## Red Rail To Pin First

- bridge admission rail for alias shapes
- runtime/front-door typed behavior for nested `path+content`

## Worker First Commands

```bash
rg -n "contains\\.file|contains\\.path|parse_repo_file_matchers|RepoFileMatcher|path:.*content:" crates docs tools -S
```

## No-Go

- do not document alias support from Sourcegraph docs alone
- do not treat nested `content:` as a free extension of current repo-file matcher parsing

## DoD

- alias cells are either executable with exact parity or explicit unsupported
- `path+content` is either backed by a real owner seam or closed as explicit unsupported
- no docs-only alias claim remains

## Not Done If

- `repo:contains.file(...)` is documented as supported without bridge/runtime proof
- `path+content` is widened by parser-only sugar onto the current repo gate executor
- `repo:contains.path(...)` is inferred from `repo.has.path` docs without exact alias proof
