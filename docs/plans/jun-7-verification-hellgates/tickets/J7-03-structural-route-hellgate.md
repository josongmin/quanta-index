# J7-03 — Structural Route Hellgate

> Archive status: `Historical program record`. Current architecture: [JUN-08-001](../../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `landed`

Goal:

- add a small runtime rail for structural mixed predicate siblings
- keep direct SG lexical `Phrase` / `Regex` sibling demotion explicit
- prove the demoted execution path at runtime too:
  - quoted SG structural token executes as a structural body
  - slash-delimited SG structural token executes as a structural regex body
- keep the sibling verdict explicit: the "not a distinct lexical sibling
  surface" witness remains owner-local in lowering because the current SG
  grammar has no separate runtime query surface for a direct lexical
  phrase/regex sibling

Owner seam:

- `crates/quanta-index-searchd-runtime/tests/e2e_structural_hellgate.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`

Covered families:

- bench-owned structural subset from `SCENARIOS`
- direct SG quoted body demotion
- direct SG slash-regex body demotion
- `file:contains(path:...)` mixed structural sibling
- `file:has.content(path:...)` mixed structural sibling
- `symbol:has.name(...)` mixed structural sibling

DoD:

- AND / OR / AND NOT exactness is pinned
- ambiguous symbol projection union is explicitly asserted
- direct SG quoted/slash token execution is pinned in runtime hellgate and
  shared front-door rails
- direct SG lexical `Phrase` / `Regex` sibling non-preservation remains pinned
  at lowering, not mislabeled as a missing runtime typed-fail rail
