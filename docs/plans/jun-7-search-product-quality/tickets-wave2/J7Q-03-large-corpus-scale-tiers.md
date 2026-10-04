# J7Q-03 — Measured scale acceptance

Status: `ACTIVE_RESIDUAL` (current-source audit 2026-10-04). Implementation and
focused tests are present; new release tier execution remains `NOT_RUN`.
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
  process, so this includes daemon, harness and generated fixture allocations
  retained in the same process. The runner now measures whole-process CPU,
  exact-file deletion and same-process daemon-thread reopen, with a fresh
  positive query after reopen; the 256-file diagnostic below exercised them.
  Current source additionally implements phase CPU and 100-ms RSS sampling
  with gap/coverage checks. Fresh release runtime evidence for these phase
  observations, daemon-only attribution, physical write I/O, OS process restart
  and quiet-host performance qualification remain open.
- Report the supported limit and any failure per tier. Do not infer restart,
  memory or large-corpus behavior from small-tier query latency or scan-vs-index.
- Re-run another host with matching input/configuration where portability is
  claimed; preserve platform exclusions and actual process/resource evidence.

Output owner: registered `scale_matrix`, with `summary.json` and
`tier_manifest.json`. Canonical host/performance and comparator acceptance also
remain in [CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md).

## Current-source audit and actionable residuals (2026-10-04)

Audit baseline: main `e43cda8c` with the owned staged overlay. Current `scale::`
tests passed 29/29, including missing/invalid samples, maximum gap, CPU arithmetic,
primary/cleanup failures and source identity/delete/reopen oracles. The latest
metadata golden explicitly states that RSS maximum includes start/end probes
outside the timed operation and process CPU includes sampler/parent probe work;
it excludes macOS `ps` child CPU. These are functional checks, not scale timings.

Run actual medium 256, large 4,096 and XL 32,768 tiers in separate new external
roots from one clean source with matching release harness binaries. Preserve
the scale default of two history generations and 16 MiB. A large run with
`--client-timeout-ms 300000 --history-max-bytes 268435456` is a separate explicit
diagnostic profile; do not alter defaults or relabel older refusals. Open-loop
retains eight generations and cannot inherit scale's two-generation policy.
Use the existing `scale_matrix` and `open_loop_matrix`; no new harness is needed.

Current-main scale execution refused `BENCH_WORKTREE_DIRTY` before product
execution. A fixed engine snapshot `7ff8251e` exists, but its release SDK proof
does not build or execute the separate scale harness. Fresh release scale
execution, per-phase resource measurements, OS-process restart qualification
and portability remain `NOT_RUN`. Historical snapshots and receipts below
must retain their own source/configuration labels.

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
below. A source or wire admission refusal must be recorded with its exact tier
and must not be converted into a throughput result.

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

The large failure exposed an error-custody defect in the harness. Source
`582cb7a5b91cc72ffa8b833c0382cd2ef4b062ca` explicitly stops the driver
and preserves measurement and cleanup errors separately in a failure artifact.
It also adds a bounded
`--client-timeout-ms` override while keeping the 30 s default, process-wide
user/system CPU deltas (`RUSAGE_SELF` covers harness plus daemon thread from
runtime boot through cleanup, excluding source generation/preflight), exact
file deletion, and same-process daemon-thread reopen measurement. The latter
does not prove OS-process restart or cold page-cache recovery. Focused owner
tests passed (`scale::` 22/22, `open_loop_matrix` 16/16,
`scale_matrix` 2/2; `--all-features --locked`). Fresh medium/large/XL runs
against detached clean source `582cb7a5` remain `NOT_RUN`. Its
`--all-features --locked` `scale_matrix`/`open_loop_matrix` build passed in
5 min 11 s; the scale binary SHA-256 is
`b6a1f658dc579edde9f477d29247b2ccf26097430a516a2ef4aef427255452a6`.
The large primary failure and whether a longer
explicit client deadline suffices remain unconfirmed.

## 2026-10-04 fresh source `582cb7a5` diagnostic result

The fixed source and `scale_matrix` binary named above were used for each
fresh, separate external output root. This run was on a busy host and does
not qualify latency, throughput, or portability. These outcomes supersede
the earlier snapshot only for their own source and configuration.

- Medium, default 30 s IPC deadline: **VERIFIED** measured rail, 256 files
  and four source repos. Build 8.564 s, activation 0.432 s, first query
  27.076 ms, warm p50 27.456 ms. Process CPU from runtime boot through
  cleanup was user 10.107 s/system 2.711 s. Peak whole-process RSS was
  182,190,080 B, including fixture allocations. Exact deletion took
  2.539 s to seal and 0.431 s to activate; same-process daemon reopen to
  readiness took 0.585 s, followed by a validated positive first query
  in 3.025 ms. All source identity, deletion/no-answer and retained-file
  checks passed. Evidence:
  `/private/tmp/qi-scale-medium-20261004-582cb7a5-r1/summary.json`.
- Large, default 30 s IPC deadline: **FAILED** with primary `ipc Read timed
  out after 30000 ms`; the refusal preserved the original error and did not
  report a cleanup error. Evidence:
  `/private/tmp/qi-scale-large-20261004-582cb7a5-default-r1/refusal.json`.
- Large, one explicit `--client-timeout-ms 300000` diagnostic: **FAILED**
  with typed daemon code `SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED`. The
  required one-generation index was 61,650,630 B; this harness was configured
  for a 16,777,216 B history byte maximum. The failure artifact has
  `limit: null` because the source-preflight limit classifier does not parse
  daemon error strings. This identifies a harness retention-budget blocker,
  not an intrinsic engine maximum. Evidence:
  `/private/tmp/qi-scale-large-20261004-582cb7a5-300s-r1/refusal.json`.
- XL, default 30 s IPC deadline: **VERIFIED typed refusal** before daemon
  start: 4,000,461 posting memberships versus the 4,000,000 source-index
  admission maximum. It is not measured XL throughput. Evidence:
  `/private/tmp/qi-scale-xlarge-20261004-582cb7a5-r1/refusal.json`.

Successful large-tier performance requires a separate, explicitly configured
history-retention budget with provenance and a fresh source/binary binding.
Changing the configured budget is a new experiment; neither refusal should
be counted as a successful tier or an intrinsic product size ceiling.

## Explicit history-budget rail follow-up

The scale runner now accepts `--history-max-bytes` in 1..=256 MiB and passes
it through the existing harness history policy. The upper bound matches the
harness's fixed total history budget; the default remains 16 MiB and two
generations. Success details and configuration digest bind both the
requested and effective history/timeout values; refusal `execution` binds
the same values and records the fixed total budget and revision-pair cap.
Build, activation, delta, deletion and reopen errors carry
their operation stage without inferring a product limit from error text.

Focused verification: harness `scale::` 24/24, `scale_matrix` binary 3/3,
teardown fault units 3/3, shared socket reopen E2E 1/1. The 1-byte history
budget unit observes a real seal refusal from the daemon. Initial binary
compilation rejected one unused result; the parser was corrected and the
3/3 binary test passed. A fresh release-profile build and a 4096-file run
with an explicit larger history budget remain **NOT_RUN**. The 582cb7a5
receipts above cannot be relabeled as results of this new runner policy.

Subsequent owner verification after the total-cap and stage-context changes:
the builder policy unit 1/1, full `scale::` units 24/24 and bounded CLI unit
1/1 passed with `--all-features --locked`. The refusal unit covers an untyped
ingest failure, preservation of an existing typed source limit, and a
cleanup-only failure. These are code-path tests; a new large-tier runtime
attempt and release-profile measurement remain **NOT_RUN**.
