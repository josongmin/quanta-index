# Test hardening — residual board

Status: `ACTIVE_RESIDUAL`
Contract: [SEP-27-005](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md)

Implemented registration/receipt/wire/model foundations are not pending API
work. Each row below retains only acceptance absent from this documentation
cleanup; source presence or historical tests are not current qualification.

| ID / owner | Remaining acceptance | Prerequisite |
| --- | --- | --- |
| QIT-00 / tools CI | Revalidate completeness of the declared P0/P1 universe and semantic independence of positive/negative/recovery/consumer roles; actual current selected/executed nonzero inventories at each required tier | Current catalog and workflow binding |
| QIT-01 / contract, IPC, SDK | Complete current public-envelope golden/negative/canonical re-encode and consumer matrix, including unknown/missing/duplicate/malformed fields and intentional legacy refusal; current storage/wire reproduction boundaries | QIT-00 |
| QIT-02 / semantic | Expand generated lifecycle oracle beyond current adapter traces; compare Build/Append/Clear/Seal/Select/QueryPinned/Tombstone/Rollback/Restart/Recover observations. Adapter generation selection does not prove process-wide activation CAS | QIT-00 |
| QIT-03 / storage | Real child-kill matrix at every relevant write/fsync/dir-fsync/rename/marker/CAS/root boundary; only prior committed or complete new generation, never mixed state | QIT-02 |
| QIT-04 / semantic/searchd | Owner-local linearization model and deterministic duplicate/reorder/delay schedules for activate/query/rollback/restart; required native race-detector evidence. Broad TSan alone does not prove linearizability | QIT-02 |
| QIT-05 / query owners | Complete independent lexical/ANN/fusion/filter and metamorphic corpora at each owning scope; retain exact oracles and detect bad rank/filter mutants. ANN fixture/oracle code is not closure for all routes | QIT-00 |
| QIT-06 / SDK/daemon | SDK-only ingest/query/lifecycle/crash/recovery matrix; exercise public front door rather than internal harness controls | QIT-01, QIT-02, QIT-05 |
| QIT-07 / correctness tooling | Risk-owner quantitative coverage/mutation/fuzz gates, survivor exceptions, seed/minimized-input retention and actual target selection; source guards are not execution | QIT-01, QIT-02, QIT-05 |
| QIT-08 / harness | Correctness-gated independent relevance/latency/RSS/index/ingest/cold-start evidence for medium/large/XL, plus platform limits | QIT-03, QIT-04, QIT-06 |
| [QIT-09](QIT-09-circleci-provider-coverage.md) / tools CI | Obtain passing final-source CircleCI regular jobs, terminal GitHub status and exact-commit PR/main receipts and reject missing/stale/wrong-scope evidence. Decide former Actions-only coverage. Weekly/release hierarchy, retention and the complete release DAG remain open under [S21-13](../../sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md) | QIT-00, QIT-07, QIT-08 |

## Unadmitted quantitative targets retained from the old plan

These are proposed acceptance targets, not claims about implemented gates:

- PR ~15 min; merge ~30–40 min; nightly bounded per artifact; weekly/release
  expanded corpus/resource/recovery work.
- P0 owner changed-line coverage >=95%; production Rust aggregate >=90%.
- Critical mutation >=85%, other selected targets >=75%; no unexplained P0
  semantic survivors. Exceptions need an owner and expiry.
- Property PR floor 256 cases; nightly lifecycle 100,000 transitions.
- Fuzz changed-target PR 30–60 s, nightly 15 min, weekly 2 h; retain seed,
  minimized input, command, toolchain and owner for every crash.
- Nightly critical deterministic repeats 100; daemon E2E repeats 20.

Review the current owners and measurement cost before enforcing a target; no
unused threshold or configured duration is passing evidence.

## Execution and closure

Keep owner-local positive/negative oracles and recovery/consumer proof distinct.
A wire/lifecycle semantic change requires a coordinated current-contract decision;
retired-version compatibility is not a default. New targets update the catalog,
selector and affected closure. Missing host/corpus/service evidence is blocked
for its claim, not a substituted local success.

Use the current Just/scripts/cargow rail and report exact command, covered scope,
exclusions and result. Terminal inventories and oracle output close a row;
compilation, counts, a different platform, lower-scope proof or an old receipt do
not. Semantica ingress and real providers retain their own source/host proof.
