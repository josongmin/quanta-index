# Documentation Archive Index

Status: `HISTORICAL INDEX`

Archive boundary: revision `eacb93289ddbec62b43991af4666aadb194114d5`,
captured 2026-09-27 before this classification.

This index covers historical records outside completed implementation-plan
packets. The custody rule is
[SEP-27-001](adr/SEP-27-001-documentation-authority-and-historical-record-custody.md).
The separate [completed-plan archive](plans/ARCHIVE-INDEX.md) owns plan packets.

## Archived record sets

| Record set | Files | Recorded source/state | Current authority |
|---|---:|---|---|
| [Sep 21 purpose static audit](analysis/quanta-index-purpose-static-audit-2026-09-21.md) ([pass 2](analysis/quanta-index-purpose-static-audit-2026-09-21-pass2.md), [pass 3](analysis/quanta-index-purpose-static-audit-2026-09-21-pass3.md)) | 3 | dirty snapshot at `3ad279a0`; static `PURPOSE_RED` observations | accepted [SEP-21 decisions](adr/SEP-21-DECISION-REGISTRY.md), current residual ledger, fresh verification |
| [Sep 16 bugbash](bugbash/sep-16/findings.md) | 8 | clean audit base `4914156f`; later frozen implementation and gate records | accepted [SEP-21 decisions](adr/SEP-21-DECISION-REGISTRY.md), current residual ledger |
| [Pre-de-channelize SSOT](ssot/channel-architecture.md) | 2 | channel-era architecture before the 2026-05-27 cutover | [MAY-27-002](adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md), live runtime source |
| [Sep 22 test-optimization audit](bugbash/sep-22-test-optimization/00-plan.md) | 6 | audit snapshot `23bd3d7f`; 18 original findings | active [TOPT index](../tickets/sep-22-test-optimization/INDEX.md) and [current closeout](../tickets/sep-22-test-optimization/SEP25-CURRENT-CLOSEOUT.md) |
| [Sep 23 TOPT receipts](../tickets/sep-22-test-optimization/RCA-2026-09-23-current-source.md) | 2 | integration and gate observations through the recorded Sep 23 sources | [SEP25 current closeout](../tickets/sep-22-test-optimization/SEP25-CURRENT-CLOSEOUT.md) |

Total: 21 historical Markdown records outside the completed-plan archive.

The Sep 16 set includes:

- `findings.md`, `implementation-progress.md`, `structural-remediation-plan.md`,
  and `test-plan.md`;
- `adr/G0-C-catalog-engine.md`, `adr/G0-L-tantivy-snapshot-reuse.md`,
  `adr/G0-R-runtime-cancellation.md`, and
  `adr/G0-S-lance-snapshot-reuse.md`.

The Sep 22 audit set includes `00-plan.md` through `05-cheesy-src-code.md`.
The Sep 23 receipt set includes `RCA-2026-09-23-current-source.md` and
`SEP23-GATE-FOLLOWUP.md`.

## Deliberately retained as active or mixed-state

| Document | Reason |
|---|---|
| [Purpose validation checklist](analysis/quanta-index-purpose-validation-checklist.md) | Reusable current audit method, not a result snapshot. |
| [DSL capability inventory](analysis/jun-4-dsl-capabilty.md) | Live capability map linked to accepted DSL and Sourcegraph decisions. |
| [Test execution optimization](analysis/test-execution-optimization-2026-09-21.md) | Contains remaining measurement work; the document is not closed as a unit. |
| [Storage architecture implementation map](ssot/may-23-storage-architecture-endgame-implementation.md) | Mixed-state implementation map with an explicit SPA-00 caveat; accepted ADRs take precedence. |
| [Wave 2 embedding handoff](handoff/jun-24-wave2-embedding-ab-hardening.md) | P1-P5 handoff work remains unresolved. |
| [State cutover runbook](operator/state-cutover-runbook.md) | Current operator procedure. |
| [Potion code embedder](potion-code-embedder.md) | Current product documentation. |
| [TOPT packet](../tickets/sep-22-test-optimization/INDEX.md) | TOPT-00 and TOPT-08 current-source qualification remain open. |

## Retrieval and use

Historical bodies stay at their original paths. Use
`git show eacb93289ddbec62b43991af4666aadb194114d5:<path>` for bytes before
the archive banners were added.

Do not carry a historical status, count, or test outcome forward. Re-freeze the
source and run the current authority rail before making a current claim.
