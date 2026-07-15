# QIT Status Board

Snapshot: live shared worktree on top of committed `b3140f8`. `In-flight` work
must not be treated as merged evidence until its required rails and a commit
exist.

| Ticket | Status | Owner | Dependencies | Done only when |
| --- | --- | --- | --- | --- |
| QIT-00 Test authority | in-flight | tools/ci | none | universe and target inventory are complete; CI binding and non-zero execution receipt are guard-enforced |
| QIT-01 Wire/version | in-flight | contract, IPC, SDK | QIT-00 | compatibility/negative/canonicalization matrix passes from SDK consumer |
| QIT-02 Lifecycle model | in-flight | semantic | QIT-00 | generated state machine compares observable engine state with pure model under fixed and expanded seed rails |
| QIT-03 Crash matrix | planned | semantic/storage | QIT-02 | every durability boundary child-kill produces prior committed or complete new generation, never mixed state |
| QIT-04 Concurrency | planned | semantic/searchd | QIT-02 | Loom owner model plus deterministic scheduler and TSan broad evidence cover stated linearization histories |
| QIT-05 Query oracle | planned | lexical/semantic/hybrid | QIT-00 | independent reference and metamorphic suites prove ranking/filter contracts |
| QIT-06 SDK black-box | planned | SDK/daemon | QIT-01,QIT-02,QIT-05 | SDK-only lifecycle/recovery matrix passes without internal control hooks |
| QIT-07 Quant gates | planned | correctness tooling | QIT-01,QIT-02,QIT-05 | thresholds, seed/artifact retention, mutation survivor policy, and target selection are blocking where stated |
| QIT-08 Quality/scale | planned | harness | QIT-03..QIT-06 | correctness-green workloads meet versioned relevance/latency/RSS/index/ingest budgets |
| QIT-09 Promotion/receipts | in-flight | Actions/tools/ci | QIT-00,QIT-07 | PR/merge/nightly/weekly tiers emit schema-validated receipts and promotion rejects missing/stale evidence |

External boundary: Semantica ingress and live OpenAI/provider evaluation retain
their own repository/platform receipts. They are not QIT completion evidence.
