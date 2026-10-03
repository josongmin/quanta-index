# J7Q-03 — Measured scale acceptance

Status: `ACTIVE_RESIDUAL`
Parent: [quality index](INDEX.md)
Owner: seeded corpus generator, storage/runtime and benchmark harness

`scale.rs` defines seeded tier generation/manifests. `scale_matrix` now accepts
`--tier medium|large|xlarge` or explicit `--all-tiers`; default is small.
Large tiers bind each file to `(source_repo_id, repo_relative_path)` under one
serving owner, check the planted source oracle and actual ingest envelope, then
measure ingest/seal, activation, cold/warm query, adapter open and one-file
delta. A selectable rail is not measured capacity until its actual run succeeds.

## Remaining acceptance

- Capture medium/large/XL on the admitted host and identical reproducible inputs.
  Record repo count, file-size distribution, hit/symbol density, seed, route mix,
  total bytes, model/index ownership and tier manifest.
- Measure ingest, open/reopen, query, restart recovery and memory independently;
  retain route-local budgets and actual storage/RSS methods. Current
  `ResourceUsageV1::observe_self` reads process-high-water RSS with
  `getrusage(RUSAGE_SELF)`: the E2E daemon runs in a thread in the same
  process, so this includes both daemon and harness allocations. It cannot
  isolate daemon RSS or per-phase peaks. CPU time, reopen/restart and delete
  behavior are not measured by the current selected-tier runner.
- Report the supported limit and any failure per tier. Do not infer restart,
  memory or large-corpus behavior from small-tier query latency or scan-vs-index.
- Re-run another host with matching input/configuration where portability is
  claimed; preserve platform exclusions and actual process/resource evidence.

Output owner: registered `scale_matrix`, with `summary.json` and
`tier_manifest.json`. Canonical host/performance and comparator acceptance also
remain in [CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md).

## 2026-10-04 measured-response hardening

The small-tier source fixture now establishes an independent result-count oracle:
all sixteen generated files contain the planted query, so a `top_k=10` response
must contain ten candidates. The first and every warm measured response are
checked after their request timer stops; a shorter successful page fails the
rail with the sample number instead of entering the latency aggregate. The
owner unit includes a source-fixture mutation and a second-sample short page.

The pre-extension `scale::` focused unit binary passed 12/12 on 2026-10-04;
`cargow` admission for the same filter timed out waiting on another build.
That result does not verify the later scoped-tier implementation.

## 2026-10-04 scoped-tier implementation

- Medium is 4 distinct source repositories × 64 files; large is 16 × 256;
  XL is 64 × 512. The serving owner count remains one. The corpus digest binds
  both repo ID and relative path. A fixture unit covers the same relative path
  in two source repositories and rejects wrong/duplicate identities.
- Before sealing, the rail checks the lexical 8 MiB per-file, 128 MiB total
  source and 4 million posting-membership limits. It previews the exact pending
  IPC envelope through the existing encoder, including its 16 MiB frame and
  128 MiB decoded-request admission. A refusal is a tier failure, not a timing.
- The global query's cold and every warm response, 32 adapter responses, and a
  separate planted query for every source repository require source-backed
  top-10 identities. Timers stop before response validation. The recorded build
  time sums ingest and seal and excludes the wire preflight cost.
- Activation proves and promotes an already opened lexical snapshot. The first
  query can therefore have no `lq_snapshot_lexical_cold_open_ms` sample; its
  query cold-open field is `null`/`unavailable` in that case, while the required
  route histogram still has one sample. Physical opening is included in
  activation timing. The valid retention minimum is two generations; one delta
  may have no reclaim, so `gc_ms` is absent when no directory bytes shrink.
- `directory_bytes` differences used by `bytes_written` and disk amplification
  are logical directory-size deltas, not measured physical write I/O; hard
  links may be counted more than once.
- `--out-dir` is required for non-default tiers. Each selected run records one
  measured tier; unselected tiers remain declarations in that artifact.
- Explicit output roots must be new absolute paths outside the checkout. A
  rejected selected tier writes `refusal.json` with seed, tier, one serving
  owner, source repo/file/byte counts, source digest, head, host, failed stage,
  original error, and a limit only when a typed source admission check proves
  it. It writes no latency zero or successful summary. The refusal writer uses
  atomic no-replace publication; an all-tier failure writes no earlier tier
  success summary from that run. IPC encoder errors retain their original text
  with `limit: null` until the IPC owner has a typed cap error.

At the scoped-tier implementation stage, actual tier runs and canonical-host
capacity/portability were `NOT_RUN`; the later diagnostic attempts are listed
below. A source limit or
wire admission refusal must be recorded with its exact tier and must not be
converted into a throughput result.

## 2026-10-04 bounded diagnostic execution

These observations are from detached clean source
`108873c0e08b3249c8c314ac451637ee4fc1ef08`, built in its own
`scale-proof-lane` with `--all-features --locked`. The `scale_matrix` binary
SHA-256 was
`36451c49a6ff1e5122c1f347f6d26800e3cdea3e9567203e5571eee38c2b7652`.
The host was busy, so these are correctness/capacity diagnostics, not a
quiet-host performance qualification.

- `--tier medium`: 256 files across four distinct source repos under one
  serving owner, exit 0. Build 9.634 s, activation 0.545 s, first query
  42.765 ms, warm p50 42.643 ms. The query cold-open histogram had zero
  samples and was correctly recorded as unavailable. Evidence:
  `/private/tmp/qi-scale-medium-20261004-108873c0-r2/summary.json`.
- `--tier large`: 4,096 files across sixteen source repos, exit 101 after
  approximately three minutes. The runtime reported a hard shutdown deadline
  with `ingest-accept` unfinished. `E2eRuntime::Drop` panicked and masked the
  original measurement error, so this attempt produced no refusal file and
  proves neither a typed source limit nor successful large-tier capacity.
- `--tier xlarge`: 32,768 files across 64 source repos, exit 1 before daemon
  start. Typed source admission refused 4,000,107 posting memberships against
  a 4,000,000 maximum. Evidence:
  `/private/tmp/qi-scale-xlarge-20261004-108873c0-r1/refusal.json`. This is a
  supported-limit observation, not measured XL throughput.
- The same immutable source's open-loop medium QPS 10 diagnostic served
  10/10, with p50 49.719 ms and p95 183.103 ms, zero errors and drops.
  Evidence: `/private/tmp/qi-open-loop-medium-20261004-108873c0-r1/`.

The large failure exposed an error-custody defect in the harness. Current
unverified owner changes explicitly stop the driver and preserve measurement
and cleanup errors separately in a failure artifact. They also add a bounded
`--client-timeout-ms` override while keeping the 30 s default, process-wide
user/system CPU deltas (`RUSAGE_SELF` covers harness plus daemon thread), exact
file deletion, and same-process daemon-thread reopen measurement. The latter
does not prove OS-process restart or cold page-cache recovery. Focused owner
tests and fresh medium/large/XL runs against one new frozen source are still
`NOT_RUN` for these changes. The large primary failure and whether a longer
explicit client deadline suffices remain unconfirmed.
