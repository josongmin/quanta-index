# Measurement Matrix

Status: `planned`

This file freezes what each quality dimension is allowed to claim.

| dimension | owner ticket | primary truth source | blocking rail | advisory rail | required artifact | allowed final wording | forbidden shortcut |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `relevance` | `J7Q-01` | judged corpus + Sourcegraph lexical overlap subset | `just rust-verify-quality-relevance` | comparison summary drilldowns | `artifacts/search-quality/relevance/latest/summary.json` | `relevance rail green` | score presence, anecdotal query, bench latency |
| `snippet_explain` | `J7Q-02` | snippet/explain golden corpus | `just rust-verify-quality-snippet` | corpus diff snapshots | `artifacts/search-quality/snippet/latest/summary.json` | `snippet/explain rail green` | field presence, non-empty summary |
| `scale` | `J7Q-03` | seeded tier manifest + measured runs | `just rust-verify-quality-scale` | exploratory tier expansion | `artifacts/search-quality/scale/latest/summary.json` | `scale tier rail green for declared tiers` | toy fixtures, one-off local repo |
| `tail` | `J7Q-04` | route-family p95/p99 artifacts | `just rust-verify-quality-tail` | p50 compare drilldowns | `artifacts/search-quality/tail/latest/summary.json` | `tail rail green for declared route budgets` | p50-only closeout, global threshold |
| `ops` | `J7Q-05` | CLI JSON snapshots + smoke tests | `just rust-verify-quality-ops` | manual operator walkthrough | `artifacts/search-quality/ops/latest/summary.json` | `operator diagnosis rail green` | human-readable only output |
| `ambiguity` | `J7Q-06` | typed error payload snapshots | `just rust-verify-quality-ambiguity` | CLI rendering examples | `artifacts/search-quality/ambiguity/latest/summary.json` | `repairability rail green` | silent rewrite, generic help blob |
| `ui_contract` | `J7Q-07` | DTO / SDK / CLI contract snapshots | `just rust-verify-quality-ui` | rendered consumer examples | `artifacts/search-quality/ui/latest/summary.json` | `UI contract rail green` | opaque blobs, free-form parsing |
| `integration` | `J7Q-08` | command wiring + doc sync | `just rust-verify-quality-all` | per-dimension reruns | `artifacts/search-quality/integration/latest/summary.json` | `quality command integration green` | one command claiming every dimension without split |

Mandatory wording rules:

- `green` without dimension is forbidden
- `competitive` requires the `relevance` row plus Sourcegraph lexical overlap artifact
- `best-in-class` is forbidden unless the overlap subset, judged corpus, and route-family split all exist

Not done if:

- a closeout cannot point to one row in this table
- a claimed artifact path is missing or ad hoc
