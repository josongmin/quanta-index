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
| [ADV-00](tickets/ADV-00-scope-lock-and-admission-bar.md) | planned | freeze admission table and claim discipline only | packet/doc truth spot-check |
| [ADV-01](tickets/ADV-01-predicate-capability-registry.md) | planned | registry only, then one widened predicate family | `tantivy_smoke` before any widen |
| [ADV-02](tickets/ADV-02-sourcegraph-structural-mixed-domain-widening.md) | planned | legality table, then `RawString`, then `Predicate` | search-plane lowering tests |
| [ADV-03](tickets/ADV-03-sourcegraph-scoped-filter-or-widening.md) | planned | decision table before any code | rejected scoped-`OR` owner rail |
| [ADV-04](tickets/ADV-04-generated-proof-truth-and-benchmark-bar.md) | planned | checker first, generator optional | docs-vs-code drift checker |
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
