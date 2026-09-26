# L4 additional audit — preview admission and wire-local consistency

The deeper audit reproduced and fixed two additional P2 defect groups. This is
selected L4 native/contract evidence, not whole-workspace qualification. Shared
checkout mutations and the aggregate regex allocation proof remain separate.

## Fixed defects

1. **P2 — unused preview pages consume request resources.** Empty regex pages
   compiled executors, charging 100,013 work units without rendering a row.
   Empty or wholly unavailable pages consumed the 256 request output-group
   slots, suppressing a later valid preview. Oversized raw input also compiled
   a regex before its cheap size refusal. Executor preparation now starts only
   for the first bounded renderable row; a slot is reserved only for an actual
   emitted output with a live allocation lease. Oversized rows refuse locally,
   preserving previews for other selected rows. Mandatory source identity,
   extent and text-authority checks still precede optional refusal. Hit IDs,
   scores, order and query truth are unchanged.
2. **P2 — decoded preview metadata can contradict emitted bytes.** The wire
   accepted mismatched context length, focus inside a UTF-8 code point, invalid
   highlight ranges/offsets, a non-primary focus, emitted text declared
   unavailable, and a forged path label. Both serialization and decoding now
   validate those local invariants. Highlight validation also applies when
   preview metadata is null. Overlapping distinct witnesses remain legal;
   duplicate/reordered spans are refused. Lexical JSON and lexical/symbol CBOR
   tests use fixed independently declared byte offsets for `é needle`.

The native target now has nine tests, including five new admission regressions.
Wire targets add six contract-base and two contract regressions. A further pass
covered primary focus ordering, Boolean witness rollback, UTF-8/context bounds,
null metadata, source binding, output lease transfer and per-row refusal. No
additional reachable P0–P2 was identified in that reviewed scope.

## Verification repairs

- Full selected lint exposed shared contract diagnostics. Changes preserve
  typed refusal behavior, use checked record lengths, enumerate coverage cases and repair
  declaration/doc placement. Core lint additionally required checked carrier size
  arithmetic, explicit mutex-guard release and equivalent error mapping. Justified
  scoped large-enum expectations preserve the existing by-value wire API; they do not box fields or change codecs.
- Pinned API tooling nightly deprecated `AtomicU64::fetch_update`, so the
  collection ledger now uses stable compare-exchange loops with the same
  acquire/release ordering, checked arithmetic and sticky refusal. Existing
  concurrency, overflow and exact-release tests cover this change.
- An attempted shared target-cache reuse executed eight native regressions
  against source containing nine. Those results are explicitly invalidated.
  Final runs use a checkout-scoped canonical target directory and require every
  expected regression name to appear as executed successfully.

## Evidence boundaries

The original shared base was `98601a66d8cab9c86232b3e62ce490c8b43b71b6` with dirty
concurrent work. Another actor committed shared changes during the audit. L4
performed no commit, push, reset, task read, task message, task polling or delegation.
The frozen source is a recorded file composition, not a clean Git revision.
Source inputs and binaries are hashed, and input hashes must match before and
after each run. Diagnostic RED results remain distinct from final proof.

## Executed proof

[Final evidence index](l4-proof/deeper/receipt.json) records exact commands,
environment, selected inputs, full snapshots, raw logs, binary digests, expected
regression names and live-tree comparison. Index SHA-256:
`68eaec3c64507a92c24e963e9935551dc071354f8f7332e12f2f7e4527ff8958`.

All heavy commands used `CARGO_BUILD_JOBS=2`, canonical checkout-scoped target
lanes and the shared resource-admission lock. No cross-snapshot target override
was used for final evidence. The independent test oracles are fixed raw ranges,
empty-page invariants, known source rows and atomic reservation/concurrency
invariants; the fuzz rail checks for crashes, not semantic completeness.

| Recorded command | Status | Executed scope |
| --- | --- | --- |
| `./scripts/cargow test -p quanta-index-contract-base -p quanta-index-contract --lib --test l4_preview_emission --test l4_preview_wire --test ipc_query_result_v2_contract --locked` | VERIFIED | 275 passed, 0 failed/ignored |
| `./scripts/cargow test -p quanta-index-core --lib --locked` | VERIFIED | 102 passed, 0 failed/ignored |
| `./scripts/cargow test -p quanta-index-lexical --lib --test l4_match_anchored_preview --test cancellation_inside_search --test execution_budget --locked` | VERIFIED | 188 passed, 0 failed/ignored; all 9 L4 native tests executed |
| `./scripts/cargow clippy -p quanta-index-contract-base -p quanta-index-contract -p quanta-index-lexical -p quanta-index-core --lib --test l4_preview_emission --test l4_preview_wire --test l4_match_anchored_preview --locked --no-deps` | VERIFIED | Selected library and integration targets |
| `just rust-fuzz-smoke` through the recorded admission wrapper | VERIFIED | 4 targets, 60-second limit each, 6,242,794 executions total, no crash |
| `just rust-public-api` through the recorded admission wrapper | FAILED | Both crates rendered successfully; contract and SDK baseline drift |
| Canonical-env `rustfmt --edition 2024 --check` | VERIFIED | All 16 touched files; exact paths and digests in format receipt |

Wire/fuzz selected-source digest:
`1f7467d3304e1ba23ed5d231a2fbe33923d7e877b327a41fdf800fc944bf835d`.
Final core/native/lint selected-source digest:
`fc88f5f79f57232ec82fe71a53ce44d566f10f3943ad9fe48dceb221923baf76`.
Fuzz and public-API receipts additionally bind the entire copied input set, so
their source digests differ from selected native manifests. The five files
changed between phases are all in core, outside the wire/fuzz dependency
closures; see [phase-boundary.json](l4-proof/deeper/phase-boundary.json).
These results are not combined into a product or live-tree qualification.

At closeout, shared HEAD was `5571132655a83824731e7909b0e310951edad52b` and remained dirty.
All 16 touched source/test files match the final frozen composition. Other
changes include Cargo configuration/dependencies, SSTable vendor removal and
lexical ingest/tests. Exact paths and hashes are in
[live-closeout.json](l4-proof/deeper/live-closeout.json).

The admission and wire RED failures are retained as diagnostic evidence. Failed
lint attempts, the interrupted admission waiter and invalidated stale-binary
attempt are not final passes. The superseded p02 counts in L4_HANDOFF are not
added to this audit's counts.

## Remaining qualification

- **FAILED:** public API baseline drift from the concurrent contract/SDK
  changes; the exact diff is retained. Baselines were not rewritten to bless
  unrelated interface changes.
- **BLOCKED:** current shared-tree qualification: Cargo dependency/config and
  ingest/test inputs changed after freezing. Native and wire receipts cover
  their recorded input sets only. See the closeout comparison for exact paths.
- **BLOCKED:** aggregate regex compiler/cache heap upper-bound proof. The fixed
  per-executor reservation bounds executor count; it does not prove total heap
  or RSS. No aggregate runtime heap overrun was reproduced. See
  [L4_REGEX_BUDGET_RESIDUAL.md](L4_REGEX_BUDGET_RESIDUAL.md).
- **NOT_RUN:** SDK fresh-process/daemon E2E, full workspace CI, latency/RSS,
  ranking qualification, release/deployment and activation.
