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
| QIT-02 / semantic | Declared generated/repeat inventories beyond the focused mixed-corpus and SDK histories. The durable-adapter mixed-corpus/kind/owner model, selective Clear and historical pins passed 3/3 and strict Clippy; the public SDK coherent source-deletion/append/replay/CAS/retry/rollback/restart history and independent controls passed 8/8 on 2026-10-07. Independent Chunk/Symbol clear is forbidden for coverage-bound SDK generations; canonical file and semantic-source tombstones already implement deletion. Repeated unsealed append/replacement is also implemented; no new append/clear API is pending | QIT-00 |
| QIT-03 / storage | Broader selected storage/marker/CAS acceptance beyond the existing eight-point daemon matrix. F15 publication/recovered-query/clone-retry checks are implemented through main70521514; the owner reports 36 I/O + 36 SIGKILL cuts passing. [E4-02](../../oct-4-parallel-closure/tickets/INDEX.md#o4-e4-02) records the focused checkpoints; current hosted and Large/XL qualification remain separate. Only prior committed or complete new generation may serve | QIT-02 |
| QIT-04 / semantic/searchd | Declared generated/repeat scopes and native race-detector execution beyond the focused bounded history. The enrolled checker and its fixed positive/negative controls passed 8/8; duplicate CAS, old-event replay, out-of-order publication refusal/exact retry, caller delay, rollback and real daemon restart are combined. Existing race, CAS, delayed-sync and child restart controls remain | QIT-02 |
| QIT-05 / query owners | Complete independent lexical/ANN/fusion/filter and metamorphic corpora at each owning scope; retain exact oracles and detect bad rank/filter mutants. ANN fixture/oracle code is not closure for all routes | QIT-00 |
| QIT-06 / SDK/daemon | SDK-only ingest/query/lifecycle/crash/recovery matrix; exercise public front door rather than internal harness controls | QIT-01, QIT-02, QIT-05 |
| QIT-07 / correctness tooling | Risk-owner quantitative coverage/mutation/fuzz gates, survivor exceptions, seed/minimized-input retention and actual target selection; source guards are not execution | QIT-01, QIT-02, QIT-05 |
| QIT-08 / harness | Correctness-gated independent relevance/latency/RSS/index/ingest/cold-start evidence for medium/large/XL, plus platform limits | QIT-03, QIT-04, QIT-06 |
| [QIT-09](QIT-09-circleci-provider-coverage.md) / tools CI | C6 and following `8642fa9b` regular main jobs/contexts and source-bound receipts are complete (4,241 and 4,268 passed respectively). Collect affected later-source results and separately selected PR/release scopes; reject stale/wrong-scope evidence. Decide former Actions-only coverage. Weekly/release hierarchy, retention and the complete release DAG remain open under [S21-13](../../sep-21-search-plane-sota-hardening/tickets/S21-13-release-evidence-and-sota-qualification.md) | QIT-00, QIT-07, QIT-08 |

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

Current model/concurrency/crash test scope is recorded in
[the ADR coverage boundary](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md#implemented-lifecycle-tests-and-remaining-coverage).
These rows retain missing coverage or execution, not observed production defects.

2026-10-07 source/selector reconciliation retired stale implementation labels.
The subsequent mixed-corpus model extension and SDK history are implemented and
focused execution passed 3/3 and 8/8 respectively. The new history is enrolled
in `test-authority.toml`; remaining generated/repeat/native acceptance is separate
from those completed oracles.

Keep owner-local positive/negative oracles and recovery/consumer proof distinct.
A wire/lifecycle semantic change requires a coordinated current-contract decision;
retired-version compatibility is not a default. New targets update the catalog,
selector and affected closure. Missing host/corpus/service evidence is blocked
for its claim, not a substituted local success.

Use the current Just/scripts/cargow rail and report exact command, covered scope,
exclusions and result. Terminal inventories and oracle output close a row;
compilation, counts, a different platform, lower-scope proof or an old receipt do
not. Semantica ingress and real providers retain their own source/host proof.
