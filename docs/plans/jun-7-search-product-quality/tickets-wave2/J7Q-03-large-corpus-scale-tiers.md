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
  retain route-local budgets and actual storage/RSS methods.
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

The scoped-tier Rust owner tests, real medium/large/XL runs, canonical-host
capacity and portability remain `NOT_RUN` at this code stage. A source limit or
wire admission refusal must be recorded with its exact tier and must not be
converted into a throughput result.
