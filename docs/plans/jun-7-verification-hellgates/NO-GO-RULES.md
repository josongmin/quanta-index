# No-Go Rules

> Archive status: `Historical program record`. Current architecture: [JUN-08-001](../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../ARCHIVE-INDEX.md).


- do not add new DSL support claims in this packet
- do not move fast truth back into a giant monolithic e2e file
- do not let perf compare stand in for correctness
- do not call `sdk_frontdoor` or `e2e_full_corpus` a small fail-fast hellgate
- do not mark cross-repo ingress green from `quanta-index`-only rails
- do not duplicate query text authority outside `SCENARIOS` when the surface is
  already represented there
- do not collapse owner-local, front-door, broad daemon, and perf evidence into
  one boolean status
