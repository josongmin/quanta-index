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
- Observed pre-update main is `d2b79fda70d8dc4c47bf75a51408190474c1fa9c` + integrated symbol-source dirty; this
  update adds active-document dirty paths. Current-source
  qualification is `NOT_RUN`; docs/comments and code bytes have changed since the previous snapshot.
- Previous `c6dd70af` actual SDK18/Python319/Rust108, three fresh validation/replays, separate CI1640/15subtests and
  12 gates, stable identity1417/package3626/232pycs, and both consumer8-mutant checks are `VERIFIED` only within
  their original bound inputs. They do not close the newly confirmed native admission P1 or qualify current main.
- The old guard accepted native-inadmissible language strings and unknown/ill-typed symbol kinds (native contracts:
  open ASCII syntax; nullable12 closed kinds), plus Unicode surrogate strings not representable as Rust UTF-8
  `String` in native DTO/raw input. Seven old language/kind variants and three input-only Unicode fields reached
  that boundary. External corrected controls rejected15 language/kind+9 surrogate variants, retained24 valid5/5
  controls and rejected18 raw-only cases. These are scoped logical consumer oracles, not signed producer forgery/full-custody exploits.
  Canonical language/kind/UTF-8 guards and raw18 persistent assertions are integrated without new authority IDs;
  root focused1 passed/318 deselected with stable code inputs is `VERIFIED` local (user `PYTHONPATH=.`; not canonical
  process custody). Owner whole328 is `VERIFIED` local with exact328 collected/JUnit/pass identities,20 artifact
  hashes and stable source/tool/environment snapshots, sealed in isolated clean `339a20cd`; it excludes root's extra
  raw18 assertions and current-main/Rust/SDK/model/quality/performance qualification. [Owner receipt](/private/tmp/qi-rbr-metadata-audit-proof/owner-receipt.json)
  SHA `d5eb65d832bbe317cf660952a574da13be49d63ea5d9782d1c46bf5dc34e68bf`. New final full capsule
  is `NOT_RUN`; do not compose focused/owner results. See G-01, not a broad feature ticket.
- Hosted CI is independently `BLOCKED` on billing (14 jobs/0 steps); local CI/gates do not substitute for it. This
  operational verification boundary is separate from user-owned external evaluation inputs.
- Shared evidence-reader path custody is bounded within G-01: same-byte raw/evidence symlinks and dangling/linked
  optional pointer ancestors now refuse through the existing no-follow reader/presence guards; a truly absent
  optional pointer retains its absence semantics. Selected pre-fix8 cases were6 failed/2 passed; post-fix four
  owning files had156 passed with stable selected inputs. [GREEN raw](/private/tmp/qi-rbr-sdk-symbol-final.TEINH2/final-store-reader-green.log)
  SHA `38b855b2d984fdcd9809880a1d42597bbcd1c6e0f3cbfd903d3e96e12b02f721`. Broad benchmark GC, host
  and streaming import are not claimed. New frozen full suite/evidence replay remains `NOT_RUN`.
- Symbol-boundary P2 is serially integrated at exact final-v2 tested source SHA `3c120e572d28fc6bbc10749a1ba3fce9aca8512d5e79158887ec291985b8992c`.
  The original six cases were five failures plus one correct control. Final owner local proof covers83 unique lib
  tests/native11 goldens, structural Rust/JS/TS ownership and Python named-scope preservation, Clippy/fmt0,
  38 verified artifact hashes and1424 stable source files. [Owner receipt](/private/tmp/qi-rbr-readiness-review/owner-receipt.json)
  SHA `218b2baab77feef40a4d1b7538a1d5966c5a8fac4fda95e4566dc7d1b10b8006`. Collection108 includes25 chunking
  tests not executed in that owner proof. Pinned turbofish ParseFailure is preserved. G-01 still requires final
  whole-source verification; this is not rank tuning, G-04 expansion or all-defect closure.
- The `11f146cfc643fa06decd9a8cbdcce8fb1810ac97` run was controlled-stopped after that discovery: actual SDK18 and12 gates retain their original
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
