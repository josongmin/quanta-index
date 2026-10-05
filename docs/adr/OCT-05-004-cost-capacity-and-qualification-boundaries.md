# OCT-05-004 — Cost, Capacity and Qualification Boundaries

Status: `Accepted`

Decided: 2026-10-05

Consolidates implemented O4-E4/I0 contracts under
[SEP-26-002](SEP-26-002-retrieval-observation-experiment-and-default-policy.md),
[SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md) and
[SEP-27-004](SEP-27-004-benchmark-capture-and-resource-custody.md).
No conditional optimization or staged operation becomes accepted implementation.
Measurements and missing authority remain in the [residual ledger](../plans/oct-4-parallel-closure/tickets/INDEX.md#e4).

## Decision

1. Preserve exact live-document BM25 statistics and source-bound file/name
   witnesses across full/delta/delete/no-op/reopen and format migration. Existing
   query plans keep default literal-first versus explicit typo and lexical versus
   semantic contribution distinct. Changed policy requires independently judged
   failures/critical strata and unused holdout; diagnostic misses alone do not
   authorize a global model, RRF, chunking or storage replacement.
2. Durable artifact publication retains file sync → rename → parent sync and
   generation/terminal custody. Group barriers are conditional on isolated sync
   cost, not a timer enclosing other work. Adoption requires old-or-complete-new
   roots, inherited-file/digest custody and independent fault/crash/reopen tests.
   Syscall faults and process kill do not establish storage power-loss guarantees.
3. Report explicit whole-call and child clock/resource domains. Request-local
   SDK/IPC and parent/worker phases explain attribution, not interchangeable
   latency. Sampled RSS is a sampled maximum; process CPU can include sampling.
   Physical I/O, logical bytes and transient disk need their actual observers.
4. ASCII scanning preserves original byte spans, Unicode fallback, bounds and
   cancellation. `query_timing_overhead.py --scanner-ab` binds separately declared
   source/binaries and checks response/status/cursor/work/config parity while
   permitting declared clock differences. Observation on/off remains a separate
   comparison. Keep/modify/withdraw requires whole-caller acceptance. Persistent
   token authority additionally needs a demonstrated repeated-scan bottleneck,
   exhaustive tokenizer/OSA witness parity and lifecycle/build/residency budgets.
5. Scale/open-loop use typed preflight, complete offered-request accounting,
   independent lifecycle/restart oracles and explicit limits. Default timeout and
   posting-cap refusal stay failures of the requested capacity gate. Diagnostic
   timeout/history overrides cannot qualify defaults; do not raise a limit or
   shrink a fixture solely to turn an observed refusal green.
6. Performance requires the declared response/output boundary, paired/randomized
   schedule, fresh roots, repetition floors and continuous admitted-host inputs.
   Missing frequency/thermal/power/load authority cannot be filled with nominal
   values. Functional, shared-host and causal samples remain scoped diagnostics.
7. One integration owner handles shared schemas/DTOs/registry/CI/dependencies and
   actual source impact. PREPARE → affected source validation → admission ISSUE
   precedes dependent captures. A later source/input change rechecks affected
   proof rather than relabeling old receipts. Normative ADRs are source-closure
   inputs; history/navigation is not a substitute for executable authority.
8. Existing result producer/schema/parser, proof registry and aggregate own
   source/binary/selector/terminal truth. Local owner proof, portable replay,
   hosted CI, Linux release, real provider, paired producer and operational action
   are distinct scopes. Zero-selected, skipped, missing or staged prerequisites
   cannot issue qualification. Product quality/holdout/performance apply only
   when that claim is requested; they do not universally block code qualification.
9. P11 deploy/activate/restore-forward recipes and typed operational result mode
   remain unimplemented. Actual actions, distinct independent pre/post success
   observers and authorized host/state/rollback inputs must be defined under
   [S21-12](../plans/sep-21-search-plane-sota-hardening/tickets/S21-12-cross-repo-terminal-receipt-cutover.md)
   before registry promotion. Shell exit zero or caller-written success JSON is
   not operational authority. Keep the existing staged refusal.

## Explicit typo declaration priority

Explicit OSA1 orders edit distance before source-attested declaration preference,
then occurrence evidence and stable file ties. A declaration boost cannot cross
an edit-distance tier. Unknown symbol coverage retains content matches; default
literal-first eligibility stays independent. Versioned scoring/cursor identity
binds this policy. File recovery does not establish declaration-position recovery.

## Selected score and optional ranking diagnostics

- File Explain resolves immutable file authority and shares the selected scorer
  with search. Additive score components, engine, boost, total and emitted score
  must agree. Optional declaration evidence validates pinned name/definition
  identity; missing coverage is unknown, not zero or a literal exclusion.
- The complete-pool rank study retains native first-page equality before cursor
  continuation, source/generation/profile bindings, all pool pages, ablations and
  explicit exclusions. Experimental declaration/boundary/occurrence policies
  are unselected. Only proven complete eligible pools receive full-rank metrics;
  file features cannot count declaration-span recovery.
- Optional study collection-budget refusal preserves valid selected-score Explain
  with its typed refusal after cancellation/deadline checks. Storage/identity and
  interruption errors propagate. A study and refusal are mutually exclusive;
  no synthetic neutral/zero experimental score fills missing diagnostics.
- Timed queries exclude later optional study calls, while process CPU/RSS can
  include them. That capture cannot issue speed authority. Production enrichment
  batching and ranking selection remain conditional on judged unused holdout and
  complete-call/resource acceptance, not observed Gin or experimental weights.

Owners: [selected scorer/Explain](../../crates/quanta-index-lexical/src/searcher/code_search.rs),
[public Explain](../../crates/quanta-index-search-plane/src/query_dispatcher/routes/explain.rs)
and [rank study](../../tools/benchmark/retrieval/code_search_rank_study.py).
Retain independent score/cursor/source/unit mutations, long NFC/NFD/case-expansion
fixtures, optional-budget versus identity/cancel failures and complete-pool parity.

## Owners and retained proof

- [Lexical authority](../../crates/quanta-index-lexical/src/file_authority.rs),
  [query execution](../../crates/quanta-index-lexical/src/searcher/code_search.rs),
  [scanner comparison](../../tools/benchmark/retrieval/query_timing_overhead.py).
- [Scale](../../crates/quanta-index-searchd-harness/src/scale.rs),
  [open loop](../../crates/quanta-index-searchd-harness/src/open_loop.rs),
  [benchmark admission](../../tools/benchmark/retrieval/run.py).
- [Source closure](../../tools/ci/source_closure.py),
  [portable proof](../../tools/benchmark/retrieval/portable_proof.py),
  [proof registry](../../tools/ci/proof-authority.toml),
  [result authority](../../tools/ci/proof_execution_result.py).
- Retain independent scanner/config/clock mutations, lifecycle/fresh-rebuild
  parity, bounded-resource refusals, offered-request reconciliation and staged/
  source-mismatch/forged-terminal negatives. Exact historical executions remain
  recoverable through the [plan history index](../plans/ARCHIVE-INDEX.md#oct-05-handoff-and-ticket-compaction).

## Consequences

Completed implementation has an ADR owner; unmeasured optimizations and release
requirements keep their active owner. Neither documentation consolidation nor
past test totals issue new benchmark, code, release or operational qualification.
