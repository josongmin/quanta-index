# Search product quality — residual index

Status: `ACTIVE_RESIDUAL`

[Parent](../README.md) · [Accepted gate decisions](../../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md)

The old `planned` labels are retired for implemented scaffolding. Scope/policy,
CLI diagnosis, typed repairs, highlight contracts and gate registration are
code-present; fresh consumer/runtime results and qualification remain separate.

| Remaining owner | Acceptance |
| --- | --- |
| [J7Q-01](J7Q-01-ranking-quality-and-relevance-corpus.md) | Independent judged route quality, hard negatives and actual Sourcegraph lexical overlap |
| [J7Q-02](J7Q-02-snippet-and-explain-quality.md) | Current match-window/explanation usefulness and wire/SDK/CLI provenance proof |
| [J7Q-03](J7Q-03-large-corpus-scale-tiers.md) | Measured medium/large/XL ingest/open/reopen/query/restart/RSS limits |
| [J7Q-04](J7Q-04-latency-tail-hardening.md) | Canonical-host admitted route-tail evidence and justified blocking budgets |

## Retained acceptance from retired implementation tickets

- Operator/SDK owner: actual daemon and CLI `doctor`, `readiness`, generation
  inspection, explanation and metrics must agree on activation/generation/route
  truth under missing, divergent and failed states. Harness `cli_snapshots.json`
  is a runtime-surface supplement, not independent proof of every CLI command.
  Use current `generation-status` and `metrics`, not the old proposed aliases.
- Query/SDK/CLI owner: negative consumer tests must keep unsupported, ambiguous
  and wrong-route codes distinct and preserve typed supported alternatives,
  route hints and docs anchors; repair metadata never silently rewrites intent.
- Contract/SDK/CLI owner: exact preview window/highlight offsets and explanation
  sections need fresh current-wire consumer proof, including UTF-8 boundaries,
  truncation, empty/unavailable preview and score/provenance reconciliation.
  Web UI and confidence fields require an explicit consumer need before design.
- Benchmark integration owner: execute and validate the required dimensions on
  one bound source/input/host. Registration and the aggregate summary do not
  satisfy external comparison or quiet-host admission. Missing dimensions,
  failed artifacts and unprovisioned overlap cannot become a passing aggregate.

These are acceptance scopes, not a claim that the current implementation is
missing. Retire each after its own terminal/oracle result; do not recreate
implemented commands or DTOs. The active code-search and MISC ledgers own
cross-source, corpus/gold, physical regex and complete producer acceptance.
