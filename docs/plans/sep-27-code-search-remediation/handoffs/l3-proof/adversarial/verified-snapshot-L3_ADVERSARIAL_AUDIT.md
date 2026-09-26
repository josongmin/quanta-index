# L3 adversarial code audit

**VERIFIED** for one repaired defect and the selected lexical test scope.
HEAD `98601a66d8cab9c86232b3e62ce490c8b43b71b6`, shared dirty `main`.
Selected input SHA256: `546cc7b000c7f0bbee6d7ac64725789d81acf1eacc6056aef735b419600b04c8`.
Receipt: `L3_ADVERSARIAL.source.json`, SHA256 `db2cae59afe32e1a09818ef5a5f31c65ab698c293e2028c5fdbb3685b555da83`.

## Reproduced P2 defect

`GroupedPageSegment::harvest` returned a decoding failure without stopping the
shared collection. The wrapper could continue into a later segment, whose work
refusal replaced the original Storage error with `LexicalCollectionBudgetExceeded`.
Harvest now marks the collection aborted on error, preserving the first failure.

The regression mutates a real native two-segment Tantivy index so the first
representative key fails UTF-8 decoding only at harvest. Before the fix, work
limit 3 returned the wrong typed budget error (`audit3-harvest-red`, one executed
failing test). After the fix, limits 3 and 100 preserve Storage, open exactly one
scorer, consume three work units and release all byte reservations. This tests
the native collector below sealed-generation admission; it does not demonstrate
a corrupted file bypassing the generation seal.

The only production change this turn is the harvest abort propagation in
`ranked_page.rs`. Tests are in `ranked_page_tests.rs`; `l3-proof/adversarial/owned.patch`
isolates this turn from earlier shared changes.

## Adversarial oracle and validation

A separate fixed golden tests 24 complete page walks and six grouping cases:
three segments, score ties, boosts 0/1/2.5, count on/off, page widths 1/2/4/8,
path/repo projections, Unicode keys and retained-byte release. It does not derive
the expected ordering from the production comparator. These are subcases of one
test, not additional terminal test counts. Focused L3 result: 31 passed.

Environment: `QUANTA_INDEX_RESOURCE_ADMISSION=1 CARGO_BUILD_JOBS=2`.

```sh
./scripts/cargow --lane test-fast-lane test -p quanta-index-lexical --lib --test execution_budget --test ranked_pages --test l3_exact_source --test cancellation_inside_search --locked
```

Final result: **190 passed, 0 failed, 0 ignored**, across the library and four
selected integration targets. Run `audit3-final-2`. Source boundary:
command launch; selected inputs remained stable through completion
and matched the final snapshot. Raw log SHA256 `59f5c0ab1ea2402d046a0f4d22bb15c229d55e24ae6d4af4e8610a4c3cb34c1b`.
Formatting and `git diff --check` are **VERIFIED**.

The receipt archives before/after manifests, raw outputs, exact commands, five
binary digests, environment/toolchain identity, the red reproduction and earlier
executions. Unselected integration tests and unrelated concurrent edits are
outside the bound selected scope. Previous 188/373-test receipts are historical
snapshots, not current proof after this change.

Current SDK/CLI, full repository CI, installed daemon E2E, ranking quality,
latency and request-wide RSS are **NOT_RUN**. No score or ranking-weight change,
commit/push/reset, or inter-task communication was performed.
