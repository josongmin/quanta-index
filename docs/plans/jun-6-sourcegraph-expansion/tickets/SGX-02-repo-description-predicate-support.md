# SGX-02 — Repo Description Predicate Support

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Goal

Record that `repo:has.description(...)` is already a supported authority-backed
surface on the current tree and is no longer part of the remaining widening
backlog.

## Current Code Fact

- the surface is executable
- a producer-published repo-description authority exists on the current tree

## Official Sourcegraph Baseline

- Sourcegraph documents `repo:has.description(...)` as a built-in repo
  predicate

## Owner Seam

- shared contract / SDK publish surface if a new authority is introduced
- lexical repo authority evaluation if a new authority lands

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/predicate_registry.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/history.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/sourcegraph_parity.py`

## Concrete Work Items

1. Keep the distinct repo-description authority explicit in docs.
2. Keep runtime/parity/public-api proof attached to the live authority-backed
   implementation.
3. Refuse any future rewrite that folds repo description into repo metadata.

## First Increment

- preserve the distinct description-authority proof
- keep the surface out of reopened backlog lists

## Red Rail To Pin First

```bash
# SGX-02 landed: repo:has.description is now a distinct authority-backed,
# regex-matched surface. The `repo_has_description` prefix runs all five
# proof tests (execute / anchors / multi-repo / invalid-regex fail-closed /
# missing-authority fail-closed / conflicting-batch fail-closed / numeric-arg
# typed-unsupported).
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_filter_execution repo_has_description -- --nocapture
python3 tools/benchmark/sourcegraph_parity.py --check
```

## Worker First Commands

```bash
rg -n "repo:has.description|description authority|RepoMeta|repo description" crates docs tools -S
sed -n '862,890p' crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs
```

## No-Go

- do not search repo source text as a fake “description”
- do not fold the surface back into `repo:has.meta(...)`
- do not reopen this ticket as future backlog unless the live authority or
  proof regresses

## DoD

- a distinct description authority and proof exist
- the reopened backlog excludes this cell

## Not Done If

- description is folded into `repo:has.meta(...)`
- source bytes or repo text are heuristically searched as fallback
- docs still list this cell as future widening
