# L3 structural completion audit

**VERIFIED within the original L3 A/B/C scope. Known unresolved findings: P0 0,
P1 0, P2 0.** One additional P2 cancellation defect was reproduced and repaired.
This is an owner implementation/audit result, not whole-engine or release qualification.

Current source: HEAD `2102966246866398f01833bebf71396831377149` plus dirty work on `main`.
The final command's 338 selected source/config/dependency inputs were unchanged
through execution and closeout. Concurrent work outside this closure was preserved.
Compatible lint edits to the owned file were retained and included in the final run.
No agent was spawned and no inter-task communication, commit, push or reset was
performed by this task.

## New P2: request interruption stopped at the traversal boundary

A deterministic query cancels on the native scorer's terminal advance. With two
work units already used, ranked merge returned `LexicalCollectionBudgetExceeded`
instead of `RequestCancelled`. Grouped harvest and merge had the same boundary:
they observed collection resources, while the request probe only surrounded the
native traversal. With a larger budget, cancelled work could continue before the
outer probe returned the cancellation.

The repair extends the existing canonical owner:

- `RequestProbe` contains the shared request and sticky observed-interruption flag.
  Native traversal and collector phases use that same state.
- `budgeted_collection` binds the request once to all already-cloned collection
  handles. Reusing the ledger for another search is an explicit invalid contract.
- Work/byte admission and harvest/merge/sort boundaries observe interruption before
  consuming more resources. Previously observed integrity/resource failures retain
  their authority. Public error codes and checkpoint names remain canonical.
- The bound state owns no collector/traversal handle, preventing a direct retention
  cycle. A cleanup regression checks the binding is released.

The native terminal and merge-entry regressions cover ranked/grouped collectors
under tight and ample work budgets. They require the cancellation code, no extra
work, no poisoned resource ledger and zero retained bytes. Separate checkpoint
cases cover cancellation and expired deadlines ahead of work/byte refusal.
Cancellation remains cooperative at documented operation boundaries; no hard
wall-clock bound for an individual engine operation or sort is claimed.

## Original requirements and omission review

[L3_REQUIREMENT_AUDIT.json](L3_REQUIREMENT_AUDIT.json) maps 13 requirement groups
to terminal passing tests in this run and identifies lifecycle exclusions.

- **A — VERIFIED:** exact local/qualified fields, NFC/case, overloads, nested and
  absent-name controls; indexed/manual Boolean paths; legacy broad/content search;
  original spelling, signature and definition bytes. These policies were audited
  and revalidated; default ranking weights were not changed.
- **B — VERIFIED:** containing pin versus source owner, same-path federation,
  source/path filters, file/repo projection and complete cursor walks. Source/path
  fixtures now have distinct content and independent fixed SHA-256 goldens, so
  hash confusion cannot hide behind identical fixture content.
- **C — VERIFIED:** pre-materialization work/examined caps, dictionary/group/fruit
  and output-buffer reservations, release lifetimes, typed refusal, cancellation,
  deadline, native TermQuery pruning, streaming count, global best representative,
  deterministic ties, empty controls and exact page/cursor behavior.
- **Previous error RCA — VERIFIED again:** malformed and duplicate key errors,
  first/middle/last harvest errors, fallible fruit admission and direct merge error
  precedence. No further reachable P0-P2 defect was identified in the inspected
  owner/caller/sibling paths after the repair.
- **Construction and compatibility:** the three production paging entry points
  construct fresh collection handles and use the canonical binder. Direct Tantivy
  callers without a request retain resource-only semantics. No public DTO, stored
  format or schema migration is introduced. No alternative source identity exists.

## Proof

`goal-final-3`: **182 library + 3 cancellation + 4 execution-budget + 6 exact-source
+ 5 ranked-pages = 200 passed, 0 failed, 0 ignored; exit 0.** This count is from one
terminal run. Focused and historical counts are not added to it.

```sh
QUANTA_INDEX_RESOURCE_ADMISSION=1 CARGO_BUILD_JOBS=2 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=1200 \
./scripts/cargow --lane test-fast-lane test -p quanta-index-lexical --lib --test execution_budget --test ranked_pages --test l3_exact_source --test cancellation_inside_search --locked
```

The behavioral red ran the same canonical wrapper with `--lib
l3_terminal_cancellation_precedes_harvest_and_merge_resources --locked` and exited
101 with one expected failing assertion. HEAD advanced during that run without
changing selected source bytes; it is content-snapshot evidence, not clean-commit
proof. Intermediate compile errors in a checkpoint lifetime and fixture Box<str>
construction were fixed before the same rails were rerun. The final source also
passes owned-file rustfmt and whitespace checks.

- Selected input SHA256: `3494250c48902998da96c8b4fdb5576779786ab1e2e517c2acf61028cd7f6258`.
- Final raw log SHA256: `535a130850fee7f408f7358e06bee2d8d12430a94abe6a2ca5aaf42244114174`.
- Receipt: [L3_SS.source.json](L3_SS.source.json), SHA256 `52ca29f67446f89f2e2ada2f947dfb31ece3ebbe647d6f861c95ccd2fc591db5`.
- Raw logs, source/dirty manifests, resolved dependencies, owned-source copies,
  patch, commands and binary hashes: `l3-proof/ss/`.

The task changed `budgeted_search.rs`, `ranked_page.rs`, `ranked_page_tests.rs`
and `tests/l3_exact_source.rs` in the lexical crate. Some of these changes were
committed by concurrent work during this task; the receipt records source bytes
and actual HEAD/dirty states rather than inferring ownership from the final diff.

Residual known L3 implementation defects: **none identified**. Whole-repository
CI, unselected integration tests, SDK/CLI qualification, installed daemon E2E,
release/deployment, physical OOM, peak RSS, performance and ranking-quality
promotion: **NOT_RUN**, outside this owner audit claim. No universal absence-of-bugs
or numerical quality score is asserted.
