# Jun 7 Verification Hellgates RFC

Status: `landed`
Date: `2026-06-08`

This packet is verification architecture only.

It follows the landed feature packets:

- [../jun-4-sourcegraph-parity/rfc.md](../jun-4-sourcegraph-parity/rfc.md)
- [../jun-5-sourcegraph-tail-gaps/rfc.md](../jun-5-sourcegraph-tail-gaps/rfc.md)
- [../jun-6-sourcegraph-expansion/rfc.md](../jun-6-sourcegraph-expansion/rfc.md)

## Scope

- normalize scenario SSOT for fast gate reuse
- add small text-route and structural-route hellgates
- separate broad daemon lifecycle from fast DSL correctness
- keep cross-repo ingress proof explicit
- keep perf compare separate from correctness

## Non-Goals

- no new DSL feature work
- no replacement of broad e2e with benchmarks
- no docs-only claim that a hellgate exists

## Final Gate Split

1. fast truth
   - `just rust-bench-dsl-truth`
   - `just rust-verify-hellgate-fast`
2. broad daemon lifecycle
   - `just rust-verify-hellgate-broad`
3. cross-repo ingress
   - `just rust-verify-hellgate-cross-repo`
4. perf compare
   - `just rust-bench-dsl-compare`

## Ticket Order

1. [J7-00](tickets/J7-00-scope-lock-and-verification-architecture.md)
2. [J7-01](tickets/J7-01-scenario-ssot-normalization.md)
3. [J7-02](tickets/J7-02-text-route-hellgate.md)
4. [J7-03](tickets/J7-03-structural-route-hellgate.md)
5. [J7-04](tickets/J7-04-cross-repo-ingress-hellgate.md)
6. [J7-05](tickets/J7-05-daemon-lifecycle-hellgate.md)
7. [J7-06](tickets/J7-06-corpus-and-inventory-rail-split.md)
8. [J7-07](tickets/J7-07-perf-hellgate-and-compare-policy.md)

## Closeout

- `J7-01` landed
- `J7-02` landed
- `J7-03` landed
- `J7-04` landed
- `J7-05` landed
- `J7-06` landed
- `J7-07` landed

Current verified snapshot on `2026-06-08`:

- `just rust-bench-dsl-truth`: green (`2 passed`)
- `just rust-verify-hellgate-fast`: green
- `just rust-verify-hellgate-broad`: green
- `env QUANTA_INDEX_SEARCHD_BIN=/Users/songmin/Library/Caches/quanta-index/target/daemon-lane/debug/quanta-index-searchd just rust-verify-hellgate-cross-repo`: red in this snapshot
  - external `semantica-codegraph-v2` boundary guard failure:
    `quanta-sdk.runtime-facade-boundary.v1`
- `just rust-bench-dsl-compare`: green
- `just rust-verify-hellgate-all`: aggregate target exists, but this snapshot
  is recorded from the component gates above instead of one completed
  monolithic rerun

Preflight:

- still repo-global `unverified`
- `scripts/check-persona-target-policy.sh` 없음
- `scripts/cg-agent-session` 없음
