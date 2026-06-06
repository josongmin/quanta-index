# SGX-01 — Repo File Path+Content Support

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Goal

Record that `repo:has.file(path:... content:...)` is already a supported
surface on the current tree and is no longer part of the remaining widening
backlog.

## Current Code Fact

- `repo:has.file(...)` and its alias surfaces are supported
- `path + content` shape is executable
- current repo-file gate correlates path and content on the same document with
  one `BooleanQuery`

## Official Sourcegraph Baseline

- Sourcegraph documents the `repo:has.file(...)` family with richer matcher
  shapes than our current executable subset

## Owner Seam

- `crates/quanta-index-lexical/src/predicate_registry.rs`
- `crates/quanta-index-lexical/src/lib.rs`
- shared runtime/front-door/parity rails

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/predicate_registry.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`

## Concrete Work Items

1. Keep owner-local, runtime, front-door, corpus, and parity proof attached to
   the live path+content implementation.
2. Keep the one-document correlation invariant explicit in docs.
3. Refuse any future rewrite that widens this into a repo-level path/content
   cross-product.

## First Increment

- preserve the current one-document path∧content correlation proof
- keep the surface out of reopened backlog lists

## Red Rail To Pin First

```bash
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_filter_execution repo_has_file_path_content_correlates_per_document_on_sourcegraph_surface -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture
```

## Worker First Commands

```bash
rg -n "parse_repo_file_matchers|RepoFileMatcher|content:" crates/quanta-index-lexical/src crates/quanta-index-searchd-runtime/tests -S
sed -n '300,380p' crates/quanta-index-lexical/src/predicate_registry.rs
sed -n '1468,1495p' crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs
```

## No-Go

- do not regress to path-only gating by silently dropping `content:`
- do not reinterpret the current one-document correlation as a repo-level
  cross-product
- do not reopen this ticket as future backlog unless the live implementation
  regresses

## DoD

- path and content are both enforced on the real owner seam
- no path-only widening
- runtime/front-door/parity/guard/docs all agree

## Not Done If

- parser admits `content:` but executor ignores it
- support is inferred from alias or matcher parsing only
- docs still list this cell as reopened future widening
