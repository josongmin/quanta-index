# SEP-21 action list

Use the [residual execution plan](FINAL-RESIDUAL-EXECUTION-PLAN.md) for
current work. The original wave checklist and importer steps are in Git
history. Check a task against current source and raw evidence before marking
it complete; a historical handoff or a `passed` JSON alias is not current
qualification.

| Area | Current action | Authority |
| --- | --- | --- |
| R0 — common result authority | Derive test outcomes from archived machine runner output; bind host and CI producer authority; add typed operational-action results. A hand-written `passed` terminal JSON is not an oracle. | [residual execution plan](FINAL-RESIDUAL-EXECUTION-PLAN.md), [S21-13](S21-13-release-evidence-and-sota-qualification.md) |
| R1 — P03-P08 release | Inventory each staged target and independent negative oracle before changing product code or promoting authority. | `tools/ci/proof-authority.toml`, `tools/ci/test-authority.toml` |
| R2 — P09 | Active sealed-identity liveness replaced the boot-time backend constant. A bounded Admin-only control/SDK/searchctl projection of the existing request-event ring is implemented. Focused contract, SDK, CLI, wrap/drop/restart, Cargo-built daemon lexical+semantic queue→backend/provider→terminal, and shared-UDS scripted observer zero-disclosure checks passed locally; none is a frozen-source release receipt. Still require supervised child/maintenance loss, independent different-UID kernel check where available, fuzz smoke (cold build interrupted), and Linux release qualification. | [S21-10](S21-10-control-authorization-readiness-and-observability.md) |
| R3 — semantic scope | Locate an independent producer source-plan oracle before any completeness wire change; preserve legitimate lexical-only/no-op deltas. | [residual execution plan](FINAL-RESIDUAL-EXECUTION-PLAN.md) |
| R4 — P10 state | The local `verify-state` owner path now rechecks source inventory and manifest authority after catalog verification. Manifest reads reject links/special files without blocking; publication uses atomic exclusive creation. The disposable-root owner target passed 40/40 locally (including post-read mutations and dangling-link write refusal). Prove the workflow on frozen source; inventory authorized target data before cutover. This is not authorized-target or release proof. | [operator runbook](../../../operator/state-cutover-runbook.md) |
| R5 — P11 exact pair | Freeze both sources and resolved Cargo path roots, then bind fresh build, QBC targets and actual runner result to one receipt. | [S21-12](S21-12-cross-repo-terminal-receipt-cutover.md) |
| R6 — P11 operations/P12 | Issue distinct deployment/activation/rollback action receipts only on authorized targets; reissue final-source dependencies and aggregate; audit historical handoffs separately. | [S21-12](S21-12-cross-repo-terminal-receipt-cutover.md), [S21-13](S21-13-release-evidence-and-sota-qualification.md) |

2026-09-24 checkpoint tested at Quanta `7dec5965` plus the RepoMap receipt
fix, committed as `3b1d7b19`: the owner integration target passed 19/19 and the malformed
terminal-sequence unit target passed 1/1. Before that edit, the clean-Quanta
P12A infrastructure recipe passed 121/121 Python tests while validating zero
proof manifests. These are local checks only. The registry still stages P09
Linux process/component-loss, P10 restore/rollback, P11 exact-pair and
deployment/activation/rollback; no final-source release bundle has
been issued. See the P11/P12A tickets for precise source and exclusions.

`just proof-authority-lint` checks registry structure only.
`just proof-authority-current-gate` checks a fresh P00 receipt.
`just proof-authority-code-gate` checks the fixed `CODE_QUALIFIED` closure
against the current Quanta/Semantica source pair before deployment. It does
not require deployment, activation, rollback, or the final aggregate.
`just proof-authority-release-gate` checks the complete current-source bundle
after those operational actions. The manual `correctness` workflow selects
`proof_stage=code` or `proof_stage=final` (the default). Neither gate runs on
ordinary PRs. Keep code qualification, deployment, activation, and rollback
as separate states; a code-gate pass is not production readiness.

The R0 proof-boundary fix is prerequisite to *authoritative* receipts, not to
parallel P09/source-plan development. A source edit invalidates existing
exact-source receipts; reissue only after the final source pair is frozen.

## 2026-09-26 P10 local closeout

- RCA: `verify-state` returned after sequential object/catalog reads without
  a terminal drift check; excluded manifest authority could also change.
  Manifest publication used `exists()` followed by a link-following write,
  and manifest reads followed links.
- Changes: share the existing source-freeze comparator; recheck inventory,
  selected manifest and manifest kind before returning; use `create_new`
  and fsync the same write handle; use `NOFOLLOW | NONBLOCK` and inspect
  the opened read handle for regular-file type and one link. Unsupported
  non-Unix manifest custody is explicitly refused.
- Local owner execution: `VERIFIED` for disposable fixtures only.
  Command: `./scripts/cargow test -p quanta-index-searchd-runtime --test
  state_migration_owner_v1`. Result: 40 passed, 0 failed, 0 ignored, 0 filtered.
  Final warm execution: build 8.00s, tests 4.26s; not a performance claim.
- Source: base `79bb8d23312d48d5ef8c0dba972e337c3e72e041` plus dirty owner
  files in a shared dirty `main`; `rustc 1.92.0`, `aarch64-apple-darwin`.
  SHA-256: `state_format.rs`
  `ea42b3ab774056269b85be933cfe3bc50049622b9fe656a91a3c67721994536e`,
  `state_migration.rs`
  `b3bf37d120f575738d240547363d2c7ea4dd5510d074326dae03a5ec1e54d00b`,
  `state_migration_owner_v1.rs`
  `ee2cf6d4a282dbaa286b92b004f6e008a8681372beed9f35942807c534453ec7`.
- Raw local log: `/tmp/quanta-p10-closeout.8oD2mA/owner.log`, SHA-256
  `a9545e561f41b38ba98859d8ff6cfb8b6eb50c0bd99caeea27bd40c3382f35da`.
  This temporary local artifact is not a retained release receipt.
- Initial executions failed on the missing manifest-kind recheck and on
  compile-time fixture/result handling; these were corrected before the
  final successful run. Formatting and owner diff whitespace checks passed.
- Excluded: frozen-source qualification, Linux process proof, whole-repo
  verification, authorized real-root restore/rollback and deployment.
  These are `NOT_RUN`, not implied by the local owner result. Backup custody
  remains read-only rather than writer exclusion; keep the root quiescent,
  and do not treat this point-in-time verification as authority after a write.
