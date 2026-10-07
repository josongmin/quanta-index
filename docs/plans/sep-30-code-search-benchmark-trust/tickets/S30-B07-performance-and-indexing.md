# S30-B07 — Equal-boundary performance and indexing acceptance

Status: `ACTIVE_RESIDUAL`; qualified performance `NOT_RUN`.
Parent: [benchmark plan](../README.md). Contract:
[CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md).
Implemented capture/clock/batch mechanisms are in
[OCT-05-002](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md);
cost, capacity and optimization stopping rules are in
[OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).

## Remaining work and ownership

| Owner | Work | Acceptance / stop condition |
| --- | --- | --- |
| E4-06 / benchmark | Admit a quiet supported host with continuous frequency, thermal, power and load observations; execute randomized paired repetitions | Missing observations remain unavailable. Apply at least five fresh roots and 1,000 route-local warm observations under the selected protocol. Busy-host diagnostics cannot acquire `PERF_QUALIFIED`. |
| E4-03 / search | Decide keep/modify/withdraw for the ASCII scanner from repeated whole-request comparisons | Independent byte/token/OSA1 parity, IDs, scores, spans, totals, cursor pages, budgets and cancellation must hold. Observation-on/off is a separate experiment; backend clocks still execute in both observation arms. |
| E4-04 / search | Consider token authority only if repeated source-token scanning dominates the complete caller | Preserve exhaustive short-name/Unicode fallback and generation/digest authority; include index-build and residency costs. No bottleneck means no new persistent index. |
| E4-02 / indexing | Attribute full/no-op/update/delete/open costs, including both preflights, coverage build, hashing and durable publication | Independent fresh-rebuild equality, inherited-file custody and corruption/crash/reopen controls. Sync batching requires isolated sync cost; full tree/base verification must not disappear to improve timing. |
| E3 / SDK/runtime | Use request-local SDK/IPC and daemon events to locate a repeated complete-call bottleneck | Join exact request/connection identities with no dropped events. Independent client/server subtraction and idle sleep do not measure transport. Connection reuse or fusion needs causal evidence plus generation, deadline, credential, cap and shutdown parity. |
| E4-05 / scale/load | Repair the frozen5796 large default timeout and XL posting admission failure, then execute matching-release offered-load/OS-process restart rails | [J7Q-03](../../jun-7-search-product-quality/tickets-wave2/J7Q-03-large-corpus-scale-tiers.md) and [J7Q-04](../../jun-7-search-product-quality/tickets-wave2/J7Q-04-latency-tail-hardening.md) retain resource/lifecycle and route-tail acceptance. An explicit timeout/retention diagnostic does not close default capacity. A later affected source epoch needs matching proof; quiet-host repetitions are separate. Synthetic source repositories under one owner do not prove multi-owner CAS concurrency. |
| E2-04 / capture, E1-06 / scoring | Complete the declared product × repository × lane × mode inventory and independent joins | [OCT-04 ledger](../../oct-4-parallel-closure/tickets/INDEX.md) owns current cells. Exact, prefix, infix, components, four typo edits, no-answer, NL and ARB are separate cohorts. Repeated Gin queries do not enlarge its source corpus. |

Further candidate intersection, bounded top-k, preview copying, shard streaming
or writer-policy changes require a repeatedly dominant measured stage. Preserve
late winners, ties, all continuation pages, complete match counts and truthful
budget errors. Existing preview-after-page, touched shards, hard links and base
commitment reuse are implemented mechanisms; do not rebuild them.

## Measurement contract

- Prove correctness before timing. Bind actual source, compiler/profile, binary,
  package/model assets, input, index universe, effective request and topology.
  SDK/IPC and an in-process BM25 call remain different profiles.
- Time request construction through complete decoded required output. Preserve
  time-to-first-result, internal stages, process startup and later persistence
  under their actual boundaries. A fresh directory is not a cold OS cache.
- Report per-query distributions/p50/p95, output bytes/native units and every
  attempted/completed/error/timeout/partial outcome; no survivor-only latency.
  Small case series, including the 20-query Semble set, remain descriptive.
- Indexing reports separate discovery/preflight/chunk/model/boot/publish/
  activate/query boundaries, source files/bytes and indexed units. SDK publish
  contains server build; seal contains commit/merge/commitment. Nested children
  cannot be added again to the enclosing duration; unavailable residuals stay
  unavailable. Use the actual current phase labels rather than old schema names.
- Record CPU, sampled process-tree RSS, logical/transient disk and physical I/O
  only from their respective observers. Different chunk counts, remote indexes
  and unattested source scope do not share an indexing denominator.
- Prespecify paired seed/order, cold/warm/warmup policy, repetition floors,
  decision and uncertainty. Preserve each arm's actual schedule and full output.
  Inconclusive improvement or correctness drift prevents optimization promotion.

## Execution and history

Use the [retrieval guide](../../../../tools/benchmark/retrieval/README.md),
existing `run.py`, `query_timing_overhead.py`, canonical `host_monitor.py` and
registered scale/tail/open-loop producers. Heavy runs share one admitted host
slot. Extend an owner only for a demonstrated missing boundary; no second harness.

F14 frozen5796 fresh SDK27 and Contract791/191 have completed actual and portable
verification. A separate matching release `scale_matrix` build and small16/medium256
causal runs are scoped diagnostics under [E4-05](../../oct-4-parallel-closure/tickets/INDEX.md#o4-e4-05).
Frozen5796 default large failed at the 30s build/seal client timeout; XL refused
before daemon startup at the first exceeded 4M posting count. An independent
large300s/256MiB diagnostic completed in127.612s, with full seal68.733s and
explicit sync49.778s. Source file/parent barriers dominate that full envelope.
Delta/noop/delete seal12.711/14.733/14.393s have only0.265/0.153/0.205s explicit
sync; their exclusive residual stages remain unmeasured. Current F15 source
reuses unchanged committed bucket objects and publishes durable pack/root
authority. Cold independently verifies the complete census, retains bounded
term/range/hash directories, and queries read selected posting ranges under one
work budget. The logical resident admission is not measured RSS. Owner checks
are in progress; old-or-complete-new crash custody and matching Large/XL/default
retention still require actual execution. Exact commands, roots and scope are in
[E4-01/02/05](../../oct-4-parallel-closure/tickets/INDEX.md#o4-e4-05).
These diagnostics do not qualify later source epochs, OS-child restart,
scanner A/B or quiet-host performance. A separate current78d2474 Medium256
OS-child stop/reap/restart owner regression passed (1 selected/82 unselected,
31.805s); Large/XL restart and resource qualification remain `NOT_RUN`.
Scanner source/build/capture custody owner tests passed49 selected cases;
the actual two-arm build/capture and whole-call decision remain `NOT_RUN`.
Exact old commands/results are recoverable through
[the history index](../../../ARCHIVE-INDEX.md#historical-record-recovery).
