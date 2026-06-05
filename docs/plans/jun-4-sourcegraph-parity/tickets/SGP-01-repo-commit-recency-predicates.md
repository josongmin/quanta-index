# SGP-01 Repo Commit Recency Predicates

Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

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
- quanta-index now owns an explicit repo-recency authority seam:
  - `crates/quanta-index-contract/src/ipc/ingest.rs`
  - `RepoCommitRecencyIngestBatch`
  - `crates/quanta-index-sdk/src/history.rs`
  - `RepoCommitRecencyBatch`
  - `crates/quanta-index-lexical/src/lib.rs`
  - lexical repo-gate execution against source-repo keyed recency sidecar
- current front-door proof exists:
  - `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
  - `crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
  - `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- shared dual-syntax parity row now exists:
  - `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- external producer ingress now auto-emits repo commit recency alongside history publish:
  - `semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/index_sdk_ingress/publish.rs`
  - `semantica-codegraph-v2/.../tests/index_sdk_ingress_publish_contract_test.rs`
- canonical producer proof is now green on the live ingress rail:
  - `index_sdk_ingress_live_repo_commit_recency_publish_and_query_roundtrip_v1`
  - producer history publish emits repo commit recency and the downstream Sourcegraph text query executes against it

## Files To Touch

- `crates/quanta-index-contract/src/ipc/ingest.rs`
- `crates/quanta-index-core/src/domains/lexical/outbound.rs`
- `crates/quanta-index-core/src/timeref.rs`
- `crates/quanta-index-lexical/src/lib.rs`
- `crates/quanta-index-lexical/src/predicate_registry.rs`
- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-sdk/src/history.rs`
- `crates/quanta-index-lexical/src/planner.rs`
- producer-side history ingest/materialization owner in `semantica-codegraph-v2`
- `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## Concrete First Increment

Current implemented increment:

1. canonical `repo:has.commit.after(...)`
2. alias `repo:contains.commit.after(...)`
3. source-repo keyed repo-recency authority batch
4. SG/native parity row
5. producer ingress auto-emission from history publish
6. canonical producer live ingress roundtrip proof

## Implementation Steps

1. land contract/sdk/runtime repo-recency authority seam
2. define repo-gate semantics over that authority
3. wire canonical + alias Sourcegraph surfaces onto timeref parsing
4. add positive/miss/invalid oracle rows across multiple repos
5. add shared SDK/front-door inventory
6. hook producer emission in `semantica-codegraph-v2`
7. prove the producer owner seam on canonical ingress rail

## Red Rail First

- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_filter_execution -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## DoD

- canonical and alias shape both execute
- miss oracle fails if repo-recency gating is ignored
- logical external repo mapping is explicit in the execution owner
- producer emits the authority batch in the real cross-repo path
- shared dual-syntax parity row exists
- canonical producer ingress proof is green on the real live roundtrip rail

## Not Done If

- only parser admission exists
- alias lowers but canonical proof is missing
- implementation fakes repo recency through per-file text matches instead of repo-level authority
- the ticket treats `predicate_registry.rs` as the primary execution owner and bypasses history shard authority
- implementation reads history only at pinned repo/revision scope without a logical external repo mapping and still claims Sourcegraph repo-gate parity
