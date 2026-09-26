# Jun 7 Search Product Quality Ticket Index

Parent RFC: [../rfc.md](../rfc.md)

Status summary:

- packet planned
- this packet owns search product quality, not DSL feature coverage
- this packet is restricted to the non-semantic code-search stack
- correctness verification remains owned by
  `git show eff53181:docs/plans/jun-7-verification-hellgates/rfc.md`
- current quality backlog is split into relevance, snippet/explain, scale,
  tail, operator UX, ambiguity repair, and UI contract lanes

Execution discipline:

- every ticket must declare a primary quality dimension:
  - relevance
  - snippet/explain
  - scale
  - tail
  - operator UX
  - ambiguity repair
  - UI contract
- every ticket must encode its preferred implementation direction inside the
  owning seam, not only in chat
- reviewer rejection examples belong in the ticket text, not in follow-up chat
- semantic retrieval and hybrid fusion quality are out of scope for all
  `J7Q-*` tickets here
- any ranking-facing `SOTA` claim must point to an explicit Sourcegraph lexical
  overlap subset

Worker read order:

1. [../WORKER_START_HERE.md](../WORKER_START_HERE.md)
2. [../NO-GO-RULES.md](../NO-GO-RULES.md)
3. [../SOURCE_TRUTH_MAP.md](../SOURCE_TRUTH_MAP.md)
4. [../MEASUREMENT_MATRIX.md](../MEASUREMENT_MATRIX.md)
5. [../COMMAND_AND_ARTIFACT_CONTRACT.md](../COMMAND_AND_ARTIFACT_CONTRACT.md)
6. [../DUMB_LLM_EXECUTION_CHECKLIST.md](../DUMB_LLM_EXECUTION_CHECKLIST.md)

## Ticket Table

| ticket | status | scope |
| --- | --- | --- |
| [J7Q-00](J7Q-00-scope-lock-and-measurement-policy.md) | planned | freeze measurement vocabulary and blocking/advisory boundaries |
| [J7Q-01](J7Q-01-ranking-quality-and-relevance-corpus.md) | planned | add relevance corpus and ranking quality gates |
| [J7Q-02](J7Q-02-snippet-and-explain-quality.md) | planned | raise snippet and explanation quality above field-presence proof |
| [J7Q-03](J7Q-03-large-corpus-scale-tiers.md) | planned | define reproducible scale tiers and large-corpus proof |
| [J7Q-04](J7Q-04-latency-tail-hardening.md) | planned | turn route-critical p95/p99 tails into explicit quality gates |
| [J7Q-05](J7Q-05-operator-ergonomics.md) | planned | add operator-facing diagnosis and readiness surfaces |
| [J7Q-06](J7Q-06-ambiguous-intent-handling.md) | planned | add repairable typed-failure ergonomics without weakening fail-closed behavior |
| [J7Q-07](J7Q-07-ui-ux-contract-surface.md) | planned | add typed UI-consumable result contract fields |
| [J7Q-08](J7Q-08-followthrough-and-gate-integration.md) | planned | integrate quality gates into stable commands and docs |
