# L2 final native-process audit

> Historical report: one-off evidence files were removed from the repository. This report alone is not current verification.

Historical snapshot: [L2_REAUDIT_20260927.md](L2_REAUDIT_20260927.md)
supersedes the no-new-P0/P1 conclusion below after a cross-stream source
publication defect was reproduced and repaired.

No additional confirmed P0/P1 remains open in the reviewed L2 mutation,
publication, coverage and recovery paths. This is a bounded code-audit finding,
not a repository-wide absence-of-bugs claim. RR-10 through RR-14 remain repaired;
their reproduced failures and owner evidence are in [L2_FINAL_AUDIT.md](L2_FINAL_AUDIT.md).

**VERIFIED:** the retained native daemon passed two SDK process tests, including
all eight named crash cuts. Lexical owner tests passed 23/23. SDK process-test
lint and search-plane library lint both exited 0 with warnings denied.
**BLOCKED:** whole-checkout qualification, the legacy shared runtime harness,
and external producer cutover. Release installation and physical power loss are
**NOT_RUN**.

Recorded source: shared dirty `main`, HEAD
`2102966246866398f01833bebf71396831377149`.
The machine-readable report binds commands, terminal
counts, raw logs, source changes, platform/toolchain, configuration and retained
state files. L2_PROCESS.source.json inventories current
L2 source. Whole-file hashes include preserved concurrent changes.

## Actual process coverage

`crates/quanta-index-sdk/tests/l2_daemon_publication.rs` launches the real daemon
against fresh state roots, uses the public SDK and sockets, and stops only its
owned children. No scripted peer or searchd harness adapter participates.

- Retargeting a known event returns its original receipt and original paired
  generation, including after a real process restart. The alternate revision
  remains unactivated.
- Reusing the event identity with a different payload is rejected with
  `BATCH_DIGEST_CONFLICT`; the following valid publication succeeds.
- A Delta cannot claim a newer source parent while cloning an older physical
  base. The correct base retains both the replacement and an unchanged file.
- Old pinned generations retain old bytes after replacement and restart.
- Text remains searchable for `NotRequested` symbol coverage. A strict symbol
  no-hit request returns `SYMBOL_COVERAGE_INCOMPLETE` before and after restart.
- With the visible head rolled back to g1, publication of g3 uses source parent
  g2. The test crashes the process with observed exit 86, restarts, retries the
  original Delta, activates once, and verifies changed/unchanged text and the
  exact replay receipt.

The crash matrix executes these cuts in order:
`after_semantic_seal`, `before_authority_record`, `after_retention_receipt`,
`after_catalog_transaction`, `after_ledger_reconcile`, `after_fence`,
`between_track_reclaims`, `before_record_forget`.

Before recovery starts, the test independently checks the persisted g1/g3
authority files exist and g2 authority is absent at every post-retention cut.
It also checks lexical g2 is physically absent between track reclaims, and both
lexical and semantic g2 directories are absent before record forget. Configuring
a retention limit alone is not treated as proof that retirement happened.

## Executed rails and boundaries

All native rails used `QUANTA_INDEX_RESOURCE_ADMISSION=0 CARGO_BUILD_JOBS=2`, under
the user's concurrent-work authorization. The full exact commands and hashes are
in the JSON report and the linked receipts.

| Receipt in `l2-proof/` | Actual outcome | Claim boundary |
| --- | --- | --- |
| `process-daemon-custody.json` | Native daemon build exited 0 | Compiler JSON identifies the executable. Three unselected test files changed; the whole-checkout capture remains `BLOCKED_SOURCE_DRIFT`. |
| `process-final-custody.json` | 2 passed, 0 failed, 0 ignored; all 8 crash cuts observed | Exact retained binary and SDK test behavior. Benchmark Python tooling changed during execution; no whole-checkout promotion. |
| `audit-lexical-current.json` | 23 passed, 0 failed, 0 ignored | Exact command snapshot, no source drift during execution. |
| `audit-owner-current-fixed.json` | Contract 163, SDK 114, search-plane 261 passed; 205 query tests filtered | A concurrent search-corpus formatting edit makes exact final-source qualification blocked. Counts are execution evidence, not one combined final qualification. |
| `process-sdk-lint-final.json` | Exit 0 | `clippy -p quanta-index-sdk --test l2_daemon_publication --locked -- -D warnings`; no source drift. |
| `audit-plane-lint-current.json` | Exit 0 | `clippy -p quanta-index-search-plane --lib --locked -- -D warnings`; no source drift. |

The daemon is preserved at
`/tmp/qi-l2-binary-qts9y_ah/quanta-index-searchd`, SHA-256
`eb80c5dc06c60d9869df2d2e13680a1be2d51a4003559fa2112d4b421b7258e3`.
The test copies it into each retained artifact root and the closeout verifies
those copies have the same hash:

- `/tmp/qi-l2-crash-7n4SHR`: eight independent crash-case state roots and logs.
- `/tmp/qi-l2-m0wm20`: original binding, conflict, coverage, Delta and restart.

Build capture now retains Cargo-reported executable artifacts immediately and
rejects a digest change during copying. New tests explicitly fail on missing
binary configuration and are run with `--ignored`; a default ignored test is
not counted as evidence. Local temporary artifacts must be retained or exported
with the report for future reproduction.

## Failures preserved and repaired

The expanded matrix initially counted the authority's `.staging` directory as
a second pair. The test now excludes only the known `.staging` and `.reclaim`
infrastructure directories, and all eight actual recovery sequences ran.
An intervening shared edit referenced the private `readiness::structural_state`
module and broke compilation; the call now uses the existing public re-export.
Both failed receipts remain recorded. Process-test lint findings were corrected
before the final native execution and final lint run.

## Remaining boundaries

- The shared runtime harness still publishes unsealed incremental batches into
  a target and then an empty seal batch. Its old DTO compilation failure is
  separate from the new native SDK proof. A coherent fixture migration remains
  open; fabricating Complete(0), source hashes or generation-derived event IDs
  would not repair its semantics. Semantica AG5 I01 offered this migration and
  received the untouched-file handoff; no completion result has been received.
- External producer issuance, source high-water/base acquisition and Semantica
  cutover were not qualified here. Producer source/coverage digests remain
  attestations, not independent full-file-byte or parser-completeness proofs.
- Full repository profiles, all public API/module gates, release packaging,
  performance and physical power-loss tests were not executed in this closeout.
  Native debug crash hooks prove process-recovery behavior only.
- Source snapshots from different commands are not merged. Raw concurrent-drift
  receipts remain blocked for whole-checkout qualification. No other worker's
  changes were reverted; L2 made no commit or push.
