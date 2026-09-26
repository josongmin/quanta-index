# J7-05 — Daemon Lifecycle Hellgate

> Archive status: `Historical program record`. Current architecture: [JUN-08-001](../../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `landed`

Goal:

- aggregate runtime boot, query, restart, replay, and fail-closed broad rails

Owner seam:

- `Justfile`

Delivered target:

- `just rust-verify-hellgate-broad`

Included rails:

- `dsl_scenarios`
- `sdk_frontdoor`
- `end_to_end`
- `e2e_restart_replay_determinism`
- `e2e_perf_chaos`
- `explain`
- `repo_map_end_to_end`
- `e2e_full_corpus`
