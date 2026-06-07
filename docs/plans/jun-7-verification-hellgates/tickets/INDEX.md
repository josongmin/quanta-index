# Jun 7 Verification Hellgates Ticket Index

Parent RFC: [../rfc.md](../rfc.md)

Status summary:

- packet landed
- feature backlog is not reopened here
- this packet owns verification shape only
- current final snapshot:
  - `rust-bench-dsl-truth` green
  - `rust-verify-hellgate-fast` green
  - `rust-verify-hellgate-broad` green
  - `rust-verify-hellgate-cross-repo` red in the current snapshot
    - external `semantica-codegraph-v2` boundary guard failure:
      `quanta-sdk.runtime-facade-boundary.v1`
  - `rust-bench-dsl-compare` green
  - `rust-verify-hellgate-all` exists but this snapshot is recorded from
    component reruns instead of one completed aggregate rerun

| ticket | status | scope |
| --- | --- | --- |
| [J7-00](J7-00-scope-lock-and-verification-architecture.md) | landed | freeze the proof-layer split |
| [J7-01](J7-01-scenario-ssot-normalization.md) | landed | `SCENARIOS` carries hellgate metadata |
| [J7-02](J7-02-text-route-hellgate.md) | landed | small text-route runtime hellgate |
| [J7-03](J7-03-structural-route-hellgate.md) | landed | small structural runtime hellgate |
| [J7-04](J7-04-cross-repo-ingress-hellgate.md) | landed | separate targeted external ingress proof lane |
| [J7-05](J7-05-daemon-lifecycle-hellgate.md) | landed | aggregate broad daemon lifecycle gate |
| [J7-06](J7-06-corpus-and-inventory-rail-split.md) | landed | document corpus vs guard roles |
| [J7-07](J7-07-perf-hellgate-and-compare-policy.md) | landed | aggregate perf policy and naming split |
