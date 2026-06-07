# Command And Artifact Contract

Status: `planned`

This file freezes future command names and artifact paths for this packet.

## Stable Command Names

- `just rust-verify-quality-relevance`
- `just rust-verify-quality-snippet`
- `just rust-verify-quality-scale`
- `just rust-verify-quality-tail`
- `just rust-verify-quality-ops`
- `just rust-verify-quality-ambiguity`
- `just rust-verify-quality-ui`
- `just rust-verify-quality-all`

Rules:

- each command must prove exactly one primary quality dimension, except
  `rust-verify-quality-all`
- `rust-verify-quality-all` may orchestrate, but it must print sub-rail results
  without erasing dimension boundaries
- `just rust-verify-quality-all` is not a substitute for per-dimension closeout

## Artifact Root

- root: `artifacts/search-quality/`

Per-dimension canonical locations:

- relevance:
  - `artifacts/search-quality/relevance/latest/summary.json`
  - `artifacts/search-quality/relevance/latest/sourcegraph-overlap.json`
  - `artifacts/search-quality/relevance/latest/query_judgments.json`
- snippet:
  - `artifacts/search-quality/snippet/latest/summary.json`
  - `artifacts/search-quality/snippet/latest/golden_windows.json`
- scale:
  - `artifacts/search-quality/scale/latest/summary.json`
  - `artifacts/search-quality/scale/latest/tier_manifest.json`
- tail:
  - `artifacts/search-quality/tail/latest/summary.json`
  - `artifacts/search-quality/tail/latest/route_budgets.json`
- ops:
  - `artifacts/search-quality/ops/latest/summary.json`
  - `artifacts/search-quality/ops/latest/cli_snapshots.json`
- ambiguity:
  - `artifacts/search-quality/ambiguity/latest/summary.json`
  - `artifacts/search-quality/ambiguity/latest/error_payloads.json`
- ui:
  - `artifacts/search-quality/ui/latest/summary.json`
  - `artifacts/search-quality/ui/latest/contract_snapshots.json`
- integration:
  - `artifacts/search-quality/integration/latest/summary.json`

## Sourcegraph Lexical Overlap Contract

Used only for overlapping non-semantic query families.

Minimum overlap buckets:

- keyword
- phrase
- regex
- path-constrained content
- repo metadata
- symbol name

Each overlap artifact must record:

- query family
- exact query text
- capture date
- Sourcegraph surface used
- quanta-index result ordering
- Sourcegraph result ordering
- pass/fail or explicit gap note

Not done if:

- command names drift by ticket
- artifacts land outside the canonical root without an explicit packet update
- external overlap evidence is not persisted
