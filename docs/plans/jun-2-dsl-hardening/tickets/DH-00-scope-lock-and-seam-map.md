# DH-00 Scope Lock and Seam Map

Parent packet: [../README.md](../README.md)

Status: `planned`

## Objective

Freeze the post-closeout hardening scope so shipped-surface correctness work
does not get mixed with optional DSL widening or historical closeout.

## Current Source Truth

- `jun-2-dsl-final-cut` is already closed and remains the shipped closeout
- `jun-2-dsl-advanced` already owns optional widening and benchmark/shadow
  claims for non-shipped surfaces
- the open residue from the current audit is on shipped surfaces only:
  runtime catalog authority integrity, runtime metadata semantics/execution,
  and predicate proof symmetry

## Current Code Pointers

- closed packet:
  `docs/plans/jun-2-dsl-final-cut/README.md`
- widening packet:
  `docs/plans/jun-2-dsl-advanced/README.md`
- hardening seams this ticket maps:
  `crates/quanta-index-search-plane/src/readiness.rs`
  `crates/quanta-index-search-plane/src/query_dispatcher.rs`
  `crates/quanta-index-lexical/src/lib.rs`

## 핵심 로직

- this packet is for hardening shipped behavior, not for widening syntax
- every hardening lane must point at one owning seam and one owning red rail
- no ticket may blur closeout truth, widening truth, and hardening truth

## 건드릴 파일

- `docs/plans/jun-2-dsl-hardening/README.md`
- `docs/plans/jun-2-dsl-hardening/tickets/*.md`
- links in sibling packets when they need a pointer to this packet:
  `docs/plans/jun-2-dsl-final-cut/README.md`
  `docs/plans/jun-2-dsl-advanced/README.md`

## 생성 가능 파일

- none beyond this packet and its tickets

## 건드리지 말 것

- the closeout verdict itself
- widening ticket scope in `jun-2-dsl-advanced`
- proof ledger semantics beyond pointer/truth-sync wording

## TODO

- [ ] freeze the hardening-only seam map
- [ ] freeze parallelization boundaries between catalog state, query semantics,
  and proof-only work
- [ ] freeze claim discipline so no ticket markets hardening as widening

## Concrete First Increment

The first PR for this ticket should do only this:

1. create the packet README and ticket index
2. define the lane map:
   `DH-01` authority integrity, `DH-02` semantics/pushdown, `DH-03` proof symmetry
3. add pointer lines in `final-cut` and `advanced` so future work does not land
   in the wrong packet

Do not change product behavior in this ticket.

## Implementation Steps

1. enumerate the hardening-only residue against live code
2. assign each residue to one owner seam and one owner rail
3. freeze the recommended start order and explicit non-goals
4. add sibling-packet pointers so the packet boundary stays stable

## Dependency / Import Constraints

- no product-code imports
- no fixture churn in this ticket
- no new benchmark claims

## Red Rails First

- packet/doc truth spot-check:
  `rg -n "closeout|advanced|hardening|dirty:only|stale:|batch_digest" docs/plans/jun-2-dsl-hardening docs/plans/jun-2-dsl-final-cut docs/plans/jun-2-dsl-advanced`

## NOT TODO

- no widening semantics
- no runtime row additions just to make the packet look active
- no reopening of historical residue that is outside the current audit bundle

## Test Plan

- docs-only spot check against the live owner seams

## DoD

- shipped-surface hardening has its own packet
- widening and closeout packets no longer compete for the same residue
- every later `DH-*` ticket inherits one stable owner seam and red rail

## Failure Modes

- closeout packet is reopened without a product regression
- widening ticket absorbs shipped-surface hardening residue
- future audit work lands in the wrong packet and loses ownership clarity
