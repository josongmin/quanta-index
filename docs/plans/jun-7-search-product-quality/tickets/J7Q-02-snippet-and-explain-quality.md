# J7Q-02 — Snippet And Explain Quality

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Goal

Raise snippet and explanation proof from “field exists” to “human-useful and
internally consistent”.

## Current Code Fact

- snippet fields are shipped on candidate surfaces
- explanation wire and tests already exist
- current assertions still skew toward presence and simple substring checks

## Owner Seam

- snippet derivation
- explanation payload contract
- runtime explain surface
- corpus snippet assertions

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/results/explanation.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/explain.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`

## Preferred Implementation Direction

- snippet quality should be asserted via hit-centered windows, offsets, and
  highlight spans
- explanation quality should be asserted via sectioned provenance from actual
  planner and runtime stages
- route-specific explanation should explain why a result ranked, not only that
  it existed
- long snippets should truncate deterministically around the most informative hit

## Layer Boundary Clarification

- this ticket owns human usefulness of emitted payloads
- it does not own core relevance ordering metrics from `J7Q-01`

## Concrete Work Items

1. Add snippet golden assertions for phrase, regex, multi-hit, and long-line
   cases.
2. Add explain assertions for planner stages, engine order, and contribution
   rows.
3. Assert route-specific explanation rationale, not just non-empty summaries.
4. Keep snippet and explanation quality separate from relevance metrics.

## First Increment

- tighten existing explain rail before adding new fields

## Red Rail To Pin First

```bash
./scripts/cargow test -p quanta-index-searchd-runtime --test explain -- --nocapture
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture
```

## Worker First Commands

```bash
rg -n "snippet|explanation|planner_trace|summary|contribution" crates/quanta-index-searchd-runtime crates/quanta-index-contract crates/quanta-index-lexical -S
sed -n '1,260p' crates/quanta-index-searchd-runtime/tests/explain.rs
sed -n '1660,1795p' crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs
```

## No-Go

- do not treat summary non-empty as sufficient
- do not weaken snippet provenance into UI-only formatting heuristics
- do not pull semantic retrieval or hybrid fusion explanation into this ticket
- do not let snippet quality collapse to “contains the needle somewhere”

## Reviewer Rejection Checklist

- reject if snippet “quality” is still asserted with substring presence only
- reject if explanation text is manually composed without stable provenance
- reject if the payload gets prettier but less deterministic

## DoD

- degraded snippets or low-information explanations fail dedicated rails

## Not Done If

- snippets are still only loosely asserted
- explanation quality is still mostly “string not empty”
