# L3 adversarial code audit

**One P2 defect repaired.** Its regression and the selected 190 tests are
**VERIFIED on the frozen snapshot** below. Qualification of the latest shared
working source is **BLOCKED by concurrent source changes**.

HEAD `98601a66d8cab9c86232b3e62ce490c8b43b71b6`, dirty `main`.
Current receipt: `L3_ADVERSARIAL.source.json`, SHA256 `6be54ab43a1a76d66b01cb6073d351d0261ebcbad15c6ad3cd63cc3f8a442310`.

## Reproduced defect and repair

A grouped representative key could fail decoding at segment harvest without
stopping collection. Entering a later segment could then replace that Storage
error with `LexicalCollectionBudgetExceeded`. `GroupedPageSegment::harvest` now
aborts the shared collection on failure and preserves the original error.

A real two-segment Tantivy fixture reproduces the wrong error before the fix.
After the fix, work limits 3 and 100 preserve Storage, open exactly one scorer,
consume three work units and release all retained bytes. This native-collector
fixture is below sealed-generation admission; it does not establish a generation
seal bypass. The only production edit this turn is the harvest error propagation
in `ranked_page.rs`; tests are in `ranked_page_tests.rs`.

An additional fixed oracle covers 24 complete page walks and six group cases:
three segments, ties, boosts 0/1/2.5, count on/off, page widths 1/2/4/8,
path/repo groups, Unicode keys and reservation release. These are subcases of
one test, not 30 additional terminal test cases.

## Verification and source boundary

`audit3-final-2`: **190 passed, 0 failed, 0 ignored**, with no source changes
between command launch and completion. Selected input SHA256:
`546cc7b000c7f0bbee6d7ac64725789d81acf1eacc6056aef735b419600b04c8`.
The immutable receipt and original report are
`l3-proof/adversarial/verified-snapshot-L3_ADVERSARIAL.source.json` and
`l3-proof/adversarial/verified-snapshot-L3_ADVERSARIAL_AUDIT.md`.
The two owned Rust files still match that verified snapshot.

Latest re-run `audit3-final-3` also executed 190 passing tests, but
`query_admission.rs` changed inside its execution window. That run is **BLOCKED**
for source qualification and is not promoted to a current-source pass. Subsequent
shared contract changes are enumerated in the current receipt. Counts from
separate runs are not combined.

Environment: `QUANTA_INDEX_RESOURCE_ADMISSION=1 CARGO_BUILD_JOBS=2`.

```sh
./scripts/cargow --lane test-fast-lane test -p quanta-index-lexical --lib --test execution_budget --test ranked_pages --test l3_exact_source --test cancellation_inside_search --locked
```

Formatting and whitespace checks for the repair passed. Fresh Cargo metadata,
source/dirty manifests, commands, logs, binary hashes, the before/after owned
patch and failed reproduction are archived under `l3-proof/adversarial/`.
Re-run the command after shared dependency inputs stabilize to close the current
source gap. Whole-repository CI, installed daemon E2E, performance, request-wide
RSS and ranking quality are **NOT_RUN**. No commit/push/reset or inter-task
communication was performed.
