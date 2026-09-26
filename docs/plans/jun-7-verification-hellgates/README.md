# Jun 7 Verification Hellgates

> Archive status: `Historical program record`. Current architecture: [JUN-08-001](../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


Status: `landed`

Purpose:

- split fast correctness hellgates from broad daemon sweeps
- keep `SCENARIOS` as golden-truth SSOT
- stop growing `e2e_filter_execution.rs` and `sdk_frontdoor.rs` as the only
  DSL proof sinks

Read order:

1. [WORKER_START_HERE.md](WORKER_START_HERE.md)
2. [NO-GO-RULES.md](NO-GO-RULES.md)
3. [SOURCE_TRUTH_MAP.md](SOURCE_TRUTH_MAP.md)
4. [DUMB_LLM_EXECUTION_CHECKLIST.md](DUMB_LLM_EXECUTION_CHECKLIST.md)
5. [rfc.md](rfc.md)
6. [tickets/INDEX.md](tickets/INDEX.md)

Current shipped gates:

- small bench truth:
  - `just rust-bench-dsl-truth`
- fast hellgates:
  - `just rust-verify-hellgate-fast`
- broad daemon lifecycle:
  - `just rust-verify-hellgate-broad`
- perf compare:
  - `just rust-bench-dsl-compare`
- full aggregate:
  - `just rust-verify-hellgate-all`

Current verified snapshot on `2026-06-08`:

- `just rust-bench-dsl-truth`
  - green (`2 passed`)
- `just rust-verify-hellgate-fast`
  - green
- `just rust-verify-hellgate-broad`
  - green
- `env QUANTA_INDEX_SEARCHD_BIN=/Users/songmin/Library/Caches/quanta-index/target/daemon-lane/debug/quanta-index-searchd just rust-verify-hellgate-cross-repo`
  - red in this snapshot
  - external `semantica-codegraph-v2` boundary guard failure:
    `quanta-sdk.runtime-facade-boundary.v1`
- `just rust-bench-dsl-compare`
  - green
- `just rust-verify-hellgate-all`
  - aggregate target exists
  - this snapshot is recorded from the component reruns above, not from one
    completed monolithic aggregate rerun

Preflight:

- still repo-global `unverified`
- `scripts/check-persona-target-policy.sh` 없음
- `scripts/cg-agent-session` 없음

Backlinks:

- feature closeout packet: [../jun-6-sourcegraph-expansion/rfc.md](../jun-6-sourcegraph-expansion/rfc.md)
- capability inventory: [../../analysis/jun-4-dsl-capabilty.md](../../analysis/jun-4-dsl-capabilty.md)
