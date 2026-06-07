# Jun 7 Search Product Quality

Status: `planned`

Purpose:

- move beyond DSL correctness into search product quality
- separate relevance, snippet quality, scale, tail, ops UX, and UI contract
  work from feature-coverage packets
- keep this packet scoped to the non-semantic code-search stack
- keep `jun-7-verification-hellgates` focused on correctness verification only

Read order:

1. [WORKER_START_HERE.md](WORKER_START_HERE.md)
2. [NO-GO-RULES.md](NO-GO-RULES.md)
3. [SOURCE_TRUTH_MAP.md](SOURCE_TRUTH_MAP.md)
4. [MEASUREMENT_MATRIX.md](MEASUREMENT_MATRIX.md)
5. [COMMAND_AND_ARTIFACT_CONTRACT.md](COMMAND_AND_ARTIFACT_CONTRACT.md)
6. [DUMB_LLM_EXECUTION_CHECKLIST.md](DUMB_LLM_EXECUTION_CHECKLIST.md)
7. [rfc.md](rfc.md)
8. [tickets-wave2/INDEX.md](tickets-wave2/INDEX.md)

Current packet target:

- ranking quality
- snippet / explain quality
- large corpus scale behavior
- latency tail hardening
- operator ergonomics
- ambiguous intent handling
- UI / UX contract surface

External competitive floor:

- for overlapping non-semantic code-search surfaces, Sourcegraph lexical
  behavior is the minimum external baseline
- this applies to already-shipped overlapping families only
- it does not require feature-matching on surfaces this repo does not ship

This packet is intentionally narrow:

- it does not reopen Sourcegraph DSL coverage
- it does not reopen permanent structural unsupported verdicts
- it does not replace `jun-7-verification-hellgates`
- it does not own semantic retrieval or hybrid fusion quality
- it does not claim learned ranking or product-web frontend work

Target bar:

- deterministic and reproducible measurements
- route-family-specific quality policy instead of one global threshold
- typed contracts instead of UI-only conventions
- labeled or seeded truth sources instead of ad hoc examples
- operator-visible diagnostics instead of repo-internal spelunking
- fail-closed query behavior preserved even when UX improves

Implementation defaults:

- offline judged relevance corpus before any ranking rewrite
- lexical / symbol / structural / history-backed result families only
- explicit external overlap suite against Sourcegraph lexical behavior for
  ranking-facing work
- snippet and explain quality proven from structured spans and provenance, not
  string presence
- scale claims backed by seeded synthetic corpora with declared tier manifests
- tail claims backed by route-aware `p95` / `p99` budgets
- operator UX backed by real `searchctl` read surfaces
- UI contract work backed by versioned DTO changes and consumer proof

Current preflight truth:

- still repo-global `unverified`
- `scripts/check-persona-target-policy.sh` 없음
- `scripts/cg-agent-session` 없음

Backlinks:

- DSL capability inventory: [../../analysis/jun-4-dsl-capabilty.md](../../analysis/jun-4-dsl-capabilty.md)
- Sourcegraph feature closeout: [../jun-6-sourcegraph-expansion/rfc.md](../jun-6-sourcegraph-expansion/rfc.md)
- verification architecture: [../jun-7-verification-hellgates/rfc.md](../jun-7-verification-hellgates/rfc.md)
