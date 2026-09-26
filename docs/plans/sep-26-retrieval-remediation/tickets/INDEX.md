# SEP-26 Retrieval Remediation

Status: `PARTIAL`

Consolidated: 2026-09-27 from clean pre-documentation snapshot
`5e6addd5814ce8b71af808ff201ddd0b18fbe6c4`.

## Canonical owners

| Concern | Canonical document |
|---|---|
| Query policy, publication, result proof and span accounting | [SEP-26-001](../../../adr/SEP-26-001-retrieval-query-publication-and-result-proof.md) |
| Observation, comparator profiles, semantic/ANN proof, defaults and resource accounting | [SEP-26-002](../../../adr/SEP-26-002-retrieval-observation-experiment-and-default-policy.md) |
| Evidence custody, experiment admission and qualification | [SEP-26-003](../../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md) |
| Compact decision lookup | [SEP-26 decision registry](../../../adr/SEP-26-DECISION-REGISTRY.md) |
| Work still required | [Active gap register](GAP-REGISTER.md) |
| Qualification commands and oracles | [Test plan](TEST-PLAN.md) |
| Historical ticket and audit map | [Archive manifest](ARCHIVE-MANIFEST.md) |

Accepted ADRs own current architecture. The RBR ticket files and long audit ledgers are retained as historical
implementation and evidence records. They do not override an ADR or the active gap register.

## Current boundary

- The architecture and implemented contract decisions are `Accepted`.
- This documentation change is inside the retrieval source closure. Earlier source-bound receipts are stale for the
  resulting revision.
- Observed pre-update main is `eacb93289ddbec62b43991af4666aadb194114d5` + shared dirty code/docs. Current-source
  qualification is `NOT_RUN`; docs/comments and code bytes have changed since the previous snapshot.
- Previous `c6dd70af` actual SDK18/Python319/Rust108, three fresh validation/replays, separate CI1640/15subtests and
  12 gates, stable identity1417/package3626/232pycs, and both consumer8-mutant checks are `VERIFIED` only within
  their original bound inputs. They do not close the newly confirmed native admission P1 or qualify current main.
- The old guard accepted native-inadmissible language strings and unknown/ill-typed symbol kinds (native contracts:
  open ASCII syntax; nullable12 closed kinds), plus Unicode surrogate strings not representable as Rust UTF-8
  `String` in native DTO/raw input. Actual language/kind15, input-only UTF-8 three and logical-snippet counterexamples reached
  that boundary. External corrected controls rejected24 invalid/raw18 cases while retaining24 valid5/5 controls.
  Canonical language/kind/UTF-8 guards and raw18 persistent assertions are integrated without new authority IDs;
  root focused1 passed/318 deselected with stable code inputs is `VERIFIED` local (user `PYTHONPATH=.`; not canonical
  process custody). Owner whole328 is `VERIFIED` local with exact328 collected/JUnit/pass identities,20 artifact
  hashes and stable source/tool/environment snapshots, sealed in isolated clean `339a20cd`; it excludes root's extra
  raw18 assertions and current-main/Rust/SDK/model/quality/performance qualification. [Owner receipt](/private/tmp/qi-rbr-metadata-audit-proof/owner-receipt.json)
  SHA `d5eb65d832bbe317cf660952a574da13be49d63ea5d9782d1c46bf5dc34e68bf`. New final full capsule
  is `NOT_RUN`; do not compose focused/owner results. See G-01, not a broad feature ticket.
- Hosted CI is independently `BLOCKED` on billing (14 jobs/0 steps); local CI/gates do not substitute for it. This
  operational verification boundary is separate from user-owned external evaluation inputs.
- New symbol-boundary P2 is `FAILED`: the native six-case probe shows malformed Rust generic owner identities
  (`fn() -> ()`/const `<`) and nested named functions misclassified as methods under TypeScript class arrow/static
  blocks and Rust impl const closures. Pinned-grammar fixtures parse without errors. An isolated owner fix is
  underway; no main integration or verified fix is claimed yet. G-01 owns this bounded producer defect, not G-04 expansion.
- The `11f1424` run was controlled-stopped after that discovery: actual SDK18 and12 gates retain their original
  completed scope; interrupted Python319/Rust108/full CI are `NOT_RUN`. Do not compose those results into closure.
- SEARCH3/native5/85-negative/ANN bounded development probes have actual local results; covered/excluded scopes
  and original receipts are in [GAP-REGISTER](GAP-REGISTER.md). They are not missing implementation or broad
  quality, restart, encoder, performance or current-main qualification.
- Final results belong in an external digest-bound closeout under [TEST-PLAN §5](TEST-PLAN.md#5-증거-묶음과-완료).
  Planned final closeout (`NOT_RUN`): `/private/tmp/qi-rbr-symbol-boundary-final.x3ddOxQY/final-closeout.json` is only
  the planned destination; it is not evidence until an actual terminal-bound artifact exists. Do not mutate these
  documents after the final source freeze to insert results.
- Final external `PAIR_VALID`, `QUALITY_DELTA` and `PERF_QUALIFIED` are `NOT_RUN`.
- Symbol typed refusal remains accepted. Authority expansion is `NOT_APPLICABLE` absent explicit new product
  scope; no ranking bug has been established. Manual admission/gold/host procurement remains user-owned.
- Paired-platform scope is macOS/Linux. Qualified Linux delegated-cgroup/Landlock positive resource proof remains
  `NOT_RUN`; fake-owner/diagnostic tests do not qualify it. Native Windows pair support remains `NOT_APPLICABLE`
  without explicit product expansion; the macOS legacy `ps` stable-PID-identity limitation remains excluded.
- Conditional same-model or incremental claims remain `NOT_APPLICABLE` unless a run explicitly enables them and
  supplies the raw proof required by SEP-26-003.

## Ticket map

| Ticket | Decision owner | Remaining work |
|---|---|---|
| RBR-00 | SEP-26-003 | G-01 |
| RBR-01 | SEP-26-002 | G-02 |
| RBR-02 | SEP-26-001 | G-01 |
| RBR-03 | SEP-26-002 | G-03 |
| RBR-04 | SEP-26-001 | G-01/G-03 |
| RBR-05 | SEP-26-001 | G-01 |
| RBR-06 | SEP-26-001 | G-03 |
| RBR-07 | SEP-26-002 | G-03 |
| RBR-08 | SEP-26-001/002 | G-04 (`NOT_APPLICABLE` unless explicitly expanded) |
| RBR-09 | SEP-26-002 | G-02/G-03 |
| RBR-10 | SEP-26-002 | G-05 |
| RBR-11 | SEP-26-002 | G-06 |
| RBR-12 | SEP-26-003 | G-01/G-03 |
