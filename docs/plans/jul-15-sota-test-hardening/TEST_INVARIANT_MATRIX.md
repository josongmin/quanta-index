# Test Invariant Matrix

Status meanings: `implemented` = committed baseline, `in-flight` = shared
worktree but not yet accepted, `planned` = required before SOTA closure.

| ID | Risk | Owner | Current evidence | Required closure | State |
| --- | --- | --- | --- | --- | --- |
| QIT-00 | P0 | `tools/ci` | target inventory, guard, catalog proof rows | complete P0/P1 universe; executable command/workflow binding; observed execution count | in-flight |
| QIT-01 | P0 | contract/IPC/SDK | v2 contract plus in-flight historical V1 IPC error-frame golden | golden bytes across all public envelopes; prior version decode; unknown/missing/duplicate/malformed fields; canonical re-encode | in-flight |
| QIT-02 | P0 | semantic generation | lifecycle trace/model test | generated Build/Append/Clear/Seal/Activate/QueryPinned/Tombstone/Rollback/Restart/Recover sequences against reference model | in-flight |
| QIT-03 | P0 | semantic storage | atomic sidecar fsync/rename; two promotion subprocess boundaries | actual child termination at every write/fsync/dir-fsync/rename/marker/CAS/root boundary; only old-committed or complete-new allowed | planned |
| QIT-04 | P0 | semantic/searchd | activation CAS race test; TSan workflow | Loom model plus deterministic duplicate/reorder/delay scheduler for activate/query/rollback/restart | planned |
| QIT-05 | P1 | lexical/semantic/hybrid/dispatcher | engine, policy, and smoke tests | exhaustive lexical and ANN reference, pure RRF/filter reference, metamorphic corpus properties | planned |
| QIT-06 | P1 | SDK/daemon | SDK frontdoor plus daemon E2E | SDK-only ingest through recovery matrix; no internal test-only control path | planned |
| QIT-07 | P1 | correctness tooling | 90% changed-line, core mutation, four fuzz targets | P0-owner >=95% changed lines; mutation survivor policy; seeded corpus/repro artifacts; PR/nightly/weekly budgets | planned |
| QIT-08 | P1 | harness | full corpus and report-only DSL benchmark | correctness-gated relevance/latency/RSS/index-size/ingest budgets at medium/large/XL scales | planned |
| QIT-09 | P0 | GitHub Actions | PR, scheduled correctness, manual trigger | explicit PR/merge/nightly/weekly/release tiers, versioned receipts, promotion contract, artifact retention | in-flight |

## Proof-role contract for P0/P1 rows

Every invariant must declare distinct targets for:

- positive behavior;
- negative or fail-closed behavior;
- recovery/restart behavior;
- consumer/front-door behavior.

All roles must be semantically independent. Different file names alone do not
satisfy independence. At least positive and negative proof must be owner-local;
consumer proof must exercise the external contract. The declared PR, merge, and
nightly rails must have a mechanically checked workflow binding and actual
execution receipt.

## Lifecycle reference model contract

State fields: complete generations, sealed set, active generation, tombstones,
pinned query visibility, and pending promotion/recovery state. Commands must
either produce a model-equivalent observable state or a typed failure with no
observable mutation. Metamorphic checks include input permutation, batch
partition, idempotent replay or typed duplicate failure, and rollback
byte-for-byte query recovery.
