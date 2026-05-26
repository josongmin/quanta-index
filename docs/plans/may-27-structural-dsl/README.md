# May 27 Structural DSL Final Plan

Status: `final-planning-packet`
Date: `2026-05-27`
Scope: structural DSL semantics beyond the shipped truthful subset.

This packet is the planning SSOT for the next structural DSL wave.
Historical tickets stay in their original paths for evidence, but their current
ownership is re-anchored here through [tickets/HISTORICAL-MAP.md](tickets/HISTORICAL-MAP.md).

---

## 1. Objective

Close the remaining structural DSL semantics without reopening the already
shipped authority model.

The target is not "broader syntax at any cost". The target is:

- deeper structural semantics on producer-authored parse-tree authority
- Sourcegraph structural frontdoor growth only after native semantics exist
- fail-closed behavior for every non-executable shape
- deterministic candidate/binding behavior under composition
- explicit proof and bounded-label observability for the live structural path

## 2. Current live truth

Current source-backed boundary:

- native structural execution already ships the truthful subset:
  - root-kind exact
  - root capture
  - root-kind plus capture
  - ordered direct-child tree-walk
  - variadic sibling capture / wildcard skip
  - `where`
  - `inside`
  - `outside`
- structural requests still require exactly one top-level structural leaf at the
  dispatcher boundary
- structural route executes only `repo:`, `file:`, and `lang:` filters
- Sourcegraph structural route ships only one quoted/keyword body plus the same
  executable filter subset
- unsupported Sourcegraph structural regex bodies and boolean composition still
  fail typed before execution
- typed structural holes remain deferred
- live ship language set remains Rust / Python / TypeScript / JavaScript / Go;
  Java is still follow-up, and C / C++ / Ruby remain post-ship
- `match { ... } + into:codeql` remains deferred

This packet assumes that shipped subset stays closed. It does not reopen
root-only debates, text fallback, or search-side reparsing.

## 3. Direction Lock

The chosen direction is `authority-preserving structural semantics deepening`.

Required:

- keep producer-authored `ParseTreeRecord` authority
- keep structural results disjoint from lexical candidates
- keep unsupported semantics typed and explicit
- widen SG structural only after native structural owns the same semantics

Forbidden:

- search-side source reparsing
- regex/text heuristics presented as structural execution
- silent fallback from structural to lexical
- opening filter breadth whose truth source lives in history/runtime/ACL systems

## 4. In Scope

- native structural boolean composition over multiple `match { ... }` leaves
- typed structural hole semantics on the shipped language set
- richer Sourcegraph structural lowering after native semantics are live
- language set expansion after semantics stabilize
- structural `into:codeql` bridge follow-on
- structural proof and bounded-label observability closeout

## 5. Explicitly Out of Scope

- history/runtime filter breadth such as `author:`, `rev:`, `dirty:`,
  `visibility:`, `context:`
- lexical/structural fused ranking
- semantic/hybrid query redesign
- search-side tree-sitter or git access
- exporter/collector productization beyond a typed bounded-label sink

## 6. Dependency Order

Execution order is fixed:

1. `SDL-01` native structural boolean composition
2. `SDL-02` typed hole semantics
3. `SDL-03` Sourcegraph structural v2 lowering
4. `SDL-04` language set expansion
5. `SDL-05` structural CodeQL bridge
6. `SDL-E2E-01` proof and observability closeout

Rules:

- `SDL-03` cannot land ahead of `SDL-01` and the relevant `SDL-02` surface.
- `SDL-04` cannot widen language claims before the matcher semantics and proof
  matrix are stable.
- `SDL-05` is downstream of the native semantics because candidate export must
  preserve the actual structural result model.
- `SDL-E2E-01` starts early for expected-failing rows, but it closes only after
  the owning behavior tickets land.

## 7. Ticket Pack

- [tickets/INDEX.md](tickets/INDEX.md)
- [tickets/HISTORICAL-MAP.md](tickets/HISTORICAL-MAP.md)
- [tickets/SDL-01-structural-boolean-composition.md](tickets/SDL-01-structural-boolean-composition.md)
- [tickets/SDL-02-typed-hole-semantics.md](tickets/SDL-02-typed-hole-semantics.md)
- [tickets/SDL-03-sourcegraph-structural-v2-lowering.md](tickets/SDL-03-sourcegraph-structural-v2-lowering.md)
- [tickets/SDL-04-language-set-expansion.md](tickets/SDL-04-language-set-expansion.md)
- [tickets/SDL-05-structural-codeql-bridge.md](tickets/SDL-05-structural-codeql-bridge.md)
- [tickets/SDL-E2E-01-structural-proof-and-observability.md](tickets/SDL-E2E-01-structural-proof-and-observability.md)

## 8. Exit Criteria

This packet is complete only when all are true:

1. native structural route no longer hard-rejects structural-only
   `AND` / `OR` / bounded `NOT` composition
2. composition semantics are deterministic on candidate identity and binding
   merge behavior
3. typed holes execute against a closed per-language admissibility table; no
   "accept then erase" behavior remains
4. Sourcegraph structural frontdoor exposes only semantics that native
   structural already executes
5. language claims are backed by producer handoff plus runtime proof, not just
   parser acceptance
6. `match { ... } + into:codeql` either executes through a typed bridge path or
   remains explicitly deferred in this packet
7. structural proof covers happy path, typed negatives, parity, plan limits,
   cancellation/isolation, and deterministic ordering
8. structural observability emits bounded labels only; raw pattern text never
   reaches metric labels

## 9. Non-Negotiable Failure Modes

- broadening parser/translator surface before executor truth exists
- defining composition in prose without candidate/binding merge rules
- widening language claims without producer parse-tree agreement
- overloading `STR_INVALID_REQUEST` when a new first-class structural error
  family is required
- treating the shipped truthful subset as proof that full structural semantics
  are already done
