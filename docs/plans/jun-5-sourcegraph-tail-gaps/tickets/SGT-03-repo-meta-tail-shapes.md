# SGT-03 Repo Meta Tail Shapes

Parent RFC: [../rfc.md](../rfc.md)

Status: `done`

## Goal

Close the remaining `repo:has.meta(...)` tail cells:

- `repo:has.meta(key)`
- `repo:has.meta(tag:)`
- slash-delimited regex key/value semantics

## Current Code Fact

- current executable subset is `repo:has.meta(key:value)` only
- key-only already has typed-fail rows and parity coverage
- current parser contract is exactly one `key:value` filter arg

## Official Sourcegraph Baseline

- Sourcegraph docs advertise:
  - key/value exact
  - key-only
  - tag/null-value style
  - slash-delimited regex key/value patterns

## Owner Seam

- `crates/quanta-index-lexical/src/predicate_registry.rs`
- `crates/quanta-index-lexical/src/lib.rs`
- `crates/quanta-index-searchd-runtime/tests/*` proof rails

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/predicate_registry.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## First Increment

Freeze the current exact cells first:

1. key-only stays typed-fail
2. `tag:` shape current behavior is explicit
3. regex shape current behavior is explicit

## Red Rail To Pin First

- current key-only typed-fail rail
- new negative rails for `tag:` and slash-delimited regex key/value cells

## Worker First Commands

```bash
rg -n "repo:has\\.meta\\(|parse_repo_meta_arg|RepoMetaArg|LEX_PREDICATE_UNIMPLEMENTED" crates docs tools -S
```

## No-Go

- do not regress exact `key:value` semantics while exploring tail cells
- do not call regex semantics supported before a regex-capable authority seam exists

## DoD

- `key:value` green semantics do not regress
- every non-`key:value` cell has either exact proof or explicit unsupported proof
- regex support is not overstated from exact-string substrate

## Not Done If

- exact `key:value` semantics drift while tail cells are widened
- regex semantics are claimed without a distinct matching substrate
- `tag:` and regex cells remain undocumented as to current exact behavior
