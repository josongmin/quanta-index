# Tickets - May 25 Lexical Enhancement Closeout

Parent doc: [../README.md](../README.md)

This ticket pack is a breaking-first implementation plan. It separates
implementation tickets from E2E proof tickets so parser/unit progress cannot be
mistaken for real runtime closure.

## 1. Implementation tickets

| Ticket | Priority | Title | Primary owner files | Blocks |
| --- | --- | --- | --- | --- |
| [LXE-00](LXE-00-truth-freeze-and-executable-matrix.md) | P0 | Truth freeze and executable matrix | `docs/plans`, `crates/*/tests` | all |
| [LXE-01](LXE-01-active-contract-and-dead-route-cleanup.md) | P0 | Active contract and dead-route cleanup | `crates/quanta-index-contract*`, SDK, search-plane | `LXE-02+` |
| [LXE-02](LXE-02-planner-authority-ir.md) | P0 | Planner authority IR | `quanta-index-core`, `quanta-index-lexical`, `search-plane` | `LXE-03+` |
| [LXE-03](LXE-03-lexical-filter-execution.md) | P0 | Lexical filter execution | `quanta-index-lexical`, `search-plane/lowering.rs` | `E2E-01`, `E2E-02` |
| [LXE-04](LXE-04-regex-trigram-real-execution.md) | P0 | Regex and trigram real execution | `quanta-index-lq-trigram`, `quanta-index-lq-regex`, `quanta-index-lexical` | `E2E-01`, `E2E-07` |
| [LXE-05](LXE-05-phrase-position-real-execution.md) | P1 | Phrase and position real execution | `quanta-index-lq-positions`, `quanta-index-lexical` | `E2E-01` |
| [LXE-06](LXE-06-symbol-select-type-execution.md) | P1 | Symbol, select, and type execution | `quanta-index-lq-symbol`, `quanta-index-lexical`, `search-plane` | `E2E-01`, `E2E-02` |
| [LXE-07](LXE-07-semantic-hybrid-planner-provenance.md) | P0 | Semantic/hybrid planner provenance | `search-plane`, `core/domains/semantic`, `core/domains/hybrid` | `E2E-03` |
| [LXE-08](LXE-08-history-live-integration.md) | P1 | History live integration | `contract/results`, `lq-history`, `search-plane` | `E2E-04` |
| [LXE-09](LXE-09-structural-live-integration.md) | P1 | Structural live integration | `lq-structural`, `search-plane`, `contract/results` | `E2E-04` |
| [LXE-10](LXE-10-observability-and-bridge-sink.md) | P1 | Observability and bridge sink | `lq-bridge`, `results`, `search-plane` | `E2E-05+` |

## 2. E2E tickets

| Ticket | Priority | Title | Proof target |
| --- | --- | --- | --- |
| [E2E-00](E2E-00-live-dsl-matrix-harness.md) | P0 | Live DSL matrix harness | every scenario writes to real indexes before querying |
| [E2E-01](E2E-01-lexical-full-fidelity-e2e.md) | P0 | Lexical full-fidelity E2E | LQ content/path/filter/regex/phrase/symbol rows |
| [E2E-02](E2E-02-sourcegraph-parity-e2e.md) | P0 | Sourcegraph parity E2E | SG syntax equals equivalent LQ or typed rejection |
| [E2E-03](E2E-03-semantic-hybrid-e2e.md) | P0 | Semantic/hybrid E2E | lexical scope materially changes semantic/hybrid results |
| [E2E-04](E2E-04-history-structural-e2e.md) | P1 | History/structural E2E | typed unavailable/not-ready boundaries and fixture-backed paths |
| [E2E-05](E2E-05-restart-replay-determinism-e2e.md) | P1 | Restart/replay determinism | reopened indexes return identical ordering/explanations |
| [E2E-06](E2E-06-full-corpus-real-engine-ci.md) | P0 | Full corpus real-engine CI | corpus rows are real runtime tests, not parser-only tests |
| [E2E-07](E2E-07-performance-and-chaos.md) | P1 | Performance and chaos | regex/trigram/fanout/cancellation budget behavior |

## 3. Dependency order

1. `LXE-00` freezes the matrix and expected failures.
2. `LXE-01` and `LXE-02` land the contract and planner ownership base.
3. `E2E-00` lands the harness and may run expected-failing rows.
4. `LXE-03`, `LXE-04`, `LXE-05`, `LXE-06` land in parallel only if owner
   files are disjoint.
5. `E2E-01` and `E2E-02` become green after lexical execution closure.
6. `LXE-07`, `LXE-08`, `LXE-09`, `LXE-10` wire adjacent surfaces.
7. `E2E-03`..`E2E-07` close runtime proof and CI promotion.

## 4. PR slicing

Recommended slices:

1. `LXE-00` + `E2E-00` skeleton with expected failures.
2. `LXE-01` + contract round-trip tests.
3. `LXE-02` + planner trace unit tests.
4. `LXE-03` + `LXE-04` if regex/filter owner edits stay isolated.
5. `LXE-05` + `LXE-06`.
6. `LXE-07`.
7. `LXE-08` + `LXE-09`.
8. `LXE-10` + `E2E-05`..`E2E-07`.

Do not merge implementation tickets without at least one owning unit test and
one E2E row either green or explicitly expected-failing with a linked ticket.

## 5. Current closeout refresh

As of `2026-05-27`, the live-source closeout refresh used:

```bash
cargo check -p quanta-index-contract
cargo check -p quanta-index-sdk
cargo test -p quanta-index-searchd-runtime --test repo_map_end_to_end
cargo test -p quanta-index-sdk --lib
cargo test -p quanta-index-searchd-runtime
```

This refresh kept the active lexical/Sourcegraph/structural closeout rail
green, including the current `e2e_dual_syntax_lowering_parity` owner test.
Follow-on owner rails now also cover `E2E-03` semantic/hybrid runtime proof,
`E2E-05` restart/replay, `E2E-06` full-corpus runtime execution, and the
currently-landed `E2E-07` boundedness owner rail on the same current tree.
The broader matrix residue has now also been retired on the same current tree,
so the May-25 pack is closed at the current live-source proof bar. This is not
yet a workspace-wide `clippy` / `cargo test --workspace` claim.

## 6. Ticket quality bar

Every ticket in this directory must answer all of the following without reading
another plan document:

- Which files are allowed to change?
- What must change in each file?
- What test rail proves the change locally?
- Which E2E rail proves persisted-runtime behavior?
- What exact condition marks the ticket done?

If any answer is missing, the ticket is incomplete and should be edited before
implementation starts.
