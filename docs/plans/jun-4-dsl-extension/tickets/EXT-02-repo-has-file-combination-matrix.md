# EXT-02 Repo Has File Combination Matrix

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Landed Result

- `path+name`
- `path+lang`
- `name+lang`
- `path+name+lang`

조합 matrix가 exact runtime/parity row로 닫혔다.

## Objective

Close the combinatorial proof gap for:

- `repo.has.file(path:..., name:...)`
- `repo.has.file(path:..., lang:...)`
- `repo.has.file(name:..., lang:...)`
- `repo.has.file(path:..., name:..., lang:...)`

## Current Source Truth

- owner seam:
  - `crates/quanta-index-lexical/src/predicate_registry.rs`
  - `crates/quanta-index-lexical/src/lib.rs`
  - `crates/quanta-index-lq-bridge/src/syntax.rs`
  - `crates/quanta-index-lq-bridge/src/translator.rs`
- current proof:
  - singleton `path`, `name`, `lang`, scalar-path shorthand are covered
  - combination matrix is not

## Files To Touch

- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- optionally `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Concrete First Increment

Start with `path+name`.

Do not add all combinations on a weak fixture. First build a fixture where:

- `path` alone overmatches
- `name` alone overmatches
- only `path+name` yields the target repo set

## Implementation Steps

1. create non-vacuous docs/repos for each combination
2. add runtime rows per combination
3. add SG/native parity rows per combination
4. optionally add one front-door scenario after runtime/parity are green

## Red Rail First

- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## DoD

- each combination has a distinct oracle
- a no-op or singleton-only implementation would fail

## Not Done If

- the fixture lets a combination masquerade as a singleton row
