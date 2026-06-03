# Jun 2 DSL Advanced

Status: `planned`
Date: `2026-06-02`
Scope: post-closeout DSL widening beyond the shipped `jun-2-dsl-final-cut` surface

This packet starts **after** [../jun-2-dsl-final-cut/README.md](../jun-2-dsl-final-cut/README.md).
It does not reopen the closeout verdict. It tracks optional widening work that is
currently typed-fail-closed or intentionally subset-limited.

Shipped-surface correctness and execution-cost hardening lives in
[../jun-2-dsl-hardening/README.md](../jun-2-dsl-hardening/README.md), not here.

---

## 1. Scope Lock

This packet owns only three widening families:

- predicate subset widening beyond the current shipped allowlist
- Sourcegraph structural mixed-domain widening beyond the current narrow subset
- truth-generation and benchmark/shadow bar needed before calling the widened
  surface “advanced” rather than just “broader”

Explicitly excluded:

- empty-query runtime semantics
- bridge-packet carriers (`into:codeql`, `scope:results`, `with:lexical`) as runtime rows
- semantic/hybrid redesign outside the existing DSL owner rails
- request-time git or ad-hoc fallback authorities

## 2. Current Source Truth

- closeout is done; executable DSL residue is gone on the shipped surface
- the main typed-fail-closed widening seam is predicate names / argument shapes
  outside the shipped subset
- the main subset-limited bridge seam is SG structural mixed-domain lowering:
  native executes broader mixed shapes than Sourcegraph lowering currently
  mirrors
- the current docs/ledger are accurate enough for closeout, but widening would
  benefit from generated capability truth instead of hand-maintained prose

## 2.1 Current Code Pointers

- packet truth:
  `docs/plans/jun-2-dsl-final-cut/README.md`,
  `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`,
  `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`
- predicate widening seam:
  `crates/quanta-index-lexical/src/lib.rs`
  `predicate_content_leaf`, `repo_has_file_constraint`,
  `lower_predicate_for_boolean_scope`, `prepare_predicate_plan`;
  planner seam in `crates/quanta-index-lexical/src/planner.rs`
  `plan_predicate_leaf`, `validate_repo_has_file_args`
- predicate proof rails:
  `crates/quanta-index-lexical/tests/tantivy_smoke.rs`,
  `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`,
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`,
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- SG structural widening seam:
  `crates/quanta-index-search-plane/src/lowering.rs`
  `lower_sourcegraph_structural_query_text`,
  `lower_sourcegraph_structural_shape`,
  `rewrite_sourcegraph_structural_expr`,
  `rewrite_sourcegraph_structural_children`;
  dispatch hook in `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- SG proof rails:
  `crates/quanta-index-lq-bridge/tests/golden_bridge.rs`,
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`,
  `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- benchmark / shadow truth:
  `crates/quanta-index-lq-norm/benches/pipeline.rs`,
  `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`,
  `crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs`

## 3. Ticket Lanes

| ticket | status | concrete first increment | red rail first |
| --- | --- | --- | --- |
| [ADV-00](tickets/ADV-00-scope-lock-and-admission-bar.md) | done | admission table frozen in §9 | packet/doc truth spot-check |
| [ADV-01](tickets/ADV-01-predicate-capability-registry.md) | done (registry + `lang:` family) | registry only, then one widened predicate family | `tantivy_smoke` before any widen |
| [ADV-02](tickets/ADV-02-sourcegraph-structural-mixed-domain-widening.md) | done (matrix + `RawString` + `Predicate`) | legality table, then `RawString`, then `Predicate` | search-plane lowering tests |
| [ADV-03](tickets/ADV-03-sourcegraph-scoped-filter-or-widening.md) | done | legality verdict frozen | rejected scoped-`OR` owner rail |
| [ADV-04](tickets/ADV-04-generated-proof-truth-and-benchmark-bar.md) | done | checker landed (`check-dsl-capability-truth.py`) | docs-vs-code drift checker |
| [HISTORICAL-MAP](tickets/HISTORICAL-MAP.md) | reference | lineage only | n/a |

## 4. Sequencing

Execution order is fixed:

1. `ADV-00` scope/admission bar
2. `ADV-01` predicate registry + widening substrate
3. `ADV-02` SG structural mixed-domain widening for raw/predicate siblings
4. `ADV-03` SG scoped-filter under mixed `OR` widening
5. `ADV-04` generated truth + shadow/benchmark acceptance

Rules:

- `ADV-02` and `ADV-03` cannot invent semantics that native execution does not already own
- `ADV-03` cannot start until `ADV-02` freezes the SG legality matrix
- `ADV-04` is required before any “advanced” or “SOTA++” claim

Cross-ticket dependencies:

- `ADV-01` owns predicate capability truth that `ADV-04` will later check or generate
- `ADV-02` owns SG legality truth that `ADV-03` may widen only after the matrix is frozen
- `ADV-03` must either legalize one scoped-`OR` family with parity proof or freeze permanent typed-fail rows
- `ADV-04` consumes `ADV-01` and `ADV-02` code-owned metadata; it must not invent a docs-only truth source

## 4.1 Concrete First Increment Order

If a new engineer starts cold, the recommended order is:

1. `ADV-00`: freeze acceptance language and benchmark/shadow bar
2. `ADV-01`: land the registry without widening semantics first
3. `ADV-01`: widen exactly one predicate family as the pilot increment
4. `ADV-02`: widen SG `RawString` sibling only
5. `ADV-02`: widen SG `Predicate` sibling only
6. `ADV-03`: decide whether scoped-filter-under-`OR` is widenable or permanently rejected
7. `ADV-04`: move doc truth from hand-maintained to generated/mechanically checked

Do not start with `ADV-03`. It depends on the legality matrix work from `ADV-02`.
Do not combine registry+widening or `RawString`+`Predicate` in the same first PR.

## 5. Program-Level Red Rails First

Run these first when work starts on the corresponding lane:

- `ADV-00`: `rg -n "advanced|widen|proof|benchmark|shadow" docs/plans/jun-2-dsl-advanced docs/plans/may-25-lexical-enhancement docs/plans/jun-2-dsl-final-cut`
- `ADV-01`: `./scripts/cargow test -p quanta-index-lexical --test tantivy_smoke -- --nocapture`
- `ADV-02`: `./scripts/cargow test -p quanta-index-search-plane --lib sourcegraph_structural_route_preserves_lexical_keyword_in_mixed_boolean_or -- --nocapture`
- `ADV-03`: `./scripts/cargow test -p quanta-index-search-plane --lib sourcegraph_structural_route_rejects_repo_scoped_filter_under_mixed_or -- --nocapture`
- `ADV-04`: run the new checker first, then `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

Note: `e2e_dual_syntax_lowering_parity` is not the default daemon proof rail. Any widening ticket that claims SG/native parity should invoke it explicitly.

## 6. Program-Level Deliverables

This packet is not complete until it ships all of these:

1. one code-owned predicate capability source
2. one code-owned SG legality source
3. one deterministic checker or generator that fails when docs drift from those sources
4. one benchmark/shadow claim template that widened tickets must fill before using an “advanced” label

## 7. Program-Level DoD

This packet is complete only when all are true:

1. predicate widening is registry-driven rather than hardcoded one-off matches
2. unsupported predicate shapes still typed-fail with stable diagnostics
3. SG structural mixed-domain subset matches native semantics for the widened shapes
4. SG scoped filters under mixed `OR` have explicit legality semantics and parity proof
5. proof ledger / matrix truth is generated or mechanically checked from code-owned capability data
6. widened surfaces have shadow or benchmark evidence, not just correctness proof

## 8. Non-Goals

- no runtime-row promotion for bridge carriers
- no empty-query execution semantics
- no “SOTA++” branding based on structure alone

## 9. Admission Bar (frozen by [ADV-00](tickets/ADV-00-scope-lock-and-admission-bar.md))

This table is the single packet-level gate. A widening lane may not start code
until its row exists here with a non-empty owning seam, red rail, and claim
gate. A lane may not be called “advanced” until its claim gate is satisfied by
real evidence, not structure.

| widening surface | current state | owning seam (code authority) | red rail first | claim gate (before “advanced”) |
| --- | --- | --- | --- | --- |
| predicate subset widening | hardcoded `match` in lexical lowering/planner; unsupported shapes typed-fail `LEX_PREDICATE_UNIMPLEMENTED` | `quanta-index-lexical` `predicate_registry.rs` + `planner.rs` + `lib.rs` | `tantivy_smoke` + `predicate_repo_has_file_plans_through_tantivy_route` | owner-local + runtime + parity proof for each widened family, and a registry row |
| SG structural `RawString` sibling | typed-fail `BRIDGE_TRANSLATE_FAIL` on the SG structural route | `quanta-index-search-plane` `lowering.rs` legality table; mirrored in `quanta-index-lq-bridge` `translator.rs` | `sourcegraph_structural_route_preserves_lexical_keyword_in_mixed_boolean_or` | native↔SG parity row + bridge golden + explicit typed-fail remainder |
| SG structural `Predicate` sibling | typed-fail `BRIDGE_TRANSLATE_FAIL` on the SG structural route | same as `RawString` sibling | same lowering owner rail | same as `RawString` sibling; cannot start before `RawString` lane is green |
| SG scoped-filter under mixed `OR` | typed-fail `BRIDGE_TRANSLATE_FAIL` by closeout design | `quanta-index-search-plane` `lowering.rs` legality table | `sourcegraph_structural_route_rejects_repo_scoped_filter_under_mixed_or` | per-family verdict (`accept` / `accept-with-rewrite` / permanent reject) with proof or a permanent typed-fail rail |
| generated capability truth + benchmark bar | hand-maintained ledger/matrix prose; no drift gate | `tools/ci/lint/check-dsl-capability-truth.py` consuming code-owned metadata | `python3 tools/ci/lint/check-dsl-capability-truth.py` | checker fails closed on drift, and a widening claim without a benchmark/shadow section |

### 9.1 Frozen start order

This order is authoritative and matches [INDEX.md](tickets/INDEX.md) and
[HISTORICAL-MAP.md](tickets/HISTORICAL-MAP.md). Deviating is a scope-creep
finding, not a shortcut.

1. `ADV-00` admission bar (this section)
2. `ADV-01` registry only, no behavior widening
3. `ADV-01` first widened predicate family
4. `ADV-02` `RawString` sibling
5. `ADV-02` `Predicate` sibling
6. `ADV-03` scoped-filter-under-`OR` verdicts
7. `ADV-04` generated/checked truth + benchmark gate

### 9.2 Claim discipline for “advanced”

No widened lane may be described as “advanced”, “SOTA”, or “SOTA++” on the
basis of structure, registry shape, or a broader accepted subset alone. The
label is admissible only when **both** are true and both are linked from the
lane’s ticket:

1. correctness evidence — owner-local rail, runtime row, and (for dual-syntax
   surfaces) an explicit native↔SG parity row; and
2. cost evidence — a benchmark or shadow measurement under the `ADV-04`
   template, not a prose promise.

A lane that has only correctness evidence is “broader”, not “advanced”. A lane
that has neither is “planned”. Reporting structure as if it were advancement is
the primary failure mode this bar exists to block.

### 9.3 Permanent exclusions

These categories are out of scope for the whole packet and may not be
reintroduced through any lane:

- bridge-packet carriers (`into:codeql`, `scope:results`, `with:lexical`) as
  runtime search-result rows — they stay `bridge_packet` carriers
- parser-only / normalizer-only shapes presented as executable surface
- empty-query execution semantics
- request-time `git`, `tree-sitter`, embedding, or other producer-owned
  fallback authorities reached from the search plane

## 10. Benchmark / Shadow Evidence Template (frozen by [ADV-04](tickets/ADV-04-generated-proof-truth-and-benchmark-bar.md))

A widening lane that wants the “advanced” label (README §9.2) must fill this
template **in its own ticket** before the label is admissible. The drift gate
`tools/ci/lint/check-dsl-capability-truth.py` fails any ticket whose `Status`
claims `advanced` without a benchmark/shadow section.

```
## Benchmark / Shadow Evidence

- correctness proof: <owner-local rail name>
- parity proof: <native↔SG parity row name>  (dual-syntax surfaces only)
- chaos/restart proof: <rail name>  (route-sensitive surfaces only)
- cost evidence (fill at least one, as a runnable command, not prose):
  - benchmark: `just rust-bench-dsl-warm` / `just rust-bench-dsl-cold` + `just rust-bench-dsl-compare` against `tools/benchmark/baselines/*.json`
  - shadow: <shadow rail + the metric and threshold measured>
- measured result: <number + baseline + delta, or the shadow verdict>
```

Rules:

- cost evidence is a command that can be re-run, never a prose promise
- a lane with only correctness/parity proof is “broader”, not “advanced”
- the drift gate is mechanical; it does not judge whether the numbers are good,
  only that the evidence section exists when the label is claimed
