# J7Q-03 — Measured scale acceptance

Status: `ACTIVE_RESIDUAL`
Parent: [quality index](INDEX.md)
Owner: seeded corpus generator, storage/runtime and benchmark harness

`scale.rs` defines seeded tier generation/manifests. `scale_matrix` measures
small-tier ingest/seal/activation/query; medium/large/XL declarations are
advisory. Declared tiers and toy fixture success are not measured capacity.

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

Focused Rust owner execution and the real `scale_matrix` rail remain `NOT_RUN`
for this source change. Medium/large/XL measurements and canonical-host
capacity acceptance remain `NOT_RUN`.
