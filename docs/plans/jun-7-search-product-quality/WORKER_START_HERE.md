# Worker Start Here

Status: `planned`

This packet does not add new DSL syntax.

It hardens product quality:

- ranking quality
- snippet usefulness
- explanation fidelity
- large corpus scale behavior
- p95 / p99 latency stability
- operator diagnosis surfaces
- ambiguity repair ergonomics
- UI-consumable result contracts

Do first:

1. confirm current correctness baseline is still green:
   - `python3 tools/benchmark/sourcegraph_parity.py --check`
   - `python3 tools/ci/lint/check-dsl-capability-truth.py`
2. read [SOURCE_TRUTH_MAP.md](SOURCE_TRUTH_MAP.md)
3. read [MEASUREMENT_MATRIX.md](MEASUREMENT_MATRIX.md)
4. read [COMMAND_AND_ARTIFACT_CONTRACT.md](COMMAND_AND_ARTIFACT_CONTRACT.md)
5. pick exactly one `J7Q-*` ticket
6. add the smallest blocking quality rail before changing behavior

Current packet facts:

- DSL capability is already broad on the live tree
- correctness hellgates are already landed in
  [../jun-7-verification-hellgates/rfc.md](../jun-7-verification-hellgates/rfc.md)
- this packet owns product quality, not feature parity
- this packet excludes semantic retrieval and hybrid fusion quality
- current gaps are quality gaps, not syntax gaps

Implementation bar:

- choose reproducible truth before tuning behavior
- separate lexical / symbol / structural / history-backed route policy whenever
  quality can diverge by route
- if the ticket changes ranking-facing behavior on overlapping surfaces, add a
  Sourcegraph lexical comparison subset
- prefer typed fields and structured metadata over display-only strings
- prefer read-only operator surfaces before any convenience mutation flow
- preserve fail-closed behavior; UX work may add repair metadata, not silent
  fallback
- if a change improves one metric by weakening another proof layer, reject it

Do treat these as blockers:

- relevance work without judged or explicitly labeled truth
- snippet quality work without offset/span assertions
- scale work without seeded corpus generation and tier manifests
- tail work without route-family budgets
- operator UX work without machine-readable CLI output
- UI contract work without wire and consumer proof
- semantic retrieval or hybrid fusion work mixed into this packet
- a competitive-quality claim without an external lexical overlap baseline

Preflight truth:

- `scripts/check-persona-target-policy.sh` 없음
- `scripts/cg-agent-session` 없음
- status stays `unverified`

Do not treat:

- correctness green as relevance green
- p50 green as tail green
- snippet field presence as snippet quality
- non-empty explanation text as explanation fidelity
- CLI existence as operator ergonomics closure
- one benchmark chart as scale proof
- a pretty rendered payload as a stable UI contract
