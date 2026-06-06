# J7-03 — Structural Route Hellgate

Status: `landed`

Goal:

- add a small runtime rail for structural mixed predicate siblings
- keep direct SG lexical `Phrase` / `Regex` sibling demotion explicit
- keep the reason explicit: this witness remains owner-local in lowering
  because the current SG grammar has no distinct runtime query surface for a
  direct lexical phrase/regex sibling

Owner seam:

- `crates/quanta-index-searchd-runtime/tests/e2e_structural_hellgate.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`

Covered families:

- bench-owned structural subset from `SCENARIOS`
- `file:contains(path:...)` mixed structural sibling
- `file:has.content(path:...)` mixed structural sibling
- `symbol:has.name(...)` mixed structural sibling

DoD:

- AND / OR / AND NOT exactness is pinned
- ambiguous symbol projection union is explicitly asserted
- direct SG lexical `Phrase` / `Regex` sibling demotion remains pinned at
  lowering, not mislabeled as a missing runtime typed-fail rail
