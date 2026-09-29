# CS-ENG-04 — Regex memory policy

Status: `DEFERRED` for an exact request-wide allocation ceiling. No measured
regex memory overrun, numerical ceiling, or external requirement for exact
`Layout`-byte accounting is recorded. This status does not claim that the
current limits prove a request-wide heap or process RSS ceiling.

## Implemented protection

- The shared regex executor and indexed Tantivy FST scope path reject patterns
  over 64 KiB before parsing. The executor has a 10 MiB per-NFA size limit
  and a 2 MiB lazy-DFA cache setting. Tantivy FST has its own state limit.
- The executor validates one AST, retains the original HIR for literal
  extraction, erases unobserved captures for matching, and compiles the
  execution HIR without reparsing a rendered pattern.
- Structural dispatch limits distinct `where` patterns across a Boolean
  request; direct producer calls retain a block-local limit. Filters compile
  before empty-result short circuits. Syntax, resource refusal, and engine
  failure remain distinct errors.
- Selected preview has logical work/collection limits and preserves selected
  hits when optional range reconstruction refuses. The source and preview
  contract is recorded in
  [SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md).

These are per-input, per-engine, or logical limits. They do not add up to an
exact request-wide physical allocation bound.

## Decision and boundary

The former proposal required one request lifetime ledger to authorize every
regex-owned `Layout` allocation *before* it happens, including parser AST/HIR,
literal extraction, compiler temporaries, retained engines, first-search
scratch, cache growth, and the separate Tantivy FST compiler. The proposal
also required typed refusal, correct release on failure and growth, and all
indexed, manual, preview, and structural routes to share the ledger.

The pinned `regex-syntax 0.8.11`, `regex-automata 0.4.14`, and
`tantivy-fst 0.5.0` APIs do not expose fallible caller-controlled allocation
at those sites. For example, FST DFA construction grows `Vec` and `HashMap`
before its state refusal. The regex meta engine reports approximate retained
memory and exposes per-engine limits, not an aggregate pre-allocation hook.
An outer counter, fixed up-front charge, or post-build `memory_usage()` check
cannot establish the proposed exact ceiling.

Therefore the exact cap is not an implementation task under the current
dependency contract. The experimental explicit-cache/fallible-`verify` API
migration was reverted: it changed every caller and serialized shared
searches without admitting the dependency's cache allocations. The retained
HIR compilation is a useful independent reduction in duplicate parser work.

No process worker is selected. An OS memory envelope would constrain a whole
worker, not exact regex-owned allocation; it needs a separate contract and
read-only pinned-generation/IPC/death-handling design. Neither a dependency
fork nor worker isolation should be presented as already implemented.

## Reopen rule

Reopen implementation only if an external contract requires a specified
request-wide byte ceiling, or actual supported-host measurements show a
regex-driven memory/SLO breach under the intended concurrency. First record
the workload, input bounds, concurrency, peak process RSS, failure behavior,
and numerical acceptance threshold. RSS measurements characterize risk but
do not prove exact regex allocation accounting.

If the exact `Layout` claim is selected, first build a controlled pinned
dependency slice covering parse → compile → first search → cache growth →
Tantivy FST construction → drop under a tiny shared allowance. Every
allocation must refuse before it occurs and release its charge on failure.
Only after that slice passes should `RequestBudgetV1` and all live routes gain
an owner-required allocation API. If the slice fails, do not ship a
ledger-only approximation as a hard cap.

Verification of the current protection is owner-local; exact request-wide
allocation admission and supported-host resource qualification are
`NOT_RUN`. Historical plans and test counts remain recoverable in Git history.
Final integration ownership is
[CS-INT-01](CS-INT-01-integration-and-qualification.md).

References: [regex-automata 0.4.14 memory contract](https://docs.rs/regex-automata/0.4.14/regex_automata/meta/struct.Regex.html#method.memory_usage),
[RE2 approximate memory policy](https://github.com/google/re2/blob/main/re2/re2.h).
