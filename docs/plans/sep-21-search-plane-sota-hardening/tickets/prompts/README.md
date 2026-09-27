# SEP-21 current execution entrypoints

The former P00-P09 copy/paste prompts and common lane contract are in Git
history. They described the initial implementation wave and are not current
execution instructions. Use the live code, `tools/ci/proof-authority.toml`,
`Justfile`, and [remaining status](../CURRENT-RESIDUAL-2026-09-26.md) to determine
the required gate. Current source identity and execution come from the selected
run; historical execution bodies are recoverable through the plan archive.

- [Residual work](../FINAL-RESIDUAL-EXECUTION-PLAN.md): remaining evidence
  and source boundaries.
- [P10 state custody](P10-state-migration.md): current-format operations and
  typed legacy refusal.
- [P11 exact pair](P11-cross-repo-cutover.md): producer protocol and four
  separate release/operational receipts.
- [P12A infrastructure](P12A-final-proof-infrastructure.md): proof tooling
  and exact-source owner receipt.
- [P12 final qualification](P12-final-qualification.md): final-source
  dependency graph and aggregate receipt.

An implementation ticket, prompt, owner test, or historical handoff cannot
promote a staged proof. Record code completion, owner tests, exact-pair
release, deployment, activation, and rollback separately.
