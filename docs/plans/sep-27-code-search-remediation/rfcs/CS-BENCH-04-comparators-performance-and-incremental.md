# CS-BENCH-04 — Comparator, performance and incremental acceptance

Status: `ACTIVE_RESIDUAL` for equal-work/resource and incremental qualification.
Native capture/index/clock and optimization contracts are owned by
[OCT-05-002](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md)
and [OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).
Current native cells are owned by [E2](../../oct-4-parallel-closure/tickets/INDEX.md#e2);
host/cost/capacity decisions by [E4](../../oct-4-parallel-closure/tickets/INDEX.md#e4).
Independent corpus/gold/unit acceptance remains BENCH-01/02/03.

## Profiles and indexed scope

| Product/reference | Required boundary |
| --- | --- |
| Quanta | Actual SDK/daemon, lexical/definition/grouped-file policies and source-bound units |
| Sourcegraph local | Pinned server/indexer/runtime, effective request, index readiness and actual source inventory |
| OpenGrok local | Pinned runtime/indexer/config, native field/order and the selected actual reader/source witness |
| cs local | Binary/flags, full indexed universe and process lifecycle |
| Semble local | Lock/model/config, actual native hybrid or supported lexical mode and execution work |
| ripgrep scan | Manifest/options and exact-match/scan-cost reference; no relevance authority |

Optional direct Zoekt isolation remains a separate engine profile. GitHub CLI
cannot stand in for Blackbird's backend; generic document BM25 cannot replace
a native code-search comparator. Bind endpoint, worker topology, VM/container
overhead, CPU/memory envelope, model assets and indexing configuration.

Disk/served-file observations and selected acquired-reader witnesses retain
their narrow OCT-05-002 scope. Before/after equality cannot rule out a mutation
reversed between probes or establish all postings/source bytes. The existing
OpenGrok full API view remains bounded to 4,096 files, 512 MiB and 900 seconds;
its source/probe limits do not qualify other scope modes. Missing indexed-universe
proof limits only its dependent comparison. Unsupported capabilities stay visible.

## Work, clocks and resources

Freeze matched semantics or native workflow, required unit/count, limits and
completion before timing. Matched file search requests ten distinct files;
chunk/span tracks keep their own budget. Collect-to-files enumeration discloses
extra work and may compare workflow cost, not equal internal engine work.

- Client request construction through complete decoded required output; CLI startup
  belongs to that workflow. A resident-worker profile is separate. First-result
  latency and stable completed top-k retain timeout/partial meanings.
- Actual server planning/candidate/verification/ranking/grouping/source/preview/
  serialization phases where available, with original clock domains. Missing
  instrumentation is unavailable; unrelated-clock subtraction cannot create it.
- Output bytes/units, examined/verified work, duplicates, score components and
  diagnostic-observation overhead where observable.
- Construction wall/CPU, process-tree RSS, final/transient disk, source/indexed
  files/bytes/units and parse/chunk/embed/index/publish/activate costs. Include
  ranked keys, coverage and all authority sidecars in total update cost.

Cold process/model/index/page cache and warm query states remain distinct. Fresh
directories cannot prove cold OS cache; unavailable eviction yields a named
diagnostic. Sampled RSS/logical bytes retain their actual observation limits.

Use existing admitted host/process/bounded-I/O owners and continuous envelope
checks. Qualified timing excludes undeclared load/builds/indexers. Prespecify
balanced randomized paired seed/order/warmup, independent rounds, distributions,
sample floors, paired uncertainty and p50/p95; p99 needs adequate tail samples.
No retry-until-favorable sampling. Open-loop/concurrency owners reconcile offered/
achieved rates, errors/timeouts/drops and queue delay; closed-loop latency alone
cannot qualify overload behavior. [B07](../../sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md)
retains current repetition floors and stopping rules.

## Incremental and recovery acceptance

Run a deterministic source-bound sequence with independent expected state:

1. Full build/readiness and baseline queries.
2. Add/edit source and verify new bytes/definitions while old readers remain valid.
3. Rename/move/delete and verify old paths/declarations disappear.
4. Malformed source retains admitted lexical text and truthful symbol capability.
5. Syntax repair replaces failed/old facts with current symbols.
6. Interrupt/restart at declared process/write stages and reconcile coherent state.

Bind source hashes, events and publication identities. An actual watcher permits
workspace-event latency; explicit ingest uses its narrower start. ACK, activation
and query visibility are distinct milestones. Await bounded observed state,
never sleep-based presumed completion. Report stale hits, lag distributions,
bytes/files reprocessed per change, CPU/RSS/disk amplification and recovery.
Readers see coherent old/new snapshots; full reindex products retain that policy.

## Completion and owner execution

Every required profile has source/index/readiness proof or explicit blockers;
matched/native rows, cold/warm/build/query/transport units and required resource
observations remain separate. Paired complete captures replay through BENCH-02
and score under BENCH-03. Selected mutation/watcher/restart behavior has actual
terminal proof. Performance requires bound source/host/inputs, sample floors and
predeclared budgets; local functional checks retain their narrower scope.

Use current registry/producers/adapters and Justfile freshness/open-loop rails;
canonical execution cannot depend on disposable external collectors. Independent
approvals remain required inputs. [CS-INT-01](CS-INT-01-integration-and-qualification.md#required-controls)
owns integration; historical probes/observations are recoverable through
[the history index](../../ARCHIVE-INDEX.md#oct-05-residual-owner-clarification).
Research references: [S02/S03/S08/S09](../references.md).
