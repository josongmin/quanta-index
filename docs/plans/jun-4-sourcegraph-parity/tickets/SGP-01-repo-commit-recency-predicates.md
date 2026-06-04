# SGP-01 Repo Commit Recency Predicates

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Objective

Implement:

- `repo:has.commit.after(...)`
- `repo:contains.commit.after(...)`

as executable Sourcegraph predicate surfaces.

## Current Source Truth

- native parser already recognizes `repo.contains.commit.after(...)` shape:
  - `crates/quanta-index-lq-norm/src/parser/implementation.rs`
- Sourcegraph bridge can already lower unknown `repo:` predicate names into canonical predicate leaves:
  - `crates/quanta-index-lq-bridge/src/translator.rs`
- this is not a Tantivy lexical-predicate owner seam:
  - the predicate needs repo-level history/recency authority, not per-file text matching
- history shard substrate already exists:
  - `crates/quanta-index-search-plane/src/query_dispatcher.rs`
  - `validate_history_timeref_filters(...)`
  - `parse_history_timeref_ms(...)`
  - `resolve_history_since_timeref_ms(...)`
  - `CommitRecord::committer_time_ms`
- but the current history authority is keyed only by `(repo_id, revision_id, generation)` and stores commits/refs/tags/diff hunks without a logical external repo dimension
  - `HistoryAuthorityState` has no `source_repo_id`-keyed repo-recency index
  - current executable repo-gate predicates (`repo.has.file`, `repo.has.content`) narrow by lexical `source_repo_id`/`repo_id`, which history state cannot currently reproduce
- therefore the primary missing seam is **logical external repo keyed history authority**, not predicate admission
- no bridge/runtime/front-door/shared proof exists

## Files To Touch

- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-search-plane/src/readiness.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-contract/src/lex/history.rs`
- `crates/quanta-index-sdk/src/history.rs`
- producer-side history ingest/materialization owner if new logical repo keyed history state is required
- `crates/quanta-index-lexical/src/planner.rs` only if native predicate admission still needs planner-level allowlisting
- `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## Concrete First Increment

Support exactly one canonical scalar time shape first, backed by the existing history timeref substrate **after** logical external repo keyed history authority exists:

1. `repo:has.commit.after(2024-01-01T00:00:00Z)`
2. alias parity: `repo:contains.commit.after(2024-01-01T00:00:00Z)`

Do not start with natural-language timeref.

## Implementation Steps

1. add or expose logical external repo keyed commit-recency authority
2. define repo-gate semantics over that authority
3. wire one canonical timestamp shape onto existing `committer_time_ms` / timeref parsing
4. add positive and miss oracle rows across multiple repos
5. add SG/native parity
6. add alias parity

## Red Rail First

- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_filter_execution -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## DoD

- canonical and alias shape both execute
- miss oracle fails if repo-recency gating is ignored
- logical external repo mapping is explicit in the execution owner

## Not Done If

- only parser admission exists
- alias lowers but canonical proof is missing
- implementation fakes repo recency through per-file text matches instead of repo-level authority
- the ticket treats `predicate_registry.rs` as the primary execution owner and bypasses history shard authority
- implementation reads history only at pinned repo/revision scope without a logical external repo mapping and still claims Sourcegraph repo-gate parity
